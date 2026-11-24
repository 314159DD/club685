//! Small tiny-skia helpers shared by the overlay renderer and the app's own UI.

use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, RadialGradient, Rect, SpreadMode, Stroke, StrokeDash,
    Transform,
};

pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

pub fn hex(h: u32) -> Color {
    Color::from_rgba8((h >> 16) as u8, (h >> 8) as u8, h as u8, 255)
}

pub fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

/// Vertical gradient between `stops` (offset 0..1, colour) over `y0..y1`.
pub fn vgradient(y0: f32, y1: f32, stops: &[(f32, Color)]) -> Paint<'static> {
    let mut p = Paint::default();
    let st: Vec<GradientStop> = stops.iter().map(|&(o, c)| GradientStop::new(o, c)).collect();
    if let Some(sh) = LinearGradient::new(Point::from_xy(0.0, y0), Point::from_xy(0.0, y1.max(y0 + 0.01)), st, SpreadMode::Pad, Transform::identity()) {
        p.shader = sh;
    }
    p.anti_alias = true;
    p
}

/// Radial gradient from `c0` at the centre to `c1` at `r`.
pub fn radial(cx: f32, cy: f32, r: f32, c0: Color, c1: Color) -> Paint<'static> {
    let mut p = Paint::default();
    let st = vec![GradientStop::new(0.0, c0), GradientStop::new(1.0, c1)];
    if let Some(sh) = RadialGradient::new(Point::from_xy(cx, cy), Point::from_xy(cx, cy), r.max(0.01), st, SpreadMode::Pad, Transform::identity()) {
        p.shader = sh;
    }
    p.anti_alias = true;
    p
}

/// Rounded rectangle path with corner radius `r` (clamped to half the short side).
pub fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    if r <= 0.01 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    let k = 0.552_284_75 * r;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn fill_rrect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, paint: &Paint) {
    if let Some(p) = rrect(x, y, w, h, r) {
        pm.fill_path(&p, paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Stroke centred on the rounded rectangle path inset by half the width, so the border stays inside the box.
pub fn stroke_rrect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, paint: &Paint) {
    let hw = width / 2.0;
    if let Some(p) = rrect(x + hw, y + hw, w - width, h - width, (r - hw).max(0.0)) {
        let st = Stroke { width, ..Default::default() };
        pm.stroke_path(&p, paint, &st, Transform::identity(), None);
    }
}

pub fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, paint: &Paint) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, paint, Transform::identity(), None);
    }
}

pub fn hline_dashed(pm: &mut Pixmap, x0: f32, x1: f32, y: f32, width: f32, dash: f32, paint: &Paint) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y);
    pb.line_to(x1, y);
    if let Some(p) = pb.finish() {
        let st = Stroke { width, dash: StrokeDash::new(vec![dash, dash], 0.0), ..Default::default() };
        pm.stroke_path(&p, paint, &st, Transform::identity(), None);
    }
}

pub fn fill_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, paint: &Paint) {
    if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(&p, paint, FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, paint: &Paint) {
    if let Some(p) = PathBuilder::from_circle(cx, cy, r - width / 2.0) {
        let st = Stroke { width, ..Default::default() };
        pm.stroke_path(&p, paint, &st, Transform::identity(), None);
    }
}
