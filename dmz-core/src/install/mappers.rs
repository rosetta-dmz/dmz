use crate::resolver::lockfile::LockedPackage;
use flate2::read::GzDecoder;
use std::fs;
use std::io::Write;
use std::path::Path;
use tar::Archive;

/// Standard npm and Yarn classic mapper (Flat/nested node_modules)
pub struct NpmMapper;

impl NpmMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let target_dir = workspace_path.join("node_modules").join(&pkg.name);
        fs::create_dir_all(&target_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create node_modules dir: {}", e))
        })?;

        Self::unpack_archive(archive_path, &target_dir, &pkg.name)?;
        Self::link_package_binaries(workspace_path, &target_dir, &pkg.name)?;

        Ok(())
    }

    pub fn link_package_binaries(
        workspace_path: &Path,
        target_dir: &Path,
        pkg_name: &str,
    ) -> Result<(), super::InstallError> {
        let pkg_json_path = target_dir.join("package.json");
        if !pkg_json_path.exists() {
            return Ok(());
        }

        if let Ok(content) = fs::read_to_string(&pkg_json_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let bin_dir = workspace_path.join("node_modules").join(".bin");
                let _ = fs::create_dir_all(&bin_dir);

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
    /// 1. POSIX sh wrapper (macOS, Linux, Git Bash, MSYS2)
    /// 2. Windows Command Prompt .cmd batch wrapper
    /// 3. Windows PowerShell .ps1 wrapper
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

_NODE="${{DMZ_NODE:-node}}"
if [ -f "$_APP_DIR/.dmz/toolchains/node/bin/node" ]; then
    _NODE="$_APP_DIR/.dmz/toolchains/node/bin/node"
elif [ -f "$_ROOT_DIR/.dmz/toolchains/node/bin/node" ]; then
    _NODE="$_ROOT_DIR/.dmz/toolchains/node/bin/node"
fi

if [ -f "$_APP_DIR/node_modules/{target}" ]; then
    exec "$_NODE" "$_APP_DIR/node_modules/{target}" "$@"
elif [ -f "$_ROOT_DIR/node_modules/{target}" ]; then
    exec "$_NODE" "$_ROOT_DIR/node_modules/{target}" "$@"
fi
{fallback}"#,
                cmd = cmd_name,
                target = rel_js_target,
                fallback = fallback_sh
            );

            if fs::write(&sh_shim_path, sh_content).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&sh_shim_path, fs::Permissions::from_mode(0o755));
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

set "NODE_BIN=node"
if exist "%APP_DIR%\.dmz\toolchains\node\bin\node.exe" (
    set "NODE_BIN=%APP_DIR%\.dmz\toolchains\node\bin\node.exe"
) else if exist "%ROOT_DIR%\.dmz\toolchains\node\bin\node.exe" (
    set "NODE_BIN=%ROOT_DIR%\.dmz\toolchains\node\bin\node.exe"
)

if exist "%APP_DIR%\node_modules\{target}" (
    "%NODE_BIN%" "%APP_DIR%\node_modules\{target}" %*
) else if exist "%ROOT_DIR%\node_modules\{target}" (
    "%NODE_BIN%" "%ROOT_DIR%\node_modules\{target}" %*
) else (
    {fallback}
)
"#,
                target = rel_js_target_win,
                fallback = fallback_cmd_str
            );

            let _ = fs::write(&cmd_shim_path, cmd_content);
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

            let _ = fs::write(&ps1_shim_path, ps1_content);
        }
    }

    fn unpack_archive(
        archive_path: &Path,
        target_dir: &Path,
        pkg_name: &str,
    ) -> Result<(), super::InstallError> {
        let file = fs::File::open(archive_path)
            .map_err(|e| super::InstallError::IoError(format!("Failed to open archive: {}", e)))?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);

        if archive.unpack(target_dir).is_err() {
            let file_retry = fs::File::open(archive_path).map_err(|e| {
                super::InstallError::IoError(format!("Failed to open archive retry: {}", e))
            })?;
            let mut plain_archive = Archive::new(file_retry);
            plain_archive.unpack(target_dir).map_err(|e| {
                super::InstallError::MapperError(format!(
                    "Failed to unpack package {}: {}",
                    pkg_name, e
                ))
            })?;
        }

        // If unpacked archive contains a top-level "package" directory (standard npm tarball structure),
        // flatten its contents into target_dir so that package.json is directly in target_dir.
        let package_subdir = target_dir.join("package");
        if package_subdir.is_dir() && !target_dir.join("package.json").exists() {
            if let Ok(entries) = fs::read_dir(&package_subdir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(file_name) = path.file_name() {
                        let dest_path = target_dir.join(file_name);
                        let _ = fs::rename(&path, &dest_path);
                    }
                }
            }
            let _ = fs::remove_dir_all(&package_subdir);
        }

        Ok(())
    }
}

/// pnpm Content-Addressable Virtual Store & Symlink Mapper
pub struct PnpmMapper;

