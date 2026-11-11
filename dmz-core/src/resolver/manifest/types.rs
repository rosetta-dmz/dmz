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
    NuGet,
    Composer,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySpec {
    pub name: String,
    pub version_req: String,
    pub ecosystem: Ecosystem,
    pub is_dev: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestInfo {
    pub ecosystem: Ecosystem,
    pub path: PathBuf,
    pub dependencies: Vec<DependencySpec>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io(std::io::Error),
    Parse(String),
    IoError(String),
    ParseError(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "Manifest IO Error: {}", err),
            Self::Parse(msg) => write!(f, "Manifest Parse Error: {}", msg),
            Self::IoError(msg) => write!(f, "Manifest IO Error: {}", msg),
            Self::ParseError(msg) => write!(f, "Manifest Parse Error: {}", msg),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ManifestError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

