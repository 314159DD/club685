//! The presentation worker. The rbmod module in rekordbox sends phase, bar and master over UDP; the split view is two
//! DWM thumbnails of the rekordbox window (zero copy, no per-frame capture, so Windows draws no capture border); the
//! meters are a small strip window redrawn once per display frame. The band and row rects come from a one-shot DXGI
//! duplication frame taken while all our overlays are hidden.
//!
//! The worker also watches the rekordbox folder for the module (`install`), so the window can offer the one-time
//! setup. The overlay windows live on this thread, so it pumps their messages.

use crate::log;
use crate::install;
use crate::prefs::{PhaseSetting, Prefs, WaveMode};
use crate::rbmod::{self, deck_pair, extrapolate, master_verdict, resolve_master, select_source, DeckView, Latest, RbRef, Source};
use crate::scene::cursor_of;
use crate::state::{DeckSnap, Shared, Snapshot};
use crate::status::{status, Attach, StatusInputs};
use crate::thumb::{fill_body, frame_offset, split_thumbs, window_rect, ModPlan, PresentSet, StripRenderer, Thumbs};
use club685_capture::{find_rekordbox, Backend, Capture, FindError, FrameBuf, Next, RbWindow};
use club685_core::{analyze, Layout, Rect, TrackerState};
use club685_overlay::{check_dpi, evaluate, pump_messages, query_inputs, query_metrics, to_screen, waveform_mask_rect, DpiError, GateState, LayoutStamp, WindowMetrics};
use club685_render::{MeterKind, SceneDeck};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FILETIME, HWND};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

pub type SharedRef = Arc<Mutex<Shared>>;

pub fn spawn(shared: SharedRef) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new().name("club685-worker".into()).spawn(move || run(shared)).expect("worker thread")
}

struct Attached {
    rb: RbWindow,
    dpi: u32,
}

fn try_attach() -> Result<Attached, Attach> {
    let rb = find_rekordbox().map_err(|e| match e {
        FindError::NotRunning => Attach::NotRunning,
        FindError::NoMainWindow { .. } => Attach::NoMainWindow,
        FindError::WrongVersion { .. } => Attach::WrongVersion,
    })?;
    let dpi = check_dpi(rb.hwnd).map_err(|e| match e {
        DpiError::Mismatch { .. } => Attach::DpiMismatch,
        DpiError::Unknown => Attach::DpiUnknown,
    })?;
    Ok(Attached { rb, dpi })
}

#[derive(Default)]
struct Stats {
    frames: u64,
}

fn meter_kind(p: PhaseSetting) -> Option<MeterKind> {
    match p {
        PhaseSetting::Off => None,
        PhaseSetting::Box => Some(MeterKind::Box),
        PhaseSetting::Line => Some(MeterKind::Line),
    }
}

/// The layout from one DXGI frame taken while our overlays were hidden.
#[derive(Clone, Copy)]
struct ModLayout {
    layout: Layout,
    cursor: Option<f32>,
    pixel_master: Option<usize>,
    metrics: WindowMetrics,
}

/// Everything one snapshot publish needs.
struct Pub {
    attach: Attach,
    capture_path: String,
    layout_txt: Option<String>,
    layout_known: bool,
    gate: Option<GateState>,
    decks: [DeckSnap; 2],
    master: Option<usize>,
    master_ok: bool,
    cursor: Option<f32>,
    bpm_int: [Option<u32>; 2],
    overlays: String,
}

/// True when (nearly) no pixel of the frame is lit: a blank first duplication image.
fn frame_is_blank(f: &club685_core::Bgra<'_>) -> bool {
    let mut lit = 0;
    for y in (0..f.h()).step_by(13) {
        for x in (0..f.w()).step_by(17) {
            let (r, g, b) = f.rgb(x, y);
            if r > 6 || g > 6 || b > 6 {
                lit += 1;
            }
        }
    }
    lit < 20
}

/// Copies the screen area `r` (composed desktop, GDI) into `buf`.
fn gdi_grab(r: Rect, buf: &mut FrameBuf) -> bool {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };
    let (w, h) = (r.width(), r.height());
    if w <= 0 || h <= 0 {
        return false;
    }
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let blt = BitBlt(dc, 0, 0, w, h, Some(screen), r.x0, r.y0, SRCCOPY | CAPTUREBLT).is_ok();
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut v = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(dc, bmp, 0, h as u32, Some(v.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        if !blt || lines == 0 {
            return false;
        }
        buf.ensure(w as usize, h as usize);
        buf.blit(Rect { x0: 0, y0: 0, x1: w, y1: h }, &v, w as usize * 4, (0, 0));
    }
    true
}

