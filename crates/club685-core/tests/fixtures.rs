mod common;
use club685_core::*;
use common::{load, ALL};

/// Runs `analyze` on the same frame three times (the trackers need three agreeing frames).
fn run3(name: &str) -> (FrameReading, Vec<FrameReading>) {
    let fx = load(name);
    let f = fx.frame();
    let mut st = TrackerState::new();
    let mut all = Vec::new();
    for i in 0..3u64 {
        all.push(analyze_at(&f, &mut st, i * 16));
    }
    (all[2].clone(), all)
}

fn bar_x(r: &FrameReading, deck: usize) -> Vec<f32> {
    r.decks[deck].markers.iter().filter(|m| m.kind == MarkerKind::Bar).map(|m| m.x).collect()
}

#[test]
fn rows_and_band_from_pixels() {
    for name in ALL {
        let (r, _) = run3(name);
        let l = r.layout.unwrap_or_else(|| panic!("{name}: no layout"));
        assert_eq!((l.rows[0].top, l.rows[0].bottom), (151, 215), "{name} row 1");
        assert_eq!((l.rows[1].top, l.rows[1].bottom), (220, 285), "{name} row 2");
        // band y=149..287 inclusive
        assert_eq!((l.band.y0, l.band.y1 - 1), (149, 287), "{name} band");
        assert_eq!((l.band.x0, l.band.x1), (0, 2560), "{name} band width");
    }
}

#[test]
fn playhead_is_1280_in_every_fixture() {
    for name in ALL {
        let (r, all) = run3(name);
        assert_eq!(all[0].cursor_raw, Some(1280.0), "{name}: single frame measurement");
        assert_eq!(all[0].cursor, None, "{name}: not locked after one frame");
        assert_eq!(all[1].cursor, None, "{name}: not locked after two frames");
        assert_eq!(r.cursor, Some(1280.0), "{name}: locked after three frames");
    }
}

#[test]
fn red_marker_centres_3band_playing_a() {
    let (r, _) = run3("3band-playing-a");
    let row1 = [156.0, 327.0, 498.0, 668.0, 839.0, 1009.0, 1180.0, 1351.0];
    let row2 = [126.0, 316.0, 506.0, 697.0, 887.0, 1078.0, 1268.0, 1459.0];
    assert_eq!(&bar_x(&r, 0)[..8], &row1);
    assert_eq!(&bar_x(&r, 1)[..8], &row2);
}

#[test]
fn red_marker_starts_3band_playing_b() {
    // TRUTH.md lists "starts"; the signature centre of the first markers is the same column.
    let (r, _) = run3("3band-playing-b");
    assert_eq!(&bar_x(&r, 0)[..2], &[49.0, 220.0]);
    // row 2 also has a marker at 18 left of the two that TRUTH.md lists (gap 190, same lattice)
    assert_eq!(&bar_x(&r, 1)[..3], &[18.0, 208.0, 398.0]);
}

#[test]
fn rgb_red_waveform_never_counts_as_marker() {
    // rgb-paused has a red waveform body; every detected bar marker must sit on the bar lattice.
    let (r, _) = run3("rgb-paused");
    for d in 0..2 {
        let bars = bar_x(&r, d);
        assert!(bars.len() >= 13 && bars.len() <= 16, "deck {d}: {} bars", bars.len());
        for p in bars.windows(2) {
            let gap = p[1] - p[0];
            assert!((gap - 170.6).abs() < 1.5, "deck {d}: bar gap {gap}");
        }
    }
}

#[test]
fn master_deck() {
    // (fixture, deck index 0/1). 3band-paused: TRUTH.md says deck 1, but the MASTER label of deck 2 is the orange
    // one in that frame (checked on a magnified crop), so the pixels win.
    let expected = [
        ("3band-playing-a", 0),
        ("3band-playing-b", 0),
        ("3band-paused", 1),
        ("blue-paused", 1),
        ("rgb-paused", 1),
        ("session-000", 1),
        ("session-020", 1),
        ("session-036", 1),
        ("session-040", 1),
        ("session-045", 1),
        ("session-050", 1),
        ("session-055", 1),
        // live 2026-11-23: deck 2 is MASTER while the mixer TRIM indicator draws a 2 px orange line inside deck 1's window
        ("rgb-master-deck2-trimline", 1),
    ];
    for (name, deck) in expected {
        let (r, all) = run3(name);
        assert_eq!(all[0].master, None, "{name}: 2-frame rule, first frame");
        assert_eq!(r.master_raw, Some(deck), "{name}");
        assert_eq!(r.master, Some(deck), "{name} tracked");
        let (on, off) = (r.decks[deck].orange_px, r.decks[1 - deck].orange_px);
        assert!(on >= 20 && off <= 2, "{name}: orange {on} vs {off}");
    }
}

