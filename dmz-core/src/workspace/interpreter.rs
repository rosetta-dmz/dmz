// Python interpreter path resolution for generated IDE settings.
//
// `python.defaultInterpreterPath` is only a hint: VS Code reports an error and does not fall back
// when it points at a file that does not exist. Settings must therefore only ever reference an
// interpreter that actually exists on disk (or that is guaranteed to be materialized).

use std::fs;
use std::path::Path;
use tracing::{info, warn};

const SETTING_KEY: &str = "python.defaultInterpreterPath";

/// Returns the interpreter sub-path (relative to a virtualenv directory) that exists on disk.
pub fn venv_interpreter_rel(venv_dir: &Path) -> Option<&'static str> {
    let candidates: &[&str] = if cfg!(windows) {
        &["Scripts/python.exe"]
    } else {
        &["bin/python", "bin/python3"]
    };
    candidates
        .iter()
        .copied()
        .find(|rel| venv_dir.join(rel).is_file())
}

/// Returns the interpreter sub-path (relative to `.dmz/toolchains/python`) that exists on disk.
pub fn bundled_interpreter_rel(toolchain_python_dir: &Path) -> Option<&'static str> {
    let candidates: &[&str] = if cfg!(windows) {
        &["python.exe"]
    } else {
        &["bin/python3", "bin/python"]
    };
    candidates
        .iter()
        .copied()
        .find(|rel| toolchain_python_dir.join(rel).is_file())
}

/// Re-validates every `python.defaultInterpreterPath` stored in the folder entries of the
/// `*.code-workspace` files at `workspace_root`.
///
/// Exported bundles embed a `.code-workspace` generated on the source machine, whose referenced
/// `.venv` is never shipped. After import, any interpreter that does not resolve is replaced by the
/// bundled toolchain interpreter when present, or removed so the Python extension auto-discovers.
pub fn sanitize_code_workspace_interpreters(workspace_root: &Path) {
    let Ok(entries) = fs::read_dir(workspace_root) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.path();
        if !file.is_file() || file.extension().is_none_or(|e| e != "code-workspace") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        if sanitize_workspace_doc(&mut doc, workspace_root) {
            match serde_json::to_string_pretty(&doc)
                .map_err(std::io::Error::other)
                .and_then(|out| super::atomic_write_file(&file, out))
            {
                Ok(()) => {
                    info!(file = ?file, "Corrected unresolved Python interpreter path in .code-workspace")
                }
                Err(e) => {
                    warn!(file = ?file, error = %e, "Failed to update .code-workspace interpreter path")
                }
            }
        }
    }
}

/// Re-validates `python.defaultInterpreterPath` and strips deprecated/dangling
/// `python.pythonPath` inside `<dir>/.vscode/settings.json` so VS Code never errors on stale
/// interpreter paths carried over from an exported workspace.
pub fn sanitize_vscode_settings_interpreters(dir: &Path, workspace_root: &Path) {
    let settings_file = dir.join(".vscode").join("settings.json");
    let Ok(content) = fs::read_to_string(&settings_file) else {
        return;
    };
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(obj) = doc.as_object_mut() else {
        return;
    };

    let mut changed = obj.remove("python.pythonPath").is_some();

    if let Some(current) = obj
        .get(SETTING_KEY)
        .and_then(|v| v.as_str())
        .map(String::from)
    {
        let expanded = expand_folder_vars(&current, dir, None);
        let expanded_path = Path::new(&expanded);
        let exists = if expanded_path.is_absolute() {
            expanded_path.is_file()
        } else {
            dir.join(expanded_path).is_file()
        };

        if !exists {
            let member_venv = dir.join(".venv");
            let bundled_dir = workspace_root.join(".dmz/toolchains/python");
            if let Some(rel) = venv_interpreter_rel(&member_venv) {
                obj.insert(
                    SETTING_KEY.to_string(),
                    serde_json::Value::String(format!("${{workspaceFolder}}/.venv/{}", rel)),
                );
            } else if let Some(rel) = bundled_interpreter_rel(&bundled_dir) {
                let rel_member = dir
                    .strip_prefix(workspace_root)
                    .unwrap_or_else(|_| Path::new("."));
                let prefix = relative_root_prefix(rel_member);
                let target = if prefix == "." {
                    format!("${{workspaceFolder}}/.dmz/toolchains/python/{}", rel)
                } else {
                    format!(
                        "${{workspaceFolder}}/{}/.dmz/toolchains/python/{}",
                        prefix, rel
                    )
                };
                obj.insert(SETTING_KEY.to_string(), serde_json::Value::String(target));
            } else {
                obj.remove(SETTING_KEY);
            }
            changed = true;
        }
    }

    if changed {
        match serde_json::to_string_pretty(&doc)
            .map_err(std::io::Error::other)
            .and_then(|out| super::atomic_write_file(&settings_file, out))
        {
            Ok(()) => {
                info!(file = ?settings_file, "Sanitized unresolved Python interpreter path in .vscode/settings.json")
            }
            Err(e) => {
                warn!(file = ?settings_file, error = %e, "Failed to update .vscode/settings.json interpreter path")
            }
        }
    }
}

