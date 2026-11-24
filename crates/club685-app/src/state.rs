//! State shared between the UI thread and the capture worker (behind one mutex).

use crate::install::ModuleState;
use crate::prefs::Prefs;
use crate::rbmod::Source;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateKind {
    Off,
    Setup,
    Searching,
    Ready,
    Active,
    Error,
}

impl StateKind {
    /// The short word on the LCD.
    pub fn word(self) -> &'static str {
        match self {
            StateKind::Off => "OFF",
            StateKind::Setup => "SETUP",
            StateKind::Searching => "SEARCHING",
            StateKind::Ready => "READY",
            StateKind::Active => "LOCKED",
            StateKind::Error => "CHECK",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckSnap {
    pub phase: Option<f32>,
    pub bars: Option<(u32, u8)>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub kind: StateKind,
    /// The plain-English status line.
    pub line: String,
    pub decks: [DeckSnap; 2],
    pub master: Option<usize>,
    pub decks_found: u8,
    pub cursor: Option<f32>,
    pub beat_px: Option<f32>,
    /// Live integer BPM per deck when a reader provides it (the BPM buttons show it).
    pub bpm_int: [Option<u32>; 2],
    /// State of the link to the rekordbox module (fresh datagrams).
    pub source: Source,
    /// Whether the module is in the rekordbox folder, and which folder.
    pub module: ModuleState,
    pub module_dir: Option<std::path::PathBuf>,
    /// What the layout grab is doing: none, or a one-shot dxgi frame.
    pub capture_path: String,
    /// rbmod datagrams per second.
    pub udp_pps: f32,
    /// Meter strip frames per second and CPU ms per frame (module mode).
    pub meter_fps: f32,
    pub meter_ms: f32,
    pub gate: String,
    pub rb_version: String,
    pub dpi: u32,
    pub window: String,
    pub layout: String,
    pub overlays: String,
    pub frames: u64,
    /// Bumped on every publish; the UI repaints when it changes.
    pub rev: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            kind: StateKind::Searching,
            line: "Looking for rekordbox…".to_string(),
            decks: [DeckSnap::default(); 2],
            master: None,
            decks_found: 0,
            cursor: None,
            beat_px: None,
            bpm_int: [None; 2],
            source: Source::Pending,
            module: ModuleState::Unknown,
            module_dir: None,
            capture_path: "none".to_string(),
            udp_pps: 0.0,
            meter_fps: 0.0,
            meter_ms: 0.0,
            gate: "-".to_string(),
            rb_version: "-".to_string(),
            dpi: 0,
            window: "-".to_string(),
            layout: "-".to_string(),
            overlays: "-".to_string(),
            frames: 0,
            rev: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct Shared {
    pub prefs: Prefs,
    pub snap: Snapshot,
    pub quit: bool,
    /// Set by the UI when the preferences changed and must be written.
    pub prefs_dirty: bool,
}

impl Shared {
    pub fn new(prefs: Prefs) -> Self {
        Self { prefs, snap: Snapshot::default(), quit: false, prefs_dirty: false }
    }
}

/// Plain-text diagnostics for the clipboard. Nothing in it identifies the user.
pub fn diagnostics_text(s: &Snapshot, p: &Prefs) -> String {
    let f = |o: Option<f32>, unit: &str| o.map_or("-".to_string(), |v| format!("{v:.1}{unit}"));
    let bars = |d: &DeckSnap| d.bars.map_or("-".to_string(), |(b, n)| format!("{b:02}.{n}"));
    let ph = |d: &DeckSnap| d.phase.map_or("-".to_string(), |v| format!("{v:.2}"));
    let mut t = String::new();
    t.push_str(&format!("CLUB685 {} for rekordbox 6.8.5\n", env!("CARGO_PKG_VERSION")));
    t.push_str(&format!("status      {} | {}\n", s.kind.word(), s.line));
    t.push_str(&format!("rekordbox   {}\n", s.rb_version));
    t.push_str(&format!("window      {} dpi {}\n", s.window, s.dpi));
    t.push_str(&format!("layout      {}\n", s.layout));
    t.push_str(&format!("playhead    {} beat {}\n", f(s.cursor, " px"), f(s.beat_px, " px")));
    t.push_str(&format!("deck 1      phase {} bars {}\n", ph(&s.decks[0]), bars(&s.decks[0])));
    t.push_str(&format!("deck 2      phase {} bars {}\n", ph(&s.decks[1]), bars(&s.decks[1])));
    t.push_str(&format!("master      {}\n", s.master.map_or("-".to_string(), |m| (m + 1).to_string())));
    t.push_str(&format!("gate        {} overlays {}\n", s.gate, s.overlays));
    let link = match s.source {
        Source::Pending => "waiting",
        Source::Module => "live (fresh packets)",
        Source::Silent => "silent",
    };
    t.push_str(&format!("module      {} | link {}
", s.module.word(), link));
    t.push_str(&format!("udp         {:.0} pkt/s meter {:.0} fps {:.2} ms
", s.udp_pps, s.meter_fps, s.meter_ms));
    t.push_str(&format!("grab        {} frames {}
", s.capture_path, s.frames));
    t.push_str(&format!("prefs       power {} waveform {:?} phase {:?} disco {}\n", p.power, p.waveform, p.phase, p.disco));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_has_no_user_data_and_lists_state() {
        let mut s = Snapshot::default();
        s.master = Some(1);
        s.decks[0].bars = Some((5, 1));
        let t = diagnostics_text(&s, &Prefs::default());
        assert!(t.contains("master      2"));
        assert!(t.contains("bars 05.1"));
        assert!(t.contains("module      unknown | link waiting"));
        assert!(!t.to_lowercase().contains("users"));
    }
}
