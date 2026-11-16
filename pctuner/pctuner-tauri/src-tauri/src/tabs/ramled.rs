//! RAM LED: switch the lights on G.SKILL (ENE controller) memory off / on, or set a colour - and the same for
//! the graphics card's LEDs (Gigabyte RTX 5080 XTREME WATERFORCE, ASUS cards), in a second section.

use crate::backend::{parse, Backend};
use pctuner_core::common::{self, Tone};
use pctuner_core::ramled::*;
use serde_json::{json, Value};

pub struct RamledTab {
    ram: RamLed,
    gpu: RamLed,
}

#[derive(serde::Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Which {
    Ram,
    Gpu,
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

#[derive(serde::Deserialize)]
struct Envelope {
    /// which section the button belongs to (the RAM when missing)
    #[serde(default)]
    target: Option<Which>,
    #[serde(flatten)]
    action: Value,
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

/// One section's view (the RAM sticks or the graphics card).
fn section(r: &RamLed) -> Value {
    let admin = common::is_admin();
    let busy = r.job.is_some();
    let found = !r.sticks.is_empty();
    let lit = r.sticks.iter().any(is_lit);
    let unknown = found && r.sticks.iter().all(|s| s.mode == MODE_UNKNOWN);
    let gpu = r.target == Target::Gpu;
    let (value, caption, tone) = if !admin {
        ("-".to_string(), if gpu { "Start PC Tuner as administrator to reach the graphics card's LEDs." } else { "Start PC Tuner as administrator to read the RAM LEDs." }.to_string(), Tone::Faint)
    } else if !r.scanned || (busy && !found) {
        ("-".into(), if gpu { "Looking for the graphics card's LED controller..." } else { "Looking for RAM LED controllers..." }.into(), Tone::Faint)
    } else if !found {
        ("-".into(), if gpu { "No supported graphics card LED controller found." } else { "No supported RAM LED controller found." }.into(), Tone::Faint)
    } else if unknown {
        ("Found".into(), "The controller cannot be read back: the lighting is shown once PC Tuner has set it.".into(), Tone::Dim)
    } else if lit {
        ("On".into(), if gpu { "The graphics card's LEDs are lit." } else { "The RAM LEDs are lit." }.into(), Tone::Accent)
    } else {
        ("Off".into(), if gpu { "The graphics card's LEDs are dark." } else { "The RAM LEDs are dark." }.into(), Tone::Dim)
    };
    let can = admin && !busy && found;
    let color_ok = r.sticks.iter().all(|s| s.color_ok);
    let [cr, cg, cb] = r.color;
    json!({
        "hero": { "value": value, "caption": caption, "tone": tone },
        "busy": busy,
        "found": found,
        "lit": lit && !unknown,
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
            "title": if gpu { "Graphics card".to_string() } else { format!("Stick {}", i + 1) },
            "state": describe(s),
            "lit": is_lit(s) && s.mode != MODE_UNKNOWN,
            "detail": if gpu {
                format!("{} zones  ·  {}  ·  I2C address 0x{:02X}", s.leds, s.name, s.addr)
            } else {
                format!("{} LEDs  ·  controller {}  ·  SMBus address 0x{:02X}", s.leds, s.name, s.addr)
            },
        })).collect::<Vec<Value>>(),
    })
}

fn act_on(r: &mut RamLed, action: Action) {
    let admin = common::is_admin();
    let busy = r.job.is_some();
    let can = admin && !busy && !r.sticks.is_empty();
    let color_ok = r.sticks.iter().all(|s| s.color_ok);
    let what = if r.target == Target::Gpu { "graphics card LEDs" } else { "RAM LEDs" };
    match action {
        Action::Rescan => {
            if admin && !busy {
                r.start(vec!["detect".into()], "");
            }
        }
        Action::Off => {
            if can {
                r.start(vec!["off".into()], &format!("Turning the {what} off..."));
            }
        }
        Action::On => {
            if can {
                let args = r.on_args();
                r.start(args, &format!("Turning the {what} on..."));
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
}

impl RamledTab {
    pub fn new() -> Self {
        RamledTab { ram: RamLed::for_target(Target::Ram), gpu: RamLed::for_target(Target::Gpu) }
    }
}

impl Backend for RamledTab {
    fn pump(&mut self) {
        self.ram.pump();
        // one helper at a time: the graphics card is looked at once the RAM scan is done
        if self.ram.scanned && self.ram.job.is_none() {
            self.gpu.pump();
        }
    }

    fn busy(&self) -> bool {
        self.ram.busy() || self.gpu.busy()
    }

    fn view(&self) -> Value {
        let mut v = section(&self.ram);
        v["gpu"] = section(&self.gpu);
        v
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let e: Envelope = parse(action)?;
        let a: Action = parse(e.action)?;
        // never two helper calls at once (both may use the hardware buses)
        if self.ram.job.is_some() || self.gpu.job.is_some() {
            if let Action::Pick { .. } | Action::Remember { .. } = a {
            } else {
                return Ok(Value::Null);
            }
        }
        match e.target.unwrap_or(Which::Ram) {
            Which::Ram => act_on(&mut self.ram, a),
            Which::Gpu => act_on(&mut self.gpu, a),
        }
        Ok(Value::Null)
    }
}
