//! Material 3 building blocks drawn with egui: switch, buttons, segmented control, cards, settings rows.

use super::theme::{icons, semibold, Palette};
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Rect, Response, RichText, Sense, Stroke, Ui, Vec2};

pub fn icon_text(glyph: &str, size: f32, color: Color32) -> RichText { RichText::new(glyph).family(icons()).size(size).color(color) }

/// M3 switch (52×32 track, thumb grows when on).
pub fn switch(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(52.0, 32.0), Sense::click());
    if response.clicked() { *on = !*on; response.mark_changed(); }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.15);
    let painter = ui.painter();
    let track = lerp_color(p.highest, p.primary, t);
    painter.rect(rect, CornerRadius::same(16), track, if t < 0.5 { Stroke::new(2.0, p.outline) } else { Stroke::NONE }, egui::StrokeKind::Inside);
    let radius = egui::lerp(8.0..=12.0, t) + if response.hovered() { 1.0 } else { 0.0 };
    let x = egui::lerp((rect.left() + 16.0)..=(rect.right() - 16.0), t);
    painter.circle_filled(egui::pos2(x, rect.center().y), radius, lerp_color(p.outline, p.on_primary, t));
    if t > 0.5 { painter.text(egui::pos2(x, rect.center().y), Align2::CENTER_CENTER, super::theme::icon::CHECK, FontId::new(11.0, icons()), p.primary.gamma_multiply(t)); }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), f(a.a(), b.a()))
}

pub enum Kind { Filled, Tonal, Text, Outlined, Danger }
pub fn button(ui: &mut Ui, p: &Palette, kind: Kind, glyph: Option<&str>, label: &str) -> Response {
    let (fill, fg, stroke) = match kind {
        Kind::Filled => (p.primary, p.on_primary, Stroke::NONE),
        Kind::Tonal => (p.secondary_container, p.on_secondary_container, Stroke::NONE),
        Kind::Text => (Color32::TRANSPARENT, p.primary, Stroke::NONE),
        Kind::Outlined => (Color32::TRANSPARENT, p.primary, Stroke::new(1.0, p.outline)),
        Kind::Danger => (p.error_container, p.on_error_container, Stroke::NONE),
    };
    let font = FontId::new(14.0, semibold());
    let label_galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), fg);
    let icon_w = if glyph.is_some() { 24.0 } else { 0.0 };
    let size = Vec2::new(label_galley.size().x + icon_w + 40.0, 40.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hover = if response.hovered() { if response.is_pointer_button_down_on() { 0.16 } else { 0.08 } } else { 0.0 };
    let base = if fill == Color32::TRANSPARENT { Color32::TRANSPARENT } else { fill };
    ui.painter().rect(rect, CornerRadius::same(20), base, stroke, egui::StrokeKind::Inside);
    if hover > 0.0 { ui.painter().rect_filled(rect, CornerRadius::same(20), fg.gamma_multiply(hover)); }
    let mut x = rect.left() + 20.0;
    if let Some(g) = glyph { ui.painter().text(egui::pos2(x + 8.0, rect.center().y), Align2::CENTER_CENTER, g, FontId::new(16.0, icons()), fg); x += icon_w; }
    ui.painter().galley(egui::pos2(x, rect.center().y - label_galley.size().y / 2.0), label_galley, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Round icon button (40 px), optional tonal fill.
pub fn icon_button(ui: &mut Ui, p: &Palette, glyph: &str, tooltip: &str, tonal: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
    let fg = if tonal { p.on_secondary_container } else { p.muted };
    if tonal { ui.painter().circle_filled(rect.center(), 20.0, p.secondary_container); }
    if response.hovered() { ui.painter().circle_filled(rect.center(), 20.0, fg.gamma_multiply(0.10)); }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::new(16.0, icons()), fg);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) }
}

