//! RTX 5080 Profiles tab - drawing only. The Afterburner file handling, presets, live readout, trial
//! guard and page state live in pctuner_core::gpu.

use crate::common::{self, Tab};
use crate::opwin::OpWindowUi as _;
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::gpu::*;

fn notice_frame(c: egui::Color32) -> egui::Frame {
    theme::card_frame().fill(theme::tint(c, 20)).stroke(egui::Stroke::new(1.0_f32, theme::tint(c, 130)))
}

/// One live stat with its graph.
fn stat_tile(ui: &mut egui::Ui, key: &str, value: f32, unit: &str, label: &str, series: &[f32]) {
    let v = theme::count_up(ui.ctx(), ("gpu-stat", key), value);
    theme::stat(ui, &format!("{v:.0} {unit}"), label, theme::TEXT);
    let w = (ui.available_width() - 12.0).max(40.0);
    theme::sparkline(ui, series, egui::vec2(w, 38.0), theme::ACCENT);
}

/// A selectable preset card. Returns true when clicked.
fn preset_card(ui: &mut egui::Ui, i: usize, p: &Preset, selected: bool, current: bool, needs_write: bool) -> bool {
    let id = egui::Id::new(("gpu-preset", i));
    let hovered = ui.ctx().read_response(id).map(|r| r.hovered()).unwrap_or(false);
    let t = ui.ctx().animate_bool_with_time(id.with("hover"), hovered, 0.12);
    let fill = theme::SURFACE.lerp_to_gamma(theme::SURFACE2, t);
    let stroke = if selected { egui::Stroke::new(1.5_f32, theme::ACCENT) } else { egui::Stroke::new(1.0_f32, theme::BORDER) };
    let r = theme::card_frame().fill(fill).stroke(stroke).show(ui, |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        ui.set_width(ui.available_width());
        ui.set_min_height(148.0);
        // columns justify their content; inside the card text is plain left-aligned
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.name).size(18.0).strong().color(if selected { theme::ACCENT } else { theme::TEXT }));
                if current {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::chip(ui, "current", theme::OK));
                }
            });
            ui.label(RichText::new(format!("Afterburner slot {}", p.slot)).size(12.0).color(theme::TEXT_FAINT));
            ui.label(RichText::new(p.text).color(theme::TEXT_DIM));
            if needs_write {
                ui.label(RichText::new("Slot needs writing (backup first)").size(12.0).color(theme::GAMING));
            }
        });
    });
    ui.interact(r.response.rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub struct GpuTab {
    g: Gpu,
}

impl GpuTab {
    pub fn new() -> Self {
        GpuTab { g: Gpu::new() }
    }

    fn trial_banner(&self, ui: &mut egui::Ui, can: bool, click: &mut Option<Click>) {
        let Some(t) = &self.g.trial else { return };
        if t.pending() {
            notice_frame(theme::GAMING).show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Safety trial");
                ui.label(RichText::new(format!("Testing {} - keep this tune?", t.preset)).size(18.0).strong().color(theme::TEXT));
                if t.unguarded {
                    ui.label(RichText::new(format!("Applied at {}.", hhmm(t.applied_at))).color(theme::TEXT_DIM));
                    theme::banner(ui, theme::LOCKED, "Automatic revert is NOT armed (the sign-in check could not be registered) - revert manually if it is unstable.");
                } else {
                    ui.label(
                        RichText::new(format!(
                            "Applied at {}. If the PC crashes while you test it, the GPU is set back to Stock at your next sign-in.",
                            hhmm(t.applied_at)
                        ))
                        .color(theme::TEXT_DIM),
                    );
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, theme::primary_button("   Keep   ")).on_hover_text("Stable: make this the confirmed tune and end the trial.").clicked() {
                        *click = Some(Click::Keep);
                    }
                    if ui.add_enabled(self.g.work.is_none(), egui::Button::new("Revert now")).on_hover_text(format!("Go back to {}.", t.previous)).clicked() {
                        *click = Some(Click::Revert);
                    }
                    ui.label(RichText::new(format!("Revert goes back to {}.", t.previous)).size(12.0).color(theme::TEXT_FAINT));
                });
            });
            ui.add_space(4.0);
        } else if t.status == "reverted" {
            notice_frame(theme::LOCKED).show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Trial reverted");
                ui.label(RichText::new(format!("Unexpected shutdown during the {} trial", t.preset)).size(18.0).strong().color(theme::TEXT));
                ui.label(RichText::new(reverted_text(t)).color(theme::TEXT_DIM));
                if !t.reason.is_empty() {
                    ui.label(RichText::new(format!("Windows logged: {}", t.reason)).size(12.0).color(theme::TEXT_FAINT));
                }
                if !t.verified && !t.last_error.is_empty() {
                    ui.label(RichText::new(format!("Last sign-in check: {}", t.last_error)).size(12.0).color(theme::TEXT_FAINT));
                }
                ui.add_space(2.0);
                if ui.button("   Dismiss   ").clicked() {
                    *click = Some(Click::Dismiss);
                }
            });
            ui.add_space(4.0);
        }
    }

    /// The most important thing on this screen: which preset is applied, with live graphs.
    fn hero_card(&self, ui: &mut egui::Ui, current: Option<usize>, click: &mut Option<Click>) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(s) = &self.g.state else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reading the GPU...").color(theme::TEXT_DIM));
                });
                return;
            };
            theme::section(ui, "Currently applied");
            ui.horizontal(|ui| {
                let (name, unit, caption, c) = hero_text(s.cfg.is_some(), s.saved, current);
                theme::hero(ui, name, unit, &caption, theme::tone(c));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui
                        .add_enabled(self.g.work.is_none(), theme::danger_button("⟲  Reset to stock"))
                        .on_hover_text("Puts the RTX 5080 back on the Stock preset: no undervolt, no memory overclock.")
                        .clicked()
                    {
                        *click = Some(Click::Reset);
                    }
                });
            });

            // one reading for the hero, this chip and the memory chip, so they always agree
            let live = self.g.live_shown();
            ui.horizontal_wrapped(|ui| {
                let (text, c) = live_chip(live);
                theme::chip(ui, &text, theme::tone(c));
                if s.ab_running {
                    theme::chip(ui, "Afterburner running", theme::OK);
                } else {
                    theme::chip(ui, "Afterburner not running - presets only hold while it runs", theme::JUNK);
                }
                if let Some(l) = live.filter(|l| l.full_clocks) {
                    theme::chip(ui, &format!("Memory {} MHz ({:+} over stock {})", l.mem, l.mem_offset, l.mem_stock), theme::DIM);
                }
            });
            if s.cfg.is_none() {
                theme::banner(ui, theme::LOCKED, &format!("No RTX 5080 profile file (VEN_10DE&DEV_2C02*.cfg) in {}\\Profiles.", ab_dir().display()));
            }

            if let Some(l) = live {
                ui.add_space(6.0);
                let series = |f: &dyn Fn(&Live) -> f32| -> Vec<f32> { self.g.history.iter().map(f).collect() };
                let (core, temp, power, util) = (series(&|l| l.core as f32), series(&|l| l.temp as f32), series(&|l| l.power as f32), series(&|l| l.util as f32));
                ui.columns(4, |c| {
                    stat_tile(&mut c[0], "core", l.core as f32, "MHz", "Core clock", &core);
                    stat_tile(&mut c[1], "temp", l.temp as f32, "°C", "Temperature", &temp);
                    stat_tile(&mut c[2], "power", l.power as f32, "W", "Power draw", &power);
                    stat_tile(&mut c[3], "util", l.util as f32, "%", "GPU load", &util);
                });
            }
        });
    }
}

