use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct ScriptingDetector;

impl ScriptingDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if dir.join("go.mod").is_file() || dir.join("go.work").is_file() {
            ecosystems.insert(ToolchainEcosystem::Go);
        }
        if dir.join("composer.json").is_file() || dir.join("composer.lock").is_file() {
            ecosystems.insert(ToolchainEcosystem::Php);
        }
        if dir.join("Gemfile").is_file() || dir.join("Gemfile.lock").is_file() {
            ecosystems.insert(ToolchainEcosystem::Ruby);
        }
        if dir.join("requirements.txt").is_file()
            || dir.join("pyproject.toml").is_file()
            || dir.join("Pipfile").is_file()
            || dir.join("Pipfile.lock").is_file()
            || dir.join("setup.py").is_file()
            || dir.join("setup.cfg").is_file()
            || dir.join("poetry.lock").is_file()
            || dir.join("uv.lock").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Python);
        }
    }
}
