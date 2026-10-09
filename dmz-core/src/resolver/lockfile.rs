use crate::resolver::manifest::Ecosystem;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub ecosystem: Ecosystem,
    pub sha256: String,
    pub source_url: Option<String>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmzLockfile {
    pub version: u32,
    pub closure_signature: String,
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Debug)]
pub enum LockError {
    IoError(String),
    SerializationError(String),
    IntegrityMismatch { expected: String, actual: String },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Lockfile IO Error: {}", msg),
            Self::SerializationError(msg) => write!(f, "Lockfile Serialization Error: {}", msg),
            Self::IntegrityMismatch { expected, actual } => write!(
                f,
                "Cryptographic Lock Mismatch! Expected sha256:{}, computed sha256:{}",
                expected, actual
            ),
        }
    }
}

impl std::error::Error for LockError {}

impl DmzLockfile {
    pub fn new(packages: Vec<LockedPackage>) -> Self {
        let mut package_map = BTreeMap::new();
        for pkg in packages {
            let key = format!("{}:{}", pkg.name, pkg.version);
            package_map.insert(key, pkg);
        }

        let closure_signature = Self::compute_closure_hash(&package_map);

        Self {
            version: 1,
            closure_signature,
            packages: package_map,
        }
    }

    /// Computes a deterministic SHA-256 signature across the entire ordered package map
    pub fn compute_closure_hash(packages: &BTreeMap<String, LockedPackage>) -> String {
        let mut hasher = Sha256::new();

        for (key, pkg) in packages {
            hasher.update(key.as_bytes());
            hasher.update(pkg.name.as_bytes());
            hasher.update(pkg.version.as_bytes());
            hasher.update(pkg.sha256.as_bytes());
        }

        format!("{:x}", hasher.finalize())
    }

    /// Saves the lockfile to `dmz.lock`
    pub fn save(&self, path: &Path) -> Result<(), LockError> {
        let json_data = serde_json::to_string_pretty(self)
            .map_err(|e| LockError::SerializationError(e.to_string()))?;

        fs::write(path, json_data)
            .map_err(|e| LockError::IoError(format!("Failed to write lockfile: {}", e)))
    }

    /// Loads and verifies cryptographic integrity of `dmz.lock`
    pub fn load_and_verify(path: &Path) -> Result<Self, LockError> {
        let content = fs::read_to_string(path)
            .map_err(|e| LockError::IoError(format!("Failed to read lockfile: {}", e)))?;

        let lockfile: Self = serde_json::from_str(&content)
            .map_err(|e| LockError::SerializationError(e.to_string()))?;

        let computed_signature = Self::compute_closure_hash(&lockfile.packages);

        if computed_signature != lockfile.closure_signature {
            return Err(LockError::IntegrityMismatch {
                expected: lockfile.closure_signature,
                actual: computed_signature,
            });
        }

        Ok(lockfile)
    }

    /// Helper to compute SHA-256 digest of raw binary closure files
    pub fn hash_bytes(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }
}