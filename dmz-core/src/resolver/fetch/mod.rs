//! Dependency artifact fetching, parallel queue management, and store cache population.

pub mod cache;
pub mod downloader;
pub mod ecosystems;
pub mod pypi;
pub mod stream;

use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::{Ecosystem, ManifestScanner};
use crate::resolver::ResolverError;
pub use pypi::select_pypi_artifact;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

pub struct DependencyFetcher;

impl DependencyFetcher {
    /// Fetches locked dependencies from dmz.lock, downloads real package artifacts, verifies hashes, and populates the local store cache.
    pub fn fetch_dependencies(
        lockfile_path: &Path,
        store_path: &Path,
    ) -> Result<usize, ResolverError> {
        let npm_base = std::env::var("DMZ_REGISTRY_NPM")
            .unwrap_or_else(|_| "https://registry.npmjs.org".to_string());
        Self::fetch_dependencies_with_npm_base(lockfile_path, store_path, &npm_base)
    }

    pub(crate) fn fetch_dependencies_with_npm_base(
        lockfile_path: &Path,
        store_path: &Path,
        npm_base: &str,
    ) -> Result<usize, ResolverError> {
        info!(path = ?lockfile_path, "Reading lockfile to fetch real dependency artifacts into store...");

        if !lockfile_path.exists() {
            return Err(ResolverError::Io(format!(
                "Lockfile not found at {:?}. Run `dmz resolve` first.",
                lockfile_path
            )));
        }

        let lockfile_content = std::fs::read_to_string(lockfile_path)
            .map_err(|e| ResolverError::Io(format!("Failed to read lockfile: {}", e)))?;

        let mut lockfile: DmzLockfile = serde_json::from_str(&lockfile_content)
            .or_else(|_| toml::from_str(&lockfile_content))
            .map_err(|e| ResolverError::Parse(format!("Failed to parse lockfile format: {}", e)))?;

        std::fs::create_dir_all(store_path).map_err(|e| {
            ResolverError::Io(format!(
                "Failed to create store directory at {:?}: {}",
                store_path, e
            ))
        })?;

        let mut fetched_count = 0;

        let crates_api_base = std::env::var("DMZ_REGISTRY_CRATES_API")
            .unwrap_or_else(|_| "https://crates.io/api/v1/crates".to_string());
        let crates_dl_base = std::env::var("DMZ_REGISTRY_CRATES_DL")
            .unwrap_or_else(|_| "https://static.crates.io/crates".to_string());
        let npm_base = npm_base.to_string();
        let pypi_base = std::env::var("DMZ_REGISTRY_PYPI")
            .unwrap_or_else(|_| "https://pypi.org/pypi".to_string());
        let golang_proxy_base = std::env::var("DMZ_REGISTRY_GOLANG")
            .unwrap_or_else(|_| "https://proxy.golang.org".to_string());

        let target_platform = lockfile.target_platform.clone();

        // Defensive sanitation of lockfile packages against stale/dirty names (e.g. from prior PEP 508 bugs)
        let mut sanitized_packages = std::collections::BTreeMap::new();
        let mut mutated_lockfile = false;

        for (key, mut pkg) in std::mem::take(&mut lockfile.packages) {
            let mut package_dirty = false;
            if pkg.ecosystem == Ecosystem::PyPI {
                if let Some((clean_name, extracted_ver)) =
                    ManifestScanner::parse_pep508_requirement(&pkg.name)
                {
                    if clean_name != pkg.name {
                        pkg.name = clean_name;
                        if (pkg.version == "*" || pkg.version.is_empty()) && extracted_ver != "*" {
                            pkg.version = extracted_ver;
                        }
                        package_dirty = true;
                        mutated_lockfile = true;
                    }
                }
            }
            if package_dirty {
                DmzLockfile::insert_package(&mut sanitized_packages, pkg);
            } else {
                sanitized_packages.insert(key, pkg);
            }
        }
        lockfile.packages = sanitized_packages;
        if mutated_lockfile {
            lockfile.closure_signature = DmzLockfile::compute_closure_hash(&lockfile.packages);
            lockfile.save(lockfile_path)?;
        }

        let mut current_wave: Vec<(String, LockedPackage)> = lockfile
            .packages
            .clone()
            .into_iter()
            .filter(|(_k, pkg)| !pkg.is_local_path())
            .collect();

        let mut seen_pypi: std::collections::HashSet<String> = lockfile
            .packages
            .values()
            .filter(|p| p.ecosystem == Ecosystem::PyPI)
            .map(|p| ManifestScanner::normalize_pypi_name(&p.name))
            .collect();
        let mut seen_npm: std::collections::HashSet<(Option<String>, String)> = lockfile
            .packages
            .values()
            .filter(|p| p.ecosystem == Ecosystem::Npm)
            .map(|p| (p.member_id.clone(), p.name.clone()))
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
                                let _ = tx.send(Err(ResolverError::Network(format!(
                                    "Failed to create HTTP client: {}",
                                    e
                                ))));
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
                    lockfile.packages.remove(&old_key);
                    if updated_pkg.ecosystem == Ecosystem::PyPI {
                        for dep_str in &updated_pkg.dependencies {
                            if let Some((dep_name, dep_ver)) =
                                ManifestScanner::parse_pep508_requirement_for_target(
                                    dep_str,
                                    target_platform.as_deref(),
                                )
                            {
                                let norm = ManifestScanner::normalize_pypi_name(&dep_name);
                                if seen_pypi.insert(norm) {
                                    let dummy_hash = DmzLockfile::hash_bytes(
                                        format!("{}:{}:{:?}", dep_name, dep_ver, Ecosystem::PyPI)
                                            .as_bytes(),
                                    );
                                    let dep_key = format!("{}:{}", dep_name, dep_ver);
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
                    } else if updated_pkg.ecosystem == Ecosystem::Npm {
                        for dep_str in &updated_pkg.dependencies {
                            let trimmed = dep_str.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            let (dep_name, dep_ver) = match trimmed.rsplit_once('@') {
                                Some((name, ver)) if !name.is_empty() => {
                                    (name.trim().to_string(), ver.trim().to_string())
                                }
                                _ => (trimmed.to_string(), "*".to_string()),
                            };
                            if dep_name.is_empty()
                                || dep_ver.starts_with("file:")
                                || dep_ver.starts_with("workspace:")
                                || dep_ver.starts_with("link:")
                                || dep_ver.starts_with("git+")
                                || dep_ver.contains("://")
                                || !ManifestScanner::is_compatible_npm_platform_pkg_for_target(
                                    &dep_name,
                                    target_platform.as_deref(),
                                )
                            {
                                continue;
                            }
                            if seen_npm.insert((updated_pkg.member_id.clone(), dep_name.clone())) {
                                let dummy_hash = DmzLockfile::hash_bytes(
                                    format!("{}:{}:{:?}", dep_name, dep_ver, Ecosystem::Npm)
                                        .as_bytes(),
                                );
                                let dep_key = format!("{}:{}", dep_name, dep_ver);
                                next_wave.push((
                                    dep_key,
                                    LockedPackage {
                                        name: dep_name,
                                        version: dep_ver,
                                        ecosystem: Ecosystem::Npm,
                                        sha256: dummy_hash,
                                        source_url: None,
                                        dependencies: Vec::new(),
                                        member_id: updated_pkg.member_id.clone(),
                                    },
                                ));
                            }
                        }
                    }
                    DmzLockfile::insert_package(&mut lockfile.packages, updated_pkg);
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
    #[ignore = "requires live internet access to pypi.org"]
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
    #[ignore = "requires live internet access to pypi.org"]
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
    #[ignore = "requires live internet access to pypi.org"]
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
    #[ignore = "requires live internet access to pypi.org"]
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

        // On host default (target None):
        // On Windows hosts, the native win_amd64 wheel scores 100+3 = 103.
        // On Linux/macOS hosts, manylinux is selected (native on Linux, or cross-platform fallback on macOS).
        let selected = DependencyFetcher::select_pypi_artifact(&urls, None);
        assert!(selected.is_some());
        let sel_url = selected.unwrap();
        if cfg!(target_os = "windows") {
            assert!(sel_url.contains("win_amd64"));
        } else {
            assert!(sel_url.contains("manylinux"));
        }

        // When target is macOS (no macOS wheels in list, fallback to cross-platform matching arch):
        let selected_mac_arm = DependencyFetcher::select_pypi_artifact(&urls, Some("darwin-arm64"));
        assert_eq!(
            selected_mac_arm.as_deref(),
            Some("https://pypi.org/manylinux_aarch64")
        );

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
        use sha2::{Digest, Sha256};

        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        let payload = b"dummy cached content";
        let expected_sha256 =
            crate::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(payload));

        let pkg = LockedPackage {
            name: "cached-pkg".to_string(),
            version: "1.0.0".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: expected_sha256,
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
        std::fs::write(&artifact_path, payload).unwrap();
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

    #[test]
    fn test_fetch_dependencies_hermetic_localhost_registries() {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };

        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();

        struct EnvReset;
        impl Drop for EnvReset {
            fn drop(&mut self) {
                for var in &[
                    "DMZ_REGISTRY_PYPI",
                    "DMZ_REGISTRY_CRATES_API",
                    "DMZ_REGISTRY_CRATES_DL",
                    "DMZ_REGISTRY_NPM",
                    "DMZ_REGISTRY_GOLANG",
                ] {
                    std::env::remove_var(var);
                }
            }
        }
        let _reset = EnvReset;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}", addr);

        let wheel_bytes = b"PK\x03\x04mock-wheel-bytes".to_vec();
        let dep_wheel_bytes = b"PK\x03\x04dep-wheel-bytes".to_vec();
        let crate_bytes = b"\x1f\x8b\x08\x00mock-crate-bytes".to_vec();
        let npm_bytes = b"\x1f\x8b\x08\x00mock-npm-tgz-bytes".to_vec();
        let go_zip_bytes = b"PK\x03\x04mock-go-mod-zip".to_vec();

        let expected_hashes = vec![
            crate::archive::format_sha256_hex(&Sha256::digest(&wheel_bytes)),
            crate::archive::format_sha256_hex(&Sha256::digest(&dep_wheel_bytes)),
            crate::archive::format_sha256_hex(&Sha256::digest(&crate_bytes)),
            crate::archive::format_sha256_hex(&Sha256::digest(&npm_bytes)),
            crate::archive::format_sha256_hex(&Sha256::digest(&go_zip_bytes)),
        ];

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_srv = Arc::clone(&shutdown);
        let base_srv = base_url.clone();

        let wheel_srv = wheel_bytes.clone();
        let dep_wheel_srv = dep_wheel_bytes.clone();
        let crate_srv = crate_bytes.clone();
        let npm_srv = npm_bytes.clone();
        let go_srv = go_zip_bytes.clone();

        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            while !shutdown_srv.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 4096];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let path = req
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("/");

                        let (status, ctype, body): (&str, &str, Vec<u8>) = match path {
                            "/pypi/alpha/1.0.0/json" | "/pypi/alpha/json" => {
                                let json = serde_json::json!({
                                    "info": {
                                        "version": "1.0.0",
                                        "requires_dist": ["beta==0.5.0"]
                                    },
                                    "urls": [{
                                        "filename": "alpha-1.0.0-py3-none-any.whl",
                                        "url": format!("{}/files/alpha-1.0.0.whl", base_srv)
                                    }]
                                });
                                ("200 OK", "application/json", json.to_string().into_bytes())
                            }
                            "/pypi/beta/0.5.0/json" | "/pypi/beta/json" => {
                                let json = serde_json::json!({
                                    "info": {
                                        "version": "0.5.0",
                                        "requires_dist": []
                                    },
                                    "urls": [{
                                        "filename": "beta-0.5.0-py3-none-any.whl",
                                        "url": format!("{}/files/beta-0.5.0.whl", base_srv)
                                    }]
                                });
                                ("200 OK", "application/json", json.to_string().into_bytes())
                            }
                            "/files/alpha-1.0.0.whl" => {
                                ("200 OK", "application/octet-stream", wheel_srv.clone())
                            }
                            "/files/beta-0.5.0.whl" => {
                                ("200 OK", "application/octet-stream", dep_wheel_srv.clone())
                            }
                            "/crates/mini_crate/mini_crate-0.2.0.crate" => {
                                ("200 OK", "application/octet-stream", crate_srv.clone())
                            }
                            "/npm/tiny_pkg/-/tiny_pkg-1.2.3.tgz" => {
                                ("200 OK", "application/octet-stream", npm_srv.clone())
                            }
                            "/golang/example.com/mod/@v/v0.3.0.zip" => {
                                ("200 OK", "application/octet-stream", go_srv.clone())
                            }
                            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
                        };

                        let header = format!(
                            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            status,
                            ctype,
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(&body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        std::env::set_var("DMZ_REGISTRY_PYPI", format!("{}/pypi", base_url));
        std::env::set_var(
            "DMZ_REGISTRY_CRATES_API",
            format!("{}/crates_api", base_url),
        );
        std::env::set_var("DMZ_REGISTRY_CRATES_DL", format!("{}/crates", base_url));
        std::env::set_var("DMZ_REGISTRY_NPM", format!("{}/npm", base_url));
        std::env::set_var("DMZ_REGISTRY_GOLANG", format!("{}/golang", base_url));

        let temp = tempdir().unwrap();
        let lockfile_path = temp.path().join("dmz.lock");
        let store_path = temp.path().join("store");

        let packages = vec![
            LockedPackage {
                name: "alpha".to_string(),
                version: "1.0.0".to_string(),
                ecosystem: Ecosystem::PyPI,
                sha256: "pending".to_string(),
                source_url: None,
                dependencies: Vec::new(),
                member_id: None,
            },
            LockedPackage {
                name: "mini_crate".to_string(),
                version: "0.2.0".to_string(),
                ecosystem: Ecosystem::Cargo,
                sha256: "pending".to_string(),
                source_url: None,
                dependencies: Vec::new(),
                member_id: None,
            },
            LockedPackage {
                name: "tiny_pkg".to_string(),
                version: "1.2.3".to_string(),
                ecosystem: Ecosystem::Npm,
                sha256: "pending".to_string(),
                source_url: None,
                dependencies: Vec::new(),
                member_id: None,
            },
            LockedPackage {
                name: "example.com/mod".to_string(),
                version: "v0.3.0".to_string(),
                ecosystem: Ecosystem::Go,
                sha256: "pending".to_string(),
                source_url: None,
                dependencies: Vec::new(),
                member_id: None,
            },
        ];

        DmzLockfile::new(packages).save(&lockfile_path).unwrap();
        let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);

        shutdown.store(true, Ordering::Relaxed);
        let _ = handle.join();

        assert!(count.is_ok(), "Offline fetch failed: {:?}", count.err());
        assert_eq!(count.unwrap(), 5);

        let reloaded = DmzLockfile::load_and_verify(&lockfile_path).unwrap();
        assert_eq!(reloaded.packages.len(), 5);
        for h in &expected_hashes {
            assert!(
                reloaded.packages.values().any(|p| &p.sha256 == h),
                "Missing expected real SHA-256 {} in lockfile",
                h
            );
        }
    }

    #[test]
    fn test_fetch_dependencies_preserves_integrity_with_local_paths_and_sanitization() {
        let temp = tempdir().unwrap();
        let store_path = temp.path().join("store");

        // 1. Clean local-path-only lockfile preserves canonical key and signature
        let local_lock_path = temp.path().join("local.lock");
        let local_pkg = LockedPackage {
            name: "standalone-app".to_string(),
            version: "path:Cargo.toml".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: DmzLockfile::hash_bytes(b"standalone-app"),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        DmzLockfile::new(vec![local_pkg])
            .save(&local_lock_path)
            .unwrap();

        let count = DependencyFetcher::fetch_dependencies(&local_lock_path, &store_path).unwrap();
        assert_eq!(count, 0);
        let reloaded_local = DmzLockfile::load_and_verify(&local_lock_path).unwrap();
        assert!(reloaded_local
            .packages
            .contains_key("standalone-app:path:Cargo.toml"));

        // 2. Dirty PEP 508 local-path lockfile (mutated_lockfile = true, fetched_count = 0)
        //    recomputes closure_signature before saving
        let dirty_lock_path = temp.path().join("dirty_local.lock");
        let dirty_pkg = LockedPackage {
            name: "dirty_pkg>=1.0".to_string(),
            version: "path:local_pkg".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: DmzLockfile::hash_bytes(b"dirty_pkg"),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };
        DmzLockfile::new(vec![dirty_pkg])
            .save(&dirty_lock_path)
            .unwrap();

        let dirty_count =
            DependencyFetcher::fetch_dependencies(&dirty_lock_path, &store_path).unwrap();
        assert_eq!(dirty_count, 0);
        let reloaded_dirty = DmzLockfile::load_and_verify(&dirty_lock_path).unwrap();
        assert!(reloaded_dirty
            .packages
            .contains_key("dirty_pkg:path:local_pkg"));
        assert!(!reloaded_dirty
            .packages
            .contains_key("dirty_pkg>=1.0:path:local_pkg"));
    }

    #[test]
    fn test_fetch_dependencies_npm_transitive_semver_and_cache_preserves_member_id() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}", addr);

        let vite_tgz = b"mock-vite-tarball-bytes".to_vec();
        let esbuild_tgz = b"mock-esbuild-tarball-bytes".to_vec();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = Arc::clone(&stop);
        let base_for_srv = base_url.clone();
        let server_handle = std::thread::spawn(move || {
            listener.set_nonblocking(true).ok();
            while !stop_srv.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 4096];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let path = req
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let (status, ctype, body): (&str, &str, Vec<u8>) = match path {
                            "/vite" => {
                                let json = format!(
                                    r#"{{
                                        "dist-tags": {{"latest": "6.0.11"}},
                                        "versions": {{
                                            "5.4.0": {{
                                                "dist": {{"tarball": "{base}/vite/-/vite-5.4.0.tgz"}}
                                            }},
                                            "6.0.11": {{
                                                "dependencies": {{"esbuild": "^0.24.0"}},
                                                "optionalDependencies": {{"fsevents-win32-only-nonexistent-win32-x64": "1.0.0"}},
                                                "dist": {{"tarball": "{base}/vite/-/vite-6.0.11.tgz"}}
                                            }},
                                            "7.0.0-alpha.1": {{
                                                "dist": {{"tarball": "{base}/vite/-/vite-7.0.0-alpha.1.tgz"}}
                                            }}
                                        }}
                                    }}"#,
                                    base = base_for_srv
                                );
                                ("200 OK", "application/json", json.into_bytes())
                            }
                            "/esbuild" => {
                                let json = format!(
                                    r#"{{
                                        "dist-tags": {{"latest": "0.25.0"}},
                                        "versions": {{
                                            "0.24.2": {{
                                                "dist": {{"tarball": "{base}/esbuild/-/esbuild-0.24.2.tgz"}}
                                            }},
                                            "0.25.0": {{
                                                "dist": {{"tarball": "{base}/esbuild/-/esbuild-0.25.0.tgz"}}
                                            }}
                                        }}
                                    }}"#,
                                    base = base_for_srv
                                );
                                ("200 OK", "application/json", json.into_bytes())
                            }
                            "/vite/-/vite-6.0.11.tgz" => {
                                ("200 OK", "application/octet-stream", vite_tgz.clone())
                            }
                            "/esbuild/-/esbuild-0.24.2.tgz" => {
                                ("200 OK", "application/octet-stream", esbuild_tgz.clone())
                            }
                            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
                        };
                        let header = format!(
                            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            status,
                            ctype,
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(&body);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        let temp = tempdir().unwrap();
        let store_path = temp.path().join("store");

        // Run 1: Member A fetches vite@^6.0.0 -> resolves 6.0.11 and transitively fetches esbuild@^0.24.0 -> 0.24.2 (not 0.25.0)
        let lock_a = temp.path().join("dmz_a.lock");
        let pkg_a = LockedPackage {
            name: "vite".to_string(),
            version: "^6.0.0".to_string(),
            ecosystem: Ecosystem::Npm,
            sha256: DmzLockfile::hash_bytes(b"vite:^6.0.0:Npm"),
            source_url: None,
            dependencies: Vec::new(),
            member_id: Some("member-a".to_string()),
        };
        DmzLockfile::new(vec![pkg_a]).save(&lock_a).unwrap();

        let count_a =
            DependencyFetcher::fetch_dependencies_with_npm_base(&lock_a, &store_path, &base_url)
                .expect("fetch run A");
        assert_eq!(count_a, 2, "Should fetch vite and transitive esbuild");

        let reloaded_a = DmzLockfile::load_and_verify(&lock_a).unwrap();
        let vite_a = reloaded_a
            .packages
            .values()
            .find(|p| p.name == "vite")
            .expect("vite in lockfile A");
        assert_eq!(vite_a.version, "6.0.11");
        assert_eq!(vite_a.member_id.as_deref(), Some("member-a"));

        let esbuild_a = reloaded_a
            .packages
            .values()
            .find(|p| p.name == "esbuild")
            .expect("transitive esbuild in lockfile A");
        assert_eq!(
            esbuild_a.version, "0.24.2",
            "^0.24.0 must match 0.24.2, not 0.25.0"
        );
        assert_eq!(esbuild_a.member_id.as_deref(), Some("member-a"));

        // Run 2: Member B resolves exact vite@6.0.11 from store cache and must keep member_id = "member-b"
        let lock_b = temp.path().join("dmz_b.lock");
        let pkg_b = LockedPackage {
            name: "vite".to_string(),
            version: "6.0.11".to_string(),
            ecosystem: Ecosystem::Npm,
            sha256: vite_a.sha256.clone(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: Some("member-b".to_string()),
        };
        DmzLockfile::new(vec![pkg_b]).save(&lock_b).unwrap();

        let count_b =
            DependencyFetcher::fetch_dependencies_with_npm_base(&lock_b, &store_path, &base_url)
                .expect("fetch run B");
        assert_eq!(count_b, 2, "Cache hit on vite should still expand esbuild");

        let reloaded_b = DmzLockfile::load_and_verify(&lock_b).unwrap();
        let vite_b = reloaded_b
            .packages
            .values()
            .find(|p| p.name == "vite")
            .expect("vite in lockfile B");
        assert_eq!(
            vite_b.member_id.as_deref(),
            Some("member-b"),
            "Store cache hit must preserve caller's member_id instead of cached member-a"
        );
        let esbuild_b = reloaded_b
            .packages
            .values()
            .find(|p| p.name == "esbuild")
            .expect("esbuild in lockfile B");
        assert_eq!(esbuild_b.member_id.as_deref(), Some("member-b"));

        stop.store(true, Ordering::Relaxed);
        let _ = server_handle.join();
    }
}
