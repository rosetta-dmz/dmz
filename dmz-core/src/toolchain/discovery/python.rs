use super::super::python_abi::PythonTargetAbi;
use super::super::types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainProfile,
};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path,
    push_deduped_file, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    select_strict_standalone_candidate, ToolchainDiscoverer,
};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

const MAX_STDLIB_WALK_DEPTH: usize = 32;

/// Returns `true` when `path` points to a macOS system `xcode-select` stub or an Xcode /
/// CommandLineTools Python installation that cannot be relocated into an air-gap bundle.
#[must_use]
pub fn is_unrelocatable_macos_python(path: &Path) -> bool {
    if cfg!(target_os = "macos") {
        is_unrelocatable_macos_python_path(path)
    } else {
        false
    }
}

fn is_unrelocatable_macos_python_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("/Applications/Xcode")
        || path_str.starts_with("/Library/Developer/CommandLineTools")
    {
        return true;
    }

    if path.parent() == Some(Path::new("/usr/bin")) {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            return name == "python" || name == "python3" || name.starts_with("python3.");
        }
    }

    false
}

/// Resolves symlink/venv/shim Python paths to the underlying base interpreter executable.
fn resolve_python_executable(candidate: &Path) -> PathBuf {
    let canon = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    if is_unrelocatable_macos_python(candidate)
        || is_unrelocatable_macos_python(&canon)
        || crate::toolchain::preflight::is_macho_xcselect_stub(&canon)
        || candidate
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/shims/")
    {
        return canon;
    }

    let mut cmd = Command::new(candidate);
    cmd.args([
        "-c",
        "import sys; print(getattr(sys, '_base_executable', sys.executable))",
    ])
    .stdin(Stdio::null());
    if let Some(safe_path) =
        crate::toolchain::preflight::sanitized_probe_path_without_macos_clt_stubs()
    {
        cmd.env("PATH", safe_path);
    }
    let probe = cmd.output();

    if let Ok(output) = probe {
        if output.status.success() {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                let resolved_str = stdout.trim();
                if !resolved_str.is_empty() {
                    let resolved_path = PathBuf::from(resolved_str);
                    if resolved_path.is_file() {
                        return resolved_path;
                    }
                }
            }
        }
    }

    canon
}

/// Extracts numeric runs from a version directory name (e.g. `cpython-3.12.7` -> `[3, 12, 7]`).
fn extract_version_tuple(path: &Path) -> Vec<u64> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    let mut current: Option<u64> = None;

    for ch in name.chars() {
        if let Some(digit) = ch.to_digit(10) {
            let next = current
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(u64::from(digit));
            current = Some(next);
        } else if let Some(val) = current.take() {
            parts.push(val);
        }
    }
    if let Some(val) = current {
        parts.push(val);
    }
    parts
}

/// Finds the newest versioned `python3.<minor>` executable inside `bin_dir` if `python3` is absent.
fn find_versioned_python_in_bin(bin_dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(bin_dir).ok()?;
    let mut matches: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                    name.starts_with("python3.")
                        && !name.ends_with("-config")
                        && !name.ends_with(".py")
                        && !name.ends_with(".pdb")
                })
        })
        .collect();

    matches.sort_by(|a, b| {
        let va = extract_version_tuple(a);
        let vb = extract_version_tuple(b);
        match vb.cmp(&va) {
            Ordering::Equal => b.cmp(a),
            other => other,
        }
    });
    matches.into_iter().next()
}

/// Scans immediate version subdirectories under `parent_dir` (e.g., `uv`, Framework `Versions`)
/// and records `bin/python3` (or versioned `bin/python3.*` / `python.exe`) candidates, newest versions first.
fn collect_subdir_python_bins(parent_dir: &Path, candidates: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(parent_dir) else {
        return;
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();

    dirs.sort_by(|a, b| {
        let va = extract_version_tuple(a);
        let vb = extract_version_tuple(b);
        match vb.cmp(&va) {
            Ordering::Equal => b.cmp(a),
            other => other,
        }
    });

    for dir in dirs {
        let bin_dir = dir.join("bin");
        let unix_bin = bin_dir.join("python3");
        if unix_bin.is_file() {
            push_unique_path(candidates, unix_bin);
        } else if let Some(versioned) = find_versioned_python_in_bin(&bin_dir) {
            push_unique_path(candidates, versioned);
        }
        if cfg!(windows) {
            let win_bin = dir.join("python.exe");
            if win_bin.is_file() {
                push_unique_path(candidates, win_bin);
            }
        }
    }
}

/// Appends candidate `bin/python3` / `python.exe` paths from explicit environment variables.
fn collect_env_python_candidates(candidates: &mut Vec<PathBuf>) {
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        if !venv.is_empty() {
            let venv_path = PathBuf::from(&venv);
            let unix_bin = venv_path.join("bin/python3");
            if unix_bin.is_file() {
                push_unique_path(candidates, unix_bin);
            }
            if cfg!(windows) {
                let win_bin = venv_path.join("Scripts/python.exe");
                if win_bin.is_file() {
                    push_unique_path(candidates, win_bin);
                }
            }
        }
    }

    for var in ["PYTHON_HOME", "PYTHON3"] {
        if let Some(val) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            let p = PathBuf::from(val);
            if p.is_file() {
                push_unique_path(candidates, p);
            } else if p.is_dir() {
                for rel in ["bin/python3", "python3", "python.exe", "Scripts/python.exe"] {
                    let candidate = p.join(rel);
                    if candidate.is_file() {
                        push_unique_path(candidates, candidate);
                    }
                }
            }
        }
    }
}

/// Appends standalone Python installs under `$HOME` (`uv`, `python.org`, `rye`, `mise`, `asdf`, `pyenv`, conda).
fn collect_home_python_candidates(home: &Path, candidates: &mut Vec<PathBuf>) {
    // Prefer `uv` (`python-build-standalone`) ahead of third-party version managers
    collect_subdir_python_bins(&home.join(".local/share/uv/python"), candidates);
    if cfg!(target_os = "macos") {
        collect_subdir_python_bins(
            &home.join("Library/Application Support/uv/python"),
            candidates,
        );
        collect_subdir_python_bins(
            &home.join("Library/Frameworks/Python.framework/Versions"),
            candidates,
        );
    }

    let user_bin_py = home.join(".local/bin/python3");
    if user_bin_py.is_file() {
        push_unique_path(candidates, user_bin_py);
    }

    // Standalone version managers & relocatable user environments (filtered for Homebrew/Nix linkage)
    for subdir in [
        ".rye/py",
        ".local/share/mise/installs/python",
        ".local/share/rtx/installs/python",
        ".asdf/installs/python",
        ".pyenv/versions",
    ] {
        collect_subdir_python_bins(&home.join(subdir), candidates);
    }

    for conda_dir in ["miniforge3", "mambaforge", "miniconda3", "anaconda3"] {
        let bin = home.join(conda_dir).join("bin/python3");
        if bin.is_file() {
            push_unique_path(candidates, bin);
        }
    }
}

