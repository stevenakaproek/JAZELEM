//! The telemetry band shown at the top of every page: processor, memory and graphics card readings,
//! refreshed once a second while a window exists. Read-only.
//!
//! Sources: the fan helper's status line (CPU temperature, voltage, power, clock - administrator only),
//! Windows (CPU load, memory in use) and the NVIDIA driver (the graphics card).

use pctuner_core::{fan, gpu, sysinfo};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

static LAST: Mutex<Option<Value>> = Mutex::new(None);

/// One reading: the text as shown, its unit, and value / max for the bar under it (no max = no bar).
fn item(key: &str, label: &str, unit: &str, value: Option<f64>, digits: usize, max: Option<f64>) -> Value {
    json!({
        "key": key,
        "label": label,
        "unit": unit,
        "text": value.map(|v| format!("{v:.digits$}")).unwrap_or_else(|| "-".into()),
        "value": value,
        "max": max,
    })
}

fn build(load: &mut sysinfo::CpuLoad, cpu_name: &str, gpu_name: &Option<String>, gpu_max: &Option<(f64, f64)>) -> Value {
    let st = fan::latest_status();
    let info = st.as_ref().and_then(|s| s.cpu_info.clone());
    let f = |v: Option<f32>| v.map(f64::from);
    let own_load = load.sample().map(f64::from);
    let cpu = json!({
        "name": info.as_ref().map(|i| sysinfo::short_cpu_name(&i.name)).filter(|n| n != "Processor").unwrap_or_else(|| cpu_name.to_string()),
        "items": [
            item("temp", "Temperature", "°C", st.as_ref().and_then(|s| f(s.cpu)), 1, Some(100.0)),
            item("volt", "Core voltage", "V", info.as_ref().and_then(|i| f(i.volt)), 2, None),
            item("power", "Package power", "W", info.as_ref().and_then(|i| f(i.power)), 1, Some(170.0)),
            item("load", "Load", "%", info.as_ref().and_then(|i| f(i.load)).or(own_load), 1, Some(100.0)),
            item("clock", "Clock", "MHz", info.as_ref().and_then(|i| f(i.clock)), 0, Some(5600.0)),
        ],
    });
    let mem = sysinfo::memory();
    let memory = json!({
        "name": "Memory",
        "items": [
            item("used_pct", "In use", "%", mem.map(|(u, t)| 100.0 * u as f64 / t.max(1) as f64), 1, Some(100.0)),
            item("used", "In use", "MB", mem.map(|(u, _)| u as f64), 0, mem.map(|(_, t)| t as f64)),
        ],
    });
    let l = gpu::gpu_live();
    let (power_max, clock_max) = gpu_max.unwrap_or((360.0, 3210.0));
    let g = |v: Option<f64>| v;
    let gpu = json!({
        "name": gpu_name.clone().unwrap_or_else(|| "Graphics card".into()),
        "items": [
            item("temp", "Temperature", "°C", g(l.as_ref().map(|l| l.temp as f64)), 1, Some(100.0)),
            item("volt", "Core voltage", "V", l.as_ref().and_then(|l| l.volt_mv).map(|v| v / 1000.0), 2, None),
            item("power", "Power draw", "W", l.as_ref().map(|l| l.power), 1, Some(power_max)),
            item("load", "Load", "%", l.as_ref().map(|l| l.util as f64), 1, Some(100.0)),
            item("clock", "Core clock", "MHz", l.as_ref().map(|l| l.core as f64), 0, Some(clock_max)),
            item("vram", "Memory used", "MB", l.as_ref().map(|l| l.vram_used_mb as f64), 0, l.as_ref().filter(|l| l.vram_total_mb > 0).map(|l| l.vram_total_mb as f64)),
        ],
    });
    json!({ "groups": [{ "kind": "cpu", "name": cpu["name"], "items": cpu["items"] }, { "kind": "memory", "name": memory["name"], "items": memory["items"] }, { "kind": "gpu", "name": gpu["name"], "items": gpu["items"] }] })
}

/// Start the once-a-second sampler. Nothing is read while no window exists (minimized to tray).
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let cpu_name = sysinfo::cpu_name();
        let card = pctuner_core::nvml::Gpu::open().ok();
        let gpu_name = card.as_ref().and_then(|g| g.name().ok());
        // bar ends: the card's default power limit and its top core clock
        let gpu_max = card.as_ref().and_then(|g| Some((g.power_limits().ok()?.2 as f64, g.core_range().ok()?.1 as f64)));
        drop(card);
        let mut load = sysinfo::CpuLoad::default();
        loop {
            if app.get_webview_window("main").is_some() {
                let v = build(&mut load, &cpu_name, &gpu_name, &gpu_max);
                if let Ok(mut l) = LAST.lock() {
                    *l = Some(v.clone());
                }
                let _ = app.emit("telemetry", v);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// The newest readings (for the page's first paint).
#[tauri::command]
pub fn telemetry() -> Value {
    LAST.lock().ok().and_then(|l| l.clone()).unwrap_or(Value::Null)
}
