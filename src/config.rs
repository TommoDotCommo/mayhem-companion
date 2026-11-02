use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where everything we write lives: config, cache, logs, learned card layout.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("AramOverlay")
}

/// Win-rate gain over the baseline needed for each tier; anything below `d` is F.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Cutoffs {
    #[serde(rename = "S+")]
    pub s_plus: f64,
    #[serde(rename = "S")]
    pub s: f64,
    #[serde(rename = "A")]
    pub a: f64,
    #[serde(rename = "B")]
    pub b: f64,
    #[serde(rename = "C")]
    pub c: f64,
    #[serde(rename = "D")]
    pub d: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub league_dir: String,
    pub data_url: String,
    pub cache_hours: f64,
    pub panel_hotkey: String,
    pub companion_hotkey: String,
    pub quit_hotkey: String,
    pub import_item_set: bool,
    pub set_summoner_spells: bool,
    pub open_companion_in_champ_select: bool,
    pub close_companion_in_game: bool,
    pub mayhem_queue_ids: Vec<i64>,
    /// Augment tiers: gain over the champion's average (after smoothing).
    pub tier_cutoffs: Cutoffs,
    /// Champion tiers: gain over 50%.
    pub champion_tier_cutoffs: Cutoffs,
    pub smoothing_games: f64,
    pub champion_smoothing_games: f64,
    pub scan_cards: bool,
    pub ocr_language: String,
    pub scan_interval_fast_ms: u64,
    pub scan_interval_slow_ms: u64,
    pub augment_levels: Vec<u32>,
    pub badge_scale: f32,
    pub panel_scale: f32,
    pub hide_from_capture: bool,
    /// Negative x = distance from the right edge of the primary screen.
    pub panel_x: i32,
    pub panel_y: i32,
    pub panel_rows: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            league_dir: r"C:\Riot Games\League of Legends".into(),
            data_url: "https://arammeta.com/api".into(),
            cache_hours: 12.0,
            panel_hotkey: "F8".into(),
            companion_hotkey: "F7".into(),
            quit_hotkey: "ctrl+shift+F8".into(),
            import_item_set: true,
            set_summoner_spells: true,
            open_companion_in_champ_select: true,
            close_companion_in_game: true,
            mayhem_queue_ids: vec![2400],
            tier_cutoffs: Cutoffs { s_plus: 0.035, s: 0.02, a: 0.008, b: -0.01, c: -0.025, d: -0.04 },
            champion_tier_cutoffs: Cutoffs { s_plus: 0.05, s: 0.03, a: 0.012, b: -0.01, c: -0.025, d: -0.04 },
            smoothing_games: 300.0,
            champion_smoothing_games: 2000.0,
            scan_cards: true,
            ocr_language: "en-US".into(),
            scan_interval_fast_ms: 300,
            scan_interval_slow_ms: 2500,
            augment_levels: vec![3, 7, 11, 15],
            badge_scale: 1.0,
            panel_scale: 1.0,
            hide_from_capture: true,
            panel_x: -24,
            panel_y: 140,
            panel_rows: 8,
        }
    }
}

impl Config {
    /// Load `config.json` from the data dir, creating it with defaults on first run.
    pub fn load() -> Config {
        let path = data_dir().join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::error!("config.json is invalid ({e}); using defaults");
                    Config::default()
                }
            },
            Err(_) => {
                let cfg = Config::default();
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
                cfg
            }
        }
    }
}
