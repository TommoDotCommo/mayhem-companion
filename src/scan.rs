//! Augment card detection: GDI strip capture -> Windows.Media.Ocr -> fuzzy name match.
//!
//! Whole-card captures confuse the OCR (it reads the card frames as giant letters),
//! so the screen is read in horizontal strips. Once the titles are found, that strip
//! is remembered per resolution, and unchanged pixels skip OCR entirely.

use crate::config::{data_dir, Config};
use crate::model::{augment_fit, norm, Meta};
use crate::overlay::{OverlayCmd, OverlayHandle};
use crate::state::{repaint, Badge, ScanMode, ScanRef, SharedRef, Tag};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// One OCR word: text + screen rectangle.
#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Cheap fingerprint of a strip (every 7th pixel) to skip OCR when nothing changed.
fn fingerprint(pixels: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for px in pixels.chunks_exact(4).step_by(7) {
        let v = u32::from_le_bytes([px[0], px[1], px[2], 0]) as u64;
        h = (h ^ v).wrapping_mul(0x100_0000_01b3);
    }
    h
}

// ------------------------------------------------------------------ OCR

pub struct Reader {
    engine: OcrEngine,
    capture: crate::capture::Grabber,
    last_print: u64,
    pub screen: (i32, i32),
}

impl Reader {
    pub fn new(language: &str) -> Result<Reader, String> {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let engine = Language::CreateLanguage(&HSTRING::from(language))
            .and_then(|l| OcrEngine::TryCreateFromLanguage(&l))
            .or_else(|_| OcrEngine::TryCreateFromUserProfileLanguages())
            .map_err(|e| format!("Windows OCR unavailable for {language}: {e}"))?;
        let screen = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        Ok(Reader { engine, capture: crate::capture::Grabber::new(), last_print: 0, screen })
    }

    /// Capture a rectangle; returns its fingerprint and lets OCR run on demand.
    fn grab(&mut self, x: i32, y: i32, w: i32, h: i32) -> u64 {
        if self.capture.grab(x, y, w, h) {
            self.last_print = fingerprint(&self.capture.buf);
        }
        self.last_print
    }

    /// Average cost of grabbing + fingerprinting a fresh strip (alternating rects forces a copy).
    pub fn bench_copy(&mut self, n: u32) -> std::time::Duration {
        let (w, h) = (self.screen.0 * 84 / 100, 120);
        let t = std::time::Instant::now();
        for i in 0..n {
            let y = if i % 2 == 0 { 550 } else { 551 };
            self.capture.grab(self.screen.0 * 8 / 100, y, w, h);
            self.last_print = fingerprint(&self.capture.buf);
        }
        t.elapsed() / n
    }

    pub fn backend(&self) -> &'static str {
        self.capture.backend()
    }

    /// OCR the last grabbed rectangle (origin x, y) into lines of words in screen coords.
    fn ocr_last(&mut self, x: i32, y: i32) -> Result<Vec<Vec<Word>>, String> {
        let (w, h) = self.capture.size;
        let buf = CryptographicBuffer::CreateFromByteArray(&self.capture.buf).map_err(|e| e.to_string())?;
        let bmp =
            SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buf, BitmapPixelFormat::Bgra8, w, h, BitmapAlphaMode::Ignore)
                .map_err(|e| e.to_string())?;
        let result = self.engine.RecognizeAsync(&bmp).and_then(|op| op.get()).map_err(|e| e.to_string())?;
        let mut lines = Vec::new();
        for line in result.Lines().map_err(|e| e.to_string())? {
            let mut words = Vec::new();
            for word in line.Words().map_err(|e| e.to_string())? {
                let r = word.BoundingRect().map_err(|e| e.to_string())?;
                let text = word.Text().map(|t| t.to_string_lossy()).unwrap_or_default();
                words.push(Word { text, x: x as f32 + r.X, y: y as f32 + r.Y, w: r.Width, h: r.Height });
            }
            if !words.is_empty() {
                lines.push(words);
            }
        }
        Ok(lines)
    }

    pub fn read(&mut self, x: i32, y: i32, w: i32, h: i32) -> Result<Vec<Vec<Word>>, String> {
        self.grab(x, y, w, h);
        self.ocr_last(x, y)
    }
}

// ------------------------------------------------------------------ matching

#[derive(Clone, Debug)]
pub struct Hit {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Matcher {
    names: HashMap<String, u32>,
    keys: Vec<String>,
}

impl Matcher {
    pub fn new(meta: &Meta) -> Matcher {
        let mut names = HashMap::new();
        for (id, a) in &meta.augs {
            if let (Ok(id), false) = (id.parse::<u32>(), a.name_en.is_empty()) {
                names.entry(norm(&a.name_en)).or_insert(id);
            }
        }
        let keys = names.keys().cloned().collect();
        Matcher { names, keys }
    }

