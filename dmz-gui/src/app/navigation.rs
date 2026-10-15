use eframe::egui;

use super::{components, Tab};

pub(super) fn show(
    ui: &mut egui::Ui,
    active_tab: &mut Tab,
    status_message: &str,
    show_status_bar: bool,
    width: f32,
    visuals: &egui::Visuals,
) {
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
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("DMZ")
                        .font(egui::FontId::proportional(28.0))
                        .strong()
                        .color(text),
                );
            });
            ui.add_space(18.0);

            for (tab, label) in [
                (Tab::Resolver, "Resolver"),
                (Tab::Builder, "Package Builder"),
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

            if show_status_bar {
                let remaining_height = ui.available_height();
                let remaining_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(remaining_width, remaining_height),
                    egui::Layout::bottom_up(egui::Align::Min),
                    |ui| {
                        let footer_height = 44.0;
                        let (footer_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), footer_height),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(footer_rect, 0.0, fill);
                        ui.painter().hline(
                            footer_rect.x_range(),
                            footer_rect.top(),
                            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
                        );

                        let inner_rect = footer_rect.shrink2(egui::vec2(18.0, 0.0));
                        let dot_center = egui::pos2(inner_rect.left() + 1.0, inner_rect.center().y);
                        ui.painter().circle_filled(
                            dot_center,
                            5.0,
                            egui::Color32::from_rgb(71, 209, 125),
                        );

                        let text_left = dot_center.x + 13.0;
                        let text_width = (inner_rect.right() - text_left).max(0.0);
                        let font_size = ui.fonts_mut(|fonts| {
                            let natural_width = fonts
                                .layout_no_wrap(
                                    status_message.to_owned(),
                                    egui::FontId::proportional(13.0),
                                    text,
                                )
                                .size()
                                .x;
                            if natural_width > text_width && natural_width > 0.0 {
                                (13.0 * text_width / natural_width).max(10.0)
                            } else {
                                13.0
                            }
                        });
                        let galley = ui.fonts_mut(|fonts| {
                            fonts.layout_no_wrap(
                                status_message.to_owned(),
                                egui::FontId::proportional(font_size),
                                text,
                            )
                        });
                        let text_position =
                            egui::pos2(text_left, footer_rect.center().y - galley.size().y / 2.0);
                        ui.painter().galley(text_position, galley, text);
                    },
                );
            }
        },
    );
}
