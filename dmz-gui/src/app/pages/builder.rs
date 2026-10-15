use eframe::egui;
use std::path::PathBuf;

use super::super::components;
use dmz_core::packaging::{BuildConfig, BuildTarget, PackageBuilder};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    DependenciesOnly,
    AppOnly,
    Unified,
}

impl From<Target> for BuildTarget {
    fn from(target: Target) -> Self {
        match target {
            Target::DependenciesOnly => Self::DependenciesOnly,
            Target::AppOnly => Self::AppOnly,
            Target::Unified => Self::Unified,
        }
    }
}

#[derive(Clone)]
pub(super) struct BuilderState {
    selected_target: Target,
    zstd_level: i32,
    output_dir: String,
    artifact_name: String,
}

impl Default for BuilderState {
    fn default() -> Self {
        Self {
            selected_target: Target::Unified,
            zstd_level: 3,
            output_dir: String::from("dist"),
            artifact_name: String::from("workspace_closure"),
        }
    }
}

impl BuilderState {
    pub(super) fn zstd_level(&self) -> i32 {
        self.zstd_level
    }
}

pub(super) fn show(
    state: &mut BuilderState,
    ui: &mut egui::Ui,
    workspace_path: &str,
    lockfile_path: &str,
    status_message: &mut String,
) {
    components::page_header(
        ui,
        "Package Builder",
        "Assemble compressed .tar.zst artifacts for selected runtime layers.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Package contents");
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(
                &mut state.selected_target,
                Target::DependenciesOnly,
                "Dependencies",
            );
            ui.radio_value(&mut state.selected_target, Target::AppOnly, "Application");
            ui.radio_value(&mut state.selected_target, Target::Unified, "Unified");
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Artifact");
        egui::Grid::new("builder_artifact_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label("Artifact name");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.artifact_name),
                );
                ui.end_row();

                ui.label("Output directory");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.output_dir),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            state.output_dir = path.display().to_string();
                        }
                    }
                });
                ui.end_row();
            });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Compression & build");
        ui.horizontal(|ui| {
            ui.label("Zstandard level");
            ui.add(egui::Slider::new(&mut state.zstd_level, 1..=22).show_value(true));
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Level 3 is the default. Higher levels can reduce transfer size at the cost of build time.",
            )
            .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
            .size(12.0),
        );
        ui.add_space(16.0);
        if components::action_button(ui, "Build artifact package", true).clicked() {
            let config = BuildConfig {
                workspace_dir: PathBuf::from(workspace_path),
                lockfile_path: PathBuf::from(lockfile_path),
                output_dir: PathBuf::from(&state.output_dir),
                target: state.selected_target.into(),
                archive_name: state.artifact_name.clone(),
                zstd_level: state.zstd_level,
            };

            match PackageBuilder::build(&config) {
                Ok(result) => {
                    *status_message = format!(
                        "Build Complete: {:?} (SHA256: {})",
                        result.artifact_path.file_name().unwrap_or_default(),
                        &result.sha256[..12]
                    );
                }
                Err(error) => {
                    *status_message = format!("Build Error: {error}");
                }
            }
        }
    });
}
