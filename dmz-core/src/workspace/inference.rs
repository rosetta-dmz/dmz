// Command and ecosystem detection algorithms for workspace members.
// Inspects package manifests, entrypoints, and scripts across Node, Rust, Python, and Go.

use super::schema::WorkspaceMember;
use std::fs;
use std::path::{Path, PathBuf};

/// Errors raised during workspace command inference and resolution.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum WorkspaceError {
    #[error("Unable to determine execution command for workspace member '{member_id}' at {member_dir}. {reason}. Specify \"exec_command\" and \"exec_args\" in dmz-workspace.toml{hint}")]
    CommandInferenceFailed {
        member_id: String,
        member_dir: String,
        /// Short, human-readable explanation of why inference failed (no trailing period).
        reason: String,
        /// Empty, or a ready-to-paste `[[members]]` snippet prefixed with `, e.g.:\n`.
        hint: String,
    },
}

/// Explains why no execution command could be inferred for a member directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceReport {
    pub reason: String,
    pub suggestion: Option<(String, Vec<String>)>,
}

impl InferenceReport {
    fn new(reason: impl Into<String>, suggestion: Option<(&str, &[&str])>) -> Self {
        Self {
            reason: reason.into(),
            suggestion: suggestion.map(|(c, a)| {
                (
                    c.to_string(),
                    a.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
                )
            }),
        }
    }

    /// Renders the ready-to-paste `dmz-workspace.toml` snippet (empty when no suggestion exists).
    pub fn toml_hint(&self, member_id: &str) -> String {
        match &self.suggestion {
            Some((cmd, args)) => {
                let args_toml = args
                    .iter()
                    .map(|a| toml_basic_string(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    ", e.g.:\n  [[members]]\n  id = {}\n  exec_command = {}\n  exec_args = [{}]",
                    toml_basic_string(member_id),
                    toml_basic_string(cmd),
                    args_toml
                )
            }
            None => String::new(),
        }
    }
}

/// Renders `value` as a TOML basic string literal.
fn toml_basic_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Names of subdirectories (one level deep) that look like standalone projects.
fn project_subdirs(member_dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(member_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            [
                "package.json",
                "Cargo.toml",
                "pyproject.toml",
                "requirements.txt",
                "go.mod",
            ]
            .iter()
            .any(|m| p.join(m).is_file())
        })
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
        .collect();
    found.sort();
    found
}

/// Builds a diagnostic explaining why inference failed for `member_dir`.
pub fn diagnose_inference_failure(member_dir: &Path) -> InferenceReport {
    // Rust: virtual workspace / multiple binaries
    if let Some(reason) = rust_ambiguity(member_dir) {
        return InferenceReport::new(reason, Some(("cargo", &["run", "--bin", "<name>"])));
    }
    // Go: multiple cmd/* binaries and no root main package
    let go_cmds = go_cmd_entries(member_dir);
    if member_dir.join("go.mod").is_file() && go_cmds.len() > 1 {
        return InferenceReport::new(
            format!(
                "multiple Go entrypoints found under cmd/ ({}) and no root main package",
                go_cmds.join(", ")
            ),
            Some(("go", &["run", "./cmd/<name>"])),
        );
    }
    if member_dir.join("package.json").is_file() {
        return InferenceReport::new(
            "package.json has no dev/start/serve/tauri script, no runnable \"main\"/\"bin\" entry, and no index.js/server.js/main.js/app.js",
            Some(("npm", &["run", "<script>"])),
        );
    }
    let has_py_manifest = ["requirements.txt", "pyproject.toml", "setup.py", "Pipfile"]
        .iter()
        .any(|m| member_dir.join(m).is_file());
    if has_py_manifest {
        return InferenceReport::new(
            "Python project found but no entrypoint (server.py/main.py/app.py, __main__.py, or [project.scripts])",
            Some(("python3", &["-m", "<package>"])),
        );
    }
    let subs = project_subdirs(member_dir);
    let reason = if subs.is_empty() {
        "no recognised project manifest (package.json, Cargo.toml, pyproject.toml, requirements.txt, go.mod) or entry script was found in the member directory".to_string()
    } else {
        format!(
            "no project manifest in the member directory itself, but projects were found in subdirectories ({}); point the member `path` at the project directory",
            subs.join(", ")
        )
    };
    InferenceReport::new(reason, None)
}

