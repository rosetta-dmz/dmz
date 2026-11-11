use std::fs;
use std::path::PathBuf;
use tar::Builder as TarBuilder;
use tempfile::tempdir;

use dmz_core::archive::exporter::AirGapManifest;
use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::toolchain::{ToolchainEcosystem, ToolchainManifestEntry};

#[test]
fn test_sidecar_sha256_generation_and_tamper_detection() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    let lockfile = DmzLockfile::new(Vec::new());
    lockfile.save(&ws.join("dmz.lock")).unwrap();

    let store_dir = ws.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();

    let output_archive = temp.path().join("dist/bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: ws.to_path_buf(),
        lockfile_path: ws.join("dmz.lock"),
        store_dir: store_dir.clone(),
        output_path: output_archive.clone(),
        ..Default::default()
    };

    let manifest = AirGapExporter::export(&export_cfg).expect("Export should succeed");

    // Verify sidecar file exists and contains matching SHA-256
    let sidecar_file = PathBuf::from(format!("{}.sha256", output_archive.display()));
    assert!(
        sidecar_file.exists(),
        "Sidecar .sha256 file must be generated"
    );
    let sidecar_content = fs::read_to_string(&sidecar_file).unwrap();
    assert!(sidecar_content.starts_with(&manifest.archive_sha256));

    // Successful import with verified sidecar
    let import_ws = tempdir().unwrap();
    let import_cfg = ImportConfig {
        archive_path: output_archive.clone(),
        target_store_dir: import_ws.path().join("store"),
        target_workspace_dir: import_ws.path().join("ws"),
        verify_closure_hash: true,
    };
    let import_res =
        AirGapImporter::import(&import_cfg).expect("Import with valid sidecar must succeed");
    assert!(import_res.workspace_path.join("Cargo.toml").exists());

    // Tamper detection: Corrupt sidecar hash and verify import fails
    fs::write(
        &sidecar_file,
        "0000000000000000000000000000000000000000000000000000000000000000  bundle.tar.zst\n",
    )
    .unwrap();
    let tamper_import_ws = tempdir().unwrap();
    let tamper_cfg = ImportConfig {
        archive_path: output_archive,
        target_store_dir: tamper_import_ws.path().join("store"),
        target_workspace_dir: tamper_import_ws.path().join("ws"),
        verify_closure_hash: true,
    };
    let tamper_err =
        AirGapImporter::import(&tamper_cfg).expect_err("Import with tampered sidecar must fail");
    assert!(tamper_err
        .to_string()
        .contains("Archive transport checksum mismatch"));
}

#[test]
fn test_pre_import_backup_snapshot_and_rollback_on_failure() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    let lockfile = DmzLockfile::new(Vec::new());
    lockfile.save(&ws.join("dmz.lock")).unwrap();

    let store_dir = ws.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();

    let output_archive = temp.path().join("dist/valid_bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: ws.to_path_buf(),
        lockfile_path: ws.join("dmz.lock"),
        store_dir: store_dir.clone(),
        output_path: output_archive.clone(),
        ..Default::default()
    };
    AirGapExporter::export(&export_cfg).expect("Export should succeed");

    // Create a target workspace with existing valuable user files
    let target_temp = tempdir().unwrap();
    let target_ws = target_temp.path();
    let precious_file = target_ws.join("uncommitted_work.txt");
    fs::write(&precious_file, "CRITICAL DATA DO NOT LOSE").unwrap();

    // Import valid bundle: verifies backup snapshot is automatically created
    let import_cfg = ImportConfig {
        archive_path: output_archive,
        target_store_dir: target_ws.join(".dmz/store"),
        target_workspace_dir: target_ws.to_path_buf(),
        verify_closure_hash: true,
    };
    let res = AirGapImporter::import(&import_cfg).expect("Import should succeed");
    assert!(res.workspace_path.join("Cargo.toml").exists());
    assert_eq!(
        fs::read_to_string(&precious_file).unwrap(),
        "CRITICAL DATA DO NOT LOSE"
    );

    let backups_dir = target_ws.join(".dmz/backups");
    assert!(
        backups_dir.exists(),
        "Pre-import backup snapshot must be stored in .dmz/backups"
    );
    let snapshots: Vec<_> = fs::read_dir(&backups_dir).unwrap().flatten().collect();
    assert!(
        !snapshots.is_empty(),
        "At least one backup snapshot must exist"
    );
    assert!(snapshots[0].path().join("uncommitted_work.txt").exists());

    // Now test rollback on failure: corrupted bundle
    let corrupt_archive = temp.path().join("dist/corrupted.tar.zst");
    fs::write(&corrupt_archive, b"not a valid zstd archive").unwrap();
    let corrupt_import_cfg = ImportConfig {
        archive_path: corrupt_archive,
        target_store_dir: target_ws.join(".dmz/store"),
        target_workspace_dir: target_ws.to_path_buf(),
        verify_closure_hash: true,
    };
    let err = AirGapImporter::import(&corrupt_import_cfg)
        .expect_err("Importing corrupted bundle must fail");
    assert!(
        err.to_string().contains("Archive Extraction Failed")
            || err.to_string().contains("zstd decoder")
    );
    // Ensure precious file is still intact after failed import
    assert_eq!(
        fs::read_to_string(&precious_file).unwrap(),
        "CRITICAL DATA DO NOT LOSE"
    );
}

