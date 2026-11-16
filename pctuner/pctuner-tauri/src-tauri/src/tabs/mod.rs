//! One backend per tab. The name is what the page uses in `tab_state` / `tab_action` and `tab:<name>` events.

use crate::backend::Backend;

mod bloat;
mod fan;
mod gpolicy;
mod gpu;
mod network;
mod perf;
mod power;
mod ramled;
mod scheduler;
mod services;
mod startup;
mod tweaks;
mod usb;

pub fn all() -> Vec<(&'static str, Box<dyn Backend>)> {
    vec![
        ("services", Box::new(services::ServicesTab::new())),
        ("startup", Box::new(startup::StartupTab::new())),
        ("bloat", Box::new(bloat::BloatTab::new())),
        ("perf", Box::new(perf::PerfTab::new())),
        ("network", Box::new(network::NetworkTab::new())),
        ("tweaks", Box::new(tweaks::TweaksTab::new())),
        ("gpolicy", Box::new(gpolicy::GpolicyTab::new())),
        ("scheduler", Box::new(scheduler::SchedulerTab::new())),
        ("gpu", Box::new(gpu::GpuTab::new())),
        ("fan", Box::new(fan::FanTab::new())),
        ("ramled", Box::new(ramled::RamledTab::new())),
        ("usb", Box::new(usb::UsbTab::new())),
        ("power", Box::new(power::PowerTab::new())),
    ]
}
