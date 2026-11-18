//! Canonical Python target ABI representation and PEP 425 / extension verification helpers.

use std::fmt;
use std::fs;
use std::path::Path;

/// Parsed PEP 440 / `.python-version` requirement distinguishing exact pins from lower-bounded ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonVersionConstraint {
    /// Explicit pin (`.python-version`, `==3.12`, `~=3.12`).
    Exact(PythonTargetAbi),
    /// Range specification such as `>=3.12` or `>=3.12,<3.14`.
    Range {
        /// Preferred lower-bound ABI reported as fallback when no interpreter is inspected.
        preferred: PythonTargetAbi,
        /// Inclusive `(major, minor)` minimum (`>=` or `>`).
        min_inclusive: Option<(u8, u8)>,
        /// Exclusive `(major, minor)` ceiling (`<`).
        max_exclusive: Option<(u8, u8)>,
    },
}

impl PythonVersionConstraint {
    /// Returns the representative `(major, minor)` version for fallback formatting.
    #[must_use]
    pub const fn fallback_abi(self) -> PythonTargetAbi {
        match self {
            Self::Exact(abi) | Self::Range { preferred: abi, .. } => abi,
        }
    }

    /// Returns `true` when `candidate` satisfies this version constraint.
    #[must_use]
    pub fn satisfies(self, candidate: PythonTargetAbi) -> bool {
        match self {
            Self::Exact(pin) => {
                (candidate.major, candidate.minor) == (pin.major, pin.minor)
                    && (!pin.free_threaded || candidate.free_threaded)
            }
            Self::Range {
                min_inclusive,
                max_exclusive,
                ..
            } => {
                let mm = (candidate.major, candidate.minor);
                let min_ok = min_inclusive.is_none_or(|m| mm >= m);
                let max_ok = max_exclusive.is_none_or(|m| mm < m);
                min_ok && max_ok
            }
        }
    }

    /// Parses a PEP 440 specifier list or `.python-version` line into an exact pin or range constraint.
    #[must_use]
    pub fn parse(raw: &str, force_exact: bool) -> Option<Self> {
        let mut exact: Option<PythonTargetAbi> = None;
        let mut first_min: Option<PythonTargetAbi> = None;
        let mut min_inclusive: Option<(u8, u8)> = None;
        let mut max_exclusive: Option<(u8, u8)> = None;

        for raw_clause in raw.split(',') {
            let clause = raw_clause.trim();
            if clause.is_empty() {
                continue;
            }
            let Some(abi) = PythonTargetAbi::parse_runtime_spec(clause) else {
                continue;
            };
            if force_exact || clause.starts_with("==") || clause.starts_with("~=") {
                if exact.is_none() {
                    exact = Some(abi);
                }
                continue;
            }
            if clause.starts_with(">=") {
                min_inclusive = Some(min_inclusive.map_or((abi.major, abi.minor), |cur| {
                    cur.max((abi.major, abi.minor))
                }));
                if first_min.is_none() {
                    first_min = Some(abi);
                }
            } else if clause.starts_with('>') {
                let next_mm = (abi.major, abi.minor.saturating_add(1));
                min_inclusive = Some(min_inclusive.map_or(next_mm, |cur| cur.max(next_mm)));
                if first_min.is_none() {
                    first_min = Some(abi);
                }
            } else if clause.starts_with("<=") {
                let ceil_mm = (abi.major, abi.minor.saturating_add(1));
                max_exclusive = Some(max_exclusive.map_or(ceil_mm, |cur| cur.min(ceil_mm)));
                if first_min.is_none() {
                    first_min = Some(abi);
                }
            } else if clause.starts_with('<') {
                max_exclusive = Some(max_exclusive.map_or((abi.major, abi.minor), |cur| {
                    cur.min((abi.major, abi.minor))
                }));
                if first_min.is_none() {
                    first_min = Some(abi);
                }
            } else if !clause.starts_with("!=") && exact.is_none() {
                exact = Some(abi);
            }
        }

        if let Some(pin) = exact {
            return Some(Self::Exact(pin));
        }
        first_min.map(|preferred| Self::Range {
            preferred,
            min_inclusive,
            max_exclusive,
        })
    }

