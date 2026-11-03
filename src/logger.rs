//! Minimal file logger: %LOCALAPPDATA%\AramOverlay\overlay.log

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

struct FileLogger {
    file: Mutex<File>,
    level: log::LevelFilter,
    role: &'static str,
}

impl log::Log for FileLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= self.level && meta.target().starts_with(env!("CARGO_CRATE_NAME"))
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
        let line = format!("{h:02}:{m:02}:{s:02}Z {} {:5} {}\n", self.role, record.level(), record.args());
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

pub fn init(role: &'static str) {
    let dir = crate::config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("overlay.log");
    // start fresh each run once the log gets big
    let big = role == "svc" && std::fs::metadata(&path).map(|m| m.len() > 4_000_000).unwrap_or(false);
    let file = OpenOptions::new().create(true).append(!big).write(true).truncate(big).open(&path);
    if let Ok(file) = file {
        let level = if std::env::var_os("ARAM_DEBUG").is_some() {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        };
        let logger = Box::leak(Box::new(FileLogger { file: Mutex::new(file), level, role }));
        let _ = log::set_logger(logger);
        log::set_max_level(level);
    }
}
