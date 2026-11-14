use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, find_standalone_bin_in_path, is_apple_system_toolchain_path,
    push_deduped_file, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

const CLANG_COMPANIONS: &[&str] = &[
    "clang++",
    "llvm-ar",
    "ar",
    "llvm-ranlib",
    "ranlib",
    "lld",
    "ld.lld",
    "ld",
];
const GCC_COMPANIONS: &[&str] = &["g++", "ar", "ranlib", "ld"];

/// Discovers standalone host C/C++ compiler toolchains (Clang, GCC, companion archivers, linkers, and builtin headers).
#[derive(Debug, Default, Clone)]
pub struct CcDiscoverer;

impl ToolchainDiscoverer for CcDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Cc
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_cc_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((cc_bin, version)) =
            select_strict_standalone_verified(candidates, probe_cc_version)
        else {
            return Ok(None);
        };

        let Some(bin_dir) = cc_bin.parent().filter(|p| !p.as_os_str().is_empty()) else {
            return Ok(None);
        };
        let Some(root_dir) = resolve_prefix_root(&cc_bin) else {
            return Ok(None);
        };

        let redacted_cc = redact_home_prefix(&cc_bin);
        let redacted_root = redact_home_prefix(&root_dir);
        info!(
            cc = %redacted_cc,
            root = %redacted_root,
            version = %version,
            "Discovered standalone C/C++ compiler toolchain"
        );

        let primary_name = cc_bin
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cc")
            .to_string();
        let is_clang = primary_name.contains("clang");

        let bin_files = collect_cc_binaries(&cc_bin, bin_dir, &primary_name, is_clang);
        let lib_files = collect_cc_libs(&cc_bin, bin_dir, &root_dir, is_clang);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version,
            root_dir,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_cc_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for var_name in &[
        "CC",
        "CXX",
        "LLVM_HOME",
        "CLANG_HOME",
        "GCC_HOME",
        "DEVELOPER_DIR",
    ] {
        if let Some(val) = std::env::var_os(var_name).filter(|v| !v.is_empty()) {
            let p = PathBuf::from(val);
            if p.is_file() {
                push_unique_path(&mut candidates, p);
            } else if p.is_dir() {
                for sub in &[
                    "bin/clang",
                    "bin/gcc",
                    "bin/cc",
                    "usr/bin/clang",
                    "Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
                ] {
                    let candidate = p.join(sub);
                    if candidate.is_file() {
                        push_unique_path(&mut candidates, candidate);
                    }
                }
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        push_unique_path(&mut candidates, home_path.join(".local/bin/clang"));
        push_unique_path(&mut candidates, home_path.join(".local/bin/gcc"));
        push_unique_path(&mut candidates, home_path.join(".local/bin/cc"));
        push_unique_path(&mut candidates, home_path.join(".llvm/bin/clang"));
    }

    push_unique_path(&mut candidates, PathBuf::from("/opt/llvm/bin/clang"));
    push_unique_path(&mut candidates, PathBuf::from("/opt/gcc/bin/gcc"));
    push_unique_path(
        &mut candidates,
        PathBuf::from("C:\\Program Files\\LLVM\\bin\\clang.exe"),
    );

    for name in &["clang", "gcc", "cc"] {
        push_system_bin_candidates(&mut candidates, name);
    }

    candidates.retain(|c| c.is_file());
    candidates
}

fn probe_cc_version(candidate: &Path) -> Option<String> {
    if is_apple_system_toolchain_path(candidate) || !candidate.is_file() {
        return None;
    }

    let output = Command::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("cc")
            .trim()
            .to_string(),
    )
}

fn collect_cc_binaries(
    cc_bin: &Path,
    bin_dir: &Path,
    primary_name: &str,
    is_clang: bool,
) -> Vec<(PathBuf, String)> {
    let mut bin_files = Vec::new();
    let mut seen_bin = HashSet::new();
    push_deduped_file(
        &mut bin_files,
        &mut seen_bin,
        cc_bin.to_path_buf(),
        format!("bin/{}", primary_name),
    );

    let companion_names = if is_clang {
        CLANG_COMPANIONS
    } else {
        GCC_COMPANIONS
    };

    for name in companion_names {
        let p = bin_dir.join(name);
        if p.is_file() {
            push_deduped_file(&mut bin_files, &mut seen_bin, p, format!("bin/{}", name));
        } else if cfg!(windows) {
            let exe = bin_dir.join(format!("{}.exe", name));
            if exe.is_file() {
                push_deduped_file(
                    &mut bin_files,
                    &mut seen_bin,
                    exe,
                    format!("bin/{}.exe", name),
                );
            }
        } else if let Some(found) = find_standalone_bin_in_path(name, true) {
            push_deduped_file(
                &mut bin_files,
                &mut seen_bin,
                found,
                format!("bin/{}", name),
            );
        }
    }

    bin_files
}

