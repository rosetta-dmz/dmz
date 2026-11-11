#[cfg(windows)]
use super::backup::copy_dir_contents;
use crate::workspace::{MemberRole, WorkspaceProjectConfig};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Computes the relative POSIX-style path string to navigate from `from` to `to`.
pub fn relative_path_between(from: &Path, to: &Path) -> String {
    let from_canon = fs::canonicalize(from).unwrap_or_else(|_| from.to_path_buf());
    let to_canon = fs::canonicalize(to).unwrap_or_else(|_| to.to_path_buf());
    if let Ok(rel) = to_canon.strip_prefix(&from_canon) {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        return if rel_str.is_empty() {
            ".".to_string()
        } else {
            rel_str
        };
    }
    let mut up_steps = 0;
    let mut curr = from_canon.as_path();
    while let Some(parent) = curr.parent() {
        up_steps += 1;
        if let Ok(rel) = to_canon.strip_prefix(parent) {
            let mut prefix = "../".repeat(up_steps);
            prefix.push_str(&rel.to_string_lossy().replace('\\', "/"));
            return prefix;
        }
        curr = parent;
    }
    to.to_string_lossy().replace('\\', "/")
}

/// Automatically hydrates offline developer tooling and configuration:
/// - Node/Tauri: Ensures `node_modules/.bin/tauri` CLI shim exists for immediate invocations
/// - Cargo: Configures `.cargo/config.toml` vendored source replacement for offline builds
/// - Python: Configures scoped folder settings for virtual environments/wheels without global popups
pub fn hydrate_offline_dependencies(
    workspace_root: &Path,
    store_dir: &Path,
    workspace_project: Option<&WorkspaceProjectConfig>,
) {
    // Collect target directories (either umbrella member folders or workspace root)
    let mut target_dirs: Vec<(PathBuf, MemberRole)> = Vec::new();
    if let Some(wp) = workspace_project {
        for member in &wp.members {
            let member_path = workspace_root.join(&member.path);
            if member_path.is_dir() {
                target_dirs.push((member_path, member.role));
            }
        }
    }
    if target_dirs.is_empty() {
        target_dirs.push((workspace_root.to_path_buf(), MemberRole::Primary));
    }

    let effective_target_store = if store_dir.is_relative() {
        workspace_root.join(store_dir)
    } else {
        store_dir.to_path_buf()
    };

    for (dir, _role) in &target_dirs {
        // 0. Member workspace parity symlinks (.dmz, dmz.lock, dmz-workspace.toml)
        if dir != workspace_root {
            let member_dmz = dir.join(".dmz");
            let root_dmz = workspace_root.join(".dmz");
            if !member_dmz.exists() {
                let rel_dmz = relative_path_between(dir, &root_dmz);
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(&rel_dmz, &member_dmz);
                #[cfg(windows)]
                {
                    let _ = fs::create_dir_all(&root_dmz);
                    let rel_dmz_win = rel_dmz.replace('/', "\\");
                    let linked = std::os::windows::fs::symlink_dir(&rel_dmz_win, &member_dmz)
                        .or_else(|_| std::os::windows::fs::symlink_dir(&root_dmz, &member_dmz))
                        .is_ok();
                    let junctioned = if !linked {
                        let member_str = member_dmz.to_string_lossy().replace('/', "\\");
                        let root_str = root_dmz.to_string_lossy().replace('/', "\\");
                        let clean_member = member_str.trim_start_matches(r"\\?\");
                        let clean_root = root_str.trim_start_matches(r"\\?\");
                        std::process::Command::new("cmd")
                            .args(["/C", "mklink", "/J", clean_member, clean_root])
                            .output()
                            .map(|o| o.status.success() && member_dmz.exists())
                            .unwrap_or(false)
                    } else {
                        true
                    };
                    if !junctioned && !member_dmz.exists() {
                        let _ = fs::create_dir_all(&member_dmz);
                        let _ = copy_dir_contents(&root_dmz, &member_dmz, false);
                    }
                }
            }

            let member_lock = dir.join("dmz.lock");
            let root_lock = workspace_root.join("dmz.lock");
            if !member_lock.exists() && root_lock.exists() {
                let rel_lock = relative_path_between(dir, &root_lock);
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(&rel_lock, &member_lock);
                #[cfg(windows)]
                {
                    let rel_lock_win = rel_lock.replace('/', "\\");
                    if std::os::windows::fs::symlink_file(&rel_lock_win, &member_lock).is_err()
                        && std::os::windows::fs::symlink_file(&root_lock, &member_lock).is_err()
                    {
                        let _ = std::fs::hard_link(&root_lock, &member_lock)
                            .or_else(|_| std::fs::copy(&root_lock, &member_lock).map(|_| ()));
                    }
                }
            }

            let member_manifest = dir.join("dmz-workspace.toml");
            let root_manifest = workspace_root.join("dmz-workspace.toml");
            if !member_manifest.exists() && root_manifest.exists() {
                let rel_manifest = relative_path_between(dir, &root_manifest);
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(&rel_manifest, &member_manifest);
                #[cfg(windows)]
                {
                    let rel_manifest_win = rel_manifest.replace('/', "\\");
                    if std::os::windows::fs::symlink_file(&rel_manifest_win, &member_manifest)
                        .is_err()
                        && std::os::windows::fs::symlink_file(&root_manifest, &member_manifest)
                            .is_err()
                    {
                        let _ =
                            std::fs::hard_link(&root_manifest, &member_manifest).or_else(|_| {
                                std::fs::copy(&root_manifest, &member_manifest).map(|_| ())
                            });
                    }
                }
            }
        }

        // 1. Cargo vendoring configuration: .cargo/config.toml
        // Per Cargo's config-relative-paths specification, `directory` in `.cargo/config.toml`
        // is resolved relative to the parent directory of `.cargo`.
        let cargo_toml = dir.join("Cargo.toml");
        let tauri_cargo_toml = dir.join("src-tauri/Cargo.toml");
        let member_vendor = dir.join(".cargo/vendor");
        let root_vendor = workspace_root.join(".cargo/vendor");

        let (vendor_dir_root, vendor_dir_tauri, force_overwrite) = if member_vendor.is_dir() {
            (
                ".cargo/vendor".to_string(),
                "../.cargo/vendor".to_string(),
                true,
            )
        } else if root_vendor.is_dir() {
            (
                relative_path_between(dir, &root_vendor),
                relative_path_between(&dir.join("src-tauri"), &root_vendor),
                true,
            )
        } else {
            (
                relative_path_between(dir, &effective_target_store),
                relative_path_between(&dir.join("src-tauri"), &effective_target_store),
                false,
            )
        };

        let cargo_targets = [
            (cargo_toml.is_file(), dir.join(".cargo"), vendor_dir_root),
            (
                tauri_cargo_toml.is_file(),
                dir.join("src-tauri/.cargo"),
                vendor_dir_tauri,
            ),
        ];

        for (exists, cargo_dir, rel_vendor) in cargo_targets {
            if exists && fs::create_dir_all(&cargo_dir).is_ok() {
                let config_path = cargo_dir.join("config.toml");
                if force_overwrite || !config_path.exists() {
                    let cargo_config_content = format!(
                        "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"{}\"\n",
                        rel_vendor
                    );
                    let _ = fs::write(&config_path, cargo_config_content);
                    info!(
                        config = ?config_path,
                        vendor = %rel_vendor,
                        "Configured offline Cargo vendoring source replacement"
                    );
                }
            }
        }

        // 2. Generic Node / NPM CLI shim hydration: node_modules/.bin/*
        let package_json = dir.join("package.json");
        if package_json.is_file() {
            crate::install::mappers::NpmMapper::hydrate_workspace_bin_shims(dir);
        }

        // 3. Python folder-scoped configuration
        let has_python =
            dir.join("requirements.txt").is_file() || dir.join("pyproject.toml").is_file();
        let is_umbrella =
            workspace_project.is_some() || workspace_root.join("dmz-workspace.toml").exists();
        if has_python {
            let member_venv = dir.join(".venv");
            if member_venv.is_dir() {
                let py_ver = crate::install::mappers::PyPiMapper::detect_python_version(dir);
                crate::install::mappers::PyPiMapper::hydrate_venv_environment(
                    dir,
                    &member_venv,
                    &py_ver,
                );
            }

            if is_umbrella {
                let vscode_dir = dir.join(".vscode");
                let _ = fs::create_dir_all(&vscode_dir);
                let settings_file = vscode_dir.join("settings.json");
                let mut settings_map = if settings_file.exists() {
                    fs::read_to_string(&settings_file)
                        .ok()
                        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default()
                } else {
                    serde_json::Map::new()
                };

                let bundled_py = workspace_root.join(".dmz/toolchains/python");
                let py_bin_exists = member_venv.join("bin/python").exists()
                    || member_venv.join("bin/python3").exists()
                    || member_venv.join("Scripts/python.exe").exists()
                    || member_venv.join("Scripts/python.bat").exists()
                    || member_venv.join("Scripts/python.cmd").exists()
                    || member_venv.join("pyvenv.cfg").exists();

                if member_venv.exists() && py_bin_exists {
                    let py_bin = if cfg!(windows) {
                        "${workspaceFolder}/.venv/Scripts/python.exe"
                    } else {
                        "${workspaceFolder}/.venv/bin/python"
                    };
                    settings_map.insert(
                        "python.defaultInterpreterPath".into(),
                        serde_json::Value::String(py_bin.into()),
                    );
                } else if bundled_py.exists() {
                    let rel_py = relative_path_between(dir, &bundled_py);
                    let py_bin = if cfg!(windows) {
                        format!("${{workspaceFolder}}/{}/python.exe", rel_py)
                    } else {
                        format!("${{workspaceFolder}}/{}/bin/python3", rel_py)
                    };
                    settings_map.insert(
                        "python.defaultInterpreterPath".into(),
                        serde_json::Value::String(py_bin),
                    );
                }
                settings_map.insert(
                    "python.terminal.activateEnvironment".into(),
                    serde_json::Value::Bool(true),
                );
                if let Ok(content) =
                    serde_json::to_string_pretty(&serde_json::Value::Object(settings_map))
                {
                    let _ = fs::write(&settings_file, content);
                    info!(
                        settings = ?settings_file,
                        "Wrote scoped Python IDE settings for workspace member"
                    );
                }
            }
        }
    }
}
