use super::super::types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainProfile,
};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Returns `true` when `path` points to Apple's system Ruby framework or `/usr/bin/ruby` stub.
pub fn is_unrelocatable_ruby_path(path: &Path) -> bool {
    if is_macos_usr_bin_stub(path) {
        return true;
    }
    if cfg!(target_os = "macos") {
        let is_apple_ruby = |p: &Path| {
            p.to_string_lossy()
                .contains("/System/Library/Frameworks/Ruby.framework")
        };
        if is_apple_ruby(path) {
            return true;
        }
        if let Ok(canonical) = fs::canonicalize(path) {
            if is_apple_ruby(&canonical) {
                return true;
            }
        }
    }
    false
}

/// Discovers official standalone Ruby runtime and Bundler package manager.
#[derive(Debug, Default, Clone)]
pub struct RubyDiscoverer;

impl ToolchainDiscoverer for RubyDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Ruby
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        self.discover_with_context(&ToolchainDiscoveryContext::default())
    }

    fn discover_with_context(
        &self,
        ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_ruby_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((ruby_bin, version)) =
            select_strict_standalone_verified(candidates, probe_ruby_version)
        else {
            return Ok(None);
        };

        let Some(ruby_root) = resolve_prefix_root(&ruby_bin) else {
            return Ok(None);
        };

        let redacted_root = redact_home_prefix(&ruby_root);
        info!(
            root = %redacted_root,
            version = %version,
            profile = %ctx.profile,
            "Discovered host Ruby toolchain"
        );

        let bin_files = collect_ruby_binaries(&ruby_bin);
        let lib_files = collect_ruby_libs_for_profile(&ruby_root, ctx.profile);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ruby,
            version,
            root_dir: ruby_root,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_ruby_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for var in ["RUBY_HOME", "RUBY_ROOT"] {
        if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            for rel in ["bin/ruby", "bin/ruby.exe"] {
                let p = PathBuf::from(&dir).join(rel);
                if p.is_file() {
                    push_unique_path(&mut candidates, p);
                }
            }
        }
    }

    let opt_ruby = PathBuf::from("/opt/ruby/bin/ruby");
    if opt_ruby.is_file() {
        push_unique_path(&mut candidates, opt_ruby);
    }

    if cfg!(target_os = "windows") {
        if let Ok(entries) = fs::read_dir(r"C:\") {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("Ruby"))
                {
                    let exe = p.join("bin/ruby.exe");
                    if exe.is_file() {
                        push_unique_path(&mut candidates, exe);
                    }
                }
            }
        }
    }

    push_system_bin_candidates(&mut candidates, "ruby");
    candidates
}

fn probe_ruby_version(candidate: &Path) -> Option<String> {
    if is_unrelocatable_ruby_path(candidate) || !candidate.is_file() {
        return None;
    }

    let output = Command::new(candidate)
        .arg("-v")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ruby".to_string());

    Some(version)
}

fn collect_ruby_binaries(ruby_bin: &Path) -> Vec<(PathBuf, String)> {
    let mut bin_files = vec![(ruby_bin.to_path_buf(), "bin/ruby".into())];
    let bin_dir = ruby_bin.parent().filter(|p| !p.as_os_str().is_empty());

    for companion in &["bundle", "gem"] {
        if let Some(dir) = bin_dir {
            let p = dir.join(companion);
            if p.is_file() {
                bin_files.push((p, format!("bin/{}", companion)));
                continue;
            }
        }
        if let Ok(found) = which::which(companion) {
            if is_non_standalone_path(&found) {
                emit_non_relocatable_notice(&found);
                continue;
            }
            if is_unrelocatable_ruby_path(&found) {
                continue;
            }
            bin_files.push((found, format!("bin/{}", companion)));
        }
    }

    bin_files
}

fn collect_ruby_libs_for_profile(
    ruby_root: &Path,
    profile: ToolchainProfile,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let lib_ruby = ruby_root.join("lib/ruby");
    if lib_ruby.is_dir() {
        if profile == ToolchainProfile::Full {
            collect_files_recursive(&lib_ruby, ruby_root, &mut lib_files, "");
        } else if let Ok(entries) = fs::read_dir(&lib_ruby) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "gems" || name == "site_ruby" {
                    continue;
                }
                if p.is_dir() {
                    collect_files_recursive(&p, ruby_root, &mut lib_files, "");
                } else if p.is_file() {
                    if let Ok(rel) = p.strip_prefix(ruby_root) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        lib_files.push((p, rel_str));
                    }
                }
            }
        }
    }

    let lib_dir = ruby_root.join("lib");
    if lib_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&lib_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let fname = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    if fname.starts_with("libruby")
                        && (fname.ends_with(".dylib") || fname.contains(".so"))
                    {
                        lib_files.push((p, format!("lib/{}", fname)));
                    }
                }
            }
        }
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_ruby_collects_stdlib_and_gem() {
        let temp = tempdir().unwrap();
        let ruby_root = temp.path().join("fake_ruby");
        let bin = ruby_root.join("bin");
        let lib_ruby = ruby_root.join("lib/ruby/3.2.0");
        let lib_sh = ruby_root.join("lib");

        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&lib_ruby).unwrap();

        fs::write(bin.join("ruby"), "ruby").unwrap();
        fs::write(bin.join("gem"), "gem").unwrap();
        fs::write(bin.join("bundle"), "bundle").unwrap();
        fs::write(lib_ruby.join("rubygems.rb"), "# rubygems").unwrap();
        fs::write(lib_ruby.join("rbconfig.rb"), "# rbconfig").unwrap();
        fs::write(lib_sh.join("libruby.3.2.dylib"), "shlib").unwrap();

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((bin.join("ruby"), "bin/ruby".into()));
        for companion in &["bundle", "gem"] {
            let p = bin.join(companion);
            if p.exists() {
                bin_files.push((p, format!("bin/{}", companion)));
            }
        }

        collect_files_recursive(&lib_ruby, &ruby_root, &mut lib_files, "");

        let bin_names: Vec<String> = bin_files.into_iter().map(|(_, r)| r).collect();
        assert!(bin_names.contains(&"bin/gem".to_string()));
        assert!(bin_names.contains(&"bin/bundle".to_string()));

        let lib_names: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(lib_names.contains(&"lib/ruby/3.2.0/rubygems.rb".to_string()));
        assert!(lib_names.contains(&"lib/ruby/3.2.0/rbconfig.rb".to_string()));
    }

    #[test]
    fn test_ruby_rejects_homebrew_and_apple_system_ruby() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/ruby"),
            PathBuf::from("/usr/local/opt/ruby/bin/ruby"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("3.3.0".to_string())),
            None
        );
        if cfg!(target_os = "macos") {
            assert!(is_unrelocatable_ruby_path(Path::new("/usr/bin/ruby")));
            assert!(is_unrelocatable_ruby_path(Path::new(
                "/System/Library/Frameworks/Ruby.framework/Versions/Current/usr/bin/ruby"
            )));
        }
        assert_eq!(resolve_prefix_root(Path::new("/ruby")), None);
    }
}