/// Connected-button segmented control. Returns true when the value changed.
pub fn segmented(ui: &mut Ui, p: &Palette, value: &mut String, options: &[(&str, &str)], width: f32) -> bool {
    let mut changed = false;
    let h = 36.0; let each = width / options.len() as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(18), Color32::TRANSPARENT, Stroke::new(1.0, p.outline), egui::StrokeKind::Inside);
    for (i, (key, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(egui::pos2(rect.left() + each * i as f32, rect.top()), Vec2::new(each, h));
        let id = ui.id().with(("seg", i, *key));
        let response = ui.interact(r, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let selected = value == key;
        let radius = CornerRadius { nw: if i == 0 { 18 } else { 0 }, sw: if i == 0 { 18 } else { 0 }, ne: if i + 1 == options.len() { 18 } else { 0 }, se: if i + 1 == options.len() { 18 } else { 0 } };
        if selected { ui.painter().rect_filled(r.shrink(1.0), radius, p.secondary_container); }
        else if response.hovered() { ui.painter().rect_filled(r.shrink(1.0), radius, p.text.gamma_multiply(0.06)); }
        if i > 0 { ui.painter().vline(r.left(), r.y_range(), Stroke::new(1.0, p.outline)); }
        let fg = if selected { p.on_secondary_container } else { p.text };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, FontId::new(13.5, semibold()), fg);
        if response.clicked() && !selected { *value = key.to_string(); changed = true; }
    }
    changed
}

/// Segmented look, but every segment toggles on its own (multi-select). Returns true when something changed.
pub fn toggles(ui: &mut Ui, p: &Palette, values: &mut [bool], labels: &[&str], width: f32) -> bool {
    let mut changed = false;
    let h = 36.0; let each = width / labels.len() as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(18), Color32::TRANSPARENT, Stroke::new(1.0, p.outline), egui::StrokeKind::Inside);
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(egui::pos2(rect.left() + each * i as f32, rect.top()), Vec2::new(each, h));
        let response = ui.interact(r, ui.id().with(("toggle", i, *label)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let last = i + 1 == labels.len();
        let radius = CornerRadius { nw: if i == 0 { 18 } else { 0 }, sw: if i == 0 { 18 } else { 0 }, ne: if last { 18 } else { 0 }, se: if last { 18 } else { 0 } };
        if values[i] { ui.painter().rect_filled(r.shrink(1.0), radius, p.secondary_container); }
        else if response.hovered() { ui.painter().rect_filled(r.shrink(1.0), radius, p.text.gamma_multiply(0.06)); }
        if i > 0 { ui.painter().vline(r.left(), r.y_range(), Stroke::new(1.0, p.outline)); }
        let fg = if values[i] { p.on_secondary_container } else { p.text };
        let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::new(13.5, semibold()), fg);
        let icon_w = if values[i] { 18.0 } else { 0.0 };
        let start = r.center().x - (galley.size().x + icon_w) / 2.0;
        if values[i] { ui.painter().text(egui::pos2(start + 7.0, r.center().y), Align2::CENTER_CENTER, super::theme::icon::CHECK, FontId::new(12.0, icons()), fg); }
        ui.painter().galley(egui::pos2(start + icon_w, r.center().y - galley.size().y / 2.0), galley, fg);
        if response.clicked() { values[i] = !values[i]; changed = true; }
    }
    changed
}

pub fn chip(ui: &mut Ui, p: &Palette, text: &str, fill: Color32, fg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(12.5, semibold()), fg);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    let _ = p; response
}

pub fn card(p: &Palette, fill: Color32) -> Frame { let _ = p; Frame::new().fill(fill).corner_radius(CornerRadius::same(24)).inner_margin(Margin::same(20)) }

pub fn section_title(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).font(FontId::new(13.5, semibold())).color(p.primary));
    ui.add_space(2.0);
}

/// Grouped settings rows inside one rounded surface.
pub fn group(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    Frame::new().fill(p.low).corner_radius(CornerRadius::same(20)).inner_margin(Margin::symmetric(4, 4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui);
    });
}

