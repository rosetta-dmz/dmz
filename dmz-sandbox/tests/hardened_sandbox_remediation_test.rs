use dmz_sandbox::agent::ephemeral_fs::EphemeralFs;
use dmz_sandbox::agent::network_proxy::DomainWhitelist;
use dmz_sandbox::platform::SandboxConfig;
use std::path::PathBuf;

#[test]
fn test_domain_whitelist_ipv6_and_host_normalization() {
    let whitelist = DomainWhitelist::new(vec![
        "::1".to_string(),
        "2001:db8::1".to_string(),
        "".to_string(),
        "".to_string(),
    ]);

    // IPv6 address with brackets and port
    assert!(whitelist.is_allowed("[::1]:8080"));
    assert!(whitelist.is_allowed("[::1]"));
    assert!(whitelist.is_allowed("::1"));
    assert!(whitelist.is_allowed("[2001:db8::1]:443"));
    assert!(whitelist.is_allowed("2001:db8::1"));

    // Trailing dot root FQDN
    assert!(whitelist.is_allowed(""));
    assert!(whitelist.is_allowed(""));
    assert!(whitelist.is_allowed(""));

    // Full URL schemes, paths, and userinfo
    assert!(whitelist.is_allowed(""));
    assert!(whitelist.is_allowed(""));
    assert!(whitelist.is_allowed(""));

    // Blocked hosts
    assert!(!whitelist.is_allowed("[::2]:8080"));
}

#[cfg(target_os = "macos")]
#[test]
fn test_seatbelt_profile_escaping_injection_attempt() {
    use dmz_sandbox::platform::generate_seatbelt_profile;

    // Craft a path attempting to inject `(allow default)` into the Seatbelt Scheme profile
    let injection_path = PathBuf::from(r#"/tmp/test" (allow default) \ "inject_dir"#);
    let config = SandboxConfig {
        workspace_path: injection_path.clone(),
        closure_path: PathBuf::from("/nix/store"),
        command: "/bin/sh".to_string(),
        args: vec![],
        envs: vec![],
        allow_network: false,
        ipc_socket_path: None,
    };

    let profile = generate_seatbelt_profile(&config);

    // Double quotes and backslashes must be properly escaped
    assert!(profile.contains(r#"\" (allow default) \\ \"inject_dir"#));

    // Profile must not contain an unescaped (allow default) directive
    assert!(!profile.contains("\n(allow default)"));
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_seatbelt_private_tmp_scoping() {
    use dmz_sandbox::platform::generate_seatbelt_profile;

    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let ws = tmp.path().join("my_workspace");
    std::fs::create_dir_all(&ws).expect("Failed to create workspace");

    // Test 1: Without IPC socket
    let config = SandboxConfig::new(&ws, "/nix/store", "/bin/sh");
    let profile = generate_seatbelt_profile(&config);

    // Blanket write to /private/tmp must be completely removed
    assert!(
        !profile.contains(r#"(allow file-write* (subpath "/private/tmp"))"#),
        "Profile must not grant blanket write access to /private/tmp"
    );
    assert!(
        !profile.contains(r#"(allow file-read* file-write* (subpath "/private/tmp"))"#),
        "Profile must not grant blanket read/write access to /private/tmp"
    );

    // Writes must be scoped to the workspace's private .dmz/tmp directory
    assert!(
        profile.contains(".dmz/tmp"),
        "Profile must scope temporary writes to .dmz/tmp"
    );

    // Test 2: Regression test - socket placed in /tmp must NOT re-open /private/tmp blanket write access
    let sock_in_tmp = PathBuf::from("/tmp/dmz_regression_test.sock");
    let config_with_sock =
        SandboxConfig::new(&ws, "/nix/store", "/bin/sh").with_ipc_socket(&sock_in_tmp);
    let profile_with_sock = generate_seatbelt_profile(&config_with_sock);

    assert!(
        !profile_with_sock.contains(r#"(allow file-write* (subpath "/private/tmp"))"#),
        "Socket in /tmp must not grant blanket subpath write access to /private/tmp"
    );
    assert!(
        !profile_with_sock.contains(r#"(allow file-read* file-write* (subpath "/private/tmp"))"#),
        "Socket in /tmp must not grant blanket subpath read-write access to /private/tmp"
    );
    assert!(
        profile_with_sock.contains(r#"(literal "/tmp/dmz_regression_test.sock")"#),
        "Safe literal socket rule must be present"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_seatbelt_empty_closure_protection() {
    use dmz_sandbox::platform::generate_seatbelt_profile;

    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let ws = tmp.path().join("my_workspace");
    std::fs::create_dir_all(&ws).expect("Failed to create workspace");

    // Empty closure path must not produce blanket read access to the entire disk
    let config = SandboxConfig::new(&ws, "", "/bin/sh");
    let profile = generate_seatbelt_profile(&config);

    assert!(
        !profile.contains(r#"(allow file-read* (subpath ""))"#),
        "Empty closure path must not produce blanket (subpath \"\") read rule"
    );
}

#[cfg(unix)]
#[test]
fn test_ephemeral_fs_atomic_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let fs =
        EphemeralFs::new("atomic_perm_test", 1024 * 1024).expect("Failed to create scratchpad");
    let path = fs.path();

    assert!(path.exists());
    let meta = std::fs::metadata(path).expect("Failed to get metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "Scratchpad directory must have mode 0700");
}
