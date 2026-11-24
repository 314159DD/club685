//! The analysis half of the pipeline, shared by the live loop and the dry run: decides which part of the window to
//! read next, runs `analyze`, and turns the reading into BPM mask rects (frame coordinates).

use club685_capture::{analysis_rect, FrameBuf};
use club685_core::{analyze, mask_rect, BpmMaskMode, FrameReading, Rect, TrackerState};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BpmOpt {
    Off,
    Mode(BpmMaskMode),
}

impl BpmOpt {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => BpmOpt::Off,
            "hide" => BpmOpt::Mode(BpmMaskMode::CoverFully),
            "one-decimal" => BpmOpt::Mode(BpmMaskMode::OneDecimal),
            "no-decimals" => BpmOpt::Mode(BpmMaskMode::NoDecimals),
            _ => return None,
        })
    }
}

/// A BPM mask rect is kept this long after the header text was last found, so one bad frame does not flash it.
const BPM_HOLD_MS: u64 = 500;

pub struct Output {
    pub reading: FrameReading,
    pub bpm_masks: [Option<Rect>; 2],
    pub analyze_ms: f64,
}

pub struct Pipeline {
    pub state: TrackerState,
    pub bpm: BpmOpt,
    need_full: bool,
    held: [Option<(Rect, u64)>; 2],
}

impl Pipeline {
    pub fn new(bpm: BpmOpt) -> Self {
        Self { state: TrackerState::new(), bpm, need_full: true, held: [None; 2] }
    }

    /// Forget everything (window moved, resized, DPI changed).
    pub fn reset(&mut self) {
        self.state.reset();
        self.need_full = true;
        self.held = [None; 2];
    }

    /// Which window rect the next capture read should copy: the whole window while no layout is known (layout
    /// discovery), otherwise only the band plus the header windows.
    pub fn next_read(&self, frame_w: i32, frame_h: i32) -> Rect {
        match (self.need_full, self.state.layout()) {
            (false, Some(l)) if l.width == frame_w && l.height == frame_h => analysis_rect(l),
            _ => Rect { x0: 0, y0: 0, x1: frame_w, y1: frame_h },
        }
    }

    pub fn process(&mut self, buf: &FrameBuf, now_ms: u64) -> Option<Output> {
        let frame = buf.bgra()?;
        let t = Instant::now();
        let reading = analyze(&frame, &mut self.state);
        let analyze_ms = t.elapsed().as_secs_f64() * 1000.0;
        self.need_full = reading.layout.is_none();
        let mut bpm_masks = [None; 2];
        if let BpmOpt::Mode(mode) = self.bpm {
            for d in 0..2 {
                if let Some(r) = reading.decks[d].bpm_rect {
                    self.held[d] = Some((mask_rect(&frame, r, mode), now_ms));
                }
                if let Some((r, at)) = self.held[d] {
                    if now_ms.saturating_sub(at) <= BPM_HOLD_MS && reading.layout.is_some() {
                        bpm_masks[d] = Some(r);
                    }
                }
            }
        }
        Some(Output { reading, bpm_masks, analyze_ms })
    }
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn phase_str(d: &club685_core::DeckReading) -> String {
    match (&d.phase, &d.phase_raw) {
        (Some(p), _) => format!("{:.2}", p.phase),
        (None, Some(p)) => format!("({:.2})", p.phase),
        _ => "-".into(),
    }
}

fn bars_str(d: &club685_core::DeckReading) -> String {
    d.bars.map_or("-".into(), |(b, n)| format!("{b}.{n}"))
}

/// The reading part of a status line.
pub fn reading_str(r: &FrameReading) -> String {
    let cursor = r.cursor.or(r.cursor_raw).map_or("-".into(), |c| format!("{c:.1}"));
    let master = r.master.map_or("-".into(), |m| format!("{}", m + 1));
    format!(
        "playhead={cursor} phase=[{} {}] master={master} bars=[{} {}]",
        phase_str(&r.decks[0]),
        phase_str(&r.decks[1]),
        bars_str(&r.decks[0]),
        bars_str(&r.decks[1])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_option_names() {
        assert_eq!(BpmOpt::parse("off"), Some(BpmOpt::Off));
        assert_eq!(BpmOpt::parse("hide"), Some(BpmOpt::Mode(BpmMaskMode::CoverFully)));
        assert_eq!(BpmOpt::parse("one-decimal"), Some(BpmOpt::Mode(BpmMaskMode::OneDecimal)));
        assert_eq!(BpmOpt::parse("no-decimals"), Some(BpmOpt::Mode(BpmMaskMode::NoDecimals)));
        assert_eq!(BpmOpt::parse("x"), None);
    }

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert_eq!(percentile(&v, 0.95), 5.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
    }
}
