//! Playhead detection and the 3-frame lock (spec section 3, adapted per section 8 for 6.8.5).
//!
//! 6.8.5 differences from the 7.2 rule: the playhead is neutral grey in both states (e0e0e0 playing, 707070
//! paused), so the neutral test is `max - min <= 8` and the brightness floor is 96; there is no red-cursor
//! branch; any column whose edge-strip pixels are red is a bar line and is rejected.

use crate::frame::{maxc, spread, Bgra};
use crate::layout::{Layout, Row};

const NEUTRAL: u8 = 8;
const MIN_LEVEL: i32 = 96;
const MIN_CONTRAST: i32 = 50;

#[inline]
fn grey(f: &Bgra, x: i32, y: i32) -> i32 {
    let p = f.rgb(x, y);
    if spread(p) <= NEUTRAL {
        p.0.min(p.1).min(p.2) as i32
    } else {
        0
    }
}

#[inline]
fn is_red(f: &Bgra, x: i32, y: i32) -> bool {
    let (r, g, b) = f.rgb(x, y);
    r >= 180 && g <= 45 && b <= 45
}

/// True when column `x` looks like the playhead inside `row`.
fn column_in_row(f: &Bgra, x: i32, row: &Row) -> bool {
    let q = row.quiet();
    let mut edge = 0;
    let mut total = 0;
    let mut levels: Vec<i32> = Vec::with_capacity(16);
    for y in row.top..=row.bottom {
        let in_strip = y < row.top + q || y > row.bottom - q;
        if in_strip && is_red(f, x, y) {
            return false; // bar line
        }
        let g = grey(f, x, y);
        if g >= MIN_LEVEL && g - grey(f, x - 3, y).min(grey(f, x + 3, y)) >= MIN_CONTRAST {
            total += 1;
            if in_strip {
                edge += 1;
                levels.push(g);
            }
        }
    }
    if edge < row.edge_hits() {
        return false;
    }
    if total >= row.continuity() {
        return true;
    }
    // Playhead partly hidden by, or lying next to, a bright waveform: count every pixel at the strip level.
    levels.sort_unstable();
    let med = levels[levels.len() / 2];
    let mut n = 0;
    for y in row.top..=row.bottom {
        let g = grey(f, x, y);
        if g > 0 && (g - med).abs() <= 2 {
            n += 1;
        }
    }
    n >= row.continuity()
}

/// Detects the playhead x in one frame. 1..3 columns spanning at most 2 px give their average.
pub fn detect_cursor(f: &Bgra, l: &Layout) -> Option<f32> {
    let w = f.w();
    let probe_y = l.rows[0].top + l.rows[0].quiet() / 2;
    let mut cols: Vec<i32> = Vec::new();
    for x in 4..(w - 4) {
        let p = f.rgb(x, probe_y);
        if spread(p) > NEUTRAL || (maxc(p) as i32) < MIN_LEVEL {
            continue;
        }
        if l.rows.iter().all(|r| column_in_row(f, x, r)) {
            cols.push(x);
            if cols.len() > 3 {
                return None;
            }
        }
    }
    if cols.is_empty() || cols[cols.len() - 1] - cols[0] > 2 {
        return None;
    }
    Some(cols.iter().sum::<i32>() as f32 / cols.len() as f32)
}

/// Locks the playhead after 3 agreeing frames and then freezes it until `reset`.
#[derive(Clone, Debug, Default)]
pub struct CursorTracker {
    candidate: Option<f32>,
    count: u8,
    locked: Option<f32>,
}

impl CursorTracker {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn locked(&self) -> Option<f32> {
        self.locked
    }
    pub fn candidate(&self) -> Option<f32> {
        self.candidate
    }
    /// A frame without a reading neither counts nor resets (a bar line can sit on the playhead for a frame).
    pub fn observe(&mut self, x: Option<f32>) {
        if self.locked.is_some() {
            return;
        }
        let Some(x) = x else { return };
        match self.candidate {
            Some(c) if (c - x).abs() <= 1.0 => self.count += 1,
            _ => self.count = 1,
        }
        self.candidate = Some(x);
        if self.count >= 3 {
            self.locked = Some(x);
        }
    }
}
