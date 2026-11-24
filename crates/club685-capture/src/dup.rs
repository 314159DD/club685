//! DXGI Desktop Duplication backend. Unlike Windows.Graphics.Capture it draws no border on Windows 10, but it sees
//! the screen, not the window: whatever covers rekordbox is in the frame too. Callers must only analyse frames
//! while rekordbox is the foreground window (`Capture::sees_screen`), and every window of ours is excluded from
//! capture with `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
//!
//! Each new desktop frame is copied (GPU side) into a window-sized texture and released at once; the returned
//! `Frame` shows the window's DWM extended frame bounds at 1:1, exactly like the WGC backend.

use crate::framebuf::FrameBuf;
use crate::freshness::{now_ticks, Freshness, Verdict, MAX_AGE_TICKS};
use crate::session::{judge, Blocked, Frame, Next, Reader, Result, Stats};
use crate::window::extended_frame_bounds;
use club685_core::Rect;
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Performance::QueryPerformanceFrequency;

/// QueryPerformanceCounter value to 100 ns ticks (the unit of `Frame.ticks` and the freshness rules).
pub fn qpc_to_ticks(qpc: i64, freq: i64) -> i64 {
    if freq <= 0 {
        return 0;
    }
    (qpc as i128 * 10_000_000 / freq as i128) as i64
}

fn inter(a: Rect, b: Rect) -> Rect {
    Rect { x0: a.x0.max(b.x0), y0: a.y0.max(b.y0), x1: a.x1.min(b.x1), y1: a.y1.min(b.y1) }
}

fn area(r: Rect) -> i64 {
    if r.width() <= 0 || r.height() <= 0 {
        0
    } else {
        r.width() as i64 * r.height() as i64
    }
}

/// Where a window sits relative to the monitors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Completely inside the monitor with this index.
    Inside(usize),
    /// Reaches outside the desktop (or is not on any monitor).
    PartlyOffScreen,
    /// Completely on the desktop, but across more than one monitor.
    SpansMonitors,
}

/// Classifies `win` against the monitor rectangles `outs` (virtual-screen coordinates, not overlapping).
pub fn classify(win: Rect, outs: &[Rect]) -> Placement {
    let total = area(win);
    let mut covered = 0i64;
    let mut hits = 0;
    let mut last = 0;
    for (i, o) in outs.iter().enumerate() {
        let a = area(inter(win, *o));
        if a > 0 {
            covered += a;
            hits += 1;
            last = i;
        }
    }
    if total == 0 || covered < total {
        Placement::PartlyOffScreen
    } else if hits == 1 {
        Placement::Inside(last)
    } else {
        Placement::SpansMonitors
    }
}

/// The part of window `win` that lies inside the monitor `out`: the source rect in the monitor's own texture
/// coordinates, and the offset `(dx, dy)` of that part inside the window frame. `None` when they do not overlap.
pub fn clip_to_output(win: Rect, out: Rect) -> Option<(Rect, i32, i32)> {
    let i = inter(win, out);
    if area(i) == 0 {
        return None;
    }
    let src = Rect { x0: i.x0 - out.x0, y0: i.y0 - out.y0, x1: i.x1 - out.x0, y1: i.y1 - out.y0 };
    Some((src, i.x0 - win.x0, i.y0 - win.y0))
}

#[derive(Clone, Copy, Debug)]
struct OutInfo {
    adapter: u32,
    output: u32,
    rect: Rect,
    rotated: bool,
}

fn enumerate_outputs() -> Result<Vec<OutInfo>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut v = Vec::new();
    let mut ai = 0u32;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(ai) } {
        let mut oi = 0u32;
        while let Ok(out) = unsafe { adapter.EnumOutputs(oi) } {
            if let Ok(d) = unsafe { out.GetDesc() } {
                if d.AttachedToDesktop.as_bool() {
                    let c = d.DesktopCoordinates;
                    // DXGI_MODE_ROTATION_UNSPECIFIED = 0, IDENTITY = 1
                    v.push(OutInfo { adapter: ai, output: oi, rect: Rect { x0: c.left, y0: c.top, x1: c.right, y1: c.bottom }, rotated: d.Rotation.0 > 1 });
                }
            }
            oi += 1;
        }
        ai += 1;
    }
    Ok(v)
}

