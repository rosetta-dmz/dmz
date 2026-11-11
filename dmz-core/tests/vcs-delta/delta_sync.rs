use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig, ImportError};
use git2::Repository;
use std::fs;

use super::common::{commit_all, setup_git_workspace};

#[test]
fn test_delta_bundle_export_and_import() {
    let (tmp, workspace, store, out, base_commit) = setup_git_workspace();

    // Export baseline bundle
    let base_bundle_path = out.join("baseline.tar.zst");
    let base_config = ExportConfig {
        workspace_path: workspace.clone(),
        lockfile_path: workspace.join("dmz.lock"),
        store_dir: store.clone(),
        output_path: base_bundle_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        ..Default::default()
    };
    let base_manifest = AirGapExporter::export(&base_config).expect("Base export failed");
    assert!(base_manifest.delta_base.is_none());

    // Set up target air-gapped machine with baseline workspace imported
    let target_ws = tmp.path().join("target_workspace");
    let target_store = tmp.path().join("target_store");

    let import_base_cfg = ImportConfig {
        archive_path: base_bundle_path,
        target_store_dir: target_store.clone(),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    };
    AirGapImporter::import(&import_base_cfg).expect("Base import failed");

    // Clone git repo directly from base workspace so target_ws is exactly at base_commit
    Repository::clone(workspace.to_str().unwrap(), target_ws.join("git_clone_tmp")).unwrap();
    // Move .git into target_ws
    fs::rename(target_ws.join("git_clone_tmp/.git"), target_ws.join(".git")).unwrap();
    let _ = fs::remove_dir_all(target_ws.join("git_clone_tmp"));

    // Make changes in original workspace: add a new file and modify an existing one, then commit
    fs::write(workspace.join("src/lib.rs"), "// updated in commit 2").unwrap();
    fs::write(workspace.join("new_feature.rs"), "pub fn extra() {}").unwrap();
    let repo = Repository::open(&workspace).unwrap();
    let _second_commit = commit_all(&repo, "Second commit adding new_feature");

    // Export delta bundle referencing base commit
    let delta_bundle_path = out.join("delta.tar.zst");
    let delta_config = ExportConfig {
        workspace_path: workspace.clone(),
        lockfile_path: workspace.join("dmz.lock"),
        store_dir: store.clone(),
        output_path: delta_bundle_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        since: Some(base_commit.clone()),
        ..Default::default()
    };

    let delta_manifest = AirGapExporter::export(&delta_config).expect("Delta export failed");
    assert!(delta_manifest.delta_base.is_some());
    let dinfo = delta_manifest.delta_base.unwrap();
    assert_eq!(dinfo.base_commit, Some(base_commit));

    // Apply delta bundle onto the target workspace
    let import_delta_cfg = ImportConfig {
        archive_path: delta_bundle_path,
        target_store_dir: target_store.clone(),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    };
    let delta_result = AirGapImporter::import(&import_delta_cfg).expect("Delta import failed");
    assert!(delta_result.is_delta);

    // Verify the newly added file and modified file were materialized
    assert!(target_ws.join("new_feature.rs").exists());
    let updated_lib = fs::read_to_string(target_ws.join("src/lib.rs")).unwrap();
    assert_eq!(updated_lib, "// updated in commit 2");
}

#[test]
fn test_delta_base_mismatch_fails_fast() {
    let (tmp, workspace, store, out, base_commit) = setup_git_workspace();

    // Make changes and export delta
    fs::write(workspace.join("extra.txt"), "hello").unwrap();
    let repo = Repository::open(&workspace).unwrap();
    commit_all(&repo, "Added extra.txt");

    let delta_bundle_path = out.join("delta.tar.zst");
    let delta_config = ExportConfig {
        workspace_path: workspace.clone(),
        lockfile_path: workspace.join("dmz.lock"),
        store_dir: store.clone(),
        output_path: delta_bundle_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        since: Some(base_commit),
        ..Default::default()
    };
    AirGapExporter::export(&delta_config).expect("Delta export failed");

    // Attempt to import delta onto a target workspace that has an incompatible git commit
    let incompatible_ws = tmp.path().join("incompatible_ws");
    fs::create_dir_all(&incompatible_ws).unwrap();
    let bad_repo = Repository::init(&incompatible_ws).unwrap();
    fs::write(incompatible_ws.join("different.txt"), "other").unwrap();
    commit_all(&bad_repo, "Different commit entirely");

    let bad_import_cfg = ImportConfig {
        archive_path: delta_bundle_path,
        target_store_dir: tmp.path().join("bad_store"),
        target_workspace_dir: incompatible_ws,
        verify_closure_hash: true,
    };

    let result = AirGapImporter::import(&bad_import_cfg);
    assert!(matches!(result, Err(ImportError::VerificationFailed(_))));
}

#[test]
fn test_export_with_git_history() {
    let (tmp, workspace, store, out, _commit) = setup_git_workspace();

    let bundle_path = out.join("with_git.tar.zst");
    let config = ExportConfig {
        workspace_path: workspace.clone(),
        lockfile_path: workspace.join("dmz.lock"),
        store_dir: store.clone(),
        output_path: bundle_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        with_git_history: true,
        ..Default::default()
    };

    let manifest = AirGapExporter::export(&config).expect("Export with git history failed");
    assert!(manifest.has_git_history);

    // Import into target
    let target_ws = tmp.path().join("git_restored_ws");
    let import_cfg = ImportConfig {
        archive_path: bundle_path,
        target_store_dir: tmp.path().join("git_store"),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    };

    let res = AirGapImporter::import(&import_cfg).expect("Import failed");
    assert!(res.has_git_history);
    assert!(target_ws.join("workspace.bundle").exists());
}
