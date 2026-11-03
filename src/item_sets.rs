use crate::model::*;
use serde_json::{json, Value};
const TAG: &str = "[Mayhem]";

// ------------------------------------------------------------------ item sets

/// Separate pages per build style, or one combined page. (the clusters' names: "AP burst", "Tank / Heartsteel"…),
/// so the shop's item-set dropdown switches between genuinely different builds. Clusters
/// sharing a style become variants on the same page. `prefer` (a cluster index) goes first.
pub fn make_item_sets(meta: &Meta, detail: &ChampDetail, cid: u32, prefer: Option<usize>, combined: bool) -> Vec<Value> {
    let block = |title: String, ids: Vec<u32>| {
        let mut seen = std::collections::HashSet::new();
        let items: Vec<Value> = ids
            .into_iter()
            .filter(|i| seen.insert(*i))
            .map(|i| json!({"id": i.to_string(), "count": 1}))
            .collect();
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
    let singles: Vec<u32> = detail
        .single_items
        .top
        .iter()
        .filter_map(|s| s.items.first().map(|i| i.id))
        .take(10)
        .collect();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pages = styles.into_iter().enumerate().map(|(rank, (style, clusters))| {
        let main = clusters[0];
        let mut blocks = vec![];
        blocks.push(block(
            format!("Core · {:.1}% WR · {:.1}k games", main.wr * 100.0, main.g / 1000.0),
            main.core.iter().map(|i| i.id).collect(),
        ));
        blocks.push(block("Full build".into(), main.completion.iter().map(|i| i.id).collect()));
        blocks.push(block(
            "Next items".into(),
            main.options.iter().chain(main.tail.iter()).map(|i| i.id).collect(),
        ));
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
            "uid": format!("mayhem-companion-{cid}-{rank}-{stamp}"),
        })
    });
    let mut pages: Vec<Value> = pages.collect();
    if combined && !pages.is_empty() {
        let mut blocks = Vec::new();
        for page in &pages {
            let style = page["title"].as_str().unwrap_or("").to_string();
            for mut block in page["blocks"].as_array().cloned().unwrap_or_default() {
                block["type"] = json!(format!("{style} · {}", block["type"].as_str().unwrap_or("")));
                blocks.push(block);
            }
        }
        pages.truncate(1);
        pages[0]["title"] = json!(format!("{TAG} {} · All builds", meta.champ_name(cid)));
        pages[0]["blocks"] = json!(blocks);
    }
    pages
}

/// Only remove recognised generated pages that apply to this champion and map.
fn tool_build(set: &Value, cid: u32) -> bool {
    let title = set["title"].as_str().unwrap_or("").to_ascii_lowercase();
    let tool = ["blitz", "porofessor", "mobalytics", "u.gg", "op.gg", "lolalytics", "facecheck", "metasrc"]
        .iter()
        .any(|name| {
            let title = title.trim_start_matches(['[', '(', ' ']);
            title.strip_prefix(name).is_some_and(|suffix| suffix.chars().next().is_none_or(|c| !c.is_alphanumeric()))
        });
    let champion = set["associatedChampions"]
        .as_array()
        .is_some_and(|ids| ids.iter().any(|id| id.as_u64() == Some(cid as u64)));
    let maps = set["associatedMaps"].as_array();
    let aram = maps.is_none_or(|ids| ids.is_empty() || ids.iter().any(|id| id.as_u64() == Some(12)));
    tool && champion && aram
}

pub fn merge_item_sets(current: &[Value], new_sets: &[Value], cid: u32, replace_tools: bool) -> Vec<Value> {
    let mut sets = new_sets.to_vec();
    sets.extend(
        current
            .iter()
            .filter(|s| {
                let ours = s["uid"].as_str().unwrap_or("").starts_with("mayhem-companion-") || s["title"].as_str().unwrap_or("").starts_with(TAG);
                !ours && !(replace_tools && tool_build(s, cid))
            })
            .cloned(),
    );
    for (rank, set) in sets.iter_mut().enumerate() {
        set["sortrank"] = json!(rank);
    }
    sets
}

#[cfg(test)]
mod tests {
    use super::*;
    fn detail() -> ChampDetail {
        serde_json::from_value(json!({"itemClusters": {"groups": [
            {"name_en":"AP", "core":[{"id":1},{"id":1}], "completion":[{"id":2}]},
            {"name_en":"Tank", "core":[{"id":3}]},
            {"name_en":"AP", "core":[{"id":4}]},
            {"name_en":"AD", "core":[{"id":5}]},
            {"name_en":"On hit", "core":[{"id":6}]},
            {"name_en":"Support", "core":[{"id":7}]}
        ]}}))
        .unwrap()
    }
    #[test]
    fn separate_styles_and_preferred_build() {
        let pages = make_item_sets(&Meta::default(), &detail(), 1, Some(2), false);
        assert_eq!(pages.len(), 5); // no silent four-style limit
        assert_eq!(pages[0]["blocks"][0]["items"][0]["id"], "4");
        assert!(pages[0]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["type"].as_str().unwrap().contains("Variant")));
        let pages = make_item_sets(&Meta::default(), &detail(), 1, None, false);
        assert_eq!(pages[0]["blocks"][0]["items"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn combined_contains_every_style_and_empty_is_safe() {
        let pages = make_item_sets(&Meta::default(), &detail(), 1, Some(1), true);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["blocks"][0]["items"][0]["id"], "3");
        let all = pages[0]["blocks"].to_string();
        for id in [1, 2, 3, 4, 5, 6, 7] {
            assert!(all.contains(&format!("\"id\":\"{id}\"")));
        }
        assert!(make_item_sets(&Meta::default(), &ChampDetail::default(), 1, None, true).is_empty());
    }
    #[test]
    fn imports_primary_and_preserves_personal_and_unrelated_sets() {
        assert!(!tool_build(&json!({"title":"My anti-Blitz build", "associatedChampions":[1]}), 1));
        assert!(!tool_build(&json!({"title":"Blitzcrank tank", "associatedChampions":[1]}), 1));
        let existing = vec![
            json!({"title":"Blitz.gg ARAM", "associatedChampions":[1], "associatedMaps":[12]}),
            json!({"title":"My build", "associatedChampions":[1]}),
            json!({"title":"U.GG", "associatedChampions":[2]}),
            json!({"title":"Porofessor", "associatedChampions":[1], "associatedMaps":[11]}),
            json!({"title":"[Mayhem] old"}),
        ];
        let new = vec![json!({"title":"[Mayhem] new"})];
        let merged = merge_item_sets(&existing, &new, 1, true);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0]["title"], "[Mayhem] new");
        assert_eq!(merged[1]["title"], "My build");
        for (i, set) in merged.iter().enumerate() {
            assert_eq!(set["sortrank"], i);
        }
        assert_eq!(merge_item_sets(&existing, &new, 1, false).len(), 5);
    }
    #[test]
    fn old_config_gets_new_defaults_and_keeps_user_preferences() {
        let cfg: crate::config::Config = serde_json::from_value(json!({"close_companion_in_game":false})).unwrap();
        assert!(!cfg.close_companion_in_game);
        assert!(cfg.replace_tool_builds && cfg.auto_update_app && cfg.auto_update_data);
        assert!(!cfg.combined_item_sets);
    }
}
