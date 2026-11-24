//! The v0.2 split view: two DWM thumbnails of the rekordbox window inside one click-through body window, plus a small
//! meter strip window above it. No capture, no copy: DWM composes the thumbnails on the GPU every vsync.
//!
//! Measured on Windows 10 19045 (examples/thumb_check): both a layered constant-alpha destination and a plain
//! WS_EX_TRANSPARENT popup render thumbnails pixel-exact, and `rcSource` is in GetWindowRect coordinates, not in
//! DWM extended-frame or client coordinates. A maximised rekordbox has its window rect 8 px outside the extended
//! frame, so layout (extended-frame) coordinates must be shifted by `frame_offset`.

use club685_core::{Rect, Row};
use club685_overlay::{OverlayKind, OverlayWindow};
use club685_render::{lit_block, split_geometry, viewport_source, Composer, MeterKind, SceneDeck};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
    DWM_TNP_VISIBLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, GetWindowRect, HWND_TOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW,
};

/// Where one split view goes. Rects of `src` are in GetWindowRect coordinates of rekordbox, rects of `dst` in the
/// body window's client coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitThumbs {
    pub header: i32,
    pub vw: i32,
    pub body_w: i32,
    pub body_h: i32,
    pub src: [Rect; 2],
    pub dst: [Rect; 2],
}

/// Offset to add to extended-frame coordinates to get GetWindowRect coordinates (8, 8 for a maximised window).
pub fn frame_offset(extended: Rect, window: Rect) -> (i32, i32) {
    (extended.x0 - window.x0, extended.y0 - window.y0)
}

/// Thumbnail rects for the split view (spec section 7). `mask_w` x `mask_h` is the band overlay (the band inset from
/// the window edges), `cursor` the playhead x and `frame_w` the width of the layout frame, both in extended-frame
/// coordinates; `off` is [`frame_offset`].
pub fn split_thumbs(mask_w: i32, mask_h: i32, dpi: u32, meter_on: bool, rows: &[Row; 2], cursor: f32, frame_w: i32, off: (i32, i32)) -> SplitThumbs {
    let row_h = rows.iter().map(|r| r.height() + 2).max().unwrap_or(0);
    let g = split_geometry(mask_w, mask_h, dpi, meter_on, row_h);
    let (sx, _) = viewport_source(cursor, g.vw, frame_w);
    let vw = g.vw.min(frame_w);
    let mut src = [Rect::default(); 2];
    let mut dst = [Rect::default(); 2];
    for d in 0..2 {
        let row = &rows[d];
        let full = row.height() + 2;
        let cut = full - g.content_h;
        let y0 = row.top - 1 + cut / 2;
        src[d] = Rect { x0: sx + off.0, y0: y0 + off.1, x1: sx + vw + off.0, y1: y0 + g.content_h + off.1 };
        let dy = g.dest_y - g.header;
        dst[d] = Rect { x0: g.dest_x[d], y0: dy, x1: g.dest_x[d] + vw, y1: dy + g.content_h };
    }
    SplitThumbs { header: g.header, vw: g.vw, body_w: mask_w, body_h: (mask_h - g.header).max(1), src, dst }
}

/// Black body bitmap with the two 1 px dividers at the gutter edges (BGRA, `w * h * 4` bytes).
pub fn fill_body(dib: &mut [u8], w: i32, h: i32, vw: i32) {
    for px in dib.chunks_exact_mut(4) {
        px.copy_from_slice(&[0, 0, 0, 255]);
    }
    for x in [vw, w - vw - 1] {
        if x < 0 || x >= w {
            continue;
        }
        for y in 0..h {
            let i = ((y * w + x) * 4) as usize;
            dib[i..i + 4].copy_from_slice(&[61, 61, 61, 255]);
        }
    }
}

/// The two registered thumbnails of one rekordbox window on one destination window.
pub struct Thumbs {
    src: HWND,
    ids: [isize; 2],
    applied: Option<[(Rect, Rect); 2]>,
    pub updates: u64,
}

impl Thumbs {
    pub fn new(dst: HWND, src: HWND) -> Option<Self> {
        unsafe {
            let a = DwmRegisterThumbnail(dst, src).ok()?;
            let b = match DwmRegisterThumbnail(dst, src) {
                Ok(b) => b,
                Err(_) => {
                    let _ = DwmUnregisterThumbnail(a);
                    return None;
                }
            };
            Some(Self { src, ids: [a, b], applied: None, updates: 0 })
        }
    }

    pub fn source(&self) -> HWND {
        self.src
    }

    /// Applies new rects; does nothing when they equal the applied ones.
    pub fn update(&mut self, t: &SplitThumbs) {
        let want = [(t.src[0], t.dst[0]), (t.src[1], t.dst[1])];
        if self.applied == Some(want) {
            return;
        }
        for (i, (s, d)) in want.iter().enumerate() {
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE,
                rcDestination: RECT { left: d.x0, top: d.y0, right: d.x1, bottom: d.y1 },
                rcSource: RECT { left: s.x0, top: s.y0, right: s.x1, bottom: s.y1 },
                opacity: 255,
                fVisible: true.into(),
                fSourceClientAreaOnly: false.into(),
            };
            unsafe {
                let _ = DwmUpdateThumbnailProperties(self.ids[i], &props);
            }
        }
        self.applied = Some(want);
        self.updates += 1;
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        for id in self.ids {
            unsafe {
                let _ = DwmUnregisterThumbnail(id);
            }
        }
    }
}

