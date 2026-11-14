use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, push_deduped_file, push_unique_path, redact_home_prefix,
    select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

const NDK_LLVM_BINS: &[&str] = &[
    "clang",
    "clang++",
    "llvm-ar",
    "llvm-ranlib",
    "lld",
    "ld.lld",
    "llvm-strip",
];

struct NdkAssets {
    bin_files: Vec<(PathBuf, String)>,
    lib_files: Vec<(PathBuf, String)>,
}

/// Discover standalone Android NDK distributions (Clang cross-compilers, sysroots, and android.toolchain.cmake).
#[derive(Debug, Default, Clone)]
pub struct AndroidNdkDiscoverer;

impl ToolchainDiscoverer for AndroidNdkDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::AndroidNdk
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_ndk_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((ndk_root, version)) =
            select_strict_standalone_verified(candidates, probe_ndk_version)
        else {
            return Ok(None);
        };

        let redacted_ndk = redact_home_prefix(&ndk_root);
        info!(
            ndk = %redacted_ndk,
            version = %version,
            "Discovered standalone Android NDK distribution"
        );

        let assets = collect_ndk_assets(&ndk_root);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::AndroidNdk,
            version,
            root_dir: ndk_root,
            bin_files: assets.bin_files,
            lib_files: assets.lib_files,
        }))
    }
}

fn collect_ndk_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for var in &["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"] {
        if let Some(val) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            let p = PathBuf::from(val);
            if p.join("build/cmake/android.toolchain.cmake").is_file() {
                push_unique_path(&mut candidates, p);
            }
        }
    }

    for var in &["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(val) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            push_ndk_subdirs(&mut candidates, &PathBuf::from(val).join("ndk"));
        }
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        for sub in &[
            "Android/Sdk/ndk",
            "Library/Android/sdk/ndk",
            "AppData/Local/Android/Sdk/ndk",
        ] {
            push_ndk_subdirs(&mut candidates, &home_path.join(sub));
        }
    }

    push_unique_path(&mut candidates, PathBuf::from("/opt/android-ndk"));
    for base in &[
        "/opt/android-sdk/ndk",
        "C:\\Program Files\\Android\\android-sdk\\ndk",
    ] {
        push_ndk_subdirs(&mut candidates, Path::new(base));
    }

    if let Ok(p) = which::which("ndk-build") {
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            if parent.join("build/cmake/android.toolchain.cmake").is_file() {
                push_unique_path(&mut candidates, parent.to_path_buf());
            }
        }
    }

    candidates.retain(|c| {
        c.is_dir() && c != Path::new("/") && c.join("build/cmake/android.toolchain.cmake").is_file()
    });
    candidates
}

fn push_ndk_subdirs(candidates: &mut Vec<PathBuf>, ndk_dir: &Path) {
    let Ok(entries) = fs::read_dir(ndk_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.join("build/cmake/android.toolchain.cmake").is_file() {
            push_unique_path(candidates, p);
        }
    }
}

fn probe_ndk_version(ndk_root: &Path) -> Option<String> {
    if !ndk_root
        .join("build/cmake/android.toolchain.cmake")
        .is_file()
    {
        return None;
    }

    let mut version = "Android NDK".to_string();
    let source_props = ndk_root.join("source.properties");
    if let Ok(content) = fs::read_to_string(&source_props) {
        for line in content.lines() {
            if line.starts_with("Pkg.Revision") {
                if let Some((_, rev)) = line.split_once('=') {
                    version = format!("Android NDK {}", rev.trim());
                    break;
                }
            }
        }
    }
    Some(version)
}

fn collect_ndk_assets(ndk_root: &Path) -> NdkAssets {
    let mut bin_files = Vec::new();
    let mut seen_bin = HashSet::new();
    let mut lib_files = Vec::new();

    for name in &["ndk-build", "ndk-build.cmd"] {
        let p = ndk_root.join(name);
        if p.is_file() {
            push_deduped_file(&mut bin_files, &mut seen_bin, p, format!("bin/{}", name));
        }
    }

    let llvm_prebuilt = ndk_root.join("toolchains/llvm/prebuilt");
    if let Ok(entries) = fs::read_dir(&llvm_prebuilt) {
        for entry in entries.flatten() {
            let host_llvm = entry.path();
            let bin_dir = host_llvm.join("bin");
            if bin_dir.is_dir() {
                for name in NDK_LLVM_BINS {
                    let p = bin_dir.join(name);
                    if p.is_file() {
                        push_deduped_file(
                            &mut bin_files,
                            &mut seen_bin,
                            p,
                            format!("bin/{}", name),
                        );
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
                    }
                }
            }

            let sysroot_inc = host_llvm.join("sysroot/usr/include");
            if sysroot_inc.is_dir() {
                collect_files_recursive(
                    &sysroot_inc,
                    &sysroot_inc,
                    &mut lib_files,
                    "sysroot/usr/include",
                );
            }

            let sysroot_lib = host_llvm.join("sysroot/usr/lib");
            if sysroot_lib.is_dir() {
                collect_files_recursive(
                    &sysroot_lib,
                    &sysroot_lib,
                    &mut lib_files,
                    "sysroot/usr/lib",
                );
            }
        }
    }

    let cmake_dir = ndk_root.join("build/cmake");
    if cmake_dir.is_dir() {
        collect_files_recursive(&cmake_dir, ndk_root, &mut lib_files, "");
    }

    NdkAssets {
        bin_files,
        lib_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_android_ndk_collects_sysroot_include_and_lib() {
        let temp = tempdir().unwrap();
        let ndk = temp.path().join("fake_ndk");
        let prebuilt = ndk.join("toolchains/llvm/prebuilt/darwin-x86_64");
        let inc = prebuilt.join("sysroot/usr/include");
        let lib = prebuilt.join("sysroot/usr/lib/aarch64-linux-android");
        let cmake = ndk.join("build/cmake");

        fs::create_dir_all(inc.join("android")).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&cmake).unwrap();

        fs::write(inc.join("android/log.h"), "#define LOG").unwrap();
        fs::write(lib.join("crtbegin_so.o"), "crt").unwrap();
        fs::write(lib.join("libc++_shared.so"), "stl").unwrap();
        fs::write(cmake.join("android.toolchain.cmake"), "# cmake").unwrap();

        let mut lib_files = Vec::new();
        collect_files_recursive(&inc, &inc, &mut lib_files, "sysroot/usr/include");
        collect_files_recursive(&lib, &lib, &mut lib_files, "sysroot/usr/lib");
        collect_files_recursive(&cmake, &ndk, &mut lib_files, "");

        let rel_names: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(rel_names.contains(&"sysroot/usr/include/android/log.h".to_string()));
        assert!(rel_names.iter().any(|p| p.contains("crtbegin_so.o")));
        assert!(rel_names.iter().any(|p| p.contains("libc++_shared.so")));
        assert!(rel_names.contains(&"build/cmake/android.toolchain.cmake".to_string()));
    }

    #[test]
    fn test_android_ndk_rejects_non_standalone_paths() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/share/android-ndk"),
            PathBuf::from("/nix/store/abc-android-ndk"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, probe_ndk_version),
            None
        );
        assert!(probe_ndk_version(Path::new("/")).is_none());
    }
}
