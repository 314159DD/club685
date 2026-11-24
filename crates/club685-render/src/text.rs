//! Text rendering with `ab_glyph` into a tiny-skia pixmap. Fonts are embedded (SIL OFL, see assets/fonts).

use ab_glyph::{point, Font, FontArc, FontRef, PxScale, ScaleFont, VariableFont};
use std::sync::OnceLock;
use tiny_skia::Pixmap;

static ORBITRON: &[u8] = include_bytes!("../../../assets/fonts/Orbitron-VF.ttf");
static SHARE_TECH_MONO: &[u8] = include_bytes!("../../../assets/fonts/ShareTechMono-Regular.ttf");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Orbitron500,
    Orbitron800,
    Mono,
    /// Arial bold from the Windows font folder when present (the meter labels), Orbitron 800 otherwise.
    LabelBold,
}

pub struct Fonts {
    orbitron500: FontArc,
    orbitron800: FontArc,
    mono: FontArc,
    label: FontArc,
}

fn orbitron(weight: f32) -> FontArc {
    let mut f = FontRef::try_from_slice(ORBITRON).expect("embedded Orbitron");
    f.set_variation(b"wght", weight);
    FontArc::new(f)
}

impl Fonts {
    fn load() -> Self {
        let mono = FontArc::new(FontRef::try_from_slice(SHARE_TECH_MONO).expect("embedded Share Tech Mono"));
        let label = std::env::var("WINDIR")
            .ok()
            .map(|w| std::path::PathBuf::from(w).join("Fonts").join("arialbd.ttf"))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| ab_glyph::FontVec::try_from_vec(b).ok())
            .map(FontArc::new)
            .unwrap_or_else(|| orbitron(800.0));
        Self { orbitron500: orbitron(500.0), orbitron800: orbitron(800.0), mono, label }
    }

    pub fn get(&self, face: Face) -> &FontArc {
        match face {
            Face::Orbitron500 => &self.orbitron500,
            Face::Orbitron800 => &self.orbitron800,
            Face::Mono => &self.mono,
            Face::LabelBold => &self.label,
        }
    }
}

pub fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(Fonts::load)
}

/// Width of `text` in pixels at size `px` with extra `spacing` after every glyph.
pub fn measure(face: Face, px: f32, text: &str, spacing: f32) -> f32 {
    let font = fonts().get(face);
    let sf = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id) + spacing;
        prev = Some(id);
    }
    w
}

/// Draws `text` with its baseline at `baseline` starting at `x`. `color` is straight RGB, `alpha` 0..1.
#[allow(clippy::too_many_arguments)]
pub fn draw(pm: &mut Pixmap, face: Face, px: f32, x: f32, baseline: f32, color: (u8, u8, u8), alpha: f32, text: &str, spacing: f32) {
    let font = fonts().get(face);
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut cx = x;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            cx += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(cx, baseline));
        if let Some(og) = font.outline_glyph(glyph) {
            let b = og.px_bounds();
            let (bx, by) = (b.min.x as i32, b.min.y as i32);
            og.draw(|gx, gy, cov| {
                let (px_, py_) = (bx + gx as i32, by + gy as i32);
                if px_ < 0 || py_ < 0 || px_ >= pw || py_ >= ph || cov <= 0.0 {
                    return;
                }
                let a = (cov * alpha).clamp(0.0, 1.0);
                let i = ((py_ * pw + px_) * 4) as usize;
                let inv = 1.0 - a;
                data[i] = (color.0 as f32 * a + data[i] as f32 * inv).round() as u8;
                data[i + 1] = (color.1 as f32 * a + data[i + 1] as f32 * inv).round() as u8;
                data[i + 2] = (color.2 as f32 * a + data[i + 2] as f32 * inv).round() as u8;
                data[i + 3] = (255.0 * a + data[i + 3] as f32 * inv).round() as u8;
            });
        }
        cx += sf.h_advance(id) + spacing;
        prev = Some(id);
    }
}

/// Distance from the baseline up to the top of capital letters, and the line's total ascent, for vertical centring.
pub fn cap_height(face: Face, px: f32) -> f32 {
    let font = fonts().get(face);
    let sf = font.as_scaled(PxScale::from(px));
    font.outline_glyph(sf.glyph_id('H').with_scale_and_position(PxScale::from(px), point(0.0, 0.0)))
        .map(|g| -g.px_bounds().min.y)
        .unwrap_or(px * 0.7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_measure_and_draw() {
        for face in [Face::Orbitron500, Face::Orbitron800, Face::Mono, Face::LabelBold] {
            let w = measure(face, 14.0, "05.1 Bars", 0.0);
            assert!(w > 20.0, "{face:?} {w}");
        }
        // the variable font instances differ: weight 800 puts more ink down than weight 500
        let ink = |face: Face| -> u64 {
            let mut pm = Pixmap::new(200, 40).unwrap();
            draw(&mut pm, face, 20.0, 2.0, 28.0, (255, 255, 255), 1.0, "CLUB685", 0.0);
            pm.data().chunks_exact(4).map(|p| p[3] as u64).sum()
        };
        assert!(ink(Face::Orbitron800) > ink(Face::Orbitron500));
        let mut pm = Pixmap::new(80, 24).unwrap();
        draw(&mut pm, Face::Mono, 14.0, 2.0, 18.0, (255, 255, 255), 1.0, "A1", 0.0);
        assert!(pm.data().iter().any(|&b| b > 0));
    }
}
