use super::*;
use crate::history::{self, Group, MatchRecord, Summary};

fn ago(created: u64) -> String {
    if created == 0 {
        return "Date unavailable".into();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let age = now.saturating_sub(created / 1000);
    if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    }
}
fn champion_name(meta: Option<&Meta>, id: u32) -> String {
    meta.and_then(|m| m.champs.get(&id.to_string()))
        .map(|c| c.name_en.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Champion #{id}"))
}
fn augment_name(meta: Option<&Meta>, id: u32) -> String {
    meta.and_then(|m| m.augs.get(&id.to_string()))
        .map(|a| a.name_en.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Augment #{id}"))
}

impl App {
    pub(super) fn tab_history(&mut self, ctx: &egui::Context, meta: Option<&Meta>) {
        let (account, revision, status) = {
            let s = self.shared.read().unwrap();
            (s.history_account.clone(), s.history_revision, s.history_status.clone())
        };
        let dir = crate::config::data_dir().join("history");
        let account = if account.is_empty() {
            history::last_account(&dir).unwrap_or_default()
        } else {
            account
        };
        if self.history_key != (account.clone(), revision) {
            self.history_key = (account.clone(), revision);
            self.history_records.clear();
            self.history_error.clear();
            if !account.is_empty() {
                match history::load(&dir, &account) {
                    Ok(records) => self.history_records = records,
                    Err(e) => self.history_error = e,
                }
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ScrollArea::vertical().id_salt("history-page").show(ui, |ui| {
                ui.heading("Your Mayhem history");
                ui.label(RichText::new("Saved on this PC. Accounts are kept separate. Only your own match statistics are stored.").color(DIM));
                ui.horizontal(|ui| {
                    if ui.button("Sync recent matches").clicked() {
                        let _ = self.tx.send(Msg::Ui(UiCmd::SyncHistory));
                    }
                    ui.label(if status.is_empty() {
                        "Open League to import recent Mayhem matches"
                    } else {
                        &status
                    });
                });
                if !self.history_error.is_empty() {
                    ui.colored_label(BAD, &self.history_error);
                }
                if self.history_records.is_empty() {
                    ui.add_space(20.0);
                    ui.label("No saved Mayhem matches for this account yet. Recent matches import while League is open, and after each game.");
                    ui.label("Imports check the latest 40 matches; older matches accumulate as you keep playing with the companion.");
                    return;
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label("Champion:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.history_search)
                            .hint_text("Search your matches")
                            .desired_width(190.0),
                    );
                    let mut patches: Vec<_> = self.history_records.iter().map(|g| g.patch.clone()).filter(|s| !s.is_empty()).collect();
                    patches.sort();
                    patches.dedup();
                    patches.reverse();
                    egui::ComboBox::from_id_salt("history-patch")
                        .selected_text(if self.history_patch.is_empty() { "All patches" } else { &self.history_patch })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.history_patch, String::new(), "All patches");
                            for patch in patches {
                                ui.selectable_value(&mut self.history_patch, patch.clone(), patch);
                            }
                        });
                });
                let query = self.history_search.to_lowercase();
                let games: Vec<MatchRecord> = self
                    .history_records
                    .iter()
                    .filter(|g| {
                        (self.history_patch.is_empty() || g.patch == self.history_patch)
                            && (query.is_empty() || champion_name(meta, g.champion).to_lowercase().contains(&query))
                    })
                    .cloned()
                    .collect();
                if games.is_empty() {
                    ui.label("No matches match these filters.");
                    return;
                }
                let mut total = Summary::default();
                for game in &games {
                    total.add(game);
                }
                let n = total.games as f64;
                ui.add_space(12.0);
                ui.label(
                    RichText::new(format!(
                        "{} games · {:.1}% wins · {:.1}/{:.1}/{:.1} average K/D/A · {:.0} champion damage per game",
                        total.games,
                        total.win_rate() * 100.0,
                        total.kills as f64 / n,
                        total.deaths as f64 / n,
                        total.assists as f64 / n,
                        total.damage as f64 / n
                    ))
                    .font(semi(15.0)),
                );
                ui.label(
                    RichText::new("Personal samples can be small. Item and augment results describe your games, not their independent effect on winning.")
                        .color(DIM),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.history_group, Group::Champion, "Champions");
                    ui.selectable_value(&mut self.history_group, Group::Augment, "Augments");
                    ui.selectable_value(&mut self.history_group, Group::Item, "Items");
                });
                let rows = history::summarize(&games, self.history_group);
                if rows.is_empty() {
                    ui.label("These matches contain no recorded data for this category.");
                }
                egui::Grid::new("personal-summary")
                    .num_columns(4)
                    .spacing([30.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for heading in ["Most played (top 20)", "Games", "Wins", "Win rate"] {
                            ui.strong(heading);
                        }
                        ui.end_row();
                        for (id, summary) in rows.into_iter().take(20) {
                            let name = match self.history_group {
                                Group::Champion => champion_name(meta, id),
                                Group::Augment => augment_name(meta, id),
                                Group::Item => meta.map(|m| m.item_name(id)).unwrap_or_else(|| format!("Item #{id}")),
                            };
                            ui.label(name);
                            ui.label(summary.games.to_string());
                            ui.label(summary.wins.to_string());
                            ui.label(format!("{:.1}%", summary.win_rate() * 100.0));
                            ui.end_row();
                        }
                    });
                ui.add_space(18.0);
                ui.heading("Matches");
                for game in games {
                    let label = format!(
                        "{} · {} · {}/{}/{} · {} · {}:{:02} · Patch {}",
                        if game.win { "WIN" } else { "LOSS" },
                        champion_name(meta, game.champion),
                        game.kills,
                        game.deaths,
                        game.assists,
                        ago(game.created_ms),
                        game.duration_secs / 60,
                        game.duration_secs % 60,
                        game.patch
                    );
                    egui::CollapsingHeader::new(RichText::new(label).color(if game.win { GOOD } else { BAD }))
                        .id_salt(game.game_id)
                        .show(ui, |ui| {
                            ui.label(format!("Damage to champions: {}", game.damage));
                            let augments: Vec<_> = game.augments.iter().map(|id| augment_name(meta, *id)).collect();
                            ui.label(format!(
                                "Augments: {}",
                                if augments.is_empty() { "Not recorded".into() } else { augments.join(" · ") }
                            ));
                            let items: Vec<_> = game
                                .items
                                .iter()
                                .map(|id| meta.map(|m| m.item_name(*id)).unwrap_or_else(|| format!("Item #{id}")))
                                .collect();
                            ui.label(format!(
                                "Final items: {}",
                                if items.is_empty() { "Not recorded".into() } else { items.join(" · ") }
                            ));
                        });
                }
            });
        });
    }
}
