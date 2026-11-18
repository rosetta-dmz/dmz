use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct CargoManifestScanner;

impl CargoManifestScanner {
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        let cargo_path = root_path.join("Cargo.toml");
        if cargo_path.exists() {
            let content = fs::read_to_string(&cargo_path).map_err(|e| {
                ManifestError::IoError(format!("Failed to read root Cargo.toml: {}", e))
            })?;

            let root_val: toml::Value = toml::from_str(&content).map_err(|e| {
                ManifestError::ParseError(format!("Invalid root Cargo.toml: {}", e))
            })?;

            let mut workspace_deps = HashMap::new();
            if let Some(ws_deps) = root_val
                .get("workspace")
                .and_then(|w| w.get("dependencies"))
                .and_then(|d| d.as_table())
            {
                for (name, val) in ws_deps {
                    workspace_deps.insert(name.clone(), val.clone());
                }
            }

            let root_deps =
                Self::extract_dependencies(root_val.get("dependencies"), &workspace_deps);
            if !root_deps.is_empty() {
                results.push(ManifestInfo {
                    ecosystem: Ecosystem::Cargo,
                    path: cargo_path.clone(),
                    dependencies: root_deps,
                });
            }

            if let Some(members) = root_val
                .get("workspace")
                .and_then(|w| w.get("members"))
                .and_then(|m| m.as_array())
            {
                for member in members {
                    if let Some(member_str) = member.as_str() {
                        let member_cargo_path = root_path.join(member_str).join("Cargo.toml");
                        if member_cargo_path.exists() {
                            let member_content =
                                fs::read_to_string(&member_cargo_path).map_err(|e| {
                                    ManifestError::IoError(format!(
                                        "Failed to read member Cargo.toml: {}",
                                        e
                                    ))
                                })?;

                            let member_val: toml::Value =
                                toml::from_str(&member_content).map_err(|e| {
                                    ManifestError::ParseError(format!(
                                        "Invalid member Cargo.toml: {}",
                                        e
                                    ))
                                })?;

                            let member_deps = Self::extract_dependencies(
                                member_val.get("dependencies"),
                                &workspace_deps,
                            );

                            tracing::info!(
                                member = %member_str,
                                dep_count = member_deps.len(),
                                "Scanned workspace member manifest"
                            );

                            results.push(ManifestInfo {
                                ecosystem: Ecosystem::Cargo,
                                path: member_cargo_path,
                                dependencies: member_deps,
                            });
                        }
                    }
                }
            }
        }

        let tauri_cargo_path = root_path.join("src-tauri").join("Cargo.toml");
        if tauri_cargo_path.exists() && !results.iter().any(|m| m.path == tauri_cargo_path) {
            let tauri_content = fs::read_to_string(&tauri_cargo_path).map_err(|e| {
                ManifestError::IoError(format!("Failed to read src-tauri/Cargo.toml: {}", e))
            })?;
            let tauri_val: toml::Value = toml::from_str(&tauri_content).map_err(|e| {
                ManifestError::ParseError(format!("Invalid src-tauri/Cargo.toml: {}", e))
            })?;
            let tauri_deps =
                Self::extract_dependencies(tauri_val.get("dependencies"), &HashMap::new());
            if !tauri_deps.is_empty() {
                results.push(ManifestInfo {
                    ecosystem: Ecosystem::Cargo,
                    path: tauri_cargo_path,
                    dependencies: tauri_deps,
                });
            }
        }

