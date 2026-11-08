use std::fs;
use tempfile::tempdir;

use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::ecosystem::CommandSpec;
use dmz_core::install::{InstallConfig, InstallError, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::resolver::ResolverEngine;
use dmz_core::toolchain::setup::ToolchainSetup;

#[test]
fn test_deterministic_archive_reproducibility() -> anyhow::Result<()> {
    std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");

    let temp_ws = tempdir()?;
    let ws_path = temp_ws.path().join("workspace");
    fs::create_dir_all(&ws_path)?;

    // Create several files in varying order
    fs::write(
        ws_path.join("Cargo.toml"),
        "[package]\nname = \"deterministic-demo\"\nversion = \"1.0.0\"\n",
    )?;
    fs::create_dir_all(ws_path.join("src"))?;
    fs::write(
        ws_path.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )?;
    fs::write(
        ws_path.join("package.json"),
        "{\n  \"name\": \"deterministic-demo\",\n  \"version\": \"1.0.0\"\n}\n",
    )?;
    fs::write(ws_path.join("README.md"), "# Deterministic Archive\n")?;

    let lockfile = DmzLockfile::new(Vec::new());
    let lockfile_path = ws_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let dist_dir1 = tempdir()?;
    let archive_path1 = dist_dir1.path().join("bundle_run1.tar.zst");
    let export_cfg1 = ExportConfig {
        workspace_path: ws_path.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: ws_path.join(".dmz/store"),
        output_path: archive_path1.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest1 = AirGapExporter::export(&export_cfg1)?;
    assert!(archive_path1.exists());
    let sha1 = manifest1.archive_sha256;
    let bytes1 = fs::read(&archive_path1)?;

    // Simulate delay and touch files to change host mtime
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(ws_path.join("README.md"), "# Deterministic Archive\n")?;

    let dist_dir2 = tempdir()?;
    let archive_path2 = dist_dir2.path().join("bundle_run2.tar.zst");
    let export_cfg2 = ExportConfig {
        workspace_path: ws_path.clone(),
        lockfile_path,
        store_dir: ws_path.join(".dmz/store"),
        output_path: archive_path2.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest2 = AirGapExporter::export(&export_cfg2)?;
    assert!(archive_path2.exists());
    let sha2 = manifest2.archive_sha256;
    let bytes2 = fs::read(&archive_path2)?;

    // Both SHA-256 and byte-for-byte contents must be identical!
    assert_eq!(
        sha1, sha2,
        "Deterministic build SHA256 must match across exports"
    );
    assert_eq!(
        bytes1, bytes2,
        "Archive byte content must be bit-for-bit identical"
    );

    Ok(())
}

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
    use sha2::{Digest, Sha256};
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
    let rust_bin_dir = ws_path.join(".dmz/toolchains/rust/bin");
    fs::create_dir_all(&rust_bin_dir)?;
    let cargo_bin = rust_bin_dir.join("cargo");
    fs::write(&cargo_bin, "#!/bin/sh\necho mock cargo\n")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cargo_bin, fs::Permissions::from_mode(0o755));
    }

    let cmd = CommandSpec::new("cargo", vec!["check".into()]).resolve_in_workspace(&ws_path);
    assert_eq!(cmd.program, cargo_bin.to_string_lossy());

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
fn test_single_pass_streaming_hash_matches_disk_hash() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws_path = temp.path().join("hash_ws");
    fs::create_dir_all(&ws_path)?;
    fs::write(
        ws_path.join("file1.txt"),
        "Single-pass streaming test content 1",
    )?;
    fs::write(
        ws_path.join("file2.txt"),
        "Single-pass streaming test content 2",
    )?;

    let lockfile = DmzLockfile::new(Vec::new());
    let lockfile_path = ws_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let archive_path = temp.path().join("single_pass_bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: ws_path,
        lockfile_path,
        store_dir: temp.path().join("store"),
        output_path: archive_path.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest = AirGapExporter::export(&export_cfg)?;
    assert!(archive_path.exists());

    // Single-pass inline computed hash must match the independently verified disk hash
    let disk_sha = AirGapExporter::calculate_file_sha256(&archive_path)?;
    assert_eq!(
        manifest.archive_sha256, disk_sha,
        "Inline streaming hash must match re-read disk hash"
    );

    Ok(())
}

#[test]
fn test_fast_clone_reflink_and_dir_recursive() -> anyhow::Result<()> {
    use dmz_core::install::fast_clone::FastClone;

    let temp = tempdir()?;
    let src_dir = temp.path().join("source_pkg");
    let dst_dir = temp.path().join("destination_pkg");

    fs::create_dir_all(src_dir.join("sub"))?;
    fs::write(
        src_dir.join("package.json"),
        "{\"name\":\"fast-clone-test\"}",
    )?;
    fs::write(src_dir.join("sub/index.js"), "console.log('fast clone');")?;

    FastClone::clone_dir_recursive(&src_dir, &dst_dir)?;

    assert!(dst_dir.join("package.json").exists());
    assert!(dst_dir.join("sub/index.js").exists());
    assert_eq!(
        fs::read_to_string(dst_dir.join("package.json"))?,
        "{\"name\":\"fast-clone-test\"}"
    );
    assert_eq!(
        fs::read_to_string(dst_dir.join("sub/index.js"))?,
        "console.log('fast clone');"
    );

    // Verify fast-clone file overwrite safety
    let single_src = src_dir.join("single.txt");
    let single_dst = dst_dir.join("single.txt");
    fs::write(&single_src, "test single clone")?;
    FastClone::clone_file(&single_src, &single_dst)?;
    assert_eq!(fs::read_to_string(&single_dst)?, "test single clone");

    // Overwrite existing
    fs::write(&single_src, "test single clone updated")?;
    FastClone::clone_file(&single_src, &single_dst)?;
    assert_eq!(
        fs::read_to_string(&single_dst)?,
        "test single clone updated"
    );

    Ok(())
}
