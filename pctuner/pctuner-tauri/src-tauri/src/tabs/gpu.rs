//! RTX 5080 Profiles: which tune is applied (with live graphs), the four presets, custom values and the
//! safety trial. All logic is pctuner_core::gpu; this backend builds the view and forwards the clicks.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::gpu::*;
use pctuner_core::opwin::OpEvent;
use serde_json::{json, Value};

pub struct GpuTab {
    g: Gpu,
    /// The page is showing this tab (the live readout only samples the card then, like the classic app).
    shown: bool,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    /// A button of the page (the core's own `Click`).
    Click { click: Click },
    /// A preset card (0-3) or the Custom card (4) was clicked.
    Select { index: usize },
    /// A custom value was changed.
    Custom { field: Field, value: i64 },
    /// The power slider was let go: set only the power limit.
    Power { pct: u32 },
    /// "OK" in the message window.
    DismissError,
    Op { ev: OpEvent },
}

impl GpuTab {
    pub fn new() -> Self {
        GpuTab { g: Gpu::new(), shown: false }
    }

    /// The trial / crash notice at the top of the tab.
    fn trial(&self) -> Value {
        let Some(t) = &self.g.trial else { return Value::Null };
        if t.pending() {
            json!({
                "kind": "pending",
                "preset": t.preset,
                "previous": t.previous,
                "appliedAt": hhmm(t.applied_at),
                "unguarded": t.unguarded,
                "canKeep": self.g.can_act(),
                "canRevert": self.g.work.is_none(),
            })
        } else if t.status == "reverted" {
            json!({
                "kind": "reverted",
                "preset": t.preset,
                "text": reverted_text(t),
                "reason": t.reason,
                "lastError": if !t.verified { t.last_error.as_str() } else { "" },
            })
        } else {
            Value::Null
        }
    }

    /// "Currently applied": the tune, the chips and the live graphs.
    fn hero(&self, current: Option<usize>) -> Value {
        let Some(s) = &self.g.state else { return Value::Null };
        // one reading for the hero and the chips, so they always agree
        let live = self.g.live_shown();
        let (name, unit, caption, tone) = hero_text(current, &s.saved, live);
        let mut chips: Vec<Value> = vec![];
        let (text, c) = live_chip(live, s.saved.factor());
        chips.push(json!({ "text": text, "tone": c }));
        if let Some(l) = live.filter(|l| l.full_clocks) {
            chips.push(json!({ "text": format!("Memory {} MHz ({:+} over stock {})", l.mem, l.mem_offset, l.mem_stock), "tone": pctuner_core::common::Tone::Dim }));
        }
        let series = |f: &dyn Fn(&Live) -> f64| -> Vec<f64> { self.g.history.iter().map(f).collect() };
        // every reading the page can show as a tile (the page keeps which ones and their order)
        let live = live.map(|l| {
            let tile = |key: &str, label: &str, unit: &str, value: Option<f64>, sub: Option<String>, f: &dyn Fn(&Live) -> f64| {
                json!({ "key": key, "label": label, "unit": unit, "value": value, "sub": sub, "hist": series(f) })
            };
            json!([
                tile("core", "Core clock", "MHz", Some(l.core as f64), None, &|l| l.core as f64),
                tile("temp", "Temperature", "°C", Some(l.temp as f64), None, &|l| l.temp as f64),
                tile("power", "Power draw", "W", Some(l.power), None, &|l| l.power),
                tile("util", "GPU load", "%", Some(l.util as f64), None, &|l| l.util as f64),
                tile("volt", "Core voltage", "mV", l.volt_mv, None, &|l| l.volt_mv.unwrap_or(0.0)),
                tile("vram", "Memory used", "MB", Some(l.vram_used_mb as f64), (l.vram_total_mb > 0).then(|| format!("of {} MB", l.vram_total_mb)), &|l| l.vram_used_mb as f64),
                tile("memclock", "Memory clock", "MHz", Some(l.mem as f64), None, &|l| l.mem as f64),
                tile("fan", "Fan speed", "%", Some(l.fan_pct as f64), None, &|l| l.fan_pct as f64),
            ])
        });
        json!({ "name": name, "unit": unit, "caption": caption, "tone": tone, "chips": chips, "error": s.error, "live": live })
    }

