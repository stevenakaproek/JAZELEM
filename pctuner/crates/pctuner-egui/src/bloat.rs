//! Remove Bloat tab - drawing only. The target list, the script run and the pop-up contents live in
//! pctuner_core::bloat.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::bloat::*;

/// A hero-row stat that is left out when the window is too narrow for it (instead of wrapping letter by letter).
fn stat_fit(ui: &mut egui::Ui, gap: f32, value: &str, label: &str, c: egui::Color32) {
    if ui.available_width() < gap + 7.0 * label.chars().count().max(value.chars().count() * 2) as f32 {
        return;
    }
    ui.add_space(gap);
    theme::stat(ui, value, label, c);
}

/// Give empty tick boxes an outline so they can be seen on a card.
fn visible_checkboxes(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for w in [&mut widgets.inactive, &mut widgets.hovered] {
        w.bg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_FAINT);
        w.rounding = egui::Rounding::same(5.0);
    }
}

pub struct BloatTab {
    b: Bloat,
}

impl BloatTab {
    pub fn new() -> Self {
        BloatTab { b: Bloat::new() }
    }
}

impl Tab for BloatTab {
    fn title(&self) -> &'static str {
        "Remove Bloat"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.b.pump();
        let running = self.b.running();
        let can = common::is_admin() && !running;

        // The remove pop-up: confirm, then it stays open, ticks off each app as it goes and shows the
        // results (with the full log behind "Show technical details").
        let can_win = self.b.can_act();
        if let Some(op) = &mut self.b.op {
            if let Some(ev) = op.show(ui.ctx(), can_win) {
                self.b.on_event(ev);
            }
        }
        common::message_ui(ui.ctx(), "Technical details", &mut self.b.log);

        theme::page_header(ui, "Remove Bloat", "Removes apps in depth (all users, leftovers, reinstall blocks). Never touches Xbox, Game Pass, Store, Security or Edge.");
        common::admin_banner(ui);

        // ---- hero: how much of the list is gone
        let apps: Vec<&Target> = TARGETS.iter().filter(|t| is_app(t)).collect();
        let last = self.b.last_run();
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match &self.b.installed {
                    Some(list) => {
                        let gone = apps.iter().filter(|t| !list.iter().any(|n| n == t.name)).count();
                        let shown = theme::count_up(ui.ctx(), "bloat-hero", gone as f32).round() as usize;
                        theme::hero(ui, &format!("{shown} of {}", apps.len()), "apps removed", "Store apps from the Safe and Optional lists that are gone from this PC", theme::ACCENT);
                    }
                    None => {
                        ui.vertical(|ui| {
                            theme::hero(ui, "...", "apps removed", "Reading the installed apps", theme::TEXT_FAINT);
                            theme::sweep(ui, 260.0);
                        });
                    }
                }
                let n = |f: &dyn Fn(&Res) -> bool| last.as_ref().map(|items| items.iter().filter(|i| f(&i.res)).count());
                let show = |v: Option<usize>| v.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
                let failed = n(&|r| matches!(r, Res::Failed(_)));
                let dim = |on: bool, c: egui::Color32| if on { c } else { theme::TEXT_FAINT };
                stat_fit(ui, 30.0, &show(n(&|r| matches!(r, Res::Removed | Res::Applied))), "removed (last run)", dim(last.is_some(), theme::OK));
                stat_fit(ui, 22.0, &show(n(&|r| matches!(r, Res::AlreadyGone | Res::AlreadySet))), "already gone", dim(last.is_some(), theme::TEXT));
                stat_fit(ui, 22.0, &show(failed), "failed", dim(last.is_some(), if failed.unwrap_or(0) > 0 { theme::LOCKED } else { theme::TEXT }));
            });
        });
        ui.add_space(2.0);

        // ---- actions: tick apps in the list by hand, then one button removes them
        let mut remove = false;
        let mut clear = false;
        let n_sel = self.b.selected().len();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(can && n_sel > 0, theme::primary_button(&format!("  Remove selected ({n_sel})  ")))
                .on_hover_text("Removes the ticked apps. Shows the list to confirm first.")
                .on_disabled_hover_text(if common::is_admin() { "Tick the apps to remove in the list below." } else { "Run PC Tuner as administrator to remove apps." })
                .clicked()
            {
                remove = true;
            }
            if ui.add_enabled(!running && n_sel > 0, egui::Button::new("Clear selection")).on_hover_text("Untick everything.").clicked() {
                clear = true;
            }
            if ui.add_enabled(!running, egui::Button::new("Rescan")).on_hover_text("Read the installed apps again.").clicked() {
                self.b.installed = None;
            }
        });
        ui.add_space(2.0);

        // ---- targets, full width
        let mut toggled = None;
        let pick = &mut self.b.pick;
        let installed = &self.b.installed;
        {
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::section(ui, "Targets");
                    ui.label(RichText::new("tick the apps you want removed").size(12.0).color(theme::TEXT_DIM));
                });
                egui::ScrollArea::vertical().id_salt("bloat-list").auto_shrink([false, false]).show(ui, |ui| {
                    visible_checkboxes(ui);
                    for (i, t) in TARGETS.iter().enumerate() {
                        if i > 0 && t.optional && !TARGETS[i - 1].optional {
                            ui.add_space(6.0);
                            ui.label(RichText::new("OPTIONAL - EACH COSTS A CONVENIENCE").size(11.0).color(theme::TEXT_FAINT).strong());
                        }
                        let state = match (installed, is_app(t)) {
                            (Some(list), true) => Some(list.iter().any(|n| n == t.name)),
                            _ => None,
                        };
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add_space(8.0);
                                match state {
                                    Some(true) => theme::chip(ui, "installed", theme::JUNK),
                                    Some(false) => theme::chip(ui, "removed", theme::OK),
                                    None => {}
                                }
                                if t.optional {
                                    theme::chip(ui, "optional", theme::OPTIONAL);
                                }
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    if ui.checkbox(&mut pick[i], "").on_hover_text("Ticked = removed by Remove selected").changed() {
                                        toggled = Some(i);
                                    }
                                    let c = if !pick[i] || state == Some(false) { theme::DIM } else { theme::TEXT };
                                    let name = RichText::new(friendly(t.name)).color(c);
                                    ui.add(egui::Label::new(name).truncate()).on_hover_text(format!("{}\nYou lose: {}", t.name.trim_end_matches(['~', '.']), t.loses));
                                });
                            });
                        });
                    }
                });
            });
        }
        if let Some(i) = toggled {
            self.b.sync_paired(i);
        }

        // button clicks are handled last, when nothing borrows the tab any more
        if clear {
            self.b.pick = vec![false; TARGETS.len()];
            ui.ctx().request_repaint();
        }
        if remove {
            self.b.open_remove();
        }
    }
}
