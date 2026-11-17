// IDE workspace settings and multi-root workspace configuration

use super::super::ToolchainError;
use std::fs;
use std::path::Path;

/// Configures `.vscode/settings.json` so IDE extensions work out-of-the-box in air-gapped environments.
/// In non-CMake workspaces, silences CMake Tools extension auto-probing to eliminate "Bad CMake executable: ''" popups.
/// In CMake workspaces with bundled CMake, automatically configures cmake.cmakePath and toolchain file.
/// Pre-configures terminal PATH and language server environments (such as rust-analyzer).
pub fn generate_ide_settings(workspace_root: &Path) -> Result<(), ToolchainError> {
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

    // Configure terminal environment for virtual environments and bundled toolchains (e.g., Rust, Go, Python)
    if toolchains_dir.exists() || workspace_root.join(".venv").is_dir() {
        let mut unix_subpaths: Vec<String> = Vec::new();
        let mut win_subpaths: Vec<String> = Vec::new();
        if workspace_root.join(".venv/bin").is_dir() {
            unix_subpaths.push("${workspaceFolder}/.venv/bin".into());
        }
        if workspace_root.join(".venv/Scripts").is_dir() {
            win_subpaths.push("${workspaceFolder}\\.venv\\Scripts".into());
        }
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
                    unix_subpaths
                        .push(format!("${{workspaceFolder}}/.dmz/toolchains/{}/bin", name));
                    win_subpaths.push(format!(
                        "${{workspaceFolder}}\\.dmz\\toolchains\\{}\\bin",
                        name
                    ));
                }
            }
        }

        if !unix_subpaths.is_empty() || !win_subpaths.is_empty() {
            let unix_path = format!("{}:${{env:PATH}}", unix_subpaths.join(":"));
            let win_path = format!("{};${{env:PATH}}", win_subpaths.join(";"));

            for platform in &[
                "terminal.integrated.env.osx",
                "terminal.integrated.env.linux",
            ] {
                if !unix_subpaths.is_empty() {
                    let mut env_obj = settings
                        .get(*platform)
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    env_obj.insert("PATH".into(), serde_json::Value::String(unix_path.clone()));
                    settings.insert(platform.to_string(), serde_json::Value::Object(env_obj));
                }
            }

            if !win_subpaths.is_empty() {
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
                if let Some(sdk_path) = crate::toolchain::preflight::resolve_host_macos_sdk_path() {
                    ra_extra_env.insert(
                        "SDKROOT".into(),
                        serde_json::Value::String(sdk_path.to_string_lossy().to_string()),
                    );
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
        let venv_rel = crate::workspace::interpreter::venv_interpreter_rel(&member_venv);
        let bundled_rel =
            crate::workspace::interpreter::bundled_interpreter_rel(&toolchains_dir.join("python"));

        if !is_umbrella && !is_umbrella_member {
            if let Some(rel) = venv_rel {
                settings.insert(
                    "python.defaultInterpreterPath".into(),
                    serde_json::Value::String(format!("${{workspaceFolder}}/.venv/{}", rel)),
                );
                settings.insert(
                    "python.terminal.activateEnvironment".into(),
                    serde_json::Value::Bool(true),
                );
            } else if let Some(rel) = bundled_rel {
                settings.insert(
                    "python.defaultInterpreterPath".into(),
                    serde_json::Value::String(format!(
                        "${{workspaceFolder}}/.dmz/toolchains/python/{}",
                        rel
                    )),
                );
            } else {
                settings.remove("python.defaultInterpreterPath");
                settings.remove("python.pythonPath");
            }
        } else {
            // Umbrella roots and multi-root members must never set an unlabeled
            // `${workspaceFolder}/...` `python.defaultInterpreterPath` in folder-level
            // `.vscode/settings.json`; multi-root `.code-workspace` configures each
            // member via `${workspaceFolder:<label>}/.venv/...`.
            settings.remove("python.defaultInterpreterPath");
            settings.remove("python.pythonPath");
            if is_umbrella_member && (has_python_project || venv_rel.is_some()) {
                settings.insert(
                    "python.terminal.activateEnvironment".into(),
                    serde_json::Value::Bool(true),
                );
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
        .map_err(|e| ToolchainError::Io(format!("Failed to create .vscode directory: {}", e)))?;

    let json_string =
        serde_json::to_string_pretty(&serde_json::Value::Object(settings)).map_err(|e| {
            ToolchainError::SetupFailed(format!("Failed to serialize settings.json: {}", e))
        })?;

    fs::write(&settings_path, json_string)
        .map_err(|e| ToolchainError::Io(format!("Failed to write .vscode/settings.json: {}", e)))?;

    // If this workspace is a true umbrella multi-workspace root (not a symlinked member), generate the .code-workspace file
    let is_true_umbrella = fs::symlink_metadata(workspace_root.join("dmz-workspace.toml"))
        .map(|m| m.is_file() && !m.file_type().is_symlink())
        .unwrap_or(false);
    if is_true_umbrella {
        let _ = generate_multi_root_workspace(workspace_root);
    }

    Ok(())
}

/// Input descriptor for one member folder included in a generated `.code-workspace` file.
pub(crate) struct WorkspaceFolderInput<'a> {
    pub label: &'a str,
    pub folder_path: &'a str,
    pub rel_path: &'a Path,
    pub fs_dir: &'a Path,
    pub ecosystem: Option<&'a str>,
}

fn is_ecosystem_member(ecosystem: Option<&str>, candidates: &[&str]) -> bool {
    ecosystem.is_some_and(|e| candidates.contains(&e))
}

fn toolchain_root_var(first_member: Option<&WorkspaceFolderInput<'_>>) -> String {
    match first_member {
        Some(m) => {
            let folder_var = crate::workspace::interpreter::workspace_folder_var(Some(m.label));
            let prefix = crate::workspace::interpreter::relative_root_prefix(m.rel_path);
            if prefix == "." {
                folder_var
            } else {
                format!("{}/{}", folder_var, prefix)
            }
        }
        None => "${workspaceFolder}".to_string(),
    }
}

/// Builds a multi-root VS Code / Cursor `.code-workspace` document shared between air-gap
/// bundle export and post-import workspace setup.
///
/// Uses unambiguous `${workspaceFolder:<label>}` variable scoping for member interpreters,
/// `node_modules/.bin`, and umbrella `.dmz/toolchains/*/bin` paths.
pub(crate) fn build_multi_root_workspace_doc(
    workspace_root: &Path,
    members: &[WorkspaceFolderInput<'_>],
    assume_bundled_python: bool,
    include_documentation: bool,
) -> serde_json::Value {
    let mut folders = Vec::new();
    let mut linked_rust_projects = Vec::new();
    let mut node_bin_unix = Vec::new();
    let mut node_bin_win = Vec::new();

    for m in members {
        let is_python = is_ecosystem_member(m.ecosystem, &["python", "pypi"])
            || m.fs_dir.join("requirements.txt").exists()
            || m.fs_dir.join("pyproject.toml").exists();

        let cargo_dirs = crate::workspace::discover_tauri_cargo_dirs(m.fs_dir);
        let node_dirs = crate::workspace::discover_node_project_roots(m.fs_dir);

        let is_rust =
            is_ecosystem_member(m.ecosystem, &["rust", "cargo"]) || !cargo_dirs.is_empty();

        let is_node = is_ecosystem_member(m.ecosystem, &["node", "npm"]) || !node_dirs.is_empty();

        if is_rust {
            for cargo_dir in cargo_dirs {
                if let Ok(rel) = cargo_dir.strip_prefix(m.fs_dir) {
                    let rel_unix = rel.to_string_lossy().replace('\\', "/");
                    if rel_unix.is_empty() || rel_unix == "." {
                        linked_rust_projects.push(format!("{}/Cargo.toml", m.folder_path));
                    } else {
                        linked_rust_projects
                            .push(format!("{}/{}/Cargo.toml", m.folder_path, rel_unix));
                    }
                }
            }
        }

        let folder_var = crate::workspace::interpreter::workspace_folder_var(Some(m.label));
        if is_python && (m.fs_dir.join(".venv").is_dir() || assume_bundled_python) {
            node_bin_unix.push(format!("{}/.venv/bin", folder_var));
            node_bin_win.push(format!("{}\\.venv\\Scripts", folder_var));
        }
        if is_node {
            node_bin_unix.push(format!("{}/node_modules/.bin", folder_var));
            node_bin_win.push(format!("{}\\node_modules\\.bin", folder_var));
            for node_dir in node_dirs {
                if let Ok(rel) = node_dir.strip_prefix(m.fs_dir) {
                    let rel_unix = rel.to_string_lossy().replace('\\', "/");
                    let rel_win = rel_unix.replace('/', "\\");
                    if !rel_unix.is_empty() && rel_unix != "." {
                        node_bin_unix
                            .push(format!("{}/{}/node_modules/.bin", folder_var, rel_unix));
                        node_bin_win
                            .push(format!("{}\\{}\\node_modules\\.bin", folder_var, rel_win));
                    }
                }
                for sub_dir in
                    crate::resolver::manifest::npm::NpmManifestScanner::discover_npm_workspace_package_dirs(
                        &node_dir,
                    )
                {
                    if let Ok(rel) = sub_dir.strip_prefix(m.fs_dir) {
                        let rel_unix = rel.to_string_lossy().replace('\\', "/");
                        let rel_win = rel_unix.replace('/', "\\");
                        if !rel_unix.is_empty() && rel_unix != "." {
                            node_bin_unix
                                .push(format!("{}/{}/node_modules/.bin", folder_var, rel_unix));
                            node_bin_win
                                .push(format!("{}\\{}\\node_modules\\.bin", folder_var, rel_win));
                        }
                    }
                }
            }
        }

        let mut folder_entry = serde_json::json!({
            "name": m.label,
            "path": m.folder_path
        });

        if is_python {
            let mut py_settings = serde_json::json!({
                "python.terminal.activateEnvironment": true
            });

            // Scope `.code-workspace` paths via `${workspaceFolder:<label>}` so VS Code's Python
            // extension never hits multi-root `${workspaceFolder}` ambiguity, and only emit a path
            // when a real interpreter exists (or is guaranteed to be bundled with `--with-toolchain`).
            if let Some(py_bin) = crate::workspace::interpreter::resolve_ide_interpreter_setting(
                m.fs_dir,
                m.rel_path,
                workspace_root,
                Some(m.label),
                assume_bundled_python,
            ) {
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

    if include_documentation && workspace_root.join("documentation").is_dir() {
        folders.push(serde_json::json!({
            "name": "Documentation",
            "path": "documentation"
        }));
    }

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

    let root_var = toolchain_root_var(members.first());
    let toolchains_dir = workspace_root.join(".dmz/toolchains");
    let mut bin_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Ok(entries) = fs::read_dir(&toolchains_dir) {
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            if name_os == "applesdk"
                && !crate::toolchain::ToolchainEnv::is_genuine_apple_developer_dir(&entry.path())
            {
                continue;
            }
            if entry.path().join("bin").exists() {
                bin_names.insert(name_os.to_string_lossy().to_string());
            }
        }
    }

    let bin_subpaths: Vec<String> = bin_names
        .into_iter()
        .map(|name| format!("{}/.dmz/toolchains/{}/bin", root_var, name))
        .collect();

    let (unix_path, win_path) = if bin_subpaths.is_empty() {
        let win_root = root_var.replace('/', "\\");
        (
            format!(
                "{}{}/.dmz/toolchains/node/bin:{}/.dmz/toolchains/rust/bin:{}/.dmz/toolchains/python/bin:${{env:PATH}}",
                unix_node_prefix, root_var, root_var, root_var
            ),
            format!(
                "{}{}\\.dmz\\toolchains\\node\\bin;{}\\.dmz\\toolchains\\rust\\bin;{}\\.dmz\\toolchains\\python;${{env:PATH}}",
                win_node_prefix, win_root, win_root, win_root
            ),
        )
    } else {
        let win_subpaths: Vec<String> = bin_subpaths.iter().map(|p| p.replace('/', "\\")).collect();
        (
            format!(
                "{}{}:${{env:PATH}}",
                unix_node_prefix,
                bin_subpaths.join(":")
            ),
            format!(
                "{}{};${{env:PATH}}",
                win_node_prefix,
                win_subpaths.join(";")
            ),
        )
    };

    let has_cmake = workspace_root.join("CMakeLists.txt").exists()
        || members.iter().any(|m| {
            m.fs_dir.join("CMakeLists.txt").exists()
                || is_ecosystem_member(m.ecosystem, &["cmake", "juce"])
        })
        || crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("cmake"), "bin/cmake")
            .is_some();

    let mut workspace_settings = serde_json::json!({
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
    });

    if !has_cmake {
        if let Some(obj) = workspace_settings.as_object_mut() {
            obj.insert(
                "cmake.configureOnOpen".to_string(),
                serde_json::Value::Bool(false),
            );
            obj.insert(
                "cmake.autoSelectActiveFolder".to_string(),
                serde_json::Value::Bool(false),
            );
            obj.insert(
                "cmake.enableAutomaticKitScan".to_string(),
                serde_json::Value::Bool(false),
            );
        }
    }

    serde_json::json!({
        "folders": folders,
        "settings": workspace_settings
    })
}

/// Generates or updates `<project>.code-workspace` for multi-root projects
pub fn generate_multi_root_workspace(workspace_root: &Path) -> Result<(), ToolchainError> {
    let manifest_path = workspace_root.join("dmz-workspace.toml");
    if !manifest_path.exists() {
        return Ok(());
    }

    let ws_project = match crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
    {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };

    let member_storage: Vec<(String, String, &Path, std::path::PathBuf, Option<&str>)> = ws_project
        .members
        .iter()
        .map(|m| {
            let folder_path = m.path.to_string_lossy().replace('\\', "/");
            let label = if !m.id.is_empty() {
                m.id.clone()
            } else {
                folder_path.clone()
            };
            (
                label,
                folder_path,
                m.path.as_path(),
                workspace_root.join(&m.path),
                m.ecosystem.as_deref(),
            )
        })
        .collect();

    let inputs: Vec<WorkspaceFolderInput<'_>> = member_storage
        .iter()
        .map(
            |(label, folder_path, rel_path, fs_dir, ecosystem)| WorkspaceFolderInput {
                label: label.as_str(),
                folder_path: folder_path.as_str(),
                rel_path,
                fs_dir: fs_dir.as_path(),
                ecosystem: *ecosystem,
            },
        )
        .collect();

    let code_workspace_obj = build_multi_root_workspace_doc(workspace_root, &inputs, false, true);

    let project_name = if !ws_project.name.is_empty() {
        &ws_project.name
    } else {
        "workspace"
    };

    let cw_path = workspace_root.join(format!("{}.code-workspace", project_name));
    let content = serde_json::to_string_pretty(&code_workspace_obj).map_err(|e| {
        ToolchainError::SetupFailed(format!("Failed to serialize code-workspace: {}", e))
    })?;
    fs::write(&cw_path, content)
        .map_err(|e| ToolchainError::Io(format!("Failed to write {}: {}", cw_path.display(), e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_generate_multi_root_workspace_uses_folder_label_scoping() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("workspaces/py-api")).unwrap();
        fs::create_dir_all(root.join("workspaces/tauri-ui")).unwrap();
        fs::write(
            root.join("workspaces/py-api/requirements.txt"),
            "fastapi==0.111.0\n",
        )
        .unwrap();
        fs::write(
            root.join("workspaces/tauri-ui/package.json"),
            r#"{"name": "tauri-ui", "devDependencies": {"@tauri-apps/cli": "^2.0.0"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("dmz-workspace.toml"),
            r#"
name = "dual-suite"

[[members]]
id = "py-api"
path = "workspaces/py-api"
role = "primary"
ecosystem = "python"

[[members]]
id = "tauri-ui"
path = "workspaces/tauri-ui"
role = "sidecar"
ecosystem = "node"
"#,
        )
        .unwrap();

        generate_multi_root_workspace(root).unwrap();
        let cw_text = fs::read_to_string(root.join("dual-suite.code-workspace")).unwrap();
        let cw: serde_json::Value = serde_json::from_str(&cw_text).unwrap();

        let osx_path = cw["settings"]["terminal.integrated.env.osx"]["PATH"]
            .as_str()
            .unwrap();
        assert!(
            osx_path.contains("${workspaceFolder:tauri-ui}/node_modules/.bin"),
            "Expected labeled node_modules/.bin in PATH, got: {}",
            osx_path
        );
        assert!(
            osx_path.contains("${workspaceFolder:py-api}/../../.dmz/toolchains/"),
            "Expected labeled relative toolchains in PATH, got: {}",
            osx_path
        );
        assert!(
            !osx_path.contains("${workspaceFolder}/"),
            "Ambiguous unlabeled ${{workspaceFolder}} should not appear in multi-root PATH: {}",
            osx_path
        );
        assert_eq!(cw["settings"]["cmake.configureOnOpen"], false);
        assert_eq!(cw["settings"]["cmake.autoSelectActiveFolder"], false);
        assert_eq!(cw["settings"]["cmake.enableAutomaticKitScan"], false);
    }

    #[test]
    fn test_generate_multi_root_workspace_preserves_cmake_when_member_uses_cmake() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("workspaces/native-lib")).unwrap();
        fs::create_dir_all(root.join("workspaces/ui")).unwrap();
        fs::write(
            root.join("workspaces/native-lib/CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nproject(native_lib)\n",
        )
        .unwrap();
        fs::write(root.join("workspaces/ui/package.json"), r#"{"name":"ui"}"#).unwrap();
        fs::write(
            root.join("dmz-workspace.toml"),
            r#"
name = "native-suite"

[[members]]
id = "native-lib"
path = "workspaces/native-lib"
role = "primary"

[[members]]
id = "ui"
path = "workspaces/ui"
role = "sidecar"
"#,
        )
        .unwrap();

        generate_multi_root_workspace(root).unwrap();
        let cw_text = fs::read_to_string(root.join("native-suite.code-workspace")).unwrap();
        let cw: serde_json::Value = serde_json::from_str(&cw_text).unwrap();

        assert!(cw["settings"].get("cmake.configureOnOpen").is_none());
        assert!(cw["settings"].get("cmake.autoSelectActiveFolder").is_none());
        assert!(cw["settings"].get("cmake.enableAutomaticKitScan").is_none());
    }
}
