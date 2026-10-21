use eframe::egui;
use std::path::{Path, PathBuf};

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
    workspace_path: &mut String,
    lockfile_path: &mut String,
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
        components::section_header(ui, "Workspace & Artifact");
        egui::Grid::new("builder_artifact_grid")
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
                        egui::TextEdit::singleline(workspace_path)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            *workspace_path = path.display().to_string();
                        }
                    }
                });
                ui.end_row();

                ui.label("Artifact Name");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.artifact_name)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0)),
                );
                ui.end_row();

                ui.label("Output Directory");
                ui.horizontal(|ui| {
                    let browse_width = 88.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.output_dir)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
                    );
                    if ui.button("Browse...").clicked() {
                        let mut dialog = rfd::FileDialog::new();
                        let ws_path = PathBuf::from(&*workspace_path);
                        if ws_path.exists() {
                            dialog = dialog.set_directory(&ws_path);
                        }
                        if let Some(path) = dialog.pick_folder() {
                            state.output_dir = path.display().to_string();
                        }
                    }
                });
                ui.end_row();
            });

        let ws_canon = {
            let ws = PathBuf::from(&*workspace_path);
            std::fs::canonicalize(&ws).unwrap_or(ws)
        };
        let effective_out = if Path::new(&state.output_dir).is_relative() {
            ws_canon.join(&state.output_dir)
        } else {
            PathBuf::from(&state.output_dir)
        };
        let preview_dest = effective_out.join(format!("{}.tar.zst", state.artifact_name));
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "Effective artifact destination: {}",
                preview_dest.display()
            ))
            .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
            .size(12.0),
        );
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
            let workspace = PathBuf::from(&*workspace_path);
            let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

            let lockfile = if Path::new(&*lockfile_path).is_relative() {
                ws_canon.join(&*lockfile_path)
            } else {
                PathBuf::from(&*lockfile_path)
            };

            let output_dir = if Path::new(&state.output_dir).is_relative() {
                ws_canon.join(&state.output_dir)
            } else {
                PathBuf::from(&state.output_dir)
            };

            let config = BuildConfig {
                workspace_dir: ws_canon,
                lockfile_path: lockfile,
                output_dir,
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
