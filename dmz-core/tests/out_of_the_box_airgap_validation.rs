// Air-Gap Validation
// Validates Root Manifest Dominance and ensures exported bundles are strictly complete
// and ready out-of-the-box

use dmz_core::toolchain::{
    DiscoveredToolchain, ToolchainArchivePackager, ToolchainEcosystem, ToolchainPackager,
    ToolchainProvider,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tempfile::tempdir;

/// Helper to create mock binary executable files.
fn create_mock_bin(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let bin_path = dir.join(name);
    fs::write(&bin_path, "#!/bin/sh\necho mock\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755));
    }
    bin_path
}

#[test]
fn test_declared_cmake_workspace_correctly_configures_cmake_and_compilers() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // Create a workspace that uses CMake and C++
    fs::write(
        ws.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(native_app C CXX)\n",
    )
    .unwrap();

    let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);

    assert!(
        detected.contains(&ToolchainEcosystem::CMake),
        "CMake must be detected from root CMakeLists.txt"
    );
    assert!(
        detected.contains(&ToolchainEcosystem::Cc),
        "Cc must be inferred companion for CMake"
    );
    assert!(
        !detected.contains(&ToolchainEcosystem::Ninja),
        "Ninja must not be falsely inferred companion build generator for CMake"
    );

    // Simulate bundled CMake, CC, and Ninja toolchains
    let target_dir = tempdir().unwrap();
    let target_ws = target_dir.path();
    fs::write(
        target_ws.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(native_app C CXX)\n",
    )
    .unwrap();

    create_mock_bin(&target_ws.join(".dmz/toolchains/cmake/bin"), "cmake");
    create_mock_bin(&target_ws.join(".dmz/toolchains/cmake/bin"), "ctest");
    create_mock_bin(&target_ws.join(".dmz/toolchains/cc/bin"), "clang");
    create_mock_bin(&target_ws.join(".dmz/toolchains/cc/bin"), "clang++");
    create_mock_bin(&target_ws.join(".dmz/toolchains/ninja/bin"), "ninja");

    // Execute target setup
    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    // Verify dmz.toolchain.cmake IS generated
    let cmake_tc = target_ws.join(".dmz/toolchains/dmz.toolchain.cmake");
    assert!(cmake_tc.exists(), "dmz.toolchain.cmake must be generated");

    let cmake_content = fs::read_to_string(&cmake_tc).unwrap();
    assert!(cmake_content.contains("CMAKE_C_COMPILER"));
    assert!(cmake_content.contains("CMAKE_CXX_COMPILER"));
    assert!(cmake_content.contains("CMAKE_MAKE_PROGRAM"));

    // Verify .vscode/settings.json configures CMake Tools
    let settings_path = target_ws.join(".vscode/settings.json");
    assert!(settings_path.exists());
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();

    assert_eq!(settings["cmake.configureOnOpen"], true);
    assert_eq!(
        settings["cmake.cmakePath"],
        "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
    );
    assert_eq!(
        settings["cmake.configureSettings"]["CMAKE_TOOLCHAIN_FILE"],
        "${workspaceFolder}/.dmz/toolchains/dmz.toolchain.cmake"
    );

    // Verify injected environment
    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut env_map);

    assert!(env_map.contains_key("CMAKE"));
    assert!(env_map.contains_key("CMAKE_TOOLCHAIN_FILE"));
    assert!(env_map.contains_key("CC"));
    assert!(env_map.contains_key("CXX"));
    assert!(env_map.contains_key("NINJA"));
    assert_eq!(env_map.get("CMAKE_GENERATOR").unwrap(), "Ninja");
}

