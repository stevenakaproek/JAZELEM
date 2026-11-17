//! Network page - drawing only. The settings, state and apply / undo logic live in pctuner_core::network.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::network::*;

pub struct NetworkTab {
    n: Network,
}

impl NetworkTab {
    pub fn new() -> Self {
        NetworkTab { n: Network::new() }
    }

    fn window(&mut self, ctx: &egui::Context) {
        let can = self.n.can_act();
        let Some(op) = &mut self.n.op else { return };
        if let Some(ev) = op.show(ctx, can) {
            self.n.on_event(ev);
        }
    }
}

impl Tab for NetworkTab {
    fn title(&self) -> &'static str {
        "Network"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.n.pump();
        self.window(ui.ctx());
        if self.n.loading.is_some() || self.n.run.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }

        // shown inside the Optimization tab, which draws the page header and admin banner
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui.add_enabled(self.n.loading.is_none(), egui::Button::new("Refresh")).clicked() {
                self.n.refresh();
            }
        });
        egui::ScrollArea::vertical().id_salt("network-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl NetworkTab {
    fn page(&mut self, ui: &mut egui::Ui) {
        // only while reading, or when no adapter could be read
        match &self.n.state {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reading your network adapter...").color(theme::TEXT_DIM));
                });
            }
            Some(s) if s.info.is_none() => {
                ui.colored_label(theme::LOCKED, s.error.clone().unwrap_or_default());
            }
            Some(_) => {}
        }

        // ---- settings
        let Some(state) = self.n.state.clone() else { return };
        if state.info.is_none() {
            return;
        }
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
                    ui.label(RichText::new(group).size(15.0).strong().color(if t.optional { theme::OPTIONAL } else { theme::TEXT }));
                    ui.add_space(2.0);
                }
                let applicable = state.applicable(t.kind);
                let set = state.is_set(t);
                // an applied setting has nothing to apply, so it cannot be selected
                let selectable = applicable && !set && self.n.run.is_none();
                let sel = self.n.selected[i] && selectable;
                let clicked = theme::select_row(ui, ("net", i), sel, selectable, |ui| {
                    ui.horizontal(|ui| {
                        if applicable {
                            theme::applied_badge(ui, set);
                        } else {
                            theme::chip(ui, "not on this adapter", theme::DIM);
                        }
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.set_width((ui.available_width() - 260.0).max(240.0));
                            ui.label(RichText::new(t.name).strong().color(if applicable { theme::TEXT } else { theme::TEXT_FAINT }));
                            ui.label(RichText::new(t.why).size(12.5).color(theme::TEXT_DIM));
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
                    self.n.selected[i] = !self.n.selected[i];
                }
                ui.add_space(3.0);
            }
            ui.add_space(8.0);
            let pending = selected_changes(&state, &self.n.selected).len();
            ui.horizontal(|ui| {
                let can = common::is_admin() && self.n.run.is_none() && self.n.loading.is_none();
                if ui
                    .add_enabled(can && pending > 0, theme::primary_button(&format!("   Apply selected ({pending})   ")))
                    .on_disabled_hover_text(if common::is_admin() { "Click the settings you want to apply." } else { "Run PC Tuner as administrator to change settings." })
                    .clicked()
                {
                    self.n.open_apply();
                }
                if pending > 0 && ui.button("Clear selection").clicked() {
                    self.n.selected = vec![false; TWEAKS.len()];
                }
                let has_undo = self.n.history.iter().any(|h| !h.undone);
                if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).clicked() {
                    self.n.open_undo(false);
                }
            });
        });
    }
}
