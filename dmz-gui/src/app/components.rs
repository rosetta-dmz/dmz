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
        egui::vec2(ui.available_width(), 40.0),
        egui::Button::new(egui::RichText::new(label).color(text_color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(10.0)
            .min_size(egui::vec2(0.0, 40.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
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
            ui.set_min_width((card_width - 34.0).max(0.0));
            ui.set_min_height(96.0);
            contents(ui);
        });
}

pub(super) fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let muted = ui.visuals().widgets.noninteractive.fg_stroke.color;
    ui.label(
        egui::RichText::new(title)
            .font(egui::FontId::proportional(22.0))
            .strong(),
    );
    ui.add_space(8.0);
    ui.label(egui::RichText::new(subtitle).color(muted).size(13.0));
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
            ui.set_min_width((card_width - 34.0).max(0.0));
            ui.set_min_height(64.0);
            contents(ui);
        })
}

pub(super) fn action_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let visuals = ui.visuals();
    let accent = visuals.selection.stroke.color;
    let button = egui::Button::new(if primary {
        egui::RichText::new(label).color(accent)
    } else {
        egui::RichText::new(label).color(visuals.widgets.inactive.fg_stroke.color)
    })
    .min_size(egui::vec2(0.0, 32.0))
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
