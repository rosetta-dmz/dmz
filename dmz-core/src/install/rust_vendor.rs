use crate::install::archive::SafeArchiveExtractor;
use crate::install::fast_clone::FastClone;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use tracing::{info, warn};

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

    /// Validate store-relative path stays safely within the store root
    pub fn is_safe_store_path(path: &Path) -> bool {
        if path.is_absolute() {
            return false;
        }
        let mut depth: usize = 0;
        for c in path.components() {
            match c {
                Component::Normal(_) => {
                    depth = depth.saturating_add(1);
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                Component::RootDir | Component::Prefix(_) => {
                    return false;
                }
            }
        }
        true
    }

    /// Safely purge existing directory, clearing read-only attributes on Windows if necessary
    fn safe_remove_dir_all(dir: &Path) -> io::Result<()> {
        if dir.exists() {
            #[cfg(windows)]
            {
                // Clear read-only flags on Windows to prevent ERROR_ACCESS_DENIED
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if let Ok(meta) = fs::metadata(&p) {
                            if meta.permissions().readonly() {
                                let mut perms = meta.permissions();
                                perms.set_readonly(false);
                                let _ = fs::set_permissions(&p, perms);
                            }
                        }
                    }
                }
            }
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn materialize(&self, lockfile_path: &Path) -> io::Result<()> {
        let lock_content = fs::read_to_string(lockfile_path)?;

        // Support modern JSON DmzLockfile natively via unified installer engine
        if serde_json::from_str::<crate::resolver::lockfile::DmzLockfile>(&lock_content).is_ok() {
            let config = crate::install::InstallConfig {
                workspace_path: self.workspace_root.clone(),
                lockfile_path: lockfile_path.to_path_buf(),
                store_dir: self.offline_store.clone(),
            };
            crate::install::InstallerEngine::install(&config)
                .map_err(|e| io::Error::other(e.to_string()))?;
            return Ok(());
        }

        let lockfile: DmzLockfile = toml::from_str(&lock_content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let cargo_dir = self.workspace_root.join(".cargo");
        let vendor_dir = cargo_dir.join("vendor");

        Self::safe_remove_dir_all(&vendor_dir)?;
        fs::create_dir_all(&vendor_dir)?;

        let native_lock_path = self.workspace_root.join("Cargo.lock");
        if native_lock_path.exists() {
            let _ = fs::remove_file(&native_lock_path);
        }

        let mut vendor_config_sources = std::collections::BTreeMap::new();

        for c in &lockfile.crates {
            // Path traversal defense on crate metadata
            if !Self::is_safe_store_path(Path::new(&c.name))
                || !Self::is_safe_store_path(&c.store_path)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Security violation: Invalid crate name or store path in lockfile: {:?}",
                        c.store_path
                    ),
                ));
            }

            let crate_folder_name = format!("{}-{}", c.name, c.version);
            let target_crate_dir = vendor_dir.join(&crate_folder_name);

            let source_path = self.offline_store.join(&c.store_path);
            if source_path.is_dir() {
                // Use FastClone (CoW reflinks/hardlinks)
                FastClone::clone_dir_recursive(&source_path, &target_crate_dir)?;
            } else if source_path.is_file() {
                // Use SafeArchiveExtractor (Zip Slip & quota defense)
                SafeArchiveExtractor::extract(&source_path, &target_crate_dir)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Crate {}:{} not found in offline store at {:?}",
                        c.name, c.version, source_path
                    ),
                ));
            }

            let checksum_content = format!(r#"{{"package": "{}", "files": {{}}}}"#, c.checksum);
            fs::write(
                target_crate_dir.join(".cargo-checksum.json"),
                checksum_content,
            )?;

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
directory = ".cargo/vendor"
"#;
        fs::write(cargo_dir.join("config.toml"), config_toml_content.trim())?;

        // Synchronize Cargo's native lockfile offline with explicit error propagation,
        // preferring the bundled workspace Rust toolchain when available.
        let bundled_cargo_name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let bundled_cargo = self
            .workspace_root
            .join(".dmz/toolchains/rust/bin")
            .join(bundled_cargo_name);
        let cargo_bin = if bundled_cargo.is_file() {
            bundled_cargo
        } else {
            std::path::PathBuf::from("cargo")
        };

        let mut cmd = std::process::Command::new(&cargo_bin);
        cmd.arg("generate-lockfile")
            .arg("--offline")
            .current_dir(&self.workspace_root);
        let bundled_rust_bin = self.workspace_root.join(".dmz/toolchains/rust/bin");
        if bundled_rust_bin.is_dir() {
            let existing_path = std::env::var("PATH").unwrap_or_default();
            let sep = if cfg!(windows) { ";" } else { ":" };
            cmd.env(
                "PATH",
                format!("{}{}{}", bundled_rust_bin.display(), sep, existing_path),
            );
        }
        let spawn_res = cmd.status();

        match spawn_res {
            Ok(status) => {
                if !status.success() {
                    return Err(io::Error::other(
                        "Failed to synchronize Cargo lockfile during vendor materialization",
                    ));
                }
            }
            Err(e) => {
                warn!(error = %e, "Cargo command execution failed during lockfile synchronization");
                return Err(io::Error::other(format!(
                    "Failed to execute cargo generate-lockfile: {}",
                    e
                )));
            }
        }

        info!(
            crates_count = lockfile.crates.len(),
            vendor_dir = ?vendor_dir,
            "Successfully materialized scoped Rust dependencies into .cargo/vendor"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_safe_store_path() {
        assert!(RustVendorMaterializer::is_safe_store_path(Path::new(
            "serde/serde-1.0.archive"
        )));
        assert!(RustVendorMaterializer::is_safe_store_path(Path::new(
            "tokio/1.0/lib"
        )));
        assert!(RustVendorMaterializer::is_safe_store_path(Path::new(
            "./sub/path"
        )));
        assert!(!RustVendorMaterializer::is_safe_store_path(Path::new(
            "/etc/shadow"
        )));
        assert!(!RustVendorMaterializer::is_safe_store_path(Path::new(
            "../outside"
        )));
        assert!(!RustVendorMaterializer::is_safe_store_path(Path::new(
            "pkg/../../outside"
        )));
    }

    #[test]
    fn test_materializer_rejects_traversal_crate_name() {
        let temp = tempdir().unwrap();
        let ws = temp.path().join("ws");
        let store = temp.path().join("store");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&store).unwrap();

        let lock_file = ws.join("dmz.lock");
        let traversal_toml = r#"
[[crates]]
name = "../traversal"
version = "1.0.0"
checksum = "12345"
store_path = "traversal/pkg.tar.gz"
"#;
        fs::write(&lock_file, traversal_toml).unwrap();

        let mat = RustVendorMaterializer::new(ws, store);
        let res = mat.materialize(&lock_file);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Security violation"));
    }
}
