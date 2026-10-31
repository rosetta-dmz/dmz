//! Official Battle-Tested Integration Test Suite for Out-of-the-Box Air-Gap Readiness.
//!
//! Validates Root Manifest Dominance and ensures exported bundles are strictly complete
//! and ready out-of-the-box on clean target machines without toolchain hallucination,
//! spurious IDE errors ("Bad CMake executable: ''"), or broken native linker configurations
//! ("Linking with 'cc' failed, exit status 1" / "missing xcrun for applesdk").

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
fn test_pure_rust_proc_macro_workspace_never_hallucinates_foreign_toolchains() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // 1. Setup a real Rust project with proc-macros and common FFI crates that previously
    // tricked heuristic directory scanners into hallucinating CMake, CC, and AppleSdk.
    let cargo_toml = r#"
[package]
name = "airgap-service"
version = "0.1.0"
edition = "2021"

[dependencies]
quote = "1.0"
proc-macro2 = "1.0"
serde = "1.0"
serde_core = "1.0"
libc = "0.2"
cc = "1.0"
swift-rs = "1.0"
"#;
    fs::write(ws.join("Cargo.toml"), cargo_toml).unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    // Simulate Cargo.lock with crate entries
    let cargo_lock = r#"
version = 3

[[package]]
name = "quote"
version = "1.0.35"

[[package]]
name = "proc-macro2"
version = "1.0.78"

[[package]]
name = "serde"
version = "1.0.197"

[[package]]
name = "serde_core"
version = "1.0.0"

[[package]]
name = "libc"
version = "0.2.153"

[[package]]
name = "cc"
version = "1.0.83"

[[package]]
name = "swift-rs"
version = "1.0.0"
"#;
    fs::write(ws.join("Cargo.lock"), cargo_lock).unwrap();

    // Simulate vendored crates directory (.cargo/vendor) containing packages named cmake, cc, swift-rs
    for crate_name in &["cmake-0.1.50", "cc-1.0.83", "swift-rs-1.0.0"] {
        let vdir = ws.join(format!(".cargo/vendor/{}", crate_name));
        fs::create_dir_all(&vdir).unwrap();
        fs::write(
            vdir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{}\"\nversion = \"1.0.0\"\n",
                crate_name
            ),
        )
        .unwrap();
    }

    // 2. Execute ecosystem detection via Root Manifest Dominance
    let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);

    // CRITICAL ASSERTION: Pure Rust workspace MUST detect strictly Rust and NOTHING extra!
    assert_eq!(
        detected.len(),
        1,
        "Pure Rust workspace must only detect 1 ecosystem (Rust), but detected: {:?}",
        detected
    );
    assert!(
        detected.contains(&ToolchainEcosystem::Rust),
        "Rust ecosystem must be detected"
    );

    // Explicitly verify none of the hallucinated toolchains are detected:
    assert!(
        !detected.contains(&ToolchainEcosystem::CMake),
        "CMake must NOT be detected in a pure Rust workspace even with cmake crate"
    );
    assert!(
        !detected.contains(&ToolchainEcosystem::Cc),
        "Cc must NOT be detected in a pure Rust workspace even with cc crate"
    );
    assert!(
        !detected.contains(&ToolchainEcosystem::AppleSdk),
        "AppleSdk must NOT be detected in a pure Rust workspace even with swift-rs crate"
    );
    assert!(
        !detected.contains(&ToolchainEcosystem::Ninja),
        "Ninja must NOT be detected in a pure Rust workspace"
    );
    assert!(
        !detected.contains(&ToolchainEcosystem::Make),
        "Make must NOT be detected in a pure Rust workspace"
    );
    assert!(
        !detected.contains(&ToolchainEcosystem::Go),
        "Go must NOT be detected in a pure Rust workspace"
    );

    // 3. Test target setup and activation in an imported air-gapped target workspace
    let target_dir = tempdir().unwrap();
    let target_ws = target_dir.path();

    // Simulate unpacked bundled Rust toolchain
    let rust_bin = target_ws.join(".dmz/toolchains/rust/bin");
    create_mock_bin(&rust_bin, "cargo");
    create_mock_bin(&rust_bin, "rustc");
    create_mock_bin(&rust_bin, "rust-analyzer");

    // Execute target setup engine
    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    // 4. Verify IDE settings (.vscode/settings.json):
    // Must prevent "Bad CMake executable: ''" popup!
    let settings_path = target_ws.join(".vscode/settings.json");
    assert!(
        settings_path.exists(),
        ".vscode/settings.json must be created"
    );

    let settings_str = fs::read_to_string(&settings_path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings_str).unwrap();

    assert_eq!(
        settings["cmake.configureOnOpen"], false,
        "cmake.configureOnOpen must be false to eliminate IDE popup"
    );
    assert_eq!(
        settings["cmake.autoSelectActiveFolder"], false,
        "cmake.autoSelectActiveFolder must be false"
    );
    assert!(
        settings.get("cmake.cmakePath").is_none(),
        "cmake.cmakePath must not be populated with empty or bad path"
    );

    // Verify rust-analyzer configuration
    assert_eq!(
        settings["rust-analyzer.server.path"],
        "${workspaceFolder}/.dmz/toolchains/rust/bin/rust-analyzer"
    );
    assert_eq!(
        settings["rust-analyzer.runnables.extraEnv"]["RUSTUP_HOME"],
        "${workspaceFolder}/.dmz/toolchains/rust"
    );

    // 5. Verify NO dmz.toolchain.cmake was generated (which confuses IDEs and build scripts)
    assert!(
        !target_ws
            .join(".dmz/toolchains/dmz.toolchain.cmake")
            .exists(),
        "dmz.toolchain.cmake must NOT exist in non-CMake workspace"
    );

    // 6. Verify environment injection:
    // Prevents "Linking with cc failed, exit status 1" and "missing xcrun for applesdk"
    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut env_map);

    assert!(
        !env_map.contains_key("CC"),
        "CC must NOT be set (host Clang must not intercept rustc proc-macro linking)"
    );
    assert!(
        !env_map.contains_key("CXX"),
        "CXX must NOT be set in pure Rust workspace"
    );
    assert!(
        !env_map.contains_key("DEVELOPER_DIR"),
        "DEVELOPER_DIR must NOT be set (prevents missing xcrun for applesdk error)"
    );
    assert!(
        !env_map.contains_key("CMAKE_TOOLCHAIN_FILE"),
        "CMAKE_TOOLCHAIN_FILE must NOT be injected in non-CMake workspace"
    );

    // Verify Rust runtime paths are cleanly configured
    assert!(env_map.contains_key("RUSTUP_HOME"));
    assert!(env_map.contains_key("CARGO_HOME"));
    let path = env_map.get("PATH").unwrap();
    assert!(
        path.replace('\\', "/").contains(".dmz/toolchains/rust/bin"),
        "PATH must prepend bundled rust bin"
    );
}