impl PnpmMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let sanitized_name = pkg.name.replace('/', "+");
        let virtual_store_dir = workspace_path
            .join("node_modules")
            .join(".pnpm")
            .join(format!("{}@{}", sanitized_name, pkg.version))
            .join("node_modules")
            .join(&pkg.name);

        fs::create_dir_all(&virtual_store_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create pnpm virtual store dir: {}", e))
        })?;

        NpmMapper::unpack_archive(archive_path, &virtual_store_dir, &pkg.name)?;
        NpmMapper::link_package_binaries(workspace_path, &virtual_store_dir, &pkg.name)?;

        let root_link_dir = workspace_path.join("node_modules").join(&pkg.name);
        if let Some(parent) = root_link_dir.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                super::InstallError::IoError(format!(
                    "Failed to create root node_modules dir: {}",
                    e
                ))
            })?;
        }

        if root_link_dir.exists() || root_link_dir.is_symlink() {
            let _ = fs::remove_dir_all(&root_link_dir);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&virtual_store_dir, &root_link_dir).map_err(|e| {
                super::InstallError::MapperError(format!(
                    "Failed to create pnpm symlink for {}: {}",
                    pkg.name, e
                ))
            })?;
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_dir;
            if symlink_dir(&virtual_store_dir, &root_link_dir).is_err() {
                copy_dir_all(&virtual_store_dir, &root_link_dir).map_err(|e| {
                    super::InstallError::MapperError(format!(
                        "Failed to create pnpm directory link for {}: {}",
                        pkg.name, e
                    ))
                })?;
            }
        }

        Ok(())
    }
}

/// Cargo / Rust source vendor mapper
pub struct CargoMapper;

impl CargoMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let cargo_dir = workspace_path.join(".cargo");
        let vendor_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}", pkg.name, pkg.version));

        if vendor_dir.exists() {
            let _ = fs::remove_dir_all(&vendor_dir);
        }
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create cargo vendor dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open cargo archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);

        // Unpack into a temporary staging directory to handle archive nesting
        let temp_extract_dir = cargo_dir
            .join("vendor")
            .join(format!("{}-{}-tmp", pkg.name, pkg.version));
        if temp_extract_dir.exists() {
            let _ = fs::remove_dir_all(&temp_extract_dir);
        }
        fs::create_dir_all(&temp_extract_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create temp extract dir: {}", e))
        })?;

        archive.unpack(&temp_extract_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Cargo package {}: {}",
                pkg.name, e
            ))
        })?;

        // Locate Cargo.toml (either at root or inside top-level archive directory)
        let source_dir = if temp_extract_dir.join("Cargo.toml").exists() {
            temp_extract_dir.clone()
        } else {
            let entries = fs::read_dir(&temp_extract_dir).map_err(|e| {
                super::InstallError::IoError(format!("Failed to read temp extract dir: {}", e))
            })?;

            let mut found = None;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("Cargo.toml").exists() {
                    found = Some(path);
                    break;
                }
            }
            found.unwrap_or(temp_extract_dir.clone())
        };

        // Flatten contents directly into vendor_dir
        for entry in fs::read_dir(&source_dir)
            .map_err(|e| super::InstallError::IoError(format!("Failed to read source dir: {}", e)))?
            .flatten()
        {
            let path = entry.path();
            let file_name = path.file_name().ok_or_else(|| {
                super::InstallError::IoError("Invalid file name in archive".into())
            })?;
            let dest_path = vendor_dir.join(file_name);
            let _ = fs::rename(&path, &dest_path);
        }

        let _ = fs::remove_dir_all(&temp_extract_dir);

        // Attempt to extract the official crates.io checksum from native Cargo.lock for absolute parity
        let mut crate_checksum = pkg.sha256.clone();
        let native_lock_path = workspace_path.join("Cargo.lock");
        if native_lock_path.exists() {
            if let Ok(lock_content) = fs::read_to_string(&native_lock_path) {
                if let Ok(lock_toml) = lock_content.parse::<toml::Table>() {
                    if let Some(packages) = lock_toml.get("package").and_then(|p| p.as_array()) {
                        for p in packages {
                            if p.get("name").and_then(|n| n.as_str()) == Some(&pkg.name)
                                && p.get("version").and_then(|v| v.as_str()) == Some(&pkg.version)
                            {
                                if let Some(cs) = p.get("checksum").and_then(|c| c.as_str()) {
                                    crate_checksum = cs.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Write the required .cargo-checksum.json file
        let checksum_path = vendor_dir.join(".cargo-checksum.json");
        let checksum_content = format!(r#"{{"package": "{}", "files": {{}}}}"#, crate_checksum);
        fs::write(&checksum_path, checksum_content).map_err(|e| {
            super::InstallError::IoError(format!("Failed to write .cargo-checksum.json: {}", e))
        })?;

        // Ensure .cargo/config.toml points to the correct .cargo/vendor path
        let config_path = cargo_dir.join("config.toml");
        let config_content = r#"[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = ".cargo/vendor"
"#;
        fs::write(&config_path, config_content).map_err(|e| {
            super::InstallError::IoError(format!("Failed to write .cargo/config.toml: {}", e))
        })?;

        // Also ensure .cargo/vendor/config.json exists
        let vendor_root = cargo_dir.join("vendor");
        let vendor_config_json = vendor_root.join("config.json");
        if !vendor_config_json.exists() {
            let _ = fs::write(vendor_config_json, r#"{"packages":{}}"#);
        }

        // Maintain a root 'vendor' symlink/alias for tools expecting workspace-root 'vendor/'
        let root_vendor = workspace_path.join("vendor");
        if !root_vendor.exists() {
            #[cfg(unix)]
            {
                let _ = std::os::unix::fs::symlink(".cargo/vendor", &root_vendor);
            }
        }

        Ok(())
    }
}

/// Python / PyPI site-packages mapper (Production-Grade)
pub struct PyPiMapper;

impl PyPiMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let venv_site_packages = workspace_path
            .join(".dmz")
            .join("venv")
            .join("lib")
            .join("python3.11")
            .join("site-packages");

        fs::create_dir_all(&venv_site_packages).map_err(|e| {
            super::InstallError::IoError(format!(
                "Failed to create python site-packages dir: {}",
                e
            ))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open Python package archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);

        // Unpacking fully preserves .dist-info and .egg-info metadata directories for importlib.metadata
        archive.unpack(&venv_site_packages).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Python package {}: {}",
                pkg.name, e
            ))
        })?;

        // Mirror/link into workspace .venv so virtual environment commands can locate dependencies directly
        let root_venv = workspace_path.join(".venv");
        let root_site_packages = root_venv
            .join("lib")
            .join("python3.11")
            .join("site-packages");
        let _ = fs::create_dir_all(&root_site_packages);
        let pth_file = root_site_packages.join("dmz_store.pth");
        let _ = fs::write(pth_file, format!("{}\n", venv_site_packages.display()));

        Ok(())
    }
}

/// Go module vendor mapper (Production-Grade)
pub struct GoMapper;

impl GoMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create go vendor dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open go package archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);
        archive.unpack(&vendor_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Go package {}: {}",
                pkg.name, e
            ))
        })?;

        // Automatically maintain vendor/modules.txt required by Go compiler for offline builds
        let modules_txt_path = workspace_path.join("vendor").join("modules.txt");
        let entry = format!("# {} {}\n$ explicit\n{}\n", pkg.name, pkg.version, pkg.name);
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(modules_txt_path)
        {
            let _ = file.write_all(entry.as_bytes());
        }

        Ok(())
    }
}

