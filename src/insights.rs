//! Explainable bench ranking. Points are a heuristic, never a match win probability.
use crate::{
    config::Config,
    model::{champion_rating, Meta},
};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct Recommendation {
    pub score: f64,
    pub base: f64,
    pub synergy: f64,
    pub composition: f64,
    pub reasons: Vec<String>,
}

pub fn recommend(meta: &Meta, cfg: &Config, candidate: u32, teammates: &[u32]) -> Option<Recommendation> {
    let champ = meta.champs.get(&candidate.to_string())?;
    if champ.g <= 0.0 || !champ.wr.is_finite() {
        return None;
    }
    let base = champion_rating(champ, cfg).delta * 100.0;
    let mut result = Recommendation { base, ..Default::default() };
    let mut seen = HashSet::new();
    let team: Vec<_> = teammates
        .iter()
        .filter(|id| **id != candidate && seen.insert(**id))
        .filter_map(|id| meta.champs.get(&id.to_string()).map(|c| (*id, c)))
        .collect();
    let mut lifts = Vec::new();
    for (id, ally) in &team {
        // Each pairing counts once; use the larger available sample if both directions exist.
        let pair = champ
            .pairs
            .iter()
            .filter(|p| p.id == *id)
            .chain(ally.pairs.iter().filter(|p| p.id == candidate))
            .filter(|p| p.g >= meta.min_games_per_pair.max(100.0) && p.lift.is_finite())
            .max_by(|a, b| a.g.total_cmp(&b.g));
        if let Some(pair) = pair {
            lifts.push(pair.lift.clamp(-0.025, 0.025));
            result.reasons.push(format!(
                "{} pairing with {} ({} games)",
                if pair.lift >= 0.0 { "Positive" } else { "Negative" },
                ally.name_en,
                pair.g as u64
            ));
        }
    }
    if !lifts.is_empty() {
        result.synergy = lifts.iter().sum::<f64>() / lifts.len() as f64 * 100.0;
    }
    if !team.is_empty() && !champ.comp.is_empty() {
        for (key, label) in [
            ("front", "frontline"),
            ("engage", "engage"),
            ("wave", "wave clear"),
            ("cc", "crowd control"),
            ("sustain", "sustain"),
        ] {
            // Only score roles when every known champion has that measurement.
            let Some(own) = champ.comp.get(key).filter(|v| v.is_finite()) else { continue };
            let values: Option<Vec<f64>> = team.iter().map(|(_, c)| c.comp.get(key).copied().filter(|v| v.is_finite())).collect();
            let Some(values) = values else { continue };
            let Some(threshold) = meta.recommendation_composition.lack_thresholds.get(key).filter(|t| **t > 0.0 && t.is_finite()) else {
                continue;
            };
            let target = threshold * (team.len() + 1) as f64 / 5.0;
            let deficit = (target - values.iter().sum::<f64>()).max(0.0);
            let filled = own.max(0.0).min(deficit);
            result.composition += (filled / target).min(1.0) * 0.6;
            if filled / target >= 0.25 {
                result.reasons.push(format!("Adds needed {label}"));
            }
        }
        let mix = |key: &str| -> Option<f64> {
            Some(
                team.iter()
                    .map(|(_, c)| c.comp.get(key).copied().filter(|v| v.is_finite() && *v >= 0.0))
                    .collect::<Option<Vec<_>>>()?
                    .iter()
                    .sum(),
            )
        };
        if let (Some(physical), Some(magic), Some(cp), Some(cm)) = (mix("phys"), mix("magic"), champ.comp.get("phys"), champ.comp.get("magic")) {
            let before_total = physical + magic;
            let after_total = before_total + cp + cm;
            if before_total > 0.0 && after_total > 0.0 && cp.is_finite() && cm.is_finite() && *cp >= 0.0 && *cm >= 0.0 {
                let improvement = (physical / before_total - 0.4).abs() - ((physical + cp) / after_total - 0.4).abs();
                result.composition += improvement.clamp(-0.25, 0.25) * 4.0;
                if improvement > 0.05 {
                    result.reasons.push("Balances physical and magic damage".into());
                }
            }
        }
    }
    result.composition = result.composition.clamp(-1.0, 2.5);
    result.score = result.base + result.synergy + result.composition;
    if team.is_empty() {
        result.reasons.push("Waiting for teammates; using champion strength".into());
    } else if result.reasons.is_empty() {
        result.reasons.push("No reliable pairing or role advantage in the available data".into());
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn team_fit_can_change_bench_order_and_duplicate_allies_do_not_inflate_it() {
        let meta: Meta = serde_json::from_value(json!({"champs": {
            "1":{"name_en":"Ally","g":10000,"wr":0.5},
            "2":{"g":10000,"wr":0.51,"pairs":[{"id":1,"g":600,"lift":0.025}]},
            "3":{"g":10000,"wr":0.52}}, "min_games_per_pair":15}))
        .unwrap();
        let cfg = Config::default();
        let a = recommend(&meta, &cfg, 2, &[1]).unwrap();
        let b = recommend(&meta, &cfg, 3, &[1]).unwrap();
        assert!(a.score > b.score);
        assert_eq!(a.score, recommend(&meta, &cfg, 2, &[1, 1, 2, 999]).unwrap().score);
        assert_eq!(recommend(&meta, &cfg, 2, &[]).unwrap().synergy, 0.0);
    }
    #[test]
    fn gaps_and_missing_profiles_are_handled_without_inventing_data() {
        let meta: Meta = serde_json::from_value(json!({"champs": {
            "1":{"g":10000,"wr":0.5,"comp":{"front":0}},
            "2":{"g":10000,"wr":0.5,"comp":{"front":3},"pairs":[{"id":1,"g":2,"lift":0.9}]},
            "3":{"g":10000,"wr":0.5}},"recommendation_composition":{"lack_thresholds":{"front":2}}}))
        .unwrap();
        let cfg = Config::default();
        let tank = recommend(&meta, &cfg, 2, &[1]).unwrap();
        assert!(tank.composition > 0.0);
        assert_eq!(tank.synergy, 0.0);
        assert_eq!(recommend(&meta, &cfg, 3, &[1]).unwrap().composition, 0.0);
        assert!(recommend(&meta, &cfg, 999, &[1]).is_none());
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn patch_comparison_reads_real_field_names_and_preserves_empty_categories() {
        let meta: Meta=serde_json::from_value(json!({"patchChanges":{
            "currentPatch":"16.21","baselinePatch":"16.20","currentGames":800000,"baselineGames":700000,
            "heroRisers":[{"id":164,"name_en":"Camille","current_wr":0.4966,"baseline_wr":0.4516,"delta":0.045,"current_games":19963,"baseline_games":11019}],
            "augmentFallers":[{"id":1328,"name_en":"Critical Rhythm","current_wr":0.483,"baseline_wr":0.5031,"delta":-0.0201,"current_games":220866,"baseline_games":223763}]
        }})).unwrap();
        let changes = meta.patch_changes;
        assert_eq!(changes.current_patch, "16.21");
        assert_eq!(changes.baseline_patch, "16.20");
        assert_eq!(changes.hero_risers[0].current_games, 19963.0);
        assert!((changes.hero_risers[0].delta * 100.0 - 4.5).abs() < 1e-9);
        assert!(changes.augment_fallers[0].delta < 0.0);
        assert!(changes.item_risers.is_empty());
        assert!(serde_json::from_value::<Meta>(json!({})).unwrap().patch_changes.hero_risers.is_empty());
    }
}
