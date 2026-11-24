//! Feasibility check for the v0.2 split view: can a click-through overlay of ours show a DWM thumbnail of the
//! rekordbox window, and in which coordinate space is `rcSource`?
//!
//! For each destination-window style it registers a thumbnail of rekordbox, shows a known source rect, grabs the
//! composed screen with GDI (the dest window is NOT excluded from capture here, so the grab can see it) and compares
//! the dest region with the same area of a screen grab taken before the window was shown. A shift search of +-16 px
//! tells where `rcSource` really points (window rect, DWM extended frame or client area).
//!
//! usage: cargo run --release -p club685-app --example thumb_check [out_dir]

use club685_capture::find_rekordbox;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmFlush, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES,
    DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetStockObject, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACK_BRUSH, CAPTUREBLT, DIB_RGB_COLORS, HBRUSH, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetWindowRect, PeekMessageW, RegisterClassW, SetLayeredWindowAttributes,
    SetWindowPos, TranslateMessage, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_SHOWWINDOW, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WINDOW_EX_STYLE,
};

unsafe extern "system" fn wp(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_NCHITTEST {
        return LRESULT(-1);
    }
    DefWindowProcW(h, m, w, l)
}

fn pump(ms: u64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(ms) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = DwmFlush();
        }
    }
}

struct Shot {
    w: i32,
    h: i32,
    bgra: Vec<u8>,
}

fn grab(x: i32, y: i32, w: i32, h: i32) -> Shot {
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let _ = BitBlt(dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut v = vec![0u8; (w * h * 4) as usize];
        GetDIBits(dc, bmp, 0, h as u32, Some(v.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        Shot { w, h, bgra: v }
    }
}

fn mad(a: &Shot, ax: i32, ay: i32, b: &Shot, bx: i32, by: i32, w: i32, h: i32) -> f64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (pa, pb) = (((ay + y) * a.w + ax + x) as usize * 4, ((by + y) * b.w + bx + x) as usize * 4);
            if ay + y < 0 || ax + x < 0 || ay + y >= a.h || ax + x >= a.w {
                return 1e9;
            }
            for c in 0..3 {
                sum += (a.bgra[pa + c] as i32 - b.bgra[pb + c] as i32).unsigned_abs() as u64;
            }
            n += 3;
        }
    }
    sum as f64 / n as f64
}

fn save_png(path: &str, s: &Shot, x: i32, y: i32, w: i32, h: i32) {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for yy in 0..h {
        for xx in 0..w {
            let p = (((y + yy) * s.w) + x + xx) as usize * 4;
            rgba.extend_from_slice(&[s.bgra[p + 2], s.bgra[p + 1], s.bgra[p], 255]);
        }
    }
    let f = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
}

