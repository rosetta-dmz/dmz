use std::fs;
use tempfile::tempdir;

use dmz_core::archive::exporter::{AirGapExporter, ExportConfig};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig};

#[test]
fn test_multiecosystem_airgap_roundtrip() -> anyhow::Result<()> {
    // 1. Setup a temporary source workspace mimicking a multi-ecosystem project
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("my_project");
    fs::create_dir_all(&workspace_path)?;

    // Drop mock manifests for Rust and Node ecosystems
    fs::write(workspace_path.join("Cargo.toml"), "[package]\nname = \"test-rust\"\nversion = \"0.1.0\"")?;
    fs::write(workspace_path.join("package.json"), "{\"name\": \"test-node\", \"version\": \"1.0.0\"}")?;

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

    // 2. Setup export destination
    let dist_dir = tempdir()?;
    let archive_path = dist_dir.path().join("workspace_closure.tar.zst");

    let export_config = ExportConfig {
        workspace_path,
        lockfile_path,
        output_path: archive_path.clone(),
        zstd_level: 3,
        ..Default::default()
    };

    // 3. Execute Export
    println!("Running air-gap export test...");
    let _manifest = AirGapExporter::export(&export_config)?;
    assert!(archive_path.exists(), "Export archive must be created");

    // 4. Setup import destination (simulating disconnected air-gap target machine)
    let target_dir = tempdir()?;
    let import_target = target_dir.path().join("imported_workspace");
    let store_path = target_dir.path().join("offline_store");

    let import_config = ImportConfig {
        archive_path,
        target_store_dir: store_path,
        target_workspace_dir: import_target.clone(),
        verify_closure_hash: true,
    };

    // 5. Execute Import
    println!("Running air-gap import test...");
    AirGapImporter::import(&import_config)?;

    // 6. Assertions: verify files landed correctly and multi-ecosystem detection works post-import
    assert!(import_target.join("Cargo.toml").exists(), "Cargo.toml should be unpacked");
    assert!(import_target.join("package.json").exists(), "package.json should be unpacked");
    assert!(import_target.join(".cargo").exists(), "Ecosystem cache should be preserved");

    Ok(())
}