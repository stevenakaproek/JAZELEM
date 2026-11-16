//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct BloatTab;

impl BloatTab {
    pub fn new() -> Self {
        BloatTab
    }
}

impl Backend for BloatTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
