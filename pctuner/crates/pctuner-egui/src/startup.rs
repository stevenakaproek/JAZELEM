//! Startup Cleaner tab - drawing only. The scanners, switch operations, history and page state live
//! in pctuner_core::startup.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use pctuner_core::startup::*;

pub struct StartupTab {
    s: Startup,
}

impl StartupTab {
    pub fn new() -> Self {
        StartupTab { s: Startup::new() }
    }

    /// Draw the operation window and act on its buttons.
    fn op_window(&mut self, ctx: &egui::Context) {
        let can = self.s.can_act();
        let Some(op) = self.s.op.as_mut() else { return };
        let Some(ev) = op.win.show(ctx, can) else { return };
        self.s.on_event(ev, can);
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        // hidden while an operation window is open (it comes back afterwards)
        if !self.s.show_history || self.s.op.is_some() {
            return;
        }
        enum H {
            UndoSel,
            UndoAll,
            Clear,
        }
        let mut act: Option<H> = None;
        let mut close = false;
        let busy = self.busy();
        let active = self.s.history.iter().filter(|h| !h.undone).count();
        let any_active = active > 0;
        let mut clicked: Option<usize> = None;
        theme::dim_backdrop(ctx, "startup-history");
        egui::Window::new("startup-history-window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width((ctx.screen_rect().width() - 80.0).min(930.0));
                theme::section(ui, "Startup Cleaner");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Change history").size(26.0).strong().color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::chip(ui, &format!("{active} can be undone"), if any_active { theme::ACCENT } else { theme::DIM });
                        theme::chip(ui, &format!("{} recorded", self.s.history.len()), theme::DIM);
                    });
                });
                ui.label(RichText::new("Click the changes you want to take back, newest first.").color(theme::TEXT_DIM));
                ui.add_space(6.0);
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if self.s.history.is_empty() {
                        ui.label(RichText::new("No changes recorded yet.").color(theme::TEXT_DIM));
                        return;
                    }
                    let order: Vec<usize> = (0..self.s.history.len()).rev().collect();
                    let hist = &self.s.history;
                    let sel = &self.s.hist_sel;
                    let max_h = (ctx.screen_rect().height() - 330.0).clamp(140.0, 360.0);
                    TableBuilder::new(ui)
                        .id_salt("startup-history-table")
                        .striped(true)
                        .resizable(false)
                        .sense(egui::Sense::click())
                        .max_scroll_height(max_h)
                        .auto_shrink([false, true])
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::exact(156.0))
                        .column(Column::exact(204.0).clip(true))
                        .column(Column::exact(130.0).clip(true))
                        .column(Column::exact(172.0).clip(true))
                        .column(Column::exact(96.0))
                        .column(Column::remainder().at_least(56.0))
                        .header(24.0, |mut h| {
                            for t in ["Time", "Name", "Type", "Location", "Change", "State"] {
                                h.col(|ui| head(ui, t));
                            }
                        })
                        .body(|body| {
                            body.rows(24.0, order.len(), |mut row| {
                                let i = order[row.index()];
                                let h = &hist[i];
                                row.set_selected(!h.undone && sel.contains(&i));
                                let (c, c2) = if h.undone { (theme::DIM, theme::DIM) } else { (theme::TEXT, theme::TEXT_DIM) };
                                row.col(|ui| cell(ui, &h.time, c2));
                                row.col(|ui| cell(ui, &h.name, c));
                                row.col(|ui| cell(ui, h.kind.label(), c2));
                                row.col(|ui| cell(ui, &h.location, c2));
                                row.col(|ui| cell(ui, if h.to { "Switched on" } else { "Switched off" }, c2));
                                row.col(|ui| cell(ui, if h.undone { "Undone" } else { "Active" }, if h.undone { theme::DIM } else { theme::ACCENT }));
                                if row.response().clicked() {
                                    clicked = Some(i);
                                }
                            });
                        });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    // only active rows count (undone ones can't be selected, this is a safety net)
                    let n = self.s.hist_sel.iter().filter(|&&i| self.s.history.get(i).is_some_and(|h| !h.undone)).count();
                    let label = if n > 0 { format!("   Undo selected ({n})   ") } else { "   Undo selected   ".to_string() };
                    if ui.add_enabled(!busy && n > 0, theme::primary_button(&label)).clicked() {
                        act = Some(H::UndoSel);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Clear undone entries"))
                        .on_hover_text("Removes already-undone rows from this list. Changes nothing on the PC.")
                        .clicked()
                    {
                        act = Some(H::Clear);
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    if busy {
                        ui.spinner();
                    } else if !self.s.hist_status.is_empty() {
                        ui.label(RichText::new(&self.s.hist_status).color(theme::TEXT_DIM));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!busy && any_active, theme::danger_button("Undo everything"))
                            .on_hover_text("Takes back every change still active in this list. You see the full list before anything happens.")
                            .clicked()
                        {
                            act = Some(H::UndoAll);
                        }
                    });
                });
            });
        if let Some(i) = clicked {
            self.s.hist_toggle(i);
        }
        if close {
            self.s.close_history();
        }
        match act {
            Some(H::UndoSel) => self.s.hist_undo_selected(),
            Some(H::UndoAll) => self.s.hist_undo_all(),
            Some(H::Clear) => self.s.hist_clear(),
            None => {}
        }
    }
}