fn main() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
    }
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let rb = find_rekordbox().expect("rekordbox not found");
    let hwnd = rb.hwnd;
    let (mut wr, mut cr, mut co) = (RECT::default(), RECT::default(), POINT::default());
    unsafe {
        let _ = GetWindowRect(hwnd, &mut wr);
        let _ = GetClientRect(hwnd, &mut cr);
        let _ = ClientToScreen(hwnd, &mut co);
    }
    println!("rekordbox window rect   {:?}", (wr.left, wr.top, wr.right, wr.bottom));
    println!("dwm extended bounds     {:?}", (rb.bounds.x0, rb.bounds.y0, rb.bounds.x1, rb.bounds.y1));
    println!("client rect / origin    {:?} @ {:?}", (cr.right, cr.bottom), (co.x, co.y));

    let hinst = unsafe { GetModuleHandleW(None).unwrap() };
    let wc = WNDCLASSW { lpfnWndProc: Some(wp), hInstance: hinst.into(), lpszClassName: w!("ThumbCheck"), hbrBackground: HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0), ..Default::default() };
    unsafe { RegisterClassW(&wc) };

    // 800x160 source rect in extended-frame coordinates, 1:1 destination far below the band
    let (sw, sh) = (800, 160);
    let sx: i32 = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(300);
    let sy: i32 = std::env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(160);
    let (dx, dy) = (100, 1100);
    let full_w = rb.bounds.x1.max(wr.right);
    let full_h = rb.bounds.y1.max(wr.bottom);
    let before = grab(0, 0, full_w, full_h.max(dy + sh + 20));

    let styles: [(&str, WINDOW_EX_STYLE, bool); 2] = [
        ("layered (LAYERED|TRANSPARENT|TOOLWINDOW|NOACTIVATE|TOPMOST, alpha 255)", WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST, true),
        ("non-layered (TRANSPARENT|TOOLWINDOW|NOACTIVATE|TOPMOST + HTTRANSPARENT)", WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST, false),
    ];
    for (name, ex, layered) in styles {
        println!("\n== {name}");
        let dst = unsafe { CreateWindowExW(ex, w!("ThumbCheck"), w!(""), WS_POPUP, dx, dy, sw, sh, None, None, Some(hinst.into()), None).unwrap() };
        if layered {
            unsafe { SetLayeredWindowAttributes(dst, COLORREF(0), 255, LWA_ALPHA).unwrap() };
        }
        let th = unsafe { DwmRegisterThumbnail(dst, hwnd) };
        let th = match th {
            Ok(t) => t,
            Err(e) => {
                println!("DwmRegisterThumbnail FAILED: {e}");
                unsafe { let _ = DestroyWindow(dst); }
                continue;
            }
        };
        let size = unsafe { DwmQueryThumbnailSourceSize(th).unwrap() };
        println!("thumbnail source size   {}x{}", size.cx, size.cy);
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE,
            rcDestination: RECT { left: 0, top: 0, right: sw, bottom: sh },
            rcSource: RECT { left: sx, top: sy, right: sx + sw, bottom: sy + sh },
            opacity: 255,
            fVisible: true.into(),
            fSourceClientAreaOnly: false.into(),
        };
        unsafe {
            DwmUpdateThumbnailProperties(th, &props).unwrap();
            let _ = SetWindowPos(dst, Some(HWND_TOPMOST), dx, dy, sw, sh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        pump(400);
        let after = grab(0, 0, before.w, before.h);
        // expected source on screen (extended-frame origin)
        let (ex0, ey0) = (rb.bounds.x0 + sx, rb.bounds.y0 + sy);
        let mut best = (1e9, 0, 0);
        for oy in -16..=16 {
            for ox in -16..=16 {
                let d = mad(&before, ex0 + ox, ey0 + oy, &after, dx, dy, sw, sh);
                if d < best.0 {
                    best = (d, ox, oy);
                }
            }
        }
        let same_as_before = mad(&before, dx, dy, &after, dx, dy, sw, sh);
        println!("dest region vs what was there before showing: mad {same_as_before:.2} (0 = nothing drawn)");
        println!("best match against screen at extended-origin + {:?}: mad {:.3}", (best.1, best.2), best.0);
        println!("  (mad at exact extended-frame origin: {:.3}, at client origin: {:.3}, at window-rect origin: {:.3})",
            mad(&before, ex0, ey0, &after, dx, dy, sw, sh),
            mad(&before, co.x + sx, co.y + sy, &after, dx, dy, sw, sh),
            mad(&before, wr.left + sx, wr.top + sy, &after, dx, dy, sw, sh));
        let tag = if layered { "layered" } else { "plain" };
        save_png(&format!("{out}/thumb_{tag}_dest.png"), &after, dx, dy, sw, sh);
        save_png(&format!("{out}/thumb_{tag}_src.png"), &before, ex0, ey0, sw, sh);
        unsafe {
            let _ = DwmUnregisterThumbnail(th);
            let _ = DestroyWindow(dst);
        }
        pump(100);
    }
}
