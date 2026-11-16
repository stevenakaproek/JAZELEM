//! Drawing of the shared "operation window" (preview -> working -> result).
//! The data model (OpWindow, OpItem, Stage, OpEvent) lives in pctuner_core::opwin and is re-exported here.

use crate::theme;
use eframe::egui::{self, RichText};
pub use pctuner_core::opwin::*;

/// Draws an `OpWindow` with egui. Bring it into scope to call `window.show(ctx, can)`.
pub trait OpWindowUi {
    /// Draw the window. `can` = the user may press Go / Undo / extras (admin and not busy).
    fn show(&mut self, ctx: &egui::Context, can: bool) -> Option<OpEvent>;
}

impl OpWindowUi for OpWindow {
    fn show(&mut self, ctx: &egui::Context, can: bool) -> Option<OpEvent> {
        let now = ctx.input(|i| i.time);
        let mut ev: Option<OpEvent> = None;
        theme::dim_backdrop(ctx, "opwin");

        egui::Window::new("operation-window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                const W: f32 = 580.0;
                ui.set_width(W);
                theme::section(ui, &self.kicker);
                let n = self.items.len();
                match self.stage {
                    Stage::Preview => {
                        ui.label(RichText::new(format!("Ready to {} {n} {}", self.verb, plural(n, &self.noun))).size(26.0).strong());
                        if !self.preview_text.is_empty() {
                            ui.label(RichText::new(&self.preview_text).color(theme::TEXT_DIM));
                        }
                        ui.add_space(6.0);
                        if let Some(w) = &self.warning {
                            theme::banner(ui, theme::GAMING, w);
                            ui.add_space(4.0);
                        }
                        let groups = group_counts(&self.items);
                        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                            ui.set_width(W - 12.0);
                            let single = groups.len() == 1 && n <= 8;
                            for (label, total, _) in &groups {
                                let body = |ui: &mut egui::Ui| {
                                    for it in self.items.iter().filter(|i| &i.group == label) {
                                        ui.horizontal(|ui| {
                                            ui.colored_label(if it.optional { theme::OPTIONAL } else { theme::ACCENT }, "•");
                                            ui.label(&it.name);
                                            if !it.detail.is_empty() {
                                                ui.label(RichText::new(&it.detail).color(theme::TEXT_DIM));
                                            }
                                            if it.optional {
                                                ui.label(RichText::new("(optional)").color(theme::TEXT_FAINT));
                                            }
                                        });
                                    }
                                };
                                if single {
                                    // few items of one kind: just list them
                                    body(ui);
                                } else {
                                    // show the names straight away unless the list is very long
                                    egui::CollapsingHeader::new(RichText::new(format!("{label}    {total}")).strong()).id_salt(label).default_open(groups.len() == 1 || n <= 30).show(ui, body);
                                }
                            }
                        });
                        ui.add_space(6.0);
                        if !self.reassure.is_empty() {
                            ui.label(RichText::new(&self.reassure).color(theme::TEXT_DIM));
                        }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(can, theme::primary_button(&format!("   {}   ", self.go_label))).clicked() {
                                ev = Some(OpEvent::Go);
                            }
                            if ui.button("Cancel").clicked() {
                                ev = Some(OpEvent::Close);
                            }
                            if !crate::common::is_admin() {
                                ui.colored_label(theme::LOCKED, "View only - start PC Tuner as administrator.");
                            }
                        });
                    }
                    Stage::Running => {
                        // demo screenshots show this stage without any work behind it
                        if std::env::var_os("PCTUNER_DEMO").is_none() {
                            mark_running(now);
                        }
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(&self.running_text).size(26.0).strong());
                        });
                        let done = self.items.iter().filter(|i| i.result.is_some()).count();
                        if self.show_steps {
                            ui.add_space(4.0);
                            theme::progress(ui, self.progress.unwrap_or(done as f32 / n.max(1) as f32), W);
                            ui.add_space(6.0);
                            let mut current_marked = false;
                            egui::ScrollArea::vertical().id_salt("op-steps").max_height(260.0).show(ui, |ui| {
                            ui.set_width(W - 12.0);
                            for it in &self.items {
                                ui.horizontal(|ui| {
                                    match &it.result {
                                        Some(Ok(_)) => {
                                            ui.colored_label(theme::OK, RichText::new("✔").strong());
                                            ui.label(&it.name);
                                        }
                                        Some(Err(_)) => {
                                            ui.colored_label(theme::LOCKED, RichText::new("✖").strong());
                                            ui.label(&it.name);
                                        }
                                        None if !current_marked => {
                                            current_marked = true;
                                            ui.add(egui::Spinner::new().size(13.0));
                                            ui.label(RichText::new(&it.name).strong());
                                        }
                                        None => {
                                            ui.colored_label(theme::TEXT_FAINT, "•");
                                            ui.label(RichText::new(&it.name).color(theme::TEXT_FAINT));
                                        }
                                    }
                                    if !it.detail.is_empty() {
                                        ui.label(RichText::new(&it.detail).color(theme::TEXT_FAINT).small());
                                    }
                                });
                            }
                            });
                        } else {
                            ui.label(RichText::new(format!("{n} {}", plural(n, &self.noun))).color(theme::TEXT_DIM));
                            ui.add_space(8.0);
                            theme::sweep(ui, W);
                        }
                        ui.add_space(4.0);
                        ctx.request_repaint();
                    }
                    Stage::Done if n == 0 => {
                        // a green tick only when "nothing to do" is good news (not for a refused action)
                        let (icon, c) = if self.empty_ok { ("✔  ", theme::OK) } else { ("", theme::TEXT) };
                        ui.label(RichText::new(format!("{icon}{}", self.empty_title)).size(26.0).strong().color(c));
                        ui.label(RichText::new(&self.empty_text).color(theme::TEXT_DIM));
                        ui.add_space(12.0);
                        if ui.add(theme::primary_button("   Close   ")).clicked() {
                            ev = Some(OpEvent::Close);
                        }
                    }
                    Stage::Done => {
                        let t0 = *self.t_done.get_or_insert(now);
                        let shown = revealed(n, now - t0);
                        let finished = shown >= n;
                        let ok_shown = self.items[..shown].iter().filter(|i| i.ok()).count();
                        let ok_total = self.ok_count();
                        let failed_total = n - ok_total;

                        ui.horizontal(|ui| {
                            ui.label(RichText::new(ok_shown.to_string()).size(46.0).strong().color(theme::OK));
                            ui.vertical(|ui| {
                                ui.add_space(12.0);
                                let text = if finished { format!("{} {}", plural(ok_total, &self.noun), self.done_word) } else { "working...".to_string() };
                                ui.label(RichText::new(text).size(20.0).strong());
                                if finished && failed_total > 0 {
                                    ui.colored_label(theme::LOCKED, format!("{failed_total} {}", self.fail_phrase));
                                }
                            });
                        });
                        theme::progress(ui, shown as f32 / n as f32, W);
                        ui.add_space(6.0);

                        // While revealing follow the newest row; when finished go back to the top, problems first.
                        let mut rows: Vec<&OpItem> = self.items[..shown].iter().collect();
                        if finished {
                            rows.sort_by_key(|i| !i.failed());
                        }
                        egui::ScrollArea::vertical().id_salt(if finished { "op-final" } else { "op-reveal" }).max_height(250.0).stick_to_bottom(!finished).show(ui, |ui| {
                            ui.set_width(W - 12.0);
                            for it in rows {
                                let row = |ui: &mut egui::Ui, icon: &str, c: egui::Color32, strong: bool| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(c, RichText::new(icon).strong());
                                        let name = RichText::new(&it.name);
                                        ui.label(if strong { name.strong() } else { name });
                                        if !it.detail.is_empty() {
                                            ui.label(RichText::new(&it.detail).color(theme::TEXT_DIM));
                                        }
                                        ui.label(RichText::new(&it.group).color(theme::TEXT_FAINT).small());
                                    });
                                };
                                match &it.result {
                                    Some(Err(why)) => {
                                        row(ui, "✖", theme::LOCKED, true);
                                        ui.indent(("why", &it.id), |ui| {
                                            ui.colored_label(theme::LOCKED, why);
                                        });
                                    }
                                    Some(Ok(note)) if !note.is_empty() => {
                                        row(ui, "✔", theme::OK, false);
                                        ui.indent(("note", &it.id), |ui| {
                                            ui.label(RichText::new(note).color(theme::GAMING).small());
                                        });
                                    }
                                    _ => row(ui, "✔", theme::OK, false),
                                }
                            }
                        });

                        if !finished {
                            ctx.request_repaint();
                            return;
                        }
                        ui.add_space(8.0);
                        let groups = group_counts(&self.items);
                        if groups.len() > 1 {
                            ui.horizontal_wrapped(|ui| {
                                for (label, total, failed) in &groups {
                                    theme::chip(ui, &format!("{label}  {}/{total}", total - failed), if *failed == 0 { theme::OK } else { theme::JUNK });
                                }
                            });
                        }
                        if let Some((text, c)) = &self.footer {
                            ui.label(RichText::new(text).color(theme::tone(*c)));
                        }
                        for nline in &self.notes {
                            ui.label(RichText::new(nline).color(theme::TEXT_DIM));
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui.add(theme::primary_button("   Close   ")).clicked() {
                                ev = Some(OpEvent::Close);
                            }
                            for (i, label) in self.extras.iter().enumerate() {
                                if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                                    ev = Some(OpEvent::Extra(i));
                                }
                            }
                            if self.can_undo && ui.add_enabled(can, egui::Button::new(&self.undo_label)).clicked() {
                                ev = Some(OpEvent::Undo);
                            }
                        });
                    }
                }
            });

        if ev == Some(OpEvent::Go) {
            self.stage = Stage::Running;
        }
        ev
    }
}