#[test]
fn test_export_fails_fast_when_locked_store_artifact_is_missing_and_import_verifies_declared_toolchain_binaries(
) {
    let temp = tempdir().unwrap();
    let src_ws = temp.path().join("incomplete_src");
    fs::create_dir_all(src_ws.join("src")).unwrap();
    fs::write(
        src_ws.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src_ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    // Lockfile declares a package that is missing from .dmz/store
    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "missing-crate".into(),
        version: "1.0.0".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    }]);
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();
    fs::create_dir_all(src_ws.join(".dmz/store")).unwrap();

    let archive_path = temp.path().join("dist/should_fail.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: src_ws.clone(),
        lockfile_path: src_ws.join("dmz.lock"),
        store_dir: src_ws.join(".dmz/store"),
        output_path: archive_path.clone(),
        with_toolchain: false,
        ..Default::default()
    };
    let err = AirGapExporter::export(&export_cfg)
        .expect_err("Export must fail fast when a locked package is missing from .dmz/store");
    assert!(
        err.to_string().contains("Missing store artifact"),
        "Expected Missing store artifact error, got: {}",
        err
    );

    let broken_tc_archive = temp.path().join("dist/broken_tc.tar.zst");
    fs::create_dir_all(broken_tc_archive.parent().unwrap()).unwrap();
    {
        let file = fs::File::create(&broken_tc_archive).unwrap();
        let zstd_enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
        let mut tar = TarBuilder::new(zstd_enc);

        let empty_lock = DmzLockfile::new(vec![]);
        let lock_json = serde_json::to_string_pretty(&empty_lock).unwrap();
        let mut lhdr = tar::Header::new_gnu();
        lhdr.set_size(lock_json.len() as u64);
        lhdr.set_mode(0o644);
        lhdr.set_cksum();
        tar.append_data(&mut lhdr, "dmz.lock", lock_json.as_bytes())
            .unwrap();

        let cargo_toml = b"[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let mut chdr = tar::Header::new_gnu();
        chdr.set_size(cargo_toml.len() as u64);
        chdr.set_mode(0o644);
        chdr.set_cksum();
        tar.append_data(&mut chdr, "Cargo.toml", &cargo_toml[..])
            .unwrap();

        let manifest = AirGapManifest {
            dmz_version: "1.2.7".into(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: vec![ToolchainManifestEntry {
                ecosystem: ToolchainEcosystem::Rust,
                version: "1.80.0".into(),
                path_in_archive: "toolchains/rust".into(),
                binaries: vec!["cargo".into(), "rustc".into()],
            }],
            workspace_project: None,
            closure_signature: empty_lock.closure_signature.clone(),
            package_count: 0,
            created_at_utc: 0,
            archive_sha256: String::new(),
        };
        let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
        let mut mhdr = tar::Header::new_gnu();
        mhdr.set_size(manifest_json.len() as u64);
        mhdr.set_mode(0o644);
        mhdr.set_cksum();
        tar.append_data(&mut mhdr, "airgap_manifest.json", manifest_json.as_bytes())
            .unwrap();

        // Only pack cargo, omitting rustc!
        let cargo_bin = b"#!/bin/sh\necho cargo\n";
        let mut bhdr = tar::Header::new_gnu();
        bhdr.set_size(cargo_bin.len() as u64);
        bhdr.set_mode(0o755);
        bhdr.set_cksum();
        tar.append_data(&mut bhdr, "toolchains/rust/bin/cargo", &cargo_bin[..])
            .unwrap();

        let zstd_enc = tar.into_inner().unwrap();
        zstd_enc.finish().unwrap();
    }

    let import_target = temp.path().join("broken_tc_imported");
    let import_cfg = ImportConfig {
        archive_path: broken_tc_archive,
        target_store_dir: import_target.join(".dmz/store"),
        target_workspace_dir: import_target,
        verify_closure_hash: true,
    };
    let import_err = AirGapImporter::import(&import_cfg)
        .expect_err("Import must fail when a declared toolchain binary is missing");
    assert!(
        import_err
            .to_string()
            .contains("missing declared binary 'rustc'"),
        "Expected missing declared binary error, got: {}",
        import_err
    );
}
