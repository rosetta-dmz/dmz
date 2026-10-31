use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

/// Target machine activation and setup engine for imported toolchains.
pub struct ToolchainSetup;

impl ToolchainSetup {
    /// Sets up imported toolchain directory: execution permissions, quarantine clearance, and activator scripts.
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<(), String> {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        let has_toolchains = toolchains_dir.exists()
            && fs::read_dir(&toolchains_dir)
                .map(|mut i| i.next().is_some())
                .unwrap_or(false);
        if !has_toolchains {
            Self::generate_ide_settings(workspace_root)?;
            return Ok(());
        }

        // 1. Ensure 0o755 execution permissions on all binary files
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fn make_exec_recursive(dir: &Path) {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            make_exec_recursive(&p);
                        } else if p.is_file() {
                            let in_bin = p
                                .parent()
                                .and_then(|parent| parent.file_name())
                                .is_some_and(|name| name == "bin");
                            if in_bin {
                                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
                            }
                        }
                    }
                }
            }
            make_exec_recursive(&toolchains_dir);
        }

        // 2. Clear macOS quarantine attribute (xattr -cr)
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("xattr")
                .arg("-cr")
                .arg(&toolchains_dir)
                .output();
        }

        // 3. Generate hermetic Clang wrapper scripts (injecting -isysroot on macOS/Unix)
        Self::generate_clang_wrappers(workspace_root)?;

        // 4. Setup Node.js companion package managers (npm, npx, pnpm, yarn)
        Self::setup_node_package_managers(workspace_root)?;

        // 5. Generate .dmz/toolchains/dmz.toolchain.cmake only if CMake is actually used
        Self::generate_cmake_toolchain(workspace_root)?;

        // 6. Generate .dmz/env.sh and .dmz/env.bat activator scripts
        Self::generate_activator_scripts(workspace_root)?;

        // 7. Configure IDE workspace settings (.vscode/settings.json)
        Self::generate_ide_settings(workspace_root)?;

        Ok(())
    }

    /// Generates hermetic wrapper scripts for Clang/LLVM compiler binaries to ensure
    /// the Apple SDK sysroot (-isysroot) is automatically passed on macOS.
    #[cfg(unix)]
    pub fn generate_clang_wrappers(workspace_root: &Path) -> Result<(), String> {
        let cc_bin = workspace_root.join(".dmz/toolchains/cc/bin");
        if !cc_bin.is_dir() {
            return Ok(());
        }

        use std::os::unix::fs::PermissionsExt;

        let wrapper_script = r#"#!/bin/sh
# DMZ Hermetic Clang Wrapper
# Automatically injects bundled Apple SDK sysroot when compiling on macOS

# 1. Resolve self path and directory purely using POSIX shell builtins (zero external utilities)
_SELF="$0"
case "$_SELF" in
    */*) ;;
    *)
        _SELF="$(command -v "$_SELF" 2>/dev/null)"
        [ -z "$_SELF" ] && _SELF="./$0"
        ;;
esac

case "$_SELF" in
    */*)
        _DIR="${_SELF%/*}"
        _NAME="${_SELF##*/}"
        ;;
    *)
        _DIR="."
        _NAME="$_SELF"
        ;;
esac

_DIR="$(cd "$_DIR" 2>/dev/null && pwd)"

# 2. Locate real compiler binary
_REAL="$_DIR/${_NAME}.real"
if [ ! -f "$_REAL" ]; then
    case "$_NAME" in
        *++|*xx|*XX)
            if [ -f "$_DIR/clang++.real" ]; then
                _REAL="$_DIR/clang++.real"
            fi
            ;;
        *)
            if [ -f "$_DIR/clang.real" ]; then
                _REAL="$_DIR/clang.real"
            fi
            ;;
    esac
fi

# 3. Locate Apple SDK root
_SDK="${SDKROOT}"
if [ -z "$_SDK" ] || [ ! -d "$_SDK" ]; then
    for _candidate in \
        "$_DIR/../../applesdk/SDKs/MacOSX.sdk" \
        "$_DIR/../../applesdk/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
    do
        if [ -d "$_candidate" ]; then
            _SDK="$_candidate"
            break
        fi
    done
fi

if [ -z "$_SDK" ] || [ ! -d "$_SDK" ]; then
    for _glob in "$_DIR"/../../applesdk/SDKs/MacOSX*.sdk "$_DIR"/../../applesdk/Platforms/MacOSX.platform/Developer/SDKs/MacOSX*.sdk; do
        if [ -d "$_glob" ]; then
            _SDK="$_glob"
            break
        fi
    done
fi

# 4. Check if -isysroot or --sysroot is already specified in arguments
_HAS_SYSROOT=0
_HAS_RESOURCE_DIR=0
for _arg in "$@"; do
    case "$_arg" in
        -isysroot|--sysroot|-isysroot=*|--sysroot=*)
            _HAS_SYSROOT=1
            ;;
        -resource-dir|-resource-dir=*)
            _HAS_RESOURCE_DIR=1
            ;;
    esac
done

# 5. Locate Clang builtin resource directory (contains include/stdarg.h, include/stddef.h)
_RES=""
if [ -d "$_DIR/../lib/clang/include" ]; then
    _RES="$_DIR/../lib/clang"
