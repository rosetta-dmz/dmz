use dmz_sandbox::platform::SandboxConfig;
use std::path::PathBuf;

#[test]
fn test_sandbox_config_ipc_builder() {
    let config = SandboxConfig::new("/tmp/test_workspace", "/nix/store", "/bin/sh")
        .with_ipc_socket("/tmp/dmz_test_sidecar.sock");

    assert_eq!(config.workspace_path, PathBuf::from("/tmp/test_workspace"));
    assert_eq!(config.closure_path, PathBuf::from("/nix/store"));
    assert_eq!(config.command, "/bin/sh");
    assert_eq!(
        config.ipc_socket_path,
        Some(PathBuf::from("/tmp/dmz_test_sidecar.sock"))
    );
    assert!(!config.allow_network);
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_seatbelt_profile_ipc_and_zero_network() {
    use dmz_sandbox::platform::generate_seatbelt_profile;

    let sock_path = PathBuf::from("/tmp/dmz_5a2f8b1c_sidecar.sock");
    let config =
        SandboxConfig::new("/tmp/workspace", "/tmp/store", "/bin/sh").with_ipc_socket(&sock_path);

    let profile = generate_seatbelt_profile(&config);

    // 1. Strict network denial is enforced
    assert!(profile.contains("(deny network*)"));

    // 2. Local Unix Domain Sockets are explicitly permitted for IPC under network denial
    assert!(profile.contains("(allow network* (to unix-socket))"));
    assert!(profile.contains("(allow network-bind (to unix-socket))"));
    assert!(profile.contains("(allow network-outbound (to unix-socket))"));

    // 3. Literal path rule for safe socket is present
    assert!(profile.contains(&format!("(literal \"{}\")", sock_path.display())));
}
