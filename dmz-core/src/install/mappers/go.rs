use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Go module vendor mapper
pub struct GoMapper;

impl GoMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        fs::create_dir_all(&vendor_dir)
            .map_err(|e| InstallError::IoError(format!("Failed to create go vendor dir: {}", e)))?;

        SafeArchiveExtractor::extract(archive_path, &vendor_dir).map_err(|e| {
            InstallError::MapperError(format!("Failed to unpack Go package {}: {}", pkg.name, e))
        })?;

        // Automatically maintain vendor/modules.txt required by Go compiler for offline builds
        let modules_txt_path = workspace_path.join("vendor").join("modules.txt");
        let entry = format!("# {} {}\n$ explicit\n{}\n", pkg.name, pkg.version, pkg.name);
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(modules_txt_path)
        {
            let _ = file.write_all(entry.as_bytes());
        }

        Ok(())
    }
}
