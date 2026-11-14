use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Discovers official standalone PHP runtime and Composer package manager.
#[derive(Debug, Default, Clone)]
pub struct PhpDiscoverer;

impl ToolchainDiscoverer for PhpDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Php
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_php_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((php_bin, version)) =
            select_strict_standalone_verified(candidates, probe_php_version)
        else {
            return Ok(None);
        };

        let Some(php_root) = resolve_prefix_root(&php_bin) else {
            return Ok(None);
        };

        let redacted_root = redact_home_prefix(&php_root);
        info!(
            root = %redacted_root,
            version = %version,
            "Discovered host PHP toolchain"
        );

        let bin_files = collect_php_binaries(&php_bin);
        let lib_files = collect_php_assets(&php_root);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Php,
            version,
            root_dir: php_root,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_php_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(php_home) = std::env::var_os("PHP_HOME").filter(|v| !v.is_empty()) {
        let base = PathBuf::from(php_home);
        for rel in ["bin/php", "php.exe", "php"] {
            let p = base.join(rel);
            if p.is_file() {
                push_unique_path(&mut candidates, p);
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        let user_php = PathBuf::from(home).join(".php/bin/php");
        if user_php.is_file() {
            push_unique_path(&mut candidates, user_php);
        }
    }

    if cfg!(target_os = "windows") {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            let p = PathBuf::from(local_app_data).join("Programs/PHP/php.exe");
            if p.is_file() {
                push_unique_path(&mut candidates, p);
            }
        }
        let win_default = PathBuf::from(r"C:\php\php.exe");
        if win_default.is_file() {
            push_unique_path(&mut candidates, win_default);
        }
    }

    let opt_php = PathBuf::from("/opt/php/bin/php");
    if opt_php.is_file() {
        push_unique_path(&mut candidates, opt_php);
    }

    push_system_bin_candidates(&mut candidates, "php");
    candidates
}

fn probe_php_version(candidate: &Path) -> Option<String> {
    if is_macos_usr_bin_stub(candidate) || !candidate.is_file() {
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
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "php".to_string());

    Some(version)
}

fn collect_php_binaries(php_bin: &Path) -> Vec<(PathBuf, String)> {
    let mut bin_files = vec![(php_bin.to_path_buf(), "bin/php".into())];

    if let Some(bin_dir) = php_bin.parent().filter(|p| !p.as_os_str().is_empty()) {
        let composer_bin = bin_dir.join("composer");
        if composer_bin.is_file() {
            bin_files.push((composer_bin, "bin/composer".into()));
            return bin_files;
        }
    }

    if let Ok(composer) = which::which("composer") {
        if is_non_standalone_path(&composer) {
            emit_non_relocatable_notice(&composer);
        } else if !is_macos_usr_bin_stub(&composer) {
            bin_files.push((composer, "bin/composer".into()));
        }
    }

    bin_files
}

fn collect_php_assets(php_root: &Path) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    for conf_sub in &["php.ini", "etc/php.ini", "etc/php"] {
        let p = php_root.join(conf_sub);
        if p.is_file() {
            lib_files.push((p, (*conf_sub).to_string()));
        } else if p.is_dir() {
            collect_files_recursive(&p, php_root, &mut lib_files, "");
        }
    }

    let lib_php = php_root.join("lib/php");
    if lib_php.is_dir() {
        collect_files_recursive(&lib_php, php_root, &mut lib_files, "");
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_php_collects_ini_and_extensions() {
        let temp = tempdir().unwrap();
        let php_root = temp.path().join("fake_php");
        let bin = php_root.join("bin");
        let ext = php_root.join("lib/php/extensions/no-debug-non-zts-20230831");

        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&ext).unwrap();

        fs::write(bin.join("php"), "php").unwrap();
        fs::write(bin.join("composer"), "composer").unwrap();
        fs::write(php_root.join("php.ini"), "[PHP]\n").unwrap();
        fs::write(ext.join("openssl.so"), "openssl").unwrap();
        fs::write(ext.join("phar.so"), "phar").unwrap();

        let mut lib_files = Vec::new();

        for conf_sub in &["php.ini", "etc/php.ini", "etc/php"] {
            let p = php_root.join(conf_sub);
            if p.is_file() {
                lib_files.push((p, conf_sub.to_string()));
            } else if p.is_dir() {
                collect_files_recursive(&p, &php_root, &mut lib_files, "");
            }
        }
        let lib_php = php_root.join("lib/php");
        if lib_php.is_dir() {
            collect_files_recursive(&lib_php, &php_root, &mut lib_files, "");
        }

        let rel_paths: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(rel_paths.contains(&"php.ini".to_string()));
        assert!(rel_paths.iter().any(|p| p.contains("openssl.so")));
        assert!(rel_paths.iter().any(|p| p.contains("phar.so")));
    }

    #[test]
    fn test_php_rejects_homebrew_stub_and_root_fallback() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/php"),
            PathBuf::from("/usr/local/Cellar/php/8.3.0/bin/php"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("8.3.0".to_string())),
            None
        );
        assert!(probe_php_version(Path::new("/opt/homebrew/bin/php")).is_none());
        if cfg!(target_os = "macos") {
            assert!(is_macos_usr_bin_stub(Path::new("/usr/bin/php")));
            assert!(probe_php_version(Path::new("/usr/bin/php")).is_none());
        }
        assert_eq!(resolve_prefix_root(Path::new("/php")), None);
    }
}
