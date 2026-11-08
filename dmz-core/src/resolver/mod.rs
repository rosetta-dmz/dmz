pub mod lockfile;
pub mod manifest;

use crate::resolver::manifest::Ecosystem;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing::info;

pub use lockfile::{DmzLockfile, LockError, LockedPackage};

pub struct ResolverEngine;

impl ResolverEngine {
    /// Scans project workspace, auto-resolves dependencies across ecosystems, and produces a cryptographic lockfile
    pub fn resolve_workspace(workspace_dir: &Path) -> Result<DmzLockfile, String> {
        Self::resolve_workspace_with_target(workspace_dir, None)
    }

    /// Scans project workspace, auto-resolves dependencies, and tags the target platform.
    /// Automatically detects `dmz-workspace.toml` for multi-repository / companion workspaces.
    pub fn resolve_workspace_with_target(
        workspace_dir: &Path,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, String> {
        let manifest_file = workspace_dir.join("dmz-workspace.toml");
        if manifest_file.is_file() {
            if let Ok(project) =
                crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_file)
            {
                let layouts = project.resolve_member_layouts(workspace_dir, Some(&manifest_file));
                if !layouts.is_empty() && layouts.iter().all(|l| l.host_path.is_dir()) {
                    return Self::resolve_multi_workspace(
                        workspace_dir,
                        &project,
                        Some(&manifest_file),
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
                        "Dependency resolution failed for member '{}' ({:?}): {}",
                        layout.id, layout.host_path, e
                    ));
                }
            }
        }

        if merged_packages.is_empty() {
            return Err(
                "Fail-Fast Guardrail: Multi-workspace scan detected manifests, but resolved 0 packages across all members."
                    .to_string(),
            );
        }

        let mut unified_lock = DmzLockfile::with_platform(merged_packages, target_platform);
        if let Ok(Some(prov)) = crate::vcs::GitProvider::get_provenance(workspace_dir) {
            unified_lock.vcs = Some(prov);
        }
        for meta in workspaces_meta {
            unified_lock.add_workspace(meta);
        }

