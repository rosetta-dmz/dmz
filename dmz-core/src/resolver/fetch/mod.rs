//! Dependency artifact fetching, parallel queue management, and store cache population.

pub mod cache;
pub mod downloader;
pub mod ecosystems;
pub mod pypi;
pub mod stream;

use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::{Ecosystem, ManifestScanner};
pub use pypi::select_pypi_artifact;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

pub struct DependencyFetcher;

impl DependencyFetcher {
    /// Fetches locked dependencies from dmz.lock, downloads real package artifacts, verifies hashes, and populates the local store cache.
    pub fn fetch_dependencies(lockfile_path: &Path, store_path: &Path) -> Result<usize, String> {
        info!(path = ?lockfile_path, "Reading lockfile to fetch real dependency artifacts into store...");

        if !lockfile_path.exists() {
            return Err(format!(
                "Lockfile not found at {:?}. Run `dmz resolve` first.",
                lockfile_path
            ));
        }

        let lockfile_content = std::fs::read_to_string(lockfile_path)
            .map_err(|e| format!("Failed to read lockfile: {}", e))?;

        let mut lockfile: DmzLockfile = serde_json::from_str(&lockfile_content)
            .or_else(|_| toml::from_str(&lockfile_content))
            .map_err(|e| format!("Failed to parse lockfile format: {}", e))?;

        std::fs::create_dir_all(store_path).map_err(|e| {
            format!(
                "Failed to create store directory at {:?}: {}",
                store_path, e
            )
        })?;

        let mut fetched_count = 0;

        let crates_api_base = std::env::var("DMZ_REGISTRY_CRATES_API")
            .unwrap_or_else(|_| "https://crates.io/api/v1/crates".to_string());
        let crates_dl_base = std::env::var("DMZ_REGISTRY_CRATES_DL")
            .unwrap_or_else(|_| "https://static.crates.io/crates".to_string());
        let npm_base = std::env::var("DMZ_REGISTRY_NPM")
            .unwrap_or_else(|_| "https://registry.npmjs.org".to_string());
        let pypi_base = std::env::var("DMZ_REGISTRY_PYPI")
            .unwrap_or_else(|_| "https://pypi.org/pypi".to_string());
        let golang_proxy_base = std::env::var("DMZ_REGISTRY_GOLANG")
            .unwrap_or_else(|_| "https://proxy.golang.org".to_string());

        let target_platform = lockfile.target_platform.clone();

        // Defensive sanitation of lockfile packages against stale/dirty names (e.g. from prior PEP 508 bugs)
        let mut sanitized_packages = std::collections::BTreeMap::new();
        let mut mutated_lockfile = false;

        for (key, mut pkg) in std::mem::take(&mut lockfile.packages) {
            if pkg.ecosystem == Ecosystem::PyPI {
                if let Some((clean_name, extracted_ver)) =
                    ManifestScanner::parse_pep508_requirement(&pkg.name)
                {
                    if clean_name != pkg.name {
                        pkg.name = clean_name;
                        if (pkg.version == "*" || pkg.version.is_empty()) && extracted_ver != "*" {
                            pkg.version = extracted_ver;
                        }
                        mutated_lockfile = true;
                    }
                }
            }
            let effective_key = format!("{}@{}", pkg.name, pkg.version);
            if effective_key != key {
                mutated_lockfile = true;
            }
            sanitized_packages.insert(effective_key, pkg);
        }
        lockfile.packages = sanitized_packages;
        if mutated_lockfile {
            let _ = lockfile.save(lockfile_path);
        }

        let mut current_wave: Vec<(String, LockedPackage)> = lockfile
            .packages
            .clone()
            .into_iter()
            .filter(|(_k, pkg)| !pkg.version.starts_with("path:") && !pkg.version.contains("path"))
            .collect();

        let mut seen_pypi: std::collections::HashSet<String> = lockfile
            .packages
            .values()
            .filter(|p| p.ecosystem == Ecosystem::PyPI)
            .map(|p| ManifestScanner::normalize_pypi_name(&p.name))
            .collect();

        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 16);

