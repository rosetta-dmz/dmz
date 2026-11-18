#[cfg(unix)]
use dmz_sandbox::agent::ephemeral_fs::EphemeralFs;
use dmz_sandbox::agent::network_proxy::DomainWhitelist;
#[cfg(target_os = "macos")]
use dmz_sandbox::platform::SandboxConfig;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[test]
fn test_domain_whitelist_ipv6_and_host_normalization() {
    let whitelist = DomainWhitelist::new(vec![
        "::1".to_string(),
        "2001:db8::1".to_string(),
        "allowed-domain.local".to_string(),
        "*.allowed-wildcard.local".to_string(),
    ]);

    // IPv6 address with brackets and port
    assert!(whitelist.is_allowed("[::1]:8080"));
    assert!(whitelist.is_allowed("[::1]"));
    assert!(whitelist.is_allowed("::1"));
    assert!(whitelist.is_allowed("[2001:db8::1]:443"));
    assert!(whitelist.is_allowed("2001:db8::1"));

    // Trailing dot root FQDN
    assert!(whitelist.is_allowed("allowed-domain.local."));
    assert!(whitelist.is_allowed("allowed-domain.local.:443"));
    assert!(whitelist.is_allowed("api.allowed-wildcard.local."));

    // Full URL schemes, paths, and userinfo
    assert!(whitelist.is_allowed("https://allowed-domain.local/api/v1/crates"));
    assert!(whitelist.is_allowed("http://user:pass@allowed-domain.local:443/test"));
    assert!(whitelist.is_allowed("https://raw.allowed-wildcard.local/foo/bar"));

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

#[cfg(target_os = "macos")]
#[test]
fn test_macos_seatbelt_metadata_modes_and_getcwd() -> Result<(), Box<dyn std::error::Error>> {
    use dmz_sandbox::platform::{
        generate_seatbelt_profile, generate_seatbelt_profile_with_options,
        run_sandboxed_with_options, MetadataReadMode, SandboxPolicyOptions,
    };

    let tmp = tempfile::tempdir()?;
    let ws = tmp.path().join("nested").join("workspace");
    let store = tmp.path().join("store");
    let outside_file = tmp.path().join("outside_secret.txt");
    std::fs::create_dir_all(&ws)?;
    std::fs::create_dir_all(&store)?;
    std::fs::write(&outside_file, "secret")?;

    let config = SandboxConfig::new(&ws, &store, "/bin/pwd");

    // 1. Default (Bazel pattern) emits global file-read-metadata without granting file-read*
    let default_profile = generate_seatbelt_profile(&config);
    assert!(default_profile.contains("(allow file-read-metadata)"));
    assert!(!default_profile.contains("\n(allow file-read*)"));

    // 2. StrictAncestors emits scoped literal ancestor metadata rules instead of global allowance
    let strict_opts = SandboxPolicyOptions {
        metadata_mode: MetadataReadMode::StrictAncestors,
        ..SandboxPolicyOptions::default()
    };
    let strict_profile = generate_seatbelt_profile_with_options(&config, &strict_opts);
    assert!(!strict_profile.contains("(allow file-read-metadata)\n"));
    assert!(strict_profile.contains("(allow file-read-metadata\n"));
    let canon_ws = std::fs::canonicalize(&ws)?;
    if let Some(parent) = canon_ws.parent() {
        assert!(strict_profile.contains(&format!("(literal \"{}\")", parent.display())));
    }

    // 3. Running /bin/pwd succeeds under Bazel-style Global metadata mode
    let status_global = run_sandboxed_with_options(&config, &SandboxPolicyOptions::default())?;
    assert!(
        status_global.success(),
        "Global metadata mode must permit getcwd() inside sandbox"
    );

    // 4. StrictAncestors permits workspace execution while blocking reads outside workspace/store
    let strict_ws_ok = SandboxConfig::new(&ws, &store, "/bin/test");
    let mut strict_ws_ok = strict_ws_ok;
    strict_ws_ok.args = vec!["-d".to_string(), ".".to_string()];
    let status_strict_ok = run_sandboxed_with_options(&strict_ws_ok, &strict_opts)?;
    assert!(
        status_strict_ok.success(),
        "StrictAncestors mode must permit workspace-scoped checks"
    );

    let mut strict_outside_denied = SandboxConfig::new(&ws, &store, "/bin/cat");
    strict_outside_denied.args = vec![outside_file.to_string_lossy().into_owned()];
    let status_strict_denied = run_sandboxed_with_options(&strict_outside_denied, &strict_opts)?;
    assert!(
        !status_strict_denied.success(),
        "StrictAncestors mode must still deny reading files outside workspace/store"
    );

    // 5. Unbound IPC socket path under symlinked temp directory includes canonicalized parent ancestor
    let sock_dir = tmp.path().join("sock_dir");
    std::fs::create_dir_all(&sock_dir)?;
    let unbound_sock = sock_dir.join("unbound_sidecar.sock");
    let sock_cfg = SandboxConfig::new(&ws, &store, "/bin/true").with_ipc_socket(&unbound_sock);
    let sock_profile = generate_seatbelt_profile_with_options(&sock_cfg, &strict_opts);
    let canon_sock_dir = std::fs::canonicalize(&sock_dir)?;
    assert!(
        sock_profile.contains(&format!("(literal \"{}\")", canon_sock_dir.display())),
        "StrictAncestors must include canonicalized parent directory for unbound IPC socket"
    );

    Ok(())
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
