//! The coordinator: League client events, champ select, imports, in-game polling,
//! and feeding the card scanner, overlay and companion UI.

use crate::config::Config;
use crate::data::Data;
use crate::lcu::{Lcu, LcuEvent};
use crate::model::*;
use crate::overlay::{OverlayCmd, OverlayHandle, PanelCol, PanelContent};
use crate::state::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Msg {
    Lcu(LcuEvent),
    Ui(UiCmd),
}

const TAG: &str = "[Mayhem]";

pub struct Engine {
    cfg: Config,
    data: Arc<Data>,
    shared: SharedRef,
    scan: ScanRef,
    relics: RelicRef,
    overlay: OverlayHandle,
    lcu: Option<Lcu>,
    phase: String,
    mayhem: Option<bool>,
    /// Champion we imported for in this champ select.
    imported: Option<u32>,
    spells: (u32, u32),
    game_champ: Option<u32>,
    expected_items: Vec<u32>,
    build: Option<BuildProfile>,
    level: u32,
    game_seen: bool,
    /// Pick count restored (or started fresh) for the current game.
    picks_loaded: bool,
    hide_panel_at: Option<Instant>,
    next_live: Instant,
}

fn u32_of(v: &Value) -> u32 {
    v.as_u64().unwrap_or(0) as u32
}

impl Engine {
    pub fn new(cfg: Config, data: Arc<Data>, shared: SharedRef, scan: ScanRef, relics: RelicRef, overlay: OverlayHandle) -> Engine {
        Engine {
            cfg,
            data,
            shared,
            scan,
            relics,
            overlay,
            lcu: None,
            phase: String::new(),
            mayhem: None,
            imported: None,
            spells: (0, 0),
            game_champ: None,
            expected_items: vec![],
            build: None,
            level: 0,
            game_seen: false,
            picks_loaded: false,
            hide_panel_at: None,
            next_live: Instant::now(),
        }
    }

    pub fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let wait = if self.in_game() { Duration::from_millis(250) } else { Duration::from_secs(5) };
            match rx.recv_timeout(wait) {
                Ok(Msg::Lcu(e)) => self.on_lcu(e),
                Ok(Msg::Ui(c)) => self.on_ui(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.in_game() && Instant::now() >= self.next_live {
                self.next_live = Instant::now() + Duration::from_secs(1);
                self.poll_live();
            }
            if self.hide_panel_at.is_some_and(|t| Instant::now() >= t) {
                self.hide_panel_at = None;
                self.overlay.send(OverlayCmd::PanelVisible(Some(false)));
            }
        }
    }

    fn in_game(&self) -> bool {
        self.mayhem == Some(true) && matches!(self.phase.as_str(), "InProgress" | "Reconnect")
    }

    fn status(&self, text: impl Into<String>) {
        let text = text.into();
        log::info!("status: {text}");
        self.shared.write().unwrap().status = text;
        repaint();
    }

    // ------------------------------------------------------------------ client events

    fn on_lcu(&mut self, e: LcuEvent) {
        match e {
            LcuEvent::Connected(l) => {
                self.lcu = Some(l);
                self.shared.write().unwrap().client = Client::Connected;
                repaint();
            }
            LcuEvent::Disconnected => {
                self.lcu = None;
                self.phase.clear();
                self.mayhem = None;
                self.reset_game();
                let mut s = self.shared.write().unwrap();
                s.client = Client::NotRunning;
                s.phase.clear();
                s.mayhem = None;
                s.champ_select = None;
                drop(s);
                repaint();
            }
            LcuEvent::Phase(p) => self.on_phase(p),
            LcuEvent::Session(s) => self.on_session(s),
        }
    }

    fn check_mayhem(&mut self) {
        let Some(lcu) = &self.lcu else { return };
        let Some(session) = lcu.get("/lol-gameflow/v1/session") else { return };
        let queue = &session["gameData"]["queue"];
        let Some(id) = queue["id"].as_i64() else { return };
        let mode = queue["gameMode"].as_str().unwrap_or("");
        // custom games have queue -1: the mode ("KIWI") shows up elsewhere in the session
        let is = self.cfg.mayhem_queue_ids.contains(&id) || session.to_string().to_ascii_uppercase().contains("\"KIWI\"");
        log::info!("queue {id} ({mode}) mayhem={is}");
        self.mayhem = Some(is);
        self.shared.write().unwrap().mayhem = Some(is);
        if !is {
            self.status("Not an ARAM Mayhem game: standing by");
        }
    }