/// Collects candidate Python executables prioritizing active venv, `uv`, official `python.org`,
/// and standalone version managers (`rye`, `mise`, `asdf`, `pyenv`, `miniforge`/`miniconda`).
fn collect_python_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    collect_env_python_candidates(&mut candidates);

    // Relocatable `uv` (`python-build-standalone`) managed installations
    if let Ok(uv_dir) = std::env::var("UV_PYTHON_INSTALL_DIR") {
        if !uv_dir.is_empty() {
            collect_subdir_python_bins(Path::new(&uv_dir), &mut candidates);
        }
    }
    if let Some(xdg_data) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        collect_subdir_python_bins(&PathBuf::from(xdg_data).join("uv/python"), &mut candidates);
    }

    let home_dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    if let Some(ref home) = home_dir {
        collect_subdir_python_bins(&home.join(".local/share/uv/python"), &mut candidates);
    }

    // First-party official `python.org` installations
    if cfg!(target_os = "macos") {
        let current_fw =
            PathBuf::from("/Library/Frameworks/Python.framework/Versions/Current/bin/python3");
        if current_fw.is_file() {
            push_unique_path(&mut candidates, current_fw);
        }
        collect_subdir_python_bins(
            Path::new("/Library/Frameworks/Python.framework/Versions"),
            &mut candidates,
        );
    }

    if cfg!(windows) {
        for var in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(root) = std::env::var_os(var) {
                if !root.is_empty() {
                    let root_path = PathBuf::from(root);
                    collect_subdir_python_bins(&root_path.join("Programs/Python"), &mut candidates);
                    collect_subdir_python_bins(&root_path, &mut candidates);
                }
            }
        }
    }

    if let Some(ref home) = home_dir {
        collect_home_python_candidates(home, &mut candidates);
    }
    collect_subdir_python_bins(Path::new("/opt/python"), &mut candidates);

    let mut standard_paths = vec![PathBuf::from("/usr/local/bin/python3")];
    if !cfg!(target_os = "macos") {
        standard_paths.push(PathBuf::from("/usr/bin/python3"));
    }
    for p in standard_paths {
        if p.is_file() {
            push_unique_path(&mut candidates, p);
        }
    }

    // Dynamically scan PATH and standard bin directories for any `python3` or `python<major>.<minor>[t]`
    // binary rather than maintaining a hardcoded list of minor versions.
    push_system_bin_candidates(&mut candidates, "python3");
    let mut search_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for fallback_dir in ["/usr/local/bin", "/usr/bin", "/bin"] {
        let p = PathBuf::from(fallback_dir);
        if !search_dirs.contains(&p) {
            search_dirs.push(p);
        }
    }
    for bin_path in collect_versioned_python_bins_in_dirs(&search_dirs) {
        push_unique_path(&mut candidates, bin_path);
    }
    candidates
}

fn collect_versioned_python_bins_in_dirs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut discovered: Vec<(PythonTargetAbi, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(abi) = PythonTargetAbi::parse_versioned_name(fname) {
                discovered.push((abi, path));
            }
        }
    }
    discovered.sort_by(|(abi_a, path_a), (abi_b, path_b)| {
        let key_a = (abi_a.major, abi_a.minor, !abi_a.free_threaded);
        let key_b = (abi_b.major, abi_b.minor, !abi_b.free_threaded);
        key_b.cmp(&key_a).then_with(|| path_a.cmp(path_b))
    });
    discovered.into_iter().map(|(_, p)| p).collect()
}

/// Checks whether a Python binary links dynamically to non-standalone package-manager paths
/// (e.g., Homebrew OpenSSL linked into a `pyenv` build) via pure-Rust Mach-O inspection.
fn has_non_standalone_dylib_deps(bin_path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(info) = crate::toolchain::setup::macho::inspect_macho_load_info(bin_path) else {
            return false;
        };
        info.dylib_deps.iter().any(|dep| {
            let trimmed = dep.trim();
            trimmed.starts_with('/') && is_non_standalone_path(Path::new(trimmed))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bin_path;
        false
    }
}

/// Unwraps shims/symlinks/venvs and filters out unrelocatable macOS stubs before strict selection.
fn filter_and_select_python(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    let mut filtered: Vec<PathBuf> = Vec::new();
    let mut skipped_macos_unrelocatable: Option<PathBuf> = None;

    for candidate in candidates {
        if is_unrelocatable_macos_python(&candidate) {
            if skipped_macos_unrelocatable.is_none() {
                skipped_macos_unrelocatable = Some(candidate);
            }
            continue;
        }

        let resolved = resolve_python_executable(&candidate);
        if is_unrelocatable_macos_python(&resolved) {
            if skipped_macos_unrelocatable.is_none() {
                skipped_macos_unrelocatable = Some(resolved);
            }
            continue;
        }

        if has_non_standalone_dylib_deps(&resolved) {
            emit_non_relocatable_notice(&resolved);
            continue;
        }

        push_unique_path(&mut filtered, resolved);
    }

    let selected = select_strict_standalone_candidate(filtered);
    if let Some(skipped) = skipped_macos_unrelocatable {
        if selected.is_some() {
            debug!(
                path = %redact_home_prefix(&skipped),
                "Ignoring macOS system/Xcode Python stub because a standalone Python installation was selected"
            );
        } else {
            warn!(
                path = %redact_home_prefix(&skipped),
                "Skipped unrelocatable macOS system/Xcode Python stub; install standalone Python via `uv python install` (python-build-standalone) or python.org"
            );
        }
    }

    selected
}

/// Resolves the installation prefix root for either a `bin/python*` binary or a macOS
/// Framework `Resources/Python.app/Contents/MacOS/Python` binary, refusing `"/"`.
fn resolve_python_root(canonical_bin: &Path) -> Option<PathBuf> {
    let prefix = if canonical_bin.ends_with("Resources/Python.app/Contents/MacOS/Python") {
        canonical_bin
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::parent)
    } else {
        canonical_bin.parent().and_then(Path::parent)
    }?;

    if prefix.as_os_str().is_empty() || prefix == Path::new("/") {
        return None;
    }

    Some(prefix.to_path_buf())
}

fn read_python_version(py_bin: &Path) -> String {
    if let Ok(output) = Command::new(py_bin)
        .args([
            "-c",
            "import sys; v=sys.version_info; t='t' if (hasattr(sys,'_is_gil_enabled') and not sys._is_gil_enabled()) else ''; print(f'Python {v[0]}.{v[1]}.{v[2]}{t}')",
        ])
        .stdin(Stdio::null())
        .output()
    {
        if output.status.success() {
            if let Ok(ver) = String::from_utf8(output.stdout) {
                let trimmed = ver.trim();
                if trimmed.starts_with("Python ") {
                    return trimmed.to_string();
                }
            }
        }
    }

    Command::new(py_bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "python3".to_string())
}

