//! Companion window (egui, GPU-rendered, repaints only on input or new data).

mod images;
mod widgets;

use crate::config::Config;
use crate::data::Data;
use crate::engine::Msg;
use crate::model::*;
use crate::state::*;
use egui::{pos2, vec2, Align, Align2, Color32, Layout, Margin, Rect, RichText, Rounding, ScrollArea, Sense, Stroke, Ui};
use images::Images;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use widgets::*;
pub use widgets::setup;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    ChampSelect,
    Champions,
    Augments,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChampSort {
    Tier,
    Name,
    Games,
}

struct ChampRow {
    cid: u32,
    name: String,
    key: String,
    rating: Rating,
}

struct AugRow {
    id: u32,
    name: String,
    key: String,
    rarity: usize,
    cats: Vec<String>,
    cat_labels: Vec<String>,
    icon: String,
    rating: Rating,
    desc: String,
}

pub struct App {
    cfg: Config,
    data: Arc<Data>,
    shared: SharedRef,
    tx: Sender<Msg>,
    images: Images,
    tab: Tab,
    search: String,
    sort: ChampSort,
    selected: Option<u32>,
    round: Option<u32>,
    aug_search: String,
    aug_rarity: [bool; 3],
    aug_cat: Option<String>,
    aug_selected: Option<u32>,
    meta_key: usize,
    champ_rows: Vec<ChampRow>,
    aug_rows: Vec<AugRow>,
    ratings: HashMap<u32, Arc<HashMap<u32, AugRating>>>,
    best_for_aug: HashMap<u32, Vec<(u32, Rating)>>,
    pending_open: Option<(String, String)>,
}

impl App {
    pub fn new(cfg: Config, data: Arc<Data>, shared: SharedRef, tx: Sender<Msg>) -> App {
        App {
            cfg,
            data,
            shared,
            tx,
            images: Images::new(),
            tab: Tab::ChampSelect,
            search: String::new(),
            sort: ChampSort::Tier,
            selected: None,
            round: None,
            aug_search: String::new(),
            aug_rarity: [true; 3],
            aug_cat: None,
            aug_selected: None,
            meta_key: 0,
            champ_rows: vec![],
            aug_rows: vec![],
            ratings: HashMap::new(),
            best_for_aug: HashMap::new(),
            pending_open: None,
        }
    }

    /// Start on a tab / with a champion or augment selected (e.g. `--tab augments --select "Mad Scientist"`).
    pub fn open(&mut self, tab: Option<&str>, select: Option<&str>) {
        self.pending_open = Some((tab.unwrap_or("").to_ascii_lowercase(), select.unwrap_or("").to_string()));
    }

    fn apply_open(&mut self, meta: &Meta) {
        let Some((tab, select)) = self.pending_open.take() else { return };
        match tab.as_str() {
            "champions" => self.tab = Tab::Champions,
            "augments" => self.tab = Tab::Augments,
            "champselect" | "champ_select" => self.tab = Tab::ChampSelect,
            _ => {}
        }
        if select.is_empty() {
            return;
        }
        if let Some(cid) = meta.find_champ(&select) {
            self.selected = Some(cid);
        }
        let key = norm(&select);
        if let Some((id, _)) = meta.augs.iter().find(|(_, a)| norm(&a.name_en) == key) {
            self.aug_selected = id.parse().ok();
        }
    }

    fn refresh_tables(&mut self, meta: &Arc<Meta>) {
        let key = Arc::as_ptr(meta) as usize;
        if key == self.meta_key {
            return;
        }
        self.meta_key = key;
        self.ratings.clear();
        self.best_for_aug.clear();
        self.champ_rows = meta
            .champs
            .iter()
            .filter_map(|(id, c)| {
                Some(ChampRow {
                    cid: id.parse().ok()?,
                    name: c.name_en.clone(),
                    key: norm(&c.name_en),
                    rating: champion_rating(c, &self.cfg),
                })
            })
            .collect();
        self.aug_rows = meta
            .augs
            .iter()
            .filter_map(|(id, a)| {
                let id: u32 = id.parse().ok()?;
                Some(AugRow {
                    id,
                    name: a.name_en.clone(),
                    key: norm(&a.name_en),
                    rarity: RARITIES.iter().position(|r| r.0 == a.rarity).unwrap_or(2),
                    cats: a.cats.clone(),
                    cat_labels: a.cats.iter().filter(|c| *c != "new").map(|c| meta.cat_label(c)).collect(),
                    icon: meta.aug_icon(id).unwrap_or_default(),
                    rating: global_aug_rating(a, &self.cfg),
                    desc: clean_desc(&a.desc_en),
                })
            })
            .collect();
        self.aug_rows.sort_by(|a, b| b.rating.delta.total_cmp(&a.rating.delta));
    }

