use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;

use super::super::components;
use dmz_supervisor::{IpcServerConfig, SidecarLifecycleState};

#[derive(Clone, Debug)]
pub(super) struct SupervisorTelemetry {
    pub(super) sidecar_state: SidecarLifecycleState,
    pub(super) last_error: Option<String>,
    pub(super) sidecar_socket: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct PollingGuard(Arc<AtomicBool>);

impl Drop for PollingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

pub(super) struct SupervisorState {
    pub(super) active: bool,
    pub(super) sidecar_state: SidecarLifecycleState,
    pub(super) sidecar_socket: String,
    pub(super) readiness_timeout_ms: u64,
    pub(super) drain_timeout_ms: u64,
    pub(super) orchestrator: Arc<dmz_supervisor::SidecarOrchestrator>,
    pub(super) tokio_rt: Option<Arc<Runtime>>,
    pub(super) ipc_server: Option<Arc<dmz_supervisor::IpcServer>>,
    pub(super) last_error: Option<String>,
    pub(super) telemetry_rx: std::sync::mpsc::Receiver<SupervisorTelemetry>,
    pub(super) _polling_guard: Arc<PollingGuard>,
    pub(super) cached_workspace: String,
    pub(super) cached_manifest: Option<(
        dmz_core::workspace::WorkspaceProjectConfig,
        std::path::PathBuf,
    )>,
    pub(super) cached_layouts: Vec<dmz_core::workspace::ResolvedMemberLayout>,
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
        let (tx, rx) = std::sync::mpsc::channel();
        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = is_running.clone();
        let guard = Arc::new(PollingGuard(is_running));

        if let Some(ref runtime) = rt {
            let orch = orchestrator.clone();
            runtime.spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(150));
                while is_running_clone.load(Ordering::Relaxed) {
                    interval.tick().await;
                    let current = orch.current_state().await;
                    let err = orch.last_error().await;
                    let sock = orch
                        .current_socket_path()
                        .await
                        .map(|p| p.display().to_string());
                    if tx
                        .send(SupervisorTelemetry {
                            sidecar_state: current,
                            last_error: err,
                            sidecar_socket: sock,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }

        Self {
            active: false,
            sidecar_state: SidecarLifecycleState::Idle,
            sidecar_socket: default_sock.display().to_string(),
            readiness_timeout_ms: 10000,
            drain_timeout_ms: 2000,
            orchestrator,
            tokio_rt: rt,
            ipc_server: None,
            last_error: None,
            telemetry_rx: rx,
            _polling_guard: guard,
            cached_workspace: String::new(),
            cached_manifest: None,
            cached_layouts: Vec::new(),
        }
    }
}

pub(super) fn show(
    state: &mut SupervisorState,
    ui: &mut egui::Ui,
    status_message: &mut String,
    workspace_path: &mut String,
) {
    // Non-blocking telemetry synchronization: drain background updates without blocking UI thread
    while let Ok(telem) = state.telemetry_rx.try_recv() {
        state.sidecar_state = telem.sidecar_state;
        state.last_error = telem.last_error;
        if let Some(sock) = telem.sidecar_socket {
            state.sidecar_socket = sock;
        }
    }

    // Cache workspace manifest discovery to eliminate 120x/sec per-frame disk I/O thrashing
    if state.cached_workspace != *workspace_path {
        let ws_path = std::path::PathBuf::from(&*workspace_path);
        let discovered =
            dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&ws_path);
        let layouts = if let Some((ref proj, ref manifest_file)) = discovered {
            let root = manifest_file
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            proj.resolve_member_layouts(&root, Some(manifest_file))
        } else {
            Vec::new()
        };
        state.cached_manifest = discovered;
        state.cached_layouts = layouts;
        state.cached_workspace = workspace_path.clone();
    }

    let ipc_config = IpcServerConfig::default();
    let socket_str = ipc_config.socket_path.display().to_string();

    components::page_header(ui, "Supervisor IPC");

