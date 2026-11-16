//! USB Port Finder tab - drawing only. The device tree, tier ranking and plug-in test live in
//! pctuner_core::usb.

use crate::common::{color, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::usb::*;

pub fn tier_color(t: u8) -> egui::Color32 {
    match t {
        1 => color::OK,
        2 => color::OPTIONAL,
        3 => color::GAMING,
        4 => color::JUNK,
        _ => color::DIM,
    }
}

/// Seconds a new plug-in test hit stays highlighted.
const HIT_GLOW: f64 = 2.5;

/// A chip in a fixed-width column, so the names after it line up.
fn chip_col(ui: &mut egui::Ui, width: f32, text: &str, c: egui::Color32) {
    ui.allocate_ui_with_layout(egui::vec2(width, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        theme::chip(ui, text, c);
    });
}

/// A hero-row stat that is left out when the window is too narrow for it (instead of wrapping letter by letter).
fn stat_fit(ui: &mut egui::Ui, gap: f32, value: &str, label: &str, c: egui::Color32) {
    if ui.available_width() < gap + 7.0 * label.chars().count().max(value.chars().count() * 2) as f32 {
        return;
    }
    ui.add_space(gap);
    theme::stat(ui, value, label, c);
}

pub struct UsbTab {
    u: Usb,
}

impl UsbTab {
    pub fn new() -> Self {
        UsbTab { u: Usb::new() }
    }

    /// Hero card: the tier your controller and headset sit on.
    fn hero_ui(&self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(m) = &self.u.map else {
                if let Some(e) = &self.u.error {
                    theme::hero(ui, "?", "controller and headset", "The USB tree could not be read", theme::LOCKED);
                    ui.colored_label(theme::LOCKED, e);
                } else {
                    // live elapsed seconds, so the hint can never be wrong (the read usually takes 2-4 s)
                    let secs = self.u.scan_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    theme::hero(ui, "...", "controller and headset", &format!("Reading the USB tree... {secs} s (usually a few seconds)"), theme::TEXT_FAINT);
                    theme::sweep(ui, ui.available_width());
                }
                return;
            };
            let key = key_devices(m);
            ui.horizontal(|ui| {
                if key.is_empty() {
                    theme::hero(ui, "-", "no controller or headset on USB", "A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.", theme::TEXT_DIM);
                } else {
                    let worst = key.iter().map(|d| d.tier).max().unwrap_or(9);
                    let same = key.iter().all(|d| d.tier == worst);
                    let has_pad = key.iter().any(|d| d.kind == "GAME CONTROLLER");
                    let has_audio = key.iter().any(|d| d.kind == "AUDIO");
                    let who = match (has_pad, has_audio) {
                        (true, true) => "controller and headset",
                        (true, false) => "game controller",
                        _ => "headset / audio",
                    };
                    let mut caption = match worst {
                        1 => "on CPU ports - nothing to do",
                        2 => "on CPU lanes - fine",
                        3 | 4 => "on a chipset port - move it to a Tier 1 port if one is free",
                        _ => "on a controller this tool does not recognise",
                    }
                    .to_string();
                    if !same {
                        caption = format!("the slowest one is {caption}");
                    }
                    if key.iter().any(|d| d.hubs > 0) {
                        caption += "  ·  one goes through a hub";
                    }
                    let value = if worst <= 4 { format!("Tier {worst}") } else { "Tier ?".to_string() };
                    theme::hero(ui, &value, who, &caption, tier_color(worst));
                }
                ui.add_space(8.0);
                for d in key.iter().take(2) {
                    stat_fit(ui, 22.0, &format!("Tier {}", d.tier), &shorten(&d.name, 24), tier_color(d.tier));
                }
                stat_fit(ui, 22.0, &m.controllers.iter().filter(|c| !is_router(c)).count().to_string(), "USB controllers", theme::TEXT);
                stat_fit(ui, 22.0, &m.devices.len().to_string(), "USB devices", theme::TEXT);
            });
        });
    }

    /// Plug-in test card: instructions, start / stop, and one highlighted row per port tried.
    fn test_ui(&mut self, ui: &mut egui::Ui, now: f64) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Plug-in test");
            ui.label(RichText::new("Find out which physical socket is which tier: plug any USB device (a flash drive or your controller) into a port, wait until its row appears below, then move it to the next port.").color(theme::TEXT_DIM));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let watching = self.u.watch.is_some();
                let clicked = if watching { ui.button("Stop plug-in test").clicked() } else { ui.add(theme::primary_button("   Start plug-in test   ")).clicked() };
                if clicked {
                    if watching {
                        self.u.stop_watch();
                    } else {
                        self.u.start_watch();
                    }
                }
                match &self.u.watch {
                    Some(w) => {
                        ui.add(egui::Spinner::new().size(14.0));
                        let text = if !w.seeded {
                            "Taking a snapshot of the connected devices..."
                        } else if w.is_reading() {
                            "New device found - reading its port... keep it plugged in"
                        } else {
                            "Watching for new devices..."
                        };
                        ui.label(RichText::new(text).color(theme::TEXT_DIM));
                    }
                    None if !self.u.log.is_empty() => {
                        ui.label(RichText::new(format!("{} port(s) tested", self.u.log.len())).color(theme::TEXT_DIM));
                    }
                    None => {}
                }
            });
            if let Some(e) = &self.u.watch_error {
                ui.colored_label(theme::LOCKED, e);
            }
            // newest first; a fresh hit glows in its tier colour and fades
            for l in self.u.log.iter().rev() {
                let glow = (1.0 - (now - l.at) / HIT_GLOW).clamp(0.0, 1.0) as f32;
                let c = tier_color(l.tier);
                egui::Frame::none()
                    .fill(theme::tint(c, (12.0 + 22.0 * glow) as u8))
                    .stroke(egui::Stroke::new(1.0_f32, theme::tint(c, (50.0 + 170.0 * glow) as u8)))
                    .rounding(theme::R_WIDGET)
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("#{}", l.n)).color(theme::TEXT_DIM).strong());
                            ui.label(RichText::new(&l.name).strong().color(theme::TEXT));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("this port = {}", tier_text(l.tier))).strong().color(c.lerp_to_gamma(theme::TEXT, 0.85 * glow)));
                            });
                        });
                        let hub = if l.hubs > 0 { format!("  ·  through {} hub(s) - a front-panel or monitor port", l.hubs) } else { String::new() };
                        ui.label(RichText::new(format!("{}  ·  {}{}", l.controller, l.port, hub)).size(12.0).color(theme::TEXT_DIM));
                    });
                if glow > 0.0 {
                    ui.ctx().request_repaint();
                }
            }
        });
    }

    fn map_ui(&self, ui: &mut egui::Ui, m: &Map) {
        // ---- the devices that matter for latency
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Your game controller and audio");
            let key = key_devices(m);
            if key.is_empty() {
                ui.label(RichText::new("None found on USB. (A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.)").color(theme::TEXT_DIM));
            }
            for d in key {
                ui.horizontal(|ui| {
                    chip_col(ui, 74.0, &format!("Tier {}", d.tier), tier_color(d.tier));
                    ui.label(RichText::new(&d.name).strong().color(theme::TEXT));
                    ui.label(RichText::new(verdict(d)).color(theme::TEXT_DIM));
                });
            }
        });
        ui.add_space(4.0);

        // ---- controllers
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "USB controllers");
                ui.label(RichText::new("closest to the CPU first").size(12.0).color(theme::TEXT_DIM));
            });
            for c in &m.controllers {
                let n = m.devices.iter().filter(|d| d.ctl_id.eq_ignore_ascii_case(&c.id)).count();
                ui.horizontal(|ui| {
                    chip_col(ui, 150.0, tier_short(c.tier), tier_color(c.tier));
                    if is_router(c) {
                        ui.label(RichText::new(&c.name).color(theme::TEXT_DIM));
                        ui.label(RichText::new("USB4 router - devices on these ports appear under the add-on chip's USB controller").color(theme::TEXT_FAINT));
                    } else {
                        ui.label(RichText::new(&c.name).color(theme::TEXT));
                        ui.label(RichText::new(if n == 1 { "1 device".to_string() } else { format!("{n} devices") }).color(if n == 0 { theme::TEXT_FAINT } else { theme::TEXT_DIM }));
                    }
                })
                .response
                .on_hover_text(format!("{}\n{}", c.id, c.path));
            }
        });
        ui.add_space(4.0);

        // ---- devices
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Connected devices");
            for d in &m.devices {
                ui.horizontal(|ui| {
                    chip_col(ui, 74.0, &format!("Tier {}", d.tier), tier_color(d.tier));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&d.name).color(theme::TEXT));
                            if !d.kind.is_empty() {
                                let key = d.kind == "GAME CONTROLLER" || d.kind == "AUDIO";
                                theme::chip(ui, &d.kind.to_lowercase(), if key { theme::ACCENT } else { theme::DIM });
                            }
                            if d.hubs > 0 {
                                theme::chip(ui, &format!("via {} hub(s)", d.hubs), theme::JUNK);
                            }
                            if d.onboard {
                                theme::chip(ui, "built in - cannot be moved", theme::DIM);
                            }
                        });
                        ui.label(RichText::new(format!("{}  ·  {}", d.controller, d.port)).size(12.0).color(theme::TEXT_FAINT));
                    });
                })
                .response
                .on_hover_text(format!("{}\n{}", d.id, d.functions));
                ui.add_space(2.0);
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new("Good to know: the latency difference between tiers is a few microseconds. What matters more is plugging straight into the PC (no hub, front panel or monitor port) and not sharing a port group with a busy webcam or drive.")
                .size(12.5)
                .color(theme::TEXT_DIM),
        );
    }
}

