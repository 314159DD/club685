//! Pure geometry of the split view, the central view and the meters (spec section 7). No pixels, no fonts.

/// Meter style (`PhaseType` 1 Box / 2 Line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterKind {
    Box,
    Line,
}

/// Gutter between the two viewports: `2 * round(clamp(width * 0.018, 25, 40) / 2)`.
pub fn gutter(width: i32) -> i32 {
    let g = (width as f32 * 0.018).clamp(25.0, 40.0);
    2 * (g / 2.0).round() as i32
}

/// Width of one viewport.
pub fn viewport_width(width: i32) -> i32 {
    ((width - gutter(width)) / 2).max(1)
}

/// Height reserved on top for the meter header when the phase meter is on: `round(65 * dpi / 120)`.
pub fn header_height(dpi: u32) -> i32 {
    (65.0 * dpi as f32 / 120.0).round() as i32
}

/// Source x of a viewport centred on the measured playhead, clamped inside the frame instead of rejected, so a
/// rekordbox "Left" playhead still works. Returns `(source_x0, playhead_offset_inside_viewport)`.
pub fn viewport_source(cursor: f32, vw: i32, frame_w: i32) -> (i32, i32) {
    let vw = vw.min(frame_w).max(1);
    let x0 = ((cursor - vw as f32 / 2.0).round() as i32).clamp(0, (frame_w - vw).max(0));
    (x0, (cursor - x0 as f32).round() as i32)
}

/// Everything the split compositor needs to know about where things go. Coordinates are overlay-local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitGeometry {
    pub width: i32,
    pub height: i32,
    pub gutter: i32,
    pub vw: i32,
    pub header: i32,
    /// Left edge of each viewport: deck 1 left-aligned, deck 2 right-aligned.
    pub dest_x: [i32; 2],
    /// Top of the row content and its height (rows are centred in the area under the header).
    pub dest_y: i32,
    pub content_h: i32,
}

pub fn split_geometry(width: i32, height: i32, dpi: u32, phase_on: bool, row_h: i32) -> SplitGeometry {
    let g = gutter(width);
    let vw = viewport_width(width);
    let header = if phase_on { header_height(dpi).min(height / 2) } else { 0 };
    let body = (height - header).max(1);
    let content_h = row_h.min(body).max(1);
    SplitGeometry {
        width,
        height,
        gutter: g,
        vw,
        header,
        dest_x: [0, width - vw],
        dest_y: header + (body - content_h) / 2,
        content_h,
    }
}

/// Meter width in logical (96 dpi) pixels: `clamp(paneWidth / s * 0.2, 220 box / 230 line, 300)`.
pub fn meter_width_logical(pane_w: f32, dpi: u32, kind: MeterKind) -> f32 {
    let s = dpi as f32 / 96.0;
    let min = match kind {
        MeterKind::Box => 220.0,
        MeterKind::Line => 230.0,
    };
    (pane_w / s * 0.2).clamp(min, 300.0)
}

/// Meter scale: `s * 0.78` in the split header, `s * 1.0` in the central view.
pub fn meter_scale(dpi: u32, central: bool) -> f32 {
    let s = dpi as f32 / 96.0;
    if central {
        s
    } else {
        s * 0.78
    }
}

/// Shrinks `scale` so that content that is `total_at_scale` wide at `scale` fits in `avail` (pane width minus 12).
pub fn fit_scale(scale: f32, total_at_scale: f32, avail: f32) -> f32 {
    if total_at_scale <= avail || total_at_scale <= 0.0 {
        scale
    } else {
        scale * avail / total_at_scale
    }
}

/// Logical meter height.
pub fn meter_height_logical(kind: MeterKind) -> f32 {
    match kind {
        MeterKind::Box => 24.0,
        MeterKind::Line => 28.0,
    }
}

/// Which beat block is lit for a phase in beats (0.0 <= phase < 4.0).
pub fn lit_block(phase: f32) -> usize {
    (phase.rem_euclid(4.0).floor() as usize).min(3)
}

/// Tick positions of the line meter relative to the meter's left edge, `(x, is_bar)`. Ticks sit every `width / 8`
/// and slide with `-phase`; every 4th tick (a bar line) is red and taller. The playhead is the centre.
pub fn line_ticks(width: f32, phase: f32) -> Vec<(f32, bool)> {
    let sp = width / 8.0;
    let cx = width / 2.0;
    let mut v = Vec::new();
    for i in -4i32..=8 {
        let x = cx + (i as f32 - phase) * sp;
        if (-0.01..=width + 0.01).contains(&x) {
            v.push((x, i.rem_euclid(4) == 0));
        }
    }
    v
}

/// Stacked rows of the central view, master first. Returns the deck order.
pub fn central_order(master: Option<usize>) -> [usize; 2] {
    match master {
        Some(1) => [1, 0],
        _ => [0, 1],
    }
}

