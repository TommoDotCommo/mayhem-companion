//! Verified GitHub release downloads, installed on the next service launch.
use crate::{
    config::{data_dir, Config},
    state::{repaint, SharedRef},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const RELEASE: &str = "https://api.github.com/repos/TommoDotCommo/mayhem-companion/releases/latest";
const DOWNLOAD: &str = "https://github.com/TommoDotCommo/mayhem-companion/releases/download/";
static CHECKING: AtomicBool = AtomicBool::new(false);
#[derive(Serialize, Deserialize)]
struct Pending {
    version: String,
    hash: String,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn newer(tag: &str) -> bool {
    let Ok(version) = semver::Version::parse(tag.trim_start_matches('v')) else {
        return false;
    };
    version.pre.is_empty() && version > semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()
}
fn status(shared: &SharedRef, message: String) {
    shared.write().unwrap().update_status = message;
    repaint();
}
pub fn check_async(shared: SharedRef) {
    if CHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        status(&shared, "Checking for app updates…".into());
        let result = check();
        status(&shared, result.unwrap_or_else(|e| format!("App update failed: {e}")));
        CHECKING.store(false, Ordering::SeqCst);
    });
}
fn check() -> Result<String, String> {
    let release: serde_json::Value = crate::net::web()
        .get(RELEASE)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().ok_or("Release has no version")?;
    if !newer(tag) {
        return Ok("App is up to date".into());
    }
    let asset = release["assets"]
        .as_array()
        .ok_or("Release has no assets")?
        .iter()
        .find(|a| a["name"] == "mayhem-companion.exe")
        .ok_or("Release has no Windows executable")?;
    let url = asset["browser_download_url"].as_str().ok_or("Missing download URL")?;
    if !url.starts_with(DOWNLOAD) {
        return Err("Unexpected release download URL".into());
    }
    let expected = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Release has no SHA-256 digest")?;
    let bytes = crate::net::get_bytes(url)?;
    if Some(bytes.len() as u64) != asset["size"].as_u64() || hash(&bytes) != expected.to_ascii_lowercase() || !bytes.starts_with(b"MZ") {
        return Err("Downloaded executable failed verification".into());
    }
    let dir = data_dir().join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Remove the marker first: an interrupted write must never install a partial download.
    let _ = std::fs::remove_file(dir.join("pending.json"));
    std::fs::write(dir.join("pending.exe"), bytes).map_err(|e| e.to_string())?;
    let pending = Pending {
        version: tag.into(),
        hash: expected.to_ascii_lowercase(),
    };
    std::fs::write(dir.join("pending.json"), serde_json::to_vec(&pending).unwrap()).map_err(|e| e.to_string())?;
    Ok(format!("{tag} downloaded. Quit from the tray and reopen to install."))
}
pub fn start(shared: SharedRef) {
    std::thread::spawn(move || loop {
        if Config::load().auto_update_app {
            check_async(shared.clone());
        }
        std::thread::sleep(Duration::from_secs(6 * 3600));
    });
}
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn installer_script(pid: u32, dir: &std::path::Path, target: &std::path::Path, expected_hash: &str, background: bool) -> String {
    let q = |p: &std::path::Path| ps_quote(&p.to_string_lossy());
    format!(
        r#"
$ErrorActionPreference = 'Stop'
Wait-Process -Id {pid} -ErrorAction SilentlyContinue
$target = {target}
$source = {source}
$backup = $target + '.previous'
$replacement = $target + '.update'
try {{
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLower() -ne {hash}) {{ throw 'Checksum mismatch' }}
    Copy-Item -LiteralPath $source -Destination $replacement -Force
    if ((Get-FileHash -LiteralPath $replacement -Algorithm SHA256).Hash.ToLower() -ne {hash}) {{ throw 'Copied update checksum mismatch' }}
    $moved = $false
    for ($i = 0; $i -lt 30; $i++) {{
        try {{ Move-Item -LiteralPath $target -Destination $backup -Force; $moved = $true; break }}
        catch {{ Start-Sleep -Seconds 1 }}
    }}
    if (!$moved) {{ throw 'Executable is still in use' }}
    try {{ Move-Item -LiteralPath $replacement -Destination $target -Force }}
    catch {{ Move-Item -LiteralPath $backup -Destination $target -Force; throw }}
    Remove-Item -LiteralPath $source -Force
}} catch {{ $_ | Out-File -LiteralPath {error} }}
Remove-Item -LiteralPath {marker} -Force -ErrorAction SilentlyContinue
Start-Process -FilePath $target {args}
"#,
        pid = pid,
        target = q(target),
        source = q(&dir.join("pending.exe")),
        hash = ps_quote(expected_hash),
        marker = q(&dir.join("pending.json")),
        error = q(&dir.join("install-error.txt")),
        args = if background { "-ArgumentList '--background'" } else { "" }
    )
}

