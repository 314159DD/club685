//! Overlay windows: layered, click-through, topmost, no-activate tool windows (ex style 0x080800A8).

use club685_core::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint, FillRect, GetDC, GetStockObject, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACK_BRUSH, DIB_RGB_COLORS, HBITMAP, HBRUSH, HDC, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
    GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, GWL_EXSTYLE, GWLP_USERDATA, TranslateMessage, WDA_EXCLUDEFROMCAPTURE, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, WM_PAINT,
    WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

static AFFINITY_FAILED: AtomicBool = AtomicBool::new(false);

/// True when `exclude_from_capture` failed for any window of ours (Windows older than 10 2004).
pub fn affinity_failed() -> bool {
    AFFINITY_FAILED.load(Ordering::SeqCst)
}

/// Hides `hwnd` from every screen capture (WDA_EXCLUDEFROMCAPTURE, Windows 10 2004+). Needed for each window we
/// create, so Desktop Duplication never sees our own overlays. On failure it logs and sets `affinity_failed`,
/// which makes the app fall back to Windows.Graphics.Capture.
pub fn exclude_from_capture(hwnd: HWND) -> bool {
    match unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } {
        Ok(()) => true,
        Err(e) => {
            eprintln!("club685: SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) failed: {e}; falling back to Windows.Graphics.Capture");
            AFFINITY_FAILED.store(true, Ordering::SeqCst);
            false
        }
    }
}

/// `exclude_from_capture` for a layered window of ours. Windows refuses the affinity on a layered window that has no
/// layered attributes yet and refuses UpdateLayeredWindow after SetLayeredWindowAttributes, so this sets the
/// attributes, applies the affinity and then resets the layered style. The affinity stays; UpdateLayeredWindow and
/// SetLayeredWindowAttributes work afterwards. Call it right after creating the window, before any layered update.
pub fn exclude_layered_window(hwnd: HWND) -> bool {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let ok = exclude_from_capture(hwnd);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as isize));
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        ok
    }
}

const CLASS: PCWSTR = w!("CLUB685_Overlay");
const HTTRANSPARENT: isize = -1;
const MA_NOACTIVATE: isize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    /// Opaque black rectangle.
    Solid,
    /// Opaque bitmap (split view, meter), painted with BitBlt into a constant-alpha layered window. Not
    /// UpdateLayeredWindow: Desktop Duplication does not honour WDA_EXCLUDEFROMCAPTURE for per-pixel-alpha layered
    /// windows, so the split view fed back into the analysis and the overlay flickered (live 2026-11-24).
    Bitmap,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mem = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if mem != 0 {
                let mut r = RECT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut r);
                let _ = BitBlt(hdc, 0, 0, r.right, r.bottom, Some(HDC(mem as *mut _)), 0, 0, SRCCOPY);
            } else {
                FillRect(hdc, &ps.rcPaint, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let hdc = HDC(wp.0 as *mut _);
            let mut r = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut r);
            FillRect(hdc, &r, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            LRESULT(1)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
}

pub struct OverlayWindow {
    pub hwnd: HWND,
    pub kind: OverlayKind,
    bitmap: Option<(HDC, HBITMAP, i32, i32, *mut u8)>,
}

impl OverlayWindow {
    pub fn new(kind: OverlayKind) -> windows::core::Result<Self> {
        register_class();
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
        let hwnd = unsafe {
            let hinst = GetModuleHandleW(None)?;
            CreateWindowExW(ex, CLASS, w!(""), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst.into()), None)?
        };
        if std::env::var_os("CLUB685_NOEXCLUDE").is_none() {
            exclude_layered_window(hwnd);
        }
        // Both kinds are constant-alpha layered windows (see OverlayKind::Bitmap for why).
        unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)? };
        Ok(Self { hwnd, kind, bitmap: None })
    }

    /// Shows the window at `r` (screen coordinates) without activating it.
    pub fn show_at(&self, r: Rect) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), r.x0, r.y0, r.width(), r.height(), SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_HIDEWINDOW | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER,
            );
        }
    }

    fn ensure_dib(&mut self, w: i32, h: i32) -> windows::core::Result<()> {
        if self.bitmap.as_ref().map_or(true, |b| b.2 != w || b.3 != h) {
            self.free_bitmap();
            unsafe {
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(Some(screen));
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
                ReleaseDC(None, screen);
                let bmp = bmp?;
                SelectObject(dc, HGDIOBJ(bmp.0));
                self.bitmap = Some((dc, bmp, w, h, bits as *mut u8));
            }
        }
        Ok(())
    }

    /// Size of the current DIB.
    pub fn dib_size(&self) -> Option<(i32, i32)> {
        self.bitmap.as_ref().map(|b| (b.2, b.3))
    }

    /// The window's DIB (BGRA, `w * h * 4` bytes), created or resized on demand. Write into it, then `present_dib`.
    pub fn dib_mut(&mut self, w: i32, h: i32) -> Option<&mut [u8]> {
        if w <= 0 || h <= 0 || self.ensure_dib(w, h).is_err() {
            return None;
        }
        let (_, _, _, _, bits) = self.bitmap?;
        Some(unsafe { std::slice::from_raw_parts_mut(bits, (w * h * 4) as usize) })
    }

    /// BitBlts the DIB into the window (no move, no resize, no z-order change).
    pub fn present_dib(&mut self) -> windows::core::Result<()> {
        let Some((dc, _, w, h, _)) = self.bitmap else { return Ok(()) };
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, dc.0 as isize);
            let wdc = GetDC(Some(self.hwnd));
            let r = BitBlt(wdc, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
            ReleaseDC(Some(self.hwnd), wdc);
            r
        }
    }

    /// Pushes a premultiplied BGRA bitmap (`w * h * 4` bytes) to the window at screen position `(x, y)`.
    pub fn update_bitmap(&mut self, x: i32, y: i32, w: i32, h: i32, bgra: &[u8]) -> windows::core::Result<()> {
        assert_eq!(self.kind, OverlayKind::Bitmap);
        assert!(w > 0 && h > 0 && bgra.len() >= (w * h * 4) as usize);
        self.ensure_dib(w, h)?;
        let (_, _, _, _, bits) = self.bitmap.unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits, (w * h * 4) as usize);
            let _ = SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
        }
        self.present_dib()
    }

    fn free_bitmap(&mut self) {
        if let Some((dc, bmp, ..)) = self.bitmap.take() {
            unsafe {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(dc);
            }
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        self.free_bitmap();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Dispatches pending window messages of this thread (the overlay windows live on the calling thread).
pub fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_windows_are_excluded_from_capture() {
        let solid = OverlayWindow::new(OverlayKind::Solid).unwrap();
        let mut bmp = OverlayWindow::new(OverlayKind::Bitmap).unwrap();
        bmp.update_bitmap(0, 0, 2, 2, &[0u8; 16]).unwrap();
        assert!(!affinity_failed(), "SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) failed on this machine");
        for w in [&solid, &bmp] {
            let mut a = 0u32;
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowDisplayAffinity(w.hwnd, &mut a).unwrap() };
            assert_eq!(a, 0x11);
        }
    }
}
