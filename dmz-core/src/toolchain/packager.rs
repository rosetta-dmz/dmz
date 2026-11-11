use super::types::{DiscoveredToolchain, ToolchainManifestEntry};
use std::fs;
use std::path::Path;
use tar::Builder as TarBuilder;
use tracing::info;

/// Packaging engine responsible for serializing discovered toolchains into an airgap archive stream.
pub struct ToolchainArchivePackager;

impl ToolchainArchivePackager {
    /// Package all provided discovered toolchains into the target tar archive under `toolchains/<ecosystem>/`.
    ///
    /// # Async Runtime Note
    /// Performs synchronous file reads and tar serialization. Use `tokio::task::spawn_blocking` in async contexts.
    pub fn package<W: std::io::Write>(
        tar_builder: &mut TarBuilder<W>,
        toolchains: &[DiscoveredToolchain],
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        Self::package_with_epoch(tar_builder, toolchains, 315_532_800)
    }

    /// Package all provided discovered toolchains into the target tar archive with a deterministic timestamp.
    ///
    /// # Async Runtime Note
    /// Performs synchronous file reads and tar serialization. Use `tokio::task::spawn_blocking` in async contexts.
    pub fn package_with_epoch<W: std::io::Write>(
        tar_builder: &mut TarBuilder<W>,
        toolchains: &[DiscoveredToolchain],
        deterministic_epoch: u64,
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        let mut manifest_entries = Vec::new();

        for tc in toolchains {
            let eco_str = tc.ecosystem.as_str();
            let base_archive_path = format!("toolchains/{}", eco_str);
            let mut binary_names = Vec::new();

            info!(
                ecosystem = eco_str,
                version = %tc.version,
                files_count = tc.bin_files.len() + tc.lib_files.len(),
                "Packaging toolchain into bundle"
            );

            // Sort files for reproducible tar header order
            let mut sorted_bins = tc.bin_files.clone();
            sorted_bins.sort_by(|a, b| a.1.cmp(&b.1));

            // Package binaries with executable permissions (0o755) and normalized ownership/timestamps
            for (host_path, rel_path) in &sorted_bins {
                let rel_p = Path::new(rel_path);
                if rel_p.is_absolute()
                    || rel_p.components().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir
                                | std::path::Component::RootDir
                                | std::path::Component::Prefix(_)
                        )
                    })
                {
                    return Err(format!(
                        "Refusing to package toolchain binary with insecure path: {}",
                        rel_path
                    ));
                }

                let archive_entry_path = format!("{}/{}", base_archive_path, rel_path);

                if let Ok(sym_meta) = fs::symlink_metadata(host_path) {
                    if sym_meta.file_type().is_symlink() {
                        if let Ok(link_target) = fs::read_link(host_path) {
                            // Only preserve self-contained relative symlinks (e.g. clang++ -> clang).
                            // If the symlink is absolute (e.g. rustup shims pointing to ~/.rustup/toolchains/...),
                            // it must be dereferenced and bundled so air-gap targets have the genuine binary.
                            if !link_target.is_absolute()
                                && !link_target.components().any(|c| {
                                    matches!(
                                        c,
                                        std::path::Component::ParentDir
                                            | std::path::Component::RootDir
                                            | std::path::Component::Prefix(_)
                                    )
                                })
                            {
                                let mut header = tar::Header::new_gnu();
                                header.set_entry_type(tar::EntryType::Symlink);
                                header.set_size(0);
                                header.set_mode(0o777);
                                header.set_mtime(deterministic_epoch);
                                header.set_uid(0);
                                header.set_gid(0);
                                let _ = header.set_username("");
                                let _ = header.set_groupname("");
                                let _ = header.set_link_name(&link_target);
                                header.set_cksum();
                                tar_builder
                                    .append_data(
                                        &mut header,
                                        &archive_entry_path,
                                        &mut std::io::empty(),
                                    )
                                    .map_err(|e| {
                                        format!(
                                            "Failed to append toolchain symlink {}: {}",
                                            archive_entry_path, e
                                        )
                                    })?;
                                if let Some(name) =
                                    Path::new(rel_path).file_name().and_then(|n| n.to_str())
                                {
                                    binary_names.push(name.to_string());
                                }
                                continue;
                            }
                        }
                    }
                }

