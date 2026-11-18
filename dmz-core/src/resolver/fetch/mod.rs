//! Dependency artifact fetching, parallel queue management, and store cache population.

pub mod cache;
pub mod downloader;
pub mod ecosystems;
pub mod pypi;
pub mod stream;

use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::{Ecosystem, ManifestScanner};
use crate::resolver::ResolverError;
pub use pypi::{select_pypi_artifact, select_pypi_artifact_for_python};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

#[derive(Debug, Clone)]
pub(crate) struct RegistryBases {
    pub(crate) crates_api: String,
    pub(crate) crates_dl: String,
    pub(crate) npm: String,
    pub(crate) pypi: String,
    pub(crate) golang: String,
}

impl RegistryBases {
    fn from_env_with_npm(npm_base: String) -> Self {
        Self {
            crates_api: std::env::var("DMZ_REGISTRY_CRATES_API")
                .unwrap_or_else(|_| "https://crates.io/api/v1/crates".to_string()),
            crates_dl: std::env::var("DMZ_REGISTRY_CRATES_DL")
                .unwrap_or_else(|_| "https://static.crates.io/crates".to_string()),
            npm: npm_base,
            pypi: std::env::var("DMZ_REGISTRY_PYPI")
                .unwrap_or_else(|_| "https://pypi.org/pypi".to_string()),
            golang: std::env::var("DMZ_REGISTRY_GOLANG")
                .unwrap_or_else(|_| "https://proxy.golang.org".to_string()),
        }
    }
}

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
        let bases = RegistryBases::from_env_with_npm(npm_base.to_string());
        Self::fetch_dependencies_with_bases(lockfile_path, store_path, &bases)
    }

    pub(crate) fn fetch_dependencies_with_bases(
        lockfile_path: &Path,
        store_path: &Path,
        bases: &RegistryBases,
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

        let crates_api_base = bases.crates_api.clone();
        let crates_dl_base = bases.crates_dl.clone();
        let npm_base = bases.npm.clone();
        let pypi_base = bases.pypi.clone();
        let golang_proxy_base = bases.golang.clone();

        let target_platform = lockfile.target_platform.clone();
        let workspace_dir = lockfile_path.parent();
        let member_roots = workspace_dir
            .map(|ws| crate::toolchain::ToolchainDetector::collect_workspace_member_dirs(ws, None))
            .unwrap_or_default();
        let target_py_minor = crate::toolchain::discovery::python::resolve_effective_python_minor(
            workspace_dir,
            &member_roots,
            target_platform.as_deref(),
        );
        let preferred_cp_tag = target_py_minor.map(|(major, minor)| format!("cp{major}{minor}"));

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
                let norm = ManifestScanner::normalize_pypi_name(&pkg.name);
                if ManifestScanner::is_obsolete_python_backport(&norm, target_py_minor) {
                    // Strip stdlib backports (e.g. `typing`, `enum34`) inherited from stale lockfiles.
                    mutated_lockfile = true;
                    continue;
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
                    let cp_tag_ref = preferred_cp_tag.as_deref();

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
                                cp_tag_ref,
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
                                ManifestScanner::parse_pep508_requirement_for_target_python(
                                    dep_str,
                                    target_platform.as_deref(),
                                    target_py_minor,
                                )
                            {
                                let norm = ManifestScanner::normalize_pypi_name(&dep_name);
                                if ManifestScanner::is_obsolete_python_backport(
                                    &norm,
                                    target_py_minor,
                                ) {
                                    continue;
                                }
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
mod tests;
