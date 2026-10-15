use eframe::egui;
use std::path::PathBuf;

use super::super::components;
use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};

pub(super) struct AirGapState {
    archive_path: String,
    store_dir: String,
}

impl Default for AirGapState {
    fn default() -> Self {
        Self {
            archive_path: String::from("dist/airgap_bundle.tar.zst"),
            store_dir: String::from(".dmz/store"),
        }
    }
}

pub(super) fn show(
    state: &mut AirGapState,
    ui: &mut egui::Ui,
    workspace_path: &str,
    lockfile_path: &str,
    zstd_level: i32,
    status_message: &mut String,
) {
    components::page_header(
        ui,
        "Air-Gap Manager",
        "Export and import verified dependency closures for offline environments.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Bundle paths");
        egui::Grid::new("airgap_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label("Bundle archive");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.archive_path),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Zstd Archive", &["zst"])
                            .save_file()
                        {
                            state.archive_path = path.display().to_string();
                        }
                    }
                });
                ui.end_row();

                ui.label("Target store");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.store_dir),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            state.store_dir = path.display().to_string();
                        }
                    }
                });
                ui.end_row();
            });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Transfer bundle");
        ui.horizontal_wrapped(|ui| {
            if components::action_button(ui, "Export bundle", true).clicked() {
                let config = ExportConfig {
                    workspace_path: PathBuf::from(workspace_path),
                    lockfile_path: PathBuf::from(lockfile_path),
                    store_dir: PathBuf::from(&state.store_dir),
                    output_path: PathBuf::from(&state.archive_path),
                    zstd_level,
                    mode: dmz_core::archive::exporter::ExportMode::Unified,
                };

                match AirGapExporter::export(&config) {
                    Ok(manifest) => {
                        *status_message = format!(
                            "Exported {} packages to bundle (SHA256: {})",
                            manifest.package_count,
                            &manifest.archive_sha256[..12]
                        );
                    }
                    Err(error) => {
                        *status_message = format!("Export Failed: {error}");
                    }
                }
            }

            if components::action_button(ui, "Import bundle", false).clicked() {
                let config = ImportConfig {
                    archive_path: PathBuf::from(&state.archive_path),
                    target_store_dir: PathBuf::from(&state.store_dir),
                    verify_closure_hash: true,
                    ..ImportConfig::default()
                };

                match AirGapImporter::import(&config) {
                    Ok(result) => {
                        *status_message = format!(
                            "Imported {} files. Verified Closure Sig: {}",
                            result.imported_packages,
                            &result.closure_signature[..12]
                        );
                    }
                    Err(error) => {
                        *status_message = format!("Import Failed: {error}");
                    }
                }
            }
        });
    });
}
