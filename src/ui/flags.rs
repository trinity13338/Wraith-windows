//! Round flag badges for servers. Windows fonts have no flag glyphs, so the flags are images:
//! the square set of flag-icons (MIT, assets/LICENSE-flag-icons.txt) packed into one atlas.
//! Nordic-cross flags were cut from the wide set so the cross stays left of centre.
use egui::{pos2, Color32, ColorImage, Context, Id, Mesh, Painter, Pos2, Stroke, TextureHandle, TextureOptions};

static ATLAS: &[u8] = include_bytes!("../../assets/flags.png");
static CODES: &str = include_str!("../../assets/flags.txt");
const CELL: f32 = 64.0;
const COLS: usize = 16;

fn regional(c: char) -> bool { (0x1F1E6..=0x1F1FF).contains(&(c as u32)) }

/// Country code ("lt") of the first flag emoji anywhere in a name.
pub fn flag_code(name: &str) -> Option<String> {
    let chars: Vec<char> = name.chars().collect();
    chars.windows(2).find(|w| regional(w[0]) && regional(w[1]))
        .map(|w| w.iter().map(|c| char::from_u32(*c as u32 - 0x1F1E6 + 'a' as u32).unwrap_or('?')).collect())
}

fn index(code: &str) -> Option<usize> { CODES.lines().position(|c| c == code) }

fn decode() -> Option<ColorImage> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(ATLAS)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight { return None; }
    Some(ColorImage::from_rgba_unmultiplied([info.width as usize, info.height as usize], &buf[..info.buffer_size()]))
}

fn texture(ctx: &Context) -> Option<TextureHandle> {
    let id = Id::new("wraith-flag-atlas");
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<TextureHandle>>(id)) { return t; }
    let t = decode().map(|img| ctx.load_texture("flags", img, TextureOptions::LINEAR));
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Paints the flag of `code` filling a circle. False when there is no image for it.
pub fn paint(painter: &Painter, code: &str, center: Pos2, radius: f32, ring: Option<Stroke>) -> bool {
    let Some(k) = index(code) else { return false };
    let Some(tex) = texture(painter.ctx()) else { return false };
    let [w, h] = tex.size();
    let (u0, v0) = ((k % COLS) as f32 * CELL / w as f32, (k / COLS) as f32 * CELL / h as f32);
    let (du, dv) = (CELL / w as f32, CELL / h as f32);
    // Half a texel in from the cell edge so neighbouring flags never bleed in.
    let (iu, iv) = (0.5 / w as f32, 0.5 / h as f32);
    let uv = |x: f32, y: f32| pos2(u0 + iu + (du - 2.0 * iu) * (x * 0.5 + 0.5), v0 + iv + (dv - 2.0 * iv) * (y * 0.5 + 0.5));
    let mut mesh = Mesh::with_texture(tex.id());
    let vertex = |pos: Pos2, uv: Pos2| egui::epaint::Vertex { pos, uv, color: Color32::WHITE };
    mesh.vertices.push(vertex(center, uv(0.0, 0.0)));
    let n = 64u32;
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU; let (x, y) = (a.cos(), a.sin());
        mesh.vertices.push(vertex(center + egui::vec2(x, y) * radius, uv(x, y)));
        if i > 0 { mesh.add_triangle(0, i, i + 1); }
    }
    painter.add(mesh);
    if let Some(s) = ring { painter.circle_stroke(center, radius, s); }
    true
}
