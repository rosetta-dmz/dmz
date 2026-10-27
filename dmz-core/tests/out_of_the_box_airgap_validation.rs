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

    // 1. Setup a real Rust project with proc-macros and common crates that previously
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
cmake = "0.1"
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
name = "cmake"
version = "0.1.50"

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
        path.contains(".dmz/toolchains/rust/bin"),
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
    assert!(path.contains(".dmz/toolchains/node/bin"));
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
