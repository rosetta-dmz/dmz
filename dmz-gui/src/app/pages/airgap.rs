use eframe::egui;
use std::path::PathBuf;

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
    ui.heading("Air-Gap Bundle Importer & Exporter");
    ui.label(
        "Manage offline, portable store closures for target environments without internet access.",
    );
    ui.add_space(10.0);

    egui::Grid::new("airgap_grid")
        .num_columns(2)
        .spacing([10.0, 10.0])
        .show(ui, |ui| {
            ui.label("Bundle Archive Path:");
            ui.horizontal(|ui| {
                let available = ui.available_width();
                ui.add(
                    egui::TextEdit::singleline(&mut state.archive_path)
                        .desired_width(available - 80.0),
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

            ui.label("Target Store Path:");
            ui.horizontal(|ui| {
                let available = ui.available_width();
                ui.add(
                    egui::TextEdit::singleline(&mut state.store_dir)
                        .desired_width(available - 80.0),
                );
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        state.store_dir = path.display().to_string();
                    }
                }
            });
            ui.end_row();
        });

    ui.add_space(15.0);

    ui.horizontal(|ui| {
        if ui.button("Export Air-Gap Bundle").clicked() {
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

        if ui.button("Import Air-Gap Bundle").clicked() {
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
}
