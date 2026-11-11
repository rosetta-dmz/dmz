use crate::resolver::manifest::Ecosystem;
use crate::vcs::VcsProvenance;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

pub const MAX_LOCKFILE_SIZE: u64 = 16 * 1024 * 1024;

pub trait LockfileReader {
    fn load_and_verify(path: &Path) -> Result<DmzLockfile, LockError>;
}

pub trait LockfileWriter {
    fn save(&self, path: &Path) -> Result<(), LockError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub ecosystem: Ecosystem,
    pub sha256: String,
    pub source_url: Option<String>,
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub id: String,
    pub path: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmzLockfile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsProvenance>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workspaces: BTreeMap<String, WorkspaceMetadata>,
    pub closure_signature: String,
    pub packages: BTreeMap<String, LockedPackage>,
}

impl Default for DmzLockfile {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[derive(Debug)]
pub enum LockError {
    Io(std::io::Error),
    Serialization(serde_json::Error),
    IntegrityMismatch { expected: String, actual: String },
    LockfileTooLarge { max_bytes: u64, actual_bytes: u64 },
    IoError(String),
    SerializationError(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "Lockfile IO Error: {}", err),
            Self::Serialization(err) => write!(f, "Lockfile Serialization Error: {}", err),
            Self::IntegrityMismatch { expected, actual } => write!(
                f,
                "Cryptographic Lock Mismatch! Expected sha256:{}, computed sha256:{}",
                expected, actual
            ),
            Self::LockfileTooLarge {
                max_bytes,
                actual_bytes,
            } => write!(
                f,
                "Lockfile exceeds maximum allowable size of {} bytes (found {} bytes)",
                max_bytes, actual_bytes
            ),
            Self::IoError(msg) => write!(f, "Lockfile IO Error: {}", msg),
            Self::SerializationError(msg) => write!(f, "Lockfile Serialization Error: {}", msg),
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Serialization(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LockError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for LockError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}

impl LockfileReader for DmzLockfile {
    fn load_and_verify(path: &Path) -> Result<Self, LockError> {
        Self::load_and_verify(path)
    }
}

impl LockfileWriter for DmzLockfile {
    fn save(&self, path: &Path) -> Result<(), LockError> {
        self.save(path)
    }
}

impl DmzLockfile {
    pub fn new(packages: Vec<LockedPackage>) -> Self {
        Self::with_platform(packages, None)
    }

    pub fn with_platform(packages: Vec<LockedPackage>, target_platform: Option<String>) -> Self {
        let mut package_map = BTreeMap::new();
        for pkg in packages {
            let base_key = format!("{}:{}", pkg.name, pkg.version);
            let key = if let Some(existing) = package_map.get(&base_key) {
                let existing_pkg: &LockedPackage = existing;
                if existing_pkg.member_id != pkg.member_id {
                    if let Some(ref mid) = pkg.member_id {
                        format!("{}@{}", mid, base_key)
                    } else {
                        base_key
                    }
                } else {
                    base_key
                }
            } else {
                base_key
            };
            package_map.insert(key, pkg);
        }

        let closure_signature = Self::compute_closure_hash(&package_map);

        Self {
            version: 1,
            target_platform,
            vcs: None,
            workspaces: BTreeMap::new(),
            closure_signature,
            packages: package_map,
        }
    }

    pub fn with_vcs(mut self, vcs: Option<VcsProvenance>) -> Self {
        self.vcs = vcs;
        self
    }

    pub fn with_workspaces(mut self, workspaces: BTreeMap<String, WorkspaceMetadata>) -> Self {
        self.workspaces = workspaces;
        self
    }

    pub fn add_workspace(&mut self, meta: WorkspaceMetadata) {
        self.workspaces.insert(meta.id.clone(), meta);
    }

    /// Computes a deterministic SHA-256 signature across the entire ordered package map
    pub fn compute_closure_hash(packages: &BTreeMap<String, LockedPackage>) -> String {
        let mut hasher = Sha256::new();

        for (key, pkg) in packages {
            hasher.update(key.as_bytes());
            hasher.update(pkg.name.as_bytes());
            hasher.update(pkg.version.as_bytes());
            hasher.update(pkg.sha256.as_bytes());
            if let Some(ref mid) = pkg.member_id {
                hasher.update(mid.as_bytes());
            }
        }

        format!("{:x}", hasher.finalize())
    }

