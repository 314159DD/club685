//! CLUB685 core: pixel analysis of one captured rekordbox 6.8.5 frame.
//!
//! `analyze(frame, state)` turns a BGRA frame into deck readings: layout, playhead, markers, phase, master deck,
//! the "NN.NBars" text and the BPM header rects. No capture, no windows, no UI.

pub mod bars_text;
pub mod bpm_mask;
pub mod cursor;
pub mod frame;
pub mod layout;
pub mod markers;
pub mod master;
pub mod phase;
mod templates;

use std::time::Instant;

pub use bars_text::{read_bars, read_bars_scored, BarsReading};
pub use bpm_mask::{mask_rect, BpmGlyphs, BpmMaskMode};
pub use cursor::{detect_cursor, CursorTracker};
pub use frame::{Bgra, Rect};
pub use layout::{find_bpm_rect, find_layout, Layout, Row};
pub use markers::{scan_row, Marker, MarkerKind, RowMarkers};
pub use master::{master_from_counts, orange_pixels, MasterTracker};
pub use phase::{PhaseKind, PhaseReading, PhaseTracker};

/// Markers this close to the playhead are ignored by the phase fit (the playhead itself reads as a line).
pub const CURSOR_EXCLUSION: f32 = 4.0;

/// State carried between frames.
#[derive(Debug)]
pub struct TrackerState {
    layout: Option<Layout>,
    cursor: CursorTracker,
    phase: [PhaseTracker; 2],
    master: MasterTracker,
    start: Instant,
}

impl Default for TrackerState {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackerState {
    pub fn new() -> Self {
        Self {
            layout: None,
            cursor: CursorTracker::new(),
            phase: [PhaseTracker::default(), PhaseTracker::default()],
            master: MasterTracker::default(),
            start: Instant::now(),
        }
    }

    /// Forget the layout and every tracker (window moved, resized, DPI changed).
    pub fn reset(&mut self) {
        self.layout = None;
        self.cursor.reset();
        for p in &mut self.phase {
            p.reset();
        }
        self.master.reset();
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeckReading {
    /// Every full-signature marker on this deck's row (cursor exclusion not applied).
    pub markers: Vec<Marker>,
    /// Phase from this frame alone.
    pub phase_raw: Option<PhaseReading>,
    /// Phase after the 3-frame tracker agreed.
    pub phase: Option<PhaseReading>,
    /// `(bar, beat)` from the "NN.NBars" text.
    pub bars: Option<(u32, u8)>,
    /// BPM header text rect (half-open) when found.
    pub bpm_rect: Option<Rect>,
    /// MASTER label search window.
    pub master_window: Rect,
    /// Orange pixels inside the MASTER window.
    pub orange_px: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FrameReading {
    pub layout: Option<Layout>,
    /// Playhead measured in this frame (only measured until locked).
    pub cursor_raw: Option<f32>,
    /// Playhead after the 3-frame lock.
    pub cursor: Option<f32>,
    pub decks: [DeckReading; 2],
    /// Master deck (0 or 1) from this frame alone.
    pub master_raw: Option<usize>,
    /// Master deck after the 2-frame tracker.
    pub master: Option<usize>,
}

/// Analyzes one frame using the wall clock for the master hold time.
pub fn analyze(frame: &Bgra<'_>, prev: &mut TrackerState) -> FrameReading {
    let now = prev.start.elapsed().as_millis() as u64;
    analyze_at(frame, prev, now)
}

/// Like [`analyze`] with an explicit clock in milliseconds.
pub fn analyze_at(frame: &Bgra<'_>, prev: &mut TrackerState, now_ms: u64) -> FrameReading {
    let mut out = FrameReading::default();

    // Layout: reuse the cached one while both rows still show markers, otherwise rediscover it.
    let mut scans: Option<[RowMarkers; 2]> = None;
    if let Some(l) = prev.layout {
        if l.width == frame.w() && l.height == frame.h() {
            let s = [scan_row(frame, &l.rows[0]), scan_row(frame, &l.rows[1])];
            if !s[0].markers.is_empty() && !s[1].markers.is_empty() {
                scans = Some(s);
            }
        }
    }
    if scans.is_none() {
        let found = find_layout(frame);
        if found != prev.layout {
            prev.reset();
            prev.layout = found;
        }
        if let Some(l) = found {
            scans = Some([scan_row(frame, &l.rows[0]), scan_row(frame, &l.rows[1])]);
        }
    }
    let (Some(l), Some(scans)) = (prev.layout, scans) else {
        return out;
    };
    out.layout = Some(l);

    // Playhead: measure until 3 frames agree, then keep the locked x.
    if prev.cursor.locked().is_none() {
        let x = detect_cursor(frame, &l);
        out.cursor_raw = x;
        prev.cursor.observe(x);
    }
    out.cursor = prev.cursor.locked();
    let cursor_used = out.cursor.or(out.cursor_raw).or(prev.cursor.candidate());

    let mut counts = [0u32; 2];
    for d in 0..2 {
        let row = &l.rows[d];
        let dr = &mut out.decks[d];
        dr.markers = scans[d].markers.clone();
        dr.master_window = l.master[d];
        dr.orange_px = orange_pixels(frame, l.master[d]);
        counts[d] = dr.orange_px;
        dr.bpm_rect = find_bpm_rect(frame, &l, d);
        if let Some(cx) = cursor_used {
            let keep = |m: &&Marker| (m.x - cx).abs() > CURSOR_EXCLUSION;
            let red: Vec<f32> = dr.markers.iter().filter(keep).filter(|m| m.kind == MarkerKind::Bar).map(|m| m.x).collect();
            let grey: Vec<f32> = dr.markers.iter().filter(keep).filter(|m| m.kind == MarkerKind::Beat).map(|m| m.x).collect();
            dr.phase_raw = phase::estimate(&red, &grey, cx);
            dr.bars = read_bars(frame, row, cx);
        }
        dr.phase = prev.phase[d].observe(dr.phase_raw);
    }
    out.master_raw = master_from_counts(counts);
    out.master = prev.master.observe(out.master_raw, now_ms);
    out
}
