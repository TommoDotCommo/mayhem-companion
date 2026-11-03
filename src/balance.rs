//! ARAM and ARAM: Mayhem champion balance changes, from the League of Legends Wiki.
//!
//! Mayhem applies every regular ARAM modifier, then its own overrides on top. The ARAM
//! values come from Module:ChampionData/data (multipliers per champion), the Mayhem
//! overrides from the "List of mode overrides" table on the ARAM: Mayhem page
//! ("Damage received changed to 0% from -10%.").

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stat {
    DmgDealt,
    DmgTaken,
    Healing,
    Shielding,
    Tenacity,
    AbilityHaste,
    AttackSpeed,
    EnergyRegen,
}

impl Stat {
    pub const ALL: [Stat; 8] = [
        Stat::DmgDealt,
        Stat::DmgTaken,
        Stat::Healing,
        Stat::Shielding,
        Stat::Tenacity,
        Stat::AbilityHaste,
        Stat::AttackSpeed,
        Stat::EnergyRegen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stat::DmgDealt => "Damage dealt",
            Stat::DmgTaken => "Damage taken",
            Stat::Healing => "Healing",
            Stat::Shielding => "Shielding",
            Stat::Tenacity => "Tenacity",
            Stat::AbilityHaste => "Ability haste",
            Stat::AttackSpeed => "Attack speed",
            Stat::EnergyRegen => "Energy regen",
        }
    }

    /// Ability haste is a flat number; everything else a percentage.
    pub fn flat(self) -> bool {
        self == Stat::AbilityHaste
    }

    /// Taking less damage is the buff.
    pub fn good_when_higher(self) -> bool {
        self != Stat::DmgTaken
    }

    fn from_lua_key(k: &str) -> Option<Stat> {
        Some(match k {
            "dmg_dealt" => Stat::DmgDealt,
            "dmg_taken" => Stat::DmgTaken,
            "healing" => Stat::Healing,
            "shielding" => Stat::Shielding,
            "tenacity" => Stat::Tenacity,
            "ability_haste" => Stat::AbilityHaste,
            "total_as" => Stat::AttackSpeed,
            "energyregen_mod" => Stat::EnergyRegen,
            _ => return None,
        })
    }

    fn from_phrase(p: &str) -> Option<Stat> {
        Some(match p.trim().to_ascii_lowercase().as_str() {
            "damage dealt" => Stat::DmgDealt,
            "damage received" | "damage taken" => Stat::DmgTaken,
            "healing done" | "healing" => Stat::Healing,
            "shielding done" | "shielding" => Stat::Shielding,
            "tenacity" => Stat::Tenacity,
            "ability haste" => Stat::AbilityHaste,
            "total attack speed" | "attack speed" => Stat::AttackSpeed,
            "energy regeneration" | "energy regen" => Stat::EnergyRegen,
            _ => return None,
        })
    }
}

/// One modifier: the value in Mayhem (percent, or flat for ability haste), plus where it
/// came from.
#[derive(Clone, Debug)]
pub struct Modifier {
    pub stat: Stat,
    pub value: f64,
    pub aram: Option<f64>,
    pub mayhem: Option<f64>,
}

impl Modifier {
    pub fn is_buff(&self) -> bool {
        (self.value > 0.0) == self.stat.good_when_higher()
    }

