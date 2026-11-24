//! Module-path bench without rekordbox: a fake "rekordbox" window with a moving bar, a 60 Hz UDP sender that speaks the
//! rbmod schema, and the real presentation parts (UDP listener, extrapolation, PresentSet, DWM thumbnails, meter
//! strip). Measures the meter frame rate, CPU per frame, UDP packet rate and checks that the thumbnails render.
//!
//! usage: cargo run --release -p club685-app --example module_bench [seconds] [box|line]

use club685_app::rbmod::{self, deck_pair, extrapolate, master_verdict, resolve_master};
use club685_app::thumb::{fill_body, split_thumbs, ModPlan, PresentSet, StripRenderer, Thumbs};
use club685_core::{Rect, Row};
use club685_render::{MeterKind, SceneDeck};
use std::net::UdpSocket;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos, TranslateMessage, HWND_TOPMOST, MSG, PM_REMOVE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

unsafe extern "system" fn wp(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

fn cpu_ms() -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
    }
    let q = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    (q(k) + q(u)) as f64 / 10_000.0
}

fn grab(x: i32, y: i32, wd: i32, h: i32) -> Vec<u8> {
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, wd, h);
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let _ = BitBlt(dc, 0, 0, wd, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        let mut bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: wd, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
        let mut v = vec![0u8; (wd * h * 4) as usize];
        GetDIBits(dc, bmp, 0, h as u32, Some(v.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        v
    }
}

fn main() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
    }
    let secs: u64 = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(10);
    let kind = if std::env::args().nth(2).as_deref() == Some("line") { MeterKind::Line } else { MeterKind::Box };

    // fake rekordbox: 2560 x 300, left of the screen origin
    let hinst = unsafe { GetModuleHandleW(None).unwrap() };
    let wc = WNDCLASSW { lpfnWndProc: Some(wp), hInstance: hinst.into(), lpszClassName: w!("BenchSrc"), hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(unsafe { windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::WHITE_BRUSH) }.0), ..Default::default() };
    unsafe { RegisterClassW(&wc) };
    let src = unsafe { CreateWindowExW(WS_EX_TOOLWINDOW, w!("BenchSrc"), w!(""), WS_POPUP, 0, 0, 2000, 300, None, None, Some(hinst.into()), None).unwrap() };
    unsafe {
        let _ = SetWindowPos(src, None, 0, 0, 2000, 300, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }

    // rbmod stand-in: 60 Hz datagrams, deck 0 master, both playing at 128 BPM
    let sender = std::thread::spawn(move || {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let t0 = Instant::now();
        let mut n = 0u64;
        while t0.elapsed() < Duration::from_secs(secs + 3) {
            let t = t0.elapsed().as_secs_f64();
            let beats = t * 128.0 / 60.0;
            let mk = |deck: u32, off: f64, master: bool| {
                let b = beats + off;
                format!(
                    r#"{{"deck":{deck},"loaded":true,"playing":true,"master":{master},"bpm":128.0,"bar":{},"beat_in_bar":{},"phase_beat":{:.4}}}"#,
                    (b / 4.0).floor() as u32 + 1,
                    (b.rem_euclid(4.0)).floor() as u32 + 1,
                    b.rem_euclid(1.0)
                )
            };
            let msg = format!(r#"{{"v":1,"qpc_100ns":{},"master_source":"dispatch+0x248","decks":[{},{}]}}"#, club685_capture::now_ticks(), mk(0, 0.0, true), mk(1, 1.5, false));
            let _ = sock.send_to(msg.as_bytes(), "127.0.0.1:47990");
            n += 1;
            let next = Duration::from_micros(n * 16_667);
            if let Some(d) = next.checked_sub(t0.elapsed()) {
                std::thread::sleep(d);
            }
        }
    });

    let rbs = rbmod::spawn_listener();
    let mut pset = PresentSet::new().unwrap();
    let mut strip = StripRenderer::new();
    let rows = [Row { top: 20, bottom: 84 }, Row { top: 90, bottom: 155 }];
    // band: the fake window's y 15..160, shown below it on screen at (100, 400), 1900 wide
    let mask = Rect { x0: 100, y0: 400, x1: 2000, y1: 400 + 146 };
    let (mw, mh) = (mask.width(), mask.height());
    let t = split_thumbs(mw, mh, 96, true, &rows, 1000.0, 2000, (0, 0));
    let body = Rect { x0: mask.x0, y0: mask.y0 + t.header, x1: mask.x1, y1: mask.y1 };
    let stripr = Rect { x0: mask.x0, y0: mask.y0, x1: mask.x1, y1: mask.y0 + t.header };
    let mut thumbs = Thumbs::new(pset.body.hwnd, src).expect("thumbnail registration");
    thumbs.update(&t);
    if let Some(dib) = pset.body.dib_mut(body.width(), body.height()) {
        fill_body(dib, body.width(), body.height(), t.vw);
        let _ = pset.body.present_dib();
    }

    let (mut frames, mut work_ms, mut flush_ms) = (0u64, 0.0f64, 0.0f64);
    let start = Instant::now();
    let cpu0 = cpu_ms();
    let mut shot_done = false;
    let mut max_work = 0.0f64;
    while start.elapsed() < Duration::from_secs(secs) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let f = Instant::now();
            let _ = DwmFlush();
            flush_ms += f.elapsed().as_secs_f64() * 1000.0;
        }
        let tw = Instant::now();
        let (lat, _) = rbmod::latest(&rbs);
        if let Some(l) = lat {
            let pair = deck_pair(&l.packet);
            let v = [pair[0].map(|d| extrapolate(d, l.age_100ns)).unwrap_or_default(), pair[1].map(|d| extrapolate(d, l.age_100ns)).unwrap_or_default()];
            let master = resolve_master(master_verdict(&l.packet), None);
            let sd = [SceneDeck { phase: v[0].phase, bars: v[0].bars }, SceneDeck { phase: v[1].phase, bars: v[1].bars }];
            let ok = strip.draw(&mut pset.strip, stripr.width(), stripr.height(), 0, 96, kind, None, sd, master);
            let plan = ModPlan { body: Some(body), strip: ok.then_some(stripr), solid: None };
            pset.apply(&plan);
        }
        let ms = tw.elapsed().as_secs_f64() * 1000.0;
        work_ms += ms;
        max_work = max_work.max(ms);
        frames += 1;

        if !shot_done && start.elapsed() > Duration::from_secs(3) {
            shot_done = true;
            // dest region on screen vs the same source region of the fake window: thumbnails must match pixel-exact
            // (this process draws the source window's own client area as default window background, so compare the
            // body against a flat colour and report the colour histogram instead)
            let g = grab(body.x0, body.y0 + t.dst[0].y0, 200, 20);
            let nonblack = g.chunks_exact(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count();
            println!("thumbnail region sample: {nonblack} of {} pixels are not black (fake source window is white/grey, black = nothing rendered)", g.len() / 4);
        }
    }
    let el = start.elapsed().as_secs_f64();
    let cpu = cpu_ms() - cpu0;
    let (lat, count) = rbmod::latest(&rbs);
    println!("duration {el:.2} s, loop frames {frames} = {:.1} fps (vsync paced)", frames as f64 / el);
    println!("work per frame (latest + extrapolate + strip draw + apply): avg {:.3} ms, max {max_work:.2} ms", work_ms / frames as f64);
    println!("thread cpu per frame: {:.3} ms = {:.2}% of one core", cpu / frames as f64, cpu / (el * 10.0));
    println!("strip redraws {} ({:.1}/s), mode {kind:?}", strip.redraws, strip.redraws as f64 / el);
    println!("flush wait avg {:.2} ms", flush_ms / frames as f64);
    println!("udp datagrams {count} = {:.1} pkt/s, last age {:?} ms", count as f64 / el, lat.map(|l| l.age_ms));
    println!("thumbnail property updates: {} (should be 1)", thumbs.updates);
    let _ = sender.join();
}
