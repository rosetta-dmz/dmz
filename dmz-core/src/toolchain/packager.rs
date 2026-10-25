use super::types::{DiscoveredToolchain, ToolchainManifestEntry};
use std::fs;
use std::path::Path;
use tar::Builder as TarBuilder;
use tracing::{info, warn};

/// Packaging engine responsible for serializing discovered toolchains into an airgap archive stream.
pub struct ToolchainArchivePackager;

impl ToolchainArchivePackager {
    /// Packages all provided discovered toolchains into the target tar archive under `toolchains/<ecosystem>/`.
    pub fn package<W: std::io::Write>(
        tar_builder: &mut TarBuilder<W>,
        toolchains: &[DiscoveredToolchain],
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

            // Package binaries with executable permissions (0o755)
            for (host_path, rel_path) in &tc.bin_files {
                let archive_entry_path = format!("{}/{}", base_archive_path, rel_path);
                if let Ok(mut file) = fs::File::open(host_path) {
                    if let Ok(metadata) = file.metadata() {
                        let mut header = tar::Header::new_gnu();
                        header.set_size(metadata.len());
                        header.set_mode(0o755);
                        header.set_cksum();
                        if let Err(e) =
                            tar_builder.append_data(&mut header, &archive_entry_path, &mut file)
                        {
                            warn!(
                                "Failed to append toolchain binary {}: {}",
                                archive_entry_path, e
                            );
                            continue;
                        }
                        if let Some(name) = Path::new(rel_path).file_name().and_then(|n| n.to_str())
                        {
                            binary_names.push(name.to_string());
                        }
                    }
                }
            }

            // Package library / runtime assets (0o644)
            for (host_path, rel_path) in &tc.lib_files {
                let archive_entry_path = format!("{}/{}", base_archive_path, rel_path);
                if let Ok(mut file) = fs::File::open(host_path) {
                    if let Ok(metadata) = file.metadata() {
                        let mut header = tar::Header::new_gnu();
                        header.set_size(metadata.len());
                        header.set_mode(0o644);
                        header.set_cksum();
                        let _ =
                            tar_builder.append_data(&mut header, &archive_entry_path, &mut file);
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
