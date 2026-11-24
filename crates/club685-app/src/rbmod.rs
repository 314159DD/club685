//! The rbmod deck-state exporter as a source: a UDP listener on 127.0.0.1:47990 (schema `"v":1`, additive), the
//! meter phase extrapolated between datagrams, the MASTER rule and the choice between module and screen capture.

use serde::Deserialize;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PORT: u16 = 47990;
/// A datagram younger than this counts as fresh.
pub const FRESH_MS: u64 = 100;
/// Once the module is live it stays live until no datagram came for this long (a lost packet burst is not an outage).
pub const GRACE_MS: u64 = 750;
/// rekordbox loads the module while it starts up; the first datagram can take this long after the window appears.
pub const STARTUP_MS: u64 = 20_000;
/// Extrapolation never runs further than this past the last datagram.
pub const MAX_EXTRAPOLATE_100NS: i64 = 5_000_000;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct DeckPacket {
    pub deck: u32,
    pub loaded: Option<bool>,
    pub playing: Option<bool>,
    pub master: Option<bool>,
    pub bpm: Option<f64>,
    pub bar: Option<u32>,
    pub beat_in_bar: Option<u32>,
    pub phase_beat: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Packet {
    pub v: u32,
    pub qpc_100ns: Option<i64>,
    pub master_source: Option<String>,
    pub decks: Vec<DeckPacket>,
}

pub fn parse(data: &[u8]) -> Option<Packet> {
    let p: Packet = serde_json::from_slice(data).ok()?;
    if p.v == 0 || p.decks.is_empty() {
        return None;
    }
    Some(p)
}

/// The packet's deck 0 and 1.
pub fn deck_pair(p: &Packet) -> [Option<&DeckPacket>; 2] {
    [p.decks.iter().find(|d| d.deck == 0), p.decks.iter().find(|d| d.deck == 1)]
}

/// How the MASTER deck is decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterVerdict {
    /// Exactly one deck is master: trust it.
    Deck(usize),
    /// The module says nobody is master (no_master_set, single_deck_unknown): show no MASTER row.
    None,
    /// Both true or both false from a build that does not say why: unknown, the pixel MASTER may be used.
    Unknown,
}

pub fn master_verdict(p: &Packet) -> MasterVerdict {
    let d = deck_pair(p);
    let m = |i: usize| d[i].and_then(|d| d.master) == Some(true);
    match (m(0), m(1)) {
        (true, false) => return MasterVerdict::Deck(0),
        (false, true) => return MasterVerdict::Deck(1),
        _ => {}
    }
    match p.master_source.as_deref() {
        Some("no_master_set") | Some("single_deck_unknown") => MasterVerdict::None,
        _ => MasterVerdict::Unknown,
    }
}

/// Master deck to draw: the module's when it is decisive, the pixel reading only when the module cannot tell.
pub fn resolve_master(v: MasterVerdict, pixel: Option<usize>) -> Option<usize> {
    match v {
        MasterVerdict::Deck(d) => Some(d),
        MasterVerdict::None => None,
        MasterVerdict::Unknown => pixel,
    }
}

/// What the meter needs for one deck at one instant.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckView {
    /// Beat position 0..4 (`beat_in_bar - 1 + phase_beat`, advanced while playing).
    pub phase: Option<f32>,
    pub bars: Option<(u32, u8)>,
    pub bpm: Option<f64>,
}

/// Phase and bar readout of a deck `age_100ns` after its datagram. Only a playing deck moves; a deck that is not
/// loaded, or from an old module without the beat fields, has no phase.
pub fn extrapolate(d: &DeckPacket, age_100ns: i64) -> DeckView {
    let bpm = d.bpm.filter(|b| b.is_finite() && *b > 0.0);
    if d.loaded == Some(false) {
        return DeckView { phase: None, bars: None, bpm };
    }
    let (Some(bib), Some(pb)) = (d.beat_in_bar, d.phase_beat) else { return DeckView { phase: None, bars: None, bpm } };
    if !(1..=4).contains(&bib) || !pb.is_finite() {
        return DeckView { phase: None, bars: None, bpm };
    }
    let age = age_100ns.clamp(0, MAX_EXTRAPOLATE_100NS) as f64 / 1e7;
    let advance = if d.playing == Some(true) { bpm.map_or(0.0, |b| b / 60.0 * age) } else { 0.0 };
    let in_bar = (bib - 1) as f64 + pb.clamp(0.0, 1.0) + advance;
    let bar0 = d.bar.map(|b| b.max(1));
    let (bar_add, in_bar) = ((in_bar / 4.0).floor() as u32, in_bar.rem_euclid(4.0));
    DeckView { phase: Some(in_bar as f32), bars: bar0.map(|b| (b + bar_add, in_bar.floor() as u8 + 1)), bpm }
}

/// State of the link to the rekordbox module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// rekordbox was just found, waiting for the first datagrams.
    #[default]
    Pending,
    /// Datagrams are arriving.
    Module,
    /// rekordbox runs but no datagrams arrive: the module is not loaded (rekordbox was started before the install).
    Silent,
}

