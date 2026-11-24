//! Full-window backdrop, as on Android: the user's photo (blurred and dimmed) or a slow animated
//! aurora in the accent colour that brightens when connected. The photo stays in Wraith's folder.

use super::theme::{self, alpha, Palette};
use egui::{pos2, Color32, ColorImage, Context, Mesh, Painter, Pos2, Rect, TextureHandle, TextureOptions};
use std::sync::{Arc, Mutex};

pub fn photo_file() -> std::path::PathBuf { crate::store::folder().join("background.img") }

struct Photo { image: Arc<ColorImage>, accent: Option<Color32>, stamp: u64 }
/// The decoded photo (shared by every window) and the blurred texture of the current window.
static PHOTO: Mutex<Option<Photo>> = Mutex::new(None);
static STAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Reads the saved photo (once, in the background).
pub fn load() {
    if LOADED.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
    std::thread::spawn(|| {
        let photo = std::fs::read(photo_file()).ok().and_then(|b| decode(&b).ok());
        *PHOTO.lock().unwrap() = photo.map(|(image, accent)| Photo { image: Arc::new(image), accent, stamp: STAMP.fetch_add(1, std::sync::atomic::Ordering::SeqCst) });
        super::repaint();
    });
}
pub fn has_photo() -> bool { PHOTO.lock().unwrap().is_some() }
pub fn photo_accent() -> Option<Color32> { PHOTO.lock().unwrap().as_ref().and_then(|p| p.accent) }

/// Validates and stores a new photo (≤ 40 MB, an image), then shows it.
pub fn set_photo(path: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 40 * 1024 * 1024 { return Err(crate::tr!("Фото больше 40 МБ", "The photo is larger than 40 MB").into()); }
    let (image, accent) = decode(&bytes)?;
    let tmp = photo_file().with_extension("tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, photo_file()).map_err(|e| e.to_string())?;
    *PHOTO.lock().unwrap() = Some(Photo { image: Arc::new(image), accent, stamp: STAMP.fetch_add(1, std::sync::atomic::Ordering::SeqCst) });
    Ok(())
}
pub fn remove_photo() { let _ = std::fs::remove_file(photo_file()); *PHOTO.lock().unwrap() = None; }

fn decode(bytes: &[u8]) -> Result<(ColorImage, Option<Color32>), String> {
    let img = image::load_from_memory(bytes).map_err(|_| crate::tr!("Это не изображение (поддерживаются JPEG, PNG, WebP)", "This is not an image (JPEG, PNG and WebP are supported)").to_string())?;
    // The backdrop is dimmed and often blurred: 1920 px on the long side is plenty.
    let img = if img.width().max(img.height()) > 1920 { img.resize(1920, 1920, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let accent = accent_of(rgba.as_raw());
    Ok((ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), accent))
}

/// A vivid, light colour of the photo (like Android's Palette lightVibrant / vibrant swatch).
fn accent_of(rgba: &[u8]) -> Option<Color32> {
    let mut buckets = [[0f64; 4]; 24]; // hue buckets: weight, r, g, b
    for px in rgba.chunks_exact(4).step_by(7) {
        if px[3] < 128 { continue; }
        let (r, g, b) = (px[0] as f64 / 255.0, px[1] as f64 / 255.0, px[2] as f64 / 255.0);
        let max = r.max(g).max(b); let min = r.min(g).min(b); let d = max - min;
        let s = if max == 0.0 { 0.0 } else { d / max };
        if s < 0.3 || max < 0.35 { continue; }
        let h = if d == 0.0 { 0.0 } else if max == r { ((g - b) / d).rem_euclid(6.0) } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 } / 6.0;
        let k = ((h * 24.0) as usize).min(23);
        let w = s * s * max;
        buckets[k][0] += w; buckets[k][1] += r * w; buckets[k][2] += g * w; buckets[k][3] += b * w;
    }
    let best = buckets.iter().max_by(|a, b| a[0].total_cmp(&b[0]))?;
    if best[0] < 1.0 { return None; }
    let c = Color32::from_rgb((best[1] / best[0] * 255.0) as u8, (best[2] / best[0] * 255.0) as u8, (best[3] / best[0] * 255.0) as u8);
    // Light vibrant: lift dark picks.
    Some(if theme::luminance(c) < 0.3 { theme::lerp(c, Color32::WHITE, 0.35) } else { c })
}

/// Three box-blur passes ≈ a Gaussian; radius in pixels of the stored image.
fn blur(img: &ColorImage, radius: usize) -> ColorImage {
    if radius == 0 { return img.clone(); }
    let [w, h] = img.size;
    let mut buf: Vec<[f32; 4]> = img.pixels.iter().map(|c| { let [r, g, b, a] = c.to_array(); [r as f32, g as f32, b as f32, a as f32] }).collect();
    let mut tmp = buf.clone();
    let pass = |src: &Vec<[f32; 4]>, dst: &mut Vec<[f32; 4]>, horizontal: bool| {
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
        let norm = 1.0 / (2 * radius + 1) as f32;
        for line in 0..lines {
            let mut acc = [0f32; 4];
            for k in 0..=2 * radius { let i = k.saturating_sub(radius).min(len - 1); let p = src[idx(line, i)]; for c in 0..4 { acc[c] += p[c]; } }
            for i in 0..len {
                for c in 0..4 { dst[idx(line, i)][c] = acc[c] * norm; }
                let out = idx(line, i.saturating_sub(radius)); let inn = idx(line, (i + radius + 1).min(len - 1));
                for c in 0..4 { acc[c] += src[inn][c] - src[out][c]; }
            }
        }
    };
    for _ in 0..3 { pass(&buf, &mut tmp, true); pass(&tmp, &mut buf, false); }
    let pixels = buf.iter().map(|p| Color32::from_rgba_premultiplied(p[0] as u8, p[1] as u8, p[2] as u8, p[3] as u8)).collect();
    ColorImage { size: [w, h], pixels, source_size: egui::vec2(w as f32, h as f32) }
}

/// The photo texture for this window at this blur, rebuilt when either changes.
fn texture(ctx: &Context, blur_amount: f32) -> Option<TextureHandle> {
    let (image, stamp) = { let g = PHOTO.lock().unwrap(); let p = g.as_ref()?; (p.image.clone(), p.stamp) };
    let key = (stamp, (blur_amount * 20.0).round() as i32);
    let id = egui::Id::new("wraith-backdrop");
    if let Some((k, t)) = ctx.data(|d| d.get_temp::<((u64, i32), TextureHandle)>(id)) { if k == key { return Some(t); } }
    // Blur scales with the picture so it looks the same at any resolution (≈ 28 dp on a phone).
    let radius = (blur_amount * image.size[0].max(image.size[1]) as f32 / 1920.0 * 40.0).round() as usize;
    let t = ctx.load_texture("backdrop", blur(&image, radius), TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, (key, t.clone())));
    Some(t)
}

