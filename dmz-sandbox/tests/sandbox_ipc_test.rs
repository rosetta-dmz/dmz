use dmz_sandbox::platform::SandboxConfig;

#[test]
fn test_sandbox_config_ipc_builder() {
    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let ws = tmp.path().join("test_workspace");
    let store = tmp.path().join("store");
    let sock = tmp.path().join("dmz_test_sidecar.sock");

    let config = SandboxConfig::new(&ws, &store, "/bin/sh").with_ipc_socket(&sock);

    assert_eq!(config.workspace_path, ws);
    assert_eq!(config.closure_path, store);
    assert_eq!(config.command, "/bin/sh");
    assert_eq!(config.ipc_socket_path, Some(sock));
    assert!(!config.allow_network);
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_seatbelt_profile_ipc_and_zero_network() {
    use dmz_sandbox::platform::generate_seatbelt_profile;

    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let ws = tmp.path().join("workspace");
    let store = tmp.path().join("store");
    let sock_path = tmp.path().join("dmz_5a2f8b1c_sidecar.sock");
    let config = SandboxConfig::new(&ws, &store, "/bin/sh").with_ipc_socket(&sock_path);

    let profile = generate_seatbelt_profile(&config);

    // Enforce strict network denial
    assert!(profile.contains("(deny network*)"));

    // Explicitly permit local Unix Domain Sockets for IPC under network denial
    assert!(profile.contains("(allow network* (to unix-socket))"));
    assert!(profile.contains("(allow network-bind (to unix-socket))"));
    assert!(profile.contains("(allow network-outbound (to unix-socket))"));

    // Literal path rule for safe socket is present
    assert!(profile.contains(&format!("(literal \"{}\")", sock_path.display())));
}

#[test]
fn test_sandbox_runner_trait_and_workspace_validation() {
    use dmz_sandbox::platform::{NativeSandboxRunner, PlatformError, SandboxRunner};
    use std::path::PathBuf;

    let runner = NativeSandboxRunner;
    let non_existent_ws = PathBuf::from("/path/that/does/not/exist/dmz_test_12345");
    let config = SandboxConfig::new(&non_existent_ws, "/nix/store", "/bin/sh");

    let res = runner.run(&config);
    assert!(res.is_err());
    match res.unwrap_err() {
        PlatformError::PlatformFault(msg) => {
            assert!(msg.contains("Workspace path does not exist"));
        }
        other => panic!("Expected PlatformFault for missing workspace, got: {:?}", other),
    }
}

#[test]
fn test_sandbox_socket_path_length_limit_rejection() {
    use dmz_sandbox::platform::{NativeSandboxRunner, PlatformError, SandboxRunner};

    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let ws = tmp.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("Failed to create workspace");

    // Construct a socket path deliberately exceeding the 104/108 byte POSIX limits
    let long_name = "x".repeat(115);
    let long_sock = tmp.path().join(format!("{}.sock", long_name));

    let runner = NativeSandboxRunner;
    let config = SandboxConfig::new(&ws, "/nix/store", "/bin/true").with_ipc_socket(&long_sock);

    let res = runner.run(&config);
    assert!(res.is_err());
    match res.unwrap_err() {
        PlatformError::SeatbeltError(msg) | PlatformError::NamespaceError(msg) => {
            assert!(msg.contains("sun_path limit"));
        }
        other => panic!("Expected sun_path limit rejection, got: {:?}", other),
    }
}
