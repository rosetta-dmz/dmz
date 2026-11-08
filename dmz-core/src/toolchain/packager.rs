use super::types::{DiscoveredToolchain, ToolchainManifestEntry};
use std::fs;
use std::path::Path;
use tar::Builder as TarBuilder;
use tracing::info;

/// Packaging engine responsible for serializing discovered toolchains into an airgap archive stream.
pub struct ToolchainArchivePackager;

impl ToolchainArchivePackager {
    /// Package all provided discovered toolchains into the target tar archive under `toolchains/<ecosystem>/`.
    pub fn package<W: std::io::Write>(
        tar_builder: &mut TarBuilder<W>,
        toolchains: &[DiscoveredToolchain],
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        Self::package_with_epoch(tar_builder, toolchains, 315_532_800)
    }

    /// Package all provided discovered toolchains into the target tar archive with a deterministic timestamp.
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
                let archive_entry_path = format!("{}/{}", base_archive_path, rel_path);
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
                let archive_entry_path = format!("{}/{}", base_archive_path, rel_path);
                if let Ok(mut file) = fs::File::open(host_path) {
                    if let Ok(metadata) = file.metadata() {
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
                }
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