impl Tab for UsbTab {
    fn title(&self) -> &'static str {
        "USB Port Finder"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if self.u.map.is_none() && self.u.map_job.is_none() && self.u.error.is_none() {
            self.u.start_scan();
        }
        let now = ui.input(|i| i.time);
        self.u.pump(now);

        theme::page_header(ui, "USB Port Finder", "Which USB controller every device is plugged into, ranked by how close it sits to the CPU. Changes nothing.");
        self.hero_ui(ui);

        let busy = self.u.map_job.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Rescan")).clicked() {
                self.u.start_scan();
            }
            if busy {
                if self.u.map.is_some() {
                    // the first read shows its sweep in the hero card
                    ui.label(RichText::new("Reading the USB tree...").color(theme::TEXT_DIM));
                    theme::sweep(ui, 220.0);
                }
            } else if let Some(m) = &self.u.map {
                ui.label(RichText::new(format!("read {}", m.time)).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(2.0);

        // screenshots only: PCTUNER_DEMO=usb:done starts scrolled down to the device list
        let mut area = egui::ScrollArea::vertical().id_salt("usb-scroll").auto_shrink([false, false]);
        if crate::opwin::demo_stage("usb") == Some(crate::opwin::Stage::Done) {
            area = area.vertical_scroll_offset(470.0);
        }
        area.show(ui, |ui| {
            self.test_ui(ui, now);
            ui.add_space(4.0);
            if let Some(m) = &self.u.map {
                self.map_ui(ui, m);
            }
        });
    }
}
