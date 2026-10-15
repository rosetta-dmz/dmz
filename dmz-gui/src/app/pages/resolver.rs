use eframe::egui;
use std::path::PathBuf;

use super::super::components;
use dmz_core::resolver::ResolverEngine;

pub(super) fn show(
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    lockfile_path: &mut String,
    status_message: &mut String,
) {
    components::page_header(
        ui,
        "Dependency Resolver",
        "Scan manifests and generate a deterministic lockfile with a cryptographic closure signature.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Workspace");
        egui::Grid::new("resolver_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label("Workspace directory");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(workspace_path),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            *workspace_path = path.display().to_string();
                        }
                    }
                });
                ui.end_row();

                ui.label("Lockfile output");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(lockfile_path),
                );
                ui.end_row();
            });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Resolve dependencies");
        if components::action_button(ui, "Scan manifests & resolve closure", true).clicked() {
            let path = PathBuf::from(&*workspace_path);
            match ResolverEngine::resolve_workspace(&path) {
                Ok(lockfile) => {
                    let out = PathBuf::from(&*lockfile_path);
                    if let Err(error) = lockfile.save(&out) {
                        *status_message = format!("Error saving lockfile: {error}");
                    } else {
                        *status_message = format!(
                            "Resolved {} packages. Closure Sig: {}",
                            lockfile.packages.len(),
                            &lockfile.closure_signature[..12]
                        );
                    }
                }
                Err(error) => {
                    *status_message = format!("Resolution Failed: {error}");
                }
            }
        }
    });
}
