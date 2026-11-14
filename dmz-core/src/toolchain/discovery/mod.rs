use super::types::{DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem};
use super::ToolchainError;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

pub mod android;
pub mod apple;
pub mod cc;
pub mod cmake;
pub mod go;
pub mod juce;
pub mod make;
pub mod ninja;
pub mod node;
pub mod php;
pub mod pkgconfig;
pub mod python;
pub mod ruby;
pub mod rust;

pub use android::AndroidNdkDiscoverer;
pub use apple::AppleSdkDiscoverer;
pub use cc::CcDiscoverer;
pub use cmake::CMakeDiscoverer;
pub use go::GoDiscoverer;
pub use juce::JuceDiscoverer;
pub use make::MakeDiscoverer;
pub use ninja::NinjaDiscoverer;
pub use node::NodeDiscoverer;
pub use php::PhpDiscoverer;
pub use pkgconfig::PkgConfigDiscoverer;
pub use python::PythonDiscoverer;
pub use ruby::RubyDiscoverer;
pub use rust::RustDiscoverer;

pub trait ToolchainDiscoverer: Send + Sync {
    /// Returns the target ecosystem managed by this discoverer.
    fn ecosystem(&self) -> ToolchainEcosystem;

    /// Discovers the host toolchain, returning resolved binaries, shared libraries, and metadata.
    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError>;

    /// Discovers the host toolchain using profile, target platform, and workspace context.
    fn discover_with_context(
        &self,
        _ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        self.discover()
    }
}

/// Registry coordinating one or more `ToolchainDiscoverer` implementations.
pub struct DiscoveryRegistry {
    discoverers: Vec<Box<dyn ToolchainDiscoverer>>,
}

impl Default for DiscoveryRegistry {
    fn default() -> Self {
        Self::with_default_discoverers()
    }
}

impl DiscoveryRegistry {
    /// Constructs a registry with the default standalone discoverers for all supported ecosystems.
    pub fn with_default_discoverers() -> Self {
        Self {
            discoverers: vec![
                Box::new(RustDiscoverer),
                Box::new(NodeDiscoverer),
                Box::new(PythonDiscoverer),
                Box::new(GoDiscoverer),
                Box::new(PhpDiscoverer),
                Box::new(RubyDiscoverer),
                Box::new(CMakeDiscoverer),
                Box::new(NinjaDiscoverer),
                Box::new(MakeDiscoverer),
                Box::new(JuceDiscoverer),
                Box::new(PkgConfigDiscoverer),
                Box::new(CcDiscoverer),
                Box::new(AndroidNdkDiscoverer),
                Box::new(AppleSdkDiscoverer),
            ],
        }
    }

    /// Creates an empty registry for custom discoverer configurations.
    pub fn empty() -> Self {
        Self {
            discoverers: Vec::new(),
        }
    }

    /// Registers a custom or alternate toolchain discoverer (Open/Closed Principle).
    pub fn register(&mut self, discoverer: Box<dyn ToolchainDiscoverer>) {
        self.discoverers.push(discoverer);
    }

    /// Discovers all available standalone toolchains across registered discoverers.
    pub fn discover_all(&self) -> Vec<DiscoveredToolchain> {
        let mut toolchains = Vec::new();
        for discoverer in &self.discoverers {
            if let Ok(Some(tc)) = discoverer.discover() {
                toolchains.push(tc);
            }
        }
        toolchains
    }

    /// Discovers standalone host toolchains scoped strictly to the specified active ecosystems.
    pub fn discover_for_ecosystems(
        &self,
        ecosystems: &HashSet<ToolchainEcosystem>,
    ) -> Vec<DiscoveredToolchain> {
        let ctx = ToolchainDiscoveryContext {
            ecosystems: ecosystems.clone(),
            ..ToolchainDiscoveryContext::default()
        };
        self.discover_for_ecosystems_with_context(ecosystems, &ctx)
    }

    /// Discovers standalone host toolchains scoped strictly to `ecosystems` and `ctx`.
    pub fn discover_for_ecosystems_with_context(
        &self,
        ecosystems: &HashSet<ToolchainEcosystem>,
        ctx: &ToolchainDiscoveryContext,
    ) -> Vec<DiscoveredToolchain> {
        let mut toolchains = Vec::new();
        for discoverer in &self.discoverers {
            if ecosystems.contains(&discoverer.ecosystem()) {
                if let Ok(Some(tc)) = discoverer.discover_with_context(ctx) {
                    toolchains.push(tc);
                }
            }
        }
        toolchains
    }
}

