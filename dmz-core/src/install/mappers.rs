use crate::resolver::lockfile::LockedPackage;
use flate2::read::GzDecoder;
use std::fs;
use std::io::Write;
use std::path::Path;
use tar::Archive;

/// Standard npm and Yarn classic mapper (Flat/nested node_modules)
pub struct NpmMapper;

impl NpmMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let target_dir = workspace_path.join("node_modules").join(&pkg.name);
        fs::create_dir_all(&target_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create node_modules dir: {}", e))
        })?;

        Self::unpack_archive(archive_path, &target_dir, &pkg.name)?;
        Self::link_package_binaries(workspace_path, &target_dir, &pkg.name)?;

        Ok(())
    }

    pub fn link_package_binaries(
        workspace_path: &Path,
        target_dir: &Path,
        pkg_name: &str,
    ) -> Result<(), super::InstallError> {
        let pkg_json_path = target_dir.join("package.json");
        if !pkg_json_path.exists() {
            return Ok(());
        }

        if let Ok(content) = fs::read_to_string(&pkg_json_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let bin_dir = workspace_path.join("node_modules").join(".bin");
                let _ = fs::create_dir_all(&bin_dir);

                let mut bin_map = std::collections::HashMap::new();
                if let Some(bin_obj) = json.get("bin").and_then(|b| b.as_object()) {
                    for (k, v) in bin_obj {
                        if let Some(path_str) = v.as_str() {
                            bin_map.insert(k.clone(), path_str.to_string());
                        }
                    }
                } else if let Some(bin_str) = json.get("bin").and_then(|b| b.as_str()) {
                    let short_name = pkg_name.split('/').next_back().unwrap_or(pkg_name);
                    bin_map.insert(short_name.to_string(), bin_str.to_string());
                }

                for (cmd_name, rel_target) in bin_map {
                    let source_file = target_dir.join(&rel_target);
                    if source_file.exists() {
                        let dest_link = bin_dir.join(&cmd_name);
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(metadata) = fs::metadata(&source_file) {
                                let mut perms = metadata.permissions();
                                perms.set_mode(0o755);
                                let _ = fs::set_permissions(&source_file, perms);
                            }
                            if dest_link.exists() || dest_link.is_symlink() {
                                let _ = fs::remove_file(&dest_link);
                            }
                            let _ = std::os::unix::fs::symlink(&source_file, &dest_link);
                        }
                        #[cfg(windows)]
                        {
                            let cmd_wrapper = bin_dir.join(format!("{}.cmd", cmd_name));
                            let script = format!(
                                "@\"%~dp0\\..\\{}\\{}\" %*\r\n",
                                pkg_name,
                                rel_target.replace('/', "\\")
                            );
                            let _ = fs::write(cmd_wrapper, script);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn unpack_archive(
        archive_path: &Path,
        target_dir: &Path,
        pkg_name: &str,
    ) -> Result<(), super::InstallError> {
        let file = fs::File::open(archive_path)
            .map_err(|e| super::InstallError::IoError(format!("Failed to open archive: {}", e)))?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);

        if archive.unpack(target_dir).is_err() {
            let file_retry = fs::File::open(archive_path).map_err(|e| {
                super::InstallError::IoError(format!("Failed to open archive retry: {}", e))
            })?;
            let mut plain_archive = Archive::new(file_retry);
            plain_archive.unpack(target_dir).map_err(|e| {
                super::InstallError::MapperError(format!(
                    "Failed to unpack package {}: {}",
                    pkg_name, e
                ))
            })?;
        }

        // If unpacked archive contains a top-level "package" directory (standard npm tarball structure),
        // flatten its contents into target_dir so that package.json is directly in target_dir.
        let package_subdir = target_dir.join("package");
        if package_subdir.is_dir() && !target_dir.join("package.json").exists() {
            if let Ok(entries) = fs::read_dir(&package_subdir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(file_name) = path.file_name() {
                        let dest_path = target_dir.join(file_name);
                        let _ = fs::rename(&path, &dest_path);
                    }
                }
            }
            let _ = fs::remove_dir_all(&package_subdir);
        }

        Ok(())
    }
}

/// pnpm Content-Addressable Virtual Store & Symlink Mapper
pub struct PnpmMapper;

impl PnpmMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let sanitized_name = pkg.name.replace('/', "+");
        let virtual_store_dir = workspace_path
            .join("node_modules")
            .join(".pnpm")
            .join(format!("{}@{}", sanitized_name, pkg.version))
            .join("node_modules")
            .join(&pkg.name);

        fs::create_dir_all(&virtual_store_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create pnpm virtual store dir: {}", e))
        })?;

        NpmMapper::unpack_archive(archive_path, &virtual_store_dir, &pkg.name)?;
        NpmMapper::link_package_binaries(workspace_path, &virtual_store_dir, &pkg.name)?;

        let root_link_dir = workspace_path.join("node_modules").join(&pkg.name);
        if let Some(parent) = root_link_dir.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                super::InstallError::IoError(format!(
                    "Failed to create root node_modules dir: {}",
                    e
                ))
            })?;
        }

        if root_link_dir.exists() || root_link_dir.is_symlink() {
            let _ = fs::remove_dir_all(&root_link_dir);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&virtual_store_dir, &root_link_dir).map_err(|e| {
                super::InstallError::MapperError(format!(
                    "Failed to create pnpm symlink for {}: {}",
                    pkg.name, e
                ))
            })?;
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_dir;
            if symlink_dir(&virtual_store_dir, &root_link_dir).is_err() {
                copy_dir_all(&virtual_store_dir, &root_link_dir).map_err(|e| {
                    super::InstallError::MapperError(format!(
                        "Failed to create pnpm directory link for {}: {}",
                        pkg.name, e
                    ))
                })?;
            }
        }

        Ok(())
    }
}

