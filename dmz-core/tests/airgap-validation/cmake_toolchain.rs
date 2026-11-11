use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

use dmz_core::toolchain::{ToolchainEcosystem, ToolchainPackager, ToolchainProvider};

use super::common::create_mock_bin;

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
