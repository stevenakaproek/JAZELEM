//! Group Policy tab - drawing only. The policy list, Registry.pol handling, apply / undo and the page
//! state live in pctuner_core::gpolicy.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::gpolicy::*;

pub struct GpolicyTab {
    g: Gpolicy,
}

impl GpolicyTab {
    pub fn new() -> Self {
        GpolicyTab { g: Gpolicy::new() }
    }

    /// One policy row. Click selects it (green); right-click opens Enable / Disable / Not configured.
    fn policy_row(&mut self, ui: &mut egui::Ui, i: usize, can_edit: bool) -> RowAct {
        let p = &POLICIES[i];
        let cur = self.g.states.get(i).cloned().unwrap_or(St::NotConfigured);
        let tgt = self.g.target(i);
        let changed = self.g.want[i].is_some();
        let picked = self.g.picked.contains(&i);
        let rec = p.rec_state();
        let mut act = RowAct::None;
        // the row's click area is registered before its content (from last frame's size), so the
        // drop-downs inside still get their own clicks
        let id = ui.id().with(("gp-row", i));
        if let Some(r) = ui.ctx().data(|d| d.get_temp::<egui::Rect>(id)) {
            let resp = ui.interact(r, id, egui::Sense::click());
            if can_edit && resp.clicked() {
                act = RowAct::Toggle;
            }
            if can_edit {
                resp.context_menu(|ui| {
                    ui.label(RichText::new(if picked && self.g.picked.len() > 1 { format!("{} selected policies", self.g.picked.len()) } else { p.name.to_string() }).size(12.0).color(theme::TEXT_DIM));
                    ui.separator();
                    for (lbl, s) in [("Enable", St::Enabled(vec![])), ("Disable", St::Disabled), ("Not configured", St::NotConfigured)] {
                        if ui.button(lbl).clicked() {
                            act = RowAct::Set(s);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("Keep as it is (no change)").clicked() {
                        act = RowAct::Reset;
                        ui.close_menu();
                    }
                });
            }
            if can_edit && resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        let frame = egui::Frame::none()
            .fill(if picked { theme::tint(theme::OK, 34) } else if changed { theme::tint(theme::ACCENT, 16) } else { egui::Color32::TRANSPARENT })
            .stroke(egui::Stroke::new(1.0_f32, if picked { theme::tint(theme::OK, 200) } else { egui::Color32::TRANSPARENT }))
            .rounding(6.0)
            .inner_margin(egui::Margin::symmetric(8.0, 5.0));
        let shown = frame.show(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(p.name).strong().color(if picked { theme::OK } else { theme::TEXT })).truncate())
                    .on_hover_text(format!("gpedit: {}\n{} > \"{}\"", if p.scope == Scope::User { "User Configuration" } else { "Computer Configuration" }, p.path, p.title));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Enabled shows its drop-downs (the values it is enabled with)
                    let mut new: Option<St> = None;
                    ui.add_enabled_ui(can_edit, |ui| {
                        if let St::Enabled(vals) = &tgt {
                            let vals = if vals.len() == p.opts.len() { vals.clone() } else { p.default_opts() };
                            for (k, o) in p.opts.iter().enumerate().rev() {
                                let sel = o.choices.iter().find(|c| c.value == vals[k]).map(|c| c.label).unwrap_or("?");
                                egui::ComboBox::from_id_salt(("gp-opt", i, k)).selected_text(sel).width(150.0).show_ui(ui, |ui| {
                                    for c in o.choices {
                                        if ui.selectable_label(c.value == vals[k], c.label).clicked() {
                                            let mut v = vals.clone();
                                            v[k] = c.value;
                                            new = Some(St::Enabled(v));
                                        }
                                    }
                                });
                                ui.label(RichText::new(o.label).size(12.0).color(theme::TEXT_DIM));
                            }
                        }
                    });
                    if changed {
                        if ui.small_button("Reset").on_hover_text("Keep it as it is now.").clicked() {
                            self.g.want[i] = None;
                        }
                        theme::chip(ui, &p.label(&tgt), theme::ACCENT);
                        ui.label(RichText::new(format!("{} ->", p.label(&cur))).size(12.0).color(theme::TEXT_DIM));
                    } else {
                        let c = match cur {
                            St::NotConfigured => theme::TEXT_DIM,
                            St::Other => theme::LOCKED,
                            _ => theme::OK,
                        };
                        theme::chip(ui, &p.label(&cur), c);
                        let in_reg = self.g.reg.get(i).cloned().unwrap_or(St::NotConfigured);
                        if cur == St::NotConfigured && in_reg != St::NotConfigured {
                            theme::chip_response(ui, &format!("registry: {}", p.label(&in_reg)), theme::OPTIONAL)
                                .on_hover_text("Not set in Group Policy, but this value is already in the registry (set by a tweak or another tool). Setting it here makes it permanent and visible in gpedit.");
                        } else if !self.g.live.get(i).copied().unwrap_or(true) && cur != St::NotConfigured {
                            theme::chip_response(ui, "not applied yet", theme::JUNK)
                                .on_hover_text("Saved in Group Policy, but Windows has not applied it yet. Restart, or Apply again.");
                        }
                    }
                    if let Some(r) = &rec {
                        if *r == tgt {
                            theme::chip(ui, "recommended", theme::ACCENT);
                        } else {
                            theme::chip(ui, &format!("recommended: {}", p.label(r)), theme::TEXT_FAINT);
                        }
                    } else {
                        theme::chip(ui, "your choice", theme::OPTIONAL);
                    }
                    if let Some(s) = new {
                        self.g.want[i] = if s == cur { None } else { Some(s) };
                    }
                });
            });
            ui.label(RichText::new(p.advice).size(12.0).color(theme::TEXT_DIM));
        });
        ui.ctx().data_mut(|d| d.insert_temp(id, shown.response.rect));
        act
    }
}

