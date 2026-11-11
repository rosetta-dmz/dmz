use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::path::Path;

/// Java Maven/Gradle local repository mapper
pub struct MavenMapper;

impl MavenMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let m2_dir = workspace_path
            .join(".m2")
            .join("repository")
            .join(&pkg.name);
        fs::create_dir_all(&m2_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create maven repo dir: {}", e))
        })?;
        let dest = m2_dir.join(format!("{}-{}.jar", pkg.name, pkg.version));
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::copy(archive_path, &dest)
            .map_err(|e| InstallError::IoError(format!("Failed to copy jar: {}", e)))?;
        Ok(())
    }
}
