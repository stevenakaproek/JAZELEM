//! Health: drive health, CHKDSK, DISM and SFC with live progress. All logic is pctuner_core::health;
//! this backend builds the view and forwards the clicks.

use crate::backend::{parse, Backend};
use pctuner_core::common;
use pctuner_core::health::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct HealthTab {
    h: Health,
    /// The page has been opened: the drive check starts then, not at program start.
    opened: AtomicBool,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    RunAll,
    Run { step: String },
    Stop,
    FixChkdsk,
    OpenLog { step: String },
    RefreshDisk,
    DismissNote,
}

impl HealthTab {
    pub fn new() -> Self {
        HealthTab { h: Health::new(), opened: AtomicBool::new(false) }
    }

    fn level_str(l: &Level) -> &'static str {
        match l {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    fn step(&self, s: StepId) -> Value {
        let h = &self.h;
        let run = h.run.as_ref().filter(|r| r.step == s);
        let queued = h.queue.contains(&s);
        let last = h.history.get(s.key());
        let live = run.map(|r| {
            let since = r.last_move.elapsed().as_secs();
            let acts: Vec<String> = h.acts.iter().filter(|a| a.cpu >= 0.1 || a.mb_s >= 0.01).map(|a| format!("{} {:.0} % CPU · {:.1} MB/s", a.name, a.cpu, a.mb_s)).collect();
            json!({
                "phase": r.phase.label(),
                "pct": r.pct,
                "stage": r.stage,
                "elapsed": r.started.elapsed().as_secs(),
                "sinceMove": since,
                "activity": if acts.is_empty() { None } else { Some(acts.join(" · ")) },
                "busy": h.since_busy() < 10,
                "note": pause_note(r.step, r.phase, r.pct, since),
                "stalled": stalled(since, h.since_busy()),
                "stopping": r.stopping,
            })
        });
        json!({
            "id": s.key(),
            "title": s.title(),
            "running": live,
            "queued": queued,
            "canRun": common::is_admin() && !h.busy(),
            "last": last.map(|r| json!({
                "time": r.time,
                "level": Self::level_str(&r.level),
                "summary": r.summary,
                "detail": r.detail,
                "secs": r.secs,
                "hasLog": !r.log.is_empty(),
                "canFix": r.can_fix && common::is_admin() && !h.busy(),
            })),
        })
    }

    fn disk(&self) -> Value {
        let h = &self.h;
        let Some(d) = &h.disk else { return json!({ "loading": true }) };
        let drives: Vec<Value> = d
            .drives
            .iter()
            .map(|x| {
                let w = drive_warnings(x);
                json!({
                    "name": x.name,
                    "kind": format!("{} {}", x.bus, x.media).trim().to_string(),
                    "sizeGb": x.size_gb,
                    "health": x.health,
                    "wear": x.wear,
                    "temp": x.temp,
                    "errors": match (x.read_errors, x.write_errors) { (None, None) => None, (r, w) => Some(r.unwrap_or(0) + w.unwrap_or(0)) },
                    "hours": x.hours,
                    "warnings": w,
                    "level": if w.is_empty() { "ok" } else if x.health.eq_ignore_ascii_case("Unhealthy") { "error" } else { "warn" },
                })
            })
            .collect();
        json!({
            "loading": false,
            "refreshing": h.disk_job.is_some(),
            "time": d.time,
            "error": d.error,
            "drives": drives,
            "cFree": d.c_free_gb,
            "cSize": d.c_size_gb,
            "lowSpace": low_space(d),
        })
    }

    /// "All good" / "Needs attention" / "Not checked yet" from the last results.
    fn overall(&self) -> (&'static str, &'static str) {
        let h = &self.h;
        let mut levels: Vec<&Level> = STEPS.iter().filter_map(|s| h.history.get(s.key()).map(|r| &r.level)).collect();
        let disk_bad = h.disk.as_ref().is_some_and(|d| d.drives.iter().any(|x| !drive_warnings(x).is_empty()) || low_space(d).is_some());
        if levels.is_empty() {
            return ("Not checked yet", "Text");
        }
        if levels.iter().any(|l| **l == Level::Error) {
            return ("Needs attention", "Error");
        }
        levels.retain(|l| **l == Level::Warn);
        if !levels.is_empty() || disk_bad {
            return ("Needs attention", "Warn");
        }
        ("All good", "Ok")
    }
}

impl Backend for HealthTab {
    fn pump(&mut self) {
        if self.opened.load(Ordering::Relaxed) && self.h.disk.is_none() && self.h.disk_job.is_none() {
            self.h.refresh_disk();
        }
        self.h.pump();
    }

    fn busy(&self) -> bool {
        self.h.busy()
    }

    fn on_exit(&mut self) {
        self.h.on_exit();
    }

    fn view(&self) -> Value {
        self.opened.store(true, Ordering::Relaxed);
        let h = &self.h;
        let (overall, tone) = self.overall();
        let admin = common::is_admin();
        let disabled: Vec<&str> = disabled_needed(start_type).iter().map(|(_, d)| *d).collect();
        json!({
            "admin": admin,
            "overall": overall,
            "tone": tone,
            "lastAll": h.last_all,
            "running": h.busy(),
            "canRunAll": admin && !h.busy(),
            "canStop": h.run.as_ref().is_some_and(|r| !r.stopping),
            "note": h.note.as_ref().map(|(ok, t)| json!({ "ok": ok, "text": t })),
            "prep": if disabled.is_empty() { None } else { Some(format!("{} {} Disabled. DISM and SFC hang or fail without {}, so they are set to Manual just for the check and put back to Disabled afterwards.", disabled.join(", "), if disabled.len() == 1 { "is" } else { "are" }, if disabled.len() == 1 { "it" } else { "them" })) },
            "disk": self.disk(),
            "steps": STEPS.iter().map(|s| self.step(*s)).collect::<Vec<_>>(),
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let admin = common::is_admin();
        match parse::<Action>(action)? {
            Action::RunAll if admin => self.h.run_all(),
            Action::Run { step } if admin => {
                if let Some(s) = StepId::from_key(&step) {
                    self.h.run_one(s);
                }
            }
            Action::FixChkdsk if admin => self.h.fix_chkdsk(),
            Action::RunAll | Action::Run { .. } | Action::FixChkdsk => {}
            Action::Stop => self.h.stop(),
            Action::OpenLog { step } => {
                if let Some(r) = StepId::from_key(&step).and_then(|s| self.h.history.get(s.key())) {
                    if !r.log.is_empty() {
                        // not common::run: that waits until Notepad is closed
                        let _ = std::process::Command::new("notepad.exe").arg(&r.log).spawn();
                    }
                }
            }
            Action::RefreshDisk => self.h.refresh_disk(),
            Action::DismissNote => self.h.note = None,
        }
        Ok(Value::Null)
    }
}
