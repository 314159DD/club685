//! The live mode preview in the LCD panel: a miniature of the waveform band as CLUB685 will present it.
//!
//! Two parts. `PhaseClock` keeps the beat position of both decks running when the measured readings are frozen
//! (rekordbox is not in front) or stale, and snaps back to the measured values as soon as they are live again.
//! `paint_preview` draws the band with the real compositor (`club685-render`) at mini scale from a stylised
//! waveform frame, so the meters, gutter, dividers and header are exactly what the overlay will show.

use crate::prefs::{PhaseSetting, WaveMode};
use crate::state::{Snapshot, StateKind};
use club685_core::{Bgra, Row};
use club685_render::{Composer, MeterKind, Scene, SceneDeck};
use std::collections::VecDeque;
use tiny_skia::Pixmap;

/// Logical size of the preview inside the LCD.
pub const PREV_W: f32 = 406.0;
pub const PREV_H: f32 = 66.0;
/// Tempo used when nothing has ever been measured (the panel is then labelled "demo").
pub const DEMO_BPM: f64 = 128.0;
/// BPM integer shown in the preview when the core never reported one.
pub const FALLBACK_BPM: u32 = 143;
/// Without a new snapshot for this long the readings count as stale.
const STALE_S: f64 = 1.0;
/// Window of the phase-rate estimate.
const RATE_WINDOW_S: f64 = 2.0;

// ---- layout choice ----------------------------------------------------------------------------------------------

/// What the band looks like for a waveform mode and a phase meter setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandLayout {
    /// Normal: rekordbox is untouched, no meter can be drawn.
    Untouched,
    /// Split: two half-width viewports with gutter and dividers, meters in the header strip when on.
    Split(Option<MeterKind>),
    /// Hide Both without a meter: a black band.
    Black,
    /// Hide Both with a meter: both meters stacked in the middle.
    Central(MeterKind),
}

pub fn band_layout(wave: WaveMode, phase: PhaseSetting) -> BandLayout {
    let meter = match phase {
        PhaseSetting::Off => None,
        PhaseSetting::Box => Some(MeterKind::Box),
        PhaseSetting::Line => Some(MeterKind::Line),
    };
    match (wave, meter) {
        (WaveMode::Normal, _) => BandLayout::Untouched,
        (WaveMode::Split, m) => BandLayout::Split(m),
        (WaveMode::HideBoth, None) => BandLayout::Black,
        (WaveMode::HideBoth, Some(k)) => BandLayout::Central(k),
    }
}

pub fn wave_name(w: WaveMode) -> &'static str {
    match w {
        WaveMode::Normal => "Normal",
        WaveMode::Split => "Split",
        WaveMode::HideBoth => "Hide Both",
    }
}

pub fn meter_name(p: PhaseSetting) -> &'static str {
    match p {
        PhaseSetting::Off => "no meter",
        PhaseSetting::Box => "Box",
        PhaseSetting::Line => "Line",
    }
}

// ---- phase clock ------------------------------------------------------------------------------------------------

/// Everything the preview needs for one paint.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewState {
    /// True while the numbers are measured from rekordbox right now.
    pub live: bool,
    /// True while the numbers are made up (no tempo ever measured).
    pub demo: bool,
    /// Beat position 0..4 for the meters. `None` (live only) draws an unlit meter.
    pub phase: [Option<f32>; 2],
    /// Continuous beat count, drives the scrolling of the stylised waveform.
    pub beats: [f64; 2],
    pub bars: [Option<(u32, u8)>; 2],
    pub master: Option<usize>,
    /// Integer BPM shown in the BPM readouts.
    pub bpm: [u32; 2],
}

impl PreviewState {
    /// Static state straight from a snapshot, without any extrapolation.
    pub fn from_snapshot(s: &Snapshot) -> Self {
        let known = s.bpm_int.iter().flatten().next().copied().unwrap_or(FALLBACK_BPM);
        let bpm = [s.bpm_int[0].unwrap_or(known), s.bpm_int[1].unwrap_or(known)];
        let phase = [s.decks[0].phase, s.decks[1].phase];
        Self {
            live: s.kind == StateKind::Active,
            demo: false,
            phase,
            beats: [phase[0].unwrap_or(0.0) as f64, phase[1].unwrap_or(0.0) as f64],
            bars: [s.decks[0].bars, s.decks[1].bars],
            master: s.master,
            bpm,
        }
    }
}

impl Default for PreviewState {
    fn default() -> Self {
        Self::from_snapshot(&Snapshot::default())
    }
}

