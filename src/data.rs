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
    balance: RwLock<Option<Arc<crate::balance::Balance>>>,
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
            balance: RwLock::new(None),
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
                let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
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
        // A new dataset can be published within the same patch.
        self.champs.write().unwrap().clear();
        *self.items.write().unwrap() = None;
        *self.aug_stats.write().unwrap() = None;
        *self.balance.write().unwrap() = None;
        *self.meta.write().unwrap() = Some((meta.clone(), Instant::now()));
        log::info!(
            "data: patch {} ({} champions, {} augments)",
            meta.patch_prefix,
            meta.champs.len(),
            meta.augs.len()
        );
        Ok(meta)
    }

    /// Explicit refresh validates the download before replacing the last good dataset.
    pub fn refresh(&self) -> Result<(), String> {
        let _guard = self.meta_lock.lock().unwrap();
        let bytes = net::get_bytes(&format!("{}/tier-list.json", self.base))?;
        let meta: Meta = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!("tier-list-{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(tmp, self.dir.join("tier-list.json")).map_err(|e| e.to_string())?;
        self.champs.write().unwrap().clear();
        *self.items.write().unwrap() = None;
        *self.aug_stats.write().unwrap() = None;
        *self.balance.write().unwrap() = None;
        *self.meta.write().unwrap() = Some((Arc::new(meta), Instant::now()));
        Ok(())
    }

    /// Each process has its own memory cache; keep both fresh while left open.
    pub fn start_refresh(self: &Arc<Self>, done: fn()) {
        let data = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            if crate::config::Config::load().auto_update_data {
                if let Err(e) = data.meta() {
                    log::warn!("data refresh: {e}");
                }
                done();
            }
        });
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
        let max_age = tier_age.min(self.max_age);
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

    /// ARAM + Mayhem champion balance changes (League of Legends Wiki, refreshed daily).
    pub fn balance(&self) -> Result<Arc<crate::balance::Balance>, String> {
        if let Some(b) = self.balance.read().unwrap().as_ref() {
            return Ok(b.clone());
        }
        let day = Some(Duration::from_secs(24 * 3600));
        let wiki = "https://wiki.leagueoflegends.com/en-us";
        let lua = self.fetch("wiki-champion-data.lua", &format!("{wiki}/Module:ChampionData/data?action=raw"), day)?;
        let page = self.fetch("wiki-aram-mayhem.txt", &format!("{wiki}/ARAM:_Mayhem?action=raw"), day)?;
        let b = crate::balance::build(&String::from_utf8_lossy(&lua), &String::from_utf8_lossy(&page));
        log::info!("balance: {} champions with ARAM/Mayhem changes", b.len());
        let b = Arc::new(b);
        *self.balance.write().unwrap() = Some(b.clone());
        Ok(b)
    }

    pub fn balance_cached(&self) -> Option<Arc<crate::balance::Balance>> {
        self.balance.read().unwrap().clone()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn same_patch_refresh_invalidates_memory_and_bad_download_keeps_last_good_data() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for body in ["{}", "{}", "invalid json"] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let mut data = Data::new(&url, 12.0);
        let dir = std::env::temp_dir().join(format!("mayhem-refresh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::get_mut(&mut data).unwrap().dir = dir.clone();
        data.refresh().unwrap();
        let first = data.meta_cached().unwrap();
        data.champs.write().unwrap().insert(1, Arc::new(ChampDetail::default()));
        data.refresh().unwrap();
        assert!(data.champ_cached(1).is_none());
        let second = data.meta_cached().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        let good_disk = std::fs::read(dir.join("tier-list.json")).unwrap();
        assert!(data.refresh().is_err());
        assert!(Arc::ptr_eq(&second, &data.meta_cached().unwrap()));
        assert_eq!(good_disk, std::fs::read(dir.join("tier-list.json")).unwrap());
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