struct Dup {
    dup: IDXGIOutputDuplication,
    index_key: (u32, u32),
    out_rect: Rect,
    reader: Reader,
    /// Window-sized copy target, recreated when the window size changes.
    win_tex: Option<(ID3D11Texture2D, i32, i32)>,
    first: bool,
}

pub struct DuplicationSession {
    hwnd: HWND,
    dup: Option<Dup>,
    freshness: Freshness,
    freq: i64,
    fail_count: u32,
    pub stats: Stats,
}

fn open_duplication(o: &OutInfo) -> Result<Dup> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let adapter1: IDXGIAdapter1 = unsafe { factory.EnumAdapters1(o.adapter)? };
    let adapter: IDXGIAdapter = adapter1.cast()?;
    let output: IDXGIOutput = unsafe { adapter.EnumOutputs(o.output)? };
    let output1: IDXGIOutput1 = output.cast()?;
    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            Some(&adapter),
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
    }
    let device = device.ok_or_else(windows::core::Error::empty)?;
    let ctx = ctx.ok_or_else(windows::core::Error::empty)?;
    let dup = unsafe { output1.DuplicateOutput(&device)? };
    Ok(Dup { dup, index_key: (o.adapter, o.output), out_rect: o.rect, reader: Reader::new(device, ctx), win_tex: None, first: true })
}

impl DuplicationSession {
    /// Starts duplicating the monitor that holds `hwnd`. Fails when the window is not cleanly on one monitor.
    pub fn start(hwnd: HWND) -> Result<Self> {
        let mut freq = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        let mut s = Self { hwnd, dup: None, freshness: Freshness::new(), freq, fail_count: 0, stats: Stats::default() };
        let bounds = extended_frame_bounds(hwnd).ok_or_else(windows::core::Error::empty)?;
        let outs = enumerate_outputs()?;
        let rects: Vec<Rect> = outs.iter().map(|o| o.rect).collect();
        match classify(bounds, &rects) {
            Placement::Inside(i) if !outs[i].rotated => s.dup = Some(open_duplication(&outs[i])?),
            Placement::Inside(_) => return Err(windows::core::Error::empty()),
            // Start anyway; `next` reports the placement problem until the window is moved.
            _ => {
                let i = best_output(bounds, &rects).ok_or_else(windows::core::Error::empty)?;
                s.dup = Some(open_duplication(&outs[i])?);
            }
        }
        Ok(s)
    }

    fn placement(&self) -> Result<(Rect, Placement, Vec<OutInfo>)> {
        let bounds = extended_frame_bounds(self.hwnd).ok_or_else(windows::core::Error::empty)?;
        let outs = enumerate_outputs()?;
        let rects: Vec<Rect> = outs.iter().map(|o| o.rect).collect();
        let p = classify(bounds, &rects);
        Ok((bounds, p, outs))
    }

