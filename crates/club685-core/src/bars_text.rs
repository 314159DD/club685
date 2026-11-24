//! OCR of rekordbox's own "NN.NBars" readout (blue 007de1, right aligned just left of the playhead, top of each row).
//!
//! The font is a fixed raster at 96 dpi, so templates built from the fixtures (see `tools/gen_templates.py`) are
//! matched against the whole string: first the constant "Bars" suffix finds the right edge, then every digit cell
//! sits at a fixed offset from it (digits 8 px apart, the "." takes 4 px) and is matched against the ten digit
//! templates. Glyphs that touch the "." never need splitting because cells are placed by offset, not by column gaps.
//! The "." itself is not matched (a bar line and its triangle can sit on it).

use crate::layout::Row;
use crate::templates::{CELL_H, CELL_W, DIGITS, SUFFIX, SUFFIX_W};
use crate::Bgra;

/// Cell origins relative to the last ink column of the text: tenths, units, tens, hundreds.
const CELL_OFFS: [i32; 4] = [-37, -49, -57, -65];
/// Maximum SSD relative to template energy for a digit or the suffix to count as a match.
const MAX_REL_SSD: f32 = 0.45;

/// Text alpha at a pixel: the blue channel when the colour has the text hue (R <= 24, G about 0.45..0.66 of B).
#[inline]
fn alpha(f: &Bgra, x: i32, y: i32) -> i32 {
    let (r, g, b) = f.rgb(x, y);
    let (g, b) = (g as u32, b as u32);
    if r <= 24 && b >= 0x30 && g * 100 >= b * 45 && g * 100 <= b * 66 {
        b as i32
    } else {
        0
    }
}

fn ssd(f: &Bgra, x0: i32, y0: i32, w: usize, tmpl: &[u8]) -> (u64, u64) {
    let mut s = 0u64;
    let mut e = 0u64;
    for r in 0..CELL_H {
        for c in 0..w {
            let t = tmpl[r * w + c] as i32;
            let a = alpha(f, x0 + c as i32, y0 + r as i32);
            s += ((a - t) * (a - t)) as u64;
            e += (t * t) as u64;
        }
    }
    (s, e)
}

fn cell_ink(f: &Bgra, x0: i32, y0: i32) -> i32 {
    let mut s = 0;
    for r in 0..CELL_H as i32 {
        for c in 0..CELL_W as i32 {
            s += alpha(f, x0 + c, y0 + r);
        }
    }
    s
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarsReading {
    pub bar: u32,
    pub beat: u8,
    /// Worst relative SSD over the suffix and all digit cells (0 is a perfect match).
    pub worst_rel_ssd: f32,
}

/// Reads the Bars text of one deck row. `cursor_x` is the playhead column.
pub fn read_bars_scored(f: &Bgra, row: &Row, cursor_x: f32) -> Option<BarsReading> {
    let y0 = row.top + 1;
    let cx = cursor_x.round() as i32;
    // Right edge: slide the suffix template over the few columns left of the playhead.
    let mut best: Option<(i32, f32)> = None;
    for e in (cx - 8)..cx {
        let (s, en) = ssd(f, e - SUFFIX_W as i32 + 1, y0, SUFFIX_W, &SUFFIX);
        let rel = s as f32 / en as f32;
        if best.map_or(true, |b| rel < b.1) {
            best = Some((e, rel));
        }
    }
    let (e, suffix_rel) = best?;
    if suffix_rel > MAX_REL_SSD {
        return None;
    }
    let mut worst = suffix_rel;
    let mut digits: Vec<u32> = Vec::new();
    for (k, off) in CELL_OFFS.iter().enumerate() {
        let x0 = e + off - 1;
        let present = cell_ink(f, x0, y0) > 600;
        if !present {
            if k < 2 {
                return None; // tenths and units are always there
            }
            break;
        }
        let mut pick: Option<(usize, f32)> = None;
        for (d, t) in DIGITS.iter().enumerate() {
            let (s, en) = ssd(f, x0, y0, CELL_W, t);
            let rel = s as f32 / en as f32;
            if pick.map_or(true, |p| rel < p.1) {
                pick = Some((d, rel));
            }
        }
        let (d, rel) = pick?;
        if rel > MAX_REL_SSD {
            return None;
        }
        worst = worst.max(rel);
        digits.push(d as u32);
    }
    // digits are tenths, units, tens, hundreds
    let beat = digits[0];
    if !(1..=4).contains(&beat) {
        return None;
    }
    let mut bar = 0u32;
    let mut mul = 1;
    for d in &digits[1..] {
        bar += d * mul;
        mul *= 10;
    }
    Some(BarsReading { bar, beat: beat as u8, worst_rel_ssd: worst })
}

/// `(bar, beat)` of the "NN.NBars" text on a row, or `None` when it is not readable.
pub fn read_bars(f: &Bgra, row: &Row, cursor_x: f32) -> Option<(u32, u8)> {
    read_bars_scored(f, row, cursor_x).map(|r| (r.bar, r.beat))
}