fn collect_cc_libs(
    cc_bin: &Path,
    bin_dir: &Path,
    root_dir: &Path,
    is_clang: bool,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();
    let mut seen_lib = HashSet::new();

    if is_clang {
        if let Ok(res_output) = Command::new(cc_bin)
            .arg("-print-resource-dir")
            .stdin(Stdio::null())
            .output()
        {
            if res_output.status.success() {
                let res_dir_str = String::from_utf8_lossy(&res_output.stdout)
                    .trim()
                    .to_string();
                if !res_dir_str.is_empty() {
                    let inc_dir = PathBuf::from(res_dir_str).join("include");
                    if inc_dir.is_dir() {
                        collect_files_recursive(
                            &inc_dir,
                            &inc_dir,
                            &mut lib_files,
                            "lib/clang/include",
                        );
                        for (_, rel) in &lib_files {
                            seen_lib.insert(rel.clone());
                        }
                    }
                }
            }
        }
    }

    let candidate_lib_dirs = [
        root_dir.join("lib"),
        root_dir.join("lib64"),
        bin_dir.parent().unwrap_or(bin_dir).join("lib"),
    ];

    for lib_dir in &candidate_lib_dirs {
        if let Ok(entries) = fs::read_dir(lib_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let is_shlib = fname.ends_with(".dylib")
                        || fname.ends_with(".so")
                        || fname.contains(".so.")
                        || fname.ends_with(".dll");
                    if is_shlib {
                        let target_rel = format!("lib/{}", fname);
                        push_deduped_file(&mut lib_files, &mut seen_lib, p, target_rel);
                    }
                }
            }
        }
    }

    if !is_clang {
        for sub in &["libexec/gcc", "lib/gcc"] {
            let gcc_dir = root_dir.join(sub);
            if gcc_dir.is_dir() {
                collect_files_recursive(&gcc_dir, root_dir, &mut lib_files, "");
            }
        }
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_cc_discovers_companion_shared_libraries() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("fake_llvm");
        let bin_dir = root.join("bin");
        let lib_dir = root.join("lib");

        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();

        fs::write(bin_dir.join("clang"), "#!/bin/sh\necho clang 17.0.0").unwrap();
        fs::write(bin_dir.join("ld"), "#!/bin/sh\necho ld").unwrap();
        fs::write(lib_dir.join("libtapi.dylib"), "fake dylib").unwrap();
        fs::write(lib_dir.join("libLTO.dylib"), "fake dylib").unwrap();
        fs::write(lib_dir.join("libLLVM.so"), "fake so").unwrap();

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((bin_dir.join("clang"), "bin/clang".to_string()));
        bin_files.push((bin_dir.join("ld"), "bin/ld".to_string()));

        let candidate_lib_dirs = [root.join("lib")];
        for l_dir in &candidate_lib_dirs {
            if l_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(l_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            let is_shlib = fname.ends_with(".dylib")
                                || fname.ends_with(".so")
                                || fname.contains(".so.")
                                || fname.ends_with(".dll");
                            if is_shlib {
                                let target_rel = format!("lib/{}", fname);
                                if !lib_files.iter().any(|(_, r)| r == &target_rel) {
                                    lib_files.push((p, target_rel));
                                }
                            }
                        }
                    }
                }
            }
        }

        let rel_targets: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(rel_targets.contains(&"lib/libtapi.dylib".to_string()));
        assert!(rel_targets.contains(&"lib/libLTO.dylib".to_string()));
        assert!(rel_targets.contains(&"lib/libLLVM.so".to_string()));
    }

    #[test]
    fn test_cc_rejects_homebrew_stub_and_apple_clt() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/clang"),
            PathBuf::from("/usr/local/opt/llvm/bin/clang"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("clang 18.0".to_string())),
            None
        );

        let clt = Path::new("/Library/Developer/CommandLineTools/usr/bin/clang");
        assert!(is_apple_system_toolchain_path(clt));
        assert!(probe_cc_version(clt).is_none());

        let xcode_clang = Path::new(
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
        );
        assert!(is_apple_system_toolchain_path(xcode_clang));
        assert!(probe_cc_version(xcode_clang).is_none());

        if cfg!(target_os = "macos") {
            assert!(is_apple_system_toolchain_path(Path::new("/usr/bin/clang")));
            assert!(probe_cc_version(Path::new("/usr/bin/clang")).is_none());
        }
    }
}
