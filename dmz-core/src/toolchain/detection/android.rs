use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct AndroidDetector;

impl AndroidDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if dir.join("Android.mk").is_file()
            || dir.join("Application.mk").is_file()
            || dir.join("app/src/main/jni/Android.mk").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::AndroidNdk);
            ecosystems.insert(ToolchainEcosystem::CMake);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        for gradle_name in &[
            "build.gradle",
            "build.gradle.kts",
            "app/build.gradle",
            "app/build.gradle.kts",
        ] {
            let p = dir.join(gradle_name);
            if p.is_file() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.contains("ndkVersion")
                        || content.contains("ndk {")
                        || content.contains("externalNativeBuild")
                    {
                        ecosystems.insert(ToolchainEcosystem::AndroidNdk);
                        ecosystems.insert(ToolchainEcosystem::CMake);
                        ecosystems.insert(ToolchainEcosystem::Cc);
                    }
                }
            }
        }
    }
}
