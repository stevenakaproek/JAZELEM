//! One backend per tab. The name is what the page uses in `tab_state` / `tab_action` and `tab:<name>` events.

use crate::backend::Backend;

mod perf;
mod power;

pub fn all() -> Vec<(&'static str, Box<dyn Backend>)> {
    vec![("perf", Box::new(perf::PerfTab::new())), ("power", Box::new(power::PowerTab::new()))]
}
