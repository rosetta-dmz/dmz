use crate::archive::exporter::hashing::calculate_file_sha256;
use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tracing::warn;

/// Cargo / Rust source vendor mapper
pub struct CargoMapper;

impl CargoMapper {
    fn hash_file_sha256(path: &Path) -> Result<String, InstallError> {
        calculate_file_sha256(path).map_err(|e| {
            InstallError::IoError(format!("Failed to hash vendored file {:?}: {}", path, e))
        })
    }

    fn collect_vendor_file_hashes(
        root: &Path,
        current: &Path,
        out: &mut BTreeMap<String, String>,
    ) -> Result<(), InstallError> {
        for entry in fs::read_dir(current)
            .map_err(|e| InstallError::IoError(format!("Failed to list {:?}: {}", current, e)))?
            .flatten()
        {
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file_name == ".cargo-checksum.json" {
                continue;
            }
            if path.is_dir() && !path.is_symlink() {
                Self::collect_vendor_file_hashes(root, &path, out)?;
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    let key = rel.to_string_lossy().replace('\\', "/");
                    let digest = Self::hash_file_sha256(&path)?;
                    out.insert(key, digest);
                }
            }
        }
        Ok(())
    }

    fn ensure_vendored_cargo_config(
        config_path: &Path,
        vendor_rel_dir: &str,
    ) -> Result<(), InstallError> {
        let block = format!(
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"{}\"\n",
            vendor_rel_dir
        );
        if !config_path.exists() {
            return fs::write(config_path, block).map_err(|e| {
                InstallError::IoError(format!("Failed to write {:?}: {}", config_path, e))
            });
        }
        let existing = fs::read_to_string(config_path).map_err(|e| {
            InstallError::IoError(format!("Failed to read {:?}: {}", config_path, e))
        })?;
        if !existing.contains("[source.vendored-sources]") {
            let merged = if existing.trim().is_empty() {
                block
            } else {
                format!("{}\n\n{}", existing.trim_end(), block)
            };
            fs::write(config_path, merged).map_err(|e| {
                InstallError::IoError(format!("Failed to update {:?}: {}", config_path, e))
            })?;
        }
        Ok(())
    }

    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let cargo_dir = workspace_path.join(".cargo");
        let vendor_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}", pkg.name, pkg.version));

        if vendor_dir.exists() {
            fs::remove_dir_all(&vendor_dir).map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to clear existing vendor dir {:?}: {}",
                    vendor_dir, e
                ))
            })?;
        }
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create cargo vendor dir: {}", e))
        })?;

        // Unpack into a temporary staging directory to handle archive nesting
        let temp_extract_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}-tmp", pkg.name, pkg.version));
        if temp_extract_dir.exists() {
            fs::remove_dir_all(&temp_extract_dir).map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to clear temp extract dir {:?}: {}",
                    temp_extract_dir, e
                ))
            })?;
        }
        fs::create_dir_all(&temp_extract_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create temp extract dir: {}", e))
        })?;

        SafeArchiveExtractor::extract(archive_path, &temp_extract_dir).map_err(|e| {
            InstallError::MapperError(format!(
                "Failed to unpack Cargo package {}: {}",
                pkg.name, e
            ))
        })?;

        // Locate Cargo.toml (either at root or inside top-level archive directory)
        let source_dir = if temp_extract_dir.join("Cargo.toml").exists() {
            temp_extract_dir.clone()
        } else {
            let entries = fs::read_dir(&temp_extract_dir).map_err(|e| {
                InstallError::IoError(format!("Failed to read temp extract dir: {}", e))
            })?;

            let mut found = None;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("Cargo.toml").exists() {
                    found = Some(path);
                    break;
                }
            }
            found.unwrap_or(temp_extract_dir.clone())
        };

        // Flatten contents directly into vendor_dir
        for entry in fs::read_dir(&source_dir)
            .map_err(|e| InstallError::IoError(format!("Failed to read source dir: {}", e)))?
            .flatten()
        {
            let path = entry.path();
            let file_name = path
                .file_name()
                .ok_or_else(|| InstallError::IoError("Invalid file name in archive".into()))?;
            let dest_path = vendor_dir.join(file_name);
            fs::rename(&path, &dest_path).map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to move vendored item {:?} -> {:?}: {}",
                    path, dest_path, e
                ))
            })?;
        }

        if let Err(e) = fs::remove_dir_all(&temp_extract_dir) {
            warn!(dir = ?temp_extract_dir, error = %e, "Failed to clean temporary cargo extract dir");
        }

        // Attempt to extract the official crates.io checksum from native Cargo.lock (or src-tauri/Cargo.lock) for absolute parity
        let mut crate_checksum = pkg.sha256.clone();
        let native_lock_path = if workspace_path.join("Cargo.lock").exists() {
            workspace_path.join("Cargo.lock")
        } else {
            workspace_path.join("src-tauri/Cargo.lock")
        };
        if native_lock_path.exists() {
            if let Ok(lock_content) = fs::read_to_string(&native_lock_path) {
                if let Ok(lock_toml) = lock_content.parse::<toml::Table>() {
                    if let Some(packages) = lock_toml.get("package").and_then(|p| p.as_array()) {
                        for p in packages {
                            if p.get("name").and_then(|n| n.as_str()) == Some(&pkg.name)
                                && p.get("version").and_then(|v| v.as_str()) == Some(&pkg.version)
                            {
                                if let Some(cs) = p.get("checksum").and_then(|c| c.as_str()) {
                                    crate_checksum = cs.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Compute per-file SHA-256 map and write the required .cargo-checksum.json file
        let mut file_hashes = BTreeMap::new();
        Self::collect_vendor_file_hashes(&vendor_dir, &vendor_dir, &mut file_hashes)?;
        let checksum_json = serde_json::json!({
            "package": crate_checksum,
            "files": file_hashes,
        });
        let checksum_path = vendor_dir.join(".cargo-checksum.json");
        let checksum_content = serde_json::to_string(&checksum_json).map_err(|e| {
            InstallError::MapperError(format!("Failed to serialize .cargo-checksum.json: {}", e))
        })?;
        fs::write(&checksum_path, checksum_content).map_err(|e| {
            InstallError::IoError(format!("Failed to write .cargo-checksum.json: {}", e))
        })?;

        // Ensure .cargo/config.toml points to the correct .cargo/vendor path without clobbering user config
        Self::ensure_vendored_cargo_config(&cargo_dir.join("config.toml"), ".cargo/vendor")?;

        // If this workspace is a Tauri project (with src-tauri/Cargo.toml), also configure
        // src-tauri/.cargo/config.toml with config-relative path `../.cargo/vendor`.
        if workspace_path.join("src-tauri/Cargo.toml").is_file() {
            let tauri_cargo_dir = workspace_path.join("src-tauri/.cargo");
            fs::create_dir_all(&tauri_cargo_dir).map_err(|e| {
                InstallError::IoError(format!("Failed to create src-tauri/.cargo: {}", e))
            })?;
            Self::ensure_vendored_cargo_config(
                &tauri_cargo_dir.join("config.toml"),
                "../.cargo/vendor",
            )?;
        }

        // Also ensure .cargo/vendor/config.json exists
        let vendor_root = cargo_dir.join("vendor");
        let vendor_config_json = vendor_root.join("config.json");
        if !vendor_config_json.exists() {
            fs::write(&vendor_config_json, r#"{"packages":{}}"#).map_err(|e| {
                InstallError::IoError(format!("Failed to write {:?}: {}", vendor_config_json, e))
            })?;
        }

        // Maintain a root 'vendor' symlink/alias for tools expecting workspace-root 'vendor/'
        let root_vendor = workspace_path.join("vendor");
        if !root_vendor.exists() {
            #[cfg(unix)]
            {
                if let Err(e) = std::os::unix::fs::symlink(".cargo/vendor", &root_vendor) {
                    warn!(error = %e, "Failed to create workspace root vendor symlink");
                }
            }
        }

        Ok(())
    }
}