#[test]
fn test_declared_cmake_workspace_correctly_configures_cmake_and_compilers() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // 1. Create a workspace that genuinely uses CMake and C++
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
        detected.contains(&ToolchainEcosystem::Ninja),
        "Ninja must be inferred companion build generator for CMake"
    );

    // 2. Simulate bundled CMake, CC, and Ninja toolchains
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
fn test_cargo_workspace_with_declared_members_detects_rust_only() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // Cargo workspace root
    let root_toml = r#"
[workspace]
members = [
    "crates/core",
    "crates/cli",
]
"#;
    fs::write(ws.join("Cargo.toml"), root_toml).unwrap();

    for member in &["crates/core", "crates/cli"] {
        let mdir = ws.join(member);
        fs::create_dir_all(&mdir).unwrap();
        fs::write(
            mdir.join("Cargo.toml"),
            format!("[package]\nname = \"{}\"\nversion = \"0.1.0\"\n", member),
        )
        .unwrap();
    }

    // Add an unrelated non-member directory with arbitrary files
    let stray = ws.join("unrelated_notes");
    fs::create_dir_all(&stray).unwrap();
    fs::write(stray.join("notes.txt"), "hello").unwrap();

    let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
    assert_eq!(detected.len(), 1);
    assert!(detected.contains(&ToolchainEcosystem::Rust));
}

#[test]
fn test_e2e_archive_packaging_and_isolated_import_readiness() {
    let src_temp = tempdir().unwrap();
    let src_ws = src_temp.path();

    // Create source workspace
    fs::write(
        src_ws.join("Cargo.toml"),
        "[package]\nname = \"e2e-app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(src_ws.join("src")).unwrap();
    fs::write(src_ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    // Create mock Rust toolchain to package
    let mock_rust_root = src_temp.path().join("mock_rust");
    let mock_rust_bin = mock_rust_root.join("bin");
    create_mock_bin(&mock_rust_bin, "cargo");
    create_mock_bin(&mock_rust_bin, "rustc");
    create_mock_bin(&mock_rust_bin, "rust-analyzer");

    let discovered = vec![DiscoveredToolchain {
        ecosystem: ToolchainEcosystem::Rust,
        version: "rustc 1.80.0".into(),
        root_dir: mock_rust_root.clone(),
        bin_files: vec![
            (mock_rust_bin.join("cargo"), "bin/cargo".into()),
            (mock_rust_bin.join("rustc"), "bin/rustc".into()),
            (
                mock_rust_bin.join("rust-analyzer"),
                "bin/rust-analyzer".into(),
            ),
        ],
        lib_files: vec![],
    }];

    // Package into in-memory tar archive
    let mut tar_builder = TarBuilder::new(Vec::new());

    let entries = ToolchainArchivePackager::package(&mut tar_builder, &discovered).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].ecosystem, ToolchainEcosystem::Rust);

    let tar_buffer = tar_builder.into_inner().unwrap();

    // Unpack tar into isolated target workspace
    let target_temp = tempdir().unwrap();
    let target_ws = target_temp.path();

    let mut archive = tar::Archive::new(&tar_buffer[..]);
    for entry_result in archive.entries().unwrap() {
        let mut entry = entry_result.unwrap();
        let path = entry.path().unwrap().to_path_buf();
        let target_path = target_ws.join(".dmz").join(path);
        fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        entry.unpack(target_path).unwrap();
    }

    // Run setup_imported_toolchains on target workspace
    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    // Verify out-of-the-box readiness:
    // 1. Cargo and rustc binaries are present and have 0o755 permissions
    let target_cargo = target_ws.join(".dmz/toolchains/rust/bin/cargo");
    assert!(target_cargo.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&target_cargo).unwrap().permissions();
        assert_eq!(perms.mode() & 0o111, 0o111, "Must be executable");
    }

    // 2. .vscode/settings.json exists with safe CMake settings and rust-analyzer path
    let settings_path = target_ws.join(".vscode/settings.json");
    assert!(settings_path.exists());
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(settings["cmake.configureOnOpen"], false);
    assert_eq!(
        settings["rust-analyzer.server.path"],
        "${workspaceFolder}/.dmz/toolchains/rust/bin/rust-analyzer"
    );

    // 3. .dmz/env.sh is valid shell script with execution permission
    let env_sh = target_ws.join(".dmz/env.sh");
    assert!(env_sh.exists());
    let env_sh_content = fs::read_to_string(&env_sh).unwrap();
    assert!(env_sh_content.contains("DMZ_WORKSPACE"));
    // Ensure no cc or applesdk toolchain was packaged or unpacked
    assert!(!target_ws.join(".dmz/toolchains/cc").exists());
    assert!(!target_ws.join(".dmz/toolchains/applesdk").exists());
    assert!(!target_ws.join(".dmz/toolchains/cmake").exists());

    // Verify runtime behavior: sourcing .dmz/env.sh activates only Rust and never exports CC, CMAKE, or DEVELOPER_DIR
    #[cfg(unix)]
    {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "cd \"{}\" && . .dmz/env.sh && env",
                target_ws.display()
            ))
            .output()
            .unwrap();
        let env_out = String::from_utf8_lossy(&output.stdout);
        assert!(!env_out.lines().any(|l| l.starts_with("CC=")));
        assert!(!env_out.lines().any(|l| l.starts_with("DEVELOPER_DIR=")));
        assert!(!env_out
            .lines()
            .any(|l| l.starts_with("CMAKE_TOOLCHAIN_FILE=")));
        assert!(env_out.lines().any(|l| l.starts_with("RUSTUP_HOME=")));
        assert!(env_out
            .lines()
            .any(|l| l.contains(".dmz/toolchains/rust/bin")));
    }
}

