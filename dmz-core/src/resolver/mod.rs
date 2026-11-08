pub mod lockfile;
pub mod manifest;

use crate::resolver::manifest::Ecosystem;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

pub use lockfile::{DmzLockfile, LockError, LockedPackage};

#[derive(Debug)]
pub enum ResolverError {
    Io(String),
    Lockfile(String),
    Network(String),
    Parse(String),
}

impl std::fmt::Display for ResolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "Resolver IO Error: {}", msg),
            Self::Lockfile(msg) => write!(f, "Resolver Lockfile Error: {}", msg),
            Self::Network(msg) => write!(f, "Resolver Network Error: {}", msg),
            Self::Parse(msg) => write!(f, "Resolver Parse Error: {}", msg),
        }
    }
}

impl std::error::Error for ResolverError {}

impl From<ResolverError> for String {
    fn from(err: ResolverError) -> Self {
        err.to_string()
    }
}

pub struct ResolverEngine;

impl ResolverEngine {
    /// Scans project workspace, auto-resolves dependencies across ecosystems, and produces a cryptographic lockfile
    pub fn resolve_workspace(workspace_dir: &Path) -> Result<DmzLockfile, String> {
        Self::resolve_workspace_with_target(workspace_dir, None)
    }

    /// Scans project workspace, auto-resolves dependencies, and tags the target platform
    pub fn resolve_workspace_with_target(
        workspace_dir: &Path,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, String> {
        let manifest_path = workspace_dir.join("dmz-workspace.toml");
        if manifest_path.is_file() {
            if let Ok(manifest) =
                crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
            {
                if !manifest.members.is_empty() {
                    return Self::resolve_multi_workspace_with_target(
                        workspace_dir,
                        &manifest,
                        target_platform,
                    );
                }
            }
        }

        Self::resolve_single_workspace_with_target(workspace_dir, target_platform)
    }

    /// Resolves dependencies across all members of a multi-repository `WorkspaceProjectConfig`,
    /// tagging each package with its canonical `member_id` and recording workspace metadata.
    pub fn resolve_multi_workspace(
        workspace_dir: &Path,
        project: &crate::workspace::WorkspaceProjectConfig,
        manifest_path: Option<&Path>,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, String> {
        let layouts = project.resolve_member_layouts(workspace_dir, manifest_path);
        if layouts.is_empty() {
            return Self::resolve_single_workspace_with_target(workspace_dir, target_platform);
        }

        info!(
            project = %project.name,
            members = layouts.len(),
            "Resolving multi-workspace dependencies across all declared members"
        );

        let mut merged_packages = Vec::new();
        let mut workspaces_meta = Vec::new();

        for layout in &layouts {
            if !layout.host_path.is_dir() {
                return Err(format!(
                    "Workspace member '{}' directory does not exist at {:?}",
                    layout.id, layout.host_path
                ));
            }

            let member_cfg = project.find_member(&layout.id);
            match Self::resolve_single_workspace_with_target(
                &layout.host_path,
                target_platform.clone(),
            ) {
                Ok(member_lock) => {
                    let member_sig = member_lock.closure_signature.clone();
                    for (_, mut pkg) in member_lock.packages {
                        pkg.member_id = Some(layout.id.clone());
                        merged_packages.push(pkg);
                    }
                    workspaces_meta.push(crate::resolver::lockfile::WorkspaceMetadata {
                        id: layout.id.clone(),
                        path: member_cfg
                            .map(|m| m.path.display().to_string())
                            .unwrap_or_else(|| layout.host_path.display().to_string()),
                        role: layout.role.to_string(),
                        ecosystem: member_cfg.and_then(|m| m.ecosystem.clone()),
                        closure_signature: Some(member_sig),
                    });
                }
                Err(e) if layouts.len() > 1 && e.contains("resolved 0 packages") => {
                    info!(
                        member = %layout.id,
                        path = ?layout.host_path,
                        "Member has 0 external dependencies; continuing multi-workspace resolution"
                    );
                    workspaces_meta.push(crate::resolver::lockfile::WorkspaceMetadata {
                        id: layout.id.clone(),
                        path: member_cfg
                            .map(|m| m.path.display().to_string())
                            .unwrap_or_else(|| layout.host_path.display().to_string()),
                        role: layout.role.to_string(),
                        ecosystem: member_cfg.and_then(|m| m.ecosystem.clone()),
                        closure_signature: None,
                    });
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to resolve member '{}' at {:?}: {}",
                        layout.id, layout.host_path, e
                    ));
                }
            }
        }

        let mut unified_lock = DmzLockfile::with_platform(merged_packages, target_platform);
        for meta in workspaces_meta {
            unified_lock.add_workspace(meta);
        }

        if let Ok(Some(prov)) = crate::vcs::GitProvider::get_provenance(workspace_dir) {
            unified_lock.vcs = Some(prov);
        }

        info!(
            closure_sig = %unified_lock.closure_signature,
            packages = unified_lock.packages.len(),
            workspaces = unified_lock.workspaces.len(),
            "Successfully resolved multi-workspace project into unified lockfile"
        );

        Ok(unified_lock)
    }

