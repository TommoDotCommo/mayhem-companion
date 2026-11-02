//! arammeta.com data models, tier maths, build detection and augment fit.

use crate::config::{Config, Cutoffs};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ------------------------------------------------------------------ tiers

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    SPlus,
    S,
    A,
    B,
    C,
    D,
    F,
}

impl Tier {
    pub fn from_delta(delta: f64, c: &Cutoffs) -> Tier {
        match delta {
            d if d >= c.s_plus => Tier::SPlus,
            d if d >= c.s => Tier::S,
            d if d >= c.a => Tier::A,
            d if d >= c.b => Tier::B,
            d if d >= c.c => Tier::C,
            d if d >= c.d => Tier::D,
            _ => Tier::F,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tier::SPlus => "S+",
            Tier::S => "S",
            Tier::A => "A",
            Tier::B => "B",
            Tier::C => "C",
            Tier::D => "D",
            Tier::F => "F",
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Tier::SPlus => [0xff, 0x4f, 0xa3],
            Tier::S => [0xff, 0x6b, 0x3d],
            Tier::A => [0xf5, 0xc5, 0x18],
            Tier::B => [0x8e, 0xc4, 0x41],
            Tier::C => [0x3a, 0xa0, 0xff],
            Tier::D => [0x8a, 0x90, 0x9a],
            Tier::F => [0xc4, 0x3c, 0x3c],
        }
    }

    /// Text colour that reads on top of `rgb()`.
    pub fn text_rgb(self) -> [u8; 3] {
        if self == Tier::F {
            [0xff, 0xff, 0xff]
        } else {
            [0x11, 0x11, 0x11]
        }
    }
}

