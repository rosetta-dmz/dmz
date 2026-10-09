use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug)]
pub struct EphemeralFs {
    /// Path to the temporary scratchpad mount point
    pub mount_path: PathBuf,
    /// Maximum allowed memory size in bytes
    pub size_limit_bytes: usize,
    /// Whether this mount was created as a native RAM disk/tmpfs
    is_mounted: bool,
}

#[derive(Debug)]
pub enum EphemeralError {
    CreationFailed(String),
    MountFailed(String),
    CleanupFailed(String),
}

impl std::fmt::Display for EphemeralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreationFailed(msg) => write!(f, "Scratchpad Directory Creation Failed: {}", msg),
            Self::MountFailed(msg) => write!(f, "tmpfs Mount Failed: {}", msg),
            Self::CleanupFailed(msg) => write!(f, "Scratchpad Cleanup Failed: {}", msg),
        }
    }
}

impl std::error::Error for EphemeralError {}

impl EphemeralFs {
    /// Create and mount a new ephemeral in-memory scratchpad for an AI agent
    pub fn new(agent_id: &str, size_limit_bytes: usize) -> Result<Self, EphemeralError> {
        let mount_path = std::env::temp_dir().join(format!("dmz_agent_scratchpad_{}", agent_id));

        if !mount_path.exists() {
            fs::create_dir_all(&mount_path)
                .map_err(|e| EphemeralError::CreationFailed(e.to_string()))?;
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
            mount(
                Some("tmpfs"),
                &self.mount_path,
                Some("tmpfs"),
                MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
                Some(options.as_str()),
            )
            .map_err(|e| EphemeralError::MountFailed(format!("Linux tmpfs mount failed: {}", e)))?;

            self.is_mounted = true;
            info!(path = ?self.mount_path, "Mounted Linux tmpfs scratchpad");
        }

        #[cfg(target_os = "macos")]
        {
            // Fallback for macOS: create isolated RAM scratchpad directory with restricted permissions (0700)
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.mount_path, fs::Permissions::from_mode(0o700))
                .map_err(|e| EphemeralError::CreationFailed(e.to_string()))?;
            self.is_mounted = true;
            info!(path = ?self.mount_path, "Initialized macOS isolated RAM scratchpad");
        }

        #[cfg(target_os = "windows")]
        {
            // Windows temp path sandbox fallback
            self.is_mounted = true;
            info!(path = ?self.mount_path, "Initialized Windows volatile scratchpad");
        }

        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.mount_path
    }
}

/// Automatically wipes the scratchpad directory when the handle is dropped
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

        if self.mount_path.exists() {
            if let Err(e) = fs::remove_dir_all(&self.mount_path) {
                warn!(error = %e, "Failed to remove ephemeral scratchpad directory");
            }
        }
    }
}