/// True for relative paths without `..`, root or prefix components (CWE-22).
fn is_contained_relative_path(path: &Path) -> bool {
    path.components().all(|c| {
        matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    })
}

/// Returns the VS Code folder variable (`${workspaceFolder:<label>}` for multi-root
/// `.code-workspace` entries, or `${workspaceFolder}` when no label is given).
pub fn workspace_folder_var(folder_label: Option<&str>) -> String {
    match folder_label.map(str::trim).filter(|s| !s.is_empty()) {
        Some(label) => format!("${{workspaceFolder:{}}}", label),
        None => "${workspaceFolder}".to_string(),
    }
}

/// Computes the relative prefix from a member path (`workspaces/foo` -> `../..`, `.` -> `.`).
pub fn relative_root_prefix(member_path: &Path) -> String {
    let depth = member_path
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .count();
    if depth == 0 {
        ".".to_string()
    } else {
        vec![".."; depth].join("/")
    }
}

/// Expands `${workspaceFolder}` and `${workspaceFolder:<label>}` prefixes against `folder_dir`.
fn expand_folder_vars(value: &str, folder_dir: &Path, folder_label: Option<&str>) -> String {
    let dir_str = folder_dir.to_string_lossy();
    let expanded = match folder_label.map(str::trim).filter(|s| !s.is_empty()) {
        Some(label) => value.replace(&format!("${{workspaceFolder:{}}}", label), &dir_str),
        None => value.to_string(),
    };
    expanded.replace("${workspaceFolder}", &dir_str)
}

/// Resolves the `python.defaultInterpreterPath` string to write for a workspace folder, checking
/// the member's `.venv` first and then the bundled Python toolchain at `workspace_root`.
///
/// When `assume_bundled` is true (e.g. during export with `--with-toolchain` before the target
/// `.dmz/toolchains/python` tree is materialized), emits the bundled path even if it does not yet
/// exist on the source workspace; import-time sanitization re-validates it on the target.
pub fn resolve_ide_interpreter_setting(
    member_dir: &Path,
    member_rel_path: &Path,
    workspace_root: &Path,
    folder_label: Option<&str>,
    assume_bundled: bool,
) -> Option<String> {
    let folder_var = workspace_folder_var(folder_label);
    if let Some(rel) = venv_interpreter_rel(&member_dir.join(".venv")) {
        return Some(format!("{}/.venv/{}", folder_var, rel));
    }

    let bundled_dir = workspace_root.join(".dmz/toolchains/python");
    let bundled_rel =
        bundled_interpreter_rel(&bundled_dir).or(assume_bundled.then_some(if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3"
        }))?;

    let prefix = relative_root_prefix(member_rel_path);
    if prefix == "." {
        Some(format!(
            "{}/.dmz/toolchains/python/{}",
            folder_var, bundled_rel
        ))
    } else {
        Some(format!(
            "{}/{}/.dmz/toolchains/python/{}",
            folder_var, prefix, bundled_rel
        ))
    }
}

