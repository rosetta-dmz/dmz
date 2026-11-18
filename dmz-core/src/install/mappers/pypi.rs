use crate::install::archive::SafeArchiveExtractor;
use crate::install::InstallError;
use crate::resolver::lockfile::LockedPackage;
use crate::resolver::manifest::ManifestScanner;
use crate::toolchain::discovery::python::{
    find_highest_python_minor_dir, resolve_workspace_python_minor,
};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use tracing::warn;

/// Python / PyPI site-packages mapper (Production-Grade)
pub struct PyPiMapper;

impl PyPiMapper {
    fn find_python_minor_dir(lib_dir: &Path) -> Option<String> {
        find_highest_python_minor_dir(lib_dir).map(|(_, name)| name)
    }

    fn parse_python_minor_tuple(py_ver: &str) -> Option<(u8, u8)> {
        let rest = py_ver.strip_prefix("python")?;
        let (maj_str, min_str) = rest.split_once('.')?;
        Some((maj_str.parse().ok()?, min_str.parse().ok()?))
    }

    fn remove_stale_typing_backport(site_packages: &Path, target_py: Option<(u8, u8)>) {
        if !target_py.is_some_and(|v| v >= (3, 5)) {
            return;
        }
        let stale_typing = site_packages.join("typing.py");
        if stale_typing.is_file() {
            if let Err(e) = fs::remove_file(&stale_typing) {
                warn!(path = ?stale_typing, error = %e, "Failed to remove shadowed site-packages/typing.py");
            }
        }
    }

    /// Detects the target Python version from bundled toolchains, workspace manifests,
    /// or existing environments, defaulting to `python3.11` for backward compatibility.
    #[must_use]
    pub fn detect_python_version(workspace_path: &Path) -> String {
        // 1. Prefer bundled toolchain .dmz/toolchains/python/lib/python3.X
        let tc_lib = workspace_path
            .join(".dmz")
            .join("toolchains")
            .join("python")
            .join("lib");
        if let Some(ver) = Self::find_python_minor_dir(&tc_lib) {
            return ver;
        }

        // 2. Consult workspace manifest Python version constraints (.python-version / pyproject.toml / uv.lock)
        if let Some((major, minor)) = resolve_workspace_python_minor(Some(workspace_path), &[]) {
            return format!("python{major}.{minor}");
        }

        // 3. Check existing .venv/lib/python3.X
        let venv_lib = workspace_path.join(".venv").join("lib");
        if let Some(ver) = Self::find_python_minor_dir(&venv_lib) {
            return ver;
        }

        // 4. Check existing .dmz/venv/lib/python3.X
        let dmz_venv_lib = workspace_path.join(".dmz").join("venv").join("lib");
        if let Some(ver) = Self::find_python_minor_dir(&dmz_venv_lib) {
            return ver;
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
export PYTHONPATH="$VENV_DIR/../.dmz/venv/lib/{py_ver}/site-packages:$VENV_DIR/lib/{py_ver}/site-packages:${{PYTHONPATH:-}}"
if [ -x "$VENV_DIR/bin/python3" ]; then
  exec "$VENV_DIR/bin/python3" -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
elif [ -x "$VENV_DIR/../.dmz/toolchains/python/bin/python3" ]; then
  exec "$VENV_DIR/../.dmz/toolchains/python/bin/python3" -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
elif [ -x "$VENV_DIR/../../../.dmz/toolchains/python/bin/python3" ]; then
  exec "$VENV_DIR/../../../.dmz/toolchains/python/bin/python3" -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
else
  HOST_PY="$(command -v python3 2>/dev/null || true)"
  if [ -n "$HOST_PY" ] && ( [ "$HOST_PY" != "/usr/bin/python3" ] || [ -d /Library/Developer/CommandLineTools/usr/bin ] || [ -d /Applications/Xcode.app ] || [ "$(uname -s)" != "Darwin" ] ); then
    exec "$HOST_PY" -c "import sys; from {module} import {func}; sys.exit({func}())" "$@"
  fi
  echo "No non-stub python3 runtime available for {cmd_name}." >&2
  exit 127
fi
"#,
                cmd_name = cmd_name,
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
set "PYTHONPATH=%VENV_DIR%\..\.dmz\venv\lib\{py_ver}\site-packages;%VENV_DIR%\lib\{py_ver}\site-packages;%PYTHONPATH%"
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
$env:PYTHONPATH = "$VenvDir/../.dmz/venv/lib/{py_ver}/site-packages;$VenvDir/lib/{py_ver}/site-packages;$env:PYTHONPATH"
python -c "import sys; from {module} import {func}; sys.exit({func}())" @args
"#,
            py_ver = py_ver,
            module = module,
            func = func,
        );
        let _ = fs::write(&ps1_path, ps1_content);
    }

