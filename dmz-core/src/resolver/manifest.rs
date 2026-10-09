use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
    /// Scans a workspace directory and returns all detected project manifests
    pub fn scan_dir(root_path: &Path) -> Result<Vec<ManifestInfo>, ManifestError> {
        let mut results = Vec::new();

        if !root_path.exists() || !root_path.is_dir() {
            return Err(ManifestError::IoError(format!(
                "Directory does not exist: {}",
                root_path.display()
            )));
        }

        // 1. Cargo.toml
        let cargo_path = root_path.join("Cargo.toml");
        if cargo_path.exists() {
            results.push(Self::parse_cargo_toml(&cargo_path)?);
        }

        // 2. package.json
        let package_json_path = root_path.join("package.json");
        if package_json_path.exists() {
            results.push(Self::parse_package_json(&package_json_path)?);
        }

        // 3. requirements.txt
        let req_path = root_path.join("requirements.txt");
        if req_path.exists() {
            results.push(Self::parse_requirements_txt(&req_path)?);
        }

        Ok(results)
    }

    fn parse_cargo_toml(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read Cargo.toml: {}", e)))?;

        let value: serde_json::Value = toml::from_str(&content)
            .map_err(|e| ManifestError::ParseError(format!("Invalid Cargo.toml: {}", e)))?;

        let mut dependencies = Vec::new();

        if let Some(deps) = value.get("dependencies").and_then(|d| d.as_object()) {
            for (name, val) in deps {
                let version_req = match val {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(map) => map
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("*")
                        .to_string(),
                    _ => "*".to_string(),
                };
                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req,
                    ecosystem: Ecosystem::Cargo,
                    is_dev: false,
                });
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::Cargo,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    fn parse_package_json(path: &Path) -> Result<ManifestInfo, ManifestError> {
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

        Ok(ManifestInfo {
            ecosystem: Ecosystem::Npm,
            path: path.to_path_buf(),
            dependencies,
        })
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