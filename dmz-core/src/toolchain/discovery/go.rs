use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path, push_unique_path,
    ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers host Go toolchain ($GOROOT, go), prioritizing official standalone GOROOT.
#[derive(Debug, Default, Clone)]
pub struct GoDiscoverer;

impl ToolchainDiscoverer for GoDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Go
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut go_candidates: Vec<PathBuf> = Vec::new();

        // 1. Explicit GOROOT
        if let Ok(goroot) = std::env::var("GOROOT") {
            let bin = PathBuf::from(goroot).join("bin/go");
            if bin.exists() {
                push_unique_path(&mut go_candidates, bin);
            }
        }

        // 2. Standard official Go installations
        let standard_paths = [PathBuf::from("/usr/local/go/bin/go")];
        for p in &standard_paths {
            if p.exists() {
                push_unique_path(&mut go_candidates, p.clone());
            }
        }

        // 3. User home ~/go or ~/sdk/go*
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            let user_go = home_path.join("go/bin/go");
            if user_go.exists() {
                push_unique_path(&mut go_candidates, user_go);
            }
            let sdk = home_path.join("sdk");
            if let Ok(entries) = fs::read_dir(sdk) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin/go");
                    if bin.exists() {
                        push_unique_path(&mut go_candidates, bin);
                    }
                }
            }
        }

        // 4. Fallback to which go
        if let Ok(p) = which::which("go") {
            push_unique_path(&mut go_candidates, p);
        }

        if go_candidates.is_empty() {
            return Ok(None);
        }

        go_candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_go: Option<PathBuf> = None;
        for candidate in go_candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }
            if chosen_go.is_none()
                || (!is_non_standalone_path(&candidate)
                    && is_non_standalone_path(chosen_go.as_ref().unwrap()))
            {
                chosen_go = Some(candidate.clone());
                if !is_non_standalone_path(&candidate) {
                    break;
                }
            }
        }

        let go_bin = match chosen_go {
            Some(g) => g,
            None => return Ok(None),
        };

        // Query GOROOT from go env
        let goroot_dir = Command::new(&go_bin)
            .arg("env")
            .arg("GOROOT")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| PathBuf::from(s.trim()))
            .unwrap_or_else(|| {
                go_bin
                    .parent()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/"))
            });

        let version = Command::new(&go_bin)
            .arg("version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "go".to_string())
            .trim()
            .to_string();

        info!(
            goroot = ?goroot_dir,
            version = %version,
            "Discovered host Go toolchain"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((go_bin.clone(), "bin/go".into()));

        // Companion compilers and tools under pkg/tool
        let pkg_tool_dir = goroot_dir.join("pkg/tool");
        if pkg_tool_dir.exists() {
            collect_files_recursive(&pkg_tool_dir, &pkg_tool_dir, &mut lib_files, "pkg/tool");
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Go,
            version,
            root_dir: goroot_dir,
            bin_files,
            lib_files,
        }))
    }
}
