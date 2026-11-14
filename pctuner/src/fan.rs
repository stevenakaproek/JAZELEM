//! Fan Tuner tab - live fan speeds and temperatures, driven by the PCTunerFan helper
//! (LibreHardwareMonitorLib + PawnIO). The helper holds the safety rules (floors, 90 C override,
//! fans back to the BIOS on exit/crash); this side shows the data and sends requests.

use crate::common::{self, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/fan_files.rs"));
}

const HISTORY: usize = 60;

// ------------------------------------------------------------------ helper protocol

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct FanInfo {
    pub id: String,
    pub hw: String,
    pub name: String,
    pub rpm: Option<f32>,
    pub percent: Option<f32>,
    pub controllable: bool,
    /// Lowest duty the driver accepts (e.g. 30 % for the RTX 5080).
    pub min: Option<f32>,
    pub mode: String,
    pub pump: bool,
    pub gpu: bool,
    pub requested: Option<f32>,
    /// The helper is running a curve for this fan.
    pub curve: bool,
}

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct TempInfo {
    pub id: String,
    pub hw: String,
    pub kind: String,
    pub name: String,
    pub value: Option<f32>,
}

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Status {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub emergency: bool,
    /// Temperatures unreadable for > 5 s: the helper has handed the fans to the BIOS until they read again.
    pub blind: bool,
    /// The helper's latest notice; it stays in every status line until a new one replaces it.
    pub msg: String,
    /// Goes up by one for every new notice (so a dismissed notice stays dismissed).
    #[serde(rename = "msgSeq")]
    pub msg_seq: u64,
    pub fans: Vec<FanInfo>,
    pub temps: Vec<TempInfo>,
}

// ------------------------------------------------------------------ what each fan should do

pub const FAN_FLOOR: f32 = 20.0;
pub const PUMP_FLOOR: f32 = 50.0;

/// How one fan is driven. `Bios` = hands off (the BIOS / graphics driver decides).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum FanSetting {
    #[default]
    Bios,
    Manual { percent: f32 },
    /// `points` are [temperature °C, duty %], sorted by temperature.
    Curve { source: Source, points: Vec<[f32; 2]> },
}

/// Which temperature a curve follows.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Cpu,
    Gpu,
    Hottest,
}

impl Source {
    pub fn key(self) -> &'static str {
        match self {
            Source::Cpu => "cpu",
            Source::Gpu => "gpu",
            Source::Hottest => "hottest",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Source::Cpu => "CPU temperature",
            Source::Gpu => "GPU temperature",
            Source::Hottest => "Hotter of CPU and GPU",
        }
    }
    /// The temperature this source reads now (falls back to whichever one is known, like the helper).
    pub fn read(self, cpu: Option<f32>, gpu: Option<f32>) -> Option<f32> {
        let hottest = match (cpu, gpu) {
            (Some(c), Some(g)) => Some(c.max(g)),
            (c, g) => c.or(g),
        };
        match self {
            Source::Cpu => cpu.or(hottest),
            Source::Gpu => gpu.or(hottest),
            Source::Hottest => hottest,
        }
    }
}

/// Duty for `temp` on a curve, linear between points, flat beyond the ends (same rule as the helper).
pub fn curve_duty(points: &[[f32; 2]], temp: f32) -> f32 {
    let Some(first) = points.first() else { return 100.0 };
    if temp <= first[0] {
        return first[1];
    }
    for w in points.windows(2) {
        let ([t0, d0], [t1, d1]) = (w[0], w[1]);
        if temp <= t1 {
            return if t1 <= t0 { d1 } else { d0 + (d1 - d0) * (temp - t0) / (t1 - t0) };
        }
    }
    points[points.len() - 1][1]
}

/// Starting curve for a fan: quiet at idle, 100 % at 85 °C, never under the fan's floor.
pub fn default_curve(name: &str, gpu: bool, pump: bool, floor: f32) -> FanSetting {
    let source = if gpu {
        Source::Gpu
    } else if pump || name.to_ascii_lowercase().contains("cpu") {
        Source::Cpu
    } else {
        Source::Hottest
    };
    const BALANCED: [[f32; 2]; 5] = [[30.0, 30.0], [50.0, 40.0], [65.0, 60.0], [75.0, 80.0], [85.0, 100.0]];
    let points = BALANCED.iter().map(|p| [p[0], p[1].max(floor)]).collect();
    FanSetting::Curve { source, points }
}

/// Lowest duty PC Tuner allows for a fan: 20 % (50 % for pumps), or the driver's own minimum if higher.
pub fn floor_for(pump: bool, driver_min: Option<f32>) -> f32 {
    let safety = if pump { PUMP_FLOOR } else { FAN_FLOOR };
    safety.max(driver_min.unwrap_or(0.0)).min(100.0)
}

// ------------------------------------------------------------------ profiles

pub const BUILTIN: [&str; 3] = ["Silent", "Balanced", "Performance"];
const PROFILES_FILE: &str = "fan-profiles.json";

/// A saved set of fan settings. Fans not listed are left on BIOS control.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Profile {
    pub fans: BTreeMap<String, FanSetting>,
    /// Older versions kept pump choices per profile; they are moved into `ProfileStore::pumps` on load.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pumps: BTreeMap<String, bool>,
}

/// %LOCALAPPDATA%\PCTuner\fan-profiles.json
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct ProfileStore {
    /// Applied when PC Tuner starts.
    pub last: Option<String>,
    pub profiles: BTreeMap<String, Profile>,
    /// Fans the user tuned on their own: profiles leave these alone ("own setting").
    pub pinned: BTreeMap<String, FanSetting>,
    /// "This is a pump" choices (override the name-based guess). A fact about the hardware, so the same
    /// for every profile.
    pub pumps: BTreeMap<String, bool>,
}

/// Curve points as the editor keeps them: in temperature order (a stable sort, so equal temperatures keep
/// their order like the helper's), 20-100 °C, at least 2 °C apart, duty 0-100 %. None when fewer than
/// 2 points are left.
pub fn normalize_points(points: &[[f32; 2]]) -> Option<Vec<[f32; 2]>> {
    let mut pts: Vec<[f32; 2]> = points
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .map(|p| [p[0].clamp(GRAPH_T.0, GRAPH_T.1), p[1].clamp(0.0, 100.0)])
        .collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        let t = match out.last() {
            Some(prev) => p[0].max(prev[0] + 2.0),
            None => p[0],
        };
        if t <= GRAPH_T.1 && out.len() < MAX_POINTS {
            out.push([t, p[1]]);
        }
    }
    (out.len() >= MIN_POINTS).then_some(out)
}

// ------------------------------------------------------------------ curve point editing

/// A curve keeps 2 to 16 points (16 is the most the fan helper accepts).
pub const MIN_POINTS: usize = 2;
pub const MAX_POINTS: usize = 16;
/// Neighbouring points stay at least this far apart (°C).
const POINT_GAP: f32 = 2.0;

/// Temperatures point `i` may take: between its neighbours (2 °C apart) and inside the graph.
fn temp_range(points: &[[f32; 2]], i: usize) -> (f32, f32) {
    let lo = if i > 0 { points[i - 1][0] + POINT_GAP } else { GRAPH_T.0 };
    let hi = if i + 1 < points.len() { points[i + 1][0] - POINT_GAP } else { GRAPH_T.1 };
    (lo, hi.max(lo))
}

/// Move point `i` to temperature `t` (whole degrees). It stays between its neighbours, so typing or
/// dragging never reorders the curve. Returns true when it moved.
pub fn set_point_temp(points: &mut [[f32; 2]], i: usize, t: f32) -> bool {
    if i >= points.len() || !t.is_finite() {
        return false;
    }
    let (lo, hi) = temp_range(points, i);
    let nt = t.round().clamp(lo, hi);
    let moved = nt != points[i][0];
    points[i][0] = nt;
    moved
}

/// Set point `i` to `d` % (whole percent, never under the fan's floor). Returns true when it changed.
pub fn set_point_duty(points: &mut [[f32; 2]], i: usize, d: f32, floor: f32) -> bool {
    if i >= points.len() || !d.is_finite() {
        return false;
    }
    let nd = d.round().clamp(floor.min(100.0), 100.0);
    let moved = nd != points[i][1];
    points[i][1] = nd;
    moved
}

/// Add a point where the curve has most room: the middle of the widest gap between two points, or past the
/// last one when that is wider. It sits on the curve (same duty as now), so adding it changes nothing until
/// it is moved. False at 16 points or when no gap is wide enough (4 °C).
pub fn add_point(points: &mut Vec<[f32; 2]>, floor: f32) -> bool {
    if points.len() >= MAX_POINTS || points.is_empty() {
        return false;
    }
    // (gap width, temperature for the new point)
    let mut best: Option<(f32, f32)> = None;
    for w in points.windows(2) {
        let gap = w[1][0] - w[0][0];
        if best.map_or(true, |b| gap > b.0) {
            best = Some((gap, ((w[0][0] + w[1][0]) / 2.0).round()));
        }
    }
    let last = points[points.len() - 1][0];
    let tail = GRAPH_T.1 - last;
    if best.map_or(true, |b| tail > b.0) {
        best = Some((tail, (last + (tail / 2.0).min(10.0)).round()));
    }
    let first = points[0][0];
    let head = first - GRAPH_T.0;
    if best.map_or(true, |b| head > b.0) {
        best = Some((head, (first - (head / 2.0).min(10.0)).round()));
    }
    match best {
        Some((gap, t)) if gap >= 2.0 * POINT_GAP => insert_at(points, t, curve_duty(points, t), floor),
        _ => false,
    }
}

/// Insert a point at `t` °C / `d` % (double-click on the graph). False when it would be within 2 °C of
/// another point, outside the graph, or past 16 points.
pub fn insert_at(points: &mut Vec<[f32; 2]>, t: f32, d: f32, floor: f32) -> bool {
    if points.len() >= MAX_POINTS || !t.is_finite() || !d.is_finite() {
        return false;
    }
    let t = t.round();
    if t < GRAPH_T.0 || t > GRAPH_T.1 || points.iter().any(|p| (p[0] - t).abs() < POINT_GAP) {
        return false;
    }
    let at = points.iter().position(|p| p[0] > t).unwrap_or(points.len());
    points.insert(at, [t, d.round().clamp(floor.min(100.0), 100.0)]);
    true
}

/// Remove point `i`. False when only 2 would be left.
pub fn remove_point(points: &mut Vec<[f32; 2]>, i: usize) -> bool {
    if points.len() <= MIN_POINTS || i >= points.len() {
        return false;
    }
    points.remove(i);
    true
}

/// A setting as loaded from fan-profiles.json, with its curve normalised (an unreadable curve becomes the
/// Balanced curve; the helper still applies the fan's floor).
fn normalize_setting(s: &mut FanSetting) {
    if let FanSetting::Curve { points, .. } = s {
        *points = normalize_points(points).unwrap_or_else(|| preset_points("Balanced").map(|p| p.to_vec()).unwrap_or_default());
    }
}

/// Tidy a loaded store: normalise every curve, and move old per-profile pump choices into the global map.
pub fn normalize_store(store: &mut ProfileStore) {
    for p in store.profiles.values_mut() {
        p.fans.values_mut().for_each(normalize_setting);
        for (id, v) in std::mem::take(&mut p.pumps) {
            store.pumps.entry(id).or_insert(v);
        }
    }
    store.pinned.values_mut().for_each(normalize_setting);
}

fn preset_points(name: &str) -> Option<[[f32; 2]; 5]> {
    match name {
        "Silent" => Some([[30.0, 20.0], [50.0, 25.0], [65.0, 40.0], [75.0, 60.0], [85.0, 100.0]]),
        "Balanced" => Some([[30.0, 30.0], [50.0, 40.0], [65.0, 60.0], [75.0, 80.0], [85.0, 100.0]]),
        "Performance" => Some([[30.0, 40.0], [50.0, 55.0], [65.0, 75.0], [75.0, 90.0], [80.0, 100.0]]),
        _ => None,
    }
}

/// A built-in profile for the fans this PC has: a curve per controllable fan that is spinning (empty
/// headers stay on BIOS), never under its floor. The GPU is included even when its fans are stopped at idle.
/// Silent leaves the graphics card on its own curve, which can stop its fans at idle.
pub fn builtin_profile(name: &str, fans: &[FanInfo], pumps: &BTreeMap<String, bool>) -> Option<Profile> {
    let pts = preset_points(name)?;
    let mut p = Profile::default();
    for f in fans.iter().filter(|f| f.controllable && (f.gpu || f.rpm.unwrap_or(0.0) > 0.0)) {
        let pump = pumps.get(&f.id).copied().unwrap_or(f.pump);
        if name == "Silent" && f.gpu {
            continue;
        }
        let floor = floor_for(pump, f.min);
        let FanSetting::Curve { source, .. } = default_curve(&f.name, f.gpu, pump, floor) else { continue };
        let points = pts.iter().map(|q| [q[0], q[1].max(floor)]).collect();
        p.fans.insert(f.id.clone(), FanSetting::Curve { source, points });
    }
    Some(p)
}

/// Can a new profile be saved under `name`? (false, reason) for an empty, too long, built-in or taken name
/// (case-insensitive, so "gaming" does not silently replace "Gaming").
pub fn new_name_check(name: &str, store: &ProfileStore) -> (bool, Option<String>) {
    if name.is_empty() {
        return (false, None);
    }
    if name.chars().count() > 32 {
        return (false, Some("Use 32 characters or fewer.".into()));
    }
    if BUILTIN.iter().any(|b| b.eq_ignore_ascii_case(name)) {
        return (false, Some(format!("{name} is a built-in profile - pick it and use Save changes, or choose another name.")));
    }
    if let Some(k) = store.profiles.keys().find(|k| k.eq_ignore_ascii_case(name)) {
        return (false, Some(format!("A profile called {k} already exists - pick it and use Save changes, or choose another name.")));
    }
    (true, None)
}