/// Returns true when `doc` was modified.
fn sanitize_workspace_doc(doc: &mut serde_json::Value, workspace_root: &Path) -> bool {
    let bundled_dir = workspace_root.join(".dmz/toolchains/python");
    let bundled_rel = bundled_interpreter_rel(&bundled_dir);
    let mut changed = false;

    let Some(folders) = doc.get_mut("folders").and_then(|f| f.as_array_mut()) else {
        return false;
    };
    for folder in folders {
        let Some(folder_path) = folder
            .get("path")
            .and_then(|p| p.as_str())
            .map(String::from)
        else {
            continue;
        };
        let folder_label = folder
            .get("name")
            .and_then(|n| n.as_str())
            .map(String::from);
        // The archive is untrusted input: only follow folder entries that stay inside the root.
        if !is_contained_relative_path(Path::new(&folder_path)) {
            continue;
        }
        let folder_dir = workspace_root.join(&folder_path);
        let Some(settings) = folder.get_mut("settings").and_then(|s| s.as_object_mut()) else {
            continue;
        };
        let Some(current) = settings.get(SETTING_KEY).and_then(|v| v.as_str()) else {
            continue;
        };
        let resolved = expand_folder_vars(current, &folder_dir, folder_label.as_deref());
        if Path::new(&resolved).is_file() {
            continue;
        }

        let prefix = relative_root_prefix(Path::new(&folder_path));
        let folder_var = workspace_folder_var(folder_label.as_deref());
        match bundled_rel {
            Some(rel) => {
                let target = if prefix == "." {
                    format!("{}/.dmz/toolchains/python/{}", folder_var, rel)
                } else {
                    format!("{}/{}/.dmz/toolchains/python/{}", folder_var, prefix, rel)
                };
                settings.insert(SETTING_KEY.to_string(), serde_json::Value::String(target));
            }
            None => {
                settings.remove(SETTING_KEY);
            }
        }
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn workspace_doc(py_path: &str) -> serde_json::Value {
        serde_json::json!({
            "folders": [
                {"name": "py", "path": "workspaces/py", "settings": {
                    "python.terminal.activateEnvironment": true,
                    SETTING_KEY: py_path
                }},
                {"name": "ui", "path": "workspaces/ui"}
            ]
        })
    }

    #[test]
    fn test_missing_venv_interpreter_is_removed_without_bundled_python() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("workspaces/py")).unwrap();
        let mut doc = workspace_doc("${workspaceFolder}/.venv/bin/python");
        assert!(sanitize_workspace_doc(&mut doc, tmp.path()));
        let settings = &doc["folders"][0]["settings"];
        assert!(settings.get(SETTING_KEY).is_none());
        assert_eq!(settings["python.terminal.activateEnvironment"], true);
    }

    #[test]
    fn test_missing_venv_interpreter_falls_back_to_bundled_python() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("workspaces/py")).unwrap();
        let py_dir = tmp.path().join(".dmz/toolchains/python");
        let rel = if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3"
        };
        fs::create_dir_all(py_dir.join(rel).parent().unwrap()).unwrap();
        fs::write(py_dir.join(rel), "").unwrap();

        let mut doc = workspace_doc("${workspaceFolder:py}/.venv/bin/python");
        assert!(sanitize_workspace_doc(&mut doc, tmp.path()));
        assert_eq!(
            doc["folders"][0]["settings"][SETTING_KEY],
            format!(
                "${{workspaceFolder:py}}/../../.dmz/toolchains/python/{}",
                rel
            )
        );
    }

    #[test]
    fn test_existing_named_venv_interpreter_is_kept() {
        let tmp = tempdir().unwrap();
        let venv_py = tmp.path().join("workspaces/py/.venv/bin/python");
        fs::create_dir_all(venv_py.parent().unwrap()).unwrap();
        fs::write(&venv_py, "").unwrap();
        let mut doc = workspace_doc("${workspaceFolder:py}/.venv/bin/python");
        assert!(!sanitize_workspace_doc(&mut doc, tmp.path()));
        assert_eq!(
            doc["folders"][0]["settings"][SETTING_KEY],
            "${workspaceFolder:py}/.venv/bin/python"
        );
    }

    #[test]
    fn test_existing_venv_interpreter_is_kept() {
        let tmp = tempdir().unwrap();
        let venv_py = tmp.path().join("workspaces/py/.venv/bin/python");
        fs::create_dir_all(venv_py.parent().unwrap()).unwrap();
        fs::write(&venv_py, "").unwrap();
        let mut doc = workspace_doc("${workspaceFolder}/.venv/bin/python");
        // On Windows the POSIX layout would not be used, but the literal path still resolves.
        assert!(!sanitize_workspace_doc(&mut doc, tmp.path()));
        assert_eq!(
            doc["folders"][0]["settings"][SETTING_KEY],
            "${workspaceFolder}/.venv/bin/python"
        );
    }

    #[test]
    fn test_folder_entries_escaping_the_workspace_root_are_ignored() {
        let tmp = tempdir().unwrap();
        let mut doc = serde_json::json!({
            "folders": [
                {"path": "../outside", "settings": {SETTING_KEY: "${workspaceFolder}/.venv/bin/python"}},
                {"path": "/abs/outside", "settings": {SETTING_KEY: "${workspaceFolder}/.venv/bin/python"}}
            ]
        });
        assert!(!sanitize_workspace_doc(&mut doc, tmp.path()));
    }

    #[test]
    fn test_venv_interpreter_rel_requires_real_file() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert!(venv_interpreter_rel(tmp.path()).is_none());
    }

    #[test]
    fn test_sanitize_vscode_settings_interpreters_drops_stale_path_and_keeps_valid() {
        let tmp = tempdir().unwrap();
        let member = tmp.path().join("workspaces/py");
        fs::create_dir_all(member.join(".vscode")).unwrap();
        fs::write(
            member.join(".vscode/settings.json"),
            r#"{
  "python.defaultInterpreterPath": "${workspaceFolder}/.venv/bin/python",
  "python.pythonPath": "/nonexistent/host/python",
  "editor.tabSize": 4
}"#,
        )
        .unwrap();

        sanitize_vscode_settings_interpreters(&member, tmp.path());
        let after_strip: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(member.join(".vscode/settings.json")).unwrap(),
        )
        .unwrap();
        assert!(after_strip.get("python.defaultInterpreterPath").is_none());
        assert!(after_strip.get("python.pythonPath").is_none());
        assert_eq!(after_strip["editor.tabSize"], 4);

        // When a real .venv interpreter exists, preserve it.
        fs::create_dir_all(member.join(".venv/bin")).unwrap();
        fs::write(member.join(".venv/bin/python"), "").unwrap();
        fs::write(
            member.join(".vscode/settings.json"),
            r#"{"python.defaultInterpreterPath": "${workspaceFolder}/.venv/bin/python"}"#,
        )
        .unwrap();
        sanitize_vscode_settings_interpreters(&member, tmp.path());
        let after_keep: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(member.join(".vscode/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            after_keep["python.defaultInterpreterPath"],
            "${workspaceFolder}/.venv/bin/python"
        );
    }
}