/// Paints the backdrop over `rect`. `energy` follows the connection (0.8 off … 1.25 connected).
pub fn paint(ctx: &Context, painter: &Painter, rect: Rect, p: &Palette, blur_amount: f32, dim: f32, energy: f32) -> bool {
    painter.rect_filled(rect, 0.0, p.bg);
    if let Some(tex) = texture(ctx, blur_amount) {
        // Cover: fill the window, crop the overflow.
        let [tw, th] = tex.size(); let (tw, th) = (tw as f32, th as f32);
        let scale = (rect.width() / tw).max(rect.height() / th);
        let (vw, vh) = (rect.width() / scale / tw, rect.height() / scale / th);
        let uv = Rect::from_min_size(pos2((1.0 - vw) / 2.0, (1.0 - vh) / 2.0), egui::vec2(vw, vh));
        painter.image(tex.id(), rect, uv, Color32::WHITE);
        vertical_gradient(painter, rect, &[(0.0, alpha(Color32::BLACK, 0.15 + dim * 0.35)), (0.42, alpha(Color32::BLACK, 0.1 + dim * 0.45)), (1.0, alpha(p.bg, 0.82 + dim * 0.16))]);
        return false;
    }
    let t = ctx.input(|i| i.time) as f32;
    let phase = t / 26.0 * std::f32::consts::TAU;
    let (w, h) = (rect.width(), rect.height());
    let o = rect.min;
    let second = theme::lerp(p.accent, Color32::from_rgb(0x6C, 0x7B, 0xFF), 0.55);
    let third = theme::lerp(p.accent, Color32::from_rgb(0x3F, 0xE0, 0xC5), 0.5);
    // On a wide window the blobs are sized by the shorter side, so they stay soft clouds.
    let unit = w.min(h * 0.8);
    blob(painter, p.accent, pos2(o.x + w * (0.25 + 0.18 * phase.cos()), o.y + h * (0.18 + 0.08 * (phase * 2.0).sin())), unit * 0.95, 0.30 * energy);
    blob(painter, second, pos2(o.x + w * (0.85 + 0.12 * phase.sin()), o.y + h * (0.42 + 0.1 * phase.cos())), unit * 0.9, 0.22 * energy);
    blob(painter, third, pos2(o.x + w * (0.35 + 0.2 * (phase + 1.3).sin()), o.y + h * (0.78 + 0.06 * (phase * 2.0).cos())), unit * 0.8, 0.14 * energy);
    vertical_gradient(painter, rect, &[(0.0, Color32::TRANSPARENT), (0.55, alpha(p.bg, 0.35)), (1.0, alpha(p.bg, 0.92))]);
    true
}

/// A radial gradient (colour → transparent) as a fan of rings with a smooth falloff.
fn blob(painter: &Painter, color: Color32, center: Pos2, radius: f32, a: f32) {
    let rings = [(0.0, 1.0), (0.25, 0.82), (0.5, 0.5), (0.75, 0.2), (1.0, 0.0)];
    let n = 72u32;
    let mut mesh = Mesh::default();
    for (ri, (r, k)) in rings.iter().enumerate() {
        let c = alpha(color, a * k);
        for i in 0..n {
            let ang = i as f32 / n as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + egui::vec2(ang.cos(), ang.sin()) * radius * *r, c);
        }
        if ri > 0 {
            let (prev, cur) = ((ri as u32 - 1) * n, ri as u32 * n);
            for i in 0..n { let j = (i + 1) % n; mesh.add_triangle(prev + i, cur + i, cur + j); mesh.add_triangle(prev + i, cur + j, prev + j); }
        }
    }
    painter.add(mesh);
}

pub fn vertical_gradient(painter: &Painter, rect: Rect, stops: &[(f32, Color32)]) {
    let mut mesh = Mesh::default();
    for (k, (t, c)) in stops.iter().enumerate() {
        let y = rect.top() + rect.height() * t;
        mesh.colored_vertex(pos2(rect.left(), y), *c); mesh.colored_vertex(pos2(rect.right(), y), *c);
        if k > 0 { let b = (k as u32 - 1) * 2; mesh.add_triangle(b, b + 1, b + 2); mesh.add_triangle(b + 1, b + 3, b + 2); }
    }
    painter.add(mesh);
}