#[test]
fn test_sidecar_sha256_generation_and_tamper_detection() {
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
    use dmz_core::resolver::lockfile::DmzLockfile;

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
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
    use dmz_core::resolver::lockfile::DmzLockfile;

    let temp = tempdir().unwrap();
    let ws = temp.path();

    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "cmake".into(),
        version: "0.1.50".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    }]);
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();

    // 2. Verify detection includes Rust, CMake, Cc, and Ninja (and companion AppleSdk on macOS)
    let detected = ToolchainProvider::detect_workspace_ecosystems(&src_ws, Some(&lockfile));
    assert!(detected.contains(&ToolchainEcosystem::Rust));
    assert!(detected.contains(&ToolchainEcosystem::CMake));
    assert!(detected.contains(&ToolchainEcosystem::Cc));
    assert!(!detected.contains(&ToolchainEcosystem::Ninja));
    #[cfg(target_os = "macos")]
    assert!(detected.contains(&ToolchainEcosystem::AppleSdk));
    #[cfg(not(target_os = "macos"))]
    assert!(!detected.contains(&ToolchainEcosystem::AppleSdk));

    // 3. Package bundle containing Rust, CMake, Cc, and Ninja toolchains
    let tc_stage = temp.path().join("tc_stage");
    let rust_bin = tc_stage.join("rust/bin");
    let cmake_bin = tc_stage.join("cmake/bin");
    let cc_bin = tc_stage.join("cc/bin");
    let ninja_bin = tc_stage.join("ninja/bin");
    create_mock_bin(&rust_bin, "cargo");
    create_mock_bin(&rust_bin, "rustc");
    create_mock_bin(&cmake_bin, "cmake");
    create_mock_bin(&cc_bin, "clang");
    create_mock_bin(&cc_bin, "clang++");
    create_mock_bin(&ninja_bin, "ninja");

    let discovered = vec![
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "1.80.0".into(),
            root_dir: tc_stage.join("rust"),
            bin_files: vec![
                (rust_bin.join("cargo"), "bin/cargo".into()),
                (rust_bin.join("rustc"), "bin/rustc".into()),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::CMake,
            version: "3.28.0".into(),
            root_dir: tc_stage.join("cmake"),
            bin_files: vec![(cmake_bin.join("cmake"), "bin/cmake".into())],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version: "17.0.0".into(),
            root_dir: tc_stage.join("cc"),
            bin_files: vec![
                (cc_bin.join("clang"), "bin/clang".into()),
                (cc_bin.join("clang++"), "bin/clang++".into()),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ninja,
            version: "1.11.1".into(),
            root_dir: tc_stage.join("ninja"),
            bin_files: vec![(ninja_bin.join("ninja"), "bin/ninja".into())],
            lib_files: vec![],
        },
    ];

    let bundle_path = temp.path().join("dist/rust_cmake_bundle.tar.zst");
    fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    {
        let file = fs::File::create(&bundle_path).unwrap();
        let zstd_enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
        let mut tar = TarBuilder::new(zstd_enc);

    let store_dir = ws.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();

        let tc_entries = ToolchainArchivePackager::package(&mut tar, &discovered).unwrap();
        let manifest = AirGapManifest {
            dmz_version: "1.2.7".into(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: tc_entries,
            workspace_project: None,
            closure_signature: lockfile.closure_signature.clone(),
            package_count: 1,
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
    use dmz_core::archive::exporter::AirGapManifest;
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
    use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
    use dmz_core::resolver::manifest::Ecosystem;
    use dmz_core::toolchain::ToolchainManifestEntry;

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

#[test]
fn test_hermetic_end_to_end_complete_codebase_bundle_import_and_stripped_path_verification() {
    use dmz_core::archive::exporter::AirGapManifest;
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ImportConfig};
    use dmz_core::ecosystem::EcosystemRegistry;
    use dmz_core::install::{InstallConfig, InstallerEngine};
    use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
    use dmz_core::resolver::manifest::Ecosystem;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256};

    let temp = tempdir().unwrap();
    let src_ws = temp.path().join("complete_polyglot_src");
    fs::create_dir_all(src_ws.join("src")).unwrap();

    // Write complete polyglot workspace manifests (Rust + Python + Node + Go + CMake)
    fs::write(
        src_ws.join("Cargo.toml"),
        r#"[package]
name = "hermetic-app"
version = "0.1.0"
edition = "2021"

[dependencies]
mini-util = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(src_ws.join("src/lib.rs"), "pub fn check() -> u32 { 42 }\n").unwrap();
    fs::write(
        src_ws.join("pyproject.toml"),
        "[project]\nname = \"py-app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_ws.join("app.py"), "def run():\n    return 'ok'\n").unwrap();
    fs::write(
        src_ws.join("package.json"),
        r#"{"name": "node-app", "version": "1.0.0", "dependencies": {"js-util": "1.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        src_ws.join("go.mod"),
        "module example.com/hermetic\n\ngo 1.22\n",
    )
    .unwrap();
    fs::write(src_ws.join("main.go"), "package main\nfunc main() {}\n").unwrap();
    fs::write(
        src_ws.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(hermetic_native C CXX)\n",
    )
    .unwrap();

    // Build real SHA-256 verified store artifacts for Cargo (`mini-util`) and npm (`js-util`)
    let store_dir = src_ws.join(".dmz/store");
    fs::create_dir_all(store_dir.join("mini-util")).unwrap();
    let crate_archive_path = store_dir.join("mini-util/mini-util-0.1.0.archive");
    {
        let file = fs::File::create(&crate_archive_path).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = TarBuilder::new(enc);
        let toml_bytes =
            b"[package]\nname = \"mini-util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
        let mut h1 = tar::Header::new_gnu();
        h1.set_size(toml_bytes.len() as u64);
        h1.set_mode(0o644);
        h1.set_cksum();
        tar.append_data(&mut h1, "mini-util-0.1.0/Cargo.toml", &toml_bytes[..])
            .unwrap();
        let lib_bytes = b"pub fn val() -> u32 { 1 }\n";
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(lib_bytes.len() as u64);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, "mini-util-0.1.0/src/lib.rs", &lib_bytes[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let crate_sha = format!(
        "{:x}",
        Sha256::digest(fs::read(&crate_archive_path).unwrap())
    );

    fs::create_dir_all(store_dir.join("js-util")).unwrap();
    let npm_archive_path = store_dir.join("js-util/js-util-1.0.0.archive");
    {
        let file = fs::File::create(&npm_archive_path).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = TarBuilder::new(enc);
        let pkg_json = br#"{"name": "js-util", "version": "1.0.0", "main": "index.js"}"#;
        let mut h1 = tar::Header::new_gnu();
        h1.set_size(pkg_json.len() as u64);
        h1.set_mode(0o644);
        h1.set_cksum();
        tar.append_data(&mut h1, "package/package.json", &pkg_json[..])
            .unwrap();
        let idx_js = b"module.exports = 42;\n";
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(idx_js.len() as u64);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, "package/index.js", &idx_js[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let npm_sha = format!("{:x}", Sha256::digest(fs::read(&npm_archive_path).unwrap()));

    let lockfile = DmzLockfile::new(vec![
        LockedPackage {
            name: "mini-util".into(),
            version: "0.1.0".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: crate_sha,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        },
        LockedPackage {
            name: "js-util".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Npm,
            sha256: npm_sha,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        },
    ]);
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();

    // Create self-contained toolchain binaries for the declared ecosystems
    // (Rust, Python, Node, Go, CMake, Cc, Ninja) that validate offline flags and workspace state
    let tc_stage = temp.path().join("tc_stage");
    let make_script = |dir: &Path, name: &str, body: &str| -> (PathBuf, String) {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\nset -e\n{}\n", body)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
        }
        (p, format!("bin/{}", name))
    };

    let discovered_toolchains = vec![
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "rustc 1.80.0".into(),
            root_dir: tc_stage.join("rust"),
            bin_files: vec![
                // Bundled cargo verifies that --offline is passed and .cargo/vendor/mini-util-0.1.0 exists!
                make_script(
                    &tc_stage.join("rust/bin"),
                    "cargo",
                    "test \"$1\" = \"check\" && test \"$2\" = \"--offline\" && test -f .cargo/vendor/mini-util-0.1.0/Cargo.toml",
                ),
                make_script(&tc_stage.join("rust/bin"), "rustc", "echo rustc 1.80.0"),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Python,
            version: "Python 3.12.0".into(),
            root_dir: tc_stage.join("python"),
            bin_files: vec![make_script(
                &tc_stage.join("python/bin"),
                "python3",
                "test \"$1\" = \"-m\" && test \"$2\" = \"compileall\" && test -f app.py",
            )],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            version: "v20.11.0".into(),
            root_dir: tc_stage.join("node"),
            bin_files: vec![
                make_script(&tc_stage.join("node/bin"), "node", "echo v20.11.0"),
                make_script(
                    &tc_stage.join("node/bin"),
                    "npm",
                    "test \"$1\" = \"ls\" && test \"$2\" = \"--offline\" && test -f node_modules/js-util/package.json",
                ),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Go,
            version: "go1.22.0".into(),
            root_dir: tc_stage.join("go"),
            bin_files: vec![make_script(
                &tc_stage.join("go/bin"),
                "go",
                "test \"$1\" = \"build\" && test -f main.go",
            )],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::CMake,
            version: "cmake 3.28.0".into(),
            root_dir: tc_stage.join("cmake"),
            bin_files: vec![make_script(
                &tc_stage.join("cmake/bin"),
                "cmake",
                "test -f \"$CMAKE_TOOLCHAIN_FILE\"",
            )],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version: "clang 17.0.0".into(),
            root_dir: tc_stage.join("cc"),
            bin_files: vec![
                make_script(&tc_stage.join("cc/bin"), "clang", "echo clang 17.0.0"),
                make_script(&tc_stage.join("cc/bin"), "clang++", "echo clang++ 17.0.0"),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ninja,
            version: "1.11.1".into(),
            root_dir: tc_stage.join("ninja"),
            bin_files: vec![make_script(
                &tc_stage.join("ninja/bin"),
                "ninja",
                "echo 1.11.1",
            )],
            lib_files: vec![],
        },
    ];

    // Package complete bundle (.tar.zst) with source, store, lockfile, and discovered toolchains + .sha256 sidecar
    let bundle_path = temp.path().join("dist/complete_hermetic_bundle.tar.zst");
    fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    {
        let file = fs::File::create(&bundle_path).unwrap();
        let zstd_enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
        let mut tar = TarBuilder::new(zstd_enc);

        tar.append_path_with_name(src_ws.join("dmz.lock"), "dmz.lock")
            .unwrap();
        tar.append_path_with_name(
            &crate_archive_path,
            "store/mini-util/mini-util-0.1.0.archive",
        )
        .unwrap();
        tar.append_path_with_name(&npm_archive_path, "store/js-util/js-util-1.0.0.archive")
            .unwrap();
        for f in &[
            "Cargo.toml",
            "src/lib.rs",
            "pyproject.toml",
            "app.py",
            "package.json",
            "go.mod",
            "main.go",
            "CMakeLists.txt",
        ] {
            tar.append_path_with_name(src_ws.join(f), f).unwrap();
        }

        let tc_entries =
            ToolchainArchivePackager::package(&mut tar, &discovered_toolchains).unwrap();
        let manifest = AirGapManifest {
            dmz_version: "1.2.7".into(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: tc_entries,
            workspace_project: None,
            closure_signature: lockfile.closure_signature.clone(),
            package_count: 2,
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

        let zstd_enc = tar.into_inner().unwrap();
        zstd_enc.finish().unwrap();
    }
    let archive_sha = AirGapExporter::calculate_file_sha256(&bundle_path).unwrap();
    fs::write(
        format!("{}.sha256", bundle_path.display()),
        format!("{}  complete_hermetic_bundle.tar.zst\n", archive_sha),
    )
    .unwrap();

    // Import onto a clean target machine directory & materialize offline dependencies
    let target_ws = temp.path().join("airgap_target_machine");
    let target_store = target_ws.join(".dmz/store");
    let import_res = AirGapImporter::import(&ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store.clone(),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    })
    .expect("Complete bundle import must succeed");
    assert!(import_res.has_toolchains);
    assert_eq!(import_res.imported_packages, 2);

    let installed_count = InstallerEngine::install(&InstallConfig {
        workspace_path: target_ws.clone(),
        lockfile_path: target_ws.join("dmz.lock"),
        store_dir: target_store,
    })
    .expect("Offline dependency materialization must succeed");
    assert_eq!(installed_count, 2);

    // Execute post-import verification across ALL active ecosystems + native build tools
    // under a strictly stripped PATH (ONLY .dmz/toolchains/*/bin and /bin)
    #[cfg(unix)]
    {
        let mut hermetic_env = HashMap::new();
        // Start with ONLY /bin so no host cargo, python3, npm, go, or cmake can leak from /usr/local or /opt/homebrew
        hermetic_env.insert("PATH".to_string(), "/bin".to_string());
        ToolchainPackager::inject_toolchain_paths(&target_ws, &mut hermetic_env);

        let registry = EcosystemRegistry::new();
        let active_handlers = registry.discover_active(&target_ws);
        assert_eq!(
            active_handlers.len(),
            4,
            "Must discover Rust, npm, Go, and Python handlers"
        );

        for handler in active_handlers {
            let cmd = handler
                .verification_command(&target_ws)
                .expect("Handler must provide verification command");
            let status = std::process::Command::new(&cmd.program)
                .args(&cmd.args)
                .current_dir(&target_ws)
                .env_clear()
                .envs(&hermetic_env)
                .status()
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to spawn bundled verification binary '{}' for ecosystem '{}': {}",
                        cmd.program,
                        handler.name(),
                        e
                    )
                });
            assert!(
                status.success(),
                "Hermetic offline verification failed for ecosystem '{}' ({} {:?})",
                handler.name(),
                cmd.program,
                cmd.args
            );
        }

        // Verify bundled native build tools (cmake, clang, ninja) execute from stripped PATH
        // and that CMAKE_TOOLCHAIN_FILE points to the generated dmz.toolchain.cmake
        for native_tool in &["cmake", "clang", "ninja"] {
            let status = std::process::Command::new(native_tool)
                .current_dir(&target_ws)
                .env_clear()
                .envs(&hermetic_env)
                .status()
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to execute bundled native build tool '{}' under stripped PATH: {}",
                        native_tool, e
                    )
                });
            assert!(
                status.success(),
                "Bundled native build tool '{}' returned non-zero under stripped PATH",
                native_tool
            );
        }
    }
}

#[test]
fn test_definitive_multi_ecosystem_compilation_proof() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    fn link_or_copy_dir(src: &Path, dst: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let _ = symlink(src, dst);
        }
        #[cfg(windows)]
        {
            let res = std::process::Command::new("cmd")
                .arg("/c")
                .arg("mklink")
                .arg("/J")
                .arg(dst)
                .arg(src)
                .output();
            if res.is_err() || !res.unwrap().status.success() {
                if std::os::windows::fs::symlink_dir(src, dst).is_err() {
                    fn copy_rec(s: &Path, d: &Path) {
                        let _ = fs::create_dir_all(d);
                        if let Ok(entries) = fs::read_dir(s) {
                            for e in entries.flatten() {
                                let p = e.path();
                                let target = d.join(e.file_name());
                                if p.is_dir() {
                                    copy_rec(&p, &target);
                                } else {
                                    let _ = fs::copy(&p, &target);
                                }
                            }
                        }
                    }
                    copy_rec(src, dst);
                }
            }
        }
    }

    // Verify Go compilation with bundled standard library sources ($GOROOT/src)
    if let Ok(go_bin) = which::which("go") {
        if let Ok(output) = std::process::Command::new(&go_bin)
            .arg("env")
            .arg("GOROOT")
            .output()
        {
            if output.status.success() {
                let host_goroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                if host_goroot.join("src").exists() {
                    let go_target_dir = target_ws.join(".dmz/toolchains/go");
                    let go_bin_dir = go_target_dir.join("bin");
                    fs::create_dir_all(&go_bin_dir).unwrap();
                    let go_exe_name = if cfg!(windows) { "go.exe" } else { "go" };
                    let _ = fs::copy(&go_bin, go_bin_dir.join(go_exe_name));

                    if host_goroot.join("pkg").exists() {
                        link_or_copy_dir(&host_goroot.join("pkg"), &go_target_dir.join("pkg"));
                    }
                    link_or_copy_dir(&host_goroot.join("src"), &go_target_dir.join("src"));

                    fs::write(
                        target_ws.join("main.go"),
                        "package main\nimport \"fmt\"\nfunc main() {\n    fmt.Println(\"Hermetic Go Success\")\n}\n",
                    ).unwrap();

                    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

                    let mut envs = HashMap::new();
                    ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

                    let go_app_name = if cfg!(windows) {
                        "go_app.exe"
                    } else {
                        "go_app"
                    };
                    let go_build = std::process::Command::new(go_bin_dir.join(go_exe_name))
                        .arg("build")
                        .arg("-o")
                        .arg(target_ws.join(go_app_name))
                        .arg(target_ws.join("main.go"))
                        .current_dir(target_ws)
                        .envs(&envs)
                        .output()
                        .expect("Failed to invoke bundled go build");

                    assert!(
                        go_build.status.success(),
                        "Go build failed: stdout={}\nstderr={}",
                        String::from_utf8_lossy(&go_build.stdout),
                        String::from_utf8_lossy(&go_build.stderr)
                    );

                    let go_run = std::process::Command::new(target_ws.join(go_app_name))
                        .output()
                        .expect("Failed to execute compiled go binary");
                    assert!(go_run.status.success());
                    assert!(String::from_utf8_lossy(&go_run.stdout).contains("Hermetic Go Success"));
                }
            }
        }
    }

    // 2. Definitively verify Python execution via bundled python / python3 binary with stdlib
    let py_candidates = ["python3", "python"];
    let py_bin_opt = py_candidates
        .iter()
        .find_map(|name| which::which(name).ok());

    if let Some(py_bin) = py_bin_opt {
        let py_target_dir = target_ws.join(".dmz/toolchains/python");
        let py_bin_dir = py_target_dir.join("bin");
        fs::create_dir_all(&py_bin_dir).unwrap();
        let _ = fs::copy(&py_bin, py_bin_dir.join("python3"));
        let _ = fs::copy(&py_bin, py_bin_dir.join("python"));

        if cfg!(windows) {
            let _ = fs::copy(&py_bin, py_bin_dir.join("python.exe"));
            let _ = fs::copy(&py_bin, py_bin_dir.join("python3.exe"));
            let _ = fs::copy(&py_bin, py_target_dir.join("python.exe"));

            if let Some(parent) = py_bin.parent() {
                if let Ok(entries) = fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                                if ext.eq_ignore_ascii_case("dll") {
                                    let fname = entry.file_name();
                                    let _ = fs::copy(&p, py_target_dir.join(&fname));
                                    let _ = fs::copy(&p, py_bin_dir.join(&fname));
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Ok(output) = std::process::Command::new(&py_bin)
            .arg("-c")
            .arg("import sysconfig; print(sysconfig.get_path('stdlib'))")
            .output()
        {
            if output.status.success() {
                let stdlib_dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                if stdlib_dir.exists() {
                    let folder_name = stdlib_dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("python3");

                    if cfg!(windows) || folder_name.eq_ignore_ascii_case("Lib") {
                        let target_lib = py_target_dir.join("Lib");
                        link_or_copy_dir(&stdlib_dir, &target_lib);
                        let target_lib_lower = py_target_dir.join("lib");
                        if !target_lib_lower.exists() {
                            link_or_copy_dir(&stdlib_dir, &target_lib_lower);
                        }
                    } else {
                        let target_lib = py_target_dir.join("lib");
                        fs::create_dir_all(&target_lib).unwrap();
                        link_or_copy_dir(&stdlib_dir, &target_lib.join(folder_name));
                    }
                }
            }
        }

        ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

        let mut envs = HashMap::new();
        ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

        let py_run_bin = if cfg!(windows) {
            py_bin_dir.join("python.exe")
        } else {
            py_bin_dir.join("python")
        };

        let py_exec = std::process::Command::new(&py_run_bin)
            .arg("-c")
            .arg("import sys; print(f'Hermetic Python {sys.version_info.major} Success')")
            .envs(&envs)
            .output()
            .expect("Failed to invoke bundled python");

        assert!(
            py_exec.status.success(),
            "Python execution failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&py_exec.stdout),
            String::from_utf8_lossy(&py_exec.stderr)
        );
        assert!(String::from_utf8_lossy(&py_exec.stdout).contains("Hermetic Python"));
    }

    // 3. Definitively verify Node.js runtime and companion package managers (npm, pnpm)
    if let Ok(node_bin) = which::which("node") {
        let node_root = target_ws.join(".dmz/toolchains/node");
        let node_bin_dir = node_root.join("bin");
        fs::create_dir_all(&node_bin_dir).unwrap();
        let _ = fs::copy(&node_bin, node_bin_dir.join("node"));

        // If host has lib/node_modules or libnode dylibs, wire them
        let canonical_node = fs::canonicalize(&node_bin).unwrap_or_else(|_| node_bin.clone());
        if let Some(host_node_root) = canonical_node.parent().and_then(|p| p.parent()) {
            let host_lib = host_node_root.join("lib");
            if host_lib.exists() {
                let target_lib = node_root.join("lib");
                fs::create_dir_all(&target_lib).unwrap();
                if let Ok(entries) = fs::read_dir(&host_lib) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        let fname = entry.file_name();
                        let s = fname.to_string_lossy();
                        if p.is_file() && s.starts_with("libnode") {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::symlink;
                                let _ = symlink(&p, target_lib.join(&*s));
                            }
                            #[cfg(windows)]
                            {
                                let _ = fs::copy(&p, target_lib.join(&*s));
                            }
                        }
                    }
                }

                let host_nm = host_lib.join("node_modules");
                if host_nm.exists() {
                    let target_nm = target_lib.join("node_modules");
                    fs::create_dir_all(&target_nm).unwrap();
                    if host_nm.join("npm").exists() {
                        link_or_copy_dir(&host_nm.join("npm"), &target_nm.join("npm"));
                    }
                    if host_nm.join("pnpm").exists() {
                        link_or_copy_dir(&host_nm.join("pnpm"), &target_nm.join("pnpm"));
                    }
                }
            }
        }

        ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

        let mut envs = HashMap::new();
        ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

        let node_exec = std::process::Command::new(node_bin_dir.join("node"))
            .arg("-e")
            .arg("console.log('Hermetic Node Success')")
            .envs(&envs)
            .output()
            .expect("Failed to invoke bundled node");

        assert!(
            node_exec.status.success(),
            "Node execution failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&node_exec.stdout),
            String::from_utf8_lossy(&node_exec.stderr)
        );
        assert!(String::from_utf8_lossy(&node_exec.stdout).contains("Hermetic Node Success"));

        // If npm was wired, verify npm --version works
        if node_bin_dir.join("npm").exists() {
            let npm_exec = std::process::Command::new(node_bin_dir.join("npm"))
                .arg("--version")
                .envs(&envs)
                .output()
                .expect("Failed to invoke bundled npm");
            assert!(
                npm_exec.status.success(),
                "npm execution failed: stdout={}\nstderr={}",
                String::from_utf8_lossy(&npm_exec.stdout),
                String::from_utf8_lossy(&npm_exec.stderr)
            );
        }

        // If pnpm was wired, verify pnpm --version works
        if node_bin_dir.join("pnpm").exists() {
            let pnpm_exec = std::process::Command::new(node_bin_dir.join("pnpm"))
                .arg("--version")
                .envs(&envs)
                .output()
                .expect("Failed to invoke bundled pnpm");
            assert!(
                pnpm_exec.status.success(),
                "pnpm execution failed: stdout={}\nstderr={}",
                String::from_utf8_lossy(&pnpm_exec.stdout),
                String::from_utf8_lossy(&pnpm_exec.stderr)
            );
        }
    }

    // Verify standalone Make under clean target machine conditions
    #[cfg(target_os = "macos")]
    {
        let make_candidates = [
            "/Applications/Xcode.app/Contents/Developer/usr/bin/make",
            "/Library/Developer/CommandLineTools/usr/bin/make",
        ];
        if let Some(make_src) = make_candidates.iter().find(|p| Path::new(p).exists()) {
            let make_bin_dir = target_ws.join(".dmz/toolchains/make/bin");
            fs::create_dir_all(&make_bin_dir).unwrap();
            let _ = fs::copy(make_src, make_bin_dir.join("make"));

            fs::write(
                target_ws.join("Makefile"),
                "all:\n\t@echo \"Hermetic Make Success\"\n",
            )
            .unwrap();

            ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

            let make_exec = std::process::Command::new(make_bin_dir.join("make"))
                .current_dir(target_ws)
                .env(
                    "DEVELOPER_DIR",
                    "/nonexistent_developer_path_for_clean_target_machine",
                )
                .output()
                .expect("Failed to invoke bundled make");

            assert!(
                make_exec.status.success(),
                "Make failed under clean machine conditions: stdout={}\nstderr={}",
                String::from_utf8_lossy(&make_exec.stdout),
                String::from_utf8_lossy(&make_exec.stderr)
            );
            assert!(String::from_utf8_lossy(&make_exec.stdout).contains("Hermetic Make Success"));
        }

        // Verify Rust compilation and linking under clean target machine conditions
        if let Ok(rustc_bin) = which::which("rustc") {
            let real_sdk = std::process::Command::new("xcrun")
                .arg("--show-sdk-path")
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                });

            let candidate_clangs = [
                "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
                "/Library/Developer/CommandLineTools/usr/bin/clang",
            ];

            if let (Some(sdk_str), Some(real_clang)) = (
                real_sdk,
                candidate_clangs.iter().find(|p| Path::new(p).exists()),
            ) {
                let sdk_path = PathBuf::from(sdk_str);
                let cc_bin = target_ws.join(".dmz/toolchains/cc/bin");
                let cc_lib = target_ws.join(".dmz/toolchains/cc/lib");
                let apple_sdk = target_ws.join(".dmz/toolchains/applesdk/SDKs/MacOSX.sdk");
                fs::create_dir_all(&cc_bin).unwrap();
                fs::create_dir_all(&cc_lib).unwrap();
                fs::create_dir_all(&apple_sdk).unwrap();

                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    if sdk_path.join("usr").exists() {
                        let _ = symlink(sdk_path.join("usr"), apple_sdk.join("usr"));
                    }
                    if sdk_path.join("System").exists() {
                        let _ = symlink(sdk_path.join("System"), apple_sdk.join("System"));
                    }
                }

                let real_clang_path = PathBuf::from(real_clang);
                let _ = fs::copy(&real_clang_path, cc_bin.join("clang"));
                if let Some(bin_dir) = real_clang_path.parent() {
                    if bin_dir.join("ld").exists() {
                        let _ = fs::copy(bin_dir.join("ld"), cc_bin.join("ld"));
                    }
                    if let Some(tc_root) = bin_dir.parent() {
                        for ldir in &[tc_root.join("usr/lib"), tc_root.join("lib")] {
                            if ldir.is_dir() {
                                for dylib in &[
                                    "libtapi.dylib",
                                    "libcodedirectory.dylib",
                                    "libLTO.dylib",
                                    "libswiftDemangle.dylib",
                                ] {
                                    let p = ldir.join(dylib);
                                    if p.exists() {
                                        let _ = fs::copy(&p, cc_lib.join(dylib));
                                    }
                                }
                            }
                        }
                    }
                }

                fs::write(
                    target_ws.join("main.rs"),
                    "fn main() {\n    println!(\"Hermetic Rust Success\");\n}\n",
                )
                .unwrap();

                ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

                let mut envs = HashMap::new();
                ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);
                envs.insert(
                    "DEVELOPER_DIR".to_string(),
                    "/nonexistent_developer_path_for_clean_target_machine".to_string(),
                );

                let rust_build = std::process::Command::new(&rustc_bin)
                    .arg(target_ws.join("main.rs"))
                    .arg("-o")
                    .arg(target_ws.join("rust_app"))
                    .current_dir(target_ws)
                    .envs(&envs)
                    .output()
                    .expect("Failed to invoke rustc");

                assert!(
                    rust_build.status.success(),
                    "Rust build failed under clean machine conditions: stdout={}\nstderr={}",
                    String::from_utf8_lossy(&rust_build.stdout),
                    String::from_utf8_lossy(&rust_build.stderr)
                );

                let rust_run = std::process::Command::new(target_ws.join("rust_app"))
                    .output()
                    .expect("Failed to execute compiled rust binary");
                assert!(rust_run.status.success());
                assert!(String::from_utf8_lossy(&rust_run.stdout).contains("Hermetic Rust Success"));
            }
        }
    }
}
