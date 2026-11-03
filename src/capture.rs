//! Screen capture for the card scanner.
//!
//! Primary: DXGI Desktop Duplication. The newest desktop frame stays on the GPU and only
//! the requested strip is copied back. When the desktop hasn't changed, Windows says so
//! immediately and nothing is copied at all. No DWM sync stall, so no frame hitches.
//!
//! Fallback: GDI BitBlt (works everywhere, ~7 ms per grab because it syncs with DWM).

use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;

pub struct Grabber {
    dxgi: Option<Dxgi>,
    gdi: Option<Gdi>,
    pub buf: Vec<u8>,
    pub size: (i32, i32),
    last_rect: (i32, i32, i32, i32),
    /// DXGI failures in a row. A fullscreen switch or alt-tab can briefly leave no frame;
    /// only give DXGI up when it keeps failing.
    dxgi_fails: u32,
}

impl Grabber {
    pub fn new() -> Grabber {
        let dxgi = match Dxgi::new() {
            Ok(d) => {
                log::info!("capture: DXGI desktop duplication");
                Some(d)
            }
            Err(e) => {
                log::info!("capture: GDI (DXGI unavailable: {e})");
                None
            }
        };
        Grabber { dxgi, gdi: None, buf: Vec::new(), size: (0, 0), last_rect: (0, 0, 0, 0), dxgi_fails: 0 }
    }

    pub fn backend(&self) -> &'static str {
        if self.dxgi.is_some() {
            "dxgi"
        } else {
            "gdi"
        }
    }

    /// Fill `buf` with the rectangle's BGRA pixels. Returns false when nothing on screen
    /// changed since the last grab of the same rectangle (`buf` still holds those pixels).
    pub fn grab(&mut self, x: i32, y: i32, w: i32, h: i32) -> bool {
        let rect = (x, y, w, h);
        let same_rect = rect == self.last_rect && !self.buf.is_empty();
        if let Some(d) = self.dxgi.as_mut() {
            match d.grab(x, y, w, h, &mut self.buf, same_rect) {
                Ok(changed) => {
                    self.dxgi_fails = 0;
                    self.size = (w, h);
                    self.last_rect = rect;
                    return changed;
                }
                Err(e) => {
                    self.dxgi_fails += 1;
                    if self.dxgi_fails >= 20 {
                        log::warn!("capture: DXGI keeps failing ({e}); switching to GDI");
                        self.dxgi = None;
                    } else {
                        log::debug!("capture: DXGI miss ({e}); GDI for this grab");
                    }
                }
            }
        }
        let gdi = self.gdi.get_or_insert_with(Gdi::new);
        gdi.grab(x, y, w, h, &mut self.buf);
        self.size = (w, h);
        self.last_rect = rect;
        true
    }
}

// ------------------------------------------------------------------ DXGI

struct Dxgi {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    output: IDXGIOutput1,
    dup: IDXGIOutputDuplication,
    origin: (i32, i32),
    /// GPU copy of the newest desktop frame (so any strip can be read at any time).
    frame: Option<ID3D11Texture2D>,
    /// CPU-readable copies, one per strip size in use (card strip, minimap, scoreboard...).
    staging: Vec<(ID3D11Texture2D, i32, i32)>,
}

