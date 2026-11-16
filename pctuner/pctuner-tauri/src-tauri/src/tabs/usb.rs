//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct UsbTab;

impl UsbTab {
    pub fn new() -> Self {
        UsbTab
    }
}

impl Backend for UsbTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
