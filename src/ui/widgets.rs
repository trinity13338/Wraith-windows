//! The Android app's building blocks drawn with egui: glass surfaces, chips, setting rows, switch,
//! pills, flag badges, the connect pill with running packets, the floating navigation, charts.

use super::icons::{self, Ic};
use super::theme::{alpha, lerp, medium, Palette};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Rect, Response, Sense, Stroke, Ui, Vec2};
use std::sync::Arc;

pub fn font(size: f32) -> FontId { FontId::new(size, FontFamily::Proportional) }
pub fn font_m(size: f32) -> FontId { FontId::new(size, medium()) }

/// Translucent dark surface with a hairline edge: readable over any photo.
pub fn glass(p: &Palette, radius: u8) -> Frame { Frame::new().fill(p.glass).stroke(Stroke::new(1.0_f32, p.stroke)).corner_radius(CornerRadius::same(radius)) }

/// One line of text, cut with an ellipsis at `max_w`.
pub fn line(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}
/// Wrapped text, at most `rows` lines.
pub fn wrapped(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32, rows: usize) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, max_w.max(10.0));
    job.wrap.max_rows = rows; job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}
/// Text with extra letter spacing (the "W R A I T H" wordmark).
pub fn spaced(ui: &Ui, text: &str, font: FontId, color: Color32, spacing: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    ui.painter().layout_job(job)
}

pub fn latency_color(p: &Palette, ms: i64) -> Color32 { if ms < 0 { p.bad } else if ms < 150 { p.good } else if ms < 400 { p.warn } else { p.bad } }

/// Status chip of the home screen: optional icon or live dot, then one line of text that never wraps.
pub fn chip(ui: &mut Ui, p: &Palette, text: &str, icon: Option<Ic>, tint: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 38.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(14), p.glass, Stroke::new(1.0_f32, p.stroke), egui::StrokeKind::Inside);
    // Icon and text stay centred in the cell whatever the numbers are.
    let icon_w = if icon.is_some() { 23.0 } else { 0.0 };
    let g = line(ui, text, font_m(14.5), tint, rect.width() - 20.0 - icon_w);
    let x = rect.center().x - (icon_w + g.size().x) / 2.0;
    if let Some(i) = icon { icons::at(painter, pos2(x + 8.0, rect.center().y), 16.0, i, p.muted); }
    ui.painter().galley(pos2(x + icon_w, rect.center().y - g.size().y / 2.0), g, tint);
}

pub fn section_title(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(22.0);
    ui.horizontal(|ui| { ui.add_space(6.0); ui.label(egui::RichText::new(text).font(font_m(13.0)).color(p.accent)); });
    ui.add_space(4.0);
}

/// A group of rows on one glass card with hairline separators.
pub fn group<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    glass(p, 26).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui)
    }).inner
}
pub fn divider(ui: &mut Ui, p: &Palette) {
    let r = ui.available_rect_before_wrap();
    ui.painter().hline((r.left() + 68.0)..=r.right(), r.top(), Stroke::new(1.0_f32, p.stroke));
}