impl Tab for GpuTab {
    fn title(&self) -> &'static str {
        "RTX 5080 Profiles"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.g.tick();
        let ctx = ui.ctx().clone();
        let busy = self.g.busy();
        let can = self.g.can_act();

        if let Some(ev) = self.g.op.as_mut().and_then(|op| op.show(&ctx, can)) {
            self.g.on_event(ev);
        }
        common::message_ui(&ctx, "RTX 5080 Profiles", &mut self.g.error);

        let mut click: Option<Click> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            theme::page_header(ui, "RTX 5080 Profiles", "Switch the card between undervolt presets stored in MSI Afterburner, and see what is applied right now.");
            common::admin_banner(ui);
            self.trial_banner(ui, can, &mut click);

            let st = self.g.state.as_ref();
            let current = st.and_then(|s| current_preset(s.saved, self.g.live_shown()));
            self.hero_card(ui, current, &mut click);

            ui.add_space(8.0);
            theme::section(ui, "Presets");
            let mut pick = None;
            ui.columns(PRESETS.len(), |cols| {
                for (i, p) in PRESETS.iter().enumerate() {
                    let needs_write = st.map(|s| s.cfg.is_some() && !s.slot_ok[i]).unwrap_or(false);
                    if preset_card(&mut cols[i], i, p, self.g.selected == Some(i), current == Some(i), needs_write) {
                        pick = Some(i);
                    }
                }
            });
            if pick.is_some() {
                self.g.selected = pick;
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = match self.g.selected {
                    Some(i) => format!("   Apply {}   ", PRESETS[i].name),
                    None => "   Apply preset   ".into(),
                };
                if ui.add_enabled(!busy && self.g.selected.is_some(), theme::primary_button(&label)).clicked() {
                    click = Some(Click::Apply);
                }
                if ui.add_enabled(!busy, egui::Button::new("Restore last backup")).clicked() {
                    click = Some(Click::Restore);
                }
                if ui.add_enabled(self.g.refresh.is_none(), egui::Button::new("Refresh")).clicked() {
                    click = Some(Click::Refresh);
                }
                if !self.g.status.is_empty() {
                    ui.label(RichText::new(&self.g.status).color(theme::TEXT_DIM));
                }
            });
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "A preset you have not kept yet starts as a trial. The automatic revert runs at your next sign-in after a crash (it cannot act during a freeze) and needs MSI Afterburner.",
                )
                .size(12.0)
                .color(theme::TEXT_DIM),
            );
        });

        if let Some(c) = click {
            self.g.click(c);
        }
    }
}
