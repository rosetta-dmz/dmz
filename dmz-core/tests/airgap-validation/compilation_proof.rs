use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path};
use tempfile::tempdir;

use dmz_core::toolchain::discovery::is_non_standalone_path;
use dmz_core::toolchain::discovery::python::is_unrelocatable_macos_python;
use dmz_core::toolchain::{
    DiscoveredToolchain, GoDiscoverer, NodeDiscoverer, PythonDiscoverer, ToolchainDiscoverer,
    ToolchainPackager,
};

fn require_toolchain_proofs() -> bool {
    std::env::var("DMZ_REQUIRE_TOOLCHAIN_PROOFS").as_deref() == Ok("1")
}

fn copy_file_strict(src: &Path, dst: &Path) {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            panic!("Failed creating parent dir for {:?}: {}", dst, e);
        });
    }
    fs::copy(src, dst).unwrap_or_else(|e| {
        panic!("Failed copying {:?} -> {:?}: {}", src, dst, e);
    });
}

fn materialize_discovered_toolchain(tc: &DiscoveredToolchain, target_dir: &Path) {
    fs::create_dir_all(target_dir).unwrap();

    for (src, rel_path) in tc.bin_files.iter().chain(tc.lib_files.iter()) {
        assert!(
            !is_non_standalone_path(src),
            "{:?} discoverer returned non-standalone package-manager path: {:?}",
            tc.ecosystem,
            src
        );
        assert!(
            !Path::new(rel_path)
                .components()
                .any(|c| !matches!(c, Component::Normal(_))),
            "Unexpected non-normal relative path in DiscoveredToolchain: {}",
            rel_path
        );

        copy_file_strict(src, &target_dir.join(rel_path));
    }
}

#[test]
fn test_compilation_proof_go() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    let Some(tc) = GoDiscoverer
        .discover()
        .expect("GoDiscoverer::discover should not error")
    else {
        assert!(
            !require_toolchain_proofs(),
            "Standalone Go toolchain required when DMZ_REQUIRE_TOOLCHAIN_PROOFS=1"
        );
        return;
    };

    let go_target_dir = target_ws.join(".dmz/toolchains/go");
    materialize_discovered_toolchain(&tc, &go_target_dir);

    fs::write(
        target_ws.join("main.go"),
        "package main\nimport \"fmt\"\nfunc main() {\n    fmt.Println(\"Hermetic Go Success\")\n}\n",
    )
    .unwrap();

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let mut envs = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

    let go_exe_name = if cfg!(windows) { "go.exe" } else { "go" };
    let go_app_name = if cfg!(windows) {
        "go_app.exe"
    } else {
        "go_app"
    };

    let go_build = std::process::Command::new(go_target_dir.join("bin").join(go_exe_name))
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

