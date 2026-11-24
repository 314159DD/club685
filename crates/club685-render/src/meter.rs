//! The CDJ-style phase meters (box and line) plus the identity label and the bars readout (spec section 7).

use crate::geometry::*;
use crate::shapes::*;
use crate::text::{self, Face};
use tiny_skia::Pixmap;

/// Master row orange and its dim variant.
pub const ORANGE: (u8, u8, u8) = (250, 107, 4);
pub const ORANGE_DIM: (u8, u8, u8) = (115, 43, 3);
/// Own deck: beat 1 red, beats 2-4 blue, dim blue.
pub const BEAT1_RED: (u8, u8, u8) = (242, 9, 6);
pub const BLUE: (u8, u8, u8) = (0, 107, 184);
pub const BLUE_DIM: (u8, u8, u8) = (0, 43, 77);
/// Inner fill of an unlit box.
pub const INNER_FILL: (u8, u8, u8) = (4, 5, 5);
/// Line meter: bar tick red and playhead bar.
pub const TICK_RED: (u8, u8, u8) = (245, 9, 6);
pub const PLAYHEAD: (u8, u8, u8) = (245, 247, 250);
pub const BAR_TEXT: (u8, u8, u8) = (230, 235, 242);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Master,
    /// Player number 1 or 2.
    Player(u8),
}

impl Role {
    pub fn label(self) -> String {
        match self {
            Role::Master => "MASTER".to_string(),
            Role::Player(n) => format!("PLAYER {n}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MeterSpec {
    pub kind: MeterKind,
    pub role: Role,
    /// Beat position 0.0..4.0; `None` draws an unlit meter.
    pub phase: Option<f32>,
    pub bars: Option<(u32, u8)>,
}

fn c(t: (u8, u8, u8)) -> tiny_skia::Color {
    rgb(t.0, t.1, t.2)
}

/// Draws the meter itself into the box `x, y, w, h` (physical pixels).
pub fn draw_meter(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, spec: &MeterSpec, k: f32) {
    match spec.kind {
        MeterKind::Box => draw_box(pm, x, y, w, h, spec, k),
        MeterKind::Line => draw_line(pm, x, y, w, h, spec, k),
    }
}

fn draw_box(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, spec: &MeterSpec, k: f32) {
    let gap = (6.0 * k).max(2.0);
    let bw = (w - 3.0 * gap) / 4.0;
    let radius = h * 0.24;
    let border = (2.0 * k).max(1.0);
    let lit = spec.phase.map(lit_block);
    for i in 0..4 {
        let bx = x + i as f32 * (bw + gap);
        let (on, off) = match spec.role {
            Role::Master => (ORANGE, ORANGE_DIM),
            Role::Player(_) => (if i == 0 { BEAT1_RED } else { BLUE }, BLUE_DIM),
        };
        if lit == Some(i) {
            fill_rrect(pm, bx, y, bw, h, radius, &solid(c(on)));
        } else {
            fill_rrect(pm, bx, y, bw, h, radius, &solid(c(off)));
            fill_rrect(pm, bx + border, y + border, bw - 2.0 * border, h - 2.0 * border, (radius - border).max(1.0), &solid(c(INNER_FILL)));
        }
    }
}

fn draw_line(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, spec: &MeterSpec, k: f32) {
    let mid = y + h / 2.0;
    let thick = (2.0 * k).max(1.0);
    let base = match spec.role {
        Role::Master => ORANGE_DIM,
        Role::Player(_) => BLUE_DIM,
    };
    let tick_col = match spec.role {
        Role::Master => ORANGE,
        Role::Player(_) => BLUE,
    };
    fill_rect(pm, x, mid - thick / 2.0, w, thick, &solid(c(base)));
    if let Some(phase) = spec.phase {
        let tw = (2.0 * k).max(1.0);
        for (tx, is_bar) in line_ticks(w, phase) {
            let th = if is_bar { h * 0.86 } else { h * 0.46 };
            let col = if is_bar { TICK_RED } else { tick_col };
            let px = (x + tx - tw / 2.0).clamp(x, x + w - tw);
            fill_rect(pm, px, mid - th / 2.0, tw, th, &solid(c(col)));
        }
    }
    let pw = (3.0 * k).max(2.0);
    fill_rect(pm, x + w / 2.0 - pw / 2.0, y, pw, h, &solid(c(PLAYHEAD)));
}

const LABEL_PX: f32 = 13.0;
const GAP: f32 = 10.0;

/// Width of label + gap + meter + gap + bars text at scale 1.0 (the label and text slots have fixed widths so the
/// meter does not jump around while the numbers change).
pub fn header_total_unit(kind: MeterKind, meter_w_logical: f32) -> f32 {
    text::measure(Face::LabelBold, LABEL_PX, "PLAYER 2", 0.0) + GAP + meter_w_logical + GAP + text::measure(Face::LabelBold, LABEL_PX, "888.8 Bars", 0.0)
        + 0.0 * meter_height_logical(kind)
}

/// Draws label, meter and bars text centred inside the pane `x, y, w, h` (physical pixels). `k` is the meter scale
/// before fitting; it shrinks if the content would be wider than `w - 12`.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, spec: &MeterSpec, meter_w_logical: f32, k0: f32) {
    let total1 = header_total_unit(spec.kind, meter_w_logical);
    let k = fit_scale(k0, total1 * k0, w - 12.0);
    let px = LABEL_PX * k;
    let label_w = text::measure(Face::LabelBold, LABEL_PX, "PLAYER 2", 0.0) * k;
    let text_w = text::measure(Face::LabelBold, LABEL_PX, "888.8 Bars", 0.0) * k;
    let gap = GAP * k;
    let mw = meter_w_logical * k;
    let mh = meter_height_logical(spec.kind) * k;
    let total = label_w + gap + mw + gap + text_w;
    let left = x + (w - total) / 2.0;
    let mid = y + h / 2.0;
    let cap = text::cap_height(Face::LabelBold, px);
    let baseline = mid + cap / 2.0;
    text::draw(pm, Face::LabelBold, px, left, baseline, ORANGE, 1.0, &spec.role.label(), 0.0);
    draw_meter(pm, left + label_w + gap, mid - mh / 2.0, mw, mh, spec, k);
    text::draw(pm, Face::LabelBold, px, left + label_w + gap + mw + gap, baseline, BAR_TEXT, 1.0, &bars_text(spec.bars), 0.0);
}
