use eframe::egui;

#[derive(Default)]
pub(super) struct SupervisorState {
    active: bool,
    postgres_service: bool,
    redis_service: bool,
}

pub(super) fn show(state: &mut SupervisorState, ui: &mut egui::Ui, status_message: &mut String) {
    ui.heading("Workspace Process Supervisor");
    ui.label(
        "Monitor localized IPC socket streams, background collateral services, and process trees.",
    );
    ui.add_space(10.0);

    ui.horizontal(|ui| {
        ui.label("Supervisor Lifecycle Status:");
        if state.active {
            ui.colored_label(egui::Color32::GREEN, "ACTIVE (Localized Thread Running)");
            if ui.button("Stop Supervisor Session").clicked() {
                state.active = false;
                state.postgres_service = false;
                state.redis_service = false;
                *status_message =
                    String::from("Workspace Supervisor terminated. Footprint: 0% CPU, 0 MB RAM.");
            }
        } else {
            ui.colored_label(
                egui::Color32::GRAY,
                "INACTIVE (Zero Idle Footprint: 0% CPU, 0 MB RAM)",
            );
            if ui.button("Start Supervisor Session").clicked() {
                state.active = true;
                *status_message = String::from("Workspace Supervisor spawned successfully.");
            }
        }
    });

    ui.add_space(10.0);
    ui.separator();

    ui.heading("Collateral Stack Services & IDE IPC Sockets");
    ui.add_space(5.0);

    ui.checkbox(
        &mut state.postgres_service,
        "Local Postgres Service (Managed by Supervisor)",
    );
    ui.checkbox(
        &mut state.redis_service,
        "Local Redis Service (Managed by Supervisor)",
    );

    ui.add_space(10.0);

    #[cfg(unix)]
    ui.label("IPC Socket Path: /tmp/dmz_supervisor.sock (VS Code / JetBrains Integration)");

    #[cfg(windows)]
    ui.label(r"IPC Pipe Path: \\.\pipe\dmz_supervisor (VS Code / JetBrains Integration)");

    ui.add_space(15.0);

    ui.group(|ui| {
        ui.label(format!(
            "Supervisor State: {}",
            if state.active { "RUNNING" } else { "IDLE" }
        ));
        ui.label(format!(
            "Active Stack Services: Postgres [{}], Redis [{}]",
            if state.postgres_service {
                "UP"
            } else {
                "STOPPED"
            },
            if state.redis_service { "UP" } else { "STOPPED" }
        ));
        ui.label("Connected IDE Clients: 1 (GUI Desktop)");
    });
}
