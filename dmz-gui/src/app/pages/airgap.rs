use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use super::super::components;
use dmz_core::archive::exporter::ExportMode as CoreExportMode;
use dmz_core::archive::{
    AirGapExporter, AirGapImporter, AirGapManifest, ExportConfig, ImportConfig,
};
use dmz_core::resolver::{ResolveOptions, ResolverEngine};
use dmz_core::toolchain::ToolchainProfile;

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

pub(super) enum AirGapTaskOutcome {
    Success(String),
    Failure(String),
    ImportSuccess {
        status_message: String,
        workspace_path: String,
    },
    ImportFailure(String),
}

pub(super) struct AirGapState {
    archive_path: String,
    store_dir: String,
    target_platform: String,
    export_mode: GuiExportMode,
    zstd_level: i32,
    auto_install: bool,
    verify_closure: bool,
    with_toolchain: bool,
    toolchain_profile: ToolchainProfile,
    include_dev: bool,
    import_include_dev: bool,
    show_export_tip: bool,
    pub(super) active_task: Option<(String, Receiver<AirGapTaskOutcome>)>,
}

impl Default for AirGapState {
    fn default() -> Self {
        Self {
            archive_path: String::from("dist/workspace_closure.tar.zst"),
            store_dir: String::from(".dmz/store"),
            target_platform: String::new(),
            export_mode: GuiExportMode::Unified,
            zstd_level: 3,
            auto_install: true,
            verify_closure: true,
            with_toolchain: true,
            toolchain_profile: ToolchainProfile::Minimal,
            include_dev: true,
            import_include_dev: true,
            show_export_tip: true,
            active_task: None,
        }
    }
}

impl AirGapState {
    pub(super) fn is_busy(&self) -> bool {
        self.active_task.is_some()
    }
}

