//! club685.exe: the CLUB685 window shell. One borderless layered window (per-pixel alpha, drawn by `ui`), a tray
//! icon, single instance, no elevation (only the `--install-module` / `--remove-module` helper runs elevated, copies or
//! deletes one file and exits) and the worker thread.
#![windows_subsystem = "windows"]

use club685_app::prefs::{self, PhaseSetting, WaveMode};
use club685_app::preview::PhaseClock;
use club685_app::install;
use club685_app::state::{diagnostics_text, Shared, Snapshot};
use club685_app::ui::{self, Action, Mode, UiRenderer, UiState};
use club685_app::worker;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{SetLastError, WIN32_ERROR, CloseHandle, COLORREF, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
    GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, KillTimer, LoadCursorW, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TrackPopupMenu,
    TranslateMessage, UpdateLayeredWindow, GWLP_USERDATA, HICON, HWND_TOP, ICONINFO, ICON_BIG, ICON_SMALL, IDC_ARROW, MB_ICONERROR, MB_OK, MF_STRING, MSG,
    SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
    SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, ULW_ALPHA, WM_APP, WM_DESTROY, WM_EXITSIZEMOVE, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCHITTEST, WM_NULL, WM_RBUTTONUP, WM_SETICON, WM_TIMER, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_POPUP,
};

const CLASS: PCWSTR = w!("CLUB685_Main");
const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW_ME: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;

struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    bits: *mut u8,
    w: i32,
    h: i32,
}

struct App {
    shared: Arc<Mutex<Shared>>,
    renderer: UiRenderer,
    dib: Dib,
    hover: Option<Action>,
    pressed: Option<Action>,
    tracking: bool,
    settings_open: bool,
    copied_until: Option<Instant>,
    last_key: u64,
    last_paint: Instant,
    dirty: bool,
    start: Instant,
    clock: PhaseClock,
    prefs_path: Option<PathBuf>,
    #[allow(dead_code)]
    icon_big: HICON,
    icon_small: HICON,
    taskbar_created: u32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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

fn message_box(text: &str) {
    let t = wide(text);
    unsafe {
        MessageBoxW(None, PCWSTR(t.as_ptr()), w!("CLUB685"), MB_OK | MB_ICONERROR);
    }
}

fn make_dib(w: i32, h: i32) -> Dib {
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
        let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).expect("DIB section");
        SelectObject(dc, HGDIOBJ(bmp.0));
        ReleaseDC(None, screen);
        Dib { dc, bmp, bits: bits as *mut u8, w, h }
    }
}

fn icon_from(pm: &tiny_skia::Pixmap) -> HICON {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let dib = make_dib(w, h);
    unsafe {
        let dst = std::slice::from_raw_parts_mut(dib.bits, (w * h * 4) as usize);
        for (i, px) in pm.pixels().iter().enumerate() {
            let c = px.demultiply();
            dst[i * 4] = c.blue();
            dst[i * 4 + 1] = c.green();
            dst[i * 4 + 2] = c.red();
            dst[i * 4 + 3] = c.alpha();
        }
        let mask = CreateBitmap(w, h, 1, 1, None);
        let info = ICONINFO { fIcon: BOOL(1), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: dib.bmp };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteObject(HGDIOBJ(dib.bmp.0));
        let _ = DeleteDC(dib.dc);
        icon
    }
}

fn tray_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    for (i, c) in "CLUB685".encode_utf16().enumerate() {
        nid.szTip[i] = c;
    }
    nid
}

fn app<'a>(hwnd: HWND) -> Option<&'a mut App> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

