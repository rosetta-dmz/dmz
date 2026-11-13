use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::fs;
use std::path::Path;

pub struct NpmManifestScanner;

impl NpmManifestScanner {
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        let package_json_path = root_path.join("package.json");
        if package_json_path.exists() {
            results.extend(Self::parse_package_json(&package_json_path)?);
        }
        Ok(())
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
        let mut results = Vec::new();
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read package.json: {}", e)))?;

        let value: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ManifestError::ParseError(format!("Invalid package.json: {}", e)))?;

        let mut dependencies = Vec::new();

        if let Some(deps) = value.get("dependencies").and_then(|d| d.as_object()) {
            for (name, val) in deps {
                let raw_ver = val.as_str().unwrap_or("*");
                let ver = Self::sanitize_npm_version(raw_ver);
                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req: ver,
                    ecosystem: Ecosystem::Npm,
                    is_dev: false,
                    dependencies: Vec::new(),
                });
            }
        }

        if let Some(dev_deps) = value.get("devDependencies").and_then(|d| d.as_object()) {
            for (name, val) in dev_deps {
                let raw_ver = val.as_str().unwrap_or("*");
                let ver = Self::sanitize_npm_version(raw_ver);
                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req: ver,
                    ecosystem: Ecosystem::Npm,
                    is_dev: true,
                    dependencies: Vec::new(),
                });
            }
        }

        let root_dir = path.parent().unwrap_or(path);

        // Merge full transitive closure from pnpm-lock.yaml, package-lock.json, or yarn.lock if present
        let pnpm_lock = root_dir.join("pnpm-lock.yaml");
        let npm_lock = root_dir.join("package-lock.json");
        let yarn_lock = root_dir.join("yarn.lock");

        let locked_npm_deps = if pnpm_lock.is_file() {
            Self::parse_pnpm_lock(&pnpm_lock)
        } else if npm_lock.is_file() {
            Self::parse_package_lock_json(&npm_lock)
        } else if yarn_lock.is_file() {
            Self::parse_yarn_lock(&yarn_lock)
        } else {
            Vec::new()
        };

        if !locked_npm_deps.is_empty() {
            super::merge_locked_dependencies(&mut dependencies, locked_npm_deps);
        }

        results.push(ManifestInfo {
            ecosystem: Ecosystem::Npm,
            path: path.to_path_buf(),
            dependencies,
        });

        let workspaces_array = value
            .get("workspaces")
            .and_then(|w| w.as_array())
            .or_else(|| {
                value
                    .get("workspaces")
                    .and_then(|w| w.get("packages"))
                    .and_then(|p| p.as_array())
            });

        if let Some(patterns) = workspaces_array {
            for pattern in patterns {
                if let Some(pat_str) = pattern.as_str() {
                    let clean_pat = pat_str.trim_end_matches("/*").trim_end_matches('*');
                    let target_dir = root_dir.join(clean_pat);

                    if target_dir.exists() && target_dir.is_dir() {
                        if let Ok(entries) = fs::read_dir(target_dir) {
                            for entry in entries.flatten() {
                                let sub_pkg_json = entry.path().join("package.json");
                                if sub_pkg_json.exists() {
                                    if let Ok(sub_manifests) =
                                        Self::parse_package_json(&sub_pkg_json)
                                    {
                                        results.extend(sub_manifests);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Filters out platform-specific optional npm packages meant for foreign OS/CPU architectures.
    pub fn is_compatible_npm_platform_pkg(name: &str) -> bool {
        let lower = name.to_lowercase();
        let host_os = std::env::consts::OS;
        let host_arch = std::env::consts::ARCH;

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
            let os_ok = match host_os {
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
                let arch_ok = match host_arch {
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

    /// Parses `pnpm-lock.yaml` (supporting lockfileVersion 5, 6, and 9) to extract all locked transitive packages.
    pub fn parse_pnpm_lock(path: &Path) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };

        let mut in_packages_or_snapshots = false;
        let mut current_entry: Option<(usize, usize)> = None; // (deps vec idx, entry indent)
        let mut deps_block_indent: Option<usize> = None;

        for line in content.lines() {
            let trimmed_end = line.trim_end();
            if trimmed_end.is_empty() || trimmed_end.trim_start().starts_with('#') {
                continue;
            }
            // Top-level YAML section header (0 leading spaces)
            if !trimmed_end.starts_with(' ') && !trimmed_end.starts_with('\t') {
                in_packages_or_snapshots =
                    trimmed_end == "packages:" || trimmed_end == "snapshots:";
                current_entry = None;
                deps_block_indent = None;
                continue;
            }
            if !in_packages_or_snapshots {
                continue;
            }

            let indent = trimmed_end.len() - trimmed_end.trim_start().len();
            let trimmed = trimmed_end.trim();

            if let Some(dep_indent) = deps_block_indent {
                if indent > dep_indent {
                    if let Some((idx, _)) = current_entry {
                        if let Some((raw_dep_name, raw_dep_ver)) = trimmed.split_once(':') {
                            let dep_name = raw_dep_name
                                .trim()
                                .trim_matches('\'')
                                .trim_matches('"')
                                .to_string();
                            let raw_ver = raw_dep_ver.trim().trim_matches('\'').trim_matches('"');
                            let ver_clean = raw_ver
                                .split('(')
                                .next()
                                .unwrap_or(raw_ver)
                                .split('_')
                                .next()
                                .unwrap_or(raw_ver)
                                .trim();
                            if !dep_name.is_empty() && !ver_clean.is_empty() {
                                let edge = format!("{}@{}", dep_name, ver_clean);
                                if let Some(spec) = deps.get_mut(idx) {
                                    if !spec.dependencies.contains(&edge) {
                                        spec.dependencies.push(edge);
                                    }
                                }
                            }
                        }
                    }
                    continue;
                } else {
                    deps_block_indent = None;
                }
            }

            if let Some((_, entry_indent)) = current_entry {
                if indent > entry_indent {
                    if trimmed == "dependencies:" || trimmed == "optionalDependencies:" {
                        deps_block_indent = Some(indent);
                    }
                    continue;
                }
            }

            let is_entry = trimmed_end.ends_with(':') || trimmed_end.ends_with(": {}");
            if !(2..=4).contains(&indent) || !is_entry {
                current_entry = None;
                deps_block_indent = None;
                continue;
            }

            let key = trimmed
                .trim_end_matches(": {}")
                .trim_end_matches(':')
                .trim_matches('\'')
                .trim_matches('"')
                .trim_start_matches('/');

            if let Some((name, version)) = Self::parse_pnpm_package_key(key) {
                if !Self::is_compatible_npm_platform_pkg(&name) {
                    current_entry = None;
                    deps_block_indent = None;
                    continue;
                }
                if let Some(existing_idx) = deps
                    .iter()
                    .position(|d: &DependencySpec| d.name == name && d.version_req == version)
                {
                    current_entry = Some((existing_idx, indent));
                } else {
                    deps.push(DependencySpec {
                        name,
                        version_req: version,
                        ecosystem: Ecosystem::Npm,
                        is_dev: false,
                        dependencies: Vec::new(),
                    });
                    current_entry = Some((deps.len() - 1, indent));
                }
                deps_block_indent = None;
            } else {
                current_entry = None;
                deps_block_indent = None;
            }
        }

        deps
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
        let base = key.split('(').next().unwrap_or(key);
        let base = base.split('_').next().unwrap_or(base);

        let (name, ver) = if let Some(rest) = base.strip_prefix('@') {
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

        if name.is_empty() || !ver.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        Some((name.to_string(), ver.to_string()))
    }

    /// Parses `package-lock.json` (lockfileVersion 1, 2, and 3) to extract all locked transitive packages.
    pub fn parse_package_lock_json(path: &Path) -> Vec<DependencySpec> {
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
                    || !Self::is_compatible_npm_platform_pkg(name)
                {
                    continue;
                }
                if !deps
                    .iter()
                    .any(|d: &DependencySpec| d.name == name && d.version_req == version)
                {
                    deps.push(DependencySpec {
                        name: name.to_string(),
                        version_req: version.to_string(),
                        ecosystem: Ecosystem::Npm,
                        is_dev: meta.get("dev").and_then(|d| d.as_bool()).unwrap_or(false),
                        dependencies: Vec::new(),
                    });
                }
            }
        }

        // v1 fallback `"dependencies"` map
        if deps.is_empty() {
            if let Some(v1_deps) = val.get("dependencies").and_then(|d| d.as_object()) {
                for (name, meta) in v1_deps {
                    if let Some(version) = meta.get("version").and_then(|v| v.as_str()) {
                        if Self::is_compatible_npm_platform_pkg(name)
                            && !version.starts_with("file:")
                        {
                            deps.push(DependencySpec {
                                name: name.clone(),
                                version_req: version.to_string(),
                                ecosystem: Ecosystem::Npm,
                                is_dev: false,
                                dependencies: Vec::new(),
                            });
                        }
                    }
                }
            }
        }

        deps
    }

    /// Parses `yarn.lock` (v1 and Berry) to extract locked package names and versions.
    pub fn parse_yarn_lock(path: &Path) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };

        let mut current_pkg_name: Option<String> = None;
        for line in content.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
                continue;
            }
            if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') && trimmed.ends_with(':') {
                let first_spec = trimmed
                    .trim_end_matches(':')
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                let pkg_name = if let Some(rest) = first_spec.strip_prefix('@') {
                    if let Some(slash_idx) = rest.find('/') {
                        let after_scope = &rest[slash_idx + 1..];
                        after_scope
                            .find('@')
                            .map(|at_idx| &first_spec[..1 + slash_idx + 1 + at_idx])
                    } else {
                        None
                    }
                } else {
                    first_spec.find('@').map(|at_idx| &first_spec[..at_idx])
                };
                current_pkg_name = pkg_name.map(|s| s.to_string());
            } else if let Some(ref name) = current_pkg_name {
                let inner = trimmed.trim();
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
                        && Self::is_compatible_npm_platform_pkg(name)
                        && !deps
                            .iter()
                            .any(|d: &DependencySpec| &d.name == name && d.version_req == version)
                    {
                        deps.push(DependencySpec {
                            name: name.clone(),
                            version_req: version.to_string(),
                            ecosystem: Ecosystem::Npm,
                            is_dev: false,
                            dependencies: Vec::new(),
                        });
                    }
                    current_pkg_name = None;
                }
            }
        }

        deps
    }
}
