use super::super::types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainProfile,
};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Discovers host Go toolchain ($GOROOT, go), prioritizing official standalone GOROOT.
#[derive(Debug, Default, Clone)]
pub struct GoDiscoverer;

impl ToolchainDiscoverer for GoDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Go
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        self.discover_with_context(&ToolchainDiscoveryContext::default())
    }

    fn discover_with_context(
        &self,
        ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let go_candidates = collect_go_candidates();
        if go_candidates.is_empty() {
            return Ok(None);
        }

        let Some((go_bin, goroot_dir)) =
            select_strict_standalone_verified(go_candidates, resolve_goroot_for_candidate)
        else {
            return Ok(None);
        };

        let version = probe_go_version(&go_bin);
        let redacted_root = redact_home_prefix(&goroot_dir);
        info!(
            goroot = %redacted_root,
            version = %version,
            profile = %ctx.profile,
            "Discovered host Go toolchain"
        );

        let bin_files = vec![(go_bin, "bin/go".into())];
        let lib_files =
            collect_go_libs_for_context(&goroot_dir, ctx.profile, ctx.target_platform.as_deref());

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Go,
            version,
            root_dir: goroot_dir,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_go_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(goroot) = std::env::var_os("GOROOT").filter(|v| !v.is_empty()) {
        let bin = PathBuf::from(goroot).join("bin/go");
        if bin.is_file() {
            push_unique_path(&mut candidates, bin);
        }
    }

    let std_go = PathBuf::from("/usr/local/go/bin/go");
    if std_go.is_file() {
        push_unique_path(&mut candidates, std_go);
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        let user_go = home_path.join("go/bin/go");
        if user_go.is_file() {
            push_unique_path(&mut candidates, user_go);
        }
        let sdk = home_path.join("sdk");
        if let Ok(entries) = fs::read_dir(sdk) {
            for entry in entries.flatten() {
                let bin = entry.path().join("bin/go");
                if bin.is_file() {
                    push_unique_path(&mut candidates, bin);
                }
            }
        }
    }

    push_system_bin_candidates(&mut candidates, "go");
    candidates
}

