//! RAM LED tab: switch the LEDs on G.SKILL (ENE controller) memory off / on, or set a colour, without
//! the vendor's lighting software.
//!
//! The work is done by the fan helper's one-shot `--ramled` mode (fanhelper\RamLed.cs), which reaches the
//! sticks over the motherboard SMBus through the PawnIO driver. This tab only starts it and shows the answer.

use crate::common::{self, Job, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

const SAVED: &str = "ramled.json";
const MODE_OFF: u8 = 0;
const MODE_STATIC: u8 = 1;
const MODE_RAINBOW: u8 = 5;

/// One stick's LED controller as the helper reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Stick {
    id: String,
    bus: String,
    addr: u8,
    name: String,
    leds: u32,
    color_ok: bool,
    mode: u8,
    speed: u8,
    dir: u8,
    /// Effect colours as stored in the controller: hex, R,B,G per LED.
    colors: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Reply {
    ok: bool,
    error: String,
    sticks: Vec<Stick>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// The last lit state seen on each stick: what "Turn on" puts back.
    last_on: Vec<Stick>,
    /// "Remember after restart" switched off.
    no_save: bool,
    /// The colour picker's last colour.
    color: Option<[u8; 3]>,
}

/// First LED's colour as R,G,B (the controller stores R,B,G).
fn first_rgb(s: &Stick) -> Option<[u8; 3]> {
    let c = s.colors.as_bytes();
    if !s.color_ok || c.len() < 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(std::str::from_utf8(&c[i..i + 2]).ok()?, 16).ok();
    Some([b(0)?, b(4)?, b(2)?])
}

/// Are the LEDs giving light: any mode but Off, except a static colour that is all black.
fn is_lit(s: &Stick) -> bool {
    match s.mode {
        MODE_OFF => false,
        MODE_STATIC if s.color_ok => s.colors.bytes().any(|b| b != b'0'),
        _ => true,
    }
}

fn describe(s: &Stick) -> String {
    match s.mode {
        MODE_OFF => "Off".into(),
        MODE_STATIC => match first_rgb(s) {
            Some(_) if !is_lit(s) => "Off (black)".into(),
            Some([r, g, b]) => format!("Static colour #{r:02X}{g:02X}{b:02X}"),
            None => "Static colour".into(),
        },
        MODE_RAINBOW => "Rainbow".into(),
        m => format!("Effect {m}"),
    }
}

/// Helper argument that puts a stick's snapshot back.
fn restore_arg(s: &Stick) -> String {
    format!("{}:{}:{}:{}:{}", s.id, s.mode, s.speed, s.dir, s.colors)
}

/// The helper prints one JSON line; anything else on stdout is ignored.
fn parse_reply(stdout: &str) -> Result<Vec<Stick>, String> {
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("The RAM LED helper gave no answer.")?;
    let r: Reply = serde_json::from_str(line.trim()).map_err(|e| format!("The RAM LED helper's answer could not be read: {e}"))?;
    if r.ok {
        Ok(r.sticks)
    } else if r.error.is_empty() {
        Err("The RAM LED helper failed.".into())
    } else {
        Err(r.error)
    }
}

fn run_helper(args: Vec<String>) -> Result<Vec<Stick>, String> {
    let exe = crate::fan::extract_helper()?;
    let out = Command::new(&exe)
        .arg("--ramled")
        .args(&args)
        .stdin(Stdio::null())
        .creation_flags(common::CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    parse_reply(&String::from_utf8_lossy(&out.stdout))
}

pub struct RamLedTab {
    sticks: Vec<Stick>,
    error: Option<String>,
    status: String,
    /// (job, true when it changes the LEDs)
    job: Option<(Job<Result<Vec<Stick>, String>>, bool)>,
    scanned: bool,
    saved: Saved,
    color: [u8; 3],
}

impl RamLedTab {
    pub fn new() -> Self {
        let saved: Saved = common::load_json(SAVED);
        let color = saved.color.unwrap_or([255, 255, 255]);
        RamLedTab { sticks: vec![], error: None, status: String::new(), job: None, scanned: false, saved, color }
    }

    fn start(&mut self, mut args: Vec<String>, what: &str) {
        let change = args[0] != "detect";
        if change && !self.saved.no_save {
            args.push("--save".into());
        }
        self.status = what.to_string();
        self.job = Some((Job::spawn(move || run_helper(args)), change));
    }

    fn finish(&mut self, result: Result<Vec<Stick>, String>, change: bool) {
        match result {
            Ok(sticks) => {
                self.error = None;
                self.status = if !change {
                    String::new()
                } else if self.saved.no_save {
                    "Done. Not saved: the LEDs go back to their stored setting at the next power-on.".into()
                } else {
                    "Done, and saved in the RAM: it stays like this after a restart.".into()
                };
                // remember the last lit state of each stick for "Turn on"
                let mut dirty = false;
                for s in sticks.iter().filter(|s| is_lit(s)) {
                    match self.saved.last_on.iter_mut().find(|o| o.id == s.id) {
                        Some(o) if o == s => {}
                        Some(o) => {
                            *o = s.clone();
                            dirty = true;
                        }
                        None => {
                            self.saved.last_on.push(s.clone());
                            dirty = true;
                        }
                    }
                }
                if dirty {
                    common::save_json(SAVED, &self.saved);
                }
                self.sticks = sticks;
            }
            Err(e) => {
                self.status.clear();
                self.error = Some(e);
            }
        }
    }

    /// "Turn on": each stick's last lit state; rainbow when none is known.
    fn on_args(&self) -> Vec<String> {
        let known: Vec<String> = self.sticks.iter().filter_map(|s| self.saved.last_on.iter().find(|o| o.id == s.id && o.name == s.name)).map(restore_arg).collect();
        if known.len() == self.sticks.len() && !known.is_empty() {
            std::iter::once("restore".to_string()).chain(known).collect()
        } else {
            vec!["rainbow".into()]
        }
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        let admin = common::is_admin();
        let busy = self.job.is_some();
        let found = !self.sticks.is_empty();
        let lit = self.sticks.iter().any(is_lit);

        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (value, caption, c) = if !admin {
                    ("-", "Start PC Tuner as administrator to read the RAM LEDs.", theme::TEXT_FAINT)
                } else if !self.scanned || (busy && !found) {
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
                    theme::stat(ui, &self.sticks.len().to_string(), "sticks with LEDs", theme::TEXT);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(admin && !busy, egui::Button::new("Rescan")).on_hover_text("Read the LED controllers again. Changes nothing.").clicked() {
                        self.start(vec!["detect".into()], "");
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
        });
        if let Some(e) = &self.error {
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
                    self.start(vec!["off".into()], "Turning the RAM LEDs off...");
                }
                let on = if lit { egui::Button::new("Turn on").min_size(egui::vec2(0.0, 32.0)) } else { theme::primary_button("   Turn on   ") };
                if ui.add_enabled(can, on).on_hover_text("Back to the last lighting the sticks had (rainbow if PC Tuner has not seen them lit).").clicked() {
                    let args = self.on_args();
                    self.start(args, "Turning the RAM LEDs on...");
                }
                ui.separator();
                if ui.add_enabled(can, egui::Button::new("Rainbow").min_size(egui::vec2(0.0, 32.0))).on_hover_text("The moving rainbow effect.").clicked() {
                    self.start(vec!["rainbow".into()], "Setting rainbow...");
                }
                ui.separator();
                let color_ok = self.sticks.iter().all(|s| s.color_ok);
                ui.add_enabled_ui(can && color_ok, |ui| {
                    if ui.color_edit_button_srgb(&mut self.color).changed() {
                        self.saved.color = Some(self.color);
                    }
                    let [r, g, b] = self.color;
                    if ui.add(egui::Button::new("Set colour").min_size(egui::vec2(0.0, 32.0))).on_hover_text("One fixed colour on every LED.").clicked() {
                        common::save_json(SAVED, &self.saved);
                        self.start(vec!["static".into(), format!("{r:02X}{g:02X}{b:02X}")], "Setting the colour...");
                    }
                });
                if found && !color_ok {
                    ui.label(RichText::new("Colour is not supported on this LED controller version.").color(theme::TEXT_FAINT));
                }
            });
            ui.add_space(4.0);
            let mut remember = !self.saved.no_save;
            if ui
                .checkbox(&mut remember, "Remember after restart (saves the setting in the RAM)")
                .on_hover_text("On: the sticks keep the setting through power-off, with nothing running. Off: it lasts until the next power-on.")
                .changed()
            {
                self.saved.no_save = !remember;
                common::save_json(SAVED, &self.saved);
            }
            if !self.status.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new(&self.status).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(6.0);

        if found {
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Sticks");
                ui.add_space(2.0);
                for (i, s) in self.sticks.iter().enumerate() {
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
        if !self.scanned && self.job.is_none() && common::is_admin() {
            self.scanned = true;
            self.start(vec!["detect".into()], "");
        }
        if let Some(r) = self.job.as_ref().and_then(|(j, _)| j.poll()) {
            let (_, change) = self.job.take().unwrap();
            self.finish(r, change);
        }
        theme::page_header(ui, "RAM LED", "Switch the lights on your memory sticks off or on, or pick a colour - no vendor software needed.");
        common::admin_banner(ui);
        egui::ScrollArea::vertical().id_salt("ramled-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }

    fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|(_, change)| *change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick(mode: u8, colors: &str) -> Stick {
        Stick { id: "0-71".into(), name: "AUDA0-E6K5-0101".into(), addr: 0x71, leds: 2, color_ok: true, mode, colors: colors.into(), ..Default::default() }
    }

    #[test]
    fn ramled_parse_reply() {
        let ok = r#"{"ok":true,"error":"","sticks":[{"id":"0-71","bus":"SMBusPawnIO PIIX4 port 0","addr":113,"name":"AUDA0-E6K5-0101","leds":8,"color_ok":true,"mode":5,"speed":0,"dir":0,"colors":"00FF"}]}"#;
        let s = parse_reply(&format!("noise\n{ok}\n")).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].addr, s[0].mode, s[0].leds, s[0].color_ok), (0x71, 5, 8, true));
        assert_eq!(parse_reply(r#"{"ok":false,"error":"No supported RAM LED controller found.","sticks":[]}"#).unwrap_err(), "No supported RAM LED controller found.");
        assert!(parse_reply("").is_err());
        assert!(parse_reply("{broken").is_err());
    }

    #[test]
    fn ramled_state() {
        assert!(!is_lit(&stick(MODE_OFF, "FF0000FF0000")));
        assert!(!is_lit(&stick(MODE_STATIC, "000000000000")));
        assert!(is_lit(&stick(MODE_STATIC, "000000000100")));
        assert!(is_lit(&stick(MODE_RAINBOW, "000000000000")));
        assert_eq!(describe(&stick(MODE_OFF, "")), "Off");
        assert_eq!(describe(&stick(MODE_STATIC, "000000000000")), "Off (black)");
        // stored R,B,G -> shown R,G,B
        assert_eq!(describe(&stick(MODE_STATIC, "FF4080FF4080")), "Static colour #FF8040");
        assert_eq!(describe(&stick(MODE_RAINBOW, "")), "Rainbow");
        assert_eq!(restore_arg(&stick(4, "AB")), "0-71:4:0:0:AB");
    }

    #[test]
    fn ramled_turn_on_uses_last_lit_state() {
        let mut t = RamLedTab { sticks: vec![], error: None, status: String::new(), job: None, scanned: true, saved: Saved::default(), color: [0; 3] };
        t.finish(Ok(vec![stick(MODE_OFF, "000000000000")]), false);
        assert_eq!(t.on_args(), vec!["rainbow".to_string()], "never seen lit: rainbow");
        t.saved.last_on = vec![stick(MODE_STATIC, "FF0000FF0000")];
        assert_eq!(t.on_args(), vec!["restore".to_string(), "0-71:1:0:0:FF0000FF0000".to_string()]);
    }

    /// Read-only: asks the helper which sticks have an LED controller. Needs administrator and the PawnIO driver.
    /// Run elevated: cargo test ramled_live_detect -- --ignored   (RAMLED_LIVE_LOG=<file> gets the result)
    #[test]
    #[ignore]
    fn ramled_live_detect() {
        assert!(common::is_admin(), "run elevated");
        let r = run_helper(vec!["detect".into()]);
        if let Ok(p) = std::env::var("RAMLED_LIVE_LOG") {
            let _ = std::fs::write(p, format!("{r:#?}"));
        }
        let sticks = r.expect("detect");
        assert!(!sticks.is_empty());
        assert!(sticks.iter().all(|s| (0x70..=0x77).contains(&s.addr)));
    }

    /// Changes the RAM LEDs, then puts them back (nothing is saved in the RAM). Needs administrator.
    /// Run elevated: cargo test ramled_live_toggle -- --ignored
    #[test]
    #[ignore]
    fn ramled_live_toggle() {
        assert!(common::is_admin(), "run elevated");
        let before = run_helper(vec!["detect".into()]).expect("detect");
        let off = run_helper(vec!["off".into()]).expect("off");
        assert!(off.iter().all(|s| s.mode == MODE_OFF));
        let back = run_helper(std::iter::once("restore".to_string()).chain(before.iter().map(restore_arg)).collect()).expect("restore");
        assert_eq!(back, before);
    }
}
