use crate::resolver::lockfile::DmzLockfile;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use sha2::{Sha256, Digest};

pub struct AirGapImporter;

impl AirGapImporter {
    /// Imports and verifies a Target C .tar.zst closure archive offline
    pub fn import_and_verify(archive_path: &Path, expected_sha256: &str, extract_to: &Path) -> Result<PathBuf, String> {
        // 1. Cryptographic SHA-256 verification
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

        // 2. Decompress and unpack .tar.zst archive offline
        fs::create_dir_all(extract_to).map_err(|e| e.to_string())?;
        let tar_file = File::open(archive_path).map_err(|e| e.to_string())?;
        let decoder = zstd::stream::Decoder::new(tar_file).map_err(|e| e.to_string())?;
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(extract_to).map_err(|e| e.to_string())?;

        // 3. Verify dmz.lock inside unpacked closure
        let lockfile_path = extract_to.join("dmz.lock");
        if lockfile_path.exists() {
            DmzLockfile::load_and_verify(&lockfile_path).map_err(|e| e.to_string())?;
        }

        Ok(extract_to.to_path_buf())
    }
}