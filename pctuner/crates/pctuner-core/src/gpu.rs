//! RTX 5080 Profiles: undervolt presets and custom values set straight on the card through the NVIDIA
//! driver - no MSI Afterburner or other tool needed.
//!
//! How a preset works: "2800 MHz at 925 mV" = a core offset that lifts the card's own voltage/frequency
//! curve until it reaches 2800 MHz at 925 mV (read once from the driver, `nvapi::read_curve`), plus a core
//! clock limit at 2800 MHz so it never climbs to higher voltages. Memory offset and power limit as before.
//! Everything is set with NVIDIA's documented management library (`nvml`). The driver forgets these at
//! every restart, so a sign-in task (`--gpu-guard`) sets the saved tune again - and, during a safety trial,
//! first checks for a crash and goes back to Stock instead.
//!
//! Logic only: the presets, applying and reading back, the trial with its sign-in guard (`guard_main`) and
//! the page state. The UI draws `Gpu` and forwards the operation window's events to `on_event`.

use crate::common::{self, Job, Tone};
use crate::nvml::{self, CLOCK_GRAPHICS, CLOCK_MEM};
use crate::opwin::{OpEvent, OpItem, OpWindow, Stage};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use winreg::enums::*;
use winreg::RegKey;

pub struct Preset {
    pub name: &'static str,
    pub target_mhz: i32,
    pub target_mv: i32,
    pub mem: i32,
    pub power: i32,
    pub text: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Stock", target_mhz: 0, target_mv: 0, mem: 0, power: 100,
        text: "Factory behaviour - no undervolt, no memory overclock. Use this to reset or to compare." },
    // NV-UV-Play's RTX 5080 tiers (v2.1.11.1, read from its log on this card, 2026-11-16)
    Preset { name: "Eco", target_mhz: 2650, target_mv: 850, mem: 0, power: 88,
        text: "Coolest and quietest. 2650 MHz at 850 mV, power limit 88 %." },
    Preset { name: "MFG", target_mhz: 2650, target_mv: 850, mem: 0, power: 100,
        text: "For DLSS Multi Frame Generation. The Eco curve (2650 MHz at 850 mV) with the full power limit." },
    Preset { name: "Balanced", target_mhz: 2750, target_mv: 900, mem: 0, power: 90,
        text: "Everyday gaming. 2750 MHz at 900 mV, power limit 90 %." },
    Preset { name: "Performance", target_mhz: 2980, target_mv: 925, mem: 0, power: 100,
        text: "High fps. 2980 MHz at 925 mV, full power limit." },
    Preset { name: "Max", target_mhz: 3150, target_mv: 975, mem: 0, power: 100,
        text: "Most fps. 3150 MHz at 975 mV, full power limit. Runs warmest of the undervolts." },
];

/// The fifth choice after the presets: values the user sets.
pub const CUSTOM: usize = 6;
pub const CUSTOM_NAME: &str = "Custom";

/// The most a preset may lift the card's stock curve. NV-UV-Play's tiers need up to +405 MHz at their voltage on
/// this card (Max at 975 mV); PC Tuner lifts the whole curve by that one amount, while NV-UV-Play lifts the lower
/// voltages even more (+368 MHz at 850 mV for Performance). So the limit is +450.
pub const MAX_LIFT: i32 = 450;

pub fn tune_name(idx: usize) -> &'static str {
    PRESETS.get(idx).map(|p| p.name).unwrap_or(CUSTOM_NAME)
}

pub fn tune_index(name: &str) -> Option<usize> {
    PRESETS.iter().position(|p| p.name == name).or((name == CUSTOM_NAME).then_some(CUSTOM))
}

// ------------------------------------------------------------------ tunings

/// What is set on the card. `max_mhz` 0 = no core clock limit; `power` is a percentage of the card's
/// default power limit; offsets in MHz (memory as the clock nvidia-smi / Afterburner show).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Tuning {
    pub max_mhz: u32,
    pub core: i32,
    pub mem: i32,
    pub power: u32,
}

pub const STOCK: Tuning = Tuning { max_mhz: 0, core: 0, mem: 0, power: 100 };

impl Tuning {
    pub fn describe(&self) -> String {
        if *self == STOCK {
            return "no offsets, no clock limit, power 100%".into();
        }
        let limit = if self.max_mhz == 0 { "no clock limit".to_string() } else { format!("max {} MHz", self.max_mhz) };
        format!("{limit}, core {:+} MHz, memory {:+} MHz, power {}%", self.core, self.mem, self.power)
    }

    /// Keep custom values inside what the card allows (and PC Tuner's own safety range).
    pub fn clamp(&self, l: &Limits) -> Tuning {
        Tuning {
            max_mhz: self.max_mhz.clamp(l.clock.0, l.clock.1),
            core: self.core.clamp(l.core.0, l.core.1),
            mem: self.mem.clamp(l.mem.0, l.mem.1),
            power: self.power.clamp(l.power.0, l.power.1),
        }
    }
}

/// What the card allows for custom values (already narrowed to PC Tuner's safety range).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub clock: (u32, u32),
    pub core: (i32, i32),
    pub mem: (i32, i32),
    pub power: (u32, u32),
    pub power_default_w: u32,
}

/// The card's limits narrowed to: core -500..+300 MHz, memory 0..+3000 MHz, clock limit 1500 MHz upward.
pub fn limits_from(core: nvml::Offset, mem: nvml::Offset, factor: f64, clock: (u32, u32), power_w: (u32, u32, u32)) -> Limits {
    let (lo, hi, def) = power_w;
    let def = def.max(1);
    let mem_max = (mem.max as f64 / factor).floor() as i32;
    Limits {
        clock: (clock.0.max(1500), clock.1.max(1500)),
        core: (core.min.max(-500), core.max.min(MAX_LIFT)),
        mem: (0, mem_max.clamp(0, 3000)),
        power: ((lo * 100).div_ceil(def), hi * 100 / def),
        power_default_w: def,
    }
}

/// Saved in gpu-tuning.json: what was applied last (the sign-in task sets it again), the custom values,
/// each preset's core offset worked out from this card's curve, and how NVML's memory offset maps onto
/// the clock (`mem_factor`, 0 = not measured yet).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Saved {
    #[serde(default)]
    pub applied: String,
    #[serde(default)]
    pub tuning: Option<Tuning>,
    #[serde(default)]
    pub custom: Option<Tuning>,
    #[serde(default)]
    pub lifts: Vec<i32>,
    #[serde(default)]
    pub mem_factor: f64,
}

pub const SAVED_FILE: &str = "gpu-tuning.json";

impl Saved {
    pub fn factor(&self) -> f64 {
        if self.mem_factor > 0.0 {
            self.mem_factor
        } else {
            1.0
        }
    }
}

pub fn load_saved() -> Saved {
    common::load_json(SAVED_FILE)
}

/// NVML's memory offset for a clock offset (nvidia-smi MHz).
pub fn mem_raw(mem: i32, factor: f64) -> i32 {
    (mem as f64 * factor).round() as i32
}

pub const CURVE_CHANGED: &str = "Another program has changed the card's voltage/frequency curve (NV-UV-Play, MSI Afterburner or similar), so its stock curve cannot be read. Close that program - or restart the PC - then try again.";

/// The stock curve keeps rising to its last point; one that ends in a flat line was undervolted by another
/// program (the driver's global offset does not show per-point changes).
pub fn curve_is_flattened(curve: &[crate::nvapi::VfPoint]) -> bool {
    let top: Vec<f32> = curve.iter().rev().take(8).map(|p| p.mhz).collect();
    top.len() == 8 && top.iter().fold(f32::MIN, |a, b| a.max(*b)) - top.iter().fold(f32::MAX, |a, b| a.min(*b)) < 10.0
}

/// Each preset's core offset from the card's stock curve: target clock minus the stock clock at the
/// target voltage (the first point at or above it). Stock is 0.
pub fn preset_lifts(curve: &[crate::nvapi::VfPoint]) -> Result<Vec<i32>, String> {
    if curve_is_flattened(curve) {
        return Err(CURVE_CHANGED.into());
    }
    PRESETS
        .iter()
        .map(|p| {
            if p.target_mhz == 0 {
                return Ok(0);
            }
            let at = crate::nvapi::clock_at(curve, p.target_mv as f32).ok_or(format!("No curve point at or above {} mV.", p.target_mv))?;
            let lift = (p.target_mhz as f32 - at.mhz).round() as i32;
            if lift > MAX_LIFT {
                return Err(format!("{} needs +{lift} MHz over this card's stock curve at {} mV - too aggressive.", p.name, at.mv));
            }
            Ok(lift)
        })
        .collect()
}

/// The presets' core offsets: worked out once from the card's curve and kept in gpu-tuning.json.
pub fn lifts(saved: &mut Saved) -> Result<Vec<i32>, String> {
    if saved.lifts.len() == PRESETS.len() {
        return Ok(saved.lifts.clone());
    }
    let g = nvml::Gpu::open()?;
    if g.offset(CLOCK_GRAPHICS)?.mhz != 0 {
        return Err("A core offset is set on the card right now, so its stock curve cannot be read. Use Reset to stock once, then try again.".into());
    }
    let curve = crate::nvapi::read_curve()?;
    let l = preset_lifts(&curve)?;
    saved.lifts = l.clone();
    common::save_json(SAVED_FILE, saved);
    Ok(l)
}

/// The tuning of a preset (with this card's core offsets) or the custom values.
pub fn tuning_of(idx: usize, lifts: &[i32], custom: Option<Tuning>) -> Tuning {
    match PRESETS.get(idx) {
        Some(p) if p.target_mhz == 0 => STOCK,
        Some(p) => Tuning { max_mhz: p.target_mhz as u32, core: lifts.get(idx).copied().unwrap_or(0), mem: p.mem, power: p.power as u32 },
        None => custom.unwrap_or_else(|| tuning_of(3, lifts, None)),
    }
}

// ------------------------------------------------------------------ Afterburner (only to warn about it)