#[test]
fn test_pure_python_workspace_never_hallucinates_foreign_toolchains() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // Python workspace with dependencies that have native C-extensions (e.g. cffi, cryptography, cmake)
    fs::write(
        ws.join("pyproject.toml"),
        r#"[project]
name = "python-service"
version = "0.1.0"
dependencies = ["cffi", "cryptography", "cmake"]
"#,
    )
    .unwrap();
    fs::write(
        ws.join("requirements.txt"),
        "cffi>=1.15.0\ncryptography>=41.0.0\ncmake>=3.25.0\n",
    )
    .unwrap();
    fs::write(ws.join("poetry.lock"), "# poetry lock\n").unwrap();

    let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);

    assert_eq!(
        detected.len(),
        1,
        "Python workspace must strictly detect Python, but detected: {:?}",
        detected
    );
    assert!(detected.contains(&ToolchainEcosystem::Python));
    assert!(!detected.contains(&ToolchainEcosystem::CMake));
    assert!(!detected.contains(&ToolchainEcosystem::Cc));
    assert!(!detected.contains(&ToolchainEcosystem::AppleSdk));
    assert!(!detected.contains(&ToolchainEcosystem::Rust));

    // Target setup validation
    let target_dir = tempdir().unwrap();
    let target_ws = target_dir.path();
    let py_bin = target_ws.join(".dmz/toolchains/python/bin");
    create_mock_bin(&py_bin, "python3");
    create_mock_bin(&py_bin, "pip");
    fs::create_dir_all(target_ws.join(".dmz/toolchains/python/lib")).unwrap();

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(target_ws.join(".vscode/settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings["cmake.configureOnOpen"], false);
    assert_eq!(
        settings["python.defaultInterpreterPath"],
        if cfg!(windows) {
            "${workspaceFolder}/.dmz/toolchains/python/python.exe"
        } else {
            "${workspaceFolder}/.dmz/toolchains/python/bin/python3"
        }
    );
    assert!(!target_ws
        .join(".dmz/toolchains/dmz.toolchain.cmake")
        .exists());

    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut env_map);
    assert!(env_map.contains_key("PYTHONHOME"));
    assert!(!env_map.contains_key("CC"));
    assert!(!env_map.contains_key("DEVELOPER_DIR"));
    assert!(!env_map.contains_key("CMAKE_TOOLCHAIN_FILE"));
}

#[test]
fn test_pure_node_pnpm_workspace_never_hallucinates_foreign_toolchains() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // pnpm monorepo workspace with native-sounding package names
    fs::write(
        ws.join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
    )
    .unwrap();
    fs::write(
        ws.join("package.json"),
        r#"{
  "name": "monorepo",
  "devDependencies": {
    "node-gyp": "^10.0.0",
    "cmake-js": "^7.0.0",
    "swift-binding": "^1.0.0"
  }
}"#,
    )
    .unwrap();
    fs::write(ws.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();

    let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);

    assert_eq!(
        detected.len(),
        1,
        "pnpm/node workspace must strictly detect Node, but detected: {:?}",
        detected
    );
    assert!(detected.contains(&ToolchainEcosystem::Node));
    assert!(!detected.contains(&ToolchainEcosystem::CMake));
    assert!(!detected.contains(&ToolchainEcosystem::Cc));
    assert!(!detected.contains(&ToolchainEcosystem::AppleSdk));
    assert!(!detected.contains(&ToolchainEcosystem::Rust));

    // Target setup validation
    let target_dir = tempdir().unwrap();
    let target_ws = target_dir.path();
    let node_bin = target_ws.join(".dmz/toolchains/node/bin");
    create_mock_bin(&node_bin, "node");
    create_mock_bin(&node_bin, "npm");
    create_mock_bin(&node_bin, "pnpm");

    let node_modules = target_ws.join(".dmz/toolchains/node/lib/node_modules");
    fs::create_dir_all(&node_modules).unwrap();

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(target_ws.join(".vscode/settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings["cmake.configureOnOpen"], false);
    assert!(!target_ws
        .join(".dmz/toolchains/dmz.toolchain.cmake")
        .exists());

    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut env_map);
    assert!(env_map.contains_key("NODE_PATH"));
    let path = env_map.get("PATH").unwrap();
    assert!(path.replace('\\', "/").contains(".dmz/toolchains/node/bin"));
    assert!(!env_map.contains_key("CC"));
    assert!(!env_map.contains_key("DEVELOPER_DIR"));
    assert!(!env_map.contains_key("CMAKE_TOOLCHAIN_FILE"));
}