/// `age_ms` is the age of the last datagram (`None` = never received), `since_attach_ms` the time since rekordbox
/// was found.
pub fn select_source(current: Source, age_ms: Option<u64>, since_attach_ms: u64) -> Source {
    match (current, age_ms) {
        (_, Some(a)) if a < FRESH_MS => Source::Module,
        (Source::Module, Some(a)) if a < GRACE_MS => Source::Module,
        (Source::Pending, _) if since_attach_ms < STARTUP_MS => Source::Pending,
        _ => Source::Silent,
    }
}

#[derive(Default)]
pub struct RbShared {
    pub last: Option<Packet>,
    pub recv: Option<Instant>,
    /// Our own QPC (100 ns) when the datagram arrived.
    pub recv_ticks: i64,
    pub count: u64,
    pub bind_error: Option<String>,
}

pub type RbRef = Arc<Mutex<RbShared>>;

/// Starts the listener thread. It retries the bind every 2 s while another process holds the port.
pub fn spawn_listener() -> RbRef {
    let shared: RbRef = Arc::new(Mutex::new(RbShared::default()));
    let s = shared.clone();
    let _ = std::thread::Builder::new().name("club685-udp".into()).spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let sock = match UdpSocket::bind(("127.0.0.1", PORT)) {
                Ok(s) => s,
                Err(e) => {
                    s.lock().unwrap().bind_error = Some(e.to_string());
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
            s.lock().unwrap().bind_error = None;
            let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
            loop {
                match sock.recv(&mut buf) {
                    Ok(n) => {
                        if let Some(p) = parse(&buf[..n]) {
                            let ticks = club685_capture::now_ticks();
                            let mut g = s.lock().unwrap();
                            g.last = Some(p);
                            g.recv = Some(Instant::now());
                            g.recv_ticks = ticks;
                            g.count += 1;
                        }
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
            }
        }
    });
    shared
}

/// A copy of the newest datagram with its age.
#[derive(Clone, Debug)]
pub struct Latest {
    pub packet: Packet,
    pub age_ms: u64,
    /// Age in 100 ns for the extrapolation: from the datagram's own QPC when sane, else from its arrival.
    pub age_100ns: i64,
    pub count: u64,
}

pub fn age_100ns(now_ticks: i64, packet_qpc: Option<i64>, recv_ticks: i64) -> i64 {
    match packet_qpc {
        Some(q) if now_ticks - q >= 0 && now_ticks - q < 50_000_000 => now_ticks - q,
        _ => (now_ticks - recv_ticks).max(0),
    }
}

pub fn latest(r: &RbRef) -> (Option<Latest>, u64) {
    let g = r.lock().unwrap();
    let count = g.count;
    match (&g.last, g.recv) {
        (Some(p), Some(t)) => {
            let now = club685_capture::now_ticks();
            (Some(Latest { packet: p.clone(), age_ms: t.elapsed().as_millis() as u64, age_100ns: age_100ns(now, p.qpc_100ns, g.recv_ticks), count }), count)
        }
        _ => (None, count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"v":1,"t_ms":12345,"qpc_100ns":900000000,"read_100ns":1200,"master_source":"dispatch+0x248","decks":[
        {"deck":0,"pos_ms":61234,"master":true,"flags":4112,"playing":true,"loaded":true,"grid_bpm":128.0,"tempo":0.02,"bpm":130.56,"beat":10,"bar":3,"beat_in_bar":2,"phase_beat":0.25,"future_field":1},
        {"deck":1,"master":false,"playing":false,"loaded":true,"bpm":120.0,"bar":7,"beat_in_bar":4,"phase_beat":0.9}]}"#;

    #[test]
    fn parses_the_documented_schema() {
        let p = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(p.v, 1);
        assert_eq!(p.qpc_100ns, Some(900_000_000));
        assert_eq!(p.decks.len(), 2);
        let d = deck_pair(&p);
        assert_eq!(d[0].unwrap().beat_in_bar, Some(2));
        assert_eq!(d[0].unwrap().bpm, Some(130.56));
        assert_eq!(d[1].unwrap().master, Some(false));
    }

    #[test]
    fn rejects_garbage_and_tolerates_old_modules() {
        assert!(parse(b"not json").is_none());
        assert!(parse(b"{}").is_none());
        assert!(parse(br#"{"v":1,"decks":[]}"#).is_none());
        // an old build without beat fields still parses, but has no phase
        let p = parse(br#"{"v":1,"decks":[{"deck":0,"playing":true,"master":false,"bpm":128.0}]}"#).unwrap();
        let v = extrapolate(&p.decks[0], 0);
        assert_eq!(v.phase, None);
        assert_eq!(v.bpm, Some(128.0));
    }

    #[test]
    fn master_rule() {
        let mk = |a: bool, b: bool, src: Option<&str>| Packet {
            v: 1,
            qpc_100ns: None,
            master_source: src.map(String::from),
            decks: vec![DeckPacket { deck: 0, master: Some(a), ..Default::default() }, DeckPacket { deck: 1, master: Some(b), ..Default::default() }],
        };
        assert_eq!(master_verdict(&mk(true, false, None)), MasterVerdict::Deck(0));
        assert_eq!(master_verdict(&mk(false, true, Some("dispatch+0x248"))), MasterVerdict::Deck(1));
        // both true (before anyone set MASTER on an old build): unknown
        assert_eq!(master_verdict(&mk(true, true, Some("dispatch+0x248"))), MasterVerdict::Unknown);
        assert_eq!(master_verdict(&mk(false, false, None)), MasterVerdict::Unknown);
        // newer builds say why nobody is master: no MASTER row, no pixel fallback
        assert_eq!(master_verdict(&mk(false, false, Some("no_master_set"))), MasterVerdict::None);
        assert_eq!(master_verdict(&mk(false, false, Some("single_deck_unknown"))), MasterVerdict::None);
        assert_eq!(resolve_master(MasterVerdict::Deck(1), Some(0)), Some(1));
        assert_eq!(resolve_master(MasterVerdict::None, Some(0)), None);
        assert_eq!(resolve_master(MasterVerdict::Unknown, Some(0)), Some(0));
        assert_eq!(resolve_master(MasterVerdict::Unknown, None), None);
    }

    fn deck(playing: bool, bar: u32, bib: u32, pb: f64, bpm: f64) -> DeckPacket {
        DeckPacket { deck: 0, loaded: Some(true), playing: Some(playing), master: None, bpm: Some(bpm), bar: Some(bar), beat_in_bar: Some(bib), phase_beat: Some(pb) }
    }

    #[test]
    fn phase_is_beat_in_bar_plus_phase_beat() {
        let v = extrapolate(&deck(false, 5, 3, 0.5, 120.0), 0);
        assert_eq!(v.phase, Some(2.5));
        assert_eq!(v.bars, Some((5, 3)));
    }

    #[test]
    fn extrapolation_moves_only_while_playing() {
        // 120 BPM = 2 beats/s: 250 ms = 0.5 beat
        let v = extrapolate(&deck(true, 5, 1, 0.0, 120.0), 2_500_000);
        assert!((v.phase.unwrap() - 0.5).abs() < 1e-5);
        let v = extrapolate(&deck(false, 5, 1, 0.0, 120.0), 2_500_000);
        assert_eq!(v.phase, Some(0.0));
        // negative age (clock skew) does not move backwards
        let v = extrapolate(&deck(true, 5, 1, 0.25, 120.0), -1000);
        assert_eq!(v.phase, Some(0.25));
        // extrapolation is capped at 0.5 s of age
        let v = extrapolate(&deck(true, 5, 1, 0.0, 120.0), 100_000_000);
        assert!((v.phase.unwrap() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn extrapolation_wraps_into_the_next_bar() {
        // beat 4 of bar 5 at 3.9, 0.2 beat later at 120 BPM (100 ms): 4.1 -> bar 6 beat 1 phase 0.1
        let v = extrapolate(&deck(true, 5, 4, 0.9, 120.0), 1_000_000);
        assert!((v.phase.unwrap() - 0.1).abs() < 1e-4, "{:?}", v.phase);
        assert_eq!(v.bars, Some((6, 1)));
        // within a bar the beat counter follows the phase
        let v = extrapolate(&deck(true, 5, 1, 0.9, 120.0), 1_000_000);
        assert_eq!(v.bars, Some((5, 2)));
    }

    #[test]
    fn unloaded_deck_has_no_phase() {
        let mut d = deck(true, 1, 1, 0.0, 120.0);
        d.loaded = Some(false);
        assert_eq!(extrapolate(&d, 0).phase, None);
        // invalid beat_in_bar
        let d = deck(true, 1, 0, 0.0, 120.0);
        assert_eq!(extrapolate(&d, 0).phase, None);
    }

    #[test]
    fn age_prefers_the_packet_clock() {
        assert_eq!(age_100ns(1_000_500, Some(1_000_000), 999_000), 500);
        // packet clock from the future or implausible: use the arrival time
        assert_eq!(age_100ns(1_000_000, Some(2_000_000), 999_000), 1000);
        assert_eq!(age_100ns(1_000_000, None, 990_000), 10_000);
    }

    #[test]
    fn link_selection() {
        use Source::*;
        assert_eq!(select_source(Silent, Some(10), 5000), Module);
        assert_eq!(select_source(Pending, Some(10), 100), Module);
        // startup grace while rekordbox loads the module
        assert_eq!(select_source(Pending, None, 500), Pending);
        assert_eq!(select_source(Pending, None, 25_000), Silent);
        // a lost packet burst keeps the link, a real outage ends it
        assert_eq!(select_source(Module, Some(300), 9000), Module);
        assert_eq!(select_source(Module, Some(800), 9000), Silent);
        assert_eq!(select_source(Module, None, 9000), Silent);
        // a merely recent packet does not revive a silent link
        assert_eq!(select_source(Silent, Some(300), 9000), Silent);
    }
}