        Ok(())
    }

    pub fn extract_dependencies(
        deps_value: Option<&toml::Value>,
        workspace_deps: &HashMap<String, toml::Value>,
    ) -> Vec<DependencySpec> {
        let mut dependencies = Vec::new();

        let Some(deps_table) = deps_value.and_then(|d| d.as_table()) else {
            return dependencies;
        };

        for (name, val) in deps_table {
            let mut resolved_val = val;

            if let Some(map) = val.as_table() {
                if map.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                    if let Some(ws_val) = workspace_deps.get(name) {
                        resolved_val = ws_val;
                    }
                }
            }

            let raw_version_req = match resolved_val {
                toml::Value::String(s) => s.clone(),
                toml::Value::Table(map) => {
                    if let Some(path_str) = map.get("path").and_then(|p| p.as_str()) {
                        format!("path:{}", path_str)
                    } else {
                        map.get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("*")
                            .to_string()
                    }
                }
                _ => "*".to_string(),
            };

            let version_req = if raw_version_req.starts_with("path:") {
                raw_version_req
            } else {
                raw_version_req
                    .trim_start_matches('=')
                    .trim_start_matches('^')
                    .trim_start_matches('~')
                    .trim()
                    .to_string()
            };

            dependencies.push(DependencySpec {
                name: name.clone(),
                version_req,
                ecosystem: Ecosystem::Cargo,
                is_dev: false,
                dependencies: Vec::new(),
            });
        }

        dependencies
    }

    /// Collects external (non-`path:`) Cargo crate names directly declared in `cargo_dir/Cargo.toml`
    /// and any declared `[workspace].members`, across `[dependencies]`, `[build-dependencies]`,
    /// target-specific tables, and optionally `[dev-dependencies]`.
    pub fn collect_direct_crate_names(cargo_dir: &Path, include_dev: bool) -> Vec<String> {
        Self::collect_direct_crate_names_with_targets(cargo_dir, include_dev, true)
    }

    /// Variant of [`Self::collect_direct_crate_names`] allowing callers with a single
    /// `--target` lockfile to validate unconditional dependencies without false-positiving
    /// on foreign `[target.<cfg>.dependencies]` tables.
    pub fn collect_direct_crate_names_with_targets(
        cargo_dir: &Path,
        include_dev: bool,
        include_target_tables: bool,
    ) -> Vec<String> {
        let mut names = Vec::new();
        let mut manifests = vec![cargo_dir.to_path_buf()];
        let mut ws_members: Vec<PathBuf> =
            crate::workspace::discovery::discover_cargo_workspace_member_dirs(cargo_dir)
                .into_iter()
                .collect();
        ws_members.sort();
        for m in ws_members {
            if !manifests.contains(&m) {
                manifests.push(m);
            }
        }

        let mut workspace_deps = Self::load_workspace_dependencies(cargo_dir);
        for manifest_dir in manifests {
            let cargo_toml = manifest_dir.join("Cargo.toml");
            let Ok(content) = fs::read_to_string(&cargo_toml) else {
                continue;
            };
            let Ok(root_val) = toml::from_str::<toml::Value>(&content) else {
                continue;
            };
            Self::merge_workspace_deps(&root_val, &mut workspace_deps);

            let mut push_table = |tbl: Option<&toml::Value>| {
                for dep in Self::extract_dependencies(tbl, &workspace_deps) {
                    if !dep.version_req.starts_with("path:") && !names.contains(&dep.name) {
                        names.push(dep.name);
                    }
                }
            };

            push_table(root_val.get("dependencies"));
            push_table(root_val.get("build-dependencies"));
            if include_dev {
                push_table(root_val.get("dev-dependencies"));
            }
            if include_target_tables {
                if let Some(targets) = root_val.get("target").and_then(|t| t.as_table()) {
                    for target_tbl in targets.values() {
                        push_table(target_tbl.get("dependencies"));
                        push_table(target_tbl.get("build-dependencies"));
                        if include_dev {
                            push_table(target_tbl.get("dev-dependencies"));
                        }
                    }
                }
            }
        }
        names
    }

    fn merge_workspace_deps(
        toml_val: &toml::Value,
        workspace_deps: &mut HashMap<String, toml::Value>,
    ) {
        if let Some(ws_deps) = toml_val
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(|d| d.as_table())
        {
            for (name, val) in ws_deps {
                workspace_deps
                    .entry(name.clone())
                    .or_insert_with(|| val.clone());
            }
        }
    }

    fn load_workspace_dependencies(cargo_dir: &Path) -> HashMap<String, toml::Value> {
        let mut deps = HashMap::new();
        let mut cursor = Some(cargo_dir);
        while let Some(dir) = cursor {
            let candidate = dir.join("Cargo.toml");
            if candidate.is_file() {
                if let Ok(content) = fs::read_to_string(&candidate) {
                    if let Ok(parsed) = toml::from_str::<toml::Value>(&content) {
                        Self::merge_workspace_deps(&parsed, &mut deps);
                        if parsed.get("workspace").is_some() {
                            break;
                        }
                    }
                }
            }
            cursor = dir.parent();
        }
        deps
    }

    /// Reads crates.io/registry `[[package]]` entries from `cargo_dir/Cargo.lock`
    /// (or an enclosing workspace `Cargo.lock` if present).
    pub fn collect_lockfile_registry_packages(cargo_dir: &Path) -> Vec<CargoLockEntry> {
        let mut lock_path = cargo_dir.join("Cargo.lock");
        if !lock_path.is_file() {
            let mut cursor = cargo_dir.parent();
            while let Some(dir) = cursor {
                let cand = dir.join("Cargo.lock");
                if cand.is_file() && dir.join("Cargo.toml").is_file() {
                    lock_path = cand;
                    break;
                }
                cursor = dir.parent();
            }
        }
        if !lock_path.is_file() {
            return Vec::new();
        }
        let Ok(content) = fs::read_to_string(&lock_path) else {
            return Vec::new();
        };
        let Ok(parsed) = content.parse::<toml::Table>() else {
            return Vec::new();
        };
        let local_pkg_name = fs::read_to_string(cargo_dir.join("Cargo.toml"))
            .ok()
            .and_then(|c| c.parse::<toml::Table>().ok())
            .and_then(|t| {
                t.get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .map(ToOwned::to_owned)
            });

        let mut entries = Vec::new();
        if let Some(packages) = parsed.get("package").and_then(|p| p.as_array()) {
            for p in packages {
                let Some(name) = p.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                let Some(version) = p.get("version").and_then(|v| v.as_str()) else {
                    continue;
                };
                let source = p.get("source").and_then(|s| s.as_str());
                let checksum = p.get("checksum").and_then(|c| c.as_str());
                let is_registry = source.is_some_and(|s| s.starts_with("registry+"))
                    || (source.is_none()
                        && checksum.is_some()
                        && local_pkg_name.as_deref() != Some(name));
                if !is_registry {
                    continue;
                }
                entries.push(CargoLockEntry {
                    name: name.to_string(),
                    version: version.to_string(),
                    checksum: checksum.map(ToOwned::to_owned),
                });
            }
        }
        entries
    }
}

/// Parsed registry crate entry from a project `Cargo.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoLockEntry {
    pub name: String,
    pub version: String,
    pub checksum: Option<String>,
}
