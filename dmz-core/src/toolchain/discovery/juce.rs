use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, push_unique_path, redact_home_prefix,
    select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use tracing::info;

/// Discovers standalone JUCE framework distributions (Projucer and JUCE modules).
#[derive(Debug, Default, Clone)]
pub struct JuceDiscoverer;

impl ToolchainDiscoverer for JuceDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Juce
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_juce_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((root_dir, ())) = select_strict_standalone_verified(candidates, verify_juce_root)
        else {
            return Ok(None);
        };

        let redacted_root = redact_home_prefix(&root_dir);
        info!(root = %redacted_root, "Discovered standalone JUCE framework distribution");

        let bin_files = collect_juce_binaries(&root_dir);
        let lib_files = collect_juce_libs(&root_dir);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Juce,
            version: "JUCE".to_string(),
            root_dir,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_juce_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(root) = std::env::var_os("JUCE_PATH").filter(|v| !v.is_empty()) {
        push_unique_path(&mut candidates, PathBuf::from(root));
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        push_unique_path(&mut candidates, home_path.join(".local/share/juce"));
        push_unique_path(&mut candidates, home_path.join("JUCE"));
        push_unique_path(&mut candidates, home_path.join(".juce"));
    }

    push_unique_path(&mut candidates, PathBuf::from("/opt/juce"));
    push_unique_path(&mut candidates, PathBuf::from("/Applications/JUCE"));

    if let Ok(p) = which::which("Projucer").or_else(|_| which::which("projucer")) {
        if let Some(p_dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            push_unique_path(&mut candidates, p_dir.to_path_buf());
            if let Some(parent) = p_dir.parent().filter(|d| !d.as_os_str().is_empty()) {
                push_unique_path(&mut candidates, parent.to_path_buf());
            }
        }
    }

    candidates.retain(|c| c.is_dir() && c != Path::new("/"));
    candidates
}

fn verify_juce_root(candidate: &Path) -> Option<()> {
    let has_modules =
        candidate.join("modules/juce_core").is_dir() || candidate.join("modules").is_dir();
    let has_projucer = candidate.join("Projucer").is_file()
        || candidate.join("bin/Projucer").is_file()
        || candidate.join("Projucer.app").is_dir()
        || candidate.join("projucer").is_file();

    if has_modules || has_projucer {
        Some(())
    } else {
        None
    }
}

fn collect_juce_binaries(root_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut bin_files = Vec::new();

    for name in &["Projucer", "bin/Projucer", "projucer"] {
        let p = root_dir.join(name);
        if p.is_file() {
            bin_files.push((p, "bin/Projucer".to_string()));
            break;
        }
    }

    let mac_app = root_dir.join("Projucer.app/Contents/MacOS/Projucer");
    if mac_app.is_file() {
        bin_files.push((mac_app, "bin/Projucer".to_string()));
    }

    bin_files
}

fn collect_juce_libs(root_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let modules_dir = root_dir.join("modules");
    if modules_dir.is_dir() {
        collect_files_recursive(&modules_dir, root_dir, &mut lib_files, "");
    }

    let cmake_file = root_dir.join("CMakeLists.txt");
    if cmake_file.is_file() {
        lib_files.push((cmake_file, "CMakeLists.txt".to_string()));
    }
    let cmake_dir = root_dir.join("extras/Build/CMake");
    if cmake_dir.is_dir() {
        collect_files_recursive(&cmake_dir, root_dir, &mut lib_files, "");
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_juce_collects_modules_and_rejects_homebrew() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/opt/juce"),
            PathBuf::from("/usr/local/Cellar/juce/8.0.0"),
        ];
        assert_eq!(select_strict_standalone_verified(brew, |_| Some(())), None);

        let temp = tempdir().unwrap();
        let juce_root = temp.path().join("JUCE");
        fs::create_dir_all(juce_root.join("modules/juce_core")).unwrap();
        fs::create_dir_all(juce_root.join("extras/Build/CMake")).unwrap();
        fs::create_dir_all(juce_root.join("bin")).unwrap();
        fs::write(juce_root.join("CMakeLists.txt"), "project(JUCE)").unwrap();
        fs::write(juce_root.join("modules/juce_core/juce_core.h"), "// core").unwrap();
        fs::write(
            juce_root.join("extras/Build/CMake/JUCEConfig.cmake.in"),
            "# cmake",
        )
        .unwrap();
        fs::write(juce_root.join("bin/Projucer"), "projucer").unwrap();

        assert_eq!(verify_juce_root(&juce_root), Some(()));
        let bins = collect_juce_binaries(&juce_root);
        assert!(bins.iter().any(|(_, rel)| rel == "bin/Projucer"));
        let libs = collect_juce_libs(&juce_root);
        assert!(libs.iter().any(|(_, rel)| rel == "CMakeLists.txt"));
        assert!(libs
            .iter()
            .any(|(_, rel)| rel == "modules/juce_core/juce_core.h"));
    }
}
