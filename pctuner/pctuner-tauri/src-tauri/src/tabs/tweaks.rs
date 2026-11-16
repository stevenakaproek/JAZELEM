//! Optimization > Windows tweaks: the post-install tweaks, their status and the last run's log.

use crate::backend::{json, op_event, parse, Backend};
use pctuner_core::common;
use pctuner_core::opwin::OpEvent;
use pctuner_core::tweaks::{parse_log, pick_mode, script_tasks, Mode, Tweaks, GROUPS, G_EDGE, G_ONEDRIVE, G_TASKS, G_XBOX};
use serde_json::{json, Value};

pub struct TweaksTab {
    t: Tweaks,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Toggle { index: usize },
    Clear,
    Apply,
    Undo,
    Refresh,
    Details { on: bool },
    Op { ev: OpEvent },
}

impl TweaksTab {
    pub fn new() -> Self {
        TweaksTab { t: Tweaks::new() }
    }

    /// May the page's buttons and rows be used: administrator, and the script is not running.
    fn can(&self) -> bool {
        common::is_admin() && !self.t.running()
    }

    fn applied(&self, index: usize) -> Option<bool> {
        self.t.status.as_ref().and_then(|s| s.applied(GROUPS[index]))
    }

    /// Only rows that still need applying count.
    fn picked(&self) -> [bool; 4] {
        std::array::from_fn(|i| self.t.selected[i] && self.applied(i) == Some(false))
    }

    /// The page's rows: (group, name, explanation), in GROUPS order.
    fn rows() -> [(&'static str, &'static str, String); 4] {
        let n_tasks = script_tasks().len();
        [
            (G_TASKS, "Telemetry and tips tasks off", format!("{n_tasks} telemetry, tips, maps and family-safety tasks are disabled. Xbox and Steam tasks are kept.")),
            (G_EDGE, "Edge preloading off", "Startup Boost and background mode off. Edge stays installed.".into()),
            (G_XBOX, "Teredo on for Xbox online play", "Teredo set to \"enterpriseclient\" by Group Policy so Windows keeps it.".into()),
            (G_ONEDRIVE, "Remove OneDrive", "Uninstalls OneDrive. \"Undo tweaks\" does not bring it back.".into()),
        ]
    }

    fn last_run(&self) -> Value {
        let t = &self.t;
        let title = match t.mode {
            Some(m) => format!("Last run - {}", m.title()),
            None => "Last run".to_string(),
        };
        let Some(s) = &t.stream else { return json!({ "title": title, "ran": false }) };
        let parsed = parse_log(&s.lines);
        let mut chips: Vec<Value> = vec![];
        for g in [G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE] {
            let total = parsed.items.iter().filter(|i| i.group == g).count();
            let failed = parsed.items.iter().filter(|i| i.group == g && i.result.is_err()).count();
            if total > 0 {
                chips.push(json!({ "text": format!("{g}  {}/{total}", total - failed), "ok": failed == 0 }));
            }
        }
        // the raw log is only sent while it is shown
        let lines: Vec<Value> = if t.show_details { s.lines.iter().map(|l| json!({ "text": l, "fail": l.contains("FAIL") })).collect() } else { vec![] };
        json!({
            "title": title,
            "ran": true,
            "running": s.exit.is_none(),
            "exit": s.exit,
            "chips": chips,
            "showDetails": t.show_details,
            "lines": lines,
        })
    }
}

impl Backend for TweaksTab {
    fn pump(&mut self) {
        self.t.pump();
    }

    fn busy(&self) -> bool {
        self.t.running()
    }

    fn view(&self) -> Value {
        let t = &self.t;
        let can = self.can();
        let rows: Vec<Value> = Self::rows()
            .iter()
            .enumerate()
            .map(|(i, (group, name, text))| {
                let applied = self.applied(i);
                let selectable = can && applied == Some(false);
                json!({
                    "index": i,
                    "name": name,
                    "text": text,
                    "danger": *group == G_ONEDRIVE,
                    "applied": applied,
                    "selectable": selectable,
                    "selected": t.selected[i] && selectable,
                    "now": t.status.as_ref().map(|s| s.now(group)),
                })
            })
            .collect();
        let n = self.picked().iter().filter(|s| **s).count();
        json!({
            "rows": rows,
            "pending": n,
            "canApply": can && n > 0,
            "canUndo": can,
            "canRefresh": !t.running() && t.status.is_some(),
            "lastRun": self.last_run(),
            "op": json(&t.op),
            "opCan": t.can_act(),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        match parse::<Action>(action)? {
            Action::Toggle { index } => {
                if index < 4 && self.can() && self.applied(index) == Some(false) {
                    self.t.selected[index] = !self.t.selected[index];
                }
            }
            Action::Clear => self.t.selected = [false; 4],
            Action::Apply => {
                if self.can() {
                    if let Some(m) = pick_mode(&self.picked()) {
                        self.t.ask(m);
                    }
                }
            }
            Action::Undo => {
                if self.can() {
                    self.t.ask(Mode::Undo);
                }
            }
            Action::Refresh => {
                if !self.t.running() && self.t.status.is_some() {
                    self.t.status = None;
                }
            }
            Action::Details { on } => self.t.show_details = on,
            Action::Op { ev } => {
                // the page disables these without `can`; never act on a stale click
                if !self.t.can_act() && matches!(ev, OpEvent::Go | OpEvent::Undo | OpEvent::Extra(_)) {
                    return Ok(Value::Null);
                }
                let ev = op_event(&mut self.t.op, ev);
                self.t.on_event(ev);
            }
        }
        Ok(Value::Null)
    }
}