    /// Reconciles multiple Python version constraints across workspace members into a single
    /// constraint satisfying every member.
    ///
    /// # Errors
    /// Returns a diagnostic describing incompatible pins or disjoint bounds across members.
    pub fn reconcile_all<I>(constraints: I) -> Result<Option<Self>, String>
    where
        I: IntoIterator<Item = Self>,
    {
        let mut combined: Option<Self> = None;
        for next in constraints {
            combined = Some(match combined {
                None => next,
                Some(cur) => cur.reconcile_with(next)?,
            });
        }
        Ok(combined)
    }

    /// Intersects two Python version constraints, failing on mutually incompatible pins/bounds.
    fn reconcile_with(self, other: Self) -> Result<Self, String> {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => {
                if (a.major, a.minor) != (b.major, b.minor) || a.free_threaded != b.free_threaded {
                    return Err(format!(
                        "Conflicting exact Python pins across workspace members: {} vs {}",
                        a, b
                    ));
                }
                Ok(Self::Exact(a))
            }
            (Self::Exact(pin), r @ Self::Range { .. })
            | (r @ Self::Range { .. }, Self::Exact(pin)) => {
                if !r.satisfies(pin) {
                    return Err(format!(
                        "Conflicting workspace Python constraints: pin {} does not satisfy member constraint (preferred {})",
                        pin,
                        r.fallback_abi()
                    ));
                }
                Ok(Self::Exact(pin))
            }
            (
                Self::Range {
                    preferred: pa,
                    min_inclusive: min_a,
                    max_exclusive: max_a,
                },
                Self::Range {
                    preferred: pb,
                    min_inclusive: min_b,
                    max_exclusive: max_b,
                },
            ) => {
                let min_inclusive = match (min_a, min_b) {
                    (Some(x), Some(y)) => Some(x.max(y)),
                    (a, b) => a.or(b),
                };
                let max_exclusive = match (max_a, max_b) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    (a, b) => a.or(b),
                };
                if let (Some(lo), Some(hi)) = (min_inclusive, max_exclusive) {
                    if lo >= hi {
                        return Err(format!(
                            "Conflicting workspace Python ranges: lower bound {}.{} >= exclusive upper bound {}.{}",
                            lo.0, lo.1, hi.0, hi.1
                        ));
                    }
                }
                let mut preferred = if (pb.major, pb.minor) > (pa.major, pa.minor) {
                    pb
                } else {
                    pa
                };
                if let Some(hi) = max_exclusive {
                    if (preferred.major, preferred.minor) >= hi {
                        if let Some(lo) = min_inclusive {
                            preferred = PythonTargetAbi::new(lo.0, lo.1, false);
                        }
                    }
                }
                if let Some(lo) = min_inclusive {
                    if (preferred.major, preferred.minor) < lo {
                        preferred = PythonTargetAbi::new(lo.0, lo.1, preferred.free_threaded);
                    }
                }
                Ok(Self::Range {
                    preferred,
                    min_inclusive,
                    max_exclusive,
                })
            }
        }
    }
}

/// Canonical Python runtime ABI tag (e.g., `python3.14` / `cp314` or free-threaded `python3.14t` / `cp314t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PythonTargetAbi {
    /// CPython major version (e.g., `3`).
    pub major: u8,
    /// CPython minor version (e.g., `14`).
    pub minor: u8,
    /// Whether the runtime uses free-threaded (no-GIL) CPython ABI (`t` suffix, e.g. `cp314t`).
    pub free_threaded: bool,
}

