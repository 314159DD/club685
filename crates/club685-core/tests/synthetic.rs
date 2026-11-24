//! Synthetic frames: other row heights and playhead positions, bars-only zoom, red waveform bodies.
use club685_core::*;

struct Canvas {
    w: usize,
    h: usize,
    data: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize, bg: u8) -> Self {
        let mut data = vec![255u8; w * h * 4];
        for p in data.chunks_mut(4) {
            p[0] = bg;
            p[1] = bg;
            p[2] = bg;
        }
        Self { w, h, data }
    }
    fn put(&mut self, x: i32, y: i32, rgb: (u8, u8, u8)) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.data[i] = rgb.2;
        self.data[i + 1] = rgb.1;
        self.data[i + 2] = rgb.0;
    }
    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, rgb: (u8, u8, u8)) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, rgb);
            }
        }
    }
    fn frame(&self) -> Bgra<'_> {
        Bgra::new(&self.data, self.w, self.h, self.w * 4).unwrap()
    }

    /// One marker at column `c` for a row spanning `top..=bottom` (edge rows carry the triangles).
    fn marker(&mut self, c: i32, top: i32, bottom: i32, bar: bool) {
        for (e, dir) in [(top, 1), (bottom, -1)] {
            let px = |k: i32| e + dir * k;
            if bar {
                let red = (255, 0, 0);
                for dx in -2..=2 {
                    self.put(c + dx, px(0), red);
                }
                self.put(c - 3, px(0), (0x33, 0, 0));
                self.put(c + 3, px(0), (0x33, 0, 0));
                self.put(c - 2, px(1), (0x7f, 0, 0));
                self.put(c + 2, px(1), (0x7f, 0, 0));
                for dx in -1..=1 {
                    self.put(c + dx, px(1), red);
                }
                self.put(c - 1, px(2), (0xcc, 0, 0));
                self.put(c + 1, px(2), (0xcc, 0, 0));
                self.put(c, px(2), red);
                self.put(c, px(3), red);
                self.put(c - 1, px(3), (0x33, 0, 0));
                self.put(c + 1, px(3), (0x33, 0, 0));
            } else {
                for k in 0..3 {
                    self.put(c, px(k), (0x7c, 0x7c, 0x7c));
                }
                self.put(c - 1, px(0), (0x72, 0x72, 0x72));
                self.put(c + 1, px(0), (0x72, 0x72, 0x72));
                self.put(c - 1, px(1), (0x3e, 0x3e, 0x3e));
                self.put(c + 1, px(1), (0x3e, 0x3e, 0x3e));
                self.put(c - 1, px(2), (0x15, 0x15, 0x15));
                self.put(c + 1, px(2), (0x15, 0x15, 0x15));
                self.put(c, px(3), (0x75, 0x75, 0x75));
            }
        }
        let line = if bar { (255, 255, 255) } else { (0x4c, 0x4c, 0x4c) };
        for y in (top + 5)..=(bottom - 5) {
            self.put(c, y, line);
        }
    }

    fn playhead(&mut self, x: i32, top: i32, bottom: i32, level: u8) {
        for y in (top - 1)..=(bottom + 1) {
            self.put(x, y, (level, level, level));
        }
    }
}

struct Scene {
    canvas: Canvas,
    rows: [(i32, i32); 2],
}

/// Two rows of `h` px at `top1` and `top1 + h + gap`, black band with a dark grey frame around it.
fn scene(w: usize, top1: i32, h: i32, gap: i32) -> Scene {
    let mut canvas = Canvas::new(w, (top1 + 2 * h + gap + 140) as usize, 0x19);
    let r1 = (top1, top1 + h - 1);
    let r2 = (r1.1 + gap + 1, r1.1 + gap + h);
    canvas.rect(0, r1.0 - 2, w as i32, r2.1 + 3, (0, 0, 0));
    Scene { canvas, rows: [r1, r2] }
}

fn analyze3(c: &Canvas) -> FrameReading {
    let f = c.frame();
    let mut st = TrackerState::new();
    let mut r = FrameReading::default();
    for i in 0..3u64 {
        r = analyze_at(&f, &mut st, i * 16);
    }
    r
}

#[test]
fn row_height_and_playhead_position_are_measured() {
    for (h, gap, top1, cursor) in [(40, 4, 100, 777), (88, 8, 130, 960), (65, 5, 151, 1280)] {
        let mut s = scene(1920.max(cursor as usize * 2 + 20), top1, h, gap);
        let (beat, origin) = (40.0f32, 311.0f32);
        for &(top, bottom) in &s.rows.clone() {
            for i in 0..80 {
                let x = (origin + beat * i as f32).round() as i32;
                if x > 8 && x < s.canvas.w as i32 - 8 {
                    s.canvas.marker(x, top, bottom, i % 4 == 0);
                }
            }
            s.canvas.playhead(cursor, top, bottom, 0xe0);
        }
        let r = analyze3(&s.canvas);
        let l = r.layout.unwrap_or_else(|| panic!("h={h}: no layout"));
        assert_eq!((l.rows[0].top, l.rows[0].bottom), s.rows[0], "h={h}");
        assert_eq!((l.rows[1].top, l.rows[1].bottom), s.rows[1], "h={h}");
        assert_eq!(l.band.y0, s.rows[0].0 - 2);
        assert_eq!(l.band.y1, s.rows[1].1 + 3);
        assert_eq!(r.cursor, Some(cursor as f32), "h={h}");
        let p = r.decks[0].phase.unwrap();
        let want = ((cursor as f32 - origin) / beat).rem_euclid(4.0);
        let diff = (p.phase - want).abs().min(4.0 - (p.phase - want).abs());
        assert!(diff < 0.08, "h={h}: phase {} want {want}", p.phase);
        assert!((p.beat_spacing - beat).abs() < 0.1);
    }
}

