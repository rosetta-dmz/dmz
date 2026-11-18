use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_toolchain_packaging_and_import_roundtrip() -> anyhow::Result<()> {
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("mock_app");
    fs::create_dir_all(&workspace_path)?;

    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"toolchain-test\"\nversion = \"0.1.0\"",
    )?;

    let lockfile_path = workspace_path.join("dmz.lock");
    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
    fs::write(&lockfile_path, mock_lockfile_json)?;

    let dist_dir = tempdir()?;
    let archive_path = dist_dir.path().join("bundle_with_tc.tar.zst");

    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: workspace_path.join(".dmz/store"),
        output_path: archive_path.clone(),
        zstd_level: 3,
        mode: ExportMode::Unified,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: true,
        multi_workspace: None,
        ..Default::default()
    };

    let manifest = AirGapExporter::export(&export_config)?;
    assert!(archive_path.exists(), "Export archive must exist");
    assert!(
        !manifest.archive_sha256.is_empty(),
        "Archive SHA256 must be computed"
    );

    for tc in &manifest.toolchains {
        assert_eq!(
            tc.ecosystem,
            dmz_core::toolchain::ToolchainEcosystem::Rust,
            "Only Rust toolchain should be packaged for a pure Rust workspace, found {:?}",
            tc.ecosystem
        );
    }

    // Test import on target
    let target_dir = tempdir()?;
    let import_target = target_dir.path().join("target_workspace");
    let store_path = target_dir.path().join("offline_store");

    let import_config = ImportConfig {
        archive_path,
        target_store_dir: store_path,
        target_workspace_dir: import_target.clone(),
        verify_closure_hash: true,
        ..Default::default()
    };

    let result = AirGapImporter::import(&import_config)?;

    if !manifest.toolchains.is_empty() {
        assert!(
            result.has_toolchains,
            "ImportResult must report has_toolchains"
        );
        let env_sh = import_target.join(".dmz/env.sh");
        assert!(env_sh.exists(), ".dmz/env.sh activator must be generated");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&env_sh)?.permissions().mode();
            assert_eq!(mode & 0o111, 0o111, ".dmz/env.sh must be executable");
        }

        // Test path injection
        let mut envs = std::collections::HashMap::new();
        dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&import_target, &mut envs);
        assert!(envs.contains_key("PATH"), "PATH should be injected");
        assert!(
            envs.get("PATH")
                .unwrap()
                .replace('\\', "/")
                .contains(".dmz/toolchains"),
            "PATH should contain .dmz/toolchains"
        );
    }

    Ok(())
}

