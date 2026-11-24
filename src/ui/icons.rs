//! Line icons on a 24×24 grid, the same paths as the Android app (`Ic`), drawn as strokes.
//! A small SVG path reader (M L H V C S Q A Z, absolute and relative) flattens them once.

use egui::{pos2, Color32, Painter, Pos2, Rect, Stroke};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ic {
    Home, Globe, Bars, Settings, Power, Plus, Close, Back, Chevron, ChevronDown, More, Refresh, Search, Star, Pulse,
    Copy, Edit, Delete, Shield, Route, Apps, Image, Palette, Link, Code, Info, Download, Up, Down, Clock, Log, Bolt,
    Check, User, Lock, Dns, Layers, Sort, Wifi, Eye, Bell, Sparkle, Folder, Paste, Monitor, Forward,
}

fn spec(i: Ic) -> (&'static str, f32) {
    match i {
        Ic::Home => ("M3.5 10.5 12 3.5l8.5 7M5.5 9v11h4.5v-6h4v6h4.5V9", 1.8),
        Ic::Globe => ("M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M3.5 9h17M3.5 15h17M12 3c-3 3.2-3 14.8 0 18M12 3c3 3.2 3 14.8 0 18", 1.8),
        Ic::Bars => ("M5.5 20v-7M12 20V5M18.5 20v-10", 2.2),
        Ic::Settings => ("M4 7h8M17 7h3M4 17h3M12 17h8M14.5 4.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 1 0 0-5M9.5 14.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 1 0 0-5", 1.8),
        Ic::Power => ("M7.2 6.5a8 8 0 1 0 9.6 0M12 3v8.5", 2.2),
        Ic::Plus => ("M5 12h14M12 5v14", 2.0),
        Ic::Close => ("M6 6l12 12M18 6 6 18", 2.0),
        Ic::Back => ("M19 12H5M11 6l-6 6 6 6", 2.0),
        Ic::Chevron => ("M9.5 6l6 6-6 6", 2.0),
        Ic::ChevronDown => ("M6 9.5l6 6 6-6", 2.0),
        Ic::More => ("M12 5h.01M12 12h.01M12 19h.01", 3.0),
        Ic::Refresh => ("M20 12a8 8 0 1 1-2.4-5.7M20 4v5h-5", 1.8),
        Ic::Search => ("M10.5 4a6.5 6.5 0 1 0 0 13a6.5 6.5 0 1 0 0-13M15.5 15.5l5 5", 1.8),
        Ic::Star => ("M12 3.5l2.6 5.4 5.9.7-4.3 4.1 1.1 5.8L12 16.7l-5.3 2.8 1.1-5.8-4.3-4.1 5.9-.7z", 1.8),
        Ic::Pulse => ("M2.5 12h4l3-7 5 14 3-7h4", 1.8),
        Ic::Copy => ("M9 9h11v11H9zM15 9V4H4v11h5", 1.8),
        Ic::Edit => ("M4 20v-4L15.5 4.5l4 4L8 20zM13.5 6.5l4 4", 1.8),
        Ic::Delete => ("M4 6.5h16M6.5 6.5l1 14h9l1-14M9.5 6.5l1-3h3l1 3", 1.8),
        Ic::Shield => ("M12 2.8l8 3.4-.8 7.4C18 18 15 20.4 12 21.4 9 20.4 6 18 4.8 13.6L4 6.2zM8.8 12l2.2 2.2 4.4-4.6", 1.8),
        Ic::Route => ("M6 3.5a2 2 0 1 0 0 4a2 2 0 1 0 0-4M18 16.5a2 2 0 1 0 0 4a2 2 0 1 0 0-4M6 7.5v6a5 5 0 0 0 5 5h5M18 16.5V12a3 3 0 0 0-3-3H6", 1.8),
        Ic::Apps => ("M5 5h4v4H5zM15 5h4v4h-4zM5 15h4v4H5zM15 15h4v4h-4z", 1.8),
        Ic::Image => ("M4 5h16v14H4zM4 16l5-5 4 4 3-3 4 4M15.5 9h.01", 1.8),
        Ic::Palette => ("M12 3a9 9 0 1 0 0 18c1.5 0 2-1 2-2s-1-1.6-1-2.6c0-1 .8-1.9 2-1.9h2a4 4 0 0 0 4-4C21 6.5 17 3 12 3M7.5 11.5h.01M10 7.5h.01M15 7.5h.01", 1.8),
        Ic::Link => ("M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1", 1.8),
        Ic::Code => ("M9 7l-5 5 5 5M15 7l5 5-5 5", 1.8),
        Ic::Info => ("M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M12 11v5.5M12 7.5h.01", 1.8),
        Ic::Download => ("M12 4v11M7 10l5 5 5-5M5 20h14", 1.8),
        Ic::Up => ("M12 19V5M6 11l6-6 6 6", 2.0),
        Ic::Down => ("M12 5v14M6 13l6 6 6-6", 2.0),
        Ic::Clock => ("M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M12 7.5V12l3 2", 1.8),
        Ic::Log => ("M5 3h14v18H5zM9 8h6M9 12h6M9 16h3", 1.8),
        Ic::Bolt => ("M13 2.5 4.5 13.5h6.5l-1 8 8.5-11h-6.5z", 1.8),
        Ic::Check => ("M5 12.5l4.5 4.5L19.5 7", 2.2),
        Ic::User => ("M12 4a4 4 0 1 0 0 8a4 4 0 1 0 0-8M4.5 20.5c1-3.8 3.8-5.8 7.5-5.8s6.5 2 7.5 5.8", 1.8),
        Ic::Lock => ("M6 10.5h12v10H6zM8.5 10.5V7.5a3.5 3.5 0 0 1 7 0v3", 1.8),
        Ic::Dns => ("M4 4h16v6H4zM4 14h16v6H4zM7.5 7h.01M7.5 17h.01", 1.8),
        Ic::Layers => ("M12 3 3 8l9 5 9-5zM3 13l9 5 9-5", 1.8),
        Ic::Sort => ("M4 7h16M7 12h10M10 17h4", 1.8),
        Ic::Wifi => ("M2.5 9a14 14 0 0 1 19 0M5.5 12.5a9.5 9.5 0 0 1 13 0M8.8 16a5 5 0 0 1 6.4 0M12 19.5h.01", 1.8),
        Ic::Eye => ("M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12M12 9.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 1 0 0-5", 1.8),
        Ic::Bell => ("M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15zM10 20.5h4", 1.8),
        Ic::Sparkle => ("M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M6 18l2.5-2.5M15.5 8.5 18 6", 1.8),
        // Linux additions in the same style.
        Ic::Folder => ("M3.5 6.5a1.5 1.5 0 0 1 1.5-1.5h4.5l2 2.5h7.5a1.5 1.5 0 0 1 1.5 1.5v9a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5z", 1.8),
        Ic::Paste => ("M8.5 5H6v16h12V5h-2.5M9 3h6v4H9zM9 12h6M9 16h4", 1.8),
        Ic::Monitor => ("M3.5 4.5h17v11h-17zM8.5 20h7M12 15.5V20", 1.8),
        Ic::Forward => ("M5 12h14M13 6l6 6-6 6", 2.0),
    }
}

