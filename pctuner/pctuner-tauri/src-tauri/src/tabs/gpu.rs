//! RTX 5080 Profiles: which preset is applied (with live graphs), the four presets, the safety trial.
//! All logic is pctuner_core::gpu; this backend builds the view and forwards the clicks.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::common::Tone;
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
    /// A preset card was clicked.
    Select { index: usize },
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

    /// "Currently applied": the preset, the chips and the live graphs.
    fn hero(&self, current: Option<usize>) -> Value {
        let Some(s) = &self.g.state else { return Value::Null };
        let (name, unit, caption, tone) = hero_text(s.cfg.is_some(), s.saved, current);
        // one reading for the hero, the live-check chip and the memory chip, so they always agree
        let live = self.g.live_shown();
        let mut chips: Vec<Value> = vec![];
        let (text, c) = live_chip(live);
        chips.push(json!({ "text": text, "tone": c }));
        if s.ab_running {
            chips.push(json!({ "text": "Afterburner running", "tone": Tone::Ok }));
        } else {
            chips.push(json!({ "text": "Afterburner not running - presets only hold while it runs", "tone": Tone::Junk }));
        }
        if let Some(l) = live.filter(|l| l.full_clocks) {
            chips.push(json!({ "text": format!("Memory {} MHz ({:+} over stock {})", l.mem, l.mem_offset, l.mem_stock), "tone": Tone::Dim }));
        }
        let no_cfg = s.cfg.is_none().then(|| format!("No RTX 5080 profile file (VEN_10DE&DEV_2C02*.cfg) in {}\\Profiles.", ab_dir().display()));
        let series = |f: &dyn Fn(&Live) -> f64| -> Vec<f64> { self.g.history.iter().map(f).collect() };
        let live = live.map(|l| {
            json!({
                "core": l.core,
                "temp": l.temp,
                "power": l.power,
                "util": l.util,
                "coreHist": series(&|l| l.core as f64),
                "tempHist": series(&|l| l.temp as f64),
                "powerHist": series(&|l| l.power),
                "utilHist": series(&|l| l.util as f64),
            })
        });
        json!({ "name": name, "unit": unit, "caption": caption, "tone": tone, "chips": chips, "noCfg": no_cfg, "live": live })
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
        let current = st.and_then(|s| current_preset(s.saved, g.live_shown()));
        let presets: Vec<Value> = PRESETS
            .iter()
            .enumerate()
            .map(|(i, p)| {
                json!({
                    "index": i,
                    "name": p.name,
                    "slot": p.slot,
                    "text": p.text,
                    "selected": g.selected == Some(i),
                    "current": current == Some(i),
                    "needsWrite": st.map(|s| s.cfg.is_some() && !s.slot_ok[i]).unwrap_or(false),
                })
            })
            .collect();
        let apply_label = match g.selected {
            Some(i) => format!("Apply {}", PRESETS[i].name),
            None => "Apply preset".into(),
        };
        json!({
            "trial": self.trial(),
            "hero": self.hero(current),
            "canReset": g.work.is_none(),
            "presets": presets,
            "applyLabel": apply_label,
            "canApply": !busy && g.selected.is_some(),
            "canRestore": !busy,
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
                    Click::Restore => !g.busy(),
                    Click::Refresh => g.refresh.is_none(),
                    Click::Keep => g.can_act(),
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
                if index < PRESETS.len() {
                    self.g.selected = Some(index);
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
