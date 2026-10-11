use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig};
use std::fs;
use std::path::PathBuf;

/// Helper to set up a temporary workspace fixture with the raw hex hash (without prefix)
fn setup_mock_workspace() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join("store");
    let output_dir = tmp_dir.path().join("dist");

    fs::create_dir_all(&workspace_path).unwrap();
    fs::create_dir_all(&workspace_path.join("src")).unwrap();
    fs::create_dir_all(&store_path).unwrap();
    fs::create_dir_all(&output_dir).unwrap();

    // Create a mock Cargo.toml
    let manifest_content = r#"
        [package]
        name = "test-app"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        serde = "1.0"
    "#;
    fs::write(workspace_path.join("Cargo.toml"), manifest_content).unwrap();
    fs::write(workspace_path.join("src/lib.rs"), "// dummy library source").unwrap();

    // Create a valid JSON lockfile with raw hex hash (validator prepends sha256:)
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

    // Create a dummy cached artifact in the store matching the package key "serde"
    let serde_store_dir = store_path.join("serde");
    fs::create_dir_all(&serde_store_dir).unwrap();
    fs::write(serde_store_dir.join("serde-1.0.archive"), b"dummy_serde_archive_bytes").unwrap();

    (tmp_dir, workspace_path, store_path, output_dir)
}

#[test]
fn test_e2e_unified_airgap_export_and_import() {
    let (_tmp_dir, workspace_path, store_path, output_dir) = setup_mock_workspace();
    let lockfile_path = workspace_path.join("dmz.lock");

    // Step 1: Test Unified Air-Gap Export (Code + Store)
    let bundle_path = output_dir.join("unified_bundle.tar.zst");
    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_path.clone(),
        output_path: bundle_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
    };

    let manifest = AirGapExporter::export(&export_config)
        .expect("Unified air-gap export failed");
    
    assert!(bundle_path.exists());
    assert_eq!(manifest.package_count, 1);
    assert!(!manifest.archive_sha256.is_empty());

    // Step 2: Test Air-Gap Import on a fresh target store
    let target_store = _tmp_dir.path().join("target_store");
    let import_config = ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store.clone(),
        verify_closure_hash: true,
    };

    let import_result = AirGapImporter::import(&import_config)
        .expect("Air-gap import failed");

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
    };
    let deps_manifest = AirGapExporter::export(&deps_config).expect("DepsOnly export failed");
    assert_eq!(deps_manifest.package_count, 1); // Store items packed, workspace code omitted
    assert!(deps_bundle.exists());
}