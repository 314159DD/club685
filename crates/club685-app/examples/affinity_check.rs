//! Checks that CLUB685's overlay windows stay out of Desktop Duplication frames.
//! It opens a plain white target window, puts a magenta Bitmap overlay and a black Solid overlay on top, captures
//! the target with the Dup backend and counts overlay-coloured pixels. Expect 0 of each.
//! usage: cargo run --release -p club685-app --example affinity_check

use club685_capture::{Backend, Capture, FrameBuf, Next};
use club685_core::Rect;
use club685_overlay::{pump_messages, OverlayKind, OverlayWindow};
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, HWND_TOPMOST, SWP_SHOWWINDOW, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE};

unsafe extern "system" fn wp(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

fn main() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
    }
    let (tx, ty, tw, th) = (0, 0, 600, 300);
    let target = unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: Some(wp), hInstance: hinst.into(), lpszClassName: w!("AffTarget"), hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0), ..Default::default() };
        RegisterClassW(&wc);
        CreateWindowExW(WS_EX_TOOLWINDOW, w!("AffTarget"), w!(""), WS_POPUP | WS_VISIBLE, tx, ty, tw, th, None, None, Some(hinst.into()), None).unwrap()
    };
    unsafe {
        let _ = SetWindowPos(target, Some(HWND_TOPMOST), tx, ty, tw, th, SWP_SHOWWINDOW);
    }

    let mut bmp = OverlayWindow::new(OverlayKind::Bitmap).unwrap();
    let solid = OverlayWindow::new(OverlayKind::Solid).unwrap();
    let (bw, bh) = (200, 100);
    let magenta: Vec<u8> = (0..bw * bh).flat_map(|_| [255u8, 0, 255, 255]).collect();
    let br = Rect { x0: tx + 50, y0: ty + 50, x1: tx + 50 + bw, y1: ty + 50 + bh };
    let sr = Rect { x0: tx + 350, y0: ty + 50, x1: tx + 550, y1: ty + 150 };
    bmp.show_at(br);
    bmp.update_bitmap(br.x0, br.y0, bw, bh, &magenta).unwrap();
    solid.show_at(sr);

    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(500) {
        pump_messages();
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut cap = Capture::start(Backend::Dup, target).expect("dup start");
    let mut buf = FrameBuf::new();
    // Duplication only yields frames when the screen changes, and its first image can be blank: keep repainting the
    // bitmap overlay (that is what the app does every frame) and take the last frame after 1.5 s.
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut got = 0;
    while Instant::now() < deadline {
        pump_messages();
        bmp.update_bitmap(br.x0, br.y0, bw, bh, &magenta).unwrap();
        if let Ok(Next::Frame(f)) = cap.next(30) {
            cap.read_full(&f, &mut buf).unwrap();
            got += 1;
        }
    }
    println!("frames captured: {got}");
    let got = got > 0;
    if !got {
        println!("no dup frame captured");
        std::process::exit(2);
    }
    let f = buf.bgra().unwrap();
    let (mut magenta_px, mut black_px, mut white_px) = (0u32, 0u32, 0u32);
    for y in 0..f.h() {
        for x in 0..f.w() {
            let (r, g, b) = f.rgb(x, y);
            if r > 200 && g < 60 && b > 200 {
                magenta_px += 1;
            } else if r < 30 && g < 30 && b < 30 {
                black_px += 1;
            } else if r > 200 && g > 200 && b > 200 {
                white_px += 1;
            }
        }
    }
    println!("captured {}x{}: magenta (bitmap overlay) = {magenta_px}, black (solid overlay) = {black_px}, white (target) = {white_px}", f.w(), f.h());
    let ok = magenta_px == 0 && black_px == 0;
    println!("{}", if ok { "PASS: overlays are excluded from capture" } else { "FAIL: overlay pixels visible in capture" });
    drop(bmp);
    drop(solid);
    unsafe {
        let _ = DestroyWindow(target);
    }
    std::process::exit(if ok { 0 } else { 1 });
}
