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
    pub(super) is_dual_repo: bool,
    pub(super) sidecar_path: String,
    pub(super) sidecar_id: String,
    pub(super) sidecar_role: String,
    pub(super) discovered_sidecar_stacks: Vec<String>,
    pub(super) abi_warnings: Vec<String>,
}

impl Default for ResolverState {
    fn default() -> Self {
        Self {
            target_platform: String::new(),
            store_dir: String::from(".dmz/store"),
            discovered_stacks: Vec::new(),
            is_dual_repo: false,
            sidecar_path: String::new(),
            sidecar_id: String::from("sidecar"),
            sidecar_role: String::from("sidecar"),
            discovered_sidecar_stacks: Vec::new(),
            abi_warnings: Vec::new(),
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

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Workspace Mode:").strong());
            if ui
                .selectable_label(!state.is_dual_repo, "Single Workspace")
                .clicked()
            {
                state.is_dual_repo = false;
            }
            if ui
                .selectable_label(state.is_dual_repo, "Multi-Repo / Companion")
                .clicked()
            {
                state.is_dual_repo = true;
            }
        });
        ui.add_space(8.0);

        egui::Grid::new("resolver_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label(if state.is_dual_repo {
                    "Workspace 1 (Anchor Root)"
                } else {
                    "Workspace Directory"
                });
                if components::path_browse_row(ui, workspace_path, None) {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        *workspace_path = path.display().to_string();
                    }
                }
                ui.end_row();

                if state.is_dual_repo {
                    ui.label("Workspace 2 (Companion)");
                    if components::path_browse_row(ui, &mut state.sidecar_path, None) {
                        let mut dialog = rfd::FileDialog::new();
                        let ws_path = PathBuf::from(&*workspace_path);
                        if ws_path.exists() {
                            dialog = dialog.set_directory(&ws_path);
                        }
                        if let Some(path) = dialog.pick_folder() {
                            state.sidecar_path = path.display().to_string();
                            if let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) {
                                state.sidecar_id = folder_name.to_string();
                            }
                        }
                    }
                    ui.end_row();

                    ui.label("Member ID");
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 32.0),
                        egui::TextEdit::singleline(&mut state.sidecar_id)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
                    );
                    ui.end_row();

                    ui.label("Member Role / Tag");
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 32.0),
                        egui::TextEdit::singleline(&mut state.sidecar_role)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0))
                            .hint_text("member, sidecar, or service"),
                    );
                    ui.end_row();
                }

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

        if state.is_dual_repo {
            ui.add_space(8.0);
            if components::action_button(ui, "Save as dmz-workspace.toml", false).clicked() {
                let ws_path = PathBuf::from(&*workspace_path);
                let sc_path = PathBuf::from(&state.sidecar_path);
                if !ws_path.exists() {
                    *status_message = String::from("Primary workspace directory does not exist.");
                } else if !sc_path.exists() {
                    *status_message = String::from("Sidecar workspace directory does not exist.");
                } else {
                    let primary_name = ws_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("primary");
                    let primary_m = dmz_core::workspace::WorkspaceMember::new(
                        primary_name,
                        &ws_path,
                        dmz_core::workspace::MemberRole::Primary,
                    );
                    let sc_role = if state.sidecar_role.trim().eq_ignore_ascii_case("service") {
                        dmz_core::workspace::MemberRole::Service
                    } else if state.sidecar_role.trim().eq_ignore_ascii_case("sidecar") {
                        dmz_core::workspace::MemberRole::Sidecar
                    } else {
                        dmz_core::workspace::MemberRole::Member
                    };
                    let sidecar_m = dmz_core::workspace::WorkspaceMember::new(
                        &state.sidecar_id,
                        &sc_path,
                        sc_role,
                    );
                    let proj_name = ws_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("project");
                    let proj_cfg = dmz_core::workspace::WorkspaceProjectConfig::new(
                        proj_name,
                        vec![primary_m, sidecar_m],
                    );
                    let manifest_file = ws_path.join("dmz-workspace.toml");
                    match proj_cfg.save_to_file(&manifest_file) {
                        Ok(()) => {
                            *status_message = format!(
                                "Generated multi-workspace manifest at {:?}",
                                manifest_file
                            );
                        }
                        Err(e) => {
                            *status_message = format!("Failed to save manifest: {}", e);
                        }
                    }
                }
            }
        }
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

                let out = if Path::new(&*lockfile_path).is_relative() {
                    ws_canon.join(&*lockfile_path)
                } else {
                    PathBuf::from(&*lockfile_path)
                };

                if state.is_dual_repo && !state.sidecar_path.trim().is_empty() {
                    let sc_path = PathBuf::from(&state.sidecar_path);
                    let sc_canon =
                        std::fs::canonicalize(&sc_path).unwrap_or_else(|_| sc_path.clone());

                    // Analyze ABI diagnostics
                    let abi = dmz_core::workspace::analyze_sidecar_abi(&sc_canon);
                    state.abi_warnings = abi.warnings;

                    match (
                        ResolverEngine::resolve_workspace_with_target(
                            &ws_canon,
                            target_opt.clone(),
                        ),
                        ResolverEngine::resolve_workspace_with_target(&sc_canon, target_opt),
                    ) {
                        (Ok(prim_lock), Ok(sc_lock)) => {
                            let mut merged = Vec::new();
                            for (_, mut pkg) in prim_lock.packages {
                                pkg.member_id = Some("primary".to_string());
                                merged.push(pkg);
                            }
                            for (_, mut pkg) in sc_lock.packages {
                                pkg.member_id = Some(state.sidecar_id.clone());
                                merged.push(pkg);
                            }
                            let mut unified =
                                dmz_core::resolver::lockfile::DmzLockfile::new(merged);
                            unified.add_workspace(
                                dmz_core::resolver::lockfile::WorkspaceMetadata {
                                    id: "primary".to_string(),
                                    path: ws_canon.display().to_string(),
                                    role: "primary".to_string(),
                                    ecosystem: None,
                                    closure_signature: None,
                                },
                            );
                            unified.add_workspace(
                                dmz_core::resolver::lockfile::WorkspaceMetadata {
                                    id: state.sidecar_id.clone(),
                                    path: sc_canon.display().to_string(),
                                    role: "sidecar".to_string(),
                                    ecosystem: None,
                                    closure_signature: None,
                                },
                            );

                            if let Err(e) = unified.save(&out) {
                                *status_message = format!("Error saving dual lockfile: {e}");
                            } else {
                                *status_message = format!(
                                    "Resolved dual workspaces: {} total packages. Sig: {}",
                                    unified.packages.len(),
                                    &unified.closure_signature
                                        [..12.min(unified.closure_signature.len())]
                                );
                            }
                        }
                        (Err(e), _) => {
                            *status_message = format!("Primary resolution failed: {e}");
                        }
                        (_, Err(e)) => {
                            *status_message = format!("Sidecar resolution failed: {e}");
                        }
                    }
                } else {
                    match ResolverEngine::resolve_workspace_with_target(&ws_canon, target_opt) {
                        Ok(lockfile) => {
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

                if state.is_dual_repo && !state.sidecar_path.trim().is_empty() {
                    let sc_path = PathBuf::from(&state.sidecar_path);
                    if let Ok(sc_stacks) = registry.process_workspace(&sc_path) {
                        state.discovered_sidecar_stacks = sc_stacks;
                    }
                    let abi = dmz_core::workspace::analyze_sidecar_abi(&sc_path);
                    state.abi_warnings = abi.warnings;
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
                ui.label(
                    egui::RichText::new(if state.is_dual_repo {
                        "Primary Detected:"
                    } else {
                        "Detected:"
                    })
                    .strong(),
                );
                for stack in &state.discovered_stacks {
                    ui.label(egui::RichText::new(stack).color(ui.visuals().selection.stroke.color));
                }
            });
        }

        if state.is_dual_repo && !state.discovered_sidecar_stacks.is_empty() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Sidecar Detected:").strong());
                for stack in &state.discovered_sidecar_stacks {
                    ui.label(
                        egui::RichText::new(stack).color(egui::Color32::from_rgb(0, 180, 216)),
                    );
                }
            });
        }

        if !state.abi_warnings.is_empty() {
            ui.add_space(8.0);
            for warn in &state.abi_warnings {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 170, 0), "⚠ ABI Advisory:");
                    ui.label(warn);
                });
            }
        }
    });
}
