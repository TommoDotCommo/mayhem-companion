//! Highlights from personal match history: what tends to go well for this player.
//! Facts describe the player's own games. They are associations, not causes, and each needs a minimum sample.
use crate::history::MatchRecord;
use crate::model::{build_profile, DdItem, Meta, BUILD_CATS};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Neutral,
}

#[derive(Clone, Debug)]
pub struct Fact {
    pub title: String,
    pub value: String,
    pub detail: String,
    pub tone: Tone,
}

/// Groups need this many games before they are compared with anything.
const MIN_GROUP: u64 = 5;
/// A single augment needs fewer games, but its record is shrunk harder toward the player's average.
const MIN_AUGMENT: u64 = 3;
/// Win-rate gaps smaller than this (after shrinking) are noise at personal sample sizes.
const MIN_GAP: f64 = 0.08;

#[derive(Clone, Copy, Default)]
struct Tally {
    games: u64,
    wins: u64,
}
impl Tally {
    fn add(&mut self, win: bool) {
        self.games += 1;
        self.wins += u64::from(win);
    }
    fn rate(&self) -> f64 {
        if self.games == 0 {
            0.0
        } else {
            self.wins as f64 / self.games as f64
        }
    }
    /// Pull small samples toward `base` so a 2–0 record doesn't outrank 9–3.
    fn shrunk(&self, base: f64, prior: f64) -> f64 {
        (self.wins as f64 + base * prior) / (self.games as f64 + prior)
    }
    fn record(&self) -> String {
        format!("{}–{}", self.wins, self.games - self.wins)
    }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}