// ------------------------------------------------------------------ arammeta models

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub champs: HashMap<String, ChampInfo>,
    pub augs: HashMap<String, AugInfo>,
    #[serde(rename = "augCategories")]
    pub aug_categories: AugCategories,
    #[serde(rename = "itemLut")]
    pub item_lut: HashMap<String, ItemLut>,
    pub patch_prefix: String,
    pub ddv: String,
    #[serde(rename = "patchChanges")]
    pub patch_changes: PatchChanges,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PatchChanges {
    #[serde(rename = "currentGames")]
    pub current_games: f64,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct ChampInfo {
    pub name_en: String,
    pub alias: String,
    pub image: String,
    pub tags: Vec<String>,
    pub wr: f64,
    pub g: f64,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct AugInfo {
    pub name_en: String,
    pub icon: String,
    pub rarity: String,
    pub desc_en: String,
    pub cats: Vec<String>,
    pub wr: f64,
    pub g: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AugCategories {
    pub order: Vec<String>,
    pub labels: HashMap<String, CatLabel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CatLabel {
    pub en: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ItemLut {
    pub e: String,
    pub p: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ChampDetail {
    pub top: HashMap<String, Vec<AugStat>>,
    pub bot: HashMap<String, Vec<AugStat>>,
    #[serde(rename = "poolAugments")]
    pub pool: Vec<AugStat>,
    pub items: Combos,
    #[serde(rename = "singleItems")]
    pub single_items: Combos,
    pub boots: Combos,
    pub spells: Combos,
    #[serde(rename = "itemClusters")]
    pub clusters: Clusters,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct AugStat {
    pub id: u32,
    pub g: f64,
    pub wr: f64,
    pub lift: f64,
    pub slots: Vec<Option<Slot>>,
}

#[derive(Debug, Default, Deserialize, Clone, Copy)]
#[serde(default)]
pub struct Slot {
    pub g: f64,
    pub wr: f64,
    pub lift: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Combos {
    pub top: Vec<Combo>,
    pub bot: Vec<Combo>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct Combo {
    pub name_en: String,
    pub g: f64,
    pub wr: f64,
    pub lift: f64,
    pub pick: f64,
    pub items: Vec<ItemRef>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct ItemRef {
    pub id: u32,
    pub name_en: String,
    pub icon: String,
    pub g: f64,
    pub wr: f64,
    pub pick: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Clusters {
    pub groups: Vec<Cluster>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct Cluster {
    pub name_en: String,
    pub core: Vec<ItemRef>,
    pub g: f64,
    pub wr: f64,
    pub pick: f64,
    pub options: Vec<ItemRef>,
    pub tail: Vec<ItemRef>,
}

/// augment-stats.json: champion id -> rarity -> augment stats (for "best champions" lookups).
pub type AugStatsFile = HashMap<String, HashMap<String, Vec<AugStat>>>;

/// Data Dragon item.json "data" entries (only what build detection needs).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DdItem {
    pub name: String,
    pub tags: Vec<String>,
    pub gold: DdGold,
    pub stats: HashMap<String, f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DdGold {
    pub total: f64,
}

#[derive(Debug, Default, Deserialize)]
pub struct DdItemFile {
    pub data: HashMap<String, DdItem>,
}

pub const RARITIES: [(&str, &str, [u8; 3]); 3] = [
    ("kPrismatic", "Prismatic", [0xe3, 0xb8, 0xff]),
    ("kGold", "Gold", [0xf5, 0xc5, 0x18]),
    ("kSilver", "Silver", [0xc3, 0xca, 0xd4]),
];

pub fn norm(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

impl Meta {
    pub fn champ_name(&self, cid: u32) -> &str {
        self.champs.get(&cid.to_string()).map(|c| c.name_en.as_str()).unwrap_or("?")
    }

    pub fn find_champ(&self, name: &str) -> Option<u32> {
        let key = norm(name);
        self.champs
            .iter()
            .find(|(_, c)| norm(&c.alias) == key || norm(&c.name_en) == key)
            .and_then(|(id, _)| id.parse().ok())
    }

    pub fn item_icon(&self, id: u32) -> String {
        format!("https://ddragon.leagueoflegends.com/cdn/{}/img/item/{id}.png", self.ddv)
    }

    pub fn item_name(&self, id: u32) -> String {
        self.item_lut.get(&id.to_string()).map(|i| i.e.clone()).unwrap_or_else(|| id.to_string())
    }

    pub fn aug_icon(&self, id: u32) -> Option<String> {
        let a = self.augs.get(&id.to_string())?;
        if a.icon.is_empty() {
            return None;
        }
        Some(if a.icon.starts_with("http") { a.icon.clone() } else { format!("https://arammeta.com/{}", a.icon) })
    }

    pub fn cat_label(&self, cat: &str) -> String {
        self.aug_categories.labels.get(cat).map(|l| l.en.clone()).unwrap_or_else(|| cat.to_string())
    }
}

/// Augment descriptions use "[數值]" for scaling numbers; show something readable.
pub fn clean_desc(desc: &str) -> String {
    let mut out = String::with_capacity(desc.len());
    let mut in_tag = false;
    for ch in desc.replace("[數值]", "X").chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

// ------------------------------------------------------------------ ratings

#[derive(Clone, Copy, Debug)]
pub struct Rating {
    pub tier: Tier,
    /// Sample-adjusted win rate (the number the tier is based on).
    pub wr: f64,
    pub delta: f64,
    pub games: f64,
}

#[derive(Clone, Debug)]
pub struct AugRating {
    pub overall: Rating,
    pub rounds: Vec<Rating>,
    /// No data for this champion: rated from all-champion stats.
    pub global: bool,
}

impl AugRating {
    /// Rating for a pick round (1-based), falling back to overall.
    pub fn at_round(&self, round: Option<u32>) -> (Rating, Option<u32>) {
        match round {
            Some(r) if r >= 1 && (r as usize) <= self.rounds.len() => (self.rounds[r as usize - 1], Some(r)),
            _ => (self.overall, None),
        }
    }
}

/// Rate every augment for a champion: win rate vs the champion's average, with small
/// samples pulled toward the average (and each pick round toward the overall result).
pub fn champ_ratings(meta: &Meta, detail: &ChampDetail, cid: u32, cfg: &Config) -> HashMap<u32, AugRating> {
    let k = cfg.smoothing_games;
    let cut = &cfg.tier_cutoffs;
    let base = meta.champs.get(&cid.to_string()).map(|c| c.wr).filter(|w| *w > 0.0).unwrap_or(0.5);
    let mut out = HashMap::new();

    let rate = |a: &AugStat| {
        let delta = (a.wr - base) * a.g / (a.g + k);
        let overall = Rating { tier: Tier::from_delta(delta, cut), wr: base + delta, delta, games: a.g };
        let rounds = a
            .slots
            .iter()
            .map(|slot| match slot {
                Some(s) => {
                    // each round has ~1/4 of the games: pull it toward the overall result
                    let rd = (s.lift * s.g + delta * k) / (s.g + k);
                    let round_base = s.wr - s.lift;
                    Rating { tier: Tier::from_delta(rd, cut), wr: round_base + rd, delta: rd, games: s.g }
                }
                None => Rating { games: 0.0, ..overall },
            })
            .collect();
        AugRating { overall, rounds, global: false }
    };
    for a in detail.top.values().flatten().chain(detail.pool.iter()) {
        out.entry(a.id).or_insert_with(|| rate(a));
    }
    for (id, a) in &meta.augs {
        let Ok(id) = id.parse::<u32>() else { continue };
        if out.contains_key(&id) || a.g <= 0.0 {
            continue;
        }
        let r = global_aug_rating(a, cfg);
        out.insert(id, AugRating { overall: r, rounds: vec![], global: true });
    }
    out
}

/// An augment across all champions, vs 50%.
pub fn global_aug_rating(a: &AugInfo, cfg: &Config) -> Rating {
    let k = cfg.smoothing_games;
    let delta = (a.wr - 0.5) * a.g / (a.g + k);
    Rating { tier: Tier::from_delta(delta, &cfg.tier_cutoffs), wr: 0.5 + delta, delta, games: a.g }
}

pub fn champion_rating(c: &ChampInfo, cfg: &Config) -> Rating {
    let k = cfg.champion_smoothing_games;
    let delta = (c.wr - 0.5) * c.g / (c.g + k);
    Rating { tier: Tier::from_delta(delta, &cfg.champion_tier_cutoffs), wr: 0.5 + delta, delta, games: c.g }
}

// ------------------------------------------------------------------ build detection

pub const BUILD_CATS: [&str; 5] = ["ad", "ap", "crit", "tank", "support"];
pub const CAT_ORDER: [&str; 11] =
    ["ap", "ad", "crit", "tank", "support", "cd", "amp", "gold", "mechanic", "other", "new"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildProfile {
    pub cats: HashSet<&'static str>,
    pub haste_items: u32,
}

impl BuildProfile {
    pub fn labels(&self, meta: &Meta) -> Vec<String> {
        CAT_ORDER.iter().filter(|c| self.cats.contains(*c)).map(|c| meta.cat_label(c)).collect()
    }
}

/// Score items by their stat amounts (not tags: Statikk Shiv is tagged SpellDamage).
pub fn build_profile(items: &HashMap<String, DdItem>, ids: &[u32]) -> BuildProfile {
    let mut score: HashMap<&'static str, f64> = BUILD_CATS.iter().map(|c| (*c, 0.0)).collect();
    let mut haste = 0;
    for id in ids {
        let Some(it) = items.get(&id.to_string()) else { continue };
        let has = |t: &str| it.tags.iter().any(|x| x == t);
        if has("Boots") || has("Consumable") || has("Trinket") || it.gold.total < 700.0 {
            continue;
        }
        let st = |k: &str| it.stats.get(k).copied().unwrap_or(0.0);
        let w = if it.gold.total >= 1600.0 { 1.0 } else { 0.5 };
        let ap = st("FlatMagicDamageMod");
        *score.get_mut("ap").unwrap() += w * ap / 90.0;
        *score.get_mut("ad").unwrap() += w * (st("FlatPhysicalDamageMod") / 55.0 + st("PercentAttackSpeedMod") / 0.8);
        *score.get_mut("crit").unwrap() += w * st("FlatCritChanceMod") / 0.25;
        *score.get_mut("tank").unwrap() +=
            w * (st("FlatHPPoolMod") / 600.0 + st("FlatArmorMod") / 60.0 + st("FlatSpellBlockMod") / 60.0);
        if has("GoldPer") || has("Aura") || (has("ManaRegen") && ap <= 60.0) {
            *score.get_mut("support").unwrap() += w;
        }
        if has("AbilityHaste") || has("CooldownReduction") {
            haste += 1;
        }
    }
    let top = score.values().cloned().fold(0.0, f64::max);
    if top < 0.5 {
        return BuildProfile { cats: HashSet::new(), haste_items: haste };
    }
    let mut cats: HashSet<&'static str> = score.iter().filter(|(_, v)| **v >= top * 0.5).map(|(c, _)| *c).collect();
    if score["crit"] >= 0.75 {
        cats.insert("crit");
        cats.insert("ad");
    }
    BuildProfile { cats, haste_items: haste }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    Fits,
    Off,
    Any,
}

/// Does an augment suit the build? Returns the verdict and the categories it's about.
pub fn augment_fit(cats: &[String], build: &BuildProfile) -> Option<(Fit, Vec<&'static str>)> {
    let cats: Vec<&'static str> = CAT_ORDER.iter().copied().filter(|c| cats.iter().any(|x| x == c)).collect();
    if cats.is_empty() {
        return None;
    }
    let b = &build.cats;
    let primary: Vec<&'static str> = cats.iter().copied().filter(|c| BUILD_CATS.contains(c)).collect();
    if !primary.is_empty() {
        if b.is_empty() {
            return Some((Fit::Any, primary));
        }
        let hit: Vec<_> =
            primary.iter().copied().filter(|c| b.contains(c) || (*c == "ad" && b.contains("crit"))).collect();
        return Some(if hit.is_empty() { (Fit::Off, primary) } else { (Fit::Fits, hit) });
    }
    if cats.contains(&"cd") && (build.haste_items >= 2 || b.contains("ap")) {
        return Some((Fit::Fits, vec!["cd"]));
    }
    if cats.contains(&"amp") && !b.is_empty() {
        let damage = b.contains("ad") || b.contains("ap") || b.contains("crit");
        return Some((if damage { Fit::Fits } else { Fit::Off }, vec!["amp"]));
    }
    let rest: Vec<_> = cats.iter().copied().filter(|c| *c != "new").collect();
    Some((Fit::Any, vec![*rest.first().unwrap_or(&cats[0])]))
}

/// Keep Flash (or whatever) on the key it's already on. None = already set.
pub fn pick_spell_slots(current: (u32, u32), want: &[u32]) -> Option<(u32, u32)> {
    let (a, b) = current;
    if want.len() != 2 {
        return None;
    }
    let set: HashSet<_> = want.iter().collect();
    if set.contains(&a) && set.contains(&b) && a != b {
        return None;
    }
    if want.contains(&b) {
        let other = if want[1] == b { want[0] } else { want[1] };
        return Some((other, b));
    }
    let first = if want.contains(&a) { a } else { want[0] };
    let second = if want[0] == first { want[1] } else { want[0] };
    Some((first, second))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spell_slots() {
        assert_eq!(pick_spell_slots((4, 32), &[4, 32]), None);
        assert_eq!(pick_spell_slots((32, 4), &[4, 32]), None);
        assert_eq!(pick_spell_slots((14, 4), &[4, 32]), Some((32, 4)));
        assert_eq!(pick_spell_slots((4, 14), &[4, 32]), Some((4, 32)));
        assert_eq!(pick_spell_slots((3, 14), &[4, 32]), Some((4, 32)));
    }

    #[test]
    fn tiers() {
        let c = Config::default().tier_cutoffs;
        assert_eq!(Tier::from_delta(0.04, &c), Tier::SPlus);
        assert_eq!(Tier::from_delta(0.0, &c), Tier::B);
        assert_eq!(Tier::from_delta(-0.05, &c), Tier::F);
    }

    #[test]
    fn fit() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let b = |v: &[&'static str], h| BuildProfile { cats: v.iter().copied().collect(), haste_items: h };
        assert_eq!(augment_fit(&s(&["ap", "cd"]), &b(&["ad", "crit"], 0)).unwrap().0, Fit::Off);
        assert_eq!(augment_fit(&s(&["ad"]), &b(&["crit", "ad"], 0)).unwrap(), (Fit::Fits, vec!["ad"]));
        assert_eq!(augment_fit(&s(&["crit"]), &b(&["ad"], 0)).unwrap().0, Fit::Off);
        assert_eq!(augment_fit(&s(&["amp"]), &b(&["tank"], 0)).unwrap().0, Fit::Off);
        assert_eq!(augment_fit(&s(&["gold", "new"]), &b(&["ap"], 0)).unwrap(), (Fit::Any, vec!["gold"]));
        assert_eq!(augment_fit(&s(&["tank"]), &b(&[], 0)).unwrap().0, Fit::Any);
    }

    #[test]
    fn desc() {
        assert_eq!(clean_desc("Gain [數值] <b>bonus</b> damage"), "Gain X bonus damage");
    }
}
