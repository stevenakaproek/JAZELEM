//! Defender exclusions card on the Performance page - drawing only.
//! The scan, the exclusion scripts and the history live in pctuner_core::defender.

use crate::common;
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::defender::Defender;

pub struct DefenderCard {
    d: Defender,
}

impl DefenderCard {
    pub fn new() -> Self {
        DefenderCard { d: Defender::new() }
    }

    pub fn busy(&self) -> bool {
        self.d.busy()
    }

    /// Poll background work; draw the operation window.
    pub fn pump(&mut self, ctx: &egui::Context) {
        self.d.pump();
        let can = self.d.can_act();
        let Some(op) = &mut self.d.op else { return };
        if let Some(ev) = op.show(ctx, can) {
            self.d.on_event(ev);
        }
    }

    /// The card on the Performance page.
    pub fn card(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Defender: game folders");
                ui.label(RichText::new("optional").size(11.5).color(theme::OPTIONAL));
            });
            ui.label(
                RichText::new("Excluded folders are not scanned by Microsoft Defender: games can load and patch a little faster, but Defender will not catch anything harmful inside them. Real-time protection stays on everywhere else.")
                    .size(12.5)
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(4.0);
            let Some(s) = self.d.scan.clone() else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for game folders...");
                });
                return;
            };
            if !s.error.is_empty() {
                theme::banner(ui, theme::GAMING, &s.error);
            }
            if s.mode.eq_ignore_ascii_case("passive") {
                theme::banner(ui, theme::GAMING, "Another antivirus is active - Defender is in passive mode, so its exclusions do nothing. Use that antivirus' own exclusions.");
            }
            let readable = s.excluded.is_some();
            if !readable && s.error.is_empty() {
                ui.label(RichText::new("Run PC Tuner as administrator to see and change Defender exclusions.").color(theme::LOCKED));
            }
            let rows = self.d.rows();
            if rows.is_empty() {
                ui.label(RichText::new("No game folders found. Add one below.").color(theme::TEXT_DIM));
            }
            for (kind, path) in &rows {
                let done = self.d.excluded(path);
                ui.horizontal(|ui| {
                    let mut on = done || self.d.is_ticked(path);
                    if ui.add_enabled(readable && !done && self.d.run.is_none(), egui::Checkbox::without_text(&mut on)).changed() {
                        self.d.set_ticked(path, on);
                    }
                    ui.label(RichText::new(path).strong());
                    ui.label(RichText::new(kind).size(12.0).color(theme::TEXT_FAINT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if done {
                            theme::chip(ui, "excluded", theme::OK);
                        } else if on && readable {
                            theme::chip(ui, "will exclude", theme::JUNK);
                        } else {
                            theme::chip(ui, "scanned", theme::DIM);
                        }
                    });
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.d.custom).hint_text("Other games folder, e.g. D:\\Games").desired_width(260.0));
                if ui.button("Add folder").clicked() {
                    self.d.add_custom();
                }
                if let Some(e) = self.d.custom_err {
                    ui.colored_label(theme::LOCKED, e);
                }
            });
            ui.add_space(6.0);
            let todo = self.d.todo(&rows);
            let can = common::is_admin() && readable && self.d.run.is_none();
            ui.horizontal(|ui| {
                let label = match todo.len() {
                    0 => "   Exclude   ".to_string(),
                    1 => "   Exclude 1 folder   ".to_string(),
                    n => format!("   Exclude {n} folders   "),
                };
                if ui.add_enabled(can && !todo.is_empty(), theme::primary_button(&label)).clicked() {
                    self.d.open_exclude(todo.clone());
                }
                if ui.add_enabled(can && self.d.has_undo(), egui::Button::new("Undo exclusions")).on_hover_text("Removes only the exclusions PC Tuner added.").clicked() {
                    self.d.open_undo(false);
                }
                if ui.add_enabled(self.d.loading.is_none(), egui::Button::new("Rescan")).clicked() {
                    self.d.refresh();
                }
            });
        });
        ui.add_space(6.0);
    }
}