impl fmt::Display for PythonTargetAbi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "python{}.{}{}",
            self.major,
            self.minor,
            if self.free_threaded { "t" } else { "" }
        )
    }
}

impl PythonTargetAbi {
    /// Constructs a new `PythonTargetAbi`.
    #[must_use]
    pub const fn new(major: u8, minor: u8, free_threaded: bool) -> Self {
        Self {
            major,
            minor,
            free_threaded,
        }
    }

    /// Formats the PEP 425 CPython ABI tag (e.g. `"cp314"` or `"cp314t"`).
    #[must_use]
    pub fn cp_tag(self) -> String {
        format!(
            "cp{}{}{}",
            self.major,
            self.minor,
            if self.free_threaded { "t" } else { "" }
        )
    }

    /// Formats the standard library directory name (e.g. `"python3.14"` or `"python3.14t"`).
    #[must_use]
    pub fn lib_dir_name(self) -> String {
        self.to_string()
    }

    /// Parses a PEP 425 CPython tag such as `"cp314"`, `"cp314t"`, or `"cp310"`.
    #[must_use]
    pub fn parse_pep425_tag(tag: &str) -> Option<Self> {
        let lower = tag.trim().to_ascii_lowercase();
        let rest = lower.strip_prefix("cp")?;
        let (ver_digits, free_threaded) = match rest.strip_suffix('t') {
            Some(r) => (r, true),
            None => (rest, false),
        };
        let mut chars = ver_digits.chars();
        let major_ch = chars.next()?;
        if !major_ch.is_ascii_digit() {
            return None;
        }
        let major = major_ch.to_digit(10).and_then(|d| u8::try_from(d).ok())?;
        if major < 3 {
            return None;
        }
        let minor_str: String = chars.collect();
        if minor_str.is_empty() || !minor_str.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let minor = minor_str.parse::<u8>().ok()?;
        Some(Self {
            major,
            minor,
            free_threaded,
        })
    }

    /// Parses a versioned Python directory or executable basename (e.g., `"python3.14"`,
    /// `"python3.14t"`, `"python3.14.exe"`), ignoring helper scripts like `python3.14-config`.
    #[must_use]
    pub fn parse_versioned_name(name: &str) -> Option<Self> {
        let trimmed = name.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.ends_with("-config")
            || lower.ends_with(".py")
            || lower.ends_with(".pdb")
            || lower.contains("-config")
        {
            return None;
        }
        let no_exe = trimmed
            .strip_suffix(".exe")
            .or_else(|| trimmed.strip_suffix(".EXE"))
            .unwrap_or(trimmed);
        let rest = no_exe
            .strip_prefix("python")
            .or_else(|| no_exe.strip_prefix("Python"))?;
        let (maj_str, min_part) = rest.split_once('.')?;
        let major = maj_str.parse::<u8>().ok()?;
        if major < 3 {
            return None;
        }
        let (min_digits, free_threaded) = match min_part.strip_suffix('t') {
            Some(d) => (d, true),
            None => (min_part, false),
        };
        if min_digits.is_empty() || !min_digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let minor = min_digits.parse::<u8>().ok()?;
        Some(Self {
            major,
            minor,
            free_threaded,
        })
    }

