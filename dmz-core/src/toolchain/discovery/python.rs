use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path,
    push_system_bin_candidates, push_unique_path, ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers host Python runtime (python3, pip), prioritizing standalone environments.
#[derive(Debug, Default, Clone)]
pub struct PythonDiscoverer;

impl ToolchainDiscoverer for PythonDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Python
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut py_candidates: Vec<PathBuf> = Vec::new();

        // 1. Virtualenv if active
        if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
            let bin = PathBuf::from(venv).join("bin/python3");
            if bin.exists() {
                push_unique_path(&mut py_candidates, bin);
            }
        }

        // 2. Standalone Pyenv versions
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let pyenv_dir = PathBuf::from(&home).join(".pyenv/versions");
            if let Ok(entries) = fs::read_dir(&pyenv_dir) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin/python3");
                    if bin.exists() {
                        push_unique_path(&mut py_candidates, bin);
                    }
                }
            }
        }

        // 3. Official Framework / Standard locations
        let standard_paths = [
            PathBuf::from("/Library/Frameworks/Python.framework/Versions/Current/bin/python3"),
            PathBuf::from("/usr/local/bin/python3"),
            PathBuf::from("/usr/bin/python3"),
        ];
        for p in &standard_paths {
            if p.exists() {
                push_unique_path(&mut py_candidates, p.clone());
            }
        }

        // 4. Fallback to system PATH and standard host bin directories
        push_system_bin_candidates(&mut py_candidates, "python3");

        if py_candidates.is_empty() {
            return Ok(None);
        }

        py_candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_py: Option<PathBuf> = None;
        for candidate in py_candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }
            if chosen_py.is_none()
                || (!is_non_standalone_path(&candidate)
                    && is_non_standalone_path(chosen_py.as_ref().unwrap()))
            {
                chosen_py = Some(candidate.clone());
                if !is_non_standalone_path(&candidate) {
                    break;
                }
            }
        }

        let py_bin = match chosen_py {
            Some(p) => p,
            None => return Ok(None),
        };

        let version = Command::new(&py_bin)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "python3".to_string())
            .trim()
            .to_string();

        let canonical_bin = fs::canonicalize(&py_bin).unwrap_or_else(|_| py_bin.clone());
        let py_root = canonical_bin
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        info!(
            py_root = ?py_root,
            version = %version,
            "Discovered host Python toolchain"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((py_bin.clone(), "bin/python3".into()));
        bin_files.push((py_bin.clone(), "bin/python".into()));

        if cfg!(windows) {
            bin_files.push((py_bin.clone(), "python.exe".into()));
            bin_files.push((py_bin.clone(), "bin/python.exe".into()));
            bin_files.push((py_bin.clone(), "bin/python3.exe".into()));

            if let Some(parent) = py_bin.parent() {
                if let Ok(entries) = fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                                if ext.eq_ignore_ascii_case("dll") {
                                    let fname = entry.file_name().to_string_lossy().to_string();
                                    bin_files.push((p.clone(), fname.clone()));
                                    bin_files.push((p, format!("bin/{}", fname)));
                                }
                            }
                        }
                    }
                }
            }
        }

        // Companion pip
        if let Some(parent) = py_bin.parent() {
            let pip3 = parent.join("pip3");
            if pip3.exists() {
                bin_files.push((pip3.clone(), "bin/pip3".into()));
                bin_files.push((pip3, "bin/pip".into()));
            } else if let Ok(p) = which::which("pip3") {
                bin_files.push((p.clone(), "bin/pip3".into()));
                bin_files.push((p, "bin/pip".into()));
            } else {
                let pip = parent.join("pip");
                if pip.exists() {
                    bin_files.push((pip.clone(), "bin/pip".into()));
                    bin_files.push((pip, "bin/pip3".into()));
                } else if let Ok(p) = which::which("pip") {
                    bin_files.push((p.clone(), "bin/pip".into()));
                    bin_files.push((p, "bin/pip3".into()));
                }
            }
        }

        // Standard library discovery for relocatable Python execution
        if let Ok(output) = Command::new(&py_bin)
            .arg("-c")
            .arg("import sysconfig; print(sysconfig.get_path('stdlib'))")
            .output()
        {
            if output.status.success() {
                let stdlib_path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stdlib_dir = PathBuf::from(&stdlib_path_str);
                if stdlib_dir.is_dir() {
                    let folder_name = stdlib_dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("python3");
                    let target_prefix = if cfg!(windows) || folder_name.eq_ignore_ascii_case("Lib")
                    {
                        "Lib".to_string()
                    } else {
                        format!("lib/{}", folder_name)
                    };
                    collect_files_recursive(
                        &stdlib_dir,
                        &stdlib_dir,
                        &mut lib_files,
                        &target_prefix,
                    );
                }
            }
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Python,
            version,
            root_dir: py_root,
            bin_files,
            lib_files,
        }))
    }
}
