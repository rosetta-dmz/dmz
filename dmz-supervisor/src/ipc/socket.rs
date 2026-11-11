use super::types::{IpcServerError, MAX_UNIX_SOCKET_PATH};
use std::path::{Path, PathBuf};

/// RAII guard that unlinks a Unix domain socket from the filesystem upon scope exit.
pub struct SocketGuard {
    path: PathBuf,
    active: bool,
}

impl SocketGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    #[allow(dead_code)]
    pub fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if self.active && self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Validates that the Unix domain socket path does not exceed the POSIX.1-2017 limit of 104 bytes.
pub fn validate_socket_path_length(path: &Path) -> Result<(), IpcServerError> {
    let len = path.as_os_str().len();
    if len >= MAX_UNIX_SOCKET_PATH {
        return Err(IpcServerError::SocketPathTooLong {
            path: path.to_path_buf(),
            len,
            max: MAX_UNIX_SOCKET_PATH,
        });
    }
    Ok(())
}

/// Prepares the parent directory for a Unix socket with restrictive 0o700 permissions.
pub async fn prepare_socket_directory(path: &Path) -> Result<(), IpcServerError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            IpcServerError::SocketDirectoryPreparation {
                path: parent.to_path_buf(),
                source: e,
            }
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(|e| IpcServerError::SocketPermission {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }
    }
    Ok(())
}
