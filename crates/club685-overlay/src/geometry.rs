//! Coordinate math: core rects are in frame (= window) coordinates, overlays live in screen coordinates.

use club685_core::Rect;

/// Waveform mask inset from the window edge: ceil(2 * dpi / 96) px (spec 7b).
pub fn waveform_inset(dpi: u32) -> i32 {
    (2 * dpi as i32 + 95) / 96
}

/// Frame rect to screen rect. `origin` is the window's DWM extended frame bounds top-left.
pub fn to_screen(origin: (i32, i32), r: Rect) -> Rect {
    Rect { x0: r.x0 + origin.0, y0: r.y0 + origin.1, x1: r.x1 + origin.0, y1: r.y1 + origin.1 }
}

/// The waveform mask: the band, inset from the left and right window edges so the window border stays visible.
/// (The band already carries a black margin above and below the rows, so no vertical inset is applied.)
pub fn waveform_mask_rect(band: Rect, frame_w: i32, dpi: u32) -> Rect {
    let n = waveform_inset(dpi);
    Rect { x0: band.x0.max(n), y0: band.y0, x1: band.x1.min(frame_w - n), y1: band.y1 }
}

/// What should be on screen right now. `None` means that overlay is hidden. Rects are screen coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub bpm: [Option<Rect>; 2],
    /// Opaque black waveform mask.
    pub wave: Option<Rect>,
    /// Per-pixel bitmap overlay (split view, central meters), same rect rules as `wave`.
    pub bitmap: Option<Rect>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.bpm.iter().all(|b| b.is_none()) && self.wave.is_none() && self.bitmap.is_none()
    }
}

/// Builds the plan. `bpm` are core mask rects per deck (frame coordinates), `band` the layout band.
pub fn plan_overlays(origin: (i32, i32), dpi: u32, frame_w: i32, bpm: [Option<Rect>; 2], band: Option<Rect>, hide_waveform: bool) -> Plan {
    let wave = match (hide_waveform, band) {
        (true, Some(b)) => Some(to_screen(origin, waveform_mask_rect(b, frame_w, dpi))),
        _ => None,
    };
    Plan { bpm: [bpm[0].map(|r| to_screen(origin, r)), bpm[1].map(|r| to_screen(origin, r))], wave, bitmap: None }
}

/// What covers the waveform band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveOverlay {
    None,
    /// Opaque black rectangle.
    Solid,
    /// A bitmap composite (split view, central meters).
    Bitmap,
}

/// Like [`plan_overlays`] with the waveform cover chosen explicitly.
pub fn plan_overlays_ex(origin: (i32, i32), dpi: u32, frame_w: i32, bpm: [Option<Rect>; 2], band: Option<Rect>, wave: WaveOverlay) -> Plan {
    let mut p = plan_overlays(origin, dpi, frame_w, bpm, band, wave == WaveOverlay::Solid);
    if let (WaveOverlay::Bitmap, Some(b)) = (wave, band) {
        p.bitmap = Some(to_screen(origin, waveform_mask_rect(b, frame_w, dpi)));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inset_is_ceil_of_two_dpi_scaled() {
        assert_eq!(waveform_inset(96), 2);
        assert_eq!(waveform_inset(120), 3);
        assert_eq!(waveform_inset(144), 3);
        assert_eq!(waveform_inset(168), 4);
        assert_eq!(waveform_inset(192), 4);
    }

    #[test]
    fn screen_rects_add_the_window_origin() {
        let r = to_screen((100, -50), Rect { x0: 218, y0: 316, x1: 260, y1: 326 });
        assert_eq!(r, Rect { x0: 318, y0: 266, x1: 360, y1: 276 });
        // second monitor to the left of the main one
        let r = to_screen((-2560, 0), Rect { x0: 0, y0: 149, x1: 2560, y1: 287 });
        assert_eq!((r.x0, r.x1), (-2560, 0));
    }

    #[test]
    fn waveform_mask_is_inset_from_both_window_edges() {
        let band = Rect { x0: 0, y0: 149, x1: 2560, y1: 287 };
        assert_eq!(waveform_mask_rect(band, 2560, 96), Rect { x0: 2, y0: 149, x1: 2558, y1: 287 });
    }

    #[test]
    fn plan_places_everything_in_screen_space() {
        let band = Rect { x0: 0, y0: 149, x1: 2560, y1: 287 };
        let bpm = [Some(Rect { x0: 217, y0: 315, x1: 255, y1: 326 }), None];
        let p = plan_overlays((10, 20), 96, 2560, bpm, Some(band), true);
        assert_eq!(p.bpm[0], Some(Rect { x0: 227, y0: 335, x1: 265, y1: 346 }));
        assert_eq!(p.bpm[1], None);
        assert_eq!(p.wave, Some(Rect { x0: 12, y0: 169, x1: 2568, y1: 307 }));
        let p = plan_overlays((10, 20), 96, 2560, bpm, Some(band), false);
        assert_eq!(p.wave, None);
        assert!(plan_overlays((0, 0), 96, 2560, [None, None], None, true).is_empty());
    }

    #[test]
    fn bitmap_plan_uses_the_same_rect_as_the_solid_mask() {
        let band = Rect { x0: 0, y0: 149, x1: 2560, y1: 287 };
        let solid = plan_overlays_ex((10, 20), 96, 2560, [None, None], Some(band), WaveOverlay::Solid);
        let bmp = plan_overlays_ex((10, 20), 96, 2560, [None, None], Some(band), WaveOverlay::Bitmap);
        assert_eq!(bmp.bitmap, solid.wave);
        assert_eq!(bmp.wave, None);
        assert_eq!(solid.bitmap, None);
        assert!(!bmp.is_empty());
        assert!(plan_overlays_ex((0, 0), 96, 2560, [None, None], Some(band), WaveOverlay::None).is_empty());
    }
}