/// Run a detached helper that waits for this process to release the executable.
/// The old executable is backed up and restored if replacing it fails.
pub fn install_pending(background: bool) -> bool {
    let install = || -> Result<(), String> {
        let dir = data_dir().join("updates");
        let marker = dir.join("pending.json");
        let pending: Pending = serde_json::from_slice(&std::fs::read(&marker).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if !newer(&pending.version) {
            let _ = std::fs::remove_file(marker);
            return Err("Ignoring older staged update".into());
        }
        let source = dir.join("pending.exe");
        let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
        if hash(&bytes) != pending.hash {
            return Err("Staged update failed verification".into());
        }
        let target = std::env::current_exe().map_err(|e| e.to_string())?;
        let script = installer_script(std::process::id(), &dir, &target, &pending.hash, background);
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
        let mut command = std::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand", &encoded]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.spawn().map_err(|e| e.to_string())?;
        Ok(())
    };
    match install() {
        Ok(()) => true,
        Err(e) => {
            log::debug!("pending update: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_order_and_quoting() {
        assert!(newer("v99.0.0"));
        assert!(!newer("v99.0.0-beta.1"));
        assert!(!newer("v0.1.0"));
        assert!(!newer("bad"));
        assert_eq!(ps_quote("Tom's folder"), "'Tom''s folder'");
        assert_eq!(hash(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}

#[cfg(all(test, windows))]
mod installer_tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn installer_replaces_and_backs_up_or_rejects_tampering_with_quoted_paths() {
        for tampered in [false, true] {
            let dir = std::env::temp_dir().join(format!("mayhem-installer-test-{}-{tampered}", std::process::id()));
            let updates = dir.join("Tom's update folder");
            let app = dir.join("Tom's app folder");
            std::fs::create_dir_all(&updates).unwrap();
            std::fs::create_dir_all(&app).unwrap();
            let target = app.join("companion.exe");
            let started = dir.join("restarted.txt");
            let old = b"old executable";
            let new = b"MZ verified new executable";
            std::fs::write(&target, old).unwrap();
            std::fs::write(updates.join("pending.exe"), new).unwrap();
            std::fs::write(updates.join("pending.json"), b"test marker").unwrap();
            let expected = if tampered { "0".repeat(64) } else { hash(new) };
            // Stub only the final restart: exercise the real file replacement script without launching a second app.
            let script = format!(
                "function Start-Process {{ param($FilePath, $ArgumentList) Set-Content -LiteralPath {} -Value $FilePath }}\n{}",
                ps_quote(&started.to_string_lossy()),
                installer_script(2147483647, &updates, &target, &expected, false)
            );
            let encoded = base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
            let status = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
                .status()
                .unwrap();
            assert!(status.success());
            assert!(!updates.join("pending.json").exists());
            assert_eq!(std::fs::read_to_string(&started).unwrap().trim(), target.to_string_lossy());
            if tampered {
                assert_eq!(std::fs::read(&target).unwrap(), old);
                assert!(updates.join("install-error.txt").exists());
                assert!(!app.join("companion.exe.previous").exists());
            } else {
                assert_eq!(std::fs::read(&target).unwrap(), new);
                assert_eq!(std::fs::read(app.join("companion.exe.previous")).unwrap(), old);
                assert!(!updates.join("pending.exe").exists());
                assert!(!updates.join("install-error.txt").exists());
            }
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
