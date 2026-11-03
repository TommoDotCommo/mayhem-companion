//! ARAM Mayhem: live augment tier badges in game + a companion app for champ select
//! and build browsing. One native Windows executable.
//!
//!   aram-mayhem.exe                   normal use
//!   aram-mayhem.exe --demo Yasuo [N]  fake champ select + scan cards on screen as pick N
//!   aram-mayhem.exe --scan Yasuo      read the screen once, log what it sees, exit

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod config;
mod data;
mod engine;
mod ipc;
mod lcu;
mod logger;
mod model;
mod net;
mod overlay;
mod scan;
mod state;
mod ui;

use config::Config;
use state::{ScanCtl, Shared};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

/// Start another copy of this exe (the companion window or the background service).
pub fn spawn_self(args: &[&str]) {
    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(args).spawn()) {
        Ok(_) => log::info!("started {args:?}"),
        Err(e) => log::error!("couldn't start {args:?}: {e}"),
    }
}

/// True if this is the first process holding `name`.
fn single_instance(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = CreateMutexW(None, false, PCWSTR(wide.as_ptr()));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn main() {
    // physical pixels everywhere: capture coordinates, overlay placement and the UI agree
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    logger::init(if mode == "--companion" { "ui " } else { "svc" });
    let cfg = Config::load();
    let data = data::Data::new(&cfg.data_url, cfg.cache_hours);
    match mode {
        "--scan" => scan_once(&cfg, &data, args.get(1).map(String::as_str).unwrap_or("Annie")),
        "--companion" => run_companion(cfg, data, &args),
        "--find-minimap" => find_minimap_in(args.get(1).map(String::as_str).unwrap_or("")),
        _ => run_service(cfg, data, &args),
    }
}

/// The always-on part: client events, imports, card scanning, overlay, tray icon.
fn run_service(cfg: Config, data: Arc<data::Data>, args: &[String]) {
    let background = args.iter().any(|a| a == "--background");
    if !single_instance("AramMayhem.Service") {
        // already running: a double-click means "show me the window"
        if !background {
            overlay::ensure_companion();
        }
        return;
    }
    log::info!("service starting ({})", env!("CARGO_PKG_VERSION"));
    let shared = Arc::new(RwLock::new(Shared::default()));
    let scan = Arc::new(Mutex::new(ScanCtl { max_round: 1, ..Default::default() }));
    let relics: state::RelicRef = Arc::new(Mutex::new(state::RelicClock::default()));
    let overlay = overlay::spawn(cfg.clone(), relics.clone());
    if cfg.scan_cards {
        scan::spawn(cfg.clone(), scan.clone(), shared.clone(), relics.clone(), overlay.clone());
    } else {
        scan.lock().unwrap().error = Some("card scanning disabled in config".into());
    }
    {
        let d = data.clone();
        std::thread::spawn(move || {
            if let Err(e) = d.meta() {
                log::error!("data: {e}");
            }
            let _ = d.items();
        });
    }
    let (tx, rx) = mpsc::channel();
    ipc::serve(shared.clone(), tx.clone());
    let mut engine = engine::Engine::new(cfg.clone(), data.clone(), shared.clone(), scan.clone(), relics, overlay.clone());
    if args.first().map(String::as_str) == Some("--demo") {
        let champ = args.get(1).cloned().unwrap_or_else(|| "Annie".into());
        let round = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(1);
        if let Err(e) = engine.demo(&champ, round) {
            log::error!("demo: {e}");
        }
        std::thread::spawn(move || for _ in rx {});
    } else {
        lcu::spawn(cfg.league_dir.clone(), tx);
        std::thread::Builder::new().name("engine".into()).spawn(move || engine.run(rx)).expect("spawn engine");
    }
    if !background {
        spawn_self(&["--companion"]);
    }
    loop {
        std::thread::park(); // quitting happens from the tray / hotkey
    }
}

/// The companion window: its own process so its GPU memory goes away when it closes.
fn run_companion(cfg: Config, data: Arc<data::Data>, args: &[String]) {
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let (tab, select) = (arg("--tab"), arg("--select"));
    if !single_instance("AramMayhem.Companion") {
        overlay::focus_companion();
        return;
    }
    let shared = Arc::new(RwLock::new(Shared::default()));
    let (tx, rx) = mpsc::channel();
    ipc::connect(shared.clone(), rx);
    {
        let d = data.clone();
        std::thread::spawn(move || {
            let _ = d.meta();
            state::repaint();
        });
    }
    let icon = overlay::render_icon(64).map(|bgra| {
        let mut rgba = Vec::with_capacity(bgra.len());
        for px in bgra.chunks_exact(4) {
            let a = px[3] as u32;
            let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a / 2) / a).min(255) as u8 };
            rgba.extend_from_slice(&[un(px[2]), un(px[1]), un(px[0]), px[3]]);
        }
        Arc::new(egui::IconData { rgba, width: 64, height: 64 })
    });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(overlay::COMPANION_TITLE)
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([980.0, 620.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, vsync: true, ..Default::default() };
    let result = eframe::run_native(
        overlay::COMPANION_TITLE,
        options,
        Box::new(move |cc| {
            let _ = state::EGUI_CTX.set(cc.egui_ctx.clone());
            ui::setup(&cc.egui_ctx);
            let mut app = ui::App::new(cfg, data, shared, tx);
            app.open(tab.as_deref(), select.as_deref());
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = result {
        log::error!("window: {e}");
    }
    std::process::exit(0);
}

/// One full sweep of the screen; logs every OCR line and the matched cards.
fn scan_once(cfg: &Config, data: &Arc<data::Data>, champ: &str) {
    let run = || -> Result<(), String> {
        let meta = data.meta()?;
        let cid = meta.find_champ(champ).ok_or("unknown champion")?;
        let detail = data.champ(cid)?;
        let ratings = Arc::new(model::champ_ratings(&meta, &detail, cid, cfg));
        let mut scanner = scan::Scanner::new(cfg)?;
        let t = std::time::Instant::now();
        let mut lines = 0;
        for (x, y, w, h) in scanner.sweep_bands() {
            for words in scanner.reader().read(x, y, w, h)? {
                lines += 1;
                let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
                log::info!("scan  y={:5}  {}", words[0].y as i32, text.join(" "));
            }
        }
        log::info!("full sweep: {lines} lines in {} ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let hits = scanner.scan(&meta, true);
        let locked = t.elapsed();
        let t = std::time::Instant::now();
        let again = scanner.scan(&meta, false);
        let strip = t.elapsed();
        let mut cached = std::time::Duration::ZERO;
        for _ in 0..20 {
            let t = std::time::Instant::now();
            let _ = scanner.scan(&meta, false);
            cached += t.elapsed();
        }
        let cached = cached / 20;
        // a live game repaints every frame: measure a forced strip copy + fingerprint (no OCR)
        let forced = scanner.reader().bench_copy(40);
        // relic timers off the scoreboard (as if Tab were held)
        let relics: state::RelicRef = Arc::new(Mutex::new(state::RelicClock::default()));
        {
            let mut r = relics.lock().unwrap();
            r.start(105.0, 150.0);
            r.game_time = 300.0;
            r.at = Some(std::time::Instant::now());
        }
        let t = std::time::Instant::now();
        scanner.relic_sync(&relics);
        scanner.relic_sync(&relics);
        let r = *relics.lock().unwrap();
        let rem: Vec<String> = r.ready_at.iter().map(|t| format!("{:.0}s", t - r.now())).collect();
        log::info!("relics (blue inner, blue outer, red outer, red inner): {rem:?} in {} ms", t.elapsed().as_millis() / 2);
        let ctl = ScanCtl { meta: Some(meta.clone()), ratings, max_round: 4, ..Default::default() };
        for b in scan::make_badges(&hits, &meta, &ctl) {
            log::info!("MATCH {} {} {:.1}% at ({}, {}) rank={:?} fit={:?}", b.name, b.tier.label(), b.wr * 100.0, b.x, b.y, b.rank, b.fit);
        }
        log::info!(
            "timing [{}]: find+lock {} ms, strip scan {} ms ({} hits), repeat scan avg {} us, strip copy avg {} us",
            scanner.reader().backend(),
            locked.as_millis(),
            strip.as_millis(),
            again.len(),
            cached.as_micros(),
            forced.as_micros()
        );
        Ok(())
    };
    if let Err(e) = run() {
        log::error!("scan: {e}");
    }
}

/// Debug: run the minimap finder on a full-screen screenshot (PNG/JPEG) and log the result.
fn find_minimap_in(path: &str) {
    let Ok(img) = image::open(path) else {
        log::error!("find-minimap: can't open {path}");
        return;
    };
    let img = img.to_rgba8();
    let (sw, sh) = (img.width() as i32, img.height() as i32);
    for flipped in [false, true] {
        let side = (sh * 55 / 100).min(sw);
        let x0 = if flipped { 0 } else { sw - side };
        let mut buf = Vec::with_capacity((side * side * 4) as usize);
        for y in sh - side..sh {
            for x in x0..x0 + side {
                let p = img.get_pixel(x as u32, y as u32).0;
                buf.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        let t = std::time::Instant::now();
        let r = scan::find_minimap(&buf, side, sh, flipped);
        let r = r.map(|(x, y, s)| (x + if flipped { 0.0 } else { x0 as f32 }, y + (sh - side) as f32, s));
        log::info!("find-minimap {path} flipped={flipped}: {r:?} in {} us", t.elapsed().as_micros());
    }
}
