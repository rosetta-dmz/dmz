use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::path::Path;

/// PHP Composer vendor mapper
pub struct ComposerMapper;

impl ComposerMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create composer vendor dir: {}", e))
        })?;

        SafeArchiveExtractor::extract(archive_path, &vendor_dir).map_err(|e| {
            InstallError::MapperError(format!(
                "Failed to unpack Composer package {}: {}",
                pkg.name, e
            ))
        })?;
        Ok(())
    }
}
