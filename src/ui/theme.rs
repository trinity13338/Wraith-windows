//! Material 3 palette (the same teal scheme as the Android app), Windows fonts and Fluent icons.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub primary: Color32, pub on_primary: Color32, pub primary_container: Color32, pub on_primary_container: Color32,
    pub secondary_container: Color32, pub on_secondary_container: Color32,
    pub tertiary: Color32, pub tertiary_container: Color32,
    pub error: Color32, pub error_container: Color32, pub on_error_container: Color32,
    pub surface: Color32, pub text: Color32, pub muted: Color32, pub outline: Color32, pub outline_variant: Color32,
    pub lowest: Color32, pub low: Color32, pub container: Color32, pub high: Color32, pub highest: Color32,
    pub good: Color32, pub warn: Color32,
}

const fn c(hex: u32) -> Color32 { Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8) }

pub const LIGHT: Palette = Palette {
    dark: false,
    primary: c(0x006B5B), on_primary: c(0xFFFFFF), primary_container: c(0x9EF2DC), on_primary_container: c(0x00201A),
    secondary_container: c(0xCDE8DF), on_secondary_container: c(0x06201A),
    tertiary: c(0x426278), tertiary_container: c(0xC8E6FF),
    error: c(0xBA1A1A), error_container: c(0xFFDAD6), on_error_container: c(0x410002),
    surface: c(0xF5FBF7), text: c(0x171D1B), muted: c(0x3F4945), outline: c(0x6F7975), outline_variant: c(0xBFC9C4),
    lowest: c(0xFFFFFF), low: c(0xEFF5F1), container: c(0xE9EFEB), high: c(0xE3EAE6), highest: c(0xDEE4E0),
    good: c(0x1E7D4A), warn: c(0x9A6400),
};
pub const DARK: Palette = Palette {
    dark: true,
    primary: c(0x82D5C0), on_primary: c(0x00382E), primary_container: c(0x005144), on_primary_container: c(0x9EF2DC),
    secondary_container: c(0x334B45), on_secondary_container: c(0xCDE8DF),
    tertiary: c(0xAACBE4), tertiary_container: c(0x2A4A5F),
    error: c(0xFFB4AB), error_container: c(0x93000A), on_error_container: c(0xFFDAD6),
    surface: c(0x0E1513), text: c(0xDEE4E0), muted: c(0xBFC9C4), outline: c(0x89938F), outline_variant: c(0x3F4945),
    lowest: c(0x090F0E), low: c(0x171D1B), container: c(0x1B211F), high: c(0x252B29), highest: c(0x303634),
    good: c(0x7BD99F), warn: c(0xF2C14E),
};

pub fn system_dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme")).map(|v| v == 0).unwrap_or(true)
}

/// The Windows accent colour (Settings → Personalisation → Colours). The DWORD is 0xAABBGGRR.
pub fn windows_accent() -> Option<Color32> {
    use winreg::enums::HKEY_CURRENT_USER;
    let v = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\DWM").and_then(|k| k.get_value::<u32, _>("AccentColor")).ok()?;
    Some(Color32::from_rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8))
}

pub fn is_dark(theme: &str) -> bool { match theme { "light" => false, "dark" => true, _ => system_dark() } }

/// Everything the palette depends on; the app rebuilds its colours when this changes.
pub fn signature(theme: &str, accent: bool) -> String {
    format!("{theme}|{}|{:?}", is_dark(theme), if accent { windows_accent() } else { None })
}

pub fn palette(theme: &str, accent: bool) -> Palette {
    let dark = is_dark(theme);
    match if accent { windows_accent() } else { None } { Some(seed) => from_seed(seed, dark), None => if dark { DARK } else { LIGHT } }
}

// ---- Material-style tonal palette from one seed colour, computed in OKLCH.

fn srgb_to_linear(c: u8) -> f32 { let c = c as f32 / 255.0; if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
fn linear_to_srgb(c: f32) -> f32 { if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } }

fn to_oklch(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (srgb_to_linear(c.r()), srgb_to_linear(c.g()), srgb_to_linear(c.b()));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let (ll, a, bb) = (0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s, 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s);
    (ll, (a * a + bb * bb).sqrt(), bb.atan2(a))
}

