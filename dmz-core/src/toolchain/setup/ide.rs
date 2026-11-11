// IDE workspace settings and multi-root workspace configuration

use std::fs;
use std::path::Path;

/// Configures `.vscode/settings.json` so IDE extensions work out-of-the-box in air-gapped environments.
/// In non-CMake workspaces, silences CMake Tools extension auto-probing to eliminate "Bad CMake executable: ''" popups.
/// In CMake workspaces with bundled CMake, automatically configures cmake.cmakePath and toolchain file.
/// Pre-configures terminal PATH and language server environments (such as rust-analyzer).
pub fn generate_ide_settings(workspace_root: &Path) -> Result<(), String> {
    let vscode_dir = workspace_root.join(".vscode");
    let settings_path = vscode_dir.join("settings.json");

    let mut settings: serde_json::Map<String, serde_json::Value> = if settings_path.exists() {
        match fs::read_to_string(&settings_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => serde_json::Map::new(),
        }
    } else {
        serde_json::Map::new()
    };

    let toolchains_dir = workspace_root.join(".dmz/toolchains");
    let bundled_cmake_bin =
        crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("cmake"), "bin/cmake");
    let has_cmake = workspace_root.join("CMakeLists.txt").exists() || bundled_cmake_bin.is_some();

    if has_cmake {
        if let Some(ref bin) = bundled_cmake_bin {
            let rel_bin = if bin.extension().is_some_and(|e| e == "exe") {
                "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake.exe"
            } else {
                "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
            };
            settings.insert(
                "cmake.cmakePath".into(),
                serde_json::Value::String(rel_bin.into()),
            );
        }
        if toolchains_dir.join("dmz.toolchain.cmake").exists() {
            let mut configure_settings = settings
                .get("cmake.configureSettings")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            configure_settings.insert(
                "CMAKE_TOOLCHAIN_FILE".into(),
                serde_json::Value::String(
                    "${workspaceFolder}/.dmz/toolchains/dmz.toolchain.cmake".into(),
                ),
            );
            settings.insert(
                "cmake.configureSettings".into(),
                serde_json::Value::Object(configure_settings),
            );
        }
        if crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("ninja"), "bin/ninja")
            .is_some()
        {
            settings.insert(
                "cmake.generator".into(),
                serde_json::Value::String("Ninja".into()),
            );
        }
        settings.insert(
            "cmake.configureOnOpen".into(),
            serde_json::Value::Bool(true),
        );
    } else {
        // Workspace does NOT use CMake.
        settings.insert(
            "cmake.configureOnOpen".into(),
            serde_json::Value::Bool(false),
        );
        settings.insert(
            "cmake.autoSelectActiveFolder".into(),
            serde_json::Value::Bool(false),
        );
    }

    let bundled_make_bin =
        crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("make"), "bin/make")
            .or_else(|| {
                crate::toolchain::ToolchainEnv::find_binary(
                    &toolchains_dir.join("make"),
                    "bin/mingw32-make",
                )
            });
    if let Some(ref bin) = bundled_make_bin {
        if let Some(fname) = bin.file_name().and_then(|n| n.to_str()) {
            settings.insert(
                "makefile.makePath".into(),
                serde_json::Value::String(format!(
                    "${{workspaceFolder}}/.dmz/toolchains/make/bin/{}",
                    fname
                )),
            );
        }
    }

    // Configure terminal environment for bundled toolchains (e.g., Rust, Go, Python)
    if toolchains_dir.exists() {
        let mut bin_subpaths: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(&toolchains_dir) {
            for entry in entries.flatten() {
                let name_os = entry.file_name();
                if name_os == "applesdk"
                    && !crate::toolchain::ToolchainEnv::is_genuine_apple_developer_dir(
                        &entry.path(),
                    )
                {
                    continue;
                }
                let bin = entry.path().join("bin");
                if bin.exists() {
                    let name = name_os.to_string_lossy().to_string();
                    bin_subpaths.push(format!("${{workspaceFolder}}/.dmz/toolchains/{}/bin", name));
                }
            }
        }

        if !bin_subpaths.is_empty() {
            let unix_path = format!("{}:${{env:PATH}}", bin_subpaths.join(":"));
            let win_path = format!("{};${{env:PATH}}", bin_subpaths.join(";"));

            for platform in &[
                "terminal.integrated.env.osx",
                "terminal.integrated.env.linux",
            ] {
                let mut env_obj = settings
                    .get(*platform)
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                env_obj.insert("PATH".into(), serde_json::Value::String(unix_path.clone()));
                settings.insert(platform.to_string(), serde_json::Value::Object(env_obj));
            }

            let mut win_env = settings
                .get("terminal.integrated.env.windows")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            win_env.insert("PATH".into(), serde_json::Value::String(win_path));
            settings.insert(
                "terminal.integrated.env.windows".into(),
                serde_json::Value::Object(win_env),
            );
        }

        // Configure rust-analyzer if Rust is bundled
        if toolchains_dir.join("rust").exists() {
            if let Some(ra_bin) = crate::toolchain::ToolchainEnv::find_binary(
                &toolchains_dir.join("rust"),
                "bin/rust-analyzer",
            ) {
                let fname = ra_bin
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("rust-analyzer");
                settings.insert(
                    "rust-analyzer.server.path".into(),
                    serde_json::Value::String(format!(
                        "${{workspaceFolder}}/.dmz/toolchains/rust/bin/{}",
                        fname
                    )),
                );
            }

            for env_setting_key in &[
                "rust-analyzer.runnables.extraEnv",
                "rust-analyzer.cargo.extraEnv",
            ] {
                let mut ra_extra_env = settings
                    .get(*env_setting_key)
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                ra_extra_env.insert(
                    "RUSTUP_HOME".into(),
                    serde_json::Value::String("${workspaceFolder}/.dmz/toolchains/rust".into()),
                );
                ra_extra_env.insert(
                    "CARGO_HOME".into(),
                    serde_json::Value::String("${workspaceFolder}/.cargo".into()),
                );
                if let Some(ref bin) = bundled_cmake_bin {
                    let fname = bin.file_name().and_then(|n| n.to_str()).unwrap_or("cmake");
                    ra_extra_env.insert(
                        "CMAKE".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/cmake/bin/{}",
                            fname
                        )),
                    );
                }
                if let Some(cc_bin) = crate::toolchain::ToolchainEnv::find_binary(
                    &toolchains_dir.join("cc"),
                    "bin/clang",
                )
                .or_else(|| {
                    crate::toolchain::ToolchainEnv::find_binary(
                        &toolchains_dir.join("cc"),
                        "bin/gcc",
                    )
                }) {
                    let fname = cc_bin
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("clang");
                    ra_extra_env.insert(
                        "CC".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/cc/bin/{}",
                            fname
                        )),
                    );
                }
                if let Some(ninja_bin) = crate::toolchain::ToolchainEnv::find_binary(
                    &toolchains_dir.join("ninja"),
                    "bin/ninja",
                ) {
                    let fname = ninja_bin
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("ninja");
                    ra_extra_env.insert(
                        "NINJA".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/ninja/bin/{}",
                            fname
                        )),
                    );
                }
                if let Some(ref make_bin) = bundled_make_bin {
                    let fname = make_bin
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("make");
                    ra_extra_env.insert(
                        "MAKE".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/make/bin/{}",
                            fname
                        )),
                    );
                }
                if let Some(pkg_bin) = crate::toolchain::ToolchainEnv::find_binary(
                    &toolchains_dir.join("pkgconfig"),
                    "bin/pkg-config",
                )
                .or_else(|| {
                    crate::toolchain::ToolchainEnv::find_binary(
                        &toolchains_dir.join("pkgconfig"),
                        "bin/pkgconf",
                    )
                }) {
                    let fname = pkg_bin
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("pkg-config");
                    ra_extra_env.insert(
                        "PKG_CONFIG".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/pkgconfig/bin/{}",
                            fname
                        )),
                    );
                }
                if let Some(sdk_path) = crate::toolchain::ToolchainEnv::find_apple_sdk_root(
                    &toolchains_dir.join("applesdk"),
                ) {
                    if let Ok(rel) = sdk_path.strip_prefix(workspace_root) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        ra_extra_env.insert(
                            "SDKROOT".into(),
                            serde_json::Value::String(format!("${{workspaceFolder}}/{}", rel_str)),
                        );
                    }
                }
                settings.insert(
                    (*env_setting_key).into(),
                    serde_json::Value::Object(ra_extra_env),
                );
            }
        }

        // Configure Python interpreter path:
        // Distinguish between a true umbrella root (regular file `dmz-workspace.toml`)
        // and a member directory inside `workspaces/<member>` (where `dmz-workspace.toml` is a symlink).
        let manifest_meta = fs::symlink_metadata(workspace_root.join("dmz-workspace.toml"));
        let is_umbrella = manifest_meta
            .as_ref()
            .map(|m| m.is_file() && !m.file_type().is_symlink())
            .unwrap_or(false);
        let is_umbrella_member = manifest_meta
            .as_ref()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        let has_python_project = workspace_root.join("requirements.txt").is_file()
            || workspace_root.join("pyproject.toml").is_file()
            || workspace_root.join("setup.py").is_file()
            || workspace_root.join("Pipfile").is_file();
        let member_venv = workspace_root.join(".venv");
        let has_valid_venv = member_venv.join("bin/python").exists()
            || member_venv.join("Scripts/python.exe").exists();

        if !is_umbrella && (!is_umbrella_member || has_python_project || has_valid_venv) {
            if has_valid_venv {
                let py_bin = if cfg!(windows) {
                    "${workspaceFolder}/.venv/Scripts/python.exe"
                } else {
                    "${workspaceFolder}/.venv/bin/python"
                };
                settings.insert(
                    "python.defaultInterpreterPath".into(),
                    serde_json::Value::String(py_bin.into()),
                );
                settings.insert(
                    "python.terminal.activateEnvironment".into(),
                    serde_json::Value::Bool(true),
                );
            } else if toolchains_dir.join("python").exists() {
                let py_dir = toolchains_dir.join("python");
                let has_py_bin = py_dir.join("bin/python3").exists()
                    || py_dir.join("bin/python").exists()
                    || py_dir.join("bin/pip").exists()
                    || py_dir.join("python.exe").exists()
                    || py_dir.join("bin/python.exe").exists();
                if has_py_bin {
                    let py_bin = if cfg!(windows) {
                        "${workspaceFolder}/.dmz/toolchains/python/python.exe"
                    } else {
                        "${workspaceFolder}/.dmz/toolchains/python/bin/python3"
                    };
                    settings.insert(
                        "python.defaultInterpreterPath".into(),
                        serde_json::Value::String(py_bin.into()),
                    );
                }
            }
        }

        if !is_umbrella
            && workspace_root.join("src-tauri/Cargo.toml").is_file()
            && !workspace_root.join("Cargo.toml").is_file()
        {
            settings.insert(
                "rust-analyzer.linkedProjects".into(),
                serde_json::json!(["src-tauri/Cargo.toml"]),
            );
        }

        // Configure Go if Go is bundled with a verified go binary
        if toolchains_dir.join("go").exists() {
            let go_dir = toolchains_dir.join("go");
            if let Some(go_bin) = crate::toolchain::ToolchainEnv::find_binary(&go_dir, "bin/go") {
                let fname = go_bin.file_name().and_then(|n| n.to_str()).unwrap_or("go");
                settings.insert(
                    "go.goroot".into(),
                    serde_json::Value::String("${workspaceFolder}/.dmz/toolchains/go".into()),
                );
                let mut alt_tools = settings
                    .get("go.alternateTools")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                alt_tools.insert(
                    "go".into(),
                    serde_json::Value::String(format!(
                        "${{workspaceFolder}}/.dmz/toolchains/go/bin/{}",
                        fname
                    )),
                );
                settings.insert(
                    "go.alternateTools".into(),
                    serde_json::Value::Object(alt_tools),
                );
            }
        }
    }

    fs::create_dir_all(&vscode_dir)
        .map_err(|e| format!("Failed to create .vscode directory: {}", e))?;

    let json_string = serde_json::to_string_pretty(&serde_json::Value::Object(settings))
        .map_err(|e| format!("Failed to serialize settings.json: {}", e))?;

    fs::write(&settings_path, json_string)
        .map_err(|e| format!("Failed to write .vscode/settings.json: {}", e))?;

    // If this workspace is a true umbrella multi-workspace root (not a symlinked member), generate the .code-workspace file
    let is_true_umbrella = fs::symlink_metadata(workspace_root.join("dmz-workspace.toml"))
        .map(|m| m.is_file() && !m.file_type().is_symlink())
        .unwrap_or(false);
    if is_true_umbrella {
        let _ = generate_multi_root_workspace(workspace_root);
    }

    Ok(())
}

