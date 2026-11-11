//! ARAM challenge progress from the League client, kept as a small local snapshot per account.
//! Mayhem games count toward these challenges. Only the owner's own progress is stored.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// "ARAM Authority": the capstone over ARAM Warrior, Finesse and Champion. Master awards the ARAM God title.
pub const ROOT: u64 = 101000;
pub const LEVELS: [&str; 10] = ["NONE", "IRON", "BRONZE", "SILVER", "GOLD", "PLATINUM", "DIAMOND", "MASTER", "GRANDMASTER", "CHALLENGER"];

pub fn rank(level: &str) -> usize {
    LEVELS.iter().position(|l| *l == level).unwrap_or(0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Title {
    pub level: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Challenge {
    pub id: u64,
    pub parent: u64,
    /// The client's name for the group this challenge sits in.
    pub parent_name: String,
    pub name: String,
    pub description: String,
    pub capstone: bool,
    pub level: String,
    pub value: f64,
    /// Where the current level starts. Grandmaster and Challenger cut-offs move with the ladder.
    pub current_threshold: f64,
    pub next_level: String,
    pub next_threshold: f64,
    /// Share of players at this level or above, in percent.
    pub percentile: f64,
    pub reverse: bool,
    /// Fixed level requirements, lowest first.
    pub thresholds: Vec<(String, f64)>,
    pub titles: Vec<Title>,
    /// What `completed_ids` counts: "CHAMPION" for one-per-champion challenges, otherwise "NONE" or empty.
    pub id_list_type: String,
    pub completed_ids: Vec<u32>,
    /// The ids that count; empty means every one of that type.
    pub available_ids: Vec<u32>,
}

impl Challenge {
    /// Progress is one per champion, and the client says which champions are done.
    pub fn per_champion(&self) -> bool {
        self.id_list_type == "CHAMPION"
    }
    /// Champions done and still to do, given every champion id that exists. Both sorted by id.
    pub fn champions(&self, all: impl Iterator<Item = u32>) -> (Vec<u32>, Vec<u32>) {
        let mut done = self.completed_ids.clone();
        done.sort_unstable();
        done.dedup();
        let mut left: Vec<u32> = if self.available_ids.is_empty() { all.collect() } else { self.available_ids.clone() };
        left.sort_unstable();
        left.dedup();
        left.retain(|id| done.binary_search(id).is_err());
        (done, left)
    }
    pub fn maxed(&self) -> bool {
        self.next_level.is_empty() || self.next_threshold <= 0.0
    }
    pub fn threshold(&self, level: &str) -> Option<f64> {
        self.thresholds.iter().find(|(l, _)| l == level).map(|(_, v)| *v)
    }
    /// Fraction of the way from the current level to the next; None when it can't be drawn as a bar.
    pub fn progress(&self) -> Option<f64> {
        if self.maxed() {
            return Some(1.0);
        }
        if self.reverse {
            return None;
        }
        let start = if self.current_threshold > 0.0 { self.current_threshold } else { self.threshold(&self.level).unwrap_or(0.0) };
        let span = self.next_threshold - start;
        (span > 0.0).then(|| ((self.value - start) / span).clamp(0.0, 1.0))
    }
    pub fn has(&self, level: &str) -> bool {
        rank(&self.level) >= rank(level)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Snapshot {
    pub fetched_ms: u64,
    pub equipped_title: String,
    /// The ARAM Authority tree: root first, then each group followed by its challenges.
    pub challenges: Vec<Challenge>,
    /// Live ARAM-only challenges outside the ARAM Authority tree.
    pub other_aram: Vec<Challenge>,
    /// Challenges that aren't ARAM-specific but progress from Mayhem games: champion mastery and summoner level.
    pub any_mode: Vec<Challenge>,
}

impl Snapshot {
    pub fn get(&self, id: u64) -> Option<&Challenge> {
        self.challenges.iter().find(|c| c.id == id)
    }
    pub fn children(&self, id: u64) -> impl Iterator<Item = &Challenge> {
        self.challenges.iter().filter(move |c| c.parent == id && c.id != id)
    }
    /// Every title in the tree with the challenge that awards it.
    pub fn titles(&self) -> Vec<(&Challenge, &Title)> {
        self.challenges.iter().flat_map(|c| c.titles.iter().map(move |t| (c, t))).collect()
    }
    /// Unfinished challenges closest to their next level.
    pub fn closest(&self, n: usize) -> Vec<&Challenge> {
        let mut rows: Vec<_> = self.challenges.iter().filter(|c| !c.capstone && !c.maxed() && c.progress().is_some()).collect();
        rows.sort_by(|a, b| b.progress().unwrap_or(0.0).total_cmp(&a.progress().unwrap_or(0.0)).then(a.id.cmp(&b.id)));
        rows.truncate(n);
        rows
    }
}

fn challenge(v: &Value) -> Challenge {
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let num = |k: &str| v[k].as_f64().filter(|n| n.is_finite()).unwrap_or(0.0);
    let mut thresholds = vec![];
    let mut titles = vec![];
    if let Some(map) = v["thresholds"].as_object() {
        for (level, t) in map {
            if let Some(value) = t["value"].as_f64() {
                thresholds.push((level.clone(), value));
            }
            for r in t["rewards"].as_array().into_iter().flatten() {
                if r["category"].as_str() == Some("TITLE") {
                    if let Some(name) = r["name"].as_str().filter(|n| !n.is_empty()) {
                        titles.push(Title { level: level.clone(), name: name.into() });
                    }
                }
            }
        }
    }
    thresholds.sort_by_key(|(l, _)| rank(l));
    titles.sort_by_key(|t| rank(&t.level));
    let level = text("currentLevel");
    let id_list = |k: &str| -> Vec<u32> { v[k].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32).collect() };
    Challenge {
        id: v["id"].as_u64().unwrap_or(0),
        parent: v["parentId"].as_u64().unwrap_or(0),
        parent_name: text("parentName"),
        name: text("name"),
        description: text("description"),
        capstone: v["isCapstone"].as_bool().unwrap_or(false),
        level: if level.is_empty() { "NONE".into() } else { level },
        value: num("currentValue"),
        current_threshold: num("currentThreshold"),
        next_level: text("nextLevel"),
        next_threshold: num("nextThreshold"),
        percentile: num("percentile"),
        reverse: v["isReverseDirection"].as_bool().unwrap_or(false),
        thresholds,
        titles,
        id_list_type: text("idListType"),
        completed_ids: id_list("completedIds"),
        available_ids: id_list("availableIds"),
    }
}

/// `all` is `/lol-challenges/v1/challenges/local-player` (a map of id to challenge); `summary` is the player summary.
pub fn parse(all: &Value, summary: &Value, now_ms: u64) -> Result<Snapshot, String> {
    let map = all.as_object().ok_or("League did not return challenge data")?;
    let find = |id: u64| map.get(&id.to_string()).or_else(|| map.values().find(|v| v["id"].as_u64() == Some(id)));
    let root = find(ROOT).ok_or("ARAM challenges are not available yet")?;
    let mut out = vec![challenge(root)];
    let ids = |v: &Value| -> Vec<u64> { v["childrenIds"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect() };
    for group_id in ids(root) {
        let Some(group) = find(group_id) else { continue };
        let mut group_row = challenge(group);
        group_row.parent = ROOT;
        out.push(group_row);
        let mut members: Vec<Challenge> = ids(group).into_iter().filter_map(|id| find(id)).map(challenge).collect();
        // Retired or missing children may still be listed; only keep the ones under this group.
        members.retain(|c| c.id > 0);
        members.iter_mut().for_each(|c| c.parent = group_id);
        members.sort_by_key(|c| c.id);
        out.extend(members);
    }
    let (mut other_aram, mut any_mode) = (vec![], vec![]);
    for v in map.values() {
        let id = v["id"].as_u64().unwrap_or(0);
        let retired = v["retireTimestamp"].as_f64().unwrap_or(0.0) > 0.0;
        if id == 0 || retired || v["isCapstone"].as_bool().unwrap_or(false) || out.iter().any(|c| c.id == id) {
            continue;
        }
        let modes: Vec<&str> = v["gameModes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let aram = |m: &&str| matches!(*m, "ARAM" | "KIWI" | "KIWI_JADE");
        // KIWI is ARAM: Mayhem. Mastery and summoner level list no modes but grow in every mode.
        let mayhem = modes.contains(&"KIWI");
        if mayhem && modes.iter().all(aram) {
            other_aram.push(challenge(v));
        } else if mayhem || (modes.is_empty() && matches!(v["source"].as_str(), Some("EOGD_CHAMPION_MASTERY" | "SUMMONER"))) {
            any_mode.push(challenge(v));
        }
    }
    other_aram.sort_by_key(|c| c.id);
    any_mode.sort_by_key(|c| c.id);
    Ok(Snapshot {
        other_aram,
        any_mode,
        fetched_ms: now_ms,
        equipped_title: summary["title"]["name"].as_str().unwrap_or("").to_string(),
        challenges: out,
    })
}

fn file(dir: &Path, account: &str) -> Result<PathBuf, String> {
    if !crate::history::safe_key(account) {
        return Err("Invalid history account".into());
    }
    Ok(dir.join(format!("{account}.challenges.json")))
}
pub fn load(dir: &Path, account: &str) -> Option<Snapshot> {
    serde_json::from_slice(&std::fs::read(file(dir, account).ok()?).ok()?).ok()
}
pub fn save(dir: &Path, account: &str, snapshot: &Snapshot) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = file(dir, account)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(snapshot).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}

/// Fetch, parse and save. Returns true when the saved progress changed.
pub fn sync(dir: &Path, account: &str, fetch: impl Fn(&str) -> Result<Value, String>, now_ms: u64) -> Result<bool, String> {
    let all = fetch("/lol-challenges/v1/challenges/local-player")?;
    let summary = fetch("/lol-challenges/v1/summary-player-data/local-player").unwrap_or(Value::Null);
    let snapshot = parse(&all, &summary, now_ms)?;
    let changed = load(dir, account).map_or(true, |old| Snapshot { fetched_ms: now_ms, ..old } != snapshot);
    save(dir, account, &snapshot)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        let points = |q: u64| json!({"category": "CHALLENGE_POINTS", "name": "", "quantity": q});
        json!({
            "101000": {"id": 101000, "name": "ARAM Authority", "isCapstone": true, "parentId": 1, "childrenIds": [101100],
                "currentLevel": "DIAMOND", "currentValue": 1500.0, "currentThreshold": 0.0, "nextLevel": "MASTER", "nextThreshold": 1850.0,
                "percentile": 1.8,
                "thresholds": {"DIAMOND": {"value": 1075.0, "rewards": [points(60)]},
                               "MASTER": {"value": 1850.0, "rewards": [{"category": "TITLE", "name": "ARAM God", "quantity": 1}, points(100)]}}},
            "101100": {"id": 101100, "name": "ARAM Warrior", "isCapstone": true, "parentId": 101000, "childrenIds": [101107, 101306, 999],
                "currentLevel": "MASTER", "currentValue": 800.0, "nextLevel": "", "nextThreshold": 0.0,
                "thresholds": {"DIAMOND": {"value": 340.0, "rewards": [{"category": "TITLE", "name": "Unstoppable", "quantity": 1}]}}},
            "101107": {"id": 101107, "name": "Farm Champs Not Minions", "description": "Get Takedowns in ARAM", "parentId": 101100,
                "currentLevel": "GRANDMASTER", "currentValue": 73518.0, "currentThreshold": 73422.0, "nextLevel": "CHALLENGER", "nextThreshold": 151330.0,
                "thresholds": {"MASTER": {"value": 30000.0}, "GRANDMASTER": {"value": 40000.0}}},
            "101306": {"id": 101306, "name": "Can't Touch This", "parentId": 101100,
                "currentLevel": "GOLD", "currentValue": 0.0, "nextLevel": "PLATINUM", "nextThreshold": 1.0,
                "idListType": "CHAMPION", "completedIds": [266, 1, 1], "availableIds": [],
                "thresholds": {"PLATINUM": {"value": 1.0, "rewards": [{"category": "TITLE", "name": "Untouchable", "quantity": 1}]}}},
            "5": {"id": 5, "name": "Unrelated", "parentId": 2},
            "6": {"id": 6, "name": "Rift only", "parentId": 2, "gameModes": ["CLASSIC", "SWIFTPLAY"]},
            "7": {"id": 7, "name": "Old ARAM", "gameModes": ["ARAM", "KIWI"], "retireTimestamp": 1668430799000u64},
            "8": {"id": 8, "name": "New ARAM", "gameModes": ["ARAM", "KIWI", "KIWI_JADE"], "retireTimestamp": 0},
            "9": {"id": 9, "name": "Everywhere", "gameModes": ["ARAM", "CLASSIC", "KIWI"]},
            "401104": {"id": 401104, "name": "Master Yourself", "parentId": 401100, "parentName": "Guru", "gameModes": [],
                "source": "EOGD_CHAMPION_MASTERY", "idListType": "CHAMPION", "completedIds": [3]},
            "401100": {"id": 401100, "name": "Guru", "isCapstone": true, "gameModes": [], "source": "CHALLENGES"},
            "510001": {"id": 510001, "name": "Fashion Forward", "gameModes": [], "source": "CAP_INVENTORY"}
        })
    }

    #[test]
    fn keeps_only_the_aram_tree_with_titles_and_progress() {
        let snap = parse(&fixture(), &json!({"title": {"name": "Poroyalty"}}), 42).unwrap();
        let ids: Vec<_> = snap.challenges.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![101000, 101100, 101107, 101306]);
        assert_eq!(snap.equipped_title, "Poroyalty");
        // Outside the tree: live ARAM-only challenges, and ones every mode (so Mayhem) feeds.
        assert_eq!(snap.other_aram.iter().map(|c| c.id).collect::<Vec<_>>(), vec![8]);
        assert_eq!(snap.any_mode.iter().map(|c| c.id).collect::<Vec<_>>(), vec![9, 401104]);
        assert_eq!(snap.any_mode[1].parent_name, "Guru");
        let root = snap.get(ROOT).unwrap();
        assert!(!root.has("MASTER"));
        assert_eq!(root.titles, vec![Title { level: "MASTER".into(), name: "ARAM God".into() }]);
        // Diamond starts at 1075 (static) because the client reports no current threshold.
        assert!((root.progress().unwrap() - (1500.0 - 1075.0) / (1850.0 - 1075.0)).abs() < 1e-9);
        assert_eq!(snap.get(101100).unwrap().progress(), Some(1.0));
        // Ladder levels use the live cut-offs.
        let farm = snap.get(101107).unwrap();
        assert!(farm.progress().unwrap() < 0.01);
        assert_eq!(snap.titles().len(), 3);
        assert_eq!(snap.children(101100).count(), 2);
        assert_eq!(snap.closest(1)[0].id, 101107);
        // One-per-champion challenges say which champions are done.
        assert!(!farm.per_champion());
        let touch = snap.get(101306).unwrap();
        assert!(touch.per_champion());
        assert_eq!(touch.champions([1, 2, 266, 3].into_iter()), (vec![1, 266], vec![2, 3]));
        let limited = Challenge { available_ids: vec![5, 1], ..touch.clone() };
        assert_eq!(limited.champions([1, 2, 3].into_iter()).1, vec![5]);
    }

    #[test]
    fn missing_tree_is_an_error_and_sync_saves_per_account() {
        assert!(parse(&json!({}), &Value::Null, 0).is_err());
        assert!(parse(&json!([]), &Value::Null, 0).is_err());
        let dir = std::env::temp_dir().join(format!("mayhem-challenges-test-{}", std::process::id()));
        let account = crate::history::account_key("owner");
        let fetch = |path: &str| -> Result<Value, String> {
            if path.ends_with("challenges/local-player") { Ok(fixture()) } else { Err("no summary".into()) }
        };
        assert!(sync(&dir, &account, fetch, 1).unwrap());
        assert!(!sync(&dir, &account, fetch, 2).unwrap(), "same progress at a later time is not a change");
        assert_eq!(load(&dir, &account).unwrap().fetched_ms, 2);
        assert!(load(&dir, &crate::history::account_key("other")).is_none());
        assert!(save(&dir, "../x", &Snapshot::default()).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
