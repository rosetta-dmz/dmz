use crate::resolver::lockfile::DmzLockfile;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};

pub struct AirGapImporter;

impl AirGapImporter {
    fn is_safe_relative_path(path: &Path) -> bool {
        !path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    }

    /// Imports and verifies a Target C .tar.zst closure archive offline
    pub fn import_and_verify(
        archive_path: &Path,
        expected_sha256: &str,
        extract_to: &Path,
    ) -> Result<PathBuf, String> {
        // Cryptographic SHA-256 verification
        let mut file = File::open(archive_path).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
        let computed_hash = format!("{:x}", hasher.finalize());

        if computed_hash != expected_sha256 {
            return Err(format!(
                "Air-gap checksum verification failed! Expected: {}, Got: {}",
                expected_sha256, computed_hash
            ));
        }

        // Decompress and unpack .tar.zst archive offline with Zip-Slip & symlink traversal guards
        fs::create_dir_all(extract_to).map_err(|e| e.to_string())?;
        let tar_file = File::open(archive_path).map_err(|e| e.to_string())?;
        let decoder = zstd::stream::Decoder::new(tar_file).map_err(|e| e.to_string())?;
        let mut archive = tar::Archive::new(decoder);

        let entries = archive.entries().map_err(|e| e.to_string())?;
        for entry_res in entries {
            let mut entry = entry_res.map_err(|e| e.to_string())?;
            let entry_path = entry.path().map_err(|e| e.to_string())?.into_owned();

            if !Self::is_safe_relative_path(&entry_path) {
                return Err(format!(
                    "Security error: Archive entry contains unsafe path traversal component: {:?}",
                    entry_path
                ));
            }

            if let Ok(Some(link_target)) = entry.link_name() {
                if !Self::is_safe_relative_path(&link_target) {
                    return Err(format!(
                        "Security error: Archive link target escapes extraction directory: {:?}",
                        link_target
                    ));
                }
            }

            let unpacked = entry.unpack_in(extract_to).map_err(|e| e.to_string())?;
            if !unpacked {
                return Err(format!(
                    "Security error: Archive entry refused by unpack_in boundary check: {:?}",
                    entry_path
                ));
            }
        }

        // Verify dmz.lock inside unpacked closure
        let lockfile_path = extract_to.join("dmz.lock");
        if lockfile_path.exists() {
            DmzLockfile::load_and_verify(&lockfile_path).map_err(|e| e.to_string())?;
        }

        Ok(extract_to.to_path_buf())
    }
}