impl From<WorkspaceError> for String {
    fn from(err: WorkspaceError) -> Self {
        err.to_string()
    }
}

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

    let pkg_json_content = fs::read_to_string(&pkg_json_path).ok();
    let pkg_json_val = pkg_json_content
        .as_deref()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok());
    let pkg_manager_field = pkg_json_val
        .as_ref()
        .and_then(|v| v.get("packageManager"))
        .and_then(|m| m.as_str())
        .unwrap_or("");

    let pm = if member_dir.join("pnpm-lock.yaml").exists()
        || workspace_root.join("pnpm-lock.yaml").exists()
        || member_dir.join("pnpm-workspace.yaml").exists()
        || workspace_root.join("pnpm-workspace.yaml").exists()
        || member_dir.join("node_modules/.pnpm").is_dir()
        || pkg_manager_field.starts_with("pnpm@")
        || pkg_manager_field == "pnpm"
    {
        "pnpm".to_string()
    } else if member_dir.join("yarn.lock").exists()
        || workspace_root.join("yarn.lock").exists()
        || pkg_manager_field.starts_with("yarn@")
        || pkg_manager_field == "yarn"
    {
        "yarn".to_string()
    } else if member_dir.join("bun.lockb").exists()
        || member_dir.join("bun.lock").exists()
        || pkg_manager_field.starts_with("bun@")
        || pkg_manager_field == "bun"
    {
        "bun".to_string()
    } else {
        "npm".to_string()
    };

    if let Some(val) = pkg_json_val {
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

        // `bin`: either a single string path or an object with exactly one entry
        let bin_path = match val.get("bin") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(serde_json::Value::Object(o)) if o.len() == 1 => {
                o.values().next().and_then(|v| v.as_str()).map(String::from)
            }
            _ => None,
        };
        if let Some(bin_file) = bin_path {
            if member_dir.join(&bin_file).is_file() {
                return Some(("node".to_string(), vec![bin_file], "node"));
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

/// Returns a reason when `cargo run` would be rejected by Cargo for this member:
/// a virtual workspace manifest (no root package), or several binary targets without `default-run`.
fn rust_ambiguity(member_dir: &Path) -> Option<String> {
    let manifest_dir = if member_dir.join("Cargo.toml").is_file() {
        member_dir.to_path_buf()
    } else if member_dir.join("src-tauri/Cargo.toml").is_file() {
        member_dir.join("src-tauri")
    } else {
        return None;
    };
    let content = fs::read_to_string(manifest_dir.join("Cargo.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&content).ok()?;

    let Some(package) = parsed.get("package") else {
        let ws = parsed.get("workspace")?;
        let members = ws
            .get("members")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        return Some(format!(
            "Cargo.toml is a virtual workspace with no root package (members: {}), so `cargo run` has no target",
            if members.is_empty() { "none declared" } else { &members }
        ));
    };

    if package.get("default-run").is_some() {
        return None;
    }

    let mut bins: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if manifest_dir.join("src/main.rs").is_file() {
        if let Some(name) = package.get("name").and_then(|n| n.as_str()) {
            bins.insert(name.to_string());
        }
    }
    if let Some(entries) = parsed.get("bin").and_then(|b| b.as_array()) {
        for e in entries {
            if let Some(name) = e.get("name").and_then(|n| n.as_str()) {
                bins.insert(name.to_string());
            }
        }
    }
    if let Ok(rd) = fs::read_dir(manifest_dir.join("src/bin")) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|e| e == "rs") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    bins.insert(stem.to_string());
                }
            } else if p.is_dir() && p.join("main.rs").is_file() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    bins.insert(name.to_string());
                }
            }
        }
    }

    if bins.len() > 1 {
        Some(format!(
            "multiple binary targets ({}) and no `default-run` in Cargo.toml",
            bins.into_iter().collect::<Vec<_>>().join(", ")
        ))
    } else {
        None
    }
}

pub fn detect_rust_command(member_dir: &Path) -> Option<(String, Vec<String>, &'static str)> {
    let cargo_toml_path = member_dir.join("Cargo.toml");
    let tauri_cargo_path = member_dir.join("src-tauri/Cargo.toml");
    if !(tauri_cargo_path.is_file() || cargo_toml_path.is_file()) {
        return None;
    }
    // Never guess: Cargo itself rejects `cargo run` for virtual workspaces and multi-binary packages.
    if rust_ambiguity(member_dir).is_some() {
        return None;
    }
    Some(("cargo".to_string(), vec!["run".to_string()], "rust"))
}

