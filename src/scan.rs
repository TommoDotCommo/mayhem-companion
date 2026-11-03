//! Augment card detection: GDI strip capture -> Windows.Media.Ocr -> fuzzy name match.
//!
//! Whole-card captures confuse the OCR (it reads the card frames as giant letters),
//! so the screen is read in horizontal strips. Once the titles are found, that strip
//! is remembered per resolution, and unchanged pixels skip OCR entirely.

use crate::config::{data_dir, Config};
use crate::model::{augment_fit, norm, Meta};
use crate::overlay::{OverlayCmd, OverlayHandle};
use crate::state::{repaint, Badge, RelicRef, ScanMode, ScanRef, SharedRef};
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

/// "1:33" / "1.33" / "0:07" -> seconds.
pub fn parse_clock(text: &str) -> Option<u32> {
    let t = text.trim().trim_end_matches(|c: char| !c.is_ascii_digit());
    let (m, s) = t.split_once([':', '.', ';'])?;
    if m.is_empty() || m.len() > 2 || s.len() != 2 {
        return None;
    }
    let (m, s): (u32, u32) = (m.parse().ok()?, s.parse().ok()?);
    (s < 60 && m < 10).then_some(m * 60 + s)
}

#[cfg(test)]
mod tests {
    use super::parse_clock;
    #[test]
    fn clocks() {
        assert_eq!(parse_clock("2:18"), Some(138));
        assert_eq!(parse_clock("1.33"), Some(93));
        assert_eq!(parse_clock("0:07"), Some(7));
        assert_eq!(parse_clock("12:34"), None); // the game clock, not a relic
        assert_eq!(parse_clock("2.5"), None);
        assert_eq!(parse_clock("Relic"), None);
    }

    #[test]
    fn split_titles_join() {
        use super::{join_split_lines, Word};
        let w = |t: &str, x: f32, y: f32, wd: f32| Word { text: t.into(), x, y, w: wd, h: 20.0 };
        // as OCR returned "Lil' Extra Help" in game, next to another card's title
        let lines = vec![
            vec![w("Extra", 748.0, 582.0, 64.0), w("Help", 819.0, 581.0, 55.0)],
            vec![w("CRIMSON", 1159.0, 582.0, 124.0), w("COBALT", 1291.0, 582.0, 110.0)],
            vec![w("Lil'", 706.0, 581.0, 34.0)],
        ];
        let joined: Vec<String> =
            join_split_lines(&lines).iter().map(|l| l.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect();
        assert_eq!(joined, vec!["Lil' Extra Help", "CRIMSON COBALT"]);
    }

    /// The frame as measured at 2560x1440 (MinimapScale 2.07): gold top bar at y=968,
    /// gold side bar at x=2089, map area 2111..2540 x 990..1418.
    #[test]
    fn minimap_frame() {
        let (sw, sh) = (2560, 1440);
        let side = sh * 55 / 100;
        let (x0, y0) = (sw - side, sh - side);
        for flipped in [false, true] {
            let mut buf = vec![30u8; (side * side * 4) as usize]; // dark blue-grey world
            let mut put = |sx: i32, sy: i32| {
                let x = if flipped { sw - 1 - sx } else { sx } - if flipped { 0 } else { x0 };
                let i = (((sy - y0) * side + x) * 4) as usize;
                buf[i..i + 3].copy_from_slice(&[73, 128, 159]); // BGR gold
            };
            for x in 2089..2560 {
                for y in 967..972 {
                    put(x, y);
                }
            }
            for y in 967..1440 {
                for x in 2089..2094 {
                    put(x, y);
                }
            }
            let (x, y, s) = super::find_minimap(&buf, side, sh, flipped).expect("minimap");
            let screen_x = if flipped { x } else { x0 as f32 + x };
            let expect_x = if flipped { 20.0 } else { 2111.0 };
            assert!((s - 429.0).abs() <= 2.0, "size {s}");
            assert!((screen_x - expect_x).abs() <= 2.0, "x {screen_x}");
            assert!((y0 as f32 + y - 990.0).abs() <= 2.0, "y {}", y0 as f32 + y);
        }
    }
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

    /// Find the minimap's map area from its gold frame (bottom-right, or bottom-left
    /// with FlipMiniMap). Returns (x, y, size) in screen pixels.
    pub fn find_minimap(&mut self, flipped: bool) -> Option<(f32, f32, f32)> {
        let (sw, sh) = self.screen;
        let side = (sh * 55 / 100).min(sw);
        let x0 = if flipped { 0 } else { sw - side };
        self.capture.grab(x0, sh - side, side, side);
        let (mx, my, s) = find_minimap(&self.capture.buf, side, sh, flipped)?;
        let x = if flipped { mx } else { (x0 as f32 + mx).round() };
        Some((x, (sh - side) as f32 + my, s))
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

    /// Grab a rectangle and return its BGRA pixels (no OCR).
    pub fn grab_raw(&mut self, x: i32, y: i32, w: i32, h: i32) -> &[u8] {
        self.capture.grab(x, y, w, h);
        &self.capture.buf
    }

    /// Debug: OCR a band of an image (BGRA) instead of the screen.
    pub fn read_image(&mut self, img: &[u8], img_w: i32, y: i32, h: i32) -> Result<Vec<Vec<Word>>, String> {
        let row = (img_w * 4) as usize;
        self.capture.buf = img[y as usize * row..(y + h) as usize * row].to_vec();
        self.capture.size = (img_w, h);
        self.ocr_last(0, y)
    }

    pub fn read(&mut self, x: i32, y: i32, w: i32, h: i32) -> Result<Vec<Vec<Word>>, String> {
        self.grab(x, y, w, h);
        self.ocr_last(x, y)
    }
}

// ------------------------------------------------------------------ matching

/// OCR sometimes returns one title as two lines ("Lil'" + "Extra Help"): join pieces that
/// sit on the same baseline with a word-sized gap between them.
fn join_split_lines(lines: &[Vec<Word>]) -> Vec<Vec<Word>> {
    let mut sorted: Vec<Vec<Word>> = lines.iter().filter(|l| !l.is_empty()).cloned().collect();
    sorted.sort_by(|a, b| a[0].x.total_cmp(&b[0].x));
    let mut out: Vec<Vec<Word>> = Vec::new();
    for line in sorted {
        let first = &line[0];
        let joined = out.iter_mut().find(|prev| {
            let last = prev.last().unwrap();
            let h = last.h.max(first.h);
            let gap = first.x - (last.x + last.w);
            (first.y - last.y).abs() < h * 0.45 && gap > -h * 0.2 && gap < h * 1.1
        });
        match joined {
            Some(prev) => prev.extend(line),
            None => out.push(line),
        }
    }
    out
}

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
        let lines = join_split_lines(lines);
        let mut hits: HashMap<u32, Hit> = HashMap::new();
        for words in &lines {
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
        self.load_key(&Self::key(screen))
    }
    fn load_key(&self, key: &str) -> Option<(i32, i32)> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let map: HashMap<String, (i32, i32)> = serde_json::from_str(&text).ok()?;
        map.get(key).copied()
    }
    fn save(&self, screen: (i32, i32), band: (i32, i32)) {
        self.save_key(&Self::key(screen), band)
    }
    fn save_key(&self, key: &str, value: (i32, i32)) {
        let mut map: HashMap<String, (i32, i32)> = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        map.insert(key.to_string(), value);
        let _ = std::fs::write(&self.path, serde_json::to_string(&map).unwrap_or_default());
    }
}

