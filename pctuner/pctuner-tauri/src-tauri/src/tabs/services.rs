//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct ServicesTab;

impl ServicesTab {
    pub fn new() -> Self {
        ServicesTab
    }
}

impl Backend for ServicesTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