    fn on_phase(&mut self, p: String) {
        if p == self.phase {
            return;
        }
        log::info!("phase {} -> {p}", self.phase);
        self.phase = p.clone();
        self.shared.write().unwrap().phase = p.clone();
        match p.as_str() {
            "ChampSelect" | "GameStart" | "InProgress" | "Reconnect" => {
                if self.mayhem.is_none() {
                    self.check_mayhem();
                }
            }
            _ => {
                self.reset_game();
                self.mayhem = None;
                self.imported = None;
                let mut s = self.shared.write().unwrap();
                s.mayhem = None;
                s.champ_select = None;
            }
        }
        if self.mayhem == Some(true) {
            match p.as_str() {
                "ChampSelect" => {
                    self.shared.write().unwrap().show_champ_select = true;
                    if self.cfg.open_companion_in_champ_select {
                        crate::overlay::ensure_companion();
                    }
                    crate::ipc::send(&crate::ipc::ToCompanion::ShowChampSelect);
                    if let Some(s) = self.lcu.as_ref().and_then(|l| l.get("/lol-champ-select/v1/session")) {
                        self.on_session(Some(s));
                    }
                }
                "InProgress" => {
                    self.shared.write().unwrap().champ_select = None;
                    self.overlay.send(OverlayCmd::PanelVisible(Some(false)));
                    if self.cfg.close_companion_in_game {
                        crate::ipc::send(&crate::ipc::ToCompanion::Close); // free its GPU memory
                    }
                    self.next_live = Instant::now();
                }
                _ => {}
            }
        }
        repaint();
    }

    fn rerolls(&self) -> Option<u32> {
        let v = self.lcu.as_ref()?.get("/lol-summoner/v1/current-summoner/rerollPoints")?;
        v["numberOfRolls"].as_u64().map(|n| n as u32)
    }

    fn on_session(&mut self, session: Option<Value>) {
        if self.phase == "ChampSelect" && self.mayhem.is_none() {
            self.check_mayhem();
        }
        if self.mayhem != Some(true) {
            return;
        }
        let Some(s) = session else { return };
        let cell = s["localPlayerCellId"].as_i64().unwrap_or(-1);
        let team = s["myTeam"].as_array().cloned().unwrap_or_default();
        let me = team.iter().find(|p| p["cellId"].as_i64() == Some(cell));
        let my_champ = me.map(|p| u32_of(&p["championId"])).unwrap_or(0);
        self.spells = me.map(|p| (u32_of(&p["spell1Id"]), u32_of(&p["spell2Id"]))).unwrap_or((0, 0));
        let bench: Vec<u32> = s["benchChampions"]
            .as_array()
            .map(|b| b.iter().map(|c| if c.is_object() { u32_of(&c["championId"]) } else { u32_of(c) }).filter(|c| *c > 0).collect())
            .unwrap_or_default();
        let teammates: Vec<u32> = team
            .iter()
            .filter(|p| p["cellId"].as_i64() != Some(cell))
            .map(|p| u32_of(&p["championId"]))
            .filter(|c| *c > 0)
            .collect();
        let champ_changed = my_champ > 0 && self.imported != Some(my_champ);
        let rerolls = if champ_changed || self.shared.read().unwrap().champ_select.is_none() {
            self.rerolls()
        } else {
            self.shared.read().unwrap().champ_select.as_ref().and_then(|c| c.rerolls)
        };
        {
            let mut sh = self.shared.write().unwrap();
            sh.champ_select = Some(ChampSelectState {
                my_champ,
                bench,
                teammates,
                rerolls,
                bench_enabled: s["benchEnabled"].as_bool().unwrap_or(true),
            });
            if my_champ > 0 {
                sh.last_champ = Some(my_champ);
            }
        }
        repaint();
        if champ_changed {
            self.imported = Some(my_champ);
            self.on_my_champ(my_champ, true, None);
        }
    }

