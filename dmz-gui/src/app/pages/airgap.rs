use eframe::egui;
use std::path::PathBuf;

use super::super::components;
use dmz_core::archive::exporter::ExportMode as CoreExportMode;
use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::ResolverEngine;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GuiExportMode {
    Unified,
    CodeOnly,
    DepsOnly,
}

impl From<GuiExportMode> for CoreExportMode {
    fn from(mode: GuiExportMode) -> Self {
        match mode {
            GuiExportMode::Unified => CoreExportMode::Unified,
            GuiExportMode::CodeOnly => CoreExportMode::CodeOnly,
            GuiExportMode::DepsOnly => CoreExportMode::DepsOnly,
        }
    }
}

pub(super) struct AirGapState {
    archive_path: String,
    store_dir: String,
    target_platform: String,
    export_mode: GuiExportMode,
    auto_install: bool,
    verify_closure: bool,
}

impl Default for AirGapState {
    fn default() -> Self {
        Self {
            archive_path: String::from("dist/workspace_closure.tar.zst"),
            store_dir: String::from(".dmz/store"),
            target_platform: String::new(),
            export_mode: GuiExportMode::Unified,
            auto_install: true,
            verify_closure: true,
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
        "Export and import verified dependency closures and offline bundles for isolated environments.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Bundle & Archive Paths");
        egui::Grid::new("airgap_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Bundle archive (.tar.zst)");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.archive_path)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
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

                ui.label("Dependency store directory");
                ui.horizontal(|ui| {
                    let browse_width = 88.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.store_dir)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            state.store_dir = path.display().to_string();
                        }
                    }
                });
                ui.end_row();

                ui.label("Target Platform");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.target_platform)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0))
                        .hint_text("Optional (e.g. linux-x86_64, macos-arm64, host default)"),
                );
                ui.end_row();
            });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Export Air-Gap Bundle");
        ui.horizontal_wrapped(|ui| {
            ui.label("Package contents:");
            ui.radio_value(
                &mut state.export_mode,
                GuiExportMode::Unified,
                "Unified (Code + Deps)",
            );
            ui.radio_value(&mut state.export_mode, GuiExportMode::CodeOnly, "Code Only");
            ui.radio_value(
                &mut state.export_mode,
                GuiExportMode::DepsOnly,
                "Dependencies Only",
            );
        });

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            if components::action_button(ui, "One-shot bundle (dmz bundle)", true).clicked() {
                let workspace = PathBuf::from(workspace_path);
                let lockfile = PathBuf::from(lockfile_path);
                let store = PathBuf::from(&state.store_dir);
                let output = PathBuf::from(&state.archive_path);
                let target_opt = if state.target_platform.trim().is_empty() {
                    None
                } else {
                    Some(state.target_platform.trim().to_string())
                };

                // 1. Resolve manifests if lockfile missing
                if !lockfile.exists() {
                    match ResolverEngine::resolve_workspace_with_target(
                        &workspace,
                        target_opt.clone(),
                    ) {
                        Ok(resolved) => {
                            let _ = resolved.save(&lockfile);
                        }
                        Err(e) => {
                            *status_message = format!("Bundle failed during resolution: {e}");
                            return;
                        }
                    }
                }

                // 2. Fetch dependencies
                if let Err(e) = ResolverEngine::fetch_dependencies(&lockfile, &store) {
                    *status_message = format!("Bundle failed during dependency fetch: {e}");
                    return;
                }

                // 3. Export
                let config = ExportConfig {
                    workspace_path: workspace,
                    lockfile_path: lockfile,
                    store_dir: store,
                    output_path: output,
                    zstd_level,
                    mode: state.export_mode.into(),
                    target_platform: target_opt,
                };

                match AirGapExporter::export(&config) {
                    Ok(manifest) => {
                        *status_message = format!(
                            "Bundle created successfully! Packages: {}, SHA256: {}",
                            manifest.package_count,
                            &manifest.archive_sha256[..12.min(manifest.archive_sha256.len())]
                        );
                    }
                    Err(error) => {
                        *status_message = format!("Bundle Export Failed: {error}");
                    }
                }
            }

            if components::action_button(ui, "Export archive (dmz airgap export)", false).clicked()
            {
                let target_opt = if state.target_platform.trim().is_empty() {
                    None
                } else {
                    Some(state.target_platform.trim().to_string())
                };

                let config = ExportConfig {
                    workspace_path: PathBuf::from(workspace_path),
                    lockfile_path: PathBuf::from(lockfile_path),
                    store_dir: PathBuf::from(&state.store_dir),
                    output_path: PathBuf::from(&state.archive_path),
                    zstd_level,
                    mode: state.export_mode.into(),
                    target_platform: target_opt,
                };

                match AirGapExporter::export(&config) {
                    Ok(manifest) => {
                        *status_message = format!(
                            "Exported {} packages to archive (SHA256: {})",
                            manifest.package_count,
                            &manifest.archive_sha256[..12.min(manifest.archive_sha256.len())]
                        );
                    }
                    Err(error) => {
                        *status_message = format!("Export Failed: {error}");
                    }
                }
            }
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Import & Restore Air-Gap Bundle (dmz import)");
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut state.auto_install,
                "Install offline dependencies into workspace layouts upon import",
            );
            ui.add_space(16.0);
            ui.checkbox(
                &mut state.verify_closure,
                "Cryptographically verify closure hash",
            );
        });

        ui.add_space(12.0);
        if components::action_button(ui, "Import & verify bundle", false).clicked() {
            let archive = PathBuf::from(&state.archive_path);
            let store = PathBuf::from(&state.store_dir);
            let workspace = PathBuf::from(workspace_path);

            let config = ImportConfig {
                archive_path: archive,
                target_store_dir: store.clone(),
                target_workspace_dir: workspace.clone(),
                verify_closure_hash: state.verify_closure,
            };

            match AirGapImporter::import(&config) {
                Ok(result) => {
                    let mut msg = format!(
                        "Imported {} files into store. Verified Closure: {}",
                        result.imported_packages,
                        &result.closure_signature[..12.min(result.closure_signature.len())]
                    );

                    if state.auto_install {
                        let lockfile = workspace.join("dmz.lock");
                        let install_config = InstallConfig {
                            workspace_path: workspace,
                            lockfile_path: lockfile,
                            store_dir: store,
                        };
                        match InstallerEngine::install(&install_config) {
                            Ok(count) => {
                                msg.push_str(&format!(" · Installed {} packages offline", count));
                            }
                            Err(e) => {
                                msg.push_str(&format!(" · Install error: {}", e));
                            }
                        }
                    }

                    *status_message = msg;
                }
                Err(error) => {
                    *status_message = format!("Import Failed: {error}");
                }
            }
        }
    });
}