/// Settings without the fans left on BIOS (so "BIOS" and "not listed" compare equal).
pub fn normalized(settings: &HashMap<String, FanSetting>) -> BTreeMap<String, FanSetting> {
    settings.iter().filter(|(_, s)| **s != FanSetting::Bios).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Fans PC Tuner asks to drive that the helper reports as not driven (no request, no curve, not in software
/// mode): it refused or dropped the setting, so the BIOS has them. While the helper is "blind" it keeps the
/// requests, so those fans are not listed here.
pub fn handed_back(settings: &HashMap<String, FanSetting>, status: &Status) -> Vec<String> {
    status
        .fans
        .iter()
        .filter(|f| f.controllable && settings.get(&f.id).is_some_and(|s| *s != FanSetting::Bios))
        .filter(|f| f.requested.is_none() && !f.curve && f.mode != "software")
        .map(|f| f.id.clone())
        .collect()
}

/// What the chip on a controllable fan card says, and its colour.
fn fan_chip(f: &FanInfo, setting: &FanSetting, can: bool, emergency: bool, blind: bool, handed_back: bool) -> (&'static str, egui::Color32) {
    let ours = *setting != FanSetting::Bios;
    if !can {
        // view-only: nothing is sent, so show what the helper reports, not the saved setting
        return match (f.mode == "software", f.curve) {
            (true, true) => ("Curve", theme::ACCENT),
            (true, false) => ("PC Tuner", theme::ACCENT),
            _ => ("BIOS", theme::DIM),
        };
    }
    if ours && blind {
        ("BIOS - temps unreadable", theme::GAMING)
    } else if ours && handed_back {
        ("BIOS (handed back)", theme::GAMING)
    } else if ours && emergency {
        ("100 % - hot", theme::LOCKED)
    } else if matches!(setting, FanSetting::Curve { .. }) {
        ("Curve", theme::ACCENT)
    } else if ours {
        ("PC Tuner", theme::ACCENT)
    } else {
        ("BIOS", theme::DIM)
    }
}

/// The helper command that puts `id` into `setting`.
pub fn command_for(id: &str, setting: &FanSetting) -> String {
    match setting {
        FanSetting::Bios => format!("auto {id}"),
        FanSetting::Manual { percent } => format!("set {id} {:.0}", percent.clamp(0.0, 100.0)),
        FanSetting::Curve { source, points } => {
            let pts: Vec<String> = points.iter().map(|p| format!("{:.0}:{:.0}", p[0], p[1].clamp(0.0, 100.0))).collect();
            format!("curve {id} {} {}", source.key(), pts.join(","))
        }
    }
}

/// One status line from the helper (anything that is not a JSON object is ignored).
pub fn parse_status(line: &str) -> Option<Status> {
    let t = line.trim().trim_start_matches('\u{feff}');
    if !t.starts_with('{') {
        return None;
    }
    serde_json::from_str(t).ok()
}

/// Colour for a temperature: green when cool, yellow when warm, red near the 90 C limit.
fn temp_color(t: f32) -> egui::Color32 {
    if t >= 85.0 {
        theme::LOCKED
    } else if t >= 70.0 {
        theme::GAMING
    } else {
        theme::ACCENT
    }
}

fn push(h: &mut VecDeque<f32>, v: f32) {
    if h.len() >= HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

/// Latest data from the helper, filled by the reader thread (keeps going while another tab is open).
#[derive(Default)]
struct Shared {
    status: Option<Status>,
    rpm: HashMap<String, VecDeque<f32>>,
    cpu: VecDeque<f32>,
    gpu: VecDeque<f32>,
    log: Vec<String>,
    exited: bool,
    /// When the last status line arrived (the helper sends one about every second).
    last: Option<Instant>,
    /// Session averages: every CPU / GPU reading since PC Tuner started. Never cleared (not even when a
    /// helper is replaced), so they cover the whole session.
    avg: AvgSums,
}

#[derive(Default, Clone, Copy, Debug)]
struct AvgSums {
    cpu_sum: f64,
    cpu_n: u64,
    gpu_sum: f64,
    gpu_n: u64,
    /// The first reading.
    first: Option<Instant>,
}

/// Average CPU / GPU temperature since PC Tuner started, and over how long.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct Avg {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub over: Option<Duration>,
}

impl AvgSums {
    fn add(&mut self, cpu: Option<f32>, gpu: Option<f32>) {
        if cpu.is_none() && gpu.is_none() {
            return;
        }
        self.first.get_or_insert_with(Instant::now);
        if let Some(c) = cpu.filter(|c| c.is_finite()) {
            self.cpu_sum += c as f64;
            self.cpu_n += 1;
        }
        if let Some(g) = gpu.filter(|g| g.is_finite()) {
            self.gpu_sum += g as f64;
            self.gpu_n += 1;
        }
    }
    fn avg(&self) -> Avg {
        let mean = |s: f64, n: u64| (n > 0).then(|| (s / n as f64) as f32);
        Avg { cpu: mean(self.cpu_sum, self.cpu_n), gpu: mean(self.gpu_sum, self.gpu_n), over: self.first.map(|t| t.elapsed()) }
    }
}

/// "42 min", "1 h 05 min", "35 s"
fn span_text(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
    }
}

/// No status for this long: the data on screen is shown as stale (display only).
const STALE_AFTER: Duration = Duration::from_secs(5);
/// A running helper silent for this long is hung and is let go (its input is closed, see `Helper::detach`). Well above the helper's own
/// 10 s watchdog, so a few slow sensor reads never cost the fans their safety loop.
const HANG_AFTER: Duration = Duration::from_secs(30);
/// A fresh helper that sends nothing for this long (opening the PawnIO driver can take a while) is hung.
const FIRST_STATUS_WITHIN: Duration = Duration::from_secs(30);
/// The helper's watchdog (Safety.WatchdogSeconds in Program.cs).
#[cfg(test)]
const HELPER_WATCHDOG: Duration = Duration::from_secs(10);

/// A hung helper that was let go (its input closed) is killed only after this long: it hands the fans back
/// itself as soon as its loop runs again, which a kill would make impossible.
const DETACHED_KILL_AFTER: Duration = Duration::from_secs(150);

/// What a helper's stderr says about the fans: (fans handed back on exit, start-up reset pass done with this
/// many fans it could not confirm). Only graphics-card fans can be handed back by a new process: a
/// motherboard header an earlier helper left in manual mode stays there until a reboot ("unconfirmed").
fn log_marks(line: &str) -> (bool, Option<u32>) {
    let pass = line.strip_prefix("reset pass done:").map(|rest| {
        // "reset pass done: <g> gpu, <m> unconfirmed"; anything else counts as unconfirmed
        rest.split(',')
            .find_map(|p| p.trim().strip_suffix(" unconfirmed").and_then(|n| n.trim().parse::<u32>().ok()))
            .unwrap_or(1)
    });
    (line.starts_with("fans returned to BIOS"), pass)
}

/// Seconds since 1970 when Windows started (to tell whether the PC rebooted since fans were left stuck).
pub(crate) fn boot_time() -> u64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // SAFETY: GetTickCount64 takes no arguments and cannot fail
    now.saturating_sub(unsafe { GetTickCount64() } / 1000)
}

/// Fans left stuck by a helper stay stuck until a reboot: remembered (with the boot time) across PC Tuner
/// restarts. Two boot times within this much are the same boot (clock adjustments).
pub(crate) const SAME_BOOT_SECS: u64 = 120;
const STUCK_FILE: &str = "fan-stuck.txt";

const STUCK_AT_START: &str = "An earlier fan helper did not hand the fans back since Windows started - they may stay at their last speed. PC Tuner drives them with your settings and its safety rules; reboot to give them back to the BIOS.";
const WAITING_FOR_OLD_HELPER: &str = "The fan helper stopped answering. Waiting for it to hand the fans back to the BIOS (it does as soon as it runs again) - until then fans may stay at their last speed. A new helper starts after it exits.";
const RESTARTED_STUCK: &str = "The fan helper stopped without handing the fans back and was restarted. It drives your fans again with its safety rules; motherboard fans cannot go back to the BIOS until you reboot (graphics-card fans were handed back).";

fn stuck_since_this_boot(saved: &str, boot: u64) -> bool {
    saved.trim().parse::<u64>().map(|b| b.abs_diff(boot) <= SAME_BOOT_SECS).unwrap_or(false)
}

/// The card shown instead of live data: (headline, detail). Only says "BIOS control" when that is known:
/// a running helper still owns its fans, and a stuck one may have left them at their last duty.
fn stale_text(helper_gone: bool, stuck: bool, age_secs: u64) -> (String, String) {
    if !helper_gone {
        return (
            format!("No data from the fan helper for {age_secs} s."),
            format!("Fans may stay at their last speed while it is stuck. If it stays silent for {} s PC Tuner closes its input: it hands the fans back once it runs again.", HANG_AFTER.as_secs()),
        );
    }
    if stuck {
        return (
            "The fan helper is not running - fans may be stuck at their last speed.".into(),
            "Reboot to give them back to the BIOS.".into(),
        );
    }
    (
        "The fan helper is not running - fans are on BIOS control.".into(),
        "The fans are safe: the BIOS / graphics driver drives them.".into(),
    )
}

/// The banner when no helper is running any more. `stuck`: a helper that drove fans went away without
/// saying it handed them back (only a reboot gives such fans back to the BIOS).
fn gone_message(stuck: bool, tail: &str) -> String {
    let head = if stuck {
        "The fan helper stopped without confirming it handed the fans back - fans may be stuck at their last speed. Reboot to give them back to the BIOS."
    } else {
        "The fan helper stopped. Fans are on BIOS control."
    };
    format!("{head} {tail}").trim().to_string()
}

impl Shared {
    /// Forget the live data (the helper is gone for good).
    fn clear(&mut self) {
        self.status = None;
        self.rpm.clear();
        self.cpu.clear();
        self.gpu.clear();
        self.last = None;
    }

    fn take_status(&mut self, s: Status) {
        self.last = Some(Instant::now());
        for f in &s.fans {
            if let Some(r) = f.rpm {
                push(self.rpm.entry(f.id.clone()).or_default(), r);
            }
        }
        if let Some(c) = s.cpu {
            push(&mut self.cpu, c);
        }
        if let Some(g) = s.gpu {
            push(&mut self.gpu, g);
        }
        self.avg.add(s.cpu, s.gpu);
        self.status = Some(s);
    }
}

/// Unpack the embedded helper to %LOCALAPPDATA%\PCTuner\fan and return the exe path.
fn extract_helper() -> Result<PathBuf, String> {
    if embedded::FILES.is_empty() {
        return Err("The fan helper was not built into this copy of PC Tuner (run fanhelper\\build.ps1, then rebuild).".into());
    }
    let dir = common::data_dir().join("fan");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    for (name, bytes) in embedded::FILES {
        let p = dir.join(name);
        let same = std::fs::read(&p).map(|old| old == *bytes).unwrap_or(false);
        if !same {
            // a file can be locked by a helper that is still shutting down; the exe check below catches real problems
            let _ = std::fs::write(&p, bytes);
        }
    }
    let exe = dir.join("PCTunerFan.exe");
    if exe.exists() {
        Ok(exe)
    } else {
        Err(format!("could not unpack the fan helper to {}", dir.display()))
    }
}

/// The running helper process.
struct Helper {
    child: Child,
    /// None once closed (detach): the helper then hands its fans back as soon as its loop runs.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    shared: Arc<Mutex<Shared>>,
    stopped: bool,
    since: Instant,
    launch: Launch,
    /// Started with --reset-only: it hands every fan back to the BIOS and exits (takes no commands).
    reset_only: bool,
    /// Its stderr said "fans returned to BIOS" (its ResetAll ran).
    back: Arc<AtomicBool>,
    /// Its reset pass: how many fans it could not confirm as handed back (u32::MAX: no pass reported yet).
    pass_unconfirmed: Arc<AtomicU32>,
    /// Its stderr has closed (every line it wrote has been read).
    err_done: Arc<AtomicBool>,
    /// When it was first seen exited.
    exited_at: Option<Instant>,
    /// When it was let go as hung (input closed, never killed before DETACHED_KILL_AFTER).
    detached_at: Option<Instant>,
}

/// How a helper starts.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Launch {
    Normal,
    /// Reset pass on every controllable control first (an earlier helper may have left fans behind): GPU
    /// fans go back to the driver; motherboard headers cannot (their saved default may be stale), so they
    /// are driven with the settings and safety rules, and go to 100 % instead of being "handed back".
    Reclaim,
    /// Only that reset pass (graphics-card fans back to the driver), then exit.
    ResetOnly,
}

