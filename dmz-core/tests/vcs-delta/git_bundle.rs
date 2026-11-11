use dmz_core::vcs::GitProvider;
use git2::Repository;
use std::fs;

use super::common::{commit_path, setup_git_workspace};

#[test]
fn test_git_provenance_detection() {
    let (_tmp, workspace, _store, _out, commit_id) = setup_git_workspace();

    // Initial clean state
    let prov = GitProvider::get_provenance(&workspace)
        .expect("GitProvider must succeed")
        .expect("Provenance must be detected");

    assert_eq!(prov.commit, commit_id);
    assert!(!prov.dirty, "Repository should be clean initially");

    // Add an untracked file without staging/committing -> dirty flag must remain false, but has_untracked must be true
    fs::write(workspace.join("untracked_file.txt"), "hello untracked").unwrap();
    let prov_with_untracked = GitProvider::get_provenance(&workspace)
        .expect("GitProvider must succeed")
        .expect("Provenance must be detected");
    assert!(
        !prov_with_untracked.dirty,
        "Repository must not be flagged dirty solely due to untracked files"
    );
    assert!(
        prov_with_untracked.has_untracked,
        "Repository must report has_untracked = true when untracked files exist"
    );
    assert!(
        prov_with_untracked
            .untracked_files
            .contains(&"untracked_file.txt".to_string()),
        "untracked_files must include newly added untracked file"
    );

    // Modify a tracked file without committing -> dirty flag must become true and file listed in uncommitted_files
    fs::write(workspace.join("src/lib.rs"), "// dirty modification").unwrap();

    let prov_dirty = GitProvider::get_provenance(&workspace)
        .expect("GitProvider must succeed")
        .expect("Provenance must be detected");

    assert!(
        prov_dirty.dirty,
        "Repository must be flagged dirty when tracked files are modified"
    );
    assert!(
        prov_dirty
            .uncommitted_files
            .contains(&"src/lib.rs".to_string()),
        "uncommitted_files must list modified tracked file"
    );

    // Commit modified file -> dirty flag becomes false while untracked file continues to be reported
    let repo = Repository::open(&workspace).unwrap();
    commit_path(&repo, "src/lib.rs", "Commit modifications to src/lib.rs");
    let prov_after_commit = GitProvider::get_provenance(&workspace)
        .expect("GitProvider must succeed")
        .expect("Provenance must be detected");
    assert!(
        !prov_after_commit.dirty,
        "Repository must be clean once tracked changes are committed"
    );
    assert!(
        prov_after_commit.has_untracked,
        "Untracked file must still be detected after committing other tracked modifications"
    );
    assert!(
        prov_after_commit.uncommitted_files.is_empty(),
        "uncommitted_files must be empty after committing"
    );
}

#[test]
fn test_git_bundle_creation_and_restore() {
    let (tmp, workspace, _store, _out, commit_id) = setup_git_workspace();
    let bundle_file = tmp.path().join("test.bundle");

    // Create bundle
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

    // Restore bundle into a fresh destination workspace
    let restore_target = tmp.path().join("restored_workspace");
    fs::create_dir_all(&restore_target).unwrap();

    GitProvider::restore_bundle(&restore_target, &bundle_file)
        .expect("Bundle restore must succeed");

    assert!(restore_target.join(".git/workspace.bundle").exists());
}
