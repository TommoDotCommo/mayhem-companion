//! In-game overlay: per-pixel-alpha layered windows drawn with Direct2D + DirectWrite.
//!
//! Windows are click-through, never take focus, hidden from screen capture, and only
//! redrawn when their content changes (no render loop, zero idle cost).

use crate::config::Config;
use crate::model::{Fit, Tier};
use crate::state::{ordinal, rank_band, Badge, RelicRef};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_APP_WAKE: u32 = WM_APP + 1;
pub const COMPANION_TITLE: &str = "ARAM Mayhem";

// ------------------------------------------------------------------ public API

#[derive(Clone, Debug, Default)]
pub struct PanelCol {
    pub title: String,
    pub color: [u8; 3],
    pub rows: Vec<(Tier, String, f64)>,
    pub avoid: Vec<(Tier, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct PanelContent {
    pub title: String,
    pub subtitle: String,
    pub cols: Vec<PanelCol>,
    pub builds: Vec<(String, String)>,
    pub footer: String,
}

pub enum OverlayCmd {
    Badges(Vec<Badge>),
    Panel(Option<PanelContent>),
    /// Some(show) or None = toggle.
    PanelVisible(Option<bool>),
}

#[derive(Clone)]
pub struct OverlayHandle {
    tx: Sender<OverlayCmd>,
    tid: Arc<AtomicU32>,
}

impl OverlayHandle {
    pub fn send(&self, cmd: OverlayCmd) {
        let _ = self.tx.send(cmd);
        let tid = self.tid.load(Ordering::Acquire);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

pub fn spawn(cfg: Config, relics: RelicRef) -> OverlayHandle {
    let (tx, rx) = channel();
    let tid = Arc::new(AtomicU32::new(0));
    let tid2 = tid.clone();
    std::thread::Builder::new()
        .name("overlay".into())
        .spawn(move || {
            if let Err(e) = unsafe { run(cfg, rx, tid2, relics) } {
                log::error!("overlay stopped: {e}");
            }
        })
        .expect("spawn overlay");
    OverlayHandle { tx, tid }
}

fn companion_hwnd() -> Option<HWND> {
    let title: Vec<u16> = COMPANION_TITLE.encode_utf16().chain(Some(0)).collect();
    unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())).ok().filter(|h| !h.is_invalid()) }
}

/// Show/raise the companion (starting it if needed), or minimise it if it's in front.
pub fn toggle_companion() {
    match companion_hwnd() {
        Some(hwnd) => unsafe {
            if GetForegroundWindow() == hwnd && !IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            } else {
                bring_to_front(hwnd);
            }
        },
        None => crate::spawn_self(&["--companion"]),
    }
}

/// Bring the companion to the front, starting it if it isn't running.
pub fn ensure_companion() {
    match companion_hwnd() {
        Some(hwnd) => unsafe { bring_to_front(hwnd) },
        None => crate::spawn_self(&["--companion"]),
    }
}

pub fn focus_companion() {
    if let Some(hwnd) = companion_hwnd() {
        unsafe { bring_to_front(hwnd) }
    }
}

fn quit() -> ! {
    log::info!("quit");
    crate::ipc::send(&crate::ipc::ToCompanion::Close);
    unsafe {
        if let Some(nid) = TRAY.get() {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid.0);
        }
    }
    std::process::exit(0);
}

// ------------------------------------------------------------------ app icon + tray

/// The app icon (pink tile, white "M"), rendered with Direct2D. Premultiplied BGRA.
pub fn render_icon(size: i32) -> Option<Vec<u8>> {
    unsafe {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let rt = d2d.CreateDCRenderTarget(&rt_props()).ok()?;
        let mut p = Painter { rt, dw, formats: HashMap::new() };
        let mem = CreateCompatibleDC(HDC::default());
        let info = dib_info(size, size);
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).ok()?;
        let old = SelectObject(mem, dib);
        let s = size as f32;
        let _ = p.rt.BindDC(mem, &RECT { left: 0, top: 0, right: size, bottom: size });
        p.rt.BeginDraw();
        p.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        p.fill(0.0, 0.0, s, s, s * 0.22, Tier::SPlus.rgb(), 1.0);
        let t = p.text("M", s * 0.72, DWRITE_FONT_WEIGHT_BLACK);
        p.draw(&t, (s - t.w) / 2.0, (s - t.h) / 2.0, [0xff, 0xff, 0xff]);
        let _ = p.rt.EndDraw(None, None);
        let out = std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize).to_vec();
        SelectObject(mem, old);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem);
        Some(out)
    }
}