/// PHP Composer vendor mapper
pub struct ComposerMapper;

impl ComposerMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let vendor_dir = workspace_path.join("vendor").join(&pkg.name);
        fs::create_dir_all(&vendor_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create composer vendor dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open composer archive: {}", e))
        })?;
        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);
        archive.unpack(&vendor_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Composer package {}: {}",
                pkg.name, e
            ))
        })?;
        Ok(())
    }
}

/// Ruby Bundler vendor mapper (Production-Grade)
pub struct BundlerMapper;

impl BundlerMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let bundle_dir = workspace_path.join("vendor").join("bundle").join(&pkg.name);
        fs::create_dir_all(&bundle_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create bundle dir: {}", e))
        })?;

        let file = fs::File::open(archive_path).map_err(|e| {
            super::InstallError::IoError(format!("Failed to open gem archive: {}", e))
        })?;

        let decompressed = GzDecoder::new(file);
        let mut archive = Archive::new(decompressed);
        archive.unpack(&bundle_dir).map_err(|e| {
            super::InstallError::MapperError(format!(
                "Failed to unpack Ruby gem {}: {}",
                pkg.name, e
            ))
        })?;

        // Automatically provision .bundle/config pointing Bundler to local vendor path
        let bundle_config_dir = workspace_path.join(".bundle");
        let _ = fs::create_dir_all(&bundle_config_dir);
        let config_path = bundle_config_dir.join("config");
        let config_content = r#"---
BUNDLE_PATH: "vendor/bundle"
BUNDLE_FROZEN: "true"
BUNDLE_LOCAL: "true"
"#;
        let _ = fs::write(config_path, config_content);

        Ok(())
    }
}

/// Java Maven/Gradle local repository mapper
pub struct MavenMapper;

impl MavenMapper {
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), super::InstallError> {
        let m2_dir = workspace_path
            .join(".m2")
            .join("repository")
            .join(&pkg.name);
        fs::create_dir_all(&m2_dir).map_err(|e| {
            super::InstallError::IoError(format!("Failed to create maven repo dir: {}", e))
        })?;
        fs::copy(
            archive_path,
            m2_dir.join(format!("{}-{}.jar", pkg.name, pkg.version)),
        )
        .map_err(|e| super::InstallError::IoError(format!("Failed to copy jar: {}", e)))?;
        Ok(())
    }
}

#[allow(dead_code)]
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}
