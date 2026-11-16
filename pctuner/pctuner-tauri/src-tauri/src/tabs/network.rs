//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct NetworkTab;

impl NetworkTab {
    pub fn new() -> Self {
        NetworkTab
    }
}

impl Backend for NetworkTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
