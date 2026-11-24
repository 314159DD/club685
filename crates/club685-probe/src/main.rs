//! club685-probe: temporary harness. Captures rekordbox 6.8.5, runs `analyze` per frame and drives the overlay
//! windows for a few seconds, printing one status line per second.
//!
//! usage: club685-probe [--bpm off|hide|one-decimal|no-decimals] [--waveform normal|hide] [--seconds N] [--capture wgc|dup]
//!                      [--dry [--fixtures DIR]]
#![cfg(windows)]

mod dry;
mod pipeline;

use club685_capture::{find_rekordbox, Backend, Capture, FrameBuf, Next, Verdict};
use club685_overlay::{check_dpi, evaluate, pump_messages, query_inputs, query_metrics, plan_overlays, GateState, LayoutStamp, OverlaySet, Plan};
use pipeline::{percentile, reading_str, BpmOpt, Pipeline};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    BOOL(1)
}

struct Args {
    bpm: BpmOpt,
    hide_waveform: bool,
    seconds: u64,
    dry: bool,
    fixtures: Option<PathBuf>,
    dump: Option<PathBuf>,
    ignore_foreground: bool,
    master_csv: Option<PathBuf>,
    capture: Backend,
    allow_second: bool,
    dump_at_ms: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { bpm: BpmOpt::Off, hide_waveform: false, seconds: 10, dry: false, fixtures: None, dump: None, ignore_foreground: false, master_csv: None, capture: Backend::from_env(), allow_second: false, dump_at_ms: 0 };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--bpm" => {
                let v = it.next().ok_or("--bpm needs a value")?;
                a.bpm = BpmOpt::parse(&v).ok_or(format!("--bpm: unknown value '{v}' (off|hide|one-decimal|no-decimals)"))?;
            }
            "--waveform" => {
                let v = it.next().ok_or("--waveform needs a value")?;
                a.hide_waveform = match v.as_str() {
                    "normal" => false,
                    "hide" => true,
                    _ => return Err(format!("--waveform: unknown value '{v}' (normal|hide)")),
                };
            }
            "--seconds" => {
                let v = it.next().ok_or("--seconds needs a value")?;
                a.seconds = v.parse::<u64>().map_err(|_| format!("--seconds: '{v}' is not a number"))?.clamp(1, 60);
            }
            "--dry" => a.dry = true,
            "--fixtures" => a.fixtures = Some(PathBuf::from(it.next().ok_or("--fixtures needs a path")?)),
            "--dump" => a.dump = Some(PathBuf::from(it.next().ok_or("--dump needs a file")?)),
            "--ignore-foreground" => a.ignore_foreground = true,
            "--master-csv" => a.master_csv = Some(PathBuf::from(it.next().ok_or("--master-csv needs a file")?)),
            "--dump-at-ms" => {
                let v = it.next().ok_or("--dump-at-ms needs a value")?;
                a.dump_at_ms = v.parse::<u64>().map_err(|_| format!("--dump-at-ms: '{v}' is not a number"))?;
            }
            "--allow-second-instance" => a.allow_second = true,
            "--capture" => {
                let v = it.next().ok_or("--capture needs a value")?;
                a.capture = Backend::parse(&v).ok_or(format!("--capture: unknown value '{v}' (wgc|dup)"))?;
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(a)
}

fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len).is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("club685-probe: {e}");
            eprintln!("usage: club685-probe [--bpm off|hide|one-decimal|no-decimals] [--waveform normal|hide] [--seconds N] [--capture wgc|dup] [--dry [--fixtures DIR]]");
            return 2;
        }
    };
    if args.dry {
        let dir = args.fixtures.clone().unwrap_or_else(dry::default_fixture_dir);
        return dry::run(&dir, args.bpm);
    }
    if is_elevated() {
        eprintln!("club685-probe: refusing to run elevated");
        return 2;
    }
    // Single instance. The handle stays open until the process exits.
    let _mutex = unsafe {
        let m = CreateMutexW(None, true, w!("Local\\CLUB685"));
        match m {
            Ok(h) if args.allow_second || GetLastError() != ERROR_ALREADY_EXISTS => h,
            _ => {
                eprintln!("club685-probe: another CLUB685 instance is already running");
                return 2;
            }
        }
    };
    unsafe {
        // rekordbox is system DPI aware; matching it keeps our coordinates equal to its.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
    live(&args)
}