pub struct Scanner {
    reader: Reader,
    band: Option<(i32, i32)>,
    relic_band: Option<(i32, i32)>,
    relic_spacing: Option<f32>,
    /// Centre of the relic timer bar (it isn't exactly the screen centre).
    relic_mid: Option<f32>,
    relic_absent: [u8; 4],
    relic_band_misses: u8,
    /// Minimap watch: consecutive reads with the relic's "+" seen / clearly gone, and when
    /// it was first not seen (a champion icon may be covering it).
    relic_seen: [u8; 4],
    relic_gone: [u8; 4],
    relic_left: [Option<f64>; 4],
    /// When the "+" was first clearly gone (not just covered).
    relic_gone_at: [Option<f64>; 4],
    /// The "+" has been seen since this relic was last due: only then can it be picked up.
    relic_was_up: [bool; 4],
    relic_watch_at: Instant,
    /// Pickup times inferred from the minimap, until the scoreboard confirms them.
    relic_pickup_at: [Option<f64>; 4],
    /// Learned from scoreboard reads (Tab): "+" vanishing -> respawn timer start (~0: the
    /// "+" goes when the heal lands), and an icon reaching the relic -> timer start (the beam).
    cal: Calibration,
    respawn: f64,
    /// Minimap frame when each relic's "+" first went missing (for the debug pictures).
    relic_gone_frame: [Option<Vec<u8>>; 4],
    /// When a champion icon first reached the relic, and its closest approach (time, px).
    relic_touch: [Option<f64>; 4],
    relic_closest: [Option<(f64, f32)>; 4],
    /// Last time the "+" was visible, and the last time an icon reached a relic that was up
    /// just before (a likely pickup, recorded at once so a Tab right after can measure it).
    relic_up_at: [f64; 4],
    relic_reach: [Option<f64>; 4],
    debug_shots: u32,
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
        let relic_band = layout.load_key(&format!("relics:{}", Layout::key(reader.screen)));
        let relic_spacing = layout.load_key(&format!("relicgap:{}", Layout::key(reader.screen))).map(|(s, _)| s as f32);
        let relic_mid = layout.load_key(&format!("relicmid:{}", Layout::key(reader.screen))).map(|(m, _)| m as f32);
        Ok(Scanner {
            reader,
            band,
            relic_band,
            relic_spacing,
            relic_mid,
            relic_absent: [0; 4],
            relic_band_misses: 0,
            relic_seen: [0; 4],
            relic_gone: [0; 4],
            relic_left: [None; 4],
            relic_gone_at: [None; 4],
            relic_was_up: [false; 4],
            relic_watch_at: Instant::now(),
            relic_pickup_at: [None; 4],
            cal: Calibration::load(cfg.relic_beam_delay),
            respawn: cfg.relic_respawn,
            relic_gone_frame: Default::default(),
            relic_touch: [None; 4],
            relic_closest: [None; 4],
            relic_up_at: [f64::NEG_INFINITY; 4],
            relic_reach: [None; 4],
            debug_shots: 0,
            layout,
            matcher: None,
            last_print: 0,
            last_found: vec![],
        })
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

