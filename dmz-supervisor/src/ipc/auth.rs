#[cfg(unix)]
use super::types::IpcServerError;

/// Verifies that the peer connecting to the Unix domain socket shares the same UID as the supervisor daemon.
#[cfg(unix)]
pub fn verify_peer_credentials(stream: &tokio::net::UnixStream) -> Result<(), IpcServerError> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    // SAFETY: getuid is an async-signal-safe POSIX syscall with no preconditions.
    let my_uid = unsafe { libc::getuid() };

    #[cfg(target_os = "linux")]
    {
        let mut ucred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt with SO_PEERCRED reads caller credentials into a validly allocated stack buffer.
        let res = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if res != 0 {
            return Err(IpcServerError::PeerAuthenticationFailed(
                "Failed to retrieve SO_PEERCRED from Unix socket".into(),
            ));
        }
        if ucred.uid != my_uid && ucred.uid != 0 {
            return Err(IpcServerError::PeerAuthenticationFailed(
                "Peer UID does not match supervisor UID".into(),
            ));
        }
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        let mut peer_uid: libc::uid_t = 0;
        let mut peer_gid: libc::gid_t = 0;
        // SAFETY: getpeereid queries the credentials of peer socket descriptor into valid stack pointers.
        let res = unsafe { libc::getpeereid(fd, &mut peer_uid, &mut peer_gid) };
        if res != 0 {
            return Err(IpcServerError::PeerAuthenticationFailed(
                "Failed to retrieve getpeereid from Unix socket".into(),
            ));
        }
        if peer_uid != my_uid && peer_uid != 0 {
            return Err(IpcServerError::PeerAuthenticationFailed(
                "Peer UID does not match supervisor UID".into(),
            ));
        }
    }

    Ok(())
}