fn live(args: &Args) -> i32 {
    let rb = match find_rekordbox() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("club685-probe: {e}");
            return 2;
        }
    };
    println!(
        "rekordbox {}.{}.{}.{} pid {} class {} bounds {:?} exe {}",
        rb.version.0, rb.version.1, rb.version.2, rb.version.3, rb.pid, rb.class, rb.bounds, rb.exe_path
    );
    let mut dpi_ok = match check_dpi(rb.hwnd) {
        Ok(d) => {
            println!("dpi {d}");
            true
        }
        Err(e) => {
            eprintln!("club685-probe: {e}");
            return 2;
        }
    };
    println!("capture backend {}", args.capture.label());
    let mut session = match Capture::start(args.capture, rb.hwnd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("club685-probe: capture failed: {e}");
            return 3;
        }
    };
    let mut set = match OverlaySet::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("club685-probe: overlay windows failed: {e}");
            return 3;
        }
    };

    let start = Instant::now();
    let deadline = Duration::from_secs(args.seconds);
    let mut pipe = Pipeline::new(args.bpm);
    let mut buf = FrameBuf::new();
    let mut stamp: Option<LayoutStamp> = None;
    let mut last_masks = [None; 2];
    let mut last_text = String::from("playhead=- phase=[- -] master=- bars=[- -]");
    let mut gate = GateState::NoLayout;
    let mut frames_total = 0u64;
    let mut sec_frames = 0u32;
    let mut sec_ms: Vec<f64> = Vec::new();
    let mut all_ms: Vec<f64> = Vec::new();
    let mut next_report = Duration::from_secs(1);
    let mut last_report = Instant::now();
    let mut full_reads = 0u64;
    let mut read_ms_sum = 0.0f64;
    let mut reads = 0u64;
    let mut read_err = 0u64;
    let mut exit = 0;
    let mut dumped = false;
    // qpc_100ns,master_raw,master (per analysed frame; -1 = none). Same time base as rbmod's qpc_100ns.
    let mut csv = args.master_csv.as_ref().and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
    if let Some(w) = csv.as_mut() { use std::io::Write; let _ = writeln!(w, "qpc_100ns,master_raw,master_deck"); }

    while !STOP.load(Ordering::SeqCst) && start.elapsed() < deadline {
        pump_messages();
        let now_ms = start.elapsed().as_millis() as u64;
        if !unsafe { IsWindow(Some(rb.hwnd)) }.as_bool() {
            set.hide_all();
            eprintln!("club685-probe: rekordbox window is gone");
            exit = 4;
            break;
        }
        match session.next(30) {
            Ok(Next::Blocked(_)) => {}
            Ok(Next::Frame(f)) if session.sees_screen() && !args.ignore_foreground && !query_inputs(rb.hwnd).foreground => drop(f),
            Ok(Next::Frame(f)) => {
                let before = query_metrics(rb.hwnd);
                let size_ok = before.is_some_and(|m| m.window.width() == f.width as i32 && m.window.height() == f.height as i32);
                if size_ok {
                    let rect = pipe.next_read(f.width as i32, f.height as i32);
                    if rect.width() as u32 == f.width && rect.height() as u32 == f.height {
                        full_reads += 1;
                    }
                    let t = Instant::now();
                    match session.read_rect(&f, rect, &mut buf) {
                        Ok(_) => {
                            read_ms_sum += t.elapsed().as_secs_f64() * 1000.0;
                            reads += 1;
                            let frame_ticks = f.ticks;
                            drop(f);
                            if let (Some(p), false) = (&args.dump, dumped) {
                                let late = args.dump_at_ms > 0 && now_ms >= args.dump_at_ms;
                                if late || (args.dump_at_ms == 0 && rect.width() as usize == buf.size().0 && rect.height() as usize == buf.size().1) {
                                    dumped = true;
                                    dump_png(p, &buf);
                                }
                            }
                            if let Some(out) = pipe.process(&buf, now_ms) {
                                if let Ok(dir) = std::env::var("CLUB685_DUMP_LOST") {
                                    static mut HAD: bool = false;
                                    static mut N: u32 = 0;
                                    let has = out.reading.layout.is_some();
                                    unsafe {
                                        if HAD && !has && N < 6 {
                                            N += 1;
                                            dump_png(&std::path::Path::new(&dir).join(format!("lost-{N}.png")), &buf);
                                        }
                                        HAD = has;
                                    }
                                }
                                let after = query_metrics(rb.hwnd);
                                if before == after {
                                    stamp = match (out.reading.layout.is_some(), after) {
                                        (true, Some(m)) => Some(LayoutStamp { metrics: m, at_ms: now_ms }),
                                        _ => None,
                                    };
                                    last_masks = out.bpm_masks;
                                    last_text = reading_str(&out.reading);
                                    if let Some(w) = csv.as_mut() {
                                        use std::io::Write;
                                        let raw = out.reading.master_raw.map_or(-1, |m| m as i64);
                                        let m = out.reading.master.map_or(-1, |m| m as i64);
                                        let _ = writeln!(w, "{frame_ticks},{raw},{m}");
                                    }
                                    sec_ms.push(out.analyze_ms);
                                    all_ms.push(out.analyze_ms);
                                    frames_total += 1;
                                    sec_frames += 1;
                                } else {
                                    // the window moved or changed while we were reading: the layout is stale
                                    pipe.reset();
                                    stamp = None;
                                }
                            }
                        }
                        Err(_) => read_err += 1,
                    }
                } else {
                    pipe.reset();
                    stamp = None;
                }
            }
            Ok(Next::Dropped(_)) | Ok(Next::None) => {}
            Err(e) => {
                set.hide_all();
                eprintln!("club685-probe: capture error: {e}");
                exit = 3;
                break;
            }
        }

        // Gate, then overlays. Every failure hides everything immediately.
        let mut inputs = query_inputs(rb.hwnd);
        if args.ignore_foreground {
            inputs.foreground = true; // test harness only: lets the overlays be checked without focusing rekordbox
        }
        gate = if dpi_ok { evaluate(&inputs, stamp.as_ref(), now_ms) } else { GateState::MetricsChanged };
        let plan = match (gate.is_open(), stamp.as_ref(), pipe.state.layout()) {
            (true, Some(s), Some(l)) => {
                let origin = (s.metrics.window.x0, s.metrics.window.y0);
                plan_overlays(origin, s.metrics.dpi, l.width, last_masks, Some(l.band), args.hide_waveform)
            }
            _ => Plan::default(),
        };
        set.apply(&plan);

        if start.elapsed() >= next_report {
            next_report += Duration::from_secs(1);
            dpi_ok = check_dpi(rb.hwnd).is_ok();
            sec_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let el = last_report.elapsed().as_secs_f64();
            last_report = Instant::now();
            let st = session.stats();
            let shown = set.current();
            let ov = format!("bpm{}+wave{}", shown.bpm.iter().flatten().count(), shown.wave.is_some() as u8);
            println!(
                "t={:>2}s fps={:.1} analyze p50={:.2}ms p95={:.2}ms {} gate={} overlays={} drop(future/stale/old)={}/{}/{}",
                start.elapsed().as_secs(),
                sec_frames as f64 / el,
                percentile(&sec_ms, 0.5),
                percentile(&sec_ms, 0.95),
                last_text,
                gate.label(),
                ov,
                st.future,
                st.stale,
                st.not_newer
            );
            sec_frames = 0;
            sec_ms.clear();
        }
    }

    set.hide_all();
    drop(set);
    drop(session);
    all_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let secs = start.elapsed().as_secs_f64();
    println!(
        "done: {:.1}s, {} frames analyzed ({:.1} fps), analyze p50={:.2}ms p95={:.2}ms, read avg {:.2}ms ({} reads, {} full, {} errors), last gate={}",
        secs,
        frames_total,
        frames_total as f64 / secs.max(0.001),
        percentile(&all_ms, 0.5),
        percentile(&all_ms, 0.95),
        if reads > 0 { read_ms_sum / reads as f64 } else { 0.0 },
        reads,
        full_reads,
        read_err,
        gate.label()
    );
    let _ = Verdict::Fresh;
    exit
}

fn dump_png(path: &std::path::Path, buf: &FrameBuf) {
    let Some(f) = buf.bgra() else { return };
    let mut rgb = Vec::with_capacity(f.width * f.height * 3);
    for y in 0..f.h() {
        for x in 0..f.w() {
            let (r, g, b) = f.rgb(x, y);
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    if let Ok(file) = std::fs::File::create(path) {
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.width as u32, f.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut w) = enc.write_header() {
            let _ = w.write_image_data(&rgb);
        }
    }
    println!("dumped first full frame to {}", path.display());
}
