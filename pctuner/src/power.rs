//! Power button: restart into the BIOS (UEFI setup), or shut down so the next start opens the BIOS.
//! Uses Windows' own `shutdown /fw` (needs administrator and a UEFI PC).

use crate::common::{self, Job};
use crate::theme;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Choice {
    RestartToBios,
    ShutdownToBios,
}

impl Choice {
    fn title(self) -> &'static str {
        match self {
            Choice::RestartToBios => "Restart into BIOS",
            Choice::ShutdownToBios => "Shut down, BIOS on next start",
        }
    }
    fn text(self) -> &'static str {
        match self {
            Choice::RestartToBios => "Windows restarts and opens your motherboard's BIOS (UEFI) setup straight away.",
            Choice::ShutdownToBios => "The PC turns off. The next time you press the power button it goes straight into the BIOS.",
        }
    }
    fn go(self) -> &'static str {
        match self {
            Choice::RestartToBios => "   Restart now   ",
            Choice::ShutdownToBios => "   Shut down now   ",
        }
    }
    /// Arguments for shutdown.exe. /fw = next boot goes to the firmware setup; /t 0 = no waiting.
    pub fn args(self) -> [&'static str; 4] {
        match self {
            Choice::RestartToBios => ["/r", "/fw", "/t", "0"],
            Choice::ShutdownToBios => ["/s", "/fw", "/t", "0"],
        }
    }
}

enum Stage {
    Choose,
    Confirm(Choice),
    /// The choice and the UI time the command was sent.
    Working(Choice, f64),
}

/// Seconds after which a restart / shut down that did not happen is reported (it was cancelled,
/// for example on Windows' "apps are preventing restart" screen, or blocked by a program).
const WORKING_TIMEOUT: f64 = 25.0;

/// True when PC Tuner is still running `WORKING_TIMEOUT` seconds after the command was sent.
fn working_timed_out(started: f64, now: f64) -> bool {
    now - started >= WORKING_TIMEOUT
}

pub struct PowerDialog {
    stage: Stage,
    /// Some(true) when the PC boots in UEFI mode (needed for /fw); checked when the window opens.
    uefi: Option<bool>,
    uefi_job: Option<Job<bool>>,
    job: Option<Job<Result<(i32, String), String>>>,
    error: Option<String>,
}

fn is_uefi() -> bool {
    common::run("cmd.exe", &["/c", "echo %firmware_type%"]).map(|(_, out)| out.trim().eq_ignore_ascii_case("UEFI")).unwrap_or(false)
}

impl PowerDialog {
    pub fn new() -> Self {
        PowerDialog { stage: Stage::Choose, uefi: None, uefi_job: Some(Job::spawn(is_uefi)), job: None, error: None }
    }

    /// Draw the window. Returns false when it should close.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if let Some(j) = &self.uefi_job {
            if let Some(v) = j.poll() {
                self.uefi = Some(v);
                self.uefi_job = None;
            }
        }
        if let Some(j) = &self.job {
            if let Some(r) = j.poll() {
                self.job = None;
                match r {
                    // shutdown.exe returns at once; Windows then closes everything
                    Ok((0, _)) => {}
                    Ok((code, out)) => {
                        self.error = Some(format!("Windows refused (code {code}): {}", out.trim()));
                        self.stage = Stage::Choose;
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.stage = Stage::Choose;
                    }
                }
            }
        }
        let admin = common::is_admin();
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
                match self.stage {
                    Stage::Choose => {
                        ui.label(RichText::new("Go into the BIOS").size(24.0).strong());
                        ui.label(RichText::new("No need to press Del at the right moment - Windows tells the motherboard to open its setup.").color(theme::TEXT_DIM));
                        ui.add_space(8.0);
                        if let Some(e) = &self.error {
                            theme::banner(ui, theme::LOCKED, e);
                            ui.add_space(4.0);
                        }
                        let blocked = if !admin {
                            Some("Run PC Tuner as administrator to use this.")
                        } else if self.uefi == Some(false) {
                            Some("This PC does not boot in UEFI mode, so Windows cannot open the BIOS for you. Press Del while the PC starts instead.")
                        } else {
                            None
                        };
                        if let Some(b) = blocked {
                            theme::banner(ui, theme::GAMING, b);
                            ui.add_space(4.0);
                        }
                        for c in [Choice::RestartToBios, Choice::ShutdownToBios] {
                            let picked = choice_card(ui, c, blocked.is_none() && self.uefi.is_some());
                            if picked {
                                self.error = None;
                                self.stage = Stage::Confirm(c);
                            }
                            ui.add_space(6.0);
                        }
                        if self.uefi.is_none() {
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
                    Stage::Confirm(c) => {
                        ui.label(RichText::new(format!("{}?", c.title())).size(24.0).strong());
                        ui.label(RichText::new(c.text()).color(theme::TEXT_DIM));
                        ui.add_space(8.0);
                        theme::banner(ui, theme::GAMING, "Save your work first: every open program closes right away. Fans go back to the BIOS.");
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.add(theme::danger_button(c.go()).min_size(egui::vec2(0.0, 32.0))).clicked() {
                                self.stage = Stage::Working(c, ctx.input(|i| i.time));
                                self.job = Some(Job::spawn(move || common::run("shutdown.exe", &c.args())));
                            }
                            if ui.button("Back").clicked() {
                                self.stage = Stage::Choose;
                            }
                        });
                    }
                    Stage::Working(c, started) if working_timed_out(started, ctx.input(|i| i.time)) => {
                        let t = match c {
                            Choice::RestartToBios => "Windows did not restart",
                            Choice::ShutdownToBios => "Windows did not shut down",
                        };
                        ui.label(RichText::new(t).size(22.0).strong());
                        ui.label(RichText::new("The restart or shut down was cancelled or blocked (for example by a program that would not close).").color(theme::TEXT_DIM));
                        ui.add_space(8.0);
                        theme::banner(ui, theme::GAMING, "Windows may still remember the BIOS request: the next restart can open the BIOS.");
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("Back").clicked() {
                                self.stage = Stage::Choose;
                            }
                            if ui.add(theme::primary_button("   Close   ")).clicked() {
                                keep = false;
                            }
                        });
                    }
                    Stage::Working(c, _) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let t = match c {
                                Choice::RestartToBios => "Restarting into the BIOS...",
                                Choice::ShutdownToBios => "Shutting down...",
                            };
                            ui.label(RichText::new(t).size(22.0).strong());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_shutdown_args() {
        assert_eq!(Choice::RestartToBios.args(), ["/r", "/fw", "/t", "0"]);
        assert_eq!(Choice::ShutdownToBios.args(), ["/s", "/fw", "/t", "0"]);
    }

    #[test]
    fn power_working_times_out() {
        assert!(!working_timed_out(100.0, 100.0));
        assert!(!working_timed_out(100.0, 124.9));
        assert!(working_timed_out(100.0, 125.0));
        assert!(working_timed_out(100.0, 500.0));
    }
}