pub enum Trailing<'a> { None, Chevron, Text(&'a str), Switch(bool), Icon(Ic), Custom(&'a dyn Fn(&mut Ui)) }

/// One line of a settings group: icon tile, title and subtitle, then a value, switch or chevron.
pub fn setting_row(ui: &mut Ui, p: &Palette, title: &str, subtitle: &str, icon: Option<Ic>, danger: bool, trailing: Trailing, clickable: bool) -> Response {
    let width = ui.available_width();
    let left = 18.0 + if icon.is_some() { 50.0 } else { 0.0 };
    let trail_w = match &trailing {
        Trailing::None => 0.0, Trailing::Chevron | Trailing::Icon(_) => 30.0, Trailing::Switch(_) => 64.0,
        Trailing::Text(t) => (ui.painter().layout_no_wrap(t.to_string(), font(14.0), p.muted).size().x + 14.0).min(width * 0.45),
        Trailing::Custom(_) => 150.0,
    };
    let text_w = (width - left - 18.0 - trail_w).max(60.0);
    let tg = wrapped(ui, title, font(16.0), if danger { p.bad } else { p.text }, text_w, 2);
    let sg = (!subtitle.is_empty()).then(|| wrapped(ui, subtitle, font(13.5), p.muted, text_w, 4));
    let text_h = tg.size().y + sg.as_ref().map(|g| g.size().y + 2.0).unwrap_or(0.0);
    let h = (text_h + 24.0).max(60.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, h), if clickable { Sense::click() } else { Sense::hover() });
    let response = if clickable { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response };
    if clickable && response.hovered() { ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::same(24), alpha(p.text, 0.035)); }
    if let Some(i) = icon {
        let tile = Rect::from_min_size(pos2(rect.left() + 18.0, rect.center().y - 18.0), vec2(36.0, 36.0));
        ui.painter().rect_filled(tile, CornerRadius::same(12), if danger { alpha(p.bad, 0.14) } else { p.accent_soft });
        icons::at(ui.painter(), tile.center(), 20.0, i, if danger { p.bad } else { p.accent });
    }
    let mut y = rect.center().y - text_h / 2.0;
    let th = tg.size().y;
    ui.painter().galley(pos2(rect.left() + left, y), tg, p.text); y += th + 2.0;
    if let Some(g) = sg { ui.painter().galley(pos2(rect.left() + left, y), g, p.muted); }
    let right = rect.right() - 18.0;
    match trailing {
        Trailing::None => {}
        Trailing::Chevron => icons::at(ui.painter(), pos2(right - 9.0, rect.center().y), 18.0, Ic::Chevron, p.faint),
        Trailing::Icon(i) => icons::at(ui.painter(), pos2(right - 9.0, rect.center().y), 18.0, i, p.faint),
        Trailing::Text(t) => { let g = line(ui, t, font(14.0), p.muted, trail_w - 8.0); ui.painter().galley(pos2(right - g.size().x, rect.center().y - g.size().y / 2.0), g, p.muted); }
        Trailing::Switch(on) => { let r = Rect::from_min_size(pos2(right - 52.0, rect.center().y - 16.0), vec2(52.0, 32.0)); paint_switch(ui, p, r, on, response.id); }
        Trailing::Custom(f) => {
            let r = Rect::from_min_max(pos2(right - trail_w, rect.top()), pos2(right, rect.bottom()));
            ui.scope_builder(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| f(ui));
        }
    }
    response
}
pub fn switch_row(ui: &mut Ui, p: &Palette, title: &str, subtitle: &str, icon: Option<Ic>, on: bool) -> bool {
    setting_row(ui, p, title, subtitle, icon, false, Trailing::Switch(on), true).clicked()
}

/// M3 switch in the accent colour (track 52×32, the thumb grows when on).
pub fn paint_switch(ui: &Ui, p: &Palette, rect: Rect, on: bool, id: egui::Id) {
    let t = ui.ctx().animate_bool_with_time(id.with("sw"), on, 0.15);
    let painter = ui.painter();
    let track = lerp(p.surface_highest, p.accent, t);
    painter.rect(rect, CornerRadius::same(16), track, if t < 0.5 { Stroke::new(2.0_f32, p.outline) } else { Stroke::NONE }, egui::StrokeKind::Inside);
    let radius = egui::lerp(8.0..=12.0, t);
    let x = egui::lerp((rect.left() + 16.0)..=(rect.right() - 16.0), t);
    painter.circle_filled(pos2(x, rect.center().y), radius, lerp(p.outline, p.on_accent, t));
}

/// Round icon button on glass (44 px by default).
pub fn icon_btn(ui: &mut Ui, p: &Palette, icon: Ic, tooltip: &str, size: f32, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let (bg, fg) = if active { (p.accent, p.on_accent) } else { (p.glass, p.text) };
    let painter = ui.painter();
    painter.circle(rect.center(), size / 2.0 - 0.5, bg, Stroke::new(1.0_f32, if active { Color32::TRANSPARENT } else { p.stroke }));
    if response.hovered() { painter.circle_filled(rect.center(), size / 2.0 - 1.0, alpha(fg, 0.08)); }
    icons::at(painter, rect.center(), size * 0.48, icon, fg);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) }
}

