//! Window capture session: IGraphicsCaptureItemInterop::CreateForWindow on the rekordbox HWND, a D3D11 device, a
//! free-threaded frame pool (BGRA8), and CopySubresourceRegion into a CPU staging texture for each read.

use crate::framebuf::FrameBuf;
use crate::freshness::{now_ticks, Freshness, Verdict};
use club685_core::Rect;
use std::ffi::c_void;
use std::sync::Arc;
use windows::core::{factory, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, HWND, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

pub type Result<T> = windows::core::Result<T>;

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Frames that passed the freshness rules.
    pub fresh: u64,
    pub future: u64,
    pub stale: u64,
    pub not_newer: u64,
    /// Frames skipped because a newer one was already waiting (drained to the newest).
    pub superseded: u64,
}

/// One captured frame. Holds the pool buffer until dropped, so read it and drop it promptly.
pub struct Frame {
    pub(crate) _frame: Option<Direct3D11CaptureFrame>,
    pub(crate) texture: ID3D11Texture2D,
    /// SystemRelativeTime in 100 ns ticks.
    pub ticks: i64,
    /// Content size in pixels (the window's DWM extended frame bounds).
    pub width: u32,
    pub height: u32,
}

/// Why a screen-based backend cannot deliver frames of the window right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The window reaches outside the desktop.
    PartlyOffScreen,
    /// The window is not on a single monitor.
    WrongOutput,
}

pub enum Next {
    Frame(Frame),
    /// The window cannot be captured where it is (Desktop Duplication only).
    Blocked(Blocked),
    /// Nothing new inside the wait time.
    None,
    /// A frame arrived but broke the freshness rules.
    Dropped(Verdict),
}

pub struct CaptureSession {
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    pool_size: SizeInt32,
    event: HANDLE,
    freshness: Freshness,
    reader: Reader,
    pub stats: Stats,
}

/// CPU read-back of a rectangle of a window-sized texture through a cached staging texture. Shared by both backends.
pub(crate) struct Reader {
    pub(crate) device: ID3D11Device,
    pub(crate) ctx: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl Reader {
    pub(crate) fn new(device: ID3D11Device, ctx: ID3D11DeviceContext) -> Self {
        Self { device, ctx, staging: None }
    }

    fn staging_for(&mut self, w: u32, h: u32) -> Result<ID3D11Texture2D> {
        if let Some((t, sw, sh)) = &self.staging {
            if *sw == w && *sh == h {
                return Ok(t.clone());
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        let tex = tex.ok_or_else(windows::core::Error::empty)?;
        self.staging = Some((tex.clone(), w, h));
        Ok(tex)
    }

    /// Copies `rect` (window coordinates) of `frame` into `dst` at the same coordinates. `dst` is resized to the
    /// frame size (and cleared) when the frame size changed. Returns the rect actually copied.
    pub(crate) fn read_rect(&mut self, frame: &Frame, rect: Rect, dst: &mut FrameBuf) -> Result<Rect> {
        dst.ensure(frame.width as usize, frame.height as usize);
        let r = dst.clamp(rect);
        if r.width() <= 0 || r.height() <= 0 {
            return Ok(r);
        }
        let (w, h) = (r.width() as u32, r.height() as u32);
        let staging = self.staging_for(w, h)?;
        let b = D3D11_BOX { left: r.x0 as u32, top: r.y0 as u32, front: 0, right: r.x1 as u32, bottom: r.y1 as u32, back: 1 };
        unsafe {
            self.ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &frame.texture, 0, Some(&b));
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let len = m.RowPitch as usize * (h as usize - 1) + w as usize * 4;
            let src = std::slice::from_raw_parts(m.pData as *const u8, len);
            dst.blit(r, src, m.RowPitch as usize, (r.x0, r.y0));
            self.ctx.Unmap(&staging, 0);
        }
        Ok(r)
    }
}

/// Applies the freshness rules and counts the verdict.
pub(crate) fn judge(freshness: &mut Freshness, stats: &mut Stats, ticks: i64) -> Verdict {
    let verdict = freshness.check(ticks, now_ticks());
    match verdict {
        Verdict::Fresh => stats.fresh += 1,
        Verdict::Future => stats.future += 1,
        Verdict::Stale => stats.stale += 1,
        Verdict::NotNewer => stats.not_newer += 1,
    }
    verdict
}

pub(crate) fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))?;
    }
    let device = device.ok_or_else(windows::core::Error::empty)?;
    let ctx = ctx.ok_or_else(windows::core::Error::empty)?;
    let dxgi: IDXGIDevice = device.cast()?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
    let winrt: IDirect3DDevice = inspectable.cast()?;
    Ok((device, ctx, winrt))
}