/// Detects whether a path is non-standalone / non-relocatable (e.g. managed by system package managers
/// that dynamically link against machine-specific cellar/system libraries).
pub fn is_non_standalone_path(path: &Path) -> bool {
    let check = |p: &Path| {
        let s = p.to_string_lossy();
        s.contains("/opt/homebrew/")
            || s.contains("/usr/local/Cellar/")
            || s.contains("/usr/local/opt/")
            || s.contains("/home/linuxbrew/")
            || s.contains("/var/lib/snapd/")
            || s.contains("/snap/")
            || s.contains("/nix/store/")
    };
    if check(path) {
        return true;
    }
    if let Ok(canonical) = fs::canonicalize(path) {
        if check(&canonical) {
            return true;
        }
    }
    false
}

/// Redacts any host `$HOME` / `$USERPROFILE` prefix from logged paths (CWE-532).
pub(super) fn redact_home_prefix(path: &Path) -> String {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(var) {
            if !home.is_empty() {
                if let Ok(rel) = path.strip_prefix(Path::new(&home)) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    return if rel_str.is_empty() {
                        "~".to_string()
                    } else {
                        format!("~/{}", rel_str)
                    };
                }
            }
        }
    }
    path.to_string_lossy().into_owned()
}

/// Emits standard agnostic guidance when a non-relocatable toolchain is encountered.
pub fn emit_non_relocatable_notice(path: &Path) {
    let redacted = redact_home_prefix(path);
    warn!(
        "Non-relocatable toolchain detected at {:?}. Please use the official standalone toolchain from the provider.",
        redacted
    );
}

/// Returns `true` when `path` points to a macOS `/usr/bin/*` system/xcode-select stub.
pub fn is_macos_usr_bin_stub(path: &Path) -> bool {
    if cfg!(target_os = "macos") {
        is_usr_bin_path(path)
    } else {
        false
    }
}

/// Returns `true` when `path` resides inside an Apple-managed Developer installation
/// (`/Library/Developer/CommandLineTools`, `Xcode*.app/Contents/Developer`, or `/usr/bin/*` stub)
/// which must be treated as a macOS host baseline prerequisite and never bundled.
pub fn is_apple_system_toolchain_path(path: &Path) -> bool {
    let matches_apple = |p: &Path| {
        let s = p.to_string_lossy();
        s.starts_with("/Library/Developer/CommandLineTools")
            || s.contains(".app/Contents/Developer/")
            || s.ends_with(".app/Contents/Developer")
            || s.contains("XcodeDefault.xctoolchain")
    };

    if is_macos_usr_bin_stub(path) || matches_apple(path) {
        return true;
    }

    fs::canonicalize(path)
        .ok()
        .is_some_and(|canon| matches_apple(&canon))
}

fn is_usr_bin_path(path: &Path) -> bool {
    if path.parent() == Some(Path::new("/usr/bin")) {
        return true;
    }
    fs::canonicalize(path)
        .ok()
        .is_some_and(|canonical| canonical.parent() == Some(Path::new("/usr/bin")))
}

/// Resolves a toolchain prefix root from a binary path (e.g., `/opt/pkg/bin/tool` -> `/opt/pkg`),
/// refusing empty paths and filesystem/drive roots (`/` or `C:\`).
pub fn resolve_prefix_root(bin_path: &Path) -> Option<PathBuf> {
    let resolved = fs::canonicalize(bin_path).unwrap_or_else(|_| bin_path.to_path_buf());
    let bin_dir = resolved.parent()?;
    let prefix = if bin_dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
    {
        bin_dir.parent()?
    } else {
        bin_dir
    };
    if prefix.as_os_str().is_empty() || prefix == Path::new("/") || prefix.parent().is_none() {
        return None;
    }
    Some(prefix.to_path_buf())
}

/// Appends `(src, rel)` only if `rel` has not already been recorded in `seen_rel`.
pub(super) fn push_deduped_file(
    acc: &mut Vec<(PathBuf, String)>,
    seen_rel: &mut HashSet<String>,
    src: PathBuf,
    rel: String,
) {
    if seen_rel.insert(rel.clone()) {
        acc.push((src, rel));
    }
}

/// Appends a path to the candidates list if not already present.
pub fn push_unique_path(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.contains(&path) {
        candidates.push(path);
    }
}

pub fn push_system_bin_candidates(candidates: &mut Vec<PathBuf>, bin_name: &str) {
    if let Ok(paths) = which::which_all(bin_name) {
        for p in paths {
            push_unique_path(candidates, p);
        }
    }
    for dir in &["/usr/local/bin", "/usr/bin", "/bin"] {
        let p = Path::new(dir).join(bin_name);
        if p.exists() {
            push_unique_path(candidates, p);
        }
    }
}

/// Searches all `PATH` entries for the first standalone executable matching `bin_name`.
pub(super) fn find_standalone_bin_in_path(
    bin_name: &str,
    reject_apple_clt: bool,
) -> Option<PathBuf> {
    let paths = which::which_all(bin_name).ok()?;
    for found in paths {
        if is_non_standalone_path(&found) {
            emit_non_relocatable_notice(&found);
            continue;
        }
        if reject_apple_clt && is_apple_system_toolchain_path(&found) {
            continue;
        }
        return Some(found);
    }
    None
}

