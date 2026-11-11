use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::path::Path;

/// Cargo / Rust source vendor mapper
pub struct CargoMapper;

impl CargoMapper {
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
            let _ = fs::remove_dir_all(&vendor_dir);
        }
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create cargo vendor dir: {}", e))
        })?;

        // Unpack into a temporary staging directory to handle archive nesting
        let temp_extract_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}-tmp", pkg.name, pkg.version));
        if temp_extract_dir.exists() {
            let _ = fs::remove_dir_all(&temp_extract_dir);
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
            let file_name = path.file_name().ok_or_else(|| {
                InstallError::IoError("Invalid file name in archive".into())
            })?;
            let dest_path = vendor_dir.join(file_name);
            let _ = fs::rename(&path, &dest_path);
        }

        let _ = fs::remove_dir_all(&temp_extract_dir);

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

        // Write the required .cargo-checksum.json file
        let checksum_path = vendor_dir.join(".cargo-checksum.json");
        let checksum_content = format!(r#"{{"package": "{}", "files": {{}}}}"#, crate_checksum);
        fs::write(&checksum_path, checksum_content).map_err(|e| {
            InstallError::IoError(format!("Failed to write .cargo-checksum.json: {}", e))
        })?;

        // Ensure .cargo/config.toml points to the correct .cargo/vendor path
        let config_path = cargo_dir.join("config.toml");
        let config_content = r#"[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = ".cargo/vendor"
"#;
        fs::write(&config_path, config_content).map_err(|e| {
            InstallError::IoError(format!("Failed to write .cargo/config.toml: {}", e))
        })?;

        // If this workspace is a Tauri project (with src-tauri/Cargo.toml), also configure
        // src-tauri/.cargo/config.toml with config-relative path `../.cargo/vendor`.
        if workspace_path.join("src-tauri/Cargo.toml").is_file() {
            let tauri_cargo_dir = workspace_path.join("src-tauri/.cargo");
            if fs::create_dir_all(&tauri_cargo_dir).is_ok() {
                let tauri_config_content = r#"[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "../.cargo/vendor"
"#;
                let _ = fs::write(tauri_cargo_dir.join("config.toml"), tauri_config_content);
            }
        }

        // Also ensure .cargo/vendor/config.json exists
        let vendor_root = cargo_dir.join("vendor");
        let vendor_config_json = vendor_root.join("config.json");
        if !vendor_config_json.exists() {
            let _ = fs::write(vendor_config_json, r#"{"packages":{}}"#);
        }

        // Maintain a root 'vendor' symlink/alias for tools expecting workspace-root 'vendor/'
        let root_vendor = workspace_path.join("vendor");
        if !root_vendor.exists() {
            #[cfg(unix)]
            {
                let _ = std::os::unix::fs::symlink(".cargo/vendor", &root_vendor);
            }
        }

        Ok(())
    }
}

