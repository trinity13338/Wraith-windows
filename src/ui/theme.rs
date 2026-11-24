//! Wraith's palette, as in the Android app: a dark base, translucent "glass" surfaces and one accent
//! colour (presets, the colour of your photo, or the Windows accent). Segoe UI from the Windows fonts folder.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub bg: Color32, pub glass: Color32, pub glass_high: Color32, pub stroke: Color32,
    pub text: Color32, pub muted: Color32, pub faint: Color32,
    pub accent: Color32, pub on_accent: Color32, pub accent_soft: Color32,
    pub good: Color32, pub warn: Color32, pub bad: Color32,
    /// Opaque surfaces for menus, text fields and dialogs.
    pub surface: Color32, pub surface_high: Color32, pub surface_highest: Color32, pub outline: Color32,
}

const fn c(hex: u32) -> Color32 { Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8) }
fn argb(hex: u32) -> Color32 { Color32::from_rgba_unmultiplied((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, (hex >> 24) as u8) }

/// Accent presets (key, colour); "photo" and "system" are resolved at runtime.
pub const ACCENTS: &[(&str, Color32)] = &[
    ("sakura", c(0xF2A7D8)), ("lavender", c(0xB8A6FF)), ("mint", c(0x8BE6C6)),
    ("ice", c(0x8FD0FF)), ("amber", c(0xFFC978)), ("coral", c(0xFF9E8A)),
];
pub fn preset(key: &str) -> Option<Color32> { ACCENTS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c) }
pub fn accent_name(key: &str) -> &'static str {
    match key {
        "sakura" => crate::tr!("Сакура", "Sakura"), "lavender" => crate::tr!("Лаванда", "Lavender"), "mint" => crate::tr!("Мята", "Mint"),
        "ice" => crate::tr!("Лёд", "Ice"), "amber" => crate::tr!("Янтарь", "Amber"), "coral" => crate::tr!("Коралл", "Coral"),
        "photo" => crate::tr!("Из фото", "From photo"), _ => crate::tr!("Система", "System"),
    }
}

pub fn luminance(c: Color32) -> f32 {
    let f = |v: u8| { let v = v as f32 / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    Color32::from_rgba_unmultiplied(f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3]))
}
/// The colour with a new opacity (0..1), whatever its alpha was.
pub fn alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub fn palette(accent: Color32, amoled: bool) -> Palette {
    // Keep any accent readable on the dark base: very dark picks are lifted towards white.
    let a = if luminance(accent) < 0.25 { lerp(accent, Color32::WHITE, 0.45) } else { accent };
    let on = if luminance(a) > 0.45 { lerp(a, Color32::BLACK, 0.82) } else { Color32::WHITE };
    Palette {
        bg: if amoled { Color32::BLACK } else { c(0x0B0A10) },
        glass: argb(0xA6141219), glass_high: argb(0xCC1E1B26), stroke: argb(0x1AFFFFFF),
        text: c(0xF5F2FA), muted: c(0xAAA5B8), faint: c(0x6E6A7A),
        accent: a, on_accent: on, accent_soft: alpha(a, 0.18),
        good: c(0x7BE0A6), warn: c(0xFFCF7A), bad: c(0xFF8F8F),
        surface: c(0x16141C), surface_high: c(0x1E1B26), surface_highest: c(0x26222F), outline: c(0x4A4655),
    }
}

/// The Windows accent colour (Settings → Personalisation → Colours), read at most every two seconds
/// so a change shows up without a restart. The DWORD is 0xAABBGGRR.
pub fn system_accent() -> Option<Color32> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(std::time::Instant, Option<Color32>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((at, v)) = *c { if at.elapsed() < std::time::Duration::from_secs(2) { return v; } }
    use winreg::enums::HKEY_CURRENT_USER;
    let v = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\DWM").and_then(|k| k.get_value::<u32, _>("AccentColor")).ok()
        .map(|v| Color32::from_rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8));
    *c = Some((std::time::Instant::now(), v));
    v
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = Visuals::dark();
    v.panel_fill = p.bg; v.window_fill = p.surface; v.extreme_bg_color = p.surface_highest; v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface; v.override_text_color = Some(p.text);
    v.window_corner_radius = CornerRadius::same(28); v.menu_corner_radius = CornerRadius::same(16);
    v.window_stroke = Stroke::new(1.0_f32, p.stroke);
    v.window_shadow = egui::epaint::Shadow { offset: [0, 10], blur: 36, spread: 0, color: Color32::from_black_alpha(170) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(150) };
    v.selection.bg_fill = alpha(p.accent, 0.35); v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.hyperlink_color = p.accent; v.error_fg_color = p.bad; v.warn_fg_color = p.warn;
    let r = CornerRadius::same(14);
    for (w, fill) in [(&mut v.widgets.noninteractive, p.surface), (&mut v.widgets.inactive, p.surface_high), (&mut v.widgets.hovered, p.surface_highest), (&mut v.widgets.active, p.surface_highest), (&mut v.widgets.open, p.surface_highest)] {
        w.corner_radius = r; w.bg_fill = fill; w.weak_bg_fill = fill; w.bg_stroke = Stroke::NONE; w.fg_stroke = Stroke::new(1.0_f32, p.text); w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.stroke);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, c(0x3A3644));
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.outline);
    v.widgets.active.bg_stroke = Stroke::new(1.5_f32, p.accent);
    v.text_cursor.stroke = Stroke::new(2.0_f32, p.accent);
    v.text_edit_bg_color = Some(p.surface_high);
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0); s.spacing.button_padding = egui::vec2(16.0, 9.0); s.spacing.interact_size.y = 36.0;
        s.spacing.scroll = egui::style::ScrollStyle::floating(); s.spacing.menu_margin = egui::Margin::same(8);
        s.text_styles.insert(egui::TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Button, FontId::new(14.5, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Small, FontId::new(12.0, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(30.0, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace));
        s.interaction.selectable_labels = false;
    });
}

pub fn medium() -> FontFamily { FontFamily::Name("semibold".into()) }
pub fn icons() -> FontFamily { FontFamily::Name("icons".into()) }

/// Segoe UI (+ Semibold) and Cascadia Mono from the Windows fonts folder; Segoe UI Symbol for arrows and stars.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let mut load = |name: &str, files: &[&str]| -> bool {
        for f in files { if let Ok(bytes) = std::fs::read(dir.join(f)) { fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes))); return true; } }
        false
    };
    let ui = load("ui", &["segoeui.ttf"]);
    let sb = load("ui-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let mono = load("mono", &["CascadiaMono.ttf", "consola.ttf"]);
    let sym = load("symbols", &["seguisym.ttf"]);
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    if ui { prop.insert(0, "ui".into()); }
    if sym { prop.insert(if ui { 1 } else { 0 }, "symbols".into()); }
    let mut semibold = vec![]; if sb { semibold.push("ui-semibold".to_string()); } if ui { semibold.push("ui".into()); }
    semibold.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(medium(), semibold);
    let icon: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    fonts.families.insert(icons(), icon);
    if mono { fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into()); }
    ctx.set_fonts(fonts);
}

/// The palette before the store is loaded (installer, message boxes): the default accent, or the saved one.
pub fn default_palette() -> Palette {
    let key = crate::store::try_setting("accent");
    palette(preset(&key).unwrap_or(ACCENTS[0].1), false)
}
