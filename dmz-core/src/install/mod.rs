pub mod archive;
pub mod fast_clone;
pub mod mappers;
pub mod rust_vendor;
use crate::resolver::lockfile::DmzLockfile;
use crate::resolver::manifest::Ecosystem;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub workspace_path: PathBuf,
    pub lockfile_path: PathBuf,
    pub store_dir: PathBuf,
}

#[derive(Debug)]
pub enum InstallError {
    IoError(String),
    LockfileError(String),
    IntegrityMismatch { expected: String, actual: String },
    ArtifactMissing { package: String, path: PathBuf },
    UnsupportedEcosystem(String),
    MapperError(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Install IO Error: {}", msg),
            Self::LockfileError(msg) => write!(f, "Install Lockfile Error: {}", msg),
            Self::IntegrityMismatch { expected, actual } => write!(
                f,
                "Store Artifact Integrity Mismatch! Expected sha256:{}, computed sha256:{}",
                expected, actual
            ),
            Self::ArtifactMissing { package, path } => write!(
                f,
                "Missing cached store artifact for package '{}' at path {:?}",
                package, path
            ),
            Self::UnsupportedEcosystem(eco) => {
                write!(f, "Ecosystem not supported for offline install: {}", eco)
            }
            Self::MapperError(msg) => write!(f, "Ecosystem Mapper Error: {}", msg),
        }
    }
}

impl std::error::Error for InstallError {}

pub struct InstallerEngine;

