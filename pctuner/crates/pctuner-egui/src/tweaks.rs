//! Windows tweaks page - drawing only. The script runs, status and window contents live in
//! pctuner_core::tweaks.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::tweaks::*;

/// Give empty tick boxes an outline so they can be seen on a card.
fn visible_checkboxes(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for w in [&mut widgets.inactive, &mut widgets.hovered] {
        w.bg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_FAINT);
        w.rounding = egui::Rounding::same(5.0);
    }
}

pub struct TweaksTab {
    t: Tweaks,
}

impl TweaksTab {
    pub fn new() -> Self {
        TweaksTab { t: Tweaks::new() }
    }

    fn window(&mut self, ctx: &egui::Context) {
        let can = self.t.can_act();
        let Some(op) = &mut self.t.op else { return };
        if let Some(ev) = op.show(ctx, can) {
            self.t.on_event(ev);
        }
    }
}

impl Tab for TweaksTab {
    fn title(&self) -> &'static str {
        "Post-install Tweaks"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.t.pump();
        self.window(ui.ctx());

        // shown inside the Optimization tab, which draws the page header and admin banner
        egui::ScrollArea::vertical().id_salt("tweaks-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl TweaksTab {
    fn page(&mut self, ui: &mut egui::Ui) {
        // ---- the tweaks + the actions
        let running = self.t.stream.as_ref().map(|s| s.exit.is_none()).unwrap_or(false);
        let can = common::is_admin() && !running;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Tweaks");
                ui.label(RichText::new("click the ones you want, then Apply selected").size(12.0).color(theme::TEXT_FAINT));
            });
            ui.add_space(2.0);
            let n_tasks = script_tasks().len();
            let rows: [(&'static str, &'static str, String); 4] = [
                (G_TASKS, "Telemetry and tips tasks off", format!("{n_tasks} telemetry, tips, maps and family-safety tasks are disabled. Xbox and Steam tasks are kept.")),
                (G_EDGE, "Edge preloading off", "Startup Boost and background mode off. Edge stays installed.".into()),
                (G_XBOX, "Teredo on for Xbox online play", "Teredo set to \"enterpriseclient\" by Group Policy so Windows keeps it.".into()),
                (G_ONEDRIVE, "Remove OneDrive", "Uninstalls OneDrive. \"Undo tweaks\" does not bring it back.".into()),
            ];
            for (i, (group, name, text)) in rows.iter().enumerate() {
                let applied = self.t.status.as_ref().and_then(|s| s.applied(group));
                let selectable = can && applied == Some(false);
                let sel = self.t.selected[i] && selectable;
                let clicked = theme::select_row(ui, ("tweak", i), sel, selectable, |ui| {
                    ui.horizontal(|ui| {
                        match applied {
                            Some(a) => theme::applied_badge(ui, a),
                            None => theme::chip(ui, "checking...", theme::DIM),
                        }
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.set_width((ui.available_width() - 200.0).max(240.0));
                            ui.label(RichText::new(*name).strong().color(if *group == G_ONEDRIVE { theme::LOCKED } else { theme::TEXT }));
                            ui.label(RichText::new(text).size(12.5).color(theme::TEXT_DIM));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(s) = &self.t.status {
                                let c = if applied == Some(true) { theme::OK } else if sel { theme::ACCENT } else { theme::TEXT_DIM };
                                ui.label(RichText::new(s.now(group)).color(c));
                            }
                        });
                    });
                });
                if clicked {
                    self.t.selected[i] = !self.t.selected[i];
                }
                ui.add_space(3.0);
            }
            // only rows that still need applying count
            let picked: [bool; 4] = std::array::from_fn(|i| self.t.selected[i] && self.t.status.as_ref().and_then(|s| s.applied(GROUPS[i])) == Some(false));
            let n = picked.iter().filter(|s| **s).count();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can && n > 0, theme::primary_button(&format!("   Apply selected ({n})   ")))
                    .on_disabled_hover_text(if common::is_admin() { "Click the tweaks you want to apply." } else { "Run PC Tuner as administrator to apply tweaks." })
                    .clicked()
                {
                    if let Some(m) = pick_mode(&picked) {
                        self.t.ask(m);
                    }
                }
                if n > 0 && ui.button("Clear selection").clicked() {
                    self.t.selected = [false; 4];
                }
                if ui.add_enabled(can, egui::Button::new("Undo tweaks")).clicked() {
                    self.t.ask(Mode::Undo);
                }
                if ui.add_enabled(!running && self.t.status.is_some(), egui::Button::new("Refresh status")).clicked() {
                    self.t.status = None;
                }
            });
        });
        ui.add_space(6.0);

        // ---- last run + raw log
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, &match self.t.mode {
                    Some(m) => format!("Last run - {}", m.title()),
                    None => "Last run".to_string(),
                });
                if self.t.stream.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        visible_checkboxes(ui);
                        ui.checkbox(&mut self.t.show_details, "Show technical details");
                    });
                }
            });
            let Some(s) = &self.t.stream else {
                ui.label(RichText::new("Nothing has run in this session yet.").color(theme::TEXT_DIM));
                return;
            };
            let parsed = parse_log(&s.lines);
            ui.horizontal_wrapped(|ui| {
                match s.exit {
                    None => {
                        ui.spinner();
                        ui.label(RichText::new("Running...").strong());
                    }
                    Some(0) => theme::chip(ui, "finished", theme::OK),
                    Some(c) => theme::chip(ui, &format!("stopped, exit code {c}"), theme::LOCKED),
                }
                for g in [G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE] {
                    let total = parsed.items.iter().filter(|i| i.group == g).count();
                    let failed = parsed.items.iter().filter(|i| i.group == g && i.result.is_err()).count();
                    if total > 0 {
                        theme::chip(ui, &format!("{g}  {}/{total}", total - failed), if failed == 0 { theme::OK } else { theme::JUNK });
                    }
                }
            });
            if self.t.show_details {
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("tweaks-log").stick_to_bottom(true).max_height(280.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for l in &s.lines {
                        let c = if l.contains("FAIL") { theme::LOCKED } else { theme::TEXT_DIM };
                        ui.label(RichText::new(l).monospace().color(c));
                    }
                });
            }
        });
    }
}
