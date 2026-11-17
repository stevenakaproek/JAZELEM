//! Performance page - drawing only. The settings, state and apply / undo logic live in pctuner_core::perf.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::perf::*;

pub struct PerfTab {
    p: Perf,
    /// Defender exclusions for game folders (its own card and history)
    defender: crate::defender::DefenderCard,
}

impl PerfTab {
    pub fn new() -> Self {
        PerfTab { p: Perf::new(), defender: crate::defender::DefenderCard::new() }
    }

    fn window(&mut self, ctx: &egui::Context) {
        let can = self.p.can_act();
        let Some(op) = &mut self.p.op else { return };
        if let Some(ev) = op.show(ctx, can) {
            self.p.on_event(ev, can);
        }
    }
}

impl Tab for PerfTab {
    fn title(&self) -> &'static str {
        "Performance"
    }

    fn busy(&self) -> bool {
        self.p.busy() || self.defender.busy()
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.p.pump();
        self.window(ui.ctx());
        self.defender.pump(ui.ctx());
        if self.p.loading.is_some() || self.p.run.is_some() || self.p.explorer.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        // shown inside the Optimization tab, which draws the page header and admin banner
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui.add_enabled(self.p.loading.is_none(), egui::Button::new("Refresh")).clicked() {
                self.p.refresh();
            }
        });
        egui::ScrollArea::vertical().id_salt("perf-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl PerfTab {
    fn page(&mut self, ui: &mut egui::Ui) {
        let Some(state) = self.p.state.clone() else {
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Reading your settings...").size(18.0).strong());
                ui.add_space(6.0);
                theme::sweep(ui, ui.available_width());
            });
            return;
        };
        if let Some(e) = &state.error {
            // without a reliable read nothing may be changed: the saved "before" values would be guesses
            theme::banner(ui, theme::LOCKED, e);
            ui.label(RichText::new("Nothing can be changed until the settings are read correctly. Press Refresh to try again.").color(theme::TEXT_DIM));
            return;
        }
        // ---- settings
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Settings");
                ui.label(RichText::new("click the ones you want, then Apply selected").size(12.0).color(theme::TEXT_FAINT));
            });
            let mut group = "";
            for (i, t) in TWEAKS.iter().enumerate() {
                if t.group != group {
                    group = t.group;
                    ui.add_space(6.0);
                    ui.label(RichText::new(group).size(15.0).strong());
                    ui.add_space(2.0);
                }
                let applicable = state.applicable(t.kind);
                let set = state.is_set(t);
                // an applied setting has nothing to apply, so it cannot be selected
                let selectable = applicable && !set && self.p.run.is_none();
                let sel = self.p.selected[i] && selectable;
                let clicked = theme::select_row(ui, ("perf", i), sel, selectable, |ui| {
                    ui.horizontal(|ui| {
                        if applicable {
                            theme::applied_badge(ui, set);
                        } else {
                            theme::chip_response(ui, "not available", theme::DIM).on_hover_text(state.blocked_reason(t.kind));
                        }
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.set_width((ui.available_width() - 260.0).max(240.0));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(t.name).strong().color(if applicable { theme::TEXT } else { theme::TEXT_FAINT }));
                                if t.optional {
                                    ui.label(RichText::new("optional").size(11.5).color(theme::OPTIONAL));
                                }
                            });
                            ui.label(RichText::new(t.why).size(12.5).color(theme::TEXT_DIM));
                            if t.kind == Kind::Hibernate {
                                if let Some(gb) = state.hiberfil_gb {
                                    ui.label(RichText::new(format!("hiberfil.sys is {gb:.1} GB now")).size(11.5).color(theme::TEXT_FAINT));
                                }
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if applicable {
                                let now = fmt(t.kind, state.value(t.kind).as_deref());
                                let (text, c) = if set {
                                    (now, theme::OK)
                                } else {
                                    (format!("{now} -> {}", fmt(t.kind, Some(t.target))), if sel { theme::ACCENT } else { theme::TEXT_DIM })
                                };
                                ui.label(RichText::new(text).color(c));
                            }
                        });
                    });
                });
                if clicked {
                    self.p.selected[i] = !self.p.selected[i];
                }
                ui.add_space(3.0);
            }
            ui.add_space(8.0);
            let pending = selected_changes(&state, &self.p.selected).len();
            ui.horizontal(|ui| {
                let can = common::is_admin() && self.p.run.is_none() && self.p.loading.is_none();
                if ui
                    .add_enabled(can && pending > 0, theme::primary_button(&format!("   Apply selected ({pending})   ")))
                    .on_disabled_hover_text(if common::is_admin() { "Click the settings you want to apply." } else { "Run PC Tuner as administrator to change settings." })
                    .clicked()
                {
                    self.p.open_apply();
                }
                if pending > 0 && ui.button("Clear selection").clicked() {
                    self.p.selected = vec![false; TWEAKS.len()];
                }
                let has_undo = self.p.history.iter().any(|h| !h.undone);
                if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).clicked() {
                    self.p.open_undo(false);
                }
                let explorer_ok = self.p.explorer.is_none() && self.p.run.is_none() && self.p.op.is_none();
                if ui.add_enabled(explorer_ok, egui::Button::new("Restart Explorer")).on_hover_text("Shows taskbar and right-click changes. Shows a preview first. Not during a game: the taskbar disappears for a moment.").clicked() {
                    self.p.open_explorer_preview();
                }
            });
            if let Some(n) = &self.p.note {
                ui.label(RichText::new(n.as_str()).size(12.0).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(6.0);

        self.defender.card(ui);
    }
}