    /// Read the relic timers off the scoreboard (while it's open) and sync the clock.
    /// The scoreboard shows "m:ss" per relic still respawning and nothing for ones that are up.
    pub fn relic_sync(&mut self, relics: &RelicRef) {
        let (sw, sh) = self.reader.screen;
        let (x0, w) = ((sw as f32 * 0.30) as i32, (sw as f32 * 0.40) as i32);
        let mut lines = vec![];
        match self.relic_band {
            Some((top, bottom)) => lines.extend(self.reader.read(x0, top, w, bottom - top).unwrap_or_default()),
            None => {
                let (h, step) = ((sh as f32 * 0.08) as i32, (sh as f32 * 0.05) as i32);
                // Mayhem shows them in a bar at the very top of the screen while Tab is held
                let mut y = 0;
                while y < (sh as f32 * 0.60) as i32 {
                    lines.extend(self.reader.read(x0, y, w, h).unwrap_or_default());
                    y += step;
                }
            }
        }
        // time tokens near the screen centre, grouped into rows
        let cx = sw as f32 / 2.0;
        let mut tokens: Vec<(f32, f32, f32, u32)> = lines
            .iter()
            .flatten()
            .filter_map(|w| parse_clock(&w.text).map(|secs| (w.x + w.w / 2.0, w.y, w.h, secs)))
            .filter(|t| (t.0 - cx).abs() < sw as f32 * 0.15)
            .collect();
        if tokens.is_empty() {
            // scoreboard not open, or every relic up. If the remembered strip keeps coming
            // back empty while the key is held, it may be stale: search the screen again.
            if self.relic_band.is_some() {
                self.relic_band_misses += 1;
                if self.relic_band_misses >= 6 {
                    self.relic_band = None;
                    self.relic_band_misses = 0;
                }
            }
            return;
        }
        self.relic_band_misses = 0;
        tokens.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut rows: Vec<Vec<(f32, f32, f32, u32)>> = vec![];
        for t in tokens {
            match rows.iter_mut().find(|r| (r[0].1 - t.1).abs() < t.2.max(4.0)) {
                Some(r) => r.push(t),
                None => rows.push(vec![t]),
            }
        }
        let mut row = rows.into_iter().max_by_key(|r| r.len()).unwrap_or_default();
        row.sort_by(|a, b| a.0.total_cmp(&b.0));
        row.dedup_by(|a, b| (a.0 - b.0).abs() < 4.0);
        if row.len() > 4 {
            return;
        }
        let th = row.iter().map(|t| t.2).fold(0.0, f32::max);
        let key = Layout::key(self.reader.screen);
        if row.len() == 4 {
            let gap = (row[3].0 - row[0].0) / 3.0;
            if self.relic_spacing.map(|s| (s - gap).abs() > 2.0).unwrap_or(true) {
                self.relic_spacing = Some(gap);
                self.layout.save_key(&format!("relicgap:{key}"), (gap.round() as i32, 0));
            }
            let mid = (row[0].0 + row[3].0) / 2.0;
            if self.relic_mid.map(|m| (m - mid).abs() > 2.0).unwrap_or(true) {
                self.relic_mid = Some(mid);
                self.layout.save_key(&format!("relicmid:{key}"), (mid.round() as i32, 0));
            }
        }
        let y = row[0].1;
        let band = ((y - th * 1.5).max(0.0) as i32, (y + th * 2.5) as i32);
        if self.relic_band.map(|b| (b.0 - band.0).abs() > 6).unwrap_or(true) {
            self.relic_band = Some(band);
            self.layout.save_key(&format!("relics:{key}"), band);
            log::info!("relic timers found at y={y}");
        }
        // which slot is each token in? slots sit at centre ± 0.5 and ± 1.5 spacings
        let s = self.relic_spacing.unwrap_or(th * 4.6);
        let mid = self.relic_mid.unwrap_or(cx);
        let slots = [mid - 1.5 * s, mid - 0.5 * s, mid + 0.5 * s, mid + 1.5 * s];
        let mut seen = [None; 4];
        for t in &row {
            let (k, d) = slots.iter().enumerate().map(|(k, sx)| (k, (t.0 - sx).abs())).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
            if d < s * 0.45 {
                seen[k] = Some(t.3);
            }
        }
        let mut c = relics.lock().unwrap();
        if !c.active {
            return;
        }
        let now = c.now();
        for k in 0..4 {
            match seen[k] {
                Some(secs) => {
                    let exact = now + secs as f64 + 0.5;
                    // how far off the timer was (the minimap watch, a hotkey, or the first spawn)
                    if (exact - c.ready_at[k]).abs() > 1.0 {
                        log::info!("relic {k}: scoreboard {secs}s, timer was {:+.1}s off", c.ready_at[k] - exact);
                    }
                    // learn pickup -> timer start from a pickup the minimap saw: a confirmed one,
                    // or an icon reaching the relic just before this new timer
                    let start = exact - self.respawn;
                    if let Some(v) = self.relic_pickup_at[k].take() {
                        let d = start - v;
                        if (-3.0..3.0).contains(&d) {
                            self.cal.offset.add(d);
                            log::info!("relic {k}: + vanished -> timer start {d:+.2}s; using {:+.2}s ({} samples)", self.cal.offset.v, self.cal.offset.n);
                        }
                    }
                    if let Some(t) = self.relic_reach[k].take() {
                        let d = start - t;
                        if (0.0..6.0).contains(&d) {
                            self.cal.beam.add(d);
                            log::info!("relic {k}: icon reached -> timer start {d:.2}s; using {:.2}s ({} samples)", self.cal.beam.v, self.cal.beam.n);
                        }
                    }
                    self.cal.save();
                    c.ready_at[k] = exact;
                    self.relic_absent[k] = 0;
                }
                None => {
                    // no timer in that slot for ~1 s of reads while others were read: it's up
                    // (one or two misses are OCR flicker, not a respawn)
                    self.relic_absent[k] = self.relic_absent[k].saturating_add(1);
                    if self.relic_absent[k] >= 4 && c.ready_at[k] > now {
                        c.ready_at[k] = now;
                    }
                }
            }
        }
        log::debug!("relic sync: {seen:?}");
    }

