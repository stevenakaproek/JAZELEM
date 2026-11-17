//! Services: the service list with its selection, the change history and the Backups window.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::common;
use pctuner_core::opwin::{OpEvent, Stage};
use pctuner_core::services::{Services, SortCol, NOTE_GAMING, NOTE_LOCKED};
use pctuner_core::svcbackup::{backup_dir, differences, windows_default, Backups};
use serde_json::{json, Value};

pub struct ServicesTab {
    s: Services,
    /// The Backups window while it is open.
    backups: Option<Backups>,
}

#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Col {
    Name,
    Display,
    Startup,
    Status,
    Note,
    Suggested,
}

#[derive(serde::Deserialize, Clone, Copy)]
enum StartType {
    Disabled,
    Manual,
    Automatic,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Toggle { name: String },
    Clear,
    Search { text: String },
    RunningOnly { on: bool },
    HideLocked { on: bool },
    StopOnDisable { on: bool },
    Sort { col: Col },
    Change { ty: StartType },
    UndoLast,
    Refresh,
    Op { ev: OpEvent },
    History,
    HistoryClose,
    HistToggle { index: usize },
    HistUndo,
    HistClear,
    Backups,
    BackupsClose,
    BackupCreate,
    BackupFolder,
    BackupRestore { index: usize },
    BackupDelete { index: usize },
    BackupRestoreDefault,
    BackupOp { ev: OpEvent },
}

fn col_of(c: SortCol) -> &'static str {
    match c {
        SortCol::Name => "name",
        SortCol::Display => "display",
        SortCol::Startup => "startup",
        SortCol::Status => "status",
        SortCol::Note => "note",
        SortCol::Suggested => "suggested",
    }
}

impl ServicesTab {
    pub fn new() -> Self {
        ServicesTab { s: Services::new(), backups: None }
    }

    /// The same rule the egui tab uses for every button that changes something.
    fn can(&self) -> bool {
        common::is_admin() && !self.busy()
    }

    /// A window is open on top of the page (the page below cannot be clicked).
    fn modal(&self) -> bool {
        self.s.op.is_some() || self.backups.is_some() || self.s.show_history
    }

    fn rows(&self) -> Vec<Value> {
        let s = &self.s;
        s.visible()
            .into_iter()
            .map(|i| {
                let v = &s.list[i];
                let suggested = s.suggest.get(&v.name_lc);
                // the row's colour: gaming, locked, safe to change (green), disabled
                let tone = match v.note {
                    NOTE_GAMING => "gaming",
                    NOTE_LOCKED => "locked",
                    _ if suggested.is_some() => "safe",
                    _ if v.start == "Disabled" => "disabled",
                    _ => "",
                };
                json!({
                    "name": v.name,
                    "display": v.display,
                    "start": v.start,
                    "startDisabled": v.start == "Disabled",
                    "state": v.state,
                    "running": v.state == "Running",
                    "note": v.note,
                    "tone": tone,
                    "suggested": suggested,
                    "suggestedDone": suggested.is_some_and(|t| *t == v.start),
                    "selected": s.selected.contains(&v.name),
                    // shown when the row is right-clicked
                    "about": pctuner_core::explain::service(&v.name, &v.display, &v.desc, v.note, suggested.map(|t| t.as_ref())),
                })
            })
            .collect()
    }

    fn history(&self) -> Value {
        let s = &self.s;
        // like the egui tab: the history window waits behind an operation window or the Backups window
        if !s.show_history || s.op.is_some() || self.backups.is_some() {
            return Value::Null;
        }
        let active = s.history.iter().filter(|h| !h.undone).count();
        let n = s.hist_sel.len();
        let rows: Vec<Value> = (0..s.history.len())
            .rev()
            .map(|i| {
                let h = &s.history[i];
                json!({
                    "index": i,
                    "time": h.time,
                    "name": h.name,
                    "display": h.display_name,
                    "from": h.from,
                    "to": h.to,
                    "undone": h.undone,
                    "selected": s.hist_sel.contains(&i),
                })
            })
            .collect();
        json!({
            "active": active,
            "recorded": s.history.len(),
            "rows": rows,
            "undoLabel": if n > 0 { format!("Undo selected ({n})") } else { "Undo selected".to_string() },
            "canUndo": self.can() && n > 0,
            "canClear": !self.busy(),
            "busy": self.busy(),
        })
    }