    /// Parses a PEP 405 `pyvenv.cfg` file, extracting `version_info` / `version` (or falling back
    /// to versioned `home` directory path components).
    #[must_use]
    pub fn parse_pyvenv_cfg(contents: &str) -> Option<Self> {
        let mut version_raw: Option<&str> = None;
        let mut version_info_raw: Option<&str> = None;
        let mut home_raw: Option<&str> = None;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, val)) = trimmed.split_once('=') else {
                continue;
            };
            let key_clean = key.trim();
            let val_clean = val.trim();
            if val_clean.is_empty() {
                continue;
            }
            if key_clean.eq_ignore_ascii_case("version_info") {
                version_info_raw = Some(val_clean);
            } else if key_clean.eq_ignore_ascii_case("version") {
                version_raw = Some(val_clean);
            } else if key_clean.eq_ignore_ascii_case("home") {
                home_raw = Some(val_clean);
            }
        }

        if let Some(abi) = version_info_raw.and_then(Self::parse_runtime_spec) {
            return Some(abi);
        }
        if let Some(abi) = version_raw.and_then(Self::parse_runtime_spec) {
            return Some(abi);
        }
        if let Some(home) = home_raw {
            for comp in Path::new(home).components().rev() {
                let s = comp.as_os_str().to_string_lossy();
                if let Some(abi) = Self::parse_versioned_name(&s) {
                    return Some(abi);
                }
                if let Some(idx) = s.find("3.") {
                    if let Some(slice) = s.get(idx..) {
                        if let Some(abi) = Self::parse_runtime_spec(slice) {
                            return Some(abi);
                        }
                    }
                }
            }
        }
        None
    }

    /// Parses a Python runtime version string, pin, or constraint clause
    /// (e.g. `"3.14"`, `"3.14t"`, `"Python 3.14.0"`, `"Python 3.14.0t"`, `">=3.12,<3.14"`).
    #[must_use]
    pub fn parse_runtime_spec(raw: &str) -> Option<Self> {
        let raw_lower = raw.to_ascii_lowercase();
        let free_threading_note = raw_lower.contains("free-threading");

        if let Some(cp) = Self::parse_pep425_tag(raw) {
            return Some(Self {
                free_threaded: cp.free_threaded || free_threading_note,
                ..cp
            });
        }

        for clause in raw.split(',') {
            let trimmed = clause
                .trim()
                .trim_start_matches(['>', '<', '=', '~', '!', '^'])
                .trim();
            let cleaned = trimmed
                .strip_prefix("Python ")
                .or_else(|| trimmed.strip_prefix("python "))
                .or_else(|| trimmed.strip_prefix("python"))
                .unwrap_or(trimmed)
                .trim();

            let mut parts = cleaned.split('.');
            let Some(major_str) = parts.next().map(str::trim) else {
                continue;
            };
            let Some(minor_str) = parts.next().map(str::trim) else {
                continue;
            };
            let Ok(major) = major_str.parse::<u8>() else {
                continue;
            };
            if major < 3 {
                continue;
            }
            let minor_digits: String = minor_str
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if minor_digits.is_empty() {
                continue;
            }
            let Ok(minor) = minor_digits.parse::<u8>() else {
                continue;
            };

            let minor_has_t = minor_str
                .strip_prefix(&minor_digits)
                .is_some_and(|rest| rest.starts_with('t'));
            let token_end = cleaned.split_whitespace().next().unwrap_or(cleaned);
            let clause_has_t = minor_has_t
                || (token_end.ends_with('t')
                    && token_end
                        .strip_suffix('t')
                        .is_some_and(|p| p.ends_with(|c: char| c.is_ascii_digit())));

            return Some(Self {
                major,
                minor,
                free_threaded: clause_has_t || free_threading_note,
            });
        }
        None
    }

    /// Scans `lib_dir` for `python3.<minor>[t]` directories and returns `(abi, dir_name)`
    /// for the newest ABI directory found (preferring higher minor, then non-`t` over `t` on tie).
    #[must_use]
    pub fn find_highest_lib_dir(lib_dir: &Path) -> Option<(Self, String)> {
        let entries = fs::read_dir(lib_dir).ok()?;
        let mut best: Option<(Self, String)> = None;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            let Some(abi) = Self::parse_versioned_name(&s) else {
                continue;
            };
            if !entry.path().is_dir() {
                continue;
            }
            let replace = match &best {
                None => true,
                Some((b, _)) => {
                    (abi.major, abi.minor, !abi.free_threaded)
                        > (b.major, b.minor, !b.free_threaded)
                }
            };
            if replace {
                best = Some((abi, s.to_string()));
            }
        }
        best
    }

    /// Parses compiled extension tags inside wheel archives (`.cpython-314-darwin.so`,
    /// `.cpython-314t-darwin.so`, `.cp314-win_amd64.pyd`).
    #[must_use]
    pub fn parse_extension_abi_tag(entry_name: &str) -> Option<Self> {
        let lower = entry_name.to_ascii_lowercase();
        if !(lower.ends_with(".so") || lower.ends_with(".pyd"))
            || lower.contains(".abi3.")
            || lower.contains(".abi3t.")
        {
            return None;
        }
        for marker in [".cpython-", ".cp"] {
            if let Some((_, rest)) = lower.split_once(marker) {
                let mut chars = rest.chars().peekable();
                let major_ch = chars.next()?;
                if !major_ch.is_ascii_digit() {
                    continue;
                }
                let major = major_ch.to_digit(10).and_then(|d| u8::try_from(d).ok())?;
                if major < 3 {
                    continue;
                }
                let mut minor_digits = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() {
                        minor_digits.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if minor_digits.is_empty() {
                    continue;
                }
                let minor = minor_digits.parse::<u8>().ok()?;
                let free_threaded = chars.peek() == Some(&'t');
                return Some(Self {
                    major,
                    minor,
                    free_threaded,
                });
            }
        }
        None
    }

    /// Returns `true` when an extension filename inside a wheel targets a foreign OS.
    #[must_use]
    pub fn is_foreign_os_extension(entry_lower: &str, target_os: &str) -> bool {
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

    /// Verifies that compiled C/Rust extensions inside a `.whl` / `.archive` match `target` ABI and `target_os`.
    ///
    /// # Errors
    /// Returns a human-readable diagnostic when incompatible extensions are found without a matching entry.
    pub fn verify_wheel_extension_abi(
        pkg_name: &str,
        archive_path: &Path,
        target: Option<Self>,
        target_os: &str,
    ) -> Result<(), String> {
        let Some(target_abi) = target else {
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
            if lower.contains(".abi3t.") {
                if target_abi.free_threaded {
                    has_compatible_ext = true;
                } else {
                    let tag = "abi3t".to_string();
                    if !mismatched.contains(&tag) {
                        mismatched.push(tag);
                    }
                }
                continue;
            }
            if lower.contains(".abi3.") {
                if !target_abi.free_threaded {
                    has_compatible_ext = true;
                } else {
                    let tag = "abi3".to_string();
                    if !mismatched.contains(&tag) {
                        mismatched.push(tag);
                    }
                }
                continue;
            }
            if let Some(ext_abi) = Self::parse_extension_abi_tag(&name) {
                if ext_abi == target_abi {
                    has_compatible_ext = true;
                } else {
                    let tag = ext_abi.cp_tag();
                    if !mismatched.contains(&tag) {
                        mismatched.push(tag);
                    }
                }
            }
        }

        if !mismatched.is_empty() && !has_compatible_ext {
            return Err(format!(
                "Python ABI/OS mismatch for package '{}': wheel contains incompatible compiled extension(s) [{}], but target runtime is {} on {}. Re-run `dmz bundle --refresh-lock` to fetch matching wheels.",
                pkg_name,
                mismatched.join(", "),
                target_abi,
                target_os
            ));
        }

        Ok(())
    }

    /// Validates a slice of `(package_name, archive_path)` PyPI bundle artifacts against a resolved
    /// target Python ABI and OS before sealing an air-gap archive.
    ///
    /// # Errors
    /// Returns the first incompatible wheel diagnostic encountered.
    pub fn verify_bundle_python_closure<'a>(
        pypi_artifacts: impl IntoIterator<Item = (&'a str, &'a Path)>,
        target: Option<Self>,
        target_os: &str,
    ) -> Result<(), String> {
        let Some(target_abi) = target else {
            return Ok(());
        };
        for (pkg_name, archive_path) in pypi_artifacts {
            Self::verify_wheel_extension_abi(pkg_name, archive_path, Some(target_abi), target_os)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_parse_runtime_and_pep425_tags_for_314_and_freethreaded() {
        assert_eq!(
            PythonTargetAbi::parse_runtime_spec("Python 3.14.2"),
            Some(PythonTargetAbi::new(3, 14, false))
        );
        assert_eq!(
            PythonTargetAbi::parse_runtime_spec("Python 3.14.0t"),
            Some(PythonTargetAbi::new(3, 14, true))
        );
        assert_eq!(
            PythonTargetAbi::parse_runtime_spec("3.14t"),
            Some(PythonTargetAbi::new(3, 14, true))
        );
        assert_eq!(
            PythonTargetAbi::parse_runtime_spec(">=3.12,<3.15"),
            Some(PythonTargetAbi::new(3, 12, false))
        );

        assert_eq!(
            PythonTargetAbi::parse_pep425_tag("cp314"),
            Some(PythonTargetAbi::new(3, 14, false))
        );
        assert_eq!(
            PythonTargetAbi::parse_pep425_tag("cp314t"),
            Some(PythonTargetAbi::new(3, 14, true))
        );
        assert_eq!(
            PythonTargetAbi::new(3, 14, true).cp_tag(),
            "cp314t".to_string()
        );
        assert_eq!(
            PythonTargetAbi::new(3, 14, true).lib_dir_name(),
            "python3.14t".to_string()
        );

        let range =
            PythonVersionConstraint::parse(">=3.12", false).expect("range >=3.12 should parse");
        assert!(matches!(range, PythonVersionConstraint::Range { .. }));
        assert!(range.satisfies(PythonTargetAbi::new(3, 14, false)));
        assert!(range.satisfies(PythonTargetAbi::new(3, 12, false)));
        assert!(!range.satisfies(PythonTargetAbi::new(3, 11, false)));

        let bounded = PythonVersionConstraint::parse(">=3.12,<3.14", false)
            .expect("bounded range should parse");
        assert!(bounded.satisfies(PythonTargetAbi::new(3, 13, false)));
        assert!(!bounded.satisfies(PythonTargetAbi::new(3, 14, false)));

        let exact = PythonVersionConstraint::parse("3.12", true).expect("exact pin should parse");
        assert!(matches!(exact, PythonVersionConstraint::Exact(_)));
        assert!(exact.satisfies(PythonTargetAbi::new(3, 12, false)));
        assert!(!exact.satisfies(PythonTargetAbi::new(3, 14, false)));
    }

    #[test]
    fn test_parse_pyvenv_cfg_and_reconcile_constraints() {
        let cfg = "home = /opt/homebrew/opt/python@3.14/Frameworks/Python.framework/Versions/3.14/bin\nversion_info = 3.14.2.final.0\n";
        assert_eq!(
            PythonTargetAbi::parse_pyvenv_cfg(cfg),
            Some(PythonTargetAbi::new(3, 14, false))
        );

        let home_only = "home = /opt/uv/cpython-3.13.2-macos-aarch64-none/bin\n";
        assert_eq!(
            PythonTargetAbi::parse_pyvenv_cfg(home_only),
            Some(PythonTargetAbi::new(3, 13, false))
        );

        let r_311 = PythonVersionConstraint::parse(">=3.11", false).expect(">=3.11 parses");
        let r_312 = PythonVersionConstraint::parse(">=3.12", false).expect(">=3.12 parses");
        let p_314 = PythonVersionConstraint::parse("3.14", true).expect("3.14 pin parses");
        let p_312 = PythonVersionConstraint::parse("3.12", true).expect("3.12 pin parses");
        let lt_314 = PythonVersionConstraint::parse(">=3.11,<3.14", false).expect("<3.14 parses");

        assert_eq!(
            PythonVersionConstraint::reconcile_all([r_311, p_314]),
            Ok(Some(p_314))
        );
        assert_eq!(
            PythonVersionConstraint::reconcile_all([r_311, r_312])
                .expect("ranges should intersect")
                .map(PythonVersionConstraint::fallback_abi),
            Some(PythonTargetAbi::new(3, 12, false))
        );
        assert!(PythonVersionConstraint::reconcile_all([p_312, p_314]).is_err());
        assert!(PythonVersionConstraint::reconcile_all([lt_314, p_314]).is_err());
    }

    #[test]
    fn test_parse_versioned_name_ignores_helpers() {
        assert_eq!(
            PythonTargetAbi::parse_versioned_name("python3.14"),
            Some(PythonTargetAbi::new(3, 14, false))
        );
        assert_eq!(
            PythonTargetAbi::parse_versioned_name("python3.14t"),
            Some(PythonTargetAbi::new(3, 14, true))
        );
        assert_eq!(
            PythonTargetAbi::parse_versioned_name("python3.16"),
            Some(PythonTargetAbi::new(3, 16, false))
        );
        assert_eq!(
            PythonTargetAbi::parse_versioned_name("python3.14-config"),
            None
        );
        assert_eq!(PythonTargetAbi::parse_versioned_name("python3"), None);
    }

    #[test]
    fn test_extension_abi_verification_for_314_and_314t() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempdir()?;
        let write_whl = |name: &str, entry: &str| -> Result<std::path::PathBuf, std::io::Error> {
            let path = tmp.path().join(name);
            let f = fs::File::create(&path)?;
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file(entry, opts)?;
            zw.write_all(b"ext")?;
            zw.finish()?;
            Ok(path)
        };

        let ext_314 = write_whl("pkg-314.archive", "yaml/_yaml.cpython-314-darwin.so")?;
        let ext_314t = write_whl("pkg-314t.archive", "yaml/_yaml.cpython-314t-darwin.so")?;
        let ext_abi3 = write_whl("pkg-abi3.archive", "cryptography/_rust.abi3.so")?;

        // Standard 3.14 accepts cp314 & abi3, rejects cp314t
        PythonTargetAbi::verify_wheel_extension_abi(
            "PyYAML",
            &ext_314,
            Some(PythonTargetAbi::new(3, 14, false)),
            "macos",
        )?;
        PythonTargetAbi::verify_wheel_extension_abi(
            "cryptography",
            &ext_abi3,
            Some(PythonTargetAbi::new(3, 14, false)),
            "macos",
        )?;
        let err_t_on_gil = PythonTargetAbi::verify_wheel_extension_abi(
            "PyYAML",
            &ext_314t,
            Some(PythonTargetAbi::new(3, 14, false)),
            "macos",
        )
        .expect_err("cp314t extension must be rejected for GIL python3.14");
        assert!(err_t_on_gil.contains("cp314t"));

        // Free-threaded 3.14t accepts cp314t, rejects cp314 & GIL abi3
        PythonTargetAbi::verify_wheel_extension_abi(
            "PyYAML",
            &ext_314t,
            Some(PythonTargetAbi::new(3, 14, true)),
            "macos",
        )?;
        let err_gil_on_t = PythonTargetAbi::verify_wheel_extension_abi(
            "PyYAML",
            &ext_314,
            Some(PythonTargetAbi::new(3, 14, true)),
            "macos",
        )
        .expect_err("cp314 extension must be rejected for python3.14t");
        assert!(err_gil_on_t.contains("cp314"));
        let err_abi3_on_t = PythonTargetAbi::verify_wheel_extension_abi(
            "cryptography",
            &ext_abi3,
            Some(PythonTargetAbi::new(3, 14, true)),
            "macos",
        )
        .expect_err("GIL abi3 extension must be rejected for python3.14t");
        assert!(err_abi3_on_t.contains("abi3"));
        Ok(())
    }
}
