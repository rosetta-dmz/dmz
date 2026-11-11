use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

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
            });
        }

        dependencies
    }
}