    fn backups_view(&self) -> Value {
        let Some(b) = &self.backups else { return Value::Null };
        let admin = common::is_admin();
        let list: Vec<Value> = b
            .backups
            .iter()
            .enumerate()
            .map(|(index, (_path, bk))| {
                let diffs = differences(&bk.services, &b.current).len();
                let note = if bk.note.is_empty() { String::new() } else { format!(" - {}", bk.note) };
                json!({
                    "index": index,
                    "time": bk.time,
                    "detail": format!("{} services{note}", bk.services.len()),
                    "matches": diffs == 0,
                    "diff": if diffs == 0 { "matches the PC now".to_string() } else { format!("{diffs} differ from now") },
                    "canRestore": admin && b.run.is_none(),
                })
            })
            .collect();
        // the built-in "Windows default" row (clean-install startup types)
        let def = windows_default();
        let def_diffs = differences(&def.services, &b.current).len();
        let windows_default = json!({
            "detail": format!("Startup types of a clean Windows 11 install ({} Windows services; drivers, apps and games are left alone)", def.services.len()),
            "matches": def_diffs == 0,
            "diff": if def_diffs == 0 { "matches the PC now".to_string() } else { format!("{def_diffs} differ from now") },
            "canRestore": admin && b.run.is_none(),
        });
        json!({
            "windowsDefault": windows_default,
            "msg": b.msg.as_ref().map(|(text, ok)| json!({ "text": text, "ok": ok })),
            "canCreate": admin && b.job.is_none(),
            "creating": b.job.is_some(),
            "list": list,
            "op": json(&b.op),
            "opCan": b.can_act(),
        })
    }
}

impl Backend for ServicesTab {
    fn pump(&mut self) {
        self.s.pump();
        if let Some(b) = &mut self.backups {
            // a restore changed services: read the list again
            if b.pump() {
                self.s.start_load();
            }
        }
    }

    /// A service change is running: closing PC Tuner waits for it.
    fn busy(&self) -> bool {
        self.s.busy() || self.backups.as_ref().map(|b| b.busy()).unwrap_or(false)
    }