impl Helper {
    fn start(shared: Arc<Mutex<Shared>>, launch: Launch) -> Result<Helper, String> {
        let exe = extract_helper()?;
        let mut args = vec!["--parent".to_string(), std::process::id().to_string()];
        match launch {
            Launch::Normal => {}
            Launch::Reclaim => args.push("--reclaim".into()),
            Launch::ResetOnly => args.push("--reset-only".into()),
        }
        let mut child = Command::new(&exe)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(common::CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("could not start the fan helper: {e}"))?;
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().ok_or("no stdin")?)));
        let out = child.stdout.take().ok_or("no stdout")?;
        let err = child.stderr.take().ok_or("no stderr")?;
        {
            let shared = shared.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(s) = parse_status(&line) {
                        if let Ok(mut sh) = shared.lock() {
                            sh.take_status(s);
                        }
                    }
                }
                if let Ok(mut sh) = shared.lock() {
                    sh.exited = true;
                }
            });
        }
        let back = Arc::new(AtomicBool::new(false));
        let pass = Arc::new(AtomicU32::new(u32::MAX));
        let err_done = Arc::new(AtomicBool::new(false));
        {
            let shared = shared.clone();
            let (back, pass, err_done) = (back.clone(), pass.clone(), err_done.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let (b, p) = log_marks(&line);
                    if b {
                        back.store(true, Ordering::SeqCst);
                    }
                    if let Some(m) = p {
                        pass.store(m, Ordering::SeqCst);
                    }
                    if let Ok(mut sh) = shared.lock() {
                        sh.log.push(line);
                        if sh.log.len() > 50 {
                            sh.log.remove(0);
                        }
                    }
                }
                err_done.store(true, Ordering::SeqCst);
            });
        }
        // keep-alive: the helper hands fans back to the BIOS if it hears nothing for 10 s
        {
            let stdin = Arc::downgrade(&stdin);
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(s) = stdin.upgrade() else { break };
                let ok = s.lock().map(|mut w| w.as_mut().is_some_and(|w| writeln!(w, "ping").and_then(|_| w.flush()).is_ok())).unwrap_or(false);
                if !ok {
                    break;
                }
            });
        }
        Ok(Helper {
            child,
            stdin,
            shared,
            stopped: false,
            since: Instant::now(),
            launch,
            reset_only: launch == Launch::ResetOnly,
            back,
            pass_unconfirmed: pass,
            err_done,
            exited_at: None,
            detached_at: None,
        })
    }

    fn send(&self, cmd: &str) -> bool {
        self.stdin.lock().map(|mut w| w.as_mut().is_some_and(|w| writeln!(w, "{cmd}").and_then(|_| w.flush()).is_ok())).unwrap_or(false)
    }

    fn alive(&mut self) -> bool {
        let alive = matches!(self.child.try_wait(), Ok(None));
        if !alive && self.exited_at.is_none() {
            self.exited_at = Some(Instant::now());
        }
        alive
    }

    /// Its reset pass covered every control (--reclaim / --reset-only) and every one was a graphics-card
    /// fan the driver took back: nothing an earlier helper left can still be stuck.
    fn pass_cleared_all(&self) -> bool {
        self.launch != Launch::Normal && self.pass_unconfirmed.load(Ordering::SeqCst) == 0
    }

    /// Let a hung helper go without killing it: its input is closed (the keep-alive stops too), so it hands
    /// its fans back (ResetAll) as soon as its loop runs again, or its own 10 s watchdog fires. Never blocks.
    fn detach(&mut self) {
        self.stopped = true; // Drop never quits / kills it
        if let Ok(mut w) = self.stdin.lock() {
            *w = None;
        }
        self.detached_at = Some(Instant::now());
    }

    /// A detached helper that is still stuck after DETACHED_KILL_AFTER: killed as a last resort.
    fn kill_if_stuck_too_long(&mut self) {
        if self.detached_at.is_some_and(|t| t.elapsed() > DETACHED_KILL_AFTER) && self.alive() {
            let _ = self.child.kill();
            self.detached_at = None;
            if let Ok(mut sh) = self.shared.lock() {
                sh.log.push(format!("the fan helper was still stuck after {} s and was stopped", DETACHED_KILL_AFTER.as_secs()));
            }
        }
    }

    /// Without blocking: None while it runs (or its last stderr lines are still coming in, up to 1 s after
    /// it exited), else whether it said it handed its fans back.
    fn outcome(&mut self) -> Option<bool> {
        if self.alive() {
            return None;
        }
        let since = self.exited_at.map(|t| t.elapsed()).unwrap_or_default();
        if !self.err_done.load(Ordering::SeqCst) && since < Duration::from_secs(1) {
            return None;
        }
        Some(self.back.load(Ordering::SeqCst))
    }

    /// Running but silent: no status for 30 s (or none at all 30 s after it started).
    fn hung(&self) -> bool {
        let last = self.shared.lock().ok().and_then(|s| s.last);
        match last {
            Some(t) => t.elapsed() > HANG_AFTER,
            None => self.since.elapsed() > FIRST_STATUS_WITHIN,
        }
    }

    /// After it exited: did it say it handed its fans back? Waits up to 1 s for the last stderr lines.
    fn handed_back(&self) -> bool {
        let t0 = Instant::now();
        while !self.err_done.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.back.load(Ordering::SeqCst)
    }

    /// Ask the helper to hand every fan back to the BIOS and exit; kill it if it does not within 3 s.
    /// Only when PC Tuner closes (on_exit / Drop): a hung helper is detached instead, never killed early.
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Ok(mut w) = self.stdin.lock() {
            if let Some(w) = w.as_mut() {
                let _ = writeln!(w, "quit");
                let _ = w.flush();
            }
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        if let Ok(mut sh) = self.shared.lock() {
            sh.log.push("helper did not exit in time and was stopped".into());
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------------ demo data (screenshots)

fn demo_status(t: f64) -> Status {
    let w = |a: f64, p: f64| (t * a + p).sin() as f32;
    let cpu = 62.0 + 4.0 * w(0.7, 0.0);
    let gpu = 58.0 + 3.0 * w(0.5, 1.0);
    let fan = |id: &str, hw: &str, name: &str, rpm: f32, pct: Option<f32>, pump: bool, gpu: bool| FanInfo {
        id: id.into(),
        hw: hw.into(),
        name: name.into(),
        rpm: Some(rpm),
        percent: pct,
        controllable: pct.is_some(),
        min: if gpu { Some(30.0) } else if pct.is_some() { Some(0.0) } else { None },
        mode: if pct.is_some() { "default".into() } else { "none".into() },
        pump,
        gpu,
        requested: None,
        curve: false,
    };
    let board = "Nuvoton NCT6687D";
    Status {
        cpu: Some(cpu),
        gpu: Some(gpu),
        emergency: false,
        blind: false,
        msg: String::new(),
        msg_seq: 0,
        fans: vec![
            fan("/lpc/nct6687d/0/control/0", board, "CPU Fan", 1180.0 + 40.0 * w(0.9, 0.2), Some(46.0), false, false),
            fan("/lpc/nct6687d/0/control/1", board, "Pump Fan", 2650.0 + 20.0 * w(0.4, 0.4), Some(70.0), true, false),
            fan("/lpc/nct6687d/0/fan/2", board, "System Fan #1", 820.0 + 25.0 * w(0.6, 0.9), None, false, false),
            fan("/lpc/nct6687d/0/fan/3", board, "System Fan #2", 805.0 + 25.0 * w(0.6, 1.4), None, false, false),
            fan("/lpc/nct6687d/0/fan/4", board, "System Fan #3", 790.0 + 25.0 * w(0.6, 2.0), None, false, false),
            fan("/gpu-nvidia/0/control/1", "NVIDIA GeForce RTX 5080", "GPU Fan", 1150.0 + 30.0 * w(0.5, 0.5), Some(40.0), false, true),
        ],
        temps: vec![
            TempInfo { id: "c".into(), hw: "AMD Ryzen 7 9800X3D".into(), kind: "Cpu".into(), name: "Core (Tctl/Tdie)".into(), value: Some(cpu) },
            TempInfo { id: "g".into(), hw: "NVIDIA GeForce RTX 5080".into(), kind: "GpuNvidia".into(), name: "GPU Core".into(), value: Some(gpu) },
            TempInfo { id: "gm".into(), hw: "NVIDIA GeForce RTX 5080".into(), kind: "GpuNvidia".into(), name: "GPU Memory Junction".into(), value: Some(gpu + 10.0) },
            TempInfo { id: "b1".into(), hw: board.into(), kind: "SuperIO".into(), name: "System".into(), value: Some(38.0) },
            TempInfo { id: "b2".into(), hw: board.into(), kind: "SuperIO".into(), name: "VRM MOS".into(), value: Some(46.0) },
        ],
    }
}

/// Make the demo fans do what the demo settings say (duty = slider value or the curve's live point),
/// the way the helper would report them.
fn demo_apply(s: &mut Status, settings: &HashMap<String, FanSetting>) {
    let (cpu, gpu) = (s.cpu, s.gpu);
    for f in s.fans.iter_mut().filter(|f| f.controllable) {
        let floor = floor_for(f.pump, f.min);
        match settings.get(&f.id) {
            Some(FanSetting::Manual { percent }) => {
                f.percent = Some(percent.clamp(floor, 100.0));
                f.requested = Some(*percent);
                f.mode = "software".into();
            }
            Some(FanSetting::Curve { source, points }) => {
                if let Some(t) = source.read(cpu, gpu) {
                    let d = curve_duty(points, t.clamp(GRAPH_T.0, GRAPH_T.1)).max(floor);
                    f.percent = Some(d);
                    f.requested = Some(d);
                }
                f.curve = true;
                f.mode = "software".into();
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ tab

/// What a fan card asks for when the user touches it.
enum CardAction {
    Set(FanSetting),
    Pump(bool),
    /// Drop the fan's own setting and use the profile's again.
    FollowProfile,
    /// Open the large curve editor for this fan.
    OpenEditor,
}

pub struct FanTab {
    shared: Arc<Mutex<Shared>>,
    helper: Option<Helper>,
    error: Option<String>,
    restarts: u32,
    /// The running helper was sent a fan setting (set / curve), so it may own fans.
    drove: bool,
    /// A hung helper that was let go (input closed, not killed): it hands its fans back once it runs again.
    /// No new helper starts until it has exited, so two never drive the same chip.
    detached: Option<Helper>,
    /// A helper that may have owned fans went away without saying it handed them back: the fans may be
    /// stuck at their last duty until a reboot (a new helper cannot restore motherboard headers).
    /// Remembered across PC Tuner restarts until the PC reboots.
    stuck: bool,
    /// `stuck` is saved to STUCK_FILE (the real app; never in tests or the demo).
    persist: bool,
    /// A --reset-only helper was already tried for stuck fans.
    reclaim_tried: bool,
    started: bool,
    demo: bool,
    /// What the user asked for, per fan id.
    settings: HashMap<String, FanSetting>,
    /// Last command sent per key, so each change is sent once (cleared when the helper restarts).
    sent: HashMap<String, String>,
    /// When each command was sent (a setting the helper still does not have a while later is sent again).
    sent_at: HashMap<String, Instant>,
    /// Fans PC Tuner should drive but the helper reports as back on the BIOS (refused or dropped setting).
    handed_back: HashSet<String>,
    /// The helper notice the user dismissed (its msgSeq).
    msg_dismissed: u64,
    afterburner: Arc<AtomicBool>,
    store: ProfileStore,
    /// The profile the settings came from (they may have been changed since).
    active: Option<String>,
    new_name: String,
    note: Option<String>,
    /// An own setting changed at this time and is not saved yet (saved shortly after, so a curve drag is one write).
    pin_dirty: Option<Instant>,
    /// The fan whose curve is open in the large editor.
    editor: Option<String>,
    /// PCTUNER_FAN=rust: the Rust sensor reader in place of the C# helper (read-only, stage 2).
    rust: Option<RustReader>,
}

/// PCTUNER_FAN=rust selects the Rust hardware backend (read-only while it is being proven).
fn rust_mode() -> bool {
    std::env::var("PCTUNER_FAN").map(|v| v.eq_ignore_ascii_case("rust")).unwrap_or(false)
}

/// Reads the sensors in Rust (crate::hw) once a second into the same Shared the helper's reader thread fills.
/// Never controls a fan. Stops when dropped.
/// The Rust fan backend (PCTUNER_FAN=rust): a thread owning the hardware (crate::hw::backend::Hardware) and the
/// safety loop (crate::hw::controller::Controller). It takes the same command lines the helper took, reads
/// the sensors and runs the loop once a second, and fills the same Shared status. On drop it hands every fan
/// back and waits for that before returning, so PC Tuner never exits with a fan still driven.
struct RustReader {
    stop: Arc<AtomicBool>,
    /// Parts that could not be opened, and what happened at start (e.g. fans restored after a crash).
    errors: Arc<Mutex<Vec<String>>>,
    tx: std::sync::mpsc::Sender<String>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The sensor reading with the controller's view added (what the helper's status line carried).
fn with_control(mut st: Status, c: &crate::hw::controller::Controller, hw: &crate::hw::backend::Hardware) -> Status {
    for f in st.fans.iter_mut() {
        f.requested = c.requested(&f.id);
        f.curve = c.has_curve(&f.id);
        f.pump = c.is_pump(hw, &f.id) || f.pump && !c.is_not_pump(&f.id);
        if c.is_touched(&f.id) {
            f.mode = "software".into();
        }
    }
    st.emergency = c.emergency;
    st.blind = c.blind;
    st.msg = c.message.clone();
    st.msg_seq = c.message_seq;
    st
}

impl RustReader {
    fn start(shared: Arc<Mutex<Shared>>) -> RustReader {
        let stop = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(vec![]));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let (s, e) = (stop.clone(), errors.clone());
        let thread = std::thread::spawn(move || {
            let mut hw = crate::hw::backend::Hardware::open();
            if let Ok(mut e) = e.lock() {
                *e = hw.sensors.errors.iter().chain(&hw.notes).cloned().collect();
            }
            let mut c = crate::hw::controller::Controller::new(Instant::now());
            // a panic below still unwinds through `hw`, whose Drop hands every fan back
            while !s.load(Ordering::Relaxed) {
                while let Ok(line) = rx.try_recv() {
                    c.handle(&mut hw, &line);
                }
                let st = hw.sensors.read();
                // view-only PC Tuner: never drive anything
                if common::is_admin() {
                    c.tick(&mut hw, Instant::now(), st.cpu, st.gpu);
                }
                let st = with_control(st, &c, &hw);
                if let Ok(mut sh) = shared.lock() {
                    sh.exited = false;
                    sh.take_status(st);
                }
                // a second between ticks, but react to stop at once
                for _ in 0..10 {
                    if s.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            c.reset_all(&mut hw);
            drop(hw); // anything left is handed back here
        });
        RustReader { stop, errors, tx, thread: Some(thread) }
    }

    fn errors(&self) -> Vec<String> {
        self.errors.lock().map(|e| e.clone()).unwrap_or_default()
    }

    fn send(&self, cmd: &str) -> bool {
        self.tx.send(cmd.to_string()).is_ok()
    }
}

impl Drop for RustReader {
    /// Stop, and wait until every fan has been handed back.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl FanTab {
    pub fn new() -> Self {
        let demo = std::env::var("PCTUNER_DEMO").map(|v| v.to_ascii_lowercase().starts_with("fan")).unwrap_or(false);
        let store = if demo { ProfileStore::default() } else { common::load_json(PROFILES_FILE) };
        Self::new_with(store, demo, true)
    }

    /// `startup`: bring back the last profile / own settings (and start the helper as administrator), or
    /// set up the demo screen.
    /// Tests pass `demo = true, startup = false`, so they never read the user's file or touch a fan.
    fn new_with(mut store: ProfileStore, demo: bool, startup: bool) -> Self {
        normalize_store(&mut store);
        let mut tab = FanTab {
            shared: Arc::new(Mutex::new(Shared::default())),
            helper: None,
            error: None,
            restarts: 0,
            drove: false,
            detached: None,
            stuck: false,
            persist: startup && !demo,
            reclaim_tried: false,
            started: false,
            demo,
            settings: HashMap::new(),
            sent: HashMap::new(),
            sent_at: HashMap::new(),
            handed_back: HashSet::new(),
            msg_dismissed: 0,
            afterburner: Arc::new(AtomicBool::new(false)),
            store,
            active: None,
            new_name: String::new(),
            note: None,
            pin_dirty: None,
            editor: None,
            rust: None,
        };
        // apply the last profile at startup (only as administrator: otherwise nothing can be controlled)
        if startup && !demo {
            // fans an earlier PC Tuner's helper left stuck (same boot): the next helper reclaims them
            let saved = std::fs::read_to_string(common::data_dir().join(STUCK_FILE)).unwrap_or_default();
            if stuck_since_this_boot(&saved, boot_time()) {
                tab.stuck = true;
            } else if !saved.is_empty() {
                let _ = std::fs::remove_file(common::data_dir().join(STUCK_FILE));
            }
            let admin = common::is_admin();
            if let Some(name) = tab.store.last.clone() {
                if let Some(p) = tab.store.profiles.get(&name).cloned() {
                    tab.use_profile(&name, &p);
                    tab.note = Some(if admin {
                        format!("{name} was applied when PC Tuner started.")
                    } else {
                        format!("{name} will be applied when PC Tuner runs as administrator.")
                    });
                    if admin {
                        tab.start();
                    }
                }
            }
            // own fan settings without a profile still come back at startup
            if tab.active.is_none() && !tab.store.pinned.is_empty() {
                for (id, s) in tab.store.pinned.clone() {
                    tab.settings.insert(id, s);
                }
                tab.note = Some(if admin {
                    "Your own fan settings were applied when PC Tuner started.".into()
                } else {
                    "Your own fan settings will be applied when PC Tuner runs as administrator.".into()
                });
                if admin {
                    tab.start();
                }
            }
            if tab.stuck && tab.error.is_none() {
                tab.error = Some(STUCK_AT_START.into());
            }
        }
        if demo && startup {
            // the Balanced profile, with the CPU fan changed by hand (shows "changed - not saved")
            if let Some(p) = builtin_profile("Balanced", &demo_status(0.0).fans, &BTreeMap::new()) {
                tab.store.profiles.insert("Balanced".into(), p.clone());
                tab.use_profile("Balanced", &p);
            }
            // the CPU fan tuned by hand: its own setting, kept when profiles change
            tab.store.pinned.insert("/lpc/nct6687d/0/control/0".into(), FanSetting::Manual { percent: 45.0 });
            tab.settings.insert("/lpc/nct6687d/0/control/0".into(), FanSetting::Manual { percent: 45.0 });
            tab.note = Some("Balanced was applied when PC Tuner started.".into());
            tab.afterburner.store(true, Ordering::Relaxed);
            // PCTUNER_DEMO=fan:editor - the large curve editor, on the first fan that runs a curve
            if std::env::var("PCTUNER_DEMO").map(|v| v.eq_ignore_ascii_case("fan:editor")).unwrap_or(false) {
                let fans = demo_status(0.0).fans;
                tab.editor = fans.iter().find(|f| matches!(tab.settings.get(&f.id), Some(FanSetting::Curve { .. }))).map(|f| f.id.clone());
            }
        }
        tab
    }

    fn start(&mut self) {
        // fans an earlier helper may have left in software mode: the new one reclaims them (GPU fans go back
        // to the driver; motherboard headers are driven again, never "handed back" to a stale duty)
        let launch = if self.stuck { Launch::Reclaim } else { Launch::Normal };
        self.start_with(launch);
    }

    /// Mark fans as (not) stuck, remembered with the boot time so a restarted PC Tuner still knows.
    fn set_stuck(&mut self, stuck: bool) {
        if self.stuck == stuck {
            return;
        }
        self.stuck = stuck;
        if !self.persist {
            return;
        }
        let p = common::data_dir().join(STUCK_FILE);
        if stuck {
            let _ = std::fs::write(p, boot_time().to_string());
        } else {
            let _ = std::fs::remove_file(p);
        }
    }

    fn start_with(&mut self, launch: Launch) {
        let first = !self.started;
        self.started = true;
        if self.demo {
            return;
        }
        if self.detached.is_some() {
            // the hung helper may still hand its fans back: never two helpers on the same chip
            return;
        }
        if let Ok(mut sh) = self.shared.lock() {
            sh.exited = false;
            // the old helper's last status is not this one's
            sh.status = None;
            sh.last = None;
        }
        if rust_mode() {
            // the Rust fan backend in place of the C# helper (opt-in until it is the default)
            self.rust = Some(RustReader::start(self.shared.clone()));
            self.error = None;
            self.sent.clear();
            self.sent_at.clear();
            self.handed_back.clear();
        } else {
            self.start_helper(launch);
        }
        if !first {
            // the Afterburner check below is already running
            return;
        }
        // is MSI Afterburner running? (it may also be driving the GPU fans)
        let flag = Arc::downgrade(&self.afterburner);
        std::thread::spawn(move || loop {
            let Some(f) = flag.upgrade() else { break };
            f.store(crate::winsvc::process_pid("MSIAfterburner.exe").is_some(), Ordering::Relaxed);
            drop(f);
            std::thread::sleep(Duration::from_secs(5));
        });
    }

    fn start_helper(&mut self, launch: Launch) {
        match Helper::start(self.shared.clone(), launch) {
            Ok(h) => {
                self.helper = Some(h);
                self.drove = false;
                self.error = None;
                // a fresh helper knows nothing: send every setting again
                self.sent.clear();
                self.sent_at.clear();
                self.handed_back.clear();
                self.msg_dismissed = 0;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Send every setting / pump choice that changed since it was last sent.
    /// Only as administrator: a view-only PC Tuner never changes a fan.
    fn sync(&mut self) {
        if !common::is_admin() {
            return;
        }
        // the Rust backend, or the C# helper
        if self.rust.is_none() {
            let Some(h) = &self.helper else { return };
            if h.reset_only {
                // it takes no commands: settings go to the next helper
                return;
            }
        }
        // fans the helper dropped or refused: shown as handed back, and sent again now and then
        let (status_at, dropped) = match self.shared.lock() {
            Ok(sh) => (sh.last, sh.status.as_ref().map(|s| handed_back(&self.settings, s)).unwrap_or_default()),
            Err(_) => (None, vec![]),
        };
        self.handed_back.clear();
        for id in dropped {
            // only once a status has come in well after the last command (the helper had time to take it)
            let Some(sent) = self.sent_at.get(&id).copied() else { continue };
            if status_at.map(|t| t > sent + Duration::from_millis(1500)).unwrap_or(false) {
                self.handed_back.insert(id.clone());
                if sent.elapsed() > Duration::from_secs(5) {
                    self.sent.remove(&id);
                }
            }
        }
        let mut cmds: Vec<(String, String)> = vec![];
        for (id, choice) in &self.store.pumps {
            cmds.push((format!("pump:{id}"), format!("pump {id} {}", if *choice { 1 } else { 0 })));
        }
        for (id, s) in &self.settings {
            cmds.push((id.clone(), command_for(id, s)));
        }
        for (key, cmd) in cmds {
            if self.sent.get(&key) == Some(&cmd) {
                continue;
            }
            let sent = match (&self.rust, &self.helper) {
                (Some(r), _) => r.send(&cmd),
                (None, Some(h)) => h.send(&cmd),
                _ => false,
            };
            if sent {
                if cmd.starts_with("set ") || cmd.starts_with("curve ") {
                    self.drove = true;
                }
                self.sent_at.insert(key.clone(), Instant::now());
                self.sent.insert(key, cmd);
            }
        }
    }

    /// Every fan back to the BIOS, for good: own settings are cleared and no profile comes back at startup.
    /// Never sends anything in view-only mode.
    fn give_all_back(&mut self) {
        if !common::is_admin() && !self.demo {
            return;
        }
        for s in self.settings.values_mut() {
            *s = FanSetting::Bios;
        }
        self.store.pinned.clear();
        self.pin_dirty = None;
        self.active = None;
        self.store.last = None;
        self.save_store();
        self.note = Some(if self.stuck || self.detached.is_some() {
            // a reclaim helper cannot hand motherboard headers back (they go to 100 %); a hung one gets nothing
            "Own settings were cleared and no profile is applied at startup until you pick one. Graphics-card fans go back to the driver; motherboard fans an earlier helper left behind cannot go back to the BIOS until you reboot.".into()
        } else {
            "Every fan is back on BIOS control. Own settings were cleared and no profile is applied at startup until you pick one.".into()
        });
        if let Some(r) = &self.rust {
            r.send("autoall");
        } else if let Some(h) = &self.helper {
            h.send("autoall");
        }
    }

    fn any_controlled(&self) -> bool {
        self.settings.values().any(|s| *s != FanSetting::Bios)
    }

    /// Take a profile's settings: listed fans as saved, every other fan back to BIOS -
    /// except fans with their own setting, which keep it.
    fn use_profile(&mut self, name: &str, p: &Profile) {
        for s in self.settings.values_mut() {
            *s = FanSetting::Bios;
        }
        for (id, s) in &p.fans {
            self.settings.insert(id.clone(), s.clone());
        }
        for (id, s) in &self.store.pinned {
            self.settings.insert(id.clone(), s.clone());
        }
        // pump choices are global (store.pumps): a profile switch leaves them alone
        self.active = Some(name.to_string());
    }

    /// The setting the active profile gives a fan (BIOS when it is not listed).
    fn profile_setting(&self, id: &str) -> FanSetting {
        self.active.as_ref().and_then(|n| self.store.profiles.get(n)).and_then(|p| p.fans.get(id).cloned()).unwrap_or_default()
    }

    /// Give a fan its own setting (kept when profiles change); saved straight away.
    fn pin(&mut self, id: &str, s: &FanSetting) {
        self.store.pinned.insert(id.to_string(), s.clone());
        self.pin_dirty = Some(Instant::now());
    }

    /// Hand a fan back to the active profile.
    fn unpin(&mut self, id: &str) {
        self.store.pinned.remove(id);
        let s = self.profile_setting(id);
        self.settings.insert(id.to_string(), s);
        self.save_store();
    }

    /// Settings of the fans that follow the profile (own settings are not part of it).
    fn following(&self) -> BTreeMap<String, FanSetting> {
        normalized(&self.settings).into_iter().filter(|(k, _)| !self.store.pinned.contains_key(k)).collect()
    }

    fn save_store(&mut self) {
        if !self.demo {
            common::save_json(PROFILES_FILE, &self.store);
        }
    }

    /// Load a profile (a built-in one is created for this PC's fans the first time) and remember it for startup.
    fn load_profile(&mut self, name: &str, fans: &[FanInfo]) {
        let p = match self.store.profiles.get(name) {
            Some(p) => p.clone(),
            None => {
                let Some(p) = builtin_profile(name, fans, &self.store.pumps) else { return };
                self.store.profiles.insert(name.to_string(), p.clone());
                p
            }
        };
        self.use_profile(name, &p);
        self.store.last = Some(name.to_string());
        self.save_store();
        self.note = Some(format!("{name} is on. It will be applied again the next time PC Tuner starts."));
    }

    fn current_profile(&self) -> Profile {
        Profile { fans: self.following(), pumps: BTreeMap::new() }
    }

    fn modified(&self) -> bool {
        match self.active.as_ref().and_then(|n| self.store.profiles.get(n)) {
            Some(p) => {
                let saved: BTreeMap<String, FanSetting> = p.fans.iter().filter(|(k, _)| !self.store.pinned.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
                self.following() != saved
            }
            None => !self.following().is_empty(),
        }
    }

    /// The profile bar: pick, save, save as, reset, delete.
    fn profile_bar(&mut self, ui: &mut egui::Ui, fans: &[FanInfo], can: bool) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Profile");
                ui.add_space(8.0);
                let mut names: Vec<String> = BUILTIN.iter().map(|s| s.to_string()).collect();
                names.extend(self.store.profiles.keys().filter(|k| !BUILTIN.contains(&k.as_str())).cloned());
                ui.add_enabled_ui(can, |ui| {
                    for n in names {
                        let on = self.active.as_deref() == Some(n.as_str());
                        if ui.selectable_label(on, format!("  {n}  ")).clicked() {
                            self.load_profile(&n, fans);
                        }
                    }
                });
            });
            ui.add_space(4.0);
            let modified = self.modified();
            ui.horizontal(|ui| {
                match &self.active {
                    Some(n) if modified => {
                        ui.label(RichText::new(n.as_str()).strong());
                        ui.colored_label(theme::GAMING, "changed - not saved");
                    }
                    Some(n) => {
                        ui.label(RichText::new(n.as_str()).strong());
                        ui.colored_label(theme::TEXT_DIM, "saved");
                    }
                    None => {
                        ui.colored_label(theme::TEXT_DIM, "No profile - pick one, or set fans below and save them as a profile.");
                    }
                }
                ui.add_space(10.0);
                ui.add_enabled_ui(can, |ui| {
                    if let Some(n) = self.active.clone() {
                        if ui.add_enabled(modified, egui::Button::new("Save changes")).on_hover_text(format!("Store the fans as they are now in {n}.")).clicked() {
                            let p = self.current_profile();
                            self.store.profiles.insert(n.clone(), p);
                            self.store.last = Some(n.clone());
                            self.save_store();
                            self.note = Some(format!("{n} saved."));
                        }
                        if BUILTIN.contains(&n.as_str()) {
                            if ui.button("Reset to default").on_hover_text(format!("Throw away your edits to {n} and use PC Tuner's default curves.")).clicked() {
                                self.store.profiles.remove(&n);
                                self.load_profile(&n, fans);
                                self.note = Some(format!("{n} is back to its default curves."));
                            }
                        } else if ui.button("Delete").clicked() {
                            self.store.profiles.remove(&n);
                            if self.store.last.as_deref() == Some(n.as_str()) {
                                self.store.last = None;
                            }
                            self.active = None;
                            self.save_store();
                            self.note = Some(format!("{n} deleted. The fans stay as they are until you change them."));
                        }
                    }
                    ui.add_space(10.0);
                    ui.add(egui::TextEdit::singleline(&mut self.new_name).hint_text("New profile name").desired_width(150.0));
                    let name = self.new_name.trim().to_string();
                    let (ok, why) = new_name_check(&name, &self.store);
                    let r = ui.add_enabled(ok, egui::Button::new("Save as new"));
                    if let Some(why) = &why {
                        if !name.is_empty() {
                            ui.label(RichText::new(why.as_str()).size(11.5).color(theme::GAMING));
                        }
                    }
                    if r.on_disabled_hover_text(why.unwrap_or_else(|| "Type a name for the new profile.".into())).clicked() {
                        let p = self.current_profile();
                        self.store.profiles.insert(name.clone(), p);
                        self.store.last = Some(name.clone());
                        self.active = Some(name.clone());
                        self.save_store();
                        self.new_name.clear();
                        self.note = Some(format!("{name} saved and will be applied when PC Tuner starts."));
                    }
                });
            });
            if let Some(n) = &self.note {
                ui.label(RichText::new(n.as_str()).size(12.0).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(6.0);
    }

    /// Restart the helper once if it stopped unexpectedly.
    /// A helper that is running but has sent nothing for 30 s is hung (its loop may be stuck in a sensor
    /// read): it is never killed early, which would leave the fans at their last duty with no safety rules.
    /// Its input is closed instead (it hands its fans back as soon as its loop runs again) and it is watched
    /// without blocking; the next helper starts only once it has exited (killed after DETACHED_KILL_AFTER).
    /// A helper that may have owned fans and left without saying it handed them back makes the fans "stuck"
    /// until a reboot: the next helper reclaims them (or, when no restart is left, a reset-only helper hands
    /// the graphics-card fans back). The banner only says "BIOS control" when that is known.
    fn watch(&mut self) {
        if let Some(d) = &mut self.detached {
            d.kill_if_stuck_too_long();
            let Some(back) = d.outcome() else { return };
            let d = self.detached.take().expect("checked above");
            self.helper_gone(d, back);
            return;
        }
        if self.helper.as_ref().is_some_and(|h| h.pass_cleared_all()) {
            // every control was a graphics-card fan and the driver took it back
            self.set_stuck(false);
        }
        let Some(h) = &mut self.helper else { return };
        if h.alive() {
            if !h.hung() {
                return;
            }
            if let Ok(mut sh) = self.shared.lock() {
                sh.log.push("the fan helper stopped answering - its input was closed so it hands the fans back once it runs again".into());
            }
            h.detach();
            self.detached = self.helper.take();
            self.error = Some(WAITING_FOR_OLD_HELPER.into());
            return;
        }
        let Some(back) = h.outcome() else { return };
        let h = self.helper.take().expect("checked above");
        self.helper_gone(h, back);
    }

    /// A helper has exited (`back`: it said it handed its fans back): note stuck fans, then restart.
    fn helper_gone(&mut self, h: Helper, back: bool) {
        if h.pass_cleared_all() {
            self.set_stuck(false);
        }
        let reset_only = h.reset_only;
        drop(h);
        if self.drove && !back {
            self.set_stuck(true);
        }
        self.drove = false;
        let tail = self.shared.lock().map(|s| s.log.last().cloned().unwrap_or_default()).unwrap_or_default();
        if !reset_only && self.restarts < 1 {
            self.restarts += 1;
            self.start();
            self.error = Some(if self.helper.is_none() {
                gone_message(self.stuck, self.error.as_deref().unwrap_or(""))
            } else if self.stuck {
                RESTARTED_STUCK.into()
            } else {
                format!("The fan helper stopped and was restarted. {tail}").trim().to_string()
            });
        } else if self.stuck && !self.reclaim_tried {
            // no restart left: one helper that hands the graphics-card fans back to the driver, then exits
            self.reclaim_tried = true;
            self.start_with(Launch::ResetOnly);
            self.error = Some(if self.helper.is_none() {
                gone_message(true, self.error.as_deref().unwrap_or(""))
            } else {
                "The fan helper stopped responding - handing the graphics-card fans back to the driver. Motherboard fans may stay at their last speed until you reboot.".into()
            });
            if let Ok(mut sh) = self.shared.lock() {
                sh.clear();
            }
        } else {
            self.error = Some(gone_message(self.stuck, &tail));
            // nothing live any more: never show its last numbers (or a 90 °C banner) as if they were current
            if let Ok(mut sh) = self.shared.lock() {
                sh.clear();
            }
        }
    }

    /// Average CPU / GPU temperature since PC Tuner started (the demo: its sample minute, shown as 42 min).
    fn averages(&self) -> Avg {
        if self.demo {
            let mut sums = AvgSums::default();
            for i in 0..HISTORY {
                let s = demo_status(30.0 - (HISTORY - i - 1) as f64);
                sums.add(s.cpu, s.gpu);
            }
            return Avg { over: Some(Duration::from_secs(42 * 60)), ..sums.avg() };
        }
        self.shared.lock().map(|sh| sh.avg.avg()).unwrap_or_default()
    }

    /// The data to draw, and whether it is stale (the helper exited, is not running, or sent nothing for 5 s).
    fn snapshot(&self) -> (Option<Status>, HashMap<String, VecDeque<f32>>, VecDeque<f32>, VecDeque<f32>, Option<Duration>) {
        if self.demo {
            // a fixed moment, so the screenshot's numbers agree with each other
            const T: f64 = 30.0;
            let mut sh = Shared::default();
            for i in 0..HISTORY {
                let mut s = demo_status(T - (HISTORY - i - 1) as f64);
                demo_apply(&mut s, &self.settings);
                sh.take_status(s);
            }
            return (sh.status, sh.rpm, sh.cpu, sh.gpu, None);
        }
        match self.shared.lock() {
            Ok(sh) => {
                let age = sh.last.map(|t| t.elapsed());
                let stale = sh.exited || (self.helper.is_none() && self.rust.is_none()) || age.is_some_and(|a| a > STALE_AFTER);
                let stale_for = (sh.status.is_some() && stale).then(|| age.unwrap_or_default());
                (sh.status.clone(), sh.rpm.clone(), sh.cpu.clone(), sh.gpu.clone(), stale_for)
            }
            Err(_) => (None, HashMap::new(), VecDeque::new(), VecDeque::new(), None),
        }
    }
}

impl Tab for FanTab {
    fn title(&self) -> &'static str {
        "Fan Tuner"
    }

    fn tick(&mut self) {
        // keeps a startup profile running while another tab is open
        if self.started {
            self.watch();
            self.sync();
        }
        if self.pin_dirty.map(|t| t.elapsed() > Duration::from_millis(600)).unwrap_or(false) {
            self.pin_dirty = None;
            self.save_store();
        }
    }

    fn on_exit(&mut self) {
        if self.pin_dirty.take().is_some() {
            self.save_store();
        }
        // the Rust backend hands every fan back before this returns
        drop(self.rust.take());
        if let Some(mut h) = self.helper.take() {
            h.stop();
            if self.drove && !h.handed_back() {
                // killed after 3 s, or it did not say so: remembered for the next start (same boot)
                self.set_stuck(true);
            }
        }
        if let Some(mut d) = self.detached.take() {
            // never killed here: with PC Tuner gone it hands its fans back once it runs again; until it is
            // known, the next PC Tuner start treats the fans as stuck
            if self.drove && (d.alive() || !d.handed_back()) {
                self.set_stuck(true);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.started {
            self.start();
        }
        self.watch();
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        let can = common::is_admin() || self.demo;

        let mut give_back = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| theme::page_header(ui, "Fan Tuner", "Live fan speeds and temperatures. Fans always go back to the BIOS when PC Tuner closes."));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                let b = theme::danger_button("⟲  Give all fans back to BIOS");
                if ui
                    .add_enabled(can && self.any_controlled(), b)
                    .on_hover_text("Every fan goes back to its BIOS / graphics-driver curve. Own settings are cleared and no profile is applied at startup until you pick one.")
                    .on_disabled_hover_text(if can { "No fan is controlled by PC Tuner." } else { "Run PC Tuner as administrator to control fans." })
                    .clicked()
                {
                    give_back = true;
                }
            });
        });
        if give_back {
            self.give_all_back();
        }
        if !common::is_admin() && !self.demo {
            theme::banner(ui, theme::LOCKED, "Run PC Tuner as administrator to see the motherboard fans and the CPU temperature, and to control fans (PawnIO driver). Only the graphics card is visible now.");
        }
        if let Some(e) = &self.error {
            theme::banner(ui, theme::LOCKED, e);
        }
        if let Some(r) = &self.rust {
            let mut text = "Rust fan backend (preview): PC Tuner reads and drives the fans itself, without the fan helper. Each fan's BIOS settings are saved before it is first driven and put back exactly.".to_string();
            let errs = r.errors();
            if !errs.is_empty() {
                text.push_str(&format!(" Not available: {}.", errs.join("; ")));
            }
            theme::banner(ui, theme::GAMING, &text);
        }
        let (status, rpm_hist, cpu_hist, gpu_hist, stale_for) = self.snapshot();
        let avg = self.averages();
        // the large editor needs live data; it is never drawn over stale numbers
        let editor_status = if stale_for.is_none() { status.clone() } else { None };
        let afterburner = self.afterburner.load(Ordering::Relaxed);
        let mut actions: Vec<(String, f32, CardAction)> = vec![];
        let mut dismiss = None;
        // a detached (hung) helper may still own fans: never "BIOS control" while it runs
        let helper_gone = !self.demo && self.helper.is_none() && self.detached.is_none() && self.rust.is_none();
        let stuck = !self.demo && self.stuck;
        egui::ScrollArea::vertical().id_salt("fan-page").auto_shrink([false, false]).show(ui, |ui| {
            if let Some(age) = stale_for {
                // old numbers are not shown as live (and a stale 90 °C banner never stays up)
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (head, sub) = stale_text(helper_gone, stuck, age.as_secs());
                    ui.label(RichText::new(head).size(18.0).strong());
                    ui.label(RichText::new(sub).color(theme::TEXT_DIM));
                });
                return;
            }
            let Some(s) = status else {
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if helper_gone {
                        let (head, sub) = stale_text(true, stuck, 0);
                        ui.label(RichText::new(head).size(18.0).strong());
                        ui.label(RichText::new(sub).color(theme::TEXT_DIM));
                    } else {
                        ui.label(RichText::new("Reading sensors...").size(18.0).strong());
                        ui.add_space(6.0);
                        theme::sweep(ui, ui.available_width());
                    }
                });
                return;
            };
            if s.blind {
                theme::banner(ui, theme::GAMING, "Temperatures cannot be read - every fan is on BIOS control until they can be read again. Your settings come back by themselves.");
            }
            // the helper's latest notice stays until dismissed
            if !s.msg.is_empty() && s.msg_seq != self.msg_dismissed {
                ui.horizontal(|ui| {
                    if ui.small_button("Dismiss").clicked() {
                        dismiss = Some(s.msg_seq);
                    }
                    ui.colored_label(theme::GAMING, &s.msg);
                });
                ui.add_space(4.0);
            }
            hero_card(ui, &s, &cpu_hist, &gpu_hist, &avg);
            self.profile_bar(ui, &s.fans, can);
            let view = View {
                settings: &self.settings,
                pinned: &self.store.pinned,
                can,
                afterburner,
                profile: self.active.as_deref(),
                handed_back: &self.handed_back,
                points_open: self.demo,
            };
            fans_and_temps(ui, &s, &rpm_hist, &view, &mut actions);
        });
        if let Some(seq) = dismiss {
            self.msg_dismissed = seq;
        }
        // the large curve editor: its changes go through the same path as a fan card's
        if let Some(eid) = self.editor.clone() {
            let fan = editor_status.as_ref().and_then(|s| s.fans.iter().find(|f| f.id == eid).map(|f| (f.clone(), s)));
            match (fan, self.settings.get(&eid).cloned()) {
                (Some((f, s)), Some(setting @ FanSetting::Curve { .. })) => {
                    let (new, close) = big_editor(ui.ctx(), &f, &setting, s, can);
                    if let Some(n) = new {
                        actions.push((eid.clone(), floor_for(f.pump, f.min), CardAction::Set(n)));
                    }
                    if close {
                        self.editor = None;
                    }
                }
                // the fan is gone, left curve mode, or the data is stale
                _ => self.editor = None,
            }
        }
        for (id, floor, a) in actions {
            match a {
                CardAction::OpenEditor => self.editor = Some(id),
                // a fan the user tunes gets its own setting: switching profiles leaves it alone
                CardAction::Set(s) => {
                    let s = match s {
                        FanSetting::Manual { percent } => FanSetting::Manual { percent: percent.max(floor) },
                        FanSetting::Curve { source, points } => FanSetting::Curve { source, points: points.into_iter().map(|p| [p[0], p[1].clamp(floor, 100.0)]).collect() },
                        other => other,
                    };
                    self.pin(&id, &s);
                    self.settings.insert(id, s);
                }
                CardAction::FollowProfile => self.unpin(&id),
                CardAction::Pump(p) => {
                    if p {
                        match self.settings.get_mut(&id) {
                            Some(FanSetting::Manual { percent }) => *percent = percent.max(PUMP_FLOOR),
                            Some(FanSetting::Curve { points, .. }) => points.iter_mut().for_each(|pt| pt[1] = pt[1].max(PUMP_FLOOR)),
                            _ => {}
                        }
                    }
                    if let Some(s) = self.store.pinned.get_mut(&id) {
                        if let (true, FanSetting::Manual { percent }) = (p, &mut *s) {
                            *percent = percent.max(PUMP_FLOOR);
                        }
                        if let (true, FanSetting::Curve { points, .. }) = (p, &mut *s) {
                            points.iter_mut().for_each(|pt| pt[1] = pt[1].max(PUMP_FLOOR));
                        }
                    }
                    // a fact about the header, the same for every profile: saved with the store
                    self.store.pumps.insert(id, p);
                    self.pin_dirty = Some(Instant::now());
                }
            }
        }
        self.sync();
    }
}

/// Read-only view of the tab state the page needs.
struct View<'a> {
    settings: &'a HashMap<String, FanSetting>,
    /// fans with their own setting (profiles leave them alone)
    pinned: &'a BTreeMap<String, FanSetting>,
    can: bool,
    afterburner: bool,
    profile: Option<&'a str>,
    /// fans the helper reports back on the BIOS although PC Tuner asked to drive them
    handed_back: &'a HashSet<String>,
    /// "Edit points" starts unfolded (demo screenshots only)
    points_open: bool,
}

/// A hero-row stat that is left out when the window is too narrow for it (instead of wrapping letter by letter).
fn stat_fit(ui: &mut egui::Ui, gap: f32, value: &str, label: &str, c: egui::Color32) {
    if ui.available_width() < gap + 7.0 * label.chars().count().max(value.chars().count() * 2) as f32 {
        return;
    }
    ui.add_space(gap);
    theme::stat(ui, value, label, c);
}

/// Banners plus the hero card: the hottest of CPU and GPU, with live graphs.
fn hero_card(ui: &mut egui::Ui, s: &Status, cpu_hist: &VecDeque<f32>, gpu_hist: &VecDeque<f32>, avg: &Avg) {
    if s.emergency {
        theme::banner(ui, theme::LOCKED, "Over 90 °C - every controlled fan is at 100 % until the CPU and GPU are back under 80 °C.");
    }
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        let hottest = s.cpu.into_iter().chain(s.gpu).fold(f32::NAN, f32::max);
        ui.horizontal(|ui| {
            if hottest.is_nan() {
                theme::hero(ui, "-", "no temperature", "CPU and GPU temperatures could not be read", theme::TEXT_DIM);
            } else {
                let v = theme::count_up(ui.ctx(), "fan-hot", hottest);
                let which = if s.gpu.unwrap_or(f32::MIN) >= s.cpu.unwrap_or(f32::MIN) { "GPU" } else { "CPU" };
                theme::hero(ui, &format!("{v:.0} °C"), "hottest", &format!("{which} is the warmest right now - 90 °C sends every fan to 100 %"), temp_color(hottest));
            }
            ui.add_space(28.0);
            let t = |v: Option<f32>| v.map(|x| format!("{x:.0} °C")).unwrap_or_else(|| "-".into());
            theme::stat(ui, &t(s.cpu), "CPU", s.cpu.map(temp_color).unwrap_or(theme::TEXT_DIM));
            ui.add_space(22.0);
            theme::stat(ui, &t(s.gpu), "GPU", s.gpu.map(temp_color).unwrap_or(theme::TEXT_DIM));
            // session averages: since PC Tuner started
            stat_fit(ui, 22.0, &t(avg.cpu), "CPU avg", avg.cpu.map(temp_color).unwrap_or(theme::TEXT_DIM));
            stat_fit(ui, 22.0, &t(avg.gpu), "GPU avg", avg.gpu.map(temp_color).unwrap_or(theme::TEXT_DIM));
            stat_fit(ui, 22.0, &s.fans.len().to_string(), "fans found", theme::TEXT);
            let ctl = s.fans.iter().filter(|f| f.controllable).count();
            stat_fit(ui, 22.0, &ctl.to_string(), "can be controlled", theme::TEXT);
        });
        if let Some(over) = avg.over {
            ui.label(RichText::new(format!("Averages since PC Tuner started · {}", span_text(over))).size(11.5).color(theme::TEXT_FAINT));
        }
        ui.add_space(6.0);
        // two graphs side by side, with room for the gap between them
        let w = ((ui.available_width() - 2.0 * ui.spacing().item_spacing.x - 16.0) / 2.0).max(120.0);
        ui.horizontal(|ui| {
            for (label, h) in [("CPU temperature", cpu_hist), ("GPU temperature", gpu_hist)] {
                ui.vertical(|ui| {
                    ui.label(RichText::new(label).size(12.0).color(theme::TEXT_FAINT));
                    let v: Vec<f32> = h.iter().cloned().collect();
                    theme::sparkline(ui, &v, egui::vec2(w, 34.0), v.last().map(|t| temp_color(*t)).unwrap_or(theme::ACCENT));
                });
                ui.add_space(8.0);
            }
        });
    });
    ui.add_space(6.0);
}

