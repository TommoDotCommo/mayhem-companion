//! Remember where the companion window was (monitor, position, size, maximized).
//!
//! Uses the Win32 window placement (physical pixels, exact across monitors with different
//! scaling) and saves it once a second while the window is open, because the companion
//! usually ends by being told to exit when a game starts, not by a close button.

use crate::config::data_dir;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Saved {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    maximized: bool,
}

fn path() -> std::path::PathBuf {
    data_dir().join("companion_window.json")
}

/// Our own top-level window with the companion title (it may still be hidden).
pub fn own_window(title: &str) -> Option<isize> {
    struct Find {
        pid: u32,
        title: Vec<u16>,
        found: isize,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp.0 as *mut Find);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == f.pid {
            let mut buf = [0u16; 128];
            let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
            if buf[..n] == f.title[..] {
                f.found = hwnd.0 as isize;
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut f = Find { pid: unsafe { GetCurrentProcessId() }, title: title.encode_utf16().collect(), found: 0 };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut f as *mut Find as isize));
    }
    (f.found != 0).then_some(f.found)
}

/// Put the window back where it was last time. Windows moves it onto a monitor if that
/// monitor is gone. Returns false when there's nothing saved.
pub fn restore(hwnd: isize) -> bool {
    let Some(s) = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str::<Saved>(&t).ok()) else {
        return false;
    };
    if s.right - s.left < 200 || s.bottom - s.top < 150 {
        return false;
    }
    let wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        flags: WINDOWPLACEMENT_FLAGS(0),
        showCmd: if s.maximized { SW_SHOWMAXIMIZED.0 as u32 } else { SW_SHOWNORMAL.0 as u32 },
        rcNormalPosition: RECT { left: s.left, top: s.top, right: s.right, bottom: s.bottom },
        ..Default::default()
    };
    unsafe { SetWindowPlacement(HWND(hwnd as _), &wp).is_ok() }
}

fn read(hwnd: isize) -> Option<Saved> {
    let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    unsafe { GetWindowPlacement(HWND(hwnd as _), &mut wp).ok()? };
    let r = wp.rcNormalPosition;
    let maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32
        || (wp.showCmd == SW_SHOWMINIMIZED.0 as u32 && wp.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0);
    Some(Saved { left: r.left, top: r.top, right: r.right, bottom: r.bottom, maximized })
}

/// Save the placement whenever it changes, until the window goes away.
pub fn keep_saved(hwnd: isize) {
    std::thread::Builder::new()
        .name("placement".into())
        .spawn(move || {
            let mut last = read(hwnd);
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if unsafe { !IsWindow(HWND(hwnd as _)).as_bool() } {
                    return;
                }
                let now = read(hwnd);
                if now.is_some() && now != last {
                    if let Ok(t) = serde_json::to_string(&now) {
                        let _ = std::fs::write(path(), t);
                    }
                    last = now;
                }
            }
        })
        .ok();
}