    fn view(&self) -> Value {
        let s = &self.s;
        let can = self.can();
        let running = s.list.iter().filter(|v| v.state == "Running").count();
        let disabled = s.list.iter().filter(|v| v.start == "Disabled").count();
        let undoable = s.history.iter().filter(|h| !h.undone).count();
        // safe-to-change services (green) that are not yet at their suggested type
        let open = s.list.iter().filter(|v| s.suggest.get(&v.name_lc).is_some_and(|t| *t != v.start)).count();
        let chip = if s.list.is_empty() {
            "Checking...".to_string()
        } else if open == 0 {
            "Every green service is at its suggested type".to_string()
        } else {
            format!("{open} green services not at their suggested type")
        };
        // the actions only act on selected rows that are shown (see selected_visible)
        let vis = s.selected_visible().len();
        let total = s.selected.len();
        let (sel_text, sel_active) = if total == 0 {
            ("Click rows to select".to_string(), false)
        } else if vis == total {
            (format!("{vis} selected"), true)
        } else if vis > 0 {
            (format!("{vis} selected ({} hidden by filter)", total - vis), true)
        } else {
            (format!("{total} selected, all hidden by filter"), false)
        };
        let status = match &s.load_error {
            Some(e) => json!({ "text": format!("Could not load services: {e}"), "error": true }),
            None if !s.last_action.is_empty() => json!({ "text": s.last_action, "error": false }),
            None => Value::Null,
        };
        json!({
            "loading": s.list.is_empty() && s.load_job.is_some(),
            "refreshing": s.load_job.is_some(),
            "busy": self.busy(),
            "hero": {
                "running": running,
                "disabled": disabled,
                "total": s.list.len(),
                "undoable": undoable,
                "chip": chip,
                "chipOk": !s.list.is_empty(),
            },
            "search": s.search,
            "runningOnly": s.running_only,
            "hideLocked": s.hide_locked,
            "stopOnDisable": s.stop_on_disable,
            "sort": col_of(s.sort),
            "sortAsc": s.sort_asc,
            "rows": self.rows(),
            "selText": sel_text,
            "selActive": sel_active,
            "canChange": can && vis > 0,
            "canClear": total > 0,
            "canUndo": can && s.history.iter().any(|h| !h.undone),
            "status": status,
            "op": json(&s.op),
            "opCan": can,
            "history": self.history(),
            "backups": self.backups_view(),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let can = self.can();
        match parse::<Action>(action)? {
            Action::Toggle { name } => {
                // only services that are in the list can be selected
                if self.s.list.iter().any(|v| v.name == name) && !self.s.selected.remove(&name) {
                    self.s.selected.insert(name);
                }
            }
            Action::Clear => self.s.clear_selection(),
            Action::Search { text } => self.s.search = text,
            Action::RunningOnly { on } => self.s.running_only = on,
            Action::HideLocked { on } => self.s.hide_locked = on,
            Action::StopOnDisable { on } => self.s.stop_on_disable = on,
            Action::Sort { col } => {
                let col = match col {
                    Col::Name => SortCol::Name,
                    Col::Display => SortCol::Display,
                    Col::Startup => SortCol::Startup,
                    Col::Status => SortCol::Status,
                    Col::Note => SortCol::Note,
                    Col::Suggested => SortCol::Suggested,
                };
                if self.s.sort == col {
                    self.s.sort_asc = !self.s.sort_asc;
                } else {
                    self.s.sort = col;
                    self.s.sort_asc = true;
                }
            }
            Action::Change { ty } => {
                // the buttons are disabled without rights, while busy and with nothing selected
                if can && !self.modal() && !self.s.selected_visible().is_empty() {
                    self.s.request_change(match ty {
                        StartType::Disabled => "Disabled",
                        StartType::Manual => "Manual",
                        StartType::Automatic => "Automatic",
                    });
                }
            }
            Action::UndoLast => {
                if can && !self.modal() && self.s.history.iter().any(|h| !h.undone) {
                    self.s.request_undo_last();
                }
            }
            Action::Refresh => {
                if self.s.load_job.is_none() {
                    self.s.start_load();
                }
            }
            Action::Op { ev } => {
                let Some(stage) = self.s.op.as_ref().map(|o| o.stage) else { return Ok(Value::Null) };
                // the page disables these without `can`; never act on a stale click
                let ok = match ev {
                    OpEvent::Go => can && stage == Stage::Preview,
                    OpEvent::Undo | OpEvent::Extra(_) => can && stage == Stage::Done,
                    OpEvent::Close => stage != Stage::Running,
                };
                if ok {
                    let ev = op_event(&mut self.s.op, ev);
                    self.s.on_event(ev);
                }
            }
            Action::History => {
                if !self.modal() {
                    self.s.show_history = true;
                }
            }
            Action::HistoryClose => self.s.show_history = false,
            Action::HistToggle { index } => {
                // entries that were undone already cannot be ticked
                if self.s.history.get(index).is_some_and(|h| !h.undone) && !self.s.hist_sel.remove(&index) {
                    self.s.hist_sel.insert(index);
                }
            }
            Action::HistUndo => {
                if can && self.s.show_history && self.s.op.is_none() && self.backups.is_none() && !self.s.hist_sel.is_empty() {
                    let idx: Vec<usize> = self.s.hist_sel.drain().collect();
                    self.s.request_undo(idx);
                }
            }
            Action::HistClear => {
                if !self.busy() && self.s.op.is_none() {
                    self.s.clear_undone_history();
                }
            }
            Action::Backups => {
                if !self.modal() {
                    self.backups = Some(Backups::new());
                }
            }
            Action::BackupsClose => {
                // the Close button is not there while the restore window is open
                if self.backups.as_ref().is_some_and(|b| b.op.is_none()) {
                    self.backups = None;
                }
            }
            Action::BackupCreate => {
                if let Some(b) = &mut self.backups {
                    if common::is_admin() && b.job.is_none() && b.op.is_none() {
                        b.create_backup();
                    }
                }
            }
            Action::BackupFolder => {
                if self.backups.is_some() {
                    let d = backup_dir();
                    let _ = std::fs::create_dir_all(&d);
                    let _ = common::run("explorer.exe", &[&d.display().to_string()]);
                }
            }
            Action::BackupRestore { index } => {
                if let Some(b) = &mut self.backups {
                    if common::is_admin() && b.run.is_none() && b.op.is_none() {
                        if let Some((_, bk)) = b.backups.get(index).cloned() {
                            let diffs = differences(&bk.services, &b.current);
                            b.open_restore(&bk, &diffs);
                        }
                    }
                }
            }
            Action::BackupRestoreDefault => {
                if let Some(b) = &mut self.backups {
                    if common::is_admin() && b.run.is_none() && b.op.is_none() {
                        let def = windows_default();
                        let diffs = differences(&def.services, &b.current);
                        b.open_restore(&def, &diffs);
                    }
                }
            }
            Action::BackupDelete { index } => {
                if let Some(b) = &mut self.backups {
                    b.delete_backup(index);
                }
            }
            Action::BackupOp { ev } => {
                let Some(b) = &mut self.backups else { return Ok(Value::Null) };
                let Some(stage) = b.op.as_ref().map(|o| o.stage) else { return Ok(Value::Null) };
                let ok = match ev {
                    OpEvent::Go => b.can_act() && stage == Stage::Preview && b.pending.is_some(),
                    OpEvent::Close => stage != Stage::Running,
                    OpEvent::Undo | OpEvent::Extra(_) => false,
                };
                if ok {
                    let ev = op_event(&mut b.op, ev);
                    b.on_event(ev);
                }
            }
        }
        Ok(Value::Null)
    }
}
