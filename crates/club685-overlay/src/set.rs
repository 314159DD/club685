//! The overlay set: two BPM masks and one waveform mask, shown, moved and hidden together with one
//! BeginDeferWindowPos / DeferWindowPos / EndDeferWindowPos batch.

use crate::geometry::Plan;
use crate::window::{OverlayKind, OverlayWindow};
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, HWND_TOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW,
};

/// Visible overlays re-assert HWND_TOPMOST at this interval even when the plan did not change.
const REASSERT: Duration = Duration::from_millis(250);

pub struct OverlaySet {
    bpm: [OverlayWindow; 2],
    wave: OverlayWindow,
    bitmap: OverlayWindow,
    applied: Plan,
    applied_at: Option<Instant>,
}

impl OverlaySet {
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            bpm: [OverlayWindow::new(OverlayKind::Solid)?, OverlayWindow::new(OverlayKind::Solid)?],
            wave: OverlayWindow::new(OverlayKind::Solid)?,
            bitmap: OverlayWindow::new(OverlayKind::Bitmap)?,
            applied: Plan::default(),
            applied_at: None,
        })
    }

    pub fn current(&self) -> &Plan {
        &self.applied
    }

    /// Makes the screen match `plan` atomically. Does nothing when the plan is unchanged and was applied recently.
    pub fn apply(&mut self, plan: &Plan) {
        let fresh = self.applied_at.is_some_and(|t| t.elapsed() < REASSERT);
        if *plan == self.applied && (fresh || plan.is_empty()) && self.applied_at.is_some() {
            return;
        }
        let wins = [&self.bpm[0], &self.bpm[1], &self.wave, &self.bitmap];
        let rects = [plan.bpm[0], plan.bpm[1], plan.wave, plan.bitmap];
        let mut ok = true;
        unsafe {
            match BeginDeferWindowPos(4) {
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
            // Fall back to one call per window; hiding must never be skipped.
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

    /// Pushes a premultiplied BGRA composite (`w * h * 4` bytes) to the bitmap overlay at screen position `(x, y)`.
    /// Call after `apply` with a plan whose `bitmap` rect has the same size.
    pub fn present_bitmap(&mut self, x: i32, y: i32, w: i32, h: i32, bgra: &[u8]) -> windows::core::Result<()> {
        if w <= 0 || h <= 0 || bgra.len() < (w * h * 4) as usize {
            return Ok(());
        }
        self.bitmap.update_bitmap(x, y, w, h, bgra)
    }

    /// Hides every overlay at once.
    pub fn hide_all(&mut self) {
        self.apply(&Plan::default());
    }
}

impl Drop for OverlaySet {
    fn drop(&mut self) {
        self.bpm[0].hide();
        self.bpm[1].hide();
        self.wave.hide();
        self.bitmap.hide();
    }
}
