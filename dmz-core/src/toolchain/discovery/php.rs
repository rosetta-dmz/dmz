use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path,
    ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Discovers host PHP runtime and Composer package manager.
#[derive(Debug, Default, Clone)]
pub struct PhpDiscoverer;

impl ToolchainDiscoverer for PhpDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Php
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let php_bin = match which::which("php") {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        if is_non_standalone_path(&php_bin) {
            emit_non_relocatable_notice(&php_bin);
        }

        let version = Command::new(&php_bin)
            .arg("-v")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.to_string()))
            .unwrap_or_else(|| "php".to_string());

        let canonical_php = fs::canonicalize(&php_bin).unwrap_or_else(|_| php_bin.clone());
        let php_root = canonical_php
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((php_bin.clone(), "bin/php".into()));

        let bin_dir = php_bin.parent().unwrap_or(Path::new(""));
        let composer_bin = bin_dir.join("composer");
        if composer_bin.exists() {
            bin_files.push((composer_bin, "bin/composer".into()));
        } else if let Ok(composer) = which::which("composer") {
            if is_non_standalone_path(&composer) {
                emit_non_relocatable_notice(&composer);
            } else {
                bin_files.push((composer, "bin/composer".into()));
            }
        }

        // Configuration files (php.ini, etc/php.ini, etc/php)
        for conf_sub in &["php.ini", "etc/php.ini", "etc/php"] {
            let p = php_root.join(conf_sub);
            if p.is_file() {
                lib_files.push((p, conf_sub.to_string()));
            } else if p.is_dir() {
                collect_files_recursive(&p, &php_root, &mut lib_files, "");
            }
        }

        // PHP extensions under lib/php
        let lib_php = php_root.join("lib/php");
        if lib_php.is_dir() {
            collect_files_recursive(&lib_php, &php_root, &mut lib_files, "");
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Php,
            version,
            root_dir: php_root,
            bin_files,
            lib_files,
        }))
    }
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
}
