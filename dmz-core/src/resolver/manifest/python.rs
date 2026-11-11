use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::fs;
use std::path::Path;

pub struct PythonManifestScanner;

impl PythonManifestScanner {
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        let req_path = root_path.join("requirements.txt");
        if req_path.exists() {
            results.push(Self::parse_requirements_txt(&req_path)?);
        }

        let pyproject_path = root_path.join("pyproject.toml");
        if pyproject_path.exists() {
            if let Ok(pyproject_info) = Self::parse_pyproject_toml(&pyproject_path) {
                if !pyproject_info.dependencies.is_empty() {
                    results.push(pyproject_info);
                }
            }
        }

        // Merge locked transitive Python closure from uv.lock or poetry.lock if present
        let uv_lock_path = root_path.join("uv.lock");
        let poetry_lock_path = root_path.join("poetry.lock");
        let py_lock_deps = if uv_lock_path.is_file() {
            Self::parse_python_toml_lockfile(&uv_lock_path)
        } else if poetry_lock_path.is_file() {
            Self::parse_python_toml_lockfile(&poetry_lock_path)
        } else {
            Vec::new()
        };
        if !py_lock_deps.is_empty() {
            if let Some(existing_py) = results.iter_mut().find(|m| m.ecosystem == Ecosystem::PyPI) {
                super::merge_locked_dependencies(&mut existing_py.dependencies, py_lock_deps);
            } else {
                let lock_path = if uv_lock_path.is_file() {
                    uv_lock_path
                } else {
                    poetry_lock_path
                };
                results.push(ManifestInfo {
                    ecosystem: Ecosystem::PyPI,
                    path: lock_path,
                    dependencies: py_lock_deps,
                });
            }
        }

