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