    fn ratings_for(&mut self, meta: &Meta, detail: &ChampDetail, cid: u32) -> Arc<HashMap<u32, AugRating>> {
        self.ratings.entry(cid).or_insert_with(|| Arc::new(champ_ratings(meta, detail, cid, &self.cfg))).clone()
    }

    fn tex(&mut self, ctx: &egui::Context, url: &str) -> Option<egui::TextureId> {
        self.images.get(ctx, url)
    }

    // ------------------------------------------------------------------ top bar

    fn top_bar(&mut self, ctx: &egui::Context, meta: &Meta) {
        let s = self.shared.read().unwrap();
        let (dot, status) = match (s.client, s.mayhem, s.phase.as_str()) {
            _ if s.demo => (WARN, "Demo mode".to_string()),
            (Client::NotRunning, ..) => (FAINT, "League client not running".into()),
            (_, Some(false), _) => (DIM, "Not a Mayhem game".into()),
            (_, Some(true), "ChampSelect") => (GOOD, "In champ select".into()),
            (_, Some(true), "InProgress") => (GOOD, "In game".into()),
            (_, _, p) if !p.is_empty() && p != "None" => (ACCENT, format!("Client: {p}")),
            _ => (ACCENT, "Client connected".into()),
        };
        drop(s);
        egui::TopBottomPanel::top("top")
            .exact_height(52.0)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::symmetric(18.0, 0.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("ARAM").font(bold(18.0)).color(TEXT));
                    ui.label(RichText::new("MAYHEM").font(bold(18.0)).color(rgb(Tier::SPlus.rgb())));
                    ui.add_space(20.0);
                    for (tab, label) in [(Tab::ChampSelect, "Champ Select"), (Tab::Champions, "Champions"), (Tab::Augments, "Augments")] {
                        let on = self.tab == tab;
                        let text = RichText::new(label).font(semi(14.5)).color(if on { TEXT } else { DIM });
                        let resp = ui.add(egui::Button::new(text).frame(false).min_size(vec2(0.0, 52.0)));
                        if on {
                            let r = resp.rect;
                            ui.painter().rect_filled(Rect::from_min_max(pos2(r.left(), r.bottom() - 3.0), r.right_bottom()), 1.5, ACCENT);
                        }
                        if resp.clicked() {
                            self.tab = tab;
                        }
                        ui.add_space(6.0);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Patch {} · {:.0}k games", meta.patch_prefix, meta.patch_changes.current_games / 1000.0))
                                .font(reg(12.5))
                                .color(FAINT),
                        );
                        ui.add_space(14.0);
                        ui.label(RichText::new(status).font(semi(13.0)).color(DIM));
                        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.5, dot);
                    });
                });
            });
    }

    // ------------------------------------------------------------------ champion rows

    /// A clickable row: icon, name, tier pill, win rate, optional extra on the right.
    fn champ_row(&mut self, ui: &mut Ui, meta: &Meta, cid: u32, h: f32, selected: bool, extra: Option<(String, Color32)>) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        if selected || resp.hovered() {
            ui.painter().rect_filled(rect, Rounding::same(6.0), if selected { CARD_HI } else { CARD });
        }
        let info = meta.champs.get(&cid.to_string()).cloned().unwrap_or_default();
        let rating = champion_rating(&info, &self.cfg);
        let size = h - 10.0;
        let icon_rect = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - size / 2.0), vec2(size, size));
        let tex = self.tex(ui.ctx(), &info.image);
        paint_icon(ui, icon_rect, tex, 6.0);
        text_at(ui, pos2(icon_rect.right() + 10.0, rect.center().y), Align2::LEFT_CENTER, &info.name_en, semi(14.5), TEXT);
        let mut x = rect.right() - 8.0;
        if let Some((t, c)) = extra {
            let r = text_at(ui, pos2(x, rect.center().y), Align2::RIGHT_CENTER, &t, semi(13.0), c);
            x = r.left() - 12.0;
        }
        let r = text_at(ui, pos2(x, rect.center().y), Align2::RIGHT_CENTER, &pct(rating.wr), reg(13.0), DIM);
        let pill_w = 30.0;
        tier_pill_at(ui, pos2(r.left() - 10.0 - pill_w, rect.center().y), rating.tier, 13.0);
        resp
    }

    // ------------------------------------------------------------------ champion detail

    fn champion_detail(&mut self, ui: &mut Ui, meta: &Arc<Meta>, cid: u32, import_ok: bool) {
        let Some(detail) = self.data.champ_cached(cid) else {
            let key = format!("champ{cid}");
            if let Some(err) = self.data.load_error(&key) {
                ui.label(RichText::new(format!("Couldn't load this champion: {err}")).color(BAD));
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading…").color(DIM));
                });
            }
            self.data.load_async(key, move |d| d.champ(cid).map(|_| ()), repaint);
            return;
        };
        let ratings = self.ratings_for(meta, &detail, cid);
        let info = meta.champs.get(&cid.to_string()).cloned().unwrap_or_default();
        let crating = champion_rating(&info, &self.cfg);

        // header
        ui.horizontal(|ui| {
            let tex = self.tex(ui.ctx(), &info.image);
            icon(ui, tex, 72.0, 10.0);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&info.name_en).font(bold(26.0)));
                    ui.add_space(4.0);
                    tier_pill(ui, crating.tier, 16.0);
                });
                ui.label(
                    RichText::new(format!(
                        "{} win rate · {:.0}k games · Mayhem {}",
                        pct(info.wr),
                        info.g / 1000.0,
                        meta.patch_prefix
                    ))
                    .font(reg(13.5))
                    .color(DIM),
                );
            });
        });

        ScrollArea::vertical().id_salt(("detail", cid)).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(ui.available_width() - 6.0);
            // ---- augments
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                section(ui, "AUGMENTS");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for (r, label) in [(Some(4), "4th"), (Some(3), "3rd"), (Some(2), "2nd"), (Some(1), "1st"), (None, "All picks")].iter() {
                        let on = self.round == *r;
                        let b = egui::Button::new(RichText::new(*label).font(semi(12.5)).color(if on { TEXT } else { DIM }))
                            .fill(if on { ACCENT.linear_multiply(0.45) } else { CARD })
                            .rounding(Rounding::same(5.0));
                        if ui.add(b).clicked() {
                            self.round = *r;
                        }
                    }
                    ui.label(RichText::new("Pick round").font(reg(12.5)).color(FAINT));
                });
            });
            let round = self.round;
            let cols: Vec<(usize, Vec<(u32, Rating)>)> = RARITIES
                .iter()
                .enumerate()
                .map(|(i, (key, ..))| {
                    let mut list: Vec<(u32, Rating)> = detail
                        .top
                        .get(*key)
                        .map(|l| l.iter().filter_map(|a| ratings.get(&a.id).map(|r| (a.id, r.at_round(round).0))).collect())
                        .unwrap_or_default();
                    list.sort_by(|a, b| b.1.delta.total_cmp(&a.1.delta));
                    (i, list)
                })
                .collect();
            ui.columns(3, |uis| {
                for (i, list) in &cols {
                    let ui = &mut uis[*i];
                    let (_, title, color) = RARITIES[*i];
                    card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(title.to_uppercase()).font(bold(12.5)).color(rgb(color)));
                        ui.add_space(4.0);
                        for (id, r) in list.iter().take(12) {
                            self.aug_line(ui, meta, *id, *r);
                        }
                        if list.len() > 12 {
                            ui.add_space(6.0);
                            ui.label(RichText::new("AVOID").font(bold(11.5)).color(BAD.linear_multiply(0.8)));
                            for (id, r) in list.iter().rev().take(3) {
                                self.aug_line(ui, meta, *id, *r);
                            }
                        }
                    });
                }
            });

            // ---- builds
            section(ui, "CORE BUILDS");
            for (i, g) in detail.clusters.groups.iter().take(3).enumerate() {
                card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&g.name_en).font(bold(15.0)));
                        ui.label(RichText::new(format!("{} WR · {:.0}% of games", pct(g.wr), g.pick * 100.0)).font(reg(13.0)).color(DIM));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let b = egui::Button::new(RichText::new("Import to client").font(semi(13.0)))
                                .fill(if import_ok { ACCENT.linear_multiply(0.55) } else { CARD_HI });
                            let resp = ui.add_enabled(import_ok, b);
                            let resp = resp.on_disabled_hover_text("Imports during champ select, for your current champion");
                            if resp.clicked() {
                                let _ = self.tx.send(Msg::Ui(UiCmd::ImportBuild { champ: cid, cluster: i }));
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for it in &g.core {
                            self.item_icon(ui, meta, it.id, 44.0, None);
                        }
                        ui.label(RichText::new("→").font(bold(18.0)).color(FAINT));
                        for it in g.options.iter().take(5) {
                            self.item_icon(ui, meta, it.id, 38.0, Some((it.wr, it.pick)));
                        }
                    });
                });
                ui.add_space(6.0);
            }

            // ---- boots / spells / items
            section(ui, "BOOTS · SPELLS · STRONGEST ITEMS");
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    for b in detail.boots.top.iter().take(3) {
                        if let Some(it) = b.items.first() {
                            self.item_icon(ui, meta, it.id, 38.0, Some((b.wr, b.pick)));
                        }
                    }
                    ui.add_space(18.0);
                    for s in detail.spells.top.iter().take(2) {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                for it in &s.items {
                                    let tex = self.tex(ui.ctx(), &it.icon);
                                    icon(ui, tex, 30.0, 5.0).on_hover_text(&it.name_en);
                                }
                            });
                            ui.label(RichText::new(format!("{} · {:.0}%", pct(s.wr), s.pick * 100.0)).font(reg(11.5)).color(DIM));
                        });
                        ui.add_space(8.0);
                    }
                    ui.add_space(18.0);
                    for s in detail.single_items.top.iter().take(8) {
                        if let Some(it) = s.items.first() {
                            self.item_icon(ui, meta, it.id, 38.0, Some((s.wr, s.pick)));
                        }
                    }
                });
            });
            ui.add_space(16.0);
        });
    }

    fn aug_line(&mut self, ui: &mut Ui, meta: &Meta, id: u32, r: Rating) {
        let Some(info) = meta.augs.get(&id.to_string()) else { return };
        let h = 30.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        if resp.hovered() {
            ui.painter().rect_filled(rect.expand(2.0), Rounding::same(5.0), CARD_HI);
        }
        let icon_rect = Rect::from_min_size(pos2(rect.left(), rect.center().y - 12.0), vec2(24.0, 24.0));
        let url = meta.aug_icon(id).unwrap_or_default();
        let tex = self.tex(ui.ctx(), &url);
        paint_icon(ui, icon_rect, tex, 4.0);
        let wr = text_at(ui, pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, &pct(r.wr), reg(12.5), DIM);
        let pill = tier_pill_at(ui, pos2(wr.left() - 40.0, rect.center().y), r.tier, 12.5);
        let _ = pill;
        let max_name = (wr.left() - 46.0) - (icon_rect.right() + 8.0);
        let name = elide(ui, &info.name_en, semi(13.5), max_name);
        text_at(ui, pos2(icon_rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, &name, semi(13.5), TEXT);
        resp.on_hover_ui(|ui| {
            ui.set_max_width(320.0);
            ui.label(RichText::new(&info.name_en).font(bold(15.0)));
            let cats: Vec<String> = info.cats.iter().filter(|c| *c != "new").map(|c| meta.cat_label(c)).collect();
            if !cats.is_empty() {
                ui.label(RichText::new(cats.join(" · ")).font(semi(12.0)).color(ACCENT));
            }
            ui.label(clean_desc(&info.desc_en));
            ui.label(RichText::new(format!("{} adjusted WR over {:.0} games", pct(r.wr), r.games)).font(reg(12.0)).color(DIM));
        });
    }

    fn item_icon(&mut self, ui: &mut Ui, meta: &Meta, id: u32, size: f32, stats: Option<(f64, f64)>) {
        let url = meta.item_icon(id);
        let tex = self.tex(ui.ctx(), &url);
        let name = meta.item_name(id);
        ui.vertical(|ui| {
            ui.set_width(size + 6.0);
            let resp = icon(ui, tex, size, 6.0);
            resp.on_hover_ui(|ui| {
                ui.label(RichText::new(&name).font(bold(14.0)));
                if let Some((wr, pick)) = stats {
                    ui.label(RichText::new(format!("{} WR · {:.0}% pick", pct(wr), pick * 100.0)).color(DIM));
                }
            });
            if let Some((wr, _)) = stats {
                ui.label(RichText::new(pct(wr)).font(reg(11.0)).color(DIM));
            }
        });
    }

    // ------------------------------------------------------------------ tabs

    fn tab_champ_select(&mut self, ctx: &egui::Context, meta: &Arc<Meta>) {
        let (cs, game, cards, status, last, demo) = {
            let s = self.shared.read().unwrap();
            (s.champ_select.clone(), s.game.clone(), s.cards.clone(), s.status.clone(), s.last_champ, s.demo)
        };
        let focus = cs.as_ref().map(|c| c.my_champ).filter(|c| *c > 0).or(game.as_ref().and_then(|g| g.champ)).or(last);
        egui::SidePanel::left("cs_left")
            .exact_width(340.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::same(16.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                ScrollArea::vertical().id_salt("cs_side").auto_shrink([false, false]).show(ui, |ui| {
                    if let Some(cs) = &cs {
                        self.champ_select_side(ui, meta, cs);
                    }
                    if let Some(g) = &game {
                        if cs.is_none() || demo {
                            self.in_game_side(ui, meta, g, &cards);
                        }
                    }
                    if cs.is_none() && game.is_none() {
                        ui.add_space(30.0);
                        ui.label(RichText::new("Waiting for an ARAM Mayhem game").font(bold(17.0)));
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "When champ select starts, this tab shows your pick, your bench and your team, \
                                 imports the build and sets your spells.\n\nIn game, augment cards get tier badges \
                                 automatically.",
                            )
                            .color(DIM),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(format!(
                                "{} in-game panel · {} this window · {} quit",
                                self.cfg.panel_hotkey, self.cfg.companion_hotkey, self.cfg.quit_hotkey
                            ))
                            .font(reg(12.0))
                            .color(FAINT),
                        );
                    }
                    if !status.is_empty() {
                        ui.add_space(14.0);
                        ui.label(RichText::new(&status).font(reg(12.5)).color(DIM));
                    }
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(Margin::same(20.0))).show(ctx, |ui| {
            match focus {
                Some(cid) => {
                    let import_ok = cs.as_ref().map(|c| c.my_champ == cid).unwrap_or(false) && !demo;
                    self.champion_detail(ui, meta, cid, import_ok)
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Pick a champion in the Champions tab to browse its builds").color(FAINT));
                    });
                }
            }
        });
    }

    fn champ_select_side(&mut self, ui: &mut Ui, meta: &Arc<Meta>, cs: &ChampSelectState) {
        let mine = meta.champs.get(&cs.my_champ.to_string()).map(|c| champion_rating(c, &self.cfg));
        ui.label(RichText::new("YOUR PICK").font(bold(12.0)).color(DIM));
        ui.add_space(4.0);
        if cs.my_champ > 0 {
            self.champ_row(ui, meta, cs.my_champ, 56.0, true, None);
        }
        if let Some(n) = cs.rerolls {
            ui.add_space(4.0);
            let b = egui::Button::new(RichText::new(format!("⟳  Reroll  ({n} left)")).font(semi(13.5)));
            if ui.add_enabled(n > 0, b).clicked() {
                let _ = self.tx.send(Msg::Ui(UiCmd::Reroll));
            }
        }
        if cs.bench_enabled && !cs.bench.is_empty() {
            ui.add_space(14.0);
            ui.label(RichText::new("BENCH").font(bold(12.0)).color(DIM));
            ui.add_space(2.0);
            let mut bench: Vec<(u32, f64)> = cs
                .bench
                .iter()
                .map(|c| (*c, meta.champs.get(&c.to_string()).map(|i| champion_rating(i, &self.cfg).delta).unwrap_or(0.0)))
                .collect();
            bench.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (cid, d) in bench {
                let diff = mine.map(|m| d - m.delta).unwrap_or(0.0);
                let (t, c) = delta_text(diff);
                let resp = self.champ_row(ui, meta, cid, 46.0, false, Some((t, c)));
                let resp = resp.on_hover_text("Click to swap");
                if resp.clicked() {
                    let _ = self.tx.send(Msg::Ui(UiCmd::SwapBench(cid)));
                }
            }
            ui.label(RichText::new("Click a bench champion to swap. ± is the win-rate gap vs your pick.").font(reg(11.5)).color(FAINT));
        }
        if !cs.teammates.is_empty() {
            ui.add_space(14.0);
            ui.label(RichText::new("TEAM").font(bold(12.0)).color(DIM));
            ui.add_space(2.0);
            for cid in cs.teammates.clone() {
                if self.champ_row(ui, meta, cid, 42.0, self.selected == Some(cid), None).clicked() {
                    self.selected = Some(cid);
                    self.tab = Tab::Champions;
                }
            }
        }
    }

    fn in_game_side(&mut self, ui: &mut Ui, meta: &Arc<Meta>, g: &GameState, cards: &[Badge]) {
        ui.add_space(10.0);
        ui.label(RichText::new("IN GAME").font(bold(12.0)).color(DIM));
        ui.add_space(4.0);
        if let Some(cid) = g.champ {
            self.champ_row(ui, meta, cid, 50.0, true, Some((format!("Lv {}", g.level), DIM)));
        }
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Next augment: pick {}", g.round)).font(semi(13.5)));
        let build = if g.build.is_empty() { "unknown".to_string() } else { g.build.join(" + ") };
        ui.label(
            RichText::new(format!("Build{}: {build}", if g.build_expected { " (expected)" } else { "" }))
                .font(reg(13.0))
                .color(DIM),
        );
        if !cards.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new("OFFERED NOW").font(bold(12.0)).color(DIM));
            for b in cards {
                card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let url = meta.aug_icon(b.aug_id).unwrap_or_default();
                        let tex = self.tex(ui.ctx(), &url);
                        icon(ui, tex, 30.0, 5.0);
                        ui.label(RichText::new(&b.name).font(semi(14.0)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(pct(b.wr)).color(DIM));
                            tier_pill(ui, b.tier, 13.0);
                            match b.tag {
                                Some(Tag::Best) => {
                                    ui.label(RichText::new("BEST").font(bold(11.5)).color(GOOD));
                                }
                                Some(Tag::Worst) => {
                                    ui.label(RichText::new("WORST").font(bold(11.5)).color(BAD));
                                }
                                None => {}
                            }
                        });
                    });
                    if let Some((fit, cats)) = &b.fit {
                        let (t, c) = match fit {
                            Fit::Fits => ("✓ fits build", GOOD),
                            Fit::Off => ("✗ off-build", WARN),
                            Fit::Any => ("• any build", DIM),
                        };
                        ui.label(RichText::new(format!("{t} · {}", cats.join(" / "))).font(semi(12.0)).color(c));
                    }
                });
                ui.add_space(4.0);
            }
        }
    }

    fn tab_champions(&mut self, ctx: &egui::Context, meta: &Arc<Meta>) {
        egui::SidePanel::left("champ_list")
            .exact_width(330.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::same(14.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search champions").desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Sort").font(reg(12.5)).color(FAINT));
                    for (s, l) in [(ChampSort::Tier, "Tier"), (ChampSort::Name, "Name"), (ChampSort::Games, "Popularity")] {
                        if ui.selectable_label(self.sort == s, RichText::new(l).font(semi(12.5))).clicked() {
                            self.sort = s;
                        }
                    }
                });
                ui.add_space(4.0);
                let q = norm(&self.search);
                let mut rows: Vec<(u32, Rating, String)> = self
                    .champ_rows
                    .iter()
                    .filter(|r| q.is_empty() || r.key.contains(&q))
                    .map(|r| (r.cid, r.rating, r.name.clone()))
                    .collect();
                match self.sort {
                    ChampSort::Tier => rows.sort_by(|a, b| b.1.delta.total_cmp(&a.1.delta)),
                    ChampSort::Name => rows.sort_by(|a, b| a.2.cmp(&b.2)),
                    ChampSort::Games => rows.sort_by(|a, b| b.1.games.total_cmp(&a.1.games)),
                }
                if self.selected.is_none() {
                    self.selected = rows.first().map(|r| r.0);
                }
                let row_h = 46.0;
                ScrollArea::vertical().id_salt("champ_rows").auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
                    for (cid, ..) in &rows[range] {
                        let sel = self.selected == Some(*cid);
                        if self.champ_row(ui, meta, *cid, row_h, sel, None).clicked() {
                            self.selected = Some(*cid);
                        }
                    }
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(Margin::same(20.0))).show(ctx, |ui| {
            if let Some(cid) = self.selected {
                let import_ok = self.shared.read().unwrap().champ_select.as_ref().map(|c| c.my_champ == cid).unwrap_or(false);
                self.champion_detail(ui, meta, cid, import_ok);
            }
        });
    }

    fn tab_augments(&mut self, ctx: &egui::Context, meta: &Arc<Meta>) {
        egui::SidePanel::right("aug_detail")
            .exact_width(360.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::same(18.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                let Some(id) = self.aug_selected else {
                    ui.label(RichText::new("Select an augment to see which champions it's best on").color(FAINT));
                    return;
                };
                self.augment_side(ui, meta, id);
            });
        egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(Margin::same(18.0))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.aug_search).hint_text("Search augments").desired_width(240.0));
                for (i, (_, label, color)) in RARITIES.iter().enumerate() {
                    let on = self.aug_rarity[i];
                    let b = egui::Button::new(RichText::new(*label).font(semi(13.0)).color(if on { rgb(*color) } else { FAINT }))
                        .fill(if on { CARD_HI } else { CARD });
                    if ui.add(b).clicked() {
                        self.aug_rarity[i] = !on;
                    }
                }
                let cats: Vec<String> = meta.aug_categories.order.iter().filter(|c| *c != "new" && *c != "other").cloned().collect();
                let current = self.aug_cat.as_ref().map(|c| meta.cat_label(c)).unwrap_or_else(|| "All categories".into());
                egui::ComboBox::from_id_salt("aug_cat").selected_text(current).width(170.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.aug_cat, None, "All categories");
                    for c in cats {
                        let label = meta.cat_label(&c);
                        ui.selectable_value(&mut self.aug_cat, Some(c), label);
                    }
                });
            });
            ui.add_space(8.0);
            let q = norm(&self.aug_search);
            let rows: Vec<usize> = self
                .aug_rows
                .iter()
                .enumerate()
                .filter(|(_, r)| self.aug_rarity[r.rarity])
                .filter(|(_, r)| q.is_empty() || r.key.contains(&q))
                .filter(|(_, r)| self.aug_cat.as_ref().map(|c| r.cats.contains(c)).unwrap_or(true))
                .map(|(i, _)| i)
                .collect();
            ui.label(RichText::new(format!("{} augments · tiers are across all champions (vs 50%)", rows.len())).font(reg(12.0)).color(FAINT));
            let row_h = 44.0;
            ScrollArea::vertical().id_salt("aug_rows").auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
                for i in &rows[range] {
                    let (id, name, rarity, labels, url, rating) = {
                        let r = &self.aug_rows[*i];
                        (r.id, r.name.clone(), r.rarity, r.cat_labels.join(" · "), r.icon.clone(), r.rating)
                    };
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    let sel = self.aug_selected == Some(id);
                    if sel || resp.hovered() {
                        ui.painter().rect_filled(rect, Rounding::same(6.0), if sel { CARD_HI } else { CARD });
                    }
                    let ir = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 15.0), vec2(30.0, 30.0));
                    let tex = self.tex(ui.ctx(), &url);
                    paint_icon(ui, ir, tex, 5.0);
                    text_at(ui, pos2(ir.right() + 10.0, rect.center().y - 8.0), Align2::LEFT_CENTER, &name, semi(14.5), TEXT);
                    let (_, rname, rcol) = RARITIES[rarity];
                    let rr = text_at(ui, pos2(ir.right() + 10.0, rect.center().y + 10.0), Align2::LEFT_CENTER, rname, semi(11.5), rgb(rcol));
                    text_at(ui, pos2(rr.right() + 8.0, rect.center().y + 10.0), Align2::LEFT_CENTER, &labels, reg(11.5), FAINT);
                    let g = text_at(ui, pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, &format!("{:.0}k games", rating.games / 1000.0), reg(12.0), FAINT);
                    let w = text_at(ui, pos2(g.left() - 18.0, rect.center().y), Align2::RIGHT_CENTER, &pct(rating.wr), reg(13.5), DIM);
                    tier_pill_at(ui, pos2(w.left() - 44.0, rect.center().y), rating.tier, 13.5);
                    if resp.clicked() {
                        self.aug_selected = Some(id);
                    }
                }
            });
        });
    }

    fn augment_side(&mut self, ui: &mut Ui, meta: &Arc<Meta>, id: u32) {
        let Some(row) = self.aug_rows.iter().find(|r| r.id == id) else { return };
        let (name, url, desc, labels, rating, rarity) =
            (row.name.clone(), row.icon.clone(), row.desc.clone(), row.cat_labels.join(" · "), row.rating, row.rarity);
        ui.horizontal(|ui| {
            let tex = self.tex(ui.ctx(), &url);
            icon(ui, tex, 56.0, 8.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&name).font(bold(20.0)));
                let (_, rname, rcol) = RARITIES[rarity];
                ui.horizontal(|ui| {
                    ui.label(RichText::new(rname).font(semi(12.5)).color(rgb(rcol)));
                    ui.label(RichText::new(&labels).font(reg(12.5)).color(DIM));
                });
            });
        });
        ui.add_space(6.0);
        ui.label(&desc);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            tier_pill(ui, rating.tier, 14.0);
            ui.label(RichText::new(format!("{} across all champions · {:.0}k games", pct(rating.wr), rating.games / 1000.0)).color(DIM));
        });
        ui.add_space(10.0);
        let Some(stats) = self.data.aug_stats_cached() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading per-champion stats…").color(DIM));
            });
            self.data.load_async("aug_stats".into(), |d| d.aug_stats().map(|_| ()), repaint);
            return;
        };
        let k = self.cfg.smoothing_games;
        let list = self.best_for_aug.entry(id).or_insert_with(|| {
            let mut out = vec![];
            for (cid, rarities) in stats.iter() {
                let Ok(cid) = cid.parse::<u32>() else { continue };
                let base = meta.champs.get(&cid.to_string()).map(|c| c.wr).unwrap_or(0.5);
                for a in rarities.values().flatten().filter(|a| a.id == id) {
                    let delta = (a.wr - base) * a.g / (a.g + k);
                    out.push((cid, Rating { tier: Tier::from_delta(delta, &self.cfg.tier_cutoffs), wr: base + delta, delta, games: a.g }));
                }
            }
            out.sort_by(|a, b| b.1.delta.total_cmp(&a.1.delta));
            out
        });
        let list = list.clone();
        ScrollArea::vertical().id_salt(("aug_side", id)).auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new("BEST ON").font(bold(12.0)).color(GOOD));
            for (cid, r) in list.iter().take(10) {
                let (t, c) = delta_text(r.delta);
                if self.champ_row(ui, meta, *cid, 40.0, false, Some((t, c))).clicked() {
                    self.selected = Some(*cid);
                    self.tab = Tab::Champions;
                }
            }
            if list.len() > 10 {
                ui.add_space(10.0);
                ui.label(RichText::new("WORST ON").font(bold(12.0)).color(BAD));
                for (cid, r) in list.iter().rev().take(5) {
                    let (t, c) = delta_text(r.delta);
                    if self.champ_row(ui, meta, *cid, 40.0, false, Some((t, c))).clicked() {
                        self.selected = Some(*cid);
                        self.tab = Tab::Champions;
                    }
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("± = win-rate gain over that champion's average").font(reg(11.5)).color(FAINT));
        });
    }
}

fn elide(ui: &Ui, text: &str, font: egui::FontId, max_w: f32) -> String {
    let w = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if w(text) <= max_w {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() && w(&format!("{s}…")) > max_w {
        s.pop();
    }
    format!("{}…", s.trim_end())
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let Some(meta) = self.data.meta_cached() else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    match self.data.load_error("meta") {
                        Some(e) => ui.label(RichText::new(format!("Couldn't load Mayhem data: {e}")).color(BAD)),
                        None => ui.label(RichText::new("Loading ARAM Mayhem data…").font(semi(16.0)).color(DIM)),
                    };
                });
            });
            self.data.load_async("meta".into(), |d| d.meta().map(|_| ()), repaint);
            return;
        };
        self.refresh_tables(&meta);
        self.apply_open(&meta);
        if std::mem::take(&mut self.shared.write().unwrap().show_champ_select) {
            self.tab = Tab::ChampSelect;
        }
        self.top_bar(ctx, &meta);
        match self.tab {
            Tab::ChampSelect => self.tab_champ_select(ctx, &meta),
            Tab::Champions => self.tab_champions(ctx, &meta),
            Tab::Augments => self.tab_augments(ctx, &meta),
        }
    }
}
