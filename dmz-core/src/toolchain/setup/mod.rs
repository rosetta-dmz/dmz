// Toolchain environment setup and configuration

pub mod activators;
pub mod cmake;
pub mod ide;
pub mod permissions;
pub mod relocate;
pub mod wrappers;

pub use activators::generate_activator_scripts;
pub use cmake::generate_cmake_toolchain;
pub use ide::{generate_ide_settings, generate_multi_root_workspace};
pub use permissions::apply_permissions_and_quarantine;
pub use relocate::relocate_bundled_binaries;
pub use wrappers::{generate_clang_wrappers, setup_node_package_managers};

use std::fs;
use std::path::Path;

/// Target machine activation and setup engine for imported toolchains.
pub struct ToolchainSetup;

impl ToolchainSetup {
    /// Set up imported toolchain directory: execution permissions, quarantine clearance, and activator scripts.
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<(), String> {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        let has_toolchains = toolchains_dir.exists()
            && fs::read_dir(&toolchains_dir)
                .map(|mut i| i.next().is_some())
                .unwrap_or(false);
        if !has_toolchains {
            Self::generate_ide_settings(workspace_root)?;
            return Ok(());
        }

        apply_permissions_and_quarantine(&toolchains_dir)?;
        Self::relocate_bundled_binaries(workspace_root)?;
        Self::generate_clang_wrappers(workspace_root)?;
        Self::setup_node_package_managers(workspace_root)?;
        Self::generate_cmake_toolchain(workspace_root)?;
        Self::generate_activator_scripts(workspace_root)?;
        Self::generate_ide_settings(workspace_root)?;

        Ok(())
    }

    /// Relocate bundled binaries and shared libraries using relative runtime search paths (RPATH).
    pub fn relocate_bundled_binaries(workspace_root: &Path) -> Result<(), String> {
        relocate::relocate_bundled_binaries(workspace_root)
    }

    /// Generate hermetic wrapper scripts for Clang/LLVM compiler binaries.
    pub fn generate_clang_wrappers(workspace_root: &Path) -> Result<(), String> {
        wrappers::generate_clang_wrappers(workspace_root)
    }

    /// Ensures Node.js companion package managers are wired to entry points.
    pub fn setup_node_package_managers(workspace_root: &Path) -> Result<(), String> {
        wrappers::setup_node_package_managers(workspace_root)
    }

    /// Generates .dmz/toolchains/dmz.toolchain.cmake for CMake projects.
    pub fn generate_cmake_toolchain(workspace_root: &Path) -> Result<(), String> {
        cmake::generate_cmake_toolchain(workspace_root)
    }

    /// Generates shell and batch activator scripts in .dmz/ for immediate developer use.
    pub fn generate_activator_scripts(workspace_root: &Path) -> Result<(), String> {
        activators::generate_activator_scripts(workspace_root)
    }

    /// Configures .vscode/settings.json for air-gapped IDE workflows.
    pub fn generate_ide_settings(workspace_root: &Path) -> Result<(), String> {
        ide::generate_ide_settings(workspace_root)
    }

    /// Generates or updates <project>.code-workspace for multi-root projects.
    pub fn generate_multi_root_workspace(workspace_root: &Path) -> Result<(), String> {
        ide::generate_multi_root_workspace(workspace_root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_non_cmake_workspace_silences_cmake_in_ide_settings() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Workspace has no CMakeLists.txt and no bundled cmake
        fs::create_dir_all(ws.join(".dmz/toolchains/rust/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/rust/bin/cargo"), "cargo").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let settings_path = ws.join(".vscode/settings.json");
        assert!(settings_path.exists());
        let content = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(val["cmake.configureOnOpen"], false);
        assert_eq!(val["cmake.autoSelectActiveFolder"], false);
        assert!(val.get("cmake.cmakePath").is_none());

        // Check terminal environment has rust toolchain
        let osx_path = val["terminal.integrated.env.osx"]["PATH"].as_str().unwrap();
        assert!(osx_path.contains(".dmz/toolchains/rust/bin"));

        // dmz.toolchain.cmake must NOT exist
        assert!(!ws.join(".dmz/toolchains/dmz.toolchain.cmake").exists());
    }

    #[test]
    fn test_cmake_workspace_configures_cmake_in_ide_settings() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::write(ws.join("CMakeLists.txt"), "project(test)").unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/cmake/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cmake/bin/cmake"), "cmake").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let settings_path = ws.join(".vscode/settings.json");
        assert!(settings_path.exists());
        let content = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(val["cmake.configureOnOpen"], true);
        assert_eq!(
            val["cmake.cmakePath"],
            "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
        );
        assert!(ws.join(".dmz/toolchains/dmz.toolchain.cmake").exists());
    }