fn collect_companion_pip(
    py_bin: &Path,
    canonical_bin: &Path,
    bin_files: &mut Vec<(PathBuf, String)>,
    seen_bin: &mut HashSet<String>,
) {
    let mut search_dirs: Vec<&Path> = Vec::new();
    if let Some(parent) = py_bin.parent() {
        search_dirs.push(parent);
    }
    if let Some(parent) = canonical_bin.parent() {
        if !search_dirs.contains(&parent) {
            search_dirs.push(parent);
        }
    }

    for dir in search_dirs {
        for name in ["pip3", "pip"] {
            let candidate = dir.join(name);
            if candidate.is_file()
                && !is_unrelocatable_macos_python(&candidate)
                && !is_non_standalone_path(&candidate)
            {
                push_deduped_file(bin_files, seen_bin, candidate.clone(), "bin/pip3".into());
                push_deduped_file(bin_files, seen_bin, candidate, "bin/pip".into());
                return;
            }
        }
    }
}

struct PythonRuntimeFiles {
    bin_files: Vec<(PathBuf, String)>,
    lib_files: Vec<(PathBuf, String)>,
}

fn should_include_python_headers(ctx: &ToolchainDiscoveryContext) -> bool {
    ctx.profile == ToolchainProfile::Full
        || ctx.ecosystems.contains(&ToolchainEcosystem::Cc)
        || ctx.ecosystems.contains(&ToolchainEcosystem::CMake)
}

fn collect_python_runtime_files(
    py_bin: &Path,
    canonical_bin: &Path,
    py_root: &Path,
    profile: ToolchainProfile,
    include_headers: bool,
) -> PythonRuntimeFiles {
    let bin_files = collect_python_binaries(py_bin, canonical_bin, py_root, profile);
    let lib_files = collect_python_libraries(py_bin, py_root, profile, include_headers);
    PythonRuntimeFiles {
        bin_files,
        lib_files,
    }
}

fn collect_python_binaries(
    py_bin: &Path,
    canonical_bin: &Path,
    py_root: &Path,
    profile: ToolchainProfile,
) -> Vec<(PathBuf, String)> {
    let mut bin_files = Vec::new();
    let mut seen_bin = HashSet::new();

    // On macOS Framework builds (`python.org`), `bin/python3.X` is `pythonw` which uses `dladdr` +
    // `posix_spawn` to launch `Resources/Python.app/Contents/MacOS/Python`. Bundling that inner
    // binary directly avoids framework path assumptions while linking `lib/Python`.
    let macos_app_py = py_root.join("Resources/Python.app/Contents/MacOS/Python");
    let bundled_py_bin = if macos_app_py.is_file() {
        macos_app_py
    } else {
        py_bin.to_path_buf()
    };

    push_deduped_file(
        &mut bin_files,
        &mut seen_bin,
        bundled_py_bin.clone(),
        "bin/python3".into(),
    );
    push_deduped_file(
        &mut bin_files,
        &mut seen_bin,
        bundled_py_bin,
        "bin/python".into(),
    );

    if cfg!(windows) {
        for rel in ["python.exe", "bin/python.exe", "bin/python3.exe"] {
            push_deduped_file(
                &mut bin_files,
                &mut seen_bin,
                py_bin.to_path_buf(),
                rel.into(),
            );
        }
        if let Some(parent) = py_bin.parent() {
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file()
                        && !p.is_symlink()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
                    {
                        let fname = entry.file_name().to_string_lossy().to_string();
                        push_deduped_file(&mut bin_files, &mut seen_bin, p.clone(), fname.clone());
                        push_deduped_file(
                            &mut bin_files,
                            &mut seen_bin,
                            p,
                            format!("bin/{}", fname),
                        );
                    }
                }
            }
        }
    }

    if profile == ToolchainProfile::Full {
        collect_companion_pip(py_bin, canonical_bin, &mut bin_files, &mut seen_bin);
    }

    if let Ok(entries) = fs::read_dir(py_root) {
        for entry in entries.flatten() {
            let p = entry.path();
            let s = entry.file_name().to_string_lossy().to_string();
            if p.is_file() && !p.is_symlink() && s.starts_with("python") && s.ends_with(".dll") {
                push_deduped_file(&mut bin_files, &mut seen_bin, p, s);
            }
        }
    }

    bin_files
}

fn collect_python_libraries(
    py_bin: &Path,
    py_root: &Path,
    profile: ToolchainProfile,
    include_headers: bool,
) -> Vec<(PathBuf, String)> {
    collect_python_libraries_for_os(py_bin, py_root, profile, include_headers, cfg!(windows))
}

fn collect_python_libraries_for_os(
    py_bin: &Path,
    py_root: &Path,
    profile: ToolchainProfile,
    include_headers: bool,
    is_windows: bool,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();
    let mut seen_lib = HashSet::new();

    if let Ok(output) = Command::new(py_bin)
        .args([
            "-c",
            "import sysconfig; print(sysconfig.get_path('stdlib'))",
        ])
        .stdin(Stdio::null())
        .output()
    {
        if output.status.success() {
            let stdlib_path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stdlib_dir = PathBuf::from(&stdlib_path_str);
            if stdlib_dir.is_dir() {
                let folder_name = stdlib_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("python3");
                let target_prefix = if is_windows {
                    "Lib".to_string()
                } else {
                    format!("lib/{folder_name}")
                };
                collect_python_files_recursive_deduped(
                    &stdlib_dir,
                    &stdlib_dir,
                    &mut lib_files,
                    &mut seen_lib,
                    &target_prefix,
                    0,
                    profile,
                );
            }
        }
    }

    let lib_dir = py_root.join("lib");
    if lib_dir.is_dir() {
        collect_python_lib_dir(&lib_dir, py_root, &mut lib_files, &mut seen_lib, profile);
        collect_python_companion_shared_libs(py_bin, &lib_dir, &mut lib_files, &mut seen_lib);
    }

    let framework_py = py_root.join("Python");
    if framework_py.is_file() {
        push_deduped_file(
            &mut lib_files,
            &mut seen_lib,
            framework_py.clone(),
            "lib/Python".to_string(),
        );
        push_deduped_file(
            &mut lib_files,
            &mut seen_lib,
            framework_py,
            "lib/libpython3.dylib".to_string(),
        );
    }

    if is_windows {
        let win_lib = py_root.join("Lib");
        if win_lib.is_dir() && !win_lib.is_symlink() {
            collect_python_files_recursive_deduped(
                &win_lib,
                py_root,
                &mut lib_files,
                &mut seen_lib,
                "",
                0,
                profile,
            );
        }

        let win_dlls = py_root.join("DLLs");
        if win_dlls.is_dir() && !win_dlls.is_symlink() {
            collect_python_files_recursive_deduped(
                &win_dlls,
                py_root,
                &mut lib_files,
                &mut seen_lib,
                "",
                0,
                profile,
            );
        }
    }

    if include_headers {
        let inc_dir = py_root.join("include");
        if inc_dir.is_dir() {
            let mut raw_inc = Vec::new();
            collect_files_recursive(&inc_dir, py_root, &mut raw_inc, "");
            for (src, rel) in raw_inc {
                push_deduped_file(&mut lib_files, &mut seen_lib, src, rel);
            }
        }
    }

    lib_files
}