fn visible_key(snap: &Snapshot, settings_open: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut c = snap.clone();
    c.rev = 0;
    c.beat_px = c.beat_px.map(|b| (b * 10.0).round() / 10.0);
    c.cursor = c.cursor.map(|b| b.round());
    for d in &mut c.decks {
        d.phase = d.phase.map(|p| p.floor());
    }
    if !settings_open {
        c.frames = 0;
        c.gate.clear();
        c.overlays.clear();
        c.window.clear();
        c.layout.clear();
        c.rb_version.clear();
        c.dpi = 0;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{c:?}").hash(&mut h);
    h.finish()
}

fn repaint(hwnd: HWND, a: &mut App) {
    let (prefs, snap) = {
        let g = a.shared.lock().unwrap();
        (g.prefs.clone(), g.snap.clone())
    };
    let copied = a.copied_until.is_some_and(|t| t > Instant::now());
    let preview = a.clock.step(a.start.elapsed().as_secs_f64(), &snap);
    let st = UiState {
        prefs: &prefs,
        snap: &snap,
        preview: &preview,
        hover: a.hover,
        pressed: a.pressed,
        settings_open: a.settings_open,
        copied,
        time: a.start.elapsed().as_secs_f32(),
    };
    let pm = a.renderer.paint(&st);
    a.last_key = visible_key(&snap, a.settings_open);
    a.last_paint = Instant::now();
    a.dirty = false;
    unsafe {
        let dst = std::slice::from_raw_parts_mut(a.dib.bits, (a.dib.w * a.dib.h * 4) as usize);
        for (i, px) in pm.data().chunks_exact(4).enumerate() {
            dst[i * 4] = px[2];
            dst[i * 4 + 1] = px[1];
            dst[i * 4 + 2] = px[0];
            dst[i * 4 + 3] = px[3];
        }
        let screen = GetDC(None);
        let size = SIZE { cx: a.dib.w, cy: a.dib.h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = UpdateLayeredWindow(hwnd, Some(screen), None, Some(&size), Some(a.dib.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        ReleaseDC(None, screen);
    }
}

fn save_prefs(a: &App) {
    let prefs = a.shared.lock().unwrap().prefs.clone();
    if let Some(p) = &a.prefs_path {
        let _ = prefs::save(p, &prefs);
    }
}

fn set_clipboard(hwnd: HWND, text: &str) -> bool {
    let w16 = wide(&text.replace('\n', "\r\n"));
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = (|| -> Option<()> {
            let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, w16.len() * 2).ok()?;
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(w16.as_ptr(), p, w16.len());
            let _ = GlobalUnlock(h);
            SetClipboardData(13, Some(HANDLE(h.0))).ok()?; // CF_UNICODETEXT
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

fn restore(hwnd: HWND) {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    if let Some(a) = app(hwnd) {
        repaint(hwnd, a);
    }
}

fn quit(hwnd: HWND) {
    if let Some(a) = app(hwnd) {
        a.shared.lock().unwrap().quit = true;
        save_prefs(a);
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

fn perform(hwnd: HWND, a: &mut App, act: Action) {
    match act {
        Action::Minimise => unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        },
        Action::Quit => {
            quit(hwnd);
            return;
        }
        Action::Power => {
            let mut g = a.shared.lock().unwrap();
            g.prefs.power = !g.prefs.power;
        }
        Action::Wave(m) => a.shared.lock().unwrap().prefs.waveform = m,
        Action::Phase(p) => a.shared.lock().unwrap().prefs.phase = p,
        Action::Disco => {
            let mut g = a.shared.lock().unwrap();
            g.prefs.disco = !g.prefs.disco;
        }
        Action::Settings => a.settings_open = !a.settings_open,
        Action::CloseSettings => a.settings_open = false,
        Action::InstallModule | Action::RemoveModule => {
            let (dir, mode) = {
                let g = a.shared.lock().unwrap();
                (g.snap.module_dir.clone(), if act == Action::InstallModule { install::FLAG_INSTALL } else { install::FLAG_REMOVE })
            };
            if let (Some(dir), Some(bundled)) = (dir, install::bundled_module_path()) {
                install::launch_elevated(mode, &bundled, &dir);
            } else {
                message_box("The bundled rekordbox module was not found. Extract the whole ZIP and start CLUB685 from the extracted folder.");
            }
        }
        Action::CopyDiag => {
            let text = {
                let g = a.shared.lock().unwrap();
                diagnostics_text(&g.snap, &g.prefs)
            };
            if set_clipboard(hwnd, &text) {
                a.copied_until = Some(Instant::now() + Duration::from_millis(1800));
            }
        }
    }
    save_prefs(a);
    a.dirty = true;
    repaint(hwnd, a);
}

fn mode_of(a: &App) -> Mode {
    Mode::of(&a.shared.lock().unwrap().snap)
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(a) = app(hwnd) else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_NCHITTEST => {
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let (sx, sy) = lparam_xy(lp);
            let s = a.renderer.scale;
            let (x, y) = ((sx - r.left) as f32 / s, (sy - r.top) as f32 / s);
            LRESULT(if ui::is_drag_region(x, y) { HTCAPTION } else { HTCLIENT })
        }
        WM_MOUSEMOVE => {
            let (px, py) = lparam_xy(lp);
            let s = a.renderer.scale;
            let h = ui::hit_test(px as f32 / s, py as f32 / s, a.settings_open, mode_of(a));
            if !a.tracking {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
                a.tracking = true;
            }
            if h != a.hover {
                a.hover = h;
                a.dirty = true;
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            a.tracking = false;
            if a.hover.is_some() {
                a.hover = None;
                a.dirty = true;
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (px, py) = lparam_xy(lp);
            let s = a.renderer.scale;
            a.pressed = ui::hit_test(px as f32 / s, py as f32 / s, a.settings_open, mode_of(a));
            if a.pressed.is_some() {
                let _ = SetCapture(hwnd);
                a.dirty = true;
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (px, py) = lparam_xy(lp);
            let s = a.renderer.scale;
            let h = ui::hit_test(px as f32 / s, py as f32 / s, a.settings_open, mode_of(a));
            let pressed = a.pressed.take();
            let _ = ReleaseCapture();
            a.dirty = true;
            match (pressed, h) {
                (Some(p), Some(h)) if p == h => perform(hwnd, a, p),
                _ => repaint(hwnd, a),
            }
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                {
                    let mut g = a.shared.lock().unwrap();
                    g.prefs.window_x = Some(r.left);
                    g.prefs.window_y = Some(r.top);
                }
                save_prefs(a);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_ID => {
            if !IsWindowVisible(hwnd).as_bool() {
                return LRESULT(0);
            }
            let (snap_key, disco) = {
                let g = a.shared.lock().unwrap();
                (visible_key(&g.snap, a.settings_open), g.prefs.disco)
            };
            let copied_expired = a.copied_until.is_some_and(|t| t <= Instant::now());
            if copied_expired {
                a.copied_until = None;
                a.dirty = true;
            }
            let since = a.last_paint.elapsed();
            // the LCD preview keeps running (30 fps at most) on the main view; the settings page is static
            let anim = !a.settings_open || disco;
            if (snap_key != a.last_key || a.dirty || anim) && since >= Duration::from_millis(33) {
                repaint(hwnd, a);
            }
            LRESULT(0)
        }
        WM_TRAY => {
            match (lp.0 & 0xffff) as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => restore(hwnd),
                WM_RBUTTONUP => {
                    let menu = CreatePopupMenu().unwrap_or_default();
                    let _ = AppendMenuW(menu, MF_STRING, 1, w!("Open CLUB685"));
                    let _ = AppendMenuW(menu, MF_STRING, 2, w!("Quit"));
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = SetForegroundWindow(hwnd);
                    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, Some(0), hwnd, None);
                    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                    let _ = DestroyMenu(menu);
                    match cmd.0 {
                        1 => restore(hwnd),
                        2 => quit(hwnd),
                        _ => {}
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SHOW_ME => {
            restore(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            let nid = tray_data(hwnd, a.icon_small);
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m == a.taskbar_created && m != 0 => {
            let nid = tray_data(hwnd, a.icon_small);
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn start_position(prefs: &prefs::Prefs, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        if let (Some(x), Some(y)) = (prefs.window_x, prefs.window_y) {
            // keep it only when a good part of the title bar is on a screen
            if x + w > vx + 120 && x < vx + vw - 120 && y >= vy - 10 && y < vy + vh - 60 {
                return (x, y);
            }
        }
        ((GetSystemMetrics(SM_CXSCREEN) - w) / 2, ((GetSystemMetrics(SM_CYSCREEN) - h) / 2).max(0))
    }
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some(install::FLAG_INSTALL) | Some(install::FLAG_REMOVE)) {
        return install::run_helper(&args);
    }
    if is_elevated() {
        message_box("CLUB685 does not run elevated (as administrator). Start it as a normal user, the same way as rekordbox.");
        return 2;
    }
    // Single instance. The handle stays open until the process exits.
    let _mutex = unsafe {
        SetLastError(WIN32_ERROR(0));
        let m = CreateMutexW(None, true, w!("Local\\CLUB685"));
        match m {
            Ok(h) if GetLastError() != ERROR_ALREADY_EXISTS => h,
            _ => {
                if let Ok(existing) = FindWindowW(CLASS, PCWSTR::null()) {
                    let _ = PostMessageW(Some(existing), WM_SHOW_ME, WPARAM(0), LPARAM(0));
                }
                return 0;
            }
        }
    };
    unsafe {
        // rekordbox is system DPI aware; matching it keeps our coordinates equal to its.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
    }

    let prefs_path = prefs::default_path();
    let prefs = prefs_path.as_deref().map(prefs::load).unwrap_or_default();
    let shared = Arc::new(Mutex::new(Shared::new(prefs.clone())));
    let worker = worker::spawn(shared.clone());

    let scale = unsafe { GetDpiForSystem() } as f32 / 96.0;
    let renderer = UiRenderer::new(scale.max(1.0));
    let (ww, wh) = renderer.size();
    let (x, y) = start_position(&prefs, ww as i32, wh as i32);

    let icon_big = icon_from(&ui::draw_icon(32));
    let icon_small = icon_from(&ui::draw_icon(16));
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: icon_big,
            ..Default::default()
        };
        RegisterClassW(&wc);
        match CreateWindowExW(WS_EX_LAYERED | WS_EX_APPWINDOW, CLASS, w!("CLUB685"), WS_POPUP, x, y, ww as i32, wh as i32, None, None, Some(hinst.into()), None) {
            Ok(h) => h,
            Err(e) => {
                message_box(&format!("CLUB685 could not create its window: {e}"));
                shared.lock().unwrap().quit = true;
                let _ = worker.join();
                return 3;
            }
        }
    };

    club685_overlay::exclude_layered_window(hwnd);
    let a = Box::new(App {
        shared: shared.clone(),
        renderer,
        dib: make_dib(ww as i32, wh as i32),
        hover: None,
        pressed: None,
        tracking: false,
        settings_open: false,
        copied_until: None,
        last_key: 0,
        last_paint: Instant::now() - Duration::from_secs(1),
        dirty: true,
        start: Instant::now(),
        clock: PhaseClock::new(),
        prefs_path,
        icon_big,
        icon_small,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
    });
    let ptr = Box::into_raw(a);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(icon_big.0 as isize)));
        SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(icon_small.0 as isize)));
        let nid = tray_data(hwnd, icon_small);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        repaint(hwnd, &mut *ptr);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOZORDER | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE);
        SetTimer(Some(hwnd), TIMER_ID, 33, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        shared.lock().unwrap().quit = true;
        let _ = worker.join();
        drop(Box::from_raw(ptr));
    }
    let _ = PhaseSetting::Off;
    let _ = WaveMode::Normal;
    0
}
