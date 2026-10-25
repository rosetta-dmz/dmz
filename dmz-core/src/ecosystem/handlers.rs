use super::{CommandSpec, EcosystemHandler};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct RustEcosystem;
impl EcosystemHandler for RustEcosystem {
    fn name(&self) -> &str {
        "rust"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("Cargo.toml").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join(".cargo"), root.join("target")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "cargo".into(),
            args: vec!["generate-lockfile".into()],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "cargo".into(),
            args: vec!["check".into(), "--offline".into()],
        })
    }
    fn materialize(
        &self,
        workspace_root: &Path,
        offline_store: &Path,
        lockfile_path: &Path,
    ) -> Result<()> {
        let materializer = crate::install::rust_vendor::RustVendorMaterializer::new(
            workspace_root.to_path_buf(),
            offline_store.to_path_buf(),
        );
        materializer.materialize(lockfile_path)?;
        Ok(())
    }
}

pub struct PnpmEcosystem;
impl EcosystemHandler for PnpmEcosystem {
    fn name(&self) -> &str {
        "pnpm"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("package.json").exists() && root.join("pnpm-lock.yaml").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules"), root.join(".pnpm-store")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "pnpm".into(),
            args: vec![
                "install".into(),
                "--frozen-lockfile".into(),
                "--offline".into(),
            ],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "pnpm".into(),
            args: vec!["test".into(), "--offline".into()],
        })
    }
}

pub struct YarnEcosystem;
impl EcosystemHandler for YarnEcosystem {
    fn name(&self) -> &str {
        "yarn"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("package.json").exists()
            && root.join("yarn.lock").exists()
            && !root.join("pnpm-lock.yaml").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "yarn".into(),
            args: vec![
                "install".into(),
                "--frozen-lockfile".into(),
                "--offline".into(),
            ],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "yarn".into(),
            args: vec!["test".into()],
        })
    }
}

pub struct NpmEcosystem;
impl EcosystemHandler for NpmEcosystem {
    fn name(&self) -> &str {
        "npm"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("package.json").exists()
            && !root.join("pnpm-lock.yaml").exists()
            && !root.join("yarn.lock").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "npm".into(),
            args: vec!["ci".into()],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "npm".into(),
            args: vec!["test".into(), "--no-audit".into()],
        })
    }
}

pub struct GoEcosystem;
impl EcosystemHandler for GoEcosystem {
    fn name(&self) -> &str {
        "go"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("go.mod").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "go".into(),
            args: vec!["mod".into(), "download".into()],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "go".into(),
            args: vec!["build".into(), "-mod=readonly".into(), "./...".into()],
        })
    }
}

pub struct PythonEcosystem;
impl EcosystemHandler for PythonEcosystem {
    fn name(&self) -> &str {
        "python"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("pyproject.toml").exists() || root.join("requirements.txt").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join(".venv"), root.join("__pycache__")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "pip".into(),
            args: vec![
                "install".into(),
                "--no-deps".into(),
                "-r".into(),
                "requirements.txt".into(),
            ],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "python3".into(),
            args: vec!["-m".into(), "py_compile".into(), ".".into()],
        })
    }
}

pub struct PhpEcosystem;
impl EcosystemHandler for PhpEcosystem {
    fn name(&self) -> &str {
        "php"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("composer.json").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "composer".into(),
            args: vec!["install".into(), "--no-dev".into()],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "composer".into(),
            args: vec!["validate".into()],
        })
    }
}

pub struct RubyEcosystem;
impl EcosystemHandler for RubyEcosystem {
    fn name(&self) -> &str {
        "ruby"
    }
    fn detect(&self, root: &Path) -> bool {
        root.join("Gemfile").exists()
    }
    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor/bundle"), root.join(".bundle")]
    }
    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "bundle".into(),
            args: vec!["install".into(), "--path".into(), "vendor/bundle".into()],
        })
    }
    fn verification_command(&self, _root: &Path) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "bundle".into(),
            args: vec!["check".into()],
        })
    }
}