fn fans_and_temps(ui: &mut egui::Ui, s: &Status, rpm_hist: &HashMap<String, VecDeque<f32>>, view: &View, actions: &mut Vec<(String, f32, CardAction)>) {
    // ---- one card per fan, three to a row
    theme::section(ui, "Fans");
    if s.fans.is_empty() {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label("No fans were found.");
        });
    }
    // headers that have never spun (nothing plugged in) go into a folded group at the end
    let (shown, empty): (Vec<&FanInfo>, Vec<&FanInfo>) = s.fans.iter().partition(|f| {
        f.gpu
            || view.settings.get(&f.id).map(|x| *x != FanSetting::Bios).unwrap_or(false)
            || rpm_hist.get(&f.id).map(|h| h.iter().any(|v| *v > 0.0)).unwrap_or(false)
            || f.rpm.unwrap_or(0.0) > 0.0
    });
    let mut cards = |ui: &mut egui::Ui, list: &[&FanInfo]| {
        for row in list.chunks(3) {
            ui.columns(3, |cols| {
                for (i, f) in row.iter().enumerate() {
                    let setting = view.settings.get(&f.id).cloned().unwrap_or_default();
                    if let Some(a) = fan_card(&mut cols[i], f, rpm_hist.get(&f.id), &setting, view, s) {
                        actions.push((f.id.clone(), floor_for(f.pump, f.min), a));
                    }
                }
            });
            ui.add_space(4.0);
        }
    };
    cards(ui, &shown);
    if !empty.is_empty() {
        let names: Vec<&str> = empty.iter().map(|f| f.name.as_str()).collect();
        egui::CollapsingHeader::new(RichText::new(format!("{} headers at 0 RPM - probably nothing plugged in", empty.len())).color(theme::TEXT_DIM))
            .id_salt("fan-empty")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new(names.join(", ")).size(12.0).color(theme::TEXT_FAINT));
                ui.add_space(4.0);
                cards(ui, &empty);
            });
    }
    ui.add_space(6.0);

    // ---- all temperatures
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, "Temperatures");
        egui::Grid::new("fan-temps").num_columns(3).spacing([24.0, 6.0]).show(ui, |ui| {
            for t in &s.temps {
                let v = t.value.unwrap_or(0.0);
                ui.label(RichText::new(&t.hw).color(theme::TEXT_DIM));
                ui.label(&t.name);
                ui.label(RichText::new(format!("{v:.0} °C")).strong().color(temp_color(v)));
                ui.end_row();
            }
        });
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new("Sensors: LibreHardwareMonitor with the PawnIO driver, updated every second. Fans never go below 20 % (pumps 50 %); at 90 °C every controlled fan goes to 100 %.")
            .size(12.0)
            .color(theme::TEXT_FAINT),
    );
}

