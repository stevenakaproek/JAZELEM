//! Optimization > Performance: the settings list and the Defender game-folder card.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::common;
use pctuner_core::defender::Defender;
use pctuner_core::opwin::OpEvent;
use pctuner_core::perf::{fmt, selected_changes, Kind, Perf, TWEAKS};
use super::backups::{self, Backups};
use pctuner_core::tabbackup::Item;
use serde_json::{json, Value};

pub struct PerfTab {
    p: Perf,
    d: Defender,
    bk: Backups,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Toggle { index: usize },
    /// The row's on / off switch (opens the preview for that one setting).
    Switch { index: usize, on: bool },
    Clear,
    Apply,
    Undo,
    Explorer,
    Refresh,
    Op { ev: OpEvent },
    DefenderTick { path: String, on: bool },
    DefenderAdd { path: String },
    DefenderExclude,
    DefenderUndo,
    DefenderRescan,
    DefenderOp { ev: OpEvent },
}

impl PerfTab {
    pub fn new() -> Self {
        PerfTab { p: Perf::new(), d: Defender::new(), bk: Backups::new("Performance", ABOUT) }
    }

    /// The Backups sheet (`can` = a change could be started now).
    fn bk_view(&self, can: bool) -> Value {
        let n = self.now();
        self.bk.view(n.as_ref().map(|x| x.1.as_slice()), can)
    }

    /// The settings for a backup (None until they are read).
    fn now(&self) -> Option<(String, Vec<Item>)> {
        let s = self.p.state.as_ref().filter(|s| s.error.is_none())?;
        Some((String::new(), pctuner_core::perf::backup_items(s)))
    }

    fn settings(&self) -> Value {
        let Some(state) = &self.p.state else { return Value::Null };
        if state.error.is_some() {
            return Value::Null;
        }
        let mut groups: Vec<Value> = vec![];
        let mut rows: Vec<Value> = vec![];
        let mut group = "";
        for (i, t) in TWEAKS.iter().enumerate() {
            if t.group != group {
                if !rows.is_empty() {
                    groups.push(json!({ "name": group, "rows": std::mem::take(&mut rows) }));
                }
                group = t.group;
            }
            let applicable = state.applicable(t.kind);
            let set = state.is_set(t);
            // an applied setting has nothing to apply, so it cannot be selected
            let selectable = applicable && !set && self.p.run.is_none();
            let extra = match (t.kind, state.hiberfil_gb) {
                (Kind::Hibernate, Some(gb)) => Some(format!("hiberfil.sys is {gb:.1} GB now")),
                _ => None,
            };
            rows.push(json!({
                "index": i,
                "name": t.name,
                "why": t.why,
                "optional": t.optional,
                "applicable": applicable,
                "blocked": if applicable { "" } else { state.blocked_reason(t.kind) },
                "set": set,
                "selectable": selectable,
                "selected": self.p.selected[i] && selectable,
                // the switch: on = the tweak is in place
                "canSwitch": applicable && common::is_admin() && self.p.run.is_none() && self.p.op.is_none() && self.p.loading.is_none(),
                "now": fmt(t.kind, state.value(t.kind).as_deref()),
                "target": fmt(t.kind, Some(t.target)),
                "extra": extra,
            }));
        }
        if !rows.is_empty() {
            groups.push(json!({ "name": group, "rows": rows }));
        }
        Value::Array(groups)
    }

    fn defender(&self) -> Value {
        let d = &self.d;
        let Some(s) = &d.scan else { return json!({ "scanning": true, "op": d.op, "opCan": d.can_act() }) };
        let readable = s.excluded.is_some();
        let rows = d.rows();
        let todo = d.todo(&rows);
        let can = common::is_admin() && readable && d.run.is_none();
        let list: Vec<Value> = rows
            .iter()
            .map(|(kind, path)| {
                let done = d.excluded(path);
                json!({ "kind": kind, "path": path, "done": done, "on": done || d.is_ticked(path), "canTick": readable && !done && d.run.is_none() })
            })
            .collect();
        json!({
            "scanning": false,
            "error": s.error,
            "passive": s.mode.eq_ignore_ascii_case("passive"),
            "readable": readable,
            "rows": list,
            "customErr": d.custom_err,
            "todo": todo.len(),
            "canExclude": can && !todo.is_empty(),
            "canUndo": can && d.has_undo(),
            "canRescan": d.loading.is_none(),
            "op": d.op,
            "opCan": d.can_act(),
        })
    }
}

impl Backend for PerfTab {
    fn pump(&mut self) {
        self.p.pump();
        self.d.pump();
    }

    fn busy(&self) -> bool {
        self.p.busy() || self.d.busy()
    }

    fn view(&self) -> Value {
        let p = &self.p;
        let pending = p.state.as_ref().map(|s| selected_changes(s, &p.selected).len()).unwrap_or(0);
        let can = common::is_admin() && p.run.is_none() && p.loading.is_none();
        json!({
            "loading": p.state.is_none(),
            "refreshing": p.loading.is_some(),
            "error": p.state.as_ref().and_then(|s| s.error.clone()),
            "groups": self.settings(),
            "pending": pending,
            "canApply": can && pending > 0,
            "canUndo": can && p.history.iter().any(|h| !h.undone),
            "canExplorer": p.explorer.is_none() && p.run.is_none() && p.op.is_none(),
            "note": p.note,
            "op": json(&p.op),
            "opCan": p.can_act(),
            "defender": self.defender(),
            "backups": self.bk_view(can && p.op.is_none()),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        if let Some(a) = backups::action(&action) {
            let now = self.now();
            if let Some(b) = self.bk.act(a, now) {
                let r = if common::is_admin() { self.p.open_restore(&b) } else { Err("Run PC Tuner as administrator to restore a backup.".to_string()) };
                self.bk.restored(r);
            }
            return Ok(Value::Null);
        }
        match parse::<Action>(action)? {
            Action::Switch { index, on } => {
                if common::is_admin() && self.p.loading.is_none() {
                    self.p.open_switch(index, on);
                }
            }
            Action::Toggle { index } => {
                if let Some(s) = self.p.selected.get_mut(index) {
                    *s = !*s;
                }
            }
            Action::Clear => self.p.selected = vec![false; TWEAKS.len()],
            Action::Apply => self.p.open_apply(),
            Action::Undo => self.p.open_undo(false),
            Action::Explorer => self.p.open_explorer_preview(),
            Action::Refresh => {
                if self.p.loading.is_none() {
                    self.p.refresh();
                }
            }
            Action::Op { ev } => {
                let can = self.p.can_act();
                // the page disables these without `can`; never act on a stale click
                if !can && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.p.op, ev);
                self.p.on_event(ev, can);
            }
            Action::DefenderTick { path, on } => self.d.set_ticked(&path, on),
            Action::DefenderAdd { path } => {
                self.d.custom = path;
                self.d.add_custom();
            }
            Action::DefenderExclude => {
                let todo = self.d.todo(&self.d.rows());
                if !todo.is_empty() {
                    self.d.open_exclude(todo);
                }
            }
            Action::DefenderUndo => self.d.open_undo(false),
            Action::DefenderRescan => {
                if self.d.loading.is_none() {
                    self.d.refresh();
                }
            }
            Action::DefenderOp { ev } => {
                if !self.d.can_act() && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.d.op, ev);
                self.d.on_event(ev);
            }
        }
        Ok(Value::Null)
    }
}

const ABOUT: &str = "A backup saves every performance setting in this list (not the Defender folders). Restore puts back only the settings that differ, with the usual preview, and can be undone.";
