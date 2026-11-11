//! Optional diagnostic captures: a screenshot and the scanner's reasoning each time augment
//! badges are shown, kept locally and bundled into a zip the owner can choose to send.
//! Off unless `save_diagnostics` is set. Nothing here uploads anything.
use crate::config::data_dir;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Mirrors `Config::save_diagnostics` for the scanner thread.
pub static ENABLED: AtomicBool = AtomicBool::new(false);
/// Set by the capture hotkey; the scanner takes it and saves a capture of that moment.
pub static REQUESTED: AtomicBool = AtomicBool::new(false);
/// Oldest captures are deleted beyond this many.
pub const MAX_CAPTURES: usize = 200;

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn dir() -> PathBuf {
    data_dir().join("diagnostics")
}

fn captures(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("jpg" | "json")))
        .collect();
    files.sort();
    files
}

/// Screenshots saved and their total size in bytes (pictures and notes).
pub fn usage() -> (usize, u64) {
    let files = captures(&dir());
    let shots = files.iter().filter(|p| p.extension().is_some_and(|e| e == "jpg")).count();
    (shots, files.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum())
}

pub fn clear() {
    for f in captures(&dir()) {
        let _ = std::fs::remove_file(f);
    }
}

fn prune(dir: &Path, keep: usize) {
    // names start with a millisecond stamp, so sorted order is oldest first
    let shots: Vec<PathBuf> = captures(dir).into_iter().filter(|p| p.extension().is_some_and(|e| e == "jpg")).collect();
    for old in shots.iter().take(shots.len().saturating_sub(keep)) {
        let _ = std::fs::remove_file(old);
        let _ = std::fs::remove_file(old.with_extension("json"));
    }
}

/// Save one capture off-thread: `<stamp>.jpg` (the screen as BGRA) and `<stamp>.json` (what the scanner saw and decided).
pub fn save(bgra: Vec<u8>, w: u32, h: u32, info: serde_json::Value) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    std::thread::spawn(move || {
        if let Err(e) = save_to(&dir(), stamp, &bgra, w, h, &info) {
            log::warn!("diagnostics: {e}");
        }
    });
}

fn save_to(dir: &Path, stamp: u128, bgra: &[u8], w: u32, h: u32, info: &serde_json::Value) -> Result<(), String> {
    if bgra.len() < (w * h * 4) as usize {
        return Err("short screen capture".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let rgb: Vec<u8> = bgra.chunks_exact(4).take((w * h) as usize).flat_map(|p| [p[2], p[1], p[0]]).collect();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 80)
        .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{stamp}.jpg")), jpg).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{stamp}.json")), serde_json::to_vec_pretty(info).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    prune(dir, MAX_CAPTURES);
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A plain zip with the files stored uncompressed (the pictures are already JPEG).
fn zip_bytes(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let (offset, crc, len) = (out.len() as u64, crc32(data), data.len() as u64);
        if offset > u32::MAX as u64 || len > u32::MAX as u64 || name.len() > u16::MAX as usize {
            return Err("diagnostics are too large for one zip; delete some screenshots".into());
        }
        // version 2.0, UTF-8 names, stored, no timestamp, crc, sizes, name length, no extra field
        let mut head = vec![];
        head.extend(20u16.to_le_bytes());
        head.extend(0x0800u16.to_le_bytes());
        head.extend(0u16.to_le_bytes());
        head.extend(0u16.to_le_bytes());
        head.extend(0x21u16.to_le_bytes());
        head.extend(crc.to_le_bytes());
        head.extend((len as u32).to_le_bytes());
        head.extend((len as u32).to_le_bytes());
        head.extend((name.len() as u16).to_le_bytes());
        head.extend(0u16.to_le_bytes());
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(&head);
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(&head);
        central.extend([0u8; 10]); // comment length, disk, internal and external attributes
        central.extend((offset as u32).to_le_bytes());
        central.extend(name.as_bytes());
    }
    if files.len() > u16::MAX as usize || out.len() as u64 + central.len() as u64 > u32::MAX as u64 {
        return Err("diagnostics are too large for one zip; delete some screenshots".into());
    }
    let start = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    Ok(out)
}

fn zip_from(data: &Path, out: &Path) -> Result<usize, String> {
    let mut files = vec![];
    for path in captures(&data.join("diagnostics")) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        files.push((format!("captures/{name}"), std::fs::read(&path).map_err(|e| e.to_string())?));
    }
    let shots = files.len() / 2;
    // what the scanner logged and where it believes the cards are
    for extra in ["overlay.log", "layout.json"] {
        if let Ok(bytes) = std::fs::read(data.join(extra)) {
            files.push((extra.to_string(), bytes));
        }
    }
    if files.is_empty() {
        return Err("Nothing to send yet: no screenshots or log".into());
    }
    let tmp = out.with_extension("zip.tmp");
    std::fs::write(&tmp, zip_bytes(&files)?).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, out).map_err(|e| e.to_string())?;
    Ok(shots)
}

/// Bundle the captures, the log and the learned layout into one zip in the data folder,
/// replacing the previous bundle. Returns the zip's path and how many screenshots it holds.
pub fn zip() -> Result<(PathBuf, usize), String> {
    let out = data_dir().join("mayhem-diagnostics.zip");
    let shots = zip_from(&data_dir(), &out)?;
    Ok((out, shots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_are_saved_pruned_and_zipped() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let data = std::env::temp_dir().join(format!("mayhem-diagnostics-test-{}", std::process::id()));
        let dir = data.join("diagnostics");
        // a 2x1 screen: one blue pixel, one red (BGRA)
        let screen = [255, 0, 0, 255, 0, 0, 255, 255];
        for stamp in 1..=3u128 {
            save_to(&dir, stamp, &screen, 2, 1, &serde_json::json!({"n": stamp as u64})).unwrap();
        }
        assert!(save_to(&dir, 9, &screen[..4], 2, 1, &serde_json::Value::Null).is_err());
        prune(&dir, 2);
        let names: Vec<String> = captures(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["2.jpg", "2.json", "3.jpg", "3.json"]);
        let shot = image::load_from_memory(&std::fs::read(dir.join("3.jpg")).unwrap()).unwrap().to_rgb8();
        assert_eq!(shot.dimensions(), (2, 1));

        std::fs::write(data.join("overlay.log"), "log line").unwrap();
        let out = std::env::var_os("DIAGNOSTICS_TEST_ZIP").map(PathBuf::from).unwrap_or_else(|| data.join("out.zip"));
        assert_eq!(zip_from(&data, &out).unwrap(), 2);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        // end record: 5 entries, and the central directory sits where it says
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]), 5);
        let start = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
        assert_eq!(&bytes[start..start + 4], b"PK\x01\x02");
        assert!(zip_from(&data.join("empty"), &data.join("none.zip")).is_err());
        std::fs::remove_dir_all(data).unwrap();
    }
}
