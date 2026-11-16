//! Services tab - drawing only. The service lists, startup-type changes, history and page state live
//! in pctuner_core::services.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use pctuner_core::services::*;

/// Tick boxes need an outline and a square corner to read as check boxes on a card
/// (the theme's 8 px widget rounding turns a 14 px box into a circle).
fn outline_checks(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for s in [&mut w.inactive, &mut w.hovered, &mut w.active] {
        s.rounding = egui::Rounding::same(3.0);
    }
    w.inactive.bg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_DIM);
}

const ROW_H: f32 = 27.0;
const HEADER_H: f32 = 26.0;

pub struct ServicesTab {
    s: Services,
    /// The Backups window while it is open.
    backups: Option<crate::svcbackup::BackupsWindow>,
}

impl ServicesTab {
    pub fn new() -> Self {
        ServicesTab { s: Services::new(), backups: None }
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let rows = self.s.visible();
        let mut sort = self.s.sort;
        let mut asc = self.s.sort_asc;
        let mut toggled: Option<String> = None;
        ui.spacing_mut().item_spacing.y = 3.0;
        let avail = (ui.available_height() - HEADER_H - ui.spacing().item_spacing.y).max(80.0);

        TableBuilder::new(ui)
            .id_salt("svc-table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(0.0)
            .max_scroll_height(avail)
            .auto_shrink([false, false])
            .column(Column::initial(190.0).at_least(60.0).clip(true))
            .column(Column::initial(290.0).at_least(80.0).clip(true))
            .column(Column::initial(150.0).at_least(60.0).clip(true))
            .column(Column::initial(90.0).at_least(50.0).clip(true))
            .column(Column::initial(150.0).at_least(60.0).clip(true))
            .column(Column::remainder().at_least(60.0).clip(true))
            .header(HEADER_H, |mut header| {
                for (col, label) in [
                    (SortCol::Name, "Name"),
                    (SortCol::Display, "Display name"),
                    (SortCol::Startup, "Startup"),
                    (SortCol::Status, "Status"),
                    (SortCol::Note, "Note"),
                    (SortCol::Suggested, "Suggested"),
                ] {
                    header.col(|ui| {
                        let active = sort == col;
                        let txt = if active { format!("{label}  {}", if asc { "⏶" } else { "⏷" }) } else { label.to_string() };
                        let rt = egui::RichText::new(txt).size(12.5).strong().color(if active { theme::ACCENT } else { theme::TEXT_DIM });
                        let r = ui.add(egui::Label::new(rt).selectable(false).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clicked() {
                            if active {
                                asc = !asc;
                            } else {
                                sort = col;
                                asc = true;
                            }
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let s = &self.s.list[rows[row.index()]];
                    row.set_selected(self.s.selected.contains(&s.name));
                    // green: safe to change (Safe + Optional lists), with the suggested type in the last column
                    let suggested = self.s.suggest.get(&s.name_lc);
                    let c = match s.note {
                        NOTE_GAMING => Some(theme::GAMING),
                        NOTE_LOCKED => Some(theme::LOCKED),
                        _ if suggested.is_some() => Some(theme::OK),
                        _ if s.start == "Disabled" => Some(theme::DIM),
                        _ => None,
                    };
                    let start_c = if s.start == "Disabled" { Some(theme::DIM) } else { c };
                    let cell = |ui: &mut egui::Ui, t: &str, c: Option<egui::Color32>| {
                        let mut rt = egui::RichText::new(t);
                        if let Some(c) = c {
                            rt = rt.color(c);
                        }
                        ui.add(egui::Label::new(rt).selectable(false).truncate());
                    };
                    row.col(|ui| cell(ui, &s.name, c));
                    row.col(|ui| cell(ui, &s.display, c));
                    row.col(|ui| cell(ui, &s.start, start_c));
                    row.col(|ui| cell(ui, &s.state, c.or(if s.state == "Running" { None } else { Some(theme::TEXT_DIM) })));
                    row.col(|ui| cell(ui, s.note, c));
                    // the suggested startup type; "done" when the service is already set that way
                    let txt = match suggested {
                        Some(t) if *t == s.start => format!("{t} ✔"),
                        Some(t) => t.clone(),
                        None => String::new(),
                    };
                    row.col(|ui| cell(ui, &txt, suggested.map(|_| theme::OK)));
                    if row.response().clicked() {
                        toggled = Some(s.name.clone());
                    }
                });
            });

        self.s.sort = sort;
        self.s.sort_asc = asc;
        if let Some(n) = toggled {
            if !self.s.selected.remove(&n) {
                self.s.selected.insert(n);
            }
        }
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        if !self.s.show_history {
            return;
        }
        let mut close = false;
        let mut undo_sel = false;
        let mut clear = false;
        let can = common::is_admin() && !self.busy();
        let active = self.s.history.iter().filter(|h| !h.undone).count();
        theme::dim_backdrop(ctx, "svc-history");
        egui::Window::new("svc-history-window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width((ctx.screen_rect().width() - 80.0).min(930.0));
                theme::section(ui, "Services");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Change history").size(26.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        chip(ui, &format!("{active} can be undone"), if active > 0 { theme::ACCENT } else { theme::DIM });
                        chip(ui, &format!("{} recorded", self.s.history.len()), theme::DIM);
                    });
                });
                ui.label(egui::RichText::new("Tick the changes you want to take back, newest first.").color(theme::TEXT_DIM));
                ui.add_space(6.0);
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if self.s.history.is_empty() {
                        ui.label(egui::RichText::new("No changes recorded yet.").color(theme::TEXT_DIM));
                        return;
                    }
                    egui::ScrollArea::both().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                        egui::Grid::new("svc-history").striped(true).num_columns(7).min_row_height(ROW_H - 4.0).spacing([16.0, 4.0]).show(ui, |ui| {
                            // the tick boxes need an outline to be visible on the card
                            outline_checks(ui);
                            ui.label("");
                            for h in ["Time", "Name", "Display name", "From", "To", "State"] {
                                ui.label(egui::RichText::new(h).size(12.5).strong().color(theme::TEXT_DIM));
                            }
                            ui.end_row();
                            for i in (0..self.s.history.len()).rev() {
                                let h = &self.s.history[i];
                                let mut on = self.s.hist_sel.contains(&i);
                                if ui.add_enabled(!h.undone, egui::Checkbox::without_text(&mut on)).changed() {
                                    if on {
                                        self.s.hist_sel.insert(i);
                                    } else {
                                        self.s.hist_sel.remove(&i);
                                    }
                                }
                                let c = if h.undone { theme::DIM } else { theme::TEXT };
                                let short = |t: &str, max: usize| -> String {
                                    if t.chars().count() > max {
                                        format!("{}...", t.chars().take(max - 3).collect::<String>())
                                    } else {
                                        t.to_string()
                                    }
                                };
                                ui.colored_label(c, &h.time);
                                ui.colored_label(c, short(&h.name, 26));
                                ui.colored_label(c, short(&h.display_name, 34)).on_hover_text(&h.display_name);
                                ui.colored_label(c, &h.from);
                                ui.colored_label(c, &h.to);
                                ui.colored_label(if h.undone { theme::DIM } else { theme::ACCENT }, if h.undone { "Undone" } else { "Active" });
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let n = self.s.hist_sel.len();
                    let label = if n > 0 { format!("   Undo selected ({n})   ") } else { "   Undo selected   ".to_string() };
                    if ui.add_enabled(can && n > 0, theme::primary_button(&label)).clicked() {
                        undo_sel = true;
                    }
                    if ui.add_enabled(!self.busy(), egui::Button::new("Clear undone entries")).on_hover_text("Removes already-undone rows from this list. Changes nothing on the PC.").clicked() {
                        clear = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    if self.busy() {
                        ui.spinner();
                    }
                    if !common::is_admin() {
                        ui.colored_label(theme::LOCKED, "View only - start PC Tuner as administrator.");
                    }
                });
            });
        if undo_sel {
            let idx: Vec<usize> = self.s.hist_sel.drain().collect();
            self.s.request_undo(idx);
        }
        if clear {
            self.s.clear_undone_history();
        }
        if close {
            self.s.show_history = false;
        }
    }

    /// Runs the open operation window and reacts to its buttons.
    fn op_window(&mut self, ctx: &egui::Context, can: bool) {
        let Some(op) = self.s.op.as_mut() else { return };
        if let Some(ev) = op.show(ctx, can) {
            self.s.on_event(ev);
        }
    }
}

/// Same look as theme::chip, but painted at its natural size (a framed chip stretches to the
/// row height when it sits in a tall row such as the hero).
fn chip(ui: &mut egui::Ui, text: &str, c: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.5), c);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(20.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 99.0, theme::tint(c, 30));
    ui.painter().galley(rect.min + egui::vec2(10.0, 4.0), galley, c);
}