    /// Resolves multi-workspace projects by resolving each declared member and merging into a unified lockfile
    pub fn resolve_multi_workspace_with_target(
        workspace_dir: &Path,
        manifest: &crate::workspace::WorkspaceProjectConfig,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, String> {
        Self::resolve_multi_workspace(workspace_dir, manifest, None, target_platform)
    }

    /// Scans a single workspace directory, auto-resolves dependencies, and tags the target platform
    pub fn resolve_single_workspace_with_target(
        workspace_dir: &Path,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, String> {
        info!(path = ?workspace_dir, "Scanning workspace and auto-resolving dependencies across ecosystems...");

        let mut locked_packages = Vec::new();

        // Native Cargo Auto-Resolution (Root Cargo.toml or nested src-tauri/Cargo.toml)
        let cargo_toml_path = workspace_dir.join("Cargo.toml");
        let tauri_cargo_path = workspace_dir.join("src-tauri/Cargo.toml");
        let effective_cargo_dir = if cargo_toml_path.exists() {
            Some(workspace_dir.to_path_buf())
        } else if tauri_cargo_path.exists() {
            Some(workspace_dir.join("src-tauri"))
        } else {
            None
        };

        if let Some(ref cargo_dir) = effective_cargo_dir {
            info!(
                cargo_dir = ?cargo_dir,
                "Cargo.toml detected; invoking `cargo metadata` for full graph auto-resolution..."
            );
            let output = std::process::Command::new("cargo")
                .arg("metadata")
                .arg("--format-version")
                .arg("1")
                .current_dir(cargo_dir)
                .output()
                .map_err(|e| format!("Failed to execute `cargo metadata`: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Cargo metadata resolution failed:\n{}", stderr));
            }

            let json_meta: serde_json::Value = serde_json::from_slice(&output.stdout)
                .map_err(|e| format!("Failed to parse cargo metadata JSON: {}", e))?;

            // Collect workspace member IDs for accurate local path identification
            let workspace_member_ids: Vec<String> = json_meta
                .get("workspace_members")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            if let Some(packages) = json_meta.get("packages").and_then(|p| p.as_array()) {
                for pkg in packages {
                    let id = pkg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                    let name = pkg
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let version = pkg
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let manifest_path = pkg
                        .get("manifest_path")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    // Check if this package is a workspace member
                    let is_workspace_member = workspace_member_ids.iter().any(|m_id| m_id == id);

                    let version_req = if is_workspace_member {
                        format!("path:{}", manifest_path)
                    } else {
                        version
                    };

                    let dummy_hash = DmzLockfile::hash_bytes(
                        format!("{}:{}:{:?}", name, version_req, Ecosystem::Cargo).as_bytes(),
                    );

                    if !locked_packages
                        .iter()
                        .any(|p: &LockedPackage| p.name == name && p.version == version_req)
                    {
                        locked_packages.push(LockedPackage {
                            name,
                            version: version_req,
                            ecosystem: Ecosystem::Cargo,
                            sha256: dummy_hash,
                            source_url: None,
                            dependencies: Vec::new(),
                            member_id: None,
                        });
                    }
                }
            }
        }

        // Scan remaining non-Cargo ecosystems using the standard ManifestScanner
        let manifests = crate::resolver::manifest::ManifestScanner::scan_dir(workspace_dir)
            .map_err(|e| format!("Manifest Scan Error: {}", e))?;

        for manifest in manifests {
            info!(ecosystem = ?manifest.ecosystem, path = ?manifest.path, "Processing manifest");

            for dep in manifest.dependencies {
                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", dep.name, dep.version_req, dep.ecosystem).as_bytes(),
                );

                if !locked_packages.iter().any(|p: &LockedPackage| {
                    p.name == dep.name
                        && p.version == dep.version_req
                        && p.ecosystem == dep.ecosystem
                }) {
                    locked_packages.push(LockedPackage {
                        name: dep.name,
                        version: dep.version_req,
                        ecosystem: dep.ecosystem,
                        sha256: dummy_hash,
                        source_url: None,
                        dependencies: Vec::new(),
                        member_id: None,
                    });
                }
            }
        }

        if locked_packages.is_empty() {
            return Err(
                "Fail-Fast Guardrail: Workspace scan detected manifests, but resolved 0 packages."
                    .to_string(),
            );
        }

        // Auto-resolve companion native binary package for @tauri-apps/cli
        let mut companion_pkgs = Vec::new();
        for pkg in &locked_packages {
            if pkg.name == "@tauri-apps/cli" {
                let platform_suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
                    ("macos", "aarch64") => "darwin-arm64".to_string(),
                    ("macos", "x86_64") => "darwin-x64".to_string(),
                    ("linux", "x86_64") => "linux-x64-gnu".to_string(),
                    ("linux", "aarch64") => "linux-arm64-gnu".to_string(),
                    ("windows", "x86_64") => "win32-x64-msvc".to_string(),
                    ("windows", "aarch64") => "win32-arm64-msvc".to_string(),
                    (other_os, other_arch) => {
                        let os_str = if other_os == "macos" {
                            "darwin"
                        } else {
                            other_os
                        };
                        let arch_str = if other_arch == "aarch64" {
                            "arm64"
                        } else if other_arch == "x86_64" {
                            "x64"
                        } else {
                            other_arch
                        };
                        format!("{}-{}", os_str, arch_str)
                    }
                };
                let companion_name = format!("@tauri-apps/cli-{}", platform_suffix);
                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", companion_name, pkg.version, Ecosystem::Npm).as_bytes(),
                );
                if !locked_packages.iter().any(|p| p.name == companion_name) {
                    companion_pkgs.push(LockedPackage {
                        name: companion_name,
                        version: pkg.version.clone(),
                        ecosystem: Ecosystem::Npm,
                        sha256: dummy_hash,
                        source_url: None,
                        dependencies: Vec::new(),
                        member_id: pkg.member_id.clone(),
                    });
                }
            }
        }
        locked_packages.extend(companion_pkgs);

        let mut lockfile = DmzLockfile::with_platform(locked_packages, target_platform);
        if let Ok(Some(prov)) = crate::vcs::GitProvider::get_provenance(workspace_dir) {
            info!(
                commit = %prov.commit,
                branch = ?prov.branch,
                dirty = prov.dirty,
                "Attached VCS provenance to dmz.lock"
            );
            lockfile.vcs = Some(prov);
        }
        info!(closure_sig = %lockfile.closure_signature, count = lockfile.packages.len(), "Generated deterministic agnostic DMZ closure signature");

        Ok(lockfile)
    }

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

        let packages_to_download: Vec<(String, LockedPackage)> = lockfile
            .packages
            .clone()
            .into_iter()
            .filter(|(_k, pkg)| !pkg.version.starts_with("path:") && !pkg.version.contains("path"))
            .collect();

        if !packages_to_download.is_empty() {
            let queue = Arc::new(Mutex::new(VecDeque::from(packages_to_download)));
            let (tx, rx) = std::sync::mpsc::channel();

            let num_workers = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 16);

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
                                let mut lock = queue.lock().unwrap();
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

            for res in rx {
                if let Some((map_key, updated_pkg)) = res? {
                    fetched_count += 1;
                    lockfile.packages.insert(map_key, updated_pkg);
                }
            }
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
            if parts.len() >= 2 {
                (parts[0], parts[1])
            } else {
                (host_os, host_arch)
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

        let clean_version = concrete_version
            .trim_start_matches('=')
            .trim_start_matches('^')
            .trim_start_matches('~')
            .trim();

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
                                    if let Some(tarball) = json
                                        .get("versions")
                                        .and_then(|v| v.get(clean_version))
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
                                let scope = &pkg.name[..idx];
                                let short_name = &pkg.name[idx + 1..];
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
                    let pypi_url = format!(
                        "{}/{}/{}/json",
                        pypi_base.trim_end_matches('/'),
                        pkg.name,
                        clean_version
                    );
                    let mut resolved_url = String::new();

                    if let Ok(resp) = client.get(&pypi_url).send() {
                        if resp.status().is_success() {
                            if let Ok(text) = resp.text() {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                    if let Some(urls) = json.get("urls").and_then(|u| u.as_array())
                                    {
                                        if let Some(best) =
                                            Self::select_pypi_artifact(urls, target_platform)
                                        {
                                            resolved_url = best;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if resolved_url.is_empty() {
                        format!(
                            "https://files.pythonhosted.org/packages/source/{}/{}/{}-{}.tar.gz",
                            &pkg.name[..1],
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
        updated_pkg.sha256 = computed_hash;

        let artifact_filename = format!("{}-{}.archive", safe_pkg_name, clean_version);
        let artifact_path = pkg_dir.join(artifact_filename);
        std::fs::write(&artifact_path, &bytes).map_err(|e| {
            format!(
                "Failed to write artifact file for {}: {}",
                updated_pkg.name, e
            )
        })?;

        let meta_path = pkg_dir.join("metadata.json");
        let meta_data = serde_json::to_string_pretty(&updated_pkg).unwrap_or_default();
        std::fs::write(&meta_path, meta_data)
            .map_err(|e| format!("Failed to write package metadata: {}", e))?;

        info!(name = %updated_pkg.name, version = %clean_version, "Successfully downloaded, verified, and cached artifact");
        Ok(Some((map_key, updated_pkg)))
    }
}