/// Cargo / Rust source vendor mapper
pub struct CargoMapper;

impl CargoMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let cargo_dir = workspace_path.join(".cargo");
        let vendor_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}", pkg.name, pkg.version));

        if vendor_dir.exists() {
            let _ = fs::remove_dir_all(&vendor_dir);
        }
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create cargo vendor dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open cargo archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);

        // Unpack into a temporary staging directory to handle archive nesting
        let temp_extract_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}-tmp", pkg.name, pkg.version));
        if temp_extract_dir.exists() {
            let _ = fs::remove_dir_all(&temp_extract_dir);
        }
        fs::create_dir_all(&temp_extract_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create temp extract dir: {}", e))
        })?;

        archive.unpack(&temp_extract_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Cargo package {}: {}",
                pkg.name, e
            ))
        })?;

        // Locate Cargo.toml (either at root or inside top-level archive directory)
        let source_dir = if temp_extract_dir.join("Cargo.toml").exists() {
            temp_extract_dir.clone()
        } else {
            let entries = fs::read_dir(&temp_extract_dir).map_err(|e| {
                super::InstallError::IoError(format!("Failed to read temp extract dir: {}", e))
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
            .map_err(|e| super::InstallError::IoError(format!("Failed to read source dir: {}", e)))?
            .flatten()
        {
            let path = entry.path();
            let file_name = path.file_name().ok_or_else(|| {
                super::InstallError::IoError("Invalid file name in archive".into())
            })?;
            let dest_path = vendor_dir.join(file_name);
            let _ = fs::rename(&path, &dest_path);
        }

        let _ = fs::remove_dir_all(&temp_extract_dir);

        // Attempt to extract the official crates.io checksum from native Cargo.lock for absolute parity
        let mut crate_checksum = pkg.sha256.clone();
        let native_lock_path = workspace_path.join("Cargo.lock");
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
            super::InstallError::IoError(format!("Failed to write .cargo-checksum.json: {}", e))
        })?;

        // Ensure .cargo/config.toml points to the correct .cargo/vendor path
        let config_path = cargo_dir.join("config.toml");
        let config_content = r#"[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = ".cargo/vendor"
"#;
        fs::write(&config_path, config_content).map_err(|e| {
            super::InstallError::IoError(format!("Failed to write .cargo/config.toml: {}", e))
        })?;

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

/// Python / PyPI site-packages mapper (Production-Grade)
pub struct PyPiMapper;

impl PyPiMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let venv_site_packages = workspace_path
            .join(".dmz")
            .join("venv")
            .join("lib")
            .join("python3.11")
            .join("site-packages");

        fs::create_dir_all(&venv_site_packages).map_err(|e| {
            super::InstallError::IoError(format!(
                "Failed to create python site-packages dir: {}",
                e
            ))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open Python package archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);

        // Unpacking fully preserves .dist-info and .egg-info metadata directories for importlib.metadata
        archive.unpack(&venv_site_packages).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Python package {}: {}",
                pkg.name, e
            ))
        })?;

        // Mirror/link into workspace .venv so virtual environment commands can locate dependencies directly
        let root_venv = workspace_path.join(".venv");
        let root_site_packages = root_venv
            .join("lib")
            .join("python3.11")
            .join("site-packages");
        let _ = fs::create_dir_all(&root_site_packages);
        let pth_file = root_site_packages.join("dmz_store.pth");
        let _ = fs::write(pth_file, format!("{}\n", venv_site_packages.display()));

        Ok(())
    }
}

/// Go module vendor mapper (Production-Grade)
pub struct GoMapper;

impl GoMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create go vendor dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open go package archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);
        archive.unpack(&vendor_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Go package {}: {}",
                pkg.name, e
            ))
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

/// PHP Composer vendor mapper
pub struct ComposerMapper;

impl ComposerMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create composer vendor dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open composer archive: {}", e))
        })?;
        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);
        archive.unpack(&vendor_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Composer package {}: {}",
                pkg.name, e
            ))
        })?;
        Ok(())
    }
}

/// Ruby Bundler vendor mapper (Production-Grade)
pub struct BundlerMapper;

impl BundlerMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let bundle_dir = workspace_path.join("vendor").join("bundle").join(&pkg.name);
        fs::create_dir_all(&bundle_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create bundle dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open gem archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);
        archive.unpack(&bundle_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
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

/// Java Maven/Gradle local repository mapper
pub struct MavenMapper;

impl MavenMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let m2_dir = workspace_path
            .join(".m2")
            .join("repository")
            .join(&pkg.name);
        fs::create_dir_all(&m2_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create maven repo dir: {}", e))
        })?;
        fs::copy(
            archive_path,
            m2_dir.join(format!("{}-{}.jar", pkg.name, pkg.version)),
        )
        .map_err(|e| super::InstallError::IoError(format!("Failed to copy jar: {}", e)))?;
        Ok(())
    }
}

#[allow(dead_code)]
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}