    /// Waits up to `wait_ms` for a new desktop frame, copies the window area and applies the freshness rules.
    pub fn next(&mut self, wait_ms: u32) -> Result<Next> {
        let (bounds, placement, outs) = self.placement()?;
        let idx = match placement {
            Placement::Inside(i) => i,
            other => {
                std::thread::sleep(std::time::Duration::from_millis(wait_ms.max(1) as u64));
                return Ok(Next::Blocked(if other == Placement::SpansMonitors { Blocked::WrongOutput } else { Blocked::PartlyOffScreen }));
            }
        };
        let o = outs[idx];
        if o.rotated {
            return Err(windows::core::Error::empty());
        }
        if self.dup.as_ref().map_or(true, |d| d.index_key != (o.adapter, o.output)) {
            self.dup = None;
        }
        if self.dup.is_none() {
            match open_duplication(&o) {
                Ok(d) => {
                    self.dup = Some(d);
                    self.fail_count = 0;
                    self.freshness.reset();
                }
                Err(e) => {
                    // Secure desktop (UAC), fullscreen exclusive app, mode change: keep trying for ~10 s.
                    self.fail_count += 1;
                    if self.fail_count > 40 {
                        return Err(e);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    return Ok(Next::None);
                }
            }
        }
        let d = self.dup.as_mut().unwrap();
        d.out_rect = o.rect;
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        match unsafe { d.dup.AcquireNextFrame(wait_ms, &mut info, &mut res) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Next::None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.dup = None;
                return Ok(Next::None);
            }
            Err(e) => return Err(e),
        }
        let first = std::mem::replace(&mut d.first, false);
        // Only the mouse moved: not a new frame for us. (The very first image is always taken.)
        if info.LastPresentTime == 0 && !first {
            unsafe {
                let _ = d.dup.ReleaseFrame();
            }
            return Ok(Next::None);
        }
        let copied = (|| -> Result<(i64, u32, u32, ID3D11Texture2D)> {
            let desk: ID3D11Texture2D = res.ok_or_else(windows::core::Error::empty)?.cast()?;
            let mut dd = D3D11_TEXTURE2D_DESC::default();
            unsafe { desk.GetDesc(&mut dd) };
            if dd.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                return Err(windows::core::Error::empty());
            }
            let (w, h) = (bounds.width(), bounds.height());
            let tex = match &d.win_tex {
                Some((t, tw, th)) if *tw == w && *th == h => t.clone(),
                _ => {
                    let desc = D3D11_TEXTURE2D_DESC {
                        Width: w as u32,
                        Height: h as u32,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: 0,
                        CPUAccessFlags: 0,
                        MiscFlags: 0,
                    };
                    let mut t: Option<ID3D11Texture2D> = None;
                    unsafe { d.reader.device.CreateTexture2D(&desc, None, Some(&mut t))? };
                    let t = t.ok_or_else(windows::core::Error::empty)?;
                    d.win_tex = Some((t.clone(), w, h));
                    t
                }
            };
            let (src, dx, dy) = clip_to_output(bounds, o.rect).ok_or_else(windows::core::Error::empty)?;
            let b = D3D11_BOX { left: src.x0 as u32, top: src.y0 as u32, front: 0, right: src.x1 as u32, bottom: src.y1 as u32, back: 1 };
            unsafe { d.reader.ctx.CopySubresourceRegion(&tex, 0, dx as u32, dy as u32, 0, &desk, 0, Some(&b)) };
            let now = now_ticks();
            let mut ticks = qpc_to_ticks(info.LastPresentTime, self.freq);
            if first && (info.LastPresentTime == 0 || now - ticks > MAX_AGE_TICKS) {
                // The initial image of a new duplication shows what is on screen right now.
                ticks = now;
            }
            Ok((ticks, w as u32, h as u32, tex))
        })();
        unsafe {
            let _ = d.dup.ReleaseFrame();
        }
        let (ticks, w, h, tex) = copied?;
        let verdict = judge(&mut self.freshness, &mut self.stats, ticks);
        if verdict != Verdict::Fresh {
            return Ok(Next::Dropped(verdict));
        }
        Ok(Next::Frame(Frame { _frame: None, texture: tex, ticks, width: w, height: h }))
    }

    pub fn read_rect(&mut self, frame: &Frame, rect: Rect, dst: &mut FrameBuf) -> Result<Rect> {
        match self.dup.as_mut() {
            Some(d) => d.reader.read_rect(frame, rect, dst),
            None => Err(windows::core::Error::empty()),
        }
    }
}