                let mut file = fs::File::open(host_path).map_err(|e| {
                    format!(
                        "Failed to open toolchain binary {:?} for {}: {}",
                        host_path, eco_str, e
                    )
                })?;
                let metadata = file.metadata().map_err(|e| {
                    format!(
                        "Failed to read metadata for toolchain binary {:?}: {}",
                        host_path, e
                    )
                })?;
                let mut header = tar::Header::new_gnu();
                header.set_size(metadata.len());
                header.set_mode(0o755);
                header.set_mtime(deterministic_epoch);
                header.set_uid(0);
                header.set_gid(0);
                let _ = header.set_username("");
                let _ = header.set_groupname("");
                header.set_cksum();
                tar_builder
                    .append_data(&mut header, &archive_entry_path, &mut file)
                    .map_err(|e| {
                        format!(
                            "Failed to append toolchain binary {}: {}",
                            archive_entry_path, e
                        )
                    })?;
                if let Some(name) = Path::new(rel_path).file_name().and_then(|n| n.to_str()) {
                    binary_names.push(name.to_string());
                }
            }

            // Sort library / runtime assets
            let mut sorted_libs = tc.lib_files.clone();
            sorted_libs.sort_by(|a, b| a.1.cmp(&b.1));

            // Package library / runtime assets (0o755 for executables/shared libraries, 0o644 for headers/data)
            for (host_path, rel_path) in &sorted_libs {
                let rel_p = Path::new(rel_path);
                if rel_p.is_absolute()
                    || rel_p.components().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir
                                | std::path::Component::RootDir
                                | std::path::Component::Prefix(_)
                        )
                    })
                {
                    return Err(format!(
                        "Refusing to package toolchain asset with insecure path: {}",
                        rel_path
                    ));
                }

                let archive_entry_path = format!("{}/{}", base_archive_path, rel_path);

                if let Ok(sym_meta) = fs::symlink_metadata(host_path) {
                    if sym_meta.file_type().is_symlink() {
                        if let Ok(link_target) = fs::read_link(host_path) {
                            if !link_target.is_absolute()
                                && !link_target.components().any(|c| {
                                    matches!(
                                        c,
                                        std::path::Component::ParentDir
                                            | std::path::Component::RootDir
                                            | std::path::Component::Prefix(_)
                                    )
                                })
                            {
                                let mut header = tar::Header::new_gnu();
                                header.set_entry_type(tar::EntryType::Symlink);
                                header.set_size(0);
                                header.set_mode(0o777);
                                header.set_mtime(deterministic_epoch);
                                header.set_uid(0);
                                header.set_gid(0);
                                let _ = header.set_username("");
                                let _ = header.set_groupname("");
                                let _ = header.set_link_name(&link_target);
                                header.set_cksum();
                                tar_builder
                                    .append_data(
                                        &mut header,
                                        &archive_entry_path,
                                        &mut std::io::empty(),
                                    )
                                    .map_err(|e| {
                                        format!(
                                            "Failed to append toolchain symlink {}: {}",
                                            archive_entry_path, e
                                        )
                                    })?;
                                continue;
                            }
                        }
                    }
                }

                let mut file = match fs::File::open(host_path) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(
                            path = ?host_path,
                            error = %e,
                            "Skipping unreadable toolchain asset"
                        );
                        continue;
                    }
                };

                let metadata = match file.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(
                            path = ?host_path,
                            error = %e,
                            "Skipping toolchain asset with unreadable metadata"
                        );
                        continue;
                    }
                };

                let is_executable_or_shlib = {
                    let p_str = rel_path.to_lowercase();
                    p_str.ends_with(".dylib")
                        || p_str.ends_with(".so")
                        || p_str.contains(".so.")
                        || p_str.ends_with(".dll")
                        || p_str.starts_with("pkg/tool/")
                        || p_str.contains("libexec/")
                        || p_str.contains("cc1")
                        || p_str.contains("collect2")
                };
                let mode = if is_executable_or_shlib { 0o755 } else { 0o644 };
                let mut header = tar::Header::new_gnu();
                header.set_size(metadata.len());
                header.set_mode(mode);
                header.set_mtime(deterministic_epoch);
                header.set_uid(0);
                header.set_gid(0);
                let _ = header.set_username("");
                let _ = header.set_groupname("");
                header.set_cksum();
                tar_builder
                    .append_data(&mut header, &archive_entry_path, &mut file)
                    .map_err(|e| {
                        format!(
                            "Failed to append toolchain asset {}: {}",
                            archive_entry_path, e
                        )
                    })?;
            }

            manifest_entries.push(ToolchainManifestEntry {
                ecosystem: tc.ecosystem,
                version: tc.version.clone(),
                path_in_archive: base_archive_path,
                binaries: binary_names,
            });
        }

        Ok(manifest_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::types::ToolchainEcosystem;
    use std::io::Cursor;
    use tar::Archive;
    use tempfile::tempdir;

    #[test]
    fn test_package_toolchain_with_deterministic_epoch_and_permissions() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let bin_path = ws.join("clang");
        fs::write(&bin_path, "#!/bin/sh\nexit 0\n").unwrap();

        let shlib_path = ws.join("libclang.dylib");
        fs::write(&shlib_path, "mock dylib bytes").unwrap();

        let header_path = ws.join("stddef.h");
        fs::write(&header_path, "/* stddef */").unwrap();

        let toolchain = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version: "17.0.0".to_string(),
            bin_files: vec![(bin_path, "bin/clang".to_string())],
            lib_files: vec![
                (shlib_path, "lib/libclang.dylib".to_string()),
                (header_path, "include/stddef.h".to_string()),
            ],
            root_dir: ws.to_path_buf(),
        };

        let mut tar_builder = TarBuilder::new(Vec::new());
        let epoch = 1_700_000_000u64;

        let entries =
            ToolchainArchivePackager::package_with_epoch(&mut tar_builder, &[toolchain], epoch)
                .unwrap();

        let tar_buf = tar_builder.into_inner().unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ecosystem, ToolchainEcosystem::Cc);
        assert_eq!(entries[0].version, "17.0.0");
        assert_eq!(entries[0].binaries, vec!["clang"]);
        assert_eq!(entries[0].path_in_archive, "toolchains/cc");

        let mut archive = Archive::new(Cursor::new(tar_buf));
        let mut entry_count = 0;

        for entry_res in archive.entries().unwrap() {
            let entry = entry_res.unwrap();
            let header = entry.header();
            let path_str = entry.path().unwrap().to_string_lossy().to_string();

            assert_eq!(header.mtime().unwrap(), epoch);
            assert_eq!(header.uid().unwrap(), 0);
            assert_eq!(header.gid().unwrap(), 0);

            if path_str == "toolchains/cc/bin/clang"
                || path_str == "toolchains/cc/lib/libclang.dylib"
            {
                assert_eq!(header.mode().unwrap() & 0o777, 0o755);
            } else if path_str == "toolchains/cc/include/stddef.h" {
                assert_eq!(header.mode().unwrap() & 0o777, 0o644);
            }

            entry_count += 1;
        }

        assert_eq!(entry_count, 3);
    }

    #[test]
    fn test_package_toolchain_preserves_relative_symlink() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let real_bin = ws.join("clang");
        fs::write(&real_bin, "#!/bin/sh\nexit 0\n").unwrap();

        let symlink_bin = ws.join("clang++");
        #[cfg(unix)]
        std::os::unix::fs::symlink("clang", &symlink_bin).unwrap();
        #[cfg(not(unix))]
        fs::write(&symlink_bin, "stub").unwrap();

        let toolchain = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version: "17.0.0".to_string(),
            bin_files: vec![
                (real_bin, "bin/clang".to_string()),
                (symlink_bin, "bin/clang++".to_string()),
            ],
            lib_files: Vec::new(),
            root_dir: ws.to_path_buf(),
        };

        let mut tar_builder = TarBuilder::new(Vec::new());

        let entries = ToolchainArchivePackager::package(&mut tar_builder, &[toolchain]).unwrap();
        let tar_buf = tar_builder.into_inner().unwrap();

        assert_eq!(entries.len(), 1);
        #[cfg(unix)]
        {
            let mut archive = Archive::new(Cursor::new(tar_buf));
            let mut found_symlink = false;
            for entry_res in archive.entries().unwrap() {
                let entry = entry_res.unwrap();
                let header = entry.header();
                if entry.path().unwrap().to_string_lossy() == "toolchains/cc/bin/clang++" {
                    assert_eq!(header.entry_type(), tar::EntryType::Symlink);
                    assert_eq!(
                        entry.link_name().unwrap().unwrap().to_string_lossy(),
                        "clang"
                    );
                    found_symlink = true;
                }
            }
            assert!(found_symlink);
        }
    }

    #[test]
    fn test_package_toolchain_rejects_path_traversal() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let fake_bin = ws.join("untrusted");
        fs::write(&fake_bin, "untrusted").unwrap();

        let toolchain = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "1.75.0".to_string(),
            bin_files: vec![(fake_bin.clone(), "../etc/shadow".to_string())],
            lib_files: Vec::new(),
            root_dir: ws.to_path_buf(),
        };

        let mut tar_buf = Vec::new();
        let mut tar_builder = TarBuilder::new(&mut tar_buf);
        let res = ToolchainArchivePackager::package(&mut tar_builder, &[toolchain]);

        assert!(res.is_err());
        assert!(res.unwrap_err().contains("insecure path"));

        let toolchain_lib = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "1.75.0".to_string(),
            bin_files: Vec::new(),
            lib_files: vec![(fake_bin, "/usr/bin/untrusted".to_string())],
            root_dir: ws.to_path_buf(),
        };

        let mut tar_buf2 = Vec::new();
        let mut tar_builder2 = TarBuilder::new(&mut tar_buf2);
        let res2 = ToolchainArchivePackager::package(&mut tar_builder2, &[toolchain_lib]);

        assert!(res2.is_err());
        assert!(res2.unwrap_err().contains("insecure path"));
    }

    #[test]
    fn test_package_empty_toolchains() {
        let mut tar_buf = Vec::new();
        let mut tar_builder = TarBuilder::new(&mut tar_buf);

        let entries = ToolchainArchivePackager::package(&mut tar_builder, &[]).unwrap();
        tar_builder.finish().unwrap();

        assert!(entries.is_empty());
    }
}