/// Flattened subpaths in the 24×24 grid: (points, closed).
type Shape = Vec<(Vec<Pos2>, bool)>;

fn shapes() -> &'static std::sync::Mutex<HashMap<Ic, std::sync::Arc<Shape>>> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<Ic, std::sync::Arc<Shape>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn shape(i: Ic) -> std::sync::Arc<Shape> {
    let mut cache = shapes().lock().unwrap();
    cache.entry(i).or_insert_with(|| std::sync::Arc::new(parse(spec(i).0))).clone()
}

/// Draws the icon centred in `rect` (square), stroke scaled with it.
pub fn paint(painter: &Painter, rect: Rect, icon: Ic, color: Color32) {
    let size = rect.width().min(rect.height());
    let scale = size / 24.0;
    let origin = rect.center() - egui::vec2(size / 2.0, size / 2.0);
    let width = (spec(icon).1 * scale).max(1.0);
    let stroke = Stroke::new(width, color);
    for (pts, closed) in shape(icon).iter() {
        let mapped: Vec<Pos2> = pts.iter().map(|p| origin + p.to_vec2() * scale).collect();
        if mapped.len() < 2 { continue; }
        if *closed { painter.add(egui::Shape::closed_line(mapped, stroke)); }
        else {
            // Round caps (and the dots made of zero-length segments).
            let (a, b) = (mapped[0], *mapped.last().unwrap());
            painter.add(egui::Shape::line(mapped, stroke));
            painter.circle_filled(a, width / 2.0, color); painter.circle_filled(b, width / 2.0, color);
        }
    }
}
pub fn at(painter: &Painter, center: Pos2, size: f32, icon: Ic, color: Color32) {
    paint(painter, Rect::from_center_size(center, egui::vec2(size, size)), icon, color);
}