#[test]
fn test_compilation_proof_python() {
    #[cfg(target_os = "macos")]
    assert!(
        is_unrelocatable_macos_python(Path::new("/usr/bin/python3")),
        "/usr/bin/python3 xcode-select stub must be rejected as non-relocatable"
    );

    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    let Some(tc) = PythonDiscoverer
        .discover()
        .expect("PythonDiscoverer::discover should not error")
    else {
        assert!(
            !require_toolchain_proofs(),
            "Standalone Python toolchain (uv/python.org) required when DMZ_REQUIRE_TOOLCHAIN_PROOFS=1"
        );
        return;
    };

    for (src, _) in &tc.bin_files {
        assert!(
            !is_unrelocatable_macos_python(src),
            "PythonDiscoverer returned unrelocatable host stub/Xcode path: {:?}",
            src
        );
    }

    let py_target_dir = target_ws.join(".dmz/toolchains/python");
    let py_bin_dir = py_target_dir.join("bin");
    materialize_discovered_toolchain(&tc, &py_target_dir);

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
        .arg("import sys, json; print(f'Hermetic Python {sys.version_info.major} Success')")
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

    // Verify relocated python works via `source .dmz/env.sh` + hydrated `.venv` without
    // relying on `DYLD_LIBRARY_PATH` (stripped by macOS SIP) and resolves `.dmz/venv` packages.
    #[cfg(unix)]
    {
        let py_ver = dmz_core::install::mappers::PyPiMapper::detect_python_version(target_ws);
        let dmz_sp = target_ws
            .join(".dmz/venv/lib")
            .join(&py_ver)
            .join("site-packages/fake_pydantic");
        fs::create_dir_all(&dmz_sp).unwrap();
        fs::write(dmz_sp.join("__init__.py"), "VERSION = 'test'\n").unwrap();

        dmz_core::install::mappers::PyPiMapper::hydrate_venv_environment(
            target_ws,
            &target_ws.join(".venv"),
            &py_ver,
        )
        .expect("hydrate_venv_environment must succeed");
        ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

        let shell_cmd = format!(
            "unset DYLD_LIBRARY_PATH LD_LIBRARY_PATH; . \"{}/.dmz/env.sh\" >/dev/null && python -c \"import sys, json, ctypes, ssl, sqlite3, typing, fake_pydantic; assert hasattr(typing, 'TypeVar'); assert 'site-packages' not in (typing.__file__ or ''), typing.__file__; print(sys.executable, fake_pydantic.VERSION)\"",
            target_ws.to_string_lossy()
        );
        let sh_out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&shell_cmd)
            .output()
            .expect("Failed to run shell env.sh + python");
        assert!(
            sh_out.status.success(),
            "Sourced env.sh python execution failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&sh_out.stdout),
            String::from_utf8_lossy(&sh_out.stderr)
        );
        let stdout = String::from_utf8_lossy(&sh_out.stdout);
        assert!(
            stdout.contains("test"),
            "Expected fake_pydantic import output, got: {}",
            stdout
        );
    }

    #[cfg(windows)]
    {
        let py_ver = dmz_core::install::mappers::PyPiMapper::detect_python_version(target_ws);
        let dmz_sp = target_ws
            .join(".dmz/venv/lib")
            .join(&py_ver)
            .join("site-packages/fake_pydantic");
        fs::create_dir_all(&dmz_sp).unwrap();
        fs::write(dmz_sp.join("__init__.py"), "VERSION = 'test'\n").unwrap();

        dmz_core::install::mappers::PyPiMapper::hydrate_venv_environment(
            target_ws,
            &target_ws.join(".venv"),
            &py_ver,
        )
        .expect("hydrate_venv_environment must succeed");
        ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

        let venv_py = target_ws.join(".venv/Scripts/python.exe");
        let py_out = std::process::Command::new(&venv_py)
            .arg("-c")
            .arg("import sys, ctypes, json; print(sys.executable)")
            .env("PATH", "")
            .output()
            .expect("Failed to invoke .venv\\Scripts\\python.exe directly");
        assert!(
            py_out.status.success(),
            "Direct Windows .venv python failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&py_out.stdout),
            String::from_utf8_lossy(&py_out.stderr)
        );
    }
}

