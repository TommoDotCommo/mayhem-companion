//! arammeta.com + Data Dragon downloads with an on-disk cache.

use crate::config::data_dir;
use crate::model::{AugStatsFile, ChampDetail, DdItem, DdItemFile, Meta};
use crate::net;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

pub struct Data {
    base: String,
    dir: PathBuf,
    max_age: Duration,
    meta: RwLock<Option<(Arc<Meta>, Instant)>>,
    champs: RwLock<HashMap<u32, Arc<ChampDetail>>>,
    items: RwLock<Option<Arc<HashMap<String, DdItem>>>>,
    aug_stats: RwLock<Option<Arc<AugStatsFile>>>,
    in_flight: Mutex<HashSet<String>>,
    failed: Mutex<HashMap<String, (Instant, String)>>,
    meta_lock: Mutex<()>,
}

fn age(path: &PathBuf) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

impl Data {
    pub fn new(base_url: &str, cache_hours: f64) -> Arc<Data> {
        let dir = data_dir().join("cache");
        let _ = std::fs::create_dir_all(&dir);
        Arc::new(Data {
            base: base_url.trim_end_matches('/').to_string(),
            dir,
            max_age: Duration::from_secs_f64(cache_hours * 3600.0),
            meta: RwLock::new(None),
            champs: RwLock::new(HashMap::new()),
            items: RwLock::new(None),
            aug_stats: RwLock::new(None),
            in_flight: Mutex::new(HashSet::new()),
            failed: Mutex::new(HashMap::new()),
            meta_lock: Mutex::new(()),
        })
    }

    /// Cached download. `max_age` None = never expires; falls back to a stale copy offline.
    fn fetch(&self, file: &str, url: &str, max_age: Option<Duration>) -> Result<Vec<u8>, String> {
        let path = self.dir.join(file);
        let fresh = match (age(&path), max_age) {
            (Some(a), Some(m)) => a < m,
            (Some(_), None) => true,
            _ => false,
        };
        if fresh {
            if let Ok(bytes) = std::fs::read(&path) {
                return Ok(bytes);
            }
        }
        match net::get_bytes(url) {
            Ok(bytes) => {
                let tmp = path.with_extension("tmp");
                if std::fs::write(&tmp, &bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
                Ok(bytes)
            }
            Err(e) => {
                log::warn!("download failed ({e}); trying cache");
                std::fs::read(&path).map_err(|_| e)
            }
        }
    }

    pub fn meta(&self) -> Result<Arc<Meta>, String> {
        if let Some((m, at)) = self.meta.read().unwrap().as_ref() {
            if at.elapsed() < self.max_age {
                return Ok(m.clone());
            }
        }
        let _guard = self.meta_lock.lock().unwrap();
        if let Some((m, at)) = self.meta.read().unwrap().as_ref() {
            if at.elapsed() < self.max_age {
                return Ok(m.clone());
            }
        }
        let bytes = self.fetch("tier-list.json", &format!("{}/tier-list.json", self.base), Some(self.max_age))?;
        let meta: Meta = serde_json::from_slice(&bytes).map_err(|e| format!("tier-list.json: {e}"))?;
        let meta = Arc::new(meta);
        let changed = self.meta.read().unwrap().as_ref().map(|(m, _)| m.patch_prefix != meta.patch_prefix);
        if changed.unwrap_or(false) {
            self.champs.write().unwrap().clear();
        }
        *self.meta.write().unwrap() = Some((meta.clone(), Instant::now()));
        log::info!("data: patch {} ({} champions, {} augments)", meta.patch_prefix, meta.champs.len(), meta.augs.len());
        Ok(meta)
    }

    pub fn meta_cached(&self) -> Option<Arc<Meta>> {
        self.meta.read().unwrap().as_ref().map(|(m, _)| m.clone())
    }

    pub fn champ(&self, cid: u32) -> Result<Arc<ChampDetail>, String> {
        if let Some(d) = self.champs.read().unwrap().get(&cid) {
            return Ok(d.clone());
        }
        let file = format!("champions_{cid}.json");
        // a champion file older than the tier list belongs to an older dataset
        let tier_age = age(&self.dir.join("tier-list.json")).unwrap_or(self.max_age);
        let max_age = tier_age.min(self.max_age) + Duration::from_secs(60);
        let bytes = self.fetch(&file, &format!("{}/champions/{cid}.json", self.base), Some(max_age))?;
        let detail: ChampDetail = serde_json::from_slice(&bytes).map_err(|e| format!("{file}: {e}"))?;
        let detail = Arc::new(detail);
        self.champs.write().unwrap().insert(cid, detail.clone());
        Ok(detail)
    }

    pub fn champ_cached(&self, cid: u32) -> Option<Arc<ChampDetail>> {
        self.champs.read().unwrap().get(&cid).cloned()
    }

    pub fn items(&self) -> Result<Arc<HashMap<String, DdItem>>, String> {
        if let Some(i) = self.items.read().unwrap().as_ref() {
            return Ok(i.clone());
        }
        let ver = self.meta()?.ddv.clone();
        let bytes = self.fetch(
            &format!("ddragon-items-{ver}.json"),
            &format!("https://ddragon.leagueoflegends.com/cdn/{ver}/data/en_US/item.json"),
            None,
        )?;
        let file: DdItemFile = serde_json::from_slice(&bytes).map_err(|e| format!("item.json: {e}"))?;
        let items = Arc::new(file.data);
        *self.items.write().unwrap() = Some(items.clone());
        Ok(items)
    }

    pub fn aug_stats(&self) -> Result<Arc<AugStatsFile>, String> {
        if let Some(a) = self.aug_stats.read().unwrap().as_ref() {
            return Ok(a.clone());
        }
        let bytes = self.fetch(
            "augment-stats.json",
            &format!("{}/champions/augment-stats.json", self.base),
            Some(self.max_age),
        )?;
        let stats: AugStatsFile = serde_json::from_slice(&bytes).map_err(|e| format!("augment-stats: {e}"))?;
        let stats = Arc::new(stats);
        *self.aug_stats.write().unwrap() = Some(stats.clone());
        Ok(stats)
    }

    pub fn aug_stats_cached(&self) -> Option<Arc<AugStatsFile>> {
        self.aug_stats.read().unwrap().clone()
    }

    /// Run a load on a background thread once; failures back off for 30 s.
    /// `done` runs afterwards (e.g. to repaint the UI).
    pub fn load_async(
        self: &Arc<Self>,
        key: String,
        job: impl FnOnce(&Data) -> Result<(), String> + Send + 'static,
        done: impl FnOnce() + Send + 'static,
    ) {
        if let Some((at, _)) = self.failed.lock().unwrap().get(&key) {
            if at.elapsed() < Duration::from_secs(30) {
                return;
            }
        }
        if !self.in_flight.lock().unwrap().insert(key.clone()) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            match job(&me) {
                Ok(()) => {
                    me.failed.lock().unwrap().remove(&key);
                }
                Err(e) => {
                    log::warn!("load {key} failed: {e}");
                    me.failed.lock().unwrap().insert(key.clone(), (Instant::now(), e));
                }
            }
            me.in_flight.lock().unwrap().remove(&key);
            done();
        });
    }

    pub fn load_error(&self, key: &str) -> Option<String> {
        self.failed.lock().unwrap().get(key).map(|(_, e)| e.clone())
    }
}
