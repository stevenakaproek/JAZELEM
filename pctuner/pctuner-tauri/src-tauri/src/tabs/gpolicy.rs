//! Group Policy: the curated policy list, row selection, right-click choices, apply and undo.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::gpolicy::{Gpolicy, RowAct, Scope, St, GROUPS, POLICIES};
use pctuner_core::opwin::OpEvent;
use serde_json::{json, Value};

pub struct GpolicyTab {
    g: Gpolicy,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Choice {
    Enabled,
    Disabled,
    NotConfigured,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    /// A click on a row: select / unselect it.
    Toggle { index: usize },
    /// A right-click choice (goes to every selected row when the clicked row is one of them).
    Set { index: usize, state: Choice },
    /// Right-click "Keep as it is (no change)".
    Keep { index: usize },
    /// The row's own "Reset" button.
    Reset { index: usize },
    /// A drop-down of an enabled policy.
    Opt { index: usize, opt: usize, value: u32 },
    Search { text: String },
    SelectRecommended,
    Apply,
    Discard,
    Undo,
    Refresh,
    Gpedit,
    ClearSelection,
    Op { ev: OpEvent },
}

impl GpolicyTab {
    pub fn new() -> Self {
        GpolicyTab { g: Gpolicy::new() }
    }

    fn loaded(&self) -> bool {
        !self.g.states.is_empty()
    }

    fn row(&self, i: usize) -> Value {
        let g = &self.g;
        let p = &POLICIES[i];
        let cur = g.states.get(i).cloned().unwrap_or(St::NotConfigured);
        let tgt = g.target(i);
        let changed = g.want[i].is_some();
        let picked = g.picked.contains(&i);
        // Enabled shows its drop-downs (the values it is enabled with)
        let opts: Vec<Value> = match &tgt {
            St::Enabled(vals) => {
                let vals = if vals.len() == p.opts.len() { vals.clone() } else { p.default_opts() };
                p.opts
                    .iter()
                    .zip(vals)
                    .map(|(o, v)| {
                        let choices: Vec<Value> = o.choices.iter().map(|c| json!({ "label": c.label, "value": c.value })).collect();
                        json!({ "label": o.label, "value": v, "known": o.choices.iter().any(|c| c.value == v), "choices": choices })
                    })
                    .collect()
            }
            _ => vec![],
        };
        let in_reg = g.reg.get(i).cloned().unwrap_or(St::NotConfigured);
        let note = if changed {
            Value::Null
        } else if cur == St::NotConfigured && in_reg != St::NotConfigured {
            json!({
                "text": format!("registry: {}", p.label(&in_reg)),
                "color": "",
                "tip": "Not set in Group Policy, but this value is already in the registry (set by a tweak or another tool). Setting it here makes it permanent and visible in gpedit.",
            })
        } else if !g.live.get(i).copied().unwrap_or(true) && cur != St::NotConfigured {
            json!({
                "text": "not applied yet",
                "color": "orange",
                "tip": "Saved in Group Policy, but Windows has not applied it yet. Restart, or Apply again.",
            })
        } else {
            Value::Null
        };
        let rec = match p.rec_state() {
            Some(r) if r == tgt => json!({ "text": "recommended", "color": "accent" }),
            Some(r) => json!({ "text": format!("recommended: {}", p.label(&r)), "color": "" }),
            None => json!({ "text": "your choice", "color": "" }),
        };
        json!({
            "index": i,
            "name": p.name,
            "advice": p.advice,
            "tip": format!("gpedit: {}\n{} > \"{}\"", if p.scope == Scope::User { "User Configuration" } else { "Computer Configuration" }, p.path, p.title),
            "picked": picked,
            "changed": changed,
            "current": p.label(&cur),
            "currentColor": match cur {
                St::NotConfigured => "",
                St::Other => "yellow",
                _ => "green",
            },
            "target": p.label(&tgt),
            "note": note,
            "rec": rec,
            "opts": opts,
            "menuTitle": if picked && g.picked.len() > 1 { format!("{} selected policies", g.picked.len()) } else { p.name.to_string() },
        })
    }
}

impl Backend for GpolicyTab {
    fn pump(&mut self) {
        self.g.pump();
    }

    fn busy(&self) -> bool {
        self.g.job.is_some()
    }