// ------------------------------------------------------------------ path reader

fn numbers(s: &str) -> Vec<(Option<char>, f32)> {
    // Tokens: a command letter (attached to the next number) or a number.
    let mut out = vec![]; let chars: Vec<char> = s.chars().collect(); let mut i = 0; let mut cmd = None;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphabetic() { cmd = Some(c); if c == 'z' || c == 'Z' { out.push((cmd.take(), f32::NAN)); } i += 1; continue; }
        if c == ' ' || c == ',' || c == '\n' { i += 1; continue; }
        let start = i; let mut seen_dot = false;
        if c == '-' || c == '+' { i += 1; }
        while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && !seen_dot)) { if chars[i] == '.' { seen_dot = true; } i += 1; }
        let text: String = chars[start..i].iter().collect();
        out.push((cmd.take(), text.parse().unwrap_or(0.0)));
        if i == start { i += 1; }
    }
    out
}

fn parse(d: &str) -> Shape {
    let toks = numbers(d);
    let mut shapes: Shape = vec![]; let mut cur: Vec<Pos2> = vec![];
    let mut p = pos2(0.0, 0.0); let mut start = p; let mut last_ctrl: Option<Pos2> = None;
    let mut i = 0; let mut cmd = 'M';
    let take = |i: &mut usize, n: usize| -> Vec<f32> { let v = toks[*i..(*i + n).min(toks.len())].iter().map(|t| t.1).collect(); *i += n; v };
    while i < toks.len() {
        if let Some(c) = toks[i].0 { cmd = c; }
        if cmd == 'z' || cmd == 'Z' {
            if !cur.is_empty() { shapes.push((std::mem::take(&mut cur), true)); }
            p = start; i += 1; last_ctrl = None; continue;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { p.to_vec2() } else { egui::Vec2::ZERO };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let v = take(&mut i, 2); if v.len() < 2 { break; }
                if cur.len() > 1 { shapes.push((std::mem::take(&mut cur), false)); } else { cur.clear(); }
                p = pos2(v[0], v[1]) + base; start = p; cur.push(p);
                cmd = if rel { 'l' } else { 'L' }; last_ctrl = None;
            }
            'L' => { let v = take(&mut i, 2); if v.len() < 2 { break; } p = pos2(v[0], v[1]) + base; cur.push(p); last_ctrl = None; }
            'H' => { let v = take(&mut i, 1); p = pos2(v[0] + if rel { p.x } else { 0.0 }, p.y); cur.push(p); last_ctrl = None; }
            'V' => { let v = take(&mut i, 1); p = pos2(p.x, v[0] + if rel { p.y } else { 0.0 }); cur.push(p); last_ctrl = None; }
            'C' | 'S' => {
                let smooth = cmd.to_ascii_uppercase() == 'S';
                let v = take(&mut i, if smooth { 4 } else { 6 }); if v.len() < if smooth { 4 } else { 6 } { break; }
                let (c1, c2, end) = if smooth {
                    let c1 = last_ctrl.map(|c| p + (p - c)).unwrap_or(p);
                    (c1, pos2(v[0], v[1]) + base, pos2(v[2], v[3]) + base)
                } else { (pos2(v[0], v[1]) + base, pos2(v[2], v[3]) + base, pos2(v[4], v[5]) + base) };
                for k in 1..=12 { let t = k as f32 / 12.0; let u = 1.0 - t;
                    cur.push(pos2(u*u*u*p.x + 3.0*u*u*t*c1.x + 3.0*u*t*t*c2.x + t*t*t*end.x, u*u*u*p.y + 3.0*u*u*t*c1.y + 3.0*u*t*t*c2.y + t*t*t*end.y)); }
                last_ctrl = Some(c2); p = end;
            }
            'Q' => {
                let v = take(&mut i, 4); if v.len() < 4 { break; }
                let (c, end) = (pos2(v[0], v[1]) + base, pos2(v[2], v[3]) + base);
                for k in 1..=10 { let t = k as f32 / 10.0; let u = 1.0 - t; cur.push(pos2(u*u*p.x + 2.0*u*t*c.x + t*t*end.x, u*u*p.y + 2.0*u*t*c.y + t*t*end.y)); }
                last_ctrl = None; p = end;
            }
            'A' => {
                let v = take(&mut i, 7); if v.len() < 7 { break; }
                let end = pos2(v[5], v[6]) + base;
                arc(&mut cur, p, v[0], v[1], v[2], v[3] != 0.0, v[4] != 0.0, end);
                p = end; last_ctrl = None;
            }
            _ => { i += 1; }
        }
    }
    if cur.len() > 1 { shapes.push((cur, false)); }
    shapes
}