fn best_output(win: Rect, outs: &[Rect]) -> Option<usize> {
    outs.iter().enumerate().max_by_key(|(_, o)| area(inter(win, **o))).map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    const M1: Rect = Rect { x0: 0, y0: 0, x1: 2560, y1: 1440 };
    const M2: Rect = Rect { x0: 2560, y0: 0, x1: 4480, y1: 1080 };

    #[test]
    fn qpc_conversion() {
        assert_eq!(qpc_to_ticks(10_000_000, 10_000_000), 10_000_000);
        // 3.0 MHz QPC (a common value): 3_000_000 counts = 1 s = 10_000_000 ticks
        assert_eq!(qpc_to_ticks(3_000_000, 3_000_000), 10_000_000);
        assert_eq!(qpc_to_ticks(1_500_000, 3_000_000), 5_000_000);
        assert_eq!(qpc_to_ticks(0, 3_000_000), 0);
        assert_eq!(qpc_to_ticks(5, 0), 0);
        // no overflow with a large uptime counter (10 days at 10 MHz)
        let c = 10i64 * 86_400 * 10_000_000;
        assert_eq!(qpc_to_ticks(c, 10_000_000), c);
        assert_eq!(qpc_to_ticks(i64::MAX / 2, 1_000_000_000), (i64::MAX / 2) / 100);
    }

    #[test]
    fn classify_inside_one_monitor() {
        let w = Rect { x0: 100, y0: 20, x1: 2660 - 100, y1: 1430 };
        assert_eq!(classify(w, &[M1, M2]), Placement::Inside(0));
        let w2 = Rect { x0: 2600, y0: 10, x1: 4400, y1: 1000 };
        assert_eq!(classify(w2, &[M1, M2]), Placement::Inside(1));
    }

    #[test]
    fn classify_off_screen_and_spanning() {
        // sticks out at the bottom of monitor 1
        assert_eq!(classify(Rect { x0: 0, y0: 100, x1: 2560, y1: 1510 }, &[M1, M2]), Placement::PartlyOffScreen);
        // sticks out left of the desktop
        assert_eq!(classify(Rect { x0: -50, y0: 0, x1: 2000, y1: 900 }, &[M1, M2]), Placement::PartlyOffScreen);
        // entirely elsewhere
        assert_eq!(classify(Rect { x0: 9000, y0: 0, x1: 9800, y1: 600 }, &[M1, M2]), Placement::PartlyOffScreen);
        // spans both monitors, fully on the desktop
        assert_eq!(classify(Rect { x0: 2000, y0: 100, x1: 3200, y1: 900 }, &[M1, M2]), Placement::SpansMonitors);
        // empty window
        assert_eq!(classify(Rect { x0: 5, y0: 5, x1: 5, y1: 5 }, &[M1]), Placement::PartlyOffScreen);
    }

    #[test]
    fn clip_maths() {
        // fully inside the second monitor: texture coordinates are relative to the monitor origin
        let w = Rect { x0: 2600, y0: 10, x1: 3000, y1: 110 };
        assert_eq!(clip_to_output(w, M2), Some((Rect { x0: 40, y0: 10, x1: 440, y1: 110 }, 0, 0)));
        // hanging over the left edge of the monitor by 50 px: the window frame offset is 50
        let w = Rect { x0: -50, y0: 100, x1: 350, y1: 200 };
        assert_eq!(clip_to_output(w, M1), Some((Rect { x0: 0, y0: 100, x1: 350, y1: 200 }, 50, 0)));
        // hanging over the bottom edge
        let w = Rect { x0: 10, y0: 1400, x1: 110, y1: 1500 };
        assert_eq!(clip_to_output(w, M1), Some((Rect { x0: 10, y0: 1400, x1: 110, y1: 1440 }, 0, 0)));
        // no overlap
        assert_eq!(clip_to_output(Rect { x0: 3000, y0: 0, x1: 3100, y1: 50 }, M1), None);
    }
}