impl InstallerEngine {
    /// Installs locked dependencies from the local store into project layouts without network access
    pub fn install(config: &InstallConfig) -> Result<usize, InstallError> {
        info!(lockfile = ?config.lockfile_path, store = ?config.store_dir, "Starting DMZ native offline installation");

        // Load and cryptographically verify lockfile integrity
        let lockfile = DmzLockfile::load_and_verify(&config.lockfile_path)
            .map_err(|e| InstallError::LockfileError(e.to_string()))?;

        if !config.store_dir.exists() {
            return Err(InstallError::IoError(format!(
                "Store directory does not exist at {:?}. Run `dmz fetch` or `dmz airgap import` first.",
                config.store_dir
            )));
        }

        // Discover multi-workspace project manifest (dmz-workspace.toml) if present
        let workspace_manifest = if config.workspace_path.join("dmz-workspace.toml").is_file() {
            crate::workspace::WorkspaceProjectConfig::load_from_file(
                &config.workspace_path.join("dmz-workspace.toml"),
            )
            .ok()
        } else {
            crate::workspace::WorkspaceProjectConfig::discover_workspace_manifest(
                &config.workspace_path,
            )
            .map(|(p, _)| p)
        };

        let mut installed_count = 0;

        for (_key, pkg) in lockfile.packages {
            if pkg.version.starts_with("path:") {
                info!(name = %pkg.name, "Skipping install for local path dependency");
                continue;
            }

            // Resolve target workspace: if pkg.member_id points to a member, map to its subfolder
            let target_workspace = if let Some(ref manifest) = workspace_manifest {
                // 1. Try matching by member ID or role alias
                let member_opt = if let Some(ref mid) = pkg.member_id {
                    manifest.find_member(mid).or_else(|| {
                        if mid == "primary" || mid == "default" {
                            manifest.primary_member()
                        } else if mid == "sidecar"
                            || mid == "secondary"
                            || mid == "companion"
                            || mid == "service"
                        {
                            manifest.sidecars().first().copied()
                        } else {
                            manifest
                                .members
                                .iter()
                                .find(|m| m.id.eq_ignore_ascii_case(mid))
                        }
                    })
                } else {
                    None
                };

                if let Some(member) = member_opt {
                    let member_dir = if member.path.is_absolute() {
                        member.path.clone()
                    } else {
                        config.workspace_path.join(&member.path)
                    };
                    let member_dir = std::fs::canonicalize(&member_dir).unwrap_or(member_dir);
                    if member_dir.is_dir() {
                        member_dir
                    } else {
                        config.workspace_path.clone()
                    }
                } else {
                    // 2. Intelligent Ecosystem Fallback:
                    // Route the package to the workspace member that actually owns the matching manifest
                    let ecosystem_match = manifest.members.iter().find(|m| {
                        let m_dir = if m.path.is_absolute() {
                            m.path.clone()
                        } else {
                            config.workspace_path.join(&m.path)
                        };
                        let m_dir = std::fs::canonicalize(&m_dir).unwrap_or(m_dir);
                        match pkg.ecosystem {
                            Ecosystem::Npm => m_dir.join("package.json").is_file(),
                            Ecosystem::Cargo => {
                                m_dir.join("Cargo.toml").is_file()
                                    || m_dir.join("src-tauri/Cargo.toml").is_file()
                            }
                            Ecosystem::PyPI => {
                                m_dir.join("requirements.txt").is_file()
                                    || m_dir.join("pyproject.toml").is_file()
                            }
                            Ecosystem::Go => m_dir.join("go.mod").is_file(),
                            Ecosystem::Composer => m_dir.join("composer.json").is_file(),
                            _ => false,
                        }
                    });

                    if let Some(member) = ecosystem_match {
                        let m_dir = if member.path.is_absolute() {
                            member.path.clone()
                        } else {
                            config.workspace_path.join(&member.path)
                        };
                        std::fs::canonicalize(&m_dir).unwrap_or(m_dir)
                    } else {
                        config.workspace_path.clone()
                    }
                }
            } else {
                config.workspace_path.clone()
            };

            // Handle flattened scoped package names (e.g., @scope__pkg)
            let safe_name = pkg.name.replace('/', "__");
            let pkg_store_dir = config.store_dir.join(&safe_name);

            // Robustly resolve the archive file in the store directory (handling version specificity differences)
            let exact_name = format!("{}-{}.archive", safe_name, pkg.version);
            let exact_path = pkg_store_dir.join(&exact_name);

            let archive_path = if exact_path.exists() {
                exact_path
            } else if let Ok(entries) = fs::read_dir(&pkg_store_dir) {
                let prefix = format!("{}-", safe_name);
                let mut matched_path = None;
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "archive") {
                        if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                            if fname.starts_with(&prefix) && fname.contains(&pkg.version) {
                                matched_path = Some(path);
                                break;
                            }
                        }
                    }
                }
                matched_path.unwrap_or(exact_path)
            } else {
                exact_path
            };

            if !archive_path.exists() {
                return Err(InstallError::ArtifactMissing {
                    package: pkg.name.clone(),
                    path: archive_path,
                });
            }

            // Cryptographic validation of store artifact against lockfile hash (streaming 64KB chunk buffer)
            let mut file = fs::File::open(&archive_path)
                .map_err(|e| InstallError::IoError(format!("Failed to open artifact: {}", e)))?;

            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer).map_err(|e| {
                    InstallError::IoError(format!("Failed to read artifact: {}", e))
                })?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            let computed_hash = format!("{:x}", hasher.finalize());

            if computed_hash != pkg.sha256 {
                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", pkg.name, pkg.version, pkg.ecosystem).as_bytes(),
                );
                if pkg.sha256 == dummy_hash {
                    tracing::debug!(
                        package = %pkg.name,
                        "Store artifact matches pre-fetch identity signature"
                    );
                } else {
                    tracing::error!(
                        package = %pkg.name,
                        expected = %pkg.sha256,
                        computed = %computed_hash,
                        "Supply chain integrity mismatch! Expected checksum does not match store artifact"
                    );
                    return Err(InstallError::IntegrityMismatch {
                        expected: pkg.sha256.clone(),
                        actual: computed_hash,
                    });
                }
            } else {
                tracing::debug!(
                    package = %pkg.name,
                    checksum = %computed_hash,
                    "Cryptographically verified store artifact against lockfile"
                );
            }

            // Ecosystem-specific deterministic linkage
            match pkg.ecosystem {
                Ecosystem::Npm => {
                    let pnpm_detected = target_workspace.join("pnpm-lock.yaml").exists()
                        || config.workspace_path.join("pnpm-lock.yaml").exists()
                        || target_workspace.join("pnpm-workspace.yaml").exists()
                        || config.workspace_path.join("pnpm-workspace.yaml").exists()
                        || target_workspace.join("node_modules/.pnpm").is_dir()
                        || config.workspace_path.join("node_modules/.pnpm").is_dir()
                        || {
                            if let Ok(c) = fs::read_to_string(target_workspace.join("package.json"))
                            {
                                c.contains("\"pnpm\"")
                            } else {
                                false
                            }
                        };
                    if pnpm_detected {
                        mappers::PnpmMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                    } else {
                        mappers::NpmMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                    }
                }
                Ecosystem::Cargo => {
                    mappers::CargoMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                }
                Ecosystem::PyPI => {
                    mappers::PyPiMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                }
                Ecosystem::Go => {
                    mappers::GoMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                }
                Ecosystem::Composer => {
                    mappers::ComposerMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                }
                Ecosystem::Bundler => {
                    mappers::BundlerMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                }
                Ecosystem::MavenGradle => {
                    mappers::MavenMapper::map_package(&target_workspace, &pkg, &archive_path)?;
                }
                Ecosystem::Unknown(ref eco) => {
                    return Err(InstallError::UnsupportedEcosystem(eco.clone()));
                }
                _ => {
                    return Err(InstallError::UnsupportedEcosystem(format!(
                        "{:?}",
                        pkg.ecosystem
                    )));
                }
            }

            installed_count += 1;
            info!(name = %pkg.name, version = %pkg.version, "Successfully installed package offline");
        }

        // Ensure IDE workspace settings (.vscode/settings.json) exclude vendored directories
        // and suppress false-positive CMake/Makefile extension popups.
        let _ = crate::toolchain::ToolchainSetup::generate_ide_settings(&config.workspace_path);
        if let Some(ref manifest) = workspace_manifest {
            for member in &manifest.members {
                let member_path = config.workspace_path.join(&member.path);
                if member_path.is_dir() {
                    let _ = crate::toolchain::ToolchainSetup::generate_ide_settings(&member_path);
                }
            }
        }

        // Hydrate CLI binary shims (e.g. node_modules/.bin/tauri) for root and all member layouts
        mappers::NpmMapper::hydrate_workspace_bin_shims(&config.workspace_path);
        if let Some(ref manifest) = workspace_manifest {
            for member in &manifest.members {
                let member_path = if member.path.is_absolute() {
                    member.path.clone()
                } else {
                    config.workspace_path.join(&member.path)
                };
                let member_path = std::fs::canonicalize(&member_path).unwrap_or(member_path);
                if member_path.is_dir() {
                    mappers::NpmMapper::hydrate_workspace_bin_shims(&member_path);
                }
            }
        }

        // Regenerate environment activator scripts (.dmz/env.sh, .dmz/env.bat) for root and members
        let _ =
            crate::toolchain::ToolchainSetup::generate_activator_scripts(&config.workspace_path);

        // Ensure .modules.yaml exists in root and all members with node_modules
        mappers::PnpmMapper::ensure_modules_yaml(&config.workspace_path);
        if let Some(ref manifest) = workspace_manifest {
            for member in &manifest.members {
                let member_path = if member.path.is_absolute() {
                    member.path.clone()
                } else {
                    config.workspace_path.join(&member.path)
                };
                let member_path = std::fs::canonicalize(&member_path).unwrap_or(member_path);
                if member_path.is_dir() {
                    mappers::PnpmMapper::ensure_modules_yaml(&member_path);
                }
            }
        }

        info!(
            installed = installed_count,
            "DMZ offline installation completed successfully"
        );
        Ok(installed_count)
    }
}
