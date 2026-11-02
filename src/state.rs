//! State shared between the engine, the card scanner, the overlay and the companion UI.

use crate::model::{AugRating, BuildProfile, Fit, Meta, Tier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tag {
    Best,
    Worst,
}

/// One detected augment card, rated for the current champion / round / build.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Badge {
    pub aug_id: u32,
    pub name: String,
    /// Screen rectangle of the card title (physical pixels).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub tier: Tier,
    #[serde(with = "nan_as_null")]
    pub wr: f64,
    #[serde(with = "nan_as_null")]
    pub delta: f64,
    pub round: Option<u32>,
    pub global: bool,
    pub tag: Option<Tag>,
    pub fit: Option<(Fit, Vec<String>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScanMode {
    #[default]
    Off,
    /// Alive: picks are unlikely, check every few seconds.
    Slow,
    /// Game start / dead: picks happen now.
    Fast,
}

/// What the card scanner needs; written by the engine, read every scan.
#[derive(Default)]
pub struct ScanCtl {
    pub mode: ScanMode,
    pub meta: Option<Arc<Meta>>,
    pub ratings: Arc<HashMap<u32, AugRating>>,
    /// Augment rounds unlocked by level.
    pub max_round: u32,
    /// Card screens seen and closed this game (= picks made). The scanner increments it.
    pub picks: u32,
    pub build: BuildProfile,
    pub error: Option<String>,
}

impl ScanCtl {
    pub fn round(&self) -> u32 {
        (self.picks + 1).min(self.max_round.max(1)).clamp(1, 4)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChampSelectState {
    pub my_champ: u32,
    pub bench: Vec<u32>,
    pub teammates: Vec<u32>,
    pub rerolls: Option<u32>,
    pub bench_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameState {
    pub champ: Option<u32>,
    pub level: u32,
    pub round: u32,
    pub dead: bool,
    pub build: Vec<String>,
    pub build_expected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Client {
    #[default]
    NotRunning,
    Connected,
}

#[derive(Default)]
pub struct Shared {
    pub client: Client,
    pub phase: String,
    /// None = not checked yet / not in a game flow.
    pub mayhem: Option<bool>,
    pub champ_select: Option<ChampSelectState>,
    pub game: Option<GameState>,
    /// The last champion we played or picked (keeps the companion useful after a game).
    pub last_champ: Option<u32>,
    pub status: String,
    pub cards: Vec<Badge>,
    /// Engine asks the UI to jump to the champ select tab.
    pub show_champ_select: bool,
    pub demo: bool,
}

pub type SharedRef = Arc<RwLock<Shared>>;

/// Lets background threads poke the companion window to repaint.
pub static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// State changed: repaint the companion (in its process) / push a snapshot (in the service).
pub fn repaint() {
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
    crate::ipc::notify();
}

/// Commands from the companion UI to the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiCmd {
    ImportBuild { champ: u32, cluster: usize },
    SwapBench(u32),
    Reroll,
}

pub type ScanRef = Arc<Mutex<ScanCtl>>;

/// JSON has no NaN ("no data"): send it as null.
mod nan_as_null {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_finite() {
            s.serialize_f64(*v)
        } else {
            s.serialize_none()
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }
}
