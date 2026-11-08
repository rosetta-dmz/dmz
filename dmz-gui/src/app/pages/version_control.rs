use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use super::super::components;
use dmz_core::archive::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::resolver::lockfile::DmzLockfile;
use dmz_core::vcs::{GitProvider, VcsProvenance};

pub(super) enum VcsTaskOutcome {
    StatusOnly(String),
    DiffPreview(Vec<String>, String),
}

pub(super) struct VersionControlState {
    pub cached_provenance: Option<VcsProvenance>,
    pub last_checked_path: String,
    pub bundle_output_path: String,
    pub since_ref: String,
    pub delta_output_path: String,
    pub clone_source: String,
    pub clone_destination: String,
    pub changed_files_preview: Vec<String>,
    pub active_task: Option<(String, Receiver<VcsTaskOutcome>)>,
}

impl Default for VersionControlState {
    fn default() -> Self {
        Self {
            cached_provenance: None,
            last_checked_path: String::new(),
            bundle_output_path: String::from("dist/workspace.bundle"),
            since_ref: String::from("HEAD~1"),
            delta_output_path: String::from("dist/workspace_delta.tar.zst"),
            clone_source: String::from("workspace.bundle"),
            clone_destination: String::from("workspace"),
            changed_files_preview: Vec::new(),
            active_task: None,
        }
    }
}

impl VersionControlState {
    pub(super) fn is_busy(&self) -> bool {
        self.active_task.is_some()
    }
}