#[test]
fn bars_text() {
    let expected: [(&str, [(u32, u8); 2]); 12] = [
        ("3band-playing-a", [(85, 3), (69, 1)]),
        ("blue-paused", [(45, 1), (113, 1)]),
        ("rgb-paused", [(45, 1), (113, 1)]),
        // the rest were read off magnified crops of the frames (not in TRUTH.md)
        ("3band-paused", [(45, 1), (113, 1)]),
        ("3band-playing-b", [(86, 1), (69, 3)]),
        ("session-000", [(45, 1), (113, 1)]),
        ("session-020", [(45, 1), (113, 1)]),
        ("session-036", [(48, 2), (115, 2)]),
        ("session-040", [(53, 3), (120, 3)]),
        ("session-045", [(59, 3), (127, 2)]),
        ("session-050", [(66, 1), (133, 4)]),
        ("session-055", [(72, 3), (140, 2)]),
    ];
    for (name, want) in expected {
        let (r, _) = run3(name);
        for d in 0..2 {
            assert_eq!(r.decks[d].bars, Some(want[d]), "{name} deck {}", d + 1);
        }
    }
}

/// Cyclic check that `phase` (0..4) lies in beat `beat` (1..4) allowing `slack` beats of snapping at the edges.
fn phase_in_beat(phase: f32, beat: u8, slack: f32) -> bool {
    let lo = (beat - 1) as f32 - slack;
    let hi = beat as f32 + slack;
    [phase, phase + 4.0, phase - 4.0].iter().any(|&p| p >= lo && p < hi)
}

#[test]
fn phase_present_in_range_and_agrees_with_bars_text() {
    for name in ALL {
        let (r, all) = run3(name);
        for d in 0..2 {
            assert!(all[0].decks[d].phase.is_none(), "{name} deck {d}: tracker needs 3 frames");
            assert!(all[1].decks[d].phase.is_none(), "{name} deck {d}: tracker needs 3 frames");
            let raw = r.decks[d].phase_raw.unwrap_or_else(|| panic!("{name} deck {d}: no raw phase"));
            let p = r.decks[d].phase.unwrap_or_else(|| panic!("{name} deck {d}: no tracked phase"));
            assert_eq!(p.kind, PhaseKind::Grid);
            assert!((0.0..4.0).contains(&p.phase), "{name} deck {d}: phase {}", p.phase);
            assert!((1..=4).contains(&p.beat));
            assert!((35.0..55.0).contains(&p.beat_spacing), "{name} deck {d}: beat {}", p.beat_spacing);
            assert!(raw.grey > 30, "{name} deck {d}: only {} beat markers", raw.grey);
            let (_, text_beat) = r.decks[d].bars.unwrap();
            assert!(phase_in_beat(p.phase, text_beat, 0.06), "{name} deck {d}: phase {} vs text beat {text_beat}", p.phase);
        }
    }
}

#[test]
fn phase_values_3band_playing_a() {
    let (r, _) = run3("3band-playing-a");
    let p1 = r.decks[0].phase.unwrap();
    let p2 = r.decks[1].phase.unwrap();
    // deck 1: bar at 1180, beat 42.65 px -> (1280-1180)/42.65 = 2.34; deck 2: bar at 1268, beat 47.6 -> 0.25
    assert!((p1.phase - 2.345).abs() < 0.05, "{}", p1.phase);
    assert!((p2.phase - 0.247).abs() < 0.05, "{}", p2.phase);
    assert!((p1.beat_spacing - 42.65).abs() < 0.2);
    assert!((p2.beat_spacing - 47.6).abs() < 0.2);
}

#[test]
fn bpm_header_rect_and_masks() {
    for name in ALL {
        let fx = load(name);
        let f = fx.frame();
        let (r, _) = run3(name);
        for (d, text_x) in [(0usize, 215), (1usize, 1549)] {
            let b = r.decks[d].bpm_rect.unwrap_or_else(|| panic!("{name} deck {d}: no BPM rect"));
            assert_eq!((b.x0, b.y0, b.x1, b.y1), (text_x, 315, text_x + 39, 325), "{name} deck {d}");
            let cover = mask_rect(&f, b, BpmMaskMode::CoverFully);
            let one = mask_rect(&f, b, BpmMaskMode::OneDecimal);
            let none = mask_rect(&f, b, BpmMaskMode::NoDecimals);
            assert!(cover.x0 <= b.x0 && cover.x1 >= b.x1 && cover.y0 <= b.y0 && cover.y1 >= b.y1);
            // "143" stays visible with NoDecimals (hidden from the dot), "143.0" with OneDecimal
            assert_eq!(none.x0 - b.x0, 21, "{name} deck {d}: no decimals starts at the dot");
            assert_eq!(one.x0 - b.x0, 32, "{name} deck {d}: one decimal starts after the first decimal");
            assert!(none.x0 > b.x0 + 15 && none.x0 < one.x0 && one.x0 < b.x1);
            assert_eq!((none.x1, one.x1), (cover.x1, cover.x1));
            let g = bpm_mask::find_glyphs(&f, b).unwrap();
            assert_eq!(g.dot, (b.x0 + 22, b.x0 + 23));
        }
    }
}
