//! CPU Scheduler: processor scheduling (Win32PrioritySeparation, foreground lock timeout) and the
//! multimedia scheduler (MMCSS) profile and tasks. Each part is edited and applied on its own.

use crate::backend::{parse, Backend};
use pctuner_core::common::{self, Tone};
use pctuner_core::scheduler::*;
use serde_json::{json, Value};

pub struct SchedulerTab {
    s: Scheduler,
    /// The task shown on the Multimedia scheduler part.
    task: usize,
    /// The part of the page shown.
    section: Section,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Section { section: Section },
    Task { index: usize },
    Interval { value: u32 },
    Length { value: u32 },
    Boost { value: u32 },
    Foreground { value: u32 },
    RestoreDefaults,
    /// A drop-down choice: the index in the list the view gave.
    Choose { task: String, name: String, index: usize },
    /// Clock rate.
    Number { task: String, name: String, value: u32 },
    AffinityAll { task: String, on: bool },
    AffinityCpu { task: String, cpu: u32, on: bool },
    Apply,
    Cancel,
    Undo,
    Backup,
    Reload,
}

fn prio_key() -> Key {
    (PRIO.to_string(), PRIO_NAME)
}

fn fg_key() -> Key {
    (FG.to_string(), FG_NAME)
}

/// CPUs offered in the affinity list (at most 32) and the mask with all of them set.
fn cpus() -> (u32, u32) {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).min(32) as u32;
    (n, if n >= 32 { u32::MAX } else { (1u32 << n) - 1 })
}

fn status_json(name: &str, cur: &Option<RegVal>, v: &Option<RegVal>, def: &Option<RegVal>) -> Value {
    let (text, tone) = status(name, cur, v, def);
    json!({ "text": text, "tone": tone })
}

impl SchedulerTab {
    pub fn new() -> Self {
        let s = Scheduler::new();
        let task = s.tasks.iter().position(|t| t == "Games").unwrap_or(0);
        let mut tab = SchedulerTab { s, task, section: Section::Processor };
        tab.settle();
        tab
    }

    /// What drawing the shown part does to the edits in the egui tab: every setting on screen has an entry,
    /// and the priority value is always a number (2 when the registry has none).
    fn settle(&mut self) {
        self.task = self.task.min(self.s.tasks.len().saturating_sub(1));
        match self.section {
            Section::Processor => {
                let p = self.prio();
                self.s.edit.insert(prio_key(), Some(RegVal::D(p)));
                self.s.edit.entry(fg_key()).or_insert(None);
            }
            Section::Multimedia => {
                for (n, _, _) in SYS_FIELDS {
                    self.s.edit.entry((String::new(), *n)).or_insert(None);
                }
                if let Some(task) = self.s.tasks.get(self.task).cloned() {
                    for (n, _, _) in TASK_FIELDS {
                        self.s.edit.entry((task.clone(), *n)).or_insert(None);
                    }
                }
            }
        }
    }

    /// The edited Win32PrioritySeparation value.
    fn prio(&self) -> u32 {
        match self.s.edit.get(&prio_key()).cloned().flatten() {
            Some(RegVal::D(v)) => v,
            _ => 2,
        }
    }

    /// Change one of the three parts. Only a changed choice rewrites the value (an odd stored value such as
    /// 0x3F is kept as is).
    fn set_parts(&mut self, f: impl FnOnce(&mut (u32, u32, u32))) {
        let mut p = self.prio();
        let before = prio_parts(p);
        let mut parts = before;
        f(&mut parts);
        if parts != before {
            p = prio_value(parts.0, parts.1, parts.2);
        }
        self.s.edit.insert(prio_key(), Some(RegVal::D(p)));
    }

    /// The key of a setting the multimedia part shows now: the profile itself or the selected task.
    fn shown_key(&self, task: &str, name: &str) -> Option<Key> {
        if task.is_empty() {
            SYS_FIELDS.iter().find(|(n, _, _)| *n == name).map(|(n, _, _)| (String::new(), *n))
        } else if self.s.tasks.get(self.task).is_some_and(|t| t == task) {
            TASK_FIELDS.iter().find(|(n, _, _)| *n == name).map(|(n, _, _)| (task.to_string(), *n))
        } else {
            None
        }
    }

