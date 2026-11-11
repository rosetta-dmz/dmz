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
            let should_replace = match &chosen_py {
                None => true,
                Some(prev) => !is_non_standalone_path(&candidate) && is_non_standalone_path(prev),
            };
            if should_replace {
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

        // On macOS Framework builds (Homebrew / python.org), `bin/python3.X` is `pythonw` which
        // uses `dladdr` + `posix_spawn` to launch `Resources/Python.app/Contents/MacOS/Python`.
        // Using the inner `Python.app/Contents/MacOS/Python` executable directly as `bin/python3`
        // avoids the framework path assumption while linking directly to `_Py_BytesMain` in `lib/Python`.
        let macos_app_py = py_root.join("Resources/Python.app/Contents/MacOS/Python");
        let bundled_py_bin = if macos_app_py.is_file() {
            macos_app_py
        } else {
            py_bin.clone()
        };

        bin_files.push((bundled_py_bin.clone(), "bin/python3".into()));
        bin_files.push((bundled_py_bin, "bin/python".into()));

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
                if !is_non_standalone_path(&p) {
                    bin_files.push((p.clone(), "bin/pip3".into()));
                    bin_files.push((p, "bin/pip".into()));
                } else {
                    emit_non_relocatable_notice(&p);
                }
            } else {
                let pip = parent.join("pip");
                if pip.exists() {
                    bin_files.push((pip.clone(), "bin/pip".into()));
                    bin_files.push((pip, "bin/pip3".into()));
                } else if let Ok(p) = which::which("pip") {
                    if !is_non_standalone_path(&p) {
                        bin_files.push((p.clone(), "bin/pip".into()));
                        bin_files.push((p, "bin/pip3".into()));
                    } else {
                        emit_non_relocatable_notice(&p);
                    }
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

        // Standard library modules, shared libraries (libpython), and C headers
        let lib_dir = py_root.join("lib");
        if lib_dir.is_dir() {
            collect_python_lib_dir(&lib_dir, &py_root, &mut lib_files);
        }
        // macOS Framework build shared library (e.g. Python.framework/Versions/3.X/Python)
        let framework_py = py_root.join("Python");
        if framework_py.is_file() {
            lib_files.push((framework_py.clone(), "lib/Python".to_string()));
            lib_files.push((framework_py, "lib/libpython3.dylib".to_string()));
        }
        // Windows root python3*.dll shared libraries
        if let Ok(entries) = fs::read_dir(&py_root) {
            for entry in entries.flatten() {
                let p = entry.path();
                let s = entry.file_name().to_string_lossy().to_string();
                if p.is_file() && s.starts_with("python") && s.ends_with(".dll") {
                    bin_files.push((p, s));
                }
            }
        }
        let win_lib = py_root.join("Lib");
        if win_lib.is_dir() {
            collect_python_files_recursive(&win_lib, &py_root, &mut lib_files, "");
        }
        let inc_dir = py_root.join("include");
        if inc_dir.is_dir() {
            collect_files_recursive(&inc_dir, &py_root, &mut lib_files, "");
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

/// Collects `lib/python3.X` standard library directories and `lib/libpython*` shared libraries.
fn collect_python_lib_dir(lib_dir: &Path, py_root: &Path, lib_files: &mut Vec<(PathBuf, String)>) {
    if let Ok(entries) = fs::read_dir(lib_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let fname = entry.file_name();
            let s = fname.to_string_lossy();
            if p.is_dir() && s.starts_with("python3.") {
                collect_python_files_recursive(&p, py_root, lib_files, "");
            } else if p.is_file()
                && (s.starts_with("libpython")
                    && (s.ends_with(".dylib")
                        || s.contains(".so")
                        || s.ends_with(".a")
                        || s.ends_with(".dll"))
                    || s == "Python")
            {
                lib_files.push((p, format!("lib/{}", s)));
            }
        }
    }
}

/// Recursively collects Python standard library files while omitting bytecode cache directories.
fn collect_python_files_recursive(
    dir: &Path,
    base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    target_prefix: &str,
) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let name = entry.file_name();
                if name == "__pycache__" || name == "idlelib" || name == "test" || name == "tests" {
                    continue;
                }
                collect_python_files_recursive(&p, base, acc, target_prefix);
            } else if p.is_file() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    let target_path = if target_prefix.is_empty() {
                        rel_str
                    } else if target_prefix.ends_with('/') {
                        format!("{}{}", target_prefix, rel_str)
                    } else {
                        format!("{}/{}", target_prefix, rel_str)
                    };
                    acc.push((p.clone(), target_path));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_python_collects_stdlib_and_skips_pycache() {
        let temp = tempdir().unwrap();
        let py_root = temp.path().join("fake_py");
        let stdlib = py_root.join("lib/python3.11");
        let pycache = stdlib.join("__pycache__");
        let inc = py_root.join("include/python3.11");

        fs::create_dir_all(&stdlib).unwrap();
        fs::create_dir_all(&pycache).unwrap();
        fs::create_dir_all(&inc).unwrap();

        fs::write(stdlib.join("os.py"), "# os").unwrap();
        fs::write(stdlib.join("sys.py"), "# sys").unwrap();
        fs::write(pycache.join("os.cpython-311.pyc"), "bytecode").unwrap();
        fs::write(inc.join("Python.h"), "#define PYTHON_H").unwrap();

        let mut lib_files = Vec::new();
        collect_python_files_recursive(&stdlib, &py_root, &mut lib_files, "");
        collect_files_recursive(&inc, &py_root, &mut lib_files, "");

        let rel_paths: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(rel_paths.contains(&"lib/python3.11/os.py".to_string()));
        assert!(rel_paths.contains(&"lib/python3.11/sys.py".to_string()));
        assert!(!rel_paths.iter().any(|p| p.contains("__pycache__")));
        assert!(rel_paths.contains(&"include/python3.11/Python.h".to_string()));
    }
}
