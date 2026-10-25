mod components;
mod navigation;
mod pages;
mod theme;

use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Resolver,
    WorkspaceSandbox,
    AgentSandbox,
    AirGap,
    VersionControl,
    Supervisor,
    Settings,
}

pub struct DmzGuiApp {
    active_tab: Tab,
    workspace_path: String,
    lockfile_path: String,
    status_message: String,
    pages: pages::Pages,
}

impl Default for DmzGuiApp {
    fn default() -> Self {
        Self {
            active_tab: Tab::Resolver,
            workspace_path: String::from("."),
            lockfile_path: String::from("dmz.lock"),
            status_message: String::from("Ready."),
            pages: pages::Pages::default(),
        }
    }
}

impl DmzGuiApp {
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default();
        app.pages
            .load_appearance(cc.storage, &mut app.status_message);
        app
    }
}

impl eframe::App for DmzGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let visuals = match self.pages.apply_appearance(&ctx) {
            Ok(visuals) => visuals,
            Err(error) => {
                self.status_message = format!("Could not apply appearance settings: {error}");
                let mut visuals = egui::Visuals::dark();
                visuals.widgets.hovered.expansion = 0.0;
                visuals.widgets.active.expansion = 0.0;
                visuals.widgets.open.expansion = 0.0;
                visuals.widgets.inactive.bg_stroke.width = visuals.widgets.hovered.bg_stroke.width;
                ctx.set_visuals(visuals.clone());
                visuals
            }
        };
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(visuals.panel_fill)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                let viewport_size = ui.available_size_before_wrap();
                ui.allocate_ui_with_layout(
                    viewport_size,
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        let navigation_width = 220.0_f32.min(ui.available_width() * 0.35);
                        navigation::show(ui, &mut self.active_tab, navigation_width, &visuals);

                        let surface_size = egui::vec2(ui.available_width(), ui.available_height());
                        ui.allocate_ui_with_layout(
                            surface_size,
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                let (surface_rect, _) =
                                    ui.allocate_exact_size(surface_size, egui::Sense::hover());
                                ui.painter().rect_filled(
                                    surface_rect,
                                    egui::CornerRadius::same(16),
                                    visuals.extreme_bg_color,
                                );
                                ui.painter().rect_stroke(
                                    surface_rect.shrink(0.5),
                                    egui::CornerRadius::same(16),
                                    egui::Stroke::new(
                                        1.0,
                                        visuals.widgets.noninteractive.bg_stroke.color,
                                    ),
                                    egui::StrokeKind::Inside,
                                );

                                let content_rect = surface_rect.shrink(16.0);
                                let mut content_ui = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(content_rect)
                                        .layout(egui::Layout::top_down(egui::Align::Min)),
                                );
                                content_ui.set_clip_rect(content_rect);
                                content_ui.vertical(|ui| {
                                    ui.add_space(2.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(
                                            ui.available_width(),
                                            components::FIELD_BUTTON_HEIGHT,
                                        ),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.heading("DMZ Engine");
                                            ui.add_space(4.0);
                                            let (indicator_rect, _) = ui.allocate_exact_size(
                                                egui::vec2(10.0, 10.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().circle_filled(
                                                indicator_rect.center(),
                                                5.0,
                                                egui::Color32::from_rgb(92, 224, 160),
                                            );
                                            ui.add_space(4.0);
                                            ui.label(
                                                egui::RichText::new("online")
                                                    .font(egui::FontId::proportional(12.0))
                                                    .color(egui::Color32::from_rgb(92, 224, 160)),
                                            );

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if components::browse_button(ui).clicked() {
                                                        if let Some(path) =
                                                            rfd::FileDialog::new().pick_folder()
                                                        {
                                                            self.workspace_path =
                                                                path.display().to_string();
                                                        }
                                                    }
                                                    let ws_label = if self
                                                        .workspace_path
                                                        .trim()
                                                        .is_empty()
                                                        || self.workspace_path.trim() == "."
                                                    {
                                                        "Workspace:".to_string()
                                                    } else {
                                                        format!(
                                                            "Workspace: {}",
                                                            self.workspace_path
                                                        )
                                                    };
                                                    ui.label(
                                                        egui::RichText::new(ws_label)
                                                            .font(egui::FontId::proportional(12.0))
                                                            .color(
                                                                visuals
                                                                    .widgets
                                                                    .noninteractive
                                                                    .fg_stroke
                                                                    .color,
                                                            ),
                                                    );
                                                },
                                            );
                                        },
                                    );
                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.add_space(10.0);

                                    let status_bar_height = if self.pages.show_status_bar() {
                                        48.0
                                    } else {
                                        0.0
                                    };
                                    egui::ScrollArea::vertical()
                                        .auto_shrink([false, false])
                                        .max_height(
                                            (ui.available_height() - status_bar_height).max(0.0),
                                        )
                                        .show(ui, |ui| {
                                            self.pages.show_tab(
                                                self.active_tab,
                                                ui,
                                                &ctx,
                                                &mut self.workspace_path,
                                                &mut self.lockfile_path,
                                                &mut self.status_message,
                                            );
                                        });
                                    if self.pages.show_status_bar() {
                                        components::status_bar(ui, &self.status_message);
                                    }
                                });
                            },
                        );
                    },
                );
            });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.pages.save(storage);
    }
}