/// Pill button: filled with the accent, or glass. `width` 0 = fit the text.
pub fn pill(ui: &mut Ui, p: &Palette, text: &str, icon: Option<Ic>, filled: bool, width: f32) -> Response {
    let fg = if filled { p.on_accent } else { p.text };
    let g = ui.painter().layout_no_wrap(text.to_string(), font_m(15.0), fg);
    let content = g.size().x + if icon.is_some() { 28.0 } else { 0.0 };
    let w = if width > 0.0 { width } else { content + 44.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(w, 50.0), Sense::click());
    let pressed = response.is_pointer_button_down_on();
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    ui.painter().rect(rect, CornerRadius::same(25), if filled { p.accent } else { p.glass }, Stroke::new(1.0_f32, if filled { Color32::TRANSPARENT } else { p.stroke }), egui::StrokeKind::Inside);
    if response.hovered() { ui.painter().rect_filled(rect, CornerRadius::same(25), alpha(fg, 0.07)); }
    let mut x = rect.center().x - content / 2.0;
    if let Some(i) = icon { icons::at(ui.painter(), pos2(x + 10.0, rect.center().y), 19.0, i, fg); x += 28.0; }
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Android's segmented choice: a glass pill, the current option filled with the accent.
pub fn segmented(ui: &mut Ui, p: &Palette, values: &[&str], labels: &[&str], current: &str) -> Option<String> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 52.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(26), p.glass, Stroke::new(1.0_f32, p.stroke), egui::StrokeKind::Inside);
    let inner = rect.shrink(4.0); let each = inner.width() / values.len() as f32;
    let mut picked = None;
    for (i, v) in values.iter().enumerate() {
        let r = Rect::from_min_size(pos2(inner.left() + each * i as f32, inner.top()), vec2(each, inner.height()));
        let resp = ui.interact(r, ui.id().with(("seg", *v, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let on = *v == current;
        if on { ui.painter().rect_filled(r, CornerRadius::same(22), p.accent); }
        else if resp.hovered() { ui.painter().rect_filled(r, CornerRadius::same(22), alpha(p.text, 0.05)); }
        let g = line(ui, labels[i], font_m(15.0), if on { p.on_accent } else { p.text }, each - 12.0);
        ui.painter().galley(r.center() - g.size() / 2.0, g.clone(), if on { p.on_accent } else { p.text });
        if resp.clicked() && !on { picked = Some(v.to_string()); }
    }
    picked
}

/// Search field: a glass pill with a magnifier and a clear button.
pub fn search_field(ui: &mut Ui, p: &Palette, value: &mut String, hint: &str) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 52.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(26), p.glass, Stroke::new(1.0_f32, p.stroke), egui::StrokeKind::Inside);
    icons::at(ui.painter(), pos2(rect.left() + 28.0, rect.center().y), 20.0, Ic::Search, p.muted);
    let clear_w = if value.is_empty() { 0.0 } else { 40.0 };
    let field = Rect::from_min_max(pos2(rect.left() + 50.0, rect.top() + 6.0), pos2(rect.right() - 16.0 - clear_w, rect.bottom() - 6.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
        ui.visuals_mut().extreme_bg_color = Color32::TRANSPARENT; ui.visuals_mut().text_edit_bg_color = Some(Color32::TRANSPARENT);
        ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE; ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE; ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
        ui.add_sized(field.size(), egui::TextEdit::singleline(value).frame(false).font(font(16.0)).vertical_align(egui::Align::Center)
            .hint_text(egui::RichText::new(hint).color(p.faint).font(font(16.0))).text_color(p.text));
    });
    if !value.is_empty() {
        let c = pos2(rect.right() - 34.0, rect.center().y);
        let r = ui.interact(Rect::from_center_size(c, Vec2::splat(32.0)), ui.id().with("clear-search"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        icons::at(ui.painter(), c, 20.0, Ic::Close, if r.hovered() { p.text } else { p.muted });
        if r.clicked() { value.clear(); }
    }
}

/// Round country flag, or the first emoji / letter of the name, on a rounded glass tile.
pub fn flag_badge(ui: &Ui, p: &Palette, rect: Rect, name: &str, selected: bool, favorite: bool) {
    let size = rect.width();
    let r = CornerRadius::same((size * 0.32) as u8);
    ui.painter().rect(rect, r, p.glass_high, Stroke::new(if selected { 2.0 } else { 1.0_f32 }, if selected { p.accent } else { p.stroke }), egui::StrokeKind::Inside);
    let painted = super::flags::flag_code(name).is_some_and(|code| super::flags::paint(ui.painter(), &code, rect.center(), size * 0.31, None));
    if !painted {
        let fallback = if favorite { "★".to_string() } else { badge_text(name) };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, fallback, FontId::new(size * 0.38, medium()), p.accent);
    }
}

/// First emoji (not a flag) or the first letter of a name.
pub fn badge_text(name: &str) -> String {
    let n = display_name(name);
    if let Some(e) = n.chars().find(|c| { let u = *c as u32; (0x1F300..=0x1FAFF).contains(&u) || (0x2600..=0x27BF).contains(&u) }) { return e.to_string(); }
    n.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "•".into())
}
/// Display name without a leading flag emoji (flags are drawn as images in the badge).
pub fn display_name(name: &str) -> String {
    let chars: Vec<char> = name.trim().chars().collect();
    if chars.len() >= 2 && (0x1F1E6..=0x1F1FF).contains(&(chars[0] as u32)) && (0x1F1E6..=0x1F1FF).contains(&(chars[1] as u32)) {
        return chars[2..].iter().collect::<String>().trim().to_string();
    }
    name.trim().to_string()
}