    fn row(&self, task: &str, name: &'static str, label: &str) -> Value {
        let key: Key = (task.to_string(), name);
        let cur = self.s.cur.get(&key).cloned().flatten();
        let v = self.s.edit.get(&key).cloned().flatten();
        let def = default_of(task, name);
        let mut row = json!({
            "task": task,
            "name": name,
            "label": label,
            "text": show(name, &v),
            "status": status_json(name, &cur, &v, &def),
        });
        match name {
            "Clock Rate" => {
                row["control"] = json!("number");
                row["number"] = json!(match &v {
                    Some(RegVal::D(n)) => *n,
                    _ => 10000,
                });
                row["min"] = json!(0);
                row["max"] = json!(1_000_000);
            }
            "Affinity" => {
                let mask = match &v {
                    Some(RegVal::D(m)) => *m,
                    _ => 0,
                };
                let (n, _) = cpus();
                row["control"] = json!("affinity");
                row["all"] = json!(mask == 0);
                row["cpus"] = json!((0..n).map(|c| mask == 0 || mask & (1 << c) != 0).collect::<Vec<bool>>());
            }
            _ => {
                let mut opts = choices(name);
                if !opts.contains(&v) {
                    opts.push(v.clone());
                }
                row["control"] = json!("choice");
                row["selected"] = json!(opts.iter().position(|o| *o == v).unwrap_or(0));
                row["options"] = json!(opts.iter().map(|o| show(name, o)).collect::<Vec<String>>());
            }
        }
        row
    }

    fn processor(&self, can: bool) -> Value {
        let (pk, fk) = (prio_key(), fg_key());
        let cur_p = self.s.cur.get(&pk).cloned().flatten();
        let p = self.prio();
        let (interval, length, boost) = prio_parts(p);
        // what the registry holds now, for the "applied" marks
        let now = prio_parts(match &cur_p {
            Some(RegVal::D(v)) => *v,
            _ => 2,
        });
        let opts = |picked: u32, applied: u32, list: &[(u32, &str)]| -> Value { list.iter().map(|(v, label)| json!({ "value": v, "label": label, "on": picked == *v, "now": applied == *v })).collect() };
        let cur_f = self.s.cur.get(&fk).cloned().flatten();
        let f = self.s.edit.get(&fk).cloned().flatten();
        json!({
            "interval": opts(interval, now.0, &[(0, "Let Windows choose (short on this PC)"), (2, "Short"), (1, "Long")]),
            "length": opts(length, now.1, &[(0, "Let Windows choose (variable on this PC)"), (2, "Fixed length"), (1, "Variable length")]),
            "boost": opts(boost, now.2, &[(0, "No foreground boost (1:1)"), (1, "Foreground double length intervals (2:1)"), (2, "Foreground triple length intervals (3:1)")]),
            "boostEnabled": can && length != 2,
            "foreground": match &f {
                Some(RegVal::D(n)) => *n,
                _ => 200000,
            },
            "foregroundMax": 2_000_000,
            "foregroundStatus": status_json(FG_NAME, &cur_f, &f, &default_of(FG, FG_NAME)),
            "bitmask": format!("{p:X}"),
            "bitmaskStatus": status_json(PRIO_NAME, &cur_p, &Some(RegVal::D(p)), &default_of(PRIO, PRIO_NAME)),
        })
    }

    fn multimedia(&self) -> Value {
        let changed: Vec<String> = self.s.pending(Section::Multimedia).into_iter().map(|c| c.task).collect();
        let task = self.s.tasks.get(self.task).cloned();
        json!({
            "system": SYS_FIELDS.iter().map(|(n, label, _)| self.row("", n, label)).collect::<Vec<Value>>(),
            "tasks": self.s.tasks.iter().map(|t| if changed.contains(t) { format!("{t}  •") } else { t.clone() }).collect::<Vec<String>>(),
            "task": self.task,
            "rows": task.map(|t| TASK_FIELDS.iter().map(|(n, label, _)| self.row(&t, n, label)).collect::<Vec<Value>>()).unwrap_or_default(),
        })
    }
}

