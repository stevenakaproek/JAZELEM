//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct RamledTab;

impl RamledTab {
    pub fn new() -> Self {
        RamledTab
    }
}

impl Backend for RamledTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