/// Small coloured dot + caption (table legend).
fn legend(ui: &mut egui::Ui, c: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, c);
        ui.label(egui::RichText::new(text).size(12.0).color(theme::TEXT_DIM));
    });
}

impl Tab for ServicesTab {
    fn title(&self) -> &'static str {
        "Services"
    }

    /// A service change is running: closing PC Tuner waits for it.
    fn busy(&self) -> bool {
        self.s.busy() || self.backups.as_ref().map(|b| b.busy()).unwrap_or(false)
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        // background work
        self.s.pump();

        let ctx = ui.ctx().clone();
        let can = common::is_admin() && !self.busy();
        self.op_window(&ctx, can);
        if let Some(b) = &mut self.backups {
            let (keep, changed) = b.show(&ctx);
            if !keep {
                self.backups = None;
            }
            if changed {
                self.s.start_load();
            }
        }
        if self.s.op.is_none() && self.backups.is_none() {
            self.history_window(&ctx);
        }

        // header, with the two view-only buttons on the title row
        let top = ui.cursor().min;
        let right = ui.max_rect().right();
        theme::page_header(ui, "Services", "Change startup types with full undo. Gaming dependencies warn first; core Windows services are locked.");
        {
            let r = egui::Rect::from_min_max(egui::pos2(right - 380.0, top.y), egui::pos2(right, top.y + 32.0));
            let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)));
            if bar.add_enabled(self.s.load_job.is_none(), egui::Button::new("Refresh")).on_hover_text("Read the service list again.").clicked() {
                self.s.start_load();
            }
            if bar.button("Backups").on_hover_text("Save or restore the startup type of every service.").clicked() {
                self.backups = Some(crate::svcbackup::BackupsWindow::new());
            }
            if bar.button("History").on_hover_text("Every change made here, with undo per entry.").clicked() {
                self.s.show_history = true;
            }
            if self.s.load_job.is_some() {
                bar.spinner();
            }
        }
        common::admin_banner(ui);

        // hero
        let running = self.s.list.iter().filter(|s| s.state == "Running").count();
        let disabled = self.s.list.iter().filter(|s| s.start == "Disabled").count();
        let undoable = self.s.history.iter().filter(|h| !h.undone).count();
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let shown = theme::count_up(ui.ctx(), "svc-running", running as f32).round() as usize;
                theme::hero(ui, &shown.to_string(), "running", "services active right now", theme::ACCENT);
                ui.add_space(30.0);
                theme::stat(ui, &disabled.to_string(), "disabled", theme::DIM);
                ui.add_space(22.0);
                theme::stat(ui, &self.s.list.len().to_string(), "total", theme::TEXT);
                ui.add_space(22.0);
                theme::stat(ui, &undoable.to_string(), "changes that can be undone", if undoable > 0 { theme::TEXT } else { theme::DIM });
                // safe-to-change services (green) that are not yet at their suggested type
                let open = self.s.list.iter().filter(|s| self.s.suggest.get(&s.name_lc).is_some_and(|t| *t != s.start)).count();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.s.list.is_empty() {
                        chip(ui, "Checking...", theme::DIM);
                    } else if open == 0 {
                        chip(ui, "Every green service is at its suggested type", theme::OK);
                    } else {
                        chip(ui, &format!("{open} green services not at their suggested type"), theme::OK);
                    }
                });
            });
        });

        // actions for the selection (laid out before the table so the table gets the rest)
        egui::TopBottomPanel::bottom("svc-bottom").frame(egui::Frame::none()).show_separator_line(false).show_inside(ui, |ui| {
            ui.add_space(8.0);
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // the actions only act on selected rows that are shown (see selected_visible)
                    let vis = self.s.selected_visible().len();
                    let total = self.s.selected.len();
                    let has = vis > 0;
                    if total == 0 {
                        chip(ui, "Click rows to select", theme::DIM);
                    } else if vis == total {
                        chip(ui, &format!("{vis} selected"), theme::ACCENT);
                    } else if vis > 0 {
                        chip(ui, &format!("{vis} selected ({} hidden by filter)", total - vis), theme::ACCENT);
                    } else {
                        chip(ui, &format!("{total} selected, all hidden by filter"), theme::DIM);
                    }
                    if ui.add_enabled(can && has, theme::danger_button("Disable")).on_hover_text("Set selected services to Disabled (they cannot start at all).").clicked() {
                        self.s.request_change("Disabled");
                    }
                    if ui.add_enabled(can && has, egui::Button::new("Manual")).on_hover_text("Set selected services to Manual (start only when something needs them).").clicked() {
                        self.s.request_change("Manual");
                    }
                    if ui.add_enabled(can && has, egui::Button::new("Automatic")).on_hover_text("Set selected services to start automatically at boot.").clicked() {
                        self.s.request_change("Automatic");
                    }
                    if total > 0 && ui.button("Clear").on_hover_text("Clear the selection.").clicked() {
                        self.s.clear_selection();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let has_undo = self.s.history.iter().any(|h| !h.undone);
                        if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).on_hover_text("Revert the most recent batch of changes.").clicked() {
                            self.s.request_undo_last();
                        }
                        if self.busy() {
                            ui.spinner();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    legend(ui, theme::OK, "Safe to change (see Suggested)");
                    legend(ui, theme::GAMING, "Gaming dependency");
                    legend(ui, theme::LOCKED, "Locked core service");
                    legend(ui, theme::DIM, "Disabled");
                    ui.add_space(10.0);
                    ui.scope(|ui| {
                        outline_checks(ui);
                        ui.checkbox(&mut self.s.stop_on_disable, "Stop when disabling").on_hover_text("Also stop a running service right away when it is set to Disabled.");
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(e) = &self.s.load_error {
                            ui.add(egui::Label::new(egui::RichText::new(format!("Could not load services: {e}")).size(12.0).color(theme::LOCKED)).truncate());
                        } else if !self.s.last_action.is_empty() {
                            ui.add(egui::Label::new(egui::RichText::new(&self.s.last_action).size(12.0).color(theme::TEXT_DIM)).truncate());
                        }
                    });
                });
            });
        });

        // toolbar + table
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 680.0).clamp(130.0, 260.0);
                ui.add(egui::TextEdit::singleline(&mut self.s.search).hint_text("Search services").desired_width(w));
                ui.scope(|ui| {
                    outline_checks(ui);
                    ui.checkbox(&mut self.s.running_only, "Running only");
                    ui.checkbox(&mut self.s.hide_locked, "Hide locked").on_hover_text("Hide core Windows services (they cannot be changed).");
                });
            });
            ui.add_space(2.0);
            if self.s.list.is_empty() && self.s.load_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Loading services...").color(theme::TEXT_DIM));
                });
                ui.allocate_space(egui::vec2(0.0, ui.available_height()));
            } else {
                self.table(ui);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Job;

    #[test]
    fn services_busy_through_tab_trait() {
        let mut t = ServicesTab { s: Services::with_history(vec![]), backups: None };
        assert!(!(&t as &dyn Tab).busy());
        // a job that never finishes (the parked thread ends with the test process)
        t.s.job = Some(Job::spawn(|| -> JobResult {
            loop {
                std::thread::park();
            }
        }));
        assert!((&t as &dyn Tab).busy());
    }
}
