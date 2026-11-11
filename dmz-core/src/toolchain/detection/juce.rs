use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct JuceDetector;

impl JuceDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".jucer") {
                    ecosystems.insert(ToolchainEcosystem::Juce);
                    ecosystems.insert(ToolchainEcosystem::Cc);
                    ecosystems.insert(ToolchainEcosystem::CMake);
                }
            }
        }
    }
}
