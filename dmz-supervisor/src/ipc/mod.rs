pub mod auth;
pub mod client;
pub mod dispatch;
pub mod server;
pub mod socket;
pub mod transport;
pub mod types;

pub use client::IpcClient;
pub use server::IpcServer;
pub use socket::SocketGuard;
pub use types::{
    DispatchContext, IpcClientError, IpcRequest, IpcResponse, IpcServerConfig, IpcServerError,
    MAX_IPC_FRAME_BYTES, MAX_UNIX_SOCKET_PATH,
};

#[cfg(test)]
mod tests {
    use super::socket::{validate_socket_path_length, SocketGuard};
    #[cfg(unix)]
    use super::types::IpcServerConfig;
    use super::types::{
        IpcClientError, IpcRequest, IpcResponse, IpcServerError, MAX_UNIX_SOCKET_PATH,
    };
    #[cfg(unix)]
    use super::IpcServer;
    use std::path::PathBuf;

    #[test]
    fn test_socket_path_length_limit_constant() {
        assert_eq!(MAX_UNIX_SOCKET_PATH, 104);
    }

    #[test]
    fn test_ipc_server_error_display_and_traits() {
        let err = IpcServerError::SocketPathTooLong {
            path: PathBuf::from("/a/very/long/path/exceeding/limit"),
            len: 120,
            max: 104,
        };
        assert!(err.contains("exceeds POSIX maximum limit"));
        let err_str: String = err.into();
        assert!(err_str.contains("120 bytes"));
    }

    #[test]
    fn test_ipc_client_error_display_and_traits() {
        let err = IpcClientError::PayloadTooLarge {
            bytes: 70000,
            max_bytes: 65536,
        };
        assert!(err.contains("exceeded maximum frame limit"));
        let err_str: String = err.into();
        assert!(err_str.contains("70000 bytes"));
    }

    #[test]
    fn test_socket_path_exact_boundary_conditions() {
        // Under POSIX, path length must be strictly < 104 bytes
        let path_103 = PathBuf::from("a".repeat(103));
        assert!(validate_socket_path_length(&path_103).is_ok());

        let path_104 = PathBuf::from("a".repeat(104));
        let res_104 = validate_socket_path_length(&path_104);
        assert!(res_104.is_err());
        assert!(matches!(
            res_104.unwrap_err(),
            IpcServerError::SocketPathTooLong {
                len: 104,
                max: 104,
                ..
            }
        ));

        let path_105 = PathBuf::from("a".repeat(105));
        let res_105 = validate_socket_path_length(&path_105);
        assert!(res_105.is_err());
        assert!(matches!(
            res_105.unwrap_err(),
            IpcServerError::SocketPathTooLong {
                len: 105,
                max: 104,
                ..
            }
        ));
    }

    #[test]
    fn test_ipc_request_response_serialization_roundtrip() {
        let requests = vec![
            IpcRequest::Ping,
            IpcRequest::GetStatus,
            IpcRequest::StartAgent {
                agent_id: "agent-x".to_string(),
                allowed_domains: vec!["crates.io".to_string()],
                scratchpad_bytes: 2048,
                command: Some("cargo".to_string()),
                args: Some(vec!["test".to_string()]),
            },
            IpcRequest::StopAgent {
                agent_id: "agent-x".to_string(),
            },
            IpcRequest::RunSandbox {
                workspace_path: "/tmp/ws".to_string(),
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                allow_network: false,
            },
            IpcRequest::GetSidecarStatus,
            IpcRequest::StopSidecarPair,
            IpcRequest::GetServiceStatus,
            IpcRequest::Shutdown,
        ];

        for req in requests {
            let serialized = serde_json::to_string(&req).expect("serialize request");
            let deserialized: IpcRequest =
                serde_json::from_str(&serialized).expect("deserialize request");
            let reserialized = serde_json::to_string(&deserialized).expect("reserialize request");
            assert_eq!(serialized, reserialized);
        }

        let responses = vec![
            IpcResponse::ok("all good"),
            IpcResponse::ok_with_data("data payload", serde_json::json!({ "count": 42 })),
            IpcResponse::error("failure message"),
        ];

        for resp in responses {
            let serialized = serde_json::to_string(&resp).expect("serialize response");
            let deserialized: IpcResponse =
                serde_json::from_str(&serialized).expect("deserialize response");
            let reserialized = serde_json::to_string(&deserialized).expect("reserialize response");
            assert_eq!(serialized, reserialized);
        }
    }

    #[test]
    fn test_raii_socket_guard_unlinks_on_drop() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let sock_path = tmp.path().join("test_guard.sock");
        std::fs::write(&sock_path, b"test").expect("write test file");
        assert!(sock_path.exists());

        {
            let _guard = SocketGuard::new(sock_path.clone());
            assert!(sock_path.exists());
        }

        assert!(
            !sock_path.exists(),
            "SocketGuard must automatically delete socket file upon drop"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_listen_unix_rejects_oversized_socket_path() {
        let long_name = "x".repeat(120);
        let path = PathBuf::from(format!("/tmp/{}.sock", long_name));
        let server = IpcServer::new(IpcServerConfig { socket_path: path });
        let res = server.listen_unix().await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(err, IpcServerError::SocketPathTooLong { .. }));
    }
}