/// Parses a Python version constraint or pin (e.g. `"3.12"`, `"Python 3.12.7"`, `">=3.12"`, `"==3.11.*"`)
/// into a `(major, minor)` pair when an explicit major.minor token is present.
pub(crate) fn parse_python_minor_spec(raw: &str) -> Option<(u8, u8)> {
    PythonTargetAbi::parse_runtime_spec(raw).map(|abi| (abi.major, abi.minor))
}

/// Scans `lib_dir` for versioned `python3.<minor>[t]` directories and returns `(minor, dir_name)`
/// for the highest numeric minor version found.
#[must_use]
pub(crate) fn find_highest_python_minor_dir(lib_dir: &Path) -> Option<(u8, String)> {
    PythonTargetAbi::find_highest_lib_dir(lib_dir).map(|(abi, name)| (abi.minor, name))
}

fn detect_bundled_python_abi(workspace_root: &Path) -> Option<PythonTargetAbi> {
    let lib_dir = workspace_root
        .join(".dmz")
        .join("toolchains")
        .join("python")
        .join("lib");
    PythonTargetAbi::find_highest_lib_dir(&lib_dir).map(|(abi, _)| abi)
}

/// Resolves the concrete [`PythonTargetAbi`] (including free-threaded `'t'` status) for
/// dependency fetching, wheel ABI selection, and PEP 508 markers.
pub(crate) fn resolve_effective_python_abi(
    workspace_root: Option<&Path>,
    member_roots: &[PathBuf],
    target_platform: Option<&str>,
) -> Option<PythonTargetAbi> {
    if let Some(ws) = workspace_root {
        if let Some(bundled) = detect_bundled_python_abi(ws) {
            return Some(bundled);
        }
    }
    let manifest_minor = resolve_workspace_python_minor(workspace_root, member_roots);
    if is_target_matching_host(target_platform) {
        let candidates =
            prioritize_candidates_by_minor(collect_python_candidates(), manifest_minor);
        if let Some(py_bin) = filter_and_select_python(candidates) {
            let ver = read_python_version(&py_bin);
            if let Some(parsed) = PythonTargetAbi::parse_runtime_spec(&ver) {
                return Some(parsed);
            }
        }
    }
    manifest_minor.map(|(major, minor)| PythonTargetAbi::new(major, minor, false))
}

/// Resolves the concrete `(major, minor)` Python ABI for dependency fetching and PEP 508 markers,
/// aligning with the already-bundled runtime or the standalone host Python selected by `PythonDiscoverer`.
pub(crate) fn resolve_effective_python_minor(
    workspace_root: Option<&Path>,
    member_roots: &[PathBuf],
    target_platform: Option<&str>,
) -> Option<(u8, u8)> {
    resolve_effective_python_abi(workspace_root, member_roots, target_platform)
        .map(|abi| (abi.major, abi.minor))
}

/// Resolves the desired `(major, minor)` Python version declared by `.python-version`,
/// `pyproject.toml`, or `uv.lock` across the workspace and its members.
pub(crate) fn resolve_workspace_python_minor(
    workspace_root: Option<&Path>,
    member_roots: &[PathBuf],
) -> Option<(u8, u8)> {
    let mut roots: Vec<&Path> = Vec::new();
    if let Some(ws) = workspace_root {
        roots.push(ws);
    }
    for member in member_roots {
        let m_path = member.as_path();
        if !roots.contains(&m_path) {
            roots.push(m_path);
        }
    }

    for dir in &roots {
        let py_ver_file = dir.join(".python-version");
        if let Ok(content) = fs::read_to_string(&py_ver_file) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some(ver) = parse_python_minor_spec(trimmed) {
                    return Some(ver);
                }
            }
        }
    }

    for dir in &roots {
        let pyproject = dir.join("pyproject.toml");
        if let Ok(content) = fs::read_to_string(&pyproject) {
            if let Ok(toml_val) = toml::from_str::<toml::Value>(&content) {
                if let Some(req) = toml_val
                    .get("project")
                    .and_then(|p| p.get("requires-python"))
                    .and_then(|v| v.as_str())
                {
                    if let Some(ver) = parse_python_minor_spec(req) {
                        return Some(ver);
                    }
                }
                if let Some(req) = toml_val
                    .get("tool")
                    .and_then(|t| t.get("poetry"))
                    .and_then(|p| p.get("dependencies"))
                    .and_then(|d| d.get("python"))
                    .and_then(|v| v.as_str())
                {
                    if let Some(ver) = parse_python_minor_spec(req) {
                        return Some(ver);
                    }
                }
            }
        }
    }

    for dir in &roots {
        let uv_lock = dir.join("uv.lock");
        if let Ok(content) = fs::read_to_string(&uv_lock) {
            if let Ok(toml_val) = toml::from_str::<toml::Value>(&content) {
                if let Some(req) = toml_val.get("requires-python").and_then(|v| v.as_str()) {
                    if let Some(ver) = parse_python_minor_spec(req) {
                        return Some(ver);
                    }
                }
            }
        }
    }

    None
}

/// Returns `true` when `target_platform` is unset or matches the current host `(os, arch)`.
pub(crate) fn is_target_matching_host(target_platform: Option<&str>) -> bool {
    let (target_os, target_arch) = crate::workspace::resolve_target_os_arch(target_platform);
    target_os == std::env::consts::OS && target_arch == std::env::consts::ARCH
}

fn candidate_matches_minor(candidate: &Path, desired: (u8, u8)) -> bool {
    let needle_dot = format!("{}.{}", desired.0, desired.1);
    let needle_bin = format!("python{}.{}", desired.0, desired.1);
    candidate.components().any(|comp| {
        let s = comp.as_os_str().to_string_lossy();
        s == needle_dot
            || s.starts_with(&format!("{}.", needle_dot))
            || s.contains(&format!("-{}", needle_dot))
            || s.contains(&format!("cpython-{}", needle_dot))
            || s == needle_bin
    })
}

fn prioritize_candidates_by_minor(
    candidates: Vec<PathBuf>,
    desired_minor: Option<(u8, u8)>,
) -> Vec<PathBuf> {
    let Some(desired) = desired_minor else {
        return candidates;
    };
    let mut matching = Vec::new();
    let mut rest = Vec::new();
    for cand in candidates {
        if candidate_matches_minor(&cand, desired) {
            matching.push(cand);
        } else {
            rest.push(cand);
        }
    }
    matching.extend(rest);
    matching
}

/// Discovers host Python runtime (`python3`, `pip`), strictly preferring standalone official
/// `python.org` and `uv` (`python-build-standalone`) builds and rejecting Homebrew/Xcode stubs.
#[derive(Debug, Default, Clone)]
pub struct PythonDiscoverer;

