//! USB Port Finder: which controller every device is plugged into, and the live plug-in test.
//! All logic is pctuner_core::usb; this backend builds the view. The only change it makes: switching an empty USB
//! controller off (and back on).

use crate::backend::{parse, Backend};
use pctuner_core::common::Tone;
use pctuner_core::usb::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Seconds a new plug-in test hit stays highlighted.
const HIT_GLOW: f64 = 2.5;

pub struct UsbTab {
    u: Usb,
    /// The UI clock the core stamps plug-in test hits with.
    clock: Instant,
    /// The page has been opened: the first scan starts then, not at program start.
    opened: AtomicBool,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Rescan,
    StartWatch,
    StopWatch,
    /// Disable (only an empty controller) or enable a USB controller.
    Controller { id: String, enable: bool },
    DismissNote,
}

fn tier_tone(t: u8) -> Tone {
    match t {
        1 => Tone::Ok,
        2 => Tone::Accent,
        3 => Tone::Warn,
        4 => Tone::Junk,
        _ => Tone::Dim,
    }
}

impl UsbTab {
    pub fn new() -> Self {
        UsbTab { u: Usb::new(), clock: Instant::now(), opened: AtomicBool::new(false) }
    }

    /// Hero: the tier your controller and headset sit on, plus the small stats beside it.
    fn hero(m: &Map) -> Value {
        let key = key_devices(m);
        let hero = if key.is_empty() {
            json!({ "value": "-", "unit": "no controller or headset on USB", "caption": "A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.", "tone": Tone::Dim })
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
            json!({ "value": value, "unit": who, "caption": caption, "tone": tier_tone(worst) })
        };
        let mut stats: Vec<Value> = key.iter().take(2).map(|d| json!({ "value": format!("Tier {}", d.tier), "label": shorten(&d.name, 24), "tone": tier_tone(d.tier) })).collect();
        stats.push(json!({ "value": m.controllers.iter().filter(|c| !is_router(c)).count().to_string(), "label": "USB controllers", "tone": Tone::Text }));
        stats.push(json!({ "value": m.devices.len().to_string(), "label": "USB devices", "tone": Tone::Text }));
        json!({ "hero": hero, "stats": stats })
    }

    fn map(m: &Map, can: bool) -> Value {
        let key: Vec<Value> = key_devices(m).iter().map(|d| json!({ "tier": format!("Tier {}", d.tier), "tone": tier_tone(d.tier), "name": d.name, "verdict": verdict(d) })).collect();
        let controllers: Vec<Value> = m
            .controllers
            .iter()
            .map(|c| {
                let n = m.devices.iter().filter(|d| d.ctl_id.eq_ignore_ascii_case(&c.id)).count();
                let router = is_router(c);
                let note = if router {
                    "USB4 router - devices on these ports appear under the add-on chip's USB controller".to_string()
                } else if n == 1 {
                    "1 device".to_string()
                } else {
                    format!("{n} devices")
                };
                let why_not = why_not_disable(m, c);
                json!({
                    "id": c.id,
                    "tier": tier_short(c.tier),
                    "tone": tier_tone(c.tier),
                    "name": c.name,
                    "router": router,
                    "note": if c.disabled { "disabled".to_string() } else { note },
                    "empty": n == 0,
                    "disabled": c.disabled,
                    "canDisable": can && why_not.is_none(),
                    "canEnable": can && c.disabled,
                    "whyNot": if c.disabled { None } else { why_not },
                    "hover": format!("{}\n{}", c.id, c.path),
                })
            })
            .collect();
        let devices: Vec<Value> = m
            .devices
            .iter()
            .map(|d| {
                json!({
                    "tier": format!("Tier {}", d.tier),
                    "tone": tier_tone(d.tier),
                    "name": d.name,
                    "kind": d.kind.to_lowercase(),
                    "keyKind": d.kind == "GAME CONTROLLER" || d.kind == "AUDIO",
                    "hubs": if d.hubs > 0 { Some(format!("via {} hub(s)", d.hubs)) } else { None },
                    "onboard": d.onboard,
                    "where": format!("{}  ·  {}", d.controller, d.port),
                    "hover": format!("{}\n{}", d.id, d.functions),
                })
            })
            .collect();
        json!({ "time": m.time, "key": key, "controllers": controllers, "devices": devices })
    }
}

impl Backend for UsbTab {
    fn pump(&mut self) {
        // starts the first scan and keeps the plug-in test running, also while the page is not shown
        if self.opened.load(Ordering::Relaxed) {
            self.u.tick(self.clock.elapsed().as_secs_f64());
        }
    }

    fn view(&self) -> Value {
        self.opened.store(true, Ordering::Relaxed);
        let u = &self.u;
        let now = self.clock.elapsed().as_secs_f64();
        let can = pctuner_core::common::is_admin() && u.change.is_none() && u.map_job.is_none();
        let watch = u.watch.as_ref().map(|w| {
            if !w.seeded {
                "Taking a snapshot of the connected devices..."
            } else if w.is_reading() {
                "New device found - reading its port... keep it plugged in"
            } else {
                "Watching for new devices..."
            }
        });
        // newest first; a fresh hit is highlighted in its tier colour for a moment
        let log: Vec<Value> = u
            .log
            .iter()
            .rev()
            .map(|l| {
                let hub = if l.hubs > 0 { format!("  ·  through {} hub(s) - a front-panel or monitor port", l.hubs) } else { String::new() };
                json!({
                    "n": l.n,
                    "name": l.name,
                    "tier": format!("this port = {}", tier_text(l.tier)),
                    "tone": tier_tone(l.tier),
                    "detail": format!("{}  ·  {}{}", l.controller, l.port, hub),
                    "fresh": now - l.at < HIT_GLOW,
                })
            })
            .collect();
        json!({
            "scanning": u.map_job.is_some(),
            // live elapsed seconds, so the hint can never be wrong (the read usually takes 2-4 s)
            "scanSecs": u.scan_started.map(|t| t.elapsed().as_secs()).unwrap_or(0),
            "error": u.error,
            "summary": u.map.as_ref().map(Self::hero),
            "map": u.map.as_ref().map(|m| Self::map(m, can)),
            "switching": u.change.as_ref().map(|(name, enable, _)| format!("{} {name}...", if *enable { "Enabling" } else { "Disabling" })),
            "switchNote": u.change_note.as_ref().map(|(ok, text)| json!({ "ok": ok, "text": text })),
            "admin": pctuner_core::common::is_admin(),
            "watch": watch,
            "tested": if u.log.is_empty() { None } else { Some(format!("{} port(s) tested", u.log.len())) },
            "watchError": u.watch_error,
            "log": log,
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        match parse::<Action>(action)? {
            Action::Rescan => {
                if self.u.map_job.is_none() {
                    self.u.start_scan();
                }
            }
            Action::StartWatch => {
                if self.u.watch.is_none() {
                    self.u.start_watch();
                }
            }
            Action::StopWatch => self.u.stop_watch(),
            Action::Controller { id, enable } => {
                if let Err(e) = self.u.switch_controller(&id, enable) {
                    self.u.change_note = Some((false, e));
                }
            }
            Action::DismissNote => self.u.change_note = None,
        }
        Ok(Value::Null)
    }
}
