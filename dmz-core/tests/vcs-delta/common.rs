use git2::{Repository, Signature};
use std::fs;
use std::path::PathBuf;

pub fn commit_all(repo: &Repository, message: &str) -> String {
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

pub fn commit_path(repo: &Repository, path: &str, message: &str) -> String {
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new(path)).unwrap();
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

pub fn setup_git_workspace() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, String) {
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