/// Linear sRGB of an OKLCH colour, or None when it is outside the sRGB gamut.
fn oklch_linear(l: f32, c: f32, h: f32) -> Option<[f32; 3]> {
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let rgb = [4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_, -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_, -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_];
    rgb.iter().all(|v| (-0.0001..=1.0001).contains(v)).then_some(rgb)
}

/// A colour of Material tone `tone` (CIE L*, 0 black … 100 white) with the given hue, at the
/// requested chroma or the highest one sRGB can show at that tone.
fn tone(tone: f32, chroma: f32, hue: f32) -> Color32 {
    let t = tone.clamp(0.0, 100.0);
    let y = if t > 8.0 { ((t + 16.0) / 116.0).powi(3) } else { t / 903.3 };
    let l = y.cbrt();
    let (mut lo, mut hi) = (0.0f32, chroma);
    if oklch_linear(l, hi, hue).is_none() { for _ in 0..20 { let mid = (lo + hi) / 2.0; if oklch_linear(l, mid, hue).is_some() { lo = mid } else { hi = mid } } } else { lo = hi; }
    let rgb = oklch_linear(l, lo, hue).unwrap_or([y, y, y]);
    let u = |v: f32| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Color32::from_rgb(u(rgb[0]), u(rgb[1]), u(rgb[2]))
}

/// Palette in the roles of Material 3 "tonal spot", seeded with the Windows accent colour.
pub fn from_seed(seed: Color32, dark: bool) -> Palette {
    let (_, c, h) = to_oklch(seed);
    let primary = c.clamp(0.0, 0.16);
    let (secondary, tertiary, neutral, variant) = (primary * 0.33, (primary * 0.5).max(0.03), (primary * 0.08).min(0.012), (primary * 0.16).min(0.024));
    let th = h + 60f32.to_radians();
    let p = |t: f32| tone(t, primary, h); let s = |t: f32| tone(t, secondary, h); let t3 = |t: f32| tone(t, tertiary, th);
    let n = |t: f32| tone(t, neutral, h); let nv = |t: f32| tone(t, variant, h);
    let base = if dark { DARK } else { LIGHT };
    if dark {
        Palette { dark, primary: p(80.0), on_primary: p(20.0), primary_container: p(30.0), on_primary_container: p(90.0),
            secondary_container: s(30.0), on_secondary_container: s(90.0), tertiary: t3(80.0), tertiary_container: t3(30.0),
            surface: n(6.0), text: n(90.0), muted: nv(80.0), outline: nv(60.0), outline_variant: nv(30.0),
            lowest: n(4.0), low: n(10.0), container: n(12.0), high: n(17.0), highest: n(22.0), ..base }
    } else {
        Palette { dark, primary: p(40.0), on_primary: p(100.0), primary_container: p(90.0), on_primary_container: p(10.0),
            secondary_container: s(90.0), on_secondary_container: s(10.0), tertiary: t3(40.0), tertiary_container: t3(90.0),
            surface: n(98.0), text: n(10.0), muted: nv(30.0), outline: nv(50.0), outline_variant: nv(80.0),
            lowest: n(100.0), low: n(96.0), container: n(94.0), high: n(92.0), highest: n(90.0), ..base }
    }
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.surface; v.window_fill = p.container; v.extreme_bg_color = p.highest; v.faint_bg_color = p.low;
    v.code_bg_color = p.low; v.override_text_color = Some(p.text);
    v.window_corner_radius = CornerRadius::same(28); v.menu_corner_radius = CornerRadius::same(12);
    v.window_stroke = Stroke::NONE; v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if p.dark { 140 } else { 50 }) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if p.dark { 120 } else { 40 }) };
    v.selection.bg_fill = p.primary.gamma_multiply(0.35); v.selection.stroke = Stroke::new(1.0, p.primary);
    v.hyperlink_color = p.primary; v.error_fg_color = p.error; v.warn_fg_color = p.warn;
    let r = CornerRadius::same(12);
    for (w, fill) in [(&mut v.widgets.noninteractive, p.surface), (&mut v.widgets.inactive, p.high), (&mut v.widgets.hovered, p.highest), (&mut v.widgets.active, p.highest), (&mut v.widgets.open, p.highest)] {
        w.corner_radius = r; w.bg_fill = fill; w.weak_bg_fill = fill; w.bg_stroke = Stroke::NONE; w.fg_stroke = Stroke::new(1.0, p.text); w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.outline_variant);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.outline);
    v.widgets.active.bg_stroke = Stroke::new(1.5, p.primary);
    v.text_cursor.stroke = Stroke::new(2.0, p.primary);
    v.text_edit_bg_color = Some(p.highest);
    let theme = if p.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.options_mut(|o| o.theme_preference = if p.dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
    ctx.set_visuals_of(theme, v.clone());
    ctx.set_visuals_of(if p.dark { egui::Theme::Light } else { egui::Theme::Dark }, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0); s.spacing.button_padding = egui::vec2(16.0, 9.0); s.spacing.interact_size.y = 36.0;
        s.spacing.scroll = egui::style::ScrollStyle::thin(); s.spacing.menu_margin = egui::Margin::same(8);
        s.text_styles.insert(egui::TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Button, FontId::new(14.5, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Small, FontId::new(12.5, FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(24.0, semibold()));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
        s.interaction.selectable_labels = false;
    });
}

