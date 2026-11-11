use dmz_core::ecosystem::CommandSpec;
use dmz_core::install::{InstallConfig, InstallError, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::resolver::ResolverEngine;
use dmz_core::toolchain::setup::ToolchainSetup;
use sha2::{Digest, Sha256};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_zero_tolerance_supply_chain_integrity_verification() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws_path = temp.path().join("workspace");
    fs::create_dir_all(&ws_path)?;

    let store_dir = temp.path().join("store");
    let pkg_store_dir = store_dir.join("tampered-pkg");
    fs::create_dir_all(&pkg_store_dir)?;

    let archive_file = pkg_store_dir.join("tampered-pkg-1.0.0.archive");
    let authentic_content = b"AUTHENTIC_PACKAGE_DATA_v1.0.0";
    let authentic_sha = format!("{:x}", Sha256::digest(authentic_content));

    // Write corrupted data into the store (supply chain tampering!)
    let tampered_content = b"TAMPERED_MALICIOUS_PACKAGE_v1.0.0";
    fs::write(&archive_file, tampered_content)?;

    // Lockfile records the authentic hash
    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "tampered-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: authentic_sha.clone(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    }]);

    let lockfile_path = ws_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let install_config = InstallConfig {
        workspace_path: ws_path.clone(),
        lockfile_path,
        store_dir,
    };

    // Install must fail fast with IntegrityMismatch
    match InstallerEngine::install(&install_config) {
        Err(InstallError::IntegrityMismatch { expected, actual }) => {
            assert_eq!(expected, authentic_sha);
            assert_ne!(actual, authentic_sha);
        }
        other => {
            panic!("Expected InstallError::IntegrityMismatch, got {:?}", other);
        }
    }

    Ok(())
}

#[test]
fn test_pypi_wheel_abi_scoring() {
    let urls = vec![
        serde_json::json!({
            "url": "https://example.com/demo-1.0.0.tar.gz",
            "filename": "demo-1.0.0.tar.gz",
            "packagetype": "sdist"
        }),
        serde_json::json!({
            "url": "https://example.com/demo-1.0.0-py3-none-any.whl",
            "filename": "demo-1.0.0-py3-none-any.whl",
            "packagetype": "bdist_wheel"
        }),
        serde_json::json!({
            "url": "https://example.com/demo-1.0.0-cp311-cp311-macosx_11_0_arm64.whl",
            "filename": "demo-1.0.0-cp311-cp311-macosx_11_0_arm64.whl",
            "packagetype": "bdist_wheel"
        }),
        serde_json::json!({
            "url": "https://example.com/demo-1.0.0-cp311-cp311-manylinux2014_x86_64.whl",
            "filename": "demo-1.0.0-cp311-cp311-manylinux2014_x86_64.whl",
            "packagetype": "bdist_wheel"
        }),
    ];

    // On macos-arm64, the macosx_*_arm64 wheel must win
    let mac_pick = ResolverEngine::select_pypi_artifact(&urls, Some("macos-arm64"));
    assert_eq!(
        mac_pick,
        Some("https://example.com/demo-1.0.0-cp311-cp311-macosx_11_0_arm64.whl".to_string())
    );

    // On linux-x86_64, the manylinux_*_x86_64 wheel must win
    let linux_pick = ResolverEngine::select_pypi_artifact(&urls, Some("linux-x86_64"));
    assert_eq!(
        linux_pick,
        Some("https://example.com/demo-1.0.0-cp311-cp311-manylinux2014_x86_64.whl".to_string())
    );

    // When only generic wheel and sdist exist, generic wheel must win
    let generic_urls = vec![
        serde_json::json!({
            "url": "https://example.com/demo-1.0.0.tar.gz",
            "filename": "demo-1.0.0.tar.gz",
            "packagetype": "sdist"
        }),
        serde_json::json!({
            "url": "https://example.com/demo-1.0.0-py3-none-any.whl",
            "filename": "demo-1.0.0-py3-none-any.whl",
            "packagetype": "bdist_wheel"
        }),
    ];
    let gen_pick = ResolverEngine::select_pypi_artifact(&generic_urls, Some("windows-x86_64"));
    assert_eq!(
        gen_pick,
        Some("https://example.com/demo-1.0.0-py3-none-any.whl".to_string())
    );

    // When only sdist exists, fallback to sdist
    let sdist_only = vec![serde_json::json!({
        "url": "https://example.com/demo-1.0.0.tar.gz",
        "filename": "demo-1.0.0.tar.gz",
        "packagetype": "sdist"
    })];
    let sdist_pick = ResolverEngine::select_pypi_artifact(&sdist_only, Some("linux-arm64"));
    assert_eq!(
        sdist_pick,
        Some("https://example.com/demo-1.0.0.tar.gz".to_string())
    );
}

