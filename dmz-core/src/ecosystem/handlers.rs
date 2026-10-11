use std::path::{Path, PathBuf};
use super::{EcosystemHandler, CommandSpec};

pub struct RustEcosystem;
impl EcosystemHandler for RustEcosystem {
    fn name(&self) -> &str { "rust" }
    fn detect(&self, root: &Path) -> bool { root.join("Cargo.toml").exists() }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join(".cargo"), root.join("target")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec { program: "cargo".into(), args: vec!["fetch".into()] })
    }
}

pub struct NodeEcosystem;
impl EcosystemHandler for NodeEcosystem {
    fn name(&self) -> &str { "node" }
    fn detect(&self, root: &Path) -> bool { root.join("package.json").exists() }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec { program: "npm".into(), args: vec!["ci".into()] })
    }
}

pub struct GoEcosystem;
impl EcosystemHandler for GoEcosystem {
    fn name(&self) -> &str { "go" }
    fn detect(&self, root: &Path) -> bool { root.join("go.mod").exists() }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec { program: "go".into(), args: vec!["mod".into(), "download".into()] })
    }
}

pub struct PythonEcosystem;
impl EcosystemHandler for PythonEcosystem {
    fn name(&self) -> &str { "python" }
    fn detect(&self, root: &Path) -> bool {
        root.join("pyproject.toml").exists() || root.join("requirements.txt").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join(".venv"), root.join("__pycache__")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec { program: "pip".into(), args: vec!["install".into(), "--no-deps".into(), "-r".into(), "requirements.txt".into()] })
    }
}