#[test]
fn test_native_build_tool_cmake_detection_and_packaging() -> anyhow::Result<()> {
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("mock_app_with_cmake");
    fs::create_dir_all(&workspace_path)?;

    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"native-test\"\nversion = \"0.1.0\"\n[dependencies]\ncmake = \"0.1\"",
    )?;

    // Manifest-driven native build: workspace declares CMakeLists.txt
    fs::write(
        workspace_path.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(native_test)\n",
    )?;

    // Verify requirement detection detects Rust, CMake, and companion toolchains (Cc and Ninja)
    let detected =
        dmz_core::toolchain::ToolchainProvider::detect_workspace_ecosystems(&workspace_path, None);
    assert!(
        detected.contains(&dmz_core::toolchain::ToolchainEcosystem::Rust),
        "Must detect Rust"
    );
    assert!(
        detected.contains(&dmz_core::toolchain::ToolchainEcosystem::CMake),
        "Must detect CMake when CMakeLists.txt is present"
    );
    assert!(
        detected.contains(&dmz_core::toolchain::ToolchainEcosystem::Cc),
        "Must infer companion Cc compiler when CMake is required"
    );
    assert!(
        !detected.contains(&dmz_core::toolchain::ToolchainEcosystem::Ninja),
        "Must not falsely infer companion build backend (Ninja) when CMake is required without build.ninja"
    );

    // Test environment injection and activator/toolchain generation
    let target_ws = src_dir.path().join("imported_target");
    let cmake_bin_dir = target_ws.join(".dmz/toolchains/cmake/bin");
    fs::create_dir_all(&cmake_bin_dir)?;
    fs::write(cmake_bin_dir.join("cmake"), "#!/bin/sh\necho cmake")?;

    let cc_bin_dir = target_ws.join(".dmz/toolchains/cc/bin");
    fs::create_dir_all(&cc_bin_dir)?;
    fs::write(cc_bin_dir.join("clang"), "#!/bin/sh\necho clang")?;
    fs::write(cc_bin_dir.join("clang++"), "#!/bin/sh\necho clang++")?;
    fs::write(cc_bin_dir.join("llvm-ar"), "#!/bin/sh\necho llvm-ar")?;
    fs::write(
        cc_bin_dir.join("llvm-ranlib"),
        "#!/bin/sh\necho llvm-ranlib",
    )?;

    let ninja_bin_dir = target_ws.join(".dmz/toolchains/ninja/bin");
    fs::create_dir_all(&ninja_bin_dir)?;
    fs::write(ninja_bin_dir.join("ninja"), "#!/bin/sh\necho ninja")?;

    let pkg_bin_dir = target_ws.join(".dmz/toolchains/pkgconfig/bin");
    let pkg_pc_dir = target_ws.join(".dmz/toolchains/pkgconfig/lib/pkgconfig");
    fs::create_dir_all(&pkg_bin_dir)?;
    fs::create_dir_all(&pkg_pc_dir)?;
    fs::write(pkg_bin_dir.join("pkg-config"), "#!/bin/sh\necho pkg-config")?;

    // Setup imported toolchains (generates dmz.toolchain.cmake and env.sh)
    dmz_core::toolchain::ToolchainPackager::setup_imported_toolchains(&target_ws)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let dmz_toolchain_cmake = target_ws.join(".dmz/toolchains/dmz.toolchain.cmake");
    assert!(
        dmz_toolchain_cmake.exists(),
        "dmz.toolchain.cmake must be autonomously generated"
    );
    let cmake_file_content = fs::read_to_string(&dmz_toolchain_cmake)?;
    assert!(
        cmake_file_content.contains("CMAKE_C_COMPILER"),
        "Must configure CMAKE_C_COMPILER in toolchain file"
    );
    assert!(
        cmake_file_content.contains("CMAKE_CXX_COMPILER"),
        "Must configure CMAKE_CXX_COMPILER in toolchain file"
    );
    assert!(
        cmake_file_content.contains("CMAKE_AR"),
        "Must configure CMAKE_AR in toolchain file"
    );

    let mut envs = std::collections::HashMap::new();
    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&target_ws, &mut envs);

    assert!(envs.contains_key("CMAKE"), "CMAKE env var must be injected");
    assert!(
        envs.get("CMAKE").unwrap().ends_with("bin/cmake"),
        "CMAKE env var must point to cmake binary"
    );
    assert!(
        envs.contains_key("CMAKE_TOOLCHAIN_FILE"),
        "CMAKE_TOOLCHAIN_FILE must be injected"
    );
    assert!(
        envs.contains_key("HOST_CMAKE_TOOLCHAIN_FILE"),
        "HOST_CMAKE_TOOLCHAIN_FILE must be injected for cmake-rs"
    );
    assert!(
        envs.contains_key("TARGET_CMAKE_TOOLCHAIN_FILE"),
        "TARGET_CMAKE_TOOLCHAIN_FILE must be injected for cmake-rs"
    );
    assert!(
        envs.contains_key("CC"),
        "CC must be injected pointing to bundled compiler"
    );
    assert!(
        envs.contains_key("CXX"),
        "CXX must be injected pointing to bundled compiler"
    );
    assert!(
        envs.contains_key("AR"),
        "AR must be injected pointing to bundled archiver"
    );
    assert!(
        envs.contains_key("RANLIB"),
        "RANLIB must be injected pointing to bundled ranlib"
    );
    assert!(envs.contains_key("NINJA"), "NINJA must be injected");
    assert_eq!(
        envs.get("CMAKE_GENERATOR").unwrap(),
        "Ninja",
        "CMAKE_GENERATOR must be set to Ninja"
    );
    assert!(
        envs.contains_key("PKG_CONFIG_PATH"),
        "PKG_CONFIG_PATH must be injected"
    );
    assert!(envs.contains_key("PATH"), "PATH must be injected");
    let path_str = envs.get("PATH").unwrap().replace('\\', "/");
    assert!(
        path_str.contains(".dmz/toolchains/cmake/bin"),
        "PATH must contain cmake bin"
    );
    assert!(
        path_str.contains(".dmz/toolchains/cc/bin"),
        "PATH must contain cc bin"
    );

    Ok(())
}

