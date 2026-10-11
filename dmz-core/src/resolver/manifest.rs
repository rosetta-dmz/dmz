use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ecosystem {
    Cargo,
    Npm,
    PyPI,
    Go,
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencySpec {
    pub name: String,
    pub version_req: String,
    pub ecosystem: Ecosystem,
    pub is_dev: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestInfo {
    pub ecosystem: Ecosystem,
    pub path: PathBuf,
    pub dependencies: Vec<DependencySpec>,
}

#[derive(Debug)]
pub enum ManifestError {
    IoError(String),
    ParseError(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Manifest IO Error: {}", msg),
            Self::ParseError(msg) => write!(f, "Manifest Parse Error: {}", msg),
        }
    }
}

impl std::error::Error for ManifestError {}

pub struct ManifestScanner;

impl ManifestScanner {
    pub fn scan_dir(root_path: &Path) -> Result<Vec<ManifestInfo>, ManifestError> {
        let mut results = Vec::new();

        if !root_path.exists() || !root_path.is_dir() {
            return Err(ManifestError::IoError(format!(
                "Directory does not exist: {}",
                root_path.display()
            )));
        }

        let cargo_path = root_path.join("Cargo.toml");
        if cargo_path.exists() {
            let content = fs::read_to_string(&cargo_path)
                .map_err(|e| ManifestError::IoError(format!("Failed to read root Cargo.toml: {}", e)))?;
            
            // Use native toml::Value instead of serde_json::Value
            let root_val: toml::Value = toml::from_str(&content)
                .map_err(|e| ManifestError::ParseError(format!("Invalid root Cargo.toml: {}", e)))?;

            // 1. Extract workspace dependencies table from [workspace.dependencies]
            let mut workspace_deps = HashMap::new();
            if let Some(ws_deps) = root_val.get("workspace")
                .and_then(|w| w.get("dependencies"))
                .and_then(|d| d.as_table()) 
            {
                for (name, val) in ws_deps {
                    workspace_deps.insert(name.clone(), val.clone());
                }
            }

            // 2. Parse root dependencies if present under [dependencies]
            let root_deps = Self::extract_dependencies(root_val.get("dependencies"), &workspace_deps);
            if !root_deps.is_empty() {
                results.push(ManifestInfo {
                    ecosystem: Ecosystem::Cargo,
                    path: cargo_path.clone(),
                    dependencies: root_deps,
                });
            }

            // 3. Extract workspace members and scan them recursively
            if let Some(members) = root_val.get("workspace")
                .and_then(|w| w.get("members"))
                .and_then(|m| m.as_array()) 
            {
                for member in members {
                    if let Some(member_str) = member.as_str() {
                        let member_cargo_path = root_path.join(member_str).join("Cargo.toml");
                        if member_cargo_path.exists() {
                            let member_content = fs::read_to_string(&member_cargo_path)
                                .map_err(|e| ManifestError::IoError(format!("Failed to read member Cargo.toml: {}", e)))?;
                            
                            let member_val: toml::Value = toml::from_str(&member_content)
                                .map_err(|e| ManifestError::ParseError(format!("Invalid member Cargo.toml: {}", e)))?;

                            let member_deps = Self::extract_dependencies(member_val.get("dependencies"), &workspace_deps);
                            
                            tracing::info!(member = %member_str, dep_count = member_deps.len(), "Scanned workspace member manifest");

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

        // 4. package.json and requirements.txt checks...
        let package_json_path = root_path.join("package.json");
        if package_json_path.exists() {
            results.extend(Self::parse_package_json(&package_json_path)?);
        }

        let req_path = root_path.join("requirements.txt");
        if req_path.exists() {
            results.push(Self::parse_requirements_txt(&req_path)?);
        }

        Ok(results)
    }

    // Helper to extract dependencies and handle `workspace = true` inheritance using toml::Value
    fn extract_dependencies(
        deps_value: Option<&toml::Value>,
        workspace_deps: &HashMap<String, toml::Value>,
    ) -> Vec<DependencySpec> {
        let mut dependencies = Vec::new();

        let Some(deps_table) = deps_value.and_then(|d| d.as_table()) else {
            return dependencies;
        };

        for (name, val) in deps_table {
            let mut resolved_val = val;

            // Check if dependency uses workspace inheritance: e.g., tokio = { workspace = true }
            if let Some(map) = val.as_table() {
                if map.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                    if let Some(ws_val) = workspace_deps.get(name) {
                        resolved_val = ws_val;
                    }
                }
            }

            let version_req = match resolved_val {
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

            dependencies.push(DependencySpec {
                name: name.clone(),
                version_req,
                ecosystem: Ecosystem::Cargo,
                is_dev: false,
            });
        }

        dependencies
    }

    fn parse_package_json(path: &Path) -> Result<Vec<ManifestInfo>, ManifestError> {
        let mut results = Vec::new();
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read package.json: {}", e)))?;

        let value: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ManifestError::ParseError(format!("Invalid package.json: {}", e)))?;

        let mut dependencies = Vec::new();

        if let Some(deps) = value.get("dependencies").and_then(|d| d.as_object()) {
            for (name, val) in deps {
                let ver = val.as_str().unwrap_or("*").to_string();
                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req: ver,
                    ecosystem: Ecosystem::Npm,
                    is_dev: false,
                });
            }
        }

        if let Some(dev_deps) = value.get("devDependencies").and_then(|d| d.as_object()) {
            for (name, val) in dev_deps {
                let ver = val.as_str().unwrap_or("*").to_string();
                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req: ver,
                    ecosystem: Ecosystem::Npm,
                    is_dev: true,
                });
            }
        }

        let root_dir = path.parent().unwrap_or(path);
        results.push(ManifestInfo {
            ecosystem: Ecosystem::Npm,
            path: path.to_path_buf(),
            dependencies,
        });

        // --- WORKSPACE AWARENESS FOR NPM/YARN/PNPM ---
        let workspaces_array = value.get("workspaces")
            .and_then(|w| w.as_array())
            .or_else(|| value.get("workspaces").and_then(|w| w.get("packages")).and_then(|p| p.as_array()));

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
                                    if let Ok(sub_manifests) = Self::parse_package_json(&sub_pkg_json) {
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

    fn parse_requirements_txt(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read requirements.txt: {}", e)))?;

        let mut dependencies = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.split("==").collect();
            let name = parts[0].trim().to_string();
            let version_req = if parts.len() > 1 {
                parts[1].trim().to_string()
            } else {
                "*".to_string()
            };

            dependencies.push(DependencySpec {
                name,
                version_req,
                ecosystem: Ecosystem::PyPI,
                is_dev: false,
            });
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::PyPI,
            path: path.to_path_buf(),
            dependencies,
        })
    }
}