#[test]
fn paused_playhead_707070_is_found() {
    let mut s = scene(1600, 120, 60, 6);
    for &(top, bottom) in &s.rows.clone() {
        s.canvas.marker(200, top, bottom, true);
        s.canvas.marker(900, top, bottom, true);
        s.canvas.playhead(800, top, bottom, 0x70);
    }
    let r = analyze3(&s.canvas);
    assert_eq!(r.cursor, Some(800.0));
}

#[test]
fn bar_line_alone_is_not_a_cursor() {
    // a bar marker (white full height line, red triangles) must never be mistaken for the playhead
    let mut s = scene(1600, 120, 60, 6);
    for &(top, bottom) in &s.rows.clone() {
        s.canvas.marker(500, top, bottom, true);
        s.canvas.marker(1100, top, bottom, true);
    }
    let f = s.canvas.frame();
    let l = find_layout(&f).unwrap();
    assert_eq!(detect_cursor(&f, &l), None);
}

#[test]
fn red_waveform_body_is_not_a_marker() {
    let mut s = scene(1600, 120, 60, 6);
    for &(top, bottom) in &s.rows.clone() {
        s.canvas.marker(300, top, bottom, true);
        s.canvas.marker(700, top, bottom, true);
        // loud RGB-mode waveform: solid red blocks touching the top and bottom edge, no gap, no white line
        s.canvas.rect(450, top, 470, top + 30, (255, 0, 0));
        s.canvas.rect(520, bottom - 30, 540, bottom + 1, (255, 0, 0));
        s.canvas.rect(600, top, 606, bottom + 1, (255, 0, 0));
    }
    let f = s.canvas.frame();
    let l = find_layout(&f).unwrap();
    for row in &l.rows {
        let bars = scan_row(&f, row).bars();
        assert_eq!(bars, vec![300.0, 700.0]);
    }
}

#[test]
fn wide_zoom_falls_back_to_bars_only() {
    let mut s = scene(1920, 120, 60, 6);
    let (bar, origin, cursor) = (240.0f32, 130.0f32, 960);
    for &(top, bottom) in &s.rows.clone() {
        for i in 0..8 {
            let x = (origin + bar * i as f32).round() as i32;
            s.canvas.marker(x, top, bottom, true);
        }
        s.canvas.playhead(cursor, top, bottom, 0xe0);
    }
    let r = analyze3(&s.canvas);
    let p = r.decks[0].phase.expect("bars-only phase");
    assert_eq!(p.kind, PhaseKind::BarsOnly);
    assert!((p.bar_spacing - bar).abs() < 0.5, "{}", p.bar_spacing);
    let want = ((cursor as f32 - origin) / bar).rem_euclid(1.0) * 4.0;
    assert!((p.phase - want).abs() < 0.1, "{} vs {want}", p.phase);
}

#[test]
fn bar_thinning_uses_smallest_consistent_spacing() {
    // one bar marker hidden behind the Bars text: the gap is two bars, the spacing stays one bar
    let reds = [100.0f32, 340.0, 580.0, 1060.0, 1300.0];
    let p = phase::estimate(&reds, &[], 700.0).unwrap();
    assert_eq!(p.kind, PhaseKind::BarsOnly);
    assert!((p.bar_spacing - 240.0).abs() < 0.5);
}

#[test]
fn grid_estimator_rejects_inconsistent_markers() {
    let reds = [100.0f32, 270.0, 440.0];
    // beats that do not sit on the beat lattice
    let greys = [150.0f32, 200.0, 230.0, 320.0, 350.0, 400.0];
    assert!(phase::estimate(&reds, &greys, 300.0).is_none());
    // too few markers
    assert!(phase::estimate(&reds[..1], &[], 300.0).is_none());
}

#[test]
fn master_tracker_needs_two_frames_and_holds_250ms() {
    let mut t = MasterTracker::default();
    assert_eq!(t.observe(Some(0), 0), None);
    assert_eq!(t.observe(Some(0), 16), Some(0));
    assert_eq!(t.observe(None, 100), Some(0));
    assert_eq!(t.observe(None, 400), None);
    assert_eq!(master_from_counts([81, 0]), Some(0));
    assert_eq!(master_from_counts([0, 81]), Some(1));
    assert_eq!(master_from_counts([81, 81]), None);
    assert_eq!(master_from_counts([10, 0]), None);
}

#[test]
fn phase_tracker_resets_on_tempo_jump() {
    let r = |s: f32| PhaseReading {
        kind: PhaseKind::Grid,
        phase: 1.0,
        beat: 2,
        beat_spacing: s,
        bar_spacing: s * 4.0,
        red: 10,
        grey: 30,
    };
    let mut t = PhaseTracker::default();
    assert!(t.observe(Some(r(42.0))).is_none());
    assert!(t.observe(Some(r(42.5))).is_none());
    assert!(t.observe(Some(r(43.0))).is_some());
    assert!(t.observe(Some(r(60.0))).is_none());
    assert!(t.observe(None).is_none());
}
