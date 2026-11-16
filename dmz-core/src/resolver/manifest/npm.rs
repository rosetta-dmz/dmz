use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

pub struct NpmManifestScanner;

impl NpmManifestScanner {
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        Self::scan_with_options(root_path, None, true, results)
    }

    pub fn scan_with_options(
        root_path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        results: &mut Vec<ManifestInfo>,
    ) -> Result<(), ManifestError> {
        let mut visited_manifests = HashSet::new();
        Self::scan_node_dir_with_options(
            root_path,
            None,
            target_platform,
            include_dev,
            &mut visited_manifests,
            results,
        )?;

        for nested_root in crate::workspace::discover_node_project_roots(root_path) {
            if nested_root != root_path {
                Self::scan_node_dir_with_options(
                    &nested_root,
                    None,
                    target_platform,
                    include_dev,
                    &mut visited_manifests,
                    results,
                )?;
            }
        }
        Ok(())
    }

    fn scan_node_dir_with_options(
        root_path: &Path,
        ancestor_lock_dir: Option<&Path>,
        target_platform: Option<&str>,
        include_dev: bool,
        visited_manifests: &mut HashSet<PathBuf>,
        results: &mut Vec<ManifestInfo>,
    ) -> Result<(), ManifestError> {
        let package_json_path = root_path.join("package.json");
        if package_json_path.exists() {
            Self::parse_package_json_with_options_visited(
                &package_json_path,
                ancestor_lock_dir,
                target_platform,
                include_dev,
                visited_manifests,
                results,
            )?;
        } else {
            let effective_lock_dir = if Self::has_npm_lockfile(root_path) {
                Some(root_path)
            } else {
                ancestor_lock_dir
            };
            // Handle pnpm workspaces / monorepos that omit a root package.json
            let workspace_dirs = Self::discover_npm_workspace_package_dirs(root_path);
            for sub_dir in workspace_dirs {
                let sub_pkg_json = sub_dir.join("package.json");
                if sub_pkg_json.is_file() {
                    Self::parse_package_json_with_options_visited(
                        &sub_pkg_json,
                        effective_lock_dir,
                        target_platform,
                        include_dev,
                        visited_manifests,
                        results,
                    )?;
                }
            }
            let pnpm_lock = root_path.join("pnpm-lock.yaml");
            if pnpm_lock.is_file() {
                let empty = HashSet::new();
                let locked = Self::parse_pnpm_lock_with_options(
                    &pnpm_lock,
                    target_platform,
                    include_dev,
                    &empty,
                    &empty,
                );
                if !locked.is_empty() {
                    results.push(ManifestInfo {
                        ecosystem: Ecosystem::Npm,
                        path: pnpm_lock,
                        dependencies: locked,
                    });
                }
            }
        }
        Ok(())
    }

    fn has_npm_lockfile(dir: &Path) -> bool {
        dir.join("pnpm-lock.yaml").is_file()
            || dir.join("package-lock.json").is_file()
            || dir.join("yarn.lock").is_file()
    }

    pub fn sanitize_npm_version(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.starts_with("file:")
            || trimmed.starts_with("workspace:")
            || trimmed.starts_with("link:")
            || trimmed.starts_with("git+")
        {
            return trimmed.to_string();
        }

        let cleaned = trimmed
            .trim_start_matches('^')
            .trim_start_matches('~')
            .trim_start_matches(">=")
            .trim_start_matches("<=")
            .trim_start_matches('>')
            .trim_start_matches('<')
            .trim_start_matches('=')
            .trim();

        if cleaned.is_empty() || cleaned == "*" {
            "*".to_string()
        } else {
            cleaned.to_string()
        }
    }

    pub fn parse_package_json(path: &Path) -> Result<Vec<ManifestInfo>, ManifestError> {
        Self::parse_package_json_with_options(path, None, true)
    }

    pub fn parse_package_json_with_options(
        path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
    ) -> Result<Vec<ManifestInfo>, ManifestError> {
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        Self::parse_package_json_with_options_visited(
            path,
            None,
            target_platform,
            include_dev,
            &mut visited,
            &mut results,
        )?;
        Ok(results)
    }

    fn parse_package_json_with_options_visited(
        path: &Path,
        ancestor_lock_dir: Option<&Path>,
        target_platform: Option<&str>,
        include_dev: bool,
        visited_manifests: &mut HashSet<PathBuf>,
        results: &mut Vec<ManifestInfo>,
    ) -> Result<(), ManifestError> {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !visited_manifests.insert(canonical) {
            return Ok(());
        }

        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read package.json: {}", e)))?;

        let value: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ManifestError::ParseError(format!("Invalid package.json: {}", e)))?;

        let (mut dependencies, prod_roots, dev_roots) =
            Self::collect_pkg_json_deps(&value, target_platform, include_dev);

        let root_dir = path.parent().unwrap_or(path);
        let lock_search_dir = if Self::has_npm_lockfile(root_dir) {
            root_dir
        } else {
            ancestor_lock_dir.unwrap_or(root_dir)
        };
        let pnpm_lock = lock_search_dir.join("pnpm-lock.yaml");
        let npm_lock = lock_search_dir.join("package-lock.json");
        let yarn_lock = lock_search_dir.join("yarn.lock");

        let locked_npm_deps = if pnpm_lock.is_file() {
            Self::parse_pnpm_lock_with_options(
                &pnpm_lock,
                target_platform,
                include_dev,
                &prod_roots,
                &dev_roots,
            )
        } else if npm_lock.is_file() {
            Self::parse_package_lock_json_with_options(&npm_lock, target_platform, include_dev)
        } else if yarn_lock.is_file() {
            Self::parse_yarn_lock_with_options(
                &yarn_lock,
                target_platform,
                include_dev,
                &prod_roots,
                &dev_roots,
            )
        } else {
            Vec::new()
        };

        if !locked_npm_deps.is_empty() {
            // When inheriting an ancestor workspace lockfile, upgrade direct deps to exact locked
            // versions rather than re-appending the entire lockfile's transitive graph per member.
            if lock_search_dir != root_dir {
                let mut upgraded = Vec::new();
                for mut dep in dependencies {
                    if let Some(matched) = locked_npm_deps.iter().find(|l| l.name == dep.name) {
                        if !matched.version_req.is_empty() && matched.version_req != "*" {
                            dep.version_req = matched.version_req.clone();
                        }
                        if !matched.dependencies.is_empty() {
                            dep.dependencies = matched.dependencies.clone();
                        }
                    }
                    upgraded.push(dep);
                }
                dependencies = upgraded;
            } else {
                super::merge_locked_dependencies(&mut dependencies, locked_npm_deps);
            }
        }
        if !include_dev {
            dependencies.retain(|d| !d.is_dev);
        }

        results.push(ManifestInfo {
            ecosystem: Ecosystem::Npm,
            path: path.to_path_buf(),
            dependencies,
        });

        Self::collect_npm_workspace_members_visited(
            root_dir,
            Some(lock_search_dir),
            target_platform,
            include_dev,
            visited_manifests,
            results,
        );

        Ok(())
    }

    fn is_local_workspace_spec(ver: &str) -> bool {
        ver.starts_with("workspace:") || ver.starts_with("file:") || ver.starts_with("link:")
    }

    fn collect_pkg_json_deps(
        value: &serde_json::Value,
        target_platform: Option<&str>,
        include_dev: bool,
    ) -> (Vec<DependencySpec>, HashSet<String>, HashSet<String>) {
        let mut dependencies = Vec::new();
        let mut prod_roots = HashSet::new();
        let mut dev_roots = HashSet::new();

        for section in ["dependencies", "optionalDependencies"] {
            if let Some(deps) = value.get(section).and_then(|d| d.as_object()) {
                for (name, val) in deps {
                    if section == "optionalDependencies"
                        && !Self::is_compatible_npm_platform_pkg_for_target(name, target_platform)
                    {
                        continue;
                    }
                    let ver = Self::sanitize_npm_version(val.as_str().unwrap_or("*"));
                    if Self::is_local_workspace_spec(&ver) {
                        continue;
                    }
                    prod_roots.insert(name.clone());
                    dependencies.push(DependencySpec {
                        name: name.clone(),
                        version_req: ver,
                        ecosystem: Ecosystem::Npm,
                        is_dev: false,
                        dependencies: Vec::new(),
                    });
                }
            }
        }

        if let Some(dev_deps) = value.get("devDependencies").and_then(|d| d.as_object()) {
            for (name, val) in dev_deps {
                let ver = Self::sanitize_npm_version(val.as_str().unwrap_or("*"));
                if Self::is_local_workspace_spec(&ver) {
                    continue;
                }
                dev_roots.insert(name.clone());
                if include_dev {
                    dependencies.push(DependencySpec {
                        name: name.clone(),
                        version_req: ver,
                        ecosystem: Ecosystem::Npm,
                        is_dev: true,
                        dependencies: Vec::new(),
                    });
                }
            }
        }

        (dependencies, prod_roots, dev_roots)
    }

    /// Discovers npm/yarn/pnpm workspace sub-package directories declared in
    /// `package.json` (`workspaces`) or `pnpm-workspace.yaml` (`packages`).
    pub fn discover_npm_workspace_package_dirs(root_dir: &Path) -> Vec<PathBuf> {
        let mut patterns = Vec::new();

        let pkg_json_path = root_dir.join("package.json");
        if let Ok(content) = fs::read_to_string(&pkg_json_path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
                let workspaces_array =
                    value
                        .get("workspaces")
                        .and_then(|w| w.as_array())
                        .or_else(|| {
                            value
                                .get("workspaces")
                                .and_then(|w| w.get("packages"))
                                .and_then(|p| p.as_array())
                        });
                if let Some(arr) = workspaces_array {
                    for pat in arr {
                        if let Some(s) = pat.as_str() {
                            patterns.push(s.trim().to_string());
                        }
                    }
                }
            }
        }

        let pnpm_ws_path = root_dir.join("pnpm-workspace.yaml");
        if let Ok(content) = fs::read_to_string(&pnpm_ws_path) {
            patterns.extend(Self::parse_pnpm_workspace_yaml_patterns(&content));
        }

        let mut included = BTreeSet::new();
        let mut excluded = HashSet::new();
        let root_canon = fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());

        for raw_pat in patterns {
            let pat = raw_pat.trim().trim_matches('\'').trim_matches('"');
            if pat.is_empty() {
                continue;
            }
            let (is_negated, clean) = if let Some(rest) = pat.strip_prefix('!') {
                (true, rest.trim())
            } else {
                (false, pat)
            };
            let clean = clean.trim_start_matches("./").trim_end_matches('/');
            if clean.is_empty() || clean == "." {
                continue;
            }

            let mut matched = Vec::new();
            Self::expand_workspace_pattern(root_dir, clean, &mut matched);
            for dir in matched {
                let canon = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
                if !canon.starts_with(&root_canon) || canon == root_canon {
                    continue;
                }
                if is_negated {
                    excluded.insert(canon);
                } else {
                    included.insert((canon, dir));
                }
            }
        }

        included
            .into_iter()
            .filter(|(canon, _)| !excluded.contains(canon))
            .map(|(_, dir)| dir)
            .collect()
    }

    fn parse_pnpm_workspace_yaml_patterns(content: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut in_packages = false;
        let mut packages_indent = 0usize;

        for line in content.lines() {
            let trimmed_end = line.trim_end();
            let trimmed = trimmed_end.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = trimmed_end.len() - trimmed_end.trim_start().len();
            if trimmed == "packages:" {
                in_packages = true;
                packages_indent = indent;
                continue;
            }
            if !in_packages {
                continue;
            }
            if indent <= packages_indent && !trimmed.starts_with('-') {
                in_packages = false;
                continue;
            }
            if let Some(item) = trimmed.strip_prefix('-') {
                let clean = item
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"')
                    .to_string();
                if !clean.is_empty() {
                    out.push(clean);
                }
            }
        }
        out
    }

    fn expand_workspace_pattern(root_dir: &Path, pattern: &str, out: &mut Vec<PathBuf>) {
        if pattern.contains("**") {
            let base = pattern
                .split("**")
                .next()
                .unwrap_or("")
                .trim_end_matches('/');
            let start = if base.is_empty() {
                root_dir.to_path_buf()
            } else {
                root_dir.join(base)
            };
            Self::collect_recursive_package_dirs(&start, 0, out);
            return;
        }

        if let Some(prefix) = pattern
            .strip_suffix("/*")
            .or_else(|| pattern.strip_suffix('*'))
        {
            let base_dir = if prefix.is_empty() {
                root_dir.to_path_buf()
            } else {
                root_dir.join(prefix.trim_end_matches('/'))
            };
            if let Ok(entries) = fs::read_dir(base_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && path.join("package.json").is_file() {
                        out.push(path);
                    }
                }
            }
            return;
        }

        let exact_dir = root_dir.join(pattern);
        if exact_dir.is_dir() && exact_dir.join("package.json").is_file() {
            out.push(exact_dir);
        }
    }

    fn collect_recursive_package_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 6 || !dir.is_dir() {
            return;
        }
        if depth > 0 && dir.join("package.json").is_file() {
            out.push(dir.to_path_buf());
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if crate::workspace::should_skip_workspace_artifact_dir(&path, &name_str, None) {
                continue;
            }
            Self::collect_recursive_package_dirs(&path, depth + 1, out);
        }
    }

    fn collect_npm_workspace_members_visited(
        root_dir: &Path,
        ancestor_lock_dir: Option<&Path>,
        target_platform: Option<&str>,
        include_dev: bool,
        visited_manifests: &mut HashSet<PathBuf>,
        results: &mut Vec<ManifestInfo>,
    ) {
        for sub_dir in Self::discover_npm_workspace_package_dirs(root_dir) {
            let sub_pkg_json = sub_dir.join("package.json");
            if sub_pkg_json.is_file() {
                let _ = Self::parse_package_json_with_options_visited(
                    &sub_pkg_json,
                    ancestor_lock_dir,
                    target_platform,
                    include_dev,
                    visited_manifests,
                    results,
                );
            }
        }
    }

    /// Filters out platform-specific optional npm packages meant for foreign OS/CPU architectures.
    pub fn is_compatible_npm_platform_pkg(name: &str) -> bool {
        Self::is_compatible_npm_platform_pkg_for_target(name, None)
    }

    /// Target-aware filter for platform-specific optional npm packages.
    pub fn is_compatible_npm_platform_pkg_for_target(
        name: &str,
        target_platform: Option<&str>,
    ) -> bool {
        let lower = name.to_lowercase();
        let (target_os, target_arch) = crate::workspace::resolve_target_os_arch(target_platform);

        let mentions_darwin = lower.contains("-darwin") || lower.contains("darwin-");
        let mentions_linux = lower.contains("-linux") || lower.contains("linux-");
        let mentions_win =
            lower.contains("-win32") || lower.contains("win32-") || lower.contains("-windows");
        let mentions_other_os = lower.contains("-android")
            || lower.contains("-freebsd")
            || lower.contains("-openbsd")
            || lower.contains("-netbsd")
            || lower.contains("-sunos")
            || lower.contains("-aix")
            || lower.contains("-harmony");

        if mentions_other_os {
            return false;
        }

        let is_os_scoped = mentions_darwin || mentions_linux || mentions_win;
        if is_os_scoped {
            let os_ok = match target_os {
                "macos" => mentions_darwin,
                "linux" => mentions_linux,
                "windows" => mentions_win,
                _ => true,
            };
            if !os_ok {
                return false;
            }

            // If OS matches, also check CPU architecture if specified in the package name
            let mentions_arm64 = lower.contains("arm64") || lower.contains("aarch64");
            let mentions_x64 = lower.contains("x64") || lower.contains("x86_64");
            let mentions_other_arch = lower.contains("-ia32")
                || lower.contains("-arm-")
                || lower.ends_with("-arm")
                || lower.contains("-ppc64")
                || lower.contains("-s390x")
                || lower.contains("-riscv64")
                || lower.contains("-loong64");

            if mentions_other_arch {
                return false;
            }
            if mentions_arm64 || mentions_x64 {
                let arch_ok = match target_arch {
                    "aarch64" => mentions_arm64 || lower.contains("universal"),
                    "x86_64" => mentions_x64 || lower.contains("universal"),
                    _ => true,
                };
                if !arch_ok {
                    return false;
                }
            }
        }

        true
    }

    fn matches_npm_constraint_array(
        constraints: Option<&Vec<serde_json::Value>>,
        current: &str,
    ) -> bool {
        let Some(items) = constraints else {
            return true;
        };
        let mut positives = Vec::new();
        for item in items {
            if let Some(s) = item.as_str() {
                if let Some(neg) = s.strip_prefix('!') {
                    if neg == current {
                        return false;
                    }
                } else {
                    positives.push(s);
                }
            }
        }
        positives.is_empty() || positives.contains(&current)
    }

    fn is_compatible_package_lock_meta(
        meta: &serde_json::Value,
        target_platform: Option<&str>,
    ) -> bool {
        let (target_os, target_arch) = crate::workspace::resolve_target_os_arch(target_platform);
        let npm_os = match target_os {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let npm_arch = match target_arch {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            "x86" => "ia32",
            other => other,
        };
        Self::matches_npm_constraint_array(meta.get("os").and_then(|v| v.as_array()), npm_os)
            && Self::matches_npm_constraint_array(
                meta.get("cpu").and_then(|v| v.as_array()),
                npm_arch,
            )
    }

    /// Parses `pnpm-lock.yaml` (supporting lockfileVersion 5, 6, and 9) to extract all locked transitive packages.
    pub fn parse_pnpm_lock(path: &Path) -> Vec<DependencySpec> {
        let empty = HashSet::new();
        Self::parse_pnpm_lock_with_options(path, None, true, &empty, &empty)
    }

    pub fn parse_pnpm_lock_with_options(
        path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        prod_roots: &HashSet<String>,
        dev_roots: &HashSet<String>,
    ) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };

        let mut parser = PnpmLockParser::new(&mut deps, target_platform);
        for line in content.lines() {
            parser.process_line(line);
        }

        let has_root_graph = parser.has_root_graph;
        let pnpm_prod_roots = parser.pnpm_prod_roots;
        Self::finalize_pnpm_deps(
            &mut deps,
            has_root_graph,
            &pnpm_prod_roots,
            prod_roots,
            dev_roots,
            include_dev,
        );
        deps
    }

    fn finalize_pnpm_deps(
        deps: &mut Vec<DependencySpec>,
        has_root_graph: bool,
        pnpm_prod_roots: &HashSet<(String, String)>,
        prod_roots: &HashSet<String>,
        dev_roots: &HashSet<String>,
        include_dev: bool,
    ) {
        if has_root_graph && !pnpm_prod_roots.is_empty() {
            let mut key_to_indices: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
            let mut name_to_indices: HashMap<&str, Vec<usize>> = HashMap::new();
            for (idx, d) in deps.iter().enumerate() {
                key_to_indices
                    .entry((d.name.as_str(), d.version_req.as_str()))
                    .or_default()
                    .push(idx);
                name_to_indices
                    .entry(d.name.as_str())
                    .or_default()
                    .push(idx);
            }

            let mut reachable = HashSet::new();
            let mut queue = VecDeque::new();
            for (root_name, root_ver) in pnpm_prod_roots {
                if let Some(indices) = key_to_indices.get(&(root_name.as_str(), root_ver.as_str()))
                {
                    for &idx in indices {
                        if reachable.insert(idx) {
                            queue.push_back(idx);
                        }
                    }
                } else if let Some(indices) = name_to_indices.get(root_name.as_str()) {
                    for &idx in indices {
                        if !deps.get(idx).is_some_and(|d| d.is_dev) && reachable.insert(idx) {
                            queue.push_back(idx);
                        }
                    }
                }
            }
            for root_name in prod_roots {
                if let Some(indices) = name_to_indices.get(root_name.as_str()) {
                    for &idx in indices {
                        if !deps.get(idx).is_some_and(|d| d.is_dev) && reachable.insert(idx) {
                            queue.push_back(idx);
                        }
                    }
                }
            }

            while let Some(idx) = queue.pop_front() {
                let edges = deps
                    .get(idx)
                    .map(|d| d.dependencies.clone())
                    .unwrap_or_default();
                for edge in edges {
                    if let Some((dep_name, dep_ver)) = Self::split_pnpm_edge(&edge) {
                        if let Some(targets) = key_to_indices.get(&(dep_name, dep_ver)) {
                            for &next_idx in targets {
                                if reachable.insert(next_idx) {
                                    queue.push_back(next_idx);
                                }
                            }
                        } else if let Some(targets) = name_to_indices.get(dep_name) {
                            for &next_idx in targets {
                                if reachable.insert(next_idx) {
                                    queue.push_back(next_idx);
                                }
                            }
                        }
                    }
                }
            }

            for (idx, dep) in deps.iter_mut().enumerate() {
                if !reachable.contains(&idx) {
                    dep.is_dev = true;
                }
            }
        } else if !dev_roots.is_empty() {
            for dep in deps.iter_mut() {
                if dev_roots.contains(&dep.name) && !prod_roots.contains(&dep.name) {
                    dep.is_dev = true;
                }
            }
        }

        if !include_dev {
            deps.retain(|d| !d.is_dev);
        }
    }

    fn split_pnpm_edge(edge: &str) -> Option<(&str, &str)> {
        let (name, ver) = edge.rsplit_once('@')?;
        if name.is_empty() || ver.is_empty() {
            return None;
        }
        Some((name, ver))
    }

    pub fn parse_pnpm_package_key(key: &str) -> Option<(String, String)> {
        if key.is_empty()
            || key.starts_with("link:")
            || key.starts_with("file:")
            || key.starts_with("workspace:")
        {
            return None;
        }
        // Strip peer dependency suffix such as `pkg@1.0.0(react@18.0.0)` or `pkg@1.0.0_react@18.0.0`
        // AFTER isolating `name` from `version`, so underscores inside package names (e.g. `lodash_es`)
        // are preserved.
        let base = key.split('(').next().unwrap_or(key);

        let (name, raw_ver) = if let Some(rest) = base.strip_prefix('@') {
            let slash_idx = rest.find('/')?;
            let after_scope = &rest[slash_idx + 1..];
            if let Some(at_idx) = after_scope.find('@') {
                let pkg_name = &base[..1 + slash_idx + 1 + at_idx];
                let ver_part = &after_scope[at_idx + 1..];
                (pkg_name, ver_part)
            } else {
                let ver_slash = after_scope.find('/')?;
                let pkg_name = &base[..1 + slash_idx + 1 + ver_slash];
                let ver_part = &after_scope[ver_slash + 1..];
                (pkg_name, ver_part)
            }
        } else if let Some(at_idx) = base.find('@') {
            (&base[..at_idx], &base[at_idx + 1..])
        } else {
            let slash_idx = base.find('/')?;
            (&base[..slash_idx], &base[slash_idx + 1..])
        };

        let ver = raw_ver.split('_').next().unwrap_or(raw_ver);
        if name.is_empty() || !ver.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        Some((name.to_string(), ver.to_string()))
    }

    /// Parses `package-lock.json` (lockfileVersion 1, 2, and 3) to extract all locked transitive packages.
    pub fn parse_package_lock_json(path: &Path) -> Vec<DependencySpec> {
        Self::parse_package_lock_json_with_options(path, None, true)
    }

    pub fn parse_package_lock_json_with_options(
        path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
    ) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) else {
            return deps;
        };

        // v2/v3 `"packages"` map
        if let Some(packages) = val.get("packages").and_then(|p| p.as_object()) {
            for (pkg_path, meta) in packages {
                if pkg_path.is_empty() || !pkg_path.contains("node_modules/") {
                    continue;
                }
                if meta.get("link").and_then(|l| l.as_bool()) == Some(true) {
                    continue;
                }
                let Some(name) = pkg_path.rsplit("node_modules/").next() else {
                    continue;
                };
                let Some(version) = meta.get("version").and_then(|v| v.as_str()) else {
                    continue;
                };
                if name.is_empty()
                    || version.is_empty()
                    || !Self::is_compatible_npm_platform_pkg_for_target(name, target_platform)
                    || !Self::is_compatible_package_lock_meta(meta, target_platform)
                {
                    continue;
                }
                let is_dev = meta.get("dev").and_then(|d| d.as_bool()).unwrap_or(false)
                    || meta
                        .get("devOptional")
                        .and_then(|d| d.as_bool())
                        .unwrap_or(false);
                Self::upsert_npm_lock_dep(&mut deps, name, version, is_dev);
            }
        }

        // v1 fallback `"dependencies"` map
        if deps.is_empty() {
            if let Some(v1_deps) = val.get("dependencies").and_then(|d| d.as_object()) {
                Self::collect_v1_package_lock_deps(v1_deps, false, target_platform, &mut deps);
            }
        }

        if !include_dev {
            deps.retain(|d| !d.is_dev);
        }
        deps
    }

    fn upsert_npm_lock_dep(
        deps: &mut Vec<DependencySpec>,
        name: &str,
        version: &str,
        is_dev: bool,
    ) {
        if let Some(existing) = deps
            .iter_mut()
            .find(|d| d.name == name && d.version_req == version)
        {
            if !is_dev {
                existing.is_dev = false;
            }
        } else {
            deps.push(DependencySpec {
                name: name.to_string(),
                version_req: version.to_string(),
                ecosystem: Ecosystem::Npm,
                is_dev,
                dependencies: Vec::new(),
            });
        }
    }

    fn collect_v1_package_lock_deps(
        v1_deps: &serde_json::Map<String, serde_json::Value>,
        inherited_dev: bool,
        target_platform: Option<&str>,
        deps: &mut Vec<DependencySpec>,
    ) {
        for (name, meta) in v1_deps {
            let Some(version) = meta.get("version").and_then(|v| v.as_str()) else {
                continue;
            };
            if version.starts_with("file:")
                || !Self::is_compatible_npm_platform_pkg_for_target(name, target_platform)
            {
                continue;
            }
            let is_dev =
                inherited_dev || meta.get("dev").and_then(|d| d.as_bool()).unwrap_or(false);
            Self::upsert_npm_lock_dep(deps, name, version, is_dev);
            if let Some(nested) = meta.get("dependencies").and_then(|d| d.as_object()) {
                Self::collect_v1_package_lock_deps(nested, is_dev, target_platform, deps);
            }
        }
    }

    /// Parses `yarn.lock` (v1 and Berry) to extract locked package names and versions.
    pub fn parse_yarn_lock(path: &Path) -> Vec<DependencySpec> {
        let empty = HashSet::new();
        Self::parse_yarn_lock_with_options(path, None, true, &empty, &empty)
    }

    pub fn parse_yarn_lock_with_options(
        path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        prod_roots: &HashSet<String>,
        dev_roots: &HashSet<String>,
    ) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };

        let mut current_idx: Option<usize> = None;
        let mut current_pkg_name: Option<String> = None;
        let mut in_dep_subblock = false;
        for line in content.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
                continue;
            }
            if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') && trimmed.ends_with(':') {
                current_pkg_name = Self::extract_yarn_entry_name(trimmed);
                current_idx = None;
                in_dep_subblock = false;
            } else {
                let inner = trimmed.trim();
                let indent = trimmed.len() - inner.len();
                if indent <= 2 && in_dep_subblock {
                    in_dep_subblock = false;
                }
                if inner == "dependencies:" || inner == "optionalDependencies:" {
                    in_dep_subblock = true;
                    continue;
                }
                if in_dep_subblock {
                    if let Some(idx) = current_idx {
                        let dep_name = inner
                            .split([' ', ':'])
                            .next()
                            .unwrap_or("")
                            .trim_matches('"')
                            .trim_matches('\'');
                        if !dep_name.is_empty() {
                            if let Some(spec) = deps.get_mut(idx) {
                                if !spec.dependencies.iter().any(|d| d == dep_name) {
                                    spec.dependencies.push(dep_name.to_string());
                                }
                            }
                        }
                    }
                    continue;
                }
                if let Some(ref name) = current_pkg_name {
                    let ver_opt = inner
                        .strip_prefix("version \"")
                        .and_then(|s| s.strip_suffix('"'))
                        .or_else(|| {
                            inner
                                .strip_prefix("version: ")
                                .map(|s| s.trim_matches('"').trim_matches('\''))
                        });
                    if let Some(version) = ver_opt {
                        if !version.is_empty()
                            && Self::is_compatible_npm_platform_pkg_for_target(
                                name,
                                target_platform,
                            )
                        {
                            if let Some(pos) = deps.iter().position(|d: &DependencySpec| {
                                &d.name == name && d.version_req == version
                            }) {
                                current_idx = Some(pos);
                            } else {
                                deps.push(DependencySpec {
                                    name: name.clone(),
                                    version_req: version.to_string(),
                                    ecosystem: Ecosystem::Npm,
                                    is_dev: false,
                                    dependencies: Vec::new(),
                                });
                                current_idx = Some(deps.len() - 1);
                            }
                        }
                    }
                }
            }
        }

        Self::prune_yarn_dev_deps(&mut deps, prod_roots, dev_roots, include_dev);
        deps
    }

    fn extract_yarn_entry_name(header: &str) -> Option<String> {
        let first_spec = header
            .trim_end_matches(':')
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        let pkg_name = if let Some(rest) = first_spec.strip_prefix('@') {
            let slash_idx = rest.find('/')?;
            let after_scope = rest.get(slash_idx + 1..)?;
            let at_idx = after_scope.find('@')?;
            first_spec.get(..1 + slash_idx + 1 + at_idx)
        } else {
            first_spec.split_once('@').map(|(name, _)| name)
        };
        pkg_name.map(str::to_string)
    }

    fn prune_yarn_dev_deps(
        deps: &mut Vec<DependencySpec>,
        prod_roots: &HashSet<String>,
        dev_roots: &HashSet<String>,
        include_dev: bool,
    ) {
        let has_edges = deps.iter().any(|d| !d.dependencies.is_empty());
        if has_edges && !prod_roots.is_empty() {
            let mut reachable = HashSet::new();
            let mut queue = VecDeque::new();
            for root in prod_roots {
                if reachable.insert(root.clone()) {
                    queue.push_back(root.clone());
                }
            }
            while let Some(name) = queue.pop_front() {
                for spec in deps.iter().filter(|d| d.name == name) {
                    for edge in &spec.dependencies {
                        if reachable.insert(edge.clone()) {
                            queue.push_back(edge.clone());
                        }
                    }
                }
            }
            for dep in deps.iter_mut() {
                if !reachable.contains(&dep.name) {
                    dep.is_dev = true;
                }
            }
        } else if !dev_roots.is_empty() {
            for dep in deps.iter_mut() {
                if dev_roots.contains(&dep.name) && !prod_roots.contains(&dep.name) {
                    dep.is_dev = true;
                }
            }
        }
        if !include_dev {
            deps.retain(|d| !d.is_dev);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PnpmSection {
    Other,
    PackagesOrSnapshots,
    Importers,
    RootDeps { is_dev: bool },
}

struct PnpmLockParser<'a> {
    deps: &'a mut Vec<DependencySpec>,
    target_platform: Option<&'a str>,
    section: PnpmSection,
    has_root_graph: bool,
    pnpm_prod_roots: HashSet<(String, String)>,
    current_entry: Option<(usize, usize)>,
    deps_block_indent: Option<usize>,
    importer_deps_indent: Option<(usize, bool)>,
    pending_root_dep_name: Option<(String, usize, bool)>,
}

impl<'a> PnpmLockParser<'a> {
    fn new(deps: &'a mut Vec<DependencySpec>, target_platform: Option<&'a str>) -> Self {
        Self {
            deps,
            target_platform,
            section: PnpmSection::Other,
            has_root_graph: false,
            pnpm_prod_roots: HashSet::new(),
            current_entry: None,
            deps_block_indent: None,
            importer_deps_indent: None,
            pending_root_dep_name: None,
        }
    }

    fn process_line(&mut self, line: &str) {
        let trimmed_end = line.trim_end();
        if trimmed_end.is_empty() || trimmed_end.trim_start().starts_with('#') {
            return;
        }
        if !trimmed_end.starts_with(' ') && !trimmed_end.starts_with('\t') {
            self.section = match trimmed_end {
                "packages:" | "snapshots:" => PnpmSection::PackagesOrSnapshots,
                "importers:" => {
                    self.has_root_graph = true;
                    PnpmSection::Importers
                }
                "dependencies:" | "optionalDependencies:" => {
                    self.has_root_graph = true;
                    PnpmSection::RootDeps { is_dev: false }
                }
                "devDependencies:" => {
                    self.has_root_graph = true;
                    PnpmSection::RootDeps { is_dev: true }
                }
                _ => PnpmSection::Other,
            };
            self.current_entry = None;
            self.deps_block_indent = None;
            self.importer_deps_indent = None;
            self.pending_root_dep_name = None;
            return;
        }

        let indent = trimmed_end.len() - trimmed_end.trim_start().len();
        let trimmed = trimmed_end.trim();

        match self.section {
            PnpmSection::PackagesOrSnapshots => self.process_pkg_line(trimmed_end, trimmed, indent),
            PnpmSection::Importers => self.process_importer_line(trimmed, indent),
            PnpmSection::RootDeps { is_dev } => self.process_root_dep_line(trimmed, indent, is_dev),
            PnpmSection::Other => {}
        }
    }

    fn process_importer_line(&mut self, trimmed: &str, indent: usize) {
        if let Some((block_indent, _)) = self.importer_deps_indent {
            if indent <= block_indent {
                self.importer_deps_indent = None;
                self.pending_root_dep_name = None;
            } else {
                let (_, is_dev) = self.importer_deps_indent.unwrap_or((0, false));
                self.process_root_dep_line(trimmed, indent, is_dev);
                return;
            }
        }
        if trimmed == "dependencies:" || trimmed == "optionalDependencies:" {
            self.importer_deps_indent = Some((indent, false));
        } else if trimmed == "devDependencies:" {
            self.importer_deps_indent = Some((indent, true));
        }
    }

    fn process_root_dep_line(&mut self, trimmed: &str, indent: usize, is_dev: bool) {
        if let Some((ref pkg_name, pkg_indent, pkg_is_dev)) = self.pending_root_dep_name.clone() {
            if indent > pkg_indent {
                if let Some(raw_ver) = trimmed.strip_prefix("version:") {
                    if !pkg_is_dev {
                        let clean_ver = Self::clean_pnpm_dep_ver(raw_ver);
                        if !clean_ver.is_empty() {
                            self.pnpm_prod_roots.insert((pkg_name.clone(), clean_ver));
                        }
                    }
                }
                return;
            }
            self.pending_root_dep_name = None;
        }

        if let Some((raw_name, raw_val)) = trimmed.split_once(':') {
            let pkg_name = raw_name.trim().trim_matches('\'').trim_matches('"');
            let val = raw_val.trim();
            if pkg_name.is_empty() {
                return;
            }
            if val.is_empty() || val == "{}" {
                self.pending_root_dep_name = Some((pkg_name.to_string(), indent, is_dev));
            } else if !is_dev {
                let clean_ver = Self::clean_pnpm_dep_ver(val);
                if !clean_ver.is_empty() {
                    self.pnpm_prod_roots
                        .insert((pkg_name.to_string(), clean_ver));
                }
            }
        }
    }

    fn process_pkg_line(&mut self, trimmed_end: &str, trimmed: &str, indent: usize) {
        if let Some(dep_indent) = self.deps_block_indent {
            if indent > dep_indent {
                if let Some((idx, _)) = self.current_entry {
                    if let Some((raw_dep_name, raw_dep_ver)) = trimmed.split_once(':') {
                        let dep_name = raw_dep_name
                            .trim()
                            .trim_matches('\'')
                            .trim_matches('"')
                            .to_string();
                        let ver_clean = Self::clean_pnpm_dep_ver(raw_dep_ver);
                        if !dep_name.is_empty() && !ver_clean.is_empty() {
                            let edge = format!("{}@{}", dep_name, ver_clean);
                            if let Some(spec) = self.deps.get_mut(idx) {
                                if !spec.dependencies.contains(&edge) {
                                    spec.dependencies.push(edge);
                                }
                            }
                        }
                    }
                }
                return;
            }
            self.deps_block_indent = None;
        }

        if let Some((idx, entry_indent)) = self.current_entry {
            if indent > entry_indent {
                if trimmed == "dependencies:" || trimmed == "optionalDependencies:" {
                    self.deps_block_indent = Some(indent);
                } else if trimmed == "dev: true" {
                    if let Some(spec) = self.deps.get_mut(idx) {
                        spec.is_dev = true;
                    }
                }
                return;
            }
        }

        let is_entry = trimmed_end.ends_with(':') || trimmed_end.ends_with(": {}");
        if !(2..=4).contains(&indent) || !is_entry {
            self.current_entry = None;
            self.deps_block_indent = None;
            return;
        }

        let key = trimmed
            .trim_end_matches(": {}")
            .trim_end_matches(':')
            .trim_matches('\'')
            .trim_matches('"')
            .trim_start_matches('/');

        if let Some((name, version)) = NpmManifestScanner::parse_pnpm_package_key(key) {
            if !NpmManifestScanner::is_compatible_npm_platform_pkg_for_target(
                &name,
                self.target_platform,
            ) {
                self.current_entry = None;
                self.deps_block_indent = None;
                return;
            }
            if let Some(existing_idx) = self
                .deps
                .iter()
                .position(|d: &DependencySpec| d.name == name && d.version_req == version)
            {
                self.current_entry = Some((existing_idx, indent));
            } else {
                self.deps.push(DependencySpec {
                    name,
                    version_req: version,
                    ecosystem: Ecosystem::Npm,
                    is_dev: false,
                    dependencies: Vec::new(),
                });
                self.current_entry = Some((self.deps.len() - 1, indent));
            }
            self.deps_block_indent = None;
        } else {
            self.current_entry = None;
            self.deps_block_indent = None;
        }
    }

    fn clean_pnpm_dep_ver(raw: &str) -> String {
        let stripped = raw.trim().trim_matches('\'').trim_matches('"');
        stripped
            .split('(')
            .next()
            .unwrap_or(stripped)
            .split('_')
            .next()
            .unwrap_or(stripped)
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_monorepo_subpackage_inherits_ancestor_pnpm_lockfile(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let root = temp.path();
        let desktop = root.join("apps/desktop");
        fs::create_dir_all(&desktop)?;

        fs::write(
            root.join("package.json"),
            r#"{"name": "monorepo", "private": true}"#,
        )?;
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/*'\n",
        )?;
        fs::write(
            root.join("pnpm-lock.yaml"),
            r#"lockfileVersion: '9.0'

importers:

  .: {}

  apps/desktop:
    dependencies:
      shared-ui:
        specifier: workspace:*
        version: link:../shared-ui
    devDependencies:
      vite:
        specifier: ^6.0.0
        version: 6.0.11

packages:

  vite@6.0.11:
    resolution: {integrity: sha512-ddd}
    hasBin: true

snapshots:

  vite@6.0.11: {}
"#,
        )?;
        fs::write(
            desktop.join("package.json"),
            r#"{
  "name": "desktop-app",
  "dependencies": { "shared-ui": "workspace:*" },
  "devDependencies": { "vite": "^6.0.0" }
}"#,
        )?;

        let mut results = Vec::new();
        NpmManifestScanner::scan_with_options(root, None, true, &mut results)?;
        let specs: Vec<_> = results.iter().flat_map(|m| m.dependencies.iter()).collect();
        assert!(
            specs.iter().any(|s| s.name == "vite" && s.version_req == "6.0.11"),
            "Expected subpackage vite to inherit locked version 6.0.11 from root pnpm-lock.yaml: {:?}",
            specs
        );
        assert!(
            !specs.iter().any(|s| s.name == "shared-ui"),
            "Expected local workspace:* dependency to be skipped: {:?}",
            specs
        );
        Ok(())
    }
}