/// Generates or updates `<project>.code-workspace` for multi-root projects
pub fn generate_multi_root_workspace(workspace_root: &Path) -> Result<(), String> {
    let manifest_path = workspace_root.join("dmz-workspace.toml");
    if !manifest_path.exists() {
        return Ok(());
    }

    let ws_project = match crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
    {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };

    let mut folders = Vec::new();
    let mut linked_rust_projects = Vec::new();
    let mut node_bin_unix = Vec::new();
    let mut node_bin_win = Vec::new();

    for m in &ws_project.members {
        let m_path_str = m.path.to_string_lossy().to_string();
        let member_full_path = workspace_root.join(&m.path);

        let is_python = m.ecosystem.as_deref() == Some("python")
            || m.ecosystem.as_deref() == Some("pypi")
            || member_full_path.join("requirements.txt").exists()
            || member_full_path.join("pyproject.toml").exists();

        let is_rust = m.ecosystem.as_deref() == Some("rust")
            || m.ecosystem.as_deref() == Some("cargo")
            || member_full_path.join("Cargo.toml").exists()
            || member_full_path.join("src-tauri/Cargo.toml").exists();

        let is_node = m.ecosystem.as_deref() == Some("node")
            || m.ecosystem.as_deref() == Some("npm")
            || member_full_path.join("package.json").exists();

        if is_rust {
            if member_full_path.join("Cargo.toml").exists() {
                linked_rust_projects.push(format!("{}/Cargo.toml", m_path_str));
            }
            if member_full_path.join("src-tauri/Cargo.toml").exists() {
                linked_rust_projects.push(format!("{}/src-tauri/Cargo.toml", m_path_str));
            }
        }

        if is_node {
            node_bin_unix.push(format!(
                "${{workspaceFolder}}/{}/node_modules/.bin",
                m_path_str
            ));
            node_bin_win.push(format!(
                "${{workspaceFolder}}\\{}\\node_modules\\.bin",
                m_path_str
            ));
        }

        let label = if !m.id.is_empty() {
            m.id.clone()
        } else {
            m_path_str.to_string()
        };

        let mut folder_entry = serde_json::json!({
            "name": label,
            "path": m_path_str
        });

        if is_python {
            let mut py_settings = serde_json::json!({
                "python.terminal.activateEnvironment": true
            });

            let has_bundled_py = workspace_root.join(".dmz/toolchains/python").exists();
            let member_venv = member_full_path.join(".venv");

            let depth = m
                .path
                .components()
                .filter(|c| matches!(c, std::path::Component::Normal(_)))
                .count();
            let rel_prefix = if depth == 0 {
                ".".to_string()
            } else {
                vec![".."; depth].join("/")
            };

            let py_bin_exists = member_venv.join("bin/python").exists()
                || member_venv.join("Scripts/python.exe").exists();

            if member_venv.exists() && py_bin_exists {
                let py_bin = if cfg!(windows) {
                    "${workspaceFolder}/.venv/Scripts/python.exe"
                } else {
                    "${workspaceFolder}/.venv/bin/python"
                };
                if let Some(obj) = py_settings.as_object_mut() {
                    obj.insert(
                        "python.defaultInterpreterPath".to_string(),
                        serde_json::Value::String(py_bin.to_string()),
                    );
                }
            } else if has_bundled_py {
                let py_bin = if rel_prefix == "." {
                    if cfg!(windows) {
                        "${workspaceFolder}/.dmz/toolchains/python/python.exe".to_string()
                    } else {
                        "${workspaceFolder}/.dmz/toolchains/python/bin/python3".to_string()
                    }
                } else if cfg!(windows) {
                    format!(
                        "${{workspaceFolder}}/{}/.dmz/toolchains/python/python.exe",
                        rel_prefix
                    )
                } else {
                    format!(
                        "${{workspaceFolder}}/{}/.dmz/toolchains/python/bin/python3",
                        rel_prefix
                    )
                };
                if let Some(obj) = py_settings.as_object_mut() {
                    obj.insert(
                        "python.defaultInterpreterPath".to_string(),
                        serde_json::Value::String(py_bin),
                    );
                }
            }

            if let Some(obj) = folder_entry.as_object_mut() {
                obj.insert("settings".to_string(), py_settings);
            }
        }

        folders.push(folder_entry);
    }

    if workspace_root.join("documentation").is_dir() {
        folders.push(serde_json::json!({
            "name": "Documentation",
            "path": "documentation"
        }));
    }

    let project_name = if !ws_project.name.is_empty() {
        &ws_project.name
    } else {
        "workspace"
    };

    let unix_node_prefix = if !node_bin_unix.is_empty() {
        format!("{}:", node_bin_unix.join(":"))
    } else {
        String::new()
    };

    let win_node_prefix = if !node_bin_win.is_empty() {
        format!("{};", node_bin_win.join(";"))
    } else {
        String::new()
    };

    let toolchains_dir = workspace_root.join(".dmz/toolchains");
    let mut bin_subpaths: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(&toolchains_dir) {
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            if name_os == "applesdk"
                && !crate::toolchain::ToolchainEnv::is_genuine_apple_developer_dir(&entry.path())
            {
                continue;
            }
            let bin = entry.path().join("bin");
            if bin.exists() {
                let name = name_os.to_string_lossy().to_string();
                bin_subpaths.push(format!("${{workspaceFolder}}/.dmz/toolchains/{}/bin", name));
            }
        }
    }

    let unix_path = if bin_subpaths.is_empty() {
        format!("{}${{workspaceFolder}}/.dmz/toolchains/node/bin:${{workspaceFolder}}/.dmz/toolchains/rust/bin:${{workspaceFolder}}/.dmz/toolchains/python/bin:${{env:PATH}}", unix_node_prefix)
    } else {
        format!(
            "{}{}:${{env:PATH}}",
            unix_node_prefix,
            bin_subpaths.join(":")
        )
    };

    let win_path = if bin_subpaths.is_empty() {
        format!("{}${{workspaceFolder}}\\.dmz\\toolchains\\node\\bin;${{workspaceFolder}}\\.dmz\\toolchains\\rust\\bin;${{workspaceFolder}}\\.dmz\\toolchains\\python;${{env:PATH}}", win_node_prefix)
    } else {
        let win_subpaths: Vec<String> = bin_subpaths.iter().map(|p| p.replace('/', "\\")).collect();
        format!(
            "{}{};${{env:PATH}}",
            win_node_prefix,
            win_subpaths.join(";")
        )
    };

    let code_workspace_obj = serde_json::json!({
        "folders": folders,
        "settings": {
            "rust-analyzer.linkedProjects": linked_rust_projects,
            "terminal.integrated.env.osx": {
                "PATH": unix_path
            },
            "terminal.integrated.env.linux": {
                "PATH": unix_path
            },
            "terminal.integrated.env.windows": {
                "PATH": win_path
            }
        }
    });

    let cw_path = workspace_root.join(format!("{}.code-workspace", project_name));
    let content = serde_json::to_string_pretty(&code_workspace_obj)
        .map_err(|e| format!("Failed to serialize code-workspace: {}", e))?;
    fs::write(&cw_path, content)
        .map_err(|e| format!("Failed to write {}: {}", cw_path.display(), e))?;

    Ok(())
}