    /// The custom values with the card's limits (null until the card has been read).
    fn custom(&self) -> Value {
        let (Some(c), Some(l)) = (self.g.custom, self.g.state.as_ref().and_then(|s| s.limits)) else { return Value::Null };
        json!({
            "maxMhz": c.max_mhz,
            "core": c.core,
            "mem": c.mem,
            "power": c.power,
            "powerWatts": l.power_default_w * c.power / 100,
            "limits": {
                "maxMhz": [l.clock.0, l.clock.1],
                "core": [l.core.0, l.core.1],
                "mem": [l.mem.0, l.mem.1],
                "power": [l.power.0, l.power.1],
            },
            "summary": c.describe(),
        })
    }
}

impl Backend for GpuTab {
    fn pump(&mut self) {
        // also picks up the sidebar's "Reset GPU to stock" request
        // only while the tab is open or a change is running (the sidebar reset opens the tab first)
        if self.shown || self.g.busy() {
            self.g.tick();
        }
    }

    fn shown(&mut self, on: bool) {
        self.shown = on;
    }

    fn busy(&self) -> bool {
        self.g.busy()
    }

    fn view(&self) -> Value {
        let g = &self.g;
        let busy = g.busy();
        let st = g.state.as_ref();
        let current = g.current();
        let lifts = st.map(|s| s.saved.lifts.clone()).unwrap_or_default();
        let presets: Vec<Value> = PRESETS
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // the exact values once the card's curve has been read (at the first apply)
                let detail = if i == 0 {
                    "no offsets, no clock limit".to_string()
                } else if lifts.len() == PRESETS.len() {
                    format!("{} mV · max {} MHz · core {:+} · power {} %", p.target_mv, p.target_mhz, lifts[i], p.power)
                } else {
                    format!("{} mV · max {} MHz · power {} %", p.target_mv, p.target_mhz, p.power)
                };
                json!({
                    "index": i,
                    "name": p.name,
                    "detail": detail,
                    "text": p.text,
                    "selected": g.selected == Some(i),
                    "current": current == Some(i),
                })
            })
            .collect();
        let apply_label = match g.selected {
            Some(i) => format!("Apply {}", tune_name(i)),
            None => "Apply preset".into(),
        };
        json!({
            "trial": self.trial(),
            "hero": self.hero(current),
            "canReset": g.work.is_none(),
            "presets": presets,
            "custom": self.custom(),
            "customSelected": g.selected == Some(CUSTOM),
            "customCurrent": current == Some(CUSTOM),
            "power": st.and_then(|s| s.limits.map(|l| json!({
                "pct": s.power_pct.unwrap_or(100),
                "min": l.power.0,
                "max": l.power.1,
                "defaultW": l.power_default_w,
                "can": g.can_act() && g.chore.is_none(),
            }))),
            "afterburner": {
                "running": st.map(|s| s.ab_running).unwrap_or(false),
                "autostart": st.map(|s| s.ab_autostart).unwrap_or(false),
                "canClose": g.chore.is_none(),
            },
            "applyLabel": apply_label,
            "canApply": !busy && g.selected.is_some() && (g.selected != Some(CUSTOM) || g.custom.is_some()),
            "canEdit": !busy,
            "canRefresh": g.refresh.is_none(),
            "status": g.status,
            "error": g.error,
            "op": json(&g.op),
            "opCan": g.can_act(),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        match parse::<Action>(action)? {
            Action::Click { click } => {
                let g = &self.g;
                // the same buttons the page disables: a stale click never acts
                let allowed = match click {
                    Click::Reset | Click::Revert => g.work.is_none(),
                    Click::Apply => !g.busy() && g.selected.is_some(),
                    Click::Refresh => g.refresh.is_none(),
                    Click::Keep => g.can_act(),
                    Click::CloseAfterburner => g.chore.is_none(),
                    Click::Dismiss => true,
                };
                // the trial buttons only exist while their notice is shown
                let shown = match click {
                    Click::Keep | Click::Revert => g.trial.as_ref().is_some_and(|t| t.pending()),
                    Click::Dismiss => g.trial.as_ref().is_some_and(|t| !t.pending() && t.status == "reverted"),
                    _ => true,
                };
                if allowed && shown {
                    self.g.click(click);
                }
            }
            Action::Select { index } => {
                if index <= CUSTOM && !self.g.busy() {
                    self.g.selected = Some(index);
                }
            }
            Action::Custom { field, value } => {
                if !self.g.busy() {
                    self.g.set_custom(field, value);
                }
            }
            Action::Power { pct } => {
                if !self.g.busy() {
                    self.g.set_power(pct);
                }
            }
            Action::DismissError => self.g.error = None,
            Action::Op { ev } => {
                // the page disables these without `can`; never act on a stale click
                if !self.g.can_act() && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.g.op, ev);
                self.g.on_event(ev);
            }
        }
        Ok(Value::Null)
    }
}
