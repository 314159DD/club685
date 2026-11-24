//! BPM mask rectangles for the three precision modes, decided by glyph columns of the header text.
//!
//! The header BPM is drawn as "NNN.NN" (for example "143.00"). Rules, derived from spec section 7b and the glyph
//! columns measured on 6.8.5 (the "." is the only glyph whose ink stays in the bottom rows, `rect.y1-3..`):
//!
//! * a column belongs to the "." when its ink starts in the last 3 rows, the run is at most 4 columns wide and the
//!   columns on both sides are blank.
//! * `CoverFully`: the whole text rect plus a 1 px margin.
//! * `NoDecimals`: hide from the column just left of the "." onward, so only "143" stays visible.
//! * `OneDecimal`: hide from the column after the first decimal digit, so "143.0" stays visible.
//!
//! When the text does not have the "NNN.NN" shape (no isolated "." found, or fewer than two glyph runs on each
//! side of it) every mode falls back to `CoverFully` for unexpected text.

use crate::frame::{Bgra, Rect};
use crate::layout::header_ink;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BpmMaskMode {
    CoverFully,
    OneDecimal,
    NoDecimals,
}

/// Glyph columns of a header text rect: the "." run and the first decimal digit run, when found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BpmGlyphs {
    /// Inclusive column range of the ".".
    pub dot: (i32, i32),
    /// Inclusive column range of the first decimal digit.
    pub first_decimal: (i32, i32),
}

fn top_ink(f: &Bgra, x: i32, rect: &Rect) -> Option<i32> {
    (rect.y0..rect.y1).find(|&y| header_ink(f, x, y))
}

/// Finds the "." and the first decimal digit inside the BPM text rect.
pub fn find_glyphs(f: &Bgra, rect: Rect) -> Option<BpmGlyphs> {
    let low = rect.y1 - 3; // the "." only has ink in the last rows
    let tops: Vec<Option<i32>> = (rect.x0..rect.x1).map(|x| top_ink(f, x, &rect)).collect();
    let n = tops.len();
    // runs of columns whose ink starts in the low rows
    let mut i = 0;
    while i < n {
        if tops[i].map_or(false, |t| t >= low) {
            let mut j = i;
            while j + 1 < n && tops[j + 1].map_or(false, |t| t >= low) {
                j += 1;
            }
            let width = (j - i + 1) as i32;
            // needs digit ink on both sides (NNN before, NN after) and must be narrow
            let left_ink = tops[..i].iter().filter(|t| t.is_some()).count();
            let right_ink = tops[j + 1..].iter().filter(|t| t.is_some()).count();
            // isolated: the columns right next to it carry no ink (the tail of a "4" crossbar does not qualify)
            let isolated = (i == 0 || tops[i - 1].is_none()) && (j + 1 >= n || tops[j + 1].is_none());
            if width <= 4 && isolated && left_ink >= 8 && right_ink >= 8 {
                let dot = (rect.x0 + i as i32, rect.x0 + j as i32);
                // first decimal: skip blanks, then take the contiguous ink run
                let mut k = j + 1;
                while k < n && tops[k].is_none() {
                    k += 1;
                }
                let start = k;
                while k < n && tops[k].is_some() {
                    k += 1;
                }
                let end = k - 1;
                if start >= n || end < start {
                    return None;
                }
                let mut first = (rect.x0 + start as i32, rect.x0 + end as i32);
                // a run wider than one digit means the first decimal touches the next one: split at the middle
                // of what remains after the "."
                if first.1 - first.0 + 1 > 9 {
                    let remain = rect.x1 - 1 - dot.1;
                    first.1 = dot.1 + remain / 2;
                }
                return Some(BpmGlyphs { dot, first_decimal: first });
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    None
}

/// Mask rectangle (half-open) for the BPM text in `rect` under `mode`.
pub fn mask_rect(f: &Bgra, rect: Rect, mode: BpmMaskMode) -> Rect {
    let full = Rect { x0: rect.x0 - 1, y0: rect.y0 - 1, x1: rect.x1 + 2, y1: rect.y1 + 1 };
    let Some(g) = find_glyphs(f, rect) else { return full };
    match mode {
        BpmMaskMode::CoverFully => full,
        BpmMaskMode::NoDecimals => Rect { x0: g.dot.0 - 1, ..full },
        BpmMaskMode::OneDecimal => Rect { x0: g.first_decimal.1 + 1, ..full },
    }
}
