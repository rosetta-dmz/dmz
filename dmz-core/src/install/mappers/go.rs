use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::io::Write;
use std::path::Path;
use tracing::warn;

/// Go module vendor mapper
pub struct GoMapper;

impl GoMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        if vendor_dir.exists() {
            fs::remove_dir_all(&vendor_dir).map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to reset existing Go vendor dir {:?}: {}",
                    vendor_dir, e
                ))
            })?;
        }
        fs::create_dir_all(&vendor_dir)
            .map_err(|e| InstallError::IoError(format!("Failed to create go vendor dir: {}", e)))?;

        // Extract zip into temporary staging folder so Go proxy `<module>@<version>/` prefixes
        // can be stripped cleanly before materializing `vendor/<module>/`.
        let safe_tmp = pkg.name.replace('/', "__");
        let tmp_dir = workspace_path
            .join(".dmz")
            .join(format!("go-tmp-{}-{}", safe_tmp, pkg.version));
        if tmp_dir.exists() {
            fs::remove_dir_all(&tmp_dir).map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to reset Go staging dir {:?}: {}",
                    tmp_dir, e
                ))
            })?;
        }
        fs::create_dir_all(&tmp_dir).map_err(|e| {
            InstallError::IoError(format!(
                "Failed to create Go staging dir {:?}: {}",
                tmp_dir, e
            ))
        })?;

        SafeArchiveExtractor::extract(archive_path, &tmp_dir).map_err(|e| {
            InstallError::MapperError(format!("Failed to unpack Go package {}: {}", pkg.name, e))
        })?;

        let prefixed_root = tmp_dir.join(format!("{}@{}", pkg.name, pkg.version));
        let source_root = if prefixed_root.is_dir() {
            prefixed_root
        } else {
            tmp_dir.clone()
        };

        crate::install::fast_clone::FastClone::clone_dir_recursive(&source_root, &vendor_dir)
            .map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to copy Go module into vendor dir {:?}: {}",
                    vendor_dir, e
                ))
            })?;

        if let Err(e) = fs::remove_dir_all(&tmp_dir) {
            warn!(dir = ?tmp_dir, error = %e, "Failed to clean temporary Go staging dir");
        }

        // Automatically maintain deduplicated vendor/modules.txt required by Go compiler for offline builds
        let modules_txt_path = workspace_path.join("vendor").join("modules.txt");
        let header_line = format!("# {} {}", pkg.name, pkg.version);
        let already_recorded = if modules_txt_path.is_file() {
            fs::read_to_string(&modules_txt_path)
                .map(|c| c.lines().any(|line| line.trim() == header_line))
                .unwrap_or(false)
        } else {
            false
        };

        if !already_recorded {
            let entry = format!("{}\n## explicit\n{}\n", header_line, pkg.name);
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&modules_txt_path)
                .map_err(|e| {
                    InstallError::IoError(format!("Failed to open vendor/modules.txt: {}", e))
                })?;
            file.write_all(entry.as_bytes()).map_err(|e| {
                InstallError::IoError(format!("Failed to write vendor/modules.txt: {}", e))
            })?;
        }

        Ok(())
    }
}
