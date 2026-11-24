//! Split and central composites. Everything is drawn into a premultiplied RGBA pixmap (always opaque) and
//! converted to BGRA for `UpdateLayeredWindow`.

use crate::geometry::*;
use crate::meter::{draw_header, MeterSpec, Role};
use club685_core::{Bgra, Row};
use tiny_skia::{Pixmap, PremultipliedColorU8};

pub const DIVIDER: (u8, u8, u8) = (61, 61, 61);

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneDeck {
    /// Beat position 0.0..4.0 (tracked phase).
    pub phase: Option<f32>,
    /// `(bar, beat)` from the Bars text.
    pub bars: Option<(u32, u8)>,
}

pub struct Scene<'a> {
    pub frame: &'a Bgra<'a>,
    pub rows: [Row; 2],
    /// Measured playhead x in frame coordinates.
    pub cursor: Option<f32>,
    pub decks: [SceneDeck; 2],
    pub master: Option<usize>,
    pub dpi: u32,
    /// `None` = phase meter off.
    pub meter: Option<MeterKind>,
}

#[derive(Default)]
pub struct Composer {
    pm: Option<Pixmap>,
}

fn role_of(deck: usize, master: Option<usize>) -> Role {
    if master == Some(deck) {
        Role::Master
    } else {
        Role::Player(deck as u8 + 1)
    }
}

impl Composer {
    pub fn new() -> Self {
        Self::default()
    }