        for _depth in 0..4 {
            if current_wave.is_empty() {
                break;
            }
            let queue = Arc::new(Mutex::new(VecDeque::from(std::mem::take(
                &mut current_wave,
            ))));
            let (tx, rx) = std::sync::mpsc::channel();

            std::thread::scope(|s| {
                for _ in 0..num_workers {
                    let queue = Arc::clone(&queue);
                    let tx = tx.clone();
                    let crates_api_ref = &crates_api_base;
                    let crates_dl_ref = &crates_dl_base;
                    let npm_ref = &npm_base;
                    let pypi_ref = &pypi_base;
                    let golang_ref = &golang_proxy_base;
                    let target_ref = target_platform.as_deref();

                    s.spawn(move || {
                        let client = match reqwest::blocking::Client::builder()
                            .user_agent(concat!(
                                "dmz-airgap-tool/",
                                env!("CARGO_PKG_VERSION"),
                                " (air-gap workspace manager)"
                            ))
                            .connect_timeout(std::time::Duration::from_secs(30))
                            .timeout(std::time::Duration::from_secs(600))
                            .tcp_keepalive(std::time::Duration::from_secs(15))
                            .build()
                        {
                            Ok(c) => c,
                            Err(e) => {
                                let _ =
                                    tx.send(Err(format!("Failed to create HTTP client: {}", e)));
                                return;
                            }
                        };

                        loop {
                            let item = {
                                let mut lock = queue
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                lock.pop_front()
                            };

                            let Some((key, pkg)) = item else {
                                break;
                            };

                            let res = downloader::fetch_single_package(
                                &client,
                                store_path,
                                crates_api_ref,
                                crates_dl_ref,
                                npm_ref,
                                pypi_ref,
                                golang_ref,
                                target_ref,
                                key,
                                pkg,
                            );

                            if tx.send(res).is_err() {
                                break;
                            }
                        }
                    });
                }
            });
            drop(tx);

            let mut next_wave = Vec::new();
            for res in rx {
                if let Some((old_key, updated_pkg)) = res? {
                    fetched_count += 1;
                    let new_key = format!("{}@{}", updated_pkg.name, updated_pkg.version);
                    if old_key != new_key {
                        lockfile.packages.remove(&old_key);
                    }
                    if updated_pkg.ecosystem == Ecosystem::PyPI {
                        for dep_str in &updated_pkg.dependencies {
                            if let Some((dep_name, dep_ver)) =
                                ManifestScanner::parse_pep508_requirement(dep_str)
                            {
                                let norm = ManifestScanner::normalize_pypi_name(&dep_name);
                                if seen_pypi.insert(norm) {
                                    let dummy_hash = DmzLockfile::hash_bytes(
                                        format!("{}:{}:{:?}", dep_name, dep_ver, Ecosystem::PyPI)
                                            .as_bytes(),
                                    );
                                    let dep_key = format!("{}@{}", dep_name, dep_ver);
                                    next_wave.push((
                                        dep_key,
                                        LockedPackage {
                                            name: dep_name,
                                            version: dep_ver,
                                            ecosystem: Ecosystem::PyPI,
                                            sha256: dummy_hash,
                                            source_url: None,
                                            dependencies: Vec::new(),
                                            member_id: updated_pkg.member_id.clone(),
                                        },
                                    ));
                                }
                            }
                        }
                    }
                    lockfile.packages.insert(new_key, updated_pkg);
                }
            }
            current_wave = next_wave;
        }

        if fetched_count > 0 {
            lockfile.closure_signature = DmzLockfile::compute_closure_hash(&lockfile.packages);
            if let Err(e) = lockfile.save(lockfile_path) {
                tracing::warn!("Failed to update lockfile with authentic hashes: {}", e);
            } else {
                info!(
                    path = ?lockfile_path,
                    closure_sig = %lockfile.closure_signature,
                    "Saved authentic package hashes and recomputed closure signature to lockfile"
                );
            }
        }

