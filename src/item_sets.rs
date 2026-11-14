use crate::model::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
const TAG: &str = "[Mayhem]";
const REFILLABLE_POTION: u32 = 2031;

// ------------------------------------------------------------------ item sets

/// Item ids with repeats folded into one icon with a count (a recipe can need a component twice).
fn counted(ids: Vec<u32>) -> Vec<Value> {
    let mut entries: Vec<(u32, u32)> = vec![];
    for id in ids {
        match entries.iter_mut().find(|(p, _)| *p == id) {
            Some((_, count)) => *count += 1,
            None => entries.push((id, 1)),
        }
    }
    entries.into_iter().map(|(id, count)| json!({"id": id.to_string(), "count": count})).collect()
}

/// An item's direct components that we know, dearest first.
fn components(items: &HashMap<String, DdItem>, id: u32) -> Vec<u32> {
    let cost = |id: &u32| items.get(&id.to_string()).map(|i| i.gold.total).unwrap_or(0.0);
    let mut parts: Vec<u32> = items
        .get(&id.to_string())
        .map(|i| i.from.iter().filter_map(|c| c.parse().ok()).filter(|c: &u32| items.contains_key(&c.to_string())).collect())
        .unwrap_or_default();
    parts.sort_by(|a, b| cost(b).partial_cmp(&cost(a)).unwrap_or(std::cmp::Ordering::Equal));
    parts
}

/// What the starting gold buys towards `target`: its dearest affordable components first,
/// breaking down the ones that don't fit. Returns the purchases and the gold left.
fn afford(items: &HashMap<String, DdItem>, target: u32, mut gold: f64, depth: u32) -> (Vec<u32>, f64) {
    let Some(item) = items.get(&target.to_string()) else { return (vec![], gold) };
    if item.gold.total <= gold {
        return (vec![target], gold - item.gold.total);
    }
    let mut bought = vec![];
    if depth < 4 {
        for part in components(items, target) {
            let (got, left) = afford(items, part, gold, depth + 1);
            bought.extend(got);
            gold = left;
        }
    }
    (bought, gold)
}

/// The first purchase: as much of the first item as the starting gold covers, then basic
/// boots towards the build's boots if they still fit, then a Refillable Potion with any spare 150g.
fn start_block(items: &HashMap<String, DdItem>, order: &[u32], gold: f64) -> Option<Value> {
    let (mut bought, mut left) = afford(items, *order.first()?, gold, 0);
    let is_boots = |id: &u32| items.get(&id.to_string()).is_some_and(|i| i.tags.iter().any(|t| t == "Boots"));
    if let Some(boots) = order.iter().skip(1).find(|id| is_boots(id)) {
        let (got, rest) = afford(items, components(items, *boots).into_iter().find(is_boots).unwrap_or(*boots), left, 0);
        bought.extend(got);
        left = rest;
    }
    // spare gold: a Refillable Potion if it fits
    if let Some(potion) = items.get(&REFILLABLE_POTION.to_string()).filter(|p| p.gold.total <= left) {
        bought.push(REFILLABLE_POTION);
        left -= potion.gold.total;
    }
    if bought.is_empty() {
        return None;
    }
    Some(json!({"type": format!("Start · {:.0}g of {:.0}g", gold - left, gold), "items": counted(bought)}))
}

/// The full build in buying order: a starting purchase, then one step per item: its components (dearest first, the
/// usual first purchase) and then the item itself. The data lists boots last; they move to
/// second, where they are normally bought.
fn buy_order(meta: &Meta, items: &HashMap<String, DdItem>, build: &[u32], start_gold: f64) -> Vec<Value> {
    let get = |id: u32| items.get(&id.to_string());
    let mut order: Vec<u32> = vec![];
    for id in build {
        if !order.contains(id) && get(*id).is_some() {
            order.push(*id);
        }
    }
    if let Some(at) = order.iter().position(|id| get(*id).is_some_and(|i| i.tags.iter().any(|t| t == "Boots"))) {
        if at > 1 {
            let boots = order.remove(at);
            order.insert(1, boots);
        }
    }
    let mut steps: Vec<Value> = start_block(items, &order, start_gold).into_iter().collect();
    for (n, id) in order.iter().enumerate() {
        let Some(item) = get(*id) else { continue };
        let mut parts = components(items, *id);
        parts.push(*id);
        steps.push(json!({
            "type": format!("Buy {} · {} · {:.0}g", n + 1, meta.item_name(*id), item.gold.total),
            "items": counted(parts),
        }));
    }
    steps
}

