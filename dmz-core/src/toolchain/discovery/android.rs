use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path, push_unique_path,
    ToolchainDiscoverer,
};
use std::fs;
use std::path::PathBuf;
use tracing::info;

/// Discover standalone Android NDK distributions (Clang cross-compilers, sysroots, and android.toolchain.cmake).
#[derive(Debug, Default, Clone)]
pub struct AndroidNdkDiscoverer;

impl ToolchainDiscoverer for AndroidNdkDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::AndroidNdk
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // Explicit environment variables
        for var in &["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"] {
            if let Ok(val) = std::env::var(var) {
                let p = PathBuf::from(val);
                if p.join("build/cmake/android.toolchain.cmake").exists() {
                    push_unique_path(&mut candidates, p);
                }
            }
        }

        // Discover inside Android SDK roots
        for var in &["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Ok(val) = std::env::var(var) {
                let ndk_root = PathBuf::from(val).join("ndk");
                if let Ok(entries) = fs::read_dir(&ndk_root) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.join("build/cmake/android.toolchain.cmake").exists() {
                            push_unique_path(&mut candidates, p);
                        }
                    }
                }
            }
        }

        // Standard user home SDK locations
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            for sub in &[
                "Android/Sdk/ndk",
                "Library/Android/sdk/ndk",
                "AppData/Local/Android/Sdk/ndk",
            ] {
                let ndk_dir = home_path.join(sub);
                if let Ok(entries) = fs::read_dir(&ndk_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.join("build/cmake/android.toolchain.cmake").exists() {
                            push_unique_path(&mut candidates, p);
                        }
                    }
                }
            }
        }

        // System standalone directories
        push_unique_path(&mut candidates, PathBuf::from("/opt/android-ndk"));
        for base in &[
            "/opt/android-sdk/ndk",
            "C:\\Program Files\\Android\\android-sdk\\ndk",
        ] {
            let p = PathBuf::from(base);
            if let Ok(entries) = fs::read_dir(&p) {
                for entry in entries.flatten() {
                    let candidate = entry.path();
                    if candidate
                        .join("build/cmake/android.toolchain.cmake")
                        .exists()
                    {
                        push_unique_path(&mut candidates, candidate);
                    }
                }
            }
        }

        // Fallback via which ndk-build
        if let Ok(p) = which::which("ndk-build") {
            if let Some(parent) = p.parent() {
                if parent.join("build/cmake/android.toolchain.cmake").exists() {
                    push_unique_path(&mut candidates, parent.to_path_buf());
                }
            }
        }

        candidates.retain(|c| c.exists() && c.join("build/cmake/android.toolchain.cmake").exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let Some(ndk_root) = candidates.first().cloned() else {
            return Ok(None);
        };
        if is_non_standalone_path(&ndk_root) {
            emit_non_relocatable_notice(&ndk_root);
        }

        // Parse NDK version from source.properties if present
        let mut version = "Android NDK".to_string();
        let source_props = ndk_root.join("source.properties");
        if source_props.exists() {
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
        }

        info!(
            ndk = ?ndk_root,
            version = %version,
            "Discovered standalone Android NDK distribution"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        // Core build script
        for name in &["ndk-build", "ndk-build.cmd"] {
            let p = ndk_root.join(name);
            if p.exists() {
                bin_files.push((p, format!("bin/{}", name)));
            }
        }

        // Prebuilt LLVM toolchains (clang, clang++, llvm-ar, lld)
        let llvm_prebuilt = ndk_root.join("toolchains/llvm/prebuilt");
        if llvm_prebuilt.is_dir() {
            if let Ok(entries) = fs::read_dir(&llvm_prebuilt) {
                for entry in entries.flatten() {
                    let host_llvm = entry.path();
                    let bin_dir = host_llvm.join("bin");
                    if bin_dir.is_dir() {
                        for name in &[
                            "clang",
                            "clang++",
                            "llvm-ar",
                            "llvm-ranlib",
                            "lld",
                            "ld.lld",
                            "llvm-strip",
                        ] {
                            let p = bin_dir.join(name);
                            if p.exists() {
                                bin_files.push((p, format!("bin/{}", name)));
                            } else if cfg!(windows) {
                                let exe = bin_dir.join(format!("{}.exe", name));
                                if exe.exists() {
                                    bin_files.push((exe, format!("bin/{}.exe", name)));
                                }
                            }
                        }
                    }

                    // Sysroot headers (sysroot/usr/include)
                    let sysroot_inc = host_llvm.join("sysroot/usr/include");
                    if sysroot_inc.is_dir() {
                        collect_files_recursive(
                            &sysroot_inc,
                            &sysroot_inc,
                            &mut lib_files,
                            "sysroot/usr/include",
                        );
                    }

                    // Sysroot libraries and runtime objects (sysroot/usr/lib)
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
        }

        // CMake toolchain scripts (build/cmake/)
        let cmake_dir = ndk_root.join("build/cmake");
        if cmake_dir.is_dir() {
            collect_files_recursive(&cmake_dir, &ndk_root, &mut lib_files, "");
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::AndroidNdk,
            version,
            root_dir: ndk_root,
            bin_files,
            lib_files,
        }))
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
}