#[test]
fn test_compilation_proof_node() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    let Some(tc) = NodeDiscoverer
        .discover()
        .expect("NodeDiscoverer::discover should not error")
    else {
        assert!(
            !require_toolchain_proofs(),
            "Standalone Node.js toolchain required when DMZ_REQUIRE_TOOLCHAIN_PROOFS=1"
        );
        return;
    };

    let node_root = target_ws.join(".dmz/toolchains/node");
    let node_bin_dir = node_root.join("bin");
    materialize_discovered_toolchain(&tc, &node_root);

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let mut envs = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

    let node_exe = if cfg!(windows) { "node.exe" } else { "node" };
    let node_exec = std::process::Command::new(node_bin_dir.join(node_exe))
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

    for mgr in ["npm", "pnpm"] {
        let mgr_path = node_bin_dir.join(mgr);
        if mgr_path.exists() {
            let mgr_exec = std::process::Command::new(&mgr_path)
                .arg("--version")
                .envs(&envs)
                .output()
                .unwrap_or_else(|e| panic!("Failed to invoke bundled {}: {}", mgr, e));
            assert!(
                mgr_exec.status.success(),
                "{} execution failed: stdout={}\nstderr={}",
                mgr,
                String::from_utf8_lossy(&mgr_exec.stdout),
                String::from_utf8_lossy(&mgr_exec.stderr)
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_compilation_proof_macos_make() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    fs::write(
        target_ws.join("Makefile"),
        "all:\n\t@echo \"Hermetic Make Success\"\n",
    )
    .unwrap();

    if !dmz_core::toolchain::check_apple_developer_tools().is_ready() {
        assert!(
            !require_toolchain_proofs(),
            "macOS Apple CLT/Xcode required when DMZ_REQUIRE_TOOLCHAIN_PROOFS=1"
        );
        return;
    }

    // Host prerequisite check must pass on macOS for Make workspaces
    let prereq = dmz_core::toolchain::verify_apple_prereq_for_workspace(target_ws, None)
        .expect("Apple CLT pre-flight must succeed on ready macOS host");
    assert!(prereq.is_some());

    // AppleSdk must not be bundled into .dmz/toolchains
    let discovered =
        dmz_core::toolchain::ToolchainProvider::discover_for_workspace(target_ws, None);
    assert!(
        !discovered
            .iter()
            .any(|tc| tc.ecosystem == dmz_core::toolchain::ToolchainEcosystem::AppleSdk),
        "AppleSdk must never be bundled"
    );

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let make_exec = std::process::Command::new("/usr/bin/make")
        .current_dir(target_ws)
        .output()
        .expect("Failed to invoke host /usr/bin/make");

    assert!(
        make_exec.status.success(),
        "Make failed against host CLT/Xcode: stdout={}\nstderr={}",
        String::from_utf8_lossy(&make_exec.stdout),
        String::from_utf8_lossy(&make_exec.stderr)
    );
    assert!(String::from_utf8_lossy(&make_exec.stdout).contains("Hermetic Make Success"));
}

#[cfg(target_os = "macos")]
#[test]
fn test_compilation_proof_macos_rust() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

    let Ok(rustc_bin) = which::which("rustc") else {
        assert!(
            !require_toolchain_proofs(),
            "rustc required when DMZ_REQUIRE_TOOLCHAIN_PROOFS=1"
        );
        return;
    };

    if !dmz_core::toolchain::check_apple_developer_tools().is_ready() {
        assert!(
            !require_toolchain_proofs(),
            "macOS Apple CLT/Xcode required when DMZ_REQUIRE_TOOLCHAIN_PROOFS=1"
        );
        return;
    }

    fs::write(
        target_ws.join("Cargo.toml"),
        "[package]\nname = \"proof\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        target_ws.join("main.rs"),
        "fn main() {\n    println!(\"Hermetic Rust Success\");\n}\n",
    )
    .unwrap();

    // Host prerequisite verification must pass for Rust on macOS without bundling applesdk
    let prereq = dmz_core::toolchain::verify_apple_prereq_for_workspace(target_ws, None)
        .expect("Apple CLT pre-flight must succeed for Rust workspace on ready macOS host");
    assert!(prereq.is_some());

    let discovered =
        dmz_core::toolchain::ToolchainProvider::discover_for_workspace(target_ws, None);
    assert!(
        !discovered
            .iter()
            .any(|tc| tc.ecosystem == dmz_core::toolchain::ToolchainEcosystem::AppleSdk),
        "AppleSdk must never be bundled"
    );

    ToolchainPackager::setup_imported_toolchains(target_ws).unwrap();

    let mut envs = HashMap::new();
    ToolchainPackager::inject_toolchain_paths(target_ws, &mut envs);

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
