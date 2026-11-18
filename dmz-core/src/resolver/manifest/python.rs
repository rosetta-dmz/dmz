use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::fs;
use std::path::Path;

pub struct PythonManifestScanner;

impl PythonManifestScanner {
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        Self::scan_with_options(root_path, results, None, false)
    }

    pub fn scan_with_options(
        root_path: &Path,
        results: &mut Vec<ManifestInfo>,
        target_platform: Option<&str>,
        include_dev: bool,
    ) -> Result<(), ManifestError> {
        let target_py = crate::toolchain::discovery::python::resolve_effective_python_minor(
            Some(root_path),
            &[],
            target_platform,
        );
        let req_path = root_path.join("requirements.txt");
        if req_path.exists() {
            results.push(Self::parse_requirements_txt_with_target_python(
                &req_path,
                target_platform,
                target_py,
            )?);
        }

        let pyproject_path = root_path.join("pyproject.toml");
        if pyproject_path.exists() {
            if let Ok(pyproject_info) = Self::parse_pyproject_toml_with_target_python(
                &pyproject_path,
                target_platform,
                target_py,
            ) {
                if !pyproject_info.dependencies.is_empty() {
                    results.push(pyproject_info);
                }
            }
        }

        // Merge locked transitive Python closure from uv.lock or poetry.lock if present
        let uv_lock_path = root_path.join("uv.lock");
        let poetry_lock_path = root_path.join("poetry.lock");
        let py_lock_deps = if uv_lock_path.is_file() {
            Self::parse_python_toml_lockfile_with_options_and_python(
                &uv_lock_path,
                target_platform,
                include_dev,
                target_py,
            )
        } else if poetry_lock_path.is_file() {
            Self::parse_python_toml_lockfile_with_options_and_python(
                &poetry_lock_path,
                target_platform,
                include_dev,
                target_py,
            )
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
        Self::parse_requirements_txt_with_target(path, None)
    }

    pub fn parse_requirements_txt_with_target(
        path: &Path,
        target_platform: Option<&str>,
    ) -> Result<ManifestInfo, ManifestError> {
        Self::parse_requirements_txt_with_target_python(path, target_platform, None)
    }

    /// Parses `requirements.txt` filtering PEP 508 markers against both `target_platform`
    /// and `target_py` (major, minor).
    ///
    /// # Errors
    /// Returns `ManifestError::IoError` if `path` cannot be read.
    pub fn parse_requirements_txt_with_target_python(
        path: &Path,
        target_platform: Option<&str>,
        target_py: Option<(u8, u8)>,
    ) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path).map_err(|e| {
            ManifestError::IoError(format!("Failed to read requirements.txt: {}", e))
        })?;

        let mut dependencies = Vec::new();

        for line in content.lines() {
            if let Some((name, version_req)) =
                Self::parse_pep508_requirement_for_target_python(line, target_platform, target_py)
            {
                dependencies.push(DependencySpec {
                    name,
                    version_req,
                    ecosystem: Ecosystem::PyPI,
                    is_dev: false,
                    dependencies: Vec::new(),
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
        Self::parse_pyproject_toml_with_target(path, None)
    }

    pub fn parse_pyproject_toml_with_target(
        path: &Path,
        target_platform: Option<&str>,
    ) -> Result<ManifestInfo, ManifestError> {
        Self::parse_pyproject_toml_with_target_python(path, target_platform, None)
    }

    /// Parses `pyproject.toml` filtering PEP 508 markers against both `target_platform`
    /// and `target_py` (major, minor).
    ///
    /// # Errors
    /// Returns `ManifestError` if `path` cannot be read or contains invalid TOML.
    pub fn parse_pyproject_toml_with_target_python(
        path: &Path,
        target_platform: Option<&str>,
        target_py: Option<(u8, u8)>,
    ) -> Result<ManifestInfo, ManifestError> {
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
                    if let Some((name, version_req)) =
                        Self::parse_pep508_requirement_for_target_python(
                            line,
                            target_platform,
                            target_py,
                        )
                    {
                        dependencies.push(DependencySpec {
                            name,
                            version_req,
                            ecosystem: Ecosystem::PyPI,
                            is_dev: false,
                            dependencies: Vec::new(),
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
                            if let Some((name, version_req)) =
                                Self::parse_pep508_requirement_for_target_python(
                                    line,
                                    target_platform,
                                    target_py,
                                )
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
                                        dependencies: Vec::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Poetry [tool.poetry.dependencies] and dev dependency groups
        if let Some(poetry) = toml_val.get("tool").and_then(|t| t.get("poetry")) {
            Self::push_poetry_table_deps(poetry.get("dependencies"), false, &mut dependencies);
            Self::push_poetry_table_deps(poetry.get("dev-dependencies"), true, &mut dependencies);
            if let Some(groups) = poetry.get("group").and_then(|g| g.as_table()) {
                for (group_name, group_val) in groups {
                    let is_dev = !group_name.eq_ignore_ascii_case("main");
                    Self::push_poetry_table_deps(
                        group_val.get("dependencies"),
                        is_dev,
                        &mut dependencies,
                    );
                }
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::PyPI,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    fn push_poetry_table_deps(
        table_opt: Option<&toml::Value>,
        is_dev: bool,
        dependencies: &mut Vec<DependencySpec>,
    ) {
        let Some(poetry_deps) = table_opt.and_then(|d| d.as_table()) else {
            return;
        };
        for (name, val) in poetry_deps {
            if name.eq_ignore_ascii_case("python") {
                continue;
            }
            if is_dev
                && dependencies
                    .iter()
                    .any(|d| Self::normalize_pypi_name(&d.name) == Self::normalize_pypi_name(name))
            {
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
                is_dev,
                dependencies: Vec::new(),
            });
        }
    }

    /// Returns `true` when `normalized_name` is a legacy Python stdlib backport (e.g. Python 2
    /// `typing` or `enum34`) that shadows built-in modules on `target_py`.
    #[must_use]
    pub(crate) fn is_obsolete_stdlib_backport(
        normalized_name: &str,
        target_py: Option<(u8, u8)>,
    ) -> bool {
        let py = target_py.unwrap_or((3, 11));
        if py < (3, 5) {
            return false;
        }
        matches!(
            normalized_name,
            "typing" | "enum34" | "pathlib" | "pathlib2" | "ordereddict" | "argparse"
        ) || (normalized_name == "dataclasses" && py >= (3, 7))
    }

    fn parse_pep508_ver_tuple(raw: &str) -> Option<(u8, u8, u16, bool)> {
        let cleaned = raw.trim().trim_matches(['\'', '"', '(', ')']).trim();
        let mut parts = cleaned.split('.');
        let maj_str = parts.next()?.trim();
        let maj = maj_str.parse::<u8>().ok()?;
        let min_part = parts.next().unwrap_or("0").trim();
        let min_wild = min_part.contains('*');
        let min_digits: String = min_part.chars().take_while(char::is_ascii_digit).collect();
        let min = if min_digits.is_empty() {
            0
        } else {
            min_digits.parse::<u8>().ok()?
        };
        let pat_part = parts.next().unwrap_or("0").trim();
        let pat_wild = pat_part.contains('*');
        let pat_digits: String = pat_part.chars().take_while(char::is_ascii_digit).collect();
        let pat = if pat_digits.is_empty() {
            0
        } else {
            pat_digits.parse::<u16>().ok()?
        };
        Some((maj, min, pat, min_wild || pat_wild))
    }

    fn eval_python_version_clause(c: &str, target_py: (u8, u8)) -> Option<bool> {
        let var = if c.contains("python_full_version") {
            "python_full_version"
        } else if c.contains("python_version") {
            "python_version"
        } else {
            return None;
        };
        let rhs = c.split_once(var)?.1.trim();
        const OPS: &[&str] = &["===", "==", "!=", ">=", "<=", "~=", ">", "<"];
        let op = OPS.iter().find(|&&o| rhs.starts_with(o))?;
        let ver_raw = rhs.strip_prefix(op)?.trim();
        let (maj, min, pat, is_wild) = Self::parse_pep508_ver_tuple(ver_raw)?;

        // If patch > 0 on same major.minor, we only know target minor -> stay permissive for `>=`/`>`.
        if var == "python_full_version"
            && maj == target_py.0
            && min == target_py.1
            && pat > 0
            && matches!(*op, ">=" | ">")
        {
            return Some(true);
        }

        let target_mm = (target_py.0, target_py.1);
        let req_mm = (maj, min);
        Some(match *op {
            "==" | "===" => target_mm == req_mm && (is_wild || pat == 0),
            "!=" => target_mm != req_mm && !is_wild,
            ">=" => target_mm >= req_mm,
            "<=" => target_mm <= req_mm,
            ">" => target_mm > req_mm,
            "<" => target_mm < req_mm,
            "~=" => target_py.0 == maj && target_py.1 >= min,
            _ => return None,
        })
    }

    fn eval_pep508_clause(clause: &str, target_os: &str, target_py: (u8, u8)) -> bool {
        let c = clause.trim().trim_matches(['(', ')']).trim();
        if let Some(py_match) = Self::eval_python_version_clause(c, target_py) {
            return py_match;
        }
        let has_os_var =
            c.contains("sys_platform") || c.contains("platform_system") || c.contains("os_name");
        if !has_os_var {
            return true;
        }
        let is_neq = c.contains("!=");
        let is_eq = !is_neq && (c.contains("==") || c.contains("===") || c.contains(" in "));
        if !is_eq && !is_neq {
            return true;
        }

        let mentions_win = c.contains("win32")
            || c.contains("windows")
            || c.contains("'nt'")
            || c.contains("\"nt\"");
        let mentions_mac = c.contains("darwin") || c.contains("macos");
        let mentions_linux = c.contains("linux");
        let mentions_bsd = c.contains("freebsd")
            || c.contains("openbsd")
            || c.contains("sunos")
            || c.contains("aix")
            || c.contains("cygwin");
        if !mentions_win && !mentions_mac && !mentions_linux && !mentions_bsd {
            return true;
        }

        let clause_os_matches = match target_os {
            "macos" => mentions_mac,
            "linux" => mentions_linux,
            "windows" => mentions_win,
            _ => true,
        };
        if is_neq {
            !clause_os_matches
        } else {
            clause_os_matches
        }
    }

    /// Evaluates a PEP 508 environment marker string against an optional target platform tag.
    pub fn matches_pep508_marker(marker: &str, target_platform: Option<&str>) -> bool {
        Self::matches_pep508_marker_for_python(marker, target_platform, None)
    }

    /// Evaluates a PEP 508 environment marker string against `target_platform` and `target_py`.
    #[must_use]
    pub fn matches_pep508_marker_for_python(
        marker: &str,
        target_platform: Option<&str>,
        target_py: Option<(u8, u8)>,
    ) -> bool {
        let m = marker.to_lowercase();
        if m.contains("extra ==") || m.contains("extra==") {
            return false;
        }
        let (target_os, _target_arch) = crate::workspace::resolve_target_os_arch(target_platform);
        let effective_py = target_py.unwrap_or((3, 11));
        m.split(" or ").any(|or_part| {
            or_part
                .split(" and ")
                .all(|clause| Self::eval_pep508_clause(clause, target_os, effective_py))
        })
    }

    /// Parses a PEP 508 dependency specification string for the current host platform.
    pub fn parse_pep508_requirement(raw_line: &str) -> Option<(String, String)> {
        Self::parse_pep508_requirement_for_target(raw_line, None)
    }

    /// Parses a PEP 508 dependency specification string (from requirements.txt, pyproject.toml, or wheel `requires_dist`).
    /// Strips extras (`pkg[extra]`), evaluates OS/extra markers against `target_platform`,
    /// and returns `(package_name, version_req)`.
    pub fn parse_pep508_requirement_for_target(
        raw_line: &str,
        target_platform: Option<&str>,
    ) -> Option<(String, String)> {
        Self::parse_pep508_requirement_for_target_python(raw_line, target_platform, None)
    }

    /// Variant of [`Self::parse_pep508_requirement_for_target`] evaluating `python_version`
    /// markers and rejecting obsolete stdlib backports against `target_py`.
    #[must_use]
    pub fn parse_pep508_requirement_for_target_python(
        raw_line: &str,
        target_platform: Option<&str>,
        target_py: Option<(u8, u8)>,
    ) -> Option<(String, String)> {
        let without_comment = raw_line.split('#').next().unwrap_or("").trim();
        if without_comment.is_empty() || without_comment.starts_with('-') {
            return None;
        }

        let mut parts = without_comment.splitn(2, ';');
        let req_part = parts.next().unwrap_or("").trim();
        if let Some(marker) = parts.next() {
            if !Self::matches_pep508_marker_for_python(marker, target_platform, target_py) {
                return None;
            }
        }

        let req_cleaned = req_part.replace(['(', ')'], "");
        let req_trimmed = req_cleaned.trim();
        if req_trimmed.is_empty() {
            return None;
        }

        // Find where the package name (and optional extras [extra1, extra2]) ends
        // and where the version specifiers or URL reference starts.
        // In PEP 508, package names only contain [a-zA-Z0-9_.-], and extras are enclosed in [...].
        // Version specifiers start with one of: ===, ==, ~=, >=, <=, !=, >, <, or @.
        const ALL_OPS: &[&str] = &["===", "==", "~=", ">=", "<=", "!=", ">", "<", "@"];

        let search_start = if let Some(close_bracket) = req_trimmed.find(']') {
            close_bracket.saturating_add(1)
        } else {
            0
        };

        let search_slice = req_trimmed.get(search_start..).unwrap_or("");
        let mut earliest_op_offset = None;
        for op in ALL_OPS {
            if let Some(idx) = search_slice.find(op) {
                match earliest_op_offset {
                    None => earliest_op_offset = Some(idx),
                    Some(cur_min) if idx < cur_min => earliest_op_offset = Some(idx),
                    _ => {}
                }
            }
        }

        let (name_part, specifiers_part) = match earliest_op_offset {
            Some(offset) => {
                let split_idx = search_start.saturating_add(offset);
                let name = req_trimmed.get(..split_idx).unwrap_or(req_trimmed);
                let spec = req_trimmed.get(split_idx..).map(str::trim).unwrap_or("");
                (name, spec)
            }
            None => (req_trimmed, ""),
        };

        // Direct URL/path reference (@ ...), ignore as not a remote registry dependency
        if specifiers_part.starts_with('@') {
            return None;
        }

        // Strip PEP 508 extras (e.g. `uvicorn[standard]` -> `uvicorn`)
        let mut name = name_part.trim().to_string();
        if let Some(bracket_idx) = name.find('[') {
            if let Some(base_name) = name.get(..bracket_idx) {
                name = base_name.trim().to_string();
            }
        }

        if name.is_empty() {
            return None;
        }

        // Parse version constraints if present
        let mut version_req = "*".to_string();
        if !specifiers_part.is_empty() {
            const CMP_OPS: &[&str] = &["===", "==", "~=", ">=", "<=", "!=", ">", "<"];

            let mut exact_ver = None;
            let mut compatible_ver = None;
            let mut lower_bound_ver = None;

            for raw_clause in specifiers_part.split(',') {
                let clause = raw_clause.trim();
                if clause.is_empty() {
                    continue;
                }
                for op in CMP_OPS {
                    if let Some(stripped) = clause.strip_prefix(op) {
                        let ver = stripped.trim();
                        if !ver.is_empty() && !ver.contains('*') {
                            match *op {
                                "==" | "===" if exact_ver.is_none() => {
                                    exact_ver = Some(ver.to_string());
                                }
                                "~=" if compatible_ver.is_none() => {
                                    compatible_ver = Some(ver.to_string());
                                }
                                ">=" if lower_bound_ver.is_none() => {
                                    lower_bound_ver = Some(ver.to_string());
                                }
                                _ => {}
                            }
                        }
                        break;
                    }
                }
            }

            if let Some(ver) = exact_ver {
                version_req = ver;
            } else if let Some(ver) = compatible_ver {
                version_req = ver;
            } else if let Some(ver) = lower_bound_ver {
                version_req = ver;
            }
        }

        if Self::is_obsolete_stdlib_backport(&Self::normalize_pypi_name(&name), target_py) {
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

    /// Parses `uv.lock` or `poetry.lock` TOML `[[package]]` entries for the host platform.
    pub fn parse_python_toml_lockfile(path: &Path) -> Vec<DependencySpec> {
        Self::parse_python_toml_lockfile_with_options(path, None, false)
    }

    /// Parses `uv.lock` or `poetry.lock` TOML `[[package]]` entries with target platform and dev-dependency filtering.
    pub fn parse_python_toml_lockfile_with_options(
        path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
    ) -> Vec<DependencySpec> {
        Self::parse_python_toml_lockfile_with_options_and_python(
            path,
            target_platform,
            include_dev,
            None,
        )
    }

    /// Parses `uv.lock` or `poetry.lock` TOML `[[package]]` entries with `target_platform`, `include_dev`,
    /// and `target_py` filtering.
    #[must_use]
    pub fn parse_python_toml_lockfile_with_options_and_python(
        path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        target_py: Option<(u8, u8)>,
    ) -> Vec<DependencySpec> {
        let Ok(content) = fs::read_to_string(path) else {
            return Vec::new();
        };
        let Ok(val) = toml::from_str::<toml::Value>(&content) else {
            return Vec::new();
        };
        let Some(packages) = val.get("package").and_then(|p| p.as_array()) else {
            return Vec::new();
        };

        let is_uv_lock = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "uv.lock");

        if is_uv_lock {
            Self::walk_uv_lock_closure(packages, target_platform, include_dev, target_py)
        } else {
            Self::parse_poetry_lock_packages(packages, include_dev, target_py)
        }
    }

    fn is_virtual_python_package(pkg: &toml::Value) -> bool {
        if let Some(source) = pkg.get("source").and_then(|s| s.as_table()) {
            return source.contains_key("virtual")
                || source.contains_key("editable")
                || source.contains_key("directory")
                || source.get("type").and_then(|t| t.as_str()) == Some("directory")
                || source.get("type").and_then(|t| t.as_str()) == Some("file");
        }
        false
    }

    fn parse_poetry_lock_packages(
        packages: &[toml::Value],
        include_dev: bool,
        target_py: Option<(u8, u8)>,
    ) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        for pkg in packages {
            if Self::is_virtual_python_package(pkg) {
                continue;
            }
            let is_dev_pkg = pkg
                .get("category")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c != "main")
                || pkg
                    .get("groups")
                    .and_then(|g| g.as_array())
                    .is_some_and(|groups| {
                        !groups.is_empty() && groups.iter().all(|g| g.as_str() != Some("main"))
                    });
            if is_dev_pkg && !include_dev {
                continue;
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
            let norm = Self::normalize_pypi_name(name);
            if Self::is_obsolete_stdlib_backport(&norm, target_py) {
                continue;
            }
            if !deps
                .iter()
                .any(|d: &DependencySpec| Self::normalize_pypi_name(&d.name) == norm)
            {
                deps.push(DependencySpec {
                    name: name.to_string(),
                    version_req: version.to_string(),
                    ecosystem: Ecosystem::PyPI,
                    is_dev: is_dev_pkg,
                    dependencies: Vec::new(),
                });
            }
        }
        deps
    }

    fn walk_uv_lock_closure(
        packages: &[toml::Value],
        target_platform: Option<&str>,
        include_dev: bool,
        target_py: Option<(u8, u8)>,
    ) -> Vec<DependencySpec> {
        let mut by_name: std::collections::HashMap<String, Vec<&toml::Value>> =
            std::collections::HashMap::new();
        let mut root_indices = Vec::new();

        for (idx, pkg) in packages.iter().enumerate() {
            if let Some(name) = pkg.get("name").and_then(|n| n.as_str()) {
                by_name
                    .entry(Self::normalize_pypi_name(name))
                    .or_default()
                    .push(pkg);
            }
            if Self::is_virtual_python_package(pkg) {
                root_indices.push(idx);
            }
        }

        let mut reachable_names: Option<std::collections::HashSet<String>> = None;
        if !root_indices.is_empty() {
            let mut visited = std::collections::HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            for root_idx in root_indices {
                if let Some(root_pkg) = packages.get(root_idx) {
                    if let Some(root_name) = root_pkg.get("name").and_then(|n| n.as_str()) {
                        let norm = Self::normalize_pypi_name(root_name);
                        if visited.insert(norm.clone()) {
                            queue.push_back(norm);
                        }
                    }
                }
            }

            while let Some(curr_norm) = queue.pop_front() {
                let Some(pkg_variants) = by_name.get(&curr_norm) else {
                    continue;
                };
                for pkg in pkg_variants {
                    if !Self::uv_pkg_matches_target(pkg, target_platform, target_py) {
                        continue;
                    }
                    let is_root = Self::is_virtual_python_package(pkg);
                    Self::enqueue_uv_dep_array(
                        pkg.get("dependencies"),
                        target_platform,
                        target_py,
                        &mut visited,
                        &mut queue,
                    );
                    if include_dev && is_root {
                        for group_key in ["dev-dependencies", "optional-dependencies"] {
                            if let Some(tbl) = pkg.get(group_key).and_then(|v| v.as_table()) {
                                for group_val in tbl.values() {
                                    Self::enqueue_uv_dep_array(
                                        Some(group_val),
                                        target_platform,
                                        target_py,
                                        &mut visited,
                                        &mut queue,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            reachable_names = Some(visited);
        }

        let mut deps = Vec::new();
        for pkg in packages {
            if Self::is_virtual_python_package(pkg) {
                continue;
            }
            if !Self::uv_pkg_matches_target(pkg, target_platform, target_py) {
                continue;
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
            let norm = Self::normalize_pypi_name(name);
            if Self::is_obsolete_stdlib_backport(&norm, target_py) {
                continue;
            }
            if let Some(ref allowed) = reachable_names {
                if !allowed.contains(&norm) {
                    continue;
                }
            }
            if !deps
                .iter()
                .any(|d: &DependencySpec| Self::normalize_pypi_name(&d.name) == norm)
            {
                deps.push(DependencySpec {
                    name: name.to_string(),
                    version_req: version.to_string(),
                    ecosystem: Ecosystem::PyPI,
                    is_dev: false,
                    dependencies: Vec::new(),
                });
            }
        }
        deps
    }

    fn uv_pkg_matches_target(
        pkg: &toml::Value,
        target_platform: Option<&str>,
        target_py: Option<(u8, u8)>,
    ) -> bool {
        let markers_match = match pkg.get("resolution-markers").and_then(|m| m.as_array()) {
            Some(markers) if !markers.is_empty() => markers
                .iter()
                .filter_map(|m| m.as_str())
                .any(|m| Self::matches_pep508_marker_for_python(m, target_platform, target_py)),
            _ => true,
        };
        if !markers_match {
            return false;
        }

        // Reject packages whose lock entry has no sdist and only foreign-OS binary wheels.
        let (target_os, _) = crate::workspace::resolve_target_os_arch(target_platform);
        if target_os != "windows" && pkg.get("sdist").is_none() {
            if let Some(wheels) = pkg.get("wheels").and_then(|w| w.as_array()) {
                if !wheels.is_empty() {
                    let all_win_wheels = wheels.iter().all(|w| {
                        let raw = w
                            .get("url")
                            .and_then(|u| u.as_str())
                            .or_else(|| w.get("filename").and_then(|f| f.as_str()))
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        !raw.is_empty()
                            && !raw.contains("-none-any.whl")
                            && (raw.contains("win_amd64")
                                || raw.contains("win_arm64")
                                || raw.contains("win32"))
                    });
                    if all_win_wheels {
                        return false;
                    }
                }
            }
        }

        true
    }

    fn enqueue_uv_dep_array(
        deps_val: Option<&toml::Value>,
        target_platform: Option<&str>,
        target_py: Option<(u8, u8)>,
        visited: &mut std::collections::HashSet<String>,
        queue: &mut std::collections::VecDeque<String>,
    ) {
        let Some(arr) = deps_val.and_then(|v| v.as_array()) else {
            return;
        };
        for dep in arr {
            if let Some(marker) = dep.get("marker").and_then(|m| m.as_str()) {
                if !Self::matches_pep508_marker_for_python(marker, target_platform, target_py) {
                    continue;
                }
            }
            if let Some(name) = dep.get("name").and_then(|n| n.as_str()) {
                let norm = Self::normalize_pypi_name(name);
                if Self::is_obsolete_stdlib_backport(&norm, target_py) {
                    continue;
                }
                if visited.insert(norm.clone()) {
                    queue.push_back(norm);
                }
            }
        }
    }
}