    /// Watch the relics' "+" icons on the minimap (10x a second, a ~160 px square grab):
    /// an icon that disappears was picked up, one that appears early has respawned.
    pub fn watch_relics(&mut self, relics: &RelicRef, cfg: &Config, dead: bool) {
        if self.relic_watch_at.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.relic_watch_at = Instant::now();
        let (mm, active) = {
            let c = relics.lock().unwrap();
            (cfg.relic_minimap.map(|m| (m[0], m[1], m[2])).or(c.minimap), c.active)
        };
        let Some((mx, my, ms)) = mm else { return };
        if !active || dead || !league_in_front() {
            self.relic_gone = [0; 4];
            self.relic_left = [None; 4];
            self.relic_gone_at = [None; 4];
            return;
        }
        let r = (ms * 0.025).round().max(4.0) as i32;
        // champion icons: a portrait in a team-coloured ring (18 px radius at a 429 px map)
        let ring_r = ms * 0.042;
        // a champion standing on the relic: its icon centre within ~150 units of it
        let pickup_px = ms * 0.012;
        let margin = r + (2.0 * ring_r) as i32 + 3;
        let x0 = (mx + 0.35 * ms) as i32 - margin;
        let y0 = (my + 0.36 * ms) as i32 - margin;
        let (w, h) = ((0.37 * ms) as i32 + 2 * margin, (0.35 * ms) as i32 + 2 * margin);
        let buf = self.reader.grab_raw(x0, y0, w, h).to_vec();
        let mut state = [Icon::Gone; 4];
        let mut icon_d: [Option<f32>; 4] = [None; 4];
        for (i, (u, v)) in crate::state::RELIC_UV.iter().enumerate() {
            let (cx, cy) = ((mx + u * ms) as i32 - x0, (my + v * ms) as i32 - y0);
            let (mut green, mut busy) = (0, 0);
            for y in (cy - r).max(0)..(cy + r + 1).min(h) {
                for x in (cx - r).max(0)..(cx + r + 1).min(w) {
                    let k = ((y * w + x) * 4) as usize;
                    let (b, g, rr) = (buf[k] as i32, buf[k + 1] as i32, buf[k + 2] as i32);
                    if g > 140 && g > rr + 40 && g > b + 40 {
                        green += 1;
                    } else if rr.max(g).max(b) - rr.min(g).min(b) > 60 && rr.max(g).max(b) > 80 {
                        busy += 1; // colourful: minions, a ping...
                    }
                }
            }
            icon_d[i] = nearest_icon(&buf, w, h, cx as f32, cy as f32, r as f32 + ring_r, ring_r);
            let area = (2 * r + 1) * (2 * r + 1);
            state[i] = if green >= 4 {
                Icon::Up
            } else if icon_d[i].is_some() || busy * 6 > area {
                Icon::Covered
            } else {
                Icon::Gone
            };
        }
        let mut c = relics.lock().unwrap();
        let now = c.now();
        let up: Vec<bool> = (0..4).map(|i| c.ready_at[i] <= now).collect();
        // greyed / dimmed screen (death, a menu): every relic that should be up reads as
        // plain gone at once. (Gone next to covered is a normal fight around the relics.)
        if up.iter().filter(|u| **u).count() >= 2 && (0..4).filter(|i| up[*i]).all(|i| state[i] == Icon::Gone) {
            return;
        }
        for i in 0..4 {
            if state[i] == Icon::Up {
                self.relic_up_at[i] = now;
            } else if icon_d[i].is_some_and(|d| d <= pickup_px)
                && now - self.relic_up_at[i] < 3.0
                && self.relic_reach[i].map(|t| now - t > 5.0).unwrap_or(true)
            {
                self.relic_reach[i] = Some(now);
                log::info!("relic {i}: icon reached it at {now:.1}s");
            }
            // the moment a champion icon reaches the relic = the pickup (closest approach
            // if none quite reaches the centre)
            if state[i] != Icon::Up {
                if let Some(d) = icon_d[i] {
                    if self.relic_touch[i].is_none() && d <= pickup_px {
                        self.relic_touch[i] = Some(now);
                    }
                    if self.relic_closest[i].map(|(_, bd)| d < bd).unwrap_or(true) {
                        self.relic_closest[i] = Some((now, d));
                    }
                }
            }
            match state[i] {
                Icon::Up => {
                    self.relic_was_up[i] = true;
                    self.relic_gone[i] = 0;
                    self.relic_left[i] = None;
                    self.relic_gone_at[i] = None;
                    self.relic_touch[i] = None;
                    self.relic_closest[i] = None;
                    self.relic_seen[i] = self.relic_seen[i].saturating_add(1);
                    if !up[i] && self.relic_seen[i] >= 5 && c.ready_at[i] - now > 3.0 {
                        log::info!("relic {i}: up on the minimap ({:.0}s early)", c.ready_at[i] - now);
                        c.ready_at[i] = now;
                        self.relic_pickup_at[i] = None;
                        self.debug_shot(i, "up-early", &buf, w, h);
                    }
                }
                Icon::Covered => {
                    self.relic_seen[i] = 0;
                    if up[i] && self.relic_left[i].is_none() {
                        self.relic_left[i] = Some(now);
                    }
                }
                Icon::Gone => {
                    self.relic_seen[i] = 0;
                    // a pickup is "+" seen, then gone. Never seen (spawning late, out of sight)
                    // or not due yet: nothing to infer.
                    if !up[i] || !self.relic_was_up[i] {
                        if !up[i] {
                            self.relic_was_up[i] = false;
                        }
                        self.relic_gone[i] = 0;
                        self.relic_left[i] = None;
                        self.relic_gone_at[i] = None;
                        self.relic_touch[i] = None;
                        self.relic_closest[i] = None;
                        continue;
                    }
                    if self.relic_gone_at[i].is_none() {
                        self.relic_gone_frame[i] = Some(buf.clone());
                    }
                    let gone_at = *self.relic_gone_at[i].get_or_insert(now);
                    let hidden_at = *self.relic_left[i].get_or_insert(gone_at);
                    self.relic_gone[i] += 1;
                    // gone (and uncovered) for a full second; the time is back-dated below
                    if now - gone_at >= 1.0 && self.relic_gone[i] >= 5 {
                        // The "+" vanishes when the heal lands, which starts the 90 s. Seen
                        // vanishing: that's the moment. Under an icon: when the icon reached
                        // the relic plus the beam, within the covered stretch.
                        let reach = self.relic_reach[i]
                            .or(self.relic_touch[i])
                            .or(self.relic_closest[i].filter(|(_, d)| *d <= pickup_px * 2.5).map(|(t, _)| t))
                            .filter(|t| *t <= gone_at && gone_at - t < 8.0);
                        let (since, how) = if gone_at - hidden_at <= 0.15 {
                            (gone_at, "seen vanishing")
                        } else if let Some(t) = reach {
                            ((t + self.cal.beam.v).clamp(hidden_at, gone_at), "icon reached it + beam")
                        } else {
                            ((hidden_at + gone_at) / 2.0, "covered, mid-point")
                        };
                        c.ready_at[i] = since + self.cal.offset.v + self.respawn;
                        self.relic_pickup_at[i] = Some(since);
                        let first = self.relic_gone_frame[i].take();
                        self.debug_shot(i, "taken-first-gone", first.as_deref().unwrap_or(&buf), w, h);
                        self.debug_shot(i, "taken-confirmed", &buf, w, h);
                        log::info!(
                            "relic {i}: heal landed at {since:.1}s ({how}; covered from {hidden_at:.1}s, gone at {gone_at:.1}s)"
                        );
                        self.relic_gone[i] = 0;
                        self.relic_left[i] = None;
                        self.relic_gone_at[i] = None;
                        self.relic_touch[i] = None;
                        self.relic_closest[i] = None;
                        self.relic_was_up[i] = false;
                    }
                }
            }
        }
    }