#[test]
fn test_toolchain_ecosystem_command_resolution() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws_path = temp.path().join("workspace");
    let rust_bin_dir = ws_path
        .join(".dmz")
        .join("toolchains")
        .join("rust")
        .join("bin");
    fs::create_dir_all(&rust_bin_dir)?;
    let cargo_bin = rust_bin_dir.join("cargo");
    fs::write(&cargo_bin, "#!/bin/sh\necho mock cargo\n")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cargo_bin, fs::Permissions::from_mode(0o755));
    }

    let cmd = CommandSpec::new("cargo", vec!["check".into()]).resolve_in_workspace(&ws_path);
    assert_eq!(
        cmd.program.replace('\\', "/"),
        cargo_bin.to_string_lossy().replace('\\', "/")
    );

    // When toolchain is not bundled, program remains unchanged
    let cmd_ambient = CommandSpec::new("nonexistent_tool", vec!["--version".into()])
        .resolve_in_workspace(&ws_path);
    assert_eq!(cmd_ambient.program, "nonexistent_tool");

    Ok(())
}

#[test]
fn test_relocate_bundled_binaries_idempotent() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws_path = temp.path().join("workspace");
    let cc_bin_dir = ws_path.join(".dmz/toolchains/cc/bin");
    fs::create_dir_all(&cc_bin_dir)?;

    let mock_bin = cc_bin_dir.join("clang");
    // Write Mach-O 64-bit magic header (0xfeedfacf)
    fs::write(&mock_bin, b"\xcf\xfa\xed\xfe\x00\x00\x00\x00")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755));
    }

    // Call relocation twice to verify safety and idempotency
    ToolchainSetup::relocate_bundled_binaries(&ws_path)
        .map_err(|e| anyhow::anyhow!("Relocation failed: {}", e))?;
    ToolchainSetup::relocate_bundled_binaries(&ws_path)
        .map_err(|e| anyhow::anyhow!("Second relocation failed: {}", e))?;

    Ok(())
}

#[test]
fn test_atomic_lockfile_and_config_persistence() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let lock_path = temp.path().join("nested/dir/dmz.lock");
    let mut lockfile = DmzLockfile::new(Vec::new());
    let pkg = LockedPackage {
        name: "test-atomic-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "abcd".to_string(),
        source_url: Some("mock".to_string()),
        dependencies: vec![],
        member_id: None,
    };
    lockfile.packages.insert(pkg.name.clone(), pkg);
    lockfile.closure_signature = DmzLockfile::compute_closure_hash(&lockfile.packages);

    lockfile.save(&lock_path)?;
    assert!(lock_path.exists());

    // Verify content loaded matches
    let loaded = DmzLockfile::load_and_verify(&lock_path)?;
    assert_eq!(loaded.packages.len(), 1);

    // Test WorkspaceProjectConfig atomic save
    let cfg_path = temp.path().join("workspace/dmz-workspace.toml");
    let ws_cfg = dmz_core::workspace::WorkspaceProjectConfig::default();
    ws_cfg.save_to_file(&cfg_path)?;
    assert!(cfg_path.exists());
    let loaded_cfg = dmz_core::workspace::WorkspaceProjectConfig::load_from_file(&cfg_path)?;
    assert_eq!(loaded_cfg.members.len(), ws_cfg.members.len());

    Ok(())
}
