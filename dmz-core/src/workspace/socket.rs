// Unix domain socket and Windows named pipe allocation and lifecycle management.
// Enforces POSIX `sockaddr_un.sun_path` length limits (< 104 bytes) and provides
// atomic unlinking to eliminate TOCTOU race conditions.

use std::fs;
use std::path::{Path, PathBuf};

/// Generates a POSIX-safe short socket path to prevent sockaddr_un.sun_path overflow (104 bytes on macOS/BSD).
pub fn safe_socket_path(workspace: &Path, member_id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let canonical = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    hasher.update(canonical.to_string_lossy().as_bytes());
    let hash_full = format!("{:x}", hasher.finalize());
    let hash = hash_full.get(..10).unwrap_or(&hash_full);

    #[cfg(unix)]
    {
        let socket_dir = std::env::temp_dir().join("dmz");
        let _ = fs::create_dir_all(&socket_dir);
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o700));
        }

        // POSIX sockaddr_un.sun_path hard limits: 104 bytes on macOS/BSD, 108 bytes on Linux.
        // Budget max filename length so total path is strictly < 104 bytes.
        let dir_len = socket_dir.as_os_str().len();
        let max_filename_len = 103usize.saturating_sub(dir_len.saturating_add(1));

        let candidate = format!("dmz_{}_{}.sock", hash, member_id);
        if candidate.len() <= max_filename_len {
            socket_dir.join(candidate)
        } else {
            // Member ID is too long for POSIX sun_path limit; generate bounded hash suffix
            let mut id_hasher = Sha256::new();
            id_hasher.update(member_id.as_bytes());
            let id_hash_full = format!("{:x}", id_hasher.finalize());
            let id_short = id_hash_full.get(..8).unwrap_or(&id_hash_full);
            let bounded = format!("dmz_{}_{}.sock", hash, id_short);
            socket_dir.join(bounded)
        }
    }

    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\dmz_{}_{}", hash, member_id))
    }
}

/// Automatically removes stale socket files from previous unclean shutdowns.
/// Employs atomic unlinking to eliminate TOCTOU race conditions.
pub fn cleanup_stale_socket(socket_path: &Path) -> Result<bool, std::io::Error> {
    #[cfg(windows)]
    if socket_path.to_string_lossy().starts_with(r"\\.\pipe\") {
        // Windows named pipes are in-memory kernel IPC objects managed by NPFS;
        // they are automatically cleaned up when process handles close, with no disk files.
        return Ok(false);
    }

    match fs::remove_file(socket_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_safe_socket_path_length_guarantee() {
        let long_path = PathBuf::from(
            "/Users/some_very_long_developer_username/Development/EnterpriseProjects/ClientServices/Subsystem/Application/WorkspaceDirectory/DeeplyNestedComponent",
        );
        let safe_sock = safe_socket_path(&long_path, "sidecar");
        let path_str = safe_sock.to_string_lossy();

        #[cfg(unix)]
        {
            // POSIX sockaddr_un.sun_path hard limits: 104 on macOS, 108 on Linux
            assert!(
                path_str.len() < 104,
                "Safe socket path '{}' exceeds 104 characters ({})",
                path_str,
                path_str.len()
            );
            assert!(path_str.ends_with("_sidecar.sock"));
        }

        #[cfg(windows)]
        {
            assert!(path_str.starts_with(r"\\.\pipe\dmz_"));
        }
    }

    #[test]
    fn test_safe_socket_path_ultra_long_member_id_bounds() {
        let ws_path = PathBuf::from("/Users/dev/workspace");
        let ultra_long_id = "a_very_long_member_id_that_would_normally_cause_sun_path_to_overflow_and_crash_the_kernel_bind_call_1234567890";
        let safe_sock = safe_socket_path(&ws_path, ultra_long_id);
        let path_str = safe_sock.to_string_lossy();

        #[cfg(unix)]
        {
            assert!(
                path_str.len() < 104,
                "Socket path with ultra-long member ID must strictly remain under 104 bytes, got {}",
                path_str.len()
            );
        }

        #[cfg(windows)]
        {
            assert!(path_str.starts_with(r"\\.\pipe\dmz_"));
        }
    }

    #[test]
    fn test_cleanup_stale_socket_atomic() -> Result<(), Box<dyn Error>> {
        let temp_dir = tempfile::tempdir()?;
        let sock_path = temp_dir.path().join("test_stale.sock");

        // Non-existent socket returns Ok(false) atomically
        assert!(!cleanup_stale_socket(&sock_path)?);

        // Existing socket is removed and returns Ok(true)
        fs::write(&sock_path, b"stale socket payload")?;
        assert!(cleanup_stale_socket(&sock_path)?);
        assert!(!sock_path.exists());

        // Subsequent call returns Ok(false) without failing with NotFound
        assert!(!cleanup_stale_socket(&sock_path)?);
        Ok(())
    }
}