    /// Save the watched minimap area as a PNG (relic_debug\), to see what the watch saw.
    fn debug_shot(&mut self, relic: usize, what: &str, bgra: &[u8], w: i32, h: i32) {
        if self.debug_shots >= 80 || bgra.len() < (w * h * 4) as usize {
            return;
        }
        self.debug_shots += 1;
        let dir = data_dir().join("relic_debug");
        let _ = std::fs::create_dir_all(&dir);
        let rgba: Vec<u8> = bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
            let _ = img.save(dir.join(format!("{stamp}_relic{relic}_{what}.png")));
        }
    }

    pub fn has_band(&self) -> bool {
        self.band.is_some()
    }

    pub fn reader(&mut self) -> &mut Reader {
        &mut self.reader
    }
}

/// Minimap frame finder on a BGRA square grabbed from the bottom corner of the screen
/// (`side` px, flush with the screen's bottom and its right edge, or left if `flipped`).
/// The frame's gold top bar and inner side bar give the map's size; the map sits a fixed
/// margin from the screen corner. Returns (x, y, size) within the square.
pub fn find_minimap(buf: &[u8], side: i32, screen_h: i32, flipped: bool) -> Option<(f32, f32, f32)> {
    if buf.len() < (side * side * 4) as usize || side < 100 {
        return None;
    }
    let k = screen_h as f32 / 1440.0;
    // x measured from the screen edge the minimap is on
    let gold = |x: i32, y: i32| {
        let px = if flipped { x } else { side - 1 - x };
        let i = ((y * side + px) * 4) as usize;
        let (b, g, r) = (buf[i] as i32, buf[i + 1] as i32, buf[i + 2] as i32);
        r >= 90 && r - b >= 40 && g - b >= 20 && r >= g
    };
    let min_run = (60.0 * k).max(40.0) as i32;
    let edge = (60.0 * k) as i32;
    // longest gold run along a line, and where it ends (counting from the screen corner side)
    let run = |horizontal: bool, at: i32| {
        let (mut cur, mut best, mut start) = (0, 0, 0);
        for t in 0..side {
            let g = if horizontal { gold(t, at) } else { gold(at, t) };
            cur = if g { cur + 1 } else { 0 };
            if cur > best {
                best = cur;
                start = t + 1 - cur;
            }
        }
        (best, start)
    };
    // top bar: first row (from the top) with a long gold run reaching the screen edge
    let top = (0..side - edge).find(|&y| {
        let (n, start) = run(true, y);
        n >= min_run && start <= edge
    })?;
    // inner side bar: the column furthest from the screen edge with a long gold run
    // reaching the bottom
    let side_col = (0..side).rev().find(|&d| {
        let (n, start) = run(false, d);
        n >= min_run && start + n >= side - edge
    })?;
    // measured at 2560x1440: frame bar to map 22 px; map to screen edge 20 px, to bottom 22 px
    let (frame, margin_side, margin_bottom) = (22.0 * k, 20.0 * k, 22.0 * k);
    let size_x = side_col as f32 + 1.0 - frame - margin_side;
    let size_y = side as f32 - margin_bottom - (top as f32 + frame);
    if size_x < 80.0 || (size_x - size_y).abs() > 8.0 * k + 3.0 {
        return None;
    }
    let s = ((size_x + size_y) / 2.0).round();
    let y = side as f32 - margin_bottom - s;
    let x = if flipped { margin_side } else { side as f32 - margin_side - s };
    Some((x, y, s))
}