impl CaptureSession {
    /// Starts capturing `hwnd`. The frames show the window's DWM extended frame bounds at 1:1.
    pub fn start(hwnd: HWND) -> Result<Self> {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let (device, ctx, winrt_device) = create_device()?;
        let (device_c, ctx_c) = (device.clone(), ctx.clone());
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 3, size)?;
        let event = unsafe { CreateEventW(None, false, false, None)? };
        let ev = Arc::new(event.0 as usize);
        let ev2 = ev.clone();
        pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
            unsafe {
                let _ = SetEvent(HANDLE(*ev2 as *mut c_void));
            }
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture()?;
        Ok(Self { winrt_device, pool, session, item, pool_size: size, event, freshness: Freshness::new(), reader: Reader::new(device_c, ctx_c), stats: Stats::default() })
    }

    /// Waits up to `wait_ms` for a frame, drains the pool to the newest one and applies the freshness rules.
    pub fn next(&mut self, wait_ms: u32) -> Result<Next> {
        let mut newest = self.drain()?;
        if newest.is_none() && wait_ms > 0 {
            let r = unsafe { WaitForSingleObject(self.event, wait_ms) };
            if r == WAIT_OBJECT_0 {
                newest = self.drain()?;
            }
        }
        let Some(frame) = newest else { return Ok(Next::None) };
        let ticks = frame.SystemRelativeTime()?.Duration;
        let cs = frame.ContentSize()?;
        if cs.Width != self.pool_size.Width || cs.Height != self.pool_size.Height {
            self.pool.Recreate(&self.winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 3, cs)?;
            self.pool_size = cs;
        }
        let verdict = judge(&mut self.freshness, &mut self.stats, ticks);
        if verdict != Verdict::Fresh {
            return Ok(Next::Dropped(verdict));
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        Ok(Next::Frame(Frame { _frame: Some(frame), texture, ticks, width: cs.Width.max(0) as u32, height: cs.Height.max(0) as u32 }))
    }

    fn drain(&mut self) -> Result<Option<Direct3D11CaptureFrame>> {
        let mut newest: Option<Direct3D11CaptureFrame> = None;
        loop {
            match self.pool.TryGetNextFrame() {
                Ok(f) => {
                    if newest.is_some() {
                        self.stats.superseded += 1;
                    }
                    newest = Some(f);
                }
                Err(_) => break,
            }
        }
        Ok(newest)
    }

    /// Copies `rect` (window coordinates) of `frame` into `dst`; see `Reader::read_rect`.
    pub fn read_rect(&mut self, frame: &Frame, rect: Rect, dst: &mut FrameBuf) -> Result<Rect> {
        self.reader.read_rect(frame, rect, dst)
    }

    /// Full-frame read, used while the layout is being discovered.
    pub fn read_full(&mut self, frame: &Frame, dst: &mut FrameBuf) -> Result<Rect> {
        self.read_rect(frame, Rect { x0: 0, y0: 0, x1: frame.width as i32, y1: frame.height as i32 }, dst)
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
        let _ = &self.item;
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}
