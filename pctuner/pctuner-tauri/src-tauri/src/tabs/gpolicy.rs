//! Group Policy: the curated policy list, each row's Not configured / Enabled / Disabled choice, apply and undo.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::gpolicy::{Gpolicy, Scope, St, GROUPS, POLICIES};
use pctuner_core::opwin::OpEvent;
use super::backups::{self, Backups};
use pctuner_core::common;
use pctuner_core::tabbackup::Item;
use serde_json::{json, Value};

pub struct GpolicyTab {
    g: Gpolicy,
    bk: Backups,
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
    /// The row's own Not configured / Enabled / Disabled control (that policy only).
    Set { index: usize, state: Choice },
    /// A click on the row's "recommended: ..." label: choose the recommended setting for that policy.
    Recommend { index: usize },
    /// The row's own "Reset" button.
    Reset { index: usize },
    /// A drop-down of an enabled policy.
    Opt { index: usize, opt: usize, value: u32 },
    Search { text: String },
    Apply,
    Discard,
    Undo,
    Refresh,
    Gpedit,
    Op { ev: OpEvent },
}

impl GpolicyTab {
    pub fn new() -> Self {
        GpolicyTab { g: Gpolicy::new(), bk: Backups::new("Group Policy", ABOUT) }
    }

    /// The Backups sheet (`can` = a change could be started now).
    fn bk_view(&self, can: bool) -> Value {
        let n = self.now();
        self.bk.view(n.as_ref().map(|x| x.1.as_slice()), can)
    }

    /// The settings for a backup (None until the policies are read).
    fn now(&self) -> Option<(String, Vec<Item>)> {
        self.loaded().then(|| (String::new(), self.g.backup_items()))
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
            Some(r) => json!({ "text": format!("recommended: {}", p.label(&r)), "color": "", "can": true }),
            None => json!({ "text": "your choice", "color": "" }),
        };
        json!({
            "index": i,
            "name": p.name,
            "advice": p.advice,
            "tip": format!("gpedit: {}\n{} > \"{}\"", if p.scope == Scope::User { "User Configuration" } else { "Computer Configuration" }, p.path, p.title),
            "changed": changed,
            "state": match &tgt {
                St::NotConfigured => "not_configured",
                St::Enabled(_) => "enabled",
                St::Disabled => "disabled",
                St::Other => "other",
            },
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
        let configured = g.states.iter().filter(|s| **s != St::NotConfigured).count();
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
            "configured": configured,
            "total": POLICIES.len(),
            "changes": n_changes,
            "canEdit": !busy,
            "canApply": can && n_changes > 0,
            "canDiscard": n_changes > 0 && !busy,
            "canUndo": can && has_undo,
            "canRefresh": !busy && g.loader.is_none(),
            "working": busy || g.loader.is_some(),
            "lastAction": g.last_action,
            "search": g.search,
            "groups": groups,
            "op": json(&g.op),
            "opCan": can,
            "backups": self.bk_view(can && g.op.is_none()),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        if let Some(a) = backups::action(&action) {
            let now = self.now();
            if let Some(b) = self.bk.act(a, now) {
                let r = if common::is_admin() { self.g.open_restore(&b) } else { Err("Run PC Tuner as administrator to restore a backup.".to_string()) };
                self.bk.restored(r);
            }
            return Ok(Value::Null);
        }
        let busy = self.g.busy_job();
        let can = self.g.can_act();
        let loaded = self.loaded();
        let n = POLICIES.len();
        match parse::<Action>(action)? {
            Action::Set { index, state } => {
                if loaded && !busy && index < n {
                    let s = match state {
                        Choice::Enabled => St::Enabled(vec![]),
                        Choice::Disabled => St::Disabled,
                        Choice::NotConfigured => St::NotConfigured,
                    };
                    self.g.set_target(index, s);
                }
            }
            Action::Recommend { index } => {
                if loaded && !busy && index < n {
                    let p = &POLICIES[index];
                    match p.rec_state() {
                        // a recommendation with its own drop-down values is taken exactly
                        Some(St::Enabled(v)) if !v.is_empty() && v.len() == p.opts.len() => {
                            let s = St::Enabled(v);
                            let cur = self.g.states.get(index).cloned().unwrap_or(St::NotConfigured);
                            self.g.want[index] = if s == cur { None } else { Some(s) };
                        }
                        Some(s) => self.g.set_target(index, s),
                        None => {}
                    }
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

const ABOUT: &str = "A backup saves the setting of every policy in this list. Restore puts back only the policies that differ, with the usual Apply preview, and can be undone.";
