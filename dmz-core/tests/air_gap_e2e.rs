use std::fs;
use tempfile::tempdir;

use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig};

#[test]
fn test_multiecosystem_airgap_roundtrip() -> anyhow::Result<()> {
    // Setup temporary source workspace mimicking a multi-ecosystem project
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("my_project");
    fs::create_dir_all(&workspace_path)?;

    // Drop mock manifests for Rust and Node ecosystems
    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"test-rust\"\nversion = \"1.0.8\"",
    )?;
    fs::write(
        workspace_path.join("package.json"),
        "{\"name\": \"test-node\", \"version\": \"1.0.0\"}",
    )?;

    // Create a valid JSON mock lockfile with an empty package map required by the exporter
    let lockfile_path = workspace_path.join("dmz.lock");
    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
    fs::write(&lockfile_path, mock_lockfile_json)?;

    // Create a mock cache directory to simulate pre-fetched dependencies
    let cargo_cache = workspace_path.join(".cargo");
    fs::create_dir_all(&cargo_cache)?;
    fs::write(cargo_cache.join("config.toml"), "# mock config")?;

    // Setup export destination
    let dist_dir = tempdir()?;
    let archive_path = dist_dir.path().join("workspace_closure.tar.zst");

    let export_config = ExportConfig {
        workspace_path,
        lockfile_path,
        output_path: archive_path.clone(),
        zstd_level: 3,
        ..Default::default()
    };

    // Execute Export
    println!("Running air-gap export test...");
    let _manifest = AirGapExporter::export(&export_config)?;
    assert!(archive_path.exists(), "Export archive must be created");

    // Setup import destination (simulating disconnected air-gap target machine)
    let target_dir = tempdir()?;
    let import_target = target_dir.path().join("imported_workspace");
    let store_path = target_dir.path().join("offline_store");

    let import_config = ImportConfig {
        archive_path,
        target_store_dir: store_path,
        target_workspace_dir: import_target.clone(),
        verify_closure_hash: true,
    };

    // Execute Import
    println!("Running air-gap import test...");
    AirGapImporter::import(&import_config)?;

    // Assertions: verify files landed correctly and multi-ecosystem detection works post-import
    assert!(
        import_target.join("Cargo.toml").exists(),
        "Cargo.toml should be unpacked"
    );
    assert!(
        import_target.join("package.json").exists(),
        "package.json should be unpacked"
    );
    assert!(
        import_target.join(".cargo").exists(),
        "Ecosystem cache should be preserved"
    );

    Ok(())
}

#[test]
fn test_relative_paths_anchor_to_selected_workspace() -> anyhow::Result<()> {
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("external_project");
    fs::create_dir_all(&workspace_path)?;

    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"external-app\"\nversion = \"0.1.0\"",
    )?;

    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
    fs::write(workspace_path.join("dmz.lock"), mock_lockfile_json)?;

    // Configure export with relative paths as the GUI/CLI default provides
    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: std::path::PathBuf::from("dmz.lock"),
        store_dir: std::path::PathBuf::from(".dmz/store"),
        output_path: std::path::PathBuf::from("dist/workspace_closure.tar.zst"),
        zstd_level: 3,
        ..Default::default()
    };

    let manifest = AirGapExporter::export(&export_config)?;
    assert!(!manifest.archive_sha256.is_empty());

    // Verify it exported into the selected workspace
    let expected_archive = workspace_path.join("dist/workspace_closure.tar.zst");
    assert!(
        expected_archive.exists(),
        "Export archive must be created inside the selected workspace"
    );

    Ok(())
}

#[test]
<<<<<<< Updated upstream
fn test_toolchain_packaging_and_import_roundtrip() -> anyhow::Result<()> {
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("mock_app");
    fs::create_dir_all(&workspace_path)?;

    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"toolchain-test\"\nversion = \"0.1.0\"",
    )?;

    let lockfile_path = workspace_path.join("dmz.lock");
=======
fn test_toolchain_packaging_and_import_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let workspace_path = temp.path().join("source_ws");
    fs::create_dir_all(&workspace_path)?;

    // Create mock workspace with Cargo.toml
    let cargo_toml = r#"[package]
