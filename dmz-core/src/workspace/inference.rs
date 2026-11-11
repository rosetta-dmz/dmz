// Command and ecosystem detection algorithms for workspace members.
// Inspects package manifests, entrypoints, and scripts across Node, Rust, Python, and Go.

use super::schema::WorkspaceMember;
use std::fs;
use std::path::{Path, PathBuf};

/// Detects the default command, arguments, and ecosystem for a workspace member directory.
pub fn detect_default_command(
    member_dir: &Path,
    workspace_root: &Path,
) -> Option<(String, Vec<String>, &'static str)> {
    detect_node_command(member_dir, workspace_root)
        .or_else(|| detect_rust_command(member_dir))
        .or_else(|| detect_python_command(member_dir))
        .or_else(|| detect_go_command(member_dir))
}

pub fn detect_node_command(
    member_dir: &Path,
    workspace_root: &Path,
) -> Option<(String, Vec<String>, &'static str)> {
    let pkg_json_path = member_dir.join("package.json");
    if !pkg_json_path.is_file() {
        return None;
    }

    let pm = if member_dir.join("pnpm-lock.yaml").exists()
        || workspace_root.join("pnpm-lock.yaml").exists()
        || member_dir.join("pnpm-workspace.yaml").exists()
        || member_dir.join("node_modules/.pnpm").is_dir()
    {
        "pnpm".to_string()
    } else if member_dir.join("yarn.lock").exists() || workspace_root.join("yarn.lock").exists() {
        "yarn".to_string()
    } else if member_dir.join("bun.lockb").exists() || member_dir.join("bun.lock").exists() {
        "bun".to_string()
    } else {
        "npm".to_string()
    };

    if let Ok(content) = fs::read_to_string(&pkg_json_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(sc) = val.get("scripts").and_then(|s| s.as_object()) {
                if sc.contains_key("tauri") {
                    return Some((
                        pm,
                        vec!["run".to_string(), "tauri".to_string(), "dev".to_string()],
                        "node",
                    ));
                } else if sc.contains_key("dev") {
                    return Some((pm, vec!["run".to_string(), "dev".to_string()], "node"));
                } else if sc.contains_key("start") {
                    return Some((pm, vec!["run".to_string(), "start".to_string()], "node"));
                } else if sc.contains_key("serve") {
                    return Some((pm, vec!["run".to_string(), "serve".to_string()], "node"));
                }
            }

            if let Some(main_file) = val.get("main").and_then(|m| m.as_str()) {
                if member_dir.join(main_file).is_file() {
                    return Some(("node".to_string(), vec![main_file.to_string()], "node"));
                }
            }
        }
    }

    for candidate in &[
        "index.js",
        "server.js",
        "main.js",
        "app.js",
        "src/index.js",
        "src/server.js",
    ] {
        if member_dir.join(candidate).is_file() {
            return Some(("node".to_string(), vec![candidate.to_string()], "node"));
        }
    }

    None
}

pub fn detect_rust_command(member_dir: &Path) -> Option<(String, Vec<String>, &'static str)> {
    let cargo_toml_path = member_dir.join("Cargo.toml");
    let tauri_cargo_path = member_dir.join("src-tauri/Cargo.toml");
    if tauri_cargo_path.is_file() || cargo_toml_path.is_file() {
        Some(("cargo".to_string(), vec!["run".to_string()], "rust"))
    } else {
        None
    }
}