/// GetWindowRect of a window as a core rect.
pub fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    Some(Rect { x0: r.left, y0: r.top, x1: r.right, y1: r.bottom })
}

/// What is on screen in module mode. Rects are screen coordinates, `None` = hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModPlan {
    /// Holds the thumbnails (split view body).
    pub body: Option<Rect>,
    /// Meter strip: the split header, or the whole band for the central meters.
    pub strip: Option<Rect>,
    /// Opaque black band (Hide Both without meter).
    pub solid: Option<Rect>,
}

impl ModPlan {
    pub fn is_empty(&self) -> bool {
        self.body.is_none() && self.strip.is_none() && self.solid.is_none()
    }
}

const REASSERT: Duration = Duration::from_millis(250);

pub struct PresentSet {
    pub body: OverlayWindow,
    pub strip: OverlayWindow,
    solid: OverlayWindow,
    applied: ModPlan,
    applied_at: Option<Instant>,
}

impl PresentSet {
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            body: OverlayWindow::new(OverlayKind::Bitmap)?,
            strip: OverlayWindow::new(OverlayKind::Bitmap)?,
            solid: OverlayWindow::new(OverlayKind::Solid)?,
            applied: ModPlan::default(),
            applied_at: None,
        })
    }

    pub fn current(&self) -> &ModPlan {
        &self.applied
    }

    pub fn visible(&self) -> bool {
        !self.applied.is_empty()
    }

    /// Makes the screen match `plan` atomically; re-asserts topmost every 250 ms while visible.
    pub fn apply(&mut self, plan: &ModPlan) {
        let fresh = self.applied_at.is_some_and(|t| t.elapsed() < REASSERT);
        if *plan == self.applied && (fresh || plan.is_empty()) && self.applied_at.is_some() {
            return;
        }
        let wins = [&self.body, &self.strip, &self.solid];
        let rects = [plan.body, plan.strip, plan.solid];
        let mut ok = true;
        unsafe {
            match BeginDeferWindowPos(3) {
                Ok(mut hdwp) => {
                    for (w, r) in wins.iter().zip(rects.iter()) {
                        let res = match r {
                            Some(r) => DeferWindowPos(hdwp, w.hwnd, Some(HWND_TOPMOST), r.x0, r.y0, r.width(), r.height(), SWP_NOACTIVATE | SWP_SHOWWINDOW),
                            None => DeferWindowPos(hdwp, w.hwnd, None, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER),
                        };
                        match res {
                            Ok(h) => hdwp = h,
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        ok = EndDeferWindowPos(hdwp).is_ok();
                    }
                }
                Err(_) => ok = false,
            }
        }
        if !ok {
            for (w, r) in wins.iter().zip(rects.iter()) {
                match r {
                    Some(r) => w.show_at(*r),
                    None => w.hide(),
                }
            }
        }
        self.applied = *plan;
        self.applied_at = Some(Instant::now());
    }

    pub fn hide_all(&mut self) {
        self.apply(&ModPlan::default());
    }
}

impl Drop for PresentSet {
    fn drop(&mut self) {
        self.body.hide();
        self.strip.hide();
        self.solid.hide();
    }
}

/// Draws the meter strip into its window, and only when something visible changed (Box: the lit block, Line: the
/// phase at 1/64 beat, plus bars, master, size).
#[derive(Default)]
pub struct StripRenderer {
    composer: Composer,
    key: Option<u64>,
    pub redraws: u32,
}