#[test]
fn test_android_ndk_and_apple_host_prereq_e2e_setup_and_environment() -> anyhow::Result<()> {
    let src_dir = tempdir()?;
    let target_ws = src_dir.path().join("mobile_imported_target");
    fs::create_dir_all(&target_ws)?;

    // Mock Android NDK structure
    let ndk_cmake_dir = target_ws.join(".dmz/toolchains/androidndk/build/cmake");
    fs::create_dir_all(&ndk_cmake_dir)?;
    fs::write(
        ndk_cmake_dir.join("android.toolchain.cmake"),
        "# Official Android NDK CMake Toolchain\n",
    )?;

    // AppleSdk must never bundle proprietary SDKs or @rpath binaries under host-prereq architecture
    assert!(
        dmz_core::toolchain::ToolchainProvider::discover_apple_sdk()?.is_none(),
        "AppleSdk discoverer must not bundle files"
    );

    // Setup imported toolchains (generates dmz.toolchain.cmake and env.sh)
    dmz_core::toolchain::ToolchainPackager::setup_imported_toolchains(&target_ws)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let dmz_toolchain_cmake = target_ws.join(".dmz/toolchains/dmz.toolchain.cmake");
    assert!(
        dmz_toolchain_cmake.exists(),
        "dmz.toolchain.cmake must be generated"
    );
    let cmake_file_content = fs::read_to_string(&dmz_toolchain_cmake)?;
    assert!(
        cmake_file_content.contains("android.toolchain.cmake"),
        "dmz.toolchain.cmake must chain official Android NDK toolchain"
    );
    assert!(
        cmake_file_content.contains("CMAKE_OSX_SYSROOT"),
        "dmz.toolchain.cmake must configure host Apple SDK sysroot when present"
    );

    let env_sh = target_ws.join(".dmz/env.sh");
    assert!(env_sh.exists(), "env.sh activator must exist");
    let env_sh_content = fs::read_to_string(&env_sh)?;
    assert!(
        env_sh_content.contains("ANDROID_NDK_HOME"),
        "env.sh must configure ANDROID_NDK_HOME"
    );
    assert!(
        !env_sh_content.contains(".dmz/toolchains/applesdk"),
        "env.sh must not reference non-bundled .dmz/toolchains/applesdk"
    );

    // Test environment map injection
    let mut envs = std::collections::HashMap::new();
    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&target_ws, &mut envs);

    assert!(
        envs.contains_key("ANDROID_NDK_HOME"),
        "ANDROID_NDK_HOME must be injected"
    );
    assert!(
        envs.contains_key("ANDROID_NDK_ROOT"),
        "ANDROID_NDK_ROOT must be injected"
    );
    assert!(
        envs.get("ANDROID_NDK_HOME")
            .unwrap()
            .replace('\\', "/")
            .ends_with(".dmz/toolchains/androidndk"),
        "ANDROID_NDK_HOME must point to bundled androidndk"
    );

    assert!(
        !envs.contains_key("DEVELOPER_DIR"),
        "DEVELOPER_DIR must not be hijacked into .dmz/toolchains/applesdk"
    );

    #[cfg(unix)]
    {
        let status_sh = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(". \"{}/.dmz/env.sh\"", target_ws.to_string_lossy()))
            .status()?;
        assert!(
            status_sh.success(),
            "/bin/sh sourcing of env.sh must succeed"
        );

        if std::path::Path::new("/bin/zsh").exists() {
            let status_zsh = std::process::Command::new("/bin/zsh")
                .arg("-c")
                .arg(format!(". \"{}/.dmz/env.sh\"", target_ws.to_string_lossy()))
                .status()?;
            assert!(
                status_zsh.success(),
                "/bin/zsh sourcing of env.sh must succeed without glob errors"
            );
        }
    }

    Ok(())
}