    /// Load a champion's data, hand it to the scanner/overlay, and import the build.
    fn on_my_champ(&mut self, cid: u32, import: bool, prefer_cluster: Option<usize>) {
        let (meta, detail) = match (self.data.meta(), self.data.champ(cid)) {
            (Ok(m), Ok(d)) => (m, d),
            (Err(e), _) | (_, Err(e)) => {
                self.status(format!("Couldn't load data: {e}"));
                return;
            }
        };
        let ratings = Arc::new(champ_ratings(&meta, &detail, cid, &self.cfg));
        self.expected_items = detail.clusters.groups.first().map(|g| g.core.iter().map(|i| i.id).collect()).unwrap_or_default();
        {
            let mut c = self.scan.lock().unwrap();
            c.meta = Some(meta.clone());
            c.ratings = ratings.clone();
        }
        self.overlay.send(OverlayCmd::Panel(Some(self.panel(&meta, &detail, &ratings, cid))));
        if import {
            self.import(&meta, &detail, cid, prefer_cluster);
        }
    }

    fn import(&self, meta: &Meta, detail: &ChampDetail, cid: u32, prefer_cluster: Option<usize>) {
        let Some(lcu) = &self.lcu else { return };
        let mut notes: Vec<String> = vec![];
        if self.cfg.import_item_set {
            let sets = make_item_sets(meta, detail, cid, prefer_cluster);
            let ok = !sets.is_empty() && import_item_sets(lcu, &sets);
            notes.push(if !ok {
                "item sets ✗".to_string()
            } else if sets.len() == 1 {
                "item set ✓".to_string()
            } else {
                format!("{} build pages ✓", sets.len())
            });
        }
        if self.cfg.set_summoner_spells && prefer_cluster.is_none() {
            let want: Vec<u32> = detail.spells.top.first().map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_default();
            if want.len() == 2 {
                match pick_spell_slots(self.spells, &want) {
                    None => notes.push("spells ✓".into()),
                    Some((a, b)) => {
                        let ok = lcu.send("PATCH", "/lol-champ-select/v1/session/my-selection", Some(&json!({"spell1Id": a, "spell2Id": b})));
                        notes.push(if ok { "spells ✓" } else { "spells ✗" }.into());
                    }
                }
            }
        }
        let build = prefer_cluster
            .and_then(|i| detail.clusters.groups.get(i))
            .map(|g| format!(" ({})", g.name_en))
            .unwrap_or_default();
        self.status(format!("{} imported{build}: {}", meta.champ_name(cid), notes.join(", ")));
    }

    fn panel(&self, meta: &Meta, detail: &ChampDetail, ratings: &HashMap<u32, AugRating>, cid: u32) -> PanelContent {
        let info = meta.champs.get(&cid.to_string()).cloned().unwrap_or_default();
        let rows = self.cfg.panel_rows;
        let cols = RARITIES
            .iter()
            .map(|(key, title, rgb)| {
                let mut list: Vec<(u32, Rating)> = detail
                    .top
                    .get(*key)
                    .map(|l| l.iter().filter_map(|a| ratings.get(&a.id).map(|r| (a.id, r.overall))).collect())
                    .unwrap_or_default();
                list.sort_by(|a, b| b.1.delta.total_cmp(&a.1.delta));
                let name = |id: u32| meta.augs.get(&id.to_string()).map(|a| a.name_en.clone()).unwrap_or_default();
                PanelCol {
                    title: title.to_string(),
                    color: *rgb,
                    rows: list.iter().take(rows).map(|(id, r)| (r.tier, name(*id), r.wr)).collect(),
                    avoid: list.iter().rev().take(3).map(|(id, r)| (r.tier, name(*id))).collect(),
                }
            })
            .collect();
        let builds = detail
            .clusters
            .groups
            .iter()
            .take(2)
            .map(|g| {
                let core: Vec<String> = g.core.iter().map(|i| meta.item_name(i.id)).collect();
                let next: Vec<String> = g.options.iter().take(2).map(|i| meta.item_name(i.id)).collect();
                (g.name_en.clone(), format!("{}  →  {}", core.join(" + "), next.join(" / ")))
            })
            .collect();
        let rating = champion_rating(&info, &self.cfg);
        PanelContent {
            title: info.name_en.clone(),
            subtitle: format!(
                "Mayhem {} · {} tier · {:.1}% WR · {:.0}k games",
                meta.patch_prefix,
                rating.tier.label(),
                info.wr * 100.0,
                info.g / 1000.0
            ),
            cols,
            builds,
            footer: format!("{} hide · {} companion · data: arammeta.com", self.cfg.panel_hotkey, self.cfg.companion_hotkey),
        }
    }

    // ------------------------------------------------------------------ companion commands