        Ok(())
    }

    pub fn parse_requirements_txt(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path).map_err(|e| {
            ManifestError::IoError(format!("Failed to read requirements.txt: {}", e))
        })?;

        let mut dependencies = Vec::new();

        for line in content.lines() {
            if let Some((name, version_req)) = Self::parse_pep508_requirement(line) {
                dependencies.push(DependencySpec {
                    name,
                    version_req,
                    ecosystem: Ecosystem::PyPI,
                    is_dev: false,
                });
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::PyPI,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    pub fn parse_pyproject_toml(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read pyproject.toml: {}", e)))?;

        let toml_val: toml::Value = toml::from_str(&content)
            .map_err(|e| ManifestError::ParseError(format!("Invalid pyproject.toml: {}", e)))?;

        let mut dependencies = Vec::new();

        // 1. PEP 621 [project.dependencies]
        if let Some(deps_arr) = toml_val
            .get("project")
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
        {
            for dep in deps_arr {
                if let Some(line) = dep.as_str() {
                    if let Some((name, version_req)) = Self::parse_pep508_requirement(line) {
                        dependencies.push(DependencySpec {
                            name,
                            version_req,
                            ecosystem: Ecosystem::PyPI,
                            is_dev: false,
                        });
                    }
                }
            }
        }

        // 1b. PEP 621 [project.optional-dependencies] and [dependency-groups] (PEP 735)
        for group_table_opt in [
            toml_val
                .get("project")
                .and_then(|p| p.get("optional-dependencies"))
                .and_then(|d| d.as_table()),
            toml_val.get("dependency-groups").and_then(|d| d.as_table()),
        ]
        .into_iter()
        .flatten()
        {
            for (_group_name, group_arr) in group_table_opt {
                if let Some(arr) = group_arr.as_array() {
                    for dep in arr {
                        if let Some(line) = dep.as_str() {
                            if let Some((name, version_req)) = Self::parse_pep508_requirement(line)
                            {
                                if !dependencies
                                    .iter()
                                    .any(|d| d.name.eq_ignore_ascii_case(&name))
                                {
                                    dependencies.push(DependencySpec {
                                        name,
                                        version_req,
                                        ecosystem: Ecosystem::PyPI,
                                        is_dev: true,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Poetry [tool.poetry.dependencies]
        if let Some(poetry_deps) = toml_val
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_table())
        {
            for (name, val) in poetry_deps {
                if name.to_lowercase() == "python" {
                    continue;
                }
                let version_req = match val {
                    toml::Value::String(s) => s
                        .trim_start_matches('^')
                        .trim_start_matches('~')
                        .to_string(),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(|s| {
                            s.trim_start_matches('^')
                                .trim_start_matches('~')
                                .to_string()
                        })
                        .unwrap_or_else(|| "*".to_string()),
                    _ => "*".to_string(),
                };

                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req,
                    ecosystem: Ecosystem::PyPI,
                    is_dev: false,
                });
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::PyPI,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    /// Parses a PEP 508 dependency specification string (from requirements.txt, pyproject.toml, or wheel `requires_dist`).
    /// Strips extras (`pkg[extra]`), evaluates basic OS/extra markers (`; extra == ...`, `; sys_platform == ...`),
    /// and returns `(package_name, version_req)`.
    pub fn parse_pep508_requirement(raw_line: &str) -> Option<(String, String)> {
        let without_comment = raw_line.split('#').next().unwrap_or("").trim();
        if without_comment.is_empty() || without_comment.starts_with('-') {
            return None;
        }

        let mut parts = without_comment.splitn(2, ';');
        let req_part = parts.next().unwrap_or("").trim();
        if let Some(marker) = parts.next() {
            let m = marker.to_lowercase();
            // Skip optional extra-only requirements unless explicitly requested
            if m.contains("extra ==") || m.contains("extra==") {
                return None;
            }
            // Evaluate basic OS/platform markers against current host OS
            let host_os = std::env::consts::OS;
            if (m.contains("sys_platform")
                || m.contains("platform_system")
                || m.contains("os_name"))
                && m.contains("==")
                && !m.contains("!=")
            {
                let mentions_win = m.contains("win32")
                    || m.contains("windows")
                    || m.contains("'nt'")
                    || m.contains("\"nt\"");
                let mentions_mac = m.contains("darwin") || m.contains("macos");
                let mentions_linux = m.contains("linux");
                let mentions_bsd = m.contains("freebsd")
                    || m.contains("openbsd")
                    || m.contains("sunos")
                    || m.contains("aix")
                    || m.contains("cygwin");

                let matches_host = match host_os {
                    "macos" => mentions_mac || (!mentions_win && !mentions_linux && !mentions_bsd),
                    "linux" => mentions_linux || (!mentions_win && !mentions_mac && !mentions_bsd),
                    "windows" => {
                        mentions_win || (!mentions_mac && !mentions_linux && !mentions_bsd)
                    }
                    _ => true,
                };
                if !matches_host {
                    return None;
                }
            }
        }

        let req_cleaned = req_part.replace(['(', ')'], "");
        let mut name = req_cleaned.trim().to_string();
        let mut version_req = "*".to_string();

        for op in &["===", "==", "~=", ">=", "<=", "!=", ">", "<", "@"] {
            if let Some(idx) = req_cleaned.find(op) {
                name = req_cleaned[..idx].trim().to_string();
                let rhs = req_cleaned[idx + op.len()..].trim();
                if *op == "@" {
                    return None; // Direct URL/path reference
                }
                let first_constraint = rhs.split(',').next().unwrap_or("").trim();
                if (*op == "==" || *op == "===" || *op == ">=" || *op == "~=")
                    && !first_constraint.is_empty()
                    && !first_constraint.contains('*')
                {
                    version_req = first_constraint.to_string();
                } else {
                    version_req = "*".to_string();
                }
                break;
            }
        }

        // Strip PEP 508 extras (e.g. `uvicorn[standard]` -> `uvicorn`)
        if let Some(bracket_idx) = name.find('[') {
            name = name[..bracket_idx].trim().to_string();
        }

        if name.is_empty() {
            return None;
        }

        Some((name, version_req))
    }

    pub fn normalize_pypi_name(name: &str) -> String {
        let mut out = String::with_capacity(name.len());
        let mut prev_sep = false;
        for c in name.chars() {
            if c == '-' || c == '_' || c == '.' {
                if !prev_sep {
                    out.push('-');
                    prev_sep = true;
                }
            } else {
                out.push(c.to_ascii_lowercase());
                prev_sep = false;
            }
        }
        out
    }

    /// Parses `uv.lock` or `poetry.lock` TOML `[[package]]` entries.
    pub fn parse_python_toml_lockfile(path: &Path) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };
        let Ok(val) = toml::from_str::<toml::Value>(&content) else {
            return deps;
        };
        let Some(packages) = val.get("package").and_then(|p| p.as_array()) else {
            return deps;
        };
        for pkg in packages {
            // Skip editable/virtual root workspace packages in uv.lock / poetry.lock
            if let Some(source) = pkg.get("source").and_then(|s| s.as_table()) {
                if source.contains_key("virtual")
                    || source.contains_key("editable")
                    || source.contains_key("directory")
                    || source.get("type").and_then(|t| t.as_str()) == Some("directory")
                    || source.get("type").and_then(|t| t.as_str()) == Some("file")
                {
                    continue;
                }
            }
            let Some(name) = pkg.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let Some(version) = pkg.get("version").and_then(|v| v.as_str()) else {
                continue;
            };
            if name.is_empty() || version.is_empty() {
                continue;
            }
            if !deps.iter().any(|d: &DependencySpec| {
                Self::normalize_pypi_name(&d.name) == Self::normalize_pypi_name(name)
            }) {
                deps.push(DependencySpec {
                    name: name.to_string(),
                    version_req: version.to_string(),
                    ecosystem: Ecosystem::PyPI,
                    is_dev: false,
                });
            }
        }
        deps
    }
}
