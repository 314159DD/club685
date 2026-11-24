//! Finds the rekordbox 6.8.5 main window: process rekordbox.exe, top-level visible window of class prefix `JUCE_`
//! with the largest area (skipping tiny border windows), file version 6.8.x.

use club685_core::Rect;
use std::ffi::c_void;
use std::fmt;
use windows::core::{BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};

/// Windows smaller than this are rekordbox's thin border windows, not the main window.
pub const MIN_MAIN_W: i32 = 600;
pub const MIN_MAIN_H: i32 = 400;

#[derive(Clone, Debug)]
pub struct RbWindow {
    pub hwnd: HWND,
    pub pid: u32,
    pub class: String,
    /// DWM extended frame bounds in screen coordinates: the area a capture frame shows, at 1:1.
    pub bounds: Rect,
    pub exe_path: String,
    pub version: (u16, u16, u16, u16),
}

#[derive(Debug)]
pub enum FindError {
    NotRunning,
    NoMainWindow { pid: u32 },
    WrongVersion { path: String, version: Option<(u16, u16, u16, u16)> },
}

impl fmt::Display for FindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FindError::NotRunning => write!(f, "rekordbox.exe is not running"),
            FindError::NoMainWindow { pid } => write!(f, "rekordbox (pid {pid}) has no visible JUCE_ main window"),
            FindError::WrongVersion { version: Some(v), .. } => {
                write!(f, "rekordbox file version {}.{}.{}.{} is not 6.8.x", v.0, v.1, v.2, v.3)
            }
            FindError::WrongVersion { version: None, .. } => write!(f, "could not read the rekordbox file version"),
        }
    }
}

impl std::error::Error for FindError {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// DWM extended frame bounds of a window (screen coordinates), the area WGC delivers.
pub fn extended_frame_bounds(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32).ok()?;
    }
    Some(Rect { x0: r.left, y0: r.top, x1: r.right, y1: r.bottom })
}

fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Fixed file version (major, minor, build, revision) from the version resource.
pub fn file_version(path: &str) -> Option<(u16, u16, u16, u16)> {
    let w = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, buf.as_mut_ptr() as *mut c_void).ok()?;
        let mut p: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let root = wide("\\");
        if !VerQueryValueW(buf.as_ptr() as *const c_void, PCWSTR(root.as_ptr()), &mut p, &mut len).as_bool() || p.is_null() {
            return None;
        }
        if (len as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>() {
            return None;
        }
        let info = &*(p as *const VS_FIXEDFILEINFO);
        Some(((info.dwFileVersionMS >> 16) as u16, info.dwFileVersionMS as u16, (info.dwFileVersionLS >> 16) as u16, info.dwFileVersionLS as u16))
    }
}

pub fn version_ok(v: (u16, u16, u16, u16)) -> bool {
    v.0 == 6 && v.1 == 8
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let v = &mut *(lparam.0 as *mut Vec<HWND>);
    v.push(hwnd);
    BOOL(1)
}

/// Candidate rule shared with the tests: JUCE class prefix and not a tiny border window.
pub fn is_main_candidate(class: &str, bounds: Rect) -> bool {
    class.starts_with("JUCE_") && bounds.width() >= MIN_MAIN_W && bounds.height() >= MIN_MAIN_H
}

pub fn find_rekordbox() -> Result<RbWindow, FindError> {
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    let mut best: Option<(i64, RbWindow)> = None;
    let mut seen_rb: Option<(u32, String)> = None;
    for hwnd in all {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            continue;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            continue;
        }
        let Some(path) = image_path(pid) else { continue };
        let is_rb = path.rsplit(['\\', '/']).next().is_some_and(|n| n.eq_ignore_ascii_case("rekordbox.exe"));
        if !is_rb {
            continue;
        }
        seen_rb.get_or_insert((pid, path.clone()));
        let class = class_name(hwnd);
        let Some(bounds) = extended_frame_bounds(hwnd) else { continue };
        if !is_main_candidate(&class, bounds) {
            continue;
        }
        let area = bounds.width() as i64 * bounds.height() as i64;
        if best.as_ref().map_or(true, |(a, _)| area > *a) {
            best = Some((area, RbWindow { hwnd, pid, class, bounds, exe_path: path, version: (0, 0, 0, 0) }));
        }
    }
    let Some((_, mut w)) = best else {
        return Err(match seen_rb {
            Some((pid, _)) => FindError::NoMainWindow { pid },
            None => FindError::NotRunning,
        });
    };
    let v = file_version(&w.exe_path);
    match v {
        Some(v) if version_ok(v) => {
            w.version = v;
            Ok(w)
        }
        other => Err(FindError::WrongVersion { path: w.exe_path, version: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_window_rule() {
        let big = Rect { x0: 0, y0: 0, x1: 2560, y1: 1410 };
        let thin = Rect { x0: 0, y0: 0, x1: 2560, y1: 6 };
        assert!(is_main_candidate("JUCE_a1b2c3", big));
        assert!(!is_main_candidate("JUCE_a1b2c3", thin));
        assert!(!is_main_candidate("Chrome_WidgetWin_1", big));
    }

    #[test]
    fn version_rule() {
        assert!(version_ok((6, 8, 5, 0)));
        assert!(!version_ok((6, 7, 4, 0)));
        assert!(!version_ok((7, 2, 0, 0)));
    }
}
