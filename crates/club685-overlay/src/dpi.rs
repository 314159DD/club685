//! DPI rule (spec section 8): rekordbox is system DPI aware. On a secondary monitor with a different scale Windows
//! bitmap-stretches the window and every pixel rule breaks, so refuse instead of guessing.

use std::fmt;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiError {
    /// Window DPI and monitor DPI differ.
    Mismatch { window: u32, monitor: u32 },
    /// A DPI query failed.
    Unknown,
}

impl fmt::Display for DpiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DpiError::Mismatch { window, monitor } => {
                write!(f, "move rekordbox to the main display (window dpi {window}, monitor dpi {monitor})")
            }
            DpiError::Unknown => write!(f, "could not read the DPI of rekordbox or its monitor"),
        }
    }
}

impl std::error::Error for DpiError {}

/// Pure rule: the scale is usable only when both DPIs are known and equal.
pub fn dpi_decision(window_dpi: u32, monitor_dpi: u32) -> Result<u32, DpiError> {
    if window_dpi == 0 || monitor_dpi == 0 {
        return Err(DpiError::Unknown);
    }
    if window_dpi != monitor_dpi {
        return Err(DpiError::Mismatch { window: window_dpi, monitor: monitor_dpi });
    }
    Ok(window_dpi)
}

/// Returns the window DPI when it equals the DPI of the monitor the window is on.
pub fn check_dpi(hwnd: HWND) -> Result<u32, DpiError> {
    let window = unsafe { GetDpiForWindow(hwnd) };
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let (mut x, mut y) = (0u32, 0u32);
    if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.is_err() {
        return Err(DpiError::Unknown);
    }
    dpi_decision(window, x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_dpi_is_accepted() {
        assert_eq!(dpi_decision(96, 96), Ok(96));
        assert_eq!(dpi_decision(144, 144), Ok(144));
    }

    #[test]
    fn mismatch_is_refused_with_the_hint() {
        let e = dpi_decision(96, 144).unwrap_err();
        assert_eq!(e, DpiError::Mismatch { window: 96, monitor: 144 });
        assert!(e.to_string().contains("move rekordbox to the main display"));
    }

    #[test]
    fn zero_is_unknown() {
        assert_eq!(dpi_decision(0, 96), Err(DpiError::Unknown));
    }
}