fn fan_card(ui: &mut egui::Ui, f: &FanInfo, hist: Option<&VecDeque<f32>>, setting: &FanSetting, view: &View, s: &Status) -> Option<CardAction> {
    let emergency = s.emergency;
    let mut action = None;
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        // the column layout is justified, which would stretch wrapped text
        ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&f.name).size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if f.controllable {
                    let (text, c) = fan_chip(f, setting, view.can, emergency, s.blind, view.handed_back.contains(&f.id));
                    theme::chip(ui, text, c);
                } else {
                    theme::chip(ui, "read-only", theme::DIM);
                }
                if f.pump {
                    theme::chip(ui, "pump", theme::OPTIONAL);
                }
                if f.controllable && view.pinned.contains_key(&f.id) {
                    theme::chip(ui, "own setting", theme::GAMING);
                }
            });
        });
        ui.label(RichText::new(&f.hw).size(12.0).color(theme::TEXT_FAINT));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let rpm = f.rpm.map(|r| format!("{r:.0}")).unwrap_or_else(|| "-".into());
            ui.label(RichText::new(rpm).size(28.0).strong().color(theme::TEXT));
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("RPM").strong());
            });
            if let Some(p) = f.percent {
                ui.add_space(14.0);
                theme::stat(ui, &format!("{p:.0} %"), "duty", theme::TEXT);
            }
        });
        let v: Vec<f32> = hist.map(|h| h.iter().cloned().collect()).unwrap_or_default();
        theme::sparkline(ui, &v, egui::vec2(ui.available_width(), 30.0), theme::ACCENT);
        if !f.controllable {
            ui.label(RichText::new("Shown only - this header cannot be controlled by the library on this board.").size(11.5).color(theme::TEXT_FAINT));
            return;
        }

        // ---- control
        ui.add_space(6.0);
        let floor = floor_for(f.pump, f.min);
        if !view.can && *setting != FanSetting::Bios {
            ui.label(RichText::new("Saved setting below - not applied (view-only).").size(11.5).color(theme::GAMING));
        } else if view.can && *setting != FanSetting::Bios && view.handed_back.contains(&f.id) {
            ui.label(RichText::new("The helper did not take this setting, so the BIOS drives this fan. PC Tuner tries again every few seconds.").size(11.5).color(theme::GAMING));
        }
        // own setting vs following the profile
        // plain labels wrap inside the card (a horizontal row would widen it past its column)
        ui.add_enabled_ui(view.can, |ui| {
            if view.pinned.contains_key(&f.id) {
                ui.label(RichText::new("Own setting - profiles leave this fan alone.").size(11.5).color(theme::GAMING));
                let tip = match view.profile {
                    Some(p) => format!("Use {p}'s setting for this fan again."),
                    None => "Let the next profile you pick set this fan.".to_string(),
                };
                if ui.small_button("Follow profile").on_hover_text(tip).clicked() {
                    action = Some(CardAction::FollowProfile);
                }
            } else {
                let who = view.profile.map(|p| format!("Follows {p}.")).unwrap_or_else(|| "Follows the profile.".into());
                ui.label(RichText::new(format!("{who} Change it here to give this fan its own setting.")).size(11.5).color(theme::TEXT_FAINT));
            }
        });
        ui.add_enabled_ui(view.can, |ui| {
            ui.horizontal(|ui| {
                let bios = *setting == FanSetting::Bios;
                if ui.selectable_label(bios, "  BIOS  ").on_hover_text("Hands off: the BIOS (or graphics driver) curve decides.").clicked() && !bios {
                    action = Some(CardAction::Set(FanSetting::Bios));
                }
                let manual = matches!(setting, FanSetting::Manual { .. });
                if ui.selectable_label(manual, "  Manual  ").on_hover_text("A fixed speed you choose with the slider.").clicked() && !manual {
                    let start = f.percent.unwrap_or(50.0).max(floor);
                    action = Some(CardAction::Set(FanSetting::Manual { percent: start.round() }));
                }
                let curve = matches!(setting, FanSetting::Curve { .. });
                if ui.selectable_label(curve, "  Curve  ").on_hover_text("Speed follows a temperature: drag the points on the graph.").clicked() && !curve {
                    action = Some(CardAction::Set(default_curve(&f.name, f.gpu, f.pump, floor)));
                }
            });
            if let FanSetting::Curve { source, points } = setting {
                let mut src = *source;
                egui::ComboBox::from_id_salt(("fan-src", &f.id)).selected_text(src.label()).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    for s in [Source::Cpu, Source::Gpu, Source::Hottest] {
                        ui.selectable_value(&mut src, s, s.label());
                    }
                });
                let mut pts = points.clone();
                let live = src.read(s.cpu, s.gpu);
                let mut changed = curve_editor(ui, &f.id, &mut pts, floor, live, 150.0);
                // exact values: type the temperature and fan % of each point (folded to keep the card small)
                egui::CollapsingHeader::new(RichText::new(format!("Edit points ({})", pts.len())).size(12.5))
                    .id_salt(("fan-points-open", &f.id))
                    .default_open(view.points_open)
                    .show(ui, |ui| {
                        changed |= point_list(ui, &f.id, &mut pts, floor);
                    });
                if ui.small_button("Open large editor").on_hover_text("A big graph with the point list beside it, for fine tuning.").clicked() {
                    action = Some(CardAction::OpenEditor);
                }
                if changed || src != *source {
                    action = Some(CardAction::Set(FanSetting::Curve { source: src, points: pts }));
                }
            }
            if let FanSetting::Manual { percent } = setting {
                let mut v = percent.max(floor);
                // leave room for the number box beside the slider so the card keeps its column width
                ui.spacing_mut().slider_width = (ui.available_width() - 96.0).max(60.0);
                let r = ui.add(egui::Slider::new(&mut v, floor..=100.0).step_by(1.0).fixed_decimals(0).suffix(" %"));
                if r.changed() {
                    action = Some(CardAction::Set(FanSetting::Manual { percent: v }));
                }
                ui.label(RichText::new(format!("Lowest allowed: {floor:.0} %")).size(11.5).color(theme::TEXT_FAINT));
            }
            if !f.gpu {
                let mut pump = f.pump;
                if ui.checkbox(&mut pump, "This is a pump (never below 50 %)").changed() {
                    action = Some(CardAction::Pump(pump));
                }
            }
        })
        .response
        .on_disabled_hover_text("Run PC Tuner as administrator to control fans.");
        if f.gpu && *setting != FanSetting::Bios && view.afterburner {
            ui.label(
                RichText::new("MSI Afterburner is running. Turn off its fan control (Settings > Fan > untick \"Enable user defined software automatic fan control\") so the two don't fight.")
                    .size(11.5)
                    .color(theme::GAMING),
            );
        }
        });
    });
    action
}

