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
                .corner_radius(12.0)
                .min_size(egui::vec2(0.0, 40.0)),
        )
    })
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Colors for one interaction state of a button
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

fn with_button_looks<R>(
    ui: &mut egui::Ui,
    idle: ButtonLook,
    hovered: ButtonLook,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
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

pub(super) const CARD_RIGHT_CLEARANCE: f32 = 2.0;

pub(super) fn section(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    let visuals = &ui.visuals();
    let card_width = (ui.available_width() - CARD_RIGHT_CLEARANCE).max(0.0);
    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            let inner_width = (card_width - 32.0).max(0.0);
            ui.set_min_width(inner_width);
            ui.set_max_width(inner_width);
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
    let card_width = (ui.available_width() - CARD_RIGHT_CLEARANCE).max(0.0);
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
            let inner_width = (card_width - 32.0).max(0.0);
            ui.set_min_width(inner_width);
            ui.set_max_width(inner_width);
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

pub(super) const BUTTON_PADDING_EXTRA: egui::Vec2 = egui::vec2(8.0, 8.0);

pub(super) const FIELD_BUTTON_HEIGHT: f32 = 32.0;

pub(super) const BROWSE_BUTTON_WIDTH: f32 = 88.0;

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

pub(super) fn browse_button(ui: &mut egui::Ui) -> egui::Response {
    field_button(ui, "Browse...", Some(BROWSE_BUTTON_WIDTH))
}

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

pub(super) fn dismissable_toast(ui: &mut egui::Ui, text: &str, open: &mut bool) -> bool {
    if !*open {
        return false;
    }
    let visuals = ui.visuals().clone();
    let accent = visuals.selection.stroke.color;
    let text_color = visuals.widgets.noninteractive.fg_stroke.color;
    let card_width = (ui.available_width() - CARD_RIGHT_CLEARANCE).max(0.0);
    let mut closed = false;

    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            let inner_width = (card_width - 32.0).max(0.0);
            ui.set_min_width(inner_width);
            ui.set_max_width(inner_width);
            let icon_size = 18.0;
            let close_size = 20.0;
            let gap = 12.0;

            let text_wrap_width = (inner_width - icon_size - gap - close_size - gap).max(60.0);
            let galley = ui.painter().layout(
                text.to_owned(),
                egui::FontId::proportional(12.0),
                text_color,
                text_wrap_width,
            );
            let text_size = galley.size();
            let row_height = text_size.y.max(close_size);

            let (row_rect, _) =
                ui.allocate_exact_size(egui::vec2(inner_width, row_height), egui::Sense::hover());
            let center_y = row_rect.center().y;

            // Vector info badge (vertically centered with text message)
            let icon_center = egui::pos2(row_rect.left() + icon_size / 2.0, center_y);
            ui.painter()
                .circle_filled(icon_center, 9.0, accent.gamma_multiply(0.2));
            ui.painter()
                .circle_stroke(icon_center, 9.0, egui::Stroke::new(1.0, accent));
            ui.painter()
                .circle_filled(egui::pos2(icon_center.x, icon_center.y - 3.5), 1.2, accent);
            ui.painter().line_segment(
                [
                    egui::pos2(icon_center.x, icon_center.y - 1.0),
                    egui::pos2(icon_center.x, icon_center.y + 4.0),
                ],
                egui::Stroke::new(1.5, accent),
            );

            let text_pos = egui::pos2(
                row_rect.left() + icon_size + gap,
                center_y - text_size.y / 2.0,
            );
            ui.painter().galley(text_pos, galley, text_color);

            let close_rect = egui::Rect::from_center_size(
                egui::pos2(row_rect.right() - close_size / 2.0, center_y),
                egui::vec2(close_size, close_size),
            );
            let close_resp = ui.interact(
                close_rect,
                ui.id().with("toast_close"),
                egui::Sense::click(),
            );
            if close_resp.clicked() {
                *open = false;
                closed = true;
            }
            let close_hovered = close_resp.hovered();
            if close_hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                ui.painter().rect_filled(
                    close_rect,
                    egui::CornerRadius::same(4),
                    visuals.widgets.hovered.bg_fill,
                );
            }
            let cross_color = if close_hovered {
                visuals.widgets.hovered.fg_stroke.color
            } else {
                visuals.widgets.noninteractive.fg_stroke.color
            };
            let r = 4.5;
            let cross_stroke = egui::Stroke::new(1.5, cross_color);
            ui.painter().line_segment(
                [
                    close_rect.center() + egui::vec2(-r, -r),
                    close_rect.center() + egui::vec2(r, r),
                ],
                cross_stroke,
            );
            ui.painter().line_segment(
                [
                    close_rect.center() + egui::vec2(-r, r),
                    close_rect.center() + egui::vec2(r, -r),
                ],
                cross_stroke,
            );
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
