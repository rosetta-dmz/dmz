#[cfg(unix)]
use super::types::IpcServerError;

/// Verifies that the peer connecting to the Unix domain socket shares the same effective UID as the supervisor daemon.
#[cfg(unix)]
pub fn verify_peer_credentials(stream: &tokio::net::UnixStream) -> Result<(), IpcServerError> {
    let cred = stream.peer_cred().map_err(|e| {
        IpcServerError::PeerAuthenticationFailed(format!(
            "Failed to retrieve peer credentials from Unix socket: {}",
            e
        ))
    })?;

    // SAFETY: geteuid is an async-signal-safe POSIX syscall with no preconditions.
    let my_uid = unsafe { libc::geteuid() };
    let peer_uid = cred.uid();

    if peer_uid != my_uid && peer_uid != 0 {
        return Err(IpcServerError::PeerAuthenticationFailed(format!(
            "Peer UID {} does not match supervisor effective UID {}",
            peer_uid, my_uid
        )));
    }

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_verify_peer_credentials_accepts_same_uid_pair() {
        let (a, _b) = tokio::net::UnixStream::pair().expect("create unix stream pair");
        verify_peer_credentials(&a).expect("same-process Unix peer must be accepted");
    }
}