const GRAPH_T: (f32, f32) = (20.0, 100.0);

/// Draggable fan curve. Returns true when the curve changed.
/// x = temperature 20-100 °C, y = duty 0-100 %. The shaded band is below the fan's floor; the red line is the 90 °C override.
/// Drag a point to move it, double-click empty graph to add one, right-click a point to remove it.
/// `height`: 150 px in a fan card, larger in the big editor.
fn curve_editor(ui: &mut egui::Ui, id: &str, points: &mut Vec<[f32; 2]>, floor: f32, live: Option<f32>, height: f32) -> bool {
    let size = egui::vec2(ui.available_width(), height);
    // the background takes double-clicks (add a point); the points are interacted with on top of it
    let (rect, bg) = ui.allocate_exact_size(size, egui::Sense::click());
    let plot = rect.shrink2(egui::vec2(6.0, 8.0));
    let to_screen = |t: f32, d: f32| {
        egui::pos2(
            plot.left() + plot.width() * (t - GRAPH_T.0) / (GRAPH_T.1 - GRAPH_T.0),
            plot.bottom() - plot.height() * d / 100.0,
        )
    };
    let from_screen = |p: egui::Pos2| {
        let t = GRAPH_T.0 + (p.x - plot.left()) / plot.width() * (GRAPH_T.1 - GRAPH_T.0);
        let d = (plot.bottom() - p.y) / plot.height() * 100.0;
        (t, d)
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, theme::tint(egui::Color32::WHITE, 6));
    // grid: every 10 °C and every 25 %
    for t in (30..=90).step_by(10) {
        let x = to_screen(t as f32, 0.0).x;
        painter.line_segment([egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())], egui::Stroke::new(1.0_f32, theme::tint(egui::Color32::WHITE, 10)));
    }
    for d in [25.0_f32, 50.0, 75.0] {
        let y = to_screen(0.0, d).y;
        painter.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], egui::Stroke::new(1.0_f32, theme::tint(egui::Color32::WHITE, 10)));
    }
    // below the floor: not allowed
    let fy = to_screen(0.0, floor).y;
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(plot.left(), fy), plot.right_bottom()), 0.0, theme::tint(theme::LOCKED, 9));
    painter.line_segment([egui::pos2(plot.left(), fy), egui::pos2(plot.right(), fy)], egui::Stroke::new(1.0_f32, theme::tint(theme::LOCKED, 90)));
    // 90 °C: every fan to 100 %
    let x90 = to_screen(90.0, 0.0).x;
    // under the floor line, ending left of the 90° line (the curve is always above the floor)
    painter.text(egui::pos2(x90.min(plot.right()) - 4.0, fy + 2.0), egui::Align2::RIGHT_TOP, format!("min {floor:.0} %"), egui::FontId::proportional(10.5), theme::tint(theme::LOCKED, 200));
    painter.line_segment([egui::pos2(x90, plot.top()), egui::pos2(x90, plot.bottom())], egui::Stroke::new(1.0_f32, theme::tint(theme::LOCKED, 160)));
    painter.text(egui::pos2(x90 + 3.0, plot.top()), egui::Align2::LEFT_TOP, "90°", egui::FontId::proportional(10.5), theme::tint(theme::LOCKED, 200));
    painter.text(plot.left_bottom() + egui::vec2(2.0, -2.0), egui::Align2::LEFT_BOTTOM, "20 °C", egui::FontId::proportional(10.5), theme::TEXT_FAINT);
    painter.text(plot.left_top() + egui::vec2(2.0, 0.0), egui::Align2::LEFT_TOP, "100 %", egui::FontId::proportional(10.5), theme::TEXT_FAINT);
    // a tall graph (the large editor) has room to label every grid line
    if height >= 250.0 {
        // from 40: the "20 °C" corner label already sits where 30 would go
        for t in (40..=80).step_by(10) {
            let x = to_screen(t as f32, 0.0).x;
            painter.text(egui::pos2(x, plot.bottom() - 2.0), egui::Align2::CENTER_BOTTOM, format!("{t} °C"), egui::FontId::proportional(10.5), theme::TEXT_FAINT);
        }
        for d in [25, 50, 75] {
            let y = to_screen(0.0, d as f32).y;
            painter.text(egui::pos2(plot.left() + 2.0, y - 1.0), egui::Align2::LEFT_BOTTOM, format!("{d} %"), egui::FontId::proportional(10.5), theme::TEXT_FAINT);
        }
    }

    // drag the points (temperatures stay in order, 2 °C apart); right-click removes one
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let mut over_point: Option<usize> = None;
    for i in 0..points.len() {
        let p = to_screen(points[i][0], points[i][1]);
        let hit = egui::Rect::from_center_size(p, egui::vec2(18.0, 18.0));
        let resp = ui.interact(hit, ui.id().with(("fan-pt", id, i)), egui::Sense::click_and_drag());
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (t, d) = from_screen(pos);
                changed |= set_point_temp(points, i, t);
                changed |= set_point_duty(points, i, d, floor);
            }
        }
        if resp.secondary_clicked() {
            remove = Some(i);
        }
        if resp.hovered() || resp.dragged() {
            over_point = Some(i);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
    if let Some(i) = remove {
        changed |= remove_point(points, i);
    }
    // double-click on the graph (not on a point): a new point there
    if over_point.is_none() && bg.double_clicked() {
        if let Some(pos) = bg.interact_pointer_pos() {
            let (t, d) = from_screen(pos);
            changed |= insert_at(points, t, d, floor);
        }
    }
    if over_point.is_none() {
        bg.on_hover_text_at_pointer("Drag a point to move it · double-click to add one · right-click a point to remove it");
    }

    // the curve, flat beyond the first and last points
    let mut line = vec![to_screen(GRAPH_T.0, points.first().map(|p| p[1]).unwrap_or(100.0).max(floor))];
    line.extend(points.iter().map(|p| to_screen(p[0], p[1].max(floor))));
    line.push(to_screen(GRAPH_T.1, points.last().map(|p| p[1]).unwrap_or(100.0).max(floor)));
    painter.add(egui::Shape::line(line, egui::Stroke::new(2.0_f32, theme::ACCENT)));
    for (i, p) in points.iter().enumerate() {
        let c = to_screen(p[0], p[1].max(floor));
        let hot = ui.ctx().is_being_dragged(ui.id().with(("fan-pt", id, i))) || over_point == Some(i);
        painter.circle(c, if hot { 7.0 } else { 5.5 }, theme::BG, egui::Stroke::new(2.0_f32, theme::ACCENT));
        if hot {
            painter.text(c + egui::vec2(0.0, -12.0), egui::Align2::CENTER_BOTTOM, format!("{:.0} °C → {:.0} %", p[0], p[1]), egui::FontId::proportional(11.5), theme::TEXT);
        }
    }

    // where the fan is right now
    if let Some(t) = live {
        let duty = curve_duty(points, t.clamp(GRAPH_T.0, GRAPH_T.1)).max(floor);
        let c = to_screen(t.clamp(GRAPH_T.0, GRAPH_T.1), duty);
        painter.circle_filled(c, 5.0, theme::GAMING);
        // pinned to the top-left corner (under "100 %"), where curves rarely go, on a backdrop so grid
        // lines and points never run through it
        let label = format!("now {t:.0} °C → {duty:.0} %");
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.5), theme::GAMING);
        let pos = plot.left_top() + egui::vec2(2.0, 15.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(3.0, 1.0));
        painter.rect_filled(bg, 3.0, egui::Color32::from_rgba_unmultiplied(theme::BG.r(), theme::BG.g(), theme::BG.b(), 200));
        painter.galley(pos, galley, theme::GAMING);
    }
    changed
}

