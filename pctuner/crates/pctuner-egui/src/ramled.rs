//! RAM LED tab - drawing only. The helper calls and the state live in pctuner_core::ramled.

use crate::common::{self, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::ramled::*;

pub struct RamLedTab {
    r: RamLed,
}

impl RamLedTab {
    pub fn new() -> Self {
        RamLedTab { r: RamLed::new() }
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        let admin = common::is_admin();
        let busy = self.r.job.is_some();
        let found = !self.r.sticks.is_empty();
        let lit = self.r.sticks.iter().any(is_lit);

        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (value, caption, c) = if !admin {
                    ("-", "Start PC Tuner as administrator to read the RAM LEDs.", theme::TEXT_FAINT)
                } else if !self.r.scanned || (busy && !found) {
                    ("-", "Looking for RAM LED controllers...", theme::TEXT_FAINT)
                } else if !found {
                    ("-", "No supported RAM LED controller found.", theme::TEXT_FAINT)
                } else if lit {
                    ("On", "The RAM LEDs are lit.", theme::ACCENT)
                } else {
                    ("Off", "The RAM LEDs are dark.", theme::DIM)
                };
                theme::hero(ui, value, "RAM LEDs", caption, c);
                ui.add_space(26.0);
                if found {
                    theme::stat(ui, &self.r.sticks.len().to_string(), "sticks with LEDs", theme::TEXT);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(admin && !busy, egui::Button::new("Rescan")).on_hover_text("Read the LED controllers again. Changes nothing.").clicked() {
                        self.r.start(vec!["detect".into()], "");
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
        });
        if let Some(e) = &self.r.error {
            theme::banner(ui, theme::LOCKED, e);
        }
        ui.add_space(6.0);

        let can = admin && !busy && found;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Lighting");
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let off = if lit { theme::primary_button("   Turn off   ") } else { egui::Button::new("Turn off").min_size(egui::vec2(0.0, 32.0)) };
                if ui.add_enabled(can, off).on_hover_text("All RAM LEDs dark.").clicked() {
                    self.r.start(vec!["off".into()], "Turning the RAM LEDs off...");
                }
                let on = if lit { egui::Button::new("Turn on").min_size(egui::vec2(0.0, 32.0)) } else { theme::primary_button("   Turn on   ") };
                if ui.add_enabled(can, on).on_hover_text("Back to the last lighting the sticks had (rainbow if PC Tuner has not seen them lit).").clicked() {
                    let args = self.r.on_args();
                    self.r.start(args, "Turning the RAM LEDs on...");
                }
                ui.separator();
                if ui.add_enabled(can, egui::Button::new("Rainbow").min_size(egui::vec2(0.0, 32.0))).on_hover_text("The moving rainbow effect.").clicked() {
                    self.r.start(vec!["rainbow".into()], "Setting rainbow...");
                }
                ui.separator();
                let color_ok = self.r.sticks.iter().all(|s| s.color_ok);
                ui.add_enabled_ui(can && color_ok, |ui| {
                    if ui.color_edit_button_srgb(&mut self.r.color).changed() {
                        self.r.saved.color = Some(self.r.color);
                    }
                    let [r, g, b] = self.r.color;
                    if ui.add(egui::Button::new("Set colour").min_size(egui::vec2(0.0, 32.0))).on_hover_text("One fixed colour on every LED.").clicked() {
                        self.r.save_settings();
                        self.r.start(vec!["static".into(), format!("{r:02X}{g:02X}{b:02X}")], "Setting the colour...");
                    }
                });
                if found && !color_ok {
                    ui.label(RichText::new("Colour is not supported on this LED controller version.").color(theme::TEXT_FAINT));
                }
            });
            ui.add_space(4.0);
            let mut remember = !self.r.saved.no_save;
            if ui
                .checkbox(&mut remember, "Remember after restart (saves the setting in the RAM)")
                .on_hover_text("On: the sticks keep the setting through power-off, with nothing running. Off: it lasts until the next power-on.")
                .changed()
            {
                self.r.saved.no_save = !remember;
                self.r.save_settings();
            }
            if !self.r.status.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new(&self.r.status).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(6.0);

        if found {
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Sticks");
                ui.add_space(2.0);
                for (i, s) in self.r.sticks.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Stick {}", i + 1)).strong().color(theme::TEXT));
                        theme::chip(ui, &describe(s), if is_lit(s) { theme::ACCENT } else { theme::DIM });
                        ui.label(RichText::new(format!("{} LEDs  ·  controller {}  ·  SMBus address 0x{:02X}", s.leds, s.name, s.addr)).color(theme::TEXT_DIM));
                    });
                }
            });
            ui.add_space(6.0);
        }
        ui.label(
            RichText::new("Works without G.SKILL Trident Z Lighting Control. If that program is running, close it: it can overwrite these settings.")
                .size(12.5)
                .color(theme::TEXT_FAINT),
        );
    }
}

impl Tab for RamLedTab {
    fn title(&self) -> &'static str {
        "RAM LED"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.r.pump();
        theme::page_header(ui, "RAM LED", "Switch the lights on your memory sticks off or on, or pick a colour - no vendor software needed.");
        common::admin_banner(ui);
        egui::ScrollArea::vertical().id_salt("ramled-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }

    fn busy(&self) -> bool {
        self.r.busy()
    }
}