impl ToolchainDiscoverer for PythonDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Python
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        self.discover_with_context(&ToolchainDiscoveryContext::default())
    }

    fn discover_with_context(
        &self,
        ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        if !is_target_matching_host(ctx.target_platform.as_deref()) {
            let (target_os, target_arch) =
                crate::workspace::resolve_target_os_arch(ctx.target_platform.as_deref());
            warn!(
                host_os = std::env::consts::OS,
                host_arch = std::env::consts::ARCH,
                target_os,
                target_arch,
                "Target platform differs from host OS/architecture; refusing to bundle incompatible host Python binaries"
            );
            return Ok(None);
        }

        let desired_minor =
            resolve_workspace_python_minor(ctx.workspace_root.as_deref(), &ctx.member_roots);
        let py_candidates =
            prioritize_candidates_by_minor(collect_python_candidates(), desired_minor);
        if py_candidates.is_empty() {
            return Ok(None);
        }

        let Some(py_bin) = filter_and_select_python(py_candidates) else {
            return Ok(None);
        };

        let canonical_bin = fs::canonicalize(&py_bin).unwrap_or_else(|_| py_bin.clone());
        let Some(py_root) = resolve_python_root(&canonical_bin) else {
            return Ok(None);
        };

        let version = read_python_version(&py_bin);
        info!(
            py_root = %redact_home_prefix(&py_root),
            version = %version,
            desired_minor = ?desired_minor,
            profile = %ctx.profile,
            "Discovered host Python toolchain"
        );

        let include_headers = should_include_python_headers(ctx);
        let runtime = collect_python_runtime_files(
            &py_bin,
            &canonical_bin,
            &py_root,
            ctx.profile,
            include_headers,
        );

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Python,
            version,
            root_dir: py_root,
            bin_files: runtime.bin_files,
            lib_files: runtime.lib_files,
        }))
    }
}

/// Collects `lib/python3.X` standard library directories and `lib/libpython*` shared libraries.
fn collect_python_lib_dir(
    lib_dir: &Path,
    py_root: &Path,
    lib_files: &mut Vec<(PathBuf, String)>,
    seen_lib: &mut HashSet<String>,
    profile: ToolchainProfile,
) {
    if let Ok(entries) = fs::read_dir(lib_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let fname = entry.file_name();
            let s = fname.to_string_lossy();
            if p.is_dir() && !p.is_symlink() && s.starts_with("python3.") {
                collect_python_files_recursive_deduped(
                    &p, py_root, lib_files, seen_lib, "", 0, profile,
                );
            } else if p.is_file()
                && (!p.is_symlink()
                    || (fs::canonicalize(&p).ok())
                        .zip(fs::canonicalize(lib_dir).ok())
                        .is_some_and(|(target, root)| target.starts_with(root)))
                && ((s.starts_with("libpython")
                    && (s.ends_with(".dylib")
                        || s.contains(".so")
                        || s.ends_with(".a")
                        || s.ends_with(".dll")))
                    || s == "Python")
            {
                push_deduped_file(lib_files, seen_lib, p, format!("lib/{}", s));
            }
        }
    }
}