#[derive(Default)]
struct DeckClock {
    /// `(time, continuous beats)` of the live measurements of the last two seconds.
    hist: VecDeque<(f64, f64)>,
    /// Continuous beat count; `total mod 4` is the phase.
    total: f64,
    init: bool,
    /// Last measured running tempo in beats per second.
    rate: Option<f64>,
    /// Bar number at beat count 0 (so the bar counter keeps counting).
    bar0: i64,
    bpm: Option<u32>,
}

impl DeckClock {
    fn bpm_rate(&self) -> Option<f64> {
        self.bpm.map(|b| b as f64 / 60.0)
    }
}

/// Keeps the beat position of both decks moving between measurements.
pub struct PhaseClock {
    decks: [DeckClock; 2],
    t_last: Option<f64>,
    last_rev: u64,
    last_rev_t: f64,
    master: Option<usize>,
    started: bool,
}

impl Default for PhaseClock {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseClock {
    pub fn new() -> Self {
        let mut decks = [DeckClock::default(), DeckClock::default()];
        for d in &mut decks {
            d.bar0 = 1;
        }
        // the two demo decks start out of step
        decks[1].total = 1.5;
        Self { decks, t_last: None, last_rev: 0, last_rev_t: 0.0, master: None, started: false }
    }

    /// Tempo to run at when not live: the measured rate, else the BPM, else the other deck's, else `None` (demo).
    fn rate_for(&self, d: usize) -> Option<f64> {
        let o = 1 - d;
        self.decks[d].rate.or(self.decks[d].bpm_rate()).or(self.decks[o].rate).or(self.decks[o].bpm_rate())
    }

    /// Advances the clock to `t` seconds and returns what to draw. Call it on every repaint.
    pub fn step(&mut self, t: f64, snap: &Snapshot) -> PreviewState {
        if !self.started {
            self.started = true;
            self.last_rev = snap.rev;
            self.last_rev_t = t;
        }
        if snap.rev != self.last_rev {
            self.last_rev = snap.rev;
            self.last_rev_t = t;
        }
        let stale = t - self.last_rev_t > STALE_S;
        let live = snap.kind == StateKind::Active && !stale;
        let dt = self.t_last.map_or(0.0, |l| (t - l).clamp(0.0, 10.0));
        self.t_last = Some(t);
        if live {
            if let Some(m) = snap.master {
                self.master = Some(m);
            }
        }
        for d in 0..2 {
            if let Some(b) = snap.bpm_int[d] {
                self.decks[d].bpm = Some(b);
            }
        }
        let demo = !live && self.rate_for(0).is_none() && self.rate_for(1).is_none();
        for d in 0..2 {
            let measured = if live { snap.decks[d].phase } else { None };
            match measured {
                Some(p) => {
                    let p = p as f64;
                    let dk = &mut self.decks[d];
                    if !dk.init {
                        dk.total = p;
                        dk.init = true;
                    } else {
                        let cur = dk.total.rem_euclid(4.0);
                        dk.total += (p - cur + 2.0).rem_euclid(4.0) - 2.0;
                    }
                    if let Some((bar, _)) = snap.decks[d].bars {
                        dk.bar0 = bar as i64 - (dk.total / 4.0).floor() as i64;
                    }
                    dk.hist.push_back((t, dk.total));
                    while dk.hist.front().is_some_and(|&(ht, _)| t - ht > RATE_WINDOW_S) {
                        dk.hist.pop_front();
                    }
                    if let (Some(&(t0, b0)), Some(&(t1, b1))) = (dk.hist.front(), dk.hist.back()) {
                        if t1 - t0 >= 0.3 {
                            let r = (b1 - b0) / (t1 - t0);
                            // 30..360 BPM counts as running; a paused deck keeps the last tempo
                            if (0.5..=6.0).contains(&r) {
                                dk.rate = Some(r);
                            }
                        }
                    }
                }
                None => {
                    let rate = self.rate_for(d).unwrap_or(DEMO_BPM / 60.0);
                    let dk = &mut self.decks[d];
                    dk.total += rate * dt;
                    // a gap in the live readings must not feed stale samples into the next estimate
                    dk.hist.clear();
                }
            }
        }
        let known = self.decks[0].bpm.or(self.decks[1].bpm).unwrap_or(FALLBACK_BPM);
        let mut out = PreviewState {
            live,
            demo,
            phase: [None; 2],
            beats: [self.decks[0].total, self.decks[1].total],
            bars: [None; 2],
            master: if live { snap.master } else { self.master.or(Some(0)) },
            bpm: [self.decks[0].bpm.unwrap_or(known), self.decks[1].bpm.unwrap_or(known)],
        };
        for d in 0..2 {
            if live {
                out.phase[d] = snap.decks[d].phase;
                out.bars[d] = snap.decks[d].bars;
            } else {
                let dk = &self.decks[d];
                let ph = dk.total.rem_euclid(4.0);
                out.phase[d] = Some(ph as f32);
                let bar = (dk.bar0 + (dk.total / 4.0).floor() as i64).max(1) as u32;
                out.bars[d] = Some((bar, ph.floor() as u8 + 1));
            }
        }
        out
    }
}

// ---- stylised band ----------------------------------------------------------------------------------------------

const BG: (u8, u8, u8) = (4, 10, 7);
const ROW_BG: (u8, u8, u8) = (7, 20, 14);
const WAVE: [((u8, u8, u8), (u8, u8, u8)); 2] = [((47, 191, 140), (159, 255, 224)), ((61, 143, 208), (166, 216, 255))];
/// Columns per bar of the stylised waveform, so it repeats seamlessly after four beats.
const COLS_PER_BAR: f64 = 88.0;

fn hash(mut x: u32) -> f32 {
    x = x.wrapping_mul(0x9e37_79b1) ^ (x >> 15);
    x = x.wrapping_mul(0x85eb_ca6b) ^ (x >> 13);
    (x & 0xffff) as f32 / 65535.0
}

struct Frame {
    data: Vec<u8>,
    w: i32,
    h: i32,
}

impl Frame {
    fn new(w: i32, h: i32) -> Self {
        let mut data = vec![255u8; (w * h * 4) as usize];
        for p in data.chunks_exact_mut(4) {
            p[0] = BG.2;
            p[1] = BG.1;
            p[2] = BG.0;
        }
        Self { data, w, h }
    }
    fn put(&mut self, x: i32, y: i32, c: (u8, u8, u8)) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        self.data[i] = c.2;
        self.data[i + 1] = c.1;
        self.data[i + 2] = c.0;
    }
    fn vline(&mut self, x: i32, y0: i32, y1: i32, c: (u8, u8, u8)) {
        for y in y0..=y1 {
            self.put(x, y, c);
        }
    }
}

