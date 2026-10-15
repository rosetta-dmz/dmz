use eframe::egui;

pub(super) fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    accent: egui::Color32,
    muted: egui::Color32,
    text: egui::Color32,
) -> egui::Response {
    let fill = if selected {
        accent.gamma_multiply(0.16)
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if selected {
        egui::Stroke::new(1.0, accent.gamma_multiply(0.65))
    } else {
        egui::Stroke::new(1.0, egui::Color32::TRANSPARENT)
    };
    let text_color = if selected { text } else { muted };

    ui.add_sized(
        egui::vec2(ui.available_width(), 38.0),
        egui::Button::new(egui::RichText::new(label).color(text_color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(10.0)
            .min_size(egui::vec2(0.0, 38.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn section(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    let visuals = &ui.visuals();
    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, contents);
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
    egui::Frame::new()
        .fill(if selected {
            visuals.selection.bg_fill.gamma_multiply(0.2)
        } else {
            visuals.extreme_bg_color
        })
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .show(ui, contents)
}

pub(super) fn action_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let visuals = ui.visuals();
    let accent = visuals.selection.stroke.color;
    let button = egui::Button::new(if primary {
        egui::RichText::new(label).color(accent)
    } else {
        egui::RichText::new(label).color(visuals.widgets.inactive.fg_stroke.color)
    })
    .min_size(egui::vec2(0.0, 30.0))
    .corner_radius(egui::CornerRadius::same(8));
    if primary {
        ui.add(
            button
                .fill(visuals.selection.bg_fill)
                .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.45))),
        )
    } else {
        ui.add(
            button
                .fill(visuals.extreme_bg_color)
                .stroke(egui::Stroke::new(
                    1.0,
                    visuals.widgets.noninteractive.bg_stroke.color,
                )),
        )
    }
}

pub(super) fn toggle(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    let size = egui::vec2(38.0, 22.0);
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
        .rect_filled(rect, egui::CornerRadius::same(11), track);
    let knob_x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), animation);
    ui.painter().circle_filled(
        egui::pos2(knob_x, rect.center().y),
        8.0,
        egui::Color32::WHITE,
    );
    response
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
    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 20.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(5), color);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        egui::CornerRadius::same(5),
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}
