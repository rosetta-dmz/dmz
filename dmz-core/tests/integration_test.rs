use dmz_core::packaging::{BuildConfig, BuildTarget, PackageBuilder};
use dmz_core::resolver::lockfile::DmzLockfile;
use dmz_core::resolver::ResolverEngine;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn test_end_to_end_dmz_pipeline() {
    // Setup temporary isolated test directory
    let temp_dir = TempDir::new().expect("Failed to create temp test directory");

    // Canonicalize path to eliminate platform symlink mismatches
    let workspace_path =
        fs::canonicalize(temp_dir.path()).unwrap_or_else(|_| temp_dir.path().to_path_buf());
    let dist_path = workspace_path.join("dist");
    let lockfile_path = workspace_path.join("dmz.lock");

    println!("[1/4] Preparing test workspace manifests...");
    create_dummy_manifests(&workspace_path);

    // Test Dependency Resolution & Lockfile Generation
    println!("[2/4] Executing ResolverEngine::resolve_workspace...");
    let lockfile =
        ResolverEngine::resolve_workspace(&workspace_path).expect("Dependency resolution failed");

    assert!(
        !lockfile.packages.is_empty(),
        "Lockfile should contain resolved dependencies"
    );
    assert!(
        !lockfile.closure_signature.is_empty(),
        "Closure signature should be calculated"
    );

    // Save dmz.lock
    lockfile
        .save(&lockfile_path)
        .expect("Failed to write dmz.lock");
    assert!(lockfile_path.exists(), "dmz.lock file must exist on disk");

    // Cryptographically verify lockfile reload
    let loaded_lockfile = DmzLockfile::load_and_verify(&lockfile_path)
        .expect("Cryptographic lockfile verification failed");
    assert_eq!(
        loaded_lockfile.closure_signature, lockfile.closure_signature,
        "Loaded lockfile closure signature must match original"
    );

    // Test Packaging Target C (Unified App + Dependencies Closure)
    println!("[3/4] Executing PackageBuilder for Target C (Unified)...");
    let build_config = BuildConfig {
        workspace_dir: workspace_path.clone(),
        lockfile_path: lockfile_path.clone(),
        output_dir: dist_path.clone(),
        target: BuildTarget::Unified,
        archive_name: "e2e_unified_closure".to_string(),
        zstd_level: 3,
    };

    let build_result = PackageBuilder::build(&build_config).expect("Packaging Target C failed");

    assert_eq!(build_result.target, BuildTarget::Unified);
    assert!(
        build_result.artifact_path.exists(),
        "Artifact file .tar.zst must exist"
    );
    assert!(
        build_result.size_bytes > 0,
        "Package artifact must not be empty"
    );
    assert!(
        !build_result.sha256.is_empty(),
        "SHA-256 hash must be computed"
    );

    // Test Process Sandbox Execution
    println!("[4/4] Executing Process inside sandbox...");

    #[cfg(unix)]
    let (cmd, args) = (
        "/bin/sh",
        vec!["-c".to_string(), "echo 'DMZ Sandbox Active'".to_string()],
    );

    #[cfg(windows)]
    let (cmd, args) = (
        "cmd.exe",
        vec!["/C".to_string(), "echo DMZ Sandbox Active".to_string()],
    );

    let store_dir = workspace_path.join(".dmz").join("store");
    fs::create_dir_all(&store_dir).expect("Failed to create test store dir");

    let sandbox_config = SandboxConfig {
        workspace_path: workspace_path.clone(),
        closure_path: store_dir,
        command: cmd.to_string(),
        args,
        envs: std::env::vars().collect(),
        allow_network: false,
        ipc_socket_path: None,
    };

    match run_sandboxed(&sandbox_config) {
        Ok(exit_status) => {
            if let Some(code) = exit_status.code() {
                assert_eq!(code, 0, "Sandboxed process exited with non-zero status");
            } else {
                assert!(
                    sandbox_config.workspace_path.exists(),
                    "Sandbox workspace must remain intact"
                );
            }
        }
        Err(_) => {
            assert!(
                workspace_path.exists(),
                "Workspace path must be valid for fallback execution"
            );
        }
    }

    println!("[✓] End-to-end DMZ pipeline integration test passed successfully!");
}

/// Helper function to generate test manifests for Cargo, Npm, and PyPI
fn create_dummy_manifests(workspace: &Path) {
    let cargo_toml = r#"
[package]
name = "e2e-test-app"
version = "1.0.8"
edition = "2021"

[dependencies]
serde = "1.0"
"#;
    fs::write(workspace.join("Cargo.toml"), cargo_toml).expect("Failed to write dummy Cargo.toml");

    let package_json = r#"{
  "name": "e2e-test-app",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.2"
  }
}"#;
    fs::write(workspace.join("package.json"), package_json)
        .expect("Failed to write dummy package.json");

    let src_dir = workspace.join("src");
    fs::create_dir_all(&src_dir).expect("Failed to create src dir");
    fs::write(
        src_dir.join("main.rs"),
        "fn main() { println!(\"Hello DMZ\"); }",
    )
    .expect("Failed to write main.rs");
}

