use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path,
    ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Discovers host Ruby runtime and Bundler package manager.
#[derive(Debug, Default, Clone)]
pub struct RubyDiscoverer;

impl ToolchainDiscoverer for RubyDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Ruby
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let ruby_bin = match which::which("ruby") {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        if is_non_standalone_path(&ruby_bin) {
            emit_non_relocatable_notice(&ruby_bin);
        }

        let version = Command::new(&ruby_bin)
            .arg("-v")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "ruby".to_string())
            .trim()
            .to_string();

        let ruby_root = ruby_bin
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((ruby_bin.clone(), "bin/ruby".into()));

        let bin_dir = ruby_bin.parent().unwrap_or(Path::new(""));
        for companion in &["bundle", "gem"] {
            let p = bin_dir.join(companion);
            if p.exists() {
                bin_files.push((p, format!("bin/{}", companion)));
            } else if let Ok(found) = which::which(companion) {
                if is_non_standalone_path(&found) {
                    emit_non_relocatable_notice(&found);
                    continue;
                }
                bin_files.push((found, format!("bin/{}", companion)));
            }
        }

        // Standard library, rubygems, and rbconfig under lib/ruby
        let lib_ruby = ruby_root.join("lib/ruby");
        if lib_ruby.is_dir() {
            collect_files_recursive(&lib_ruby, &ruby_root, &mut lib_files, "");
        }

        // Shared libruby libraries under lib/
        let lib_dir = ruby_root.join("lib");
        if lib_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&lib_dir) {
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

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ruby,
            version,
            root_dir: ruby_root,
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
}
