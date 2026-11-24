//! Layout discovery from pixels: the waveform band, the two deck rows, each deck's MASTER label window and BPM
//! header text rect.
//!
//! Row height is user adjustable in rekordbox, so nothing here assumes y=151. The rows are anchored on the bar
//! and beat triangles at their top and bottom edges (a full-signature hit at the same column on a top edge and on
//! the next bottom edge below it). The band is the black stripe around the rows. The MASTER and BPM positions are
//! measured 96 dpi windows relative to the band bottom and the deck pane edges; they are only starting windows.

use crate::frame::{maxc, Bgra, Rect};
use crate::markers::signature_at;

/// One deck's waveform row. `top` and `bottom` are the inclusive edge rows that carry the marker triangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub top: i32,
    pub bottom: i32,
}

impl Row {
    pub fn height(&self) -> i32 {
        self.bottom - self.top + 1
    }
    /// Height of the top/bottom strip where beat markers live (about 0.093 * row).
    pub fn quiet(&self) -> i32 {
        ((self.height() as f32 * 0.093).round() as i32).max(3)
    }
    /// Minimum playhead pixels inside the two strips (0.14 * row).
    pub fn edge_hits(&self) -> i32 {
        ((self.height() as f32 * 0.14).round() as i32).max(3)
    }
    /// Minimum playhead pixels over the row height (0.91 * row).
    pub fn continuity(&self) -> i32 {
        ((self.height() as f32 * 0.91).round() as i32).max(8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    /// Black waveform band, both rows plus the black margin around them.
    pub band: Rect,
    pub rows: [Row; 2],
    /// Search window for each deck's MASTER label.
    pub master: [Rect; 2],
}

/// x offset of deck 2's pane: the two decks are `(w - MIXER) / 2` wide with a fixed mixer strip between them.
const MIXER_W: i32 = 108;

/// Left edge of a deck pane for a window of width `w`.
pub fn pane_left(w: i32, deck: usize) -> i32 {
    if deck == 0 {
        0
    } else {
        (w + MIXER_W) / 2
    }
}

fn row_is_dark(f: &Bgra, y: i32) -> bool {
    if y < 0 || y >= f.h() {
        return false;
    }
    let row = f.row(y);
    let mut n = 0;
    let mut dark = 0;
    let mut x = 0;
    while x < f.width {
        let i = x * 4;
        n += 1;
        if row[i].max(row[i + 1]).max(row[i + 2]) <= 0x14 {
            dark += 1;
        }
        x += 4;
    }
    dark * 100 >= n * 96
}

/// Full-signature hits `(column, y, dir)` for edge rows `y < y_end`. Stride-2 prefilter on the triangle pixel and
/// on the gap/line pixels 4 and 5 rows further in, then the signature test on the pixel and its two neighbours.
fn scan_hits(f: &Bgra, y_end: i32) -> Vec<(i32, i32, i8)> {
    let w = f.w();
    let h = f.h();
    let mut hits: Vec<(i32, i32, i8)> = Vec::new();
    for y in 0..y_end.min(h) {
        let row = f.row(y);
        let down = if y + 5 < h { Some((f.row(y + 4), f.row(y + 5))) } else { None };
        let up = if y >= 5 { Some((f.row(y - 4), f.row(y - 5))) } else { None };
        let mut x = 4;
        while x < w - 4 {
            let i = x as usize * 4;
            let (b, g, r) = (row[i], row[i + 1], row[i + 2]);
            let red = r >= 0xe0 && g <= 0x30 && b <= 0x30;
            let grey = r.max(g).max(b) - r.min(g).min(b) <= 6 && (0x6c..=0x8c).contains(&r);
            if red || grey {
                // gap pixel dark; the line pixel is on the centre column, which may be one column off `x`
                let gap_line = |pair: Option<(&[u8], &[u8])>| {
                    pair.map_or(false, |(gap, line)| {
                        gap[i].max(gap[i + 1]).max(gap[i + 2]) <= 0x28
                            && (line[i - 2].max(line[i + 2]).max(line[i + 6])) >= 0x40
                    })
                };
                let d = gap_line(down);
                let u = gap_line(up);
                if d || u {
                    for c in (x - 1)..=(x + 1) {
                        if d && signature_at(f, c, y, 1).is_some() {
                            hits.push((c, y, 1));
                        }
                        if u && signature_at(f, c, y, -1).is_some() {
                            hits.push((c, y, -1));
                        }
                    }
                }
            }
            x += 2;
        }
    }
    hits
}

/// Pairs top hits with the nearest bottom hit below in the same column and keeps the two best rows.
fn rows_from_hits(mut hits: Vec<(i32, i32, i8)>) -> Option<[Row; 2]> {
    hits.sort_unstable();
    hits.dedup();
    let mut pairs: Vec<((i32, i32), u32)> = Vec::new();
    let mut i = 0;
    while i < hits.len() {
        let c = hits[i].0;
        let mut j = i;
        while j < hits.len() && hits[j].0 == c {
            j += 1;
        }
        let group = &hits[i..j];
        for &(_, ty, d) in group {
            if d != 1 {
                continue;
            }
            let by = group.iter().filter(|&&(_, by, bd)| bd == -1 && by - ty >= 20).map(|&(_, by, _)| by).min();
            if let Some(by) = by {
                match pairs.iter_mut().find(|p| p.0 == (ty, by)) {
                    Some(p) => p.1 += 1,
                    None => pairs.push(((ty, by), 1)),
                }
            }
        }
        i = j;
    }
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    let mut rows: Vec<Row> = Vec::new();
    for ((t, b), _) in pairs {
        if rows.iter().all(|r| t > r.bottom || b < r.top) {
            rows.push(Row { top: t, bottom: b });
        }
        if rows.len() == 2 {
            break;
        }
    }
    if rows.len() != 2 {
        return None;
    }
    rows.sort_by_key(|r| r.top);
    Some([rows[0], rows[1]])
}

/// Finds the two rows and the band. Returns `None` when fewer than two rows with full-signature markers exist.
pub fn find_layout(f: &Bgra) -> Option<Layout> {
    let w = f.w();
    let h = f.h();
    if w < 400 || h < 200 {
        return None;
    }
    // The band sits in the upper part of the window; scan that first and the whole frame only if it is not there.
    let upper = (h * 11 / 20).max(200).min(h);
    let rows = match rows_from_hits(scan_hits(f, upper)) {
        Some(r) => r,
        None if upper < h => rows_from_hits(scan_hits(f, h))?,
        None => return None,
    };
    let mut y0 = rows[0].top;
    for _ in 0..8 {
        if row_is_dark(f, y0 - 1) {
            y0 -= 1;
        } else {
            break;
        }
    }
    let mut y1 = rows[1].bottom;
    for _ in 0..8 {
        if row_is_dark(f, y1 + 1) {
            y1 += 1;
        } else {
            break;
        }
    }
    let band = Rect { x0: 0, y0, x1: w, y1: y1 + 1 };
    let band_bottom = y1;
    let my0 = band_bottom + 20;
    let my1 = band_bottom + 50;
    let master = [
        Rect { x0: w / 2 - 140, y0: my0, x1: w / 2 - 30, y1: my1 },
        Rect { x0: w - 140, y0: my0, x1: w, y1: my1 },
    ];
    Some(Layout { width: w, height: h, band, rows, master })
}

/// Ink pixel of the grey header text: near neutral and visible on the black header.
#[inline]
pub fn header_ink(f: &Bgra, x: i32, y: i32) -> bool {
    let p = f.rgb(x, y);
    crate::frame::spread(p) <= 24 && maxc(p) >= 0x30
}

/// Finds the BPM text word ("143.00") in a deck header. The window starts where the BPM sits in the 96 dpi
/// layout (deck pane left + ~216 px, 28..40 px below the band bottom); the word is the first run of ink columns
/// after the window start, split from the following key text by a gap of 4 or more blank columns.
pub fn find_bpm_rect(f: &Bgra, l: &Layout, deck: usize) -> Option<Rect> {
    let band_bottom = l.band.y1 - 1;
    let wy0 = band_bottom + 26;
    let wy1 = band_bottom + 40;
    let p = pane_left(l.width, deck);
    let wx0 = p + 190;
    let wx1 = (p + 330).min(l.width);
    let col_ink = |x: i32| (wy0..wy1).any(|y| header_ink(f, x, y));
    let mut x = wx0;
    while x < wx1 && !col_ink(x) {
        x += 1;
    }
    if x >= wx1 {
        return None;
    }
    let x0 = x;
    let mut last = x;
    let mut gap = 0;
    while x < wx1 {
        if col_ink(x) {
            last = x;
            gap = 0;
        } else {
            gap += 1;
            if gap >= 4 {
                break;
            }
        }
        x += 1;
    }
    let width = last - x0 + 1;
    if !(20..=70).contains(&width) {
        return None;
    }
    let mut ya = i32::MAX;
    let mut yb = i32::MIN;
    for y in wy0..wy1 {
        if (x0..=last).any(|x| header_ink(f, x, y)) {
            ya = ya.min(y);
            yb = yb.max(y);
        }
    }
    if ya == i32::MAX || !(6..=14).contains(&(yb - ya + 1)) {
        return None;
    }
    Some(Rect { x0, y0: ya, x1: last + 1, y1: yb + 1 })
}