    fn extract_cpython_minor_tag(entry_name: &str) -> Option<u8> {
        let lower = entry_name.to_ascii_lowercase();
        if !(lower.ends_with(".so") || lower.ends_with(".pyd")) || lower.contains(".abi3.") {
            return None;
        }
        for marker in [".cpython-3", ".cp3"] {
            if let Some((_, rest)) = lower.split_once(marker) {
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    return digits.parse::<u8>().ok();
                }
            }
        }
        None
    }

    fn is_foreign_os_extension(entry_lower: &str, target_os: &str) -> bool {
        if target_os == "macos" {
            entry_lower.ends_with(".pyd")
                || entry_lower.contains("linux-gnu.so")
                || entry_lower.contains("linux-musl.so")
        } else if target_os == "linux" {
            entry_lower.ends_with(".pyd") || entry_lower.contains("-darwin.so")
        } else if target_os == "windows" {
            entry_lower.ends_with(".so")
        } else {
            false
        }
    }

    fn verify_wheel_extension_abi(
        pkg_name: &str,
        archive_path: &Path,
        target_py: Option<(u8, u8)>,
    ) -> Result<(), InstallError> {
        Self::verify_wheel_extension_abi_for_os(
            pkg_name,
            archive_path,
            target_py,
            std::env::consts::OS,
        )
    }

    fn verify_wheel_extension_abi_for_os(
        pkg_name: &str,
        archive_path: &Path,
        target_py: Option<(u8, u8)>,
        target_os: &str,
    ) -> Result<(), InstallError> {
        let Some((major, target_minor)) = target_py else {
            return Ok(());
        };
        let Ok(file) = fs::File::open(archive_path) else {
            return Ok(());
        };
        let Ok(archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
            return Ok(());
        };

        let mut has_compatible_ext = false;
        let mut mismatched: Vec<String> = Vec::new();

        for name_res in archive.file_names() {
            let Ok(name) = name_res else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".so") || lower.ends_with(".pyd")) {
                continue;
            }
            if Self::is_foreign_os_extension(&lower, target_os) {
                let tag = format!("foreign-os({})", name.rsplit('/').next().unwrap_or(&name));
                if !mismatched.contains(&tag) {
                    mismatched.push(tag);
                }
                continue;
            }
            if lower.contains(".abi3.") {
                has_compatible_ext = true;
                continue;
            }
            if let Some(ext_minor) = Self::extract_cpython_minor_tag(&name) {
                if ext_minor == target_minor {
                    has_compatible_ext = true;
                } else {
                    let tag = format!("cp{major}{ext_minor}");
                    if !mismatched.contains(&tag) {
                        mismatched.push(tag);
                    }
                }
            }
        }

        if !mismatched.is_empty() && !has_compatible_ext {
            return Err(InstallError::MapperError(format!(
                "Python ABI/OS mismatch for package '{}': wheel contains incompatible compiled extension(s) [{}], but target runtime is python{}.{} on {}. Re-run `dmz bundle --refresh-lock` to fetch matching wheels.",
                pkg_name,
                mismatched.join(", "),
                major,
                target_minor,
                target_os
            )));
        }

        Ok(())
    }

    /// Unpacks a Python package archive into `.dmz/venv`, creates `.venv` `.pth` wiring,
    /// and hydrates the virtual environment interpreter.
    ///
    /// # Errors
    /// Returns `InstallError` if archive extraction, `.venv` wiring, ABI validation, or I/O fails.
    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let py_ver = Self::detect_python_version(workspace_path);
        let target_py = Self::parse_python_minor_tuple(&py_ver);
        let venv_site_packages = workspace_path
            .join(".dmz")
            .join("venv")
            .join("lib")
            .join(&py_ver)
            .join("site-packages");

        let norm_name = ManifestScanner::normalize_pypi_name(&pkg.name);
        if ManifestScanner::is_obsolete_python_backport(&norm_name, target_py) {
            Self::remove_stale_typing_backport(&venv_site_packages, target_py);
            warn!(
                package = %pkg.name,
                python = %py_ver,
                "Skipping obsolete Python stdlib backport package"
            );
            return Ok(());
        }

        Self::verify_wheel_extension_abi(&pkg.name, archive_path, target_py)?;

        fs::create_dir_all(&venv_site_packages).map_err(|e| {
            InstallError::IoError(format!("Failed to create python site-packages dir: {}", e))
        })?;

        SafeArchiveExtractor::extract(archive_path, &venv_site_packages).map_err(|e| {
            InstallError::MapperError(format!(
                "Failed to unpack Python package {}: {}",
                pkg.name, e
            ))
        })?;

        Self::remove_stale_typing_backport(&venv_site_packages, target_py);

        // Write PEP 376 INSTALLER metadata attribution
        Self::write_installer_attribution(&venv_site_packages);

        // Hydrate console scripts (e.g., uvicorn, flask, pytest, jinja2)
        Self::hydrate_console_scripts(workspace_path, &venv_site_packages, &py_ver);

        // Expose `.dmz/venv/lib/<py_ver>/site-packages` via lightweight `.pth` pointers inside
        // `.venv` instead of cloning every extracted file twice across the workspace.
        let root_venv = workspace_path.join(".venv");
        let root_site_packages = root_venv.join("lib").join(&py_ver).join("site-packages");
        fs::create_dir_all(&root_site_packages).map_err(|e| {
            InstallError::IoError(format!("Failed to create .venv site-packages: {}", e))
        })?;
        let pth_target = format!("../../../../.dmz/venv/lib/{}/site-packages\n", py_ver);
        let pth_file = root_site_packages.join("dmz_store.pth");
        crate::workspace::atomic_write_file(&pth_file, pth_target.as_bytes())
            .map_err(|e| InstallError::IoError(format!("Failed to write {:?}: {}", pth_file, e)))?;
        #[cfg(unix)]
        {
            let py3_alias = root_venv.join("lib").join("python3");
            if !py3_alias.exists() && fs::symlink_metadata(&py3_alias).is_err() {
                if let Err(e) = std::os::unix::fs::symlink(&py_ver, &py3_alias) {
                    warn!(error = %e, "Failed to create python3 site-packages alias symlink");
                }
            }
        }
        #[cfg(windows)]
        {
            let py3_sp = root_venv.join("lib").join("python3").join("site-packages");
            fs::create_dir_all(&py3_sp).map_err(|e| {
                InstallError::IoError(format!("Failed to create {:?}: {}", py3_sp, e))
            })?;
            crate::workspace::atomic_write_file(
                &py3_sp.join("dmz_store.pth"),
                pth_target.as_bytes(),
            )
            .map_err(|e| InstallError::IoError(format!("Failed to write py3 .pth: {}", e)))?;

            let win_sp = root_venv.join("Lib").join("site-packages");
            fs::create_dir_all(&win_sp).map_err(|e| {
                InstallError::IoError(format!("Failed to create {:?}: {}", win_sp, e))
            })?;
            let win_pth = format!("../../../.dmz/venv/lib/{}/site-packages\n", py_ver);
            crate::workspace::atomic_write_file(&win_sp.join("dmz_store.pth"), win_pth.as_bytes())
                .map_err(|e| InstallError::IoError(format!("Failed to write Lib .pth: {}", e)))?;
        }

        // Hydrate runnable python interpreter launcher and pyvenv.cfg
        Self::hydrate_venv_environment(workspace_path, &root_venv, &py_ver)?;

        Ok(())
    }

    /// Hydrates a PEP 405 virtual environment into `.venv` using a real symlink (Unix) or
    /// interpreter copy (Windows) to the resolved Python binary — bundled `.dmz/toolchains/python`
    /// first, then host `PATH` outside `.venv` — instead of synthetic shell stubs.
    ///
    /// # Errors
    /// Returns `InstallError` if `.venv` directories, config, or Python launchers fail to write.
    pub fn hydrate_venv_environment(
        workspace_path: &Path,
        root_venv: &Path,
        py_ver: &str,
    ) -> Result<(), InstallError> {
        let path_env = std::env::var_os("PATH");
        Self::hydrate_venv_environment_with_path(
            workspace_path,
            root_venv,
            py_ver,
            path_env.as_deref(),
        )
    }

    pub(crate) fn hydrate_venv_environment_with_path(
        workspace_path: &Path,
        root_venv: &Path,
        py_ver: &str,
        path_env: Option<&OsStr>,
    ) -> Result<(), InstallError> {
        let venv_bin = root_venv.join("bin");
        let venv_scripts = root_venv.join("Scripts");
        fs::create_dir_all(&venv_bin)
            .map_err(|e| InstallError::IoError(format!("Failed to create .venv/bin: {}", e)))?;
        fs::create_dir_all(&venv_scripts)
            .map_err(|e| InstallError::IoError(format!("Failed to create .venv/Scripts: {}", e)))?;

        Self::remove_stale_python_launchers(&venv_bin, &venv_scripts, py_ver);

        // Create standard POSIX activate script regardless of interpreter discovery.
        let activate_path = venv_bin.join("activate");
        let _ = fs::remove_file(&activate_path);
        let activate_content = r#"VIRTUAL_ENV="$(cd "$(dirname "$0")/.." && pwd)"
export VIRTUAL_ENV
unset PYTHONHOME 2>/dev/null || true
_OLD_PATH="$PATH"
PATH="$VIRTUAL_ENV/bin:$PATH"
export PATH
"#;
        if let Err(e) = fs::write(&activate_path, activate_content) {
            warn!(path = ?activate_path, error = %e, "Failed to write venv activate script");
        }

        let pyvenv_cfg = root_venv.join("pyvenv.cfg");
        let _ = fs::remove_file(&pyvenv_cfg);

        let Some(resolved) =
            Self::resolve_base_python_with_path(workspace_path, root_venv, path_env)
        else {
            warn!(
                venv = ?root_venv,
                "No Python interpreter found in bundled .dmz/toolchains/python or host PATH; omitting broken .venv/bin/python"
            );
            return Ok(());
        };

        let ver_clean = py_ver.trim_start_matches("python");
        let cfg_content = format!(
            "home = {}\ninclude-system-site-packages = false\nversion = {}.0\n",
            resolved.home_dir.display(),
            ver_clean
        );
        fs::write(&pyvenv_cfg, cfg_content)
            .map_err(|e| InstallError::IoError(format!("Failed to write pyvenv.cfg: {}", e)))?;

        #[cfg(unix)]
        {
            Self::link_unix_python_binaries(&venv_bin, &resolved, py_ver)?;
        }

        #[cfg(windows)]
        {
            Self::materialize_windows_python_binaries(&venv_scripts, &resolved)?;
        }

        Ok(())
    }

    fn remove_stale_python_launchers(venv_bin: &Path, venv_scripts: &Path, py_ver: &str) {
        let mut unix_names = vec!["python", "python3"];
        if py_ver != "python" && py_ver != "python3" {
            unix_names.push(py_ver);
        }
        for name in unix_names {
            let p = venv_bin.join(name);
            if p.exists() || fs::symlink_metadata(&p).is_ok() {
                let _ = fs::remove_file(&p);
            }
        }
        for name in &["python.exe", "python3.exe", "python.bat", "python.cmd"] {
            let p = venv_scripts.join(name);
            if p.exists() || fs::symlink_metadata(&p).is_ok() {
                let _ = fs::remove_file(&p);
            }
        }
    }

    #[cfg(unix)]
    fn link_unix_python_binaries(
        venv_bin: &Path,
        resolved: &ResolvedPython,
        py_ver: &str,
    ) -> Result<(), InstallError> {
        let primary_target = if resolved.is_bundled {
            Self::relative_path_to(venv_bin, &resolved.executable)
        } else {
            resolved.executable.clone()
        };
        let py3_link = venv_bin.join("python3");
        std::os::unix::fs::symlink(&primary_target, &py3_link).map_err(|e| {
            InstallError::IoError(format!(
                "Failed to symlink {:?} -> {:?}: {}",
                py3_link, primary_target, e
            ))
        })?;

        if !py3_link.is_file() {
            let _ = fs::remove_file(&py3_link);
            return Err(InstallError::IoError(format!(
                "Created Python symlink at {:?} does not resolve to an existing file (target: {:?})",
                py3_link, primary_target
            )));
        }

        let py_link = venv_bin.join("python");
        if let Err(e) = std::os::unix::fs::symlink("python3", &py_link) {
            warn!(path = ?py_link, error = %e, "Failed to symlink python -> python3");
        }

        if py_ver != "python" && py_ver != "python3" {
            let ver_link = venv_bin.join(py_ver);
            if let Err(e) = std::os::unix::fs::symlink("python3", &ver_link) {
                warn!(path = ?ver_link, error = %e, "Failed to create versioned python symlink");
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn materialize_windows_python_binaries(
        venv_scripts: &Path,
        resolved: &ResolvedPython,
    ) -> Result<(), InstallError> {
        let exe_dst = venv_scripts.join("python.exe");
        fs::copy(&resolved.executable, &exe_dst).map_err(|e| {
            InstallError::IoError(format!(
                "Failed to materialize {:?} from {:?}: {}",
                exe_dst, resolved.executable, e
            ))
        })?;
        let py3_dst = venv_scripts.join("python3.exe");
        if let Err(e) = fs::copy(&resolved.executable, &py3_dst) {
            warn!(path = ?py3_dst, error = %e, "Failed to copy python3.exe into .venv/Scripts");
        }

        let mut src_dirs: Vec<&Path> = vec![resolved.home_dir.as_path()];
        if let Some(parent) = resolved.executable.parent() {
            if parent != resolved.home_dir.as_path() {
                src_dirs.push(parent);
            }
        }
        Self::copy_windows_companion_dlls(&src_dirs, venv_scripts);
        Ok(())
    }

    // Side-by-side DLL copy matching Bazel `rules_python` so Scripts\python.exe finds python3*.dll.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn copy_windows_companion_dlls(src_dirs: &[&Path], venv_scripts: &Path) {
        let mut copied: std::collections::HashSet<std::ffi::OsString> =
            std::collections::HashSet::new();
        for dir in src_dirs {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                let is_non_symlink_dll = fs::symlink_metadata(&p).is_ok_and(|m| m.is_file())
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"));
                if !is_non_symlink_dll {
                    continue;
                }
                let fname = entry.file_name();
                if copied.insert(fname.clone()) {
                    let dst = venv_scripts.join(&fname);
                    if let Err(e) = fs::copy(&p, &dst) {
                        warn!(src = ?p, dst = ?dst, error = %e, "Failed to copy Python companion DLL");
                    }
                }
            }
        }
    }

    fn resolve_base_python_with_path(
        workspace_path: &Path,
        root_venv: &Path,
        path_env: Option<&OsStr>,
    ) -> Option<ResolvedPython> {
        let mut roots_to_scan = vec![workspace_path];
        if let Some(venv_parent) = root_venv.parent() {
            if venv_parent != workspace_path {
                roots_to_scan.push(venv_parent);
            }
        }

        let bundled_candidates: &[&str] = if cfg!(windows) {
            &["python.exe", "Scripts/python.exe"]
        } else {
            &["bin/python3", "bin/python"]
        };

        for origin in roots_to_scan {
            let mut cursor = Some(origin);
            for _depth in 0..=3 {
                let Some(dir) = cursor else {
                    break;
                };
                let toolchain_py = dir.join(".dmz").join("toolchains").join("python");
                for rel in bundled_candidates {
                    let cand = toolchain_py.join(rel);
                    if cand.is_file() {
                        let canon_cand = fs::canonicalize(&cand).unwrap_or_else(|_| cand.clone());
                        let home_dir = canon_cand
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or(toolchain_py.clone());
                        return Some(ResolvedPython {
                            executable: cand,
                            home_dir,
                            is_bundled: true,
                        });
                    }
                }
                cursor = dir.parent();
            }
        }

        let path_str = path_env?;
        let venv_canon = fs::canonicalize(root_venv).unwrap_or_else(|_| root_venv.to_path_buf());
        let host_names: &[&str] = if cfg!(windows) {
            &["python.exe", "python3.exe"]
        } else {
            &["python3", "python"]
        };

        for dir in std::env::split_paths(path_str) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let dir_canon = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if dir.starts_with(root_venv) || dir_canon.starts_with(&venv_canon) {
                continue;
            }
            for name in host_names {
                let cand = dir.join(name);
                if cand.is_file()
                    && !crate::toolchain::preflight::is_uninstalled_macos_clt_shim(
                        &cand,
                        workspace_path,
                    )
                {
                    let home_dir = cand
                        .parent()
                        .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
                        .unwrap_or(dir_canon.clone());
                    return Some(ResolvedPython {
                        executable: cand,
                        home_dir,
                        is_bundled: false,
                    });
                }
            }
        }

        None
    }

    /// Resolves a verified non-stub host or bundled Python interpreter for `workspace_path`,
    /// never returning a macOS `/usr/bin/python*` Apple CLT popup shim when CLT is absent.
    #[must_use]
    pub fn resolve_non_stub_host_python(workspace_path: &Path) -> Option<PathBuf> {
        Self::resolve_base_python_with_path(
            workspace_path,
            &workspace_path.join(".venv"),
            std::env::var_os("PATH").as_deref(),
        )
        .map(|resolved| resolved.executable)
    }

    /// Returns `true` when either a bundled Python toolchain or a host Python interpreter
    /// outside any workspace `.venv` is available to hydrate a virtual environment.
    #[must_use]
    pub fn has_available_base_python(workspace_path: &Path) -> bool {
        Self::resolve_base_python_with_path(
            workspace_path,
            &workspace_path.join(".venv"),
            std::env::var_os("PATH").as_deref(),
        )
        .is_some()
    }

    fn normalize_lexical(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for comp in path.components() {
            match comp {
                Component::CurDir => {}
                Component::ParentDir => {
                    if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                        out.pop();
                    } else if !out.has_root() {
                        out.push("..");
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    fn relative_path_to(from_dir: &Path, target: &Path) -> PathBuf {
        let from_norm = Self::normalize_lexical(from_dir);
        let to_norm = Self::normalize_lexical(target);
        let from_comps: Vec<Component<'_>> = from_norm.components().collect();
        let to_comps: Vec<Component<'_>> = to_norm.components().collect();

        let common = from_comps
            .iter()
            .zip(to_comps.iter())
            .take_while(|(a, b)| a == b)
            .count();
        if common == 0 && (from_norm.is_absolute() || to_norm.is_absolute()) {
            return target.to_path_buf();
        }

        let mut rel = PathBuf::new();
        for _ in from_comps.iter().skip(common) {
            rel.push("..");
        }
        for comp in to_comps.iter().skip(common) {
            rel.push(comp.as_os_str());
        }
        rel
    }
}

struct ResolvedPython {
    executable: PathBuf,
    home_dir: PathBuf,
    is_bundled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_copy_windows_companion_dlls_copies_only_regular_dlls() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().join("home");
        let bin = tmp.path().join("bin");
        let scripts = tmp.path().join("Scripts");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&scripts).unwrap();

        fs::write(home.join("python312.dll"), b"dll-home").unwrap();
        fs::write(bin.join("vcruntime140.dll"), b"dll-bin").unwrap();
        fs::write(home.join("README.txt"), b"ignore").unwrap();

        PyPiMapper::copy_windows_companion_dlls(&[&home, &bin], &scripts);
        assert_eq!(
            fs::read(scripts.join("python312.dll")).unwrap(),
            b"dll-home"
        );
        assert_eq!(
            fs::read(scripts.join("vcruntime140.dll")).unwrap(),
            b"dll-bin"
        );
        assert!(!scripts.join("README.txt").exists());
    }

    #[test]
    fn test_detect_python_version_prefers_bundled_and_manifest_over_stale_dmz_venv(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let ws = tmp.path();

        // Stale .dmz/venv/lib/python3.11 exists from an earlier bundle
        fs::create_dir_all(ws.join(".dmz/venv/lib/python3.11/site-packages"))?;
        assert_eq!(PyPiMapper::detect_python_version(ws), "python3.11");

        // Workspace .python-version pins 3.12 -> overrides stale .dmz/venv/lib/python3.11
        fs::write(ws.join(".python-version"), "3.12\n")?;
        assert_eq!(PyPiMapper::detect_python_version(ws), "python3.12");

        // Bundled toolchain python3.12 takes highest priority even without .python-version
        fs::remove_file(ws.join(".python-version"))?;
        fs::create_dir_all(ws.join(".dmz/toolchains/python/lib/python3.12/site-packages"))?;
        assert_eq!(PyPiMapper::detect_python_version(ws), "python3.12");
        Ok(())
    }

    #[test]
    fn test_map_package_skips_obsolete_typing_backport_and_cleans_stale_file(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let ws = tmp.path();
        fs::create_dir_all(ws.join(".dmz/toolchains/python/lib/python3.11"))?;
        let sp = ws.join(".dmz/venv/lib/python3.11/site-packages");
        fs::create_dir_all(&sp)?;
        let stale_typing = sp.join("typing.py");
        fs::write(
            &stale_typing,
            b"AnyStr = TypeVar('AnyStr', bytes, unicode)\n",
        )?;

        let archive_path = ws.join("typing-3.7.4.3-py3-none-any.whl");
        fs::write(&archive_path, b"not-a-zip-should-never-extract")?;

        let pkg = LockedPackage {
            name: "typing".to_string(),
            version: "3.7.4.3".to_string(),
            ecosystem: crate::resolver::manifest::Ecosystem::PyPI,
            sha256: "0".repeat(64),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };

        PyPiMapper::map_package(ws, &pkg, &archive_path)?;
        assert!(
            !stale_typing.exists(),
            "stale site-packages/typing.py must be removed for Python >= 3.5"
        );
        Ok(())
    }

    fn write_mock_wheel_with_entry(
        path: &Path,
        entry_name: &str,
        bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write as _;
        let file = fs::File::create(path)?;
        let mut zw = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zw.start_file(entry_name, options)?;
        zw.write_all(bytes)?;
        zw.finish()?;
        Ok(())
    }

    #[test]
    fn test_map_package_rejects_mismatched_cpython_abi_and_accepts_matching_or_abi3(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let ws = tmp.path();
        fs::create_dir_all(ws.join(".dmz/toolchains/python/lib/python3.12"))?;

        let wheel_311 = ws.join("pydantic_core-2.27.1-cp311-cp311-macosx_11_0_arm64.archive");
        write_mock_wheel_with_entry(
            &wheel_311,
            "pydantic_core/_pydantic_core.cpython-311-darwin.so",
            b"fake-macho",
        )?;

        let pkg = LockedPackage {
            name: "pydantic_core".to_string(),
            version: "2.27.1".to_string(),
            ecosystem: crate::resolver::manifest::Ecosystem::PyPI,
            sha256: "0".repeat(64),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        };

        let err = PyPiMapper::map_package(ws, &pkg, &wheel_311)
            .expect_err("cp311 native extension must be rejected for python3.12 runtime");
        assert!(err.to_string().contains("cp311"));
        assert!(err.to_string().contains("python3.12"));

        let abi3_entry = if cfg!(windows) {
            "cryptography/hazmat/bindings/_rust.pyd"
        } else {
            "cryptography/hazmat/bindings/_rust.abi3.so"
        };
        let wheel_abi3 = ws.join("cryptography-43.0.0-cp39-abi3-macosx_10_9_universal2.archive");
        write_mock_wheel_with_entry(&wheel_abi3, abi3_entry, b"fake-abi3")?;
        let abi3_pkg = LockedPackage {
            name: "cryptography".to_string(),
            version: "43.0.0".to_string(),
            ..pkg.clone()
        };
        PyPiMapper::map_package(ws, &abi3_pkg, &wheel_abi3)?;

        let host_ext_entry = if cfg!(target_os = "macos") {
            "pydantic_core/_pydantic_core.cpython-312-darwin.so"
        } else if cfg!(target_os = "linux") {
            "pydantic_core/_pydantic_core.cpython-312-x86_64-linux-gnu.so"
        } else {
            "pydantic_core/_pydantic_core.cp312-win_amd64.pyd"
        };
        let wheel_312 = ws.join("pydantic_core-2.27.1-cp312-cp312-macosx_11_0_arm64.archive");
        write_mock_wheel_with_entry(&wheel_312, host_ext_entry, b"fake-native-312")?;
        PyPiMapper::map_package(ws, &pkg, &wheel_312)?;

        // Foreign-OS compiled extension with matching minor must also fail fast.
        let wheel_linux_312 =
            ws.join("pydantic_core-2.27.1-cp312-cp312-manylinux_2_17_x86_64.archive");
        write_mock_wheel_with_entry(
            &wheel_linux_312,
            "pydantic_core/_pydantic_core.cpython-312-x86_64-linux-gnu.so",
            b"fake-elf-312",
        )?;
        let os_err = PyPiMapper::verify_wheel_extension_abi_for_os(
            "pydantic_core",
            &wheel_linux_312,
            Some((3, 12)),
            "macos",
        )
        .expect_err("linux-gnu.so extension must be rejected for macos runtime");
        assert!(os_err.to_string().contains("foreign-os"));
        Ok(())
    }
}