fn thread_cpu_100ns() -> u64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        if GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return 0;
        }
    }
    let q = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    q(k) + q(u)
}

struct W {
    shared: SharedRef,
    start: Instant,
    rb: RbRef,
    pset: Option<PresentSet>,
    buf: FrameBuf,
    att: Option<Attached>,
    attached_at: Instant,
    attach_state: Attach,
    last_find: Option<Instant>,
    last_dpi_check: Instant,
    stats: Stats,
    rb_version: String,
    window_txt: String,
    gate_state: Option<GateState>,
    last_publish: Instant,
    rev: u64,
    source: Source,
    capture_path: String,
    // the rekordbox module in its folder
    expected_hash: Option<String>,
    module_state: install::ModuleState,
    module_dir: Option<std::path::PathBuf>,
    last_module_check: Option<Instant>,
    // layout and presentation
    mlayout: Option<ModLayout>,
    last_grab: Instant,
    last_metrics: Option<WindowMetrics>,
    metrics_changed: Instant,
    thumbs: Option<Thumbs>,
    body_fill: Option<(i32, i32, i32)>,
    strip: StripRenderer,
    fast_flushes: u32,
    // statistics
    udp_count_prev: u64,
    udp_count: u64,
    udp_pps: f32,
    mod_frames: u32,
    mod_work_ms: f64,
    meter_fps: f32,
    meter_ms: f32,
    cpu_prev: u64,
    stat_t: Instant,
}

fn run(shared: SharedRef) {
    let mut w = W {
        shared,
        start: Instant::now(),
        rb: rbmod::spawn_listener(),
        pset: PresentSet::new().ok(),
        buf: FrameBuf::new(),
        att: None,
        attached_at: Instant::now(),
        attach_state: Attach::NotRunning,
        last_find: None,
        last_dpi_check: Instant::now(),
        stats: Stats::default(),
        rb_version: "-".to_string(),
        window_txt: "-".to_string(),
        gate_state: None,
        last_publish: Instant::now() - Duration::from_secs(1),
        rev: 0,
        source: Source::Pending,
        capture_path: "none".to_string(),
        expected_hash: install::expected_hash(),
        module_state: install::ModuleState::Unknown,
        module_dir: None,
        last_module_check: None,
        mlayout: None,
        last_grab: Instant::now() - Duration::from_secs(10),
        last_metrics: None,
        metrics_changed: Instant::now(),
        thumbs: None,
        body_fill: None,
        strip: StripRenderer::new(),
        fast_flushes: 0,
        udp_count_prev: 0,
        udp_count: 0,
        udp_pps: 0.0,
        mod_frames: 0,
        mod_work_ms: 0.0,
        meter_fps: 0.0,
        meter_ms: 0.0,
        cpu_prev: thread_cpu_100ns(),
        stat_t: Instant::now(),
    };
    log::line("worker started");
    w.main_loop();
    if let Some(p) = w.pset.as_mut() {
        p.hide_all();
    }
}

impl W {
    fn hide_module(&mut self) {
        if let Some(p) = self.pset.as_mut() {
            p.hide_all();
        }
    }

    fn hide_everything(&mut self) {
        self.hide_module();
    }