struct TrayData(NOTIFYICONDATAW);
unsafe impl Send for TrayData {}
unsafe impl Sync for TrayData {}
static TRAY: std::sync::OnceLock<TrayData> = std::sync::OnceLock::new();
const WM_APP_TRAY: u32 = WM_APP + 2;
const WM_APP_TRAY_FWD: u32 = WM_APP + 3;

unsafe fn add_tray(hinst: HINSTANCE) -> Option<HWND> {
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("AramOverlayLayer"),
        w!("AramMayhemTray"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        HWND::default(),
        HMENU::default(),
        hinst,
        None,
    )
    .ok()?;
    let size = 32;
    let bgra = render_icon(size)?;
    let mem = CreateCompatibleDC(HDC::default());
    let mut bits = std::ptr::null_mut();
    let color = CreateDIBSection(mem, &dib_info(size, size), DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).ok()?;
    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
    let mask = CreateBitmap(size, size, 1, 1, None);
    let icon = CreateIconIndirect(&ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color }).ok()?;
    let _ = DeleteDC(mem);
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    for (i, c) in "ARAM Mayhem".encode_utf16().enumerate() {
        nid.szTip[i] = c;
    }
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    let _ = TRAY.set(TrayData(nid));
    Some(hwnd)
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("AramMayhem");

fn autostart_enabled() -> bool {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_READ, &mut key).is_err() {
            return false;
        }
        let found = RegQueryValueExW(key, RUN_VALUE, None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        found
    }
}

fn set_autostart(on: bool) {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let cmd: Vec<u16> = format!("\"{exe}\" --background").encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
            let _ = RegSetValueExW(key, RUN_VALUE, 0, REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

unsafe fn tray_menu(hwnd: HWND) -> u32 {
    let Ok(menu) = CreatePopupMenu() else { return 0 };
    let _ = AppendMenuW(menu, MF_STRING, 1, w!("Open companion"));
    let _ = AppendMenuW(menu, MF_STRING, 2, w!("Show / hide in-game panel"));
    let flags = if autostart_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
    let _ = AppendMenuW(menu, flags, 3, w!("Start with Windows"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, 4, w!("Quit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
    cmd.0 as u32
}

unsafe fn bring_to_front(hwnd: HWND) {
    if IsIconic(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    } else {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    let _ = SetForegroundWindow(hwnd);
}

pub fn parse_hotkey(text: &str) -> Option<(u32, u32)> {
    let parts: Vec<String> = text.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
    let (key, mods) = parts.split_last()?;
    let mut m = MOD_NOREPEAT.0;
    for p in mods {
        m |= match p.as_str() {
            "alt" => MOD_ALT.0,
            "ctrl" | "control" => MOD_CONTROL.0,
            "shift" => MOD_SHIFT.0,
            "win" => MOD_WIN.0,
            _ => return None,
        };
    }
    let vk = if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        0x6F + n
    } else {
        match key.as_str() {
            "space" => 0x20,
            "tab" => 0x09,
            "insert" => 0x2D,
            "home" => 0x24,
            "end" => 0x23,
            "pageup" => 0x21,
            "pagedown" => 0x22,
            "`" => 0xC0,
            k if k.len() == 1 => k.to_ascii_uppercase().chars().next()? as u32,
            _ => return None,
        }
    };
    Some((m, vk))
}

// ------------------------------------------------------------------ drawing helpers

fn rt_props() -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    }
}

fn dib_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
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
    }
}

fn color(rgb: [u8; 3], a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgb[0] as f32 / 255.0, g: rgb[1] as f32 / 255.0, b: rgb[2] as f32 / 255.0, a }
}

const BG: [u8; 3] = [0x10, 0x14, 0x1b];
const BG2: [u8; 3] = [0x0a, 0x0d, 0x12];
const FG: [u8; 3] = [0xe8, 0xec, 0xf1];
const DIM: [u8; 3] = [0x8b, 0x95, 0xa3];

struct Text {
    layout: IDWriteTextLayout,
    w: f32,
    h: f32,
}

struct Painter {
    rt: ID2D1DCRenderTarget,
    dw: IDWriteFactory,
    formats: HashMap<(u32, i32), IDWriteTextFormat>,
}

impl Painter {
    unsafe fn text(&mut self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT) -> Text {
        let key = ((size * 10.0) as u32, weight.0);
        if !self.formats.contains_key(&key) {
            let f = self
                .dw
                .CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
                .expect("text format");
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            self.formats.insert(key, f);
        }
        let wide: Vec<u16> = s.encode_utf16().collect();
        let layout = self.dw.CreateTextLayout(&wide, &self.formats[&key], 4000.0, 1000.0).expect("text layout");
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = layout.GetMetrics(&mut m);
        Text { layout, w: m.widthIncludingTrailingWhitespace, h: m.height }
    }