impl Tab for GpolicyTab {
    fn title(&self) -> &'static str {
        "Group Policy"
    }

    fn busy(&self) -> bool {
        self.g.job.is_some()
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.g.pump();
        let busy = self.g.busy_job();
        let can = self.g.can_act();

        if let Some(w) = self.g.op.as_mut() {
            if let Some(ev) = w.show(ui.ctx(), can) {
                self.g.on_event(ev);
            }
        }
        if busy {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }

        theme::page_header(ui, "Group Policy", "Gaming and privacy policies in plain English - no gpedit needed. Every change can be undone.");
        common::admin_banner(ui);
        if !self.g.gpo {
            theme::banner(ui, theme::OPTIONAL, &format!(
                "Windows {} has no Group Policy editor: policies are written straight to the registry. A few protected ones (Widgets) may be refused - remove Widgets in Remove Bloat instead.",
                self.g.edition
            ));
        }
        if let Some(e) = &self.g.load_error {
            theme::banner(ui, theme::LOCKED, e);
        }

        // hero
        let loaded = !self.g.states.is_empty();
        let recs: Vec<usize> = (0..POLICIES.len()).filter(|&i| POLICIES[i].rec_state().is_some()).collect();
        let at_rec = recs.iter().filter(|&&i| self.g.states.get(i) == POLICIES[i].rec_state().as_ref()).count();
        let n_changes = self.g.changes().len();
        let has_undo = self.g.history.iter().any(|b| !b.undone);
        let mut select = false;
        let mut apply = false;
        let mut discard = false;
        let mut undo = false;
        let mut refresh = false;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if loaded {
                    theme::hero(ui, &format!("{at_rec} of {}", recs.len()), "recommended", "policies already set the recommended way", if at_rec == recs.len() { theme::OK } else { theme::ACCENT });
                } else {
                    theme::hero(ui, "...", "recommended", "Reading the policies", theme::TEXT_FAINT);
                }
                ui.add_space(26.0);
                theme::stat(ui, &n_changes.to_string(), "changes selected", if n_changes > 0 { theme::ACCENT } else { theme::TEXT });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(can && n_changes > 0, theme::primary_button(&format!("  Apply changes ({n_changes})  "))).on_hover_text("Shows every change first.").clicked() {
                        apply = true;
                    }
                    if ui.add_enabled(loaded && !busy, egui::Button::new("Select recommended")).on_hover_text("Selects the recommended setting for every policy that has one. Review, change or reset rows, then Apply.").clicked() {
                        select = true;
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.g.search).hint_text("Search policies").desired_width(220.0));
            if ui.add_enabled(n_changes > 0 && !busy, egui::Button::new("Discard changes")).clicked() {
                discard = true;
            }
            if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).on_hover_text("Shows what goes back before anything changes.").clicked() {
                undo = true;
            }
            if ui.add_enabled(!busy && self.g.loader.is_none(), egui::Button::new("Refresh")).clicked() {
                refresh = true;
            }
            if self.g.gpo && ui.button("Open gpedit").on_hover_text("Windows' own Group Policy editor (everything set here shows there).").clicked() {
                let _ = std::process::Command::new("mmc.exe").arg("gpedit.msc").spawn();
            }
            if !self.g.picked.is_empty() && ui.button(format!("Clear selection ({})", self.g.picked.len())).clicked() {
                self.g.picked.clear();
            }
            if busy || self.g.loader.is_some() {
                ui.spinner();
            }
            if self.g.picked.is_empty() && self.g.last_action.is_empty() {
                ui.label(RichText::new("Click policies to select them, right-click to enable or disable.").size(12.0).color(theme::TEXT_FAINT));
            }
            if !self.g.last_action.is_empty() {
                ui.label(RichText::new(&self.g.last_action).size(12.0).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(4.0);

        // the list, by group
        let q = self.g.search.trim().to_lowercase();
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("gp-list").auto_shrink([false, false]).show(ui, |ui| {
                if !loaded {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Reading the policies...").color(theme::TEXT_DIM));
                    });
                    return;
                }
                for g in GROUPS {
                    let rows: Vec<usize> = (0..POLICIES.len())
                        .filter(|&i| POLICIES[i].group == *g)
                        .filter(|&i| {
                            let p = &POLICIES[i];
                            q.is_empty() || [p.name, p.title, p.advice].iter().any(|s| s.to_lowercase().contains(&q))
                        })
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    ui.add_space(6.0);
                    theme::section(ui, g);
                    for i in rows {
                        let act = self.policy_row(ui, i, !busy);
                        self.g.row_action(i, act);
                    }
                }
            });
        });

        if select {
            self.g.select_recommended();
        }
        if discard {
            self.g.discard();
        }
        if apply {
            self.g.open_apply();
        }
        if undo {
            self.g.open_undo();
        }
        if refresh {
            self.g.reload();
        }
    }
}
