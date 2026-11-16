//! Remove Bloat: tick apps by hand, one button removes them; the results live in the operation window.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::bloat::{friendly, is_app, Bloat, TARGETS};
use pctuner_core::common;
use pctuner_core::opwin::OpEvent;
use serde_json::{json, Value};

pub struct BloatTab {
    b: Bloat,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Toggle { index: usize },
    Clear,
    Remove,
    Rescan,
    Op { ev: OpEvent },
    CloseLog,
}

impl BloatTab {
    pub fn new() -> Self {
        BloatTab { b: Bloat::new() }
    }
}

impl Backend for BloatTab {
    fn pump(&mut self) {
        self.b.pump();
    }

    fn busy(&self) -> bool {
        self.b.running()
    }

    fn view(&self) -> Value {
        let b = &self.b;
        let running = b.running();
        let admin = common::is_admin();
        let n_sel = b.selected().len();
        let can_remove = admin && !running && n_sel > 0;
        let rows: Vec<Value> = TARGETS
            .iter()
            .enumerate()
            .map(|(i, t)| {
                json!({
                    "index": i,
                    "name": friendly(t.name),
                    "tip": format!("{}\nYou lose: {}\nTicked = removed by Remove selected", t.name.trim_end_matches(['~', '.']), t.loses),
                    "optional": t.optional,
                    "installed": b.is_installed(t),
                    "picked": b.pick[i],
                })
            })
            .collect();
        let apps = TARGETS.iter().filter(|t| is_app(t)).count();
        let gone = b.installed.as_ref().map(|list| TARGETS.iter().filter(|t| is_app(t) && !list.iter().any(|n| n == t.name)).count());
        json!({
            "scanning": b.installed.is_none(),
            "apps": apps,
            "gone": gone,
            "rows": rows,
            "selected": n_sel,
            "canRemove": can_remove,
            "removeTip": if can_remove {
                "Removes the ticked apps. Shows the list to confirm first."
            } else if admin {
                "Tick the apps to remove in the list below."
            } else {
                "Run PC Tuner as administrator to remove apps."
            },
            "canClear": !running && n_sel > 0,
            "canRescan": !running,
            "op": json(&b.op),
            "opCan": b.can_act(),
            "log": b.log,
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let running = self.b.running();
        match parse::<Action>(action)? {
            Action::Toggle { index } => {
                if let Some(p) = self.b.pick.get_mut(index) {
                    *p = !*p;
                }
            }
            Action::Clear => {
                if !running {
                    self.b.pick = vec![false; TARGETS.len()];
                }
            }
            Action::Remove => {
                if common::is_admin() && !running && !self.b.selected().is_empty() && self.b.op.is_none() {
                    self.b.open_remove();
                }
            }
            Action::Rescan => {
                if !running {
                    self.b.installed = None;
                }
            }
            Action::Op { ev } => {
                // the page disables these without `can`; never act on a stale click
                if !self.b.can_act() && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                // "Go" only means something while a confirmed run is waiting
                if ev == OpEvent::Go && self.b.pending.is_none() {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.b.op, ev);
                self.b.on_event(ev);
            }
            Action::CloseLog => self.b.log = None,
        }
        Ok(Value::Null)
    }
}