#[test]
fn test_pure_go_workspace_never_hallucinates_foreign_toolchains() {
    let temp = tempdir().unwrap();
    let ws = temp.path();

    // Go multi-module workspace
    fs::write(
        ws.join("go.work"),
        "go 1.22\n\nuse (\n\t./service\n\t./pkg\n)\n",
    )
    .unwrap();
    fs::write(ws.join("go.mod"), "module example.com/app\n\ngo 1.22\n").unwrap();

    let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);

    assert_eq!(detected.len(), 1);
    assert!(detected.contains(&ToolchainEcosystem::Go));
    assert!(!detected.contains(&ToolchainEcosystem::CMake));
    assert!(!detected.contains(&ToolchainEcosystem::Cc));
    assert!(!detected.contains(&ToolchainEcosystem::AppleSdk));

    let target_dir = tempdir().unwrap();
    let target_ws = target_dir.path();
    let go_bin = target_ws.join(".dmz/toolchains/go/bin");
    create_mock_bin(&go_bin, "go");

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(target_ws.join(".vscode/settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings["cmake.configureOnOpen"], false);
    assert_eq!(
        settings["go.goroot"],
        "${workspaceFolder}/.dmz/toolchains/go"
    );

    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut env_map);
    assert!(env_map.contains_key("GOROOT"));
    assert!(!env_map.contains_key("CC"));
    assert!(!env_map.contains_key("DEVELOPER_DIR"));
    assert!(!env_map.contains_key("CMAKE_TOOLCHAIN_FILE"));
}