/// One settings row: title/subtitle left, control right. The whole row toggles a switch when clicked.
pub fn row(ui: &mut Ui, p: &Palette, title: &str, subtitle: &str, control: impl FnOnce(&mut Ui)) {
    Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let control_w = 230.0_f32.min(ui.available_width() * 0.45);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - control_w - 12.0).max(120.0));
                ui.label(RichText::new(title).size(14.5).color(p.text));
                if !subtitle.is_empty() { ui.label(RichText::new(subtitle).size(12.5).color(p.muted)); }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| control(ui));
        });
    });
}
pub fn switch_row(ui: &mut Ui, p: &Palette, title: &str, subtitle: &str, value: &mut bool) -> bool {
    let mut changed = false;
    let before = *value;
    let top = ui.cursor().top();
    row(ui, p, title, subtitle, |ui| { if switch(ui, p, value).changed() { changed = true; } });
    let rect = Rect::from_min_max(egui::pos2(ui.min_rect().left(), top), egui::pos2(ui.min_rect().right(), ui.cursor().top()));
    let r = ui.interact(rect, ui.id().with(("row", title)), Sense::click());
    if !changed && r.clicked() { *value = !before; changed = true; }
    if r.hovered() { ui.painter().rect_filled(rect, CornerRadius::same(16), p.text.gamma_multiply(0.03)); }
    changed
}
pub fn divider(ui: &mut Ui, p: &Palette) {
    let r = ui.available_rect_before_wrap();
    ui.painter().hline((r.left() + 16.0)..=(r.right() - 16.0), r.top(), Stroke::new(1.0, p.outline_variant.gamma_multiply(0.6)));
}

/// Small dropdown that shows the label of the current value.
pub fn picker(ui: &mut Ui, id: &str, value: &mut String, options: &[(&str, &str)]) -> bool {
    let mut changed = false;
    let current = options.iter().find(|(k, _)| *k == value.as_str()).map(|(_, l)| *l).unwrap_or(value.as_str()).to_string();
    egui::ComboBox::from_id_salt(id).selected_text(current).width(190.0).show_ui(ui, |ui| {
        for (k, l) in options { if ui.selectable_label(value == k, *l).clicked() && value != k { *value = k.to_string(); changed = true; } }
    });
    changed
}

/// Two-letter badge for a server: country code from a flag emoji, else initials.
pub fn badge_text(name: &str) -> String {
    let chars: Vec<char> = name.trim().chars().collect();
    if chars.len() >= 2 {
        let (a, b) = (chars[0] as u32, chars[1] as u32);
        if (0x1F1E6..=0x1F1FF).contains(&a) && (0x1F1E6..=0x1F1FF).contains(&b) {
            return [a, b].iter().map(|c| char::from_u32(c - 0x1F1E6 + 'A' as u32).unwrap_or('?')).collect();
        }
    }
    // Any other first emoji is shown as it is (egui's emoji font draws it).
    if let Some(e) = chars.iter().find(|c| { let u = **c as u32; (0x1F300..=0x1FAFF).contains(&u) || (0x2600..=0x27BF).contains(&u) }) { return e.to_string(); }
    let clean: String = name.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    let mut words = clean.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let second = words.next().and_then(|w| w.chars().next());
    match (first, second) { (Some(a), Some(b)) => format!("{}{}", a.to_uppercase(), b.to_uppercase()), (Some(a), None) => a.to_uppercase().to_string(), _ => "•".into() }
}
/// Display name without a leading flag emoji (Windows has no flag glyphs).
pub fn display_name(name: &str) -> String {
    let chars: Vec<char> = name.trim().chars().collect();
    if chars.len() >= 2 && (0x1F1E6..=0x1F1FF).contains(&(chars[0] as u32)) && (0x1F1E6..=0x1F1FF).contains(&(chars[1] as u32)) {
        return chars[2..].iter().collect::<String>().trim().to_string();
    }
    name.trim().to_string()
}

pub fn proportional(size: f32) -> FontId { FontId::new(size, FontFamily::Proportional) }
