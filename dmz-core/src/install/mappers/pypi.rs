use crate::install::archive::SafeArchiveExtractor;
use crate::install::fast_clone::FastClone;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use std::fs;
use std::path::Path;

/// Python / PyPI site-packages mapper (Production-Grade)
pub struct PyPiMapper;

impl PyPiMapper {
    /// Detects the target Python version from existing environments or bundled toolchains,
    /// defaulting to `python3.11` for backward compatibility.
    pub fn detect_python_version(workspace_path: &Path) -> String {
        // 1. Check existing .dmz/venv/lib/python3.X
        let dmz_venv_lib = workspace_path.join(".dmz").join("venv").join("lib");
        if let Ok(entries) = fs::read_dir(&dmz_venv_lib) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.starts_with("python3.") && entry.path().is_dir() {
                    return s.to_string();
                }
            }
        }

        // 2. Check existing .venv/lib/python3.X
        let venv_lib = workspace_path.join(".venv").join("lib");
        if let Ok(entries) = fs::read_dir(&venv_lib) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.starts_with("python3.") && entry.path().is_dir() {
                    return s.to_string();
                }
            }
        }

        // 3. Check bundled toolchain .dmz/toolchains/python/lib/python3.X
        let tc_lib = workspace_path
            .join(".dmz")
            .join("toolchains")
            .join("python")
            .join("lib");
        if let Ok(entries) = fs::read_dir(&tc_lib) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.starts_with("python3.") && entry.path().is_dir() {
                    return s.to_string();
                }
            }
        }

        "python3.11".to_string()
    }

    /// Writes PEP 376 INSTALLER metadata attribution (`dmz\n`) into any `.dist-info` directories.
    fn write_installer_attribution(site_packages: &Path) {
        if let Ok(entries) = fs::read_dir(site_packages) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|s| s.ends_with(".dist-info"))
                {
                    let installer_file = path.join("INSTALLER");
                    let _ = fs::write(installer_file, "dmz\n");
                }
            }
        }
    }

    /// Hydrates cross-platform CLI console script shims declared in `*.dist-info/entry_points.txt`.
    fn hydrate_console_scripts(workspace_path: &Path, site_packages: &Path, py_ver: &str) {
        let entries = match fs::read_dir(site_packages) {
            Ok(e) => e,
            Err(_) => return,
        };

        let venv_bin = workspace_path.join(".venv").join("bin");
        let dmz_venv_bin = workspace_path.join(".dmz").join("venv").join("bin");

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.ends_with(".dist-info") {
                continue;
            }

            let entry_points_file = path.join("entry_points.txt");
            if !entry_points_file.is_file() {
                continue;
            }

            if let Ok(content) = fs::read_to_string(&entry_points_file) {
                let mut in_console_scripts = false;
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        in_console_scripts =
                            trimmed == "[console_scripts]" || trimmed == "[gui_scripts]";
                        continue;
                    }
                    if in_console_scripts && !trimmed.is_empty() && !trimmed.starts_with('#') {
                        if let Some((cmd_raw, target_raw)) = trimmed.split_once('=') {
                            let cmd_name = cmd_raw.trim();
                            let target = target_raw.trim();
                            if let Some((module_raw, func_raw)) = target.split_once(':') {
                                let module = module_raw.trim();
                                let func_clean = func_raw
                                    .split(|c: char| c.is_whitespace() || c == '[')
                                    .next()
                                    .unwrap_or("main")
                                    .trim();
                                if !cmd_name.is_empty() && !module.is_empty() {
                                    Self::write_python_cmd_shim_triplet(
                                        &venv_bin, cmd_name, module, func_clean, py_ver,
                                    );
                                    Self::write_python_cmd_shim_triplet(
                                        &dmz_venv_bin,
                                        cmd_name,
                                        module,
                                        func_clean,
                                        py_ver,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Writes the cross-platform cmd-shim triplet for a Python console script:
    /// 1. POSIX sh wrapper (macOS, Linux)
    /// 2. Windows Command Prompt .cmd batch wrapper
    /// 3. Windows PowerShell .ps1 wrapper
    fn write_python_cmd_shim_triplet(
        bin_dir: &Path,
        cmd_name: &str,
        module: &str,
        func: &str,
        py_ver: &str,
    ) {
        let _ = fs::create_dir_all(bin_dir);

        // 1. POSIX shell wrapper (macOS, Linux)
        #[cfg(unix)]
        {
            let sh_path = bin_dir.join(cmd_name);
            let sh_content = format!(
                r#"#!/bin/sh
VENV_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export PYTHONPATH="$VENV_DIR/lib/{py_ver}/site-packages:$PYTHONPATH"
if [ -x "$VENV_DIR/bin/python3" ]; then
  exec "$VENV_DIR/bin/python3" -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
elif [ -x "$VENV_DIR/../.dmz/toolchains/python/bin/python3" ]; then
  exec "$VENV_DIR/../.dmz/toolchains/python/bin/python3" -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
else
  exec python3 -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
fi
"#,
                py_ver = py_ver,
                module = module,
                func = func,
            );
            if fs::write(&sh_path, sh_content).is_ok() {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&sh_path, fs::Permissions::from_mode(0o755));
            }
        }

        // 2. Windows Command Prompt .cmd batch wrapper
        let cmd_path = bin_dir.join(format!("{}.cmd", cmd_name));
        let cmd_content = format!(
            r#"@echo off
setlocal
set "VENV_DIR=%~dp0.."
set "PYTHONPATH=%VENV_DIR%\lib\{py_ver}\site-packages;%PYTHONPATH%"
if exist "%VENV_DIR%\Scripts\python.exe" (
  "%VENV_DIR%\Scripts\python.exe" -c "import sys; from {module} import {func}; sys.exit({func}())" %*
) else (
  python -c "import sys; from {module} import {func}; sys.exit({func}())" %*
)
"#,
            py_ver = py_ver,
            module = module,
            func = func,
        );
        let _ = fs::write(&cmd_path, cmd_content);

        // 3. Windows PowerShell .ps1 wrapper
        let ps1_path = bin_dir.join(format!("{}.ps1", cmd_name));
        let ps1_content = format!(
            r#"$VenvDir = Split-Path (Split-Path $PSScriptRoot -Parent)
$env:PYTHONPATH = "$VenvDir/lib/{py_ver}/site-packages;$env:PYTHONPATH"
python -c "import sys; from {module} import {func}; sys.exit({func}())" @args
"#,
            py_ver = py_ver,
            module = module,
            func = func,
        );
        let _ = fs::write(&ps1_path, ps1_content);
    }

    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let py_ver = Self::detect_python_version(workspace_path);
        let venv_site_packages = workspace_path
            .join(".dmz")
            .join("venv")
            .join("lib")
            .join(&py_ver)
            .join("site-packages");

        fs::create_dir_all(&venv_site_packages).map_err(|e| {
            InstallError::IoError(format!(
                "Failed to create python site-packages dir: {}",
                e
            ))
        })?;

        SafeArchiveExtractor::extract(archive_path, &venv_site_packages).map_err(|e| {
            InstallError::MapperError(format!(
                "Failed to unpack Python package {}: {}",
                pkg.name, e
            ))
        })?;

        // Write PEP 376 INSTALLER metadata attribution
        Self::write_installer_attribution(&venv_site_packages);

        // Hydrate console scripts (e.g., uvicorn, flask, pytest, jinja2)
        Self::hydrate_console_scripts(workspace_path, &venv_site_packages, &py_ver);

        // Mirror/link into workspace .venv so virtual environment commands and IDE static analyzers
        // (Pylance/Pyright) can locate dependencies directly inside .venv/lib/<py_ver>/site-packages
        let root_venv = workspace_path.join(".venv");
        let root_site_packages = root_venv.join("lib").join(&py_ver).join("site-packages");
        let _ = fs::create_dir_all(&root_site_packages);
        let _ = FastClone::clone_dir_recursive(&venv_site_packages, &root_site_packages);
        #[cfg(unix)]
        {
            let py3_alias = root_venv.join("lib").join("python3");
            if !py3_alias.exists() && fs::symlink_metadata(&py3_alias).is_err() {
                let _ = std::os::unix::fs::symlink(&py_ver, &py3_alias);
            }
        }
        let pth_file = root_site_packages.join("dmz_store.pth");
        let _ = fs::write(
            pth_file,
            format!("../../../../.dmz/venv/lib/{}/site-packages\n", py_ver),
        );

        // Hydrate runnable python interpreter launcher and pyvenv.cfg
        Self::hydrate_venv_environment(workspace_path, &root_venv, &py_ver);

        Ok(())
    }

    /// Hydrates runnable python interpreter launcher scripts and configuration into .venv
    pub fn hydrate_venv_environment(_workspace_path: &Path, root_venv: &Path, py_ver: &str) {
        let venv_bin = root_venv.join("bin");
        let venv_scripts = root_venv.join("Scripts");
        let _ = fs::create_dir_all(&venv_bin);
        let _ = fs::create_dir_all(&venv_scripts);

        // 1. pyvenv.cfg (always overwrite to eliminate any leaked host paths)
        let pyvenv_cfg = root_venv.join("pyvenv.cfg");
        let _ = fs::remove_file(&pyvenv_cfg);
        let ver_clean = py_ver.trim_start_matches("python");
        let cfg_content = format!(
            "home = /usr/bin\ninclude-system-site-packages = false\nversion = {}.0\n",
            ver_clean
        );
        let _ = fs::write(&pyvenv_cfg, cfg_content);

        // 2. Unix launcher: root_venv/bin/python and root_venv/bin/python3
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let sh_content = format!(
                r#"#!/bin/sh
VENV_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export VIRTUAL_ENV="$VENV_DIR"
export PATH="$VENV_DIR/bin:$PATH"
export PYTHONPATH="$VENV_DIR/lib/{py_ver}/site-packages:${{PYTHONPATH:-}}"

# Check bundled toolchain python across single-repo and umbrella member depths
_PY=""
_PY_HOME=""
for _CAND_ROOT in \
    "$VENV_DIR/../.dmz/toolchains/python" \
    "$VENV_DIR/../../.dmz/toolchains/python" \
    "$VENV_DIR/../../../.dmz/toolchains/python" \
    "$VENV_DIR/.dmz/toolchains/python"; do
    if [ -x "$_CAND_ROOT/bin/python3" ]; then
        _PY="$_CAND_ROOT/bin/python3"
        _PY_HOME="$(cd "$_CAND_ROOT" 2>/dev/null && pwd)"
        break
    elif [ -x "$_CAND_ROOT/bin/python" ]; then
        _PY="$_CAND_ROOT/bin/python"
        _PY_HOME="$(cd "$_CAND_ROOT" 2>/dev/null && pwd)"
        break
    fi
done

if [ -n "$_PY_HOME" ]; then
    export PYTHONHOME="$_PY_HOME"
    export DYLD_FALLBACK_LIBRARY_PATH="$_PY_HOME/lib:${{DYLD_FALLBACK_LIBRARY_PATH:-}}"
    export LD_LIBRARY_PATH="$_PY_HOME/lib:${{LD_LIBRARY_PATH:-}}"
elif command -v python3 >/dev/null 2>&1; then
    _PY="python3"
elif command -v python >/dev/null 2>&1; then
    _PY="python"
fi

if [ -n "$_PY" ]; then
    exec "$_PY" "$@"
else
    echo "DMZ Error: No Python interpreter found in bundled toolchains or system PATH." >&2
    exit 1
fi
"#,
                py_ver = py_ver
            );

            for name in &["python", "python3"] {
                let bin_path = venv_bin.join(name);
                let _ = fs::remove_file(&bin_path);
                if fs::write(&bin_path, &sh_content).is_ok() {
                    let _ = fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755));
                }
            }

            // Also create activate script
            let activate_path = venv_bin.join("activate");
            let _ = fs::remove_file(&activate_path);
            let activate_content = r#"VIRTUAL_ENV="$(cd "$(dirname "$0")/.." && pwd)"
export VIRTUAL_ENV
_OLD_PATH="$PATH"
PATH="$VIRTUAL_ENV/bin:$PATH"
export PATH
"#;
            let _ = fs::write(&activate_path, activate_content);
        }

        // 3. Windows launcher: root_venv/Scripts/python.bat
        let cmd_path = venv_scripts.join("python.bat");
        let _ = fs::remove_file(&cmd_path);
        let cmd_content = format!(
            r#"@echo off
set "VENV_DIR=%~dp0.."
set "VIRTUAL_ENV=%VENV_DIR%"
set "PATH=%VENV_DIR%\Scripts;%PATH%"
set "PYTHONPATH=%VENV_DIR%\lib\{py_ver}\site-packages;%PYTHONPATH%"

set "_PY=python"
if exist "%VENV_DIR%\..\.dmz\toolchains\python\python.exe" (
    set "_PY=%VENV_DIR%\..\.dmz\toolchains\python\python.exe"
) else if exist "%VENV_DIR%\..\..\.dmz\toolchains\python\python.exe" (
    set "_PY=%VENV_DIR%\..\..\.dmz\toolchains\python\python.exe"
) else if exist "%VENV_DIR%\..\..\..\.dmz\toolchains\python\python.exe" (
    set "_PY=%VENV_DIR%\..\..\..\.dmz\toolchains\python\python.exe"
)

"%_PY%" %*
"#,
            py_ver = py_ver
        );
        let _ = fs::write(&cmd_path, cmd_content);
    }
}

