use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ecosystem {
    Cargo,
    Npm,
    PyPI,
    Go,
    MavenGradle,
    Bundler,
    Composer,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySpec {
    pub name: String,
    pub version_req: String,
    pub ecosystem: Ecosystem,
    pub is_dev: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestInfo {
    pub ecosystem: Ecosystem,
    pub path: PathBuf,
    pub dependencies: Vec<DependencySpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("Manifest IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Manifest Parse Error: {0}")]
    Parse(String),
    #[error("Manifest IO Error: {0}")]
    IoError(String),
    #[error("Manifest Parse Error: {0}")]
    ParseError(String),
}