/// "NN.N Bars" (`%02d.%d Bars`).
pub fn bars_text(bars: Option<(u32, u8)>) -> String {
    match bars {
        Some((bar, beat)) => format!("{bar:02}.{beat} Bars"),
        None => "--.- Bars".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_is_even_and_clamped() {
        assert_eq!(gutter(2556), 40);
        for w in [400, 800, 1300, 1920, 2556] {
            assert_eq!(gutter(w) % 2, 0);
            assert!((24..=40).contains(&gutter(w)));
        }
        // 1920 * 0.018 = 34.56 -> 17.28 rounds to 17 -> 34
        assert_eq!(gutter(1920), 34);
        // 800 * 0.018 = 14.4 -> clamped to 25 -> 12.5 rounds to 13 -> 26
        assert_eq!(gutter(800), 26);
    }

    #[test]
    fn viewport_fills_the_rest() {
        let w = 2556;
        assert_eq!(viewport_width(w), (2556 - 40) / 2);
        assert!(2 * viewport_width(w) + gutter(w) <= w);
    }

    #[test]
    fn header_scales_with_dpi() {
        assert_eq!(header_height(120), 65);
        assert_eq!(header_height(96), 52);
        assert_eq!(header_height(144), 78);
    }

    #[test]
    fn viewport_is_centred_on_the_measured_cursor() {
        let (x0, off) = viewport_source(1278.0, 1258, 2560);
        assert_eq!(x0, 649);
        assert_eq!(off, 629);
        // measured, not assumed: a cursor off centre shifts the viewport
        let (x0b, _) = viewport_source(1180.0, 1258, 2560);
        assert_eq!(x0b, 551);
    }

    #[test]
    fn left_playhead_clamps_inside_the_band() {
        let (x0, off) = viewport_source(300.0, 1258, 2560);
        assert_eq!(x0, 0);
        assert_eq!(off, 300);
        let (x0, off) = viewport_source(2500.0, 1258, 2560);
        assert_eq!(x0, 2560 - 1258);
        assert_eq!(off, 2500 - x0);
    }

    #[test]
    fn viewport_wider_than_frame_does_not_panic() {
        let (x0, _) = viewport_source(100.0, 5000, 800);
        assert_eq!(x0, 0);
    }

    #[test]
    fn split_geometry_places_decks_left_and_right() {
        let g = split_geometry(2556, 138, 96, true, 67);
        assert_eq!(g.header, 52);
        assert_eq!(g.dest_x, [0, 2556 - g.vw]);
        assert_eq!(g.dest_y, 52 + (86 - 67) / 2);
        let g = split_geometry(2556, 138, 96, false, 67);
        assert_eq!(g.header, 0);
        assert_eq!(g.dest_y, (138 - 67) / 2);
        // a row taller than the space is cut instead of overflowing
        let g = split_geometry(2556, 138, 96, true, 200);
        assert_eq!(g.content_h, 86);
        assert_eq!(g.dest_y, 52);
    }

    #[test]
    fn meter_width_follows_the_clamp() {
        // 96 dpi, pane 1000: 200 -> min 220 (box) / 230 (line)
        assert_eq!(meter_width_logical(1000.0, 96, MeterKind::Box), 220.0);
        assert_eq!(meter_width_logical(1000.0, 96, MeterKind::Line), 230.0);
        assert_eq!(meter_width_logical(1250.0, 96, MeterKind::Box), 250.0);
        assert_eq!(meter_width_logical(5000.0, 96, MeterKind::Box), 300.0);
        // 120 dpi works in logical pixels: 1562 px pane = 1249.6 logical -> 249.9
        let w = meter_width_logical(1562.0, 120, MeterKind::Box);
        assert!((w - 249.92).abs() < 0.05);
    }

    #[test]
    fn meter_scale_split_and_central() {
        assert!((meter_scale(96, false) - 0.78).abs() < 1e-6);
        assert!((meter_scale(96, true) - 1.0).abs() < 1e-6);
        assert!((meter_scale(120, false) - 0.975).abs() < 1e-6);
    }

    #[test]
    fn fit_scale_only_shrinks() {
        assert_eq!(fit_scale(1.0, 300.0, 400.0), 1.0);
        assert!((fit_scale(1.0, 400.0, 200.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lit_block_follows_beat() {
        assert_eq!(lit_block(0.0), 0);
        assert_eq!(lit_block(0.99), 0);
        assert_eq!(lit_block(1.0), 1);
        assert_eq!(lit_block(3.9), 3);
        assert_eq!(lit_block(4.0), 0);
        assert_eq!(lit_block(-0.1), 3);
    }

    #[test]
    fn ticks_slide_and_every_fourth_is_a_bar() {
        let t = line_ticks(240.0, 0.0);
        // spacing is width / 8 = 30, the red tick sits under the playhead at phase 0
        assert!(t.iter().any(|&(x, bar)| (x - 120.0).abs() < 1e-3 && bar));
        assert!(t.iter().any(|&(x, bar)| (x - 150.0).abs() < 1e-3 && !bar));
        // the ticks run from -4 to +4 beats around the playhead: the edges are red bar ticks
        assert!(t.iter().any(|&(x, bar)| x.abs() < 1e-3 && bar));
        let t2 = line_ticks(240.0, 1.0);
        assert!(t2.iter().any(|&(x, bar)| (x - 120.0).abs() < 1e-3 && !bar));
        assert!(t2.iter().any(|&(x, bar)| (x - 90.0).abs() < 1e-3 && bar));
        for (x, _) in &t2 {
            assert!((0.0..=240.0).contains(x));
        }
    }

    #[test]
    fn central_puts_master_on_top() {
        assert_eq!(central_order(Some(0)), [0, 1]);
        assert_eq!(central_order(Some(1)), [1, 0]);
        assert_eq!(central_order(None), [0, 1]);
    }

    #[test]
    fn bars_text_format() {
        assert_eq!(bars_text(Some((5, 1))), "05.1 Bars");
        assert_eq!(bars_text(Some((113, 1))), "113.1 Bars");
        assert_eq!(bars_text(None), "--.- Bars");
    }
}