    /// "+5%", "−10%", "+20" (ability haste).
    pub fn value_text(&self) -> String {
        let sign = if self.value > 0.0 { "+" } else { "−" };
        let n = self.value.abs();
        let n = if (n - n.round()).abs() < 0.05 { format!("{n:.0}") } else { format!("{n:.1}") };
        if self.stat.flat() {
            format!("{sign}{n}")
        } else {
            format!("{sign}{n}%")
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChampBalance {
    /// Modifiers in effect in Mayhem (zeros left out).
    pub mods: Vec<Modifier>,
    /// Mayhem changes to the champion's abilities / base stats, as plain text.
    pub notes: Vec<String>,
}

pub type Balance = HashMap<u32, ChampBalance>;

/// Combine the two wiki sources into Mayhem balance per champion id.
pub fn build(champ_data_lua: &str, mayhem_wikitext: &str) -> Balance {
    let (names, aram) = parse_champion_data(champ_data_lua);
    let mayhem = parse_mayhem(mayhem_wikitext);
    let mut out: Balance = HashMap::new();
    for (&id, mods) in &aram {
        let e = out.entry(id).or_default();
        for (&stat, &v) in mods {
            e.mods.push(Modifier { stat, value: v, aram: Some(v), mayhem: None });
        }
    }
    for (name, (mods, notes)) in mayhem {
        let Some(&id) = names.get(&name) else {
            log::debug!("balance: no champion id for Mayhem entry {name}");
            continue;
        };
        let e = out.entry(id).or_default();
        for (stat, v) in mods {
            match e.mods.iter_mut().find(|m| m.stat == stat) {
                Some(m) => {
                    m.value = v;
                    m.mayhem = Some(v);
                }
                None => e.mods.push(Modifier { stat, value: v, aram: None, mayhem: Some(v) }),
            }
        }
        e.notes.extend(notes);
    }
    for b in out.values_mut() {
        b.mods.retain(|m| m.value.abs() > 1e-6 || m.aram.is_some_and(|a| a.abs() > 1e-6));
        b.mods.sort_by_key(|m| Stat::ALL.iter().position(|s| *s == m.stat));
    }
    out
}

/// Module:ChampionData/data: champion name -> id, and id -> ARAM modifiers (as percent /
/// flat deltas).
fn parse_champion_data(lua: &str) -> (HashMap<String, u32>, HashMap<u32, HashMap<Stat, f64>>) {
    let mut names = HashMap::new();
    let mut aram = HashMap::new();
    let mut current: Option<(String, Option<u32>)> = None;
    let mut in_aram = false;
    let mut block: HashMap<Stat, f64> = HashMap::new();
    for raw in lua.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.starts_with("--") {
            continue;
        }
        // top-level entry: two-space indent, ["Name"] = {
        if line.starts_with("  [\"") && !line.starts_with("   ") && t.ends_with('{') {
            if let Some(name) = quoted_key(t) {
                current = Some((name, None));
                in_aram = false;
                continue;
            }
        }
        let Some((name, id)) = current.as_mut() else { continue };
        if id.is_none() && t.starts_with("[\"id\"]") {
            if let Some(v) = t.split('=').nth(1).and_then(|v| v.trim().trim_end_matches(',').trim().parse::<u32>().ok()) {
                *id = Some(v);
                names.insert(name.clone(), v);
            }
            continue;
        }
        if t.starts_with("[\"aram\"]") && t.ends_with('{') {
            in_aram = true;
            block.clear();
            continue;
        }
        if in_aram {
            if t.starts_with('}') {
                in_aram = false;
                if let Some(id) = *id {
                    if !block.is_empty() {
                        aram.insert(id, block.clone());
                    }
                }
                continue;
            }
            if let (Some(k), Some(v)) = (quoted_key(t), t.split('=').nth(1)) {
                let v = v.trim().trim_end_matches(',').trim();
                if let (Some(stat), Ok(v)) = (Stat::from_lua_key(&k), v.parse::<f64>()) {
                    let delta = if stat.flat() { v } else { (v - 1.0) * 100.0 };
                    block.insert(stat, (delta * 10.0).round() / 10.0);
                }
            }
        }
    }
    (names, aram)
}

fn quoted_key(t: &str) -> Option<String> {
    let rest = t.strip_prefix("[\"")?;
    let end = rest.find("\"]")?;
    Some(rest[..end].to_string())
}

/// The Mayhem overrides table: champion -> (modifier overrides, other changes as text).
fn parse_mayhem(text: &str) -> Vec<(String, (Vec<(Stat, f64)>, Vec<String>))> {
    let Some(start) = text.find("=== List of mode overrides ===") else { return vec![] };
    let section = &text[start..];
    let section = &section[..section.find("\n== ").unwrap_or(section.len())];
    let mut out = vec![];
    for row in section.split("\n|-") {
        // cells start with "| " at the beginning of a line: name, general, abilities
        let mut cells: Vec<String> = vec![];
        for line in row.lines() {
            if let Some(rest) = line.strip_prefix('|') {
                if rest.starts_with('}') || rest.starts_with('-') {
                    continue;
                }
                cells.push(rest.trim().to_string());
            } else if let Some(last) = cells.last_mut() {
                last.push('\n');
                last.push_str(line);
            }
        }
        if cells.len() < 2 {
            continue;
        }
        let Some(name) = template_arg(&cells[0], "ci") else { continue };
        let mut mods = vec![];
        let mut notes = vec![];
        for line in cells[1].lines() {
            let l = line.trim();
            let Some(item) = l.strip_prefix('*') else { continue };
            let item = item.trim_start_matches('*').trim();
            match parse_override(item) {
                Some(m) => mods.push(m),
                None => notes.push(clean_wiki(item)),
            }
        }
        if let Some(ab) = cells.get(2) {
            let mut head = String::new();
            for line in ab.lines() {
                let l = line.trim();
                if l.is_empty() {
                    continue;
                }
                if let Some(item) = l.strip_prefix('*') {
                    let item = clean_wiki(item.trim_start_matches('*').trim());
                    notes.push(if head.is_empty() { item } else { format!("{head}: {item}") });
                } else if l.starts_with("{{ai|") {
                    // {{ai|W|Bard}} -> "W"
                    head = template_args(l, "ai").and_then(|a| a.first().cloned()).unwrap_or_default();
                }
            }
        }
        notes.retain(|n| !n.is_empty());
        if !mods.is_empty() || !notes.is_empty() {
            out.push((name, (mods, notes)));
        }
    }
    out
}

/// "Damage received changed to 0% from -10%." -> (DmgTaken, 0.0)
fn parse_override(item: &str) -> Option<(Stat, f64)> {
    let (what, rest) = item.split_once(" changed to ")?;
    let stat = Stat::from_phrase(what)?;
    let num: String = rest.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.')).collect();
    Some((stat, num.trim_start_matches('+').parse().ok()?))
}

fn template_arg(text: &str, name: &str) -> Option<String> {
    template_args(text, name)?.into_iter().next()
}

fn template_args(text: &str, name: &str) -> Option<Vec<String>> {
    let open = format!("{{{{{name}|");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find("}}")? + start;
    Some(text[start..end].split('|').map(|s| s.trim().to_string()).collect())
}

/// Wikitext to plain text: templates reduced to their display text, links to their label.
pub fn clean_wiki(s: &str) -> String {
    let mut t = s.to_string();
    // innermost templates first
    for _ in 0..12 {
        let Some(open) = t.rfind("{{") else { break };
        let Some(close) = t[open..].find("}}").map(|c| c + open) else { break };
        let inner = &t[open + 2..close];
        let mut parts = inner.split('|');
        let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let args: Vec<&str> = parts.filter(|a| !a.contains('=')).map(str::trim).collect();
        let shown = match name.as_str() {
            "tip" | "sti" | "ui" | "ii" | "csl" | "cai" => args.last().copied().unwrap_or(""),
            "note" | "bug" => "",
            _ => args.first().copied().unwrap_or(""),
        }
        .to_string();
        t.replace_range(open..close + 2, &shown);
    }
    // [[link|label]] / [[link]]
    while let Some(open) = t.find("[[") {
        let Some(close) = t[open..].find("]]").map(|c| c + open) else { break };
        let inner = t[open + 2..close].to_string();
        let shown = inner.rsplit('|').next().unwrap_or("").to_string();
        t.replace_range(open..close + 2, &shown);
    }
    t.replace("'''", "").replace("''", "").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUA: &str = r#"return {
  ["Kai'Sa"] = {
    ["id"]         = 145,
    ["apiname"]    = "Kaisa",
    ["stats"] = {
      ["aram"] = {
        ["total_as"] = 1.025,
        ["dmg_dealt"] = 1,
        ["dmg_taken"] = 0.9,
      },
    },
  },
  ["Alistar"] = {
    ["id"]         = 12,
    ["stats"] = {
      ["aram"] = {
        ["dmg_dealt"] = 0.95,
        ["healing"] = 0.8,
        ["ability_haste"] = -10,
      },
    },
  },
}"#;

    const WIKI: &str = "=== List of mode overrides ===\n{|\n|-\n| {{ci|Kai'Sa}}\n| Special modifiers\n* Damage received changed to 0% from -10%.\n| \n|-\n| {{ci|Alistar}}\n| Special modifiers\n* Damage dealt changed to 0% from -5%.\n| {{ai|W|Alistar}}\n* Cooldown changed to {{fd|10}} seconds.\n|}\n== Patch History ==\n";

    #[test]
    fn combines_aram_and_mayhem() {
        let b = build(LUA, WIKI);
        let kaisa = &b[&145];
        // ARAM -10% damage taken is overridden to 0 by Mayhem; attack speed +2.5% stays
        let taken = kaisa.mods.iter().find(|m| m.stat == Stat::DmgTaken).unwrap();
        assert_eq!((taken.value, taken.aram, taken.mayhem), (0.0, Some(-10.0), Some(0.0)));
        let asp = kaisa.mods.iter().find(|m| m.stat == Stat::AttackSpeed).unwrap();
        assert!((asp.value - 2.5).abs() < 1e-9 && asp.is_buff());
        let ali = &b[&12];
        let heal = ali.mods.iter().find(|m| m.stat == Stat::Healing).unwrap();
        assert!((heal.value + 20.0).abs() < 1e-9 && !heal.is_buff());
        let ah = ali.mods.iter().find(|m| m.stat == Stat::AbilityHaste).unwrap();
        assert_eq!(ah.value_text(), "−10");
        assert_eq!(ali.notes, vec!["W: Cooldown changed to 10 seconds.".to_string()]);
    }

    #[test]
    fn wiki_text() {
        assert_eq!(clean_wiki("Shield strength health ratio changed to {{as|15% '''bonus''' health}}."), "Shield strength health ratio changed to 15% bonus health.");
        assert_eq!(clean_wiki("{{tip|immobilized|immobilizing crowd control}} and [[Health relic|relics]]"), "immobilizing crowd control and relics");
    }
}
