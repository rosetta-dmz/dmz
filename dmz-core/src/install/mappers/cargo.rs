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

    /// Reads the configured `[source.vendored-sources].directory` value from a `.cargo/config.toml` file.
    pub(crate) fn read_vendored_source_directory(config_path: &Path) -> Option<String> {
        let existing = fs::read_to_string(config_path).ok()?;
        let table = existing.parse::<toml::Table>().ok()?;
        table
            .get("source")
            .and_then(|s| s.get("vendored-sources"))
            .and_then(|v| v.get("directory"))
            .and_then(|d| d.as_str())
            .map(ToOwned::to_owned)
    }

    /// Ensures `config_path` replaces `crates-io` with `[source.vendored-sources]` pointing
    /// cleanly at `vendor_rel_dir`, preserving any unrelated user TOML configuration tables.
    pub(crate) fn ensure_vendored_cargo_config(
        config_path: &Path,
        vendor_rel_dir: &str,
    ) -> Result<(), InstallError> {
        let mut root_table = if config_path.exists() {
            let existing = fs::read_to_string(config_path).map_err(|e| {
                InstallError::IoError(format!("Failed to read {:?}: {}", config_path, e))
            })?;
            existing
                .parse::<toml::Table>()
                .unwrap_or_else(|_| toml::Table::new())
        } else {
            toml::Table::new()
        };

        let existing_dir = root_table
            .get("source")
            .and_then(|s| s.get("vendored-sources"))
            .and_then(|v| v.get("directory"))
            .and_then(|d| d.as_str())
            .map(ToOwned::to_owned);
        let existing_replace = root_table
            .get("source")
            .and_then(|s| s.get("crates-io"))
            .and_then(|c| c.get("replace-with"))
            .and_then(|r| r.as_str())
            .map(ToOwned::to_owned);

        if config_path.exists()
            && existing_replace.as_deref() == Some("vendored-sources")
            && existing_dir.as_deref() == Some(vendor_rel_dir)
        {
            return Ok(());
        }

        let source_val = root_table
            .entry("source")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !source_val.is_table() {
            *source_val = toml::Value::Table(toml::Table::new());
        }
        if let Some(source_tbl) = source_val.as_table_mut() {
            let crates_io_val = source_tbl
                .entry("crates-io")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !crates_io_val.is_table() {
                *crates_io_val = toml::Value::Table(toml::Table::new());
            }
            if let Some(crates_tbl) = crates_io_val.as_table_mut() {
                crates_tbl.insert(
                    "replace-with".to_string(),
                    toml::Value::String("vendored-sources".to_string()),
                );
            }

            let vendored_val = source_tbl
                .entry("vendored-sources")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !vendored_val.is_table() {
                *vendored_val = toml::Value::Table(toml::Table::new());
            }
            if let Some(vendored_tbl) = vendored_val.as_table_mut() {
                vendored_tbl.insert(
                    "directory".to_string(),
                    toml::Value::String(vendor_rel_dir.to_string()),
                );
            }
        }

        let rendered = toml::to_string_pretty(&root_table).map_err(|e| {
            InstallError::MapperError(format!("Failed to serialize {:?}: {}", config_path, e))
        })?;
        crate::workspace::atomic_write_file(config_path, rendered.as_bytes())
            .map_err(|e| InstallError::IoError(format!("Failed to write {:?}: {}", config_path, e)))
    }

    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        Self::map_package_excluding(workspace_path, pkg, archive_path, &[])
    }

    pub fn map_package_excluding(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
        excluded_dirs: &[std::path::PathBuf],
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

        // Attempt to extract the official crates.io checksum from any discovered native Cargo.lock
        let discovered_cargo_dirs =
            crate::workspace::discover_tauri_cargo_dirs_excluding(workspace_path, excluded_dirs);
        let mut crate_checksum = pkg.sha256.clone();
        'lock_search: for cargo_root in &discovered_cargo_dirs {
            let native_lock_path = cargo_root.join("Cargo.lock");
            if !native_lock_path.is_file() {
                continue;
            }
            if let Ok(lock_content) = fs::read_to_string(&native_lock_path) {
                if let Ok(lock_toml) = lock_content.parse::<toml::Table>() {
                    if let Some(packages) = lock_toml.get("package").and_then(|p| p.as_array()) {
                        for p in packages {
                            if p.get("name").and_then(|n| n.as_str()) == Some(&pkg.name)
                                && p.get("version").and_then(|v| v.as_str()) == Some(&pkg.version)
                            {
                                if let Some(cs) = p.get("checksum").and_then(|c| c.as_str()) {
                                    crate_checksum = cs.to_string();
                                    break 'lock_search;
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

        // Configure vendoring for root, src-tauri, and any nested Tauri subprojects (excluding sibling members).
        let vendor_root = cargo_dir.join("vendor");
        Self::ensure_vendored_cargo_config(&cargo_dir.join("config.toml"), ".cargo/vendor")?;

        for tauri_dir in discovered_cargo_dirs {
            if tauri_dir == workspace_path {
                continue;
            }
            let tauri_cargo_dir = tauri_dir.join(".cargo");
            fs::create_dir_all(&tauri_cargo_dir).map_err(|e| {
                InstallError::IoError(format!("Failed to create {:?}: {}", tauri_cargo_dir, e))
            })?;
            let rel_vendor = crate::archive::importer::hydration::relative_path_between(
                &tauri_dir,
                &vendor_root,
            );
            Self::ensure_vendored_cargo_config(&tauri_cargo_dir.join("config.toml"), &rel_vendor)?;
        }

        // Also ensure .cargo/vendor/config.json exists
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
