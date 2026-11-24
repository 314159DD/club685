//! Beat grid estimator and the 3-frame phase tracker (spec section 4), plus the wide-zoom bars-only fallback.
//!
//! Inputs are the cursor-excluded x positions of the red (bar) and grey (beat) markers of one row.

const TOL: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PhaseKind {
    /// Full grid fit from bar and beat markers.
    Grid,
    /// Wide zoom: only bar markers are drawn. `phase` is the cursor fraction of the red spacing times 4 and is
    /// only a beat position when the visible bar markers are one bar apart; take the beat from the Bars text.
    BarsOnly,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseReading {
    pub kind: PhaseKind,
    /// Beat position in the bar, 0.0 <= phase < 4.0 (beat 1 starts at 0.0).
    pub phase: f32,
    /// 1..=4, `floor(phase) + 1`.
    pub beat: u8,
    /// Pixels per beat (`bar_spacing / 4`).
    pub beat_spacing: f32,
    /// Pixels between the visible bar markers (one bar for `Grid`).
    pub bar_spacing: f32,
    pub red: usize,
    pub grey: usize,
}

struct Fit {
    beat: f32,
    origin: f32,
}

fn valid_bar(bar: f32, reds: &[f32], grid: &[f32]) -> bool {
    let beat = bar / 4.0;
    if !(12.0..=400.0).contains(&beat) {
        return false;
    }
    let r0 = reds[0];
    for &r in reds {
        let d = (r - r0) / bar;
        if (d - d.round()).abs() * bar > TOL {
            return false;
        }
    }
    let mut seen = 0u8;
    for &m in grid {
        let t = (m - r0) / beat;
        if (t - t.round()).abs() * beat > TOL {
            return false;
        }
        seen |= 1 << (t.round() as i64).rem_euclid(4);
    }
    if seen != 0b1111 {
        return false;
    }
    let need = 3.max((grid.len() - 1) / 2);
    let good = grid.windows(2).filter(|p| ((p[1] - p[0]) - beat).abs() <= TOL).count();
    good >= need
}

fn candidates(reds: &[f32]) -> Vec<f32> {
    let mut c: Vec<f32> = Vec::new();
    for p in reds.windows(2) {
        let g = p[1] - p[0];
        for k in 1..=4 {
            c.push(g / k as f32);
        }
    }
    let span = reds[reds.len() - 1] - reds[0];
    let base = c.clone();
    for b in base {
        let m = (span / b).round();
        if m >= 1.0 {
            c.push(span / m);
        }
    }
    c
}

/// Least squares fit `x = origin + beat * i`, with `i` the beat index relative to the first red marker.
fn least_squares(r0: f32, beat: f32, grid: &[f32]) -> Fit {
    let n = grid.len() as f32;
    let (mut si, mut sx, mut sii, mut six) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &m in grid {
        let i = ((m - r0) / beat).round();
        si += i;
        sx += m;
        sii += i * i;
        six += i * m;
    }
    let den = n * sii - si * si;
    if den.abs() < 1e-6 {
        return Fit { beat, origin: r0 };
    }
    let b = (n * six - si * sx) / den;
    let a = (sx - b * si) / n;
    Fit { beat: b, origin: a }
}

fn sorted(v: &[f32]) -> Vec<f32> {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s
}

fn estimate_grid(red: &[f32], grey: &[f32]) -> Option<Fit> {
    if red.len() < 2 || red.len() + grey.len() < 6 {
        return None;
    }
    let reds = sorted(red);
    let all: Vec<f32> = reds.iter().chain(grey.iter()).copied().collect();
    let grid = sorted(&all);
    let ok: Vec<f32> = candidates(&reds).into_iter().filter(|&b| valid_bar(b, &reds, &grid)).collect();
    if ok.is_empty() {
        return None;
    }
    let lo = ok.iter().cloned().fold(f32::MAX, f32::min) / 4.0;
    let hi = ok.iter().cloned().fold(f32::MIN, f32::max) / 4.0;
    if hi - lo > TOL {
        return None;
    }
    let beat = ok.iter().sum::<f32>() / ok.len() as f32 / 4.0;
    Some(least_squares(reds[0], beat, &grid))
}

fn finish(origin: f32, beat: f32, cursor: f32) -> f32 {
    let t = ((cursor - origin) / beat).rem_euclid(4.0);
    if (t - t.round()).abs() * beat <= 1.0 {
        t.round().rem_euclid(4.0)
    } else {
        t
    }
}

/// Bars only: the smallest consistent red spacing. Every gap must be a whole multiple of it.
fn estimate_bars_only(red: &[f32], cursor: f32) -> Option<PhaseReading> {
    if red.len() < 2 {
        return None;
    }
    let reds = sorted(red);
    let gaps: Vec<f32> = reds.windows(2).map(|p| p[1] - p[0]).collect();
    let g = gaps.iter().cloned().fold(f32::MAX, f32::min);
    if g < 48.0 {
        return None;
    }
    let mut total_m = 0.0;
    for &x in &gaps {
        let m = (x / g).round();
        if m < 1.0 {
            return None;
        }
        total_m += m;
    }
    let span = reds[reds.len() - 1] - reds[0];
    let s = span / total_m;
    for &x in &gaps {
        let m = (x / s).round();
        if (x - m * s).abs() > TOL {
            return None;
        }
    }
    let frac = ((cursor - reds[0]) / s).rem_euclid(1.0);
    let phase = frac * 4.0;
    Some(PhaseReading {
        kind: PhaseKind::BarsOnly,
        phase,
        beat: (phase.floor() as u8 % 4) + 1,
        beat_spacing: s / 4.0,
        bar_spacing: s,
        red: reds.len(),
        grey: 0,
    })
}

/// Phase of the cursor from one row's markers. Falls back to bars only when no beat marker is visible.
pub fn estimate(red: &[f32], grey: &[f32], cursor: f32) -> Option<PhaseReading> {
    if let Some(fit) = estimate_grid(red, grey) {
        let phase = finish(fit.origin, fit.beat, cursor);
        return Some(PhaseReading {
            kind: PhaseKind::Grid,
            phase,
            beat: (phase.floor() as u8 % 4) + 1,
            beat_spacing: fit.beat,
            bar_spacing: fit.beat * 4.0,
            red: red.len(),
            grey: grey.len(),
        });
    }
    if grey.is_empty() {
        return estimate_bars_only(red, cursor);
    }
    None
}

/// Outputs a reading only after 3 consecutive frames whose beat spacing agrees within 2 px.
#[derive(Clone, Debug, Default)]
pub struct PhaseTracker {
    count: u8,
    last: f32,
}

impl PhaseTracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// Returns the reading once the tracker is stable.
    pub fn observe(&mut self, r: Option<PhaseReading>) -> Option<PhaseReading> {
        let Some(r) = r else {
            self.count = 0;
            return None;
        };
        if self.count > 0 && (self.last - r.beat_spacing).abs() <= TOL {
            self.count = self.count.saturating_add(1);
        } else {
            self.count = 1;
        }
        self.last = r.beat_spacing;
        if self.count >= 3 {
            Some(r)
        } else {
            None
        }
    }
}