    fn on_ui(&mut self, cmd: UiCmd) {
        let Some(lcu) = self.lcu.clone() else {
            self.status("League client not running");
            return;
        };
        match cmd {
            UiCmd::ImportBuild { champ, cluster } => {
                let in_cs = self.phase == "ChampSelect" && self.imported == Some(champ);
                if !in_cs {
                    self.status("Builds import during champ select for your current champion");
                    return;
                }
                self.on_my_champ(champ, true, Some(cluster));
            }
            UiCmd::SwapBench(cid) => {
                if !lcu.send("POST", &format!("/lol-champ-select/v1/session/bench/swap/{cid}"), None) {
                    self.status("Swap failed (someone else may have taken it)");
                }
            }
            UiCmd::Reroll => {
                if !lcu.send("POST", "/lol-champ-select/v1/session/my-selection/reroll", None) {
                    self.status("Reroll failed");
                }
                let rr = self.rerolls();
                if let Some(cs) = self.shared.write().unwrap().champ_select.as_mut() {
                    cs.rerolls = rr;
                }
                repaint();
            }
        }
    }

    // ------------------------------------------------------------------ in game

    fn reset_game(&mut self) {
        {
            let mut c = self.scan.lock().unwrap();
            c.mode = ScanMode::Off;
            c.picks = 0;
            c.game_start = 0.0;
            c.max_round = 1;
            c.build = BuildProfile::default();
        }
        self.relics.lock().unwrap().active = false;
        self.game_champ = None;
        self.build = None;
        self.level = 0;
        self.game_seen = false;
        self.picks_loaded = false;
        self.overlay.send(OverlayCmd::Badges(vec![]));
        self.overlay.send(OverlayCmd::PanelVisible(Some(false)));
        let mut s = self.shared.write().unwrap();
        s.game = None;
        s.cards.clear();
    }

    fn poll_live(&mut self) {
        let snapshot = crate::net::local()
            .get("https://127.0.0.1:2999/liveclientdata/allgamedata")
            .timeout(Duration::from_millis(900))
            .call()
            .ok()
            .and_then(|r| r.into_json::<Value>().ok());
        let Some(v) = snapshot else {
            self.scan.lock().unwrap().mode = ScanMode::Off; // loading screen
            return;
        };
        let active = &v["activePlayer"];
        let name = active["riotId"].as_str().or(active["summonerName"].as_str()).unwrap_or("").to_string();
        let level = active["level"].as_u64().unwrap_or(1) as u32;
        let players = v["allPlayers"].as_array().cloned().unwrap_or_default();
        let me = players
            .iter()
            .find(|p| p["riotId"].as_str() == Some(&name) || p["summonerName"].as_str() == Some(&name))
            .cloned()
            .unwrap_or(Value::Null);
        let dead = me["isDead"].as_bool().unwrap_or(false);
        let items: Vec<u32> = me["items"].as_array().map(|a| a.iter().map(|i| u32_of(&i["itemID"])).collect()).unwrap_or_default();
        let game_time = v["gameData"]["gameTime"].as_f64().unwrap_or(0.0);
        if !self.picks_loaded {
            self.picks_loaded = true;
            let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let start = unix - game_time;
            let mut c = self.scan.lock().unwrap();
            c.game_start = start;
            if let Some(p) = crate::state::load_picks(start).filter(|p| *p > c.picks) {
                log::info!("resumed this game's pick count: {p}");
                c.picks = p;
            }
        }
        if self.cfg.relic_timers {
            let mut r = self.relics.lock().unwrap();
            // new game (or the clock jumped back): first spawns at 1:45 outer / 2:30 inner
            if !r.active || game_time + 5.0 < r.game_time {
                r.start(self.cfg.relic_first_outer, self.cfg.relic_first_inner);
                log::info!("relic timers started at {game_time:.0}s");
            }
            r.game_time = game_time;
            r.at = Some(Instant::now());
            r.my_side_blue = me["team"].as_str().map(|t| t == "ORDER");
        }

        if self.game_champ.is_none() {
            let from_cs = self.imported;
            let from_live = me["rawChampionName"]
                .as_str()
                .and_then(|r| r.rsplit('_').next())
                .and_then(|raw| self.data.meta().ok()?.find_champ(raw));
            let Some(cid) = from_live.or(from_cs) else { return };
            self.game_champ = Some(cid);
            self.shared.write().unwrap().last_champ = Some(cid);
            self.on_my_champ(cid, false, None);
        }

        let mode = if dead || game_time < 150.0 { ScanMode::Fast } else { ScanMode::Slow };
        let unlocked = self.cfg.augment_levels.iter().filter(|l| level >= **l).count().max(1) as u32;
        let profile = self.data.items().ok().map(|dd| {
            let p = build_profile(&dd, &items);
            if p.cats.is_empty() {
                (build_profile(&dd, &self.expected_items), true)
            } else {
                (p, false)
            }
        });
        let (round, scan_error) = {
            let mut c = self.scan.lock().unwrap();
            c.mode = mode;
            c.max_round = unlocked;
            if let Some((p, _)) = &profile {
                c.build = p.clone();
            }
            (c.round(), c.error.is_some())
        };
        if let Some((p, expected)) = &profile {
            if self.build.as_ref() != Some(p) {
                self.build = Some(p.clone());
                if let Some(meta) = self.data.meta_cached() {
                    log::info!("build ({}): {:?}", if *expected { "expected" } else { "items" }, p.labels(&meta));
                }
            }
        }
        let meta = self.data.meta_cached();
        let new_state = GameState {
            champ: self.game_champ,
            level,
            round,
            dead,
            build: match (&profile, &meta) {
                (Some((p, _)), Some(m)) => p.labels(m),
                _ => vec![],
            },
            build_expected: profile.as_ref().map(|p| p.1).unwrap_or(true),
        };
        {
            let mut s = self.shared.write().unwrap();
            let changed = s.game.as_ref().map(|g| (g.level, g.round, g.dead, &g.build)) != Some((level, round, dead, &new_state.build));
            s.game = Some(new_state);
            if changed {
                drop(s);
                repaint();
            }
        }

        // without OCR, pop the panel up when an augment is due
        if scan_error {
            let due = !self.game_seen || (level != self.level && self.cfg.augment_levels.contains(&level));
            if due {
                self.overlay.send(OverlayCmd::PanelVisible(Some(true)));
                self.hide_panel_at = Some(Instant::now() + Duration::from_secs(25));
            }
        }
        self.game_seen = true;
        self.level = level;
    }

