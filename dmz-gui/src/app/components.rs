use eframe::egui;

pub(super) fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    accent: egui::Color32,
    muted: egui::Color32,
    text: egui::Color32,
) -> egui::Response {
    let hover = ui.visuals().widgets.hovered;
    let (idle, hovered) = if selected {
        let stroke = accent.gamma_multiply(0.65);
        (
            ButtonLook::new(accent.gamma_multiply(0.16), stroke, text),
            ButtonLook::new(accent.gamma_multiply(0.24), stroke, text),
        )
    } else {
        (
            ButtonLook::new(
                egui::Color32::TRANSPARENT,
                egui::Color32::TRANSPARENT,
                muted,
            ),
            ButtonLook::new(hover.bg_fill, hover.bg_stroke.color, text),
        )
    };

    with_button_looks(ui, idle, hovered, |ui| {
        ui.add_sized(
            egui::vec2(ui.available_width(), 40.0),
            egui::Button::new(label)
                .corner_radius(12.0) // Updated from 10.0 to match cards/sections (12 pt grid token)
                .min_size(egui::vec2(0.0, 40.0)),
        )
    })
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Colors for one interaction state of a button.
#[derive(Clone, Copy)]
struct ButtonLook {
    fill: egui::Color32,
    stroke: egui::Color32,
    text: egui::Color32,
}

impl ButtonLook {
    fn new(fill: egui::Color32, stroke: egui::Color32, text: egui::Color32) -> Self {
        Self { fill, stroke, text }
    }
}

/// Runs `add_contents` in a scope whose widget `Visuals` use `idle` for the inactive
/// state and `hovered` for the hovered/active states. This lets plain `egui::Button`s
/// handle hover themselves (calling `Button::fill`/`stroke` would override hover effects).
///
/// Stroke width is kept at 1.0 in every state because egui derives button padding from
/// it; differing widths would make buttons grow/shrink on hover.
fn with_button_looks<R>(
    ui: &mut egui::Ui,
    idle: ButtonLook,
    hovered: ButtonLook,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        // The theme sets a global text color, which would otherwise win over `fg_stroke`.
        visuals.override_text_color = None;
        let widgets = &mut visuals.widgets;
        for (state, look) in [
            (&mut widgets.inactive, idle),
            (&mut widgets.hovered, hovered),
            (&mut widgets.active, hovered),
        ] {
            state.weak_bg_fill = look.fill;
            state.bg_fill = look.fill;
            state.bg_stroke = egui::Stroke::new(1.0, look.stroke);
            state.fg_stroke.color = look.text;
            state.expansion = 0.0;
        }
        add_contents(ui)
    })
    .inner
}

pub(super) fn section(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    let visuals = &ui.visuals();
    let card_width = ui.available_width();
    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width((card_width - 32.0).max(0.0));
            contents(ui);
        });
}

pub(super) fn page_header(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .font(egui::FontId::proportional(22.0))
            .strong(),
    );
    ui.add_space(16.0);
}

pub(super) fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).size(16.0).strong());
    ui.add_space(16.0);
}

pub(super) fn card(
    ui: &mut egui::Ui,
    selected: bool,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::InnerResponse<()> {
    let visuals = &ui.visuals();
    let border = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.widgets.noninteractive.bg_stroke.color
    };
    let card_width = ui.available_width();
    egui::Frame::new()
        .fill(if selected {
            visuals.selection.bg_fill.gamma_multiply(0.2)
        } else {
            visuals.extreme_bg_color
        })
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width((card_width - 32.0).max(0.0));
            ui.set_min_height(64.0);
            contents(ui);
        })
}

pub(super) fn action_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let visuals = ui.visuals();
    let accent = visuals.selection.stroke.color;
    let (idle, hovered) = if primary {
        (
            ButtonLook::new(
                visuals.selection.bg_fill,
                accent.gamma_multiply(0.45),
                accent,
            ),
            ButtonLook::new(
                accent.gamma_multiply(0.26),
                accent.gamma_multiply(0.75),
                accent,
            ),
        )
    } else {
        let text = visuals.widgets.inactive.fg_stroke.color;
        (
            ButtonLook::new(
                visuals.extreme_bg_color,
                visuals.widgets.noninteractive.bg_stroke.color,
                text,
            ),
            ButtonLook::new(
                visuals.widgets.hovered.bg_fill,
                visuals.widgets.hovered.bg_stroke.color,
                text,
            ),
        )
    };

    with_button_looks(ui, idle, hovered, |ui| {
        ui.spacing_mut().button_padding += BUTTON_PADDING_EXTRA;
        ui.add(
            egui::Button::new(label)
                .min_size(egui::vec2(0.0, 32.0))
                .corner_radius(egui::CornerRadius::same(8)),
        )
    })
}

/// Extra internal room for button text (2.0px).
pub(super) const BUTTON_PADDING_EXTRA: egui::Vec2 = egui::vec2(8.0, 8.0);

/// Standard height for single-line input field buttons (32.0px).
pub(super) const FIELD_BUTTON_HEIGHT: f32 = 32.0;

