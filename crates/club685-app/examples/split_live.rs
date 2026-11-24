//! Live check of the split view against a running rekordbox, without starting club685.exe: one GDI grab for the layout,
//! then the real PresentSet + DWM thumbnails + meter strip for 3 s, then a screenshot of the top of the screen.
//! Overlays are NOT excluded from capture here (CLUB685_NOEXCLUDE) so the screenshot can show them.
//!
//! usage: cargo run --release -p club685-app --example split_live <out.png> [box|line|off]

use club685_app::rbmod::{self, deck_pair, extrapolate, master_verdict, resolve_master};
use club685_app::thumb::{fill_body, frame_offset, split_thumbs, window_rect, ModPlan, PresentSet, StripRenderer, Thumbs};
use club685_capture::{find_rekordbox, FrameBuf};
use club685_core::{analyze, Rect, TrackerState};
use club685_overlay::{pump_messages, query_metrics, to_screen, waveform_mask_rect};
use club685_render::{MeterKind, SceneDeck};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::SwitchToThisWindow;

fn grab(x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
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
        v
    }
}

fn main() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
    }
    std::env::set_var("CLUB685_NOEXCLUDE", "1");
    let out = std::env::args().nth(1).expect("out.png");
    let kind = match std::env::args().nth(2).as_deref() {
        Some("line") => Some(MeterKind::Line),
        Some("off") => None,
        _ => Some(MeterKind::Box),
    };
    let rb = find_rekordbox().expect("rekordbox not found");
    unsafe { SwitchToThisWindow(rb.hwnd, true) };
    std::thread::sleep(Duration::from_millis(800));
    let m = query_metrics(rb.hwnd).expect("metrics");
    let (w, h) = (m.window.width(), m.window.height());
    let px = grab(m.window.x0, m.window.y0, w, h);
    let mut buf = FrameBuf::new();
    buf.ensure(w as usize, h as usize);
    buf.blit(Rect { x0: 0, y0: 0, x1: w, y1: h }, &px, w as usize * 4, (0, 0));
    let reading = analyze(&buf.bgra().unwrap(), &mut TrackerState::new());
    let l = reading.layout.expect("no layout found (is the 2Deck Horizontal waveform visible?)");
    let cursor = reading.cursor_raw.or(reading.cursor).expect("no playhead");
    println!("layout band y{}..{} rows {:?} cursor {cursor} dpi {} window {:?}", l.band.y0, l.band.y1, l.rows, m.dpi, m.window);

    let rbs = rbmod::spawn_listener();
    let mut pset = PresentSet::new().unwrap();
    let mut strip = StripRenderer::new();
    let fm = waveform_mask_rect(l.band, l.width, m.dpi);
    let mask = to_screen((m.window.x0, m.window.y0), fm);
    let win = window_rect(rb.hwnd).unwrap();
    let off = frame_offset(m.window, win);
    let t = split_thumbs(mask.width(), mask.height(), m.dpi, kind.is_some(), &l.rows, cursor, l.width, off);
    let body = Rect { x0: mask.x0, y0: mask.y0 + t.header, x1: mask.x1, y1: mask.y1 };
    let stripr = Rect { x0: mask.x0, y0: mask.y0, x1: mask.x1, y1: mask.y0 + t.header };
    println!("window rect {:?} offset {:?} thumbs src {:?} dst {:?} header {}", win, off, t.src, t.dst, t.header);
    let mut thumbs = Thumbs::new(pset.body.hwnd, rb.hwnd).expect("thumbnail registration");
    thumbs.update(&t);
    if let Some(dib) = pset.body.dib_mut(body.width(), body.height()) {
        fill_body(dib, body.width(), body.height(), t.vw);
        let _ = pset.body.present_dib();
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        pump_messages();
        unsafe {
            let _ = DwmFlush();
        }
        let (lat, _) = rbmod::latest(&rbs);
        let (mut decks, mut master) = ([SceneDeck::default(); 2], None);
        if let Some(lt) = lat {
            let pair = deck_pair(&lt.packet);
            for d in 0..2 {
                let v = pair[d].map(|x| extrapolate(x, lt.age_100ns)).unwrap_or_default();
                decks[d] = SceneDeck { phase: v.phase, bars: v.bars };
            }
            master = resolve_master(master_verdict(&lt.packet), reading.master_raw);
        }
        let mut plan = ModPlan { body: Some(body), strip: None, solid: None };
        if let Some(k) = kind {
            if strip.draw(&mut pset.strip, stripr.width(), stripr.height(), 0, m.dpi, k, None, decks, master) {
                plan.strip = Some(stripr);
            }
        }
        pset.apply(&plan);
    }
    let shot = grab(0, 0, 2560, 440);
    let f = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), 2560, 440);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut rgba = Vec::with_capacity(shot.len());
    for p in shot.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    let (lat, count) = rbmod::latest(&rbs);
    println!("udp {count} datagrams, last age {:?} ms; thumb updates {}; strip redraws {}", lat.map(|l| l.age_ms), thumbs.updates, strip.redraws);
}