fn pp(v: f64) -> String {
    format!("{:+.0} pp", v * 100.0)
}
fn champion(meta: Option<&Meta>, id: u32) -> String {
    meta.and_then(|m| m.champs.get(&id.to_string()))
        .map(|c| c.name_en.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Champion #{id}"))
}
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Best and worst groups (by shrunk win rate) that clear `MIN_GROUP` and sit clearly away from `base`.
fn extremes(groups: &HashMap<String, Tally>, base: f64) -> (Option<(String, Tally)>, Option<(String, Tally)>) {
    let mut rows: Vec<_> = groups.iter().filter(|(_, t)| t.games >= MIN_GROUP).map(|(k, t)| (k.clone(), *t)).collect();
    rows.sort_by(|a, b| b.1.shrunk(base, 5.0).total_cmp(&a.1.shrunk(base, 5.0)).then(a.0.cmp(&b.0)));
    let best = rows.first().filter(|(_, t)| t.shrunk(base, 5.0) - base >= MIN_GAP / 2.0).cloned();
    let worst = rows.last().filter(|(_, t)| base - t.shrunk(base, 5.0) >= MIN_GAP / 2.0).cloned();
    let worst = worst.filter(|w| best.as_ref().map_or(true, |b| b.0 != w.0));
    (best, worst)
}

/// `games` may be in any order. `items` (Data Dragon) enables build-style facts; `meta` enables names, classes and expectations.
pub fn facts(games: &[MatchRecord], meta: Option<&Meta>, items: Option<&HashMap<String, DdItem>>) -> Vec<Fact> {
    let mut out = Vec::new();
    if games.len() < MIN_GROUP as usize {
        return out;
    }
    let mut all = Tally::default();
    for g in games {
        all.add(g.win);
    }
    let base = all.rate();

    // Beating the odds: actual results against everyone's win rate on the same champions.
    if let Some(meta) = meta {
        let known: Vec<_> = games
            .iter()
            .filter_map(|g| meta.champs.get(&g.champion.to_string()).filter(|c| c.g > 0.0 && c.wr.is_finite()).map(|c| (g, c.wr)))
            .collect();
        if known.len() >= 10 {
            let expected = known.iter().map(|(_, wr)| wr).sum::<f64>() / known.len() as f64;
            let actual = known.iter().filter(|(g, _)| g.win).count() as f64 / known.len() as f64;
            let gap = actual - expected;
            out.push(Fact {
                title: if gap >= 0.0 { "Beating the odds" } else { "Below the odds" }.into(),
                value: pp(gap),
                detail: format!(
                    "You win {} on champions that win {} for everyone else ({} games).",
                    pct(actual),
                    pct(expected),
                    known.len()
                ),
                tone: if gap >= 0.02 { Tone::Good } else if gap <= -0.02 { Tone::Bad } else { Tone::Neutral },
            });
        }
    }

    // Champion classes.
    if let Some(meta) = meta {
        let mut classes: HashMap<String, Tally> = HashMap::new();
        for g in games {
            for tag in meta.champs.get(&g.champion.to_string()).map(|c| c.tags.as_slice()).unwrap_or_default() {
                classes.entry(tag.clone()).or_default().add(g.win);
            }
        }
        let (best, worst) = extremes(&classes, base);
        if let Some((tag, t)) = best {
            out.push(Fact {
                title: format!("{tag}s suit you"),
                value: pct(t.rate()),
                detail: format!("{} on {tag}s, against {} across all your games.", t.record(), pct(base)),
                tone: Tone::Good,
            });
        }
        if let Some((tag, t)) = worst {
            out.push(Fact {
                title: format!("{tag}s are a struggle"),
                value: pct(t.rate()),
                detail: format!("{} on {tag}s, against {} across all your games.", t.record(), pct(base)),
                tone: Tone::Bad,
            });
        }
    }

    // Build style from final items.
    if let Some(items) = items {
        let mut styles: HashMap<String, Tally> = HashMap::new();
        for g in games {
            let profile = build_profile(items, &g.items);
            for cat in BUILD_CATS.iter().filter(|c| profile.cats.contains(*c)) {
                styles.entry(cat.to_string()).or_default().add(g.win);
            }
        }
        let label = |cat: &str| meta.map(|m| m.cat_label(cat)).unwrap_or_else(|| cat.to_uppercase());
        if let (Some((cat, t)), _) = extremes(&styles, base) {
            out.push(Fact {
                title: "Best build style".into(),
                value: format!("{} · {}", label(&cat), pct(t.rate())),
                detail: format!("{} when your final items lean {}, against {} overall.", t.record(), label(&cat), pct(base)),
                tone: Tone::Good,
            });
        }
    }

    // Augment categories: games with at least one augment of a category against games with none.
    // Only games with recorded augments take part, so missing data doesn't count as "without".
    let with_augments: Vec<_> = games.iter().filter(|g| !g.augments.is_empty()).collect();
    if let Some(meta) = meta.filter(|_| with_augments.len() >= 2 * MIN_GROUP as usize) {
        let mut best: Option<(String, Tally, Tally, f64)> = None;
        let mut cats: Vec<&String> = meta.augs.values().flat_map(|a| a.cats.iter()).filter(|c| *c != "new" && *c != "other").collect();
        cats.sort();
        cats.dedup();
        for cat in cats {
            let (mut yes, mut no) = (Tally::default(), Tally::default());
            for g in &with_augments {
                let has = g.augments.iter().any(|a| meta.augs.get(&a.to_string()).is_some_and(|a| a.cats.contains(cat)));
                if has { yes.add(g.win) } else { no.add(g.win) }
            }
            if yes.games < MIN_GROUP || no.games < MIN_GROUP {
                continue;
            }
            let lift = yes.shrunk(base, 5.0) - no.shrunk(base, 5.0);
            if lift >= MIN_GAP && best.as_ref().map_or(true, |b| lift > b.3) {
                best = Some((cat.clone(), yes, no, lift));
            }
        }
        if let Some((cat, yes, no, _)) = best {
            out.push(Fact {
                title: "Augments that click".into(),
                value: meta.cat_label(&cat),
                detail: format!(
                    "{} ({}) in games with {} augments, {} ({}) without.",
                    pct(yes.rate()),
                    yes.record(),
                    meta.cat_label(&cat),
                    pct(no.rate()),
                    no.record()
                ),
                tone: Tone::Good,
            });
        }
    }

    // Standout single augment.
    {
        let mut augs: HashMap<u32, Tally> = HashMap::new();
        for g in games {
            let mut seen = std::collections::HashSet::new();
            for a in g.augments.iter().filter(|a| **a > 0 && seen.insert(**a)) {
                augs.entry(*a).or_default().add(g.win);
            }
        }
        let best = augs
            .iter()
            .filter(|(_, t)| t.games >= MIN_AUGMENT)
            .max_by(|a, b| a.1.shrunk(base, 6.0).total_cmp(&b.1.shrunk(base, 6.0)).then(b.0.cmp(a.0)));
        if let Some((id, t)) = best.filter(|(_, t)| t.shrunk(base, 6.0) - base >= MIN_GAP / 2.0) {
            let info = meta.and_then(|m| m.augs.get(&id.to_string()));
            let name = info.map(|a| a.name_en.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Augment #{id}"));
            let everyone = info
                .filter(|a| a.g > 0.0 && a.wr.is_finite())
                .map(|a| format!(" Everyone else wins {} with it.", pct(a.wr)))
                .unwrap_or_default();
            out.push(Fact {
                title: "Lucky augment".into(),
                value: name,
                detail: format!("{} in your games ({}).{everyone}", t.record(), pct(t.rate())),
                tone: Tone::Good,
            });
        }
    }

    // Game length.
    {
        let mut short = Tally::default();
        let mut long = Tally::default();
        for g in games.iter().filter(|g| g.duration_secs > 0) {
            if g.duration_secs < 15 * 60 {
                short.add(g.win)
            } else if g.duration_secs >= 20 * 60 {
                long.add(g.win)
            }
        }
        if short.games >= MIN_GROUP && long.games >= MIN_GROUP {
            let gap = long.shrunk(base, 5.0) - short.shrunk(base, 5.0);
            if gap.abs() >= MIN_GAP {
                let (title, value, detail) = if gap > 0.0 {
                    ("Late-game closer", pct(long.rate()), format!("{} in games over 20 minutes, {} ({}) under 15.", long.record(), pct(short.rate()), short.record()))
                } else {
                    ("Fast starter", pct(short.rate()), format!("{} in games under 15 minutes, {} ({}) past 20.", short.record(), pct(long.rate()), long.record()))
                };
                out.push(Fact { title: title.into(), value, detail, tone: Tone::Neutral });
            }
        }
    }

    // Deaths per 10 minutes, split at the player's own median.
    {
        let mut rates: Vec<(f64, bool)> = games
            .iter()
            .filter(|g| g.duration_secs >= 300)
            .map(|g| (g.deaths as f64 * 600.0 / g.duration_secs as f64, g.win))
            .collect();
        if rates.len() >= 2 * MIN_GROUP as usize {
            rates.sort_by(|a, b| a.0.total_cmp(&b.0));
            let median = rates[rates.len() / 2].0;
            let (mut low, mut high) = (Tally::default(), Tally::default());
            for (r, win) in &rates {
                if *r < median { low.add(*win) } else { high.add(*win) }
            }
            if low.games >= MIN_GROUP && high.games >= MIN_GROUP {
                let gap = low.shrunk(base, 5.0) - high.shrunk(base, 5.0);
                if gap >= MIN_GAP {
                    out.push(Fact {
                        title: "Staying alive pays".into(),
                        value: pct(low.rate()),
                        detail: format!(
                            "{} with under {:.1} deaths per 10 minutes, {} ({}) above that.",
                            low.record(),
                            median,
                            pct(high.rate()),
                            high.record()
                        ),
                        tone: Tone::Good,
                    });
                } else if gap <= -MIN_GAP {
                    out.push(Fact {
                        title: "Fight-hungry".into(),
                        value: pct(high.rate()),
                        detail: format!(
                            "You win more when you die more: {} at {:.1}+ deaths per 10 minutes, {} below.",
                            high.record(),
                            median,
                            low.record()
                        ),
                        tone: Tone::Neutral,
                    });
                }
            }
        }
    }

    // Streaks, oldest to newest.
    {
        let mut ordered: Vec<_> = games.iter().collect();
        ordered.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.game_id.cmp(&b.game_id)));
        let (mut best, mut run) = (0, 0);
        for g in &ordered {
            run = if g.win { run + 1 } else { 0 };
            best = best.max(run);
        }
        let last = ordered.last().map(|g| g.win).unwrap_or(false);
        let current = ordered.iter().rev().take_while(|g| g.win == last).count();
        if best >= 2 {
            out.push(Fact {
                title: "Streaks".into(),
                value: format!("Best: {best} wins"),
                detail: format!("You're on a {current}-game {} streak.", if last { "winning" } else { "losing" }),
                tone: Tone::Neutral,
            });
        }
    }

    // Personal best damage.
    if let Some(g) = games.iter().max_by_key(|g| (g.damage, g.game_id)).filter(|g| g.damage > 0) {
        out.push(Fact {
            title: "Biggest damage game".into(),
            value: thousands(g.damage),
            detail: format!(
                "{} · {}/{}/{} · {}",
                champion(meta, g.champion),
                g.kills,
                g.deaths,
                g.assists,
                if g.win { "Win" } else { "Loss" }
            ),
            tone: Tone::Neutral,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn game(id: u64, champion: u32, win: bool, minutes: u64, deaths: u64, augments: Vec<u32>) -> MatchRecord {
        MatchRecord {
            game_id: id,
            champion,
            patch: "16.22".into(),
            created_ms: id * 1000,
            duration_secs: minutes * 60,
            win,
            kills: 5,
            deaths,
            assists: 10,
            damage: 20_000 + id,
            items: vec![],
            augments,
        }
    }
    fn meta() -> Meta {
        serde_json::from_value(json!({
            "champs": {
                "1": {"name_en": "Lux", "tags": ["Mage"], "g": 10000, "wr": 0.48},
                "2": {"name_en": "Malphite", "tags": ["Tank"], "g": 10000, "wr": 0.52}},
            "augs": {
                "10": {"name_en": "BEES!!!", "cats": ["ap"], "g": 5000, "wr": 0.52},
                "11": {"name_en": "Plain", "cats": ["tank"], "g": 5000, "wr": 0.5}}
        }))
        .unwrap()
    }

    #[test]
    fn small_histories_produce_nothing() {
        let games: Vec<_> = (0..4).map(|i| game(i, 1, true, 18, 5, vec![10])).collect();
        assert!(facts(&games, Some(&meta()), None).is_empty());
    }

    #[test]
    fn strong_patterns_surface_and_noise_does_not() {
        let mut games = vec![];
        // Lux with BEES!!! in long games: 9–1. Malphite with Plain in short games: 2–8.
        for i in 0..10 {
            games.push(game(i, 1, i != 0, 22, 4, vec![10]));
            games.push(game(100 + i, 2, i < 2, 12, 9, vec![11]));
        }
        let found = facts(&games, Some(&meta()), None);
        let titles: Vec<_> = found.iter().map(|f| f.title.as_str()).collect();
        assert!(titles.contains(&"Mages suit you"), "{titles:?}");
        assert!(titles.contains(&"Tanks are a struggle"), "{titles:?}");
        assert!(titles.contains(&"Late-game closer"), "{titles:?}");
        assert!(titles.contains(&"Staying alive pays"), "{titles:?}");
        let lucky = found.iter().find(|f| f.title == "Lucky augment").unwrap();
        assert_eq!(lucky.value, "BEES!!!");
        assert!(lucky.detail.starts_with("9–1"));
        let augs = found.iter().find(|f| f.title == "Augments that click").unwrap();
        assert_eq!(augs.value, "ap");
        // 11 wins from 20 games on champions averaging 50%.
        let odds = found.iter().find(|f| f.title == "Beating the odds").unwrap();
        assert_eq!(odds.value, "+5 pp");
        let best = found.iter().find(|f| f.title == "Biggest damage game").unwrap();
        assert_eq!(best.value, "20,109");
        assert!(best.detail.starts_with("Malphite"));

        // An even split everywhere has no story to tell beyond records.
        let even: Vec<_> = (0..20).map(|i| game(i, 1 + (i % 2) as u32, i % 4 < 2, 12 + (i % 3) * 5, 5, vec![10])).collect();
        let titles: Vec<_> = facts(&even, Some(&meta()), None).into_iter().map(|f| f.title).collect();
        assert!(!titles.iter().any(|t| t.contains("suit") || t.contains("struggle") || t == "Lucky augment"), "{titles:?}");
    }

    #[test]
    fn streaks_follow_time_order() {
        let mut games: Vec<_> = [true, true, true, false, true, true].iter().enumerate().map(|(i, w)| game(i as u64, 1, *w, 18, 5, vec![])).collect();
        games.reverse();
        let streak = facts(&games, None, None).into_iter().find(|f| f.title == "Streaks").unwrap();
        assert_eq!(streak.value, "Best: 3 wins");
        assert_eq!(streak.detail, "You're on a 2-game winning streak.");
    }
}
