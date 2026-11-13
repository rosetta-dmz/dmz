use crate::install::archive::SafeArchiveExtractor;
use crate::install::fast_clone::FastClone;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::path::Path;

/// Standard npm and Yarn classic mapper (Flat/nested node_modules)
pub struct NpmMapper;

impl NpmMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let target_dir = workspace_path.join("node_modules").join(&pkg.name);
        fs::create_dir_all(&target_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create node_modules dir: {}", e))
        })?;

        Self::unpack_archive(archive_path, &target_dir, &pkg.name)?;
        Self::link_package_binaries(workspace_path, &target_dir, &pkg.name)?;

        Ok(())
    }

    pub fn link_package_binaries(
        workspace_path: &Path,
        target_dir: &Path,
        pkg_name: &str,
    ) -> Result<(), InstallError> {
        let pkg_json_path = target_dir.join("package.json");
        if !pkg_json_path.exists() {
            return Ok(());
        }

        if let Ok(content) = fs::read_to_string(&pkg_json_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let bin_dir = workspace_path.join("node_modules").join(".bin");
                fs::create_dir_all(&bin_dir).map_err(|e| {
                    InstallError::IoError(format!(
                        "Failed to create npm bin dir {:?}: {}",
                        bin_dir, e
                    ))
                })?;

                let mut bin_map = std::collections::HashMap::new();
                if let Some(bin_obj) = json.get("bin").and_then(|b| b.as_object()) {
                    for (k, v) in bin_obj {
                        if let Some(path_str) = v.as_str() {
                            bin_map.insert(k.clone(), path_str.to_string());
                        }
                    }
                } else if let Some(bin_str) = json.get("bin").and_then(|b| b.as_str()) {
                    let short_name = pkg_name.split('/').next_back().unwrap_or(pkg_name);
                    bin_map.insert(short_name.to_string(), bin_str.to_string());
                }

                for (cmd_name, rel_target) in bin_map {
                    let full_target =
                        format!("{}/{}", pkg_name, rel_target.trim_start_matches("./"));
                    let fallback = if pkg_name.contains("tauri") {
                        Some("cargo tauri")
                    } else {
                        None
                    };
                    Self::write_cmd_shim_triplet(&bin_dir, &cmd_name, &full_target, fallback);
                }
            }
        }

        Ok(())
    }

    /// Hydrates cross-platform CLI binary shims in `node_modules/.bin/` for all declared
    /// or installed executables in the workspace without hardcoded framework checks.
    pub fn hydrate_workspace_bin_shims(workspace_path: &Path) {
        let bin_dir = workspace_path.join("node_modules/.bin");
        if fs::create_dir_all(&bin_dir).is_err() {
            return;
        }

        // 1. Scan installed packages in node_modules/ (including @scoped packages)
        let nm_dir = workspace_path.join("node_modules");
        if nm_dir.is_dir() {
            Self::scan_and_link_node_modules(&nm_dir, &bin_dir);
        }

        // 2. Discover from workspace root package.json for pre-install / air-gap offline readiness
        let root_pkg_json = workspace_path.join("package.json");
        if root_pkg_json.is_file() {
            Self::discover_and_link_from_root_manifest(&root_pkg_json, &bin_dir);
        }

        // 3. Ensure node_modules/.modules.yaml exists so pnpm recognizes packages as installed
        super::pnpm::PnpmMapper::ensure_modules_yaml(workspace_path);
    }

    /// Recursively scans `node_modules/` (up to 2 levels for scoped packages) and materializes shims
    fn scan_and_link_node_modules(nm_dir: &Path, bin_dir: &Path) {
        let entries = match fs::read_dir(nm_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with('.') {
                continue;
            }

            if name_str.starts_with('@') {
                if let Ok(scoped_entries) = fs::read_dir(&path) {
                    for scoped_entry in scoped_entries.flatten() {
                        let scoped_path = scoped_entry.path();
                        if scoped_path.is_dir() {
                            let pkg_name = format!(
                                "{}/{}",
                                name_str,
                                scoped_entry.file_name().to_string_lossy()
                            );
                            Self::link_single_package_binaries(&scoped_path, bin_dir, &pkg_name);
                        }
                    }
                }
            } else {
                Self::link_single_package_binaries(&path, bin_dir, &name_str);
            }
        }
    }

    fn link_single_package_binaries(pkg_dir: &Path, bin_dir: &Path, pkg_name: &str) {
        let pkg_json_path = pkg_dir.join("package.json");
        if !pkg_json_path.is_file() {
            return;
        }

        let content = match fs::read_to_string(&pkg_json_path) {
            Ok(c) => c,
            Err(_) => return,
        };

        let json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(j) => j,
            Err(_) => return,
        };

        let mut bin_map = std::collections::HashMap::new();
        if let Some(bin_obj) = json.get("bin").and_then(|b| b.as_object()) {
            for (k, v) in bin_obj {
                if let Some(path_str) = v.as_str() {
                    bin_map.insert(k.clone(), path_str.to_string());
                }
            }
        } else if let Some(bin_str) = json.get("bin").and_then(|b| b.as_str()) {
            let short_name = pkg_name.split('/').next_back().unwrap_or(pkg_name);
            bin_map.insert(short_name.to_string(), bin_str.to_string());
        }

        for (cmd_name, rel_target) in bin_map {
            let full_target = format!("{}/{}", pkg_name, rel_target.trim_start_matches("./"));
            let fallback = if pkg_name.contains("tauri") {
                Some("cargo tauri")
            } else {
                None
            };
            Self::write_cmd_shim_triplet(bin_dir, &cmd_name, &full_target, fallback);
        }
    }

    /// Discovers dependencies and scripts from root package.json to materialize offline readiness shims
    fn discover_and_link_from_root_manifest(root_pkg_json: &Path, bin_dir: &Path) {
        let content = match fs::read_to_string(root_pkg_json) {
            Ok(c) => c,
            Err(_) => return,
        };

        let json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(j) => j,
            Err(_) => return,
        };

        let mut declared_deps = std::collections::HashSet::new();
        for dep_key in &["dependencies", "devDependencies", "optionalDependencies"] {
            if let Some(obj) = json.get(*dep_key).and_then(|d| d.as_object()) {
                for k in obj.keys() {
                    declared_deps.insert(k.clone());
                }
            }
        }

        // Canonical mapping for well-known and dual-stack CLI packages
        for dep in &declared_deps {
            if dep == "@tauri-apps/cli" || dep == "tauri" {
                Self::write_cmd_shim_triplet(
                    bin_dir,
                    "tauri",
                    "@tauri-apps/cli/bin/tauri.js",
                    Some("cargo tauri"),
                );
            } else if dep == "vite" {
                Self::write_cmd_shim_triplet(bin_dir, "vite", "vite/bin/vite.js", None);
            } else if dep == "eslint" {
                Self::write_cmd_shim_triplet(bin_dir, "eslint", "eslint/bin/eslint.js", None);
            } else if dep == "next" {
                Self::write_cmd_shim_triplet(bin_dir, "next", "next/dist/bin/next", None);
            } else if dep == "typescript" {
                Self::write_cmd_shim_triplet(bin_dir, "tsc", "typescript/bin/tsc", None);
                Self::write_cmd_shim_triplet(bin_dir, "tsserver", "typescript/bin/tsserver", None);
            }
        }

        // For any declared script command (e.g., "tauri": "tauri"), ensure the invoked binary is shimmed
        if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
            for (_, script_val) in scripts {
                if let Some(script_str) = script_val.as_str() {
                    let first_cmd = script_str.split_whitespace().next().unwrap_or("").trim();
                    if !first_cmd.is_empty()
                        && !first_cmd.contains('/')
                        && !first_cmd.contains('\\')
                        && !bin_dir.join(first_cmd).exists()
                    {
                        for dep in &declared_deps {
                            let short_dep = dep.split('/').next_back().unwrap_or(dep);
                            if short_dep == first_cmd || dep == first_cmd {
                                let rel_target = format!("{}/bin/{}.js", dep, first_cmd);
                                let fallback = if dep.contains("tauri") {
                                    Some("cargo tauri")
                                } else {
                                    None
                                };
                                Self::write_cmd_shim_triplet(
                                    bin_dir,
                                    first_cmd,
                                    &rel_target,
                                    fallback,
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Writes the full cross-platform cmd-shim triplet:
    /// POSIX sh wrapper (macOS, Linux, Git Bash, MSYS2)
    /// Windows Command Prompt .cmd batch wrapper
    /// Windows PowerShell .ps1 wrapper
    pub fn write_cmd_shim_triplet(
        bin_dir: &Path,
        cmd_name: &str,
        rel_js_target: &str,
        fallback_cmd: Option<&str>,
    ) {
        let sh_shim_path = bin_dir.join(cmd_name);
        let cmd_shim_path = bin_dir.join(format!("{}.cmd", cmd_name));
        let ps1_shim_path = bin_dir.join(format!("{}.ps1", cmd_name));

        let rel_js_target_win = rel_js_target.replace('/', "\\");

        // 1. POSIX Shell Script wrapper
        if !sh_shim_path.exists() {
            let fallback_sh = match fallback_cmd {
                Some(fb) => format!("exec {} \"$@\" 2>/dev/null || exec npx {} \"$@\"\n", fb, cmd_name),
                None => format!("exec npx {} \"$@\" 2>/dev/null || exec node \"$_APP_DIR/node_modules/{}\" \"$@\"\n", cmd_name, rel_js_target),
            };

            let sh_content = format!(
                r#"#!/bin/sh
# DMZ Hermetic CLI shim: {cmd}
_BIN_DIR="$(cd "$(dirname "$0")" && pwd)"
_APP_DIR="$(cd "$_BIN_DIR/../.." && pwd)"
_ROOT_DIR="$(cd "$_APP_DIR/.." && pwd)"
_MASTER_DIR="$(cd "$_APP_DIR/../.." && pwd)"

_NODE="${{DMZ_NODE:-node}}"
if [ -f "$_APP_DIR/.dmz/toolchains/node/bin/node" ]; then
    _NODE="$_APP_DIR/.dmz/toolchains/node/bin/node"
elif [ -f "$_ROOT_DIR/.dmz/toolchains/node/bin/node" ]; then
    _NODE="$_ROOT_DIR/.dmz/toolchains/node/bin/node"
elif [ -f "$_MASTER_DIR/.dmz/toolchains/node/bin/node" ]; then
    _NODE="$_MASTER_DIR/.dmz/toolchains/node/bin/node"
fi

if [ -f "$_APP_DIR/node_modules/{target}" ]; then
    exec "$_NODE" "$_APP_DIR/node_modules/{target}" "$@"
elif [ -f "$_ROOT_DIR/node_modules/{target}" ]; then
    exec "$_NODE" "$_ROOT_DIR/node_modules/{target}" "$@"
elif [ -f "$_MASTER_DIR/node_modules/{target}" ]; then
    exec "$_NODE" "$_MASTER_DIR/node_modules/{target}" "$@"
fi
{fallback}"#,
                cmd = cmd_name,
                target = rel_js_target,
                fallback = fallback_sh
            );

            match fs::write(&sh_shim_path, sh_content) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Err(e) =
                            fs::set_permissions(&sh_shim_path, fs::Permissions::from_mode(0o755))
                        {
                            tracing::warn!(path = ?sh_shim_path, error = %e, "Failed to chmod .bin shim");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(path = ?sh_shim_path, error = %e, "Failed to write .bin sh shim");
                }
            }
        }

        // 2. Windows Command Prompt .cmd batch wrapper
        if !cmd_shim_path.exists() {
            let fallback_cmd_str = match fallback_cmd {
                Some(fb) => format!("{} %*", fb),
                None => format!("npx {} %*", cmd_name),
            };

            let cmd_content = format!(
                r#"@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
for %%I in ("%SCRIPT_DIR%..\..") do set "APP_DIR=%%~fI"
for %%I in ("%APP_DIR%\..") do set "ROOT_DIR=%%~fI"
for %%I in ("%APP_DIR%\..\..") do set "MASTER_DIR=%%~fI"

set "NODE_BIN=node"
if exist "%APP_DIR%\.dmz\toolchains\node\bin\node.exe" (
    set "NODE_BIN=%APP_DIR%\.dmz\toolchains\node\bin\node.exe"
) else if exist "%ROOT_DIR%\.dmz\toolchains\node\bin\node.exe" (
    set "NODE_BIN=%ROOT_DIR%\.dmz\toolchains\node\bin\node.exe"
) else if exist "%MASTER_DIR%\.dmz\toolchains\node\bin\node.exe" (
    set "NODE_BIN=%MASTER_DIR%\.dmz\toolchains\node\bin\node.exe"
)

if exist "%APP_DIR%\node_modules\{target}" (
    "%NODE_BIN%" "%APP_DIR%\node_modules\{target}" %*
) else if exist "%ROOT_DIR%\node_modules\{target}" (
    "%NODE_BIN%" "%ROOT_DIR%\node_modules\{target}" %*
) else if exist "%MASTER_DIR%\node_modules\{target}" (
    "%NODE_BIN%" "%MASTER_DIR%\node_modules\{target}" %*
) else (
    {fallback}
)
"#,
                target = rel_js_target_win,
                fallback = fallback_cmd_str
            );

            if let Err(e) = fs::write(&cmd_shim_path, cmd_content) {
                tracing::warn!(path = ?cmd_shim_path, error = %e, "Failed to write .bin .cmd shim");
            }
        }

        // 3. Windows PowerShell .ps1 wrapper
        if !ps1_shim_path.exists() {
            let fallback_ps1_str = match fallback_cmd {
                Some(fb) => format!("& {} $args", fb),
                None => format!("& npx {} $args", cmd_name),
            };

            let ps1_content = format!(
                r#"#!/usr/bin/env pwsh
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$appDir = Split-Path -Parent (Split-Path -Parent $scriptDir)
$rootDir = Split-Path -Parent $appDir

$nodeBin = "node"
$appNode = Join-Path $appDir ".dmz\toolchains\node\bin\node.exe"
$rootNode = Join-Path $rootDir ".dmz\toolchains\node\bin\node.exe"
if (Test-Path $appNode) {{
    $nodeBin = $appNode
}} elseif (Test-Path $rootNode) {{
    $nodeBin = $rootNode
}}

$appTarget = Join-Path $appDir "node_modules\{target}"
$rootTarget = Join-Path $rootDir "node_modules\{target}"

if (Test-Path $appTarget) {{
    & $nodeBin $appTarget $args
}} elseif (Test-Path $rootTarget) {{
    & $nodeBin $rootTarget $args
}} else {{
    {fallback}
}}
"#,
                target = rel_js_target_win,
                fallback = fallback_ps1_str
            );

            if let Err(e) = fs::write(&ps1_shim_path, ps1_content) {
                tracing::warn!(path = ?ps1_shim_path, error = %e, "Failed to write .bin .ps1 shim");
            }
        }
    }

    pub fn unpack_archive(
        archive_path: &Path,
        target_dir: &Path,
        pkg_name: &str,
    ) -> Result<(), InstallError> {
        let parent_store = archive_path.parent().unwrap_or(archive_path);
        let archive_stem = archive_path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("default");
        let cache_unpacked = parent_store.join(".unpacked").join(archive_stem);

        if cache_unpacked.join("package.json").exists() {
            // Instant CoW reflink or hardlink clone from unpacked cache
            fs::create_dir_all(target_dir).map_err(|e| {
                InstallError::IoError(format!(
                    "Failed to create target directory {:?}: {}",
                    target_dir, e
                ))
            })?;
            FastClone::clone_dir_recursive(&cache_unpacked, target_dir).map_err(|e| {
                InstallError::MapperError(format!(
                    "Failed to fast-clone package {}: {}",
                    pkg_name, e
                ))
            })?;
            return Ok(());
        }

        SafeArchiveExtractor::extract(archive_path, target_dir).map_err(|e| {
            InstallError::MapperError(format!("Failed to unpack package {}: {}", pkg_name, e))
        })?;

        // If unpacked archive contains a top-level "package" directory (standard npm tarball structure),
        // flatten its contents into target_dir so that package.json is directly in target_dir.
        let package_subdir = target_dir.join("package");
        if package_subdir.is_dir() && !target_dir.join("package.json").exists() {
            if let Ok(entries) = fs::read_dir(&package_subdir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(file_name) = path.file_name() {
                        let dest_path = target_dir.join(file_name);
                        fs::rename(&path, &dest_path).map_err(|e| {
                            InstallError::IoError(format!(
                                "Failed to flatten npm package entry {:?} -> {:?}: {}",
                                path, dest_path, e
                            ))
                        })?;
                    }
                }
            }
            if let Err(e) = fs::remove_dir_all(&package_subdir) {
                tracing::warn!(dir = ?package_subdir, error = %e, "Failed to remove temporary package subdir");
            }
        }

        // Seed the extracted structure in the store for instant subsequent member fast-cloning
        if !cache_unpacked.exists() {
            let _ = fs::create_dir_all(cache_unpacked.parent().unwrap_or(parent_store));
            let _ = FastClone::clone_dir_recursive(target_dir, &cache_unpacked);
            // Mark cached store files read-only (Nix store immutability)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fn make_tree_readonly(dir: &Path) {
                    if let Ok(entries) = fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_dir() {
                                make_tree_readonly(&p);
                            } else if let Ok(meta) = fs::metadata(&p) {
                                let mut perms = meta.permissions();
                                let mode = perms.mode();
                                perms.set_mode(mode & !0o222);
                                let _ = fs::set_permissions(&p, perms);
                            }
                        }
                    }
                }
                make_tree_readonly(&cache_unpacked);
            }
        }

        Ok(())
    }
}
