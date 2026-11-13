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

    // Create a valid cached crate archive in the store with real SHA-256 lockfile metadata
    let mut tar_builder = tar::Builder::new(Vec::new());
    let crate_toml = b"[package]\nname = \"serde\"\nversion = \"1.0.0\"\n";
    let mut hdr = tar::Header::new_gnu();
    hdr.set_size(crate_toml.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_cksum();
    tar_builder
        .append_data(&mut hdr, "serde-1.0.0/Cargo.toml", &crate_toml[..])
        .unwrap();
    let tar_bytes = tar_builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
    let archive_bytes = gz.finish().unwrap();

    let serde_store_dir = store_path.join("serde");
    fs::create_dir_all(&serde_store_dir).unwrap();
    fs::write(serde_store_dir.join("serde-1.0.0.archive"), &archive_bytes).unwrap();

    let lockfile = dmz_core::resolver::lockfile::DmzLockfile::new(vec![
        dmz_core::resolver::lockfile::LockedPackage {
            name: "serde".to_string(),
            version: "1.0.0".to_string(),
            ecosystem: dmz_core::resolver::manifest::Ecosystem::Cargo,
            sha256: dmz_core::resolver::lockfile::DmzLockfile::hash_bytes(&archive_bytes),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        },
    ]);
    lockfile.save(&workspace_path.join("dmz.lock")).unwrap();

    let initial_commit = commit_all(&repo, "Initial commit");

    (
        tmp_dir,
        workspace_path,
        store_path,
        output_dir,
        initial_commit,
    )
}