/// SVG elliptical arc (endpoint form) flattened into `out`.
fn arc(out: &mut Vec<Pos2>, p0: Pos2, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: Pos2) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || p0 == p1 { out.push(p1); return; }
    let phi = rot_deg.to_radians(); let (cs, sn) = (phi.cos(), phi.sin());
    let dx = (p0.x - p1.x) / 2.0; let dy = (p0.y - p1.y) / 2.0;
    let x1 = cs * dx + sn * dy; let y1 = -sn * dx + cs * dy;
    let lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lam > 1.0 { let s = lam.sqrt(); rx *= s; ry *= s; }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).sqrt() };
    if large == sweep { coef = -coef; }
    let cx1 = coef * rx * y1 / ry; let cy1 = -coef * ry * x1 / rx;
    let cx = cs * cx1 - sn * cy1 + (p0.x + p1.x) / 2.0; let cy = sn * cx1 + cs * cy1 + (p0.y + p1.y) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| { let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy); a };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 { dt -= std::f32::consts::TAU; } else if sweep && dt < 0.0 { dt += std::f32::consts::TAU; }
    let n = ((dt.abs() / std::f32::consts::TAU) * 48.0).ceil().max(4.0) as usize;
    for k in 1..=n {
        let t = t1 + dt * k as f32 / n as f32;
        let (x, y) = (rx * t.cos(), ry * t.sin());
        out.push(pos2(cs * x - sn * y + cx, sn * x + cs * y + cy));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_parses() {
        use super::*;
        for i in [Ic::Home, Ic::Globe, Ic::Palette, Ic::Eye, Ic::More, Ic::Star, Ic::Folder, Ic::Shield, Ic::Refresh] {
            let s = parse(spec(i).0);
            assert!(!s.is_empty(), "{i:?}");
            for (pts, _) in s.iter() { for p in pts { assert!(p.x.is_finite() && p.y.is_finite() && p.x > -1.0 && p.x < 25.0 && p.y > -1.0 && p.y < 25.0, "{i:?} {p:?}"); } }
        }
    }
}