/// Rate detected cards for the current champion, pick round and build.
pub fn make_badges(hits: &[Hit], meta: &Meta, ctl: &crate::state::ScanCtl) -> Vec<Badge> {
    let round = ctl.round();
    hits
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
            // rank against every augment of the same rarity (one offer = one rarity)
            let rank = info.filter(|_| r.delta.is_finite()).map(|info| {
                let (mut of, mut above) = (0u32, 0u32);
                for (id, ar) in ctl.ratings.iter() {
                    if meta.augs.get(&id.to_string()).map(|a| a.rarity == info.rarity) != Some(true) {
                        continue;
                    }
                    let d = ar.at_round(Some(round)).0.delta;
                    if d.is_finite() {
                        of += 1;
                        if d > r.delta {
                            above += 1;
                        }
                    }
                }
                (above + 1, of.max(above + 1))
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
                rank,
                fit,
            }
        })
        .collect()
}

/// A running average that settles after ~8 samples but keeps adapting.
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Learned {
    v: f64,
    n: u32,
}

impl Learned {
    fn add(&mut self, x: f64) {
        self.n += 1;
        let w = 1.0 / self.n.min(8) as f64;
        self.v = if self.n == 1 { x } else { self.v * (1.0 - w) + x * w };
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Calibration {
    offset: Learned,
    beam: Learned,
}

impl Calibration {
    fn path() -> std::path::PathBuf {
        data_dir().join("relic_calibration.json")
    }

    fn load(default_beam: f64) -> Calibration {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Calibration { offset: Learned { v: 0.0, n: 0 }, beam: Learned { v: default_beam, n: 0 } })
    }

    fn save(&self) {
        if let Ok(t) = serde_json::to_string(self) {
            let _ = std::fs::write(Self::path(), t);
        }
    }
}

/// Distance from (cx, cy) to the nearest champion icon centre within `search` px: a ring of
/// team colour (ally blue / enemy red) of radius `ring_r`. Minions are small solid dots and
/// don't make a ring. BGRA buffer `w` x `h`.
pub fn nearest_icon(buf: &[u8], w: i32, h: i32, cx: f32, cy: f32, search: f32, ring_r: f32) -> Option<f32> {
    const ANGLES: usize = 32;
    let team = |x: i32, y: i32| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return false;
        }
        let k = ((y * w + x) * 4) as usize;
        let (b, g, r) = (buf[k] as i32, buf[k + 1] as i32, buf[k + 2] as i32);
        (b > 170 && b > r + 80 && g > 100) || (r > 160 && r > g + 90 && r > b + 90)
    };
    let dirs: Vec<(f32, f32)> =
        (0..ANGLES).map(|a| (a as f32 * std::f32::consts::TAU / ANGLES as f32)).map(|t| (t.cos(), t.sin())).collect();
    let mut best: Option<f32> = None;
    let s = search.ceil() as i32;
    for dy in (-s..=s).step_by(2) {
        for dx in (-s..=s).step_by(2) {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d > search || best.is_some_and(|b| d >= b) {
                continue;
            }
            let (px, py) = (cx + dx as f32, cy + dy as f32);
            let hits = dirs
                .iter()
                .filter(|(c, sn)| {
                    [-1.5f32, 0.0, 1.0].iter().any(|o| {
                        let rr = ring_r + o;
                        team((px + c * rr).round() as i32, (py + sn * rr).round() as i32)
                    })
                })
                .count();
            // most of the ring visible (other icons can overlap part of it)
            if hits * 100 >= ANGLES * 55 {
                best = Some(d);
            }
        }
    }
    best
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Up,
    Covered,
    Gone,
}