    // ------------------------------------------------------------------ demo

    pub fn demo(&mut self, champ: &str, round: u32) -> Result<(), String> {
        let meta = self.data.meta()?;
        let cid = meta.find_champ(champ).ok_or_else(|| format!("unknown champion {champ}"))?;
        let detail = self.data.champ(cid)?;
        self.on_my_champ(cid, false, None);
        let mut ranked: Vec<(u32, f64)> = meta
            .champs
            .iter()
            .filter_map(|(id, c)| Some((id.parse().ok()?, champion_rating(c, &self.cfg).delta)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let pick = |i: usize| ranked.get(i % ranked.len().max(1)).map(|r| r.0).unwrap_or(cid);
        {
            let mut s = self.shared.write().unwrap();
            s.demo = true;
            s.client = Client::Connected;
            s.mayhem = Some(true);
            s.phase = "ChampSelect".into();
            s.last_champ = Some(cid);
            s.champ_select = Some(ChampSelectState {
                my_champ: cid,
                bench: vec![pick(3), pick(40), pick(120)],
                teammates: vec![pick(10), pick(70), pick(100), pick(150)],
                rerolls: Some(2),
                bench_enabled: true,
            });
            s.status = "Demo mode".into();
        }
        let items = self.data.items()?;
        let expected: Vec<u32> = detail.clusters.groups.first().map(|g| g.core.iter().map(|i| i.id).collect()).unwrap_or_default();
        let build = build_profile(&items, &expected);
        {
            let mut c = self.scan.lock().unwrap();
            c.mode = ScanMode::Fast;
            c.max_round = 4;
            c.picks = round.saturating_sub(1);
            c.build = build.clone();
        }
        self.shared.write().unwrap().game =
            Some(GameState { champ: Some(cid), level: 11, round, dead: true, build: build.labels(&meta), build_expected: true });
        self.overlay.send(OverlayCmd::PanelVisible(Some(true)));
        {
            // demo relics: one up, one about to spawn, two counting down
            let mut r = self.relics.lock().unwrap();
            r.start(105.0, 150.0);
            r.game_time = 600.0;
            r.at = Some(Instant::now());
            r.ready_at = [600.0 + 47.0, 590.0, 600.0 + 8.0, 600.0 + 83.0];
            r.my_side_blue = Some(true);
        }
        repaint();
        Ok(())
    }
}

// ------------------------------------------------------------------ item sets

/// One item-set page per build style (the clusters' names: "AP burst", "Tank / Heartsteel"…),
/// so the shop's item-set dropdown switches between genuinely different builds. Clusters
/// sharing a style become variants on the same page. `prefer` (a cluster index) goes first.
pub fn make_item_sets(meta: &Meta, detail: &ChampDetail, cid: u32, prefer: Option<usize>) -> Vec<Value> {
    let block = |title: String, ids: Vec<u32>| {
        let mut seen = std::collections::HashSet::new();
        let items: Vec<Value> = ids.into_iter().filter(|i| seen.insert(*i)).map(|i| json!({"id": i.to_string(), "count": 1})).collect();
        json!({"type": title, "items": items})
    };
    let mut order: Vec<usize> = (0..detail.clusters.groups.len()).collect();
    if let Some(p) = prefer.filter(|p| *p < order.len()) {
        order.retain(|i| *i != p);
        order.insert(0, p);
    }
    // group by style name, keeping the order styles first appear in
    let mut styles: Vec<(String, Vec<&Cluster>)> = vec![];
    for i in order {
        let g = &detail.clusters.groups[i];
        let name = if g.name_en.is_empty() { "Build".to_string() } else { g.name_en.clone() };
        match styles.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => v.push(g),
            None => styles.push((name, vec![g])),
        }
    }
    let boots: Vec<u32> = detail.boots.top.iter().filter_map(|b| b.items.first().map(|i| i.id)).collect();
    let singles: Vec<u32> = detail.single_items.top.iter().filter_map(|s| s.items.first().map(|i| i.id)).take(10).collect();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let pages = styles.into_iter().take(4).enumerate().map(|(rank, (style, clusters))| {
        let main = clusters[0];
        let mut blocks = vec![];
        blocks.push(block(
            format!("Core · {:.1}% WR · {:.1}k games", main.wr * 100.0, main.g / 1000.0),
            main.core.iter().map(|i| i.id).collect(),
        ));
        blocks.push(block("Full build".into(), main.completion.iter().map(|i| i.id).collect()));
        blocks.push(block("Next items".into(), main.options.iter().chain(main.tail.iter()).map(|i| i.id).collect()));
        for (n, v) in clusters.iter().skip(1).enumerate() {
            blocks.push(block(
                format!("Variant {} core · {:.1}% WR", n + 2, v.wr * 100.0),
                v.core.iter().chain(v.options.iter()).map(|i| i.id).collect(),
            ));
        }
        let mut b: Vec<u32> = main.route_boot.iter().map(|i| i.id).collect();
        b.extend(boots.iter());
        blocks.push(block("Boots".into(), b));
        blocks.push(block("Highest win-rate items".into(), singles.clone()));
        blocks.retain(|b| b["items"].as_array().map(|a| !a.is_empty()).unwrap_or(false));
        json!({
            "title": format!("{TAG} {} · {style}", meta.champ_name(cid)),
            "associatedChampions": [cid], "associatedMaps": [],
            "blocks": blocks, "map": "any", "mode": "any", "type": "custom", "sortrank": rank,
            "startedFrom": "blank", "preferredItemSlots": [],
            "uid": format!("aram-mayhem-{cid}-{rank}-{stamp}"),
        })
    });
    pages.collect()
}

fn import_item_sets(lcu: &Lcu, new_sets: &[Value]) -> bool {
    let Some(me) = lcu.get("/lol-summoner/v1/current-summoner") else { return false };
    let sid = me["summonerId"].as_u64().unwrap_or(0);
    let path = format!("/lol-item-sets/v1/item-sets/{sid}/sets");
    let current = lcu.get(&path).unwrap_or(json!({}));
    let mut sets: Vec<Value> = current["itemSets"]
        .as_array()
        .map(|a| a.iter().filter(|s| !s["title"].as_str().unwrap_or("").starts_with(TAG)).cloned().collect())
        .unwrap_or_default();
    sets.extend(new_sets.iter().cloned());
    let body = json!({
        "accountId": me.get("accountId").cloned().unwrap_or(current["accountId"].clone()),
        "itemSets": sets,
        "timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
    });
    lcu.send("PUT", &path, Some(&body))
}