impl Backend for SchedulerTab {
    fn view(&self) -> Value {
        let can = common::is_admin();
        let section = self.section;
        let pending = self.s.pending(section);
        let report: Vec<Value> = if self.s.report_for == section { self.s.report.iter().map(|(text, tone)| json!({ "text": text, "tone": tone })).collect() } else { vec![] };
        let sections: Vec<Value> = [(Section::Processor, "Processor scheduling"), (Section::Multimedia, "Multimedia scheduler")]
            .iter()
            .map(|(s, name)| json!({ "id": s, "name": name, "dirty": !self.s.pending(*s).is_empty() }))
            .collect();
        json!({
            "can": can,
            "section": section,
            "sections": sections,
            "applied": self.s.applied(section),
            "processor": if section == Section::Processor { self.processor(can) } else { Value::Null },
            "multimedia": if section == Section::Multimedia { self.multimedia() } else { Value::Null },
            "pending": pending.iter().map(|c| c.line()).collect::<Vec<String>>(),
            "canApply": can && !pending.is_empty(),
            "canCancel": !pending.is_empty(),
            "canUndo": can && self.s.last_save(section).is_some(),
            "report": report,
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let action = parse::<Action>(action)?;
        let can = common::is_admin();
        let section = self.section;
        // the page disables every control and these buttons without administrator rights
        let needs_admin = !matches!(action, Action::Section { .. } | Action::Task { .. } | Action::Cancel | Action::Backup | Action::Reload);
        if needs_admin && !can {
            return Ok(Value::Null);
        }
        // a click meant for the part that is not shown never acts
        let part = match action {
            Action::Interval { .. } | Action::Length { .. } | Action::Boost { .. } | Action::Foreground { .. } | Action::RestoreDefaults => Some(Section::Processor),
            Action::Task { .. } | Action::Choose { .. } | Action::Number { .. } | Action::AffinityAll { .. } | Action::AffinityCpu { .. } => Some(Section::Multimedia),
            _ => None,
        };
        if part.is_some_and(|p| p != section) {
            return Ok(Value::Null);
        }
        match action {
            Action::Section { section } => self.section = section,
            Action::Task { index } => {
                if index < self.s.tasks.len() {
                    self.task = index;
                }
            }
            Action::Interval { value } => {
                if matches!(value, 0..=2) {
                    self.set_parts(|p| p.0 = value);
                }
            }
            Action::Length { value } => {
                if matches!(value, 0..=2) {
                    self.set_parts(|p| p.1 = value);
                }
            }
            Action::Boost { value } => {
                // the boost choices are disabled with a fixed length
                if matches!(value, 0..=2) && prio_parts(self.prio()).1 != 2 {
                    self.set_parts(|p| p.2 = value);
                }
            }
            Action::Foreground { value } => {
                self.s.edit.insert(fg_key(), Some(RegVal::D(value.min(2_000_000))));
            }
            Action::RestoreDefaults => {
                self.s.edit.insert(prio_key(), Some(RegVal::D(2)));
                self.s.edit.insert(fg_key(), default_of(FG, FG_NAME));
            }
            Action::Choose { task, name, index } => {
                if let Some(key) = self.shown_key(&task, &name) {
                    // the extra entry after the fixed choices is the value already picked
                    if !matches!(key.1, "Clock Rate" | "Affinity") {
                        if let Some(v) = choices(key.1).into_iter().nth(index) {
                            self.s.edit.insert(key, v);
                        }
                    }
                }
            }
            Action::Number { task, name, value } => {
                if let Some(key) = self.shown_key(&task, &name) {
                    if key.1 == "Clock Rate" {
                        self.s.edit.insert(key, Some(RegVal::D(value.min(1_000_000))));
                    }
                }
            }
            Action::AffinityAll { task, on } => {
                if let Some(key) = self.shown_key(&task, "Affinity") {
                    self.s.edit.insert(key, Some(RegVal::D(if on { 0 } else { 1 })));
                }
            }
            Action::AffinityCpu { task, cpu, on } => {
                let (n, full) = cpus();
                if let Some(key) = self.shown_key(&task, "Affinity").filter(|_| cpu < n) {
                    let mask = match self.s.edit.get(&key).cloned().flatten() {
                        Some(RegVal::D(m)) => m,
                        _ => 0,
                    };
                    let base = if mask == 0 { full } else { mask };
                    let m = if on { base | (1 << cpu) } else { base & !(1 << cpu) };
                    // all CPUs = 0; never an empty mask
                    if m != 0 {
                        self.s.edit.insert(key, Some(RegVal::D(if m & full == full { 0 } else { m })));
                    }
                }
            }
            Action::Apply => {
                if !self.s.pending(section).is_empty() {
                    self.s.save(section);
                }
            }
            Action::Cancel => {
                let cur: Vec<(Key, Option<RegVal>)> = self.s.cur.iter().filter(|(k, _)| section_of(&k.0) == section).map(|(k, v)| (k.clone(), v.clone())).collect();
                for (k, v) in cur {
                    self.s.edit.insert(k, v);
                }
            }
            Action::Undo => {
                if self.s.last_save(section).is_some() {
                    self.s.undo_last(section);
                }
            }
            Action::Backup => {
                self.s.report_for = section;
                self.s.report = vec![match self.s.backup(section) {
                    Ok(p) => (format!("Backup saved: {p}"), Tone::Ok),
                    Err(e) => (format!("Backup failed: {e}"), Tone::Error),
                }];
            }
            Action::Reload => self.s.reload(section),
        }
        self.settle();
        Ok(Value::Null)
    }
}
