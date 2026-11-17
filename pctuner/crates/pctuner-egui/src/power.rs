//! Power button window - drawing only. The choices and the shutdown call live in pctuner_core::power.

use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::power::*;

pub struct PowerDialog {
    p: Power,
}

impl PowerDialog {
    pub fn new() -> Self {
        PowerDialog { p: Power::new() }
    }

    /// Draw the window. Returns false when it should close.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        self.p.pump();
        let mut keep = true;
        theme::dim_backdrop(ctx, "power");
        egui::Window::new("power-dialog")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                theme::section(ui, "Power");
                match self.p.stage {
                    Stage::Choose => {
                        ui.label(RichText::new("Go into the BIOS").size(24.0).strong());
                        ui.label(RichText::new("No need to press Del at the right moment - Windows tells the motherboard to open its setup.").color(theme::TEXT_DIM));
                        ui.add_space(8.0);
                        if let Some(e) = &self.p.error {
                            theme::banner(ui, theme::LOCKED, e);
                            ui.add_space(4.0);
                        }
                        let blocked = self.p.blocked();
                        if let Some(b) = blocked {
                            theme::banner(ui, theme::GAMING, b);
                            ui.add_space(4.0);
                        }
                        for c in [Choice::RestartToBios, Choice::ShutdownToBios] {
                            if choice_card(ui, c, self.p.can_pick()) {
                                self.p.pick(c, ctx.input(|i| i.time));
                            }
                            ui.add_space(6.0);
                        }
                        ui.label(RichText::new("Clicking one acts at once: every open program closes without saving.").size(12.0).color(theme::GAMING));
                        if self.p.uefi.is_none() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new("Checking the firmware type...").color(theme::TEXT_DIM));
                            });
                        }
                        ui.add_space(6.0);
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    }
                    Stage::Working(c, started) if working_timed_out(started, ctx.input(|i| i.time)) => {
                        ui.label(RichText::new(c.failed_title()).size(22.0).strong());
                        ui.label(RichText::new("The restart or shut down was cancelled or blocked (for example by a program that would not close).").color(theme::TEXT_DIM));
                        ui.add_space(8.0);
                        theme::banner(ui, theme::GAMING, "Windows may still remember the BIOS request: the next restart can open the BIOS.");
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("Back").clicked() {
                                self.p.back();
                            }
                            if ui.add(theme::primary_button("   Close   ")).clicked() {
                                keep = false;
                            }
                        });
                    }
                    Stage::Working(c, _) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(c.working_title()).size(22.0).strong());
                        });
                        ui.label(RichText::new("Windows is closing your programs.").color(theme::TEXT_DIM));
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                }
            });
        keep
    }
}

/// One big clickable choice. Returns true when clicked.
fn choice_card(ui: &mut egui::Ui, c: Choice, enabled: bool) -> bool {
    let id = ui.id().with(("power-choice", c.title()));
    let hovered = enabled && ui.ctx().read_response(id).map(|r| r.hovered()).unwrap_or(false);
    let hover = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.12);
    let border = theme::BORDER.lerp_to_gamma(theme::ACCENT, hover);
    let icon = match c {
        Choice::RestartToBios => "⟲",
        Choice::ShutdownToBios => "⬇",
    };
    let inner = theme::card_frame().fill(theme::SURFACE2).stroke(egui::Stroke::new(1.0_f32, border)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon).size(26.0).color(if enabled { theme::ACCENT } else { theme::TEXT_FAINT }));
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(c.title()).size(16.0).strong().color(if enabled { theme::TEXT } else { theme::TEXT_FAINT }));
                ui.label(RichText::new(c.text()).size(12.5).color(theme::TEXT_DIM));
            });
        });
    });
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let r = ui.interact(inner.response.rect, id, sense);
    if enabled {
        r.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    enabled && r.clicked()
}