else
    for _rcand in "$_DIR"/../lib/clang/*; do
        if [ -d "$_rcand/include" ]; then
            _RES="$_rcand"
            break
        fi
    done
fi

_EXTRA_ARGS=""
if [ -n "$_SDK" ] && [ -d "$_SDK" ]; then
    export SDKROOT="$_SDK"
    if [ "$_HAS_SYSROOT" -eq 0 ]; then
        _EXTRA_ARGS="-isysroot $_SDK"
    fi
fi

if [ -n "$_RES" ] && [ -d "$_RES" ] && [ "$_HAS_RESOURCE_DIR" -eq 0 ]; then
    _EXTRA_ARGS="$_EXTRA_ARGS -resource-dir $_RES"
fi

if [ -n "$_EXTRA_ARGS" ]; then
    exec "$_REAL" $_EXTRA_ARGS "$@"
fi

exec "$_REAL" "$@"
"#;

        let compiler_names = ["clang", "clang++", "cc", "c++"];

        for name in &compiler_names {
            let orig_path = cc_bin.join(name);
            let real_path = cc_bin.join(format!("{}.real", name));

            if orig_path.exists() || fs::symlink_metadata(&orig_path).is_ok() {
                let is_symlink = fs::symlink_metadata(&orig_path)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false);

                if !real_path.exists() {
                    if is_symlink {
                        let _ = fs::remove_file(&orig_path);
                    } else {
                        fs::rename(&orig_path, &real_path).map_err(|e| {
                            format!("Failed to rename {} to {}.real: {}", name, name, e)
                        })?;
                    }
                } else if is_symlink {
                    let _ = fs::remove_file(&orig_path);
                }

                fs::write(&orig_path, wrapper_script)
                    .map_err(|e| format!("Failed to write wrapper script for {}: {}", name, e))?;
                let _ = fs::set_permissions(&orig_path, fs::Permissions::from_mode(0o755));
                if real_path.exists() {
                    let _ = fs::set_permissions(&real_path, fs::Permissions::from_mode(0o755));
                }
            } else if real_path.exists() {
                fs::write(&orig_path, wrapper_script)
                    .map_err(|e| format!("Failed to write wrapper script for {}: {}", name, e))?;
                let _ = fs::set_permissions(&orig_path, fs::Permissions::from_mode(0o755));
                let _ = fs::set_permissions(&real_path, fs::Permissions::from_mode(0o755));
            } else if (*name == "cc"
                && (cc_bin.join("clang.real").exists() || cc_bin.join("clang").exists()))
                || (*name == "c++"
                    && (cc_bin.join("clang++.real").exists() || cc_bin.join("clang++").exists()))
            {
                fs::write(&orig_path, wrapper_script)
                    .map_err(|e| format!("Failed to write wrapper script for {}: {}", name, e))?;
                let _ = fs::set_permissions(&orig_path, fs::Permissions::from_mode(0o755));
            }
        }

        Ok(())
    }

    #[cfg(not(unix))]
    pub fn generate_clang_wrappers(_workspace_root: &Path) -> Result<(), String> {
        Ok(())
    }

    /// Ensures Node.js companion package managers (npm, npx, pnpm, yarn) are correctly wired
    /// to their module entry points in `lib/node_modules/`.
    pub fn setup_node_package_managers(workspace_root: &Path) -> Result<(), String> {
        let node_dir = workspace_root.join(".dmz/toolchains/node");
        if !node_dir.is_dir() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let bin_dir = node_dir.join("bin");
            let nm_dir = node_dir.join("lib/node_modules");

            if nm_dir.is_dir() {
                let _ = fs::create_dir_all(&bin_dir);

                let managers = [
                    ("npm", "lib/node_modules/npm/bin/npm-cli.js"),
                    ("npx", "lib/node_modules/npm/bin/npx-cli.js"),
                    ("pnpm", "lib/node_modules/pnpm/bin/pnpm.cjs"),
                    ("yarn", "lib/node_modules/yarn/bin/yarn.js"),
                    ("corepack", "lib/node_modules/corepack/dist/corepack.js"),
                ];

                for (name, rel_target) in &managers {
                    let target_full = node_dir.join(rel_target);
                    if target_full.exists() {
                        let link_path = bin_dir.join(name);
                        let rel_symlink_target = format!("../{}", rel_target);
                        if link_path.exists() || fs::symlink_metadata(&link_path).is_ok() {
                            let _ = fs::remove_file(&link_path);
                        }
                        let _ = symlink(&rel_symlink_target, &link_path);
                    }
                }
            }
        }

        Ok(())
    }

    /// Generates `.dmz/toolchains/dmz.toolchain.cmake` to wire bundled compilers and tools for CMake.
    pub fn generate_cmake_toolchain(workspace_root: &Path) -> Result<(), String> {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        if !toolchains_dir.exists() {
            return Ok(());
        }

        let has_cmake_or_cross_toolchain =
            crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("cmake"), "bin/cmake")
                .is_some()
                || workspace_root.join("CMakeLists.txt").is_file()
                || toolchains_dir.join("androidndk").exists();

        if !has_cmake_or_cross_toolchain {
            let stale_cmake = toolchains_dir.join("dmz.toolchain.cmake");
            if stale_cmake.exists() {
                let _ = fs::remove_file(stale_cmake);
            }
            return Ok(());
        }

        let toolchain_cmake_path = toolchains_dir.join("dmz.toolchain.cmake");
        let cmake_content = r#"# ==============================================================================
# DMZ Autogenerated Hermetic Air-Gap CMake Toolchain
# Automatically generated by DMZ to guarantee isolated, zero-dependency builds
# ==============================================================================
cmake_minimum_required(VERSION 3.10)

set(DMZ_TOOLCHAINS_DIR "${CMAKE_CURRENT_LIST_DIR}")

# 1. Darwin / Apple Platform Sysroot Detection (evaluated prior to compiler probing)
if(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin" OR EXISTS "${DMZ_TOOLCHAINS_DIR}/applesdk" OR DEFINED ENV{SDKROOT})
    set(CMAKE_SYSTEM_NAME "Darwin" CACHE STRING "Target System" FORCE)

    if(NOT DEFINED CMAKE_OSX_SYSROOT OR CMAKE_OSX_SYSROOT STREQUAL "")
        if(DEFINED ENV{SDKROOT} AND EXISTS "$ENV{SDKROOT}")
            set(CMAKE_OSX_SYSROOT "$ENV{SDKROOT}" CACHE PATH "Apple SDK Root" FORCE)
        else()
            file(GLOB _DMZ_APPLE_SDKS
                "${DMZ_TOOLCHAINS_DIR}/applesdk/SDKs/MacOSX*.sdk"
                "${DMZ_TOOLCHAINS_DIR}/applesdk/Platforms/MacOSX.platform/Developer/SDKs/MacOSX*.sdk"
            )
            list(LENGTH _DMZ_APPLE_SDKS _DMZ_NUM_SDKS)
            if(_DMZ_NUM_SDKS GREATER 0)
                list(GET _DMZ_APPLE_SDKS 0 _DMZ_FIRST_SDK)
                set(CMAKE_OSX_SYSROOT "${_DMZ_FIRST_SDK}" CACHE PATH "Apple SDK Root" FORCE)
            endif()
        endif()
    endif()

    if(CMAKE_OSX_SYSROOT AND EXISTS "${CMAKE_OSX_SYSROOT}")
        set(ENV{SDKROOT} "${CMAKE_OSX_SYSROOT}")
    endif()
endif()

# 2. Configure C and C++ Compilers
if(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/clang")
    set(CMAKE_C_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/clang" CACHE FILEPATH "C Compiler" FORCE)
    set(CMAKE_CXX_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/clang++" CACHE FILEPATH "C++ Compiler" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/clang.exe")
    set(CMAKE_C_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/clang.exe" CACHE FILEPATH "C Compiler" FORCE)
    set(CMAKE_CXX_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/clang++.exe" CACHE FILEPATH "C++ Compiler" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/gcc")
    set(CMAKE_C_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/gcc" CACHE FILEPATH "C Compiler" FORCE)
    set(CMAKE_CXX_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/g++" CACHE FILEPATH "C++ Compiler" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/gcc.exe")
    set(CMAKE_C_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/gcc.exe" CACHE FILEPATH "C Compiler" FORCE)
    set(CMAKE_CXX_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/g++.exe" CACHE FILEPATH "C++ Compiler" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/cc")
    set(CMAKE_C_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/cc" CACHE FILEPATH "C Compiler" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/cc.exe")
    set(CMAKE_C_COMPILER "${DMZ_TOOLCHAINS_DIR}/cc/bin/cc.exe" CACHE FILEPATH "C Compiler" FORCE)
endif()

# 3. Configure Archiver and Ranlib
if(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ar")
    set(CMAKE_AR "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ar" CACHE FILEPATH "Archiver" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ar.exe")
    set(CMAKE_AR "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ar.exe" CACHE FILEPATH "Archiver" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/ar")
    set(CMAKE_AR "${DMZ_TOOLCHAINS_DIR}/cc/bin/ar" CACHE FILEPATH "Archiver" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/ar.exe")
    set(CMAKE_AR "${DMZ_TOOLCHAINS_DIR}/cc/bin/ar.exe" CACHE FILEPATH "Archiver" FORCE)
endif()

if(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ranlib")
    set(CMAKE_RANLIB "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ranlib" CACHE FILEPATH "Ranlib" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ranlib.exe")
    set(CMAKE_RANLIB "${DMZ_TOOLCHAINS_DIR}/cc/bin/llvm-ranlib.exe" CACHE FILEPATH "Ranlib" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/ranlib")
    set(CMAKE_RANLIB "${DMZ_TOOLCHAINS_DIR}/cc/bin/ranlib" CACHE FILEPATH "Ranlib" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/cc/bin/ranlib.exe")
    set(CMAKE_RANLIB "${DMZ_TOOLCHAINS_DIR}/cc/bin/ranlib.exe" CACHE FILEPATH "Ranlib" FORCE)
endif()

# 4. Configure Build Generator (Ninja if bundled)
if(EXISTS "${DMZ_TOOLCHAINS_DIR}/ninja/bin/ninja")
    set(CMAKE_MAKE_PROGRAM "${DMZ_TOOLCHAINS_DIR}/ninja/bin/ninja" CACHE FILEPATH "Ninja" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/ninja/bin/ninja.exe")
    set(CMAKE_MAKE_PROGRAM "${DMZ_TOOLCHAINS_DIR}/ninja/bin/ninja.exe" CACHE FILEPATH "Ninja" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/make/bin/make")
    set(CMAKE_MAKE_PROGRAM "${DMZ_TOOLCHAINS_DIR}/make/bin/make" CACHE FILEPATH "Make" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/make/bin/make.exe")
    set(CMAKE_MAKE_PROGRAM "${DMZ_TOOLCHAINS_DIR}/make/bin/make.exe" CACHE FILEPATH "Make" FORCE)
elseif(EXISTS "${DMZ_TOOLCHAINS_DIR}/make/bin/mingw32-make.exe")
    set(CMAKE_MAKE_PROGRAM "${DMZ_TOOLCHAINS_DIR}/make/bin/mingw32-make.exe" CACHE FILEPATH "Make" FORCE)
endif()

# 5. Scope CMake search roots strictly to DMZ bundled toolchain assets and Apple SDK sysroot
set(CMAKE_FIND_ROOT_PATH "${DMZ_TOOLCHAINS_DIR}" CACHE PATH "Find Root Path" FORCE)
if(CMAKE_OSX_SYSROOT AND EXISTS "${CMAKE_OSX_SYSROOT}")
    list(APPEND CMAKE_FIND_ROOT_PATH "${CMAKE_OSX_SYSROOT}")
endif()

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER CACHE STRING "" FORCE)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY BOTH CACHE STRING "" FORCE)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE BOTH CACHE STRING "" FORCE)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE BOTH CACHE STRING "" FORCE)

# 6. Android platform NDK integration (chain official toolchain if present)
if(EXISTS "${DMZ_TOOLCHAINS_DIR}/androidndk/build/cmake/android.toolchain.cmake")
    set(ENV{ANDROID_NDK_HOME} "${DMZ_TOOLCHAINS_DIR}/androidndk")
    include("${DMZ_TOOLCHAINS_DIR}/androidndk/build/cmake/android.toolchain.cmake")
elseif(CMAKE_SYSTEM_NAME STREQUAL "Android" AND DEFINED ENV{ANDROID_NDK_HOME} AND EXISTS "$ENV{ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake")
    include("$ENV{ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake")
endif()
"#;

        fs::write(&toolchain_cmake_path, cmake_content)
            .map_err(|e| format!("Failed to write dmz.toolchain.cmake: {}", e))?;

        Ok(())
    }

    /// Generates shell and batch activator scripts in `.dmz/` for immediate developer use.
    pub fn generate_activator_scripts(workspace_root: &Path) -> Result<(), String> {
        let env_sh_path = workspace_root.join(".dmz/env.sh");
        let env_bat_path = workspace_root.join(".dmz/env.bat");

        let mut env_sh_content = r#"#!/bin/sh
# DMZ Toolchain Environment Activator
# Source this script to activate bundled compilers and runtimes:
#   source .dmz/env.sh

# Ensure safe globbing behavior across shells (POSIX sh, bash, and zsh)
if [ -n "$ZSH_VERSION" ]; then
    setopt localoptions nonomatch nullglob 2>/dev/null || true
fi

# Determine DMZ workspace root reliably across bash, zsh, and dash/POSIX sh
if [ -n "$BASH_SOURCE" ]; then
    _DMZ_SELF="$BASH_SOURCE"
elif [ -n "$ZSH_VERSION" ]; then
    _DMZ_SELF="${(%):-%x}"
else
    _DMZ_SELF="$0"
fi

if [ -f "$_DMZ_SELF" ]; then
    DMZ_WS="$(cd "$(dirname "$_DMZ_SELF")/.." && pwd)"
elif [ -d "./.dmz" ]; then
    DMZ_WS="$(pwd)"
elif [ -d "../.dmz" ]; then
    DMZ_WS="$(cd .. && pwd)"
else
    DMZ_WS="$(cd "$(dirname "$0")/.." 2>/dev/null && pwd)"
fi
export DMZ_WORKSPACE="$DMZ_WS"

# Prepend all bundled toolchain bin directories to PATH
if [ -d "$DMZ_WS/.dmz/toolchains" ]; then
    for tc_bin in "$DMZ_WS"/.dmz/toolchains/*/bin; do
        if [ -d "$tc_bin" ]; then
            case "$tc_bin" in
                *"/applesdk/bin")
                    if [ ! -d "$DMZ_WS/.dmz/toolchains/applesdk/SDKs" ] && [ ! -d "$DMZ_WS/.dmz/toolchains/applesdk/Platforms" ]; then
                        continue
                    fi
                    ;;
            esac
            export PATH="$tc_bin:$PATH"
        fi
    done