    /// Once per 1.5 s: is the bundled module in the rekordbox folder?
    fn refresh_module(&mut self) {
        if self.last_module_check.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) {
            return;
        }
        self.last_module_check = Some(Instant::now());
        let running = self.att.as_ref().map(|a| a.rb.exe_path.clone());
        self.module_dir = install::find_rekordbox_dir(running.as_deref());
        self.module_state = install::module_state(self.module_dir.as_deref(), self.expected_hash.as_deref());
    }

    fn reset_module(&mut self) {
        self.mlayout = None;
        self.thumbs = None;
        self.body_fill = None;
        self.strip.forget();
        self.last_grab = Instant::now() - Duration::from_secs(10);
    }

    fn main_loop(&mut self) {
        loop {
            pump_messages();
            let (prefs, quit) = {
                let g = self.shared.lock().unwrap();
                (g.prefs.clone(), g.quit)
            };
            if quit {
                break;
            }
            let now_ms = self.start.elapsed().as_millis() as u64;
            self.refresh_module();

            if !prefs.power {
                if self.att.is_some() {
                    self.att = None;
                    self.reset_module();
                    self.source = Source::Pending;
                    self.capture_path = "none".to_string();
                }
                self.hide_everything();
                let p = self.blank_pub(Attach::NotRunning);
                self.publish(&prefs, &p);
                std::thread::sleep(Duration::from_millis(60));
                continue;
            }

            if self.att.is_none() {
                if self.last_find.map_or(true, |t| t.elapsed() >= Duration::from_secs(1)) {
                    self.last_find = Some(Instant::now());
                    match try_attach() {
                        Ok(a) => {
                            self.rb_version = format!("{}.{}.{}.{} pid {}", a.rb.version.0, a.rb.version.1, a.rb.version.2, a.rb.version.3, a.rb.pid);
                            self.attach_state = Attach::Attached;
                            self.attached_at = Instant::now();
                            self.source = Source::Pending;
                            self.reset_module();
                            self.last_module_check = None;
                            log::line(&format!("attached to rekordbox {}", self.rb_version));
                            self.att = Some(a);
                        }
                        Err(e) => self.attach_state = e,
                    }
                }
                if self.att.is_none() {
                    self.hide_everything();
                    let p = self.blank_pub(self.attach_state.clone());
                    self.publish(&prefs, &p);
                    std::thread::sleep(Duration::from_millis(60));
                    continue;
                }
            }

            // rekordbox still there, still at the same scale?
            let mut lost: Option<Attach> = None;
            {
                let a = self.att.as_mut().unwrap();
                if !unsafe { IsWindow(Some(a.rb.hwnd)) }.as_bool() {
                    lost = Some(Attach::NotRunning);
                } else if self.last_dpi_check.elapsed() >= Duration::from_secs(1) {
                    self.last_dpi_check = Instant::now();
                    match check_dpi(a.rb.hwnd) {
                        Ok(d) if d == a.dpi => {}
                        Ok(d) => {
                            a.dpi = d;
                            self.mlayout = None;
                        }
                        Err(DpiError::Mismatch { .. }) => lost = Some(Attach::DpiMismatch),
                        Err(DpiError::Unknown) => lost = Some(Attach::DpiUnknown),
                    }
                }
            }
            if let Some(l) = lost {
                self.attach_state = l;
                self.att = None;
                self.reset_module();
                self.last_find = Some(Instant::now());
                self.hide_everything();
                continue;
            }

            // is the module talking?
            let (latest, count) = rbmod::latest(&self.rb);
            let next = select_source(self.source, latest.as_ref().map(|l| l.age_ms), self.attached_at.elapsed().as_millis() as u64);
            if next != self.source {
                log::line(&format!("module link {:?} -> {:?} (last datagram age {:?} ms)", self.source, next, latest.as_ref().map(|l| l.age_ms)));
                self.source = next;
                if next != Source::Module {
                    self.hide_module();
                    self.reset_module();
                }
            }
            self.update_udp_rate(count);

            match (self.source, latest) {
                (Source::Module, Some(l)) => {
                    self.pace();
                    let t = Instant::now();
                    self.module_tick(&prefs, &l, now_ms);
                    self.mod_frames += 1;
                    self.mod_work_ms += t.elapsed().as_secs_f64() * 1000.0;
                    self.roll_stats();
                }
                _ => {
                    self.hide_everything();
                    let p = self.blank_pub(Attach::Attached);
                    self.publish(&prefs, &p);
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
        }
    }

    fn update_udp_rate(&mut self, count: u64) {
        self.udp_count = count;
    }

    /// Once per second: meter frame rate, CPU per frame, and one log line.
    fn roll_stats(&mut self) {
        let dt = self.stat_t.elapsed();
        if dt < Duration::from_secs(1) {
            return;
        }
        let secs = dt.as_secs_f32();
        let cpu = thread_cpu_100ns();
        let cpu_ms = (cpu.saturating_sub(self.cpu_prev)) as f64 / 10_000.0;
        self.cpu_prev = cpu;
        self.udp_pps = self.udp_count.saturating_sub(self.udp_count_prev) as f32 / secs;
        self.udp_count_prev = self.udp_count;
        self.meter_fps = self.mod_frames as f32 / secs;
        self.meter_ms = if self.mod_frames > 0 { (self.mod_work_ms / self.mod_frames as f64) as f32 } else { 0.0 };
        if log::enabled() {
            let (body, strip) = self.pset.as_ref().map_or((false, false), |p| (p.current().body.is_some(), p.current().strip.is_some()));
            log::line(&format!(
                "module frames {:.1}/s work {:.3} ms/frame thread cpu {:.3} ms/frame ({:.1}% of one core) strip redraws {}/s udp {:.1} pkt/s thumb updates {} body {} strip {} capture {}",
                self.meter_fps,
                self.meter_ms,
                if self.mod_frames > 0 { cpu_ms / self.mod_frames as f64 } else { 0.0 },
                cpu_ms / (secs as f64 * 10.0),
                self.strip.redraws,
                self.udp_pps,
                self.thumbs.as_ref().map_or(0, |t| t.updates),
                body,
                strip,
                self.capture_path
            ));
        }
        self.mod_frames = 0;
        self.mod_work_ms = 0.0;
        self.strip.redraws = 0;
        self.stat_t = Instant::now();
    }

    /// Waits for the next display frame (DWM composition tick); falls back to a short sleep if DwmFlush returns at once.
    fn pace(&mut self) {
        let t = Instant::now();
        unsafe {
            let _ = DwmFlush();
        }
        if t.elapsed() < Duration::from_millis(1) {
            self.fast_flushes += 1;
        } else {
            self.fast_flushes = 0;
        }
        if self.fast_flushes >= 8 {
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    fn blank_pub(&self, attach: Attach) -> Pub {
        Pub {
            attach,
            capture_path: self.capture_path.clone(),
            layout_txt: None,
            layout_known: false,
            gate: None,
            decks: [DeckSnap::default(); 2],
            master: None,
            master_ok: false,
            cursor: None,
            bpm_int: [None; 2],
            overlays: "none".to_string(),
        }
    }

    // ---- presentation -------------------------------------------------------------------------------------------

    /// One hide, wait, grab, analyse cycle: the layout from a single border-free DXGI frame.
    fn grab_layout(&mut self, hwnd: HWND, m: WindowMetrics) -> Option<ModLayout> {
        self.hide_everything();
        unsafe {
            let _ = DwmFlush();
            let _ = DwmFlush();
        }
        std::thread::sleep(Duration::from_millis(15));
        pump_messages();
        self.capture_path = "dxgi one-shot (no border)".to_string();
        log::line("layout grab: dxgi duplication, overlays hidden");
        let t = Instant::now();
        let mut result = None;
        let mut via = "dxgi";
        let mut gotten = false;
        if let Ok(mut cap) = Capture::start(Backend::Dup, hwnd) {
            // the first image of a new duplication is sometimes blank: wait for one that is not
            let deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < deadline && !gotten {
                match cap.next(60) {
                    Ok(Next::Frame(f)) => {
                        if f.width as i32 == m.window.width() && f.height as i32 == m.window.height() && cap.read_full(&f, &mut self.buf).is_ok() {
                            drop(f);
                            gotten = self.buf.bgra().is_some_and(|fr| !frame_is_blank(&fr));
                        }
                    }
                    Ok(Next::Blocked(_)) | Err(_) => break,
                    _ => {}
                }
            }
        }
        if !gotten {
            // static desktop, so duplication delivered nothing usable: a plain GDI grab of the composed screen is
            // just as border-free
            via = "gdi";
            gotten = gdi_grab(m.window, &mut self.buf) && self.buf.bgra().is_some_and(|fr| !frame_is_blank(&fr));
        }
        if gotten {
            if let Some(frame) = self.buf.bgra() {
                let reading = analyze(&frame, &mut TrackerState::new());
                if let Some(layout) = reading.layout {
                    result = Some(ModLayout { layout, cursor: cursor_of(&reading), pixel_master: reading.master_raw, metrics: m });
                }
            }
        }
        self.capture_path = "none (module)".to_string();
        log::line(&format!(
            "layout grab ({via}) done in {} ms: {}",
            t.elapsed().as_millis(),
            result.as_ref().map_or("no layout".to_string(), |r| format!("band y{}..{} cursor {:?} pixel master {:?}", r.layout.band.y0, r.layout.band.y1, r.cursor, r.pixel_master))
        ));
        result.filter(|r| query_metrics(hwnd) == Some(r.metrics))
    }

    fn module_tick(&mut self, prefs: &Prefs, lat: &Latest, now_ms: u64) {
        let (hwnd, dpi) = {
            let a = self.att.as_ref().unwrap();
            (a.rb.hwnd, a.dpi)
        };
        let pair = deck_pair(&lat.packet);
        let views: [DeckView; 2] = [
            pair[0].map(|d| extrapolate(d, lat.age_100ns)).unwrap_or_default(),
            pair[1].map(|d| extrapolate(d, lat.age_100ns)).unwrap_or_default(),
        ];
        let inputs = query_inputs(hwnd);
        let want_overlay = prefs.waveform != WaveMode::Normal;
        let meter = meter_kind(prefs.phase);

        // layout: only from one-shot frames, only when unknown or the window changed
        let cur = inputs.metrics;
        if cur != self.last_metrics {
            self.last_metrics = cur;
            self.metrics_changed = Instant::now();
        }
        if self.mlayout.as_ref().is_some_and(|l| Some(l.metrics) != cur) {
            self.mlayout = None;
        }
        if !want_overlay && self.mlayout.is_some() {
            // nothing to draw: keep the last layout but never take another frame
        } else if want_overlay {
            let visible = self.pset.as_ref().is_some_and(|p| p.visible());
            let complete = self.mlayout.as_ref().is_some_and(|l| l.cursor.is_some());
            let due = match &self.mlayout {
                None => self.metrics_changed.elapsed() >= Duration::from_millis(250) && self.last_grab.elapsed() >= Duration::from_secs(1),
                Some(_) => !visible && self.last_grab.elapsed() >= Duration::from_secs(if complete { 5 } else { 1 }),
            };
            let can = inputs.foreground && inputs.enabled && inputs.visible && !inputs.iconic && inputs.gui_flags & 0x1E == 0 && cur.is_some();
            if due && can {
                let m = cur.unwrap();
                self.last_grab = Instant::now();
                let found = self.grab_layout(hwnd, m);
                self.stats.frames += 1;
                if found.is_some() || self.mlayout.is_none() {
                    self.mlayout = found;
                }
            }
        }

        // gate: the layout stamp is refreshed every frame while the window metrics still equal the measured ones
        let stamp = if want_overlay {
            self.mlayout.as_ref().map(|l| LayoutStamp { metrics: l.metrics, at_ms: now_ms })
        } else {
            cur.map(|m| LayoutStamp { metrics: m, at_ms: now_ms })
        };
        let gate = evaluate(&inputs, stamp.as_ref(), now_ms);
        self.gate_state = Some(gate);

        let master_verdict = master_verdict(&lat.packet);
        let master = resolve_master(master_verdict, self.mlayout.as_ref().and_then(|l| l.pixel_master));
        let sdecks = [SceneDeck { phase: views[0].phase, bars: views[0].bars }, SceneDeck { phase: views[1].phase, bars: views[1].bars }];

        // what goes on screen
        let mut plan = ModPlan::default();
        if let (true, true, Some(ml), Some(m)) = (want_overlay, gate.is_open(), self.mlayout, cur) {
            let l = ml.layout;
            let fm = waveform_mask_rect(l.band, l.width, m.dpi);
            let mask = to_screen((m.window.x0, m.window.y0), fm);
            let (mw, mh) = (mask.width(), mask.height());
            match prefs.waveform {
                WaveMode::Split => {
                    if let (Some(cursor), Some(win)) = (ml.cursor, window_rect(hwnd)) {
                        let off = frame_offset(m.window, win);
                        let t = split_thumbs(mw, mh, m.dpi, meter.is_some(), &l.rows, cursor, l.width, off);
                        let body = Rect { x0: mask.x0, y0: mask.y0 + t.header, x1: mask.x1, y1: mask.y1 };
                        if let Some(p) = self.pset.as_mut() {
                            let (bw, bh) = (body.width(), body.height());
                            if self.body_fill != Some((bw, bh, t.vw)) {
                                if let Some(dib) = p.body.dib_mut(bw, bh) {
                                    fill_body(dib, bw, bh, t.vw);
                                    let _ = p.body.present_dib();
                                    self.body_fill = Some((bw, bh, t.vw));
                                }
                            }
                            if self.thumbs.as_ref().is_some_and(|th| th.source() != hwnd) {
                                self.thumbs = None;
                            }
                            if self.thumbs.is_none() {
                                self.thumbs = Thumbs::new(p.body.hwnd, hwnd);
                                if self.thumbs.is_none() {
                                    log::line("DwmRegisterThumbnail failed");
                                }
                            }
                            if let Some(th) = self.thumbs.as_mut() {
                                th.update(&t);
                                plan.body = Some(body);
                            }
                        }
                        if let (Some(kind), true) = (meter, t.header > 0) {
                            let strip = Rect { x0: mask.x0, y0: mask.y0, x1: mask.x1, y1: mask.y0 + t.header };
                            if let Some(p) = self.pset.as_mut() {
                                if self.strip.draw(&mut p.strip, strip.width(), strip.height(), 0, m.dpi, kind, None, sdecks, master) {
                                    plan.strip = Some(strip);
                                }
                            }
                        }
                    }
                }
                WaveMode::HideBoth => match meter {
                    None => plan.solid = Some(mask),
                    Some(kind) => {
                        let local_cursor = ml.cursor.map(|c| c - fm.x0 as f32);
                        if let Some(p) = self.pset.as_mut() {
                            if self.strip.draw(&mut p.strip, mw, mh, 1, m.dpi, kind, local_cursor, sdecks, master) {
                                plan.strip = Some(mask);
                            }
                        }
                    }
                },
                WaveMode::Normal => {}
            }
        }
        if let Some(p) = self.pset.as_mut() {
            p.apply(&plan);
        }

        let overlays = format!("body{}+strip{}+solid{}", plan.body.is_some() as u8, plan.strip.is_some() as u8, plan.solid.is_some() as u8);
        let layout_known = !want_overlay || self.mlayout.is_some() || !inputs.foreground;
        let ml = self.mlayout.as_ref();
        let p = Pub {
            attach: Attach::Attached,
            capture_path: self.capture_path.clone(),
            layout_txt: ml.map(|ml| {
                let l = &ml.layout;
                format!("band y{}..{} rows [{}..{}] [{}..{}]", l.band.y0, l.band.y1, l.rows[0].top, l.rows[0].bottom, l.rows[1].top, l.rows[1].bottom)
            }),
            layout_known,
            gate: Some(gate),
            decks: [DeckSnap { phase: views[0].phase, bars: views[0].bars }, DeckSnap { phase: views[1].phase, bars: views[1].bars }],
            master,
            master_ok: true,
            cursor: ml.and_then(|l| l.cursor),
            bpm_int: [views[0].bpm.map(|b| b.round() as u32), views[1].bpm.map(|b| b.round() as u32)],
            overlays,
        };
        self.window_txt = cur.map_or("-".to_string(), |m| format!("{}x{}", m.window.width(), m.window.height()));
        self.publish_with_dpi(prefs, &p, dpi);
    }

    // ---- snapshot -----------------------------------------------------------------------------------------------

    fn publish(&mut self, prefs: &Prefs, p: &Pub) {
        self.publish_with_dpi(prefs, p, 0);
    }

    fn publish_with_dpi(&mut self, prefs: &Prefs, p: &Pub, dpi: u32) {
        if self.last_publish.elapsed() < Duration::from_millis(40) {
            return;
        }
        self.last_publish = Instant::now();
        let inputs = StatusInputs {
            attach: p.attach.clone(),
            layout: p.layout_known,
            gate: p.gate,
            cursor: p.cursor.is_some() || !p.layout_known,
            master: p.master_ok,
            phase: [p.decks[0].phase.is_some(), p.decks[1].phase.is_some()],
            source: self.source,
            module: self.module_state,
        };
        let (kind, line) = status(prefs, &inputs);
        self.rev += 1;
        let mut g = self.shared.lock().unwrap();
        let s: &mut Snapshot = &mut g.snap;
        s.kind = kind;
        s.line = line;
        s.rev = self.rev;
        s.decks = p.decks;
        s.master = p.master;
        s.decks_found = if (p.layout_known && p.layout_txt.is_some()) || (self.source == Source::Module && p.attach == Attach::Attached) { 2 } else { 0 };
        s.cursor = p.cursor;
        s.beat_px = None;
        s.bpm_int = p.bpm_int;
        s.source = self.source;
        s.module = self.module_state;
        s.module_dir = self.module_dir.clone();
        s.capture_path = p.capture_path.clone();
        s.udp_pps = self.udp_pps;
        s.meter_fps = self.meter_fps;
        s.meter_ms = self.meter_ms;
        s.gate = p.gate.map_or("-".to_string(), |g| g.label().to_string());
        s.rb_version = self.rb_version.clone();
        s.dpi = dpi;
        s.window = self.window_txt.clone();
        s.layout = p.layout_txt.clone().unwrap_or_else(|| "-".to_string());
        s.overlays = p.overlays.clone();
        s.frames = self.stats.frames;
    }
}
