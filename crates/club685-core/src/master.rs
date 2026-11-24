//! MASTER label detector: orange text (ff8c19 / f08317) in the deck header, plus the 2-frame tracker.

use crate::frame::{Bgra, Rect};

#[inline]
fn orange(r: u8, g: u8, b: u8) -> bool {
    r >= 180 && (60..=150).contains(&g) && b <= 60
}

/// Narrowest run of adjacent orange columns that can belong to the MASTER label. The label is about 36 px
/// wide at 96 dpi; the mixer's TRIM indicator next to deck 1 is a 2 px orange line and must not count
/// (live 2026-11-23: it made deck 1 look "active" and hid deck 2's real MASTER).
const MIN_LABEL_RUN: i32 = 6;

/// Counts orange pixels (R >= 180, 60 <= G <= 150, B <= 60) in `rect`, keeping only runs of adjacent
/// orange columns at least `MIN_LABEL_RUN` wide (letters of the label sit 1-2 px apart, so a gap of up
/// to 2 empty columns still joins a run).
pub fn orange_pixels(f: &Bgra, rect: Rect) -> u32 {
    let (x0, x1) = (rect.x0.max(0), rect.x1.min(f.w()));
    if x1 <= x0 {
        return 0;
    }
    let mut cols = vec![0u32; (x1 - x0) as usize];
    for y in rect.y0.max(0)..rect.y1.min(f.h()) {
        for x in x0..x1 {
            let (r, g, b) = f.rgb(x, y);
            if orange(r, g, b) {
                cols[(x - x0) as usize] += 1;
            }
        }
    }
    let mut total = 0;
    let mut i = 0usize;
    while i < cols.len() {
        if cols[i] == 0 {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut sum = 0;
        let mut gap = 0;
        while i < cols.len() && gap <= 2 {
            if cols[i] > 0 {
                sum += cols[i];
                end = i;
                gap = 0;
            } else {
                gap += 1;
            }
            i += 1;
        }
        if (end - start + 1) as i32 >= MIN_LABEL_RUN {
            total += sum;
        }
    }
    total
}

/// Orange bounding box inside `rect`, if any orange pixel exists.
pub fn orange_bounds(f: &Bgra, rect: Rect) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for y in rect.y0.max(0)..rect.y1.min(f.h()) {
        for x in rect.x0.max(0)..rect.x1.min(f.w()) {
            let (r, g, b) = f.rgb(x, y);
            if orange(r, g, b) {
                out = Some(match out {
                    None => Rect { x0: x, y0: y, x1: x + 1, y1: y + 1 },
                    Some(c) => Rect { x0: c.x0.min(x), y0: c.y0.min(y), x1: c.x1.max(x + 1), y1: c.y1.max(y + 1) },
                });
            }
        }
    }
    out
}

const ACTIVE_MIN: u32 = 20;
const INACTIVE_MAX: u32 = 2;

/// Deck that is active while the other is not. `None` when either deck is in between (unknown).
pub fn master_from_counts(c: [u32; 2]) -> Option<usize> {
    let active = [c[0] >= ACTIVE_MIN, c[1] >= ACTIVE_MIN];
    let idle = [c[0] <= INACTIVE_MAX, c[1] <= INACTIVE_MAX];
    if active[0] && idle[1] {
        Some(0)
    } else if active[1] && idle[0] {
        Some(1)
    } else {
        None
    }
}

/// Switches only after 2 agreeing frames and keeps the last master through unknown frames for 250 ms.
#[derive(Clone, Debug, Default)]
pub struct MasterTracker {
    candidate: Option<usize>,
    count: u8,
    master: Option<usize>,
    last_seen_ms: u64,
}

impl MasterTracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn observe(&mut self, raw: Option<usize>, now_ms: u64) -> Option<usize> {
        match raw {
            Some(m) => {
                if self.candidate == Some(m) {
                    self.count = self.count.saturating_add(1);
                } else {
                    self.candidate = Some(m);
                    self.count = 1;
                }
                if self.count >= 2 {
                    self.master = Some(m);
                    self.last_seen_ms = now_ms;
                }
            }
            None => {
                self.count = 0;
                self.candidate = None;
                if now_ms.saturating_sub(self.last_seen_ms) > 250 {
                    self.master = None;
                }
            }
        }
        self.master
    }
}
