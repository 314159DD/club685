//! Renders the split / central composites from the fixtures and the main window in every state to PNGs under
//! `club685/target/previews/`, so the result can be reviewed without rekordbox.
//!
//! usage: cargo run --release -p club685-app --example previews

use club685_app::prefs::{BpmSetting, PhaseSetting, Prefs, WaveMode};
use club685_app::preview::{PhaseClock, PreviewState};
use club685_app::scene::build_scene;
use club685_app::state::{DeckSnap, Snapshot, StateKind};
use club685_app::ui::{self, Action, UiRenderer, UiState};
use club685_core::{analyze_at, Bgra, FrameReading, TrackerState};
use club685_render::{Composer, MeterKind, Scene};
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

struct Fixture {
    data: Vec<u8>,
    stride: usize,
    width: usize,
    height: usize,
}

impl Fixture {
    fn frame(&self) -> Bgra<'_> {
        Bgra::new(&self.data, self.width, self.height, self.stride).unwrap()
    }
}

fn load(path: &Path) -> Fixture {
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let ch = if info.color_type == png::ColorType::Rgba { 4 } else { 3 };
    let (w, h) = (info.width as usize, info.height as usize);
    let mut data = vec![255u8; w * h * 4];
    for i in 0..w * h {
        data[i * 4] = buf[i * ch + 2];
        data[i * 4 + 1] = buf[i * ch + 1];
        data[i * 4 + 2] = buf[i * ch];
    }
    Fixture { data, stride: w * 4, width: 2560.min(w), height: 1410.min(h) }
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn pm_rgba(pm: &Pixmap) -> Vec<u8> {
    let mut v = Vec::with_capacity(pm.data().len());
    for px in pm.pixels() {
        let c = px.demultiply();
        v.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    v
}

/// Writes the composite with the untouched band above it for comparison.
fn write_compare(path: &Path, frame: &Bgra, band_y0: i32, band_y1: i32, x0: i32, x1: i32, comp: &Pixmap) {
    let w = (x1 - x0) as u32;
    let bh = (band_y1 - band_y0) as u32;
    let h = bh + 8 + comp.height();
    let mut out = vec![0u8; (w * h * 4) as usize];
    for p in out.chunks_exact_mut(4) {
        p[3] = 255;
        p[0] = 40;
        p[1] = 44;
        p[2] = 52;
    }
    for y in 0..bh as i32 {
        for x in 0..w as i32 {
            let (r, g, b) = frame.rgb(x0 + x, band_y0 + y);
            let i = ((y as u32 * w + x as u32) * 4) as usize;
            out[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    let c = pm_rgba(comp);
    for y in 0..comp.height() {
        let dst = ((bh + 8 + y) * w * 4) as usize;
        let src = (y * comp.width() * 4) as usize;
        let n = (comp.width().min(w) * 4) as usize;
        out[dst..dst + n].copy_from_slice(&c[src..src + n]);
    }
    write_png(path, w, h, &out);
}

fn read(fx: &Fixture) -> FrameReading {
    let frame = fx.frame();
    let mut st = TrackerState::new();
    let mut r = FrameReading::default();
    for i in 0..6u64 {
        r = analyze_at(&frame, &mut st, i * 20);
    }
    r
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = root.join("target").join("previews");
    std::fs::create_dir_all(&out).unwrap();
    let mut written: Vec<String> = Vec::new();

    // ---- composites from fixtures --------------------------------------------------------------------------
    let mut comp = Composer::new();
    for name in ["3band-playing-a", "3band-paused", "blue-paused", "rgb-paused"] {
        let fx = load(&root.join("fixtures").join(format!("{name}.png")));
        let frame = fx.frame();
        let r = read(&fx);
        let l = r.layout.expect("layout");
        let mask = club685_overlay::waveform_mask_rect(l.band, l.width, 96);
        let (w, h) = (mask.width(), mask.height());
        println!(
            "{name}: band {:?} rows {:?} cursor {:?} master {:?} phase {:?}/{:?} bars {:?}/{:?}",
            l.band,
            l.rows,
            r.cursor,
            r.master,
            r.decks[0].phase.map(|p| p.phase),
            r.decks[1].phase.map(|p| p.phase),
            r.decks[0].bars,
            r.decks[1].bars
        );
        let variants: Vec<(&str, bool, Option<MeterKind>, Option<f32>)> = if name == "3band-playing-a" {
            vec![
                ("split-box", true, Some(MeterKind::Box), None),
                ("split-line", true, Some(MeterKind::Line), None),
                ("split-nometer", true, None, None),
                ("central-box", false, Some(MeterKind::Box), None),
                ("central-line", false, Some(MeterKind::Line), None),
                ("split-box-left-playhead", true, Some(MeterKind::Box), Some(300.0)),
            ]
        } else {
            vec![("split-box", true, Some(MeterKind::Box), None), ("central-line", false, Some(MeterKind::Line), None)]
        };
        for (v, split, meter, cursor) in variants {
            let mut scene: Scene = build_scene(&frame, &r, 96, meter).unwrap();
            if let Some(c) = cursor {
                scene.cursor = Some(c);
            }
            let ok = if split { comp.split(&scene, w, h) } else { comp.central(&scene, w, h) };
            assert!(ok, "{name} {v}");
            let p = out.join(format!("{name}__{v}.png"));
            write_compare(&p, &frame, l.band.y0, l.band.y1, mask.x0, mask.x1, comp.pixmap().unwrap());
            written.push(p.display().to_string());
        }
    }

    // ---- main window states --------------------------------------------------------------------------------
    let r1 = UiRenderer::new(1.0);
    let r125 = UiRenderer::new(1.25);
    let base_prefs = Prefs { power: true, bpm: BpmSetting::Hide, waveform: WaveMode::Split, phase: PhaseSetting::Box, ..Prefs::default() };
    let mut live = Snapshot::default();
    live.kind = StateKind::Active;
    live.line = "Split active".to_string();
    live.decks = [DeckSnap { phase: Some(0.3), bars: Some((85, 1)) }, DeckSnap { phase: Some(2.6), bars: Some((69, 3)) }];
    live.master = Some(0);
    live.decks_found = 2;
    live.cursor = Some(1280.0);
    live.beat_px = Some(42.7);
    live.bpm_int = [Some(143), None];
    live.gate = "open".into();
    live.overlays = "body1+strip1+solid0".into();
    live.source = club685_app::rbmod::Source::Module;
    live.udp_pps = 60.0;
    live.meter_fps = 143.0;
    live.rb_version = "6.8.5.0 pid 4242".into();
    live.window = "2560x1410".into();
    live.dpi = 96;
    live.layout = "band y149..287 rows [151..215] [220..285]".into();
    live.frames = 8123;

    let mut searching = Snapshot::default();
    searching.line = "Looking for rekordbox…".into();
    let mut ready = live.clone();
    ready.kind = StateKind::Ready;
    ready.line = "Ready · switch to rekordbox".into();
    ready.gate = "not-foreground".into();
    let mut waiting = live.clone();
    waiting.master = None;
    waiting.line = "Split active · waiting for MASTER".into();
    let mut err = Snapshot::default();
    err.kind = StateKind::Error;
    err.line = "Move rekordbox to the main display".into();
    let mut master2 = live.clone();
    master2.master = Some(1);
    master2.decks = [DeckSnap { phase: Some(1.2), bars: Some((45, 2)) }, DeckSnap { phase: Some(0.1), bars: Some((113, 1)) }];

    let line_prefs = Prefs { phase: PhaseSetting::Line, waveform: WaveMode::HideBoth, bpm: BpmSetting::OneDecimal, ..base_prefs.clone() };
    let disco_prefs = Prefs { disco: true, ..base_prefs.clone() };
    let off_prefs = Prefs { power: false, ..Prefs::default() };

    #[allow(clippy::type_complexity)]
    let states: Vec<(&str, &UiRenderer, Prefs, Snapshot, Option<Action>, bool, f32)> = vec![
        ("window-1-locked-split-box", &r1, base_prefs.clone(), live.clone(), None, false, 0.0),
        ("window-2-master-deck2-line-hideboth", &r1, line_prefs, master2, Some(Action::Wave(WaveMode::Split)), false, 0.0),
        ("window-3-searching", &r1, Prefs::default(), searching, None, false, 0.0),
        ("window-4-ready-switch-to-rekordbox", &r1, base_prefs.clone(), ready, None, false, 0.0),
        ("window-5-waiting-for-master", &r1, base_prefs.clone(), waiting, None, false, 0.0),
        ("window-6-display-error", &r1, base_prefs.clone(), err, None, false, 0.0),
        ("window-7-power-off", &r1, off_prefs, Snapshot::default(), None, false, 0.0),
        ("window-8-disco-a", &r1, disco_prefs.clone(), live.clone(), None, false, 0.4),
        ("window-9-disco-b", &r1, disco_prefs, live.clone(), None, false, 1.6),
        ("window-10-settings", &r1, base_prefs.clone(), live.clone(), Some(Action::CopyDiag), true, 0.0),
        ("window-11-dpi125", &r125, base_prefs, live.clone(), None, false, 0.0),
    ];
    for (name, rend, prefs, snap, hover, settings, t) in states {
        let pv = PreviewState::from_snapshot(&snap);
        let st = UiState { prefs: &prefs, snap: &snap, preview: &pv, hover, pressed: None, settings_open: settings, copied: false, time: t };
        let pm = rend.paint(&st);
        // flatten on the design page colour so the rounded corners are visible
        let mut bg = Pixmap::new(pm.width(), pm.height()).unwrap();
        bg.fill(tiny_skia::Color::from_rgba8(0x1a, 0x1d, 0x22, 255));
        bg.draw_pixmap(0, 0, pm.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        let p = out.join(format!("{name}.png"));
        write_png(&p, bg.width(), bg.height(), &pm_rgba(&bg));
        written.push(p.display().to_string());
    }
    // ---- LCD mode preview: every combination, cropped from the window at 2x --------------------------------------
    let r2 = UiRenderer::new(2.0);
    let lcd_png = |name: &str, prefs: &Prefs, snap: &Snapshot, pv: &PreviewState, written: &mut Vec<String>| {
        let st = UiState { prefs, snap, preview: pv, hover: None, pressed: None, settings_open: false, copied: false, time: 0.0 };
        let pm = r2.paint(&st);
        // LCD housing is [11, 51, 438, 170] logical
        let (x0, y0, w, h) = (11 * 2, 51 * 2, 438 * 2, 170 * 2);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let c = pm.pixel(x, y).unwrap().demultiply();
                rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), 255]);
            }
        }
        let p = out.join(format!("lcd-{name}.png"));
        write_png(&p, w, h, &rgba);
        written.push(p.display().to_string());
    };
    // frozen: rekordbox not in front, readings extrapolated by the clock after a measured run at 143 BPM
    let mut frozen = Snapshot::default();
    frozen.kind = StateKind::Ready;
    frozen.line = "Ready · switch to rekordbox".into();
    frozen.decks_found = 2;
    frozen.master = Some(0);
    frozen.bpm_int = [Some(143), Some(143)];
    let mut clock = PhaseClock::new();
    {
        let mut lv = live.clone();
        let mut t = 0.0f64;
        let mut rev = 1u64;
        while t < 2.0 {
            let ph = ((t * 143.0 / 60.0) % 4.0) as f32;
            lv.decks = [DeckSnap { phase: Some(ph), bars: Some((85, 1)) }, DeckSnap { phase: Some((ph + 1.0) % 4.0), bars: Some((69, 2)) }];
            lv.rev = rev;
            clock.step(t, &lv);
            rev += 1;
            t += 0.033;
        }
    }
    let frozen_pv = clock.step(2.9, &frozen);
    assert!(!frozen_pv.live && !frozen_pv.demo);
    let mut demo_clock = PhaseClock::new();
    let mut demo_snap = Snapshot::default();
    demo_snap.kind = StateKind::Searching;
    demo_snap.line = "Looking for rekordbox…".into();
    demo_clock.step(0.0, &demo_snap);
    let demo_pv = demo_clock.step(1.7, &demo_snap);
    assert!(demo_pv.demo);

    let base = Prefs { power: true, ..Prefs::default() };
    for (wn, w) in [("normal", WaveMode::Normal), ("split", WaveMode::Split), ("hideboth", WaveMode::HideBoth)] {
        for (pn, ph) in [("phase-off", PhaseSetting::Off), ("phase-box", PhaseSetting::Box), ("phase-line", PhaseSetting::Line)] {
            let prefs = Prefs { waveform: w, phase: ph, bpm: BpmSetting::Off, ..base.clone() };
            lcd_png(&format!("{wn}-{pn}"), &prefs, &frozen, &frozen_pv, &mut written);
        }
    }
    for (bn, b) in [("bpm-off", BpmSetting::Off), ("bpm-hide", BpmSetting::Hide), ("bpm-143dot", BpmSetting::OneDecimal), ("bpm-143dot0", BpmSetting::NoDecimals)] {
        let prefs = Prefs { waveform: WaveMode::Split, phase: PhaseSetting::Box, bpm: b, ..base.clone() };
        lcd_png(&format!("split-phase-box-{bn}"), &prefs, &frozen, &frozen_pv, &mut written);
    }
    // the same frozen preview a moment later: meters and waveform have moved
    let later = clock.step(3.35, &frozen);
    let prefs = Prefs { waveform: WaveMode::Split, phase: PhaseSetting::Box, bpm: BpmSetting::Off, ..base.clone() };
    lcd_png("split-phase-box-frozen-later", &prefs, &frozen, &later, &mut written);
    // live, rekordbox in front: the real statuses and the measured values
    let live_pv = PreviewState::from_snapshot(&live);
    lcd_png("live-split-box", &Prefs { waveform: WaveMode::Split, phase: PhaseSetting::Box, bpm: BpmSetting::Hide, ..base.clone() }, &live, &live_pv, &mut written);
    // nothing ever measured: demo tempo
    lcd_png("demo-hideboth-line", &Prefs { waveform: WaveMode::HideBoth, phase: PhaseSetting::Line, ..base.clone() }, &demo_snap, &demo_pv, &mut written);
    lcd_png("demo-split-box", &Prefs { waveform: WaveMode::Split, phase: PhaseSetting::Box, ..base.clone() }, &demo_snap, &demo_pv, &mut written);

    // the app icon, large
    let icon = ui::draw_icon(256);
    let p = out.join("icon-256.png");
    write_png(&p, 256, 256, &pm_rgba(&icon));
    written.push(p.display().to_string());
    for w in written {
        println!("wrote {w}");
    }
}