pub fn detect_python_command(member_dir: &Path) -> Option<(String, Vec<String>, &'static str)> {
    let py_req_path = member_dir.join("requirements.txt");
    let pyproject_path = member_dir.join("pyproject.toml");
    let setup_py_path = member_dir.join("setup.py");
    let pipfile_path = member_dir.join("Pipfile");

    if !py_req_path.is_file()
        && !pyproject_path.is_file()
        && !setup_py_path.is_file()
        && !pipfile_path.is_file()
    {
        return None;
    }

    let py_cmd = if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    };

    let candidates = [
        "server.py",
        "main.py",
        "app.py",
        "backend.py",
        "run.py",
        "manage.py",
        "src/server.py",
        "src/main.py",
        "src/app.py",
    ];
    for candidate in &candidates {
        if member_dir.join(candidate).is_file() {
            return Some((py_cmd, vec![candidate.to_string()], "python"));
        }
    }

    // Check pyproject.toml [project.scripts] / [tool.poetry.scripts]
    if pyproject_path.is_file() {
        if let Ok(content) = fs::read_to_string(&pyproject_path) {
            if let Ok(parsed) = content.parse::<toml::Value>() {
                let scripts_table = parsed
                    .get("project")
                    .and_then(|p| p.get("scripts"))
                    .and_then(|s| s.as_table())
                    .or_else(|| {
                        parsed
                            .get("tool")
                            .and_then(|t| t.get("poetry"))
                            .and_then(|p| p.get("scripts"))
                            .and_then(|s| s.as_table())
                    });
                if let Some(scripts) = scripts_table {
                    for (_script_name, target_val) in scripts {
                        if let Some(target_str) = target_val.as_str() {
                            if let Some((mod_name, fn_name)) = target_str.split_once(':') {
                                let mod_name = mod_name.trim();
                                let fn_name = fn_name.trim();
                                let is_safe_ident = |s: &str| {
                                    !s.is_empty()
                                        && s.chars()
                                            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                                };
                                if is_safe_ident(mod_name) && is_safe_ident(fn_name) {
                                    return Some((
                                        py_cmd,
                                        vec![
                                            "-c".to_string(),
                                            format!(
                                                "import sys, {}; sys.exit({}.{}())",
                                                mod_name, mod_name, fn_name
                                            ),
                                        ],
                                        "python",
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Check 1-level package subdirectories for main.py / server.py / app.py
    if let Ok(entries) = fs::read_dir(member_dir) {
        let mut subdirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                        !name.starts_with('.')
                            && !matches!(
                                name,
                                "venv"
                                    | "node_modules"
                                    | "target"
                                    | "dist"
                                    | "build"
                                    | "tests"
                                    | "test"
                                    | "__pycache__"
                            )
                    })
            })
            .collect();
        subdirs.sort();
        for sub in subdirs {
            if let Some(sub_name) = sub.file_name().and_then(|n| n.to_str()) {
                for entry_script in &["main.py", "server.py", "app.py"] {
                    if sub.join(entry_script).is_file() {
                        return Some((
                            py_cmd,
                            vec![format!("{}/{}", sub_name, entry_script)],
                            "python",
                        ));
                    }
                }
            }
        }
    }

    None
}

pub fn detect_go_command(member_dir: &Path) -> Option<(String, Vec<String>, &'static str)> {
    let go_mod_path = member_dir.join("go.mod");
    if go_mod_path.is_file() {
        Some((
            "go".to_string(),
            vec!["run".to_string(), ".".to_string()],
            "go",
        ))
    } else {
        None
    }
}

pub fn resolve_command(
    member: &WorkspaceMember,
    member_dir: &Path,
    workspace_root: &Path,
) -> Result<(String, Vec<String>), String> {
    if let Some(ref cmd) = member.exec_command {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() {
            return Ok((
                trimmed.to_string(),
                member.exec_args.clone().unwrap_or_default(),
            ));
        }
    }

    if let Some((detected_cmd, detected_args, _eco)) =
        detect_default_command(member_dir, workspace_root)
    {
        let args = member.exec_args.clone().unwrap_or(detected_args);
        return Ok((detected_cmd, args));
    }

    Err(format!(
        "Unable to determine execution command for workspace member '{}' at {:?}. Specify \"exec_command\" and \"exec_args\" in dmz-workspace.toml",
        member.id, member_dir
    ))
}

pub fn populate_detected_defaults(
    member: &mut WorkspaceMember,
    member_dir: &Path,
    workspace_root: &Path,
) {
    let member_dir = fs::canonicalize(member_dir).unwrap_or_else(|_| member_dir.to_path_buf());
    if !member_dir.is_dir() {
        return;
    }

    if let Some((cmd, args, eco)) = detect_default_command(&member_dir, workspace_root) {
        if member.ecosystem.is_none() {
            member.ecosystem = Some(eco.to_string());
        }
        if member.exec_command.is_none() {
            member.exec_command = Some(cmd);
        }
        if member.exec_args.is_none() && !args.is_empty() {
            member.exec_args = Some(args);
        }
    } else if member.ecosystem.is_none() {
        if member_dir.join("package.json").is_file() {
            member.ecosystem = Some("node".to_string());
        } else if member_dir.join("Cargo.toml").is_file()
            || member_dir.join("src-tauri/Cargo.toml").is_file()
        {
            member.ecosystem = Some("rust".to_string());
        } else if member_dir.join("pyproject.toml").is_file()
            || member_dir.join("requirements.txt").is_file()
        {
            member.ecosystem = Some("python".to_string());
        } else if member_dir.join("go.mod").is_file() {
            member.ecosystem = Some("go".to_string());
        }
    }
}

impl WorkspaceMember {
    pub fn detect_default_command(
        member_dir: &Path,
        workspace_root: &Path,
    ) -> Option<(String, Vec<String>, &'static str)> {
        detect_default_command(member_dir, workspace_root)
    }

    pub fn resolve_command(
        &self,
        member_dir: &Path,
        workspace_root: &Path,
    ) -> Result<(String, Vec<String>), String> {
        resolve_command(self, member_dir, workspace_root)
    }

    pub fn populate_detected_defaults(&mut self, member_dir: &Path, workspace_root: &Path) {
        populate_detected_defaults(self, member_dir, workspace_root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use tempfile::tempdir;

    #[test]
    fn test_detect_rust_command() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )?;
        let res = detect_rust_command(dir.path());
        assert!(res.is_some());
        let (cmd, args, eco) = res.ok_or("missing rust detection")?;
        assert_eq!(cmd, "cargo");
        assert_eq!(args, vec!["run"]);
        assert_eq!(eco, "rust");
        Ok(())
    }

    #[test]
    fn test_detect_node_command_package_json() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("package.json"),
            "{\"scripts\": {\"dev\": \"vite\"}}",
        )?;
        let res = detect_node_command(dir.path(), dir.path());
        assert!(res.is_some());
        let (_, args, eco) = res.ok_or("missing node detection")?;
        assert_eq!(args, vec!["run", "dev"]);
        assert_eq!(eco, "node");
        Ok(())
    }

    #[test]
    fn test_detect_go_command() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(dir.path().join("go.mod"), "module example.com/test\n")?;
        let res = detect_go_command(dir.path());
        assert!(res.is_some());
        let (cmd, _, eco) = res.ok_or("missing go detection")?;
        assert_eq!(cmd, "go");
        assert_eq!(eco, "go");
        Ok(())
    }
}
