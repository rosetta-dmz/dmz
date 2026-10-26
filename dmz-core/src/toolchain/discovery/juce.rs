use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path, push_unique_path,
    ToolchainDiscoverer,
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

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1. Explicit JUCE_PATH environment variable
        if let Ok(root) = std::env::var("JUCE_PATH") {
            push_unique_path(&mut candidates, PathBuf::from(root));
        }

        // 2. Standard user and system standalone directories
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            push_unique_path(&mut candidates, home_path.join(".local/share/juce"));
            push_unique_path(&mut candidates, home_path.join("JUCE"));
            push_unique_path(&mut candidates, home_path.join(".juce"));
        }

        push_unique_path(&mut candidates, PathBuf::from("/opt/juce"));
        push_unique_path(&mut candidates, PathBuf::from("/Applications/JUCE"));

        // 3. Projucer executable on PATH
        if let Ok(p) = which::which("Projucer").or_else(|_| which::which("projucer")) {
            let p_dir = p.parent().unwrap_or(Path::new(""));
            push_unique_path(&mut candidates, p_dir.to_path_buf());
            if let Some(parent) = p_dir.parent() {
                push_unique_path(&mut candidates, parent.to_path_buf());
            }
        }

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_root: Option<PathBuf> = None;
        for candidate in candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }

            let has_modules =
                candidate.join("modules/juce_core").exists() || candidate.join("modules").exists();
            let has_projucer = candidate.join("Projucer").exists()
                || candidate.join("bin/Projucer").exists()
                || candidate.join("Projucer.app").exists()
                || candidate.join("projucer").exists();

            if has_modules || has_projucer {
                chosen_root = Some(candidate);
                break;
            }
        }

        let root_dir = match chosen_root {
            Some(r) => r,
            None => return Ok(None),
        };

        info!(root = ?root_dir, "Discovered standalone JUCE framework distribution");

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        // Check for Projucer binary
        for name in &["Projucer", "bin/Projucer", "projucer"] {
            let p = root_dir.join(name);
            if p.is_file() {
                bin_files.push((p, "bin/Projucer".to_string()));
                break;
            }
        }

        // Check for macOS Projucer.app
        let mac_app = root_dir.join("Projucer.app/Contents/MacOS/Projucer");
        if mac_app.is_file() {
            bin_files.push((mac_app, "bin/Projucer".to_string()));
        }

        // Capture JUCE core modules if present
        let modules_dir = root_dir.join("modules");
        if modules_dir.exists() {
            collect_files_recursive(&modules_dir, &root_dir, &mut lib_files, "");
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Juce,
            version: "JUCE".to_string(),
            root_dir,
            bin_files,
            lib_files,
        }))
    }
}
