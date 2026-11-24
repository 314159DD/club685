//! The overlay gate (spec 7b). Overlays may be visible only while all of this holds:
//! rekordbox is the foreground window, enabled, visible, not minimised; GetGUIThreadInfo flags & 0x1E == 0
//! (no menu, move/size or popup mode); window rect (DWM extended frame bounds), client rect, DPI and PID equal
//! what they were when the layout was measured; and the layout is younger than 1 s. Any failure hides everything.

use club685_core::Rect;
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GUITHREADINFO,
};

pub const MAX_LAYOUT_AGE_MS: u64 = 1000;
/// GUI_INMOVESIZE (2) | GUI_INMENUMODE (4) | GUI_SYSTEMMENUMODE (8) | GUI_POPUPMENUMODE (0x10).
pub const GUI_BLOCKING_FLAGS: u32 = 0x1E;

/// What must stay constant between layout measurement and showing an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowMetrics {
    /// DWM extended frame bounds, screen coordinates.
    pub window: Rect,
    pub client_w: i32,
    pub client_h: i32,
    pub dpi: u32,
    pub pid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateInputs {
    pub foreground: bool,
    pub enabled: bool,
    pub visible: bool,
    pub iconic: bool,
    pub gui_flags: u32,
    /// Current metrics; `None` when they could not be read.
    pub metrics: Option<WindowMetrics>,
}

/// Metrics at the time the layout was last confirmed by a fresh frame, and when that was (ms, any monotonic clock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutStamp {
    pub metrics: WindowMetrics,
    pub at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    Open,
    NoLayout,
    NotForeground,
    Disabled,
    NotVisible,
    Minimized,
    GuiMode,
    MetricsUnreadable,
    MetricsChanged,
    LayoutStale,
}

impl GateState {
    pub fn is_open(self) -> bool {
        self == GateState::Open
    }

    pub fn label(self) -> &'static str {
        match self {
            GateState::Open => "open",
            GateState::NoLayout => "no-layout",
            GateState::NotForeground => "not-foreground",
            GateState::Disabled => "disabled",
            GateState::NotVisible => "not-visible",
            GateState::Minimized => "minimized",
            GateState::GuiMode => "menu/move/popup",
            GateState::MetricsUnreadable => "metrics-unreadable",
            GateState::MetricsChanged => "window-changed",
            GateState::LayoutStale => "layout-stale",
        }
    }
}

/// Pure gate decision.
pub fn evaluate(inp: &GateInputs, stamp: Option<&LayoutStamp>, now_ms: u64) -> GateState {
    if !inp.visible {
        return GateState::NotVisible;
    }
    if inp.iconic {
        return GateState::Minimized;
    }
    if !inp.enabled {
        return GateState::Disabled;
    }
    if !inp.foreground {
        return GateState::NotForeground;
    }
    if inp.gui_flags & GUI_BLOCKING_FLAGS != 0 {
        return GateState::GuiMode;
    }
    let Some(stamp) = stamp else { return GateState::NoLayout };
    let Some(m) = inp.metrics else { return GateState::MetricsUnreadable };
    if m != stamp.metrics {
        return GateState::MetricsChanged;
    }
    if now_ms.saturating_sub(stamp.at_ms) >= MAX_LAYOUT_AGE_MS {
        return GateState::LayoutStale;
    }
    GateState::Open
}

/// Current window rect (DWM extended frame bounds), client size, DPI and PID.
pub fn query_metrics(hwnd: HWND) -> Option<WindowMetrics> {
    let mut r = RECT::default();
    let mut c = RECT::default();
    let mut pid = 0u32;
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32).ok()?;
        GetClientRect(hwnd, &mut c).ok()?;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 || pid == 0 {
            return None;
        }
        Some(WindowMetrics {
            window: Rect { x0: r.left, y0: r.top, x1: r.right, y1: r.bottom },
            client_w: c.right - c.left,
            client_h: c.bottom - c.top,
            dpi,
            pid,
        })
    }
}

/// Snapshot of everything the gate looks at.
pub fn query_inputs(hwnd: HWND) -> GateInputs {
    unsafe {
        let mut gui = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        let tid = GetWindowThreadProcessId(hwnd, None);
        let gui_flags = if GetGUIThreadInfo(tid, &mut gui).is_ok() { gui.flags.0 } else { 0 };
        GateInputs {
            foreground: GetForegroundWindow() == hwnd,
            enabled: IsWindowEnabled(hwnd).as_bool(),
            visible: IsWindowVisible(hwnd).as_bool(),
            iconic: IsIconic(hwnd).as_bool(),
            gui_flags,
            metrics: query_metrics(hwnd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> WindowMetrics {
        WindowMetrics { window: Rect { x0: 0, y0: 0, x1: 2560, y1: 1410 }, client_w: 2560, client_h: 1380, dpi: 96, pid: 4242 }
    }

    fn good() -> GateInputs {
        GateInputs { foreground: true, enabled: true, visible: true, iconic: false, gui_flags: 0, metrics: Some(metrics()) }
    }

    fn stamp(at: u64) -> LayoutStamp {
        LayoutStamp { metrics: metrics(), at_ms: at }
    }

    #[test]
    fn open_when_everything_holds() {
        assert_eq!(evaluate(&good(), Some(&stamp(1000)), 1500), GateState::Open);
    }

    #[test]
    fn each_failure_closes_the_gate() {
        let s = stamp(1000);
        let mut i = good();
        i.foreground = false;
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::NotForeground);
        let mut i = good();
        i.enabled = false;
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::Disabled);
        let mut i = good();
        i.visible = false;
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::NotVisible);
        let mut i = good();
        i.iconic = true;
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::Minimized);
        assert_eq!(evaluate(&good(), None, 1100), GateState::NoLayout);
    }

    #[test]
    fn gui_flags_mask_is_0x1e() {
        let s = stamp(1000);
        for (flags, open) in [(0u32, true), (1, true), (2, false), (4, false), (8, false), (0x10, false), (0x20, true), (0x1e, false)] {
            let mut i = good();
            i.gui_flags = flags;
            assert_eq!(evaluate(&i, Some(&s), 1100).is_open(), open, "flags {flags:#x}");
        }
    }

    #[test]
    fn metrics_must_match_the_layout() {
        let s = stamp(1000);
        let mut i = good();
        i.metrics = Some(WindowMetrics { window: Rect { x0: 10, ..metrics().window }, ..metrics() });
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::MetricsChanged);
        let mut i = good();
        i.metrics = Some(WindowMetrics { client_h: 1000, ..metrics() });
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::MetricsChanged);
        let mut i = good();
        i.metrics = Some(WindowMetrics { dpi: 120, ..metrics() });
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::MetricsChanged);
        let mut i = good();
        i.metrics = Some(WindowMetrics { pid: 1, ..metrics() });
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::MetricsChanged);
        let mut i = good();
        i.metrics = None;
        assert_eq!(evaluate(&i, Some(&s), 1100), GateState::MetricsUnreadable);
    }

    #[test]
    fn layout_must_be_younger_than_one_second() {
        let s = stamp(1000);
        assert_eq!(evaluate(&good(), Some(&s), 1999), GateState::Open);
        assert_eq!(evaluate(&good(), Some(&s), 2000), GateState::LayoutStale);
        assert_eq!(evaluate(&good(), Some(&s), 5000), GateState::LayoutStale);
    }
}