fn resolve_goroot_for_candidate(go_bin: &Path) -> Option<PathBuf> {
    if is_macos_usr_bin_stub(go_bin) || !go_bin.is_file() {
        return None;
    }

    let goroot_dir = Command::new(go_bin)
        .args(["env", "GOROOT"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty() && *p != Path::new("/") && p.is_dir())
        .or_else(|| resolve_prefix_root(go_bin))?;

    if is_non_standalone_path(&goroot_dir) {
        emit_non_relocatable_notice(&goroot_dir);
        return None;
    }

    Some(goroot_dir)
}

fn probe_go_version(go_bin: &Path) -> String {
    Command::new(go_bin)
        .arg("version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "go".to_string())
}

fn to_go_os_arch(os: &str, arch: &str) -> String {
    let go_os = match os {
        "macos" => "darwin",
        other => other,
    };
    let go_arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("{}_{}", go_os, go_arch)
}

fn collect_go_libs_for_context(
    goroot_dir: &Path,
    profile: ToolchainProfile,
    target_platform: Option<&str>,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let pkg_tool = goroot_dir.join("pkg/tool");
    if pkg_tool.is_dir() {
        match profile {
            ToolchainProfile::Full => {
                collect_files_recursive(&pkg_tool, &pkg_tool, &mut lib_files, "pkg/tool");
            }
            ToolchainProfile::Minimal => {
                let mut allowed_targets = HashSet::new();
                allowed_targets.insert(to_go_os_arch(std::env::consts::OS, std::env::consts::ARCH));
                let (t_os, t_arch) = crate::workspace::resolve_target_os_arch(target_platform);
                allowed_targets.insert(to_go_os_arch(t_os, t_arch));

                if let Ok(entries) = fs::read_dir(&pkg_tool) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if p.is_file() {
                            lib_files.push((p, format!("pkg/tool/{}", name)));
                        } else if p.is_dir() && allowed_targets.contains(&name) {
                            collect_files_recursive(
                                &p,
                                &p,
                                &mut lib_files,
                                &format!("pkg/tool/{}", name),
                            );
                        }
                    }
                }
            }
        }
    }

    for (sub, prefix) in &[("src", "src"), ("pkg/include", "pkg/include")] {
        let dir = goroot_dir.join(sub);
        if dir.is_dir() {
            collect_files_recursive(&dir, &dir, &mut lib_files, prefix);
        }
    }

    for rel in &["lib/time/zoneinfo.zip", "go.env"] {
        let p = goroot_dir.join(rel);
        if p.is_file() {
            lib_files.push((p, (*rel).to_string()));
        }
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_go_collects_src_and_runtime_assets() {
        let temp = tempdir().unwrap();
        let goroot = temp.path().join("fake_goroot");
        let bin = goroot.join("bin");
        let src_fmt = goroot.join("src/fmt");
        let pkg_tool = goroot.join("pkg/tool/darwin_arm64");

        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&src_fmt).unwrap();
        fs::create_dir_all(&pkg_tool).unwrap();

        fs::write(bin.join("go"), "go").unwrap();
        fs::write(src_fmt.join("print.go"), "package fmt").unwrap();
        fs::write(pkg_tool.join("compile"), "compile").unwrap();

        let mut lib_files = Vec::new();

        let pkg_tool_dir = goroot.join("pkg/tool");
        if pkg_tool_dir.exists() {
            collect_files_recursive(&pkg_tool_dir, &pkg_tool_dir, &mut lib_files, "pkg/tool");
        }
        let src_dir = goroot.join("src");
        if src_dir.is_dir() {
            collect_files_recursive(&src_dir, &src_dir, &mut lib_files, "src");
        }

        let rel_paths: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(rel_paths.contains(&"src/fmt/print.go".to_string()));
        assert!(rel_paths.iter().any(|p| p.starts_with("pkg/tool/")));
    }

    #[test]
    fn test_go_minimal_profile_prunes_foreign_pkg_tool_targets(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let goroot = temp.path().join("fake_goroot");
        fs::create_dir_all(goroot.join("src/fmt"))?;
        fs::create_dir_all(goroot.join("pkg/include"))?;
        fs::create_dir_all(goroot.join("pkg/tool/linux_amd64"))?;
        fs::create_dir_all(goroot.join("pkg/tool/windows_amd64"))?;
        fs::create_dir_all(goroot.join("pkg/tool/freebsd_amd64"))?;

        fs::write(goroot.join("src/fmt/print.go"), "package fmt")?;
        fs::write(goroot.join("pkg/include/textflag.h"), "#define NOSPLIT")?;
        fs::write(goroot.join("pkg/tool/linux_amd64/compile"), "linux")?;
        fs::write(goroot.join("pkg/tool/windows_amd64/compile.exe"), "win")?;
        fs::write(goroot.join("pkg/tool/freebsd_amd64/compile"), "bsd")?;

        let min_rels: Vec<String> =
            collect_go_libs_for_context(&goroot, ToolchainProfile::Minimal, Some("linux-x64"))
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(min_rels.contains(&"pkg/tool/linux_amd64/compile".to_string()));
        assert!(min_rels.contains(&"pkg/include/textflag.h".to_string()));
        assert!(min_rels.contains(&"src/fmt/print.go".to_string()));
        assert!(!min_rels.iter().any(|r| r.contains("windows_amd64")));
        assert!(!min_rels.iter().any(|r| r.contains("freebsd_amd64")));

        let full_rels: Vec<String> =
            collect_go_libs_for_context(&goroot, ToolchainProfile::Full, Some("linux-x64"))
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(full_rels.contains(&"pkg/tool/linux_amd64/compile".to_string()));
        assert!(full_rels.contains(&"pkg/tool/windows_amd64/compile.exe".to_string()));
        assert!(full_rels.contains(&"pkg/tool/freebsd_amd64/compile".to_string()));

        Ok(())
    }
}
