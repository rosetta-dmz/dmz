// Hermetic compiler wrappers and Node package manager symlinks

use super::super::ToolchainError;
#[cfg(unix)]
use std::fs;
use std::path::Path;

/// Generate hermetic wrapper scripts for Clang/LLVM compiler binaries to ensure
/// the Apple SDK sysroot (-isysroot) is automatically passed on macOS.
#[cfg(unix)]
pub fn generate_clang_wrappers(workspace_root: &Path) -> Result<(), ToolchainError> {
    let cc_bin = workspace_root.join(".dmz/toolchains/cc/bin");
    if !cc_bin.is_dir() {
        return Ok(());
    }

    use std::os::unix::fs::PermissionsExt;

    let wrapper_script = r#"#!/bin/sh
# DMZ Hermetic Clang Wrapper
# Automatically injects bundled Apple SDK sysroot when compiling on macOS

# Resolve self path and directory purely using POSIX shell builtins (zero external utilities)
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

# Locate real compiler binary
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

# Locate Apple SDK root
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

# Check if -isysroot or --sysroot is already specified in arguments
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

# Locate Clang builtin resource directory (contains include/stdarg.h, include/stddef.h)
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

            let is_already_wrapper = if orig_path.is_file() {
                fs::read_to_string(&orig_path)
                    .map(|content| content.contains("# DMZ Hermetic Clang Wrapper"))
                    .unwrap_or(false)
            } else {
                false
            };

            if !is_already_wrapper {
                if !real_path.exists() {
                    if is_symlink {
                        let _ = fs::remove_file(&orig_path);
                    } else {
                        fs::rename(&orig_path, &real_path).map_err(|e| {
                            ToolchainError::SetupFailed(format!(
                                "Failed to rename {} to {}.real: {}",
                                name, name, e
                            ))
                        })?;
                    }
                } else if is_symlink {
                    let _ = fs::remove_file(&orig_path);
                }
            }

            fs::write(&orig_path, wrapper_script).map_err(|e| {
                ToolchainError::SetupFailed(format!(
                    "Failed to write wrapper script for {}: {}",
                    name, e
                ))
            })?;
            let _ = fs::set_permissions(&orig_path, fs::Permissions::from_mode(0o755));
            if real_path.exists() {
                let _ = fs::set_permissions(&real_path, fs::Permissions::from_mode(0o755));
            }
        } else if real_path.exists() {
            fs::write(&orig_path, wrapper_script).map_err(|e| {
                ToolchainError::SetupFailed(format!(
                    "Failed to write wrapper script for {}: {}",
                    name, e
                ))
            })?;
            let _ = fs::set_permissions(&orig_path, fs::Permissions::from_mode(0o755));
            let _ = fs::set_permissions(&real_path, fs::Permissions::from_mode(0o755));
        } else if (*name == "cc"
            && (cc_bin.join("clang.real").exists() || cc_bin.join("clang").exists()))
            || (*name == "c++"
                && (cc_bin.join("clang++.real").exists() || cc_bin.join("clang++").exists()))
        {
            fs::write(&orig_path, wrapper_script).map_err(|e| {
                ToolchainError::SetupFailed(format!(
                    "Failed to write wrapper script for {}: {}",
                    name, e
                ))
            })?;
            let _ = fs::set_permissions(&orig_path, fs::Permissions::from_mode(0o755));
        }
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn generate_clang_wrappers(_workspace_root: &Path) -> Result<(), ToolchainError> {
    Ok(())
}

/// Ensures Node.js companion package managers (npm, npx, pnpm, yarn) are correctly wired
/// to their module entry points in `lib/node_modules/`.
pub fn setup_node_package_managers(workspace_root: &Path) -> Result<(), ToolchainError> {
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