fn head(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).strong().color(theme::TEXT_DIM));
}

fn cell(ui: &mut egui::Ui, text: &str, c: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(c)).truncate().selectable(false));
}

impl Tab for StartupTab {
    fn title(&self) -> &'static str {
        "Startup Cleaner"
    }

    /// A change or an Explorer restart is running: closing PC Tuner waits for it.
    fn busy(&self) -> bool {
        self.s.busy()
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.s.pump();

        self.history_window(&ctx);
        self.op_window(&ctx);

        let busy = self.busy();
        let ready =self.s.scanned && !(self.s.job.is_some() && self.s.entries.is_empty());

        // header: title on the left, page-level tools on the right
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui.add_enabled(!busy, egui::Button::new("Restart Explorer")).on_hover_text("Needed for Explorer add-on changes to take effect. Shows a preview first.").clicked() {
                self.s.op = Some(explorer_op());
            }
            if ui.button("History...").on_hover_text("Every change this tool made, with undo.").clicked() {
                self.s.show_history = true;
            }
            if ui.add_enabled(!busy, egui::Button::new("Rescan")).clicked() {
                self.s.rescan();
            }
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                theme::page_header(ui, "Startup Cleaner", "Everything that starts with Windows. Items are switched off, never deleted - every change can be undone.");
            });
        });
        common::admin_banner(ui);

        // hero: junk still on
        let shown: Vec<&Entry> = self.s.entries.iter().filter(|e| e.category != "Windows").collect();
        let on = shown.iter().filter(|e| e.enabled).count();
        let junk = self.s.entries.iter().filter(|e| e.enabled && e.category == "Junk").count();
        let optional = self.s.entries.iter().filter(|e| e.enabled && e.category == "Optional").count();
        let pending = self.s.history.iter().filter(|h| !h.undone).count();
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if ready {
                    let n = theme::count_up(&ctx, "startup-junk", junk as f32).round() as usize;
                    let (c, caption) =
                        if junk == 0 { (theme::ACCENT, "Nothing left to clean.") } else { (theme::JUNK, "Safe to switch off - nothing is deleted.") };
                    theme::hero(ui, &n.to_string(), "junk still on", caption, c);
                } else {
                    theme::hero(ui, "-", "junk still on", "Scanning startup locations...", theme::TEXT_FAINT);
                }
                ui.add_space(26.0);
                let val = |n: usize| if ready { n.to_string() } else { "-".to_string() };
                theme::stat(ui, &val(on), "items switched on", theme::TEXT);
                ui.add_space(16.0);
                theme::stat(ui, &val(optional), "optional still on", if optional == 0 || !ready { theme::TEXT } else { theme::OPTIONAL });
                ui.add_space(16.0);
                theme::stat(ui, &pending.to_string(), "changes can be undone", theme::TEXT);
            });
        });
        for w in self.s.history_warning.iter().chain(&self.s.warnings) {
            theme::banner(ui, theme::LOCKED, w);
        }

        // list
        let visible = self.s.visible();
        let sel: Vec<Entry> = visible.iter().map(|&i| &self.s.entries[i]).filter(|e| self.s.selected.contains(&e.id)).cloned().collect();
        let mut clicked: Option<String> = None;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.s.search).hint_text("Search name, publisher, command").desired_width(230.0));
                egui::ComboBox::from_id_salt("startup-kind")
                    .width(170.0)
                    .selected_text(self.s.kind_filter.map_or("All types", |k| k.label()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.s.kind_filter, None, "All types");
                        for k in KINDS {
                            ui.selectable_value(&mut self.s.kind_filter, Some(k), k.label());
                        }
                    });
                ui.checkbox(&mut self.s.enabled_only, "Enabled only");
                ui.checkbox(&mut self.s.show_windows, "Show all Windows tasks");
                if busy {
                    ui.spinner();
                    ui.label(RichText::new(if self.s.job.is_some() { "Working / scanning..." } else { "Restarting Explorer..." }).color(theme::TEXT_DIM));
                } else if !self.s.last_action.is_empty() {
                    ui.label(RichText::new(&self.s.last_action).color(theme::TEXT_DIM));
                }
            });
            ui.horizontal_wrapped(|ui| {
                let has_sel = !sel.is_empty() && !busy;
                let n = sel.iter().filter(|e| e.enabled).count();
                if ui
                    .add_enabled(has_sel && n > 0, theme::primary_button(&format!("Clean selected ({n})")))
                    .on_hover_text("Click rows in the list to select them, then switch them off here (nothing is deleted). Shows a preview first; one click undoes it.")
                    .clicked()
                {
                    self.s.op = Some(clean_sel_op(&sel));
                }
                ui.separator();
                if ui.add_enabled(has_sel, egui::Button::new("Switch on")).on_hover_text("Switch the selected items back on. Shows a preview first.").clicked() {
                    self.s.switch(sel.clone(), true);
                }
                if ui.add_enabled(!busy && pending > 0, egui::Button::new("Undo last change")).on_hover_text("Shows what goes back before anything changes.").clicked() {
                    self.s.undo_last();
                }
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{} selected", sel.len())).color(theme::TEXT_DIM));
                if ui.small_button("Select all shown").clicked() {
                    for &i in &visible {
                        self.s.selected.insert(self.s.entries[i].id.clone());
                    }
                }
                if ui.small_button("Clear selection").clicked() {
                    self.s.selected.clear();
                }
                ui.add_space(6.0);
                theme::chip(ui, "Junk", theme::JUNK);
                theme::chip(ui, "Optional", theme::OPTIONAL);
                theme::chip(ui, "Keep", theme::GAMING);
                theme::chip(ui, "Switched off", theme::DIM);
            });
            ui.add_space(2.0);

            let entries = &self.s.entries;
            let selected = &self.s.selected;
            TableBuilder::new(ui)
                .id_salt("startup-table")
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .min_scrolled_height(0.0)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(34.0))
                .column(Column::initial(180.0).at_least(60.0).clip(true))
                .column(Column::initial(120.0).at_least(50.0).clip(true))
                .column(Column::initial(115.0).at_least(50.0).clip(true))
                .column(Column::initial(105.0).at_least(50.0).clip(true))
                .column(Column::initial(230.0).at_least(60.0).clip(true))
                .column(Column::remainder().at_least(60.0).clip(true))
                .header(24.0, |mut h| {
                    for t in ["On", "Name", "Type", "Location", "Publisher", "Advice", "Command"] {
                        h.col(|ui| head(ui, t));
                    }
                })
                .body(|body| {
                    body.rows(22.0, visible.len(), |mut row| {
                        let e = &entries[visible[row.index()]];
                        row.set_selected(selected.contains(&e.id));
                        // name / advice carry the category colour; the rest stays quiet
                        let (c, c2) = if !e.enabled {
                            (theme::DIM, theme::TEXT_FAINT)
                        } else {
                            let c = match e.category {
                                "Junk" => theme::JUNK,
                                "Optional" => theme::OPTIONAL,
                                "Keep" => theme::GAMING,
                                _ => theme::TEXT,
                            };
                            (c, theme::TEXT_DIM)
                        };
                        row.col(|ui| cell(ui, if e.enabled { "On" } else { "Off" }, c));
                        row.col(|ui| cell(ui, &e.name, c));
                        row.col(|ui| cell(ui, e.kind.label(), c2));
                        row.col(|ui| cell(ui, &e.location, c2));
                        row.col(|ui| cell(ui, &e.publisher, c2));
                        row.col(|ui| cell(ui, e.advice, c));
                        row.col(|ui| cell(ui, &e.command, c2));
                        if row.response().clicked() {
                            clicked = Some(e.id.clone());
                        }
                    });
                });
        });
        if let Some(id) = clicked {
            if !self.s.selected.remove(&id) {
                self.s.selected.insert(id);
            }
        }
    }
}