pub fn semibold() -> FontFamily { FontFamily::Name("semibold".into()) }
pub fn icons() -> FontFamily { FontFamily::Name("icons".into()) }

/// Segoe UI (+ Semibold), Segoe Fluent Icons and Cascadia Mono from the Windows fonts folder.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let mut load = |name: &str, files: &[&str]| -> bool {
        for f in files { if let Ok(bytes) = std::fs::read(dir.join(f)) { fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes))); return true; } }
        false
    };
    let ui = load("segoe", &["segoeui.ttf"]);
    let sb = load("segoe-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let ic = load("fluent", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    let mono = load("mono", &["CascadiaMono.ttf", "consola.ttf"]);
    let sym = load("symbols", &["seguisym.ttf"]);
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    if ui { prop.insert(0, "segoe".into()); }
    if sym { prop.push("symbols".into()); }
    let mut semibold = vec![]; if sb { semibold.push("segoe-semibold".to_string()); } if ui { semibold.push("segoe".into()); }
    semibold.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(self::semibold(), semibold);
    let mut icon = vec![]; if ic { icon.push("fluent".to_string()); } icon.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(icons(), icon);
    if mono { fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into()); }
    ctx.set_fonts(fonts);
}

/// Segoe Fluent Icons code points.
pub mod icon {
    pub const POWER: &str = "\u{E7E8}"; pub const HOME: &str = "\u{E80F}"; pub const SETTINGS: &str = "\u{E713}"; pub const JOURNAL: &str = "\u{E9D9}";
    pub const ADD: &str = "\u{E710}"; pub const REFRESH: &str = "\u{E72C}"; pub const SEARCH: &str = "\u{E721}"; pub const STAR: &str = "\u{E735}";
    pub const MORE: &str = "\u{E712}"; pub const DELETE: &str = "\u{E74D}"; pub const COPY: &str = "\u{E8C8}"; pub const CODE: &str = "\u{E943}";
    pub const PULSE: &str = "\u{E945}"; pub const DOWN: &str = "\u{E70D}"; pub const RIGHT: &str = "\u{E76C}"; pub const SHIELD: &str = "\u{EA18}";
    pub const GLOBE: &str = "\u{E774}"; pub const DOWNLOAD: &str = "\u{E896}"; pub const UPLOAD: &str = "\u{E898}"; pub const INFO: &str = "\u{E946}";
    pub const EDIT: &str = "\u{E70F}"; pub const CLEAR: &str = "\u{E894}"; pub const FOLDER: &str = "\u{E838}"; pub const LINK: &str = "\u{E71B}";
    pub const ERROR: &str = "\u{EA39}"; pub const CHECK: &str = "\u{E73E}"; pub const CLOSE: &str = "\u{E711}"; pub const CLOCK: &str = "\u{E823}";
    pub const PASTE: &str = "\u{E77F}"; pub const ROUTE: &str = "\u{E81E}"; pub const SPEED: &str = "\u{EC4A}"; pub const SERVER: &str = "\u{E968}";
    pub const UPDATE: &str = "\u{E895}"; pub const HELP: &str = "\u{E897}";
}
