use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig, ImportError};
use dmz_core::vcs::GitProvider;
use git2::{Repository, Signature};
use std::fs;
use std::path::PathBuf;

fn commit_all(repo: &Repository, message: &str) -> String {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();

    let sig = Signature::now("DMZ Test", "test@dmz.local").unwrap();

    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().unwrap()),
        Err(_) => None,
    };

    let parents: Vec<&git2::Commit> = match &parent {
        Some(p) => vec![p],
        None => vec![],
    };

    let commit_oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap();

    commit_oid.to_string()
}

fn setup_git_workspace() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, String) {
    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join("store");
    let output_dir = tmp_dir.path().join("dist");

    fs::create_dir_all(&workspace_path).unwrap();
    fs::create_dir_all(workspace_path.join("src")).unwrap();
    fs::create_dir_all(&store_path).unwrap();
    fs::create_dir_all(&output_dir).unwrap();

    // Initialize git repository
    let repo = Repository::init(&workspace_path).unwrap();

    // Create Cargo.toml and src/lib.rs
    fs::write(
        workspace_path.join("Cargo.toml"),
        r#"[package]
name = "test-app"
version = "1.0.8"
edition = "2021"

[dependencies]
serde = "1.0"
"#,
    )
    .unwrap();
    fs::write(workspace_path.join("src/lib.rs"), "// version 1").unwrap();

    // Initial lockfile
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
    fs::write(workspace_path.join("dmz.lock"), lockfile_content).unwrap();

    // Create a dummy cached artifact in the store
    let serde_store_dir = store_path.join("serde");
    fs::create_dir_all(&serde_store_dir).unwrap();
    fs::write(
        serde_store_dir.join("serde-1.0.archive"),
        b"dummy_serde_archive_bytes",
    )
    .unwrap();

    let initial_commit = commit_all(&repo, "Initial commit");

    (
        tmp_dir,
        workspace_path,
        store_path,
        output_dir,
        initial_commit,
    )
}

#[test]
fn test_git_provenance_detection() {
    let (_tmp, workspace, _store, _out, commit_id) = setup_git_workspace();

    // 1. Initial clean state
    let prov = GitProvider::get_provenance(&workspace)
        .expect("GitProvider must succeed")
        .expect("Provenance must be detected");

    assert_eq!(prov.commit, commit_id);
    assert!(!prov.dirty, "Repository should be clean initially");

    // 2. Modify a file without committing -> dirty flag must become true
    fs::write(workspace.join("src/lib.rs"), "// dirty modification").unwrap();

    let prov_dirty = GitProvider::get_provenance(&workspace)
        .expect("GitProvider must succeed")
        .expect("Provenance must be detected");

    assert!(
        prov_dirty.dirty,
        "Repository must be flagged dirty when files are modified"
    );
}

#[test]
fn test_git_bundle_creation_and_restore() {
    let (tmp, workspace, _store, _out, commit_id) = setup_git_workspace();
    let bundle_file = tmp.path().join("test.bundle");

    // 1. Create bundle
    let bundle_info = GitProvider::create_bundle(&workspace, &bundle_file, None)
        .expect("Bundle creation must succeed");

    assert_eq!(bundle_info.head_commit, commit_id);
    assert!(bundle_info.object_count > 0);
    assert!(bundle_file.exists());

    // Verify bundle header format v2
    let content = fs::read(&bundle_file).unwrap();
    let header_prefix = b"# v2 git bundle\n";
    assert!(
        content.starts_with(header_prefix),
        "Bundle must adhere to Git bundle v2 header"
    );

    // 2. Restore bundle into a fresh destination workspace
    let restore_target = tmp.path().join("restored_workspace");
    fs::create_dir_all(&restore_target).unwrap();

    GitProvider::restore_bundle(&restore_target, &bundle_file)
        .expect("Bundle restore must succeed");

    assert!(restore_target.join(".git/workspace.bundle").exists());
}

#[test]
fn test_delta_bundle_export_and_import() {
    let (tmp, workspace, store, out, base_commit) = setup_git_workspace();

    // 1. Export baseline bundle
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

    // 2. Set up target air-gapped machine with baseline workspace imported
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

    // 3. Make changes in original workspace: add a new file and modify an existing one, then commit
    fs::write(workspace.join("src/lib.rs"), "// updated in commit 2").unwrap();
    fs::write(workspace.join("new_feature.rs"), "pub fn extra() {}").unwrap();
    let repo = Repository::open(&workspace).unwrap();
    let _second_commit = commit_all(&repo, "Second commit adding new_feature");

    // 4. Export delta bundle referencing base commit
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

    // 5. Apply delta bundle onto the target workspace
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