/// Selects the highest-priority genuinely standalone candidate from `candidates` and strictly
/// rejects non-relocatable package-manager installs (Homebrew, Linuxbrew, Nix, Snap) without
/// falling back to them.
pub fn select_strict_standalone_candidate(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    select_strict_standalone_verified(candidates, |_| Some(())).map(|(p, ())| p)
}

/// Evaluates `candidates` in priority order using `probe`, strictly rejecting non-standalone
/// package-manager installs (Homebrew, Linuxbrew, Nix, Snap) with a notice and never falling
/// back to them.
pub fn select_strict_standalone_verified<T, F>(
    candidates: Vec<PathBuf>,
    mut probe: F,
) -> Option<(PathBuf, T)>
where
    F: FnMut(&Path) -> Option<T>,
{
    for candidate in candidates {
        if is_non_standalone_path(&candidate) {
            emit_non_relocatable_notice(&candidate);
            continue;
        }
        if let Some(value) = probe(&candidate) {
            return Some((candidate, value));
        }
    }
    None
}

/// Recursively collects all files within a directory relative to a base directory,
/// with bounded depth to prevent stack overflow and symlink loops (CWE-400).
pub fn collect_files_recursive(
    dir: &Path,
    base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    target_prefix: &str,
) {
    collect_files_bounded(dir, base, acc, target_prefix, 0, 32);
}

fn collect_files_bounded(
    dir: &Path,
    base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    target_prefix: &str,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && !p.is_symlink() {
                collect_files_bounded(&p, base, acc, target_prefix, depth + 1, max_depth);
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
                    acc.push((p.clone(), target_path));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_strict_standalone_candidate_never_falls_back_to_non_standalone() {
        let non_standalone = vec![
            PathBuf::from("/opt/homebrew/bin/python3"),
            PathBuf::from("/usr/local/Cellar/node/22.0.0/bin/node"),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin/go"),
            PathBuf::from("/nix/store/abc123-go-1.22/bin/go"),
            PathBuf::from("/snap/node/current/bin/node"),
        ];
        assert_eq!(select_strict_standalone_candidate(non_standalone), None);

        let mixed = vec![
            PathBuf::from("/opt/homebrew/bin/node"),
            PathBuf::from("/usr/local/go/bin/go"),
        ];
        assert_eq!(
            select_strict_standalone_candidate(mixed),
            Some(PathBuf::from("/usr/local/go/bin/go"))
        );
    }

    #[test]
    fn test_select_strict_standalone_verified_rejects_homebrew_and_failed_probes() {
        let candidates = vec![
            PathBuf::from("/opt/homebrew/bin/cmake"),
            PathBuf::from("/usr/local/bad/bin/cmake"),
            PathBuf::from("/opt/cmake/bin/cmake"),
        ];
        let picked = select_strict_standalone_verified(candidates, |p| {
            if p == Path::new("/opt/cmake/bin/cmake") {
                Some("3.30.0".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            picked,
            Some((PathBuf::from("/opt/cmake/bin/cmake"), "3.30.0".to_string()))
        );

        let only_brew = vec![PathBuf::from("/opt/homebrew/bin/cmake")];
        assert_eq!(
            select_strict_standalone_verified(only_brew, |_| Some("3.30.0")),
            None
        );
    }

    #[test]
    fn test_resolve_prefix_root_and_usr_bin_detection() {
        assert_eq!(
            resolve_prefix_root(Path::new("/opt/pkg/bin/pkg-config")),
            Some(PathBuf::from("/opt/pkg"))
        );
        assert_eq!(
            resolve_prefix_root(Path::new("/opt/ninja/ninja")),
            Some(PathBuf::from("/opt/ninja"))
        );
        assert_eq!(
            resolve_prefix_root(Path::new("/bin/dmz_nonexistent_tool")),
            None
        );
        assert_eq!(resolve_prefix_root(Path::new("bin/tool")), None);
        assert_eq!(resolve_prefix_root(Path::new("/tool")), None);

        assert!(is_usr_bin_path(Path::new("/usr/bin/php")));
        assert!(!is_usr_bin_path(Path::new("/usr/local/bin/php")));
    }

    #[test]
    fn test_redact_home_prefix_masks_user_home() {
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let sample = Path::new(&home).join(".local/bin/tool");
            assert_eq!(redact_home_prefix(&sample), "~/.local/bin/tool");
            assert_eq!(redact_home_prefix(Path::new(&home)), "~");
        }
        assert_eq!(
            redact_home_prefix(Path::new("/opt/toolchain/bin/tool")),
            "/opt/toolchain/bin/tool"
        );
    }
}