/// Every point of a curve as typed values: temperature and fan %, a Remove button, and "Add point".
/// Click a value to type it, or drag it. Temperatures stay between their neighbours (2 °C apart), fan %
/// never goes under the fan's floor. Returns true when the curve changed.
fn point_list(ui: &mut egui::Ui, id: &str, points: &mut Vec<[f32; 2]>, floor: f32) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let can_remove = points.len() > MIN_POINTS;
    egui::Grid::new(("fan-points", id)).num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new("#").size(11.5).color(theme::TEXT_FAINT));
        ui.label(RichText::new("Temperature").size(11.5).color(theme::TEXT_FAINT));
        ui.label(RichText::new("Fan").size(11.5).color(theme::TEXT_FAINT));
        ui.label("");
        ui.end_row();
        for i in 0..points.len() {
            ui.label(RichText::new(format!("{}", i + 1)).color(theme::TEXT_DIM));
            let (lo, hi) = temp_range(points, i);
            let mut t = points[i][0];
            let r = ui
                .add(egui::DragValue::new(&mut t).range(lo..=hi).speed(0.2).fixed_decimals(0).suffix(" °C"))
                .on_hover_text(format!("Click to type. {lo:.0}-{hi:.0} °C (stays between its neighbours)."));
            if r.changed() {
                changed |= set_point_temp(points, i, t);
            }
            let mut d = points[i][1];
            let r = ui
                .add(egui::DragValue::new(&mut d).range(floor..=100.0).speed(0.3).fixed_decimals(0).suffix(" %"))
                .on_hover_text(format!("Click to type. {floor:.0}-100 % (never under this fan's floor)."));
            if r.changed() {
                changed |= set_point_duty(points, i, d, floor);
            }
            if ui.add_enabled(can_remove, egui::Button::new("Remove").small()).on_hover_text("Remove this point").on_disabled_hover_text("A curve needs at least 2 points.").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        changed |= remove_point(points, i);
    }
    let full = points.len() >= MAX_POINTS;
    let tip = if full { format!("A curve can have at most {MAX_POINTS} points.") } else { "Adds a point where the curve has most room, on the curve as it is now.".to_string() };
    if ui.add_enabled(!full, egui::Button::new("+ Add point").small()).on_hover_text(&tip).on_disabled_hover_text(&tip).clicked() {
        changed |= add_point(points, floor);
    }
    changed
}

