use super::*;
use crate::challenges::{self, Challenge, Snapshot, ROOT};
use crate::facts::thousands;

const GOD: &str = "ARAM God";

fn level_color(level: &str) -> Color32 {
    match level {
        "IRON" => Color32::from_rgb(0x8a, 0x8f, 0x96),
        "BRONZE" => Color32::from_rgb(0xc0, 0x84, 0x57),
        "SILVER" => Color32::from_rgb(0xb8, 0xc2, 0xcc),
        "GOLD" => Color32::from_rgb(0xf0, 0xbe, 0x4a),
        "PLATINUM" => Color32::from_rgb(0x4f, 0xc8, 0xb4),
        "DIAMOND" => Color32::from_rgb(0x6a, 0xa8, 0xff),
        "MASTER" => Color32::from_rgb(0xc4, 0x7c, 0xff),
        "GRANDMASTER" => Color32::from_rgb(0xff, 0x66, 0x66),
        "CHALLENGER" => Color32::from_rgb(0xff, 0xd8, 0x7a),
        _ => FAINT,
    }
}
fn level_name(level: &str) -> String {
    if level.is_empty() || level == "NONE" {
        return "Unranked".into();
    }
    let mut c = level.chars();
    c.next().map(|f| f.to_string() + &c.as_str().to_lowercase()).unwrap_or_default()
}
fn amount(v: f64) -> String {
    if v.fract().abs() > 1e-6 {
        format!("{v:.1}")
    } else {
        thousands(v.max(0.0) as u64)
    }
}
fn pill(ui: &Ui, pos: egui::Pos2, level: &str) -> f32 {
    let color = level_color(level);
    paint_pill(ui, pos, &level_name(level), semi(12.0), color.linear_multiply(0.18), color)
}
fn bar(ui: &Ui, rect: Rect, frac: f64, color: Color32) {
    ui.painter().rect_filled(rect, Rounding::same(rect.height() / 2.0), CARD_HI);
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * frac.clamp(0.0, 1.0) as f32, rect.height()));
    if fill.width() > 0.5 {
        ui.painter().rect_filled(fill, Rounding::same(rect.height() / 2.0), color);
    }
}
fn elided(ui: &Ui, pos: egui::Pos2, text: &str, font: egui::FontId, color: Color32, width: f32) -> Rect {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = ui.fonts(|f| f.layout_job(job));
    let rect = Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}
fn progress_text(c: &Challenge) -> (String, String) {
    if c.maxed() {
        (amount(c.value), "Highest level".into())
    } else {
        (format!("{} / {}", amount(c.value), amount(c.next_threshold)), format!("→ {}", level_name(&c.next_level)))
    }
}

/// One challenge: name and description on the left; level, progress bar and numbers on the right.
/// `open` is set for challenges with a champion list; returns true when the row was clicked.
fn challenge_row(ui: &mut Ui, c: &Challenge, open: Option<bool>) -> bool {
    let sense = if open.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), sense);
    if resp.hovered() || open == Some(true) {
        ui.painter().rect_filled(rect, Rounding::same(6.0), CARD);
    }
    let right_w = 360.0_f32.min(rect.width() * 0.55);
    let mut indent = 10.0;
    if let Some(open) = open {
        resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        // Disclosure arrow: right when closed, down when open.
        let m = pos2(rect.left() + 15.0, rect.top() + 15.0);
        let pts = if open {
            vec![m + vec2(-5.0, -3.0), m + vec2(5.0, -3.0), m + vec2(0.0, 4.0)]
        } else {
            vec![m + vec2(-3.0, -5.0), m + vec2(4.0, 0.0), m + vec2(-3.0, 5.0)]
        };
        ui.painter().add(egui::Shape::convex_polygon(pts, if resp.hovered() { TEXT } else { DIM }, Stroke::NONE));
        indent = 28.0;
    }
    let left_w = rect.width() - right_w - 14.0 - indent;
    elided(ui, rect.left_top() + vec2(indent, 6.0), &c.name, semi(14.5), TEXT, left_w);
    elided(ui, rect.left_top() + vec2(indent, 26.0), &c.description, reg(12.5), DIM, left_w);
    let x = rect.right() - right_w;
    pill(ui, pos2(x, rect.center().y), &c.level);
    let bar_left = x + 104.0;
    let bar_right = rect.right() - 10.0;
    let (value, next) = progress_text(c);
    text_at(ui, pos2(bar_left, rect.top() + 14.0), Align2::LEFT_CENTER, &value, semi(12.5), TEXT);
    text_at(ui, pos2(bar_right, rect.top() + 14.0), Align2::RIGHT_CENTER, &next, reg(12.0), DIM);
    let bar_rect = Rect::from_min_max(pos2(bar_left, rect.top() + 28.0), pos2(bar_right, rect.top() + 34.0));
    match c.progress() {
        Some(frac) => bar(ui, bar_rect, frac, level_color(if c.maxed() { &c.level } else { &c.next_level })),
        None => {
            text_at(ui, pos2(bar_left, bar_rect.center().y), Align2::LEFT_CENTER, "Lower is better", reg(11.5), FAINT);
        }
    }
    let clicked = resp.clicked();
    let mut hover = c.description.clone();
    if c.percentile > 0.0 && !c.capstone {
        hover += &format!("\nTop {:.1}% of players", c.percentile);
    }
    if let Some(open) = open {
        hover += if open { "\nClick to hide the champions" } else { "\nClick to see which champions are done and which are left" };
    }
    if !hover.trim().is_empty() {
        resp.on_hover_text(hover.trim());
    }
    clicked
}