fn detect_top_level_python_script(member_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(member_dir).ok()?;
    let mut scripts: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "py"))
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .filter(|name| !matches!(name.as_str(), "setup.py" | "conftest.py" | "__init__.py"))
        .collect();
    scripts.sort();
    scripts.into_iter().next()
}

/// Directories never scanned for Python entrypoints (hidden, virtualenv, build output, tests).
fn is_ignored_python_scan_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
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
}

pub fn detect_python_command(member_dir: &Path) -> Option<(String, Vec<String>, &'static str)> {
    let py_req_path = member_dir.join("requirements.txt");
    let pyproject_path = member_dir.join("pyproject.toml");
    let setup_py_path = member_dir.join("setup.py");
    let pipfile_path = member_dir.join("Pipfile");

    let has_manifest = py_req_path.is_file()
        || pyproject_path.is_file()
        || setup_py_path.is_file()
        || pipfile_path.is_file();

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
            if let Ok(parsed) = toml::from_str::<toml::Value>(&content) {
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

    // A directory with `__main__.py` is directly runnable (`python .`, PEP 338)
    if member_dir.join("__main__.py").is_file() {
        return Some((py_cmd, vec![".".to_string()], "python"));
    }

    if !has_manifest {
        // Standalone .py script detection when manifests are absent (sorted deterministically)
        if let Some(script) = detect_top_level_python_script(member_dir) {
            return Some((py_cmd, vec![script], "python"));
        }
        return detect_python_module_entry(member_dir, false).map(|args| (py_cmd, args, "python"));
    }

    // Check 1-level package subdirectories for main.py / server.py / app.py
    if let Ok(entries) = fs::read_dir(member_dir) {
        let mut subdirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|name| !is_ignored_python_scan_dir(name))
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

    // Package with `__main__.py` (`python -m <pkg>`)
    if let Some(args) = detect_python_module_entry(member_dir, pyproject_path.is_file()) {
        return Some((py_cmd, args, "python"));
    }

    // Fallback inside manifest-backed directory: any non-setup root .py file
    detect_top_level_python_script(member_dir).map(|script| (py_cmd, vec![script], "python"))
}

/// Finds a single importable package that ships `__main__.py` and returns `["-m", "<pkg>"]`.
///
/// Flat layout (`<member>/<pkg>/__main__.py`) works with the member directory on `sys.path`.
/// `src/` layout (`<member>/src/<pkg>/__main__.py`) additionally requires the package to be
/// installed (e.g. editable), so it is only inferred when a `pyproject.toml` is present.
/// Multiple candidates are ambiguous and yield `None`.
fn detect_python_module_entry(member_dir: &Path, has_pyproject: bool) -> Option<Vec<String>> {
    let is_ident = |s: &str| {
        !s.is_empty()
            && !s.starts_with(|c: char| c.is_ascii_digit())
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    let scan = |dir: &Path| -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join("__main__.py").is_file())
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .filter(|n| !is_ignored_python_scan_dir(n) && is_ident(n))
            .collect();
        names.sort();
        names
    };

    let mut candidates = scan(member_dir);
    if has_pyproject {
        candidates.extend(scan(&member_dir.join("src")));
        candidates.sort();
        candidates.dedup();
    }
    match candidates.as_slice() {
        [only] => Some(vec!["-m".to_string(), only.clone()]),
        _ => None,
    }
}

/// Names of `cmd/<name>/` directories that contain a `main.go`.
fn go_cmd_entries(member_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(member_dir.join("cmd"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("main.go").is_file())
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    names.sort();
    names
}

/// True when a root-level `.go` file declares `package main`.
fn go_root_has_main_package(member_dir: &Path) -> bool {
    fs::read_dir(member_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "go"))
        .filter(|p| !p.to_string_lossy().ends_with("_test.go"))
        .any(|p| {
            fs::read_to_string(p)
                .map(|c| c.lines().any(|l| l.trim() == "package main"))
                .unwrap_or(false)
        })
}

pub fn detect_go_command(member_dir: &Path) -> Option<(String, Vec<String>, &'static str)> {
    let go_mod_path = member_dir.join("go.mod");
    if !go_mod_path.is_file() {
        return None;
    }
    if !go_root_has_main_package(member_dir) {
        let cmds = go_cmd_entries(member_dir);
        match cmds.as_slice() {
            [] => {}
            [only] => {
                return Some((
                    "go".to_string(),
                    vec!["run".to_string(), format!("./cmd/{only}")],
                    "go",
                ))
            }
            _ => return None,
        }
    }
    Some((
        "go".to_string(),
        vec!["run".to_string(), ".".to_string()],
        "go",
    ))
}

