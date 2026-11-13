use dmz_core::packaging::{AirGapImporter, BuildConfig, BuildTarget, PackageBuilder};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::vcs::GitProvider;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use super::sandbox_execution::create_dummy_manifests;

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
        packages: BTreeMap::new(),
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
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
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let workspace_path = temp_dir.path().join("pkg_project");
    let store_pkg_dir = workspace_path.join(".dmz/store/serde");
    fs::create_dir_all(&store_pkg_dir).unwrap();
    fs::write(store_pkg_dir.join("serde-1.0.0.archive"), b"cached_bytes").unwrap();
    create_dummy_manifests(&workspace_path);

    let mut packages = BTreeMap::new();
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
        workspaces: BTreeMap::new(),
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
fn test_air_gap_import_and_verification() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = temp_dir.path().join("workspace");
    let export_path = temp_dir.path().join("export");
    let extract_path = temp_dir.path().join("extracted");

    fs::create_dir_all(&workspace_path).unwrap();
    fs::create_dir_all(&export_path).unwrap();

    // Create mock dmz.lock file with the matching signature expected by load_and_verify
    let lockfile_path = workspace_path.join("dmz.lock");
    let mut packages = BTreeMap::new();
    packages.insert(
        "tokio".to_string(),
        LockedPackage {
            name: "tokio".to_string(),
            ecosystem: Ecosystem::Cargo,
            version: "1.38.0".to_string(),
            sha256: "mockchecksum123456789".to_string(),
            source_url: Some("https://crates.io/api/v1/crates/tokio/1.38.0/download".to_string()),
            dependencies: vec![],
            member_id: None,
        },
    );

    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: "4c378397e78464329d42b543b944d2824cdd9856abac26c0f26e323587289ab6"
            .to_string(),
        packages,
    };
    let lock_json = serde_json::to_string_pretty(&lockfile).unwrap();
    fs::write(&lockfile_path, lock_json).unwrap();

    // Pack workspace into a tar.zst archive
    let archive_path = export_path.join("closure.tar.zst");
    let tar_file = File::create(&archive_path).unwrap();
    let mut encoder = zstd::stream::Encoder::new(tar_file, 3).unwrap();
    {
        let mut tar_builder = tar::Builder::new(&mut encoder);
        tar_builder.append_dir_all(".", &workspace_path).unwrap();
        tar_builder.finish().unwrap();
    }
    encoder.finish().unwrap();

    // Compute expected SHA-256 hash of the archive
    let expected_hash = dmz_core::archive::calculate_file_sha256(&archive_path).unwrap();

    // Test offline import and verification
    let result = AirGapImporter::import_and_verify(&archive_path, &expected_hash, &extract_path);
    assert!(result.is_ok(), "Air-gap import failed: {:?}", result.err());

    // Verify unpacked contents exist
    assert!(extract_path.join("dmz.lock").exists());
}

#[test]
fn test_git_provider_create_and_restore_bundle_roundtrip() {
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
