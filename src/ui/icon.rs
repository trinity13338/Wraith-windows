// The Wraith "W" mark (same shape as the Android launcher icon), rasterised for the window, tray and exe.

const W: &[(f32, f32)] = &[(27.0, 34.0), (37.0, 34.0), (43.0, 63.0), (51.0, 43.0), (58.0, 43.0), (66.0, 63.0), (72.0, 34.0), (82.0, 34.0), (71.0, 76.0), (62.0, 76.0), (54.0, 57.0), (46.0, 76.0), (37.0, 76.0)];
const DOT: &[(f32, f32)] = &[(51.0, 27.0), (58.0, 27.0), (58.0, 34.0), (51.0, 34.0)];

fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false; let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i]; let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi { c = !c; }
        j = i;
    }
    c
}

/// RGBA pixels, size×size, 4×4 supersampled. Rounded dark tile with the teal mark.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let scale = 108.0 / size as f32; let radius = 24.0;
    for py in 0..size {
        for px in 0..size {
            let (mut bg, mut fg) = (0u32, 0u32);
            for sy in 0..4 { for sx in 0..4 {
                let x = (px as f32 + (sx as f32 + 0.5) / 4.0) * scale; let y = (py as f32 + (sy as f32 + 0.5) / 4.0) * scale;
                let dx = (radius - x).max(x - (108.0 - radius)).max(0.0); let dy = (radius - y).max(y - (108.0 - radius)).max(0.0);
                if dx * dx + dy * dy <= radius * radius { bg += 1; if inside(W, x, y) || inside(DOT, x, y) { fg += 1; } }
            } }
            let i = ((py * size + px) * 4) as usize;
            if bg == 0 { continue; }
            let t = fg as f32 / bg as f32;
            let (r, g, b) = (lerp(0x0B, 0xA7, t), lerp(0x10, 0xEB, t), lerp(0x14, 0xD5, t));
            out[i] = r; out[i + 1] = g; out[i + 2] = b; out[i + 3] = (bg * 255 / 16) as u8;
        }
    }
    out
}
fn lerp(a: u8, b: u8, t: f32) -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 }

/// .ico file with 16–256 px images (32-bit BMP entries).
#[allow(dead_code)]
pub fn ico() -> Vec<u8> {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut images = vec![];
    for &s in &sizes {
        let px = rgba(s); let mut bmp = vec![];
        bmp.extend_from_slice(&40u32.to_le_bytes()); bmp.extend_from_slice(&(s as i32).to_le_bytes()); bmp.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes()); bmp.extend_from_slice(&32u16.to_le_bytes()); bmp.extend_from_slice(&[0u8; 24]);
        for y in (0..s).rev() { for x in 0..s { let i = ((y * s + x) * 4) as usize; bmp.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]); } }
        let mask_row = ((s + 31) / 32 * 4) as usize; bmp.extend(std::iter::repeat(0u8).take(mask_row * s as usize));
        images.push((s, bmp));
    }
    let mut out = vec![0, 0, 1, 0]; out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, bmp) in &images {
        let d = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[d, d, 0, 0]); out.extend_from_slice(&1u16.to_le_bytes()); out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(bmp.len() as u32).to_le_bytes()); out.extend_from_slice(&offset.to_le_bytes()); offset += bmp.len() as u32;
    }
    for (_, bmp) in images { out.extend(bmp); }
    out
}
