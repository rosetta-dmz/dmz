use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::path::Path;

/// Ruby Bundler vendor mapper (Production-Grade)
pub struct BundlerMapper;

impl BundlerMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let bundle_dir = workspace_path.join("vendor").join("bundle").join(&pkg.name);
        fs::create_dir_all(&bundle_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create bundle dir: {}", e))
        })?;

        SafeArchiveExtractor::extract(archive_path, &bundle_dir).map_err(|e| {
            InstallError::MapperError(format!(
                "Failed to unpack Ruby gem {}: {}",
                pkg.name, e
            ))
        })?;

        // Automatically provision .bundle/config pointing Bundler to local vendor path
        let bundle_config_dir = workspace_path.join(".bundle");
        let _ = fs::create_dir_all(&bundle_config_dir);
        let config_path = bundle_config_dir.join("config");
        let config_content = r#"---
BUNDLE_PATH: "vendor/bundle"
BUNDLE_FROZEN: "true"
BUNDLE_LOCAL: "true"
"#;
        let _ = fs::write(config_path, config_content);

        Ok(())
    }
}

