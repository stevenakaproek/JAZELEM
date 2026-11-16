//! RAM LED: switch the lights on G.SKILL (ENE controller) memory off / on, or set a colour.

use crate::backend::{parse, Backend};
use pctuner_core::common::{self, Tone};
use pctuner_core::ramled::*;
use serde_json::{json, Value};

pub struct RamledTab {
    r: RamLed,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Rescan,
    Off,
    On,
    Rainbow,
    /// The colour picker moved: "#RRGGBB".
    Pick { color: String },
    SetColor,
    Remember { on: bool },
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

impl RamledTab {
    pub fn new() -> Self {
        RamledTab { r: RamLed::new() }
    }
}

impl Backend for RamledTab {
    fn pump(&mut self) {
        self.r.pump();
    }

    fn busy(&self) -> bool {
        self.r.busy()
    }

    fn view(&self) -> Value {
        let r = &self.r;
        let admin = common::is_admin();
        let busy = r.job.is_some();
        let found = !r.sticks.is_empty();
        let lit = r.sticks.iter().any(is_lit);
        let (value, caption, tone) = if !admin {
            ("-", "Start PC Tuner as administrator to read the RAM LEDs.", Tone::Faint)
        } else if !r.scanned || (busy && !found) {
            ("-", "Looking for RAM LED controllers...", Tone::Faint)
        } else if !found {
            ("-", "No supported RAM LED controller found.", Tone::Faint)
        } else if lit {
            ("On", "The RAM LEDs are lit.", Tone::Accent)
        } else {
            ("Off", "The RAM LEDs are dark.", Tone::Dim)
        };
        let can = admin && !busy && found;
        let color_ok = r.sticks.iter().all(|s| s.color_ok);
        let [cr, cg, cb] = r.color;
        json!({
            "hero": { "value": value, "caption": caption, "tone": tone },
            "busy": busy,
            "found": found,
            "lit": lit,
            "count": r.sticks.len(),
            "canRescan": admin && !busy,
            "error": r.error,
            "can": can,
            "canColor": can && color_ok,
            "colorUnsupported": found && !color_ok,
            "color": format!("#{cr:02x}{cg:02x}{cb:02x}"),
            "remember": !r.saved.no_save,
            "status": r.status,
            "sticks": r.sticks.iter().enumerate().map(|(i, s)| json!({
                "title": format!("Stick {}", i + 1),
                "state": describe(s),
                "lit": is_lit(s),
                "detail": format!("{} LEDs  ·  controller {}  ·  SMBus address 0x{:02X}", s.leds, s.name, s.addr),
            })).collect::<Vec<Value>>(),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let r = &mut self.r;
        let admin = common::is_admin();
        let busy = r.job.is_some();
        let can = admin && !busy && !r.sticks.is_empty();
        let color_ok = r.sticks.iter().all(|s| s.color_ok);
        match parse::<Action>(action)? {
            Action::Rescan => {
                if admin && !busy {
                    r.start(vec!["detect".into()], "");
                }
            }
            Action::Off => {
                if can {
                    r.start(vec!["off".into()], "Turning the RAM LEDs off...");
                }
            }
            Action::On => {
                if can {
                    let args = r.on_args();
                    r.start(args, "Turning the RAM LEDs on...");
                }
            }
            Action::Rainbow => {
                if can {
                    r.start(vec!["rainbow".into()], "Setting rainbow...");
                }
            }
            Action::Pick { color } => {
                if let Some(c) = parse_color(&color).filter(|_| can && color_ok) {
                    r.color = c;
                    r.saved.color = Some(c);
                }
            }
            Action::SetColor => {
                if can && color_ok {
                    let [cr, cg, cb] = r.color;
                    r.save_settings();
                    r.start(vec!["static".into(), format!("{cr:02X}{cg:02X}{cb:02X}")], "Setting the colour...");
                }
            }
            Action::Remember { on } => {
                r.saved.no_save = !on;
                r.save_settings();
            }
        }
        Ok(Value::Null)
    }
}