/// Separate pages per build style, or one combined page. (the clusters' names: "AP burst", "Tank / Heartsteel"…),
/// so the shop's item-set dropdown switches between genuinely different builds. Clusters
/// sharing a style become variants on the same page. `prefer` (a cluster index) goes first.
/// With `items` (Data Dragon recipes) each page spells the build out as buy steps with components.
pub fn make_item_sets(
    meta: &Meta,
    detail: &ChampDetail,
    cid: u32,
    prefer: Option<usize>,
    combined: bool,
    items: Option<&HashMap<String, DdItem>>,
    start_gold: f64,
) -> Vec<Value> {
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
        let build: Vec<u32> = if main.completion.is_empty() { &main.core } else { &main.completion }.iter().map(|i| i.id).collect();
        let steps = items.map(|dd| buy_order(meta, dd, &build, start_gold)).unwrap_or_default();
        if steps.is_empty() {
            blocks.push(block("Full build".into(), main.completion.iter().map(|i| i.id).collect()));
        } else {
            blocks.extend(steps);
        }
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

/// A title or file name that starts with a known build tool's name ("Blitz: AP Zyra", "!!!blitz-0").
fn tool_title(title: &str) -> bool {
    let title = title.to_ascii_lowercase();
    let title = title.trim_start_matches(|c: char| !c.is_alphanumeric());
    ["blitz", "porofessor", "mobalytics", "u.gg", "op.gg", "lolalytics", "facecheck", "metasrc"]
        .iter()
        .any(|name| title.strip_prefix(name).is_some_and(|suffix| suffix.chars().next().is_none_or(|c| !c.is_alphanumeric())))
}

/// Only remove recognised generated pages that apply to this champion and map.
fn tool_build(set: &Value, cid: u32) -> bool {
    let tool = tool_title(set["title"].as_str().unwrap_or(""));
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

/// Some tools (Blitz) bypass the client's item sets and drop a file straight into the game's
/// `Config/Global/Recommended` (every champion) or `Config/Champions/<alias>/Recommended`. The
/// game reads those itself, so a leftover one opens ahead of ours whatever the client's
/// order says, even for a different champion. Removes the recognised ones that apply to this
/// champion on ARAM; the client's own `RIOT_ItemSet_*` exports and anything personal stay.
pub fn remove_tool_files(league_dir: &Path, alias: &str) -> Vec<String> {
    let config = league_dir.join("Config");
    let mut dirs = vec![config.join("Global").join("Recommended")];
    let alias = crate::model::norm(alias);
    if !alias.is_empty() {
        for champ in std::fs::read_dir(config.join("Champions")).into_iter().flatten().flatten() {
            if crate::model::norm(&champ.file_name().to_string_lossy()) == alias {
                dirs.push(champ.path().join("Recommended"));
            }
        }
    }
    let mut removed = vec![];
    for file in dirs.iter().flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten()) {
        let path = file.path();
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let json = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if !json || stem.to_ascii_lowercase().starts_with("riot_itemset") {
            continue;
        }
        let Some(set) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            continue;
        };
        let title = set["title"].as_str().unwrap_or("");
        let other_map = set["map"].as_str().is_some_and(|m| !m.eq_ignore_ascii_case("any") && !m.eq_ignore_ascii_case("HA"));
        if other_map || !(tool_title(title) || tool_title(&stem)) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(if title.is_empty() { stem } else { title.to_string() }),
            Err(e) => log::warn!("couldn't remove {}: {e}", path.display()),
        }
    }
    removed
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
        let pages = make_item_sets(&Meta::default(), &detail(), 1, Some(2), false, None, 1400.0);
        assert_eq!(pages.len(), 5); // no silent four-style limit
        assert_eq!(pages[0]["blocks"][0]["items"][0]["id"], "4");
        assert!(pages[0]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["type"].as_str().unwrap().contains("Variant")));
        let pages = make_item_sets(&Meta::default(), &detail(), 1, None, false, None, 1400.0);
        assert_eq!(pages[0]["blocks"][0]["items"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn combined_contains_every_style_and_empty_is_safe() {
        let pages = make_item_sets(&Meta::default(), &detail(), 1, Some(1), true, None, 1400.0);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["blocks"][0]["items"][0]["id"], "3");
        let all = pages[0]["blocks"].to_string();
        for id in [1, 2, 3, 4, 5, 6, 7] {
            assert!(all.contains(&format!("\"id\":\"{id}\"")));
        }
        assert!(make_item_sets(&Meta::default(), &ChampDetail::default(), 1, None, true, None, 1400.0).is_empty());
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
    fn buy_order_lists_components_then_item_with_boots_second() {
        let detail: ChampDetail = serde_json::from_value(json!({"itemClusters": {"groups": [
            {"name_en":"AD", "core":[{"id":10}], "completion":[{"id":10},{"id":20},{"id":30}]}
        ]}}))
        .unwrap();
        let dd: HashMap<String, DdItem> = serde_json::from_value(json!({
            "1": {"gold":{"total":350}}, "2": {"gold":{"total":1300}}, "3": {"gold":{"total":300}},
            "10": {"gold":{"total":3000}, "from":["1","2","1","999"]},
            "20": {"gold":{"total":2800}},
            "30": {"gold":{"total":1100}, "from":["3"], "tags":["Boots"]},
            "2031": {"gold":{"total":150}}
        }))
        .unwrap();
        let pages = make_item_sets(&Meta::default(), &detail, 1, None, false, Some(&dd), 1400.0);
        let blocks = pages[0]["blocks"].as_array().unwrap();
        let buys: Vec<&Value> = blocks.iter().filter(|b| b["type"].as_str().unwrap().starts_with("Buy ")).collect();
        assert_eq!(buys.len(), 3);
        // 1400g: the 1300g component, not the 350g ones; the 100g left doesn't reach boots
        let start = blocks.iter().find(|b| b["type"].as_str().unwrap().starts_with("Start")).unwrap();
        assert_eq!(start["type"], "Start · 1300g of 1400g");
        assert_eq!(start["items"], json!([{"id":"2","count":1}]));
        // 1000g: the big component is out of reach, so two small ones and basic boots
        let pages = make_item_sets(&Meta::default(), &detail, 1, None, false, Some(&dd), 1000.0);
        let start = pages[0]["blocks"].as_array().unwrap().iter().find(|b| b["type"].as_str().unwrap().starts_with("Start")).unwrap().clone();
        assert_eq!(start["type"], "Start · 1000g of 1000g");
        assert_eq!(start["items"], json!([{"id":"1","count":2},{"id":"3","count":1}]));
        // 1500g: 200g spare after the big component, so a Refillable Potion joins it
        let pages = make_item_sets(&Meta::default(), &detail, 1, None, false, Some(&dd), 1500.0);
        let start = pages[0]["blocks"].as_array().unwrap().iter().find(|b| b["type"].as_str().unwrap().starts_with("Start")).unwrap().clone();
        assert_eq!(start["type"], "Start · 1450g of 1500g");
        assert_eq!(start["items"], json!([{"id":"2","count":1},{"id":"2031","count":1}]));
        assert!(buys[0]["type"].as_str().unwrap().ends_with("3000g"));
        // dearest component first, the doubled one counted, unknown ids dropped, item last
        assert_eq!(buys[0]["items"], json!([{"id":"2","count":1},{"id":"1","count":2},{"id":"10","count":1}]));
        assert_eq!(buys[1]["items"], json!([{"id":"3","count":1},{"id":"30","count":1}]));
        assert_eq!(buys[2]["items"], json!([{"id":"20","count":1}]));
        assert!(!blocks.iter().any(|b| b["type"] == "Full build"));
        // without recipes the plain full-build row stays
        let pages = make_item_sets(&Meta::default(), &detail, 1, None, false, None, 1400.0);
        assert!(pages[0]["blocks"].as_array().unwrap().iter().any(|b| b["type"] == "Full build"));
    }
    #[test]
    fn removes_only_tool_files_for_this_champion_and_map() {
        let root = std::env::temp_dir().join(format!("mayhem-tool-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let put = |dir: &str, name: &str, body: Value| {
            let dir = root.join("Config").join(dir).join("Recommended");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), body.to_string()).unwrap();
            dir.join(name)
        };
        let global = put("Global", "!!!blitz-0.json", json!({"title":"Blitz: AP Zyra", "championKey":"Zyra", "map":"any", "priority":true}));
        let rift = put("Global", "blitz-sr.json", json!({"title":"Blitz: Zyra", "map":"SR"}));
        let mine = put("Global", "mine.json", json!({"title":"My anti-Blitz build", "map":"any"}));
        let client = put("Champions/TwistedFate", "RIOT_ItemSet_6.json", json!({"title":"U.GG - Twisted Fate", "map":"HA"}));
        let champ = put("Champions/TwistedFate", "ugg.json", json!({"title":"U.GG - Twisted Fate", "map":"HA"}));
        let other = put("Champions/Olaf", "ugg.json", json!({"title":"U.GG - Olaf", "map":"HA"}));
        let removed = remove_tool_files(&root, "TwistedFate");
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"Blitz: AP Zyra".to_string()));
        assert!(!global.exists() && !champ.exists());
        assert!(rift.exists() && mine.exists() && client.exists() && other.exists());
        assert!(remove_tool_files(&root.join("missing"), "Olaf").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn old_config_gets_new_defaults_and_keeps_user_preferences() {
        let cfg: crate::config::Config = serde_json::from_value(json!({"close_companion_in_game":false})).unwrap();
        assert!(!cfg.close_companion_in_game);
        assert!(cfg.replace_tool_builds && cfg.auto_update_app && cfg.auto_update_data);
        assert!(!cfg.combined_item_sets);
    }
}
