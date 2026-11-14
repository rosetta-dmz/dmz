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
    let probe = Command::new(candidate)
        .args([
            "-c",
            "import sys; print(getattr(sys, '_base_executable', sys.executable))",
        ])
        .stdin(Stdio::null())
        .output();

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

    fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf())
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

    // Fallback to all system PATH entries and standard host bin directories
    for bin_name in [
        "python3",
        "python3.13",
        "python3.12",
        "python3.11",
        "python3.10",
    ] {
        push_system_bin_candidates(&mut candidates, bin_name);
    }
    candidates
}

/// Checks whether a Python binary links dynamically to non-standalone package-manager paths
/// (e.g., Homebrew OpenSSL linked into a `pyenv` build).
fn has_non_standalone_dylib_deps(bin_path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(output) = Command::new("otool")
            .arg("-L")
            .arg(bin_path)
            .stdin(Stdio::null())
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let dep_path = line
                .trim()
                .split(" (compatibility version")
                .next()
                .unwrap_or("")
                .trim();
            if dep_path.starts_with('/') && is_non_standalone_path(Path::new(dep_path)) {
                return true;
            }
        }
        false
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
            if p.is_file() && s.starts_with("python") && s.ends_with(".dll") {
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
                let target_prefix = if cfg!(windows) || folder_name.eq_ignore_ascii_case("Lib") {
                    "Lib".to_string()
                } else {
                    format!("lib/{}", folder_name)
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

    let win_lib = py_root.join("Lib");
    if win_lib.is_dir() {
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
        let py_candidates = collect_python_candidates();
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

/// Collects companion shared libraries under `<py_root>/lib/` referenced via Mach-O `otool -L`
/// by `py_bin` (or already-discovered `lib/libpython*` dylibs) so non-monolithic standalone
/// runtimes (e.g., `pyenv`, `miniforge`) remain self-contained in the bundle.
#[cfg(target_os = "macos")]
fn collect_python_companion_shared_libs(
    py_bin: &Path,
    lib_dir: &Path,
    lib_files: &mut Vec<(PathBuf, String)>,
    seen_lib: &mut HashSet<String>,
) {
    let mut inspect_targets = vec![py_bin.to_path_buf()];
    for (src, rel) in lib_files.iter() {
        if rel.starts_with("lib/") && rel.ends_with(".dylib") {
            inspect_targets.push(src.clone());
        }
    }

    let mut queue = inspect_targets;
    let mut inspected: HashSet<PathBuf> = HashSet::new();
    let canonical_lib_dir = fs::canonicalize(lib_dir).unwrap_or_else(|_| lib_dir.to_path_buf());

    while let Some(target) = queue.pop() {
        if !inspected.insert(target.clone()) {
            continue;
        }
        let Ok(out) = Command::new("otool")
            .arg("-L")
            .arg(&target)
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines().skip(1) {
            let trimmed = line.trim();
            let dep = trimmed
                .split(" (compatibility version")
                .next()
                .unwrap_or("")
                .trim();
            if dep.is_empty() {
                continue;
            }
            let dep_name = dep
                .strip_prefix("@rpath/")
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
                        .strip_prefix(&canonical_lib_dir)
                        .ok()
                        .and_then(|p| p.to_str())
                });

            let Some(rel_name) = dep_name else {
                continue;
            };
            if rel_name.contains("..") || rel_name.is_empty() {
                continue;
            }
            let candidate = lib_dir.join(rel_name);
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

#[cfg(not(target_os = "macos"))]
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
            } else if p.is_file() {
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
}