#[test]
fn test_pure_php_and_ruby_workspaces_never_hallucinate_foreign_toolchains() {
    let php_temp = tempdir().unwrap();
    fs::write(php_temp.path().join("composer.json"), "{}").unwrap();
    fs::write(php_temp.path().join("composer.lock"), "{}").unwrap();

    let php_detected = ToolchainProvider::detect_workspace_ecosystems(php_temp.path(), None);
    assert_eq!(php_detected.len(), 1);
    assert!(php_detected.contains(&ToolchainEcosystem::Php));
    assert!(!php_detected.contains(&ToolchainEcosystem::CMake));
    assert!(!php_detected.contains(&ToolchainEcosystem::Cc));

    let ruby_temp = tempdir().unwrap();
    fs::write(
        ruby_temp.path().join("Gemfile"),
        "source 'https://rubygems.org'\n",
    )
    .unwrap();
    fs::write(ruby_temp.path().join("Gemfile.lock"), "").unwrap();

    let ruby_detected = ToolchainProvider::detect_workspace_ecosystems(ruby_temp.path(), None);
    assert_eq!(ruby_detected.len(), 1);
    assert!(ruby_detected.contains(&ToolchainEcosystem::Ruby));
    assert!(!ruby_detected.contains(&ToolchainEcosystem::CMake));
    assert!(!ruby_detected.contains(&ToolchainEcosystem::Cc));
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

    // 1. Verify sidecar file exists and contains matching SHA-256
    let sidecar_file = PathBuf::from(format!("{}.sha256", output_archive.display()));
    assert!(
        sidecar_file.exists(),
        "Sidecar .sha256 file must be generated"
    );
    let sidecar_content = fs::read_to_string(&sidecar_file).unwrap();
    assert!(sidecar_content.starts_with(&manifest.archive_sha256));

    // 2. Successful import with verified sidecar
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

    // 3. Tamper detection: Corrupt sidecar hash and verify import fails
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
fn test_imported_bundle_with_stray_applesdk_is_purged_and_never_poisons_developer_dir() {
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
    use dmz_core::resolver::lockfile::DmzLockfile;

    let temp = tempdir().unwrap();
    let src_ws = temp.path().join("src");
    fs::create_dir_all(&src_ws).unwrap();

    // 1. Create a pure Rust project
    fs::write(
        src_ws.join("Cargo.toml"),
        "[package]\nname = \"pure-rust-service\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(src_ws.join("src")).unwrap();
    fs::write(src_ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    let lockfile = DmzLockfile::new(Vec::new());
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();

    let store_dir = src_ws.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();

    let output_archive = temp.path().join("dist/bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: src_ws.clone(),
        lockfile_path: src_ws.join("dmz.lock"),
        store_dir: store_dir.clone(),
        output_path: output_archive.clone(),
        ..Default::default()
    };
    AirGapExporter::export(&export_cfg).expect("Export should succeed");

    // 2. Prepare target workspace that previously had a stray/corrupted applesdk stub directory
    let target_ws = temp.path().join("target_ws");
    let stray_applesdk_bin = target_ws.join(".dmz/toolchains/applesdk/bin");
    fs::create_dir_all(&stray_applesdk_bin).unwrap();
    fs::write(stray_applesdk_bin.join("xcrun"), "#!/bin/sh\nexit 1\n").unwrap();
    fs::write(stray_applesdk_bin.join("clang"), "#!/bin/sh\nexit 1\n").unwrap();

    // 3. Import the bundle into the target workspace
    let import_cfg = ImportConfig {
        archive_path: output_archive,
        target_store_dir: target_ws.join(".dmz/store"),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    };
    AirGapImporter::import(&import_cfg).expect("Import must succeed");

    // 4. Verify stray applesdk stub was purged by Root Manifest Dominance
    assert!(
        !target_ws.join(".dmz/toolchains/applesdk").exists(),
        "Stray applesdk stub must be purged upon importing a pure Rust workspace"
    );

    // 5. Verify environment injection never poisons DEVELOPER_DIR or PATH
    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(&target_ws, &mut env_map);

    assert!(
        !env_map.contains_key("DEVELOPER_DIR"),
        "DEVELOPER_DIR must NEVER be injected for non-Apple workspace"
    );
    assert!(
        !env_map.contains_key("SDKROOT"),
        "SDKROOT must NOT be injected"
    );
    if let Some(path) = env_map.get("PATH") {
        assert!(
            !path.contains("applesdk/bin"),
            "PATH must not contain stub applesdk/bin"
        );
    }

    // 6. Verify .dmz/env.sh does not export DEVELOPER_DIR
    let env_sh = target_ws.join(".dmz/env.sh");
    if env_sh.exists() {
        let content = fs::read_to_string(&env_sh).unwrap();
        assert!(
            !content.contains("export DEVELOPER_DIR="),
            "env.sh must NOT contain export DEVELOPER_DIR="
        );
        #[cfg(unix)]
        {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "cd \"{}\" && . .dmz/env.sh && env",
                    target_ws.display()
                ))
                .output()
                .unwrap();
            let env_out = String::from_utf8_lossy(&output.stdout);
            assert!(
                !env_out.lines().any(|l| l.starts_with("DEVELOPER_DIR=")),
                "DEVELOPER_DIR must NOT be exported in shell environment"
            );
        }
    }
}

#[test]
fn test_cross_platform_windows_binary_resolution_in_toolchain_env() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();

    // 1. Mock Windows-style toolchain layout with .exe extensions
    let toolchains_dir = ws.join(".dmz/toolchains");
    fs::create_dir_all(toolchains_dir.join("cc/bin")).unwrap();
    fs::create_dir_all(toolchains_dir.join("cmake/bin")).unwrap();
    fs::create_dir_all(toolchains_dir.join("cmake/share/cmake-3.28")).unwrap();
    fs::create_dir_all(toolchains_dir.join("ninja/bin")).unwrap();
    fs::create_dir_all(toolchains_dir.join("make/bin")).unwrap();
    fs::create_dir_all(toolchains_dir.join("pkgconfig/bin")).unwrap();
    fs::create_dir_all(toolchains_dir.join("pkgconfig/lib/pkgconfig")).unwrap();
    fs::create_dir_all(toolchains_dir.join("go/bin")).unwrap();

    fs::write(toolchains_dir.join("cc/bin/clang.exe"), "mock clang").unwrap();
    fs::write(toolchains_dir.join("cc/bin/clang++.exe"), "mock clang++").unwrap();
    fs::write(toolchains_dir.join("cc/bin/llvm-ar.exe"), "mock ar").unwrap();
    fs::write(toolchains_dir.join("cc/bin/llvm-ranlib.exe"), "mock ranlib").unwrap();
    fs::write(toolchains_dir.join("cmake/bin/cmake.exe"), "mock cmake").unwrap();
    fs::write(toolchains_dir.join("ninja/bin/ninja.exe"), "mock ninja").unwrap();
    fs::write(
        toolchains_dir.join("make/bin/mingw32-make.exe"),
        "mock make",
    )
    .unwrap();
    fs::write(
        toolchains_dir.join("pkgconfig/bin/pkg-config.exe"),
        "mock pkg-config",
    )
    .unwrap();
    fs::write(toolchains_dir.join("go/bin/go.exe"), "mock go").unwrap();

    // 2. Inject toolchain paths
    let mut env_map = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(ws, &mut env_map);

    // 3. Verify that .exe binaries are correctly resolved across all native toolchains
    assert!(
        env_map.get("CC").is_some_and(|v| v.ends_with("clang.exe")),
        "CC must resolve clang.exe, got: {:?}",
        env_map.get("CC")
    );
    assert!(
        env_map
            .get("CXX")
            .is_some_and(|v| v.ends_with("clang++.exe")),
        "CXX must resolve clang++.exe, got: {:?}",
        env_map.get("CXX")
    );
    assert!(
        env_map
            .get("AR")
            .is_some_and(|v| v.ends_with("llvm-ar.exe")),
        "AR must resolve llvm-ar.exe, got: {:?}",
        env_map.get("AR")
    );
    assert!(
        env_map
            .get("RANLIB")
            .is_some_and(|v| v.ends_with("llvm-ranlib.exe")),
        "RANLIB must resolve llvm-ranlib.exe, got: {:?}",
        env_map.get("RANLIB")
    );
    assert!(
        env_map
            .get("CMAKE")
            .is_some_and(|v| v.ends_with("cmake.exe")),
        "CMAKE must resolve cmake.exe, got: {:?}",
        env_map.get("CMAKE")
    );
    assert!(
        env_map
            .get("NINJA")
            .is_some_and(|v| v.ends_with("ninja.exe")),
        "NINJA must resolve ninja.exe, got: {:?}",
        env_map.get("NINJA")
    );
    assert!(
        env_map
            .get("MAKE")
            .is_some_and(|v| v.ends_with("mingw32-make.exe")),
        "MAKE must resolve mingw32-make.exe, got: {:?}",
        env_map.get("MAKE")
    );
    assert!(
        env_map
            .get("PKG_CONFIG")
            .is_some_and(|v| v.ends_with("pkg-config.exe")),
        "PKG_CONFIG must resolve pkg-config.exe, got: {:?}",
        env_map.get("PKG_CONFIG")
    );
    assert!(
        env_map.contains_key("PKG_CONFIG_PATH"),
        "PKG_CONFIG_PATH must be set"
    );
    assert!(
        env_map.contains_key("GOROOT"),
        "GOROOT must be set for go.exe"
    );

    // 4. Verify dmz.toolchain.cmake contains .exe checks
    dmz_core::toolchain::ToolchainSetup::generate_cmake_toolchain(ws).unwrap();
    let cmake_tc = toolchains_dir.join("dmz.toolchain.cmake");
    assert!(cmake_tc.exists(), "dmz.toolchain.cmake must be generated");
    let cmake_content = fs::read_to_string(&cmake_tc).unwrap();
    assert!(cmake_content.contains("clang.exe"));
    assert!(cmake_content.contains("llvm-ar.exe"));
    assert!(cmake_content.contains("ninja.exe"));
    assert!(cmake_content.contains("mingw32-make.exe"));
}

