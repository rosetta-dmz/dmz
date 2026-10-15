use eframe::egui;
use std::path::PathBuf;

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
    ui.heading("Deterministic Package Builder");
    ui.label("Assemble compressed .tar.zst packages targeting specific runtime layers.");
    ui.add_space(10.0);

    ui.horizontal(|ui| {
        ui.label("Target Mode:");
        ui.radio_value(
            &mut state.selected_target,
            Target::DependenciesOnly,
            "Target: Dependencies Only",
        );
        ui.radio_value(
            &mut state.selected_target,
            Target::AppOnly,
            "Target: Code Only",
        );
        ui.radio_value(
            &mut state.selected_target,
            Target::Unified,
            "Target: Unified",
        );
    });

    ui.add_space(10.0);

    egui::Grid::new("builder_grid")
        .num_columns(3)
        .spacing([10.0, 10.0])
        .show(ui, |ui| {
            ui.label("Artifact Base Name:");
            ui.text_edit_singleline(&mut state.artifact_name);
            ui.label("");
            ui.end_row();

            ui.label("Output Directory:");
            ui.text_edit_singleline(&mut state.output_dir);
            if ui.button("Browse...").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    state.output_dir = path.display().to_string();
                }
            }
            ui.end_row();

            ui.label("Zstd Compression Level:");
            ui.add(egui::Slider::new(&mut state.zstd_level, 1..=22));
            ui.label("");
            ui.end_row();
        });

    ui.add_space(5.0);
    ui.group(|ui| {
        ui.label("What is Zstd Compression Level?");
        ui.colored_label(
            egui::Color32::GRAY,
            "Zstd (Zstandard) is a real-time compression algorithm. Level 3 is default. Higher levels optimize compression ratio for physical air-gapped network transfers.",
        );
    });

    ui.add_space(15.0);

    if ui.button("Build Artifact Package").clicked() {
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
}
