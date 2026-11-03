//! State shared between the engine, the card scanner, the overlay and the companion UI.

use crate::model::{AugRating, BuildProfile, Fit, Meta, Tier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

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
    /// Place among every augment of the same rarity for this champion and round
    /// (1 = highest win rate), and how many there are.
    pub rank: Option<(u32, u32)>,
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
    /// When this game started (unix seconds), to keep `picks` across a service restart.
    pub game_start: f64,
    pub build: BuildProfile,
    pub error: Option<String>,
}

fn picks_file() -> std::path::PathBuf {
    crate::config::data_dir().join("picks.json")
}

/// Remember the pick count for this game (a restart mid-game would otherwise lose it).
pub fn save_picks(game_start: f64, picks: u32) {
    if game_start > 0.0 {
        let _ = std::fs::write(picks_file(), format!("{{\"start\":{game_start:.0},\"picks\":{picks}}}"));
    }
}

/// The saved pick count, if it belongs to the game that started at `game_start`.
pub fn load_picks(game_start: f64) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(picks_file()).ok()?).ok()?;
    ((v["start"].as_f64()? - game_start).abs() < 30.0)
        .then(|| v["picks"].as_u64().map(|p| p as u32))
        .flatten()
}

fn relics_file() -> std::path::PathBuf {
    crate::config::data_dir().join("relics.json")
}

pub fn save_relics(game_start: f64, ready_at: [f64; 4]) {
    if game_start > 0.0 {
        let v = serde_json::json!({ "start": game_start.round(), "ready_at": ready_at });
        let _ = std::fs::write(relics_file(), v.to_string());
    }
}

/// Saved relic timers, if they belong to the game that started at `game_start`.
pub fn load_relics(game_start: f64) -> Option<[f64; 4]> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(relics_file()).ok()?).ok()?;
    if (v["start"].as_f64()? - game_start).abs() >= 30.0 {
        return None;
    }
    let a: Vec<f64> = v["ready_at"].as_array()?.iter().filter_map(|x| x.as_f64()).collect();
    a.try_into().ok()
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
    pub update_status: String,
    pub history_account: String,
    pub history_revision: u64,
    pub history_status: String,
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
    ReloadSettings,
    CheckUpdates,
    RefreshData,
    SyncHistory,
    ImportBuild { champ: u32, cluster: usize },
    SwapBench(u32),
    Reroll,
}

pub type ScanRef = Arc<Mutex<ScanCtl>>;

/// Heal relic timers, in scoreboard order: blue inner, blue outer, red outer, red inner.
/// Times are game-clock seconds; `game_time` + `at` let anyone extrapolate "now".
#[derive(Clone, Copy, Debug, Default)]
pub struct RelicClock {
    pub active: bool,
    pub ready_at: [f64; 4],
    pub game_time: f64,
    pub at: Option<std::time::Instant>,
    pub my_side_blue: Option<bool>,
    /// When a slot was last marked by hotkey (for press-again-to-undo).
    pub marked: [Option<std::time::Instant>; 4],
    pub before_mark: [f64; 4],
    /// Minimap map area on screen (x, y, size in physical px), found by the scanner.
    pub minimap: Option<(f32, f32, f32)>,
}

impl RelicClock {
    pub fn now(&self) -> f64 {
        self.game_time + self.at.map(|a| a.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    pub fn start(&mut self, first_outer: f64, first_inner: f64) {
        *self = RelicClock {
            active: true,
            ready_at: [first_inner, first_outer, first_outer, first_inner],
            ..*self
        };
        self.marked = [None; 4];
    }
}

pub type RelicRef = Arc<Mutex<RelicClock>>;

/// Relic centres on the Howling Abyss minimap (fraction of the map area), scoreboard order:
/// blue inner, blue outer, red outer, red inner. Measured in game.
pub const RELIC_UV: [(f32, f32); 4] = [(0.3748, 0.6895), (0.4627, 0.5925), (0.5912, 0.4659), (0.6941, 0.3818)];

/// 1 -> "1st", 22 -> "22nd", 13 -> "13th".
pub fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// Rank colour: top 10% green, bottom 10% red, otherwise neutral.
pub fn rank_band(rank: u32, of: u32) -> i8 {
    let p = if of > 1 { (rank - 1) as f64 / (of - 1) as f64 } else { 0.0 };
    if p <= 0.1 {
        1
    } else if p >= 0.9 {
        -1
    } else {
        0
    }
}

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
