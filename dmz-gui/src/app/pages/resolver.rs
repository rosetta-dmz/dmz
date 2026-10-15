use eframe::egui;
use std::path::PathBuf;

use dmz_core::resolver::ResolverEngine;

pub(super) fn show(
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    lockfile_path: &mut String,
    status_message: &mut String,
) {
    ui.heading("Dependency Resolver & Lockfile Generator");
    ui.label("Scan project manifests (Cargo.toml, package.json, requirements.txt) and generate cryptographic closure signatures.");
    ui.add_space(10.0);

    egui::Grid::new("resolver_grid")
        .num_columns(2)
        .spacing([10.0, 10.0])
        .show(ui, |ui| {
            ui.label("Workspace Directory:");
            ui.horizontal(|ui| {
                let available = ui.available_width();
                ui.add(egui::TextEdit::singleline(workspace_path).desired_width(available - 80.0));
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        *workspace_path = path.display().to_string();
                    }
                }
            });
            ui.end_row();

            ui.label("Lockfile Output:");
            ui.add(egui::TextEdit::singleline(lockfile_path).desired_width(f32::INFINITY));
            ui.end_row();
        });

    ui.add_space(15.0);

    if ui.button("Scan Manifests & Resolve Closure").clicked() {
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
}