pub(super) fn show(
    state: &mut AirGapState,
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    lockfile_path: &mut String,
    status_message: &mut String,
) {
    if let Some((ref label, ref rx)) = state.active_task {
        match rx.try_recv() {
            Ok(outcome) => {
                match outcome {
                    AirGapTaskOutcome::Success(msg) => {
                        *status_message = msg;
                    }
                    AirGapTaskOutcome::Failure(err) => {
                        *status_message = err;
                    }
                    AirGapTaskOutcome::ImportSuccess {
                        status_message: msg,
                        workspace_path: new_ws,
                    } => {
                        *status_message = msg;
                        *workspace_path = new_ws;
                    }
                    AirGapTaskOutcome::ImportFailure(err) => {
                        *status_message = err;
                    }
                }
                state.active_task = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx().request_repaint();
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                *status_message = format!("Air-gap operation '{label}' terminated unexpectedly.");
                state.active_task = None;
            }
        }
    }

    components::page_header(ui, "Air-Gap Manager");

    components::section(ui, |ui| {
        components::section_header(ui, "Bundle & Archive Paths");
        egui::Grid::new("airgap_grid")
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

                ui.label("Bundle Archive");
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), components::FIELD_BUTTON_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let btn_width = components::BROWSE_BUTTON_WIDTH;
                        let spacing = ui.spacing().item_spacing.x;
                        let field_width =
                            (ui.available_width() - (btn_width * 2.0) - (spacing * 2.0)).max(80.0);
                        ui.add_sized(
                            egui::vec2(field_width, components::FIELD_BUTTON_HEIGHT),
                            egui::TextEdit::singleline(&mut state.archive_path)
                                .align(egui::Align2::LEFT_CENTER)
                                .margin(egui::Margin::symmetric(8, 0)),
                        );
                        if components::field_button(ui, "Open...", Some(btn_width))
                            .on_hover_text("Select an existing .tar.zst bundle to import")
                            .clicked()
                        {
                            let mut dialog = rfd::FileDialog::new()
                                .add_filter("Zstd Archive", &["zst", "tar.zst"]);
                            let ws_path = PathBuf::from(&*workspace_path);
                            if ws_path.exists() {
                                dialog = dialog.set_directory(&ws_path);
                            }
                            if let Some(path) = dialog.pick_file() {
                                state.archive_path = path.display().to_string();
                            }
                        }
                        if components::field_button(ui, "Save As...", Some(btn_width))
                            .on_hover_text("Choose output file destination for export")
                            .clicked()
                        {
                            let mut dialog = rfd::FileDialog::new()
                                .add_filter("Zstd Archive", &["zst", "tar.zst"]);
                            let ws_path = PathBuf::from(&*workspace_path);
                            if ws_path.exists() {
                                dialog = dialog.set_directory(&ws_path);
                            }
                            if let Some(path) = dialog.save_file() {
                                state.archive_path = path.display().to_string();
                            }
                        }
                    },
                );
                ui.end_row();

                ui.label("Dependency Store Directory");
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
                        .align(egui::Align2::LEFT_CENTER)
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
            ui.label("Package Contents:");
            ui.radio_value(
                &mut state.export_mode,
                GuiExportMode::Unified,
                "Complete Bundle",
            );
            ui.radio_value(
                &mut state.export_mode,
                GuiExportMode::CodeOnly,
                "Codebase Only",
            );
            ui.radio_value(
                &mut state.export_mode,
                GuiExportMode::DepsOnly,
                "Dependencies Only",
            );
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Zstandard level:");
            ui.add(egui::Slider::new(&mut state.zstd_level, 1..=22).show_value(true));
        });

        ui.add_space(8.0);
        ui.checkbox(
            &mut state.with_toolchain,
            "Package compiler toolchains into bundle",
        );

        ui.add_space(4.0);
        ui.checkbox(&mut state.include_dev, "Include dev dependencies");

        ui.add_space(4.0);
        ui.add_enabled_ui(state.with_toolchain, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Toolchain Profile:");
                ui.radio_value(
                    &mut state.toolchain_profile,
                    ToolchainProfile::Minimal,
                    "Minimal",
                );
                ui.radio_value(&mut state.toolchain_profile, ToolchainProfile::Full, "Full");
            });
        });

        let is_busy = state.is_busy();
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                if components::action_button(ui, "Bundle (Fetch)", true)
                    .on_hover_text(
                        "Resolve dependencies, fetch remote packages into .dmz/store, and export archive (requires network)",
                    )
                    .clicked()
                {
                    let workspace = PathBuf::from(&*workspace_path);
                    let ws_canon =
                        std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

                    let lockfile = if Path::new(&*lockfile_path).is_relative() {
                        ws_canon.join(&*lockfile_path)
                    } else {
                        PathBuf::from(&*lockfile_path)
                    };
                    let store = if Path::new(&state.store_dir).is_relative() {
                        ws_canon.join(&state.store_dir)
                    } else {
                        PathBuf::from(&state.store_dir)
                    };
                    let output = if Path::new(&state.archive_path).is_relative() {
                        ws_canon.join(&state.archive_path)
                    } else {
                        PathBuf::from(&state.archive_path)
                    };
                    let target_opt = if state.target_platform.trim().is_empty() {
                        None
                    } else {
                        Some(state.target_platform.trim().to_string())
                    };

                    let zstd_level = state.zstd_level;
                    let export_mode = state.export_mode;
                    let with_toolchain = state.with_toolchain;
                    let toolchain_profile = state.toolchain_profile;
                    let include_dev = state.include_dev;
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task = Some((
                        "Bundling air-gap closure (fetching dependencies)...".to_string(),
                        rx,
                    ));

                    thread::spawn(move || {
                        let multi_workspace =
                            dmz_core::workspace::WorkspaceProjectConfig::discover_or_from_lockfile_scoped(
                                &ws_canon, &lockfile,
                            );

                        if dmz_core::resolver::lockfile::DmzLockfile::file_needs_refresh(
                            &lockfile,
                            target_opt.as_deref(),
                            include_dev,
                            multi_workspace.as_ref(),
                        ) {
                            let resolve_options = ResolveOptions {
                                target_platform: target_opt.clone(),
                                include_dev,
                            };
                            let resolve_res = if let Some(ref mws) = multi_workspace {
                                ResolverEngine::resolve_multi_workspace_with_options(
                                    &ws_canon,
                                    mws,
                                    None,
                                    &resolve_options,
                                )
                            } else {
                                ResolverEngine::resolve_workspace_with_options(
                                    &ws_canon,
                                    resolve_options,
                                )
                            };

                            match resolve_res {
                                Ok(resolved) => {
                                    let _ = resolved.save(&lockfile);
                                }
                                Err(e) => {
                                    let _ = tx.send(AirGapTaskOutcome::Failure(format!(
                                        "Bundle failed during resolution: {e}"
                                    )));
                                    ctx.request_repaint();
                                    return;
                                }
                            }
                        }

                        if let Err(e) = ResolverEngine::fetch_dependencies(&lockfile, &store) {
                            let _ = tx.send(AirGapTaskOutcome::Failure(format!(
                                "Bundle failed during dependency fetch: {e}"
                            )));
                            ctx.request_repaint();
                            return;
                        }

                        let config = ExportConfig {
                            workspace_path: ws_canon.clone(),
                            lockfile_path: lockfile.clone(),
                            store_dir: store,
                            output_path: output,
                            zstd_level,
                            mode: export_mode.into(),
                            target_platform: target_opt,
                            with_toolchain,
                            include_dev,
                            toolchain_profile,
                            multi_workspace,
                            ..Default::default()
                        };

                        let outcome = match AirGapExporter::export(&config) {
                            Ok(manifest) => AirGapTaskOutcome::Success(format_export_success_summary(
                                "Bundle created successfully.",
                                &ws_canon,
                                &lockfile,
                                &manifest,
                            )),
                            Err(error) => {
                                AirGapTaskOutcome::Failure(format!("Bundle Export Failed: {error}"))
                            }
                        };
                        let _ = tx.send(outcome);
                        ctx.request_repaint();
                    });
                }

                if components::action_button(ui, "Export (Local)", false)
                    .on_hover_text(
                        "Export directly from existing local .dmz/store with zero network access (fully offline)",
                    )
                    .clicked()
                {
                    let workspace = PathBuf::from(&*workspace_path);
                    let ws_canon =
                        std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

                    let lockfile = if Path::new(&*lockfile_path).is_relative() {
                        ws_canon.join(&*lockfile_path)
                    } else {
                        PathBuf::from(&*lockfile_path)
                    };
                    let store = if Path::new(&state.store_dir).is_relative() {
                        ws_canon.join(&state.store_dir)
                    } else {
                        PathBuf::from(&state.store_dir)
                    };
                    let output = if Path::new(&state.archive_path).is_relative() {
                        ws_canon.join(&state.archive_path)
                    } else {
                        PathBuf::from(&state.archive_path)
                    };
                    let target_opt = if state.target_platform.trim().is_empty() {
                        None
                    } else {
                        Some(state.target_platform.trim().to_string())
                    };

                    let zstd_level = state.zstd_level;
                    let export_mode = state.export_mode;
                    let with_toolchain = state.with_toolchain;
                    let toolchain_profile = state.toolchain_profile;
                    let include_dev = state.include_dev;
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task = Some((
                        "Exporting air-gap archive from local store...".to_string(),
                        rx,
                    ));

                    thread::spawn(move || {
                        let multi_workspace =
                            dmz_core::workspace::WorkspaceProjectConfig::discover_or_from_lockfile_scoped(
                                &ws_canon, &lockfile,
                            );

                        let config = ExportConfig {
                            workspace_path: ws_canon.clone(),
                            lockfile_path: lockfile.clone(),
                            store_dir: store,
                            output_path: output,
                            zstd_level,
                            mode: export_mode.into(),
                            target_platform: target_opt,
                            with_toolchain,
                            include_dev,
                            toolchain_profile,
                            multi_workspace,
                            ..Default::default()
                        };

                        let outcome = match AirGapExporter::export(&config) {
                            Ok(manifest) => AirGapTaskOutcome::Success(format_export_success_summary(
                                "Export completed.",
                                &ws_canon,
                                &lockfile,
                                &manifest,
                            )),
                            Err(error) => {
                                AirGapTaskOutcome::Failure(format!("Export Failed: {error}"))
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

    if state.show_export_tip {
        ui.add_space(16.0);
        components::dismissable_toast(
            ui,
            "Level 3 is the default. Higher levels can reduce transfer size at the cost of build time. Select Bundle (Fetch) to automatically resolve and fetch dependencies before export, or Export (Local) to package an already-cached local store directly with zero network access.",
            &mut state.show_export_tip,
        );
    }

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Import Air-Gap Archive");
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), components::FIELD_BUTTON_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label("Archive to Import:");
                let browse_width = 120.0;
                let spacing = ui.spacing().item_spacing.x;
                let field_width = (ui.available_width() - browse_width - spacing).max(80.0);
                ui.add_sized(
                    egui::vec2(field_width, components::FIELD_BUTTON_HEIGHT),
                    egui::TextEdit::singleline(&mut state.archive_path)
                        .align(egui::Align2::LEFT_CENTER)
                        .margin(egui::Margin::symmetric(8, 0)),
                );

                if components::field_button(ui, "Open Archive...", Some(browse_width))
                    .on_hover_text("Select an existing .tar.zst archive to import")
                    .clicked()
                {
                    let mut dialog =
                        rfd::FileDialog::new().add_filter("Zstd Archive", &["zst", "tar.zst"]);
                    let ws_path = PathBuf::from(&*workspace_path);
                    if ws_path.exists() {
                        dialog = dialog.set_directory(&ws_path);
                    }
                    if let Some(path) = dialog.pick_file() {
                        state.archive_path = path.display().to_string();
                    }
                }
            },
        );
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut state.auto_install,
                "Install offline dependencies into workspace",
            );
            ui.add_space(16.0);
            ui.checkbox(
                &mut state.verify_closure,
                "Cryptographically verify closure hash",
            );
            ui.add_space(16.0);
            ui.checkbox(&mut state.import_include_dev, "Include dev dependencies");
        });

        ui.add_space(12.0);
        let is_busy = state.is_busy();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                if components::action_button(ui, "Import & Verify Archive", false).clicked() {
                    let workspace = PathBuf::from(&*workspace_path);
                    let _ = std::fs::create_dir_all(&workspace);
                    let ws_canon =
                        std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

                    let archive_raw = Path::new(&state.archive_path);
                    let archive = if archive_raw.is_relative() {
                        if ws_canon.join(archive_raw).exists() {
                            ws_canon.join(archive_raw)
                        } else if archive_raw.exists() {
                            std::fs::canonicalize(archive_raw)
                                .unwrap_or_else(|_| archive_raw.to_path_buf())
                        } else {
                            ws_canon.join(archive_raw)
                        }
                    } else {
                        archive_raw.to_path_buf()
                    };

                    if !archive.exists() {
                        *status_message = format!(
                            "Archive file not found at {:?}. Click 'Open Archive...' to select a valid .tar.zst package.",
                            archive
                        );
                        return;
                    }

                    let store = if Path::new(&state.store_dir).is_relative() {
                        ws_canon.join(&state.store_dir)
                    } else {
                        PathBuf::from(&state.store_dir)
                    };

                    let verify_closure = state.verify_closure;
                    let auto_install = state.auto_install;
                    let include_dev = state.import_include_dev;
                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task =
                        Some(("Importing and verifying archive...".to_string(), rx));

                    thread::spawn(move || {
                        let config = ImportConfig {
                            archive_path: archive,
                            target_store_dir: store.clone(),
                            target_workspace_dir: ws_canon.clone(),
                            verify_closure_hash: verify_closure,
                            include_dev,
                            install_dependencies: auto_install,
                        };

                        let outcome = match AirGapImporter::import(&config) {
                            Ok(result) => {
                                let short_sig: String =
                                    result.closure_signature.chars().take(12).collect();
                                let mut msg = format!(
                                    "Imported {} files into store. Verified Closure: {}",
                                    result.imported_packages, short_sig
                                );
                                if let Some(ref proj) = result.workspace_project {
                                    msg.push_str(&format!(
                                        " · Dual-Workspace '{}' ({} members) active",
                                        proj.name,
                                        proj.members.len()
                                    ));
                                }

                                if let Some(ref wp) = result.workspace_project {
                                    msg.push_str(&format!(
                                        " · Restored dual-workspace umbrella ({} members: primary='{}'). Open via .code-workspace",
                                        wp.members.len(),
                                        wp.primary_member()
                                            .map(|m| m.path.display().to_string())
                                            .unwrap_or_else(|| "primary".to_string())
                                    ));
                                }

                                let new_ws = result.workspace_path.display().to_string();

                                if auto_install {
                                    if let Some(ref warn_msg) = result.install_warning {
                                        msg.push_str(&format!(
                                            " · Offline install warning: {}",
                                            warn_msg
                                        ));
                                    } else if result.installed_packages > 0 {
                                        msg.push_str(&format!(
                                            " · Materialized {} packages into workspace offline",
                                            result.installed_packages
                                        ));
                                    }
                                }

                                AirGapTaskOutcome::ImportSuccess {
                                    status_message: msg,
                                    workspace_path: new_ws,
                                }
                            }
                            Err(error) => {
                                AirGapTaskOutcome::ImportFailure(format!("Import Failed: {error}"))
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
}

fn format_export_success_summary(
    prefix: &str,
    workspace_canon: &Path,
    lockfile_path: &Path,
    manifest: &AirGapManifest,
) -> String {
    let ws_info = if let Some(ref proj) = manifest.workspace_project {
        format!(" [Dual-Workspace: {} members]", proj.members.len())
    } else {
        String::new()
    };
    let short_sha = manifest
        .archive_sha256
        .get(..12)
        .unwrap_or(&manifest.archive_sha256);

    let mut summary = format!(
        "{prefix} Packages: {}, SHA256: {short_sha}{ws_info}",
        manifest.package_count
    );

    if let Some(prereq_note) = dmz_core::toolchain::export_apple_prereq_note_from_lockfile(
        workspace_canon,
        lockfile_path,
        manifest.target_platform.as_deref(),
    ) {
        summary.push('\n');
        summary.push_str(&prereq_note);
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_airgap_grid_layout() {
        let ctx = egui::Context::default();
        let mut state = AirGapState::default();
        let mut ws = ".".to_string();
        let mut lock = "dmz.lock".to_string();
        let mut msg = String::new();

        assert!(state.show_export_tip);

        for _ in 0..3 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                show(&mut state, ui, &mut ws, &mut lock, &mut msg);
            });
            out.textures_delta.clear();
        }

        state.show_export_tip = false;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            show(&mut state, ui, &mut ws, &mut lock, &mut msg);
        });
        out.textures_delta.clear();
    }
}