        info!(
            lib_fetched = fetched_count,
            "Store artifact population completed successfully"
        );
        Ok(fetched_count)
    }

    /// Evaluates Python wheel ABI tags and selects the best matching artifact for target platform.
    #[inline]
    pub fn select_pypi_artifact(
        urls: &[serde_json::Value],
        target_platform: Option<&str>,
    ) -> Option<String> {
        pypi::select_pypi_artifact(urls, target_platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_fetch_dependencies_pypi_transitive_pep508() {
        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        let pkg = LockedPackage {
            name: "charset_normalizer".to_string(),
            version: "2".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: "dummy".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        let lockfile = DmzLockfile::new(vec![pkg]);
        lockfile.save(&lockfile_path).unwrap();

        let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);
        assert!(count.is_ok(), "Failed to fetch: {:?}", count.err());
        assert_eq!(count.unwrap(), 1);

        // Check that artifact was downloaded and cached with canonical version
        let cached_artifact = store_path
            .join("charset_normalizer")
            .join("charset_normalizer-2.0.0.archive");
        assert!(
            cached_artifact.exists(),
            "Expected cached artifact at {:?}",
            cached_artifact
        );
    }

    #[test]
    fn test_fetch_dependencies_pypi_requests_transitive_closure() {
        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        let pkg = LockedPackage {
            name: "requests".to_string(),
            version: "2.31.0".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: "dummy".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        let lockfile = DmzLockfile::new(vec![pkg]);
        lockfile.save(&lockfile_path).unwrap();

        let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);
        assert!(count.is_ok(), "Failed to fetch: {:?}", count.err());
        assert!(count.unwrap() >= 5);

        // Verify charset-normalizer was fetched cleanly into store
        let cn_hyphen = store_path.join("charset-normalizer");
        let cn_underscore = store_path.join("charset_normalizer");
        assert!(
            cn_hyphen.exists() || cn_underscore.exists(),
            "Expected charset-normalizer or charset_normalizer in store"
        );
    }

    #[test]
    fn test_fetch_dependencies_pypi_legacy_dirty_lockfile_healed() {
        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        // Simulate an existing lockfile containing a dirty PEP 508 package name
        let pkg = LockedPackage {
            name: "charset_normalizer<4,".to_string(),
            version: "2".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: "dummy".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        let lockfile = DmzLockfile::new(vec![pkg]);
        lockfile.save(&lockfile_path).unwrap();

        let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);
        assert!(count.is_ok(), "Failed to fetch: {:?}", count.err());
        assert_eq!(count.unwrap(), 1);

        // Verify that lockfile on disk was automatically healed and saved
        let reloaded = DmzLockfile::load_and_verify(&lockfile_path).unwrap();
        assert!(
            !reloaded.packages.contains_key("charset_normalizer<4,@2"),
            "Dirty key should have been removed from lockfile"
        );
        let has_clean = reloaded
            .packages
            .values()
            .any(|p| p.name == "charset_normalizer");
        assert!(has_clean, "Lockfile should contain sanitized package name");

        // Verify artifact was cached
        let cn_hyphen = store_path.join("charset-normalizer");
        let cn_underscore = store_path.join("charset_normalizer");
        assert!(
            cn_hyphen.exists() || cn_underscore.exists(),
            "Expected cached artifact in store"
        );
    }

    #[test]
    fn test_fetch_dependencies_pypi_cuda_bindings_platform_wheel_fallback() {
        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        // cuda-bindings distributes ONLY platform wheels (no sdist, no macOS wheels)
        let pkg = LockedPackage {
            name: "cuda-bindings".to_string(),
            version: "13.5.0".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: "dummy".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        let lockfile = DmzLockfile::new(vec![pkg]);
        lockfile.save(&lockfile_path).unwrap();

        let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);
        assert!(
            count.is_ok(),
            "Failed to fetch cuda-bindings: {:?}",
            count.err()
        );
        assert!(count.unwrap() >= 1);

        let cuda_dir = store_path.join("cuda-bindings");
        assert!(
            cuda_dir.exists(),
            "Expected cuda-bindings directory in store"
        );

        // Ensure artifact was downloaded and cached as a valid archive
        let entries: Vec<_> = std::fs::read_dir(&cuda_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("archive"))
            .collect();
        assert!(
            !entries.is_empty(),
            "Expected cached .archive file for cuda-bindings"
        );
    }

    #[test]
    fn test_select_pypi_artifact_scoring_priorities() {
        let urls = vec![
            serde_json::json!({
                "filename": "cuda_bindings-13.5.0-cp311-cp311-win_amd64.whl",
                "url": "https://pypi.org/win_amd64"
            }),
            serde_json::json!({
                "filename": "cuda_bindings-13.5.0-cp311-cp311-manylinux_2_28_x86_64.whl",
                "url": "https://pypi.org/manylinux_x86_64"
            }),
            serde_json::json!({
                "filename": "cuda_bindings-13.5.0-cp311-cp311-manylinux_2_28_aarch64.whl",
                "url": "https://pypi.org/manylinux_aarch64"
            }),
        ];

        // On a macOS host with target None (defaults to host, which on Apple Silicon is aarch64):
        // Linux aarch64 wheel gets 40 + 3 = 43, win gets 30+3 = 33
        let selected = DependencyFetcher::select_pypi_artifact(&urls, None);
        assert!(selected.is_some());
        let sel_url = selected.unwrap();
        assert!(sel_url.contains("manylinux"));

        // When explicit target is linux-x86_64:
        let selected_linux = DependencyFetcher::select_pypi_artifact(&urls, Some("linux-x86_64"));
        assert_eq!(
            selected_linux.as_deref(),
            Some("https://pypi.org/manylinux_x86_64")
        );

        // When explicit target is windows-x86_64:
        let selected_win = DependencyFetcher::select_pypi_artifact(&urls, Some("windows-x86_64"));
        assert_eq!(selected_win.as_deref(), Some("https://pypi.org/win_amd64"));
    }

    #[test]
    fn test_fetch_dependencies_cached_artifact_fast_path() {
        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        let pkg = LockedPackage {
            name: "cached-pkg".to_string(),
            version: "1.0.0".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: "precomputed_hash_12345".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        let lockfile = DmzLockfile::new(vec![pkg.clone()]);
        lockfile.save(&lockfile_path).unwrap();

        // Pre-populate store with matching artifact and metadata
        let pkg_dir = store_path.join("cached-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let artifact_path = pkg_dir.join("cached-pkg-1.0.0.archive");
        std::fs::write(&artifact_path, b"dummy cached content").unwrap();
        let meta_path = pkg_dir.join("metadata.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&pkg).unwrap()).unwrap();

        let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);
        assert!(
            count.is_ok(),
            "Failed to reuse cached artifact: {:?}",
            count.err()
        );
        assert_eq!(count.unwrap(), 1);
    }
}
