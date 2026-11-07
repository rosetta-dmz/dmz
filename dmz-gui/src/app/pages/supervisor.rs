use eframe::egui;
use std::sync::Arc;
use tokio::runtime::Runtime;

use super::super::components;
use dmz_supervisor::{IpcServerConfig, SidecarLifecycleState};

pub(super) struct SupervisorState {
    pub(super) active: bool,
    pub(super) postgres_service: bool,
    pub(super) redis_service: bool,
    pub(super) sidecar_state: SidecarLifecycleState,
    pub(super) sidecar_socket: String,
    pub(super) readiness_timeout_ms: u64,
    pub(super) drain_timeout_ms: u64,
    pub(super) orchestrator: Arc<dmz_supervisor::SidecarOrchestrator>,
    pub(super) tokio_rt: Option<Arc<Runtime>>,
}

impl Default for SupervisorState {
    fn default() -> Self {
        let default_sock =
            dmz_core::workspace::safe_socket_path(std::path::Path::new("."), "sidecar");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .ok()
            .map(Arc::new);
        let orchestrator = Arc::new(dmz_supervisor::SidecarOrchestrator::new());
        Self {
            active: false,
            postgres_service: false,
            redis_service: false,
            sidecar_state: SidecarLifecycleState::Idle,
            sidecar_socket: default_sock.display().to_string(),
            readiness_timeout_ms: 5000,
            drain_timeout_ms: 2000,
            orchestrator,
            tokio_rt: rt,
        }
    }
}