/// State of the connect pill's running packets, kept across frames.
#[derive(Clone, Copy, Default)]
struct Lanes { up: f32, down: f32, up_speed: f32, down_speed: f32, presence: f32, last: f64 }

/// The connect control: a wide pill. Off it says what it does. On, packets run through it in two
/// lanes (upload to the right on top, download to the left below), each as fast as its traffic.
/// While connecting the packets dash quickly and blink, as if knocking.
pub fn connect_button(ui: &mut Ui, p: &Palette, state: crate::core::State, rx: i64, tx: i64) -> Response {
    use crate::core::State;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 80.0), Sense::click());
    let on = state == State::Connected; let busy = matches!(state, State::Connecting | State::Stopping);
    let id = response.id;
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let mut l: Lanes = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let dt = if l.last == 0.0 { 0.0 } else { ((now - l.last) as f32).min(0.1) };
    l.last = now;
    let speed = |rate: i64| if busy { 240.0 } else { 38.0 + 46.0 * (1.0 + rate as f32 / 1024.0).log10().min(4.0) };
    let ease = (dt / 0.9).min(1.0);
    l.up_speed += (speed(tx) - l.up_speed) * ease; l.down_speed += (speed(rx) - l.down_speed) * ease;
    l.up += l.up_speed * dt; l.down += l.down_speed * dt;
    let target = if on || busy { 1.0 } else { 0.0 };
    l.presence += (target - l.presence) * (dt / 0.5).min(1.0) * 3.0;
    l.presence = l.presence.clamp(0.0, 1.0);
    ctx.data_mut(|d| d.insert_temp(id, l));

    let fill_t = ctx.animate_value_with_time(id.with("fill"), if on { 1.0 } else if busy { 0.55 } else { 0.0 }, 0.45);
    let fill = if fill_t <= 0.0 { p.glass_high } else { lerp(p.glass_high, p.accent, fill_t) };
    let ink = lerp(p.text, p.on_accent, ctx.animate_bool_with_time(id.with("ink"), on || busy, 0.45));
    let pressed = response.is_pointer_button_down_on();
    let rect = if pressed { rect.shrink2(vec2(rect.width() * 0.0175, 1.4)) } else { rect };
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect(rect, CornerRadius::same(40), fill, Stroke::new(1.0_f32, if on || busy { Color32::TRANSPARENT } else { p.stroke }), egui::StrokeKind::Inside);
    if response.hovered() && !pressed { painter.rect_filled(rect, CornerRadius::same(40), alpha(ink, 0.05)); }

    if l.presence > 0.01 {
        let area = rect.shrink2(vec2(26.0, 0.0));
        let blink = if busy { 0.45 + 0.55 * (0.5 + 0.5 * ((now as f32) * std::f32::consts::TAU / 0.84).sin()) } else { 1.0 };
        lane(&painter, area, area.center().y - 9.0, l.up, true, blink * 0.75 * l.presence, 0, ink);
        lane(&painter, area, area.center().y + 9.0, l.down, false, blink * l.presence, 3, ink);
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    if l.presence < 0.3 {
        let a = 1.0 - l.presence / 0.3;
        let g = ui.painter().layout_no_wrap(crate::tr!("Подключиться", "Connect").into(), font_m(21.0), alpha(ink, a));
        let total = 24.0 + 12.0 + g.size().x;
        let x = rect.center().x - total / 2.0;
        icons::at(&painter, pos2(x + 12.0, rect.center().y), 24.0, Ic::Power, alpha(p.accent, a));
        painter.galley(pos2(x + 36.0, rect.center().y - g.size().y / 2.0), g, ink);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One repeating strip of packets of different sizes with gaps, sliding by `offset`; comets with a fading tail.
fn lane(painter: &egui::Painter, area: Rect, cy: f32, offset: f32, rightwards: bool, a: f32, seed: usize, ink: Color32) {
    const LENGTHS: [f32; 8] = [22.0, 38.0, 14.0, 28.0, 46.0, 18.0, 32.0, 12.0];
    let (h, fade, gap) = (6.0, 34.0, 9.0);
    let extra = |i: usize| ((i * 7 + seed * 3) % 5) as f32 * 6.0;
    let strip: f32 = (0..LENGTHS.len()).map(|i| LENGTHS[(i + seed) % 8] + gap + extra(i)).sum();
    let width = area.width();
    let mut start = -(offset % strip);
    while start < width {
        let mut x = start;
        for i in 0..LENGTHS.len() {
            let len = LENGTHS[(i + seed) % 8];
            let left = if rightwards { x } else { width - x - len };
            let mid = left + len / 2.0;
            let edge = ((mid.min(width - mid)) / fade).clamp(0.0, 1.0);
            if edge > 0.0 && left < width && left + len > 0.0 {
                let k = a * edge;
                comet(painter, Rect::from_min_size(pos2(area.left() + left, cy - h / 2.0), vec2(len, h)), ink, k, rightwards);
            }
            x += len + gap + extra(i);
        }
        start += strip;
    }
}
/// A rounded bar with a horizontal alpha gradient: solid head, transparent tail.
fn comet(painter: &egui::Painter, r: Rect, ink: Color32, k: f32, rightwards: bool) {
    let n = 10; let mut mesh = egui::Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = r.left() + r.width() * t;
        let head = if rightwards { t } else { 1.0 - t };
        let a = k * if head < 0.5 { head * 1.2 } else { 0.6 + (head - 0.5) * 0.8 };
        // Rounded ends: the strip narrows at both tips.
        let tip = ((t.min(1.0 - t) * r.width()) / (r.height() / 2.0)).min(1.0);
        let half = r.height() / 2.0 * (1.0 - (1.0 - tip).powi(2)).sqrt();
        let c = alpha(ink, a);
        mesh.colored_vertex(pos2(x, r.center().y - half), c); mesh.colored_vertex(pos2(x, r.center().y + half), c);
        if i > 0 { let b = (i as u32 - 1) * 2; mesh.add_triangle(b, b + 1, b + 2); mesh.add_triangle(b + 1, b + 3, b + 2); }
    }
    painter.add(mesh);
}

/// Floating navigation: a glass pill for the main sections and a separate round settings button.
pub fn floating_nav(ui: &mut Ui, p: &Palette, items: &[(Ic, &str)], selected: usize, settings: bool) -> Option<usize> {
    let mut picked = None;
    let total = ui.available_width();
    let pill = Rect::from_min_size(ui.cursor().min, vec2(total - 78.0, 66.0));
    let round = Rect::from_min_size(pos2(pill.right() + 12.0, pill.top()), vec2(66.0, 66.0));
    ui.allocate_rect(pill.union(round), Sense::hover());
    ui.painter().rect(pill, CornerRadius::same(33), p.glass_high, Stroke::new(1.0_f32, p.stroke), egui::StrokeKind::Inside);
    let inner = pill.shrink(7.0);
    // The active item is wider (1.6 : 1 : 1) and shows its label.
    let weights: Vec<f32> = (0..items.len()).map(|i| { let t = ui.ctx().animate_bool_with_time(ui.id().with(("nav", i)), i == selected && !settings, 0.3); 1.0 + 0.6 * t }).collect();
    let sum: f32 = weights.iter().sum();
    let mut x = inner.left();
    for (i, (icon, text)) in items.iter().enumerate() {
        let w = inner.width() * weights[i] / sum;
        let r = Rect::from_min_size(pos2(x, inner.top()), vec2(w, inner.height())); x += w;
        let t = (weights[i] - 1.0) / 0.6;
        let resp = ui.interact(r, ui.id().with(("nav-item", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if t > 0.01 { ui.painter().rect_filled(r, CornerRadius::same(26), alpha(p.accent, 0.18 * t)); }
        else if resp.hovered() { ui.painter().rect_filled(r, CornerRadius::same(26), alpha(p.text, 0.05)); }
        let g = ui.painter().layout_no_wrap(text.to_string(), font_m(15.0), alpha(p.text, t));
        let content = 22.0 + if t > 0.01 { 9.0 + g.size().x } else { 0.0 };
        let cx = if content > r.width() - 12.0 { r.center().x - 11.0 } else { r.center().x - content / 2.0 + 11.0 };
        icons::at(ui.painter(), pos2(cx, r.center().y), 22.0, *icon, lerp(p.muted, p.accent, t));
        if t > 0.01 && content <= r.width() - 12.0 { ui.painter().galley(pos2(cx + 20.0, r.center().y - g.size().y / 2.0), g, p.text); }
        if resp.clicked() { picked = Some(i); }
    }
    let resp = ui.interact(round, ui.id().with("nav-settings"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(crate::tr!("Настройки", "Settings"));
    ui.painter().circle(round.center(), 33.0, if settings { p.accent } else { p.glass_high }, Stroke::new(1.0_f32, p.stroke));
    if resp.hovered() && !settings { ui.painter().circle_filled(round.center(), 32.0, alpha(p.text, 0.05)); }
    icons::at(ui.painter(), round.center(), 24.0, Ic::Settings, if settings { p.on_accent } else { p.muted });
    if resp.clicked() { picked = Some(usize::MAX); }
    picked
}

/// Live speed: download as a filled area, upload as a line (the last 60 seconds).
pub fn speed_chart(ui: &mut Ui, p: &Palette, samples: &[(i64, i64)], height: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    for i in 1..=3 { let y = rect.top() + rect.height() * i as f32 / 4.0; painter.hline(rect.x_range(), y, Stroke::new(1.0_f32, p.stroke)); }
    if samples.len() < 2 { return rect; }
    let n = 60.0;
    let max = samples.iter().map(|s| s.0.max(s.1)).max().unwrap_or(0).max(16 * 1024) as f32 * 1.15;
    let step = rect.width() / (n - 1.0);
    let offset = (n - samples.len() as f32) * step;
    let y = |v: i64| rect.bottom() - (v as f32 / max) * rect.height();
    let down: Vec<egui::Pos2> = samples.iter().enumerate().map(|(i, s)| pos2(rect.left() + offset + i as f32 * step, y(s.0))).collect();
    let up: Vec<egui::Pos2> = samples.iter().enumerate().map(|(i, s)| pos2(rect.left() + offset + i as f32 * step, y(s.1))).collect();
    // Area under the download line, fading downwards.
    let mut mesh = egui::Mesh::default();
    for (i, pt) in down.iter().enumerate() {
        mesh.colored_vertex(*pt, alpha(p.accent, 0.35 * ((rect.bottom() - pt.y) / rect.height()).clamp(0.15, 1.0)));
        mesh.colored_vertex(pos2(pt.x, rect.bottom()), Color32::TRANSPARENT);
        if i > 0 { let b = (i as u32 - 1) * 2; mesh.add_triangle(b, b + 1, b + 2); mesh.add_triangle(b + 1, b + 3, b + 2); }
    }
    painter.add(mesh);
    painter.add(egui::Shape::line(down, Stroke::new(2.5_f32, p.accent)));
    painter.add(egui::Shape::line(up, Stroke::new(1.6_f32, alpha(p.text, 0.7))));
    rect
}

/// Daily totals as rounded bars, today in full accent.
pub fn day_bars(ui: &mut Ui, p: &Palette, days: &[(String, i64)], height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let max = days.iter().map(|d| d.1).max().unwrap_or(0).max(1) as f32;
    let slot = rect.width() / days.len().max(1) as f32; let bar = slot * 0.56;
    for (i, (_, v)) in days.iter().enumerate() {
        let h = (*v as f32 / max * rect.height()).max(4.0);
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * slot + (slot - bar) / 2.0, rect.bottom() - h), vec2(bar, h));
        let c = if *v > 0 { alpha(p.accent, if i + 1 == days.len() { 1.0 } else { 0.55 }) } else { p.glass_high };
        ui.painter().rect_filled(r, CornerRadius::same((bar / 2.0).min(255.0) as u8), c);
    }
    let (labels, _) = ui.allocate_exact_size(vec2(rect.width(), 16.0), Sense::hover());
    for (i, (d, _)) in days.iter().enumerate() {
        if i % 2 == 1 { ui.painter().text(pos2(labels.left() + i as f32 * slot + slot / 2.0, labels.center().y), Align2::CENTER_CENTER, d, font(10.5), p.faint); }
    }
}

