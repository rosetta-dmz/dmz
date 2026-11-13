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
        let repo_root = workspace_path.join(".m2").join("repository");
        let (m2_dir, jar_name) = if let Some((group_id, artifact_id)) = pkg
            .name
            .split_once(':')
            .or_else(|| pkg.name.split_once('/'))
        {
            let mut dir = repo_root;
            for part in group_id.split('.').filter(|p| !p.is_empty()) {
                dir = dir.join(part);
            }
            (
                dir.join(artifact_id).join(&pkg.version),
                format!("{}-{}.jar", artifact_id, pkg.version),
            )
        } else {
            let safe_artifact = pkg.name.replace(['/', '\\'], "-");
            (
                repo_root.join(&safe_artifact).join(&pkg.version),
                format!("{}-{}.jar", safe_artifact, pkg.version),
            )
        };
        fs::create_dir_all(&m2_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create maven repo dir: {}", e))
        })?;
        let dest = m2_dir.join(jar_name);
        fs::copy(archive_path, &dest)
            .map_err(|e| InstallError::IoError(format!("Failed to copy jar: {}", e)))?;
        Ok(())
    }
}
