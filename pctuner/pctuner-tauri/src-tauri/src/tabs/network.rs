//! Optimization > Network: the adapter and Windows network settings list.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::common;
use pctuner_core::network::{fmt, selected_changes, Network, TWEAKS};
use pctuner_core::opwin::OpEvent;
use super::backups::{self, Backups};
use pctuner_core::tabbackup::Item;
use serde_json::{json, Value};

pub struct NetworkTab {
    n: Network,
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
    Refresh,
    Op { ev: OpEvent },
}

impl NetworkTab {
    pub fn new() -> Self {
        NetworkTab { n: Network::new(), bk: Backups::new("Network", ABOUT) }
    }

    /// A row can be clicked: it exists on this adapter, is not applied yet and nothing is running.
    /// The Backups sheet (`can` = a change could be started now).
    fn bk_view(&self, can: bool) -> Value {
        let n = self.now();
        self.bk.view(n.as_ref().map(|x| x.1.as_slice()), can)
    }

    /// The settings for a backup (None until the adapter is read).
    fn now(&self) -> Option<(String, Vec<Item>)> {
        let s = self.n.state.as_ref().filter(|s| s.error.is_none() && s.info.is_some())?;
        Some((s.adapter().to_string(), pctuner_core::network::backup_items(s)))
    }

    fn selectable(&self, index: usize) -> bool {
        let (Some(state), Some(t)) = (&self.n.state, TWEAKS.get(index)) else { return false };
        state.info.is_some() && state.applicable(t.kind) && !state.is_set(t) && self.n.run.is_none()
    }

    /// May "Apply selected" / "Undo last change" be pressed (before their own conditions)?
    fn can(&self) -> bool {
        common::is_admin() && self.n.run.is_none() && self.n.loading.is_none()
    }

    /// The settings are only shown once an adapter was read.
    fn pending(&self) -> usize {
        match &self.n.state {
            Some(s) if s.info.is_some() => selected_changes(s, &self.n.selected).len(),
            _ => 0,
        }
    }

    fn has_undo(&self) -> bool {
        self.n.history.iter().any(|h| !h.undone)
    }

    fn settings(&self) -> Value {
        let Some(state) = &self.n.state else { return Value::Null };
        if state.info.is_none() {
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
            let selectable = applicable && !set && self.n.run.is_none();
            rows.push(json!({
                "index": i,
                "name": t.name,
                "why": t.why,
                "optional": t.optional,
                "applicable": applicable,
                "set": set,
                "selectable": selectable,
                "selected": self.n.selected[i] && selectable,
                // the switch: on = the tweak is in place
                "canSwitch": applicable && self.can() && self.n.op.is_none(),
                "now": fmt(t.kind, state.value(t.kind).as_deref()),
                "target": fmt(t.kind, Some(t.target)),
            }));
        }
        if !rows.is_empty() {
            groups.push(json!({ "name": group, "rows": rows }));
        }
        Value::Array(groups)
    }
}

impl Backend for NetworkTab {
    fn pump(&mut self) {
        self.n.pump();
    }

    fn busy(&self) -> bool {
        self.n.busy()
    }

    fn view(&self) -> Value {
        let n = &self.n;
        let pending = self.pending();
        let can = self.can();
        json!({
            "loading": n.state.is_none(),
            "refreshing": n.loading.is_some(),
            // only when no adapter could be read
            "error": n.state.as_ref().filter(|s| s.info.is_none()).map(|s| s.error.clone().unwrap_or_default()),
            "groups": self.settings(),
            "pending": pending,
            "canApply": can && pending > 0,
            "canUndo": can && self.has_undo(),
            "op": json(&n.op),
            "opCan": n.can_act(),
            "backups": self.bk_view(can && n.op.is_none()),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        if let Some(a) = backups::action(&action) {
            let now = self.now();
            if let Some(b) = self.bk.act(a, now) {
                let r = if common::is_admin() { self.n.open_restore(&b) } else { Err("Run PC Tuner as administrator to restore a backup.".to_string()) };
                self.bk.restored(r);
            }
            return Ok(Value::Null);
        }
        match parse::<Action>(action)? {
            Action::Switch { index, on } => {
                if self.can() {
                    self.n.open_switch(index, on);
                }
            }
            Action::Toggle { index } => {
                if self.selectable(index) {
                    self.n.selected[index] = !self.n.selected[index];
                }
            }
            Action::Clear => self.n.selected = vec![false; TWEAKS.len()],
            Action::Apply => {
                if self.can() && self.pending() > 0 {
                    self.n.open_apply();
                }
            }
            Action::Undo => {
                if self.can() && self.has_undo() {
                    self.n.open_undo(false);
                }
            }
            Action::Refresh => {
                if self.n.loading.is_none() {
                    self.n.refresh();
                }
            }
            Action::Op { ev } => {
                // the page disables these without `can`; never act on a stale click
                if !self.n.can_act() && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.n.op, ev);
                self.n.on_event(ev);
            }
        }
        Ok(Value::Null)
    }
}

const ABOUT: &str = "A backup saves every network setting in this list. Restore puts back only the settings that differ, with the usual preview, and can be undone. Adapter settings are only restored on the adapter the backup was made on.";
