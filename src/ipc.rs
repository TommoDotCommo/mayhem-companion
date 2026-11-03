//! Service <-> companion link: line-delimited JSON over 127.0.0.1.
//!
//! The service (engine, scanner, overlay) runs all the time and is small. The companion
//! window is a separate process that opens for champ select / browsing and closes in
//! game, so its GPU memory is only paid while it's useful.

use crate::config::data_dir;
use crate::state::{Badge, ChampSelectState, Client, GameState, SharedRef, UiCmd};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub client: Client,
    pub phase: String,
    pub mayhem: Option<bool>,
    pub champ_select: Option<ChampSelectState>,
    pub game: Option<GameState>,
    pub last_champ: Option<u32>,
    pub status: String,
    #[serde(default)]
    pub update_status: String,
    #[serde(default)]
    pub history_account: String,
    #[serde(default)]
    pub history_revision: u64,
    #[serde(default)]
    pub history_status: String,
    pub cards: Vec<Badge>,
    pub demo: bool,
}

#[derive(Serialize, Deserialize)]
pub enum ToCompanion {
    Snapshot(Snapshot),
    ShowChampSelect,
    Close,
}

#[derive(Serialize, Deserialize)]
pub enum ToService {
    Cmd(UiCmd),
}

fn port_file() -> std::path::PathBuf {
    data_dir().join("service.port")
}

// ------------------------------------------------------------------ service side

struct Server {
    shared: SharedRef,
    conn: Mutex<Option<TcpStream>>,
    dirty: Mutex<bool>,
    wake: Condvar,
}

static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

/// Called by `state::repaint()`: schedule a snapshot push to the companion.
pub fn notify() {
    if let Some(s) = SERVER.get() {
        *s.dirty.lock().unwrap() = true;
        s.wake.notify_one();
    }
}

pub fn send(msg: &ToCompanion) {
    let Some(s) = SERVER.get() else { return };
    let mut conn = s.conn.lock().unwrap();
    if let Some(stream) = conn.as_mut() {
        let mut line = serde_json::to_vec(msg).unwrap_or_default();
        line.push(b'\n');
        if stream.write_all(&line).is_err() {
            *conn = None;
        }
    }
}

fn snapshot(shared: &SharedRef) -> Snapshot {
    let s = shared.read().unwrap();
    Snapshot {
        client: s.client,
        phase: s.phase.clone(),
        mayhem: s.mayhem,
        champ_select: s.champ_select.clone(),
        game: s.game.clone(),
        last_champ: s.last_champ,
        status: s.status.clone(),
        update_status: s.update_status.clone(),
        history_account: s.history_account.clone(),
        history_revision: s.history_revision,
        history_status: s.history_status.clone(),
        cards: s.cards.clone(),
        demo: s.demo,
    }
}

pub fn serve(shared: SharedRef, tx: Sender<crate::engine::Msg>) {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("ipc: can't listen: {e}");
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let _ = std::fs::write(port_file(), port.to_string());
    let server = Arc::new(Server {
        shared,
        conn: Mutex::new(None),
        dirty: Mutex::new(false),
        wake: Condvar::new(),
    });
    let _ = SERVER.set(server.clone());

    // accept companions (a new one replaces the old)
    let srv = server.clone();
    std::thread::Builder::new()
        .name("ipc-accept".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if stream.peer_addr().map(|a| !a.ip().is_loopback()).unwrap_or(true) {
                    continue;
                }
                stream.set_nodelay(true).ok();
                let Ok(reader) = stream.try_clone() else { continue };
                *srv.conn.lock().unwrap() = Some(stream);
                log::info!("companion connected");
                send(&ToCompanion::Snapshot(snapshot(&srv.shared)));
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(reader).lines() {
                        let Ok(line) = line else { break };
                        if let Ok(ToService::Cmd(cmd)) = serde_json::from_str(&line) {
                            let _ = tx.send(crate::engine::Msg::Ui(cmd));
                        }
                    }
                    log::info!("companion disconnected");
                });
            }
        })
        .expect("spawn ipc accept");

    // push snapshots when state changes (coalesced to <= 60/s)
    std::thread::Builder::new()
        .name("ipc-push".into())
        .spawn(move || loop {
            {
                let mut dirty = server.dirty.lock().unwrap();
                while !*dirty {
                    dirty = server.wake.wait(dirty).unwrap();
                }
                *dirty = false;
            }
            std::thread::sleep(Duration::from_millis(16));
            if server.conn.lock().unwrap().is_some() {
                send(&ToCompanion::Snapshot(snapshot(&server.shared)));
            }
        })
        .expect("spawn ipc push");
}

// ------------------------------------------------------------------ companion side

/// Keep the companion's copy of the state in sync and forward its commands.
pub fn connect(shared: SharedRef, cmds: Receiver<crate::engine::Msg>) {
    let current: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
    let writer = current.clone();
    std::thread::spawn(move || {
        for msg in cmds {
            if let crate::engine::Msg::Ui(cmd) = msg {
                let mut line = serde_json::to_vec(&ToService::Cmd(cmd)).unwrap_or_default();
                line.push(b'\n');
                if let Some(s) = writer.lock().unwrap().as_mut() {
                    let _ = s.write_all(&line);
                }
            }
        }
    });
    std::thread::Builder::new()
        .name("ipc-client".into())
        .spawn(move || {
            let mut started_service = false;
            loop {
                let stream = std::fs::read_to_string(port_file())
                    .ok()
                    .and_then(|p| p.trim().parse::<u16>().ok())
                    .and_then(|port| TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300)).ok());
                let Some(stream) = stream else {
                    {
                        let mut s = shared.write().unwrap();
                        s.client = Client::NotRunning;
                        s.status = "Starting the background service…".into();
                    }
                    crate::state::repaint();
                    if !started_service {
                        started_service = true;
                        crate::spawn_self(&["--background"]);
                    }
                    std::thread::sleep(Duration::from_millis(700));
                    continue;
                };
                stream.set_nodelay(true).ok();
                *current.lock().unwrap() = stream.try_clone().ok();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    match serde_json::from_str::<ToCompanion>(&line) {
                        Ok(ToCompanion::Snapshot(snap)) => {
                            let mut s = shared.write().unwrap();
                            s.client = snap.client;
                            s.phase = snap.phase;
                            s.mayhem = snap.mayhem;
                            s.champ_select = snap.champ_select;
                            s.game = snap.game;
                            s.last_champ = snap.last_champ;
                            s.status = snap.status;
                            s.update_status = snap.update_status;
                            s.history_account = snap.history_account;
                            s.history_revision = snap.history_revision;
                            s.history_status = snap.history_status;
                            s.cards = snap.cards;
                            s.demo = snap.demo;
                        }
                        Ok(ToCompanion::ShowChampSelect) => shared.write().unwrap().show_champ_select = true,
                        Ok(ToCompanion::Close) => {
                            log::info!("companion: closing (game started)");
                            std::process::exit(0);
                        }
                        Err(_) => continue,
                    }
                    crate::state::repaint();
                }
                *current.lock().unwrap() = None;
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .expect("spawn ipc client");
}