/// Another GPU tuner is running (MSI Afterburner or NV-UV-Play): it sets the card too.
pub fn ab_running() -> bool {
    common::run("tasklist", &["/NH", "/FO", "CSV"])
        .map(|(_, o)| {
            let o = o.to_lowercase();
            o.contains("\"msiafterburner.exe\"") || o.contains("\"nv-uv-play.exe\"")
        })
        .unwrap_or(false)
}

/// Afterburner is set to start with Windows (a Run entry or an enabled scheduled task). Read-only.
pub fn ab_autostart() -> bool {
    let run = |hk, path: &str| {
        RegKey::predef(hk)
            .open_subkey(path)
            .map(|k| k.enum_values().filter_map(|v| v.ok()).any(|(n, v)| format!("{n} {v}").to_lowercase().contains("msiafterburner")))
            .unwrap_or(false)
    };
    let in_run = run(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run")
        || run(HKEY_LOCAL_MACHINE, r"Software\Microsoft\Windows\CurrentVersion\Run")
        || run(HKEY_LOCAL_MACHINE, r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run");
    in_run
        || common::run("schtasks", &["/Query", "/FO", "CSV", "/NH"])
            .map(|(_, o)| o.lines().any(|l| { let l = l.to_lowercase(); l.contains("msiafterburner") && !l.contains("disabled") }))
            .unwrap_or(false)
}

pub fn close_ab() -> Result<(), String> {
    let _ = common::run("taskkill", &["/IM", "NV-UV-Play.exe"]);
    common::run("taskkill", &["/F", "/IM", "MSIAfterburner.exe"]).map(|_| ())
}

// ------------------------------------------------------------------ live GPU readout

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub core: i64,
    pub mem: i64,
    pub mem_stock: i64,
    /// the memory clock over its stock speed (only meaningful at full clocks)
    pub mem_offset: i64,
    pub power: f64,
    pub temp: i64,
    pub pstate: String,
    pub util: i64,
    /// memory only runs at full speed in P0
    pub full_clocks: bool,
    /// the offsets the driver reports right now (core MHz, NVML memory value)
    pub core_ofs: i32,
    pub mem_ofs_raw: i32,
    /// core voltage, mV (None when the driver does not say)
    pub volt_mv: Option<f64>,
    /// video memory in use / installed, MB
    pub vram_used_mb: i64,
    pub vram_total_mb: i64,
    /// GPU fan speed, %
    pub fan_pct: i64,
}

/// One reading through the NVIDIA driver; None when it cannot be read.
pub fn gpu_live() -> Option<Live> {
    let g = nvml::Gpu::open().ok()?;
    let r = g.read();
    let core_ofs = g.offset(CLOCK_GRAPHICS).ok()?.mhz;
    let mem_ofs_raw = g.offset(CLOCK_MEM).ok()?.mhz;
    let (mem, stock) = (r.mem_mhz as i64, r.max_mem_mhz as i64);
    Some(Live {
        core: r.core_mhz as i64,
        mem,
        mem_stock: stock,
        mem_offset: mem - stock,
        power: r.power_w as f64,
        temp: r.temp_c as i64,
        pstate: format!("P{}", r.pstate),
        util: r.util as i64,
        full_clocks: r.pstate == 0,
        core_ofs,
        mem_ofs_raw,
        volt_mv: crate::nvapi::core_voltage().map(f64::from),
        vram_used_mb: r.vram_used_mb as i64,
        vram_total_mb: r.vram_total_mb as i64,
        fan_pct: r.fan_pct as i64,
    })
}

/// What the card runs: the saved tune when the driver reports its offsets, Stock when it reports none
/// (also right after a restart, before the sign-in task has set the tune again), None = offsets set by
/// something else. Without a reading, the last applied tune.
pub fn current_tune(saved: &Saved, live: Option<&Live>) -> Option<usize> {
    let Some(l) = live else { return tune_index(&saved.applied) };
    if let (Some(i), Some(t)) = (tune_index(&saved.applied), saved.tuning) {
        if l.core_ofs == t.core && l.mem_ofs_raw == mem_raw(t.mem, saved.factor()) {
            return Some(i);
        }
    }
    (l.core_ofs == 0 && l.mem_ofs_raw == 0).then_some(0)
}

pub struct State {
    pub saved: Saved,
    pub live: Option<Live>,
    pub current: Option<usize>,
    pub limits: Option<Limits>,
    /// why the card could not be read (no NVIDIA driver, ...)
    pub error: Option<String>,
    pub ab_running: bool,
    pub ab_autostart: bool,
    /// the power limit the card has right now, % of its default
    pub power_pct: Option<u32>,
}

pub fn card_limits(saved: &Saved) -> Result<Limits, String> {
    let g = nvml::Gpu::open()?;
    let (lo, hi, def, _) = g.power_limits()?;
    Ok(limits_from(g.offset(CLOCK_GRAPHICS)?, g.offset(CLOCK_MEM)?, saved.factor(), g.core_range()?, (lo, hi, def)))
}

pub fn current_state() -> State {
    let saved = load_saved();
    let live = gpu_live();
    let limits = card_limits(&saved);
    let current = current_tune(&saved, live.as_ref());
    State {
        current,
        live,
        error: limits.as_ref().err().cloned(),
        limits: limits.ok(),
        saved,
        ab_running: ab_running(),
        ab_autostart: ab_autostart(),
        power_pct: power_now_pct(),
    }
}

/// How sure we are that the card got a tune.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Applied {
    /// it could not be set
    Failed,
    /// the driver reports exactly the tune's offsets
    Confirmed,
    /// set, but it could not be read back
    Unverified,
    /// the driver reports different offsets
    Mismatch,
}

/// Result of one step of an operation: Ok(note, may be empty) or Err(reason).
pub type StepResult = Result<String, String>;

pub const STEP_SET: &str = "set";
pub const STEP_CONFIRM: &str = "confirm";
pub const STEP_SAVE: &str = "save";
pub const STEP_TRIAL: &str = "trial";
pub const STEP_END_TRIAL: &str = "endtrial";

/// Nearest of the usual memory-offset scales (NVML value per nvidia-smi MHz).
pub fn snap_factor(f: f64) -> f64 {
    *[0.5, 1.0, 2.0].iter().min_by(|a, b| (*a - f).abs().total_cmp(&(*b - f).abs())).unwrap()
}

/// Measure how NVML's memory offset shows on the memory clock: 0, then +100 for a moment. Only at full
/// clocks (P0); None when it cannot be measured. Leaves the memory offset at +100 (set next anyway).
fn measure_mem_factor(g: &nvml::Gpu) -> Option<f64> {
    g.set_offset(CLOCK_MEM, 0).ok()?;
    std::thread::sleep(Duration::from_millis(1000));
    let a = g.read();
    g.set_offset(CLOCK_MEM, 100).ok()?;
    std::thread::sleep(Duration::from_millis(1500));
    let b = g.read();
    let d = b.mem_mhz as f64 - a.mem_mhz as f64;
    (a.pstate == 0 && b.pstate == 0 && (20.0..=400.0).contains(&d)).then(|| snap_factor(100.0 / d))
}

/// Set a tuning on the card (runs in a Job), then read it back. Every step is reported as it completes.
pub fn apply_tuning(t: &Tuning, report: &dyn Fn(&str, StepResult)) -> Applied {
    let set = || -> Result<(nvml::Gpu, f64, String), String> {
        let g = nvml::Gpu::open()?;
        let mut saved = load_saved();
        let mut note = String::new();
        if saved.mem_factor <= 0.0 && t.mem != 0 {
            match measure_mem_factor(&g) {
                Some(f) => {
                    saved.mem_factor = f;
                    common::save_json(SAVED_FILE, &saved);
                }
                None => note = "Memory offset scale not measured yet (the card was not at full clocks) - checked again next time.".into(),
            }
        }
        let factor = saved.factor();
        g.set_offset(CLOCK_GRAPHICS, t.core)?;
        g.set_offset(CLOCK_MEM, mem_raw(t.mem, factor))?;
        if t.max_mhz == 0 {
            g.reset_clocks()?;
        } else {
            let (lo, _) = g.core_range()?;
            g.lock_clocks(lo, t.max_mhz)?;
        }
        let (lo, hi, def, cur) = g.power_limits()?;
        let want = (def * t.power / 100).clamp(lo, hi);
        if want != cur {
            g.set_power_limit(want)?;
        }
        Ok((g, factor, note))
    };
    let (g, factor, note) = match set() {
        Ok(x) => x,
        Err(e) => {
            report(STEP_SET, Err(e));
            return Applied::Failed;
        }
    };
    report(STEP_SET, Ok(note));
    let back = (|| Ok::<_, String>((g.offset(CLOCK_GRAPHICS)?.mhz, g.offset(CLOCK_MEM)?.mhz)))();
    let (v, r) = match back {
        Ok((c, m)) if c == t.core && m == mem_raw(t.mem, factor) => (Applied::Confirmed, Ok(format!("The driver reports core {c:+} MHz, memory {:+} MHz.", t.mem))),
        Ok((c, m)) => (Applied::Mismatch, Err(format!("The driver reports core {c:+} MHz and memory value {m:+} - not what was set. Another tool (Afterburner?) may have changed it."))),
        Err(e) => (Applied::Unverified, Err(format!("Set, but it could not be read back: {e}"))),
    };
    report(STEP_CONFIRM, r);
    v
}

// ------------------------------------------------------------------ trial + auto-revert + sign-in re-apply
//
// Applying a tune the user has not confirmed yet starts a TRIAL: gpu-trial.json (pending). The sign-in
// task (`PCTuner.exe --gpu-guard`) runs after every sign-in while a tune other than Stock is saved or a
// trial is armed: after a crash during a trial it puts the GPU back on Stock, otherwise it sets the saved
// tune again (the driver forgets it at every restart). Keep / Revert / applying Stock end the trial.