#[derive(Clone, Copy, PartialEq, Default)]
pub(super) enum ListShow {
    #[default]
    All,
    Left,
    Done,
}

fn plain(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The champions still to do, then the ones done, for a one-per-champion challenge.
/// `search` and `show` narrow the lists; while searching, matches are named.
fn champion_lists(ui: &mut Ui, images: &mut Images, meta: Option<&Meta>, c: &Challenge, search: &mut String, show: &mut ListShow) {
    egui::Frame::none().inner_margin(Margin { left: 28.0, right: 10.0, top: 4.0, bottom: 12.0 }).show(ui, |ui| {
        let Some(meta) = meta else {
            ui.label(RichText::new(format!("{} champions done. The list appears once the Mayhem data has loaded.", c.completed_ids.len())).color(DIM));
            return;
        };
        let (done, left) = c.champions(meta.champs.keys().filter_map(|k| k.parse().ok()));
        if done.is_empty() && c.value >= 1.0 {
            ui.label(RichText::new("The League client hasn't said which champions are done yet. Press Refresh with League open.").color(DIM));
            return;
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(search).hint_text("Search champions").desired_width(220.0));
            for (v, label) in [(ListShow::All, "All".to_string()), (ListShow::Left, format!("Left · {}", left.len())), (ListShow::Done, format!("Done · {}", done.len()))] {
                if ui.selectable_label(*show == v, RichText::new(label).font(semi(12.5))).clicked() {
                    *show = v;
                }
            }
        });
        ui.add_space(4.0);
        let needle = plain(search);
        let mut shown = 0;
        let mut group = |ui: &mut Ui, title: &str, color: Color32, ids: &[u32], dim: bool| {
            let mut rows: Vec<(String, String)> = ids
                .iter()
                .filter_map(|id| match meta.champs.get(&id.to_string()) {
                    Some(i) => (needle.is_empty() || plain(&i.name_en).contains(&needle) || plain(&i.alias).contains(&needle)).then(|| (i.name_en.clone(), i.image.clone())),
                    None => needle.is_empty().then(|| (format!("Champion {id}"), String::new())),
                })
                .collect();
            if rows.is_empty() {
                return;
            }
            rows.sort();
            shown += rows.len();
            let count = if needle.is_empty() { format!("{} {title}", ids.len()) } else { format!("{} of {} {title}", rows.len(), ids.len()) };
            ui.label(RichText::new(count).font(semi(13.0)).color(color));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for (name, image) in rows {
                    // Named chips while searching; bare icons otherwise.
                    let name_w = if needle.is_empty() { 0.0 } else { ui.fonts(|f| f.layout_no_wrap(name.clone(), semi(13.0), TEXT).size().x) + 16.0 };
                    let (rect, resp) = ui.allocate_exact_size(vec2(36.0 + name_w, 36.0), Sense::hover());
                    if name_w > 0.0 {
                        ui.painter().rect_filled(rect, Rounding::same(5.0), CARD);
                        text_at(ui, pos2(rect.left() + 44.0, rect.center().y), Align2::LEFT_CENTER, &name, semi(13.0), if dim { DIM } else { TEXT });
                    }
                    let icon = Rect::from_min_size(rect.min, vec2(36.0, 36.0));
                    match images.get(ui.ctx(), &image).filter(|_| !image.is_empty()) {
                        Some(tex) => {
                            let tint = if dim { Color32::from_gray(80) } else { Color32::WHITE };
                            egui::Image::new(egui::load::SizedTexture::new(tex, icon.size())).rounding(5.0).tint(tint).paint_at(ui, icon);
                        }
                        None => paint_icon(ui, icon, None, 5.0),
                    }
                    if dim {
                        text_at(ui, icon.right_bottom() - vec2(3.0, 2.0), Align2::RIGHT_BOTTOM, "✓", bold(13.0), GOOD);
                    }
                    resp.on_hover_text(format!("{name} · {}", if dim { "done" } else { "left" }));
                }
            });
            ui.add_space(6.0);
        };
        if *show != ListShow::Done {
            if left.is_empty() {
                ui.label(RichText::new("✓ Every champion done").font(semi(13.0)).color(GOOD));
            } else {
                group(ui, "left", TEXT, &left, false);
            }
        }
        if *show != ListShow::Left {
            group(ui, "done", GOOD, &done, true);
        }
        if shown == 0 && !needle.is_empty() {
            ui.label(RichText::new(format!("No champions match \"{}\".", search.trim())).color(DIM));
        }
    });
}

