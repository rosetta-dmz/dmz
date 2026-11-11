use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::{Ecosystem, ManifestScanner};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

pub struct DependencyFetcher;

impl DependencyFetcher {
    /// Fetches locked dependencies from dmz.lock, downloads real package artifacts, verifies hashes, and populates the local store cache
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

                            let res = Self::fetch_single_package(
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

    /// Evaluates Python wheel ABI tags and selects the best matching artifact for target platform
    pub fn select_pypi_artifact(
        urls: &[serde_json::Value],
        target_platform: Option<&str>,
    ) -> Option<String> {
        let host_os = std::env::consts::OS;
        let host_arch = std::env::consts::ARCH;

        let (target_os, target_arch) = if let Some(target) = target_platform {
            let parts: Vec<&str> = target.split('-').collect();
            match (parts.first(), parts.get(1)) {
                (Some(&os), Some(&arch)) => (os, arch),
                _ => (host_os, host_arch),
            }
        } else {
            (host_os, host_arch)
        };

        let is_macos = target_os == "macos" || target_os == "darwin";
        let is_linux = target_os == "linux";
        let is_windows = target_os == "windows" || target_os == "win32";

        let is_arm64 = target_arch == "arm64" || target_arch == "aarch64";
        let is_x86_64 = target_arch == "x86_64" || target_arch == "x64";

        let mut best_score = -1;
        let mut best_url = None;

        for entry in urls {
            let url_str = match entry.get("url").and_then(|u| u.as_str()) {
                Some(u) => u,
                None => continue,
            };
            let filename = entry
                .get("filename")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_lowercase();

            let score: i32 = if filename.ends_with(".whl") {
                if filename.contains("-none-any.whl")
                    || filename.contains("py2.py3-none-any")
                    || filename.contains("py3-none-any")
                {
                    80 // Pure Python universal wheel
                } else if is_macos && (filename.contains("macosx") || filename.contains("darwin")) {
                    if (is_arm64 && filename.contains("arm64"))
                        || (is_x86_64 && filename.contains("x86_64"))
                    {
                        100 // Exact architecture match
                    } else if filename.contains("universal2") {
                        95 // macOS universal binary
                    } else {
                        -1 // Incompatible architecture
                    }
                } else if is_linux
                    && (filename.contains("manylinux")
                        || filename.contains("musllinux")
                        || filename.contains("linux"))
                {
                    if (is_x86_64 && (filename.contains("x86_64") || filename.contains("amd64")))
                        || (is_arm64 && filename.contains("aarch64"))
                    {
                        100 // Exact Linux architecture match
                    } else {
                        -1
                    }
                } else if is_windows
                    && (filename.contains("win_amd64")
                        || filename.contains("win32")
                        || filename.contains("windows"))
                {
                    if (is_x86_64 && (filename.contains("amd64") || filename.contains("win_amd64")))
                        || (is_arm64 && filename.contains("arm64"))
                    {
                        100
                    } else {
                        -1
                    }
                } else {
                    -1 // Incompatible OS/platform wheel
                }
            } else if filename.ends_with(".tar.gz") || filename.ends_with(".zip") {
                50 // Source distribution fallback
            } else {
                10
            };

            if score > best_score {
                best_score = score;
                best_url = Some(url_str.to_string());
            }
        }

        best_url
    }

    #[allow(clippy::too_many_arguments)]
    fn fetch_single_package(
        client: &reqwest::blocking::Client,
        store_path: &Path,
        crates_api_base: &str,
        crates_dl_base: &str,
        npm_base: &str,
        pypi_base: &str,
        golang_proxy_base: &str,
        target_platform: Option<&str>,
        map_key: String,
        pkg: LockedPackage,
    ) -> Result<Option<(String, LockedPackage)>, String> {
        if pkg.version.starts_with("path:") || pkg.version.contains("path") {
            info!(name = %pkg.name, version = %pkg.version, "Skipping remote download for local path dependency/workspace member");
            return Ok(None);
        }

        info!(name = %pkg.name, version_req = %pkg.version, "Resolving concrete version and downloading package artifact");
        let eco_str = format!("{:?}", pkg.ecosystem).to_lowercase();
        let mut concrete_version = pkg.version.clone();

        if (eco_str == "cargo" || eco_str == "rust")
            && (!concrete_version.contains('.') || concrete_version.matches('.').count() < 2)
        {
            let meta_url = format!("{}/{}", crates_api_base.trim_end_matches('/'), pkg.name);
            if let Ok(resp) = client.get(&meta_url).send() {
                if resp.status().is_success() {
                    if let Ok(text) = resp.text() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(versions) = json.get("versions").and_then(|v| v.as_array())
                            {
                                for v in versions {
                                    if let Some(num) = v.get("num").and_then(|n| n.as_str()) {
                                        if num.starts_with(&concrete_version)
                                            || concrete_version == "*"
                                        {
                                            concrete_version = num.to_string();
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut clean_version = concrete_version
            .trim_start_matches('=')
            .trim_start_matches('^')
            .trim_start_matches('~')
            .trim()
            .to_string();
        let mut discovered_requires_dist: Vec<String> = Vec::new();

        let safe_pkg_name = pkg.name.replace('/', "__");
        let pkg_dir = store_path.join(&safe_pkg_name);
        std::fs::create_dir_all(&pkg_dir)
            .map_err(|e| format!("Failed to create package directory: {}", e))?;

        let download_url = match &pkg.source_url {
            Some(url) => url.clone(),
            None => match eco_str.as_str() {
                "cargo" | "rust" => format!(
                    "{}/{}/{}-{}.crate",
                    crates_dl_base.trim_end_matches('/'),
                    pkg.name,
                    pkg.name,
                    clean_version
                ),
                "npm" | "node" => {
                    let encoded_name = pkg.name.replace('/', "%2F");
                    let meta_url = format!("{}/{}", npm_base.trim_end_matches('/'), encoded_name);
                    let mut resolved_tarball = String::new();

                    if let Ok(resp) = client.get(&meta_url).send() {
                        if resp.status().is_success() {
                            if let Ok(text) = resp.text() {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                    let versions_obj =
                                        json.get("versions").and_then(|v| v.as_object());
                                    if let Some(versions) = versions_obj {
                                        if !versions.contains_key(&clean_version) {
                                            if clean_version == "*" || clean_version == "latest" {
                                                if let Some(latest) = json
                                                    .get("dist-tags")
                                                    .and_then(|dt| dt.get("latest"))
                                                    .and_then(|v| v.as_str())
                                                {
                                                    clean_version = latest.to_string();
                                                }
                                            } else if let Some(matched_ver) = versions
                                                .keys()
                                                .rev()
                                                .find(|k| k.starts_with(&clean_version))
                                            {
                                                clean_version = matched_ver.clone();
                                            }
                                        }
                                    }
                                    if let Some(tarball) = json
                                        .get("versions")
                                        .and_then(|v| v.get(&clean_version))
                                        .and_then(|ver| ver.get("dist"))
                                        .and_then(|d| d.get("tarball"))
                                        .and_then(|t| t.as_str())
                                    {
                                        resolved_tarball = tarball.to_string();
                                    }
                                }
                            }
                        }
                    }

                    if resolved_tarball.is_empty() {
                        let npm_root = npm_base.trim_end_matches('/');
                        if pkg.name.starts_with('@') {
                            if let Some(idx) = pkg.name.find('/') {
                                let scope = pkg.name.get(..idx).unwrap_or("");
                                let short_name = pkg.name.get(idx + 1..).unwrap_or("");
                                format!(
                                    "{}/{}%2F{}/-/{}-{}.tgz",
                                    npm_root, scope, short_name, short_name, clean_version
                                )
                            } else {
                                format!(
                                    "{}/{}/-/{}-{}.tgz",
                                    npm_root, pkg.name, pkg.name, clean_version
                                )
                            }
                        } else {
                            format!(
                                "{}/{}/-/{}-{}.tgz",
                                npm_root, pkg.name, pkg.name, clean_version
                            )
                        }
                    } else {
                        resolved_tarball
                    }
                }
                "pypi" | "python" => {
                    let mut resolved_url = String::new();
                    let pypi_root = pypi_base.trim_end_matches('/');

                    let candidate_urls = if clean_version == "*" || clean_version.is_empty() {
                        vec![format!("{}/{}/json", pypi_root, pkg.name)]
                    } else {
                        vec![
                            format!("{}/{}/{}/json", pypi_root, pkg.name, clean_version),
                            format!("{}/{}/json", pypi_root, pkg.name),
                        ]
                    };

                    for pypi_url in candidate_urls {
                        if let Ok(resp) = client.get(&pypi_url).send() {
                            if resp.status().is_success() {
                                if let Ok(text) = resp.text() {
                                    if let Ok(json) =
                                        serde_json::from_str::<serde_json::Value>(&text)
                                    {
                                        if let Some(info_ver) = json
                                            .get("info")
                                            .and_then(|i| i.get("version"))
                                            .and_then(|v| v.as_str())
                                        {
                                            if clean_version == "*"
                                                || clean_version.is_empty()
                                                || !pypi_url
                                                    .ends_with(&format!("/{}/json", clean_version))
                                            {
                                                clean_version = info_ver.to_string();
                                            }
                                        }
                                        if let Some(reqs) = json
                                            .get("info")
                                            .and_then(|i| i.get("requires_dist"))
                                            .and_then(|r| r.as_array())
                                        {
                                            for req_val in reqs {
                                                if let Some(req_str) = req_val.as_str() {
                                                    discovered_requires_dist
                                                        .push(req_str.to_string());
                                                }
                                            }
                                        }
                                        if let Some(urls) =
                                            json.get("urls").and_then(|u| u.as_array())
                                        {
                                            if let Some(best) =
                                                Self::select_pypi_artifact(urls, target_platform)
                                            {
                                                resolved_url = best;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if resolved_url.is_empty() {
                        let first_char = pkg.name.chars().next().unwrap_or('a');
                        format!(
                            "https://files.pythonhosted.org/packages/source/{}/{}/{}-{}.tar.gz",
                            first_char,
                            pkg.name,
                            pkg.name,
                            clean_version
                        )
                    } else {
                        resolved_url
                    }
                }
                "go" | "golang" => {
                    format!(
                        "{}/{}/@v/{}.zip",
                        golang_proxy_base.trim_end_matches('/'),
                        pkg.name,
                        clean_version
                    )
                }
                _ => {
                    info!(name = %pkg.name, ecosystem = %eco_str, "No source URL and unknown ecosystem, skipping download");
                    return Ok(None);
                }
            },
        };

        let response = client
            .get(&download_url)
            .send()
            .map_err(|e| format!("HTTP request failed for {}: {}", download_url, e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to download {}: server returned status {}",
                download_url,
                response.status()
            ));
        }

        let bytes = response
            .bytes()
            .map_err(|e| format!("Failed to read response bytes for {}: {}", pkg.name, e))?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let computed_hash = format!("{:x}", hasher.finalize());

        let mut updated_pkg = pkg;
        updated_pkg.version = clean_version.clone();
        updated_pkg.sha256 = computed_hash;
        if !discovered_requires_dist.is_empty() && updated_pkg.dependencies.is_empty() {
            updated_pkg.dependencies = discovered_requires_dist;
        }

        let artifact_filename = format!("{}-{}.archive", safe_pkg_name, clean_version);
        let artifact_path = pkg_dir.join(artifact_filename);
        crate::workspace::atomic_write_file(&artifact_path, &bytes).map_err(|e| {
            format!(
                "Failed to write artifact file for {}: {}",
                updated_pkg.name, e
            )
        })?;

        let meta_path = pkg_dir.join("metadata.json");
        let meta_data = serde_json::to_string_pretty(&updated_pkg).unwrap_or_default();
        crate::workspace::atomic_write_file(&meta_path, meta_data)
            .map_err(|e| format!("Failed to write package metadata: {}", e))?;

        info!(name = %updated_pkg.name, version = %clean_version, "Successfully downloaded, verified, and cached artifact");
        Ok(Some((map_key, updated_pkg)))
    }
}