/// The game window is in front (not alt-tabbed, not another app over it).
fn league_in_front() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};
    unsafe {
        let mut name = [0u16; 64];
        let n = GetClassNameW(GetForegroundWindow(), &mut name);
        String::from_utf16_lossy(&name[..n.max(0) as usize]) == "RiotWindowClass"
    }
}

/// League's "Flip minimap" setting puts the minimap bottom-left.
fn minimap_flipped(league_dir: &str) -> bool {
    std::fs::read_to_string(std::path::Path::new(league_dir).join("Config").join("game.cfg"))
        .map(|t| t.lines().any(|l| l.trim().replace(' ', "").eq_ignore_ascii_case("FlipMiniMap=1")))
        .unwrap_or(false)
}

/// Look for the minimap now and then while relic timers run (it can't be found while
/// dead: the screen goes grey). Keeps the last good position.
fn track_minimap(cfg: &Config, scanner: &mut Scanner, relics: &RelicRef, last: &mut Instant) {
    if !cfg.relic_timers || cfg.relic_minimap.is_some() {
        return;
    }
    let known = relics.lock().unwrap().minimap;
    let every = if known.is_some() { 30 } else { 3 };
    if !relics.lock().unwrap().active || last.elapsed() < Duration::from_secs(every) {
        return;
    }
    *last = Instant::now();
    let t = Instant::now();
    let found = scanner.reader().find_minimap(minimap_flipped(&cfg.league_dir));
    if let Some(m) = found {
        let moved = known.map(|k| (k.0 - m.0).abs() > 3.0 || (k.1 - m.1).abs() > 3.0 || (k.2 - m.2).abs() > 3.0);
        if moved.unwrap_or(true) {
            log::info!("minimap at x={} y={} size={} ({} us)", m.0, m.1, m.2, t.elapsed().as_micros());
            relics.lock().unwrap().minimap = Some(m);
        }
    } else if known.is_none() {
        log::debug!("minimap not found");
    }
}