fi

# Configure ecosystem environments
if [ -d "$DMZ_WS/.dmz/toolchains/rust" ]; then
    export RUSTUP_HOME="$DMZ_WS/.dmz/toolchains/rust"
    export CARGO_HOME="${CARGO_HOME:-$DMZ_WS/.cargo}"
    export DYLD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/rust/lib:$DYLD_LIBRARY_PATH"
    export LD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/rust/lib:$LD_LIBRARY_PATH"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/go" ] && ([ -f "$DMZ_WS/.dmz/toolchains/go/bin/go" ] || [ -f "$DMZ_WS/.dmz/toolchains/go/bin/go.exe" ]); then
    export GOROOT="$DMZ_WS/.dmz/toolchains/go"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/python" ] && ([ -d "$DMZ_WS/.dmz/toolchains/python/lib" ] || [ -d "$DMZ_WS/.dmz/toolchains/python/Lib" ]); then
    export PYTHONHOME="$DMZ_WS/.dmz/toolchains/python"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/node" ]; then
    if [ -d "$DMZ_WS/.dmz/toolchains/node/lib/node_modules" ]; then
        export NODE_PATH="$DMZ_WS/.dmz/toolchains/node/lib/node_modules${NODE_PATH:+:$NODE_PATH}"
    fi
