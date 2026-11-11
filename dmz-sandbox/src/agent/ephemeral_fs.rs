use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug)]
pub struct EphemeralFs {
    /// Path to the temporary scratchpad mount point
    pub mount_path: PathBuf,
    /// Maximum allowed memory size in bytes
    pub size_limit_bytes: usize,
    /// Whether or not mount was created as a native RAM disk/tmpfs
    pub is_mounted: bool,
}

#[derive(Debug)]
pub enum EphemeralError {
    CreationFailed(String),
    MountFailed(String),
    CleanupFailed(String),
    SecurityViolation(String),
}

impl std::fmt::Display for EphemeralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreationFailed(msg) => write!(f, "Scratchpad Directory Creation Failed: {}", msg),
            Self::MountFailed(msg) => write!(f, "tmpfs Mount Failed: {}", msg),
            Self::CleanupFailed(msg) => write!(f, "Scratchpad Cleanup Failed: {}", msg),
            Self::SecurityViolation(msg) => write!(f, "Security Violation in Scratchpad: {}", msg),
        }
    }
}

impl std::error::Error for EphemeralError {}

impl EphemeralFs {
    /// Create and mount a new ephemeral in-memory scratchpad for an AI agent
    pub fn new(agent_id: &str, size_limit_bytes: usize) -> Result<Self, EphemeralError> {
        let sanitized_id: String = agent_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe_id = if sanitized_id.is_empty() {
            "default"
        } else {
            &sanitized_id
        };

        // Generate unique process and timestamp nonce to eliminate TOCTOU collisions and pre-creation attacks
        let pid = std::process::id();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        let temp_dir = std::env::temp_dir();
        let folder_name = format!("dmz_agent_scratchpad_{}_{}_{:x}", safe_id, pid, timestamp);
        let mount_path = temp_dir.join(folder_name);

        // Atomic creation with restrictive 0o700 permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            use std::os::unix::fs::MetadataExt;

            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(&mount_path)
                .map_err(|e| EphemeralError::CreationFailed(format!("Failed to create {:?}: {}", mount_path, e)))?;

            // Defense-in-depth: Verify directory ownership strictly matches current process UID
            let meta = fs::symlink_metadata(&mount_path)
                .map_err(|e| EphemeralError::CreationFailed(format!("Failed to read metadata for {:?}: {}", mount_path, e)))?;

            if meta.file_type().is_symlink() {
                let _ = fs::remove_file(&mount_path);
                return Err(EphemeralError::SecurityViolation(format!(
                    "Scratchpad path {:?} is a symlink",
                    mount_path
                )));
            }

            // SAFETY: libc::getuid is an async-signal-safe POSIX syscall with no invariants
            let current_uid = unsafe { libc::getuid() };
            if meta.uid() != current_uid {
                let _ = fs::remove_dir(&mount_path);
                return Err(EphemeralError::SecurityViolation(format!(
                    "Scratchpad directory {:?} UID mismatch (expected {}, found {})",
                    mount_path, current_uid, meta.uid()
                )));
            }
        }

        #[cfg(not(unix))]
        {
            fs::create_dir(&mount_path).map_err(|e| {
                EphemeralError::CreationFailed(format!("Failed to create {:?}: {}", mount_path, e))
            })?;
        }

        let mut instance = Self {
            mount_path,
            size_limit_bytes,
            is_mounted: false,
        };

        instance.mount_in_memory()?;
        Ok(instance)
    }

    /// Mounts RAM-backed storage natively depending on the host OS
    fn mount_in_memory(&mut self) -> Result<(), EphemeralError> {
        #[cfg(target_os = "linux")]
        {
            use nix::mount::{mount, MsFlags};

            let options = format!("size={}", self.size_limit_bytes);
            let mount_res = mount(
                Some("tmpfs"),
                &self.mount_path,
                Some("tmpfs"),
                MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
                Some(options.as_str()),
            );

            if mount_res.is_ok() {
                self.is_mounted = true;
                info!(path = ?self.mount_path, "Mounted Linux tmpfs scratchpad");
            } else {
                // Fallback for unprivileged Linux environments
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&self.mount_path, fs::Permissions::from_mode(0o700))
                    .map_err(|e| EphemeralError::MountFailed(format!("Failed to set permissions: {}", e)))?;
                self.is_mounted = false;
                info!(
                    path = ?self.mount_path,
                    "Initialized Linux volatile scratchpad with restricted permissions (unprivileged fallback)"
                );
            }
        }

        #[cfg(target_os = "macos")]
        {
            // MacOS: isolated directory with restricted permissions (0700)
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.mount_path, fs::Permissions::from_mode(0o700))
                .map_err(|e| EphemeralError::MountFailed(format!("Failed to set permissions: {}", e)))?;
            self.is_mounted = false;
            info!(path = ?self.mount_path, "Initialized macOS disk scratchpad with 0700 permissions");
        }

        #[cfg(target_os = "windows")]
        {
            // Windows temp path sandbox fallback
            self.is_mounted = false;
            info!(path = ?self.mount_path, "Initialized Windows disk scratchpad");
        }

        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.mount_path
    }
}

/// Automatically wipe scratchpad directory when handle is dropped
impl Drop for EphemeralFs {
    fn drop(&mut self) {
        info!(path = ?self.mount_path, "Tearing down AI agent ephemeral scratchpad");

        #[cfg(target_os = "linux")]
        if self.is_mounted {
            use nix::mount::{umount2, MntFlags};
            if let Err(e) = umount2(&self.mount_path, MntFlags::MNT_DETACH) {
                warn!(error = %e, "Failed to unmount Linux tmpfs");
            }
        }

        // Symlink traversal defense: verify it is a real directory and not a hijacked symlink before deleting
        if let Ok(meta) = fs::symlink_metadata(&self.mount_path) {
            if meta.file_type().is_symlink() {
                warn!(path = ?self.mount_path, "Refusing to recursively delete scratchpad: path is a symlink");
                let _ = fs::remove_file(&self.mount_path);
            } else if let Err(e) = fs::remove_dir_all(&self.mount_path) {
                warn!(error = %e, "Failed to remove ephemeral scratchpad directory");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ephemeral_fs_sanitizes_agent_id() {
        let fs = EphemeralFs::new("../../traversal_agent/unsafe:name", 1024 * 1024).unwrap();
        let path = fs.path().to_path_buf();
        assert!(path.starts_with(std::env::temp_dir()));
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert!(!file_name.contains('/'));
        assert!(!file_name.contains('\\'));
        assert!(!file_name.contains(':'));
        assert!(file_name.starts_with("dmz_agent_scratchpad_"));
    }

    #[test]
    fn test_ephemeral_fs_unique_instances_do_not_collide() {
        let fs1 = EphemeralFs::new("same_agent", 1024 * 1024).unwrap();
        let fs2 = EphemeralFs::new("same_agent", 1024 * 1024).unwrap();

        assert_ne!(fs1.path(), fs2.path());
        assert!(fs1.path().exists());
        assert!(fs2.path().exists());

        let p2 = fs2.path().to_path_buf();
        drop(fs1);
        // Dropping fs1 must not destroy fs2's scratchpad
        assert!(p2.exists());
    }
}