fn key_held(vk: u32) -> bool {
    unsafe { (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

pub fn spawn(cfg: Config, ctl: ScanRef, shared: SharedRef, relics: RelicRef, overlay: OverlayHandle) {
    let scoreboard_vk = crate::overlay::parse_hotkey(&cfg.scoreboard_key).map(|(_, vk)| vk).unwrap_or(0x09);
    std::thread::Builder::new()
        .name("scanner".into())
        .spawn(move || {
            // the OCR engine costs ~15 MB: create it when a game needs it, release it after
            let mut scanner: Option<Scanner> = None;
            let mut off_since = Instant::now();
            let mut shown: Vec<(u32, i32, i32)> = vec![];
            // the offer that last closed: if it comes straight back, it wasn't a pick
            let mut last_closed: Vec<u32> = vec![];
            let mut closed_at = Instant::now();
            // a scan that sees only some of the shown cards (one title misread this time)
            let mut partial_since: Option<Instant> = None;
            let mut misses = 0u32;
            let mut last_full = Instant::now() - Duration::from_secs(60);
            let mut last_minimap = Instant::now() - Duration::from_secs(60);
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
                if relics.lock().unwrap().active && key_held(scoreboard_vk) {
                    scanner.relic_sync(&relics);
                }
                track_minimap(&cfg, scanner, &relics, &mut last_minimap);
                let dead = || shared.read().unwrap().game.as_ref().is_some_and(|g| g.dead);
                if cfg.relic_timers {
                    scanner.watch_relics(&relics, &cfg, dead());
                }
                let t0 = Instant::now();
                // a full sweep reads ~13 strips, so only fall back to it now and then
                let full = (!scanner.has_band() && last_full.elapsed() > Duration::from_secs(2))
                    || (misses >= 6 && last_full.elapsed() > Duration::from_secs(3));
                if full {
                    last_full = Instant::now();
                }
                let hits = if full || scanner.has_band() { scanner.scan(&meta, full) } else { vec![] };
                let partial = !hits.is_empty()
                    && hits.len() < shown.len()
                    && hits.iter().all(|h| shown.iter().any(|s| s.0 == h.id));
                let hold = partial && partial_since.get_or_insert_with(Instant::now).elapsed() < Duration::from_millis(1500);
                if !partial {
                    partial_since = None;
                }
                if hold {
                    misses = 0; // keep the full set on screen
                } else if !hits.is_empty() {
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
                        if shown.is_empty() && !last_closed.is_empty() && closed_at.elapsed() < Duration::from_secs(20) {
                            let same = hits.iter().filter(|h| last_closed.contains(&h.id)).count();
                            if same >= 2.min(last_closed.len()) {
                                let mut c = ctl.lock().unwrap();
                                c.picks = c.picks.saturating_sub(1);
                                crate::state::save_picks(c.game_start, c.picks);
                                log::info!("same offer reopened: not a pick (picks {})", c.picks);
                            }
                            last_closed.clear();
                        }
                        shown = key;
                        log::info!(
                            "cards: {}",
                            badges
                                .iter()
                                .map(|b| match b.rank {
                                    Some((r, of)) => format!("{} {} ({r}/{of})", b.name, b.tier.label()),
                                    None => format!("{} {}", b.name, b.tier.label()),
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        overlay.send(OverlayCmd::Badges(badges.clone()));
                        shared.write().unwrap().cards = badges;
                        repaint();
                    }
                } else {
                    misses += 1;
                    if !shown.is_empty() && misses >= 2 {
                        last_closed = shown.iter().map(|k| k.0).collect();
                        closed_at = Instant::now();
                        shown.clear();
                        let picks = {
                            let mut c = ctl.lock().unwrap();
                            c.picks += 1;
                            crate::state::save_picks(c.game_start, c.picks);
                            c.picks
                        };
                        log::info!("cards closed (pick {picks} made)");
                        overlay.send(OverlayCmd::Badges(vec![]));
                        shared.write().unwrap().cards.clear();
                        repaint();
                    }
                }
                // sleep until the next scan, but wake early for the scoreboard key
                let deadline = t0 + Duration::from_millis(interval).max(Duration::from_millis(30));
                while Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(40));
                    if cfg.relic_timers {
                        scanner.watch_relics(&relics, &cfg, dead());
                    }
                    if key_held(scoreboard_vk) && relics.lock().unwrap().active {
                        scanner.relic_sync(&relics);
                        std::thread::sleep(Duration::from_millis(210));
                    }
                }
            }
        })
        .expect("spawn scanner");
}