pub const TRIAL_FILE: &str = "gpu-trial.json";
pub const CONFIRMED_FILE: &str = "gpu-confirmed.json";
pub const GUARD_LOG: &str = "gpu-guard.log";
pub const TASK_PATH: &str = r"\PCTuner\";
pub const TASK_NAME: &str = "GpuApply";
/// The task's name before PC Tuner set the GPU itself (removed whenever the task is updated).
pub const OLD_TASK_NAME: &str = "GpuTrialGuard";
/// Seconds: Win32_OperatingSystem.LastBootUpTime can shift slightly when the clock is synchronised.
pub const BOOT_TOLERANCE: i64 = 90;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Trial {
    /// tune being tested
    #[serde(default)]
    pub preset: String,
    /// tune to go back to with "Revert now" (the last confirmed one, or Stock)
    #[serde(default)]
    pub previous: String,
    /// the previous tune's values when it was Custom
    #[serde(default)]
    pub previous_tuning: Option<Tuning>,
    /// unix seconds
    #[serde(default)]
    pub applied_at: i64,
    /// unix seconds of the boot the trial started in (0 = unknown)
    #[serde(default)]
    pub boot_time: i64,
    /// "pending" | "reverted"
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reason: String,
    /// true when the sign-in check could not be registered: the trial has NO automatic revert
    #[serde(default)]
    pub unguarded: bool,
    /// "reverted" only: the guard saw Stock confirmed (the driver reported no offsets).
    /// false = Stock was sent but not confirmed, and the sign-in check stays to try again.
    #[serde(default)]
    pub verified: bool,
    /// "reverted" only: how many times the sign-in check has sent Stock for this trial
    #[serde(default)]
    pub attempts: u32,
    /// "reverted" only: Stock actually reached the GPU at one attempt at least (false = every attempt failed
    /// to set it, so the GPU may still run the crashing tune)
    #[serde(default)]
    pub sent: bool,
    /// "reverted" only: how many of the attempts actually set Stock on the GPU
    #[serde(default)]
    pub sends: u32,
    /// "reverted" only: Stock was sent MAX_REVERT_ATTEMPTS times without confirmation and the sign-in check stopped
    #[serde(default)]
    pub gave_up: bool,
    /// "reverted" only: what went wrong at the last attempt (empty when Stock was confirmed)
    #[serde(default)]
    pub last_error: String,
}

/// The sign-in check sends Stock at most this many times after a crash, then stops and says so.
pub const MAX_REVERT_ATTEMPTS: u32 = 3;

impl Trial {
    pub fn pending(&self) -> bool {
        self.status == "pending"
    }
    /// reverted after a crash, but Stock was not confirmed: the sign-in check sends it again (until it gives up)
    pub fn revert_unverified(&self) -> bool {
        self.status == "reverted" && !self.verified && !self.gave_up
    }
    /// the sign-in check still has work to do for this trial
    pub fn armed(&self) -> bool {
        self.pending() || self.revert_unverified()
    }
}