    unsafe fn brush(&self, rgb: [u8; 3], a: f32) -> ID2D1SolidColorBrush {
        self.rt.CreateSolidColorBrush(&color(rgb, a), None).expect("brush")
    }

    unsafe fn fill(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: [u8; 3], a: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r };
        self.rt.FillRoundedRectangle(&rr, &self.brush(rgb, a));
    }

    unsafe fn stroke(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: [u8; 3], width: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r };
        self.rt.DrawRoundedRectangle(&rr, &self.brush(rgb, 1.0), width, None);
    }

    unsafe fn draw(&self, t: &Text, x: f32, y: f32, rgb: [u8; 3]) {
        self.rt.DrawTextLayout(D2D_POINT_2F { x, y }, &t.layout, &self.brush(rgb, 1.0), D2D1_DRAW_TEXT_OPTIONS_NONE);
    }
}

// ------------------------------------------------------------------ layered windows

struct Layer {
    hwnd: HWND,
    visible: bool,
    rect: (i32, i32, i32, i32),
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    if msg == WM_APP_TRAY {
        let _ = PostThreadMessageW(GetCurrentThreadId(), WM_APP_TRAY_FWD, wp, lp);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

struct Overlay {
    cfg: Config,
    hinst: HINSTANCE,
    painter: Painter,
    badges: Vec<Layer>,
    panel: Option<Layer>,
    panel_content: Option<PanelContent>,
    panel_shown: bool,
    dpi_scale: f32,
    relics: RelicRef,
    relic_layer: Option<Layer>,
    relic_key: String,
}

impl Overlay {
    unsafe fn new_layer(&self) -> Layer {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("AramOverlayLayer"),
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            HWND::default(),
            HMENU::default(),
            self.hinst,
            None,
        )
        .expect("overlay window");
        if self.cfg.hide_from_capture {
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }
        Layer { hwnd, visible: false, rect: (0, 0, 0, 0) }
    }

    /// Render `draw` into a w x h premultiplied bitmap and push it to the layered window.
    unsafe fn present(painter: &mut Painter, layer: &mut Layer, x: i32, y: i32, w: i32, h: i32, draw: impl FnOnce(&mut Painter)) {
        let (w, h) = (w.max(1), h.max(1));
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        let info = dib_info(w, h);
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(HWND::default(), screen);
            return;
        };
        let old = SelectObject(mem, dib);
        let rect = RECT { left: 0, top: 0, right: w, bottom: h };
        if painter.rt.BindDC(mem, &rect).is_ok() {
            painter.rt.BeginDraw();
            painter.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            draw(painter);
            let _ = painter.rt.EndDraw(None, None);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                layer.hwnd,
                screen,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: w, cy: h }),
                mem,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            layer.rect = (x, y, w, h);
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
    }

    unsafe fn show(layer: &mut Layer, on: bool) {
        if on {
            let _ = ShowWindow(layer.hwnd, SW_SHOWNOACTIVATE);
            Self::raise(layer);
        } else {
            let _ = ShowWindow(layer.hwnd, SW_HIDE);
        }
        layer.visible = on;
    }

    unsafe fn raise(layer: &Layer) {
        let _ = SetWindowPos(layer.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    // ---- tier badges above the augment cards

    unsafe fn set_badges(&mut self, badges: &[Badge]) {
        while self.badges.len() < badges.len() {
            let layer = self.new_layer();
            self.badges.push(layer);
        }
        for (i, layer) in self.badges.iter_mut().enumerate() {
            let Some(b) = badges.get(i) else {
                if layer.visible {
                    Self::show(layer, false);
                }
                continue;
            };
            let p = &mut self.painter;
            let s = (b.h * 0.85).clamp(14.0, 44.0) * self.cfg.badge_scale;
            let pad = s * 0.32;
            let tier_t = p.text(b.tier.label(), s, DWRITE_FONT_WEIGHT_BOLD);
            let mut detail = if b.wr.is_finite() { format!("{:.1}%", b.wr * 100.0) } else { "no data".into() };
            if let Some(r) = b.round {
                detail = format!("R{r}  {detail}");
            }
            if b.global {
                detail.push_str(" (all)");
            }
            let detail_t = p.text(&detail, s * 0.78, DWRITE_FONT_WEIGHT_SEMI_BOLD);
            let tag_t = b.rank.map(|(r, of)| p.text(&format!("{} / {of}", ordinal(r)), s * 0.62, DWRITE_FONT_WEIGHT_BOLD));
            let fit_t = b.fit.as_ref().map(|(f, cats)| {
                let head = match f {
                    Fit::Fits => "✓ FITS BUILD",
                    Fit::Off => "✗ OFF-BUILD",
                    Fit::Any => "• ANY BUILD",
                };
                (*f, p.text(&format!("{head} · {}", cats.join(" / ")), s * 0.58, DWRITE_FONT_WEIGHT_BOLD))
            });
            let row1 = s * 1.45;
            let tier_w = (tier_t.w + 2.0 * pad).max(s * 1.35);
            let detail_w = detail_t.w + 2.0 * pad;
            let tag_w = tag_t.as_ref().map(|t| t.w + 1.6 * pad).unwrap_or(0.0);
            let row2 = fit_t.as_ref().map(|(_, t)| t.h + pad * 0.7).unwrap_or(0.0);
            let inner_w = (tier_w + detail_w + tag_w + if tag_w > 0.0 { pad * 0.5 } else { 0.0 })
                .max(fit_t.as_ref().map(|(_, t)| t.w + 2.0 * pad).unwrap_or(0.0));
            let (w, h) = ((inner_w + 6.0).ceil(), (row1 + row2 + 6.0).ceil());
            let x = (b.x + b.w / 2.0 - w / 2.0) as i32;
            let y = (b.y - h - b.h * 0.5) as i32;
            let tier = b.tier;
            // the single best / worst augment of the whole set gets a coloured frame
            let border = match b.rank {
                Some((1, _)) => [0x4a, 0xde, 0x80],
                Some((r, of)) if r == of && of > 1 => [0xf8, 0x71, 0x71],
                _ => tier.rgb(),
            };
            let tag = b.rank.map(|(r, of)| rank_band(r, of));
            Self::present(p, layer, x, y, w as i32, h as i32, |p| {
                p.fill(0.5, 0.5, w - 1.0, h - 1.0, 6.0, BG, 0.95);
                p.fill(3.0, 3.0, tier_w, row1, 4.0, tier.rgb(), 1.0);
                p.draw(&tier_t, 3.0 + (tier_w - tier_t.w) / 2.0, 3.0 + (row1 - tier_t.h) / 2.0, tier.text_rgb());
                p.draw(&detail_t, 3.0 + tier_w + pad, 3.0 + (row1 - detail_t.h) / 2.0, FG);
                if let (Some(t), Some(tag)) = (&tag_t, tag) {
                    let bx = w - 3.0 - tag_w;
                    let bh = row1 * 0.66;
                    let rgb = match tag {
                        1 => [0x1f, 0x7a, 0x45],
                        -1 => [0x9b, 0x2c, 0x2c],
                        _ => [0x3a, 0x41, 0x50],
                    };
                    p.fill(bx, 3.0 + (row1 - bh) / 2.0, tag_w, bh, 3.0, rgb, 1.0);
                    p.draw(t, bx + (tag_w - t.w) / 2.0, 3.0 + (row1 - t.h) / 2.0, [0xff, 0xff, 0xff]);
                }
                if let Some((f, t)) = &fit_t {
                    p.fill(3.0, 3.0 + row1, w - 6.0, row2, 3.0, BG2, 1.0);
                    let rgb = match f {
                        Fit::Fits => [0x4a, 0xde, 0x80],
                        Fit::Off => [0xfb, 0x92, 0x3c],
                        Fit::Any => DIM,
                    };
                    p.draw(t, 3.0 + pad, 3.0 + row1 + (row2 - t.h) / 2.0, rgb);
                }
                p.stroke(1.0, 1.0, w - 2.0, h - 2.0, 6.0, border, 2.0);
            });
            if !layer.visible {
                Self::show(layer, true);
            }
        }
    }

    // ---- the F8 panel

    unsafe fn render_panel(&mut self) {
        let Some(c) = self.panel_content.clone() else {
            if let Some(l) = self.panel.as_mut() {
                Self::show(l, false);
            }
            crate::scan::OWN_WINDOWS.lock().unwrap().clear();
            return;
        };
        if self.panel.is_none() {
            self.panel = Some(self.new_layer());
        }
        let p = &mut self.painter;
        let s = 15.0 * self.dpi_scale * self.cfg.panel_scale;
        let pad = s;
        let title = p.text(&c.title, s * 1.45, DWRITE_FONT_WEIGHT_BOLD);
        let sub = p.text(&c.subtitle, s * 0.85, DWRITE_FONT_WEIGHT_NORMAL);
        let row_h = s * 1.55;
        struct ColT {
            head: Text,
            color: [u8; 3],
            rows: Vec<(Tier, Text, Text, Text)>,
            avoid: Vec<Text>,
            w: f32,
            name_w: f32,
        }
        let mut cols = vec![];
        for col in &c.cols {
            let head = p.text(&col.title.to_uppercase(), s * 0.82, DWRITE_FONT_WEIGHT_BOLD);
            let rows: Vec<_> = col
                .rows
                .iter()
                .map(|(t, n, wr)| {
                    (
                        *t,
                        p.text(t.label(), s, DWRITE_FONT_WEIGHT_BOLD),
                        p.text(n, s, DWRITE_FONT_WEIGHT_NORMAL),
                        p.text(&format!("{:.1}%", wr * 100.0), s * 0.85, DWRITE_FONT_WEIGHT_NORMAL),
                    )
                })
                .collect();
            let avoid: Vec<_> = col
                .avoid
                .iter()
                .map(|(t, n)| p.text(&format!("✗ {}  {n}", t.label()), s * 0.8, DWRITE_FONT_WEIGHT_NORMAL))
                .collect();
            let name_w = rows.iter().map(|r| r.2.w).fold(0.0, f32::max);
            let wr_w = rows.iter().map(|r| r.3.w).fold(0.0, f32::max);
            let w = (s * 1.9 + name_w + s * 0.8 + wr_w).max(avoid.iter().map(|a| a.w).fold(head.w, f32::max));
            cols.push(ColT { head, color: col.color, rows, avoid, w, name_w });
        }
        let builds: Vec<_> = c
            .builds
            .iter()
            .map(|(k, v)| (p.text(&format!("{k}:"), s * 0.85, DWRITE_FONT_WEIGHT_NORMAL), p.text(v, s * 0.85, DWRITE_FONT_WEIGHT_NORMAL)))
            .collect();
        let label_w = builds.iter().map(|b| b.0.w).fold(0.0, f32::max);
        let footer = p.text(&c.footer, s * 0.75, DWRITE_FONT_WEIGHT_NORMAL);
        let gap = s * 1.3;
        let cols_w: f32 = cols.iter().map(|c| c.w).sum::<f32>() + gap * (cols.len().saturating_sub(1)) as f32;
        let builds_w = builds.iter().map(|b| label_w + s * 0.5 + b.1.w).fold(0.0, f32::max);
        let max_rows = cols.iter().map(|c| c.rows.len()).max().unwrap_or(0) as f32;
        let max_avoid = cols.iter().map(|c| c.avoid.len()).max().unwrap_or(0) as f32;
        let inner_w = cols_w.max(title.w + s + sub.w).max(builds_w).max(footer.w);
        let w = (inner_w + 2.0 * pad).ceil();
        let h = (pad + title.h + s * 0.5 + s * 1.3 + max_rows * row_h + max_avoid * s * 1.15 + s * 0.6
            + builds.len() as f32 * s * 1.35
            + s * 0.5
            + footer.h
            + pad)
            .ceil();
        let (sw, _) = (GetSystemMetrics(SM_CXSCREEN), 0);
        let x = if self.cfg.panel_x < 0 { sw - w as i32 + self.cfg.panel_x } else { self.cfg.panel_x };
        let y = (self.cfg.panel_y as f32 * self.dpi_scale) as i32;
        let layer = self.panel.as_mut().unwrap();
        Self::present(p, layer, x, y, w as i32, h as i32, |p| {
            p.fill(0.5, 0.5, w - 1.0, h - 1.0, 8.0, BG, 0.94);
            p.stroke(0.5, 0.5, w - 1.0, h - 1.0, 8.0, [0x2a, 0x33, 0x40], 1.0);
            let mut y = pad;
            p.draw(&title, pad, y, FG);
            p.draw(&sub, pad + title.w + s, y + title.h - sub.h - s * 0.1, DIM);
            y += title.h + s * 0.5;
            let mut x = pad;
            for col in &cols {
                p.draw(&col.head, x, y, col.color);
                let mut ry = y + s * 1.3;
                for (tier, tt, nt, wt) in &col.rows {
                    p.draw(tt, x, ry, tier.rgb());
                    p.draw(nt, x + s * 1.9, ry, FG);
                    p.draw(wt, x + col.w - wt.w, ry + (nt.h - wt.h) / 2.0, DIM);
                    ry += row_h;
                }
                ry += s * 0.2;
                for a in &col.avoid {
                    p.draw(a, x, ry, [0xc0, 0x6c, 0x6c]);
                    ry += s * 1.15;
                }
                let _ = col.name_w;
                x += col.w + gap;
            }
            y += s * 1.3 + max_rows * row_h + max_avoid * s * 1.15 + s * 0.6;
            for (k, v) in &builds {
                p.draw(k, pad, y, DIM);
                p.draw(v, pad + label_w + s * 0.5, y, FG);
                y += s * 1.35;
            }
            y += s * 0.5;
            p.draw(&footer, pad, y, DIM);
        });
        let shown = self.panel_shown;
        let layer = self.panel.as_mut().unwrap();
        if layer.visible != shown {
            Self::show(layer, shown);
        }
        let mut own = crate::scan::OWN_WINDOWS.lock().unwrap();
        own.clear();
        if shown {
            let (x, y, w, h) = layer.rect;
            own.push((x as f32, y as f32, w as f32, h as f32));
        }
    }

    // ---- heal relic timers: on the minimap over each relic, or a bar at the top

    unsafe fn render_relics(&mut self) {
        let c = *self.relics.lock().unwrap();
        if !c.active {
            if let Some(l) = self.relic_layer.as_mut() {
                if l.visible {
                    Self::show(l, false);
                }
            }
            self.relic_key.clear();
            return;
        }
        if self.cfg.relic_on_minimap {
            if let Some(m) = self.cfg.relic_minimap.map(|m| (m[0], m[1], m[2])).or(c.minimap) {
                return self.render_relics_minimap(&c, m);
            }
        }
        let now = c.now();
        let cells: Vec<(String, f64)> = (0..4)
            .map(|i| {
                let rem = c.ready_at[i] - now;
                if rem <= 0.0 {
                    ("UP".to_string(), rem)
                } else {
                    let s = rem.ceil() as i64;
                    (format!("{}:{:02}", s / 60, s % 60), rem)
                }
            })
            .collect();
        let key = format!("{:?}{:?}", cells.iter().map(|c| &c.0).collect::<Vec<_>>(), c.my_side_blue);
        if key == self.relic_key && self.relic_layer.as_ref().is_some_and(|l| l.visible) {
            return; // nothing visible changed: no redraw
        }
        self.relic_key = key;
        if self.relic_layer.is_none() {
            self.relic_layer = Some(self.new_layer());
        }
        let p = &mut self.painter;
        let s = 15.0 * self.dpi_scale * self.cfg.panel_scale;
        let (cell_w, cell_h, gap, mid) = (s * 4.0, s * 2.1, s * 0.3, s * 0.9);
        let marker_h = if c.my_side_blue.is_some() { s * 1.1 } else { 0.0 };
        let w = (4.0 * cell_w + 2.0 * gap + mid + 4.0).ceil();
        let h = (cell_h + marker_h + 4.0).ceil();
        let texts: Vec<Text> = cells
            .iter()
            .map(|(t, _)| p.text(t, s * 1.05, if t == "UP" { DWRITE_FONT_WEIGHT_BOLD } else { DWRITE_FONT_WEIGHT_SEMI_BOLD }))
            .collect();
        let plus = p.text("+", s * 1.1, DWRITE_FONT_WEIGHT_BLACK);
        let you = p.text("YOUR SIDE", s * 0.62, DWRITE_FONT_WEIGHT_BOLD);
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let x = sw / 2 - (w as i32) / 2 + (self.cfg.relic_x as f32 * self.dpi_scale) as i32;
        let y = (self.cfg.relic_y as f32 * self.dpi_scale) as i32;
        let layer = self.relic_layer.as_mut().unwrap();
        Self::present(p, layer, x, y, w as i32, h as i32, |p| {
            let mut cx = 2.0;
            for i in 0..4 {
                if i == 2 {
                    cx += mid - gap;
                }
                let blue = i < 2;
                let side = if blue { [0x3a, 0xa0, 0xff] } else { [0xff, 0x5a, 0x5a] };
                let (label, rem) = (&cells[i].0, cells[i].1);
                let up = label == "UP";
                p.fill(cx, 2.0, cell_w, cell_h, 5.0, BG, 0.88);
                p.fill(cx + 4.0, 2.0, cell_w - 8.0, 3.0, 1.5, side, 1.0);
                if up {
                    p.stroke(cx + 0.5, 2.5, cell_w - 1.0, cell_h - 1.0, 5.0, [0x4a, 0xde, 0x80], 1.5);
                }
                let green = [0x4a, 0xde, 0x80];
                let plus_rgb = if up { green } else { [0x3a, 0x55, 0x48] };
                let t = &texts[i];
                let content = plus.w + s * 0.25 + t.w;
                let x0 = cx + (cell_w - content) / 2.0;
                p.draw(&plus, x0, 2.0 + (cell_h - plus.h) / 2.0 + s * 0.05, plus_rgb);
                let rgb = if up {
                    green
                } else if rem <= 10.0 {
                    [0xf5, 0xc5, 0x18]
                } else {
                    FG
                };
                p.draw(t, x0 + plus.w + s * 0.25, 2.0 + (cell_h - t.h) / 2.0 + s * 0.1, rgb);
                cx += cell_w + gap;
            }
            if let Some(mine_blue) = c.my_side_blue {
                let pair_w = 2.0 * cell_w + gap;
                let px = if mine_blue { 2.0 } else { 2.0 + pair_w + mid };
                let ty = 2.0 + cell_h + 2.0;
                p.draw(&you, px + (pair_w - you.w) / 2.0, ty, if mine_blue { [0x3a, 0xa0, 0xff] } else { [0xff, 0x5a, 0x5a] });
            }
        });
        let layer = self.relic_layer.as_mut().unwrap();
        if !layer.visible {
            Self::show(layer, true);
        }
    }

    /// A timer over each relic that's down, on the minimap (the game's own "+" icon marks
    /// the ones that are up).
    unsafe fn render_relics_minimap(&mut self, c: &crate::state::RelicClock, (mx, my, ms): (f32, f32, f32)) {
        // relic centres on the Howling Abyss minimap (fraction of the map area), scoreboard
        // order: blue inner, blue outer, red outer, red inner. Measured in game.
        const RELIC_UV: [(f32, f32); 4] = [(0.3748, 0.6895), (0.4627, 0.5925), (0.5912, 0.4659), (0.6941, 0.3818)];
        let now = c.now();
        let labels: Vec<Option<(String, f64)>> = (0..4)
            .map(|i| {
                let rem = c.ready_at[i] - now;
                (rem > 0.0).then(|| {
                    let s = rem.ceil() as i64;
                    (format!("{}:{:02}", s / 60, s % 60), rem)
                })
            })
            .collect();
        let key = format!("mm{:?}{mx}{my}{ms}", labels.iter().map(|l| l.as_ref().map(|l| &l.0)).collect::<Vec<_>>());
        if key == self.relic_key {
            return;
        }
        self.relic_key = key;
        if labels.iter().all(|l| l.is_none()) {
            if let Some(l) = self.relic_layer.as_mut() {
                if l.visible {
                    Self::show(l, false);
                }
            }
            return;
        }
        if self.relic_layer.is_none() {
            self.relic_layer = Some(self.new_layer());
        }
        let p = &mut self.painter;
        let s = (ms * 0.042).clamp(11.0, 28.0) * self.cfg.panel_scale;
        let texts: Vec<Option<(Text, f64)>> =
            labels.iter().map(|l| l.as_ref().map(|(t, rem)| (p.text(t, s, DWRITE_FONT_WEIGHT_BOLD), *rem))).collect();
        let size = ms.round() as i32;
        let layer = self.relic_layer.as_mut().unwrap();
        Self::present(p, layer, mx.round() as i32, my.round() as i32, size, size, |p| {
            for (i, t) in texts.iter().enumerate() {
                let Some((t, rem)) = t else { continue };
                let (cx, cy) = (RELIC_UV[i].0 * ms, RELIC_UV[i].1 * ms);
                let (bw, bh) = (t.w + s * 0.55, t.h + s * 0.2);
                let (bx, by) = ((cx - bw / 2.0).clamp(1.0, ms - bw - 1.0), (cy - bh / 2.0).clamp(1.0, ms - bh - 1.0));
                let side = if i < 2 { [0x3a, 0xa0, 0xff] } else { [0xff, 0x5a, 0x5a] };
                p.fill(bx, by, bw, bh, bh * 0.3, BG, 0.92);
                p.stroke(bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, bh * 0.3, side, 1.5);
                let rgb = if *rem <= 10.0 { [0xf5, 0xc5, 0x18] } else { FG };
                p.draw(t, bx + (bw - t.w) / 2.0, by + (bh - t.h) / 2.0, rgb);
            }
        });
        if !layer.visible {
            Self::show(layer, true);
        }
    }

    /// Hotkey: relic `i` was just taken (press again within 3 s to undo).
    fn mark_relic(&mut self, i: usize) {
        let mut c = self.relics.lock().unwrap();
        if !c.active {
            return;
        }
        let now = c.now();
        if c.marked[i].is_some_and(|t| t.elapsed().as_secs_f64() < 3.0) {
            c.ready_at[i] = c.before_mark[i];
            c.marked[i] = None;
            log::info!("relic {i}: mark undone");
        } else {
            c.before_mark[i] = c.ready_at[i];
            c.ready_at[i] = now + self.cfg.relic_beam_delay + self.cfg.relic_respawn;
            c.marked[i] = Some(std::time::Instant::now());
            log::info!("relic {i}: taken (hotkey)");
        }
        drop(c);
        self.relic_key.clear();
        unsafe { self.render_relics() };
    }

    unsafe fn handle(&mut self, cmd: OverlayCmd) {
        match cmd {
            OverlayCmd::Badges(b) => self.set_badges(&b),
            OverlayCmd::Panel(c) => {
                self.panel_content = c;
                self.render_panel();
            }
            OverlayCmd::PanelVisible(v) => {
                self.panel_shown = v.unwrap_or(!self.panel_shown);
                if self.panel_content.is_none() && self.panel_shown {
                    self.panel_content = Some(PanelContent {
                        title: "ARAM Mayhem".into(),
                        subtitle: "waiting for a Mayhem game".into(),
                        footer: format!("{} hide · {} companion", self.cfg.panel_hotkey, self.cfg.companion_hotkey),
                        ..Default::default()
                    });
                }
                self.render_panel();
            }
        }
    }
}

unsafe fn run(cfg: Config, rx: Receiver<OverlayCmd>, tid: Arc<AtomicU32>, relics: RelicRef) -> windows::core::Result<()> {
    // make sure this thread has a message queue before publishing its id
    let mut msg = MSG::default();
    let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
    tid.store(GetCurrentThreadId(), Ordering::Release);

    let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        lpszClassName: w!("AramOverlayLayer"),
        ..Default::default()
    };
    RegisterClassW(&class);

    let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
    let rt = d2d.CreateDCRenderTarget(&rt_props())?;
    rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

    let dpi_scale = GetDpiForSystem() as f32 / 96.0;
    let hotkeys = [
        (1, cfg.panel_hotkey.clone()),
        (2, cfg.companion_hotkey.clone()),
        (3, cfg.quit_hotkey.clone()),
    ];
    let relic_keys: Vec<(i32, String)> =
        cfg.relic_hotkeys.iter().take(4).enumerate().map(|(i, k)| (11 + i as i32, k.clone())).collect();
    for (id, combo) in hotkeys.iter().chain(relic_keys.iter()) {
        match parse_hotkey(combo) {
            Some((m, vk)) => {
                if RegisterHotKey(HWND::default(), *id, HOT_KEY_MODIFIERS(m), vk).is_err() {
                    log::error!("hotkey {combo} is taken by another app; change it in config.json");
                }
            }
            None => log::error!("can't parse hotkey {combo}"),
        }
    }
    // thread timers (no window) get IDs assigned by Windows: keep the ones it returns
    let raise_timer = SetTimer(HWND::default(), 0, 1500, None);
    let relic_timer = SetTimer(HWND::default(), 0, 200, None); // relic tick; redraws only when a digit changes
    let tray = add_tray(hinst);

    let mut ov = Overlay {
        cfg,
        hinst,
        painter: Painter { rt, dw, formats: HashMap::new() },
        badges: vec![],
        panel: None,
        panel_content: None,
        panel_shown: false,
        dpi_scale,
        relics,
        relic_layer: None,
        relic_key: String::new(),
    };
    while let Ok(cmd) = rx.try_recv() {
        ov.handle(cmd);
    }
    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
        match msg.message {
            WM_APP_WAKE => {
                while let Ok(cmd) = rx.try_recv() {
                    ov.handle(cmd);
                }
            }
            WM_HOTKEY => match msg.wParam.0 {
                1 => ov.handle(OverlayCmd::PanelVisible(None)),
                2 => toggle_companion(),
                3 => quit(),
                id @ 11..=14 => ov.mark_relic(id - 11),
                _ => {}
            },
            WM_APP_TRAY_FWD => match (msg.lParam.0 as u32) & 0xffff {
                WM_LBUTTONUP => ensure_companion(),
                WM_RBUTTONUP => {
                    let cmd = tray.map(|h| tray_menu(h)).unwrap_or(0);
                    match cmd {
                        1 => ensure_companion(),
                        2 => ov.handle(OverlayCmd::PanelVisible(None)),
                        3 => set_autostart(!autostart_enabled()),
                        4 => quit(),
                        _ => {}
                    }
                }
                _ => {}
            },
            WM_TIMER if msg.wParam.0 == relic_timer => ov.render_relics(),
            WM_TIMER if msg.wParam.0 == raise_timer => {
                // borderless games can push themselves above us; reassert topmost
                for l in ov.badges.iter().chain(ov.panel.iter()).chain(ov.relic_layer.iter()) {
                    if l.visible {
                        Overlay::raise(l);
                    }
                }
            }
            _ => {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    Ok(())
}