impl StripRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn forget(&mut self) {
        self.key = None;
    }

    /// `mode` 0 = split header (`w` x header), 1 = central meters over the whole band. `cursor` is the middle x in
    /// strip-local pixels (central only). Returns false when it cannot be drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, win: &mut OverlayWindow, w: i32, h: i32, mode: u8, dpi: u32, kind: MeterKind, cursor: Option<f32>, decks: [SceneDeck; 2], master: Option<usize>) -> bool {
        if w <= 0 || h <= 0 {
            return false;
        }
        let q = |p: Option<f32>| p.map_or(-1, |p| if kind == MeterKind::Box { lit_block(p) as i32 } else { (p * 64.0) as i32 });
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (mode, w, h, dpi, kind as u8, q(decks[0].phase), q(decks[1].phase), decks[0].bars, decks[1].bars, master, cursor.map(|c| c as i32)).hash(&mut hs);
        let key = hs.finish();
        if win.dib_size() == Some((w, h)) && self.key == Some(key) {
            return true;
        }
        let ok = if mode == 0 { self.composer.split_header(w, h, dpi, kind, decks, master) } else { self.composer.central_meters(w, h, dpi, kind, cursor, decks, master) };
        if !ok {
            return false;
        }
        let Some(dib) = win.dib_mut(w, h) else { return false };
        if !self.composer.write_bgra(dib) {
            return false;
        }
        let _ = win.present_dib();
        self.key = Some(key);
        self.redraws += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> [Row; 2] {
        [Row { top: 151, bottom: 215 }, Row { top: 220, bottom: 285 }]
    }

    #[test]
    fn thumbnails_are_viewport_wide_left_and_right_aligned() {
        // band 2556 x 138 (window 2560 wide, 2 px inset), 96 dpi, no meter, playhead in the middle
        let t = split_thumbs(2556, 138, 96, false, &rows(), 1280.0, 2560, (8, 8));
        assert_eq!(t.header, 0);
        assert_eq!(t.vw, (2556 - 40) / 2);
        assert_eq!(t.dst[0].x0, 0);
        assert_eq!(t.dst[0].width(), t.vw);
        assert_eq!(t.dst[1].x1, 2556);
        assert_eq!(t.dst[1].width(), t.vw);
        // the gutter between them is 40 px
        assert_eq!(t.dst[1].x0 - t.dst[0].x1, 40);
        // both rows use the same source x, centred on the playhead: x0 = round(1280 - 629)
        assert_eq!(t.src[0].x0, 651 + 8);
        assert_eq!(t.src[1].x0, t.src[0].x0);
        assert_eq!(t.src[0].width(), t.vw);
    }

    #[test]
    fn source_rows_follow_the_layout_rows_and_the_frame_offset() {
        let t = split_thumbs(2556, 138, 96, false, &rows(), 1280.0, 2560, (8, 8));
        // row_h = max(65, 66) + 2 = 68 (each row crop is one px above and below the row, cut evenly when too tall)
        assert_eq!(t.src[0].height(), 68.min(138));
        assert_eq!(t.src[0].y0, 150 + 8);
        assert_eq!(t.src[1].y0, 219 + 8);
        // without the invisible border the offset is zero
        let t0 = split_thumbs(2556, 138, 96, false, &rows(), 1280.0, 2560, (0, 0));
        assert_eq!(t.src[0].x0 - t0.src[0].x0, 8);
        assert_eq!(t.src[1].y0 - t0.src[1].y0, 8);
    }

    #[test]
    fn meter_header_moves_the_body_down() {
        let t = split_thumbs(2556, 138, 96, true, &rows(), 1280.0, 2560, (0, 0));
        assert_eq!(t.header, 52);
        assert_eq!(t.body_h, 138 - 52);
        // destination is body-local: rows centred in the 86 px under the header
        assert_eq!(t.dst[0].y0, (86 - 68) / 2);
        assert_eq!(t.dst[0].y0, t.dst[1].y0);
        assert!(t.dst[0].y1 <= t.body_h);
    }

    #[test]
    fn a_tall_row_is_cut_evenly() {
        let tall = [Row { top: 10, bottom: 109 }, Row { top: 120, bottom: 219 }];
        let t = split_thumbs(2556, 138, 96, true, &tall, 1280.0, 2560, (0, 0));
        // 102 px row in 86 px: 16 px cut, 8 off the top
        assert_eq!(t.src[0].height(), 86);
        assert_eq!(t.src[0].y0, 9 + 8);
    }

    #[test]
    fn left_playhead_clamps_the_source() {
        let t = split_thumbs(2556, 138, 96, false, &rows(), 300.0, 2560, (0, 0));
        assert_eq!(t.src[0].x0, 0);
        let t = split_thumbs(2556, 138, 96, false, &rows(), 2500.0, 2560, (0, 0));
        assert_eq!(t.src[0].x1, 2560);
    }

    #[test]
    fn frame_offset_of_a_maximised_window() {
        let ext = Rect { x0: 0, y0: 0, x1: 2560, y1: 1410 };
        let win = Rect { x0: -8, y0: -8, x1: 2568, y1: 1418 };
        assert_eq!(frame_offset(ext, win), (8, 8));
    }

    #[test]
    fn body_bitmap_has_dividers_at_the_gutter_edges() {
        let (w, h, vw) = (100, 4, 40);
        let mut v = vec![7u8; (w * h * 4) as usize];
        fill_body(&mut v, w, h, vw);
        let px = |x: i32, y: i32| &v[((y * w + x) * 4) as usize..((y * w + x) * 4 + 3) as usize];
        assert_eq!(px(vw, 2), &[61, 61, 61]);
        assert_eq!(px(w - vw - 1, 0), &[61, 61, 61]);
        assert_eq!(px(5, 1), &[0, 0, 0]);
        assert_eq!(px(vw + 1, 1), &[0, 0, 0]);
    }

    #[test]
    fn plan_is_empty_by_default() {
        assert!(ModPlan::default().is_empty());
        assert!(!ModPlan { solid: Some(Rect { x0: 0, y0: 0, x1: 1, y1: 1 }), ..Default::default() }.is_empty());
    }
}