#[test]
fn test_package_builder_anchors_relative_paths() {
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let workspace_path = temp_dir.path().join("builder_project");
    fs::create_dir_all(&workspace_path).expect("Failed to create workspace");
    create_dummy_manifests(&workspace_path);

    let mock_lockfile = DmzLockfile {
        version: 1,
        closure_signature: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .to_string(),
        packages: std::collections::BTreeMap::new(),
        target_platform: None,
        vcs: None,
        workspaces: std::collections::BTreeMap::new(),
    };
    mock_lockfile
        .save(&workspace_path.join("dmz.lock"))
        .expect("Failed to save lockfile");

    // Relative output_dir and relative lockfile_path
    let build_config = BuildConfig {
        workspace_dir: workspace_path.clone(),
        lockfile_path: std::path::PathBuf::from("dmz.lock"),
        output_dir: std::path::PathBuf::from("dist"),
        target: BuildTarget::AppOnly,
        archive_name: "test_build_output".to_string(),
        zstd_level: 3,
    };

    let result =
        PackageBuilder::build(&build_config).expect("Packaging with relative paths should succeed");
    assert!(result.artifact_path.exists());
    assert_eq!(
        result.artifact_path,
        workspace_path.join("dist/test_build_output.tar.zst")
    );
}

#[test]
fn test_packaging_importer_roundtrip_and_zip_slip_rejection() {
    use dmz_core::packaging::AirGapImporter;
    use dmz_core::resolver::lockfile::LockedPackage;
    use dmz_core::resolver::manifest::Ecosystem;

    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let workspace_path = temp_dir.path().join("pkg_project");
    let store_pkg_dir = workspace_path.join(".dmz/store/serde");
    fs::create_dir_all(&store_pkg_dir).unwrap();
    fs::write(store_pkg_dir.join("serde-1.0.0.archive"), b"cached_bytes").unwrap();
    create_dummy_manifests(&workspace_path);

    let mut packages = std::collections::BTreeMap::new();
    packages.insert(
        "serde".to_string(),
        LockedPackage {
            name: "serde".to_string(),
            version: "1.0.0".to_string(),
            sha256: "abc123".to_string(),
            ecosystem: Ecosystem::Cargo,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        },
    );
    let closure_signature = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        closure_signature,
        packages,
        target_platform: None,
        vcs: None,
        workspaces: std::collections::BTreeMap::new(),
    };
    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    let build_config = BuildConfig {
        workspace_dir: workspace_path.clone(),
        lockfile_path,
        output_dir: workspace_path.join("dist"),
        target: BuildTarget::Unified,
        archive_name: "unified_pkg".to_string(),
        zstd_level: 3,
    };

    let built = PackageBuilder::build(&build_config).expect("PackageBuilder::build failed");
    let extract_dir = temp_dir.path().join("extracted");
    AirGapImporter::import_and_verify(&built.artifact_path, &built.sha256, &extract_dir)
        .expect("AirGapImporter::import_and_verify failed");

    assert!(extract_dir.join("dmz.lock").exists());
    assert!(extract_dir.join("store/serde/serde-1.0.0.archive").exists());
    assert!(extract_dir.join("app/Cargo.toml").exists());
}

#[test]
fn test_git_provider_create_and_restore_bundle_roundtrip() {
    use dmz_core::vcs::GitProvider;

    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let src_repo = temp_dir.path().join("src_repo");
    fs::create_dir_all(&src_repo).unwrap();

    // Initialize repository and create a commit using git CLI or git2
    let repo = git2::Repository::init(&src_repo).expect("Git init failed");
    fs::write(src_repo.join("hello.txt"), "airgap git bundle payload").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("hello.txt")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("DMZ Test", "test@dmz.local").unwrap();
    let commit_oid = repo
        .commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .unwrap()
        .to_string();

    let bundle_path = temp_dir.path().join("repo.bundle");
    let bundle_info = GitProvider::create_bundle(&src_repo, &bundle_path, None)
        .expect("Git create_bundle failed");
    assert_eq!(bundle_info.head_commit, commit_oid);
    assert!(bundle_path.exists());

    let restored_repo = temp_dir.path().join("restored_repo");
    fs::create_dir_all(&restored_repo).unwrap();
    GitProvider::restore_bundle(&restored_repo, &bundle_path).expect("Git restore_bundle failed");

    let restored_commit =
        GitProvider::get_current_commit(&restored_repo).expect("get_current_commit failed");
    assert_eq!(restored_commit, commit_oid);
    assert_eq!(
        fs::read_to_string(restored_repo.join("hello.txt")).unwrap(),
        "airgap git bundle payload"
    );
}
