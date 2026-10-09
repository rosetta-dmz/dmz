pub fn render_dashboard(ui: &mut egui::Ui, locked_packages: &mut Vec<(String, String, bool)>) {
    ui.heading("DMZ Workspace Control Center");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("System Telemetry:");
        ui.colored_label(egui::Color32::GREEN, "CPU: 0.4% | RAM: 42 MB | Idle Footprint: 0 MB");
    });

    ui.add_space(10.0);
    ui.heading("Dependency Version Pinning");
    
    egio_table_placeholder(ui, locked_packages);
}

fn egio_table_placeholder(ui: &mut egui::Ui, packages: &mut Vec<(String, String, bool)>) {
    egui::Grid::new("dependency_grid").striped(true).show(ui, |ui| {
        ui.strong("Package Name");
        ui.strong("Pinned Version");
        ui.strong("Status / Actions");
        ui.end_row();

        // Example mock row for GUI verification
        ui.label("serde");
        ui.label("1.0.197");
        ui.label("Pinned [✓]");
        ui.end_row();
    });
}