/// Standard width for browse buttons (88.0px).
pub(super) const BROWSE_BUTTON_WIDTH: f32 = 88.0;

/// Standard button used adjacent to text fields (e.g. "Browse...", "Open...", "Save As...").
pub(super) fn field_button(ui: &mut egui::Ui, label: &str, width: Option<f32>) -> egui::Response {
    let btn_width = width.unwrap_or(BROWSE_BUTTON_WIDTH);
    ui.scope(|ui| {
        ui.spacing_mut().button_padding += BUTTON_PADDING_EXTRA;
        ui.add_sized(
            egui::vec2(btn_width, FIELD_BUTTON_HEIGHT),
            egui::Button::new(label),
        )
    })
    .inner
}

/// Standard "Browse..." button with uniform 88.0x32.0 dimensions.
pub(super) fn browse_button(ui: &mut egui::Ui) -> egui::Response {
    field_button(ui, "Browse...", Some(BROWSE_BUTTON_WIDTH))
}

/// Pairs a single-line TextEdit with an aligned trailing button (like "Browse...").
/// Automatically computes field width so the button is flush with the container right edge.
/// Uses an exact 32.0px horizontal layout matching FIELD_BUTTON_HEIGHT to prevent vertical shift.
/// Returns true if the trailing button was clicked.
pub(super) fn path_browse_row(
    ui: &mut egui::Ui,
    path_val: &mut String,
    button_label: Option<&str>,
) -> bool {
    let btn_label = button_label.unwrap_or("Browse...");
    let btn_width = BROWSE_BUTTON_WIDTH;
    let spacing = ui.spacing().item_spacing.x;
    let field_width = (ui.available_width() - btn_width - spacing).max(80.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), FIELD_BUTTON_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_sized(
                egui::vec2(field_width, FIELD_BUTTON_HEIGHT),
                egui::TextEdit::singleline(path_val)
                    .vertical_align(egui::Align::Center)
                    .margin(egui::Margin::symmetric(8, 0)),
            );
            field_button(ui, btn_label, Some(btn_width)).clicked()
        },
    )
    .inner
}

pub(super) fn toggle(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    let size = egui::vec2(40.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }

    let visuals = ui.visuals();
    let accent = visuals.selection.stroke.color;
    let off = visuals.widgets.noninteractive.bg_stroke.color;
    let animation = ui.ctx().animate_bool_with_time(response.id, *value, 0.11);
    let track = lerp_color(off, accent, animation);
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(12), track);
    let knob_x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), animation);
    ui.painter().circle_filled(
        egui::pos2(knob_x, rect.center().y),
        8.0,
        egui::Color32::WHITE,
    );
    response
}

/// Renders a closeable info toast / callout banner.
/// Returns true if the close button was clicked.
pub(super) fn dismissable_toast(ui: &mut egui::Ui, text: &str, open: &mut bool) -> bool {
    if !*open {
        return false;
    }
    let visuals = ui.visuals();
    let accent = visuals.selection.stroke.color;
    let text_color = visuals.widgets.noninteractive.fg_stroke.color;
    let card_width = ui.available_width();
    let mut closed = false;

    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_min_width((card_width - 32.0).max(0.0));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("ℹ").color(accent).size(15.0).strong());
                ui.add_space(6.0);
                let close_button_width = 24.0;
                let text_width = (ui.available_width() - close_button_width - 12.0).max(80.0);
                ui.add_sized(
                    egui::vec2(text_width, 0.0),
                    egui::Label::new(
                        egui::RichText::new(text)
                            .color(text_color)
                            .size(12.0),
                    )
                    .wrap(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close_btn = egui::Button::new(
                        egui::RichText::new("✕").size(12.0).color(text_color),
                    )
                    .frame(false)
                    .min_size(egui::vec2(20.0, 20.0));
                    if ui
                        .add(close_btn)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        *open = false;
                        closed = true;
                    }
                });
            });
        });

    closed
}

pub(super) fn status_bar(ui: &mut egui::Ui, status_message: &str) {
    let visuals = ui.visuals();
    let text = visuals
        .override_text_color
        .unwrap_or(visuals.widgets.inactive.fg_stroke.color);
    let border = visuals.widgets.noninteractive.bg_stroke.color;
    let (footer_rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::hover());
    ui.painter().hline(
        footer_rect.x_range(),
        footer_rect.top(),
        egui::Stroke::new(1.0, border),
    );

    let inner_rect = footer_rect.shrink2(egui::vec2(16.0, 0.0));
    let dot_center = egui::pos2(inner_rect.left() + 1.0, inner_rect.center().y + 4.0);
    ui.painter()
        .circle_filled(dot_center, 5.0, egui::Color32::from_rgb(71, 209, 125));

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
    let text_position = egui::pos2(
        text_left,
        footer_rect.center().y - galley.size().y / 2.0 + 4.0,
    );
    ui.painter().galley(text_position, galley, text);
}

fn lerp_color(a: egui::Color32, b: egui::Color32, amount: f32) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |from: u8, to: u8| (from as f32 + (to as f32 - from as f32) * amount).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

pub(super) fn swatch(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 24.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(4), color);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}
