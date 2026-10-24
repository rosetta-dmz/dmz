use eframe::egui;
use std::path::{Path, PathBuf};

use super::super::components;
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::ResolverEngine;

pub(super) struct ResolverState {
    pub(super) target_platform: String,
    pub(super) store_dir: String,
    pub(super) discovered_stacks: Vec<String>,
}

impl Default for ResolverState {
    fn default() -> Self {
        Self {
            target_platform: String::new(),
            store_dir: String::from(".dmz/store"),
            discovered_stacks: Vec::new(),
        }
    }
}

pub(super) fn show(
    state: &mut ResolverState,
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    lockfile_path: &mut String,
    status_message: &mut String,
) {
    components::page_header(ui, "Dependency Resolver");

    components::section(ui, |ui| {
        components::section_header(ui, "Workspace & Targets");
        egui::Grid::new("resolver_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Workspace Directory");
                if components::path_browse_row(ui, workspace_path, None) {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        *workspace_path = path.display().to_string();
                    }
                }
                ui.end_row();

                ui.label("Lockfile Path");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(lockfile_path)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0)),
                );
                ui.end_row();

                ui.label("Store Cache Directory");
                if components::path_browse_row(ui, &mut state.store_dir, None) {
                    let mut dialog = rfd::FileDialog::new();
                    let ws_path = PathBuf::from(&*workspace_path);
                    if ws_path.exists() {
                        dialog = dialog.set_directory(&ws_path);
                    }
                    if let Some(path) = dialog.pick_folder() {
                        state.store_dir = path.display().to_string();
                    }
                }
                ui.end_row();

                ui.label("Target Platform");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.target_platform)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0))
                        .hint_text("Optional (e.g. macos-arm64, linux-x86_64, host default)"),
                );
                ui.end_row();
            });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Dependency Lifecycle Actions");
        ui.horizontal_wrapped(|ui| {
            if components::action_button(ui, "Scan & Resolve", true).clicked() {
                let workspace = PathBuf::from(&*workspace_path);
                let ws_canon =
                    std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
                let target_opt = if state.target_platform.trim().is_empty() {
                    None
                } else {
                    Some(state.target_platform.trim().to_string())
                };

                match ResolverEngine::resolve_workspace_with_target(&ws_canon, target_opt) {
                    Ok(lockfile) => {
                        let out = if Path::new(&*lockfile_path).is_relative() {
                            ws_canon.join(&*lockfile_path)
                        } else {
                            PathBuf::from(&*lockfile_path)
                        };
                        if let Err(error) = lockfile.save(&out) {
                            *status_message = format!("Error saving lockfile: {error}");
                        } else {
                            *status_message = format!(
                                "Resolved {} packages. Closure Sig: {}",
                                lockfile.packages.len(),
                                &lockfile.closure_signature
                                    [..12.min(lockfile.closure_signature.len())]
                            );
                        }
                    }
                    Err(error) => {
                        *status_message = format!("Resolution Failed: {error}");
                    }
                }
            }

            if components::action_button(ui, "Fetch into Store", false).clicked() {
                let workspace = PathBuf::from(&*workspace_path);
                let ws_canon =
                    std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
                let lock_path = if Path::new(&*lockfile_path).is_relative() {
                    ws_canon.join(&*lockfile_path)
                } else {
                    PathBuf::from(&*lockfile_path)
                };
                let store = if Path::new(&state.store_dir).is_relative() {
                    ws_canon.join(&state.store_dir)
                } else {
                    PathBuf::from(&state.store_dir)
                };
                match ResolverEngine::fetch_dependencies(&lock_path, &store) {
                    Ok(count) => {
                        *status_message = format!(
                            "Fetched and authenticated {} packages into store {:?}",
                            count, store
                        );
                    }
                    Err(e) => {
                        *status_message = format!("Fetch failed: {}", e);
                    }
                }
            }

            if components::action_button(ui, "Install Offline", false).clicked() {
                let workspace = PathBuf::from(&*workspace_path);
                let ws_canon =
                    std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
                let lock_path = if Path::new(&*lockfile_path).is_relative() {
                    ws_canon.join(&*lockfile_path)
                } else {
                    PathBuf::from(&*lockfile_path)
                };
                let store = if Path::new(&state.store_dir).is_relative() {
                    ws_canon.join(&state.store_dir)
                } else {
                    PathBuf::from(&state.store_dir)
                };

                let config = InstallConfig {
                    workspace_path: ws_canon,
                    lockfile_path: lock_path,
                    store_dir: store,
                };
                match InstallerEngine::install(&config) {
                    Ok(count) => {
                        *status_message = format!(
                            "Successfully installed {} packages offline into project layouts",
                            count
                        );
                    }
                    Err(e) => {
                        *status_message = format!("Offline installation failed: {}", e);
                    }
                }
            }
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Ecosystem Discovery & Verification");
        ui.horizontal_wrapped(|ui| {
            if components::action_button(ui, "Discover Active Stacks", false).clicked() {
                let workspace = PathBuf::from(&*workspace_path);
                let registry = EcosystemRegistry::new();
                match registry.process_workspace(&workspace) {
                    Ok(stacks) => {
                        state.discovered_stacks = stacks.clone();
                        *status_message = format!("Discovered active ecosystems: {:?}", stacks);
                    }
                    Err(e) => {
                        *status_message = format!("Ecosystem discovery failed: {}", e);
                    }
                }
            }

            if components::action_button(ui, "Verify Active Toolchains", false).clicked() {
                let workspace = PathBuf::from(&*workspace_path);
                let registry = EcosystemRegistry::new();
                let active = registry.discover_active(&workspace);
                let mut passed = 0;
                let mut failed = 0;
                for eco in active {
                    if let Some(cmd) = eco.verification_command(&workspace) {
                        match std::process::Command::new(&cmd.program)
                            .args(&cmd.args)
                            .current_dir(&workspace)
                            .status()
                        {
                            Ok(status) if status.success() => {
                                passed += 1;
                            }
                            _ => {
                                failed += 1;
                            }
                        }
                    }
                }
                *status_message = format!(
                    "Ecosystem toolchain verification: {} passed, {} failed",
                    passed, failed
                );
            }
        });

        if !state.discovered_stacks.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Detected:").strong());
                for stack in &state.discovered_stacks {
                    ui.label(egui::RichText::new(stack).color(ui.visuals().selection.stroke.color));
                }
            });
        }
    });
}
