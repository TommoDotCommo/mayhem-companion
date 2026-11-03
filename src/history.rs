//! Local-only personal results. Files contain only the owner's game statistics and an opaque account key.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MatchRecord {
    pub game_id: u64,
    pub champion: u32,
    pub patch: String,
    pub created_ms: u64,
    pub duration_secs: u64,
    pub win: bool,
    pub kills: u64,
    pub deaths: u64,
    pub assists: u64,
    pub damage: u64,
    pub items: Vec<u32>,
    pub augments: Vec<u32>,
}
#[derive(Clone, Copy, Default)]
pub struct Summary {
    pub games: u64,
    pub wins: u64,
    pub kills: u64,
    pub deaths: u64,
    pub assists: u64,
    pub damage: u64,
}
impl Summary {
    pub fn add(&mut self, game: &MatchRecord) {
        self.games += 1;
        self.wins += u64::from(game.win);
        self.kills += game.kills;
        self.deaths += game.deaths;
        self.assists += game.assists;
        self.damage += game.damage;
    }
    pub fn win_rate(&self) -> f64 {
        if self.games == 0 {
            0.0
        } else {
            self.wins as f64 / self.games as f64
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Champion,
    Augment,
    Item,
}
pub fn summarize(games: &[MatchRecord], group: Group) -> Vec<(u32, Summary)> {
    let mut rows: HashMap<u32, Summary> = HashMap::new();
    for game in games {
        let ids = match group {
            Group::Champion => vec![game.champion],
            Group::Augment => game.augments.clone(),
            Group::Item => game.items.clone(),
        };
        let mut seen = std::collections::HashSet::new();
        for id in ids.into_iter().filter(|id| *id > 0 && seen.insert(*id)) {
            rows.entry(id).or_default().add(game);
        }
    }
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by(|a, b| b.1.games.cmp(&a.1.games).then_with(|| b.1.wins.cmp(&a.1.wins)).then(a.0.cmp(&b.0)));
    rows
}

pub fn account_key(puuid: &str) -> String {
    format!("{:x}", Sha256::digest(puuid.as_bytes()))
}
fn safe_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}
fn file(dir: &Path, account: &str) -> Result<PathBuf, String> {
    if !safe_key(account) {
        return Err("Invalid history account".into());
    }
    Ok(dir.join(format!("{account}.json")))
}
pub fn load(dir: &Path, account: &str) -> Result<Vec<MatchRecord>, String> {
    let path = file(dir, account)?;
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("Couldn't read saved history: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.to_string()),
    }
}
pub fn last_account(dir: &Path) -> Option<String> {
    let key = std::fs::read_to_string(dir.join("last-account.txt")).ok()?;
    safe_key(&key).then_some(key)
}
pub fn save(dir: &Path, account: &str, records: &[MatchRecord]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = file(dir, account)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(records).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("last-account.txt"), account).map_err(|e| e.to_string())
}
pub fn merge(records: &mut Vec<MatchRecord>, game: MatchRecord) {
    if let Some(old) = records.iter_mut().find(|old| old.game_id == game.game_id) {
        *old = game;
    } else {
        records.push(game);
    }
    records.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then(b.game_id.cmp(&a.game_id)));
}
pub fn is_mayhem(v: &Value, queues: &[i64]) -> bool {
    v["queueId"].as_i64().is_some_and(|id| queues.contains(&id)) || v["gameMode"].as_str() == Some("KIWI")
}
/// Identify the local participant by identity. Never assume the first participant is the owner.
pub fn parse_match(v: &Value, puuid: &str, summoner: u64, queues: &[i64]) -> Result<Option<MatchRecord>, String> {
    if !is_mayhem(v, queues) {
        return Ok(None);
    }
    let pid = v["participantIdentities"]
        .as_array()
        .and_then(|ids| {
            ids.iter().find(|id| {
                let p = &id["player"];
                p["puuid"].as_str().is_some_and(|p| !puuid.is_empty() && p == puuid) || (summoner > 0 && p["summonerId"].as_u64() == Some(summoner))
            })
        })
        .and_then(|id| id["participantId"].as_u64())
        .ok_or("Local player was not identified in this match")?;
    let p = v["participants"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["participantId"].as_u64() == Some(pid)))
        .ok_or("Personal match statistics are not available yet")?;
    let stats = &p["stats"];
    let win = stats["win"]
        .as_bool()
        .or_else(|| match stats["win"].as_str()? {
            "Win" => Some(true),
            "Fail" | "Loss" => Some(false),
            _ => None,
        })
        .ok_or("Match result is not available yet")?;
    let number = |key: &str| stats[key].as_u64().ok_or_else(|| format!("Match statistic {key} is not available yet"));
    let ids = |prefix: &str, range: std::ops::RangeInclusive<u32>| {
        range
            .filter_map(|i| stats[format!("{prefix}{i}")].as_u64())
            .filter(|id| *id > 0 && *id <= u32::MAX as u64)
            .map(|id| id as u32)
            .collect()
    };
    let game_id = v["gameId"].as_u64().filter(|id| *id > 0).ok_or("Missing match ID")?;
    let champion = p["championId"]
        .as_u64()
        .filter(|id| *id > 0 && *id <= u32::MAX as u64)
        .ok_or("Missing champion")? as u32;
    Ok(Some(MatchRecord {
        game_id,
        champion,
        patch: v["gameVersion"].as_str().unwrap_or("").split('.').take(2).collect::<Vec<_>>().join("."),
        created_ms: v["gameCreation"].as_u64().unwrap_or(0),
        duration_secs: v["gameDuration"].as_u64().unwrap_or(0),
        win,
        kills: number("kills")?,
        deaths: number("deaths")?,
        assists: number("assists")?,
        damage: number("totalDamageDealtToChampions")?,
        items: ids("item", 0..=6),
        augments: ids("playerAugment", 1..=6),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value {
        json!({"gameId":7,"queueId":2400,"gameVersion":"16.21.123","gameCreation":1000,
        "participantIdentities":[{"participantId":1,"player":{"puuid":"other"}},{"participantId":2,"player":{"puuid":"owner","summonerId":8}}],
        "participants":[{"participantId":1,"championId":2,"stats":{"win":false}},
            {"participantId":2,"championId":1,"stats":{"win":true,"kills":9,"deaths":0,"assists":4,"totalDamageDealtToChampions":12000,"item0":123,"item1":123,"playerAugment1":44}}]})
    }
    #[test]
    fn personal_identity_queue_and_result_are_required() {
        let v = fixture();
        let game = parse_match(&v, "owner", 0, &[2400]).unwrap().unwrap();
        assert_eq!((game.champion, game.kills, game.deaths, game.patch.as_str()), (1, 9, 0, "16.21"));
        assert_eq!(game.augments, vec![44]);
        assert!(parse_match(&v, "missing", 0, &[2400]).is_err());
        assert!(parse_match(&v, "owner", 0, &[450]).unwrap().is_none());
        let mut unfinished = v.clone();
        unfinished["participants"][1]["stats"]["win"] = Value::Null;
        assert!(parse_match(&unfinished, "owner", 0, &[2400]).is_err());
        unfinished=v.clone();unfinished["participants"][1]["stats"]["kills"]=Value::Null;
        assert!(parse_match(&unfinished,"owner",0,&[2400]).is_err());
        assert_eq!(parse_match(&v, "", 8, &[2400]).unwrap().unwrap(), game);
    }
    #[test]
    fn deduplication_summary_and_accounts() {
        let game = parse_match(&fixture(), "owner", 0, &[2400]).unwrap().unwrap();
        let mut games = vec![];
        merge(&mut games, game.clone());
        merge(&mut games, game);
        assert_eq!(games.len(), 1);
        let rows = summarize(&games, Group::Item);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1.games, 1);
        assert_eq!(rows[0].1.win_rate(), 1.0);
        let dir = std::env::temp_dir().join(format!("mayhem-history-test-{}", std::process::id()));
        let a = account_key("owner");
        let b = account_key("other");
        save(&dir, &a, &games).unwrap();
        let saved=std::fs::read_to_string(file(&dir,&a).unwrap()).unwrap();
        assert!(!saved.contains("owner") && !saved.contains("other"));
        assert_eq!(load(&dir, &a).unwrap(), games);
        assert!(load(&dir, &b).unwrap().is_empty());
        assert_eq!(last_account(&dir), Some(a.clone()));
        assert!(load(&dir, "../config").is_err());
        std::fs::write(file(&dir, &a).unwrap(), b"bad json").unwrap();
        assert!(load(&dir, &a).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

pub struct SyncResult {
    pub account: String,
    pub games: usize,
    pub added: usize,
    pub unavailable: usize,
}
/// Backfill recent games and retry incomplete results on the next sync. Credentials and names never reach disk.
pub fn sync(dir: &Path, queues: &[i64], fetch: impl Fn(&str) -> Result<Value, String>, account_ready: impl Fn(&str)) -> Result<SyncResult, String> {
    let me = fetch("/lol-summoner/v1/current-summoner")?;
    let puuid = me["puuid"].as_str().filter(|s| !s.is_empty()).ok_or("Waiting for your League account")?;
    let summoner = me["summonerId"].as_u64().unwrap_or(0);
    let account = account_key(puuid);
    account_ready(&account);
    let mut records = load(dir, &account)?;
    let recent = fetch("/lol-match-history/v1/products/lol/current-summoner/matches?begIndex=0&endIndex=39")?;
    let games = recent["games"]["games"]
        .as_array()
        .or_else(|| recent["games"].as_array())
        .or_else(|| recent.as_array())
        .ok_or("League did not return a match list; try again after a game")?;
    let mut added = 0;
    let mut unavailable = 0;
    for summary in games.iter().filter(|v| is_mayhem(v, queues)) {
        let Some(id) = summary["gameId"].as_u64().filter(|id| *id > 0) else {
            continue;
        };
        if records.iter().any(|game| game.game_id == id) {
            continue;
        }
        match fetch(&format!("/lol-match-history/v1/games/{id}")).and_then(|v| parse_match(&v, puuid, summoner, queues)) {
            Ok(Some(game)) if game.game_id == id => {
                merge(&mut records, game);
                added += 1;
            }
            Ok(_) | Err(_) => unavailable += 1,
        }
    }
    save(dir, &account, &records)?;
    Ok(SyncResult {
        account,
        games: records.len(),
        added,
        unavailable,
    })
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn sync_filters_queues_deduplicates_and_retries_unavailable_details() {
        let dir = std::env::temp_dir().join(format!("mayhem-sync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fetch = |path: &str| -> Result<Value, String> {
            if path.ends_with("current-summoner") {
                return Ok(json!({"puuid":"owner","summonerId":9}));
            }
            if path.contains("/matches?") {
                return Ok(json!({"games":{"games":[{"gameId":1,"queueId":2400},{"gameId":2,"queueId":2400},{"gameId":3,"queueId":450}]}}));
            }
            if path.ends_with("/2") {
                return Err("not ready".into());
            }
            if path.ends_with("/1") {
                return Ok(json!({"gameId":1,"queueId":2400,"gameVersion":"16.21.1",
                "participantIdentities":[{"participantId":1,"player":{"puuid":"owner"}}],
                "participants":[{"participantId":1,"championId":1,"stats":{"win":true,"kills":1,"deaths":0,"assists":3,"totalDamageDealtToChampions":1234}}]}));
            }
            panic!("unexpected endpoint: {path}");
        };
        let first = sync(&dir, &[2400], fetch, |key| assert_eq!(key, account_key("owner"))).unwrap();
        assert_eq!((first.games, first.added, first.unavailable), (1, 1, 1));
        let second = sync(
            &dir,
            &[2400],
            |path| {
                assert!(!path.ends_with("/1"), "already imported games must not be refetched");
                fetch(path)
            },
            |_| {},
        )
        .unwrap();
        assert_eq!((second.games, second.added, second.unavailable), (1, 0, 1));
        assert!(load(&dir, &account_key("different owner")).unwrap().is_empty());
        assert!(sync(&dir, &[2400], |_| Err("offline".into()), |_| {}).is_err());
        assert_eq!(load(&dir, &first.account).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
