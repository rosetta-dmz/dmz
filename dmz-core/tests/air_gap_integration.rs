use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::packaging::AirGapImporter;
use std::collections::BTreeMap;
use std::fs::{self, File};
use sha2::{Sha256, Digest};

#[test]
fn test_air_gap_import_and_verification() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = temp_dir.path().join("workspace");
    let export_path = temp_dir.path().join("export");
    let extract_path = temp_dir.path().join("extracted");

    fs::create_dir_all(&workspace_path).unwrap();
    fs::create_dir_all(&export_path).unwrap();

    // 1. Create a mock dmz.lock file with the matching signature expected by load_and_verify
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
        },
    );

    let lockfile = DmzLockfile {
        version: 1,
        // Match the expected signature or prefix checked by your lockfile verification logic
        closure_signature: "4c378397e78464329d42b543b944d2824cdd9856abac26c0f26e323587289ab6".to_string(),
        packages,
    };
    let lock_json = serde_json::to_string_pretty(&lockfile).unwrap();
    fs::write(&lockfile_path, lock_json).unwrap();

    // 2. Pack workspace into a tar.zst archive
    let archive_path = export_path.join("closure.tar.zst");
    let tar_file = File::create(&archive_path).unwrap();
    let mut encoder = zstd::stream::Encoder::new(tar_file, 3).unwrap();
    {
        let mut tar_builder = tar::Builder::new(&mut encoder);
        tar_builder.append_dir_all(".", &workspace_path).unwrap();
        tar_builder.finish().unwrap();
    }
    encoder.finish().unwrap();

    // 3. Compute expected SHA-256 hash of the archive
    let mut f = File::open(&archive_path).unwrap();
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher).unwrap();
    let expected_hash = format!("{:x}", hasher.finalize());

    // 4. Test offline import and verification
    let result = AirGapImporter::import_and_verify(&archive_path, &expected_hash, &extract_path);
    assert!(result.is_ok(), "Air-gap import failed: {:?}", result.err());

    // 5. Verify unpacked contents exist
    assert!(extract_path.join("dmz.lock").exists());
}