/// A top-level section of the page: a large title, a line of explanation and a rule.
fn part(ui: &mut Ui, title: &str, about: &str) {
    ui.add_space(26.0);
    ui.label(RichText::new(title).font(bold(22.0)).color(TEXT));
    if !about.is_empty() {
        ui.label(RichText::new(about).color(DIM));
    }
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, Rounding::ZERO, BORDER);
    ui.add_space(4.0);
}

fn hero(ui: &mut Ui, snap: &Snapshot, root: &Challenge) {
    let god = root.titles.iter().find(|t| t.name == GOD).map(|t| t.level.clone()).unwrap_or_else(|| "MASTER".into());
    let need = root.threshold(&god).unwrap_or(0.0);
    let earned = root.has(&god);
    egui::Frame::none()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, if earned { level_color("MASTER").linear_multiply(0.6) } else { BORDER }))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::same(18.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(GOD).font(bold(28.0)).color(level_color("MASTER")));
                ui.add_space(8.0);
                if earned {
                    ui.label(RichText::new("✓ Earned").font(semi(15.0)).color(GOOD));
                    if snap.equipped_title == GOD {
                        ui.label(RichText::new("· equipped").font(reg(14.0)).color(DIM));
                    }
                } else {
                    ui.label(RichText::new(format!("{} points to go", amount(need - root.value))).font(semi(15.0)).color(TEXT));
                }
            });
            let how = if earned {
                format!("You reached {} in {}. The challenges below keep climbing toward Grandmaster and Challenger.", level_name(&god), root.name)
            } else {
                format!("Reach {} in {}: level up the ARAM Warrior, Finesse and Champion challenges below. Every level earns points toward it.", level_name(&god), root.name)
            };
            ui.label(RichText::new(how).color(DIM));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} {}", root.name, level_name(&root.level))).font(semi(14.0)).color(level_color(&root.level)));
                ui.label(RichText::new(format!("{} / {} points", amount(root.value), amount(need))).color(TEXT));
                if root.percentile > 0.0 {
                    ui.label(RichText::new(format!("· top {:.1}% of players", root.percentile)).color(DIM));
                }
            });
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
            bar(ui, r, if need > 0.0 { root.value / need } else { 1.0 }, level_color("MASTER"));
        });
}

fn title_chips(ui: &mut Ui, snap: &Snapshot) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (c, t) in snap.titles() {
            let earned = c.has(&t.level);
            let detail = if earned {
                format!("{} · {}", c.name, level_name(&t.level))
            } else {
                let to_go = c.threshold(&t.level).map(|need| format!(" · {} to go", amount(need - c.value))).unwrap_or_default();
                format!("{} {}{to_go}", c.name, level_name(&t.level))
            };
            egui::Frame::none()
                .fill(if earned { GOOD.linear_multiply(0.08) } else { CARD })
                .stroke(Stroke::new(1.0_f32, if earned { GOOD.linear_multiply(0.45) } else { BORDER }))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(12.0, 7.0))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let mark = if earned { "✓ " } else { "" };
                        let equipped = if snap.equipped_title == t.name { "  (equipped)" } else { "" };
                        ui.add(egui::Label::new(RichText::new(format!("{mark}{}{equipped}", t.name)).font(semi(14.0)).color(if earned { GOOD } else { TEXT })).extend());
                        ui.add(egui::Label::new(RichText::new(detail).font(reg(12.0)).color(DIM)).extend());
                    });
                });
        }
    });
}

