use eframe::egui;

use super::super::components;

#[derive(Default)]
pub(super) struct SupervisorState {
    active: bool,
    postgres_service: bool,
    redis_service: bool,
}

pub(super) fn show(state: &mut SupervisorState, ui: &mut egui::Ui, status_message: &mut String) {
    components::page_header(
        ui,
        "Supervisor IPC",
        "Manage workspace services and inspect local process-supervisor status.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Supervisor lifecycle");
        ui.horizontal_wrapped(|ui| {
            if state.active {
                ui.colored_label(egui::Color32::GREEN, "Active · localized thread running");
                if components::action_button(ui, "Stop session", false).clicked() {
                    state.active = false;
                    state.postgres_service = false;
                    state.redis_service = false;
                    *status_message = String::from(
                        "Workspace Supervisor terminated. Footprint: 0% CPU, 0 MB RAM.",
                    );
                }
            } else {
                ui.colored_label(
                    ui.visuals().widgets.noninteractive.fg_stroke.color,
                    "Inactive · zero idle footprint",
                );
                if components::action_button(ui, "Start session", true).clicked() {
                    state.active = true;
                    *status_message = String::from("Workspace Supervisor spawned successfully.");
                }
            }
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Managed services");
        ui.checkbox(&mut state.postgres_service, "Local Postgres service");
        ui.checkbox(&mut state.redis_service, "Local Redis service");
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "IPC and session details");
        #[cfg(unix)]
        ui.label("IPC socket · /tmp/dmz_supervisor.sock");

        #[cfg(windows)]
        ui.label(r"IPC pipe · \\.\pipe\dmz_supervisor");

        ui.add_space(8.0);
        egui::Grid::new("supervisor_summary")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Supervisor");
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