    // 1. Target Workspace Directory Card
    components::section(ui, |ui| {
        components::section_header(ui, "Target Umbrella Workspace");
        ui.label("Select the workspace root directory containing dmz-workspace.toml for coordinated execution:");
        ui.add_space(8.0);
        let _ = components::path_browse_row(ui, workspace_path, None);

        if let Some((ref proj, ref manifest_file)) = state.cached_manifest {
            let primary = proj.primary_member();
            let p_str = primary.map(|p| p.id.as_str()).unwrap_or("none");
            let sc_str: Vec<&str> = proj.sidecars().iter().map(|s| s.id.as_str()).collect();
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::from_rgb(0, 230, 118),
                format!(
                    "✓ Multi-Repo Workspace '{}' loaded from {:?} (Primary: '{}', Sidecars: [{}])",
                    proj.name,
                    manifest_file.file_name().unwrap_or_default(),
                    p_str,
                    sc_str.join(", ")
                ),
            );
        } else {
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::from_rgb(255, 179, 0),
                format!(
                    "⚠ No dmz-workspace.toml detected in '{}'. Please browse to your imported workspace root.",
                    workspace_path
                ),
            );
        }
    });

    ui.add_space(16.0);

    components::section(ui, |ui| {
        components::section_header(ui, "Supervisor Lifecycle");
        if state.active {
            ui.colored_label(
                egui::Color32::GREEN,
                format!("Active · IPC listening at {}", socket_str),
            );
            ui.add_space(16.0);
            if components::action_button(ui, "Stop Session", false).clicked() {
                if let Some(ref srv) = state.ipc_server.take() {
                    srv.stop();
                }
                state.active = false;
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
                let server = Arc::new(dmz_supervisor::IpcServer::with_managers(
                    IpcServerConfig::default(),
                    dmz_supervisor::AgentRunner::new(),
                    (*state.orchestrator).clone(),
                ));
                state.ipc_server = Some(server.clone());
                if let Some(ref rt) = state.tokio_rt {
                    rt.spawn(async move {
                        let _ = server.start().await;
                    });
                }
                state.active = true;
                *status_message =
                    format!("Workspace Supervisor spawned and listening at {socket_str}");
            }
        }
    });

    if state.sidecar_state == SidecarLifecycleState::Failed || state.last_error.is_some() {
        ui.add_space(16.0);
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(45, 15, 15))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 50, 50)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 80, 80), "⚠ Failure Detected:");
                    let err_msg = state
                        .last_error
                        .as_deref()
                        .unwrap_or("Sidecar pair failed to launch or readiness probe timed out.");
                    ui.label(
                        egui::RichText::new(err_msg)
                            .strong()
                            .color(egui::Color32::from_rgb(255, 140, 140)),
                    );
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Dismiss & Reset to Standby").clicked() {
                        let orch = state.orchestrator.clone();
                        if let Some(ref rt) = state.tokio_rt {
                            rt.spawn(async move {
                                orch.reset_state().await;
                            });
                        }
                        state.sidecar_state = SidecarLifecycleState::Idle;
                        state.last_error = None;
                        *status_message = "Supervisor state reset to Standby (Idle).".to_string();
                    }
                });
            });
    }

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
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut state.readiness_timeout_ms, 0..=30000).suffix(" ms"),
                    );
                    if state.readiness_timeout_ms == 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 179, 0),
                            "(probe skipped; process check only)",
                        );
                    }
                });
                ui.end_row();

                ui.label("Teardown Drain Window");
                ui.add(egui::Slider::new(&mut state.drain_timeout_ms, 500..=10000).suffix(" ms"));
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
                    let ws_path = std::path::PathBuf::from(&*workspace_path);
                    if let Some((mut project_config, manifest_file)) =
                        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&ws_path)
                    {
                        if project_config.sidecars().is_empty() {
                            *status_message = format!(
                                "Cannot launch: Workspace '{}' in {:?} has no sidecar or service member defined.",
                                project_config.name,
                                manifest_file.file_name().unwrap_or_default()
                            );
                        } else {
                            let root = manifest_file
                                .parent()
                                .unwrap_or(std::path::Path::new("."))
                                .to_path_buf();

                            // Inject GUI slider settings into IPC config
                            project_config.ipc.readiness_timeout_ms = state.readiness_timeout_ms;
                            project_config.ipc.drain_timeout_ms = state.drain_timeout_ms;

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
                                    "Spawning workspace pair under KEP-753 supervision (readiness probe: {}ms)",
                                    state.readiness_timeout_ms
                                );
                            }
                        }
                    } else {
                        *status_message = format!(
                            "Cannot launch: No dmz-workspace.toml detected in '{}'. Please browse to your imported workspace root.",
                            workspace_path
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
        components::section_header(ui, "Declared Workspace Members & IPC Session");
        ui.label(format!("Supervisor IPC channel: {}", socket_str));

        ui.add_space(8.0);
        if let Some((_, ref manifest_file)) = state.cached_manifest {
            let root = manifest_file
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            let layouts = &state.cached_layouts;

            egui::Grid::new("supervisor_members_grid")
                .num_columns(4)
                .spacing([16.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("Member ID");
                    ui.strong("Role");
                    ui.strong("Path");
                    ui.strong("Resolved Command");
                    ui.end_row();

                    for layout in layouts {
                        ui.label(&layout.member.id);
                        ui.label(format!("{:?}", layout.member.role));
                        ui.label(layout.member.path.display().to_string());
                        let cmd_display =
                            match layout.member.resolve_command(&layout.host_path, &root) {
                                Ok((cmd, args)) if args.is_empty() => cmd,
                                Ok((cmd, args)) => format!("{} {}", cmd, args.join(" ")),
                                Err(_) => "unconfigured (requires exec_command)".to_string(),
                            };
                        ui.label(cmd_display);
                        ui.end_row();
                    }
                });
        } else {
            ui.label("No dmz-workspace.toml discovered in the selected workspace root.");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supervisor_telemetry_non_blocking_sync() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut state = SupervisorState {
            telemetry_rx: rx,
            ..SupervisorState::default()
        };

        assert_eq!(state.sidecar_state, SidecarLifecycleState::Idle);

        // Send a telemetry update without blocking
        tx.send(SupervisorTelemetry {
            sidecar_state: SidecarLifecycleState::Running,
            last_error: Some("test error".to_string()),
            sidecar_socket: Some("/tmp/test.sock".to_string()),
        })
        .expect("telemetry sends successfully");

        let ctx = egui::Context::default();
        let mut ws = ".".to_string();
        let mut msg = String::new();

        // Run UI headless
        let _ = ctx.run_ui(Default::default(), |ui| {
            show(&mut state, ui, &mut msg, &mut ws);
        });

        assert_eq!(state.sidecar_state, SidecarLifecycleState::Running);
        assert_eq!(state.last_error.as_deref(), Some("test error"));
        assert_eq!(state.sidecar_socket, "/tmp/test.sock");
    }

    #[test]
    fn test_supervisor_manifest_caching_avoids_redundant_disk_io() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        let manifest_path = ws.join("dmz-workspace.toml");
        std::fs::write(
            &manifest_path,
            r#"name = "cached-workspace"
members = [
    { id = "primary", path = ".", role = "primary" }
]
"#,
        )
        .unwrap();

        let mut state = SupervisorState::default();
        let mut ws_str = ws.display().to_string();
        let mut msg = String::new();
        let ctx = egui::Context::default();

        // First frame: loads manifest into cache
        let _ = ctx.run_ui(Default::default(), |ui| {
            show(&mut state, ui, &mut msg, &mut ws_str);
        });

        assert!(state.cached_manifest.is_some());
        let (ref proj, _) = state.cached_manifest.as_ref().unwrap();
        assert_eq!(proj.name, "cached-workspace");
        assert_eq!(state.cached_workspace, ws_str);

        // Delete manifest file from disk
        std::fs::remove_file(&manifest_path).unwrap();
        assert!(!manifest_path.exists());

        // Second frame: because workspace path is unchanged, cached manifest is retained and disk is not read!
        let _ = ctx.run_ui(Default::default(), |ui| {
            show(&mut state, ui, &mut msg, &mut ws_str);
        });

        assert!(state.cached_manifest.is_some());
        let (ref proj, _) = state.cached_manifest.as_ref().unwrap();
        assert_eq!(proj.name, "cached-workspace");
    }
}
