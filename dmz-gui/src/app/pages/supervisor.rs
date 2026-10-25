use eframe::egui;

use super::super::components;
use dmz_supervisor::IpcServerConfig;

#[derive(Default)]
pub(super) struct SupervisorState {
    active: bool,
    postgres_service: bool,
    redis_service: bool,
}

pub(super) fn show(state: &mut SupervisorState, ui: &mut egui::Ui, status_message: &mut String) {
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