    fn view(&self) -> Value {
        let g = &self.g;
        let busy = g.busy_job();
        let can = g.can_act();
        let loaded = self.loaded();
        let recs: Vec<usize> = (0..POLICIES.len()).filter(|&i| POLICIES[i].rec_state().is_some()).collect();
        let at_rec = recs.iter().filter(|&&i| g.states.get(i) == POLICIES[i].rec_state().as_ref()).count();
        let n_changes = g.changes().len();
        let has_undo = g.history.iter().any(|b| !b.undone);
        let q = g.search.trim().to_lowercase();
        let mut groups: Vec<Value> = vec![];
        if loaded {
            for group in GROUPS {
                let rows: Vec<Value> = (0..POLICIES.len())
                    .filter(|&i| POLICIES[i].group == *group)
                    .filter(|&i| {
                        let p = &POLICIES[i];
                        q.is_empty() || [p.name, p.title, p.advice].iter().any(|s| s.to_lowercase().contains(&q))
                    })
                    .map(|i| self.row(i))
                    .collect();
                if !rows.is_empty() {
                    groups.push(json!({ "name": group, "rows": rows }));
                }
            }
        }
        json!({
            "loaded": loaded,
            "gpo": g.gpo,
            "edition": g.edition,
            "loadError": g.load_error,
            "atRec": at_rec,
            "recs": recs.len(),
            "changes": n_changes,
            "canEdit": !busy,
            "canApply": can && n_changes > 0,
            "canSelect": loaded && !busy,
            "canDiscard": n_changes > 0 && !busy,
            "canUndo": can && has_undo,
            "canRefresh": !busy && g.loader.is_none(),
            "working": busy || g.loader.is_some(),
            "picked": g.picked.len(),
            "lastAction": g.last_action,
            "search": g.search,
            "groups": groups,
            "op": json(&g.op),
            "opCan": can,
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let busy = self.g.busy_job();
        let can = self.g.can_act();
        let loaded = self.loaded();
        let n = POLICIES.len();
        match parse::<Action>(action)? {
            Action::Toggle { index } => {
                if loaded && !busy && index < n {
                    self.g.row_action(index, RowAct::Toggle);
                }
            }
            Action::Set { index, state } => {
                if loaded && !busy && index < n {
                    let s = match state {
                        Choice::Enabled => St::Enabled(vec![]),
                        Choice::Disabled => St::Disabled,
                        Choice::NotConfigured => St::NotConfigured,
                    };
                    self.g.row_action(index, RowAct::Set(s));
                }
            }
            Action::Keep { index } => {
                if loaded && !busy && index < n {
                    self.g.row_action(index, RowAct::Reset);
                }
            }
            Action::Reset { index } => {
                if index < n {
                    self.g.want[index] = None;
                }
            }
            Action::Opt { index, opt, value } => {
                if loaded && !busy && index < n {
                    let p = &POLICIES[index];
                    // only an Enabled policy shows drop-downs, and only its own choices can be picked
                    if let St::Enabled(vals) = self.g.target(index) {
                        let mut v = if vals.len() == p.opts.len() { vals } else { p.default_opts() };
                        if p.opts.get(opt).is_some_and(|o| o.choices.iter().any(|c| c.value == value)) {
                            v[opt] = value;
                            let s = St::Enabled(v);
                            let cur = self.g.states.get(index).cloned().unwrap_or(St::NotConfigured);
                            self.g.want[index] = if s == cur { None } else { Some(s) };
                        }
                    }
                }
            }
            Action::Search { text } => self.g.search = text,
            Action::SelectRecommended => {
                if loaded && !busy {
                    self.g.select_recommended();
                }
            }
            Action::Apply => {
                if can && loaded && !self.g.changes().is_empty() && self.g.op.is_none() {
                    self.g.open_apply();
                }
            }
            Action::Discard => {
                if !busy && !self.g.changes().is_empty() {
                    self.g.discard();
                }
            }
            Action::Undo => {
                if can && self.g.op.is_none() && self.g.history.iter().any(|b| !b.undone) {
                    self.g.open_undo();
                }
            }
            Action::Refresh => {
                if !busy && self.g.loader.is_none() {
                    self.g.reload();
                }
            }
            Action::Gpedit => {
                if self.g.gpo {
                    let _ = std::process::Command::new("mmc.exe").arg("gpedit.msc").spawn();
                }
            }
            Action::ClearSelection => self.g.picked.clear(),
            Action::Op { ev } => {
                // the page disables these without `can`; never act on a stale click
                if !can && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                // "Go" only means something while a confirmed change is waiting
                if ev == OpEvent::Go && self.g.pending.is_none() {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.g.op, ev);
                self.g.on_event(ev);
            }
        }
        Ok(Value::Null)
    }
}
