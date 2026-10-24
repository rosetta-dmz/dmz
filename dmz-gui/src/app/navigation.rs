use eframe::egui;

use super::{components, Tab};

pub(super) fn show(ui: &mut egui::Ui, active_tab: &mut Tab, width: f32, visuals: &egui::Visuals) {
    let height = ui.available_height();
    let fill = visuals.panel_fill;
    let accent = visuals.selection.bg_fill;
    let muted = visuals.widgets.noninteractive.fg_stroke.color;
    let text = visuals
        .override_text_color
        .unwrap_or(visuals.widgets.inactive.fg_stroke.color);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(width);
            ui.set_min_height(height);
            ui.painter().rect_filled(ui.max_rect(), 0.0, fill);
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("DMZ")
                        .font(egui::FontId::proportional(28.0))
                        .strong()
                        .color(text),
                );
            });
            ui.add_space(16.0);

            for (tab, label) in [
                (Tab::Resolver, "Resolver"),
                (Tab::WorkspaceSandbox, "Workspace Sandbox"),
                (Tab::AgentSandbox, "Agent Sandbox"),
                (Tab::AirGap, "Air-Gap Manager"),
                (Tab::Supervisor, "Supervisor IPC"),
                (Tab::Settings, "Settings"),
            ] {
                let response =
                    components::nav_item(ui, *active_tab == tab, label, accent, muted, text);
                if response.clicked() {
                    *active_tab = tab;
                }
            }
        },
    );
}
