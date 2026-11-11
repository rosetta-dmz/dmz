use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

/// Helper to set up a temporary workspace fixture with the raw hex hash (no prefix)
fn setup_mock_workspace() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join("store");
    let output_dir = tmp_dir.path().join("dist");

    fs::create_dir_all(&workspace_path).unwrap();
    fs::create_dir_all(workspace_path.join("src")).unwrap();
    fs::create_dir_all(&store_path).unwrap();
    fs::create_dir_all(&output_dir).unwrap();

    // Create a mock Cargo.toml
    let manifest_content = r#"
        [package]
        name = "test-app"
        version = "1.0.8"
        edition = "2021"

        [dependencies]
        serde = "1.0"
    "#;
    fs::write(workspace_path.join("Cargo.toml"), manifest_content).unwrap();
    fs::write(workspace_path.join("src/lib.rs"), "// dummy library source").unwrap();

    // Create JSON lockfile with raw hex hash (validator prepends sha256:)
    let lockfile_content = r#"{
        "closure_signature": "9ab6c6dbe61cc71e9274a3981cad4468c314a86f8693c7724c23c24c84113c99",
        "version": 1,
        "packages": {
            "serde": {
                "name": "serde",
                "version": "1.0.0",
                "sha256": "dummy_hash_serde",
                "ecosystem": "Cargo",
                "dependencies": []
            }
        }
    }"#;
    let lockfile_path = workspace_path.join("dmz.lock");
    fs::write(&lockfile_path, lockfile_content).unwrap();

    // Create dummy cached artifact in the store matching the package key "serde"
    let serde_store_dir = store_path.join("serde");
    fs::create_dir_all(&serde_store_dir).unwrap();
    fs::write(
        serde_store_dir.join("serde-1.0.archive"),
        b"dummy_serde_archive_bytes",
    )
    .unwrap();

    (tmp_dir, workspace_path, store_path, output_dir)
}

#[test]
fn test_e2e_unified_airgap_export_and_import() {
    let (_tmp_dir, workspace_path, store_path, output_dir) = setup_mock_workspace();
    let lockfile_path = workspace_path.join("dmz.lock");

    // Test Unified Air-Gap Export (Code + Store)
    let bundle_path = output_dir.join("unified_bundle.tar.zst");
    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_path.clone(),
        output_path: bundle_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        target_platform: None,
        ..Default::default()
    };

    let manifest = AirGapExporter::export(&export_config).expect("Unified air-gap export failed");

    assert!(bundle_path.exists());
    assert_eq!(manifest.package_count, 1);
    assert!(!manifest.archive_sha256.is_empty());

    // Test Air-Gap Import on a fresh target store
    let target_store = _tmp_dir.path().join("target_store");
    let import_config = ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store.clone(),
        verify_closure_hash: true,
        ..ImportConfig::default()
    };

    let import_result = AirGapImporter::import(&import_config).expect("Air-gap import failed");

    assert_eq!(import_result.imported_packages, 1);
    assert!(target_store.join("serde").exists());
}

#[test]
fn test_e2e_code_only_and_deps_only_modes() {
    let (_tmp_dir, workspace_path, store_path, output_dir) = setup_mock_workspace();
    let lockfile_path = workspace_path.join("dmz.lock");

    // Test Code-Only Export
    let code_bundle = output_dir.join("code_only.tar.zst");
    let code_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_path.clone(),
        output_path: code_bundle.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        ..Default::default()
    };
    let code_manifest = AirGapExporter::export(&code_config).expect("CodeOnly export failed");
    assert_eq!(code_manifest.package_count, 0); // No store items packed
    assert!(code_bundle.exists());

    // Test Deps-Only Export
    let deps_bundle = output_dir.join("deps_only.tar.zst");
    let deps_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_path.clone(),
        output_path: deps_bundle.clone(),
        zstd_level: 3,
        mode: ExportMode::DepsOnly,
        target_platform: None,
        ..Default::default()
    };
    let deps_manifest = AirGapExporter::export(&deps_config).expect("DepsOnly export failed");
    assert_eq!(deps_manifest.package_count, 1); // Store items packed, workspace code omitted
    assert!(deps_bundle.exists());
}

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

    // Create valid JSON mock lockfile with an empty package map required by the exporter
    let lockfile_path = workspace_path.join("dmz.lock");
    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
    fs::write(&lockfile_path, mock_lockfile_json)?;

    // Create mock cache directory to simulate pre-fetched dependencies
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

    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: PathBuf::from("dmz.lock"),
        output_path: PathBuf::from("bundle.tar.zst"),
        zstd_level: 3,
        ..Default::default()
    };

    let _manifest = AirGapExporter::export(&export_config)?;
    assert!(workspace_path.join("bundle.tar.zst").exists());

    let target_dir = tempdir()?;
    let dest_workspace = target_dir.path().join("target_workspace");
    fs::create_dir_all(&dest_workspace)?;

    let import_config = ImportConfig {
        archive_path: workspace_path.join("bundle.tar.zst"),
        target_workspace_dir: dest_workspace.clone(),
        target_store_dir: PathBuf::from(".dmz/store"),
        verify_closure_hash: true,
    };

    AirGapImporter::import(&import_config)?;
    assert!(dest_workspace.join("Cargo.toml").exists());
    assert!(dest_workspace.join(".dmz/store").exists());

    Ok(())
}

