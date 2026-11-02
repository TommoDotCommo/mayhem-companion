//! League client API: lockfile auth, REST calls, and the WAMP event websocket
//! (instant champ select / game phase updates instead of polling).

use crate::net;
use base64::Engine as _;
use serde_json::Value;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::{Connector, Message};

#[derive(Clone, Debug)]
pub struct Lcu {
    pub port: u16,
    auth: String,
}

impl Lcu {
    pub fn from_lockfile(league_dir: &str) -> Option<Lcu> {
        let text = std::fs::read_to_string(PathBuf::from(league_dir).join("lockfile")).ok()?;
        let parts: Vec<&str> = text.trim().split(':').collect();
        if parts.len() < 5 {
            return None;
        }
        let port = parts[2].parse().ok()?;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("riot:{}", parts[3]));
        Some(Lcu { port, auth: format!("Basic {token}") })
    }

    fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.port)
    }

    pub fn get(&self, path: &str) -> Option<Value> {
        net::local().get(&self.url(path)).set("Authorization", &self.auth).call().ok()?.into_json().ok()
    }

    pub fn send(&self, method: &str, path: &str, body: Option<&Value>) -> bool {
        let req = net::local().request(method, &self.url(path)).set("Authorization", &self.auth);
        let res = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match res {
            Ok(_) => true,
            Err(ureq::Error::Status(code, resp)) => {
                log::warn!("{method} {path} -> {code} {}", resp.into_string().unwrap_or_default().chars().take(300).collect::<String>());
                false
            }
            Err(e) => {
                log::warn!("{method} {path} failed: {e}");
                false
            }
        }
    }
}

pub enum LcuEvent {
    Connected(Lcu),
    Disconnected,
    Phase(String),
    Session(Option<Value>),
}

/// Background thread: wait for the client, subscribe to events, reconnect forever.
pub fn spawn(league_dir: String, tx: Sender<crate::engine::Msg>) {
    std::thread::Builder::new()
        .name("lcu".into())
        .spawn(move || {
            let mut connected = false;
            loop {
                if let Some(lcu) = Lcu::from_lockfile(&league_dir) {
                    match listen(&lcu, &tx) {
                        Ok(()) => {}
                        Err(e) => log::debug!("lcu websocket: {e}"),
                    }
                    connected = true;
                }
                if connected {
                    connected = false;
                    let _ = tx.send(crate::engine::Msg::Lcu(LcuEvent::Disconnected));
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("spawn lcu");
}

fn listen(lcu: &Lcu, tx: &Sender<crate::engine::Msg>) -> Result<(), String> {
    let mut req = format!("wss://127.0.0.1:{}/", lcu.port).into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("Authorization", lcu.auth.parse().map_err(|_| "auth header")?);
    let tcp = TcpStream::connect(("127.0.0.1", lcu.port)).map_err(|e| e.to_string())?;
    tcp.set_nodelay(true).ok();
    let (mut ws, _) = tungstenite::client_tls_with_config(req, tcp, None, Some(Connector::Rustls(net::local_tls())))
        .map_err(|e| e.to_string())?;
    for ev in ["OnJsonApiEvent_lol-gameflow_v1_gameflow-phase", "OnJsonApiEvent_lol-champ-select_v1_session"] {
        ws.send(Message::Text(format!("[5,\"{ev}\"]"))).map_err(|e| e.to_string())?;
    }
    log::info!("connected to League client on port {}", lcu.port);
    let send = |e| tx.send(crate::engine::Msg::Lcu(e)).map_err(|_| "engine gone".to_string());
    send(LcuEvent::Connected(lcu.clone()))?;
    // initial state (events only fire on change)
    if let Some(Value::String(phase)) = lcu.get("/lol-gameflow/v1/gameflow-phase") {
        send(LcuEvent::Phase(phase))?;
    }
    if let Some(session) = lcu.get("/lol-champ-select/v1/session") {
        send(LcuEvent::Session(Some(session)))?;
    }
    loop {
        let msg = ws.read().map_err(|e| e.to_string())?;
        let Message::Text(text) = msg else { continue };
        let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(&text) else { continue };
        if parts.first().and_then(|v| v.as_i64()) != Some(8) {
            continue;
        }
        let Some(payload) = parts.get(2) else { continue };
        let uri = payload.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let deleted = payload.get("eventType").and_then(|e| e.as_str()) == Some("Delete");
        let data = payload.get("data").cloned().unwrap_or(Value::Null);
        match uri {
            "/lol-gameflow/v1/gameflow-phase" => {
                if let Value::String(p) = data {
                    send(LcuEvent::Phase(p))?;
                }
            }
            "/lol-champ-select/v1/session" => {
                send(LcuEvent::Session(if deleted || data.is_null() { None } else { Some(data) }))?;
            }
            _ => {}
        }
    }
}
