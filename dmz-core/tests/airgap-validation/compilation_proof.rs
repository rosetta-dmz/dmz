use std::collections::HashMap;
use std::fs;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;

use dmz_core::toolchain::ToolchainPackager;

use super::common::link_or_copy_dir;

#[test]
fn test_definitive_multi_ecosystem_compilation_proof() {
    let temp = tempdir().unwrap();
    let target_ws = temp.path();

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