fi

if [ -d "$DMZ_WS/.dmz/toolchains/php" ] && ([ -f "$DMZ_WS/.dmz/toolchains/php/bin/php" ] || [ -f "$DMZ_WS/.dmz/toolchains/php/php.ini" ]); then
    export PHPRC="$DMZ_WS/.dmz/toolchains/php"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/ruby" ] && ([ -f "$DMZ_WS/.dmz/toolchains/ruby/bin/ruby" ] || [ -f "$DMZ_WS/.dmz/toolchains/ruby/bin/bundle" ] || [ -d "$DMZ_WS/.dmz/toolchains/ruby/gems" ]); then
    export GEM_HOME="$DMZ_WS/.dmz/toolchains/ruby"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/cc" ]; then
    if [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/clang" ]; then
        export CC="$DMZ_WS/.dmz/toolchains/cc/bin/clang"
        export CXX="$DMZ_WS/.dmz/toolchains/cc/bin/clang++"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/gcc" ]; then
        export CC="$DMZ_WS/.dmz/toolchains/cc/bin/gcc"
        export CXX="$DMZ_WS/.dmz/toolchains/cc/bin/g++"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/cc" ]; then
        export CC="$DMZ_WS/.dmz/toolchains/cc/bin/cc"
    fi
    if [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ar" ]; then
        export AR="$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ar"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/ar" ]; then
        export AR="$DMZ_WS/.dmz/toolchains/cc/bin/ar"
    fi
    if [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ranlib" ]; then
        export RANLIB="$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ranlib"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/ranlib" ]; then
        export RANLIB="$DMZ_WS/.dmz/toolchains/cc/bin/ranlib"
    fi
fi

if [ -f "$DMZ_WS/.dmz/toolchains/cmake/bin/cmake" ]; then
    export CMAKE="$DMZ_WS/.dmz/toolchains/cmake/bin/cmake"
    if [ -d "$DMZ_WS/.dmz/toolchains/cmake/share" ]; then
        for cm_share in "$DMZ_WS"/.dmz/toolchains/cmake/share/cmake-*; do
            if [ -d "$cm_share" ]; then
                export CMAKE_ROOT="$cm_share"
                break
            fi
        done
    fi
fi

if [ -f "$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake" ] && ([ -d "$DMZ_WS/.dmz/toolchains/cmake" ] || [ -f "$DMZ_WS/CMakeLists.txt" ]); then
    export CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake"
    export HOST_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake"
    export TARGET_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake"
fi

if [ -f "$DMZ_WS/.dmz/toolchains/ninja/bin/ninja" ]; then
    export NINJA="$DMZ_WS/.dmz/toolchains/ninja/bin/ninja"
    export CMAKE_GENERATOR="Ninja"
fi

if [ -f "$DMZ_WS/.dmz/toolchains/make/bin/make" ]; then
    export MAKE="$DMZ_WS/.dmz/toolchains/make/bin/make"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/juce" ]; then
    export JUCE_PATH="$DMZ_WS/.dmz/toolchains/juce"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/androidndk" ]; then
    export ANDROID_NDK_HOME="$DMZ_WS/.dmz/toolchains/androidndk"
    export ANDROID_NDK_ROOT="$DMZ_WS/.dmz/toolchains/androidndk"
    if [ -f "$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake" ]; then
        if [ ! -f "$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake" ]; then
            export CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake"
            export HOST_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake"
            export TARGET_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake"
        fi
    fi
fi

if [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig" ]; then
    if [ -f "$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkg-config" ]; then
        export PKG_CONFIG="$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkg-config"
    elif [ -f "$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkgconf" ]; then
        export PKG_CONFIG="$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkgconf"
    fi
    if [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig/lib/pkgconfig" ] || [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig/share/pkgconfig" ]; then
        export PKG_CONFIG_PATH="$DMZ_WS/.dmz/toolchains/pkgconfig/lib/pkgconfig:$DMZ_WS/.dmz/toolchains/pkgconfig/share/pkgconfig:$PKG_CONFIG_PATH"
    fi
fi
"#
        .to_string();

        if crate::toolchain::ToolchainEnv::is_genuine_apple_developer_dir(
            &workspace_root.join(".dmz/toolchains/applesdk"),
        ) || crate::toolchain::ToolchainEnv::find_apple_sdk_root(
            &workspace_root.join(".dmz/toolchains/applesdk"),
        )
        .is_some()
        {
            env_sh_content.push_str(
                r#"
if [ -d "$DMZ_WS/.dmz/toolchains/applesdk" ]; then
    for sdk_parent in "$DMZ_WS/.dmz/toolchains/applesdk/Platforms/MacOSX.platform/Developer/SDKs" "$DMZ_WS/.dmz/toolchains/applesdk/SDKs"; do
        if [ -d "$sdk_parent" ]; then
            for sdk_path in "$sdk_parent"/MacOSX*.sdk; do
                if [ -d "$sdk_path" ]; then
                    export SDKROOT="$sdk_path"
                    break 2
                fi
            done
        fi
    done
    if [ -f "$DMZ_WS/.dmz/toolchains/applesdk/usr/bin/xcrun" ] || [ -f "$DMZ_WS/.dmz/toolchains/applesdk/bin/xcrun" ]; then
        export DEVELOPER_DIR="$DMZ_WS/.dmz/toolchains/applesdk"
    fi
fi
"#,
            );
        }

        env_sh_content.push_str(
            "\necho \"DMZ bundled toolchains activated. PATH and runtime variables configured.\"\n",
        );

        let env_bat_content = r#"@echo off
REM DMZ Toolchain Environment Activator (Windows)
set DMZ_WORKSPACE=%~dp0..

for /d %%D in ("%DMZ_WORKSPACE%\.dmz\toolchains\*") do (
    if exist "%%D\bin" (
        set "PATH=%%D\bin;%PATH%"
    )
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\rust" (
    set "RUSTUP_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\rust"
    if not defined CARGO_HOME set "CARGO_HOME=%DMZ_WORKSPACE%\.cargo"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\go\bin\go.exe" (
    set "GOROOT=%DMZ_WORKSPACE%\.dmz\toolchains\go"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\python\Lib" (
    set "PYTHONHOME=%DMZ_WORKSPACE%\.dmz\toolchains\python"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\python\lib" (
    set "PYTHONHOME=%DMZ_WORKSPACE%\.dmz\toolchains\python"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\node\lib\node_modules" (
    set "NODE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\node\lib\node_modules;%NODE_PATH%"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\php\bin\php.exe" (
    set "PHPRC=%DMZ_WORKSPACE%\.dmz\toolchains\php"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\ruby\bin\ruby.exe" (
    set "GEM_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\ruby"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang.exe"
    set "CXX=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang++.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\gcc.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\gcc.exe"
    set "CXX=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\g++.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\cc.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\cc.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ar.exe" (
    set "AR=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ar.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ar.exe" (
    set "AR=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ar.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ranlib.exe" (
    set "RANLIB=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ranlib.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ranlib.exe" (
    set "RANLIB=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ranlib.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe" (
    set "CMAKE=%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake" (
    if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake" (
        set "CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "HOST_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "TARGET_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
    ) else if exist "%DMZ_WORKSPACE%\CMakeLists.txt" (
        set "CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "HOST_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "TARGET_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
    )
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\ninja\bin\ninja.exe" (
    set "NINJA=%DMZ_WORKSPACE%\.dmz\toolchains\ninja\bin\ninja.exe"
    set "CMAKE_GENERATOR=Ninja"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\make.exe" (
    set "MAKE=%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\make.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\mingw32-make.exe" (
    set "MAKE=%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\mingw32-make.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\juce" (
    set "JUCE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\juce"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\androidndk" (
    set "ANDROID_NDK_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\androidndk"
    set "ANDROID_NDK_ROOT=%DMZ_WORKSPACE%\.dmz\toolchains\androidndk"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkgconf.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkgconf.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\lib\pkgconfig" (
    set "PKG_CONFIG_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\lib\pkgconfig;%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\share\pkgconfig;%PKG_CONFIG_PATH%"
)

echo DMZ bundled toolchains activated.
"#;

        fs::create_dir_all(workspace_root.join(".dmz"))
            .map_err(|e| format!("Failed to create .dmz directory: {}", e))?;

        fs::write(&env_sh_path, env_sh_content)
            .map_err(|e| format!("Failed to write env.sh: {}", e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&env_sh_path, fs::Permissions::from_mode(0o755));
        }

        fs::write(&env_bat_path, env_bat_content)
            .map_err(|e| format!("Failed to write env.bat: {}", e))?;

        Ok(())
    }

    /// Configures `.vscode/settings.json` so IDE extensions work out-of-the-box in air-gapped environments.
    /// In non-CMake workspaces, silences CMake Tools extension auto-probing to eliminate "Bad CMake executable: ''" popups.
    /// In CMake workspaces with bundled CMake, automatically configures cmake.cmakePath and toolchain file.
    /// Pre-configures terminal PATH and language server environments (such as rust-analyzer).
    pub fn generate_ide_settings(workspace_root: &Path) -> Result<(), String> {
        let vscode_dir = workspace_root.join(".vscode");
        let settings_path = vscode_dir.join("settings.json");

        let mut settings: serde_json::Map<String, serde_json::Value> = if settings_path.exists() {
            match fs::read_to_string(&settings_path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => serde_json::Map::new(),
            }
        } else {
            serde_json::Map::new()
        };

        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        let bundled_cmake_bin =
            crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("cmake"), "bin/cmake");
        let has_cmake =
            workspace_root.join("CMakeLists.txt").exists() || bundled_cmake_bin.is_some();

        if has_cmake {
            if let Some(ref bin) = bundled_cmake_bin {
                let rel_bin = if bin.extension().is_some_and(|e| e == "exe") {
                    "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake.exe"
                } else {
                    "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
                };
                settings.insert(
                    "cmake.cmakePath".into(),
                    serde_json::Value::String(rel_bin.into()),
                );
            }
            if toolchains_dir.join("dmz.toolchain.cmake").exists() {
                let mut configure_settings = settings
                    .get("cmake.configureSettings")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                configure_settings.insert(
                    "CMAKE_TOOLCHAIN_FILE".into(),
                    serde_json::Value::String(
                        "${workspaceFolder}/.dmz/toolchains/dmz.toolchain.cmake".into(),
                    ),
                );
                settings.insert(
                    "cmake.configureSettings".into(),
                    serde_json::Value::Object(configure_settings),
                );
            }
            if crate::toolchain::ToolchainEnv::find_binary(
                &toolchains_dir.join("ninja"),
                "bin/ninja",
            )
            .is_some()
            {
                settings.insert(
                    "cmake.generator".into(),
                    serde_json::Value::String("Ninja".into()),
                );
            }
            settings.insert(
                "cmake.configureOnOpen".into(),
                serde_json::Value::Bool(true),
            );
        } else {
            // Workspace does NOT use CMake.
            settings.insert(
                "cmake.configureOnOpen".into(),
                serde_json::Value::Bool(false),
            );
            settings.insert(
                "cmake.autoSelectActiveFolder".into(),
                serde_json::Value::Bool(false),
            );
        }

        let bundled_make_bin =
            crate::toolchain::ToolchainEnv::find_binary(&toolchains_dir.join("make"), "bin/make")
                .or_else(|| {
                    crate::toolchain::ToolchainEnv::find_binary(
                        &toolchains_dir.join("make"),
                        "bin/mingw32-make",
                    )
                });
        if let Some(ref bin) = bundled_make_bin {
            if let Some(fname) = bin.file_name().and_then(|n| n.to_str()) {
                settings.insert(
                    "makefile.makePath".into(),
                    serde_json::Value::String(format!(
                        "${{workspaceFolder}}/.dmz/toolchains/make/bin/{}",
                        fname
                    )),
                );
            }
        }

        // Configure terminal environment for bundled toolchains (e.g., Rust, Go, Python)
        if toolchains_dir.exists() {
            let mut bin_subpaths: Vec<String> = Vec::new();
            if let Ok(entries) = fs::read_dir(&toolchains_dir) {
                for entry in entries.flatten() {
                    let name_os = entry.file_name();
                    if name_os == "applesdk"
                        && !crate::toolchain::ToolchainEnv::is_genuine_apple_developer_dir(
                            &entry.path(),
                        )
                    {
                        continue;
                    }
                    let bin = entry.path().join("bin");
                    if bin.exists() {
                        let name = name_os.to_string_lossy().to_string();
                        bin_subpaths
                            .push(format!("${{workspaceFolder}}/.dmz/toolchains/{}/bin", name));
                    }
                }
            }

            if !bin_subpaths.is_empty() {
                let unix_path = format!("{}:${{env:PATH}}", bin_subpaths.join(":"));
                let win_path = format!("{};${{env:PATH}}", bin_subpaths.join(";"));

                for platform in &[
                    "terminal.integrated.env.osx",
                    "terminal.integrated.env.linux",
                ] {
                    let mut env_obj = settings
                        .get(*platform)
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    env_obj.insert("PATH".into(), serde_json::Value::String(unix_path.clone()));
                    settings.insert(platform.to_string(), serde_json::Value::Object(env_obj));
                }

                let mut win_env = settings
                    .get("terminal.integrated.env.windows")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                win_env.insert("PATH".into(), serde_json::Value::String(win_path));
                settings.insert(
                    "terminal.integrated.env.windows".into(),
                    serde_json::Value::Object(win_env),
                );
            }

            // Configure rust-analyzer if Rust is bundled
            if toolchains_dir.join("rust").exists() {
                if let Some(ra_bin) = crate::toolchain::ToolchainEnv::find_binary(
                    &toolchains_dir.join("rust"),
                    "bin/rust-analyzer",
                ) {
                    let fname = ra_bin
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("rust-analyzer");
                    settings.insert(
                        "rust-analyzer.server.path".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/rust/bin/{}",
                            fname
                        )),
                    );
                }

                for env_setting_key in &[
                    "rust-analyzer.runnables.extraEnv",
                    "rust-analyzer.cargo.extraEnv",
                ] {
                    let mut ra_extra_env = settings
                        .get(*env_setting_key)
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    ra_extra_env.insert(
                        "RUSTUP_HOME".into(),
                        serde_json::Value::String("${workspaceFolder}/.dmz/toolchains/rust".into()),
                    );
                    ra_extra_env.insert(
                        "CARGO_HOME".into(),
                        serde_json::Value::String("${workspaceFolder}/.cargo".into()),
                    );
                    if let Some(ref bin) = bundled_cmake_bin {
                        let fname = bin.file_name().and_then(|n| n.to_str()).unwrap_or("cmake");
                        ra_extra_env.insert(
                            "CMAKE".into(),
                            serde_json::Value::String(format!(
                                "${{workspaceFolder}}/.dmz/toolchains/cmake/bin/{}",
                                fname
                            )),
                        );
                    }
                    if let Some(cc_bin) = crate::toolchain::ToolchainEnv::find_binary(
                        &toolchains_dir.join("cc"),
                        "bin/clang",
                    )
                    .or_else(|| {
                        crate::toolchain::ToolchainEnv::find_binary(
                            &toolchains_dir.join("cc"),
                            "bin/gcc",
                        )
                    }) {
                        let fname = cc_bin
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("clang");
                        ra_extra_env.insert(
                            "CC".into(),
                            serde_json::Value::String(format!(
                                "${{workspaceFolder}}/.dmz/toolchains/cc/bin/{}",
                                fname
                            )),
                        );
                    }
                    if let Some(ninja_bin) = crate::toolchain::ToolchainEnv::find_binary(
                        &toolchains_dir.join("ninja"),
                        "bin/ninja",
                    ) {
                        let fname = ninja_bin
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("ninja");
                        ra_extra_env.insert(
                            "NINJA".into(),
                            serde_json::Value::String(format!(
                                "${{workspaceFolder}}/.dmz/toolchains/ninja/bin/{}",
                                fname
                            )),
                        );
                    }
                    if let Some(ref make_bin) = bundled_make_bin {
                        let fname = make_bin
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("make");
                        ra_extra_env.insert(
                            "MAKE".into(),
                            serde_json::Value::String(format!(
                                "${{workspaceFolder}}/.dmz/toolchains/make/bin/{}",
                                fname
                            )),
                        );
                    }
                    if let Some(pkg_bin) = crate::toolchain::ToolchainEnv::find_binary(
                        &toolchains_dir.join("pkgconfig"),
                        "bin/pkg-config",
                    )
                    .or_else(|| {
                        crate::toolchain::ToolchainEnv::find_binary(
                            &toolchains_dir.join("pkgconfig"),
                            "bin/pkgconf",
                        )
                    }) {
                        let fname = pkg_bin
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("pkg-config");
                        ra_extra_env.insert(
                            "PKG_CONFIG".into(),
                            serde_json::Value::String(format!(
                                "${{workspaceFolder}}/.dmz/toolchains/pkgconfig/bin/{}",
                                fname
                            )),
                        );
                    }
                    if let Some(sdk_path) = crate::toolchain::ToolchainEnv::find_apple_sdk_root(
                        &toolchains_dir.join("applesdk"),
                    ) {
                        if let Ok(rel) = sdk_path.strip_prefix(workspace_root) {
                            let rel_str = rel.to_string_lossy().replace('\\', "/");
                            ra_extra_env.insert(
                                "SDKROOT".into(),
                                serde_json::Value::String(format!(
                                    "${{workspaceFolder}}/{}",
                                    rel_str
                                )),
                            );
                        }
                    }
                    settings.insert(
                        (*env_setting_key).into(),
                        serde_json::Value::Object(ra_extra_env),
                    );
                }
            }

            // Configure Python if Python is bundled with a verified interpreter binary
            if toolchains_dir.join("python").exists() {
                let py_dir = toolchains_dir.join("python");
                let has_py_bin = py_dir.join("bin/python3").exists()
                    || py_dir.join("bin/python").exists()
                    || py_dir.join("bin/pip").exists()
                    || py_dir.join("python.exe").exists()
                    || py_dir.join("bin/python.exe").exists();
                if has_py_bin {
                    let py_bin = if cfg!(windows) {
                        "${workspaceFolder}/.dmz/toolchains/python/python.exe"
                    } else {
                        "${workspaceFolder}/.dmz/toolchains/python/bin/python3"
                    };
                    settings.insert(
                        "python.defaultInterpreterPath".into(),
                        serde_json::Value::String(py_bin.into()),
                    );
                }
            }

            // Configure Go if Go is bundled with a verified go binary
            if toolchains_dir.join("go").exists() {
                let go_dir = toolchains_dir.join("go");
                if let Some(go_bin) = crate::toolchain::ToolchainEnv::find_binary(&go_dir, "bin/go")
                {
                    let fname = go_bin.file_name().and_then(|n| n.to_str()).unwrap_or("go");
                    settings.insert(
                        "go.goroot".into(),
                        serde_json::Value::String("${workspaceFolder}/.dmz/toolchains/go".into()),
                    );
                    let mut alt_tools = settings
                        .get("go.alternateTools")
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    alt_tools.insert(
                        "go".into(),
                        serde_json::Value::String(format!(
                            "${{workspaceFolder}}/.dmz/toolchains/go/bin/{}",
                            fname
                        )),
                    );
                    settings.insert(
                        "go.alternateTools".into(),
                        serde_json::Value::Object(alt_tools),
                    );
                }
            }
        }

        fs::create_dir_all(&vscode_dir)
            .map_err(|e| format!("Failed to create .vscode directory: {}", e))?;

        let json_string = serde_json::to_string_pretty(&serde_json::Value::Object(settings))
            .map_err(|e| format!("Failed to serialize settings.json: {}", e))?;

        fs::write(&settings_path, json_string)
            .map_err(|e| format!("Failed to write .vscode/settings.json: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_non_cmake_workspace_silences_cmake_in_ide_settings() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Workspace has no CMakeLists.txt and no bundled cmake
        fs::create_dir_all(ws.join(".dmz/toolchains/rust/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/rust/bin/cargo"), "cargo").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let settings_path = ws.join(".vscode/settings.json");
        assert!(settings_path.exists());
        let content = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(val["cmake.configureOnOpen"], false);
        assert_eq!(val["cmake.autoSelectActiveFolder"], false);
        assert!(val.get("cmake.cmakePath").is_none());

        // Check terminal environment has rust toolchain
        let osx_path = val["terminal.integrated.env.osx"]["PATH"].as_str().unwrap();
        assert!(osx_path.contains(".dmz/toolchains/rust/bin"));

        // dmz.toolchain.cmake must NOT exist
        assert!(!ws.join(".dmz/toolchains/dmz.toolchain.cmake").exists());
    }

    #[test]
    fn test_cmake_workspace_configures_cmake_in_ide_settings() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::write(ws.join("CMakeLists.txt"), "project(test)").unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/cmake/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cmake/bin/cmake"), "cmake").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let settings_path = ws.join(".vscode/settings.json");
        assert!(settings_path.exists());
        let content = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(val["cmake.configureOnOpen"], true);
        assert_eq!(
            val["cmake.cmakePath"],
            "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
        );
        assert!(ws.join(".dmz/toolchains/dmz.toolchain.cmake").exists());
    }

    #[test]
    fn test_bundled_cmake_without_root_cmakelists_configures_cmake_path_and_toolchain() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Rust workspace where CMake is bundled for a dependency (e.g. libz-ng-sys -> cmake) without a root CMakeLists.txt
        fs::create_dir_all(ws.join(".dmz/toolchains/cmake/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cmake/bin/cmake"), "cmake").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let settings_path = ws.join(".vscode/settings.json");
        assert!(settings_path.exists());
        let content = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            val["cmake.cmakePath"],
            "${workspaceFolder}/.dmz/toolchains/cmake/bin/cmake"
        );
        assert_eq!(
            val["cmake.configureSettings"]["CMAKE_TOOLCHAIN_FILE"],
            "${workspaceFolder}/.dmz/toolchains/dmz.toolchain.cmake"
        );
        assert!(ws.join(".dmz/toolchains/dmz.toolchain.cmake").exists());
    }

    #[test]
    fn test_rust_analyzer_cargo_extra_env_and_verified_toolchain_binaries() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::create_dir_all(ws.join(".dmz/toolchains/rust/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/rust/bin/cargo"), "cargo").unwrap();
        fs::write(
            ws.join(".dmz/toolchains/rust/bin/rust-analyzer"),
            "rust-analyzer",
        )
        .unwrap();

        // Empty python and go dirs without binaries must NOT write broken IDE paths
        fs::create_dir_all(ws.join(".dmz/toolchains/python")).unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/go")).unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let content = fs::read_to_string(ws.join(".vscode/settings.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            val["rust-analyzer.cargo.extraEnv"]["RUSTUP_HOME"],
            "${workspaceFolder}/.dmz/toolchains/rust"
        );
        assert_eq!(
            val["rust-analyzer.cargo.extraEnv"]["CARGO_HOME"],
            "${workspaceFolder}/.cargo"
        );
        assert!(val.get("python.defaultInterpreterPath").is_none());
        assert!(val.get("go.goroot").is_none());
    }

    #[test]
    fn test_generate_clang_wrappers_and_idempotency() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Simulate cc toolchain with clang, clang++ binaries
        let cc_bin = ws.join(".dmz/toolchains/cc/bin");
        fs::create_dir_all(&cc_bin).unwrap();
        fs::write(cc_bin.join("clang"), "#!/bin/sh\necho real-clang").unwrap();
        fs::write(cc_bin.join("clang++"), "#!/bin/sh\necho real-clang++").unwrap();

        // Simulate applesdk
        let sdk_dir = ws.join(".dmz/toolchains/applesdk/SDKs/MacOSX.sdk/usr/include");
        fs::create_dir_all(&sdk_dir).unwrap();
        fs::write(sdk_dir.join("stdio.h"), "/* stdio */").unwrap();
        fs::write(sdk_dir.join("zlib.h"), "/* zlib */").unwrap();

        // Setup toolchains
        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        #[cfg(unix)]
        {
            // Verify clang.real and clang++.real exist and have original contents
            let real_clang = cc_bin.join("clang.real");
            let real_clangxx = cc_bin.join("clang++.real");
            assert!(real_clang.exists());
            assert!(real_clangxx.exists());
            assert_eq!(
                fs::read_to_string(&real_clang).unwrap(),
                "#!/bin/sh\necho real-clang"
            );
            assert_eq!(
                fs::read_to_string(&real_clangxx).unwrap(),
                "#!/bin/sh\necho real-clang++"
            );

            // Verify clang and clang++ are hermetic wrappers
            let clang_wrapper = fs::read_to_string(cc_bin.join("clang")).unwrap();
            assert!(clang_wrapper.contains("DMZ Hermetic Clang Wrapper"));
            assert!(clang_wrapper.contains("-isysroot"));

            // Verify cc wrapper was generated as well
            let cc_path = cc_bin.join("cc");
            assert!(cc_path.exists());
            let cc_wrapper = fs::read_to_string(&cc_path).unwrap();
            assert!(cc_wrapper.contains("DMZ Hermetic Clang Wrapper"));

            // Verify permissions are 0o755
            use std::os::unix::fs::PermissionsExt;
            for p in &[&real_clang, &real_clangxx, &cc_bin.join("clang"), &cc_path] {
                let mode = fs::metadata(p).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "Must be executable: {:?}", p);
            }

            // Verify idempotency: running setup again must preserve real binaries
            ToolchainSetup::setup_imported_toolchains(ws).unwrap();
            assert_eq!(
                fs::read_to_string(&real_clang).unwrap(),
                "#!/bin/sh\necho real-clang"
            );
            assert_eq!(
                fs::read_to_string(&real_clangxx).unwrap(),
                "#!/bin/sh\necho real-clang++"
            );
        }
    }

    #[test]
    fn test_cmake_toolchain_with_apple_sdk_sysroot() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::write(ws.join("CMakeLists.txt"), "project(test)").unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/cmake/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cmake/bin/cmake"), "cmake").unwrap();
        fs::create_dir_all(ws.join(".dmz/toolchains/cc/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/cc/bin/clang"), "clang").unwrap();

        let sdk_dir = ws.join(".dmz/toolchains/applesdk/SDKs/MacOSX.sdk/usr/include");
        fs::create_dir_all(&sdk_dir).unwrap();
        fs::write(sdk_dir.join("zlib.h"), "/* zlib */").unwrap();

        ToolchainSetup::setup_imported_toolchains(ws).unwrap();

        let cmake_tc = ws.join(".dmz/toolchains/dmz.toolchain.cmake");
        assert!(cmake_tc.exists());
        let content = fs::read_to_string(&cmake_tc).unwrap();

        // Must configure Darwin and CMAKE_OSX_SYSROOT prior to compiler check
        assert!(content.contains("CMAKE_SYSTEM_NAME \"Darwin\""));
        assert!(content.contains("CMAKE_OSX_SYSROOT"));
        assert!(content.contains("CMAKE_FIND_ROOT_PATH_MODE_INCLUDE BOTH"));
        assert!(content.contains("CMAKE_FIND_ROOT_PATH_MODE_LIBRARY BOTH"));

        // env.sh must export SDKROOT and NOT export DEVELOPER_DIR (since no xcrun)
        let env_sh = fs::read_to_string(ws.join(".dmz/env.sh")).unwrap();
        assert!(env_sh.contains("export SDKROOT="));
        assert!(env_sh.contains("[ -f \"$DMZ_WS/.dmz/toolchains/applesdk/usr/bin/xcrun\" ]"));
    }
}