name = "mock-app"
version = "0.1.0"
edition = "2021"
"#;
    fs::write(workspace_path.join("Cargo.toml"), cargo_toml)?;

>>>>>>> Stashed changes
    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
<<<<<<< Updated upstream
    fs::write(&lockfile_path, mock_lockfile_json)?;

    let dist_dir = tempdir()?;
    let archive_path = dist_dir.path().join("bundle_with_tc.tar.zst");

    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        output_path: archive_path.clone(),
        zstd_level: 3,
        with_toolchain: true,
        ..Default::default()
    };

    println!("Testing air-gap export with toolchain bundling...");
=======
    fs::write(workspace_path.join("dmz.lock"), mock_lockfile_json)?;

    let archive_path = temp.path().join("bundle_with_toolchains.tar.zst");

    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: workspace_path.join("dmz.lock"),
        store_dir: workspace_path.join(".dmz/store"),
        output_path: archive_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: true,
    };

>>>>>>> Stashed changes
    let manifest = AirGapExporter::export(&export_config)?;
    assert!(archive_path.exists(), "Export archive must exist");
    assert!(
        !manifest.archive_sha256.is_empty(),
        "Archive SHA256 must be computed"
    );

<<<<<<< Updated upstream
    // If host has rustc/cargo, toolchains should be detected and recorded in manifest
    if which::which("rustc").is_ok() {
        assert!(
            !manifest.toolchains.is_empty(),
            "Manifest should record discovered host toolchains"
        );
        let has_rust = manifest.toolchains.iter().any(|t| t.ecosystem == "rust");
        assert!(has_rust, "Rust toolchain should be recorded in manifest");
    }

    // Test import on target
    let target_dir = tempdir()?;
    let import_target = target_dir.path().join("target_workspace");
    let store_path = target_dir.path().join("offline_store");

    let import_config = ImportConfig {
        archive_path,
        target_store_dir: store_path,
=======
    // Import into fresh target directory
    let import_target = temp.path().join("target_ws");
    fs::create_dir_all(&import_target)?;

    let import_config = ImportConfig {
        archive_path: archive_path.clone(),
        target_store_dir: import_target.join(".dmz/store"),
>>>>>>> Stashed changes
        target_workspace_dir: import_target.clone(),
        verify_closure_hash: true,
    };

<<<<<<< Updated upstream
    println!("Testing air-gap import of bundle with toolchains...");
    let result = AirGapImporter::import(&import_config)?;

    if which::which("rustc").is_ok() {
=======
    let result = AirGapImporter::import(&import_config)?;

    if !manifest.toolchains.is_empty() {
>>>>>>> Stashed changes
        assert!(
            result.has_toolchains,
            "ImportResult must report has_toolchains"
        );
<<<<<<< Updated upstream
        let tc_rust = import_target.join(".dmz/toolchains/rust/bin");
        assert!(
            tc_rust.exists(),
            "Rust toolchain bin directory must be unpacked in .dmz/toolchains"
        );

        let env_sh = import_target.join(".dmz/env.sh");
        assert!(env_sh.exists(), ".dmz/env.sh activator must be generated");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&env_sh)?.permissions().mode();
            assert_eq!(mode & 0o111, 0o111, ".dmz/env.sh must be executable");
        }

        // Test path injection
        let mut envs = std::collections::HashMap::new();
        dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&import_target, &mut envs);
        assert!(envs.contains_key("PATH"), "PATH should be injected");
=======
        let env_sh = import_target.join(".dmz/env.sh");
        assert!(env_sh.exists(), ".dmz/env.sh activator must be generated");

        let mut envs = std::collections::HashMap::new();
        dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&import_target, &mut envs);
        assert!(envs.contains_key("PATH"), "PATH must be injected");
>>>>>>> Stashed changes
        assert!(
            envs.get("PATH").unwrap().contains(".dmz/toolchains"),
            "PATH should contain .dmz/toolchains"
        );
<<<<<<< Updated upstream
        assert!(
            envs.contains_key("RUSTUP_HOME"),
            "RUSTUP_HOME should be configured"
        );
=======
>>>>>>> Stashed changes
    }

    Ok(())
}