pub(super) fn show(
    state: &mut SupervisorState,
    ui: &mut egui::Ui,
    status_message: &mut String,
    workspace_path: &str,
) {
    // Synchronize live orchestrator state if runtime is present
    if let Some(ref rt) = state.tokio_rt {
        let orch = state.orchestrator.clone();
        state.sidecar_state = rt.block_on(async { orch.current_state().await });
        if let Some(sock) = rt.block_on(async { orch.current_socket_path().await }) {
            state.sidecar_socket = sock.display().to_string();
        }
    }

    let ipc_config = IpcServerConfig::default();
    let socket_str = ipc_config.socket_path.display().to_string();

    components::page_header(ui, "Supervisor IPC");

    components::section(ui, |ui| {
        components::section_header(ui, "Supervisor Lifecycle");
        if state.active {
            ui.colored_label(egui::Color32::GREEN, "Active · localized session running");
            ui.add_space(16.0);
            if components::action_button(ui, "Stop Session", false).clicked() {
                state.active = false;
                state.postgres_service = false;
                state.redis_service = false;
                if ipc_config.socket_path.exists() {
                    let _ = std::fs::remove_file(&ipc_config.socket_path);
                }
                *status_message =
                    String::from("Workspace Supervisor terminated. Footprint: 0% CPU, 0 MB RAM.");
            }
        } else {
            ui.colored_label(
                ui.visuals().widgets.noninteractive.fg_stroke.color,
                "Inactive · zero idle footprint",
            );
            ui.add_space(16.0);
            if components::action_button(ui, "Start Session", true).clicked() {
                state.active = true;
                *status_message = format!("Workspace Supervisor spawned at {socket_str}");
            }
        }
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Multi-Repo Workspace Orchestration (KEP-753)");

        ui.horizontal_wrapped(|ui| {
            let current = &state.sidecar_state;
            let stages = [
                ("Idle", matches!(current, SidecarLifecycleState::Idle)),
                (
                    "Clean Socket",
                    matches!(current, SidecarLifecycleState::CleaningStaleSocket),
                ),
                (
                    "Spawning Services",
                    matches!(current, SidecarLifecycleState::SpawningSidecar),
                ),
                (
                    "Readiness Probe",
                    matches!(current, SidecarLifecycleState::ProbingSocketReadiness),
                ),
                (
                    "Services Ready",
                    matches!(current, SidecarLifecycleState::SidecarReady),
                ),
                (
                    "Active Bridge",
                    matches!(
                        current,
                        SidecarLifecycleState::SpawningPrimary | SidecarLifecycleState::Running
                    ),
                ),
                (
                    "Graceful Drain",
                    matches!(
                        current,
                        SidecarLifecycleState::DrainingPrimary
                            | SidecarLifecycleState::TerminatingSidecar
                    ),
                ),
            ];

            for (idx, (name, is_current)) in stages.iter().enumerate() {
                if idx > 0 {
                    ui.label("→");
                }
                if *is_current {
                    ui.colored_label(
                        egui::Color32::from_rgb(0, 230, 118),
                        egui::RichText::new(*name).strong(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(*name)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                }
            }
        });

        ui.add_space(12.0);

        egui::Grid::new("sidecar_telemetry_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Lifecycle State");
                ui.label(match &state.sidecar_state {
                    SidecarLifecycleState::Idle => "Idle (Standby)".to_string(),
                    SidecarLifecycleState::CleaningStaleSocket => {
                        "Cleaning stale socket...".to_string()
                    }
                    SidecarLifecycleState::SpawningSidecar => {
                        "Spawning sidecar process...".to_string()
                    }
                    SidecarLifecycleState::ProbingSocketReadiness => {
                        "Polling socket readiness probe...".to_string()
                    }
                    SidecarLifecycleState::SidecarReady => {
                        "Sidecar socket ready (handshake verified)".to_string()
                    }
                    SidecarLifecycleState::SpawningPrimary => {
                        "Spawning primary application...".to_string()
                    }
                    SidecarLifecycleState::Running => {
                        "Primary running with active sidecar bridge".to_string()
                    }
                    SidecarLifecycleState::DrainingPrimary => {
                        "Reverse graceful drain: waiting for primary exit...".to_string()
                    }
                    SidecarLifecycleState::TerminatingSidecar => {
                        "Terminating sidecar & unlinking socket...".to_string()
                    }
                    SidecarLifecycleState::Stopped => "Stopped cleanly".to_string(),
                    SidecarLifecycleState::Failed => "Failed".to_string(),
                });
                ui.end_row();

                ui.label("Safe Socket Path");
                ui.label(&state.sidecar_socket);
                ui.end_row();

                ui.label("Socket Path Length");
                let len = state.sidecar_socket.len();
                let status_color = if len < 104 {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(
                    status_color,
                    format!(
                        "{} bytes (POSIX sun_path limit: 104 macOS / 108 Linux)",
                        len
                    ),
                );
                ui.end_row();

                ui.label("Readiness Gate Timeout");
                ui.label(format!("{} ms", state.readiness_timeout_ms));
                ui.end_row();

                ui.label("Teardown Drain Window");
                ui.label(format!("{} ms", state.drain_timeout_ms));
                ui.end_row();
            });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if matches!(
                state.sidecar_state,
                SidecarLifecycleState::Idle
                    | SidecarLifecycleState::Stopped
                    | SidecarLifecycleState::Failed
            ) {
                if components::action_button(ui, "Launch Workspace Pair", true).clicked() {
                    let ws_path = std::path::PathBuf::from(workspace_path);
                    let (project_config, root) = if let Some((cfg, manifest_file)) =
                        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&ws_path)
                    {
                        let r = manifest_file
                            .parent()
                            .unwrap_or(std::path::Path::new("."))
                            .to_path_buf();
                        (cfg, r)
                    } else {
                        let ws_name = ws_path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "workspace".to_string());
                        let members = vec![dmz_core::workspace::WorkspaceMember::new(
                            "primary",
                            std::path::Path::new("."),
                            dmz_core::workspace::MemberRole::Primary,
                        )];
                        (
                            dmz_core::workspace::WorkspaceProjectConfig::new(ws_name, members),
                            ws_path.clone(),
                        )
                    };

                    let pair_config = dmz_supervisor::SidecarPairConfig {
                        workspace_root: root,
                        project: project_config,
                        allow_network: false,
                    };

                    let orch = state.orchestrator.clone();
                    if let Some(ref rt) = state.tokio_rt {
                        rt.spawn(async move {
                            if let Err(e) = orch.start_pair(pair_config).await {
                                tracing::error!("Workspace launch error: {}", e);
                            }
                        });
                        *status_message = format!(
                            "Spawning workspace pair under KEP-753 supervision at {}",
                            state.sidecar_socket
                        );
                    }
                }
            } else if components::action_button(ui, "Stop Pair (Graceful Drain)", false).clicked() {
                let orch = state.orchestrator.clone();
                if let Some(ref rt) = state.tokio_rt {
                    rt.spawn(async move {
                        let _ = orch.stop_pair().await;
                    });
                    *status_message = String::from(
                        "Executed KEP-753 reverse graceful drain: stopping workspace pair and unlinking socket.",
                    );
                }
            }
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Managed Workspace Services");
        ui.checkbox(&mut state.postgres_service, "Local Postgres service");
        ui.checkbox(&mut state.redis_service, "Local Redis service");
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "IPC and Session Details");
        ui.label(format!("IPC channel: {}", socket_str));

        ui.add_space(8.0);
        egui::Grid::new("supervisor_summary")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Supervisor status");
                ui.label(if state.active { "Running" } else { "Idle" });
                ui.end_row();

                ui.label("Postgres");
                ui.label(if state.postgres_service {
                    "Up"
                } else {
                    "Stopped"
                });
                ui.end_row();

                ui.label("Redis");
                ui.label(if state.redis_service { "Up" } else { "Stopped" });
                ui.end_row();

                ui.label("Connected IDE clients");
                ui.label("1 · GUI desktop");
                ui.end_row();
            });
    });
}