    fn canvas(&mut self, w: i32, h: i32) -> Option<&mut Pixmap> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let need = self.pm.as_ref().map_or(true, |p| p.width() as i32 != w || p.height() as i32 != h);
        if need {
            self.pm = Pixmap::new(w as u32, h as u32);
        }
        let pm = self.pm.as_mut()?;
        pm.fill(tiny_skia::Color::BLACK);
        Some(pm)
    }

    pub fn size(&self) -> Option<(i32, i32)> {
        self.pm.as_ref().map(|p| (p.width() as i32, p.height() as i32))
    }

    pub fn pixmap(&self) -> Option<&Pixmap> {
        self.pm.as_ref()
    }

    /// Premultiplied BGRA bytes of the last composite.
    pub fn bgra(&self, out: &mut Vec<u8>) {
        out.clear();
        if let Some(pm) = &self.pm {
            out.reserve(pm.data().len());
            for px in pm.data().chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
    }

    /// Split view for an overlay of `out_w` x `out_h` pixels. Returns false when it cannot be drawn (no measured
    /// playhead).
    pub fn split(&mut self, scene: &Scene, out_w: i32, out_h: i32) -> bool {
        let Some(cursor) = scene.cursor else { return false };
        let row_h = scene.rows.iter().map(|r| r.height() + 2).max().unwrap_or(0);
        let g = split_geometry(out_w, out_h, scene.dpi, scene.meter.is_some(), row_h);
        let frame_w = scene.frame.w();
        let Some(pm) = self.canvas(out_w, out_h) else { return false };
        let (sx, _) = viewport_source(cursor, g.vw, frame_w);
        let vw = g.vw.min(frame_w);

        for d in 0..2 {
            let row = &scene.rows[d];
            // crop: one pixel above and below the row (the playhead sticks out there), cut evenly if too tall
            let full = row.height() + 2;
            let cut = full - g.content_h;
            let src_y0 = row.top - 1 + cut / 2;
            copy_crop(pm, scene.frame, sx, src_y0, vw, g.content_h, g.dest_x[d], g.dest_y);
        }
        let div = tiny_skia::PremultipliedColorU8::from_rgba(DIVIDER.0, DIVIDER.1, DIVIDER.2, 255).unwrap();
        // dividers at both gutter edges, and under the header
        let body_h = out_h - g.header;
        vline(pm, g.vw, g.header, body_h, div);
        vline(pm, out_w - g.vw - 1, g.header, body_h, div);
        if g.header > 0 {
            hline(pm, 0, out_w, g.header - 1, div);
        }

        if let Some(kind) = scene.meter {
            let pane_w = g.vw as f32;
            let k0 = meter_scale(scene.dpi, false);
            let mw = meter_width_logical(pane_w, scene.dpi, kind);
            for d in 0..2 {
                let spec = MeterSpec { kind, role: role_of(d, scene.master), phase: scene.decks[d].phase, bars: scene.decks[d].bars };
                draw_header(pm, g.dest_x[d] as f32, 0.0, pane_w, g.header as f32, &spec, mw, k0);
            }
        }
        true
    }

    /// Header strip of the split view alone (`out_w` x `header` px): both meters, a divider along the bottom edge.
    /// Used when the two waveforms are DWM thumbnails and only the meter row is drawn by us.
    pub fn split_header(&mut self, out_w: i32, header: i32, dpi: u32, kind: MeterKind, decks: [SceneDeck; 2], master: Option<usize>) -> bool {
        let Some(pm) = self.canvas(out_w, header) else { return false };
        let vw = viewport_width(out_w);
        let pane_w = vw as f32;
        let k0 = meter_scale(dpi, false);
        let mw = meter_width_logical(pane_w, dpi, kind);
        for d in 0..2 {
            let dest_x = if d == 0 { 0 } else { out_w - vw };
            let spec = MeterSpec { kind, role: role_of(d, master), phase: decks[d].phase, bars: decks[d].bars };
            draw_header(pm, dest_x as f32, 0.0, pane_w, header as f32, &spec, mw, k0);
        }
        let div = tiny_skia::PremultipliedColorU8::from_rgba(DIVIDER.0, DIVIDER.1, DIVIDER.2, 255).unwrap();
        hline(pm, 0, out_w, header - 1, div);
        true
    }

    /// Copies the last composite into `dst` as BGRA (`dst.len()` must be at least `w * h * 4`).
    pub fn write_bgra(&self, dst: &mut [u8]) -> bool {
        let Some(pm) = &self.pm else { return false };
        let src = pm.data();
        if dst.len() < src.len() {
            return false;
        }
        for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            o[0] = i[2];
            o[1] = i[1];
            o[2] = i[0];
            o[3] = i[3];
        }
        true
    }

    /// Central view: a black overlay with both meters stacked in the middle, master on top.
    pub fn central(&mut self, scene: &Scene, out_w: i32, out_h: i32) -> bool {
        let Some(kind) = scene.meter else { return self.canvas(out_w, out_h).is_some() };
        self.central_meters(out_w, out_h, scene.dpi, kind, scene.cursor, scene.decks, scene.master)
    }

    /// The central view without a frame: `cursor` is the x of the middle in overlay-local pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn central_meters(&mut self, out_w: i32, out_h: i32, dpi: u32, kind: MeterKind, cursor: Option<f32>, decks: [SceneDeck; 2], master: Option<usize>) -> bool {
        let Some(pm) = self.canvas(out_w, out_h) else { return false };
        let mut k = meter_scale(dpi, true);
        let mw = meter_width_logical(out_w as f32, dpi, kind);
        let row_of = |k: f32| meter_height_logical(kind) * k * 1.35;
        // both rows must fit into the band height
        if 2.0 * row_of(k) > out_h as f32 - 4.0 {
            k *= (out_h as f32 - 4.0) / (2.0 * row_of(k));
        }
        let row_h = row_of(k);
        // content is centred on the measured playhead when known, otherwise on the band, and kept inside
        let half = (crate::meter::header_total_unit(kind, mw) * k / 2.0 + 6.0).min(out_w as f32 / 2.0);
        let centre_x = cursor.map_or(out_w as f32 / 2.0, |c| c.clamp(0.0, out_w as f32));
        let px = (centre_x - half).clamp(0.0, out_w as f32 - 2.0 * half);
        let top = (out_h as f32 - 2.0 * row_h) / 2.0;
        for (slot, d) in central_order(master).into_iter().enumerate() {
            let spec = MeterSpec { kind, role: role_of(d, master), phase: decks[d].phase, bars: decks[d].bars };
            draw_header(pm, px, top + slot as f32 * row_h, 2.0 * half, row_h, &spec, mw, k);
        }
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_crop(pm: &mut Pixmap, f: &Bgra, sx: i32, sy: i32, w: i32, h: i32, dx: i32, dy: i32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for y in 0..h {
        let (fy, oy) = (sy + y, dy + y);
        if fy < 0 || fy >= f.h() || oy < 0 || oy >= ph {
            continue;
        }
        let src = f.row(fy);
        for x in 0..w {
            let (fx, ox) = (sx + x, dx + x);
            if fx < 0 || fx >= f.w() || ox < 0 || ox >= pw {
                continue;
            }
            let s = fx as usize * 4;
            let i = ((oy * pw + ox) * 4) as usize;
            data[i] = src[s + 2];
            data[i + 1] = src[s + 1];
            data[i + 2] = src[s];
            data[i + 3] = 255;
        }
    }
}

fn vline(pm: &mut Pixmap, x: i32, y0: i32, h: i32, c: PremultipliedColorU8) {
    let (w, ph) = (pm.width() as i32, pm.height() as i32);
    if x < 0 || x >= w {
        return;
    }
    for y in y0.max(0)..(y0 + h).min(ph) {
        pm.pixels_mut()[(y * w + x) as usize] = c;
    }
}

fn hline(pm: &mut Pixmap, x0: i32, x1: i32, y: i32, c: PremultipliedColorU8) {
    let (w, ph) = (pm.width() as i32, pm.height() as i32);
    if y < 0 || y >= ph {
        return;
    }
    for x in x0.max(0)..x1.min(w) {
        pm.pixels_mut()[(y * w + x) as usize] = c;
    }
}
