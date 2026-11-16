//! Processes: background processes that can be switched off or lowered, with undo and a before / after
//! measurement. All logic is pctuner_core::procs; this backend builds the view and forwards the clicks.

use crate::backend::{parse, Backend};
use pctuner_core::common;
use pctuner_core::procs::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ProcsTab {
    p: Procs,
    /// The page has been opened: the first read starts then, not at program start.
    opened: AtomicBool,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Off { exe: String },
    Lower { exe: String },
    Undo { exe: String },
    UndoAll,
    Measure,
    Refresh,
    DismissNote,
}

impl ProcsTab {
    pub fn new() -> Self {
        ProcsTab { p: Procs::new(), opened: AtomicBool::new(false) }
    }

    fn can(&self) -> bool {
        common::is_admin() && !self.p.busy()
    }

    fn row(&self, i: usize, e: &Entry) -> Value {
        let p = &self.p;
        let live = p.live.get(e.exe);
        let active = p.active(e.exe).map(|i| &p.history[i]);
        let (state, tone) = state_text(e, p.setups.get(i), active, live.is_some());
        let can = self.can() && active.is_none();
        json!({
            "exe": e.exe,
            "file": format!("{}.exe", e.exe),
            "title": e.title,
            "what": e.what,
            "loses": e.loses,
            "kind": match e.kind { Kind::Off => "off", Kind::Lower => "lower", Kind::Keep => "keep" },
            "running": live.map(|l| l.count).unwrap_or(0),
            "mb": live.map(|l| l.mb.round() as u64),
            "state": state,
            "tone": tone,
            "changed": active.is_some(),
            "when": active.map(|c| c.time.clone()),
            "detail": active.map(|c| c.writes.iter().map(|w| w.line()).chain(c.notes.iter().cloned()).collect::<Vec<_>>().join("\n")),
            "canOff": can && e.kind == Kind::Off,
            "canLower": can && e.kind != Kind::Keep,
            "canUndo": self.can() && active.is_some(),
        })
    }

    /// The newest measurement against the one before it.
    fn compare(&self) -> Value {
        let m = &self.p.measures;
        let Some(last) = m.last() else { return Value::Null };
        let prev = if m.len() >= 2 { m.get(m.len() - 2) } else { None };
        let side = |x: &Measure| {
            json!({
                "time": x.time,
                "setup": if x.setup.is_empty() { "no changes".to_string() } else { x.setup.join(", ") },
                "listedMb": x.listed_mb().round() as u64,
                "listedCpu": format!("{:.2}", x.listed_cpu()),
                "pcCpu": x.pc_cpu.map(|c| format!("{c:.1}")),
                "ramMb": x.ram_used_mb,
            })
        };
        // one line per changeable process seen in either measurement
        let rows: Vec<Value> = CATALOG
            .iter()
            .filter(|e| e.kind != Kind::Keep)
            .filter(|e| last.procs.contains_key(e.exe) || prev.is_some_and(|p| p.procs.contains_key(e.exe)))
            .map(|e| {
                let f = |x: Option<&Measure>| {
                    x.map(|x| match x.procs.get(e.exe) {
                        Some(p) => json!({ "mb": p.mb.round() as u64, "cpu": p.cpu_pct.map(|c| format!("{c:.2}")) }),
                        None => json!({ "mb": 0, "cpu": "0.00", "gone": true }),
                    })
                };
                json!({ "title": e.title, "file": format!("{}.exe", e.exe), "before": f(prev), "after": f(Some(last)) })
            })
            .collect();
        let saved = prev.map(|p| (p.listed_mb() - last.listed_mb()).round() as i64);
        json!({
            "before": prev.map(side),
            "after": side(last),
            "rows": rows,
            "savedMb": saved,
            "count": m.len(),
            "warnings": prev.map(|p| unlike(p, last)).unwrap_or_default(),
        })
    }
}

impl Backend for ProcsTab {
    fn pump(&mut self) {
        if self.opened.load(Ordering::Relaxed) && self.p.setups.is_empty() && self.p.scan.is_none() {
            self.p.rescan();
        }
        self.p.pump();
    }

    fn busy(&self) -> bool {
        self.p.busy()
    }

    fn view(&self) -> Value {
        self.opened.store(true, Ordering::Relaxed);
        let p = &self.p;
        let rows: Vec<Value> = CATALOG.iter().enumerate().map(|(i, e)| self.row(i, e)).collect();
        let changed = p.history.iter().filter(|c| !c.undone).count();
        let running_mb: f64 = CATALOG.iter().filter(|e| e.kind != Kind::Keep).filter_map(|e| p.live.get(e.exe)).map(|l| l.mb).sum();
        let running = CATALOG.iter().filter(|e| e.kind != Kind::Keep).filter(|e| p.live.contains_key(e.exe)).count();
        json!({
            "loading": p.setups.is_empty(),
            "refreshing": p.scan.is_some(),
            "admin": common::is_admin(),
            "busy": p.work.as_ref().map(|(l, _)| format!("{l}...")),
            "note": p.note.as_ref().map(|(ok, t)| json!({ "ok": ok, "text": t })),
            "rows": rows,
            "hero": {
                "running": running,
                "runningMb": running_mb.round() as u64,
                "changed": changed,
            },
            "canUndoAll": self.can() && changed > 0,
            // the sign-in task that closes switched-off processes again
            "signIn": if p.off_list().is_empty() { None } else { Some(match p.task {
                Some(true) => format!("Closed again 30 s and 2 min after every sign-in by the task {CLOSE_TASK_PATH}{CLOSE_TASK_NAME} (Undo removes it)."),
                Some(false) => "The sign-in task that closes switched-off processes is missing - it is added when PC Tuner runs as administrator.".to_string(),
                None => String::new(),
            }) },
            "measuring": p.measuring.as_ref().map(|(t, _)| json!({ "secs": t.elapsed().as_secs().min(MEASURE_SECS), "of": MEASURE_SECS })),
            "compare": self.compare(),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let admin = common::is_admin();
        match parse::<Action>(action)? {
            Action::Off { exe } if admin => self.p.apply(&exe, What::Off),
            Action::Lower { exe } if admin => self.p.apply(&exe, What::Lower),
            Action::Undo { exe } if admin => self.p.undo(&exe),
            Action::UndoAll if admin => self.p.undo_all(),
            Action::Off { .. } | Action::Lower { .. } | Action::Undo { .. } | Action::UndoAll => {}
            Action::Measure => self.p.start_measure(),
            Action::Refresh => self.p.rescan(),
            Action::DismissNote => self.p.note = None,
        }
        Ok(Value::Null)
    }
}
