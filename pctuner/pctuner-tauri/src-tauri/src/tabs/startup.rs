//! Startup Cleaner: everything that starts with Windows, the selection, the change history and the
//! operation window. The scanners, switch operations and history live in pctuner_core::startup.

use crate::backend::{json, parse, Backend};
use pctuner_core::opwin::{OpEvent, Stage};
use pctuner_core::startup::{clean_sel_op, explorer_op, Entry, Kind, Startup, KINDS};
use super::backups::{self, Backups};
use pctuner_core::common;
use pctuner_core::tabbackup::Item;
use serde_json::{json, Value};

pub struct StartupTab {
    s: Startup,
    bk: Backups,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Toggle { id: String },
    SelectAll,
    ClearSelection,
    Search { text: String },
    KindFilter { value: Option<Kind> },
    EnabledOnly { on: bool },
    ShowWindows { on: bool },
    Clean,
    SwitchOn,
    UndoLast,
    Rescan,
    Explorer,
    HistoryOpen,
    HistoryClose,
    HistToggle { index: usize },
    HistUndoSelected,
    HistUndoAll,
    HistClear,
    Op { ev: OpEvent },
}

impl StartupTab {
    pub fn new() -> Self {
        StartupTab { s: Startup::new(), bk: Backups::new("Startup", ABOUT) }
    }

    /// The selected items among the rows shown now (the egui tab's `sel`).
    /// The Backups sheet (`can` = a change could be started now).
    fn bk_view(&self, can: bool) -> Value {
        let n = self.now();
        self.bk.view(n.as_ref().map(|x| x.1.as_slice()), can)
    }

    /// The settings for a backup (None until the first scan is done).
    fn now(&self) -> Option<(String, Vec<Item>)> {
        (self.s.scanned && self.s.job.is_none() && !self.s.entries.is_empty()).then(|| (String::new(), self.s.backup_items()))
    }

    fn sel(&self) -> Vec<Entry> {
        let s = &self.s;
        s.visible().iter().map(|&i| &s.entries[i]).filter(|e| s.selected.contains(&e.id)).cloned().collect()
    }

    /// Changes in the history that can still be undone.
    fn pending(&self) -> usize {
        self.s.history.iter().filter(|h| !h.undone).count()
    }

    fn history(&self) -> Value {
        let s = &self.s;
        let busy = s.busy();
        let active = self.pending();
        // only active rows count (undone ones can't be selected, this is a safety net)
        let n = s.hist_sel.iter().filter(|&&i| s.history.get(i).is_some_and(|h| !h.undone)).count();
        // newest first
        let rows: Vec<Value> = (0..s.history.len())
            .rev()
            .map(|i| {
                let h = &s.history[i];
                json!({
                    "index": i,
                    "time": h.time,
                    "name": h.name,
                    "type": h.kind.label(),
                    "location": h.location,
                    "change": if h.to { "Switched on" } else { "Switched off" },
                    "state": if h.undone { "Undone" } else { "Active" },
                    "undone": h.undone,
                    "selected": !h.undone && s.hist_sel.contains(&i),
                })
            })
            .collect();
        json!({
            // hidden while an operation window is open (it comes back afterwards)
            "open": s.show_history && s.op.is_none(),
            "recorded": s.history.len(),
            "active": active,
            "rows": rows,
            "selected": n,
            "undoSelectedLabel": if n > 0 { format!("Undo selected ({n})") } else { "Undo selected".to_string() },
            "canUndoSelected": !busy && n > 0,
            "canClear": !busy,
            "canUndoAll": !busy && active > 0,
            "busy": busy,
            "status": s.hist_status,
        })
    }
}

impl Backend for StartupTab {
    fn pump(&mut self) {
        self.s.pump();
    }

    /// A change or an Explorer restart is running: closing PC Tuner waits for it.
    fn busy(&self) -> bool {
        self.s.busy()
    }

