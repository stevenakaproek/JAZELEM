//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct SchedulerTab;

impl SchedulerTab {
    pub fn new() -> Self {
        SchedulerTab
    }
}

impl Backend for SchedulerTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