    /// Saves the lockfile to `dmz.lock` atomically
    pub fn save(&self, path: &Path) -> Result<(), LockError> {
        let json_data = serde_json::to_string_pretty(self)?;
        crate::workspace::atomic_write_file(path, json_data)?;
        Ok(())
    }

    /// Loads and verifies cryptographic integrity of `dmz.lock`
    pub fn load_and_verify(path: &Path) -> Result<Self, LockError> {
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() {
            return Err(LockError::Io(std::io::Error::other(
                format!("Path {:?} is a directory, expected lockfile", path),
            )));
        }
        if metadata.len() > MAX_LOCKFILE_SIZE {
            return Err(LockError::LockfileTooLarge {
                max_bytes: MAX_LOCKFILE_SIZE,
                actual_bytes: metadata.len(),
            });
        }

        let mut reader = file.take(MAX_LOCKFILE_SIZE + 1);
        let mut content = String::new();
        reader.read_to_string(&mut content)?;
        if content.len() as u64 > MAX_LOCKFILE_SIZE {
            return Err(LockError::LockfileTooLarge {
                max_bytes: MAX_LOCKFILE_SIZE,
                actual_bytes: content.len() as u64,
            });
        }

        let lockfile: Self = serde_json::from_str(&content)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_workspace_lockfile_compatibility() {
        let pkg1 = LockedPackage {
            name: "express".to_string(),
            version: "4.19.2".to_string(),
            ecosystem: Ecosystem::Npm,
            sha256: "a1b2c3d4".to_string(),
            source_url: None,
            dependencies: vec![],
            member_id: Some("frontend".to_string()),
        };

        let pkg2 = LockedPackage {
            name: "tokio".to_string(),
            version: "1.38.0".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: "e5f6a7b8".to_string(),
            source_url: None,
            dependencies: vec![],
            member_id: Some("sidecar".to_string()),
        };

        let mut lockfile = DmzLockfile::new(vec![pkg1, pkg2]);
        lockfile.add_workspace(WorkspaceMetadata {
            id: "frontend".to_string(),
            path: "./apps/web".to_string(),
            role: "primary".to_string(),
            ecosystem: Some("npm".to_string()),
            closure_signature: None,
        });
        lockfile.add_workspace(WorkspaceMetadata {
            id: "sidecar".to_string(),
            path: "../engine-sidecar".to_string(),
            role: "sidecar".to_string(),
            ecosystem: Some("cargo".to_string()),
            closure_signature: None,
        });

        assert_eq!(lockfile.workspaces.len(), 2);
        assert_eq!(lockfile.packages.len(), 2);

        // Test serialization / deserialization roundtrip
        let serialized = serde_json::to_string_pretty(&lockfile).unwrap();
        let deserialized: DmzLockfile = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.workspaces.len(), 2);
        assert_eq!(deserialized.packages.len(), 2);
        assert_eq!(
            deserialized.closure_signature,
            DmzLockfile::compute_closure_hash(&deserialized.packages)
        );
    }

    #[test]
    fn test_lockfile_save_load_and_integrity_verification() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dmz.lock");

        let pkg = LockedPackage {
            name: "test-pkg".to_string(),
            version: "1.0.0".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: "fake-sha".to_string(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };
        let lockfile = DmzLockfile::new(vec![pkg]);
        lockfile.save(&path).unwrap();

        // Successful verified load
        let loaded = DmzLockfile::load_and_verify(&path).unwrap();
        assert_eq!(loaded, lockfile);

        // Tampered content fails integrity verification
        let mut tampered = lockfile.clone();
        tampered.closure_signature = "bad-hash".to_string();
        tampered.save(&path).unwrap();
        let err = DmzLockfile::load_and_verify(&path).unwrap_err();
        match err {
            LockError::IntegrityMismatch { expected, .. } => {
                assert_eq!(expected, "bad-hash");
            }
            other => panic!("Expected IntegrityMismatch, got {:?}", other),
        }

        // Loading a directory path fails with IO error
        let dir_err = DmzLockfile::load_and_verify(temp.path()).unwrap_err();
        assert!(matches!(dir_err, LockError::Io(_)));
    }

    #[test]
    fn test_lockfile_default_and_trait_conformance() {
        let lockfile = DmzLockfile::default();
        assert_eq!(lockfile.version, 1);
        assert!(lockfile.packages.is_empty());
        assert_eq!(
            lockfile.closure_signature,
            DmzLockfile::compute_closure_hash(&lockfile.packages)
        );
    }
}
