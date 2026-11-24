//! CLUB685 capture: finds the rekordbox 6.8.5 main window and reads BGRA frames of it with
//! DXGI Desktop Duplication (default, no border on Windows 10) or Windows.Graphics.Capture. Only the band area is copied to the CPU once the layout is known.

pub mod dup;
pub mod framebuf;
pub mod freshness;
pub mod session;
pub mod window;

pub use framebuf::FrameBuf;
pub use freshness::{now_ticks, Freshness, Verdict, MAX_AGE_TICKS, MAX_FUTURE_TICKS};
pub use dup::DuplicationSession;
pub use session::{Blocked, CaptureSession, Frame, Next, Stats};
pub use window::{extended_frame_bounds, find_rekordbox, FindError, RbWindow};

use club685_core::Rect;
use windows::Win32::Foundation::HWND;

/// Which capture API feeds the analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Windows.Graphics.Capture: sees the window itself, but Windows 10 draws a yellow border around it.
    Wgc,
    /// DXGI Desktop Duplication: no border, sees the screen (see `Capture::sees_screen`).
    Dup,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wgc" => Some(Backend::Wgc),
            "dup" | "dxgi" => Some(Backend::Dup),
            _ => None,
        }
    }

    /// `CLUB685_CAPTURE=wgc|dup`, default Wgc. Dup is border-free, but on Windows 10 it shows our excluded overlays as
    /// black (affinity_check example, 2026-11-24), so the split view hides the band it needs and the overlay flickers.
    pub fn from_env() -> Backend {
        std::env::var("CLUB685_CAPTURE").ok().and_then(|v| Backend::parse(&v)).unwrap_or(Backend::Wgc)
    }

    pub fn label(self) -> &'static str {
        match self {
            Backend::Wgc => "wgc",
            Backend::Dup => "dup",
        }
    }
}

/// A running capture of the rekordbox window with either backend. Frames are in window coordinates.
pub enum Capture {
    Wgc(CaptureSession),
    Dup(DuplicationSession),
}

impl Capture {
    pub fn start(backend: Backend, hwnd: HWND) -> windows::core::Result<Capture> {
        Ok(match backend {
            Backend::Wgc => Capture::Wgc(CaptureSession::start(hwnd)?),
            Backend::Dup => Capture::Dup(DuplicationSession::start(hwnd)?),
        })
    }

    pub fn backend(&self) -> Backend {
        match self {
            Capture::Wgc(_) => Backend::Wgc,
            Capture::Dup(_) => Backend::Dup,
        }
    }

    /// True when frames show the screen, so a window covering rekordbox would be in the picture. Analysis must
    /// then only use frames while rekordbox is the foreground window.
    pub fn sees_screen(&self) -> bool {
        matches!(self, Capture::Dup(_))
    }

    pub fn next(&mut self, wait_ms: u32) -> windows::core::Result<Next> {
        match self {
            Capture::Wgc(s) => s.next(wait_ms),
            Capture::Dup(s) => s.next(wait_ms),
        }
    }

    pub fn read_rect(&mut self, frame: &Frame, rect: Rect, dst: &mut FrameBuf) -> windows::core::Result<Rect> {
        match self {
            Capture::Wgc(s) => s.read_rect(frame, rect, dst),
            Capture::Dup(s) => s.read_rect(frame, rect, dst),
        }
    }

    pub fn read_full(&mut self, frame: &Frame, dst: &mut FrameBuf) -> windows::core::Result<Rect> {
        self.read_rect(frame, Rect { x0: 0, y0: 0, x1: frame.width as i32, y1: frame.height as i32 }, dst)
    }

    pub fn stats(&self) -> Stats {
        match self {
            Capture::Wgc(s) => s.stats,
            Capture::Dup(s) => s.stats,
        }
    }
}

/// Rows of the window the analysis needs once the layout is known: from just above the band down to the end of
/// the MASTER / BPM header windows (`band.y1 + 50`, see core `find_layout`), full window width.
pub fn analysis_rect(layout: &club685_core::Layout) -> club685_core::Rect {
    club685_core::Rect {
        x0: 0,
        y0: (layout.band.y0 - 2).max(0),
        x1: layout.width,
        y1: (layout.band.y1 + 52).min(layout.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use club685_core::{Layout, Row};

    #[test]
    fn backend_parse() {
        assert_eq!(Backend::parse("WGC"), Some(Backend::Wgc));
        assert_eq!(Backend::parse(" dup "), Some(Backend::Dup));
        assert_eq!(Backend::parse("x"), None);
    }

    #[test]
    fn analysis_rect_covers_band_and_header_windows() {
        let l = Layout {
            width: 2560,
            height: 1410,
            band: Rect { x0: 0, y0: 149, x1: 2560, y1: 287 },
            rows: [Row { top: 151, bottom: 215 }, Row { top: 220, bottom: 285 }],
            master: [Rect::default(); 2],
        };
        let r = analysis_rect(&l);
        assert_eq!((r.y0, r.y1), (147, 339));
        // the master window ends at band_bottom (286) + 50 = 336
        assert!(r.y1 >= 336);
    }
}
