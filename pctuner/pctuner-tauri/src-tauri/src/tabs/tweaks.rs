//! Placeholder backend: replaced when this tab is rebuilt.

use crate::backend::Backend;
use serde_json::Value;

pub struct TweaksTab;

impl TweaksTab {
    pub fn new() -> Self {
        TweaksTab
    }
}

impl Backend for TweaksTab {
    fn view(&self) -> Value {
        Value::Null
    }

    fn act(&mut self, _action: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}