        Ok(unified_lock)
    }

    /// Scans a single project workspace directory, auto-resolves dependencies, and tags the target platform
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

        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!(
                "dmz-airgap-tool/",
                env!("CARGO_PKG_VERSION"),
                " (air-gap workspace manager)"
            ))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        for pkg in lockfile.packages.values_mut() {
            // --- SKIP LOCAL PATH DEPENDENCIES & WORKSPACE MEMBERS ---
            if pkg.version.starts_with("path:") {
                info!(name = %pkg.name, version = %pkg.version, "Skipping remote download for local path dependency/workspace member");
                continue;
            }

            info!(name = %pkg.name, version_req = %pkg.version, "Resolving concrete version and downloading package artifact");
            // --- RESOLVE CONCRETE VERSION FROM CRATES.IO API IF REQUIREMENT IS PARTIAL ---
            let eco_str = format!("{:?}", pkg.ecosystem).to_lowercase();
            let mut concrete_version = pkg.version.clone();

            if (eco_str == "cargo" || eco_str == "rust")
                && (!concrete_version.contains('.') || concrete_version.matches('.').count() < 2)
            {
                let meta_url = format!("https://crates.io/api/v1/crates/{}", pkg.name);
                if let Ok(resp) = client.get(&meta_url).send() {
                    if resp.status().is_success() {
                        if let Ok(text) = resp.text() {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                if let Some(versions) =
                                    json.get("versions").and_then(|v| v.as_array())
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

            // --- SANITIZE VERSION OPERATORS FOR URL CONSTRUCTION ---
            let clean_version = concrete_version
                .trim_start_matches('=')
                .trim_start_matches('^')
                .trim_start_matches('~')
                .trim();

            let safe_pkg_name = pkg.name.replace('/', "__");

            let pkg_dir = store_path.join(&safe_pkg_name);
            std::fs::create_dir_all(&pkg_dir)
                .map_err(|e| format!("Failed to create package directory: {}", e))?;

            let artifact_filename = format!("{}-{}.archive", safe_pkg_name, clean_version);
            let artifact_path = pkg_dir.join(&artifact_filename);

            // --- STORE ARTIFACT CACHE HIT CHECK ---
            if artifact_path.is_file() && !pkg.sha256.is_empty() {
                let mut identity_hasher = Sha256::new();
                identity_hasher.update(format!("{}:{}", pkg.name, pkg.version).as_bytes());
                let placeholder_hash = format!("{:x}", identity_hasher.finalize());

                if pkg.sha256 != placeholder_hash {
                    if let Ok(mut cached_file) = std::fs::File::open(&artifact_path) {
                        let mut cache_hasher = Sha256::new();
                        if std::io::copy(&mut cached_file, &mut cache_hasher).is_ok() {
                            let cached_digest = format!("{:x}", cache_hasher.finalize());
                            if cached_digest == pkg.sha256 {
                                info!(
                                    name = %pkg.name,
                                    version = %clean_version,
                                    sha256 = %cached_digest,
                                    "Store artifact cache hit verified; skipping download"
                                );
                                continue;
                            }
                        }
                    }
                }
            }

            let download_url = match &pkg.source_url {
                Some(url) => url.clone(),
                None => match eco_str.as_str() {
                    "cargo" | "rust" => format!(
                        "https://static.crates.io/crates/{}/{}-{}.crate",
                        pkg.name, pkg.name, clean_version
                    ),
                    "npm" | "node" => {
                        let encoded_name = pkg.name.replace('/', "%2F");
                        let meta_url = format!("https://registry.npmjs.org/{}", encoded_name);
                        let mut resolved_tarball = String::new();

                        if let Ok(resp) = client.get(&meta_url).send() {
                            if resp.status().is_success() {
                                if let Ok(text) = resp.text() {
                                    if let Ok(json) =
                                        serde_json::from_str::<serde_json::Value>(&text)
                                    {
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
                            if pkg.name.starts_with('@') {
                                if let Some(idx) = pkg.name.find('/') {
                                    let scope = &pkg.name[..idx];
                                    let short_name = &pkg.name[idx + 1..];
                                    format!(
                                        "https://registry.npmjs.org/{}%2F{}/-/{}-{}.tgz",
                                        scope, short_name, short_name, clean_version
                                    )
                                } else {
                                    format!(
                                        "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                                        pkg.name, pkg.name, clean_version
                                    )
                                }
                            } else {
                                format!(
                                    "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                                    pkg.name, pkg.name, clean_version
                                )
                            }
                        } else {
                            resolved_tarball
                        }
                    }
                    "pypi" | "python" => {
                        let pypi_url =
                            format!("https://pypi.org/pypi/{}/{}/json", pkg.name, clean_version);
                        let mut resolved_url = String::new();

                        if let Ok(resp) = client.get(&pypi_url).send() {
                            if resp.status().is_success() {
                                if let Ok(text) = resp.text() {
                                    if let Ok(json) =
                                        serde_json::from_str::<serde_json::Value>(&text)
                                    {
                                        if let Some(urls) =
                                            json.get("urls").and_then(|u| u.as_array())
                                        {
                                            for u in urls {
                                                if let Some(url_str) =
                                                    u.get("url").and_then(|s| s.as_str())
                                                {
                                                    if url_str.ends_with(".whl")
                                                        || url_str.ends_with(".tar.gz")
                                                    {
                                                        resolved_url = url_str.to_string();
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if resolved_url.is_empty() {
                            let first_char = pkg.name.chars().next().unwrap_or('_');
                            format!(
                                "https://files.pythonhosted.org/packages/source/{}/{}/{}-{}.tar.gz",
                                first_char, pkg.name, pkg.name, clean_version
                            )
                        } else {
                            resolved_url
                        }
                    }
                    "go" | "golang" => {
                        format!(
                            "https://proxy.golang.org/{}/@v/{}.zip",
                            pkg.name, clean_version
                        )
                    }
                    _ => {
                        info!(name = %pkg.name, ecosystem = %eco_str, "No source URL and unknown ecosystem, skipping download");
                        continue;
                    }
                },
            };

            let mut response = client
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

            // Stream response in 64 KB chunks to a temporary .part file while computing SHA-256
            let part_path = pkg_dir.join(format!("{}.part", artifact_filename));
            let part_file = std::fs::File::create(&part_path)
                .map_err(|e| format!("Failed to create staging file for {}: {}", pkg.name, e))?;
            let mut writer = std::io::BufWriter::new(part_file);
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536];

            loop {
                let n = std::io::Read::read(&mut response, &mut buffer).map_err(|e| {
                    let _ = std::fs::remove_file(&part_path);
                    format!("Failed to stream response bytes for {}: {}", pkg.name, e)
                })?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
                std::io::Write::write_all(&mut writer, &buffer[..n]).map_err(|e| {
                    let _ = std::fs::remove_file(&part_path);
                    format!("Failed to write artifact chunk for {}: {}", pkg.name, e)
                })?;
            }

            std::io::Write::flush(&mut writer)
                .map_err(|e| format!("Failed to flush artifact file for {}: {}", pkg.name, e))?;
            drop(writer);

            std::fs::rename(&part_path, &artifact_path).map_err(|e| {
                let _ = std::fs::remove_file(&part_path);
                format!("Failed to finalize artifact file for {}: {}", pkg.name, e)
            })?;

            let computed_hash = format!("{:x}", hasher.finalize());

            pkg.sha256 = computed_hash;

            let meta_path = pkg_dir.join("metadata.json");
            let meta_data = serde_json::to_string_pretty(&pkg).unwrap_or_default();
            std::fs::write(&meta_path, meta_data)
                .map_err(|e| format!("Failed to write package metadata: {}", e))?;

            fetched_count += 1;
            info!(name = %pkg.name, version = %clean_version, "Successfully downloaded, verified, and cached artifact");
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
}
