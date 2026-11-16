//! What every tab gives the shell: a view to draw and actions to take.

use pctuner_core::opwin::{OpEvent, OpWindow, Stage};
use serde_json::Value;

pub trait Backend: Send {
    /// Poll background work. Called about five times a second, also while the window is hidden.
    fn pump(&mut self) {}
    /// Everything the page needs to draw this tab.
    fn view(&self) -> Value;
    /// Something the user did on the page.
    fn act(&mut self, action: Value) -> Result<Value, String>;
    /// A change is being applied right now (closing waits for it).
    fn busy(&self) -> bool {
        false
    }
    /// PC Tuner is closing.
    fn on_exit(&mut self) {}
}

/// Read a typed action from the page.
pub fn parse<T: serde::de::DeserializeOwned>(action: Value) -> Result<T, String> {
    serde_json::from_value(action).map_err(|e| format!("unknown action: {e}"))
}

/// A view from anything serialisable.
pub fn json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// The page's buttons in an operation window. "Go" switches the window to its running stage first, exactly
/// like the egui window does before it reports the click.
pub fn op_event(op: &mut Option<OpWindow>, ev: OpEvent) -> OpEvent {
    if ev == OpEvent::Go {
        if let Some(op) = op {
            op.stage = Stage::Running;
        }
    }
    ev
}

/// An operation window in its running stage counts as busy.
#[allow(dead_code)]
pub fn op_running(op: &Option<OpWindow>) -> bool {
    op.as_ref().is_some_and(|o| o.stage == Stage::Running)
}