#[test]
fn test_bundle_import_enforces_1_to_1_toolchain_isolation_and_generates_ide_settings_without_bundled_toolchains(
) {
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
    use dmz_core::resolver::lockfile::DmzLockfile;

    let temp = tempdir().unwrap();
    let src_ws = temp.path().join("rust_only_src");
    fs::create_dir_all(src_ws.join("src")).unwrap();

    fs::write(
        src_ws.join("Cargo.toml"),
        r#"[package]
name = "pure-rust-app"
version = "1.0.0"
edition = "2021"

[dependencies]
"#,
    )
    .unwrap();
    fs::write(src_ws.join("src/main.rs"), "fn main() {}\n").unwrap();

    let lockfile = DmzLockfile::new(Vec::new());
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();
    fs::create_dir_all(src_ws.join(".dmz/store")).unwrap();

    let archive_path = temp.path().join("dist/pure_rust_bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: src_ws.clone(),
        lockfile_path: src_ws.join("dmz.lock"),
        store_dir: src_ws.join(".dmz/store"),
        output_path: archive_path.clone(),
        with_toolchain: false,
        ..Default::default()
    };
    AirGapExporter::export(&export_cfg).unwrap();

    // Target workspace has stray foreign toolchain directories (e.g. cmake, applesdk, go)
    let target_ws = temp.path().join("imported_rust_only");
    fs::create_dir_all(target_ws.join(".dmz/toolchains/cmake/bin")).unwrap();
    fs::create_dir_all(target_ws.join(".dmz/toolchains/applesdk/bin")).unwrap();
    fs::create_dir_all(target_ws.join(".dmz/toolchains/go/bin")).unwrap();

    let import_cfg = ImportConfig {
        archive_path,
        target_store_dir: target_ws.join(".dmz/store"),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    };
    AirGapImporter::import(&import_cfg).unwrap();

    // 1. Verify all unrequested foreign toolchains were purged (1-to-1 workspace isolation)
    assert!(!target_ws.join(".dmz/toolchains/cmake").exists());
    assert!(!target_ws.join(".dmz/toolchains/applesdk").exists());
    assert!(!target_ws.join(".dmz/toolchains/go").exists());
    assert!(
        !target_ws.join(".dmz/toolchains").exists(),
        "Empty .dmz/toolchains must be cleaned up when no toolchains remain"
    );

    // 2. Verify .vscode/settings.json was generated even though has_toolchains == false
    let settings_path = target_ws.join(".vscode/settings.json");
    assert!(
        settings_path.exists(),
        ".vscode/settings.json must be generated even when has_toolchains == false"
    );
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(settings["cmake.configureOnOpen"], false);
    assert_eq!(settings["cmake.autoSelectActiveFolder"], false);
}

