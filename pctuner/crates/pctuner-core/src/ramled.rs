//! RAM LED: switch the LEDs on G.SKILL (ENE controller) memory off / on, or set a colour, without
//! the vendor's lighting software.
//!
//! The work is done by the fan helper's one-shot `--ramled` mode (fanhelper\RamLed.cs), which reaches the
//! sticks over the motherboard SMBus through the PawnIO driver. This module starts it and keeps the state.

use crate::common::{self, Job};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

pub const SAVED: &str = "ramled.json";
pub const MODE_OFF: u8 = 0;
pub const MODE_STATIC: u8 = 1;
pub const MODE_RAINBOW: u8 = 5;

/// One stick's LED controller as the helper reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stick {
    pub id: String,
    pub bus: String,
    pub addr: u8,
    pub name: String,
    pub leds: u32,
    pub color_ok: bool,
    pub mode: u8,
    pub speed: u8,
    pub dir: u8,
    /// Effect colours as stored in the controller: hex, R,B,G per LED.
    pub colors: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Reply {
    pub ok: bool,
    pub error: String,
    pub sticks: Vec<Stick>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The last lit state seen on each stick: what "Turn on" puts back.
    pub last_on: Vec<Stick>,
    /// "Remember after restart" switched off.
    pub no_save: bool,
    /// The colour picker's last colour.
    pub color: Option<[u8; 3]>,
}

/// First LED's colour as R,G,B (the controller stores R,B,G).
pub fn first_rgb(s: &Stick) -> Option<[u8; 3]> {
    let c = s.colors.as_bytes();
    if !s.color_ok || c.len() < 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(std::str::from_utf8(&c[i..i + 2]).ok()?, 16).ok();
    Some([b(0)?, b(4)?, b(2)?])
}

/// Are the LEDs giving light: any mode but Off, except a static colour that is all black.
pub fn is_lit(s: &Stick) -> bool {
    match s.mode {
        MODE_OFF => false,
        MODE_STATIC if s.color_ok => s.colors.bytes().any(|b| b != b'0'),
        _ => true,
    }
}

pub fn describe(s: &Stick) -> String {
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
pub fn restore_arg(s: &Stick) -> String {
    format!("{}:{}:{}:{}:{}", s.id, s.mode, s.speed, s.dir, s.colors)
}

/// The helper prints one JSON line; anything else on stdout is ignored.
pub fn parse_reply(stdout: &str) -> Result<Vec<Stick>, String> {
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

pub fn run_helper(args: Vec<String>) -> Result<Vec<Stick>, String> {
    let exe = crate::fan_files::extract_helper()?;
    let out = Command::new(&exe)
        .arg("--ramled")
        .args(&args)
        .stdin(Stdio::null())
        .creation_flags(common::CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    parse_reply(&String::from_utf8_lossy(&out.stdout))
}

/// The RAM LED state: the sticks found, the running helper call, and what is remembered between runs.
pub struct RamLed {
    pub sticks: Vec<Stick>,
    pub error: Option<String>,
    pub status: String,
    /// (job, true when it changes the LEDs)
    pub job: Option<(Job<Result<Vec<Stick>, String>>, bool)>,
    pub scanned: bool,
    pub saved: Saved,
    pub color: [u8; 3],
}

impl Default for RamLed {
    fn default() -> Self {
        Self::new()
    }
}

impl RamLed {
    pub fn new() -> Self {
        let saved: Saved = common::load_json(SAVED);
        let color = saved.color.unwrap_or([255, 255, 255]);
        RamLed { sticks: vec![], error: None, status: String::new(), job: None, scanned: false, saved, color }
    }

    pub fn start(&mut self, mut args: Vec<String>, what: &str) {
        let change = args[0] != "detect";
        if change && !self.saved.no_save {
            args.push("--save".into());
        }
        self.status = what.to_string();
        self.job = Some((Job::spawn(move || run_helper(args)), change));
    }

    pub fn finish(&mut self, result: Result<Vec<Stick>, String>, change: bool) {
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
    pub fn on_args(&self) -> Vec<String> {
        let known: Vec<String> = self.sticks.iter().filter_map(|s| self.saved.last_on.iter().find(|o| o.id == s.id && o.name == s.name)).map(restore_arg).collect();
        if known.len() == self.sticks.len() && !known.is_empty() {
            std::iter::once("restore".to_string()).chain(known).collect()
        } else {
            vec!["rainbow".into()]
        }
    }

    /// Call often (every frame / tick): starts the first detect when allowed and takes a finished helper call.
    pub fn pump(&mut self) {
        if !self.scanned && self.job.is_none() && common::is_admin() {
            self.scanned = true;
            self.start(vec!["detect".into()], "");
        }
        if let Some(r) = self.job.as_ref().and_then(|(j, _)| j.poll()) {
            let (_, change) = self.job.take().unwrap();
            self.finish(r, change);
        }
    }

    /// True while a call that changes the LEDs is running.
    pub fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|(_, change)| *change)
    }

    /// Store the colour picker's colour.
    pub fn save_settings(&self) {
        common::save_json(SAVED, &self.saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn stick(mode: u8, colors: &str) -> Stick {
        Stick { id: "0-71".into(), name: "AUDA0-E6K5-0101".into(), addr: 0x71, leds: 2, color_ok: true, mode, colors: colors.into(), ..Default::default() }
    }

    #[test]
    pub fn ramled_parse_reply() {
        let ok = r#"{"ok":true,"error":"","sticks":[{"id":"0-71","bus":"SMBusPawnIO PIIX4 port 0","addr":113,"name":"AUDA0-E6K5-0101","leds":8,"color_ok":true,"mode":5,"speed":0,"dir":0,"colors":"00FF"}]}"#;
        let s = parse_reply(&format!("noise\n{ok}\n")).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].addr, s[0].mode, s[0].leds, s[0].color_ok), (0x71, 5, 8, true));
        assert_eq!(parse_reply(r#"{"ok":false,"error":"No supported RAM LED controller found.","sticks":[]}"#).unwrap_err(), "No supported RAM LED controller found.");
        assert!(parse_reply("").is_err());
        assert!(parse_reply("{broken").is_err());
    }

    #[test]
    pub fn ramled_state() {
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
    pub fn ramled_turn_on_uses_last_lit_state() {
        let mut t = RamLed { sticks: vec![], error: None, status: String::new(), job: None, scanned: true, saved: Saved::default(), color: [0; 3] };
        t.finish(Ok(vec![stick(MODE_OFF, "000000000000")]), false);
        assert_eq!(t.on_args(), vec!["rainbow".to_string()], "never seen lit: rainbow");
        t.saved.last_on = vec![stick(MODE_STATIC, "FF0000FF0000")];
        assert_eq!(t.on_args(), vec!["restore".to_string(), "0-71:1:0:0:FF0000FF0000".to_string()]);
    }

    /// Read-only: asks the helper which sticks have an LED controller. Needs administrator and the PawnIO driver.
    /// Run elevated: cargo test ramled_live_detect -- --ignored   (RAMLED_LIVE_LOG=<file> gets the result)
    #[test]
    #[ignore]
    pub fn ramled_live_detect() {
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
    pub fn ramled_live_toggle() {
        assert!(common::is_admin(), "run elevated");
        let before = run_helper(vec!["detect".into()]).expect("detect");
        let off = run_helper(vec!["off".into()]).expect("off");
        assert!(off.iter().all(|s| s.mode == MODE_OFF));
        let back = run_helper(std::iter::once("restore".to_string()).chain(before.iter().map(restore_arg)).collect()).expect("restore");
        assert_eq!(back, before);
    }
}