impl App {
    /// A challenge row; one-per-champion challenges open to show the champions done and left.
    fn challenge_entry(&mut self, ui: &mut Ui, meta: Option<&Meta>, c: &Challenge) {
        if !c.per_champion() {
            challenge_row(ui, c, None);
            return;
        }
        let open = self.challenges_open.contains(&c.id);
        if challenge_row(ui, c, Some(open)) {
            if open {
                self.challenges_open.remove(&c.id);
            } else {
                self.challenges_open.insert(c.id);
            }
        } else if open {
            champion_lists(ui, &mut self.images, meta, c, &mut self.challenges_search, &mut self.challenges_show);
        }
    }

    pub(super) fn tab_challenges(&mut self, ctx: &egui::Context, meta: Option<&Meta>) {
        let (account, revision, client) = {
            let s = self.shared.read().unwrap();
            (s.history_account.clone(), s.history_revision, s.client)
        };
        let dir = crate::config::data_dir().join("history");
        let account = if account.is_empty() { crate::history::last_account(&dir).unwrap_or_default() } else { account };
        if self.challenges_key != (account.clone(), revision) {
            self.challenges_key = (account.clone(), revision);
            self.challenges = if account.is_empty() { None } else { challenges::load(&dir, &account) };
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            page_scroll(ui, "challenges-page", |ui| {
                ui.heading("Challenges");
                let Some(snap) = self.challenges.clone() else {
                    ui.label(RichText::new("Your challenge progress loads from the League client. Mayhem games count toward ARAM challenges.").color(DIM));
                    ui.add_space(8.0);
                    ui.label(if client == Client::NotRunning {
                        "Open League to load your ARAM challenges."
                    } else {
                        "Loading your ARAM challenges…"
                    });
                    return;
                };
                ui.horizontal(|ui| {
                    let age = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().saturating_sub(snap.fetched_ms / 1000);
                    let when = if age < 90 { "just now".to_string() } else if age < 5400 { format!("{} min ago", age / 60) } else { format!("{} h ago", age / 3600) };
                    ui.label(RichText::new(format!("Updated {when} from your League client · Mayhem games count toward ARAM challenges")).color(DIM));
                    if ui.button("Refresh").clicked() {
                        let _ = self.tx.send(Msg::Ui(UiCmd::SyncHistory));
                    }
                });
                let Some(root) = snap.get(ROOT) else { return };
                part(ui, GOD, "The title for reaching Master in ARAM Authority, and the other titles ARAM challenges award.");
                ui.add_space(6.0);
                hero(ui, &snap, root);

                ui.add_space(18.0);
                ui.heading("ARAM titles");
                title_chips(ui, &snap);

                let closest = snap.closest(5);
                if !closest.is_empty() {
                    ui.add_space(18.0);
                    ui.heading("Closest to the next level");
                    for c in closest {
                        self.challenge_entry(ui, meta, c);
                    }
                }

                part(ui, "ARAM challenges", "Only ARAM and Mayhem games count. Every level here earns points toward ARAM God.");
                for group in snap.children(ROOT) {
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let heading = ui.heading(&group.name).rect;
                        let width = pill(ui, pos2(heading.right() + 10.0, heading.center().y), &group.level);
                        ui.add_space(width + 20.0);
                        let text = if group.maxed() {
                            format!("{} points · highest level", amount(group.value))
                        } else {
                            format!("{} / {} points to {}", amount(group.value), amount(group.next_threshold), level_name(&group.next_level))
                        };
                        ui.label(RichText::new(text).color(DIM));
                    });
                    for c in snap.children(group.id) {
                        self.challenge_entry(ui, meta, c);
                    }
                }
                if !snap.other_aram.is_empty() {
                    ui.add_space(14.0);
                    ui.heading("Other ARAM challenges");
                    ui.label(RichText::new("ARAM-only, but outside ARAM Authority, so they don't count toward ARAM God.").color(DIM));
                    for c in &snap.other_aram {
                        self.challenge_entry(ui, meta, c);
                    }
                }

                if !snap.any_mode.is_empty() {
                    part(ui, "Any mode", "Not ARAM-specific, but Mayhem games move them: champion mastery and summoner level. They don't count toward ARAM God.");
                    let mut group = None;
                    for c in &snap.any_mode {
                        if group != Some(&c.parent_name) {
                            group = Some(&c.parent_name);
                            ui.add_space(14.0);
                            ui.heading(match c.parent_name.as_str() {
                                "Guru" => "Champion mastery",
                                "Virtuoso" => "Mastery by class",
                                "" => "Account",
                                name => name,
                            });
                        }
                        self.challenge_entry(ui, meta, c);
                    }
                }
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Grandmaster and Challenger cut-offs move with the ladder; numbers are as the League client reports them.")
                        .font(reg(12.0))
                        .color(FAINT),
                );
            });
        });
    }
}