/// Draws two stylised waveform rows with beat ticks and a centre playhead. Each row scrolls with its deck's beats.
fn draw_band(f: &mut Frame, rows: &[Row; 2], beats: [f64; 2], scale: f32) {
    let beat_px = 44.0 * scale as f64;
    let cx = f.w / 2;
    let thick = (scale.round() as i32).max(1);
    for (d, row) in rows.iter().enumerate() {
        let (y0, y1) = (row.top, row.bottom);
        for y in y0..=y1 {
            for x in 0..f.w {
                f.put(x, y, ROW_BG);
            }
        }
        let mid = (y0 + y1) as f32 / 2.0;
        let half_max = (y1 - y0 + 1) as f32 * 0.46;
        let (outer, inner) = WAVE[d];
        let mut prev_beat = ((beats[d] * beat_px - cx as f64 - 1.0) / beat_px).floor() as i64;
        for x in 0..f.w {
            let timeline = beats[d] * beat_px + (x - cx) as f64;
            let bp = timeline / beat_px;
            let u = bp.rem_euclid(4.0);
            let col = (u / 4.0 * COLS_PER_BAR).floor() as u32;
            let n = hash(col.wrapping_add(d as u32 * 977));
            let kick = (1.0 - bp.rem_euclid(1.0) as f32).powi(2);
            let amp = (0.16 + 0.42 * kick + 0.34 * n).clamp(0.08, 1.0);
            let half = (half_max * amp).max(1.0);
            let (t, b) = ((mid - half).round() as i32, (mid + half).round() as i32);
            f.vline(x, t.max(y0), b.min(y1), outer);
            let ih = half * 0.45;
            f.vline(x, ((mid - ih).round() as i32).max(y0), ((mid + ih).round() as i32).min(y1), inner);
            // beat ticks at the row edges, red and longer on the bar line
            let beat = bp.floor() as i64;
            if beat != prev_beat {
                let bar = beat.rem_euclid(4) == 0;
                let len = if bar { ((y1 - y0) as f32 * 0.4) as i32 } else { (3.0 * scale).round() as i32 };
                let col = if bar { (242, 9, 6) } else { (150, 165, 158) };
                f.vline(x, y0, (y0 + len).min(y1), col);
                f.vline(x, (y1 - len).max(y0), y1, col);
            }
            prev_beat = beat;
        }
    }
    // playhead over both rows
    for k in 0..thick {
        f.vline(cx - thick / 2 + k, rows[0].top - 1, rows[1].bottom + 1, (245, 247, 250));
    }
}