impl Dxgi {
    fn new() -> Result<Dxgi, String> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| e.to_string())?;
            // the output showing the primary monitor (desktop origin 0,0), on whichever GPU drives it
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                let mut j = 0;
                while let Ok(output) = adapter.EnumOutputs(j) {
                    j += 1;
                    let Ok(desc) = output.GetDesc() else { continue };
                    let r: RECT = desc.DesktopCoordinates;
                    if !desc.AttachedToDesktop.as_bool() || r.left != 0 || r.top != 0 {
                        continue;
                    }
                    let mut device = None;
                    let mut ctx = None;
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        Some(&[D3D_FEATURE_LEVEL_11_0]),
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut ctx),
                    )
                    .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
                    let device = device.ok_or("no device")?;
                    let ctx = ctx.ok_or("no context")?;
                    let output: IDXGIOutput1 = output.cast().map_err(|e| e.to_string())?;
                    let dup = output.DuplicateOutput(&device).map_err(|e| format!("DuplicateOutput: {e}"))?;
                    return Ok(Dxgi { device, ctx, output, dup, origin: (r.left, r.top), frame: None, staging: Vec::new() });
                }
            }
            Err("primary output not found".into())
        }
    }

    /// Pull the newest desktop frame into `self.frame`. Ok(true) if the image changed.
    unsafe fn update(&mut self, timeout_ms: u32) -> Result<bool, String> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                // resolution change, UAC prompt, fullscreen switch...: start over
                self.dup = self.output.DuplicateOutput(&self.device).map_err(|e| format!("re-duplicate: {e}"))?;
                self.frame = None;
                return Ok(false);
            }
            Err(e) => return Err(format!("AcquireNextFrame: {e}")),
        }
        // LastPresentTime 0 = only the mouse moved: the desktop image is the same
        let changed = info.LastPresentTime != 0 || self.frame.is_none();
        if changed {
            if let Some(res) = resource {
                let tex: ID3D11Texture2D = res.cast().map_err(|e| e.to_string())?;
                if self.frame.is_none() {
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    tex.GetDesc(&mut desc);
                    desc.Usage = D3D11_USAGE_DEFAULT;
                    desc.BindFlags = 0;
                    desc.CPUAccessFlags = 0;
                    desc.MiscFlags = 0;
                    let mut frame = None;
                    self.device.CreateTexture2D(&desc, None, Some(&mut frame)).map_err(|e| e.to_string())?;
                    self.frame = frame;
                }
                if let Some(frame) = &self.frame {
                    self.ctx.CopyResource(frame, &tex);
                }
            }
        }
        let _ = self.dup.ReleaseFrame();
        Ok(changed)
    }

    fn grab(&mut self, x: i32, y: i32, w: i32, h: i32, buf: &mut Vec<u8>, same_rect: bool) -> Result<bool, String> {
        unsafe {
            let mut changed = self.update(0)?;
            if self.frame.is_none() {
                changed = self.update(200)?; // first grab: wait for a frame
            }
            let Some(frame) = self.frame.clone() else { return Err("no desktop frame yet".into()) };
            if !changed && same_rect {
                return Ok(false);
            }
            let found = self.staging.iter().position(|(_, sw, sh)| (*sw, *sh) == (w, h));
            let idx = if let Some(i) = found {
                i
            } else {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w as u32,
                    Height: h as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut tex = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut tex)).map_err(|e| e.to_string())?;
                if self.staging.len() >= 6 {
                    self.staging.remove(0);
                }
                self.staging.push((tex.ok_or("staging")?, w, h));
                self.staging.len() - 1
            };
            let (staging, ..) = &self.staging[idx];
            let (ox, oy) = (x - self.origin.0, y - self.origin.1);
            let src = D3D11_BOX { left: ox as u32, top: oy as u32, front: 0, right: (ox + w) as u32, bottom: (oy + h) as u32, back: 1 };
            self.ctx.CopySubresourceRegion(staging, 0, 0, 0, 0, &frame, 0, Some(&src));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| format!("Map: {e}"))?;
            let row = (w * 4) as usize;
            buf.resize(row * h as usize, 0);
            let src = mapped.pData as *const u8;
            for r in 0..h as usize {
                std::ptr::copy_nonoverlapping(src.add(r * mapped.RowPitch as usize), buf.as_mut_ptr().add(r * row), row);
            }
            self.ctx.Unmap(staging, 0);
            Ok(true)
        }
    }
}

// ------------------------------------------------------------------ GDI fallback

struct Gdi {
    screen: HDC,
    mem: HDC,
    bmp: HBITMAP,
    size: (i32, i32),
}

impl Gdi {
    fn new() -> Gdi {
        unsafe {
            let screen = GetDC(HWND::default());
            let mem = CreateCompatibleDC(screen);
            Gdi { screen, mem, bmp: HBITMAP::default(), size: (0, 0) }
        }
    }

    /// Layered windows (our overlay) aren't captured without CAPTUREBLT.
    fn grab(&mut self, x: i32, y: i32, w: i32, h: i32, buf: &mut Vec<u8>) {
        unsafe {
            if self.size != (w, h) {
                if !self.bmp.is_invalid() {
                    let _ = DeleteObject(self.bmp);
                }
                self.bmp = CreateCompatibleBitmap(self.screen, w, h);
                SelectObject(self.mem, self.bmp);
                self.size = (w, h);
            }
            buf.resize((w * h * 4) as usize, 0);
            let _ = BitBlt(self.mem, 0, 0, w, h, self.screen, x, y, SRCCOPY);
            let mut info = BITMAPINFO {
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
            GetDIBits(self.mem, self.bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        }
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        unsafe {
            if !self.bmp.is_invalid() {
                let _ = DeleteObject(self.bmp);
            }
            let _ = DeleteDC(self.mem);
            ReleaseDC(HWND::default(), self.screen);
        }
    }
}