#[test]
fn test_rust_workspace_with_cmake_dependency_bundles_and_installs_cmake_upon_import() {
    use dmz_core::archive::exporter::AirGapManifest;
    use dmz_core::archive::{AirGapExporter, AirGapImporter, ImportConfig};
    use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
    use dmz_core::resolver::manifest::Ecosystem;

    let temp = tempdir().unwrap();
    let src_ws = temp.path().join("rust_cmake_src");
    fs::create_dir_all(src_ws.join("src")).unwrap();

    // 1. Rust workspace that depends on flate2 with zlib-ng -> libz-ng-sys -> cmake (no root CMakeLists.txt)
    fs::write(
        src_ws.join("Cargo.toml"),
        r#"[package]
name = "zlib-ng-service"
version = "0.1.0"
edition = "2021"

[dependencies]
flate2 = { version = "1.0", features = ["zlib-ng"] }
"#,
    )
    .unwrap();
    fs::write(src_ws.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        src_ws.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "flate2"
version = "1.0.30"

[[package]]
name = "libz-ng-sys"
version = "1.1.15"

[[package]]
name = "cmake"
version = "0.1.50"
"#,
    )
    .unwrap();

    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "cmake".into(),
        version: "0.1.50".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        source_url: None,
        dependencies: vec![],
    }]);
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();

    // 2. Verify detection includes Rust, CMake, Cc, and Ninja (and never AppleSdk/AndroidNdk)
    let detected = ToolchainProvider::detect_workspace_ecosystems(&src_ws, Some(&lockfile));
    assert!(detected.contains(&ToolchainEcosystem::Rust));
    assert!(detected.contains(&ToolchainEcosystem::CMake));
    assert!(detected.contains(&ToolchainEcosystem::Cc));
    assert!(detected.contains(&ToolchainEcosystem::Ninja));
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

        tar.append_path_with_name(src_ws.join("dmz.lock"), "dmz.lock")
            .unwrap();
        tar.append_path_with_name(src_ws.join("Cargo.toml"), "Cargo.toml")
            .unwrap();
        tar.append_path_with_name(src_ws.join("Cargo.lock"), "Cargo.lock")
            .unwrap();
        tar.append_path_with_name(src_ws.join("src/main.rs"), "src/main.rs")
            .unwrap();

        let tc_entries = ToolchainArchivePackager::package(&mut tar, &discovered).unwrap();
        let manifest = AirGapManifest {
            dmz_version: "1.2.7".into(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: tc_entries,
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

        tar.into_inner().unwrap().finish().unwrap();
    }
    let archive_sha = AirGapExporter::calculate_file_sha256(&bundle_path).unwrap();
    fs::write(
        format!("{}.sha256", bundle_path.display()),
        format!("{}  rust_cmake_bundle.tar.zst\n", archive_sha),
    )
    .unwrap();

    // 4. Import bundle onto target machine and verify CMake is installed and configured!
    let target_ws = temp.path().join("imported_rust_cmake");
    AirGapImporter::import(&ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_ws.join(".dmz/store"),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    })
    .expect("Import must succeed and keep bundled CMake");

    assert!(
        target_ws.join(".dmz/toolchains/cmake/bin/cmake").is_file(),
        "Bundled CMake binary must be installed in .dmz/toolchains/cmake/bin/cmake upon import"
    );
    assert!(
        target_ws
            .join(".dmz/toolchains/dmz.toolchain.cmake")
            .is_file(),
        "dmz.toolchain.cmake must be generated for bundled CMake"
    );

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(target_ws.join(".vscode/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings["cmake.cmakePath"],
        "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
    );
    assert_eq!(
        settings["cmake.configureSettings"]["CMAKE_TOOLCHAIN_FILE"],
        "${workspaceFolder}/.dmz/toolchains/dmz.toolchain.cmake"
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

    // 1. Lockfile declares a package that is missing from .dmz/store
    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "missing-crate".into(),
        version: "1.0.0".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        source_url: None,
        dependencies: vec![],
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

    // 2. Construct an archive whose airgap_manifest.json declares a Rust toolchain binary ("rustc")
    // that was omitted from the archive's toolchains/rust/bin directory.
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

    // 1. Write complete polyglot workspace manifests (Rust + Python + Node + Go + CMake)
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

    // 2. Build real SHA-256 verified store artifacts for Cargo (`mini-util`) and npm (`js-util`)
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
        },
        LockedPackage {
            name: "js-util".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Npm,
            sha256: npm_sha,
            source_url: None,
            dependencies: vec![],
        },
    ]);
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();

    // 3. Create self-contained toolchain binaries for the declared ecosystems
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

    // 4. Package complete bundle (.tar.zst) with source, store, lockfile, and discovered toolchains + .sha256 sidecar
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

    // 5. Import onto a clean target machine directory & materialize offline dependencies
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

    // 6. Execute post-import verification across ALL active ecosystems + native build tools
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

        // Also verify bundled native build tools (cmake, clang, ninja) execute from stripped PATH
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
#[cfg(target_os = "macos")]
fn test_definitive_macos_clean_target_machine_native_compilation_proof() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    // 1. Locate real Xcode / Command Line Tools SDK and standalone Clang binary
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

    let real_sdk_path = match real_sdk {
        Some(s) if std::path::Path::new(&s).exists() => std::path::PathBuf::from(s),
        _ => return, // Skip test if host has no Xcode / SDK installed
    };

    let candidate_clangs = [
        "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
        "/Library/Developer/CommandLineTools/usr/bin/clang",
    ];
    let real_clang_path = match candidate_clangs
        .iter()
        .find(|p| std::path::Path::new(p).exists())
    {
        Some(p) => std::path::PathBuf::from(p),
        None => return,
    };

    // 2. Simulate imported toolchains directory on a clean target machine
    let cc_bin = target_ws.join(".dmz/toolchains/cc/bin");
    let apple_sdk = target_ws.join(".dmz/toolchains/applesdk/SDKs/MacOSX.sdk");
    fs::create_dir_all(&cc_bin).unwrap();
    fs::create_dir_all(&apple_sdk).unwrap();

    // Symlink SDK sysroot structure into target workspace
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        if real_sdk_path.join("usr").exists() {
            let _ = symlink(real_sdk_path.join("usr"), apple_sdk.join("usr"));
        }
        if real_sdk_path.join("System").exists() {
            let _ = symlink(real_sdk_path.join("System"), apple_sdk.join("System"));
        }
        if real_sdk_path.join("SDKSettings.plist").exists() {
            let _ = fs::copy(
                real_sdk_path.join("SDKSettings.plist"),
                apple_sdk.join("SDKSettings.plist"),
            );
        }
    }

    // Copy real standalone clang and linker binaries into cc/bin/
    fs::copy(&real_clang_path, cc_bin.join("clang")).unwrap();
    if let Some(bin_dir) = real_clang_path.parent() {
        if bin_dir.join("ld").exists() {
            let _ = fs::copy(bin_dir.join("ld"), cc_bin.join("ld"));
        }
        if let Some(toolchain_root) = bin_dir.parent() {
            let lib_dirs = [toolchain_root.join("usr/lib"), toolchain_root.join("lib")];
            let cc_lib = target_ws.join(".dmz/toolchains/cc/lib");
            fs::create_dir_all(&cc_lib).unwrap();
            for ldir in &lib_dirs {
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

    // Populate standalone make toolchain if available
    let make_candidates = [
        "/Applications/Xcode.app/Contents/Developer/usr/bin/make",
        "/Library/Developer/CommandLineTools/usr/bin/make",
    ];
    if let Some(make_src) = make_candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
    {
        let make_bin = target_ws.join(".dmz/toolchains/make/bin");
        fs::create_dir_all(&make_bin).unwrap();
        let _ = fs::copy(make_src, make_bin.join("make"));
    }

    // Populate compiler builtin resource directory (e.g. lib/clang/include) containing stdarg.h
    if let Ok(res_out) = std::process::Command::new(&real_clang_path)
        .arg("-print-resource-dir")
        .output()
    {
        if res_out.status.success() {
            let res_dir = std::path::PathBuf::from(String::from_utf8_lossy(&res_out.stdout).trim());
            let target_res_inc = target_ws.join(".dmz/toolchains/cc/lib/clang/include");
            fs::create_dir_all(&target_res_inc).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if let Ok(entries) = fs::read_dir(res_dir.join("include")) {
                    for entry in entries.flatten() {
                        let _ = symlink(entry.path(), target_res_inc.join(entry.file_name()));
                    }
                }
            }
        }
    }

    // Add minimal CMakeLists.txt to trigger CMake toolchain generation
    fs::write(
        target_ws.join("CMakeLists.txt"),
        r#"cmake_minimum_required(VERSION 3.10)
project(AirGapProof C)
find_path(ZLIB_INCLUDE_DIR zlib.h)
if(NOT ZLIB_INCLUDE_DIR)
    message(FATAL_ERROR "zlib.h NOT found in airgap sysroot")
endif()
add_executable(proof main.c)
target_link_libraries(proof z)
"#,
    )
    .unwrap();

    fs::write(
        target_ws.join("main.c"),
        r#"#include <stdio.h>
#include <stdlib.h>
#include <zlib.h>
int main() {
    printf("Hermetic AirGap Proof: zlib %s\n", zlibVersion());
    return 0;
}
"#,
    )
    .unwrap();

    // 3. Execute ToolchainSetup::setup_imported_toolchains
    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    // 4. Verify wrapper script exists and was generated
    assert!(cc_bin.join("clang.real").exists());
    assert!(cc_bin.join("clang").exists());

    // 5. DEFINITIVE PROOF #1: Direct Clang compilation under clean target machine conditions
    // Setting DEVELOPER_DIR to an invalid path simulates a clean target machine with NO Xcode/CLT!
    // Clearing SDKROOT ensures compilation relies 100% on DMZ's hermetic wrapper and bundled SDK.
    let output = std::process::Command::new(cc_bin.join("clang"))
        .arg("-o")
        .arg(target_ws.join("direct_proof"))
        .arg(target_ws.join("main.c"))
        .arg("-lz")
        .env(
            "DEVELOPER_DIR",
            "/nonexistent_developer_path_for_clean_target_machine",
        )
        .env_remove("SDKROOT")
        .output()
        .expect("Failed to invoke clang wrapper");

    assert!(
        output.status.success(),
        "Direct clang compilation failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = std::process::Command::new(target_ws.join("direct_proof"))
        .output()
        .expect("Failed to run compiled binary");
    assert!(run_output.status.success());
    let stdout_str = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout_str.contains("Hermetic AirGap Proof: zlib"));

    // 6. DEFINITIVE PROOF #2: CMake configuration & compilation using dmz.toolchain.cmake
    // CMake must be able to compile the test program and locate zlib.h under clean machine conditions!
    if let Ok(cmake_path) = which::which("cmake") {
        let build_dir = target_ws.join("cmake_build");
        fs::create_dir_all(&build_dir).unwrap();

        let cmake_configure = std::process::Command::new(&cmake_path)
            .arg("-B")
            .arg(&build_dir)
            .arg("-S")
            .arg(target_ws)
            .arg(format!(
                "-DCMAKE_TOOLCHAIN_FILE={}",
                target_ws
                    .join(".dmz/toolchains/dmz.toolchain.cmake")
                    .display()
            ))
            .env(
                "DEVELOPER_DIR",
                "/nonexistent_developer_path_for_clean_target_machine",
            )
            .env_remove("SDKROOT")
            .output()
            .expect("Failed to invoke cmake");

        assert!(
            cmake_configure.status.success(),
            "CMake configure failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&cmake_configure.stdout),
            String::from_utf8_lossy(&cmake_configure.stderr)
        );

        let cmake_build = std::process::Command::new(&cmake_path)
            .arg("--build")
            .arg(&build_dir)
            .env(
                "DEVELOPER_DIR",
                "/nonexistent_developer_path_for_clean_target_machine",
            )
            .env_remove("SDKROOT")
            .output()
            .expect("Failed to invoke cmake --build");

        assert!(
            cmake_build.status.success(),
            "CMake build failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&cmake_build.stdout),
            String::from_utf8_lossy(&cmake_build.stderr)
        );

        let cmake_run = std::process::Command::new(build_dir.join("proof"))
            .output()
            .expect("Failed to run cmake-built proof binary");
        assert!(cmake_run.status.success());
    }
}

#[test]
fn test_definitive_multi_ecosystem_compilation_proof() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    // 1. Definitively verify Go compilation with bundled standard library sources ($GOROOT/src)
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
                    let _ = fs::copy(&go_bin, go_bin_dir.join("go"));

                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::symlink;
                        if host_goroot.join("pkg").exists() {
                            let _ = symlink(host_goroot.join("pkg"), go_target_dir.join("pkg"));
                        }
                        let _ = symlink(host_goroot.join("src"), go_target_dir.join("src"));
                    }

                    fs::write(
                        target_ws.join("main.go"),
                        "package main\nimport \"fmt\"\nfunc main() {\n    fmt.Println(\"Hermetic Go Success\")\n}\n",
                    ).unwrap();

                    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

                    let mut envs = HashMap::new();
                    ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

                    let go_build = std::process::Command::new(go_bin_dir.join("go"))
                        .arg("build")
                        .arg("-o")
                        .arg(target_ws.join("go_app"))
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

                    let go_run = std::process::Command::new(target_ws.join("go_app"))
                        .output()
                        .expect("Failed to execute compiled go binary");
                    assert!(go_run.status.success());
                    assert!(String::from_utf8_lossy(&go_run.stdout).contains("Hermetic Go Success"));
                }
            }
        }
    }

    // 2. Definitively verify Python execution via bundled python / python3 binary with stdlib
    if let Ok(py_bin) = which::which("python3") {
        let py_target_dir = target_ws.join(".dmz/toolchains/python");
        let py_bin_dir = py_target_dir.join("bin");
        fs::create_dir_all(&py_bin_dir).unwrap();
        let _ = fs::copy(&py_bin, py_bin_dir.join("python3"));
        let _ = fs::copy(&py_bin, py_bin_dir.join("python"));

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
                    let target_lib = py_target_dir.join("lib");
                    fs::create_dir_all(&target_lib).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::symlink;
                        let _ = symlink(&stdlib_dir, target_lib.join(folder_name));
                    }
                }
            }
        }

        ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

        let mut envs = HashMap::new();
        ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

        let py_exec = std::process::Command::new(py_bin_dir.join("python"))
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

        // If host has lib/node_modules with npm or pnpm, wire them to prove execution
        let canonical_node = fs::canonicalize(&node_bin).unwrap_or_else(|_| node_bin.clone());
        if let Some(host_node_root) = canonical_node.parent().and_then(|p| p.parent()) {
            let host_nm = host_node_root.join("lib/node_modules");
            if host_nm.exists() {
                let target_nm = node_root.join("lib/node_modules");
                fs::create_dir_all(&target_nm).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    if host_nm.join("npm").exists() {
                        let _ = symlink(host_nm.join("npm"), target_nm.join("npm"));
                    }
                    if host_nm.join("pnpm").exists() {
                        let _ = symlink(host_nm.join("pnpm"), target_nm.join("pnpm"));
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

    // 4. Definitively verify standalone Make under clean target machine conditions
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

        // 5. Definitively verify Rust compilation and linking under clean target machine conditions
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