#[test]
fn test_export_rejects_mismatched_python_wheel_and_cleans_partial_archive() -> anyhow::Result<()> {
    use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
    use dmz_core::resolver::manifest::Ecosystem;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let tmp = tempdir()?;
    let ws = tmp.path().join("py_app");
    let store = ws.join(".dmz/store");
    let v2 = store.join("pyyaml/v2");
    fs::create_dir_all(&v2)?;

    fs::write(ws.join(".python-version"), "3.14\n")?;
    fs::write(ws.join("requirements.txt"), "PyYAML==6.0.2\n")?;

    // Write a mock PyYAML wheel containing a cp312 native extension against a pinned python3.14 workspace.
    let wheel_name = "pyyaml-6.0.2-cp312-cp312-macosx_11_0_arm64.archive";
    let wheel_path = v2.join(wheel_name);
    {
        let f = fs::File::create(&wheel_path)?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("yaml/_yaml.cpython-312-darwin.so", opts)?;
        zw.write_all(b"mock-cp312-so")?;
        zw.finish()?;
    }
    let wheel_sha = dmz_core::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(
        fs::read(&wheel_path)?,
    ));

    let mut lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "PyYAML".to_string(),
        version: "6.0.2".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: wheel_sha,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    lockfile.target_platform = Some("macos-arm64".to_string());
    let lock_path = ws.join("dmz.lock");
    lockfile.save(&lock_path)?;

    let out_archive = tmp.path().join("dist/should_fail.tar.zst");
    let err = AirGapExporter::export(&ExportConfig {
        workspace_path: ws.clone(),
        lockfile_path: lock_path.clone(),
        store_dir: store.clone(),
        output_path: out_archive.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        target_platform: Some("macos-arm64".to_string()),
        with_toolchain: false,
        ..Default::default()
    })
    .expect_err("export must fail fast when packed PyPI wheel ABI mismatches python3.14");
    let err_msg = err.to_string();
    assert!(err_msg.contains("cp312"), "unexpected error: {err_msg}");
    assert!(
        err_msg.contains("python3.14"),
        "unexpected error: {err_msg}"
    );
    assert!(
        !out_archive.exists(),
        "partial export archive must be removed on validation failure"
    );

    // Replace with matching cp314 extension wheel and confirm export succeeds cleanly.
    fs::remove_file(&wheel_path)?;
    let wheel_314_path = v2.join("pyyaml-6.0.3-cp314-cp314-macosx_11_0_arm64.archive");
    {
        let f = fs::File::create(&wheel_314_path)?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("yaml/_yaml.cpython-314-darwin.so", opts)?;
        zw.write_all(b"mock-cp314-so")?;
        zw.finish()?;
    }
    let sha_314 = dmz_core::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(
        fs::read(&wheel_314_path)?,
    ));
    let mut valid_lock = DmzLockfile::new(vec![LockedPackage {
        name: "PyYAML".to_string(),
        version: "6.0.3".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: sha_314,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    valid_lock.target_platform = Some("macos-arm64".to_string());
    valid_lock.save(&lock_path)?;

    let ok_archive = tmp.path().join("dist/valid_314.tar.zst");
    let manifest = AirGapExporter::export(&ExportConfig {
        workspace_path: ws,
        lockfile_path: lock_path,
        store_dir: store,
        output_path: ok_archive.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        target_platform: Some("macos-arm64".to_string()),
        with_toolchain: false,
        ..Default::default()
    })?;
    assert_eq!(manifest.package_count, 1);
    assert!(ok_archive.is_file());
    Ok(())
}

#[test]
fn test_import_atomically_replaces_stale_python_toolchain_dir() -> anyhow::Result<()> {
    use dmz_core::archive::exporter::AirGapManifest;
    use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
    use dmz_core::resolver::manifest::Ecosystem;
    use dmz_core::toolchain::{ToolchainEcosystem, ToolchainManifestEntry};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use zstd::stream::write::Encoder as ZstdEncoder;

    let tmp = tempdir()?;
    let target_ws = tmp.path().join("target_ws");
    // Pre-populate stale python3.14 toolchain dir from an earlier import
    fs::create_dir_all(target_ws.join(".dmz/toolchains/python/lib/python3.14"))?;
    fs::write(
        target_ws.join(".dmz/toolchains/python/lib/python3.14/os.py"),
        "# stale 3.14",
    )?;

    // Build an archive shipping python3.12 + cp312 PyYAML wheel cleanly
    let stage = tmp.path().join("stage");
    fs::create_dir_all(&stage)?;
    let wheel_path = stage.join("pyyaml-6.0.2-cp312-cp312-macosx_11_0_arm64.archive");
    let entry_ext = if cfg!(target_os = "macos") {
        "yaml/_yaml.cpython-312-darwin.so"
    } else if cfg!(target_os = "linux") {
        "yaml/_yaml.cpython-312-x86_64-linux-gnu.so"
    } else {
        "yaml/_yaml.cp312-win_amd64.pyd"
    };
    {
        let f = fs::File::create(&wheel_path)?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("yaml/__init__.py", opts)?;
        zw.write_all(b"__version__ = '6.0.2'\n")?;
        zw.start_file(entry_ext, opts)?;
        zw.write_all(b"mock-cp312")?;
        zw.finish()?;
    }
    let wheel_bytes = fs::read(&wheel_path)?;
    let wheel_sha =
        dmz_core::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(&wheel_bytes));

    let lock = DmzLockfile::new(vec![LockedPackage {
        name: "PyYAML".to_string(),
        version: "6.0.2".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: wheel_sha,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    let lock_json = serde_json::to_string_pretty(&lock)?;

    let archive_path = tmp.path().join("bundle_312.tar.zst");
    {
        let file = fs::File::create(&archive_path)?;
        let zstd = ZstdEncoder::new(file, 1)?;
        let mut tar = tar::Builder::new(zstd);

        let manifest = AirGapManifest {
            dmz_version: env!("CARGO_PKG_VERSION").to_string(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: vec![ToolchainManifestEntry {
                ecosystem: ToolchainEcosystem::Python,
                version: "Python 3.12.7".to_string(),
                path_in_archive: "toolchains/python".to_string(),
                binaries: vec!["python3".to_string()],
            }],
            workspace_project: None,
            closure_signature: lock.closure_signature.clone(),
            package_count: 1,
            created_at_utc: 315_532_800,
            archive_sha256: String::new(),
        };
        let manifest_json = serde_json::to_string_pretty(&manifest)?;

        let mut append_bytes = |name: &str, data: &[u8]| -> anyhow::Result<()> {
            let mut hdr = tar::Header::new_gnu();
            hdr.set_size(data.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar.append_data(&mut hdr, name, data)?;
            Ok(())
        };

        append_bytes("airgap_manifest.json", manifest_json.as_bytes())?;
        append_bytes("dmz.lock", lock_json.as_bytes())?;
        append_bytes("requirements.txt", b"PyYAML==6.0.2\n")?;
        append_bytes(
            "store/pyyaml/v2/pyyaml-6.0.2-cp312-cp312-macosx_11_0_arm64.archive",
            &wheel_bytes,
        )?;
        append_bytes("toolchains/python/bin/python3", b"#!/bin/sh\n")?;
        append_bytes("toolchains/python/lib/python3.12/os.py", b"# 3.12 stdlib\n")?;

        let zstd_fin = tar.into_inner()?;
        zstd_fin.finish()?;
    }

    let import_res = AirGapImporter::import(&ImportConfig {
        archive_path,
        target_workspace_dir: target_ws.clone(),
        target_store_dir: target_ws.join(".dmz/store"),
        verify_closure_hash: true,
        install_dependencies: true,
        ..Default::default()
    })?;
    assert!(import_res.has_toolchains);
    assert_eq!(
        import_res.installed_packages, 1,
        "expected PyYAML to install offline: {import_res:?}"
    );
    assert!(
        !target_ws
            .join(".dmz/toolchains/python/lib/python3.14")
            .exists(),
        "stale python3.14 dir must be removed before extracting python3.12 toolchain"
    );
    assert!(target_ws
        .join(".dmz/toolchains/python/lib/python3.12/os.py")
        .is_file());
    assert_eq!(
        dmz_core::install::mappers::PyPiMapper::detect_python_version(&target_ws),
        "python3.12"
    );
    assert!(target_ws
        .join(".dmz/venv/lib/python3.12/site-packages/yaml/__init__.py")
        .is_file());
    Ok(())
}