    /// Exact match, or a close one for longer names (OCR misreads a letter or two).
    pub fn lookup(&self, text: &str) -> Option<u32> {
        let key = norm(text);
        if let Some(id) = self.names.get(&key) {
            return Some(*id);
        }
        if key.len() < 7 {
            return None; // short names ("POWER", "BEES!!!") must match exactly
        }
        self.keys
            .iter()
            .filter(|k| k.len().abs_diff(key.len()) <= 3)
            .map(|k| (k, strsim::normalized_levenshtein(k, &key)))
            .filter(|(_, s)| *s >= 0.85)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| self.names[k])
    }

    /// Card titles in OCR output. `ignore` = screen rects of our own windows.
    pub fn find(&self, lines: &[Vec<Word>], ignore: &[(f32, f32, f32, f32)]) -> Vec<Hit> {
        let mut hits: HashMap<u32, Hit> = HashMap::new();
        for words in lines {
            let (cx, cy) = (words[0].x, words[0].y);
            if ignore.iter().any(|(x, y, w, h)| cx >= *x && cx <= x + w && cy >= *y && cy <= y + h) {
                continue;
            }
            let n = words.len();
            // whole line first (fuzzy), then exact sub-spans: OCR can merge neighbouring titles
            let mut spans = vec![(0, n)];
            for i in 0..n {
                for j in i + 1..=(i + 4).min(n) {
                    if (i, j) != (0, n) {
                        spans.push((i, j));
                    }
                }
            }
            let mut used = vec![false; n];
            for (i, j) in spans {
                if used[i..j].iter().any(|u| *u) {
                    continue;
                }
                let text: Vec<&str> = words[i..j].iter().map(|w| w.text.as_str()).collect();
                let text = text.join(" ");
                let id = if (i, j) == (0, n) { self.lookup(&text) } else { self.names.get(&norm(&text)).copied() };
                let Some(id) = id else { continue };
                used[i..j].iter_mut().for_each(|u| *u = true);
                let x0 = words[i..j].iter().map(|w| w.x).fold(f32::MAX, f32::min);
                let y0 = words[i..j].iter().map(|w| w.y).fold(f32::MAX, f32::min);
                let x1 = words[i..j].iter().map(|w| w.x + w.w).fold(f32::MIN, f32::max);
                let y1 = words[i..j].iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max);
                let hit = Hit { id, x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
                if hits.get(&id).map(|h| hit.h > h.h).unwrap_or(true) {
                    hits.insert(id, hit);
                }
            }
        }
        let mut found: Vec<Hit> = hits.into_values().collect();
        if found.len() <= 1 {
            return found;
        }
        // real card titles share a baseline and size: keep the biggest such row
        found.sort_by(|a, b| b.h.total_cmp(&a.h));
        let mut rows: Vec<Vec<Hit>> = Vec::new();
        for f in found {
            match rows.iter_mut().find(|r| {
                let r0 = &r[0];
                (f.y - r0.y).abs() < r0.h * 1.2 && f.h / r0.h > 0.6 && f.h / r0.h < 1.6
            }) {
                Some(row) => row.push(f),
                None => rows.push(vec![f]),
            }
        }
        let mut best = rows.into_iter().max_by_key(|r| r.len()).unwrap_or_default();
        best.truncate(4);
        best.sort_by(|a, b| a.x.total_cmp(&b.x));
        best
    }
}

// ------------------------------------------------------------------ scanner thread

/// Screen rects of our own visible overlay windows (the panel lists augment names).
pub static OWN_WINDOWS: std::sync::Mutex<Vec<(f32, f32, f32, f32)>> = std::sync::Mutex::new(Vec::new());

/// Our windows the OCR must not read: the overlay panel, plus the companion window
/// when it's in front (it lists augment names too). Behind the game it's invisible anyway.
fn own_windows() -> Vec<(f32, f32, f32, f32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId};
    let mut out = OWN_WINDOWS.lock().map(|v| v.clone()).unwrap_or_default();
    unsafe {
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let mut r = RECT::default();
        if pid == std::process::id() && GetWindowRect(fg, &mut r).is_ok() {
            out.push((r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32));
        }
    }
    out
}

struct Layout {
    path: std::path::PathBuf,
}

impl Layout {
    fn key(screen: (i32, i32)) -> String {
        format!("{}x{}", screen.0, screen.1)
    }
    fn load(&self, screen: (i32, i32)) -> Option<(i32, i32)> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let map: HashMap<String, (i32, i32)> = serde_json::from_str(&text).ok()?;
        map.get(&Self::key(screen)).copied()
    }
    fn save(&self, screen: (i32, i32), band: (i32, i32)) {
        let mut map: HashMap<String, (i32, i32)> = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        map.insert(Self::key(screen), band);
        let _ = std::fs::write(&self.path, serde_json::to_string(&map).unwrap_or_default());
    }
}