/// Write gpu-trial.json through a temp file and a rename, so the tab never reads half a file.
pub fn save_trial(t: &Trial) {
    let path = common::data_dir().join(TRIAL_FILE);
    let tmp = path.with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(t) {
        if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// "Keep" was clicked on `shown` (what the banner showed); `disk` is gpu-trial.json read just now.
/// Ok = the trial to confirm. Never confirms a trial the sign-in check has reverted in the meantime.
pub fn keep_target(shown: Option<&Trial>, disk: Option<&Trial>) -> Result<Trial, String> {
    let Some(t) = disk.filter(|t| t.pending()) else {
        return Err("The trial is no longer pending - see the notice above.".into());
    };
    if shown.map(|s| (&s.preset, s.applied_at)) != Some((&t.preset, t.applied_at)) {
        return Err("The trial changed - check the notice and try again.".into());
    }
    Ok(t.clone())
}

#[derive(Serialize, Deserialize, Default)]
pub struct Confirmed {
    #[serde(default)]
    pub preset: String,
}

pub fn load_trial() -> Option<Trial> {
    common::load_json::<Option<Trial>>(TRIAL_FILE)
}

pub fn delete_trial() {
    let _ = std::fs::remove_file(common::data_dir().join(TRIAL_FILE));
}

pub fn load_confirmed() -> Option<String> {
    Some(common::load_json::<Confirmed>(CONFIRMED_FILE).preset).filter(|p| !p.is_empty())
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Decision {
    /// no trial file, or the trial is not pending
    NoTrial,
    /// the PC has not restarted since the tune was applied
    StillSameBoot,
    /// restarted without a crash: the trial stays pending
    CleanReboot,
    /// a crash was logged after the tune was applied: go back to Stock
    RevertAfterCrash,
    /// an earlier revert sent Stock but could not confirm it: send it again
    RetryRevert,
}

/// What the sign-in check does about a trial. Pure: all inputs are passed in. Boot times are unix seconds, 0 = unknown.
pub fn guard_decision(trial: Option<&Trial>, current_boot_time: i64, crash_events_after_apply: bool) -> Decision {
    if trial.map(|t| t.revert_unverified()).unwrap_or(false) {
        return Decision::RetryRevert;
    }
    let Some(t) = trial.filter(|t| t.pending()) else { return Decision::NoTrial };
    let boot_known = t.boot_time > 0 && current_boot_time > 0;
    if boot_known && (current_boot_time - t.boot_time).abs() <= BOOT_TOLERANCE {
        return Decision::StillSameBoot;
    }
    if crash_events_after_apply {
        Decision::RevertAfterCrash
    } else {
        Decision::CleanReboot
    }
}

/// Unix seconds of the last boot (Win32_OperatingSystem.LastBootUpTime); 0 when it cannot be read. Read-only.
pub fn boot_time() -> i64 {
    common::ps("([DateTimeOffset](Get-CimInstance Win32_OperatingSystem).LastBootUpTime).ToUnixTimeSeconds()")
        .ok()
        .and_then(|o| o.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// First crash event in the System log after `since` (unix seconds): Kernel-Power 41, EventLog 6008 or a
/// BugCheck / WER-SystemErrorReporting 1001. Ok(None) = no crash. Read-only.
pub fn crash_event_after(since: i64) -> Result<Option<String>, String> {
    let script = format!(
        r#"$t = [DateTimeOffset]::FromUnixTimeSeconds({since}).LocalDateTime
$e = Get-WinEvent -FilterHashtable @{{ LogName = 'System'; Id = 41, 6008, 1001; StartTime = $t }} -ErrorAction SilentlyContinue |
    Where-Object {{
        ($_.Id -eq 41 -and $_.ProviderName -like '*Kernel-Power*') -or
        ($_.Id -eq 6008 -and $_.ProviderName -eq 'EventLog') -or
        ($_.Id -eq 1001 -and ($_.ProviderName -like '*BugCheck*' -or $_.ProviderName -like '*WER-SystemErrorReporting*'))
    }} | Select-Object -First 1
if ($e) {{ "CRASH event $($e.Id) ($($e.ProviderName)) at $($e.TimeCreated.ToString('yyyy-MM-dd HH:mm:ss'))" }} else {{ 'NONE' }}"#
    );
    let out = common::ps(&script)?;
    let line = out.lines().map(str::trim).rev().find(|l| !l.is_empty()).unwrap_or("");
    match line.strip_prefix("CRASH ") {
        Some(what) => Ok(Some(what.to_string())),
        None if line == "NONE" => Ok(None),
        None => Err(format!("unexpected event-log answer: {line}")),
    }
}

/// PowerShell that registers the sign-in task: this exe with --gpu-guard, 30 s after the current user
/// signs in, elevated. The task of older versions is removed.
pub fn register_task_script(exe: &str) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
Unregister-ScheduledTask -TaskPath {path} -TaskName {old} -Confirm:$false -ErrorAction SilentlyContinue
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute {exe} -Argument '--gpu-guard'
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$trigger.Delay = 'PT30S'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskPath {path} -TaskName {name} -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null"#,
        exe = common::psq(exe),
        path = common::psq(TASK_PATH),
        name = common::psq(TASK_NAME),
        old = common::psq(OLD_TASK_NAME),
    )
}

pub fn register_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program's path: {e}"))?;
    common::ps(&register_task_script(&exe.display().to_string())).map(|_| ())
}

pub fn unregister_task_script() -> String {
    format!(
        "Unregister-ScheduledTask -TaskPath {p} -TaskName {n} -Confirm:$false -ErrorAction SilentlyContinue\nUnregister-ScheduledTask -TaskPath {p} -TaskName {o} -Confirm:$false -ErrorAction SilentlyContinue\nif (Get-ScheduledTask -TaskPath {p} -TaskName {n} -ErrorAction SilentlyContinue) {{ throw 'the sign-in task is still registered (run as administrator)' }}",
        p = common::psq(TASK_PATH),
        n = common::psq(TASK_NAME),
        o = common::psq(OLD_TASK_NAME),
    )
}

pub fn unregister_task() -> Result<(), String> {
    common::ps(&unregister_task_script()).map(|_| ())
}

/// Does the sign-in task have work: a saved tune other than Stock to set again, or an armed trial?
pub fn task_needed(saved: &Saved, trial: Option<&Trial>) -> bool {
    trial.map(|t| t.armed()).unwrap_or(false)
        || tune_index(&saved.applied).is_some_and(|i| i != 0)
        || saved.tuning.is_some_and(|t| t.power != 100)
}

/// Register or remove the sign-in task to match what is saved now.
pub fn sync_task() -> StepResult {
    if task_needed(&load_saved(), load_trial().as_ref()) {
        register_task().map(|_| "PC Tuner sets it again 30 seconds after each sign-in (the driver forgets it at every restart).".into())
    } else {
        unregister_task().map(|_| String::new())
    }
}

/// Start a trial and write gpu-trial.json (pending) with the sign-in task. The file records whether the
/// task is really there, so the tab never promises a revert that cannot happen.
pub fn start_trial(preset: &str, previous: &str, previous_tuning: Option<Tuning>) -> StepResult {
    let registered = register_task();
    let trial = Trial {
        preset: preset.to_string(),
        previous: previous.to_string(),
        previous_tuning,
        applied_at: chrono::Utc::now().timestamp(),
        boot_time: boot_time(),
        status: "pending".into(),
        reason: String::new(),
        unguarded: registered.is_err(),
        ..Default::default()
    };
    common::save_json(TRIAL_FILE, &trial);
    if load_trial().as_ref() != Some(&trial) {
        return Err(format!("The trial could not be saved to {} - no automatic revert.", common::data_dir().join(TRIAL_FILE).display()));
    }
    registered.map_err(|e| format!("Trial noted, but the sign-in check could not be registered - no automatic revert: {e}"))?;
    Ok("If the PC crashes, the GPU goes back to Stock at your next sign-in.".into())
}

/// End a trial the sign-in check still acts on (pending, or a revert not confirmed yet): delete its file
/// and update the sign-in task. A settled "reverted" notice is left for the user to read.
pub fn end_trial() -> StepResult {
    if load_trial().map(|t| t.armed()).unwrap_or(false) {
        delete_trial();
    }
    sync_task().map(|_| String::new())
}

pub fn guard_log(line: &str) {
    use std::io::Write;
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(common::data_dir().join(GUARD_LOG)).and_then(|mut f| writeln!(f, "{}  {line}", common::now()));
}

/// The trial file after the guard sent Stock following a crash (or a retry). Whatever happened, the trial is
/// "reverted" from now on, so the tab never offers Keep on a tune that crashed the PC. `fallback_ok`: Stock
/// was confirmed another way (only asked for when the read-back failed).
/// Not confirmed: the sign-in check stays to send Stock again, until MAX_REVERT_ATTEMPTS sends, then it gives up.
pub fn revert_outcome(t: &Trial, crash_text: String, applied: Applied, fallback_ok: bool) -> Trial {
    let verified = match applied {
        Applied::Confirmed => true,
        Applied::Unverified => fallback_ok,
        Applied::Failed | Applied::Mismatch => false,
    };
    let attempts = t.attempts.saturating_add(1);
    let sent_now = applied != Applied::Failed;
    let last_error = match applied {
        _ if verified => String::new(),
        Applied::Failed => "Stock could not be set on the GPU".into(),
        Applied::Mismatch => "the GPU reported other settings than Stock".into(),
        _ => "Stock was sent but could not be read back from the GPU".into(),
    };
    Trial {
        status: "reverted".into(),
        reason: crash_text,
        verified,
        attempts,
        sent: t.sent || sent_now,
        sends: t.sends.saturating_add(sent_now as u32),
        gave_up: !verified && attempts >= MAX_REVERT_ATTEMPTS,
        last_error,
        ..t.clone()
    }
}

/// Set only the power limit (a percentage of the card's default), keeping the tune that is on. It is saved
/// with the tune, so the sign-in task sets it again after a restart. Runs in a Job; administrator.
pub fn set_power_only(pct: u32) -> Result<(), String> {
    let g = nvml::Gpu::open()?;
    let (lo, hi, def, _) = g.power_limits()?;
    let watts = (def * pct / 100).clamp(lo, hi);
    g.set_power_limit(watts)?;
    let mut s = load_saved();
    let mut t = s.tuning.unwrap_or(STOCK);
    t.power = pct;
    s.tuning = Some(t);
    if s.applied.is_empty() {
        s.applied = PRESETS[0].name.to_string();
    }
    if s.applied == CUSTOM_NAME {
        s.custom = Some(t);
    }
    common::save_json(SAVED_FILE, &s);
    sync_task().map(|_| ())
}

/// The power limit the card has right now, as a percentage of its default.
pub fn power_now_pct() -> Option<u32> {
    let g = nvml::Gpu::open().ok()?;
    let (_, _, def, cur) = g.power_limits().ok()?;
    (def > 0).then(|| ((cur as f64 / def as f64) * 100.0).round() as u32)
}

fn save_applied(name: &str, t: Tuning, custom: bool) {
    let mut s = load_saved();
    s.applied = name.to_string();
    s.tuning = Some(t);
    if custom {
        s.custom = Some(t);
    }
    common::save_json(SAVED_FILE, &s);
}

/// Sign-in task (`PCTuner.exe --gpu-guard [--dry-run]`). Returns the process exit code. After a crash
/// during a trial it sets Stock; otherwise it sets the saved tune again. `dry_run` only logs.
pub fn guard_main(dry_run: bool) -> i32 {
    let mode = if dry_run { "dry-run" } else { "guard" };
    let report = |id: &str, r: StepResult| guard_log(&format!("[{mode}]   step {id}: {r:?}"));
    let trial = load_trial();
    if let Some(t) = trial.as_ref().filter(|t| t.armed()) {
        let (decision, crash_text) = if t.revert_unverified() {
            guard_log(&format!("[{mode}] decision=RetryRevert preset={} (Stock was sent earlier but not confirmed)", t.preset));
            (Decision::RetryRevert, t.reason.clone())
        } else {
            let boot = boot_time();
            let (crash, crash_text) = match crash_event_after(t.applied_at) {
                Ok(Some(what)) => (true, what),
                Ok(None) => (false, "no crash events".to_string()),
                Err(e) => (false, format!("event log could not be read: {e}")),
            };
            let decision = guard_decision(Some(t), boot, crash);
            guard_log(&format!(
                "[{mode}] decision={decision:?} preset={} applied_at={} trial_boot={} current_boot={boot} crash={crash} ({crash_text})",
                t.preset, t.applied_at, t.boot_time
            ));
            (decision, crash_text)
        };
        if matches!(decision, Decision::RevertAfterCrash | Decision::RetryRevert) {
            if dry_run {
                return 0;
            }
            // crash during the trial: Stock, and Stock is what is saved from now on
            let applied = apply_tuning(&STOCK, &report);
            if applied != Applied::Failed {
                save_applied(PRESETS[0].name, STOCK, false);
            }
            let done = revert_outcome(t, crash_text, applied, false);
            save_trial(&done);
            let code = if matches!(applied, Applied::Failed | Applied::Mismatch) { 1 } else { 0 };
            if done.armed() {
                guard_log(&format!(
                    "[{mode}] revert NOT confirmed, attempt {}/{MAX_REVERT_ATTEMPTS} ({}); sign-in task kept to try again",
                    done.attempts, done.last_error
                ));
                return code;
            }
            let what = if done.verified { "GPU set back to Stock (confirmed)".to_string() } else { format!("revert NOT confirmed after {} attempts ({}); giving up", done.attempts, done.last_error) };
            match sync_task() {
                Ok(_) => guard_log(&format!("[{mode}] {what}; sign-in task updated")),
                Err(e) => guard_log(&format!("[{mode}] {what}; sign-in task NOT updated: {e}")),
            }
            return code;
        }
    } else {
        let why = match &trial {
            Some(t) => format!("trial status is \"{}\"", t.status),
            None => "no trial file".into(),
        };
        guard_log(&format!("[{mode}] decision={:?} ({why})", guard_decision(trial.as_ref(), 0, false)));
    }
    // no crash to deal with: set the saved tune again (the driver forgets it at every restart)
    let saved = load_saved();
    match (tune_index(&saved.applied), saved.tuning) {
        (Some(i), Some(t)) if i != 0 || t.power != 100 => {
            guard_log(&format!("[{mode}] setting {} again: {}", saved.applied, t.describe()));
            if dry_run {
                return 0;
            }
            let v = apply_tuning(&t, &report);
            guard_log(&format!("[{mode}] {} -> {v:?}", saved.applied));
            if v == Applied::Failed {
                1
            } else {
                0
            }
        }
        _ => {
            guard_log(&format!("[{mode}] nothing to set again (saved: \"{}\")", saved.applied));
            0
        }
    }
}

// ------------------------------------------------------------------ tab

/// What happens to the trial once the tune is on the card.
#[derive(Clone, Debug, PartialEq)]
pub enum After {
    Nothing,
    StartTrial { previous: String, previous_tuning: Option<Tuning> },
    /// `visible`: shown as a step (a trial is pending); otherwise it ends quietly.
    EndTrial { visible: bool },
}

pub enum Action {
    Apply { idx: usize, tuning: Tuning, after: After },
}

/// Trial bookkeeping for applying tune `idx` with `tuning`, given what the user confirmed and whether a trial
/// is pending. Custom is "the confirmed one" only with the same values.
pub fn plan_after(idx: usize, confirmed: Option<&str>, trial_pending: bool) -> After {
    if idx == 0 {
        After::EndTrial { visible: trial_pending }
    } else if confirmed == Some(tune_name(idx)) && idx != CUSTOM {
        if trial_pending {
            After::EndTrial { visible: true }
        } else {
            After::Nothing
        }
    } else {
        After::StartTrial { previous: confirmed.unwrap_or(PRESETS[0].name).to_string(), previous_tuning: None }
    }
}

/// The whole Apply operation (runs in a Job).
pub fn run_apply(idx: usize, tuning: &Tuning, after: &After, report: &dyn Fn(&str, StepResult)) {
    // an earlier trial the sign-in check still acts on (pending, or a crash revert not confirmed yet)
    let prior = load_trial().filter(|t| t.armed());
    // A new trial is armed BEFORE the card gets the tune: an unstable tune usually crashes the PC the
    // moment it is applied, and the sign-in check must already exist then.
    if let After::StartTrial { previous, previous_tuning } = after {
        report(STEP_TRIAL, start_trial(tune_name(idx), previous, *previous_tuning));
    }
    if apply_tuning(tuning, report) == Applied::Failed {
        // the card never got it: no trial for it (an earlier armed trial is written back as it was)
        if let After::StartTrial { .. } = after {
            match &prior {
                Some(t) => save_trial(t),
                None => delete_trial(),
            }
            let _ = sync_task();
        }
        return;
    }
    save_applied(tune_name(idx), *tuning, idx == CUSTOM);
    if let After::EndTrial { visible } = after {
        let r = end_trial();
        if *visible {
            report(STEP_END_TRIAL, r);
        }
    }
    report(STEP_SAVE, sync_task());
}

/// The operation window for applying a tune: one item per step.
pub fn apply_window(kicker: &str, idx: usize, t: &Tuning, after: &After, ab_running: bool) -> OpWindow {
    let name = tune_name(idx);
    let mut items = vec![];
    // armed before the card gets the tune, so a crash at the moment of applying is covered
    if let After::StartTrial { .. } = after {
        items.push(OpItem::new(STEP_TRIAL, "Arm the safety trial (auto-revert after a crash)", ""));
    }
    items.push(OpItem::new(STEP_SET, "Set it on the GPU", "").detail(t.describe()));
    items.push(OpItem::new(STEP_CONFIRM, "Read it back from the GPU", ""));
    if let After::EndTrial { visible: true } = after {
        items.push(OpItem::new(STEP_END_TRIAL, "End the safety trial", ""));
    }
    let save = if idx == 0 { "Stop setting a tune at sign-in" } else { "Set it again at every sign-in" };
    items.push(OpItem::new(STEP_SAVE, save, ""));
    let mut w = OpWindow::new(kicker, "run", "done", items).noun("step", "steps");
    w.show_steps = true;
    w.fail_phrase = "did not complete".into();
    w.preview_text = match PRESETS.get(idx) {
        Some(p) => format!("{}: {}", p.name, p.text),
        None => format!("Custom: {}.", t.describe()),
    };
    w.running_text = format!("Applying {name}...");
    w.go_label = if idx == 0 { "Reset to stock".into() } else { format!("Apply {name}") };
    let mut warn: Vec<String> = vec![];
    if name == "Max" || name == "Performance" {
        warn.push("Runs warmest - test stability before long sessions.".into());
    }
    if idx == CUSTOM {
        warn.push("Custom values are not tested by PC Tuner - test stability before long sessions.".into());
    }
    if ab_running {
        warn.push("MSI Afterburner or NV-UV-Play is running and may overwrite these settings - close it first.".into());
    }
    if !warn.is_empty() {
        w.warning = Some(warn.join("\n"));
    }
    w.reassure = match after {
        After::StartTrial { previous, .. } => format!("Starts as a trial: Keep it or Revert to {previous} afterwards. After a crash the GPU goes back to Stock at your next sign-in."),
        _ if idx == 0 => "Stock is the factory behaviour: no undervolt, no memory overclock.".into(),
        _ => "This is the tune you already confirmed.".into(),
    };
    w
}

/// A running operation: step results arrive on `rx`, `job` yields when everything is over.
pub struct Work {
    pub rx: mpsc::Receiver<(String, StepResult)>,
    pub job: Job<()>,
}

pub const HISTORY: usize = 60;

/// The RTX 5080 Profiles page's state: what was read, the live samples, the trial and the running work.
pub struct Gpu {
    pub state: Option<State>,
    pub refresh: Option<Job<State>>,
    pub next_refresh: Instant,
    /// light ~1 s sampler feeding the sparklines
    pub sampler: Option<Job<Option<Live>>>,
    pub next_sample: Instant,
    pub history: VecDeque<Live>,
    /// 0..CUSTOM a preset, CUSTOM the custom values
    pub selected: Option<usize>,
    /// the custom values being edited (None until the card has been read)
    pub custom: Option<Tuning>,
    pub op: Option<OpWindow>,
    pub op_action: Option<Action>,
    pub work: Option<Work>,
    /// updating the sign-in task after "Keep", closing Afterburner
    pub chore: Option<Job<Result<(), String>>>,
    pub status: String,
    pub error: Option<String>,
    pub trial: Option<Trial>,
    pub confirmed: Option<String>,
    /// PCTUNER_DEMO sample data on screen: nothing is written or run
    pub demo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum Click {
    Reset,
    Apply,
    Refresh,
    Keep,
    Revert,
    Dismiss,
    CloseAfterburner,
}

/// A custom value the page changes.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    MaxMhz,
    Core,
    Mem,
    Power,
}

/// The "Trial reverted" notice: says Stock is set only when the sign-in check confirmed it.
pub fn reverted_text(t: &Trial) -> String {
    if t.verified {
        format!(
            "Windows logged an unexpected shutdown during the {} trial, so the GPU was set back to Stock at sign-in. If it was not a power cut, try a milder preset.",
            t.preset
        )
    } else if !t.sent {
        let next = if t.gave_up { "the sign-in check has stopped" } else { "the sign-in check tries again next time" };
        format!(
            "Windows logged an unexpected shutdown during the {} trial. Stock could NOT be loaded onto the GPU at sign-in (see below), so the GPU may still run the {} tune; use Reset to stock ({next}). If it was not a power cut, try a milder preset.",
            t.preset, t.preset
        )
    } else if t.gave_up {
        let n = t.sends.max(1);
        format!(
            "Windows logged an unexpected shutdown during the {} trial. Stock was sent to the GPU at {n} sign-in{} but never confirmed, so the sign-in check has stopped. Check the readout above, or use Reset to stock. If it was not a power cut, try a milder preset.",
            t.preset,
            if n == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Windows logged an unexpected shutdown during the {} trial. Stock was sent to the GPU at sign-in - not yet confirmed; check the readout above (the sign-in check tries again next time). If it was not a power cut, try a milder preset.",
            t.preset
        )
    }
}

/// `t` confirmed by a reading: Some(trial marked verified) when it is a revert not confirmed yet and the
/// driver reports no offsets.
pub fn settled_revert(t: &Trial, live: Option<&Live>) -> Option<Trial> {
    let stock = live.is_some_and(|l| l.core_ofs == 0 && l.mem_ofs_raw == 0);
    (t.revert_unverified() && stock).then(|| Trial { verified: true, last_error: String::new(), ..t.clone() })
}

/// Hero of "Currently applied": (name, unit, caption, colour).
pub fn hero_text(current: Option<usize>, saved: &Saved, live: Option<&Live>) -> (String, String, String, Tone) {
    match (current, live) {
        (_, None) => ("Unknown".into(), String::new(), "The GPU could not be read through the NVIDIA driver.".into(), Tone::Off),
        (Some(0), Some(_)) => ("Stock".into(), "preset".into(), "Factory settings: no offsets, no clock limit.".into(), Tone::Accent),
        (Some(i), Some(_)) => {
            let t = saved.tuning.unwrap_or(STOCK);
            (tune_name(i).into(), if i == CUSTOM { "settings".into() } else { "preset".into() }, t.describe(), Tone::Accent)
        }
        (None, Some(l)) => (
            "Other".into(),
            "settings".into(),
            format!("Set by another tool (Afterburner?): core {:+} MHz, memory value {:+}.", l.core_ofs, l.mem_ofs_raw),
            Tone::Junk,
        ),
    }
}

/// The live chip, from the one reading on screen: (text, colour).
pub fn live_chip(live: Option<&Live>, factor: f64) -> (String, Tone) {
    match live {
        Some(l) => (
            format!("Driver reports: core {:+} MHz, memory {:+} MHz", l.core_ofs, (l.mem_ofs_raw as f64 / factor).round() as i64),
            if l.core_ofs == 0 && l.mem_ofs_raw == 0 { Tone::Dim } else { Tone::Ok },
        ),
        None => ("The NVIDIA driver could not be read".into(), Tone::Off),
    }
}

pub fn hhmm(unix: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(unix, 0).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

impl Default for Gpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpu {
    pub fn new() -> Self {
        let mut tab = Gpu {
            state: None,
            refresh: None,
            next_refresh: Instant::now(),
            sampler: None,
            next_sample: Instant::now(),
            history: VecDeque::with_capacity(HISTORY),
            selected: None,
            custom: None,
            op: None,
            op_action: None,
            work: None,
            chore: None,
            status: String::new(),
            error: None,
            trial: load_trial(),
            confirmed: load_confirmed(),
            demo: false,
        };
        tab.demo_setup();
        tab
    }

    /// PCTUNER_DEMO=gpu:<preview|running|done|trial|reverted> - sample data for screenshots, nothing is changed.
    pub fn demo_setup(&mut self) {
        let v = std::env::var("PCTUNER_DEMO").unwrap_or_default().to_lowercase();
        let sample = |status: &str, reason: &str| Trial {
            preset: "Performance".into(),
            previous: "Balanced".into(),
            applied_at: chrono::Utc::now().timestamp() - 25 * 60,
            boot_time: 0,
            status: status.into(),
            reason: reason.into(),
            verified: true,
            ..Default::default()
        };
        match v.as_str() {
            "gpu:trial" => {
                self.demo = true;
                self.trial = Some(sample("pending", ""));
            }
            "gpu:unguarded" => {
                self.demo = true;
                self.trial = Some(Trial { unguarded: true, ..sample("pending", "") });
            }
            "gpu:reverted" => {
                self.demo = true;
                self.trial = Some(sample("reverted", "event 41 (Microsoft-Windows-Kernel-Power) at 2026-11-11 21:47:03"));
            }
            _ => {
                let Some(stage) = crate::opwin::demo_stage("gpu") else { return };
                self.demo = true;
                let t = Tuning { max_mhz: 2980, core: 318, mem: 0, power: 100 };
                let mut w = apply_window("APPLY PERFORMANCE", 4, &t, &After::StartTrial { previous: "Balanced".into(), previous_tuning: None }, false);
                match stage {
                    Stage::Preview => {}
                    Stage::Running => {
                        w.items[0].result = Some(Ok(String::new()));
                        w.items[1].result = Some(Ok(String::new()));
                        w.stage = Stage::Running;
                    }
                    Stage::Done => {
                        w.finish(|it| match it.id.as_str() {
                            STEP_CONFIRM => Some(Ok("The driver reports core +318 MHz, memory +0 MHz.".into())),
                            STEP_TRIAL => Some(Ok("If the PC crashes, the GPU goes back to Stock at your next sign-in.".into())),
                            _ => None,
                        });
                        w.notes.push("Trial running - use Keep or Revert at the top of this tab once you have tested it.".into());
                    }
                }
                self.op = Some(w);
            }
        }
    }

    /// The reading on screen: the newest 1 s sample, else the last state refresh.
    pub fn live_shown(&self) -> Option<&Live> {
        self.history.back().or(self.state.as_ref().and_then(|s| s.live.as_ref()))
    }

    /// What the card runs, from the reading on screen.
    pub fn current(&self) -> Option<usize> {
        let s = self.state.as_ref()?;
        current_tune(&s.saved, self.live_shown())
    }

    pub fn reload_trial(&mut self) {
        if !self.demo {
            self.trial = load_trial();
            self.confirmed = load_confirmed();
        }
    }

    /// A crash revert the sign-in check could not confirm is settled once the tab sees no offsets:
    /// the notice then reads as confirmed and the sign-in task is updated.
    pub fn settle_revert(&mut self) {
        if self.demo || self.work.is_some() || !self.trial.as_ref().map(|t| t.revert_unverified()).unwrap_or(false) {
            return;
        }
        // decide from the file on disk, which the sign-in check may have changed
        let Some(done) = load_trial().and_then(|t| settled_revert(&t, self.history.back())) else { return };
        save_trial(&done);
        self.trial = Some(done);
        if self.chore.is_none() {
            self.chore = Some(Job::spawn(|| sync_task().map(|_| ())));
        }
    }

    pub fn pump(&mut self) {
        if let Some(s) = self.refresh.as_ref().and_then(|j| j.poll()) {
            if self.selected.is_none() {
                self.selected = s.current.or(tune_index(&s.saved.applied));
            }
            if self.custom.is_none() {
                if let Some(l) = &s.limits {
                    self.custom = Some(tuning_of(CUSTOM, &s.saved.lifts, s.saved.custom).clamp(l));
                }
            }
            self.state = Some(s);
            self.refresh = None;
            self.next_refresh = Instant::now() + Duration::from_secs(3);
            // the sign-in check may have reverted the trial while the tab is open
            self.reload_trial();
        }
        if self.refresh.is_none() && Instant::now() >= self.next_refresh {
            self.refresh = Some(Job::spawn(current_state));
        }

        // light sampler: one reading about once per second
        if let Some(j) = &self.sampler {
            if let Some(l) = j.poll() {
                if let Some(l) = l {
                    if self.history.len() >= HISTORY {
                        self.history.pop_front();
                    }
                    self.history.push_back(l);
                }
                self.sampler = None;
                self.settle_revert();
            }
        }
        if self.sampler.is_none() && Instant::now() >= self.next_sample {
            self.next_sample = Instant::now() + Duration::from_secs(1);
            self.sampler = Some(Job::spawn(gpu_live));
        }

        if let Some(w) = &self.work {
            // The step sender lives inside the job's closure, so a disconnected channel means the job is over -
            // also when the worker died without a result, which Job::poll() alone would never report.
            let _ = w.job.poll();
            let mut finished = false;
            loop {
                match w.rx.try_recv() {
                    Ok((id, r)) => {
                        if let Some(it) = self.op.as_mut().and_then(|op| op.items.iter_mut().find(|i| i.id == id)) {
                            it.result = Some(r);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if finished {
                self.work = None;
                if let Some(op) = &mut self.op {
                    op.finish(|it| Some(it.result.clone().unwrap_or_else(|| Err("Not run - an earlier step failed.".into()))));
                    let ok = |id: &str| op.items.iter().any(|i| i.id == id && matches!(i.result, Some(Ok(_))));
                    if ok(STEP_TRIAL) && ok(STEP_SET) {
                        op.notes.push("Trial running - use Keep or Revert at the top of this tab once you have tested it.".into());
                    }
                }
                self.reload_trial();
                self.next_refresh = Instant::now();
            }
        }

        if let Some(j) = &self.chore {
            if let Some(r) = j.poll() {
                if let Err(e) = r {
                    self.error = Some(e);
                }
                self.chore = None;
                self.next_refresh = Instant::now();
            }
        }
    }

    /// The power slider: set only the power limit (percent of the default), in the background.
    pub fn set_power(&mut self, pct: u32) {
        let Some(l) = self.state.as_ref().and_then(|s| s.limits) else { return };
        if !self.can_act() || self.chore.is_some() {
            return;
        }
        let pct = pct.clamp(l.power.0, l.power.1);
        self.status = format!("Power limit set to {pct} % ({} W).", l.power_default_w * pct / 100);
        self.chore = Some(Job::spawn(move || set_power_only(pct).map_err(|e| format!("The power limit could not be set:\n{e}"))));
    }

    /// Change one custom value (kept inside the card's limits).
    pub fn set_custom(&mut self, f: Field, v: i64) {
        let (Some(c), Some(l)) = (self.custom.as_mut(), self.state.as_ref().and_then(|s| s.limits)) else { return };
        match f {
            Field::MaxMhz => c.max_mhz = v.max(0) as u32,
            Field::Core => c.core = v as i32,
            Field::Mem => c.mem = v as i32,
            Field::Power => c.power = v.max(0) as u32,
        }
        *c = c.clamp(&l);
        self.selected = Some(CUSTOM);
    }

    /// Open the operation window (preview) for applying tune `idx` (`with`: these values instead, for a
    /// revert to an earlier custom tune).
    pub fn open_apply(&mut self, idx: usize, kicker: String, with: Option<Tuning>) {
        // plan from the file on disk: the sign-in check may have changed it
        self.reload_trial();
        let mut saved = load_saved();
        let tuning = if let Some(t) = with {
            t
        } else if idx == 0 {
            STOCK
        } else if idx == CUSTOM {
            let Some(t) = self.custom else {
                self.error = Some("The GPU has not been read yet - wait a moment and try again.".into());
                return;
            };
            t
        } else {
            match lifts(&mut saved) {
                Ok(l) => tuning_of(idx, &l, None),
                Err(e) => {
                    self.error = Some(format!("Could not work out {} for this card:\n{e}", tune_name(idx)));
                    return;
                }
            }
        };
        // a revert that is not confirmed yet also keeps the sign-in check: applying a tune settles it
        let pending = self.trial.as_ref().map(|t| t.armed()).unwrap_or(false);
        let mut after = plan_after(idx, self.confirmed.as_deref(), pending);
        if let After::StartTrial { previous, previous_tuning } = &mut after {
            if previous == CUSTOM_NAME {
                *previous_tuning = saved.custom;
            }
        }
        let ab = self.state.as_ref().map(|s| s.ab_running).unwrap_or(false);
        self.op = Some(apply_window(&kicker, idx, &tuning, &after, ab));
        self.op_action = Some(Action::Apply { idx, tuning, after });
    }

    /// "Go" was clicked in the operation window: start the background work.
    pub fn go(&mut self) {
        let Some(action) = self.op_action.take() else {
            // demo window: nothing to run
            if let Some(op) = &mut self.op {
                op.stage = Stage::Preview;
            }
            return;
        };
        let (tx, rx) = mpsc::channel();
        let job = Job::spawn(move || {
            let report = |id: &str, r: StepResult| {
                let _ = tx.send((id.to_string(), r));
            };
            match action {
                Action::Apply { idx, tuning, after } => run_apply(idx, &tuning, &after, &report),
            }
        });
        self.work = Some(Work { rx, job });
    }

    pub fn click(&mut self, c: Click) {
        match c {
            Click::Reset => self.open_apply(0, "RESET GPU TO STOCK".into(), None),
            Click::Apply => match self.selected {
                Some(idx) => self.open_apply(idx, format!("APPLY {}", tune_name(idx).to_uppercase()), None),
                None => self.status = "Pick a preset first.".into(),
            },
            Click::Refresh => self.next_refresh = Instant::now(),
            Click::CloseAfterburner => {
                if self.chore.is_none() {
                    self.chore = Some(Job::spawn(close_ab));
                }
            }
            Click::Keep => {
                if self.demo {
                    if let Some(t) = self.trial.take() {
                        self.status = format!("{} kept as your confirmed tune.", t.preset);
                    }
                    return;
                }
                // decide from the file on disk, never from the copy on screen: the sign-in check may have reverted it
                let shown = self.trial.clone();
                self.reload_trial();
                match keep_target(shown.as_ref(), self.trial.as_ref()) {
                    Err(e) => self.status = e,
                    Ok(t) => {
                        self.status = format!("{} kept as your confirmed tune.", t.preset);
                        common::save_json(CONFIRMED_FILE, &Confirmed { preset: t.preset.clone() });
                        self.confirmed = Some(t.preset);
                        self.trial = None;
                        delete_trial();
                        self.chore = Some(Job::spawn(|| sync_task().map(|_| ())));
                    }
                }
            }
            Click::Revert => {
                self.reload_trial();
                let (prev, with) = self.trial.as_ref().map(|t| (t.previous.clone(), t.previous_tuning)).unwrap_or_default();
                let idx = tune_index(&prev).unwrap_or(0);
                // a custom tune comes back with the values it had then
                let with = if idx == CUSTOM { with.or(load_saved().custom) } else { None };
                if idx == CUSTOM && with.is_none() {
                    self.open_apply(0, "REVERT TO STOCK".into(), None);
                } else {
                    self.open_apply(idx, format!("REVERT TO {}", tune_name(idx).to_uppercase()), with);
                }
            }
            Click::Dismiss => {
                if self.demo {
                    self.trial = None;
                    return;
                }
                // only a notice is dismissed: a trial that became pending again stays
                self.reload_trial();
                if self.trial.as_ref().map(|t| t.pending()).unwrap_or(false) {
                    return;
                }
                let unverified = self.trial.as_ref().map(|t| t.revert_unverified()).unwrap_or(false);
                self.trial = None;
                delete_trial();
                if unverified && self.chore.is_none() {
                    // the sign-in check was kept for this revert; without the file it has nothing to do for it
                    self.chore = Some(Job::spawn(|| sync_task().map(|_| ())));
                }
            }
        }
    }

    /// An apply is running.
    pub fn busy(&self) -> bool {
        self.work.is_some()
    }

    /// May changes be made now (administrator, nothing running, not a demo)?
    pub fn can_act(&self) -> bool {
        common::is_admin() && !self.busy() && !self.demo
    }

    /// Call often: polls background work and handles the sidebar's "Reset GPU to stock" request.
    pub fn tick(&mut self) {
        self.pump();
        if common::GPU_RESET_REQUEST.swap(false, std::sync::atomic::Ordering::Relaxed) && !self.busy() {
            self.click(Click::Reset);
        }
    }

    /// React to a button of the operation window.
    pub fn on_event(&mut self, ev: OpEvent) {
        match ev {
            OpEvent::Go => self.go(),
            OpEvent::Close => {
                self.op = None;
                self.op_action = None;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvapi::VfPoint;

    /// This card's curve around the presets (read from the driver on 2026-11-16; 975 mV approximate).
    fn curve() -> Vec<VfPoint> {
        [(450.0, 300.0), (845.0, 2265.0), (850.0, 2355.0), (900.0, 2565.0), (925.0, 2662.0), (950.0, 2752.0), (975.0, 2830.0), (1240.0, 3345.0)]
            .iter()
            .map(|&(mv, mhz)| VfPoint { mv, mhz })
            .collect()
    }

    fn live(core_ofs: i32, mem_ofs_raw: i32) -> Live {
        Live { core: 2805, mem: 15001, mem_stock: 15001, pstate: "P0".into(), full_clocks: true, core_ofs, mem_ofs_raw, ..Default::default() }
    }

    #[test]
    fn gpu_preset_lifts_from_the_curve() {
        // NV-UV-Play's tiers: Eco, MFG, Balanced, Performance, Max
        assert_eq!(preset_lifts(&curve()).unwrap(), vec![0, 295, 295, 185, 318, 320]);
        let lifts = vec![0, 295, 295, 185, 318, 320];
        assert_eq!(tuning_of(0, &lifts, None), STOCK);
        assert_eq!(tuning_of(1, &lifts, None), Tuning { max_mhz: 2650, core: 295, mem: 0, power: 88 });
        assert_eq!(tuning_of(2, &lifts, None), Tuning { max_mhz: 2650, core: 295, mem: 0, power: 100 });
        assert_eq!(tuning_of(3, &lifts, None), Tuning { max_mhz: 2750, core: 185, mem: 0, power: 90 });
        assert_eq!(tuning_of(5, &lifts, None), Tuning { max_mhz: 3150, core: 320, mem: 0, power: 100 });
        // custom: the saved values, else Balanced to start from
        let mine = Tuning { max_mhz: 2750, core: 100, mem: 500, power: 90 };
        assert_eq!(tuning_of(CUSTOM, &lifts, Some(mine)), mine);
        assert_eq!(tuning_of(CUSTOM, &lifts, None), tuning_of(3, &lifts, None));
        // a slow chip: more than +450 is refused
        let slow: Vec<VfPoint> = curve().iter().map(|p| VfPoint { mhz: p.mhz - 200.0, ..*p }).collect();
        assert!(preset_lifts(&slow).unwrap_err().contains("too aggressive"));
        // no point at the preset's voltage
        assert!(preset_lifts(&curve()[..2]).unwrap_err().contains("No curve point at or above 850 mV"));
        // a curve another program flattened (NV-UV-Play's Eco: 2650 MHz from 850 mV up) is refused
        let mut flat: Vec<VfPoint> = (0..20).map(|k| VfPoint { mv: 850.0 + 10.0 * k as f32, mhz: 2650.0 }).collect();
        flat.insert(0, VfPoint { mv: 450.0, mhz: 300.0 });
        assert_eq!(preset_lifts(&flat).unwrap_err(), CURVE_CHANGED);
        assert!(!curve_is_flattened(&curve()));
        assert_eq!((tune_name(CUSTOM), tune_index("Custom"), tune_index("Balanced"), tune_index("x")), ("Custom", Some(CUSTOM), Some(3), None));
    }

    #[test]
    fn gpu_custom_limits() {
        // this RTX 5080: offsets -1000..+1000 / -2000..+6000, P0 300..3210 MHz, power 250..450 W (360 default)
        let core = nvml::Offset { mhz: 0, min: -1000, max: 1000 };
        let mem = nvml::Offset { mhz: 0, min: -2000, max: 6000 };
        let l = limits_from(core, mem, 1.0, (300, 3210), (250, 450, 360));
        assert_eq!(l, Limits { clock: (1500, 3210), core: (-500, 450), mem: (0, 3000), power: (70, 125), power_default_w: 360 });
        // a memory scale of 2 halves what the card allows in nvidia-smi MHz
        assert_eq!(limits_from(core, mem, 2.0, (300, 3210), (250, 450, 360)).mem, (0, 3000));
        assert_eq!(limits_from(core, nvml::Offset { mhz: 0, min: 0, max: 2000 }, 2.0, (300, 3210), (250, 450, 360)).mem, (0, 1000));
        let wild = Tuning { max_mhz: 4000, core: 900, mem: -50, power: 10 };
        assert_eq!(wild.clamp(&l), Tuning { max_mhz: 3210, core: 450, mem: 0, power: 70 });
        assert_eq!(STOCK.describe(), "no offsets, no clock limit, power 100%");
        assert_eq!(Tuning { max_mhz: 2800, core: 138, mem: 1500, power: 100 }.describe(), "max 2800 MHz, core +138 MHz, memory +1500 MHz, power 100%");
        assert_eq!((mem_raw(1500, 1.0), mem_raw(1500, 2.0), mem_raw(1500, 0.5)), (1500, 3000, 750));
        assert_eq!((snap_factor(0.98), snap_factor(2.1), snap_factor(0.55)), (1.0, 2.0, 0.5));
    }

    #[test]
    fn gpu_current_tune() {
        let bal = Tuning { max_mhz: 2800, core: 138, mem: 1500, power: 100 };
        let saved = Saved { applied: "Balanced".into(), tuning: Some(bal), mem_factor: 2.0, ..Default::default() };
        // the driver reports the saved tune's offsets (memory in NVML's scale)
        assert_eq!(current_tune(&saved, Some(&live(138, 3000))), Some(3));
        // restarted and not set again yet: the card is on Stock
        assert_eq!(current_tune(&saved, Some(&live(0, 0))), Some(0));
        // something else set offsets
        assert_eq!(current_tune(&saved, Some(&live(200, 1000))), None);
        // no reading: the last applied tune
        assert_eq!(current_tune(&saved, None), Some(3));
        assert_eq!(current_tune(&Saved::default(), None), None);
        let (name, unit, cap, tone) = hero_text(Some(3), &saved, Some(&live(138, 3000)));
        assert_eq!((name.as_str(), unit.as_str(), tone), ("Balanced", "preset", Tone::Accent));
        assert!(cap.contains("max 2800 MHz"));
        assert_eq!(hero_text(None, &saved, Some(&live(200, 0))).0, "Other");
        assert_eq!(hero_text(Some(3), &saved, None).0, "Unknown");
        assert_eq!(live_chip(Some(&live(138, 3000)), 2.0).0, "Driver reports: core +138 MHz, memory +1500 MHz");
        // the sign-in task only has work for a tune other than Stock, or an armed trial
        assert!(task_needed(&saved, None));
        assert!(!task_needed(&Saved { applied: "Stock".into(), ..Default::default() }, None));
        assert!(!task_needed(&Saved::default(), None));
        assert!(task_needed(&Saved::default(), Some(&trial("pending", 1))));
        // gpu-tuning.json round trip; older / partial files read with defaults
        let s = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&s).unwrap(), saved);
        assert_eq!(serde_json::from_str::<Saved>("{}").unwrap().factor(), 1.0);
    }

    fn trial(status: &str, boot: i64) -> Trial {
        Trial { preset: "Performance".into(), previous: "Balanced".into(), applied_at: 1_800_000_000, boot_time: boot, status: status.into(), ..Default::default() }
    }

    #[test]
    fn gpu_guard_decision() {
        let boot = 1_799_990_000;
        // no file, or a trial that is not pending
        assert_eq!(guard_decision(None, boot, true), Decision::NoTrial);
        let confirmed_revert = Trial { verified: true, ..trial("reverted", boot) };
        assert_eq!(guard_decision(Some(&confirmed_revert), boot + 5000, true), Decision::NoTrial);
        // Stock was sent but not confirmed: the check sends it again, whatever the boot or event log says
        assert_eq!(guard_decision(Some(&trial("reverted", boot)), boot + 5000, false), Decision::RetryRevert);
        assert_eq!(guard_decision(Some(&trial("reverted", boot)), boot, true), Decision::RetryRevert);
        // same boot (small clock drift allowed): nothing to do, whatever the event log says
        assert_eq!(guard_decision(Some(&trial("pending", boot)), boot, false), Decision::StillSameBoot);
        assert_eq!(guard_decision(Some(&trial("pending", boot)), boot + 20, true), Decision::StillSameBoot);
        // restarted without a crash: the trial stays pending
        assert_eq!(guard_decision(Some(&trial("pending", boot)), boot + 20_000, false), Decision::CleanReboot);
        // restarted and a crash was logged after the apply
        assert_eq!(guard_decision(Some(&trial("pending", boot)), boot + 20_000, true), Decision::RevertAfterCrash);
        // unknown boot time on either side: the crash events decide
        assert_eq!(guard_decision(Some(&trial("pending", 0)), boot, true), Decision::RevertAfterCrash);
        assert_eq!(guard_decision(Some(&trial("pending", boot)), 0, false), Decision::CleanReboot);
    }

    #[test]
    fn gpu_trial_json_round_trip() {
        let t = Trial {
            reason: "event 41 (Microsoft-Windows-Kernel-Power) at 2026-11-11 21:47:03".into(),
            previous_tuning: Some(Tuning { max_mhz: 2750, core: 100, mem: 500, power: 90 }),
            ..trial("reverted", 1_799_990_000)
        };
        let s = serde_json::to_string_pretty(&t).unwrap();
        assert!(s.contains("\"status\": \"reverted\"") && s.contains("\"applied_at\": 1800000000"));
        assert_eq!(serde_json::from_str::<Trial>(&s).unwrap(), t);
        // the file is read as Option<Trial>: missing fields default, "null" is no trial
        let partial: Option<Trial> = serde_json::from_str(r#"{"preset":"Balanced","status":"pending"}"#).unwrap();
        assert!(partial.as_ref().unwrap().pending() && partial.unwrap().boot_time == 0);
        assert_eq!(serde_json::from_str::<Option<Trial>>("null").unwrap(), None);
        let c: Confirmed = serde_json::from_str(&serde_json::to_string(&Confirmed { preset: "Balanced".into() }).unwrap()).unwrap();
        assert_eq!(c.preset, "Balanced");
    }

    #[test]
    fn gpu_trial_plan_and_window() {
        // Stock always ends a trial; the confirmed preset ends a pending one; anything else starts a trial
        assert_eq!(plan_after(0, Some("Balanced"), true), After::EndTrial { visible: true });
        assert_eq!(plan_after(0, None, false), After::EndTrial { visible: false });
        assert_eq!(plan_after(3, Some("Balanced"), false), After::Nothing);
        assert_eq!(plan_after(3, Some("Balanced"), true), After::EndTrial { visible: true });
        assert_eq!(plan_after(4, Some("Balanced"), false), After::StartTrial { previous: "Balanced".into(), previous_tuning: None });
        assert_eq!(plan_after(1, None, false), After::StartTrial { previous: "Stock".into(), previous_tuning: None });
        // custom values always start a trial: they may differ from the ones kept before
        assert!(matches!(plan_after(CUSTOM, Some("Custom"), false), After::StartTrial { .. }));

        let t = Tuning { max_mhz: 2900, core: 148, mem: 2000, power: 100 };
        let w = apply_window("APPLY PERFORMANCE", 4, &t, &After::StartTrial { previous: "Balanced".into(), previous_tuning: None }, true);
        assert_eq!(w.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![STEP_TRIAL, STEP_SET, STEP_CONFIRM, STEP_SAVE]);
        assert!(w.items.iter().all(|i| i.group.is_empty()));
        assert!(w.show_steps && w.stage == Stage::Preview);
        let warn = w.warning.unwrap();
        assert!(warn.contains("Runs warmest") && warn.contains("Afterburner or NV-UV-Play is running"));
        let w = apply_window("RESET GPU TO STOCK", 0, &STOCK, &After::EndTrial { visible: false }, false);
        assert_eq!(w.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![STEP_SET, STEP_CONFIRM, STEP_SAVE]);
        assert!(w.warning.is_none());
        assert!(apply_window("APPLY CUSTOM", CUSTOM, &t, &After::Nothing, false).warning.unwrap().contains("Custom values"));

        // the task scripts (never run by the tests)
        let s = register_task_script(r"C:\Apps\It's\PCTuner.exe");
        assert!(s.contains(r"-Execute 'C:\Apps\It''s\PCTuner.exe' -Argument '--gpu-guard'"));
        assert!(s.contains("-AtLogOn") && s.contains("PT30S") && s.contains("-RunLevel Highest") && s.contains("-Force"));
        assert!(s.contains(r"-TaskPath '\PCTuner\' -TaskName 'GpuApply'"));
        assert!(s.contains(r"Unregister-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'GpuTrialGuard'"), "the old task goes");
        let u = unregister_task_script();
        assert!(u.starts_with(r"Unregister-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'GpuApply'") && u.contains("'GpuTrialGuard'"));
    }

    /// Read-only: boot time and the crash-event query against this PC's System log.
    #[test]
    fn gpu_guard_queries() {
        let boot = boot_time();
        println!("boot time: {boot} ({})", hhmm(boot));
        assert!(boot > 1_600_000_000);
        // nothing can have crashed in the future
        assert_eq!(crash_event_after(chrono::Utc::now().timestamp() + 3600), Ok(None));
        println!("crash events in the last 30 days: {:?}", crash_event_after(chrono::Utc::now().timestamp() - 30 * 86400));
    }

    /// Live, read-only: the card's state as the tab sees it (`cargo test gpu_state_live -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn gpu_state_live() {
        let s = current_state();
        println!("live {:?}\nlimits {:?}\nerror {:?}\ncurrent {:?}\nAfterburner running {} autostart {}", s.live, s.limits, s.error, s.current, s.ab_running, s.ab_autostart);
        println!("preset lifts from the curve: {:?}", crate::nvapi::read_curve().and_then(|c| preset_lifts(&c)));
    }

    #[test]
    fn gpu_keep_checks_the_file() {
        let shown = trial("pending", 1);
        // the sign-in check reverted the trial while the tab showed it as pending: Keep must not confirm it
        let reverted = Trial { reason: "event 41".into(), ..trial("reverted", 1) };
        assert!(keep_target(Some(&shown), Some(&reverted)).unwrap_err().contains("no longer pending"));
        assert!(keep_target(Some(&shown), None).is_err(), "file gone");
        // a different trial was started in the meantime
        let newer = Trial { applied_at: shown.applied_at + 60, ..trial("pending", 1) };
        assert!(keep_target(Some(&shown), Some(&newer)).unwrap_err().contains("changed"));
        let other = Trial { preset: "Balanced".into(), ..trial("pending", 1) };
        assert!(keep_target(Some(&shown), Some(&other)).is_err());
        assert!(keep_target(None, Some(&shown)).is_err());
        assert_eq!(keep_target(Some(&shown), Some(&shown)), Ok(shown.clone()));
        // what still needs the sign-in check
        assert!(trial("pending", 1).armed() && trial("reverted", 1).armed());
        assert!(!Trial { verified: true, ..trial("reverted", 1) }.armed());
    }

    #[test]
    fn gpu_revert_verification() {
        // only a confirmed Stock (read back from the driver) settles the revert
        let crashed = trial("pending", 1);
        let out = |t: &Trial, a: Applied, fallback: bool| revert_outcome(t, "event 41".into(), a, fallback);
        let ok = out(&crashed, Applied::Confirmed, false);
        assert!(ok.verified && !ok.armed() && ok.status == "reverted" && ok.reason == "event 41" && ok.last_error.is_empty());
        assert!(out(&crashed, Applied::Unverified, true).verified);
        // whatever happened after a crash, the trial is "reverted": Keep is never offered on the crashing tune
        for a in [Applied::Unverified, Applied::Mismatch, Applied::Failed] {
            let r = out(&crashed, a, false);
            assert!(r.status == "reverted" && !r.verified && r.revert_unverified() && !r.last_error.is_empty(), "{a:?}");
            assert!(keep_target(Some(&crashed), Some(&r)).is_err());
            assert_eq!(guard_decision(Some(&r), 0, false), Decision::RetryRevert);
        }
        assert!(!out(&crashed, Applied::Mismatch, true).verified, "the GPU runs something else");
        // the retry is bounded: after MAX_REVERT_ATTEMPTS sends the check gives up and says so
        let mut t = crashed.clone();
        for n in 1..=MAX_REVERT_ATTEMPTS {
            t = out(&t, Applied::Unverified, false);
            assert_eq!(t.attempts, n);
            assert_eq!(t.armed(), n < MAX_REVERT_ATTEMPTS);
        }
        assert!(t.gave_up && !t.verified);
        assert_eq!(guard_decision(Some(&t), 0, false), Decision::NoTrial, "a leftover task does nothing about it");
        assert!(reverted_text(&t).contains("sign-in check has stopped") && reverted_text(&t).contains("at 3 sign-ins"));
        // Stock never reached the GPU: the notice must not say it was sent, before or after giving up
        let failed = out(&crashed, Applied::Failed, false);
        assert!(!failed.sent && failed.sends == 0 && failed.armed());
        assert!(!reverted_text(&failed).contains("was sent") && reverted_text(&failed).contains("could NOT be loaded"));
        let mut f = crashed.clone();
        for _ in 0..MAX_REVERT_ATTEMPTS {
            f = out(&f, Applied::Failed, false);
        }
        assert!(f.gave_up && !f.sent && f.attempts == MAX_REVERT_ATTEMPTS);
        assert!(!reverted_text(&f).contains("was sent") && reverted_text(&f).contains("could NOT be loaded"));
        // a failed attempt, one that reached the GPU, another failed one: only the real send is counted
        let mixed = out(&out(&out(&crashed, Applied::Failed, false), Applied::Unverified, false), Applied::Failed, false);
        assert!(mixed.gave_up && mixed.sent && mixed.sends == 1);
        assert!(reverted_text(&mixed).contains("at 1 sign-in but never confirmed"));
        // a later confirmation still settles it
        assert!(out(&Trial { attempts: 1, ..out(&crashed, Applied::Failed, false) }, Applied::Confirmed, false).verified);
        // the tab settles an unconfirmed revert once the driver reports no offsets
        let unv = out(&crashed, Applied::Unverified, false);
        let settled = settled_revert(&unv, Some(&live(0, 0))).unwrap();
        assert!(settled.verified && !settled.armed() && settled.last_error.is_empty());
        assert_eq!(settled_revert(&unv, Some(&live(148, 2000))), None, "not Stock");
        assert_eq!(settled_revert(&unv, None), None);
        assert_eq!(settled_revert(&crashed, Some(&live(0, 0))), None, "pending trials are not touched");
        // the notice only states Stock as fact when it was confirmed
        let t = Trial { sent: true, sends: 1, attempts: 1, ..trial("reverted", 1) };
        assert!(reverted_text(&t).contains("not yet confirmed"));
        assert!(reverted_text(&Trial { verified: true, ..t }).contains("was set back to Stock at sign-in"));
        // older "reverted" files without the field read as unconfirmed
        assert!(!serde_json::from_str::<Trial>(r#"{"preset":"Balanced","status":"reverted"}"#).unwrap().verified);
    }
}