    #[test]
    fn test_bundled_cmake_without_root_cmakelists_configures_cmake_path_and_toolchain() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Rust workspace where CMake is bundled for a dependency (e.g. libz-ng-sys -> cmake) without a root CMakeLists.txt
        fs::create_dir_all(ws.join(".dmz/toolchains/cmake/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cmake/bin/cmake"), "cmake").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let settings_path = ws.join(".vscode/settings.json");
        assert!(settings_path.exists());
        let content = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            val["cmake.cmakePath"],
            "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
        );
        assert_eq!(
            val["cmake.configureSettings"]["CMAKE_TOOLCHAIN_FILE"],
            "${workspaceFolder}/.dmz/toolchains/dmz.toolchain.cmake"
        );
        assert!(ws.join(".dmz/toolchains/dmz.toolchain.cmake").exists());
    }

    #[test]
    fn test_rust_analyzer_cargo_extra_env_and_verified_toolchain_binaries() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::create_dir_all(ws.join(".dmz/toolchains/rust/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/rust/bin/cargo"), "cargo").unwrap();
        fs::write(
            ws.join(".dmz/toolchains/rust/bin/rust-analyzer"),
            "rust-analyzer",
        )
        .unwrap();

        // Empty python and go dirs without binaries must NOT write broken IDE paths
        fs::create_dir_all(ws.join(".dmz/toolchains/python")).unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/go")).unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let content = fs::read_to_string(ws.join(".vscode/settings.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            val["rust-analyzer.cargo.extraEnv"]["RUSTUP_HOME"],
            "${workspaceFolder}/.dmz/toolchains/rust"
        );
        assert_eq!(
            val["rust-analyzer.cargo.extraEnv"]["CARGO_HOME"],
            "${workspaceFolder}/.cargo"
        );
        assert!(val.get("python.defaultInterpreterPath").is_none());
        assert!(val.get("go.goroot").is_none());
    }

    #[test]
    fn test_generate_clang_wrappers_and_idempotency() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Simulate cc toolchain with clang, clang++ binaries
        let cc_bin = ws.join(".dmz/toolchains/cc/bin");
        fs::create_dir_all(&cc_bin).unwrap();
        fs::write(cc_bin.join("clang"), "#!/bin/sh\necho real-clang").unwrap();
        fs::write(cc_bin.join("clang++"), "#!/bin/sh\necho real-clang++").unwrap();

        // Simulate applesdk
        let sdk_dir = ws.join(".dmz/toolchains/applesdk/SDKs/MacOSX.sdk/usr/include");
        fs::create_dir_all(&sdk_dir).unwrap();
        fs::write(sdk_dir.join("stdio.h"), "/* stdio */").unwrap();
        fs::write(sdk_dir.join("zlib.h"), "/* zlib */").unwrap();

        // Setup toolchains
        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        #[cfg(unix)]
        {
            // Verify clang.real and clang++.real exist and have original contents
            let real_clang = cc_bin.join("clang.real");
            let real_clangxx = cc_bin.join("clang++.real");
            assert!(real_clang.exists());
            assert!(real_clangxx.exists());
            assert_eq!(
                fs::read_to_string(&real_clang).unwrap(),
                "#!/bin/sh\necho real-clang"
            );
            assert_eq!(
                fs::read_to_string(&real_clangxx).unwrap(),
                "#!/bin/sh\necho real-clang++"
            );

            // Verify clang and clang++ are hermetic wrappers
            let clang_wrapper = fs::read_to_string(cc_bin.join("clang")).unwrap();
            assert!(clang_wrapper.contains("DMZ Hermetic Clang Wrapper"));
            assert!(clang_wrapper.contains("-isysroot"));

            // Verify cc wrapper was generated as well
            let cc_path = cc_bin.join("cc");
            assert!(cc_path.exists());
            let cc_wrapper = fs::read_to_string(&cc_path).unwrap();
            assert!(cc_wrapper.contains("DMZ Hermetic Clang Wrapper"));

            // Verify permissions are 0o755
            use std::os::unix::fs::PermissionsExt;
            for p in &[&real_clang, &real_clangxx, &cc_bin.join("clang"), &cc_path] {
                let mode = fs::metadata(p).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "Must be executable: {:?}", p);
            }

            // Verify idempotency: running setup again must preserve real binaries
            ToolchainSetup::setup_imported_toolchains(ws).unwrap();
            assert_eq!(
                fs::read_to_string(&real_clang).unwrap(),
                "#!/bin/sh\necho real-clang"
            );
            assert_eq!(
                fs::read_to_string(&real_clangxx).unwrap(),
                "#!/bin/sh\necho real-clang++"
            );
        }
    }

    #[test]
    fn test_cmake_toolchain_with_apple_sdk_sysroot() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::write(ws.join("CMakeLists.txt"), "project(test)").unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/cmake/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cmake/bin/cmake"), "cmake").unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/cc/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cc/bin/clang"), "clang").unwrap();

        let sdk_dir = ws.join(".dmz/toolchains/applesdk/SDKs/MacOSX.sdk/usr/include");
        fs::create_dir_all(&sdk_dir).unwrap();
        fs::write(sdk_dir.join("zlib.h"), "/* zlib */").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let cmake_tc = ws.join(".dmz/toolchains/dmz.toolchain.cmake");
        assert!(cmake_tc.exists());
        let content = fs::read_to_string(&cmake_tc).unwrap();

        // Must configure Darwin and CMAKE_OSX_SYSROOT prior to compiler check
        assert!(content.contains("CMAKE_SYSTEM_NAME \"Darwin\""));
        assert!(content.contains("CMAKE_OSX_SYSROOT"));
        assert!(content.contains("CMAKE_FIND_ROOT_PATH_MODE_INCLUDE BOTH"));
        assert!(content.contains("CMAKE_FIND_ROOT_PATH_MODE_LIBRARY BOTH"));

        // env.sh must export SDKROOT and NOT export DEVELOPER_DIR (since no xcrun)
        let env_sh = fs::read_to_string(ws.join(".dmz/env.sh")).unwrap();
        assert!(env_sh.contains("export SDKROOT="));
        assert!(env_sh.contains("[ -f \"$DMZ_WS/.dmz/toolchains/applesdk/usr/bin/xcrun\" ]"));
    }

    #[test]
    fn test_setup_imported_toolchains_is_strictly_idempotent() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let cc_bin = ws.join(".dmz/toolchains/cc/bin");
        fs::create_dir_all(&cc_bin).unwrap();
        fs::write(
            cc_bin.join("clang"),
            "#!/bin/sh\necho \"real clang binary\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("clang", cc_bin.join("cc")).unwrap();
        }
        #[cfg(not(unix))]
        {
            fs::write(cc_bin.join("cc"), "#!/bin/sh\necho \"real cc binary\"\n").unwrap();
        }

        let rust_bin = ws.join(".dmz/toolchains/rust/bin");
        fs::create_dir_all(&rust_bin).unwrap();
        fs::write(rust_bin.join("cargo"), "cargo").unwrap();

        // 1. First execution
        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        #[cfg(unix)]
        {
            let real_clang = cc_bin.join("clang.real");
            assert!(
                real_clang.exists(),
                "clang.real must exist after first setup"
            );
            assert_eq!(
                fs::read_to_string(&real_clang).unwrap(),
                "#!/bin/sh\necho \"real clang binary\"\n"
            );

            let clang_wrapper = fs::read_to_string(cc_bin.join("clang")).unwrap();
            assert!(clang_wrapper.contains("# DMZ Hermetic Clang Wrapper"));

            let cc_wrapper = fs::read_to_string(cc_bin.join("cc")).unwrap();
            assert!(cc_wrapper.contains("# DMZ Hermetic Clang Wrapper"));
            assert!(
                !cc_bin.join("cc.real").exists(),
                "cc.real must not be created"
            );
        }

        let env_sh_1 = fs::read_to_string(ws.join(".dmz/env.sh")).unwrap();

        // 2. Second execution (must be strictly idempotent)
        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        #[cfg(unix)]
        {
            let real_clang = cc_bin.join("clang.real");
            assert_eq!(
                fs::read_to_string(&real_clang).unwrap(),
                "#!/bin/sh\necho \"real clang binary\"\n",
                "clang.real must remain intact and never be overwritten by a wrapper script"
            );
            assert!(
                !cc_bin.join("cc.real").exists(),
                "cc.real must not be created on repeated setup runs"
            );
        }

        let env_sh_2 = fs::read_to_string(ws.join(".dmz/env.sh")).unwrap();
        assert_eq!(
            env_sh_1, env_sh_2,
            "env.sh content must be completely identical across repeated setups"
        );

        // 3. Third execution
        ToolchainSetup::setup_imported_toolchains(ws).unwrap();
        let env_sh_3 = fs::read_to_string(ws.join(".dmz/env.sh")).unwrap();
        assert_eq!(env_sh_1, env_sh_3);
    }
}