pub struct Scanner {
    reader: Reader,
    band: Option<(i32, i32)>,
    layout: Layout,
    matcher: Option<(usize, Matcher)>,
    last_print: u64,
    last_found: Vec<Hit>,
}

impl Scanner {
    pub fn new(cfg: &Config) -> Result<Scanner, String> {
        let reader = Reader::new(&cfg.ocr_language)?;
        let layout = Layout { path: data_dir().join("layout.json") };
        let band = layout.load(reader.screen);
        Ok(Scanner { reader, band, layout, matcher: None, last_print: 0, last_found: vec![] })
    }

    fn matcher(&mut self, meta: &std::sync::Arc<Meta>) -> &Matcher {
        let key = std::sync::Arc::as_ptr(meta) as usize;
        if self.matcher.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
            self.matcher = Some((key, Matcher::new(meta)));
        }
        &self.matcher.as_ref().unwrap().1
    }

    fn x_range(&self) -> (i32, i32) {
        let sw = self.reader.screen.0;
        ((sw as f32 * 0.08) as i32, (sw as f32 * 0.84) as i32)
    }

    pub fn sweep_bands(&self) -> Vec<(i32, i32, i32, i32)> {
        let (sw, sh) = self.reader.screen;
        let _ = sw;
        let (x0, w) = self.x_range();
        let (h, step) = ((sh as f32 * 0.10) as i32, (sh as f32 * 0.05) as i32);
        let mut out = vec![];
        let mut y = (sh as f32 * 0.08) as i32;
        while y < (sh as f32 * 0.78) as i32 {
            out.push((x0, y, w, h));
            y += step.max(1);
        }
        out
    }

    /// Scan the remembered strip (cheap) or sweep the screen (only when needed).
    pub fn scan(&mut self, meta: &std::sync::Arc<Meta>, full: bool) -> Vec<Hit> {
        let ignore = own_windows();
        if !full {
            let Some((top, bottom)) = self.band else { return vec![] };
            let (x0, w) = self.x_range();
            let print = self.reader.grab(x0, top, w, bottom - top);
            if print == self.last_print {
                return self.last_found.clone(); // nothing on screen changed: skip OCR
            }
            self.last_print = print;
            let lines = match self.reader.ocr_last(x0, top) {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("ocr: {e}");
                    return vec![];
                }
            };
            let found = self.matcher(meta).find(&lines, &ignore);
            self.last_found = found.clone();
            return found;
        }
        for (x, y, w, h) in self.sweep_bands() {
            let lines = match self.reader.read(x, y, w, h) {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("ocr: {e}");
                    continue;
                }
            };
            let found = self.matcher(meta).find(&lines, &ignore);
            if !found.is_empty() {
                log::debug!("sweep y={y}: {} hits, first at ({}, {}), ignoring {:?}", found.len(), found[0].x, found[0].y, ignore);
            }
            if found.len() >= 2 {
                let top = found.iter().map(|f| f.y).fold(f32::MAX, f32::min);
                let bottom = found.iter().map(|f| f.y + f.h).fold(f32::MIN, f32::max);
                let pad = (bottom - top) * 1.5 + 10.0;
                let band = ((top - pad).max(0.0) as i32, (bottom + pad) as i32);
                if self.band != Some(band) {
                    log::info!("card titles at y={top}; scanning strip {band:?} from now on");
                    self.layout.save(self.reader.screen, band);
                }
                self.band = Some(band);
                self.last_print = 0;
                self.last_found = found.clone();
                return found;
            }
        }
        vec![]
    }

    pub fn has_band(&self) -> bool {
        self.band.is_some()
    }

    pub fn reader(&mut self) -> &mut Reader {
        &mut self.reader
    }
}

/// Rate detected cards for the current champion, pick round and build.
pub fn make_badges(hits: &[Hit], meta: &Meta, ctl: &crate::state::ScanCtl) -> Vec<Badge> {
    let round = ctl.round();
    let mut out: Vec<Badge> = hits
        .iter()
        .map(|h| {
            let info = meta.augs.get(&h.id.to_string());
            let name = info.map(|a| a.name_en.clone()).unwrap_or_default();
            let (rating, used_round, global) = match ctl.ratings.get(&h.id) {
                Some(r) => {
                    let (rt, rd) = r.at_round(Some(round));
                    (Some(rt), rd, r.global)
                }
                None => (None, None, false),
            };
            let fit = info.and_then(|a| augment_fit(&a.cats, &ctl.build)).map(|(f, cats)| {
                (f, cats.iter().map(|c| meta.cat_label(c)).collect::<Vec<_>>())
            });
            let r = rating.unwrap_or(crate::model::Rating {
                tier: crate::model::Tier::B,
                wr: f64::NAN,
                delta: f64::NAN,
                games: 0.0,
            });
            Badge {
                aug_id: h.id,
                name,
                x: h.x,
                y: h.y,
                w: h.w,
                h: h.h,
                tier: r.tier,
                wr: r.wr,
                delta: r.delta,
                round: used_round,
                global,
                tag: None,
                fit,
            }
        })
        .collect();
    let mut ranked: Vec<usize> = (0..out.len()).filter(|i| out[*i].delta.is_finite()).collect();
    ranked.sort_by(|a, b| out[*a].delta.total_cmp(&out[*b].delta));
    if ranked.len() >= 2 {
        let (lo, hi) = (ranked[0], ranked[ranked.len() - 1]);
        if out[hi].delta - out[lo].delta > 0.002 {
            out[hi].tag = Some(Tag::Best);
            out[lo].tag = Some(Tag::Worst);
        }
    }
    out
}

