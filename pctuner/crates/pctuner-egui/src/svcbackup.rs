//! The Services "Backups" window - drawing only.
//! Creating, listing and restoring backups lives in pctuner_core::svcbackup.

use crate::common;
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
pub use pctuner_core::svcbackup::*;

/// The Backups window (opened from the Services tab).
pub struct BackupsWindow {
    b: Backups,
}

impl BackupsWindow {
    pub fn new() -> Self {
        BackupsWindow { b: Backups::new() }
    }

    pub fn busy(&self) -> bool {
        self.b.busy()
    }

    /// Draw; returns (keep open, services changed so the list should reload).
    pub fn show(&mut self, ctx: &egui::Context) -> (bool, bool) {
        let changed = self.b.pump();
        let can = self.b.can_act();
        if let Some(op) = &mut self.b.op {
            if let Some(ev) = op.show(ctx, can) {
                self.b.on_event(ev);
            }
            return (true, changed);
        }

        let mut keep = true;
        let admin = common::is_admin();
        theme::dim_backdrop(ctx, "svc-backups");
        egui::Window::new("svc-backups").title_bar(false).collapsible(false).resizable(false).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).frame(theme::modal_frame(ctx)).show(ctx, |ui| {
            ui.set_width(620.0);
            theme::section(ui, "Services");
            ui.label(RichText::new("Backups").size(24.0).strong());
            ui.label(RichText::new("A backup saves the startup type of every service. Each one has three ways back: Restore here, a double-click RESTORE .cmd (works in Safe Mode), and a full .reg export.").color(theme::TEXT_DIM));
            ui.add_space(6.0);
            if let Some((m, ok)) = &self.b.msg {
                theme::banner(ui, if *ok { theme::OK } else { theme::LOCKED }, m);
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(admin && self.b.job.is_none(), theme::primary_button("   Create backup now   ")).clicked() {
                    self.b.create_backup();
                }
                if self.b.job.is_some() {
                    ui.spinner();
                }
                if ui.button("Open backup folder").clicked() {
                    let d = backup_dir();
                    let _ = std::fs::create_dir_all(&d);
                    let _ = common::run("explorer.exe", &[&d.display().to_string()]);
                }
            });
            if !admin {
                ui.colored_label(theme::LOCKED, "Run PC Tuner as administrator to create or restore backups.");
            }
            ui.add_space(6.0);
            if self.b.backups.is_empty() {
                ui.label(RichText::new("No backups yet. PC Tuner also makes one by itself before the first service change.").color(theme::TEXT_DIM));
            }
            let current = self.b.current.clone();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (_path, b) in self.b.backups.clone() {
                    let diffs = differences(&b.services, &current);
                    theme::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&b.time).strong());
                                let note = if b.note.is_empty() { String::new() } else { format!(" - {}", b.note) };
                                ui.label(RichText::new(format!("{} services{note}", b.services.len())).size(12.0).color(theme::TEXT_DIM));
                                let d = if diffs.is_empty() { "matches the PC now".to_string() } else { format!("{} differ from now", diffs.len()) };
                                ui.label(RichText::new(d).size(12.0).color(if diffs.is_empty() { theme::OK } else { theme::GAMING }));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let btn = if diffs.is_empty() { egui::Button::new("Restore this backup") } else { theme::primary_button("  Restore this backup  ") };
                                if ui.add_enabled(admin && self.b.run.is_none(), btn).on_hover_text("Puts every service back to the startup type in this backup.").clicked() {
                                    self.b.open_restore(&b, &diffs);
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
            });
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                keep = false;
            }
        });
        (keep, changed)
    }
}
