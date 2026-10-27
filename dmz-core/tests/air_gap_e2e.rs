use std::fs;
use tempfile::tempdir;

use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::archive::importer::{AirGapImporter, ImportConfig};

#[test]
fn test_multiecosystem_airgap_roundtrip() -> anyhow::Result<()> {
    // Setup temporary source workspace mimicking a multi-ecosystem project
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("my_project");
    fs::create_dir_all(&workspace_path)?;

    // Drop mock manifests for Rust and Node ecosystems
    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"test-rust\"\nversion = \"1.0.8\"",
    )?;
    fs::write(
        workspace_path.join("package.json"),
        "{\"name\": \"test-node\", \"version\": \"1.0.0\"}",
    )?;

    // Create a valid JSON mock lockfile with an empty package map required by the exporter
    let lockfile_path = workspace_path.join("dmz.lock");
    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
    fs::write(&lockfile_path, mock_lockfile_json)?;

    // Create a mock cache directory to simulate pre-fetched dependencies
    let cargo_cache = workspace_path.join(".cargo");
    fs::create_dir_all(&cargo_cache)?;
    fs::write(cargo_cache.join("config.toml"), "# mock config")?;

    // Setup export destination
    let dist_dir = tempdir()?;
    let archive_path = dist_dir.path().join("workspace_closure.tar.zst");

    let export_config = ExportConfig {
        workspace_path,
        lockfile_path,
        output_path: archive_path.clone(),
        zstd_level: 3,
        ..Default::default()
    };

    // Execute Export
    println!("Running air-gap export test...");
    let _manifest = AirGapExporter::export(&export_config)?;
    assert!(archive_path.exists(), "Export archive must be created");

    // Setup import destination (simulating disconnected air-gap target machine)
    let target_dir = tempdir()?;
    let import_target = target_dir.path().join("imported_workspace");
    let store_path = target_dir.path().join("offline_store");

    let import_config = ImportConfig {
        archive_path,
        target_store_dir: store_path,
        target_workspace_dir: import_target.clone(),
        verify_closure_hash: true,
    };

    // Execute Import
    println!("Running air-gap import test...");
    AirGapImporter::import(&import_config)?;

    // Assertions: verify files landed correctly and multi-ecosystem detection works post-import
    assert!(
        import_target.join("Cargo.toml").exists(),
        "Cargo.toml should be unpacked"
    );
    assert!(
        import_target.join("package.json").exists(),
        "package.json should be unpacked"
    );
    assert!(
        import_target.join(".cargo").exists(),
        "Ecosystem cache should be preserved"
    );

    Ok(())
}

#[test]
fn test_relative_paths_anchor_to_selected_workspace() -> anyhow::Result<()> {
    let src_dir = tempdir()?;
    let workspace_path = src_dir.path().join("external_project");
    fs::create_dir_all(&workspace_path)?;

    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"external-app\"\nversion = \"0.1.0\"",
    )?;

    let mock_lockfile_json = r#"{
        "version": 1,
        "closure_signature": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "packages": {}
    }"#;
    fs::write(workspace_path.join("dmz.lock"), mock_lockfile_json)?;

    // Configure export with relative paths as the GUI/CLI default provides
    let export_config = ExportConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path: std::path::PathBuf::from("dmz.lock"),
        store_dir: std::path::PathBuf::from(".dmz/store"),
        output_path: std::path::PathBuf::from("dist/workspace_closure.tar.zst"),
        zstd_level: 3,
        ..Default::default()
    };

    let manifest = AirGapExporter::export(&export_config)?;
    assert!(!manifest.archive_sha256.is_empty());

    // Verify it exported into the selected workspace
    let expected_archive = workspace_path.join("dist/workspace_closure.tar.zst");
    assert!(
        expected_archive.exists(),
        "Export archive must be created inside the selected workspace"
    );

    Ok(())
}

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
    };

    let manifest = AirGapExporter::export(&export_config)?;
    assert!(archive_path.exists(), "Export archive must exist");
    assert!(
        !manifest.archive_sha256.is_empty(),
        "Archive SHA256 must be computed"
    );

    // Pure Rust workspace must only package Rust toolchain (no Go, Python, Node, etc.)
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
            envs.get("PATH").unwrap().contains(".dmz/toolchains"),
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
        detected.contains(&dmz_core::toolchain::ToolchainEcosystem::Ninja),
        "Must infer companion build backend (Ninja) when CMake is required"
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
    assert!(
        envs.get("PATH")
            .unwrap()
            .contains(".dmz/toolchains/cmake/bin"),
        "PATH must contain cmake bin"
    );
    assert!(
        envs.get("PATH").unwrap().contains(".dmz/toolchains/cc/bin"),
        "PATH must contain cc bin"
    );

    Ok(())
}

#[test]
fn test_android_ndk_and_apple_sdk_e2e_setup_and_environment() -> anyhow::Result<()> {
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

    // Mock Apple SDK and CLT structure
    let apple_sdk_dir = target_ws
        .join(".dmz/toolchains/applesdk/Platforms/MacOSX.platform/Developer/SDKs/MacOSX14.0.sdk");
    let apple_bin_dir = target_ws.join(".dmz/toolchains/applesdk/bin");
    fs::create_dir_all(&apple_sdk_dir)?;
    fs::create_dir_all(&apple_bin_dir)?;
    fs::write(apple_bin_dir.join("xcrun"), "#!/bin/sh\necho xcrun")?;
    fs::write(apple_bin_dir.join("swift"), "#!/bin/sh\necho swift")?;

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
        "dmz.toolchain.cmake must configure Apple SDK sysroot"
    );

    let env_sh = target_ws.join(".dmz/env.sh");
    assert!(env_sh.exists(), "env.sh activator must exist");
    let env_sh_content = fs::read_to_string(&env_sh)?;
    assert!(
        env_sh_content.contains("ANDROID_NDK_HOME"),
        "env.sh must configure ANDROID_NDK_HOME"
    );
    assert!(
        env_sh_content.contains("DEVELOPER_DIR"),
        "env.sh must configure DEVELOPER_DIR"
    );
    assert!(
        env_sh_content.contains("SDKROOT"),
        "env.sh must configure SDKROOT"
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
            .ends_with(".dmz/toolchains/androidndk"),
        "ANDROID_NDK_HOME must point to bundled androidndk"
    );

    assert!(
        envs.contains_key("DEVELOPER_DIR"),
        "DEVELOPER_DIR must be injected"
    );
    assert!(
        envs.get("DEVELOPER_DIR")
            .unwrap()
            .ends_with(".dmz/toolchains/applesdk"),
        "DEVELOPER_DIR must point to bundled applesdk"
    );

    assert!(envs.contains_key("SDKROOT"), "SDKROOT must be injected");
    assert!(
        envs.get("SDKROOT").unwrap().contains("MacOSX14.0.sdk"),
        "SDKROOT must resolve to the bundled Apple SDK directory"
    );

    assert!(envs.contains_key("PATH"), "PATH must be injected");
    assert!(
        envs.get("PATH")
            .unwrap()
            .contains(".dmz/toolchains/applesdk/bin"),
        "PATH must contain applesdk bin"
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