/// The large curve editor (a modal over the Fan Tuner page) for one fan. Returns the new setting when the user
/// changed it, and whether the window should close.
fn big_editor(ctx: &egui::Context, f: &FanInfo, setting: &FanSetting, s: &Status, can: bool) -> (Option<FanSetting>, bool) {
    let FanSetting::Curve { source, points } = setting else { return (None, true) };
    let floor = floor_for(f.pump, f.min);
    let mut out = None;
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    theme::dim_backdrop(ctx, "fan-editor");
    egui::Window::new("fan-curve-editor")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme::modal_frame(ctx))
        .show(ctx, |ui| {
            const W: f32 = 860.0;
            ui.set_width(W);
            theme::section(ui, "FAN CURVE");
            ui.horizontal(|ui| {
                ui.label(RichText::new(&f.name).size(22.0).strong());
                ui.label(RichText::new(&f.hw).color(theme::TEXT_DIM));
            });
            ui.add_space(4.0);
            ui.add_enabled_ui(can, |ui| {
                let mut src = *source;
                let mut pts = points.clone();
                ui.horizontal(|ui| {
                    ui.label("Follows");
                    egui::ComboBox::from_id_salt(("fan-src-big", &f.id)).selected_text(src.label()).width(220.0).show_ui(ui, |ui| {
                        for x in [Source::Cpu, Source::Gpu, Source::Hottest] {
                            ui.selectable_value(&mut src, x, x.label());
                        }
                    });
                    ui.label(RichText::new(format!("{} points · lowest allowed {floor:.0} %", pts.len())).color(theme::TEXT_FAINT));
                });
                ui.add_space(6.0);
                let live = src.read(s.cpu, s.gpu);
                let mut changed = false;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(W - 280.0);
                        changed |= curve_editor(ui, &format!("{}-big", f.id), &mut pts, floor, live, 340.0);
                        ui.label(RichText::new("Drag a point to move it · double-click the graph to add one · right-click a point to remove it.").size(11.5).color(theme::TEXT_FAINT));
                    });
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().id_salt(("fan-points-big", &f.id)).max_height(360.0).show(ui, |ui| {
                            changed |= point_list(ui, &format!("{}-big", f.id), &mut pts, floor);
                        });
                    });
                });
                if changed || src != *source {
                    out = Some(FanSetting::Curve { source: src, points: pts });
                }
            })
            .response
            .on_disabled_hover_text("Run PC Tuner as administrator to control fans.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary_button("   Done   ")).clicked() {
                    close = true;
                }
                ui.label(RichText::new("Changes apply at once and give this fan its own setting.").color(theme::TEXT_DIM));
            });
        });
    (out, close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_parse_status_line() {
        let line = r#"{"cpu":61.5,"gpu":null,"emergency":false,"msg":"","fans":[{"id":"/gpu-nvidia/0/control/1","hw":"RTX","name":"GPU Fan","rpm":1158,"percent":40,"controllable":true,"mode":"default","pump":false,"gpu":true,"requested":null}],"temps":[{"id":"t","hw":"RTX","kind":"GpuNvidia","name":"GPU Core","value":24.8}]}"#;
        let s = parse_status(line).expect("parses");
        assert_eq!(s.cpu, Some(61.5));
        assert_eq!(s.gpu, None);
        assert_eq!(s.fans.len(), 1);
        assert!(s.fans[0].controllable && s.fans[0].gpu);
        assert_eq!(s.fans[0].rpm, Some(1158.0));
        assert_eq!(s.temps[0].value, Some(24.8));
        assert!(parse_status("fans returned to BIOS: exit").is_none());
        assert!(parse_status("{broken").is_none());
    }

    #[test]
    fn fan_history_is_bounded() {
        let mut sh = Shared::default();
        for i in 0..(HISTORY + 15) {
            sh.take_status(demo_status(i as f64));
        }
        assert_eq!(sh.cpu.len(), HISTORY);
        assert!(sh.rpm.values().all(|h| h.len() == HISTORY));
    }

    #[test]
    fn fan_floors_and_commands() {
        assert_eq!(floor_for(false, None), 20.0);
        assert_eq!(floor_for(true, None), 50.0);
        assert_eq!(floor_for(false, Some(30.0)), 30.0, "driver minimum wins when higher");
        assert_eq!(floor_for(true, Some(30.0)), 50.0);
        assert_eq!(command_for("/lpc/x/control/0", &FanSetting::Bios), "auto /lpc/x/control/0");
        assert_eq!(command_for("/gpu/0/control/1", &FanSetting::Manual { percent: 55.4 }), "set /gpu/0/control/1 55");
        assert_eq!(command_for("id", &FanSetting::Manual { percent: 140.0 }), "set id 100");
        let json = serde_json::to_string(&FanSetting::Manual { percent: 40.0 }).unwrap();
        assert_eq!(json, r#"{"mode":"manual","percent":40.0}"#);
        assert_eq!(serde_json::from_str::<FanSetting>(r#"{"mode":"bios"}"#).unwrap(), FanSetting::Bios);
    }

    #[test]
    fn fan_curves() {
        let pts = vec![[30.0, 25.0], [50.0, 40.0], [70.0, 70.0], [85.0, 100.0]];
        assert_eq!(curve_duty(&pts, 20.0), 25.0);
        assert_eq!(curve_duty(&pts, 40.0), 32.5);
        assert_eq!(curve_duty(&pts, 70.0), 70.0);
        assert_eq!(curve_duty(&pts, 95.0), 100.0);
        assert_eq!(curve_duty(&[], 50.0), 100.0, "an empty curve fails safe");
        let cmd = command_for("/lpc/x/control/0", &FanSetting::Curve { source: Source::Cpu, points: pts.clone() });
        assert_eq!(cmd, "curve /lpc/x/control/0 cpu 30:25,50:40,70:70,85:100");
        // default curves respect the floor and pick a sensible temperature
        let FanSetting::Curve { source, points } = default_curve("Pump Fan", false, true, 50.0) else { panic!() };
        assert_eq!(source, Source::Cpu);
        assert!(points.iter().all(|p| p[1] >= 50.0));
        assert!(matches!(default_curve("GPU Fan", true, false, 30.0), FanSetting::Curve { source: Source::Gpu, .. }));
        assert!(matches!(default_curve("System Fan #1", false, false, 20.0), FanSetting::Curve { source: Source::Hottest, .. }));
        // sources fall back to whichever temperature is known
        assert_eq!(Source::Hottest.read(Some(60.0), Some(70.0)), Some(70.0));
        assert_eq!(Source::Cpu.read(None, Some(55.0)), Some(55.0));
        assert_eq!(Source::Gpu.read(None, None), None);
        let json = serde_json::to_string(&FanSetting::Curve { source: Source::Hottest, points: vec![[30.0, 30.0]] }).unwrap();
        assert_eq!(json, r#"{"mode":"curve","source":"hottest","points":[[30.0,30.0]]}"#);
    }

    #[test]
    fn fan_profiles() {
        let fans = demo_status(0.0).fans;
        let pumps = BTreeMap::new();
        let silent = builtin_profile("Silent", &fans, &pumps).unwrap();
        let perf = builtin_profile("Performance", &fans, &pumps).unwrap();
        assert!(builtin_profile("Nope", &fans, &pumps).is_none());
        // only controllable fans are listed; Silent leaves the GPU on its own curve
        assert_eq!(perf.fans.len(), fans.iter().filter(|f| f.controllable).count());
        assert!(!silent.fans.contains_key("/gpu-nvidia/0/control/1"));
        assert!(perf.fans.contains_key("/gpu-nvidia/0/control/1"));
        assert!(!perf.fans.contains_key("/lpc/nct6687d/0/fan/2"), "read-only fans are never in a profile");
        // an empty header (0 RPM) stays on BIOS; a GPU stopped at idle is still included
        let mut more = fans.clone();
        more.push(FanInfo { id: "/lpc/x/control/9".into(), name: "System Fan #4".into(), rpm: Some(0.0), controllable: true, ..Default::default() });
        more.iter_mut().filter(|f| f.gpu).for_each(|f| f.rpm = Some(0.0));
        let bal = builtin_profile("Balanced", &more, &pumps).unwrap();
        assert!(!bal.fans.contains_key("/lpc/x/control/9"));
        assert!(bal.fans.contains_key("/gpu-nvidia/0/control/1"));
        // floors hold inside every built-in curve: pump >= 50, GPU >= its 30 % driver minimum
        let pump = &silent.fans["/lpc/nct6687d/0/control/1"];
        let FanSetting::Curve { points, .. } = pump else { panic!() };
        assert!(points.iter().all(|p| p[1] >= PUMP_FLOOR));
        let FanSetting::Curve { points, .. } = &perf.fans["/gpu-nvidia/0/control/1"] else { panic!() };
        assert!(points.iter().all(|p| p[1] >= 30.0));
        // BIOS entries do not count as a change
        let mut cur: HashMap<String, FanSetting> = perf.fans.clone().into_iter().collect();
        cur.insert("/x".into(), FanSetting::Bios);
        assert_eq!(normalized(&cur), perf.fans);
        // the store round-trips through JSON
        let mut store = ProfileStore { last: Some("Performance".into()), ..Default::default() };
        store.profiles.insert("Performance".into(), perf.clone());
        let back: ProfileStore = serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert_eq!(back.last.as_deref(), Some("Performance"));
        assert_eq!(back.profiles["Performance"], perf);
    }

    /// The user's case: Performance is on, the pump is set to Manual by hand, then a profile is picked.
    #[test]
    fn fan_own_settings_survive_profile_switch() {
        let fans = demo_status(0.0).fans;
        let pump = "/lpc/nct6687d/0/control/1";
        let cpu = "/lpc/nct6687d/0/control/0";
        // demo + no startup: never reads or writes fan-profiles.json and never starts the helper
        let mut t = FanTab::new_with(ProfileStore::default(), true, false);
        t.load_profile("Performance", &fans);
        assert!(matches!(t.settings[pump], FanSetting::Curve { .. }));
        // the user sets the pump to Manual 60 %
        t.pin(pump, &FanSetting::Manual { percent: 60.0 });
        t.settings.insert(pump.into(), FanSetting::Manual { percent: 60.0 });
        assert!(!t.modified(), "an own setting is not a change to the profile");
        // switching profiles keeps the pump, changes the others
        t.load_profile("Silent", &fans);
        assert_eq!(t.settings[pump], FanSetting::Manual { percent: 60.0 }, "the pump keeps its own setting");
        assert_eq!(t.settings[cpu], t.store.profiles["Silent"].fans[cpu], "other fans follow the profile");
        // saving the profile does not swallow the own setting
        let saved = t.current_profile();
        assert!(!saved.fans.contains_key(pump));
        // Follow profile hands the pump back
        t.unpin(pump);
        assert_eq!(t.settings[pump], t.store.profiles["Silent"].fans[pump]);
        assert!(t.store.pinned.is_empty());
        // and the store round-trips with own settings
        t.pin(cpu, &FanSetting::Manual { percent: 45.0 });
        let back: ProfileStore = serde_json::from_str(&serde_json::to_string(&t.store).unwrap()).unwrap();
        assert_eq!(back.pinned[cpu], FanSetting::Manual { percent: 45.0 });
        // older files without "pinned" still load
        let old: ProfileStore = serde_json::from_str(r#"{"last":"Balanced","profiles":{}}"#).unwrap();
        assert!(old.pinned.is_empty());
    }

    /// "Give all fans back to BIOS" also clears own settings, so the next profile does not take them over again.
    #[test]
    fn fan_give_back_clears_own_settings() {
        let fans = demo_status(0.0).fans;
        let cpu = "/lpc/nct6687d/0/control/0";
        let mut t = FanTab::new_with(ProfileStore::default(), true, false);
        t.load_profile("Balanced", &fans);
        t.pin(cpu, &FanSetting::Manual { percent: 45.0 });
        t.settings.insert(cpu.into(), FanSetting::Manual { percent: 45.0 });
        t.give_all_back();
        assert!(t.store.pinned.is_empty(), "own settings are cleared");
        assert!(t.settings.values().all(|s| *s == FanSetting::Bios));
        assert!(t.active.is_none() && t.store.last.is_none(), "no profile comes back at startup");
        assert!(!t.any_controlled());
        t.load_profile("Silent", &fans);
        assert_eq!(t.settings[cpu], t.store.profiles["Silent"].fans[cpu], "the CPU fan follows Silent, not the old Manual 45 %");
    }

    /// Pump choices belong to the hardware: switching profiles never drops them, old per-profile ones are merged.
    #[test]
    fn fan_pump_choices_are_global() {
        let mut store = ProfileStore::default();
        let mut a = Profile::default();
        a.pumps.insert("/x/cpu".into(), true);
        a.pumps.insert("/x/pump".into(), false);
        store.profiles.insert("A".into(), a);
        store.profiles.insert("B".into(), Profile::default());
        let mut t = FanTab::new_with(store, true, false);
        assert_eq!(t.store.pumps.get("/x/cpu"), Some(&true), "old per-profile choices are merged into the global map");
        assert_eq!(t.store.pumps.get("/x/pump"), Some(&false));
        assert!(t.store.profiles.values().all(|p| p.pumps.is_empty()));
        let (pa, pb) = (t.store.profiles["A"].clone(), t.store.profiles["B"].clone());
        t.use_profile("A", &pa);
        t.use_profile("B", &pb);
        assert_eq!(t.store.pumps.len(), 2, "a profile without pump choices keeps them");
        let json = serde_json::to_string(&t.current_profile()).unwrap();
        assert!(!json.contains("pumps"), "saved profiles no longer carry pump choices: {json}");
        let back: ProfileStore = serde_json::from_str(&serde_json::to_string(&t.store).unwrap()).unwrap();
        assert_eq!(back.pumps, t.store.pumps);
    }

    /// Typing, adding and removing curve points keeps the curve valid for the helper.
    #[test]
    fn fan_point_editing() {
        let base = || vec![[30.0_f32, 30.0], [50.0, 40.0], [65.0, 60.0], [75.0, 80.0], [85.0, 100.0]];
        // typed temperatures stay between the neighbours (2 °C apart) and are whole degrees: never a reorder
        let mut p = base();
        assert!(set_point_temp(&mut p, 1, 57.4));
        assert_eq!(p[1][0], 57.0);
        assert!(set_point_temp(&mut p, 1, 90.0));
        assert_eq!(p[1][0], 63.0, "clamped 2 °C under the next point");
        assert!(set_point_temp(&mut p, 1, 5.0));
        assert_eq!(p[1][0], 32.0, "clamped 2 °C over the previous point");
        assert!(set_point_temp(&mut p, 0, 0.0) && p[0][0] == 20.0);
        assert!(set_point_temp(&mut p, 4, 150.0) && p[4][0] == 100.0);
        assert!(!set_point_temp(&mut p, 4, 100.2), "rounds to the same value: no change");
        assert!(!set_point_temp(&mut p, 9, 50.0) && !set_point_temp(&mut p, 0, f32::NAN));
        // fan % never under the floor, never over 100
        let mut p = base();
        assert!(set_point_duty(&mut p, 0, 5.0, 30.0) == false && p[0][1] == 30.0, "30 % already at the floor");
        assert!(set_point_duty(&mut p, 1, 5.0, 35.0) && p[1][1] == 35.0);
        assert!(set_point_duty(&mut p, 1, 140.0, 35.0) && p[1][1] == 100.0);
        // add: the middle of the widest gap (30-50 here), on the curve as it is (no change in behaviour)
        let mut p = base();
        assert!(add_point(&mut p, 20.0));
        assert_eq!(p.len(), 6);
        assert_eq!(p[1], [40.0, curve_duty(&base(), 40.0)]);
        // the area past the last point counts too
        let mut p = vec![[30.0, 30.0], [40.0, 50.0]];
        assert!(add_point(&mut p, 20.0));
        assert_eq!(p[2], [50.0, 50.0], "10 °C past the last point, flat like the curve");
        // full: 16 points
        let mut p = base();
        while add_point(&mut p, 20.0) {}
        assert_eq!(p.len(), MAX_POINTS);
        assert!(!add_point(&mut p, 20.0) && !insert_at(&mut p, 99.0, 50.0, 20.0));
        assert!(p.windows(2).all(|w| w[1][0] - w[0][0] >= 2.0), "always 2 °C apart: {p:?}");
        // insert (double-click): kept in order, refused within 2 °C of a point or off the graph
        let mut p = base();
        assert!(insert_at(&mut p, 57.6, 47.0, 20.0));
        assert_eq!(p[2], [58.0, 47.0]);
        assert!(!insert_at(&mut p, 59.0, 50.0, 20.0), "1 °C from 58");
        assert!(!insert_at(&mut p, 10.0, 50.0, 20.0) && !insert_at(&mut p, 105.0, 50.0, 20.0));
        assert!(insert_at(&mut p, 95.0, 10.0, 25.0) && p.last().unwrap() == &[95.0, 25.0], "duty raised to the floor");
        // remove: never under 2 points
        let mut p = vec![[30.0, 30.0], [60.0, 60.0], [90.0, 100.0]];
        assert!(remove_point(&mut p, 1));
        assert!(!remove_point(&mut p, 0) && p.len() == MIN_POINTS);
        assert!(!remove_point(&mut p, 7));
        // whatever was edited, the helper accepts the command (at most 16 "t:d" pairs)
        let mut p = base();
        while add_point(&mut p, 20.0) {}
        let cmd = command_for("x", &FanSetting::Curve { source: Source::Cpu, points: p });
        let pairs = cmd.rsplit(' ').next().unwrap().split(',').count();
        assert_eq!(pairs, MAX_POINTS);
        // a hand-edited file with more than 16 points is cut to 16
        let many: Vec<[f32; 2]> = (0..30).map(|i| [20.0 + 2.0 * i as f32, 50.0]).collect();
        assert_eq!(normalize_points(&many).unwrap().len(), MAX_POINTS);
    }

    /// The session average adds every reading and survives a helper being replaced.
    #[test]
    fn fan_session_average() {
        let mut sh = Shared::default();
        assert_eq!(sh.avg.avg(), Avg::default());
        for (c, g) in [(Some(40.0), Some(50.0)), (Some(60.0), None), (None, Some(70.0)), (None, None)] {
            sh.take_status(Status { cpu: c, gpu: g, ..Default::default() });
        }
        let a = sh.avg.avg();
        assert_eq!((a.cpu, a.gpu), (Some(50.0), Some(60.0)));
        assert!(a.over.is_some());
        sh.clear();
        sh.take_status(Status { cpu: Some(80.0), ..Default::default() });
        assert_eq!(sh.avg.avg().cpu, Some(60.0), "kept across clear(): (40 + 60 + 80) / 3");
        assert_eq!(span_text(Duration::from_secs(35)), "35 s");
        assert_eq!(span_text(Duration::from_secs(42 * 60 + 5)), "42 min");
        assert_eq!(span_text(Duration::from_secs(3900)), "1 h 05 min");
    }

    /// Curves from fan-profiles.json are normalised the same way the helper reads them.
    #[test]
    fn fan_curve_normalisation() {
        // duplicate temperatures: stable order (the helper's ParsePoints keeps it too), pushed 2 °C apart
        let n = normalize_points(&[[28.0, 82.0], [28.0, 54.0]]).unwrap();
        assert_eq!(n, vec![[28.0, 82.0], [30.0, 54.0]]);
        assert_eq!(curve_duty(&n, 20.0), 82.0);
        assert_eq!(curve_duty(&n, 40.0), 54.0);
        // unsorted: same points and duties as the helper's selftest
        let n = normalize_points(&[[70.0, 70.0], [30.0, 25.0], [50.0, 40.0], [85.0, 100.0]]).unwrap();
        assert_eq!(n, vec![[30.0, 25.0], [50.0, 40.0], [70.0, 70.0], [85.0, 100.0]]);
        assert_eq!(curve_duty(&n, 20.0), 25.0);
        assert_eq!(curve_duty(&n, 40.0), 32.5);
        assert_eq!(curve_duty(&n, 95.0), 100.0);
        // out of range values are clamped; too few points are refused
        assert_eq!(normalize_points(&[[5.0, -10.0], [150.0, 140.0]]).unwrap(), vec![[20.0, 0.0], [100.0, 100.0]]);
        assert!(normalize_points(&[[50.0, 50.0]]).is_none());
        assert!(normalize_points(&[[100.0, 50.0], [100.0, 60.0]]).is_none());
        // a loaded store: every curve is normalised, an unusable one becomes Balanced
        let mut store: ProfileStore = serde_json::from_str(
            r#"{"profiles":{"X":{"fans":{"f":{"mode":"curve","source":"cpu","points":[[60,50]]}}}},
                "pinned":{"g":{"mode":"curve","source":"gpu","points":[[40,60],[30,30]]}}}"#,
        )
        .unwrap();
        normalize_store(&mut store);
        let FanSetting::Curve { points, .. } = &store.profiles["X"].fans["f"] else { panic!() };
        assert_eq!(points.as_slice(), preset_points("Balanced").unwrap().as_slice());
        let FanSetting::Curve { points, .. } = &store.pinned["g"] else { panic!() };
        assert_eq!(points, &vec![[30.0, 30.0], [40.0, 60.0]]);
    }

    /// The chip shows what the helper really does: view-only, blind and refused settings are not "Curve".
    #[test]
    fn fan_chips_follow_the_helper() {
        let gpu = FanInfo { id: "g".into(), controllable: true, gpu: true, mode: "default".into(), ..Default::default() };
        let curve = FanSetting::Curve { source: Source::Gpu, points: vec![[30.0, 30.0], [85.0, 100.0]] };
        assert_eq!(fan_chip(&gpu, &curve, false, false, false, false).0, "BIOS", "view-only: the saved curve is not applied");
        let mut driven = gpu.clone();
        driven.mode = "software".into();
        driven.curve = true;
        driven.requested = Some(40.0);
        assert_eq!(fan_chip(&driven, &curve, false, false, false, false).0, "Curve");
        assert_eq!(fan_chip(&gpu, &curve, true, false, false, false).0, "Curve");
        assert_eq!(fan_chip(&gpu, &curve, true, false, false, true).0, "BIOS (handed back)");
        assert_eq!(fan_chip(&gpu, &curve, true, true, true, false).0, "BIOS - temps unreadable");
        assert_eq!(fan_chip(&gpu, &curve, true, true, false, false).0, "100 % - hot");
        assert_eq!(fan_chip(&gpu, &FanSetting::Bios, true, true, false, false).0, "BIOS");
        // a setting the helper dropped (requested null, no curve, default mode) is found
        let settings: HashMap<String, FanSetting> = [("g".to_string(), curve.clone())].into_iter().collect();
        let status = |f: &FanInfo| Status { fans: vec![f.clone()], ..Default::default() };
        assert_eq!(handed_back(&settings, &status(&gpu)), vec!["g".to_string()]);
        assert!(handed_back(&settings, &status(&driven)).is_empty());
        let mut paused = gpu.clone();
        paused.requested = Some(40.0); // blind: the helper keeps the request while the BIOS has the fan
        assert!(handed_back(&settings, &status(&paused)).is_empty());
        assert!(handed_back(&HashMap::new(), &status(&gpu)).is_empty(), "a fan on BIOS is not handed back");
        // the new status fields
        let s = parse_status(r#"{"cpu":50,"gpu":40,"emergency":false,"blind":true,"msg":"x","msgSeq":3,"fans":[],"temps":[]}"#).unwrap();
        assert!(s.blind);
        assert_eq!((s.msg.as_str(), s.msg_seq), ("x", 3));
    }

    /// When the helper is gone its last numbers are stale, and they are cleared when it stops for good.
    #[test]
    fn fan_stale_data() {
        let mut sh = Shared::default();
        assert!(sh.last.is_none());
        sh.take_status(demo_status(0.0));
        assert!(sh.last.is_some() && sh.status.is_some());
        sh.clear();
        assert!(sh.status.is_none() && sh.cpu.is_empty() && sh.gpu.is_empty() && sh.rpm.is_empty() && sh.last.is_none());
        // no helper running (never started here): an old status is reported as stale
        let t = FanTab::new_with(ProfileStore::default(), false, false);
        t.shared.lock().unwrap().take_status(demo_status(0.0));
        let (s, _, _, _, stale) = t.snapshot();
        assert!(s.is_some() && stale.is_some());
    }

    /// A hung helper is only declared hung well after the helper's own watchdog, and the screen only says
    /// "BIOS control" when that is known.
    #[test]
    fn fan_hang_and_bios_claims() {
        assert!(HANG_AFTER >= HELPER_WATCHDOG * 2, "hang threshold safely above the helper's 10 s watchdog");
        assert!(HANG_AFTER > STALE_AFTER);
        assert!(DETACHED_KILL_AFTER >= Duration::from_secs(120), "a hung helper gets minutes to recover before a kill");
        assert_eq!(log_marks("fans returned to BIOS: PC Tuner closed"), (true, None));
        // "reset pass done" alone is no proof: only GPU fans count, motherboard headers stay unconfirmed
        assert_eq!(log_marks("reset pass done: 1 gpu, 3 unconfirmed"), (false, Some(3)));
        assert_eq!(log_marks("reset pass done: 2 gpu, 0 unconfirmed"), (false, Some(0)));
        assert_eq!(log_marks("reset pass done: 3 fan(s) handed back to the BIOS"), (false, Some(1)), "an old format is unconfirmed");
        assert_eq!(log_marks("fans left at 100 % (they cannot go back to the BIOS until a reboot): exit"), (false, None));
        assert_eq!(log_marks("helper did not exit in time and was stopped"), (false, None));
        assert!(gone_message(false, "").contains("BIOS control"));
        let stuck = gone_message(true, "x");
        assert!(stuck.contains("may be stuck") && stuck.contains("Reboot") && !stuck.contains("Restart PC Tuner"));
        assert!(!stuck.contains("on BIOS control") && stuck.ends_with(" x"));
        // a running (silent) helper still owns its fans: never "BIOS control", no promise of a hand-back
        let (h, d) = stale_text(false, false, 7);
        assert!(h.contains("7 s") && !h.contains("BIOS control") && !d.contains("are safe") && d.contains("may stay"));
        let (h, d) = stale_text(true, true, 0);
        assert!(h.contains("may be stuck") && d.contains("eboot") && !d.contains("Restart PC Tuner"));
        let (h, _) = stale_text(true, false, 0);
        assert!(h.contains("BIOS control"));
        for m in [STUCK_AT_START, WAITING_FOR_OLD_HELPER, RESTARTED_STUCK] {
            assert!(!m.contains("on BIOS control") && !m.contains("Restart PC Tuner"), "{m}");
        }
        // stuck fans are remembered until the PC reboots
        assert!(stuck_since_this_boot("1000000", 1000000 + SAME_BOOT_SECS));
        assert!(!stuck_since_this_boot("1000000", 1000000 + SAME_BOOT_SECS + 1));
        assert!(!stuck_since_this_boot("", 1000000) && !stuck_since_this_boot("junk", 1000000));
        let b = boot_time();
        assert!(b > 1_500_000_000 && boot_time().abs_diff(b) <= 2, "boot time is stable");
        // a fresh tab: nothing driven, nothing stuck, nothing saved (tests never write the user's files)
        let mut t = FanTab::new_with(ProfileStore::default(), false, false);
        assert!(!t.drove && !t.stuck && !t.reclaim_tried && t.detached.is_none() && !t.persist);
        t.set_stuck(true);
        assert!(t.stuck);
        t.set_stuck(false);
        assert!(!t.stuck);
    }

    /// The demo screen (README screenshots): duty agrees with the slider and with the curve's live point.
    #[test]
    fn fan_demo_numbers_agree() {
        let t = FanTab::new_with(ProfileStore::default(), true, true);
        let (s, ..) = t.snapshot();
        let s = s.unwrap();
        let cpu_fan = s.fans.iter().find(|f| f.id == "/lpc/nct6687d/0/control/0").unwrap();
        assert_eq!(cpu_fan.percent, Some(45.0), "Manual 45 % shows 45 % duty");
        for f in s.fans.iter().filter(|f| f.controllable) {
            if let Some(FanSetting::Curve { source, points }) = t.settings.get(&f.id) {
                let live = source.read(s.cpu, s.gpu).unwrap();
                let want = curve_duty(points, live.clamp(GRAPH_T.0, GRAPH_T.1)).max(floor_for(f.pump, f.min));
                assert_eq!(f.percent, Some(want), "{} duty matches its curve's live point", f.name);
            }
        }
    }

    #[test]
    fn fan_new_profile_names() {
        let mut store = ProfileStore::default();
        store.profiles.insert("Gaming".into(), Profile::default());
        assert!(new_name_check("Quiet", &store).0);
        assert!(!new_name_check("", &store).0);
        assert!(!new_name_check("Gaming", &store).0, "an existing profile is not silently replaced");
        assert!(!new_name_check("gaming", &store).0, "case does not matter");
        assert!(!new_name_check("balanced", &store).0, "built-in names are refused in any case");
        assert!(!new_name_check(&"é".repeat(33), &store).0);
        assert!(new_name_check(&"é".repeat(20), &store).0, "the limit counts characters, not bytes");
    }

    #[test]
    fn fan_temp_colours() {
        assert_eq!(temp_color(50.0), theme::ACCENT);
        assert_eq!(temp_color(75.0), theme::GAMING);
        assert_eq!(temp_color(88.0), theme::LOCKED);
    }
}