pub fn resolve_command(
    member: &WorkspaceMember,
    member_dir: &Path,
    workspace_root: &Path,
) -> Result<(String, Vec<String>), WorkspaceError> {
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

    let report = diagnose_inference_failure(member_dir);
    Err(WorkspaceError::CommandInferenceFailed {
        member_id: member.id.clone(),
        member_dir: member_dir.display().to_string(),
        hint: report.toml_hint(&member.id),
        reason: report.reason,
    })
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
    ) -> Result<(String, Vec<String>), WorkspaceError> {
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

    #[test]
    fn test_detect_standalone_python_script_without_manifest() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(dir.path().join("worker.py"), "print('ok')\n")?;
        let res = detect_default_command(dir.path(), dir.path());
        let (cmd, args, eco) = res.ok_or("missing standalone python detection")?;
        assert!(cmd == "python3" || cmd == "python");
        assert_eq!(args, vec!["worker.py"]);
        assert_eq!(eco, "python");
        Ok(())
    }

    #[test]
    fn test_detect_pnpm_via_package_manager_field() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("package.json"),
            r#"{"packageManager": "pnpm@9.0.0", "scripts": {"start": "node index.js"}}"#,
        )?;
        let (cmd, args, eco) =
            detect_node_command(dir.path(), dir.path()).ok_or("missing pnpm detection")?;
        assert_eq!(cmd, "pnpm");
        assert_eq!(args, vec!["run", "start"]);
        assert_eq!(eco, "node");
        Ok(())
    }

    #[test]
    fn test_detect_python_root_dunder_main() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"x\"\n",
        )?;
        fs::write(dir.path().join("__main__.py"), "")?;
        let (_, args, eco) =
            detect_python_command(dir.path()).ok_or("missing __main__ detection")?;
        assert_eq!(args, vec!["."]);
        assert_eq!(eco, "python");
        Ok(())
    }

    #[test]
    fn test_detect_python_flat_and_src_module_entry() -> Result<(), Box<dyn Error>> {
        // flat layout works without pyproject
        let flat = tempdir()?;
        fs::create_dir_all(flat.path().join("app"))?;
        fs::write(flat.path().join("app/__main__.py"), "")?;
        let (_, args, _) = detect_python_command(flat.path()).ok_or("missing flat module")?;
        assert_eq!(args, vec!["-m", "app"]);

        // src layout requires pyproject.toml
        let src = tempdir()?;
        fs::create_dir_all(src.path().join("src/engine"))?;
        fs::write(src.path().join("src/engine/__main__.py"), "")?;
        assert!(detect_python_command(src.path()).is_none());
        fs::write(
            src.path().join("pyproject.toml"),
            "[project]\nname = \"engine\"\n",
        )?;
        let (_, args, _) = detect_python_command(src.path()).ok_or("missing src module")?;
        assert_eq!(args, vec!["-m", "engine"]);
        Ok(())
    }

    #[test]
    fn test_detect_python_multiple_module_entries_is_ambiguous() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        for pkg in ["alpha", "beta"] {
            fs::create_dir_all(dir.path().join(pkg))?;
            fs::write(dir.path().join(pkg).join("__main__.py"), "")?;
        }
        assert!(detect_python_command(dir.path()).is_none());
        Ok(())
    }

    #[test]
    fn test_detect_node_bin_entry() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("bin"))?;
        fs::write(dir.path().join("bin/cli.js"), "")?;
        fs::write(
            dir.path().join("package.json"),
            r#"{"name": "x", "bin": {"x": "bin/cli.js"}, "scripts": {"build": "tsc"}}"#,
        )?;
        let (cmd, args, _) =
            detect_node_command(dir.path(), dir.path()).ok_or("missing node bin")?;
        assert_eq!(cmd, "node");
        assert_eq!(args, vec!["bin/cli.js"]);
        Ok(())
    }

    #[test]
    fn test_rust_default_run_and_single_bin_are_runnable() -> Result<(), Box<dyn Error>> {
        let single = tempdir()?;
        fs::create_dir_all(single.path().join("src"))?;
        fs::write(single.path().join("src/main.rs"), "fn main() {}")?;
        fs::write(
            single.path().join("Cargo.toml"),
            "[package]\nname = \"one\"\n",
        )?;
        assert!(detect_rust_command(single.path()).is_some());

        let multi = tempdir()?;
        fs::create_dir_all(multi.path().join("src/bin"))?;
        fs::write(multi.path().join("src/main.rs"), "fn main() {}")?;
        fs::write(multi.path().join("src/bin/tool.rs"), "fn main() {}")?;
        fs::write(
            multi.path().join("Cargo.toml"),
            "[package]\nname = \"multi\"\ndefault-run = \"multi\"\n",
        )?;
        assert!(detect_rust_command(multi.path()).is_some());
        Ok(())
    }

    #[test]
    fn test_rust_multi_bin_without_default_run_fails_with_candidates() -> Result<(), Box<dyn Error>>
    {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("src/bin"))?;
        fs::write(dir.path().join("src/main.rs"), "fn main() {}")?;
        fs::write(dir.path().join("src/bin/tool.rs"), "fn main() {}")?;
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"multi\"\n",
        )?;
        assert!(detect_rust_command(dir.path()).is_none());

        let member = WorkspaceMember::new("svc", dir.path(), crate::workspace::MemberRole::Sidecar);
        let err = member
            .resolve_command(dir.path(), dir.path())
            .expect_err("ambiguous rust member must fail")
            .to_string();
        assert!(err.contains("'svc'"), "{err}");
        assert!(err.contains("multi, tool"), "{err}");
        assert!(err.contains("default-run"), "{err}");
        assert!(err.contains("exec_command = \"cargo\""), "{err}");
        Ok(())
    }

    #[test]
    fn test_rust_virtual_workspace_fails_with_member_list() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\"]\n",
        )?;
        assert!(detect_default_command(dir.path(), dir.path()).is_none());
        let report = diagnose_inference_failure(dir.path());
        assert!(report.reason.contains("virtual workspace"));
        assert!(report.reason.contains("a, b"));
        Ok(())
    }

    #[test]
    fn test_go_cmd_layout_detection() -> Result<(), Box<dyn Error>> {
        let one = tempdir()?;
        fs::write(one.path().join("go.mod"), "module x\n")?;
        fs::create_dir_all(one.path().join("cmd/server"))?;
        fs::write(one.path().join("cmd/server/main.go"), "package main\n")?;
        let (_, args, _) = detect_go_command(one.path()).ok_or("missing go cmd")?;
        assert_eq!(args, vec!["run", "./cmd/server"]);

        let many = tempdir()?;
        fs::write(many.path().join("go.mod"), "module x\n")?;
        for n in ["a", "b"] {
            fs::create_dir_all(many.path().join("cmd").join(n))?;
            fs::write(
                many.path().join("cmd").join(n).join("main.go"),
                "package main\n",
            )?;
        }
        assert!(detect_go_command(many.path()).is_none());

        let root_main = tempdir()?;
        fs::write(root_main.path().join("go.mod"), "module x\n")?;
        fs::write(root_main.path().join("main.go"), "package main\n")?;
        let (_, args, _) = detect_go_command(root_main.path()).ok_or("missing root main")?;
        assert_eq!(args, vec!["run", "."]);
        Ok(())
    }

    #[test]
    fn test_diagnostics_name_member_and_subdirectory_projects() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("backend"))?;
        fs::write(
            dir.path().join("backend/pyproject.toml"),
            "[project]\nname=\"b\"\n",
        )?;
        let member =
            WorkspaceMember::new("root", dir.path(), crate::workspace::MemberRole::Primary);
        let err = member
            .resolve_command(dir.path(), dir.path())
            .expect_err("no manifest at member root")
            .to_string();
        assert!(err.contains("Unable to determine execution command for workspace member 'root'"));
        assert!(err.contains("subdirectories (backend)"), "{err}");
        assert!(err.contains("Specify \"exec_command\" and \"exec_args\" in dmz-workspace.toml"));
        Ok(())
    }

    #[test]
    fn test_node_without_runnable_entry_reports_actionable_snippet() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("package.json"),
            r#"{"name": "x", "scripts": {"build": "tsc"}}"#,
        )?;
        let member = WorkspaceMember::new("ui", dir.path(), crate::workspace::MemberRole::Sidecar);
        let err = member
            .resolve_command(dir.path(), dir.path())
            .expect_err("node without entry must fail")
            .to_string();
        assert!(
            err.contains("package.json has no dev/start/serve/tauri script"),
            "{err}"
        );
        assert!(err.contains("id = \"ui\""), "{err}");
        assert!(err.contains("exec_command = \"npm\""), "{err}");
        Ok(())
    }
}
