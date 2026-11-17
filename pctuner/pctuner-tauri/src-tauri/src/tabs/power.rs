//! Power window: restart into the BIOS, or shut down so the next start opens the BIOS.

use crate::backend::{parse, Backend};
use pctuner_core::power::{working_timed_out, Choice, Power, Stage};
use serde_json::{json, Value};
use std::time::Instant;

pub struct PowerTab {
    p: Power,
    /// The clock the working stage is timed with.
    t0: Instant,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    /// The window was opened: check the firmware type again, start from the choices.
    Open,
    Pick { choice: Choice },
    Back,
}

impl PowerTab {
    pub fn new() -> Self {
        PowerTab { p: Power::new(), t0: Instant::now() }
    }
    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }
}

impl Backend for PowerTab {
    fn pump(&mut self) {
        self.p.pump();
    }

    fn view(&self) -> Value {
        let choice = |c: Choice| json!({ "id": c, "title": c.title(), "text": c.text() });
        let stage = match self.p.stage {
            Stage::Choose => json!({ "kind": "choose" }),
            Stage::Working(c, started) if working_timed_out(started, self.now()) => json!({ "kind": "timed_out", "title": c.failed_title() }),
            Stage::Working(c, _) => json!({ "kind": "working", "title": c.working_title() }),
        };
        json!({
            "stage": stage,
            "choices": [choice(Choice::RestartToBios), choice(Choice::ShutdownToBios)],
            "blocked": self.p.blocked(),
            "canPick": self.p.can_pick(),
            "checking": self.p.uefi.is_none(),
            "error": self.p.error,
        })
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        match parse::<Action>(action)? {
            Action::Open => {
                if matches!(self.p.stage, Stage::Choose) {
                    self.p = Power::new();
                }
            }
            Action::Pick { choice } => {
                if self.p.can_pick() && matches!(self.p.stage, Stage::Choose) {
                    let now = self.now();
                    self.p.pick(choice, now);
                }
            }
            Action::Back => self.p.back(),
        }
        Ok(Value::Null)
    }
}
