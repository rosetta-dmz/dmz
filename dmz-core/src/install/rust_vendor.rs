use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
pub struct LockedCrate {
    pub name: String,
    pub version: String,
    pub checksum: String,
    pub store_path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct DmzLockfile {
    pub crates: Vec<LockedCrate>,
}

pub struct RustVendorMaterializer {
    workspace_root: PathBuf,
    offline_store: PathBuf,
}

impl RustVendorMaterializer {
    pub fn new(workspace_root: PathBuf, offline_store: PathBuf) -> Self {
        Self {
            workspace_root,
            offline_store,
        }
    }

    pub fn materialize(&self, lockfile_path: &Path) -> io::Result<()> {
        let lock_content = fs::read_to_string(lockfile_path)?;
        let lockfile: DmzLockfile = toml::from_str(&lock_content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let cargo_dir = self.workspace_root.join(".cargo");
        let vendor_dir = cargo_dir.join("vendor");

        if vendor_dir.exists() {
            fs::remove_dir_all(&vendor_dir)?;
        }
        fs::create_dir_all(&vendor_dir)?;

        let native_lock_path = self.workspace_root.join("Cargo.lock");
        if native_lock_path.exists() {
            fs::remove_file(&native_lock_path)?;
        }

        let mut vendor_config_sources = std::collections::BTreeMap::new();

        for c in &lockfile.crates {
            let crate_folder_name = format!("{}-{}", c.name, c.version);
            let target_crate_dir = vendor_dir.join(&crate_folder_name);

            let source_path = self.offline_store.join(&c.store_path);
            if source_path.is_dir() {
                copy_dir_all(&source_path, &target_crate_dir)?;
            } else if source_path.exists() {
                unpack_crate_tarball(&source_path, &target_crate_dir)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Crate {}:{} not found in offline store at {:?}",
                        c.name, c.version, source_path
                    ),
                ));
            }

            vendor_config_sources.insert(
                crate_folder_name,
                serde_json::json!({
                    "files": {},
                    "package": c.checksum
                }),
            );
        }

        let mut registry_config = std::collections::BTreeMap::new();
        registry_config.insert(
            "packages".to_string(),
            serde_json::Value::Object(vendor_config_sources.into_iter().collect()),
        );

        fs::write(
            vendor_dir.join("config.json"),
            serde_json::to_string_pretty(&registry_config)?,
        )?;

        let config_toml_content = r#"
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
"#;
        fs::write(cargo_dir.join("config.toml"), config_toml_content.trim())?;

        // Synchronize Cargo's native lockfile offline with the vendored sources
        let status = std::process::Command::new("cargo")
            .arg("generate-lockfile")
            .arg("--offline")
            .current_dir(&self.workspace_root)
            .status();

        if let Ok(status) = status {
            if !status.success() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Failed to synchronize Cargo lockfile during vendor materialization",
                ));
            }
        }

        println!(
            "[dmz install] Successfully materialized scoped Rust dependencies into .cargo/vendor"
        );
        Ok(())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

fn unpack_crate_tarball(archive_path: &Path, target_dir: &Path) -> io::Result<()> {
    let tar_file = File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(tar_file);
    let mut archive = tar::Archive::new(decoder);
    fs::create_dir_all(target_dir)?;
    archive.unpack(target_dir)?;
    Ok(())
}
