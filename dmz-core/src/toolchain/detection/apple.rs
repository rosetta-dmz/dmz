use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct AppleDetector;

impl AppleDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if dir.join("Package.swift").is_file()
            || dir.join("Podfile").is_file()
            || dir.join("Cartfile").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::AppleSdk);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".xcodeproj") || name_str.ends_with(".xcworkspace") {
                    ecosystems.insert(ToolchainEcosystem::AppleSdk);
                    ecosystems.insert(ToolchainEcosystem::Cc);
                }
            }
        }
    }
}