/// Paints the band preview at physical size `PREV_W * scale` x `PREV_H * scale`.
pub fn paint_preview(wave: WaveMode, phase: PhaseSetting, st: &PreviewState, scale: f32) -> Pixmap {
    let w = (PREV_W * scale).round() as i32;
    let h = (PREV_H * scale).round() as i32;
    let mut black = Pixmap::new(w as u32, h as u32).expect("preview pixmap");
    black.fill(tiny_skia::Color::BLACK);

    let rh = ((h - 4) / 2).max(4);
    let fh = 2 * rh + 4;
    let rows = [Row { top: 1, bottom: rh }, Row { top: rh + 3, bottom: 2 * rh + 2 }];
    let layout = band_layout(wave, phase);
    if layout == BandLayout::Black {
        return black;
    }
    let mut frame = Frame::new(w, fh);
    draw_band(&mut frame, &rows, st.beats, scale);

    let mut pm = black;
    let mut comp = Composer::new();
    let decks = [SceneDeck { phase: st.phase[0], bars: st.bars[0] }, SceneDeck { phase: st.phase[1], bars: st.bars[1] }];
    let bgra = Bgra::new(&frame.data, w as usize, fh as usize, w as usize * 4).expect("preview frame");
    let mk = |meter: Option<MeterKind>, dpi: f32| Scene { frame: &bgra, rows, cursor: Some(w as f32 / 2.0), decks, master: st.master, dpi: (dpi * scale).round() as u32, meter };
    match layout {
        BandLayout::Untouched => {
            for y in 0..fh.min(h) {
                let s = (y * w * 4) as usize;
                let n = (w * 4) as usize;
                let dst = &mut pm.data_mut()[s..s + n];
                for (o, i) in dst.chunks_exact_mut(4).zip(frame.data[s..s + n].chunks_exact(4)) {
                    o[0] = i[2];
                    o[1] = i[1];
                    o[2] = i[0];
                    o[3] = 255;
                }
            }
        }
        BandLayout::Split(meter) => {
            if comp.split(&mk(meter, 60.0), w, h) {
                if let Some(p) = comp.pixmap() {
                    pm = p.clone();
                }
            }
        }
        BandLayout::Central(kind) => {
            if comp.central(&mk(Some(kind), 72.0), w, h) {
                if let Some(p) = comp.pixmap() {
                    pm = p.clone();
                }
            }
        }
        BandLayout::Black => {}
    }
    pm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_snap(rev: u64, phases: [Option<f32>; 2]) -> Snapshot {
        let mut s = Snapshot::default();
        s.kind = StateKind::Active;
        s.rev = rev;
        s.decks[0].phase = phases[0];
        s.decks[1].phase = phases[1];
        s.master = Some(0);
        s
    }

    fn frozen_snap(rev: u64) -> Snapshot {
        let mut s = Snapshot::default();
        s.kind = StateKind::Ready;
        s.rev = rev;
        s
    }

    /// Feeds 1 s of live readings at 143 BPM (2.383 beats/s), 33 ms apart. Returns the time and the last phase.
    fn run_live(c: &mut PhaseClock, secs: f64) -> (f64, f32) {
        let rate = 143.0 / 60.0;
        let mut t = 0.0;
        let mut p = 0.0;
        let mut rev = 1;
        while t <= secs {
            p = ((t * rate + 0.5).rem_euclid(4.0)) as f32;
            c.step(t, &live_snap(rev, [Some(p), Some(p)]));
            rev += 1;
            t += 0.033;
        }
        (t, p)
    }

    #[test]
    fn extrapolation_wraps_inside_zero_to_four() {
        let mut c = PhaseClock::new();
        let (t0, _) = run_live(&mut c, 1.5);
        let mut wrapped = 0;
        let mut prev = None;
        for i in 1..200 {
            let t = t0 + i as f64 * 0.033;
            let out = c.step(t, &frozen_snap(1000));
            assert!(!out.live && !out.demo);
            let p = out.phase[0].unwrap();
            assert!((0.0..4.0).contains(&p), "phase {p}");
            if prev.is_some_and(|q: f32| p < q) {
                wrapped += 1;
            }
            prev = Some(p);
        }
        // 6.6 s at 2.38 beats/s is about four wraps
        assert!((3..=5).contains(&wrapped), "wrapped {wrapped} times");
    }

    #[test]
    fn extrapolation_keeps_the_measured_tempo() {
        let mut c = PhaseClock::new();
        let (t0, _) = run_live(&mut c, 1.5);
        // one second of frozen time moves the meter by the measured 2.38 beats
        let a = c.step(t0, &frozen_snap(1000)).phase[0].unwrap() as f64;
        let b = c.step(t0 + 1.0, &frozen_snap(1000)).phase[0].unwrap() as f64;
        let moved = (b - a).rem_euclid(4.0);
        assert!((moved - 143.0 / 60.0).abs() < 0.08, "moved {moved}");
    }

    #[test]
    fn snaps_back_to_the_measured_value() {
        let mut c = PhaseClock::new();
        let (t0, _) = run_live(&mut c, 1.5);
        let out = c.step(t0 + 3.0, &frozen_snap(1000));
        assert!(!out.live);
        let out = c.step(t0 + 3.033, &live_snap(2000, [Some(1.25), None]));
        assert!(out.live);
        assert_eq!(out.phase[0], Some(1.25));
        // a deck without a measured phase draws an unlit meter while live
        assert_eq!(out.phase[1], None);
    }

    #[test]
    fn stale_readings_count_as_frozen() {
        let mut c = PhaseClock::new();
        let (t0, _) = run_live(&mut c, 1.0);
        let same = live_snap(5, [Some(0.5), Some(0.5)]);
        c.step(t0, &same);
        assert!(c.step(t0 + 0.5, &same).live);
        assert!(!c.step(t0 + 1.6, &same).live);
    }

    #[test]
    fn demo_runs_at_128_bpm_until_a_tempo_is_known() {
        let mut c = PhaseClock::new();
        let a = c.step(0.0, &frozen_snap(1));
        assert!(a.demo);
        let b = c.step(1.0, &frozen_snap(1));
        assert!(b.demo);
        let moved = (b.phase[0].unwrap() - a.phase[0].unwrap()).rem_euclid(4.0) as f64;
        assert!((moved - 128.0 / 60.0).abs() < 0.01, "moved {moved}");
        // a known BPM ends the demo
        let mut s = frozen_snap(1);
        s.bpm_int = [Some(120), None];
        let c2 = c.step(2.0, &s);
        assert!(!c2.demo);
        assert_eq!(c2.bpm, [120, 120]);
        let d = c.step(3.0, &s);
        let moved = (d.phase[0].unwrap() - c2.phase[0].unwrap()).rem_euclid(4.0) as f64;
        assert!((moved - 2.0).abs() < 0.01, "moved {moved}");
    }

    #[test]
    fn frozen_bars_keep_counting() {
        let mut c = PhaseClock::new();
        let mut s = live_snap(1, [Some(0.0), Some(0.0)]);
        s.decks[0].bars = Some((85, 1));
        c.step(0.0, &s);
        let out = c.step(4.0, &frozen_snap(1));
        // 4 s at 128 BPM demo tempo is 8.5 beats: two bars further
        let (bar, beat) = out.bars[0].unwrap();
        assert!((86..=88).contains(&bar), "bar {bar}");
        assert!((1..=4).contains(&beat));
    }

    #[test]
    fn layout_choice_per_mode() {
        use PhaseSetting::*;
        use WaveMode::*;
        for p in [Off, Box, Line] {
            assert_eq!(band_layout(Normal, p), BandLayout::Untouched);
        }
        assert_eq!(band_layout(Split, Off), BandLayout::Split(None));
        assert_eq!(band_layout(Split, Box), BandLayout::Split(Some(MeterKind::Box)));
        assert_eq!(band_layout(Split, Line), BandLayout::Split(Some(MeterKind::Line)));
        assert_eq!(band_layout(HideBoth, Off), BandLayout::Black);
        assert_eq!(band_layout(HideBoth, Box), BandLayout::Central(MeterKind::Box));
        assert_eq!(band_layout(HideBoth, Line), BandLayout::Central(MeterKind::Line));
    }

    #[test]
    fn black_band_and_meter_layouts_paint() {
        let st = PreviewState::default();
        let size = (PREV_W as u32, PREV_H as u32);
        let black = paint_preview(WaveMode::HideBoth, PhaseSetting::Off, &st, 1.0);
        assert_eq!((black.width(), black.height()), size);
        assert!(black.pixels().iter().all(|p| p.red() == 0 && p.green() == 0 && p.blue() == 0));
        for (w, p) in [(WaveMode::Normal, PhaseSetting::Off), (WaveMode::Split, PhaseSetting::Box), (WaveMode::HideBoth, PhaseSetting::Line)] {
            let pm = paint_preview(w, p, &st, 1.25);
            assert_eq!((pm.width(), pm.height()), ((PREV_W * 1.25).round() as u32, (PREV_H * 1.25).round() as u32));
            assert!(pm.pixels().iter().any(|p| p.green() > 0), "{w:?} {p:?} is empty");
        }
    }
}