pub(super) fn show(
    state: &mut VersionControlState,
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    lockfile_path: &mut String,
    status_message: &mut String,
) {
    if let Some((ref label, ref rx)) = state.active_task {
        match rx.try_recv() {
            Ok(outcome) => {
                match outcome {
                    VcsTaskOutcome::StatusOnly(msg) => {
                        *status_message = msg;
                    }
                    VcsTaskOutcome::DiffPreview(files, msg) => {
                        state.changed_files_preview = files;
                        *status_message = msg;
                    }
                }
                state.active_task = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx().request_repaint();
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                *status_message = format!("VCS operation '{label}' terminated unexpectedly.");
                state.active_task = None;
            }
        }
    }

    components::page_header(ui, "Version Control & Embedded Git");

    let ws_path = PathBuf::from(&*workspace_path);
    let ws_canon = std::fs::canonicalize(&ws_path).unwrap_or_else(|_| ws_path.clone());

    // Auto-refresh provenance if workspace path changed
    if state.last_checked_path != *workspace_path {
        state.cached_provenance = GitProvider::get_provenance(&ws_canon).ok().flatten();
        state.last_checked_path = workspace_path.clone();
    }

    // Workspace Git Provenance Banner
    components::section(ui, |ui| {
        components::section_header(ui, "Repository Provenance & Integrity");

        egui::Grid::new("vcs_provenance_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Workspace Path:");
                ui.monospace(ws_canon.display().to_string());
                ui.end_row();

                ui.label("Repository Status:");
                match &state.cached_provenance {
                    Some(prov) => {
                        ui.horizontal(|ui| {
                            let (badge_rect, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            let (color, text) = if prov.dirty && prov.has_untracked {
                                (
                                    egui::Color32::from_rgb(230, 160, 40),
                                    "Detected (Uncommitted Changes & Untracked Files)",
                                )
                            } else if prov.dirty {
                                (
                                    egui::Color32::from_rgb(230, 160, 40),
                                    "Detected (Uncommitted Changes)",
                                )
                            } else if prov.has_untracked {
                                (
                                    egui::Color32::from_rgb(100, 180, 240),
                                    "Detected (Working Tree Clean, Untracked Files Present)",
                                )
                            } else {
                                (
                                    egui::Color32::from_rgb(92, 224, 160),
                                    "Detected (Working Tree Clean)",
                                )
                            };
                            ui.painter().circle_filled(badge_rect.center(), 5.0, color);
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(text).color(color).strong());
                        });
                        ui.end_row();

                        ui.label("HEAD Commit:");
                        ui.monospace(&prov.commit);
                        ui.end_row();

                        ui.label("Active Branch:");
                        ui.monospace(prov.branch.as_deref().unwrap_or("(Detached HEAD)"));
                        ui.end_row();

                        if prov.dirty && !prov.uncommitted_files.is_empty() {
                            ui.label("Uncommitted Changes:");
                            ui.vertical(|ui| {
                                for f in prov.uncommitted_files.iter().take(5) {
                                    ui.monospace(format!("  • {}", f));
                                }
                                if prov.uncommitted_files.len() > 5 {
                                    ui.label(format!(
                                        "  ... and {} more",
                                        prov.uncommitted_files.len() - 5
                                    ));
                                }
                            });
                            ui.end_row();
                        }

                        if prov.has_untracked && !prov.untracked_files.is_empty() {
                            ui.label("Untracked Files:");
                            ui.vertical(|ui| {
                                for f in prov.untracked_files.iter().take(5) {
                                    ui.monospace(format!("  • {}", f));
                                }
                                if prov.untracked_files.len() > 5 {
                                    ui.label(format!(
                                        "  ... and {} more",
                                        prov.untracked_files.len() - 5
                                    ));
                                }
                            });
                            ui.end_row();
                        }
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("No active Git repository detected")
                                .color(egui::Color32::from_rgb(200, 80, 80)),
                        );
                        ui.end_row();
                    }
                }
            });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if components::action_button(ui, "Refresh Status", false).clicked() {
                state.cached_provenance = GitProvider::get_provenance(&ws_canon).ok().flatten();
                *status_message = format!(
                    "VCS status refreshed: {}",
                    if state.cached_provenance.is_some() {
                        "Git repository detected"
                    } else {
                        "Not a Git repository"
                    }
                );
            }

            if let Some(ref prov) = state.cached_provenance {
                if components::action_button(ui, "Sync Provenance to dmz.lock", true).clicked() {
                    let lock_path = if Path::new(&*lockfile_path).is_relative() {
                        ws_canon.join(&*lockfile_path)
                    } else {
                        PathBuf::from(&*lockfile_path)
                    };

                    if lock_path.exists() {
                        match DmzLockfile::load_and_verify(&lock_path) {
                            Ok(mut lock) => {
                                lock.vcs = Some(prov.clone());
                                if let Err(e) = lock.save(&lock_path) {
                                    *status_message = format!("Failed to save dmz.lock: {e}");
                                } else {
                                    *status_message = format!(
                                        "Updated dmz.lock with commit {} (branch: {:?})",
                                        &prov.commit[..8.min(prov.commit.len())],
                                        prov.branch
                                    );
                                }
                            }
                            Err(e) => {
                                *status_message = format!("Failed to load dmz.lock: {e}");
                            }
                        }
                    } else {
                        *status_message = String::from("dmz.lock does not exist; run scan first.");
                    }
                }
            }
        });
    });

    ui.add_space(16.0);

    // Deterministic Git Bundle Export
    components::section(ui, |ui| {
        components::section_header(ui, "Deterministic Git Bundle Export");

        egui::Grid::new("vcs_bundle_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Bundle Destination:");
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), components::FIELD_BUTTON_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let btn_width = components::BROWSE_BUTTON_WIDTH;
                        let spacing = ui.spacing().item_spacing.x;
                        let field_width = (ui.available_width() - btn_width - spacing).max(80.0);
                        ui.add_sized(
                            egui::vec2(field_width, components::FIELD_BUTTON_HEIGHT),
                            egui::TextEdit::singleline(&mut state.bundle_output_path)
                                .vertical_align(egui::Align::Center)
                                .margin(egui::Margin::symmetric(8, 0)),
                        );
                        if components::field_button(ui, "Save As...", Some(btn_width)).clicked() {
                            let mut dialog =
                                rfd::FileDialog::new().add_filter("Git Bundle", &["bundle"]);
                            if ws_canon.exists() {
                                dialog = dialog.set_directory(&ws_canon);
                            }
                            if let Some(path) = dialog.save_file() {
                                state.bundle_output_path = path.display().to_string();
                            }
                        }
                    },
                );
                ui.end_row();
            });

        ui.add_space(12.0);
        let is_busy = state.is_busy();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                if components::action_button(ui, "Export Git Bundle (.bundle)", true).clicked() {
                    let bundle_dest = if Path::new(&state.bundle_output_path).is_relative() {
                        ws_canon.join(&state.bundle_output_path)
                    } else {
                        PathBuf::from(&state.bundle_output_path)
                    };
                    let ws = ws_canon.clone();
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task = Some(("Exporting Git bundle...".to_string(), rx));

                    thread::spawn(move || {
                        let outcome = match GitProvider::create_bundle(&ws, &bundle_dest, None) {
                            Ok(info) => VcsTaskOutcome::StatusOnly(format!(
                                "Created Git bundle at {:?} ({} objects, commit {})",
                                bundle_dest,
                                info.object_count,
                                &info.head_commit[..8.min(info.head_commit.len())]
                            )),
                            Err(e) => {
                                VcsTaskOutcome::StatusOnly(format!("Bundle Export Failed: {e}"))
                            }
                        };
                        let _ = tx.send(outcome);
                        ctx.request_repaint();
                    });
                }
            });

            if let Some((ref label, _)) = state.active_task {
                ui.add_space(8.0);
                components::inline_spinner_status(ui, label);
            }
        });
    });

    ui.add_space(16.0);

    // Incremental Delta Transfer & Diff Preview
    components::section(ui, |ui| {
        components::section_header(ui, "Incremental Delta Packaging & Diff Preview");

        egui::Grid::new("vcs_delta_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Baseline (Commit / Manifest):");
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), components::FIELD_BUTTON_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let btn_width = components::BROWSE_BUTTON_WIDTH;
                        let spacing = ui.spacing().item_spacing.x;
                        let field_width = (ui.available_width() - btn_width - spacing).max(80.0);
                        ui.add_sized(
                            egui::vec2(field_width, components::FIELD_BUTTON_HEIGHT),
                            egui::TextEdit::singleline(&mut state.since_ref)
                                .hint_text("e.g. HEAD~1, v1.0.0, or previous_bundle.tar.zst")
                                .vertical_align(egui::Align::Center)
                                .margin(egui::Margin::symmetric(8, 0)),
                        );
                        if components::field_button(ui, "Browse...", Some(btn_width)).clicked() {
                            let mut dialog = rfd::FileDialog::new()
                                .add_filter("Bundle or Manifest", &["zst", "tar.zst", "json"]);
                            if ws_canon.exists() {
                                dialog = dialog.set_directory(&ws_canon);
                            }
                            if let Some(path) = dialog.pick_file() {
                                state.since_ref = path.display().to_string();
                            }
                        }
                    },
                );
                ui.end_row();

                ui.label("Delta Archive Output:");
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), components::FIELD_BUTTON_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let btn_width = components::BROWSE_BUTTON_WIDTH;
                        let spacing = ui.spacing().item_spacing.x;
                        let field_width = (ui.available_width() - btn_width - spacing).max(80.0);
                        ui.add_sized(
                            egui::vec2(field_width, components::FIELD_BUTTON_HEIGHT),
                            egui::TextEdit::singleline(&mut state.delta_output_path)
                                .vertical_align(egui::Align::Center)
                                .margin(egui::Margin::symmetric(8, 0)),
                        );
                        if components::field_button(ui, "Save As...", Some(btn_width)).clicked() {
                            let mut dialog = rfd::FileDialog::new()
                                .add_filter("Zstd Archive", &["zst", "tar.zst"]);
                            if ws_canon.exists() {
                                dialog = dialog.set_directory(&ws_canon);
                            }
                            if let Some(path) = dialog.save_file() {
                                state.delta_output_path = path.display().to_string();
                            }
                        }
                    },
                );
                ui.end_row();
            });

        ui.add_space(12.0);
        let is_busy = state.is_busy();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                if components::action_button(ui, "Preview Changed Files", false).clicked() {
                    let ws = ws_canon.clone();
                    let since_ref = state.since_ref.clone();
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task = Some(("Diffing repository changes...".to_string(), rx));

                    thread::spawn(move || {
                        let outcome = match GitProvider::get_changed_files_since(&ws, &since_ref) {
                            Ok(files) => {
                                let file_strs: Vec<String> =
                                    files.iter().map(|p| p.display().to_string()).collect();
                                let msg = format!(
                                    "Found {} modified/added files since {}",
                                    file_strs.len(),
                                    since_ref
                                );
                                VcsTaskOutcome::DiffPreview(file_strs, msg)
                            }
                            Err(e) => VcsTaskOutcome::StatusOnly(format!("Diff failed: {e}")),
                        };
                        let _ = tx.send(outcome);
                        ctx.request_repaint();
                    });
                }

                if components::action_button(ui, "Export Delta Bundle (.tar.zst)", true).clicked() {
                    let lockfile = if Path::new(&*lockfile_path).is_relative() {
                        ws_canon.join(&*lockfile_path)
                    } else {
                        PathBuf::from(&*lockfile_path)
                    };
                    let store = ws_canon.join(".dmz/store");
                    let output = if Path::new(&state.delta_output_path).is_relative() {
                        ws_canon.join(&state.delta_output_path)
                    } else {
                        PathBuf::from(&state.delta_output_path)
                    };

                    let ws = ws_canon.clone();
                    let since_ref = state.since_ref.clone();
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task = Some(("Exporting delta bundle...".to_string(), rx));

                    thread::spawn(move || {
                        let multi_workspace =
                            dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(
                                &ws,
                            )
                            .map(|(cfg, _)| cfg);

                        let config = ExportConfig {
                            workspace_path: ws,
                            lockfile_path: lockfile,
                            store_dir: store,
                            output_path: output.clone(),
                            zstd_level: 3,
                            mode: ExportMode::Unified,
                            target_platform: None,
                            with_git_history: true,
                            since: Some(since_ref),
                            with_toolchain: false,
                            multi_workspace,
                        };

                        let outcome = match AirGapExporter::export(&config) {
                            Ok(manifest) => VcsTaskOutcome::StatusOnly(format!(
                                "Delta bundle exported to {:?} (checksum: {})",
                                output,
                                &manifest.archive_sha256[..12.min(manifest.archive_sha256.len())]
                            )),
                            Err(e) => VcsTaskOutcome::StatusOnly(format!("Delta Export Failed: {e}")),
                        };
                        let _ = tx.send(outcome);
                        ctx.request_repaint();
                    });
                }
            });

            if let Some((ref label, _)) = state.active_task {
                ui.add_space(8.0);
                components::inline_spinner_status(ui, label);
            }
        });

        if !state.changed_files_preview.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!(
                    "Modified Files ({}):",
                    state.changed_files_preview.len()
                ))
                .strong(),
            );
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .show(ui, |ui| {
                    for f in &state.changed_files_preview {
                        ui.monospace(format!("  • {}", f));
                    }
                });
        }
    });

    ui.add_space(16.0);

    // Offline Git Clone & Restore
    components::section(ui, |ui| {
        components::section_header(ui, "Offline Git Restore & Clone");

        egui::Grid::new("vcs_restore_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Source Bundle:");
                if components::path_browse_row(ui, &mut state.clone_source, None) {
                    let mut dialog = rfd::FileDialog::new().add_filter("Git Bundle", &["bundle"]);
                    if ws_canon.exists() {
                        dialog = dialog.set_directory(&ws_canon);
                    }
                    if let Some(path) = dialog.pick_file() {
                        state.clone_source = path.display().to_string();
                    }
                }
                ui.end_row();

                ui.label("Destination Workspace:");
                if components::path_browse_row(ui, &mut state.clone_destination, None) {
                    let mut dialog = rfd::FileDialog::new();
                    if ws_canon.exists() {
                        dialog = dialog.set_directory(&ws_canon);
                    }
                    if let Some(path) = dialog.pick_folder() {
                        state.clone_destination = path.display().to_string();
                    }
                }
                ui.end_row();
            });

        ui.add_space(12.0);
        let is_busy = state.is_busy();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                if components::action_button(ui, "Restore / Clone from Bundle", true).clicked() {
                    let bundle_src = if Path::new(&state.clone_source).is_relative() {
                        ws_canon.join(&state.clone_source)
                    } else {
                        PathBuf::from(&state.clone_source)
                    };
                    let target_ws = if Path::new(&state.clone_destination).is_relative() {
                        ws_canon.join(&state.clone_destination)
                    } else {
                        PathBuf::from(&state.clone_destination)
                    };
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task =
                        Some(("Restoring repository from bundle...".to_string(), rx));

                    thread::spawn(move || {
                        let outcome = match GitProvider::restore_bundle(&target_ws, &bundle_src) {
                            Ok(()) => VcsTaskOutcome::StatusOnly(format!(
                                "Successfully restored Git repository into {:?}",
                                target_ws
                            )),
                            Err(e) => VcsTaskOutcome::StatusOnly(format!("Restore Failed: {e}")),
                        };
                        let _ = tx.send(outcome);
                        ctx.request_repaint();
                    });
                }
            });

            if let Some((ref label, _)) = state.active_task {
                ui.add_space(8.0);
                components::inline_spinner_status(ui, label);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_control_page_renders() {
        let ctx = egui::Context::default();
        let mut state = VersionControlState::default();
        let mut ws = ".".to_string();
        let mut lock = "dmz.lock".to_string();
        let mut msg = String::new();

        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                show(&mut state, ui, &mut ws, &mut lock, &mut msg);
            });
        }

        // When all changes are committed, cached provenance must detect working tree clean (dirty == false)
        if let Some(ref prov) = state.cached_provenance {
            assert!(
                !prov.dirty,
                "Expected working tree clean (dirty == false) when all changes are committed"
            );
        }
    }
}