pub fn spawn(cfg: Config, ctl: ScanRef, shared: SharedRef, overlay: OverlayHandle) {
    std::thread::Builder::new()
        .name("scanner".into())
        .spawn(move || {
            // the OCR engine costs ~15 MB: create it when a game needs it, release it after
            let mut scanner: Option<Scanner> = None;
            let mut off_since = Instant::now();
            let mut shown: Vec<(u32, i32, i32)> = vec![];
            let mut misses = 0u32;
            let mut last_full = Instant::now() - Duration::from_secs(60);
            loop {
                let (mode, meta) = {
                    let c = ctl.lock().unwrap();
                    (c.mode, c.meta.clone())
                };
                let interval = match mode {
                    ScanMode::Fast => cfg.scan_interval_fast_ms,
                    ScanMode::Slow => cfg.scan_interval_slow_ms,
                    ScanMode::Off => 0,
                };
                if interval == 0 && scanner.is_some() && off_since.elapsed() > Duration::from_secs(60) {
                    scanner = None;
                    log::info!("released OCR engine");
                }
                if interval > 0 {
                    off_since = Instant::now();
                }
                if interval > 0 && scanner.is_none() && ctl.lock().unwrap().error.is_none() {
                    match Scanner::new(&cfg) {
                        Ok(s) => scanner = Some(s),
                        Err(e) => {
                            log::warn!("card scanning unavailable: {e}");
                            ctl.lock().unwrap().error = Some(e);
                        }
                    }
                }
                let Some(scanner) = scanner.as_mut() else {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                };
                let Some(meta) = meta.filter(|_| interval > 0) else {
                    if !shown.is_empty() {
                        shown.clear();
                        overlay.send(OverlayCmd::Badges(vec![]));
                        shared.write().unwrap().cards.clear();
                        repaint();
                    }
                    std::thread::sleep(Duration::from_millis(400));
                    continue;
                };
                let t0 = Instant::now();
                // a full sweep reads ~13 strips, so only fall back to it now and then
                let full = (!scanner.has_band() && last_full.elapsed() > Duration::from_secs(2))
                    || (misses >= 6 && last_full.elapsed() > Duration::from_secs(3));
                if full {
                    last_full = Instant::now();
                }
                let hits = if full || scanner.has_band() { scanner.scan(&meta, full) } else { vec![] };
                if !hits.is_empty() {
                    misses = 0;
                    let key: Vec<(u32, i32, i32)> =
                        hits.iter().map(|h| (h.id, h.x as i32 / 20, h.y as i32 / 20)).collect();
                    // re-rate when cards change, or when round/build changed under the same cards
                    let badges = {
                        let c = ctl.lock().unwrap();
                        make_badges(&hits, &meta, &c)
                    };
                    let changed = key != shown || {
                        let s = shared.read().unwrap();
                        s.cards.iter().map(|b| (b.tier, b.round)).ne(badges.iter().map(|b| (b.tier, b.round)))
                    };
                    if changed {
                        shown = key;
                        log::info!(
                            "cards: {}",
                            badges.iter().map(|b| format!("{} {}", b.name, b.tier.label())).collect::<Vec<_>>().join(", ")
                        );
                        overlay.send(OverlayCmd::Badges(badges.clone()));
                        shared.write().unwrap().cards = badges;
                        repaint();
                    }
                } else {
                    misses += 1;
                    if !shown.is_empty() && misses >= 2 {
                        shown.clear();
                        let picks = {
                            let mut c = ctl.lock().unwrap();
                            c.picks += 1;
                            c.picks
                        };
                        log::info!("cards closed (pick {picks} made)");
                        overlay.send(OverlayCmd::Badges(vec![]));
                        shared.write().unwrap().cards.clear();
                        repaint();
                    }
                }
                let spent = t0.elapsed();
                std::thread::sleep(Duration::from_millis(interval).saturating_sub(spent).max(Duration::from_millis(30)));
            }
        })
        .expect("spawn scanner");
}
