use super::*;
impl App {
    pub(super) fn tab_changes(&mut self, ctx: &egui::Context, meta: &Meta) {
        let changes = &meta.patch_changes;
        egui::CentralPanel::default().show(ctx,|ui| {
            page_scroll(ui,"patch-changes",|ui| {
                ui.heading("Patch changes");
                if changes.current_patch.is_empty() || changes.baseline_patch.is_empty() {
                    ui.label("Patch comparison data is not available in this dataset yet."); return;
                }
                ui.label(RichText::new(format!("Patch {} compared with {} · {:.0}k current games / {:.0}k previous games",
                    changes.current_patch,changes.baseline_patch,changes.current_games/1000.0,changes.baseline_games/1000.0)).font(semi(15.0)));
                ui.label(RichText::new("Observed changes in win rate, in percentage points. These are statistical trends, not a list of balance edits or proof that a patch caused the change.").color(DIM));
                for (category,risers,fallers) in [("Champions",&changes.hero_risers,&changes.hero_fallers),
                    ("Augments",&changes.augment_risers,&changes.augment_fallers),("Items",&changes.item_risers,&changes.item_fallers)] {
                    ui.add_space(18.0);ui.heading(category);
                    for (direction,rows,color) in [("Rising",risers,GOOD),("Falling",fallers,BAD)] {
                        ui.label(RichText::new(direction).font(semi(14.0)).color(color));
                        if rows.is_empty() {ui.label("No qualifying changes in this sample.");continue;}
                        egui::Grid::new(("changes",category,direction)).num_columns(5).spacing([24.0,7.0]).striped(true).show(ui,|ui| {
                            for heading in ["Name","Previous WR","Current WR","Change","Games: previous → current"] {ui.strong(heading);}ui.end_row();
                            for row in rows.iter().filter(|r| r.current_wr.is_finite() && r.baseline_wr.is_finite() && r.delta.is_finite() && r.current_games>0.0 && r.baseline_games>0.0) {
                                let name=if row.name_en.is_empty() { format!("#{id}",id=row.id) } else {row.name_en.clone()};
                                let clickable=ui.selectable_label(false,name);
                                if clickable.clicked() {
                                    if category=="Champions" {self.selected=Some(row.id);self.tab=Tab::Champions;}
                                    if category=="Augments" {self.aug_selected=Some(row.id);self.tab=Tab::Augments;}
                                }
                                ui.label(format!("{:.1}%",row.baseline_wr*100.0));ui.label(format!("{:.1}%",row.current_wr*100.0));
                                ui.colored_label(if row.delta>=0.0 {GOOD} else {BAD},format!("{:+.2} pp",row.delta*100.0));
                                ui.label(format!("{} → {}",row.baseline_games as u64,row.current_games as u64));ui.end_row();
                            }
                        });
                    }
                }
                ui.add_space(12.0);ui.label(RichText::new("Source: arammeta.com · only changes meeting the source's sample requirements are included.").color(FAINT));
            });
        });
    }
}