/// Collects companion shared libraries under `<py_root>/lib/` referenced by `py_bin` or `libpython*`
/// using pure-Rust Mach-O (macOS) or ELF `DT_NEEDED` (Linux) inspection without external tools.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn collect_python_companion_shared_libs(
    py_bin: &Path,
    lib_dir: &Path,
    lib_files: &mut Vec<(PathBuf, String)>,
    seen_lib: &mut HashSet<String>,
) {
    let mut queue = vec![py_bin.to_path_buf()];
    for (src, rel) in lib_files.iter() {
        let is_shared = if cfg!(target_os = "macos") {
            rel.ends_with(".dylib")
        } else {
            rel.contains(".so")
        };
        if rel.starts_with("lib/") && is_shared {
            queue.push(src.clone());
        }
    }

    let mut inspected: HashSet<PathBuf> = HashSet::new();
    let canonical_lib_dir = fs::canonicalize(lib_dir).unwrap_or_else(|_| lib_dir.to_path_buf());

    while let Some(target) = queue.pop() {
        if !inspected.insert(target.clone()) {
            continue;
        }

        let dep_names = extract_companion_lib_rel_names(&target, lib_dir, &canonical_lib_dir);
        for rel_name in dep_names {
            if rel_name.contains("..") || rel_name.is_empty() {
                continue;
            }
            let candidate = lib_dir.join(&rel_name);
            if candidate.is_file() {
                let target_rel = format!("lib/{}", rel_name.replace('\\', "/"));
                let was_new = !seen_lib.contains(&target_rel);
                push_deduped_file(lib_files, seen_lib, candidate.clone(), target_rel);
                if was_new {
                    queue.push(candidate);
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn extract_companion_lib_rel_names(
    target: &Path,
    lib_dir: &Path,
    canonical_lib_dir: &Path,
) -> Vec<String> {
    let Some(info) = crate::toolchain::setup::macho::inspect_macho_load_info(target) else {
        return Vec::new();
    };
    info.dylib_deps
        .iter()
        .filter_map(|raw_dep| {
            let dep = raw_dep.trim();
            if dep.is_empty() {
                return None;
            }
            dep.strip_prefix("@rpath/")
                .or_else(|| dep.strip_prefix("@loader_path/"))
                .or_else(|| dep.strip_prefix("@executable_path/../lib/"))
                .or_else(|| {
                    Path::new(dep)
                        .strip_prefix(lib_dir)
                        .ok()
                        .and_then(|p| p.to_str())
                })
                .or_else(|| {
                    Path::new(dep)
                        .strip_prefix(canonical_lib_dir)
                        .ok()
                        .and_then(|p| p.to_str())
                })
                .map(str::to_string)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn extract_companion_lib_rel_names(
    target: &Path,
    _lib_dir: &Path,
    _canonical_lib_dir: &Path,
) -> Vec<String> {
    const SYSTEM_ELF_PREFIXES: &[&str] = &[
        "libc.so",
        "libm.so",
        "libpthread.so",
        "libdl.so",
        "librt.so",
        "libutil.so",
        "ld-linux",
    ];
    let Some(info) = crate::toolchain::setup::elf::inspect_elf_load_info(target) else {
        return Vec::new();
    };
    info.needed
        .into_iter()
        .filter(|name| {
            !name.is_empty()
                && !name.contains('/')
                && !SYSTEM_ELF_PREFIXES.iter().any(|p| name.starts_with(p))
        })
        .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn collect_python_companion_shared_libs(
    _py_bin: &Path,
    _lib_dir: &Path,
    _lib_files: &mut Vec<(PathBuf, String)>,
    _seen_lib: &mut HashSet<String>,
) {
}

/// Recursively collects Python stdlib files up to a bounded depth while skipping symlinks and caches.
fn collect_python_files_recursive_deduped(
    dir: &Path,
    base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    seen_rel: &mut HashSet<String>,
    target_prefix: &str,
    depth: usize,
    profile: ToolchainProfile,
) {
    if depth > MAX_STDLIB_WALK_DEPTH {
        return;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && !p.is_symlink() {
                let name = entry.file_name();
                // Always exclude caches, stdlib tests/IDE, and ambient host package directories
                // (`site-packages`/`dist-packages`) regardless of profile; project dependencies
                // must flow exclusively through `dmz.lock`.
                if name == "__pycache__"
                    || name == "idlelib"
                    || name == "test"
                    || name == "tests"
                    || name == "site-packages"
                    || name == "dist-packages"
                {
                    continue;
                }
                if profile == ToolchainProfile::Minimal
                    && (name == "tkinter"
                        || name == "turtledemo"
                        || name == "ensurepip"
                        || name == "pydoc_data")
                {
                    continue;
                }
                collect_python_files_recursive_deduped(
                    &p,
                    base,
                    acc,
                    seen_rel,
                    target_prefix,
                    depth + 1,
                    profile,
                );
            } else if p.is_file() && !p.is_symlink() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    let target_path = if target_prefix.is_empty() {
                        rel_str
                    } else if target_prefix.ends_with('/') {
                        format!("{}{}", target_prefix, rel_str)
                    } else {
                        format!("{}/{}", target_prefix, rel_str)
                    };
                    push_deduped_file(acc, seen_rel, p, target_path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_python_collects_stdlib_skips_pycache_and_deduplicates() {
        let temp = tempdir().unwrap();
        let py_root = temp.path().join("fake_py");
        let stdlib = py_root.join("lib/python3.11");
        let pycache = stdlib.join("__pycache__");
        let inc = py_root.join("include/python3.11");

        fs::create_dir_all(&stdlib).unwrap();
        fs::create_dir_all(&pycache).unwrap();
        fs::create_dir_all(&inc).unwrap();

        fs::write(stdlib.join("os.py"), "# os").unwrap();
        fs::write(stdlib.join("sys.py"), "# sys").unwrap();
        fs::write(pycache.join("os.cpython-311.pyc"), "bytecode").unwrap();
        fs::write(inc.join("Python.h"), "#define PYTHON_H").unwrap();

        let mut lib_files = Vec::new();
        let mut seen_lib = HashSet::new();
        collect_python_files_recursive_deduped(
            &stdlib,
            &py_root,
            &mut lib_files,
            &mut seen_lib,
            "",
            0,
            ToolchainProfile::Minimal,
        );
        // Repeating via target prefix avoids duplicating the same target relative path.
        collect_python_files_recursive_deduped(
            &stdlib,
            &stdlib,
            &mut lib_files,
            &mut seen_lib,
            "lib/python3.11",
            0,
            ToolchainProfile::Minimal,
        );
        collect_files_recursive(&inc, &py_root, &mut lib_files, "");

        let rel_paths: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert_eq!(
            rel_paths
                .iter()
                .filter(|p| *p == "lib/python3.11/os.py")
                .count(),
            1
        );
        assert!(rel_paths.contains(&"lib/python3.11/sys.py".to_string()));
        assert!(!rel_paths.iter().any(|p| p.contains("__pycache__")));
        assert!(rel_paths.contains(&"include/python3.11/Python.h".to_string()));
    }

    #[test]
    fn test_unrelocatable_macos_python_path_flags_system_and_xcode() {
        let unrelocatable = [
            "/usr/bin/python",
            "/usr/bin/python3",
            "/usr/bin/python3.9",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/python3",
            "/Applications/Xcode-beta.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3",
            "/Library/Developer/CommandLineTools/usr/bin/python3",
            "/Library/Developer/CommandLineTools/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3",
        ];
        for path in unrelocatable {
            assert!(
                is_unrelocatable_macos_python_path(Path::new(path)),
                "Expected unrelocatable path to be flagged: {}",
                path
            );
        }

        let standalone = [
            "/Users/dev/.local/share/uv/python/cpython-3.12.7-macos-aarch64-none/bin/python3",
            "/Library/Frameworks/Python.framework/Versions/3.12/bin/python3",
            "/usr/local/bin/python3",
        ];
        for path in standalone {
            assert!(
                !is_unrelocatable_macos_python_path(Path::new(path)),
                "Expected standalone path not to be flagged: {}",
                path
            );
        }

        assert!(is_non_standalone_path(Path::new(
            "/opt/homebrew/opt/python@3.12/bin/python3.12"
        )));
    }

    #[test]
    fn test_collect_subdir_python_bins_orders_versions_numerically_newest_first() {
        let temp = tempdir().unwrap();
        let uv_dir = temp.path().join("uv/python");
        let py39 = uv_dir.join("cpython-3.9.20-macos-aarch64-none/bin");
        let py311 = uv_dir.join("cpython-3.11.9-macos-aarch64-none/bin");
        let py312 = uv_dir.join("cpython-3.12.7-macos-aarch64-none/bin");
        let incomplete = uv_dir.join("cpython-3.13.0-incomplete");

        fs::create_dir_all(&py39).unwrap();
        fs::create_dir_all(&py311).unwrap();
        fs::create_dir_all(&py312).unwrap();
        fs::create_dir_all(&incomplete).unwrap();

        let bin39 = py39.join("python3");
        let bin311 = py311.join("python3");
        let bin312 = py312.join("python3");
        fs::write(&bin39, "").unwrap();
        fs::write(&bin311, "").unwrap();
        fs::write(&bin312, "").unwrap();

        let mut candidates = Vec::new();
        collect_subdir_python_bins(&uv_dir, &mut candidates);

        assert_eq!(candidates, vec![bin312, bin311, bin39]);
    }

    #[test]
    fn test_resolve_python_root_handles_standard_framework_and_rejects_root() {
        assert_eq!(
            resolve_python_root(Path::new(
                "/home/user/.local/share/uv/python/cpython-3.12.7/bin/python3"
            )),
            Some(PathBuf::from(
                "/home/user/.local/share/uv/python/cpython-3.12.7"
            ))
        );
        assert_eq!(
            resolve_python_root(Path::new(
                "/Library/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python"
            )),
            Some(PathBuf::from(
                "/Library/Frameworks/Python.framework/Versions/3.12"
            ))
        );
        assert_eq!(resolve_python_root(Path::new("/bin/python3")), None);
        assert_eq!(resolve_python_root(Path::new("/python3")), None);
    }

    #[test]
    fn test_filter_and_select_python_rejects_homebrew_even_without_alternatives() {
        let candidates = vec![
            PathBuf::from("/opt/homebrew/bin/python3"),
            PathBuf::from("/usr/local/Cellar/python@3.12/3.12.7/bin/python3"),
        ];
        assert_eq!(filter_and_select_python(candidates), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_discoverer_never_bundles_xcode_homebrew_or_usr_bin_shim() {
        assert!(is_unrelocatable_macos_python(Path::new("/usr/bin/python3")));
        if let Ok(Some(tc)) = PythonDiscoverer.discover() {
            for (src, _) in &tc.bin_files {
                assert!(
                    !is_unrelocatable_macos_python(src),
                    "PythonDiscoverer returned unrelocatable macOS binary: {:?}",
                    src
                );
                assert!(
                    !is_non_standalone_path(src),
                    "PythonDiscoverer returned non-standalone package-manager binary: {:?}",
                    src
                );
            }
        }
    }

    #[test]
    fn test_python_minimal_profile_excludes_host_site_packages_and_dev_stdlib(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let py_root = temp.path().join("py_root");
        let py_lib = py_root.join("lib/python3.12");
        fs::create_dir_all(py_lib.join("site-packages/leaked_pkg"))?;
        fs::create_dir_all(py_lib.join("dist-packages/leaked_deb"))?;
        fs::create_dir_all(py_lib.join("ensurepip"))?;
        fs::create_dir_all(py_lib.join("tkinter"))?;
        fs::create_dir_all(py_lib.join("json"))?;

        fs::write(py_lib.join("os.py"), "# stdlib")?;
        fs::write(py_lib.join("json/__init__.py"), "# stdlib json")?;
        fs::write(
            py_lib.join("site-packages/leaked_pkg/__init__.py"),
            "# host",
        )?;
        fs::write(
            py_lib.join("dist-packages/leaked_deb/__init__.py"),
            "# host",
        )?;
        fs::write(py_lib.join("ensurepip/__init__.py"), "# ensurepip")?;
        fs::write(py_lib.join("tkinter/__init__.py"), "# tk")?;
        fs::write(py_root.join("lib/libpython3.12.dylib"), "dylib")?;

        let mut min_files = Vec::new();
        let mut min_seen = HashSet::new();
        collect_python_lib_dir(
            &py_root.join("lib"),
            &py_root,
            &mut min_files,
            &mut min_seen,
            ToolchainProfile::Minimal,
        );
        let min_rels: Vec<String> = min_files.into_iter().map(|(_, r)| r).collect();
        assert!(min_rels.contains(&"lib/python3.12/os.py".to_string()));
        assert!(min_rels.contains(&"lib/python3.12/json/__init__.py".to_string()));
        assert!(min_rels.contains(&"lib/libpython3.12.dylib".to_string()));
        assert!(!min_rels.iter().any(|r| r.contains("site-packages")));
        assert!(!min_rels.iter().any(|r| r.contains("dist-packages")));
        assert!(!min_rels.iter().any(|r| r.contains("ensurepip")));
        assert!(!min_rels.iter().any(|r| r.contains("tkinter")));

        let mut full_files = Vec::new();
        let mut full_seen = HashSet::new();
        collect_python_lib_dir(
            &py_root.join("lib"),
            &py_root,
            &mut full_files,
            &mut full_seen,
            ToolchainProfile::Full,
        );
        let full_rels: Vec<String> = full_files.into_iter().map(|(_, r)| r).collect();
        // Even under Full, host `site-packages` and `dist-packages` must stay excluded.
        assert!(!full_rels.iter().any(|r| r.contains("site-packages")));
        assert!(!full_rels.iter().any(|r| r.contains("dist-packages")));
        assert!(full_rels.iter().any(|r| r.contains("ensurepip")));
        assert!(full_rels.iter().any(|r| r.contains("tkinter")));

        Ok(())
    }

    #[test]
    fn test_python_minimal_profile_gates_pip_and_c_headers(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let py_root = temp.path().join("py_root");
        let bin = py_root.join("bin");
        let inc = py_root.join("include/python3.12");
        fs::create_dir_all(&bin)?;
        fs::create_dir_all(&inc)?;

        let py_bin = bin.join("python3");
        fs::write(&py_bin, "python3")?;
        fs::write(bin.join("pip"), "pip")?;
        fs::write(bin.join("pip3"), "pip3")?;
        fs::write(inc.join("Python.h"), "/* header */")?;

        let ctx_plain = ToolchainDiscoveryContext {
            profile: ToolchainProfile::Minimal,
            ..ToolchainDiscoveryContext::default()
        };
        assert!(!should_include_python_headers(&ctx_plain));

        let ctx_cc = ToolchainDiscoveryContext {
            profile: ToolchainProfile::Minimal,
            ecosystems: [ToolchainEcosystem::Cc].into_iter().collect(),
            ..ToolchainDiscoveryContext::default()
        };
        assert!(should_include_python_headers(&ctx_cc));

        let min_rt = collect_python_runtime_files(
            &py_bin,
            &py_bin,
            &py_root,
            ToolchainProfile::Minimal,
            false,
        );
        let min_bins: Vec<String> = min_rt.bin_files.into_iter().map(|(_, r)| r).collect();
        let min_libs: Vec<String> = min_rt.lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(min_bins.contains(&"bin/python3".to_string()));
        assert!(!min_bins.iter().any(|r| r.contains("pip")));
        assert!(!min_libs.iter().any(|r| r.starts_with("include/")));

        let min_native_rt = collect_python_runtime_files(
            &py_bin,
            &py_bin,
            &py_root,
            ToolchainProfile::Minimal,
            true,
        );
        assert!(min_native_rt
            .lib_files
            .iter()
            .any(|(_, r)| r == "include/python3.12/Python.h"));

        let full_rt =
            collect_python_runtime_files(&py_bin, &py_bin, &py_root, ToolchainProfile::Full, true);
        let full_bins: Vec<String> = full_rt.bin_files.into_iter().map(|(_, r)| r).collect();
        let full_libs: Vec<String> = full_rt.lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(full_bins.contains(&"bin/pip".to_string()));
        assert!(full_bins.contains(&"bin/pip3".to_string()));
        assert!(full_libs.contains(&"include/python3.12/Python.h".to_string()));

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_filter_and_select_python_skips_stub_when_standalone_present() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir()?;
        let standalone_dir = temp.path().join("uv/cpython-3.12.8/bin");
        fs::create_dir_all(&standalone_dir)?;
        let standalone_py = standalone_dir.join("python3");
        fs::write(
            &standalone_py,
            "#!/bin/sh\necho \"/tmp/standalone_uv_root\"\n",
        )?;
        fs::set_permissions(&standalone_py, fs::Permissions::from_mode(0o755))?;

        let canonical_standalone = fs::canonicalize(&standalone_py)?;
        let selected =
            filter_and_select_python(vec![PathBuf::from("/usr/bin/python3"), standalone_py]);
        assert_eq!(selected, Some(canonical_standalone));
        Ok(())
    }

    #[test]
    fn test_collect_subdir_python_bins_falls_back_to_versioned_binary() -> std::io::Result<()> {
        let temp = tempdir()?;
        let py_bin_dir = temp.path().join("3.12.7/bin");
        fs::create_dir_all(&py_bin_dir)?;
        let versioned = py_bin_dir.join("python3.12");
        fs::write(&versioned, "")?;
        // Ensure helper tools like python3.12-config are ignored
        fs::write(py_bin_dir.join("python3.12-config"), "")?;

        let mut candidates = Vec::new();
        collect_subdir_python_bins(temp.path(), &mut candidates);
        assert_eq!(candidates, vec![versioned]);
        Ok(())
    }

    #[test]
    fn test_collect_home_python_candidates_prefers_uv_over_pyenv_and_mise() -> std::io::Result<()> {
        let temp = tempdir()?;
        let home = temp.path();
        let uv_py = home.join(".local/share/uv/python/cpython-3.12.8/bin/python3");
        let pyenv_py = home.join(".pyenv/versions/3.12.7/bin/python3");
        let mise_py = home.join(".local/share/mise/installs/python/3.12.6/bin/python3");
        for py in [&uv_py, &pyenv_py, &mise_py] {
            if let Some(p) = py.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(py, "")?;
        }

        let mut candidates = Vec::new();
        collect_home_python_candidates(home, &mut candidates);
        assert_eq!(candidates.first(), Some(&uv_py));
        assert!(candidates.contains(&pyenv_py));
        assert!(candidates.contains(&mise_py));
        Ok(())
    }

    #[test]
    fn test_resolve_workspace_python_minor_from_manifests() -> std::io::Result<()> {
        let temp = tempdir()?;
        let ws = temp.path();

        fs::write(
            ws.join("pyproject.toml"),
            "[project]\nname = \"app\"\nrequires-python = \">=3.12,<3.14\"\n",
        )?;
        assert_eq!(resolve_workspace_python_minor(Some(ws), &[]), Some((3, 12)));

        // .python-version overrides pyproject.toml when present
        fs::write(ws.join(".python-version"), "# pinned\n3.11.9\n")?;
        assert_eq!(resolve_workspace_python_minor(Some(ws), &[]), Some((3, 11)));

        let uv_only = tempdir()?;
        fs::write(
            uv_only.path().join("uv.lock"),
            "version = 1\nrequires-python = \">=3.13\"\n",
        )?;
        assert_eq!(
            resolve_workspace_python_minor(Some(uv_only.path()), &[]),
            Some((3, 13))
        );
        Ok(())
    }

    #[test]
    fn test_cross_target_platform_skips_incompatible_host_python() {
        let cross_target = if cfg!(target_os = "linux") {
            "macos-arm64"
        } else {
            "linux-x86_64"
        };
        assert!(!is_target_matching_host(Some(cross_target)));
        assert!(is_target_matching_host(None));

        let ctx = ToolchainDiscoveryContext {
            target_platform: Some(cross_target.to_string()),
            ..ToolchainDiscoveryContext::default()
        };
        assert!(PythonDiscoverer
            .discover_with_context(&ctx)
            .expect("cross-target discover should not error")
            .is_none());
    }

    #[test]
    fn test_prioritize_candidates_by_minor_reorders_matching_version_first() {
        let py313 = PathBuf::from("/opt/uv/cpython-3.13.2/bin/python3");
        let py311 = PathBuf::from("/opt/uv/cpython-3.11.9/bin/python3");
        let py312 = PathBuf::from("/opt/uv/cpython-3.12.7/bin/python3");

        let ordered = prioritize_candidates_by_minor(
            vec![py313.clone(), py311.clone(), py312.clone()],
            Some((3, 12)),
        );
        assert_eq!(ordered, vec![py312, py313, py311]);
    }

    #[test]
    fn test_python_collects_windows_dlls_dir_and_skips_file_symlinks() -> std::io::Result<()> {
        let temp = tempdir()?;
        let py_root = temp.path().join("py_win");
        let dlls = py_root.join("DLLs");
        let lib_dir = py_root.join("Lib");
        fs::create_dir_all(&dlls)?;
        fs::create_dir_all(&lib_dir)?;

        fs::write(dlls.join("_ssl.pyd"), b"pyd")?;
        fs::write(dlls.join("libssl-3-x64.dll"), b"dll")?;
        fs::write(lib_dir.join("os.py"), b"# os")?;

        #[cfg(unix)]
        {
            let secret = temp.path().join("outside.py");
            fs::write(&secret, b"secret")?;
            std::os::unix::fs::symlink(&secret, lib_dir.join("leaked.py"))?;
        }

        let py_bin = py_root.join("python.exe");
        fs::write(&py_bin, b"exe")?;
        let collected = collect_python_libraries_for_os(
            &py_bin,
            &py_root,
            ToolchainProfile::Minimal,
            false,
            true,
        );
        let rels: Vec<&str> = collected.iter().map(|(_, r)| r.as_str()).collect();
        assert!(rels.contains(&"DLLs/_ssl.pyd"));
        assert!(rels.contains(&"DLLs/libssl-3-x64.dll"));
        assert!(rels.contains(&"Lib/os.py"));
        assert!(!rels.contains(&"Lib/leaked.py"));

        // Non-Windows collection must never emit Windows `Lib/` duplicates even on case-insensitive APFS.
        let non_win = collect_python_libraries_for_os(
            &py_bin,
            &py_root,
            ToolchainProfile::Minimal,
            false,
            false,
        );
        assert!(
            !non_win
                .iter()
                .any(|(_, r)| r.starts_with("Lib/") || r.starts_with("DLLs/")),
            "Unix collection must never emit Windows Lib/ or DLLs/ paths"
        );
        Ok(())
    }

    #[test]
    fn test_resolve_effective_python_minor_aligns_bundled_runtime_over_lower_bound(
    ) -> std::io::Result<()> {
        let temp = tempdir()?;
        let ws = temp.path();
        fs::write(
            ws.join("pyproject.toml"),
            "[project]\nname = \"demo\"\nrequires-python = \">=3.11\"\n",
        )?;
        assert_eq!(parse_python_minor_spec("Python 3.12.7"), Some((3, 12)));

        // Cross-target without host bundling keeps manifest minor
        assert_eq!(
            resolve_effective_python_minor(Some(ws), &[], Some("windows-x86_64")),
            Some((3, 11))
        );

        // Once .dmz/toolchains/python ships 3.14t or 3.12, it overrides `>=3.11` lower-bound heuristics.
        fs::create_dir_all(ws.join(".dmz/toolchains/python/lib/python3.12"))?;
        assert_eq!(
            resolve_effective_python_minor(Some(ws), &[], None),
            Some((3, 12))
        );

        fs::create_dir_all(ws.join(".dmz/toolchains/python/lib/python3.14t"))?;
        assert_eq!(
            resolve_effective_python_abi(Some(ws), &[], None),
            Some(PythonTargetAbi::new(3, 14, true))
        );
        Ok(())
    }

    #[test]
    fn test_collect_versioned_python_bins_in_dirs_discovers_314_314t_and_315_dynamically(
    ) -> std::io::Result<()> {
        let tmp = tempdir()?;
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir)?;
        for name in [
            "python3.12",
            "python3.14",
            "python3.14t",
            "python3.15",
            "python3.14-config",
        ] {
            fs::write(bin_dir.join(name), b"")?;
        }
        let found = collect_versioned_python_bins_in_dirs(std::slice::from_ref(&bin_dir));
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        assert_eq!(
            names,
            vec![
                "python3.15".to_string(),
                "python3.14".to_string(),
                "python3.14t".to_string(),
                "python3.12".to_string(),
            ]
        );
        Ok(())
    }
}
