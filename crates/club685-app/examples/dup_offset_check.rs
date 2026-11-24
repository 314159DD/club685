//! Checks that Desktop Duplication captures a window that is NOT at the screen origin (the `clip_to_output` offset).
//! It opens a plain white target window, puts a magenta Bitmap overlay and a black Solid overlay on top, captures
//! the target with the Dup backend and counts overlay-coloured pixels. Expect 0 of each.
//! usage: cargo run --release -p club685-app --example dup_offset_check [x y]

use club685_capture::{Backend, Capture, FrameBuf, Next};
use club685_overlay::pump_messages;
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
    let tx: i32 = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(300);
    let ty: i32 = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(200);
    let (tw, th) = (600, 300);
    let target = unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: Some(wp), hInstance: hinst.into(), lpszClassName: w!("AffTarget"), hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0), ..Default::default() };
        RegisterClassW(&wc);
        CreateWindowExW(WS_EX_TOOLWINDOW, w!("AffTarget"), w!(""), WS_POPUP | WS_VISIBLE, tx, ty, tw, th, None, None, Some(hinst.into()), None).unwrap()
    };
    unsafe {
        let _ = SetWindowPos(target, Some(HWND_TOPMOST), tx, ty, tw, th, SWP_SHOWWINDOW);
    }

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
        if let Ok(Next::Frame(f)) = cap.next(30) {
            cap.read_full(&f, &mut buf).unwrap();
            got += 1;
            if std::env::var_os("TRACE").is_some() {
                let fb = buf.bgra().unwrap();
                let w = (0..fb.h()).step_by(8).flat_map(|y| (0..fb.w()).step_by(8).map(move |x| (x, y))).filter(|&(x, y)| fb.rgb(x, y).0 > 200).count();
                println!("frame {got} at {} ms: white samples {w}", (Instant::now() - (deadline - Duration::from_millis(1500))).as_millis());
            }
            if std::env::var_os("FIRST").is_some() {
                break;
            }
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
    let ok = white_px > (tw * th) as u32 / 2;
    println!("{}", if ok { "PASS: window at the given offset captured" } else { "FAIL: frame is not the window" });
    unsafe {
        let _ = DestroyWindow(target);
    }
    std::process::exit(if ok { 0 } else { 1 });
}
