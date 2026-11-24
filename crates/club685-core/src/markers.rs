//! Bar and beat marker detection by full signature.
//!
//! A marker is a small triangle on the row edge, one black gap pixel, then a line:
//!
//! ```text
//! bar  : ff0000 triangle (5 px wide at the edge row, 3/1/1 over the next rows), gap, ffffff line
//! beat : 7c7c7c triangle (3 px wide at the edge row, 1 px below), gap, 4c4c4c line
//! ```
//! The triangle points away from the row edge: down at the top edge, up at the bottom edge. Because the whole
//! signature is checked, red pixels of an RGB-mode waveform body never count as markers.

use crate::frame::{maxc, spread, Bgra};
use crate::layout::Row;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    Bar,
    Beat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub x: f32,
    pub kind: MarkerKind,
    /// Seen at both the top and the bottom edge.
    pub both: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RowMarkers {
    pub markers: Vec<Marker>,
}

impl RowMarkers {
    pub fn bars(&self) -> Vec<f32> {
        self.markers.iter().filter(|m| m.kind == MarkerKind::Bar).map(|m| m.x).collect()
    }
    pub fn beats(&self) -> Vec<f32> {
        self.markers.iter().filter(|m| m.kind == MarkerKind::Beat).map(|m| m.x).collect()
    }
}

#[inline]
fn is_red((r, g, b): (u8, u8, u8)) -> bool {
    r >= 0xe0 && g <= 0x30 && b <= 0x30
}

#[inline]
fn neutral_in(p: (u8, u8, u8), lo: u8, hi: u8) -> bool {
    spread(p) <= 6 && p.0 >= lo && p.0 <= hi
}

#[inline]
fn dark(p: (u8, u8, u8)) -> bool {
    maxc(p) <= 0x28
}

/// Checks the full marker signature with the triangle's edge row at `e`. `dir` is +1 when the marker hangs down
/// from a top edge, -1 when it rises from a bottom edge. `c` is the centre column.
pub fn signature_at(f: &Bgra, c: i32, e: i32, dir: i32) -> Option<MarkerKind> {
    let p = |dx: i32, k: i32| f.rgb(c + dx, e + dir * k);
    let first = p(0, 0);
    if is_red(first) {
        if !(is_red(p(0, 1)) && is_red(p(0, 2)) && is_red(p(0, 3))) {
            return None;
        }
        // row 0: five red px, row 2: cc ff cc, side pixels already tapered
        if !(is_red(p(-1, 0)) && is_red(p(1, 0)) && is_red(p(-2, 0)) && is_red(p(2, 0))) {
            return None;
        }
        if p(-2, 2).0 >= 0x60 || p(2, 2).0 >= 0x60 || p(-1, 3).0 >= 0x80 || p(1, 3).0 >= 0x80 {
            return None;
        }
        if !dark(p(0, 4)) {
            return None;
        }
        let white = |q: (u8, u8, u8)| q.0 >= 0xe8 && q.1 >= 0xe8 && q.2 >= 0xe8;
        if white(p(0, 5)) && white(p(0, 6)) {
            return Some(MarkerKind::Bar);
        }
        return None;
    }
    if neutral_in(first, 0x6c, 0x8c) {
        if !(neutral_in(p(0, 1), 0x6c, 0x8c) && neutral_in(p(0, 2), 0x6c, 0x8c)) {
            return None;
        }
        if !(neutral_in(p(-1, 0), 0x60, 0x84) && neutral_in(p(1, 0), 0x60, 0x84)) {
            return None;
        }
        if !neutral_in(p(0, 3), 0x60, 0x8c) || !dark(p(0, 4)) {
            return None;
        }
        // the tapered flanks of a real triangle are dim; a flat grey block is not a marker
        if p(-2, 1).0 > 0x60 || p(2, 1).0 > 0x60 || p(-1, 2).0 > 0x60 || p(1, 2).0 > 0x60 {
            return None;
        }
        if neutral_in(p(0, 5), 0x40, 0x58) && neutral_in(p(0, 6), 0x40, 0x58) {
            return Some(MarkerKind::Beat);
        }
    }
    None
}

#[inline]
fn candidate_px((r, g, b): (u8, u8, u8)) -> bool {
    (r >= 0xe0 && g <= 0x30 && b <= 0x30) || (r.max(g).max(b) - r.min(g).min(b) <= 6 && (0x6c..=0x8c).contains(&r))
}

/// All markers on one row, merged from the top and the bottom edge. A marker seen at only one edge is kept (for
/// example when the "NN.NBars" text covers its top triangle), because the full signature already rules out
/// false positives.
pub fn scan_row(f: &Bgra, row: &Row) -> RowMarkers {
    let w = f.w();
    let mut top: Vec<(i32, MarkerKind)> = Vec::new();
    let mut bot: Vec<(i32, MarkerKind)> = Vec::new();
    for x in 3..(w - 3) {
        if candidate_px(f.rgb(x, row.top)) {
            if let Some(k) = signature_at(f, x, row.top, 1) {
                top.push((x, k));
            }
        }
        if candidate_px(f.rgb(x, row.bottom)) {
            if let Some(k) = signature_at(f, x, row.bottom, -1) {
                bot.push((x, k));
            }
        }
    }
    let mut out: Vec<Marker> = Vec::new();
    let mut used = vec![false; bot.len()];
    for &(xt, kt) in &top {
        let mut found = None;
        for (j, &(xb, kb)) in bot.iter().enumerate() {
            if !used[j] && kb == kt && (xb - xt).abs() <= 2 {
                found = Some(j);
                break;
            }
        }
        match found {
            Some(j) => {
                used[j] = true;
                out.push(Marker { x: (xt + bot[j].0) as f32 / 2.0, kind: kt, both: true });
            }
            None => out.push(Marker { x: xt as f32, kind: kt, both: false }),
        }
    }
    for (j, &(xb, kb)) in bot.iter().enumerate() {
        if !used[j] {
            out.push(Marker { x: xb as f32, kind: kb, both: false });
        }
    }
    out.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
    RowMarkers { markers: out }
}