    fn view(&self) -> Value {
        let s = &self.s;
        let busy = s.busy();
        let ready = s.scanned && !(s.job.is_some() && s.entries.is_empty());

        // hero: junk still on
        let on = s.entries.iter().filter(|e| e.category != "Windows" && e.enabled).count();
        let junk = s.entries.iter().filter(|e| e.enabled && e.category == "Junk").count();
        let optional = s.entries.iter().filter(|e| e.enabled && e.category == "Optional").count();
        let pending = self.pending();
        let val = |n: usize| if ready { n.to_string() } else { "-".to_string() };
        let (hero_tone, caption) = if !ready {
            ("Faint", "Scanning startup locations...")
        } else if junk == 0 {
            ("Ok", "Nothing left to clean.")
        } else {
            ("Junk", "Safe to switch off - nothing is deleted.")
        };

        // list
        let visible = s.visible();
        let mut sel_count = 0;
        let mut sel_on = 0;
        let rows: Vec<Value> = visible
            .iter()
            .map(|&i| {
                let e = &s.entries[i];
                let selected = s.selected.contains(&e.id);
                if selected {
                    sel_count += 1;
                    if e.enabled {
                        sel_on += 1;
                    }
                }
                // name / advice carry the category colour; the rest stays quiet
                let tone = if !e.enabled {
                    "Dim"
                } else {
                    match e.category {
                        "Junk" => "Junk",
                        "Optional" => "Warn",
                        "Keep" => "Ok",
                        _ => "Text",
                    }
                };
                json!({
                    "id": e.id,
                    "on": e.enabled,
                    "name": e.name,
                    "type": e.kind.label(),
                    "location": e.location,
                    "publisher": e.publisher,
                    "advice": e.advice,
                    "command": e.command,
                    "tone": tone,
                    "selected": selected,
                    // shown when the row is right-clicked
                    "about": pctuner_core::explain::startup(&e.name, &e.command, &e.publisher, e.kind.label(), e.category, e.advice),
                })
            })
            .collect();
        let has_sel = sel_count > 0 && !busy;
        let status = if busy {
            if s.job.is_some() { "Working / scanning..." } else { "Restarting Explorer..." }
        } else {
            s.last_action.as_str()
        };
        let warnings: Vec<&String> = s.history_warning.iter().chain(&s.warnings).collect();
        let kinds: Vec<Value> = KINDS.iter().map(|k| json!({ "value": k, "label": k.label() })).collect();

        json!({
            "ready": ready,
            "busy": busy,
            "status": status,
            "hero": {
                "junk": val(junk),
                "tone": hero_tone,
                "caption": caption,
                "on": val(on),
                "optional": val(optional),
                "optionalTone": if optional == 0 || !ready { "Text" } else { "Warn" },
                "pending": pending.to_string(),
            },
            "warnings": warnings,
            "search": s.search,
            "kindFilter": s.kind_filter,
            "kinds": kinds,
            "enabledOnly": s.enabled_only,
            "showWindows": s.show_windows,
            "rows": rows,
            "selected": sel_count,
            "cleanCount": sel_on,
            "canClean": has_sel && sel_on > 0,
            "canSwitchOn": has_sel,
            "canUndoLast": !busy && pending > 0,
            "canRescan": !busy,
            "canExplorer": !busy,
            "history": self.history(),
            "op": json(&s.op.as_ref().map(|o| &o.win)),
            "opCan": s.can_act(),
            "backups": self.bk_view(s.can_act() && s.op.is_none()),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        if let Some(a) = backups::action(&action) {
            let now = self.now();
            if let Some(b) = self.bk.act(a, now) {
                let r = if common::is_admin() { self.s.open_restore(&b) } else { Err("Run PC Tuner as administrator to restore a backup.".to_string()) };
                self.bk.restored(r);
            }
            return Ok(Value::Null);
        }
        let busy = self.s.busy();
        match parse::<Action>(action)? {
            Action::Toggle { id } => {
                // only rows that exist can be selected
                if self.s.entries.iter().any(|e| e.id == id) && !self.s.selected.remove(&id) {
                    self.s.selected.insert(id);
                }
            }
            Action::SelectAll => {
                for i in self.s.visible() {
                    let id = self.s.entries[i].id.clone();
                    self.s.selected.insert(id);
                }
            }
            Action::ClearSelection => self.s.selected.clear(),
            Action::Search { text } => self.s.search = text,
            Action::KindFilter { value } => self.s.kind_filter = value,
            Action::EnabledOnly { on } => self.s.enabled_only = on,
            Action::ShowWindows { on } => self.s.show_windows = on,
            Action::Clean => {
                let sel = self.sel();
                // the button is disabled without a switched-on item in the selection, and while busy
                if !busy && self.s.op.is_none() && sel.iter().any(|e| e.enabled) {
                    self.s.op = Some(clean_sel_op(&sel));
                }
            }
            Action::SwitchOn => {
                let sel = self.sel();
                if !busy && self.s.op.is_none() && !sel.is_empty() {
                    self.s.switch(sel, true);
                }
            }
            Action::UndoLast => {
                if !busy && self.s.op.is_none() && self.pending() > 0 {
                    self.s.undo_last();
                }
            }
            Action::Rescan => self.s.rescan(),
            Action::Explorer => {
                if !busy && self.s.op.is_none() {
                    self.s.op = Some(explorer_op());
                }
            }
            Action::HistoryOpen => self.s.show_history = true,
            Action::HistoryClose => self.s.close_history(),
            Action::HistToggle { index } => self.s.hist_toggle(index),
            Action::HistUndoSelected => {
                if !busy && self.s.op.is_none() {
                    self.s.hist_undo_selected();
                }
            }
            Action::HistUndoAll => {
                if !busy && self.s.op.is_none() {
                    self.s.hist_undo_all();
                }
            }
            Action::HistClear => {
                if !busy {
                    self.s.hist_clear();
                }
            }
            Action::Op { ev } => {
                let can = self.s.can_act();
                // the page disables these without `can`; never act on a stale click
                if !can && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                // "Go" switches the window to its running stage first, like the egui window does
                if ev == OpEvent::Go {
                    if let Some(op) = self.s.op.as_mut() {
                        op.win.stage = Stage::Running;
                    }
                }
                self.s.on_event(ev, can);
            }
        }
        Ok(Value::Null)
    }
}

const ABOUT: &str = "A backup saves whether each startup item is switched on or off. Restore switches back only the items that differ (new items are left as they are), with the usual preview, and can be undone.";
