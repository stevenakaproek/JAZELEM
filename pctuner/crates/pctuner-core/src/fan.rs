//! Fan Tuner tab - live fan speeds and temperatures, driven by the PCTunerFan helper
//! (LibreHardwareMonitorLib + PawnIO). The helper holds the safety rules (floors, 90 C override,
//! fans back to the BIOS on exit/crash); this side shows the data and sends requests.
//!
//! Logic only: the helper process, profiles, curves, safety watch and the page state (`Fan`).
//! `FanEngine` runs it on its own thread, so fans are driven with no window open.

use crate::common;
use crate::common::Tone;
use crate::fan_files::extract_helper;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};



pub const HISTORY: usize = 60;
/// The temperature range (C) of the curve graph, and of every curve point.
pub const GRAPH_T: (f32, f32) = (20.0, 100.0);

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
    /// The speed sensor this fan's RPM is read from ("" = none).
    #[serde(rename = "rpmId")]
    pub rpm_id: String,
}

/// One fan speed sensor (every one the helper sees, paired or not).
#[derive(Deserialize, Serialize, Clone, Default, Debug, PartialEq)]
#[serde(default)]
pub struct RpmInfo {
    pub id: String,
    pub hw: String,
    pub name: String,
    pub rpm: Option<f32>,
}

/// The fan test the helper is running or last ran: "calibrate" (duty sweep) or "pair" (find the speed sensor).
#[derive(Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(default)]
pub struct JobInfo {
    pub seq: u64,
    pub kind: String,
    pub id: String,
    pub step: u32,
    pub steps: u32,
    pub duty: Option<f32>,
    pub done: bool,
    pub error: String,
    pub paired: String,
    pub start: Option<f32>,
    pub stop: Option<f32>,
    pub max: Option<f32>,
    pub points: Vec<[f32; 2]>,
}

/// A fan's measured behaviour (saved per fan, a fact about the hardware).
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Calibration {
    /// (duty %, RPM) from 100 % down to 0 %
    pub points: Vec<[f32; 2]>,
    /// lowest duty that still spins on the way down (None: it never spun)
    pub stop: Option<f32>,
    /// duty it starts again at from standstill (None: it never stopped, or did not start up to 60 %)
    pub start: Option<f32>,
    pub max: Option<f32>,
    pub when: String,
}

impl Calibration {
    pub fn from_job(j: &JobInfo, when: String) -> Self {
        Calibration { points: j.points.clone(), stop: j.stop, start: j.start, max: j.max, when }
    }
    /// "Stops below 20 %, starts at 30 %, 1400 RPM max" - or that it never stops.
    pub fn summary(&self) -> String {
        let max = self.max.map(|m| format!("{m:.0} RPM at 100 %")).unwrap_or_else(|| "no RPM reading".into());
        let never_stops = self.points.iter().any(|p| p[0] == 0.0 && p[1] > 0.0);
        match (self.stop, self.start) {
            _ if never_stops => format!("{max}, never stops (still spins at 0 %)"),
            (Some(s), Some(st)) => format!("{max}, stops below {s:.0} %, starts again at {st:.0} %"),
            (Some(s), None) => format!("{max}, stops below {s:.0} %, did not start again up to 60 %"),
            _ => max,
        }
    }
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
    /// Every fan speed sensor (for pairing).
    pub rpms: Vec<RpmInfo>,
    pub job: Option<JobInfo>,
}

// ------------------------------------------------------------------ what each fan should do

/// No speed floor of PC Tuner's own: fans and pumps go as low as the user sets them (the helper's 90 °C
/// override still sends every controlled fan to 100 %). Only the controller's own minimum applies.
pub const FAN_FLOOR: f32 = 0.0;
pub const PUMP_FLOOR: f32 = 0.0;

/// How one fan is driven. `Bios` = hands off (the BIOS / graphics driver decides).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum FanSetting {
    #[default]
    Bios,
    Manual { percent: f32 },
    /// `points` are [temperature °C, duty %], sorted by temperature.
    Curve { source: Source, points: Vec<[f32; 2]> },
    /// Two steady speeds: `idle` % until the temperature reaches `load_temp`, then `load` % until it
    /// falls back to `idle_temp` (the gap is the hysteresis). Made for gaming: no revving on short spikes.
    Trigger { source: Source, idle_temp: f32, load_temp: f32, idle: f32, load: f32 },
}

/// Trigger values as the editor and the helper accept them: whole numbers, temperatures 20-100 °C with the
/// load temperature at least 2 °C above the idle one, speeds between the fan's floor and 100 %.
pub fn fix_trigger(idle_temp: f32, load_temp: f32, idle: f32, load: f32, floor: f32) -> (f32, f32, f32, f32) {
    let num = |v: f32, fallback: f32| if v.is_finite() { v.round() } else { fallback };
    let it = num(idle_temp, 55.0).clamp(GRAPH_T.0, GRAPH_T.1 - 2.0);
    let lt = num(load_temp, 65.0).clamp(it + 2.0, GRAPH_T.1);
    let f = floor.clamp(0.0, 100.0);
    (it, lt, num(idle, 30.0).clamp(f, 100.0), num(load, 70.0).clamp(f, 100.0))
}

/// Starting trigger for a fan, picked for gaming on a Ryzen 7 9800X3D + RTX 5080:
/// - CPU fan / pump (CPU): idle until 55 °C, gaming from 65 °C, 30 % -> 65 %
/// - graphics card fans (GPU): 50 °C / 60 °C, 30 % -> 60 % (30 % is the RTX driver minimum)
/// - case fans (hotter of the two; the GPU heats the case most when gaming): 55 °C / 65 °C, 30 % -> 70 %
pub fn default_trigger(name: &str, gpu: bool, pump: bool, floor: f32) -> FanSetting {
    let FanSetting::Curve { source, .. } = default_curve(name, gpu, pump, floor) else { unreachable!() };
    let (it, lt, idle, load) = match source {
        Source::Cpu => (55.0, 65.0, 30.0, 65.0),
        Source::Gpu => (50.0, 60.0, 30.0, 60.0),
        Source::Hottest => (55.0, 65.0, 30.0, 70.0),
    };
    let (idle_temp, load_temp, idle, load) = fix_trigger(it, lt, idle, load, floor);
    FanSetting::Trigger { source, idle_temp, load_temp, idle, load }
}

/// Speed a trigger runs at: `loaded` is whether it was in its gaming state before this temperature.
pub fn trigger_loaded(loaded: bool, temp: f32, idle_temp: f32, load_temp: f32) -> bool {
    if temp >= load_temp {
        true
    } else if temp <= idle_temp {
        false
    } else {
        loaded
    }
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

/// Lowest duty PC Tuner allows for a fan: the driver's / controller's own minimum (0 % when it has none).
pub fn floor_for(pump: bool, driver_min: Option<f32>) -> f32 {
    let safety = if pump { PUMP_FLOOR } else { FAN_FLOOR };
    safety.max(driver_min.unwrap_or(0.0)).min(100.0)
}

// ------------------------------------------------------------------ profiles

pub const BUILTIN: [&str; 3] = ["Silent", "Balanced", "Performance"];
pub const PROFILES_FILE: &str = "fan-profiles.json";

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
    /// Speed sensor chosen for a fan control (control id -> sensor id), when not the built-in pairing.
    pub pairs: BTreeMap<String, String>,
    /// Calibration results per fan control.
    pub calib: BTreeMap<String, Calibration>,
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
        if t <= GRAPH_T.1 {
            out.push([t, p[1]]);
        }
    }
    (out.len() >= 2).then_some(out)
}

/// A curve keeps at least this many points...
pub const MIN_POINTS: usize = 2;
/// ...and at most this many (the helper refuses longer curves).
pub const MAX_POINTS: usize = 16;

/// The temperatures point `i` may take: inside the graph and 2 °C away from its neighbours.
pub fn temp_range(points: &[[f32; 2]], i: usize) -> (f32, f32) {
    let lo = if i > 0 { points[i - 1][0] + 2.0 } else { GRAPH_T.0 };
    let hi = if i + 1 < points.len() { points[i + 1][0] - 2.0 } else { GRAPH_T.1 };
    (lo, hi.max(lo))
}

/// Move point `i` (dragged or typed): whole numbers, temperature kept between its neighbours, speed
/// between the fan's floor and 100 %. True when the point changed.
pub fn set_point(points: &mut [[f32; 2]], i: usize, t: f32, d: f32, floor: f32) -> bool {
    let Some(old) = points.get(i).copied() else { return false };
    if !t.is_finite() || !d.is_finite() {
        return false;
    }
    let (lo, hi) = temp_range(points, i);
    let new = [t.round().clamp(lo, hi), d.round().clamp(floor.min(100.0), 100.0)];
    points[i] = new;
    new != old
}

/// Add a point (a click on the graph). Refused when the curve is full or another point is closer than
/// 2 °C. Returns the new point's index.
pub fn add_point(points: &mut Vec<[f32; 2]>, t: f32, d: f32, floor: f32) -> Option<usize> {
    if points.len() >= MAX_POINTS || !t.is_finite() || !d.is_finite() {
        return None;
    }
    let t = t.round().clamp(GRAPH_T.0, GRAPH_T.1);
    if points.iter().any(|p| (p[0] - t).abs() < 2.0) {
        return None;
    }
    let i = points.iter().position(|p| p[0] > t).unwrap_or(points.len());
    points.insert(i, [t, d.round().clamp(floor.min(100.0), 100.0)]);
    Some(i)
}

/// Remove point `i`. Refused when only the minimum is left.
pub fn remove_point(points: &mut Vec<[f32; 2]>, i: usize) -> bool {
    if points.len() <= MIN_POINTS || i >= points.len() {
        return false;
    }
    points.remove(i);
    true
}

/// A setting as loaded from fan-profiles.json, with its curve normalised (an unreadable curve becomes the
/// Balanced curve; the helper still applies the fan's floor).
pub fn normalize_setting(s: &mut FanSetting) {
    match s {
        FanSetting::Curve { points, .. } => {
            *points = normalize_points(points).unwrap_or_else(|| preset_points("Balanced").map(|p| p.to_vec()).unwrap_or_default());
        }
        FanSetting::Trigger { idle_temp, load_temp, idle, load, .. } => {
            (*idle_temp, *load_temp, *idle, *load) = fix_trigger(*idle_temp, *load_temp, *idle, *load, 0.0);
        }
        _ => {}
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

pub fn preset_points(name: &str) -> Option<[[f32; 2]; 5]> {
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
    preset_points(name)?;
    let mut p = Profile::default();
    for f in fans.iter().filter(|f| f.controllable && (f.gpu || f.rpm.unwrap_or(0.0) > 0.0)) {
        if let Some(s) = fan_preset(name, f, pumps) {
            p.fans.insert(f.id.clone(), s);
        }
    }
    Some(p)
}

/// A preset's curve for one fan: the preset's points, never under the fan's floor, following the temperature
/// that suits the fan. None for an unknown preset, a fan that cannot be controlled, and Silent on the
/// graphics card (its own curve can stop the fans at idle).
pub fn fan_preset(name: &str, f: &FanInfo, pumps: &BTreeMap<String, bool>) -> Option<FanSetting> {
    let pts = preset_points(name)?;
    if !f.controllable || (name == "Silent" && f.gpu) {
        return None;
    }
    let pump = pumps.get(&f.id).copied().unwrap_or(f.pump);
    let floor = floor_for(pump, f.min);
    let FanSetting::Curve { source, .. } = default_curve(&f.name, f.gpu, pump, floor) else { return None };
    Some(FanSetting::Curve { source, points: pts.iter().map(|q| [q[0], q[1].max(floor)]).collect() })
}

/// Which preset a fan's setting is exactly (None: BIOS, manual, a trigger or an edited curve).
pub fn preset_of(setting: &FanSetting, f: &FanInfo, pumps: &BTreeMap<String, bool>) -> Option<&'static str> {
    BUILTIN.iter().copied().find(|n| fan_preset(n, f, pumps).as_ref() == Some(setting))
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
pub fn fan_chip(f: &FanInfo, setting: &FanSetting, can: bool, emergency: bool, blind: bool, handed_back: bool) -> (&'static str, Tone) {
    let ours = *setting != FanSetting::Bios;
    if !can {
        // view-only: nothing is sent, so show what the helper reports, not the saved setting
        return match (f.mode == "software", f.curve) {
            (true, true) if matches!(setting, FanSetting::Trigger { .. }) => ("Trigger", Tone::Accent),
            (true, true) => ("Curve", Tone::Accent),
            (true, false) => ("PC Tuner", Tone::Accent),
            _ => ("BIOS", Tone::Off),
        };
    }
    if ours && blind {
        ("BIOS - temps unreadable", Tone::Warn)
    } else if ours && handed_back {
        ("BIOS (handed back)", Tone::Warn)
    } else if ours && emergency {
        ("100 % - hot", Tone::Error)
    } else if matches!(setting, FanSetting::Curve { .. }) {
        ("Curve", Tone::Accent)
    } else if matches!(setting, FanSetting::Trigger { .. }) {
        ("Trigger", Tone::Accent)
    } else if ours {
        ("PC Tuner", Tone::Accent)
    } else {
        ("BIOS", Tone::Off)
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
        FanSetting::Trigger { source, idle_temp, load_temp, idle, load } => {
            let (it, lt, i, l) = fix_trigger(*idle_temp, *load_temp, *idle, *load, 0.0);
            format!("trigger {id} {} {it:.0} {lt:.0} {i:.0} {l:.0}", source.key())
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
pub fn temp_color(t: f32) -> Tone {
    if t >= 85.0 {
        Tone::Error
    } else if t >= 70.0 {
        Tone::Warn
    } else {
        Tone::Accent
    }
}

pub fn push(h: &mut VecDeque<f32>, v: f32) {
    if h.len() >= HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

/// Latest data from the helper, filled by the reader thread (keeps going while another tab is open).
#[derive(Default)]
pub struct Shared {
    pub status: Option<Status>,
    pub rpm: HashMap<String, VecDeque<f32>>,
    pub cpu: VecDeque<f32>,
    pub gpu: VecDeque<f32>,
    pub log: Vec<String>,
    pub exited: bool,
    /// When the last status line arrived (the helper sends one about every second).
    pub last: Option<Instant>,
}

/// No status for this long: the data on screen is shown as stale (display only).
pub const STALE_AFTER: Duration = Duration::from_secs(5);
/// A running helper silent for this long is hung and is let go (its input is closed, see `Helper::detach`). Well above the helper's own
/// 10 s watchdog, so a few slow sensor reads never cost the fans their safety loop.
pub const HANG_AFTER: Duration = Duration::from_secs(30);
/// A fresh helper that sends nothing for this long (opening the PawnIO driver can take a while) is hung.
pub const FIRST_STATUS_WITHIN: Duration = Duration::from_secs(30);
/// The helper's watchdog (Safety.WatchdogSeconds in Program.cs).
#[cfg(test)]
pub const HELPER_WATCHDOG: Duration = Duration::from_secs(10);

/// A hung helper that was let go (its input closed) is killed only after this long: it hands the fans back
/// itself as soon as its loop runs again, which a kill would make impossible.
pub const DETACHED_KILL_AFTER: Duration = Duration::from_secs(150);

/// What a helper's stderr says about the fans: (fans handed back on exit, start-up reset pass done with this
/// many fans it could not confirm). Only graphics-card fans can be handed back by a new process: a
/// motherboard header an earlier helper left in manual mode stays there until a reboot ("unconfirmed").
pub fn log_marks(line: &str) -> (bool, Option<u32>) {
    let pass = line.strip_prefix("reset pass done:").map(|rest| {
        // "reset pass done: <g> gpu, <m> unconfirmed"; anything else counts as unconfirmed
        rest.split(',')
            .find_map(|p| p.trim().strip_suffix(" unconfirmed").and_then(|n| n.trim().parse::<u32>().ok()))
            .unwrap_or(1)
    });
    (line.starts_with("fans returned to BIOS"), pass)
}

/// Seconds since 1970 when Windows started (to tell whether the PC rebooted since fans were left stuck).
pub fn boot_time() -> u64 {
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
pub const SAME_BOOT_SECS: u64 = 120;
pub const STUCK_FILE: &str = "fan-stuck.txt";

pub const STUCK_AT_START: &str = "An earlier fan helper did not hand the fans back since Windows started - they may stay at their last speed. PC Tuner drives them with your settings and its safety rules; reboot to give them back to the BIOS.";
pub const WAITING_FOR_OLD_HELPER: &str = "The fan helper stopped answering. Waiting for it to hand the fans back to the BIOS (it does as soon as it runs again) - until then fans may stay at their last speed. A new helper starts after it exits.";
pub const RESTARTED_STUCK: &str = "The fan helper stopped without handing the fans back and was restarted. It drives your fans again with its safety rules; motherboard fans cannot go back to the BIOS until you reboot (graphics-card fans were handed back).";

pub fn stuck_since_this_boot(saved: &str, boot: u64) -> bool {
    saved.trim().parse::<u64>().map(|b| b.abs_diff(boot) <= SAME_BOOT_SECS).unwrap_or(false)
}

/// The card shown instead of live data: (headline, detail). Only says "BIOS control" when that is known:
/// a running helper still owns its fans, and a stuck one may have left them at their last duty.
pub fn stale_text(helper_gone: bool, stuck: bool, age_secs: u64) -> (String, String) {
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
pub fn gone_message(stuck: bool, tail: &str) -> String {
    let head = if stuck {
        "The fan helper stopped without confirming it handed the fans back - fans may be stuck at their last speed. Reboot to give them back to the BIOS."
    } else {
        "The fan helper stopped. Fans are on BIOS control."
    };
    format!("{head} {tail}").trim().to_string()
}

impl Shared {
    /// Forget the live data (the helper is gone for good).
    pub fn clear(&mut self) {
        self.status = None;
        self.rpm.clear();
        self.cpu.clear();
        self.gpu.clear();
        self.last = None;
    }

    pub fn take_status(&mut self, s: Status) {
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
        self.status = Some(s);
    }
}


/// The running helper process.
pub struct Helper {
    pub child: Child,
    /// None once closed (detach): the helper then hands its fans back as soon as its loop runs.
    pub stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub shared: Arc<Mutex<Shared>>,
    pub stopped: bool,
    pub since: Instant,
    pub launch: Launch,
    /// Started with --reset-only: it hands every fan back to the BIOS and exits (takes no commands).
    pub reset_only: bool,
    /// Its stderr said "fans returned to BIOS" (its ResetAll ran).
    pub back: Arc<AtomicBool>,
    /// Its reset pass: how many fans it could not confirm as handed back (u32::MAX: no pass reported yet).
    pub pass_unconfirmed: Arc<AtomicU32>,
    /// Its stderr has closed (every line it wrote has been read).
    pub err_done: Arc<AtomicBool>,
    /// When it was first seen exited.
    pub exited_at: Option<Instant>,
    /// When it was let go as hung (input closed, never killed before DETACHED_KILL_AFTER).
    pub detached_at: Option<Instant>,
}

/// How a helper starts.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Launch {
    Normal,
    /// Reset pass on every controllable control first (an earlier helper may have left fans behind): GPU
    /// fans go back to the driver; motherboard headers cannot (their saved default may be stale), so they
    /// are driven with the settings and safety rules, and go to 100 % instead of being "handed back".
    Reclaim,
    /// Only that reset pass (graphics-card fans back to the driver), then exit.
    ResetOnly,
}

impl Helper {
    pub fn start(shared: Arc<Mutex<Shared>>, launch: Launch) -> Result<Helper, String> {
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

    pub fn send(&self, cmd: &str) -> bool {
        self.stdin.lock().map(|mut w| w.as_mut().is_some_and(|w| writeln!(w, "{cmd}").and_then(|_| w.flush()).is_ok())).unwrap_or(false)
    }

    pub fn alive(&mut self) -> bool {
        let alive = matches!(self.child.try_wait(), Ok(None));
        if !alive && self.exited_at.is_none() {
            self.exited_at = Some(Instant::now());
        }
        alive
    }

    /// Its reset pass covered every control (--reclaim / --reset-only) and every one was a graphics-card
    /// fan the driver took back: nothing an earlier helper left can still be stuck.
    pub fn pass_cleared_all(&self) -> bool {
        self.launch != Launch::Normal && self.pass_unconfirmed.load(Ordering::SeqCst) == 0
    }

    /// Let a hung helper go without killing it: its input is closed (the keep-alive stops too), so it hands
    /// its fans back (ResetAll) as soon as its loop runs again, or its own 10 s watchdog fires. Never blocks.
    pub fn detach(&mut self) {
        self.stopped = true; // Drop never quits / kills it
        if let Ok(mut w) = self.stdin.lock() {
            *w = None;
        }
        self.detached_at = Some(Instant::now());
    }

    /// A detached helper that is still stuck after DETACHED_KILL_AFTER: killed as a last resort.
    pub fn kill_if_stuck_too_long(&mut self) {
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
    pub fn outcome(&mut self) -> Option<bool> {
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
    pub fn hung(&self) -> bool {
        let last = self.shared.lock().ok().and_then(|s| s.last);
        match last {
            Some(t) => t.elapsed() > HANG_AFTER,
            None => self.since.elapsed() > FIRST_STATUS_WITHIN,
        }
    }

    /// After it exited: did it say it handed its fans back? Waits up to 1 s for the last stderr lines.
    pub fn handed_back(&self) -> bool {
        let t0 = Instant::now();
        while !self.err_done.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.back.load(Ordering::SeqCst)
    }

    /// Ask the helper to hand every fan back to the BIOS and exit; kill it if it does not within 3 s.
    /// Only when PC Tuner closes (on_exit / Drop): a hung helper is detached instead, never killed early.
    pub fn stop(&mut self) {
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

pub fn demo_status(t: f64) -> Status {
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
        rpm_id: id.replace("/control/", "/fan/"),
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
        rpms: [("/lpc/nct6687d/0/fan/0", "CPU Fan", 1180.0), ("/lpc/nct6687d/0/fan/1", "Pump Fan", 2650.0), ("/lpc/nct6687d/0/fan/2", "System Fan #1", 820.0), ("/lpc/nct6687d/0/fan/3", "System Fan #2", 805.0), ("/lpc/nct6687d/0/fan/4", "System Fan #3", 790.0)]
            .iter()
            .map(|(id, name, rpm)| RpmInfo { id: (*id).into(), hw: board.into(), name: (*name).into(), rpm: Some(*rpm) })
            .collect(),
        job: None,
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
pub fn demo_apply(s: &mut Status, settings: &HashMap<String, FanSetting>) {
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
            Some(FanSetting::Trigger { source, idle_temp, load_temp, idle, load }) => {
                if let Some(t) = source.read(cpu, gpu) {
                    // the demo has no history: halfway between the two temperatures counts as gaming
                    let d = if trigger_loaded(t >= (idle_temp + load_temp) / 2.0, t, *idle_temp, *load_temp) { *load } else { *idle }.max(floor);
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
pub enum CardAction {
    Set(FanSetting),
    Pump(bool),
    /// Drop the fan's own setting and use the profile's again.
    FollowProfile,
}

/// The Fan Tuner state: the helper, what the user asked for per fan, and the profiles.
pub struct Fan {
    pub shared: Arc<Mutex<Shared>>,
    pub helper: Option<Helper>,
    pub error: Option<String>,
    pub restarts: u32,
    /// The running helper was sent a fan setting (set / curve), so it may own fans.
    pub drove: bool,
    /// A hung helper that was let go (input closed, not killed): it hands its fans back once it runs again.
    /// No new helper starts until it has exited, so two never drive the same chip.
    pub detached: Option<Helper>,
    /// A helper that may have owned fans went away without saying it handed them back: the fans may be
    /// stuck at their last duty until a reboot (a new helper cannot restore motherboard headers).
    /// Remembered across PC Tuner restarts until the PC reboots.
    pub stuck: bool,
    /// `stuck` is saved to STUCK_FILE (the real app; never in tests or the demo).
    pub persist: bool,
    /// A --reset-only helper was already tried for stuck fans.
    pub reclaim_tried: bool,
    pub started: bool,
    pub demo: bool,
    /// What the user asked for, per fan id.
    pub settings: HashMap<String, FanSetting>,
    /// Last command sent per key, so each change is sent once (cleared when the helper restarts).
    pub sent: HashMap<String, String>,
    /// When each command was sent (a setting the helper still does not have a while later is sent again).
    pub sent_at: HashMap<String, Instant>,
    /// Fans PC Tuner should drive but the helper reports as back on the BIOS (refused or dropped setting).
    pub handed_back: HashSet<String>,
    /// The helper notice the user dismissed (its msgSeq).
    pub msg_dismissed: u64,
    pub afterburner: Arc<AtomicBool>,
    pub store: ProfileStore,
    /// The profile the settings came from (they may have been changed since).
    pub active: Option<String>,
    pub new_name: String,
    pub note: Option<String>,
    /// An own setting changed at this time and is not saved yet (saved shortly after, so a curve drag is one write).
    pub pin_dirty: Option<Instant>,
    /// The last fan test result taken from the helper (its seq; the helper counts from 1 again after a restart).
    pub job_seen: u64,
}

impl Fan {
    pub fn new() -> Self {
        let demo = std::env::var("PCTUNER_DEMO").map(|v| v.to_ascii_lowercase().starts_with("fan")).unwrap_or(false);
        let store = if demo { ProfileStore::default() } else { common::load_json(PROFILES_FILE) };
        Self::new_with(store, demo, true)
    }

    /// `startup`: bring back the last profile / own settings (and start the helper as administrator), or
    /// set up the demo screen.
    /// Tests pass `demo = true, startup = false`, so they never read the user's file or touch a fan.
    pub fn new_with(mut store: ProfileStore, demo: bool, startup: bool) -> Self {
        normalize_store(&mut store);
        let mut tab = Fan {
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
            job_seen: 0,
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
        }
        tab
    }

    pub fn start(&mut self) {
        // fans an earlier helper may have left in software mode: the new one reclaims them (GPU fans go back
        // to the driver; motherboard headers are driven again, never "handed back" to a stale duty)
        let launch = if self.stuck { Launch::Reclaim } else { Launch::Normal };
        self.start_with(launch);
    }

    /// Mark fans as (not) stuck, remembered with the boot time so a restarted PC Tuner still knows.
    pub fn set_stuck(&mut self, stuck: bool) {
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

    pub fn start_with(&mut self, launch: Launch) {
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
                self.job_seen = 0;
            }
            Err(e) => self.error = Some(e),
        }
        if !first {
            // the Afterburner check below is already running
            return;
        }
        // is MSI Afterburner running? (it may also be driving the GPU fans)
        let flag = Arc::downgrade(&self.afterburner);
        std::thread::spawn(move || loop {
            let Some(f) = flag.upgrade() else { break };
            let running = common::run("tasklist", &["/FI", "IMAGENAME eq MSIAfterburner.exe", "/NH"])
                .map(|(_, out)| out.to_ascii_lowercase().contains("msiafterburner.exe"))
                .unwrap_or(false);
            f.store(running, Ordering::Relaxed);
            drop(f);
            std::thread::sleep(Duration::from_secs(5));
        });
    }

    /// Send every setting / pump choice that changed since it was last sent.
    /// Only as administrator: a view-only PC Tuner never changes a fan.
    pub fn sync(&mut self) {
        if !common::is_admin() {
            return;
        }
        let Some(h) = &self.helper else { return };
        if h.reset_only {
            // it takes no commands: settings go to the next helper
            return;
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
        for (id, sensor) in &self.store.pairs {
            cmds.push((format!("pair:{id}"), format!("pair {id} {sensor}")));
        }
        for (id, s) in &self.settings {
            cmds.push((id.clone(), command_for(id, s)));
        }
        for (key, cmd) in cmds {
            if self.sent.get(&key) != Some(&cmd) && h.send(&cmd) {
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
    pub fn give_all_back(&mut self) {
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
        if let Some(h) = &self.helper {
            h.send("autoall");
        }
    }

    pub fn any_controlled(&self) -> bool {
        self.settings.values().any(|s| *s != FanSetting::Bios)
    }

    /// Take a profile's settings: listed fans as saved, every other fan back to BIOS -
    /// except fans with their own setting, which keep it.
    pub fn use_profile(&mut self, name: &str, p: &Profile) {
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
    pub fn profile_setting(&self, id: &str) -> FanSetting {
        self.active.as_ref().and_then(|n| self.store.profiles.get(n)).and_then(|p| p.fans.get(id).cloned()).unwrap_or_default()
    }

    /// Give a fan its own setting (kept when profiles change); saved straight away.
    pub fn pin(&mut self, id: &str, s: &FanSetting) {
        self.store.pinned.insert(id.to_string(), s.clone());
        self.pin_dirty = Some(Instant::now());
    }

    /// Move away from profiles (the new app sets presets per fan): every fan keeps the setting it has now as
    /// its own, and no profile is active or comes back at startup. Nothing changes on the fans.
    pub fn adopt_profile_settings(&mut self) {
        if self.active.is_none() && self.store.last.is_none() {
            return;
        }
        for (id, s) in normalized(&self.settings) {
            self.store.pinned.entry(id).or_insert(s);
        }
        self.active = None;
        self.store.last = None;
        self.note = None;
        self.save_store();
    }

    /// Hand a fan back to the active profile.
    pub fn unpin(&mut self, id: &str) {
        self.store.pinned.remove(id);
        let s = self.profile_setting(id);
        self.settings.insert(id.to_string(), s);
        self.save_store();
    }

    /// Settings of the fans that follow the profile (own settings are not part of it).
    pub fn following(&self) -> BTreeMap<String, FanSetting> {
        normalized(&self.settings).into_iter().filter(|(k, _)| !self.store.pinned.contains_key(k)).collect()
    }

    pub fn save_store(&mut self) {
        if !self.demo {
            common::save_json(PROFILES_FILE, &self.store);
        }
    }

    /// Load a profile (a built-in one is created for this PC's fans the first time) and remember it for startup.
    pub fn load_profile(&mut self, name: &str, fans: &[FanInfo]) {
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

    pub fn current_profile(&self) -> Profile {
        Profile { fans: self.following(), pumps: BTreeMap::new() }
    }

    pub fn modified(&self) -> bool {
        match self.active.as_ref().and_then(|n| self.store.profiles.get(n)) {
            Some(p) => {
                let saved: BTreeMap<String, FanSetting> = p.fans.iter().filter(|(k, _)| !self.store.pinned.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
                self.following() != saved
            }
            None => !self.following().is_empty(),
        }
    }


    /// Restart the helper once if it stopped unexpectedly.
    /// A helper that is running but has sent nothing for 30 s is hung (its loop may be stuck in a sensor
    /// read): it is never killed early, which would leave the fans at their last duty with no safety rules.
    /// Its input is closed instead (it hands its fans back as soon as its loop runs again) and it is watched
    /// without blocking; the next helper starts only once it has exited (killed after DETACHED_KILL_AFTER).
    /// A helper that may have owned fans and left without saying it handed them back makes the fans "stuck"
    /// until a reboot: the next helper reclaims them (or, when no restart is left, a reset-only helper hands
    /// the graphics-card fans back). The banner only says "BIOS control" when that is known.
    pub fn watch(&mut self) {
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
    pub fn helper_gone(&mut self, h: Helper, back: bool) {
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

    /// The data to draw, and whether it is stale (the helper exited, is not running, or sent nothing for 5 s).
    pub fn snapshot(&self) -> (Option<Status>, HashMap<String, VecDeque<f32>>, VecDeque<f32>, VecDeque<f32>, Option<Duration>) {
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
                let stale = sh.exited || self.helper.is_none() || age.is_some_and(|a| a > STALE_AFTER);
                let stale_for = (sh.status.is_some() && stale).then(|| age.unwrap_or_default());
                (sh.status.clone(), sh.rpm.clone(), sh.cpu.clone(), sh.gpu.clone(), stale_for)
            }
            Err(_) => (None, HashMap::new(), VecDeque::new(), VecDeque::new(), None),
        }
    }

    /// The fan test running now (None when none is running).
    pub fn running_test(&self) -> Option<JobInfo> {
        let sh = self.shared.lock().ok()?;
        sh.status.as_ref()?.job.clone().filter(|j| !j.done)
    }

    /// Start a fan test on one fan: "calibrate" or "pair". Administrator only; one at a time.
    pub fn start_test(&mut self, kind: &str, id: &str) -> Result<(), String> {
        if !common::is_admin() {
            return Err("Run PC Tuner as administrator to test fans.".into());
        }
        if self.running_test().is_some() {
            return Err("A fan test is already running.".into());
        }
        let cmd = match kind {
            "calibrate" => format!("calibrate {id}"),
            "pair" => format!("pairdetect {id}"),
            _ => return Err("unknown test".into()),
        };
        match &self.helper {
            Some(h) if !h.reset_only && h.send(&cmd) => Ok(()),
            _ => Err("The fan helper is not running.".into()),
        }
    }

    pub fn cancel_test(&mut self) {
        if let Some(h) = &self.helper {
            h.send("jobcancel");
        }
    }

    /// Choose a fan's speed sensor by hand ("auto" = the built-in pairing). Saved for every profile.
    pub fn pair(&mut self, id: &str, sensor: &str) {
        if sensor == "auto" {
            self.store.pairs.remove(id);
            self.sent.remove(&format!("pair:{id}"));
            if let Some(h) = &self.helper {
                if common::is_admin() {
                    h.send(&format!("pair {id} auto"));
                }
            }
        } else {
            self.store.pairs.insert(id.to_string(), sensor.to_string());
        }
        // a calibration was measured on the old sensor
        self.store.calib.remove(id);
        self.save_store();
    }

    /// Keep a finished test's result: a calibration, or the speed sensor pairing found.
    pub fn take_test_result(&mut self) {
        let job = self.shared.lock().ok().and_then(|sh| sh.status.as_ref().and_then(|s| s.job.clone()));
        let Some(j) = job.filter(|j| j.done && j.seq > self.job_seen) else { return };
        self.job_seen = j.seq;
        if !j.error.is_empty() {
            return;
        }
        match j.kind.as_str() {
            "calibrate" => {
                self.store.calib.insert(j.id.clone(), Calibration::from_job(&j, common::now()));
            }
            "pair" if !j.paired.is_empty() => {
                self.store.pairs.insert(j.id.clone(), j.paired.clone());
                // the helper already uses it
                self.sent.insert(format!("pair:{}", j.id), format!("pair {} {}", j.id, j.paired));
                self.store.calib.remove(&j.id);
            }
            _ => return,
        }
        self.save_store();
    }

    /// May fans be changed (administrator, or the demo screen)?
    pub fn can(&self) -> bool {
        common::is_admin() || self.demo
    }

    /// The page was opened: start the helper the first time (view-only shows the graphics card).
    pub fn ensure_started(&mut self) {
        if !self.started {
            self.start();
        }
    }

    /// Is MSI Afterburner running (it may also be driving the GPU fans)?
    pub fn afterburner_running(&self) -> bool {
        self.afterburner.load(Ordering::Relaxed)
    }

    /// No helper is running and none is expected back (a detached, hung helper may still own fans).
    pub fn helper_is_gone(&self) -> bool {
        !self.demo && self.helper.is_none() && self.detached.is_none()
    }

    /// Fans may be stuck at their last speed until a reboot.
    pub fn fans_stuck(&self) -> bool {
        !self.demo && self.stuck
    }

    /// The periodic work: watch the helper, send changed settings, save own settings shortly after an edit.
    /// Runs on the engine thread, so a startup profile keeps running with no window.
    pub fn tick(&mut self) {
        if self.started {
            self.watch();
            self.take_test_result();
            self.sync();
        }
        if self.pin_dirty.map(|t| t.elapsed() > Duration::from_millis(600)).unwrap_or(false) {
            self.pin_dirty = None;
            self.save_store();
        }
    }

    /// PC Tuner is closing: save, stop the helper (it hands the fans back to the BIOS) and remember stuck fans.
    pub fn on_exit(&mut self) {
        if self.pin_dirty.take().is_some() {
            self.save_store();
        }
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

    /// What the user did on a fan card (`floor`: the lowest duty this fan accepts).
    pub fn apply(&mut self, id: String, floor: f32, a: CardAction) {
        match a {
            // a fan the user tunes gets its own setting: switching profiles leaves it alone
            CardAction::Set(s) => {
                let s = match s {
                    FanSetting::Manual { percent } => FanSetting::Manual { percent: percent.max(floor) },
                    FanSetting::Curve { source, points } => FanSetting::Curve { source, points: points.into_iter().map(|p| [p[0], p[1].clamp(floor, 100.0)]).collect() },
                    FanSetting::Trigger { source, idle_temp, load_temp, idle, load } => {
                        let (idle_temp, load_temp, idle, load) = fix_trigger(idle_temp, load_temp, idle, load, floor);
                        FanSetting::Trigger { source, idle_temp, load_temp, idle, load }
                    }
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
                        Some(FanSetting::Trigger { idle, load, .. }) => {
                            *idle = idle.max(PUMP_FLOOR);
                            *load = load.max(PUMP_FLOOR);
                        }
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
                    if let (true, FanSetting::Trigger { idle, load, .. }) = (p, &mut *s) {
                        *idle = idle.max(PUMP_FLOOR);
                        *load = load.max(PUMP_FLOOR);
                    }
                }
                // a fact about the header, the same for every profile: saved with the store
                self.store.pumps.insert(id, p);
                self.pin_dirty = Some(Instant::now());
            }
        }
    }

    /// The profile names to offer: the built-in ones, then the user's own.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = BUILTIN.iter().map(|s| s.to_string()).collect();
        names.extend(self.store.profiles.keys().filter(|k| !BUILTIN.contains(&k.as_str())).cloned());
        names
    }

    /// "Save changes": store the fans as they are now in the active profile.
    pub fn save_changes(&mut self) {
        let Some(n) = self.active.clone() else { return };
        let p = self.current_profile();
        self.store.profiles.insert(n.clone(), p);
        self.store.last = Some(n.clone());
        self.save_store();
        self.note = Some(format!("{n} saved."));
    }

    /// "Reset to default": throw away the edits to a built-in profile.
    pub fn reset_builtin(&mut self, n: &str, fans: &[FanInfo]) {
        self.store.profiles.remove(n);
        self.load_profile(n, fans);
        self.note = Some(format!("{n} is back to its default curves."));
    }

    /// "Delete": remove one of the user's profiles (the fans stay as they are).
    pub fn delete_profile(&mut self, n: &str) {
        self.store.profiles.remove(n);
        if self.store.last.as_deref() == Some(n) {
            self.store.last = None;
        }
        self.active = None;
        self.save_store();
        self.note = Some(format!("{n} deleted. The fans stay as they are until you change them."));
    }

    /// "Save as new": store the fans as they are now under `new_name`.
    pub fn save_as_new(&mut self) {
        let name = self.new_name.trim().to_string();
        let p = self.current_profile();
        self.store.profiles.insert(name.clone(), p);
        self.store.last = Some(name.clone());
        self.active = Some(name.clone());
        self.save_store();
        self.new_name.clear();
        self.note = Some(format!("{name} saved and will be applied when PC Tuner starts."));
    }
}

/// The fan engine: one `Fan` shared with a thread of its own that watches the helper and sends settings,
/// whether or not a window is open. The UI locks it to draw and to change settings.
pub struct FanEngine {
    fan: Arc<Mutex<Fan>>,
    stop: Arc<AtomicBool>,
}

/// How often the engine thread runs `Fan::tick`.
pub const ENGINE_TICK: Duration = Duration::from_millis(250);

impl FanEngine {
    /// Wrap a `Fan` and start its thread.
    pub fn new(fan: Fan) -> Self {
        let fan = Arc::new(Mutex::new(fan));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s) = (fan.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("fan-engine".into()).spawn(move || {
            while !s.load(Ordering::Relaxed) {
                f.lock().unwrap_or_else(|e| e.into_inner()).tick();
                std::thread::sleep(ENGINE_TICK);
            }
        });
        FanEngine { fan, stop }
    }

    /// The state, for drawing or changing (keep the lock short: the engine thread waits for it).
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Fan> {
        self.fan.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// PC Tuner is closing: stop the thread and hand the fans back.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.lock().on_exit();
    }
}

impl Drop for FanEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
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

    /// Trigger: gaming defaults per fan kind, the helper command, value rules, and old profile files.
    #[test]
    fn fan_trigger() {
        let t = |s: FanSetting| match s {
            FanSetting::Trigger { source, idle_temp, load_temp, idle, load } => (source, idle_temp, load_temp, idle, load),
            _ => panic!("not a trigger"),
        };
        assert_eq!(t(default_trigger("CPU Fan", false, false, 0.0)), (Source::Cpu, 55.0, 65.0, 30.0, 65.0));
        assert_eq!(t(default_trigger("Pump Fan", false, true, 0.0)), (Source::Cpu, 55.0, 65.0, 30.0, 65.0));
        assert_eq!(t(default_trigger("GPU Fan", true, false, 30.0)), (Source::Gpu, 50.0, 60.0, 30.0, 60.0));
        assert_eq!(t(default_trigger("System Fan #1", false, false, 0.0)), (Source::Hottest, 55.0, 65.0, 30.0, 70.0));
        assert_eq!(t(default_trigger("GPU Fan", true, false, 40.0)).3, 40.0, "never under the driver minimum");
        // value rules
        assert_eq!(fix_trigger(60.0, 55.0, 30.0, 70.0, 0.0), (60.0, 62.0, 30.0, 70.0), "gaming point kept 2 °C above idle");
        assert_eq!(fix_trigger(5.0, 150.0, -5.0, 120.0, 20.0), (20.0, 100.0, 20.0, 100.0));
        assert_eq!(fix_trigger(f32::NAN, 65.4, 30.0, 70.0, 0.0), (55.0, 65.0, 30.0, 70.0));
        // latch
        assert!(!trigger_loaded(false, 64.0, 55.0, 65.0));
        assert!(trigger_loaded(false, 65.0, 55.0, 65.0));
        assert!(trigger_loaded(true, 58.0, 55.0, 65.0));
        assert!(!trigger_loaded(true, 55.0, 55.0, 65.0));
        // helper command and file format
        let s = FanSetting::Trigger { source: Source::Hottest, idle_temp: 55.0, load_temp: 65.0, idle: 30.0, load: 70.0 };
        assert_eq!(command_for("/lpc/x/control/2", &s), "trigger /lpc/x/control/2 hottest 55 65 30 70");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"mode":"trigger","source":"hottest","idle_temp":55.0,"load_temp":65.0,"idle":30.0,"load":70.0}"#);
        assert_eq!(serde_json::from_str::<FanSetting>(&json).unwrap(), s);
        // a saved trigger with bad values is repaired on load; files without triggers load as before
        let mut store: ProfileStore = serde_json::from_str(
            r#"{"pinned":{"a":{"mode":"trigger","source":"cpu","idle_temp":70,"load_temp":60,"idle":30,"load":65},
                "b":{"mode":"curve","source":"gpu","points":[[30,30],[80,100]]}}}"#,
        )
        .unwrap();
        normalize_store(&mut store);
        assert_eq!(store.pinned["a"], FanSetting::Trigger { source: Source::Cpu, idle_temp: 70.0, load_temp: 72.0, idle: 30.0, load: 65.0 });
        assert!(matches!(store.pinned["b"], FanSetting::Curve { .. }));
        // chip
        let mut f = FanInfo { id: "a".into(), controllable: true, ..Default::default() };
        assert_eq!(fan_chip(&f, &s, true, false, false, false).0, "Trigger");
        f.mode = "software".into();
        f.curve = true;
        assert_eq!(fan_chip(&f, &s, false, false, false, false).0, "Trigger", "view-only shows the helper's trigger too");
    }

    #[test]
    fn fan_floors_and_commands() {
        // no floor of PC Tuner's own, fan or pump: only the driver's minimum applies
        assert_eq!(floor_for(false, None), 0.0);
        assert_eq!(floor_for(true, None), 0.0);
        assert_eq!(floor_for(false, Some(30.0)), 30.0, "driver minimum still holds");
        assert_eq!(floor_for(true, Some(30.0)), 30.0);
        let mut pts = vec![[30.0, 50.0], [85.0, 100.0]];
        assert!(set_point(&mut pts, 0, 30.0, 35.0, floor_for(true, None)), "a pump curve point can go to 35 %");
        assert_eq!(pts[0], [30.0, 35.0]);
        assert!(set_point(&mut pts, 0, 30.0, 0.0, floor_for(false, None)));
        assert_eq!(pts[0], [30.0, 0.0], "down to 0 %");
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
        let mut t = Fan::new_with(ProfileStore::default(), true, false);
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
        let mut t = Fan::new_with(ProfileStore::default(), true, false);
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
    fn fan_presets_per_fan() {
        let fans = demo_status(0.0).fans;
        let pumps = BTreeMap::new();
        // the same curve a whole-PC profile gives that fan
        for name in BUILTIN {
            let p = builtin_profile(name, &fans, &pumps).unwrap();
            for f in fans.iter().filter(|f| f.controllable) {
                assert_eq!(fan_preset(name, f, &pumps).as_ref(), p.fans.get(&f.id), "{name} {}", f.name);
            }
        }
        let cpu = fans.iter().find(|f| f.name == "CPU Fan").unwrap();
        let gpu = fans.iter().find(|f| f.gpu).unwrap();
        let sys = fans.iter().find(|f| !f.controllable).unwrap();
        assert_eq!(fan_preset("Silent", gpu, &pumps), None, "Silent leaves the graphics card alone");
        assert_eq!(fan_preset("Balanced", sys, &pumps), None, "read-only fan");
        assert_eq!(fan_preset("Turbo", cpu, &pumps), None);
        let perf = fan_preset("Performance", cpu, &pumps).unwrap();
        assert_eq!(preset_of(&perf, cpu, &pumps), Some("Performance"));
        assert_eq!(preset_of(&FanSetting::Manual { percent: 50.0 }, cpu, &pumps), None);
        assert_eq!(preset_of(&FanSetting::Bios, cpu, &pumps), None);

        // moving off profiles: every fan keeps its setting as its own, no profile comes back
        let mut f = Fan::new_with(ProfileStore::default(), true, false);
        f.load_profile("Balanced", &fans);
        f.apply(cpu.id.clone(), 0.0, CardAction::Set(FanSetting::Manual { percent: 45.0 }));
        let before = normalized(&f.settings);
        f.adopt_profile_settings();
        assert!(f.active.is_none() && f.store.last.is_none());
        assert_eq!(normalized(&f.settings), before);
        assert_eq!(f.store.pinned, before, "all of them are own settings now");
        assert_eq!(f.store.pinned.get(&cpu.id), Some(&FanSetting::Manual { percent: 45.0 }), "an own setting is not overwritten");
        // a preset on one fan touches only that fan
        f.apply(cpu.id.clone(), 0.0, CardAction::Set(perf.clone()));
        assert_eq!(f.settings.get(&cpu.id), Some(&perf));
        assert_eq!(f.settings.get(&gpu.id), before.get(&gpu.id));
    }

    #[test]
    fn fan_tests_keep_their_results() {
        let mut f = Fan::new_with(ProfileStore::default(), true, false);
        let job = |seq: u64, kind: &str, error: &str| JobInfo {
            seq,
            kind: kind.into(),
            id: "/lpc/nct6687d/0/control/0".into(),
            done: true,
            error: error.into(),
            paired: "/lpc/nct6687d/0/fan/2".into(),
            stop: Some(20.0),
            start: Some(30.0),
            max: Some(1400.0),
            points: vec![[100.0, 1400.0], [20.0, 600.0], [10.0, 0.0], [0.0, 0.0]],
            ..Default::default()
        };
        let put = |f: &mut Fan, j: JobInfo| f.shared.lock().unwrap().status = Some(Status { job: Some(j), ..Default::default() });
        // a calibration is kept per fan, once
        put(&mut f, job(1, "calibrate", ""));
        f.take_test_result();
        let c = f.store.calib.get("/lpc/nct6687d/0/control/0").cloned().expect("kept");
        assert_eq!((c.stop, c.start, c.max, c.points.len()), (Some(20.0), Some(30.0), Some(1400.0), 4));
        assert_eq!(c.summary(), "1400 RPM at 100 %, stops below 20 %, starts again at 30 %");
        // a detected pairing replaces the calibration (it was measured on the old sensor)
        put(&mut f, job(2, "pair", ""));
        f.take_test_result();
        assert_eq!(f.store.pairs.get("/lpc/nct6687d/0/control/0").map(String::as_str), Some("/lpc/nct6687d/0/fan/2"));
        assert!(f.store.calib.is_empty());
        // a failed or cancelled test changes nothing
        put(&mut f, job(3, "calibrate", "Cancelled."));
        f.take_test_result();
        assert!(f.store.calib.is_empty() && f.job_seen == 3);
        // choosing the default sensor again forgets the pairing
        f.pair("/lpc/nct6687d/0/control/0", "auto");
        assert!(f.store.pairs.is_empty());
        // a fan that never stops says so
        let never = Calibration { points: vec![[100.0, 900.0], [0.0, 300.0]], stop: Some(0.0), max: Some(900.0), ..Default::default() };
        assert_eq!(never.summary(), "900 RPM at 100 %, never stops (still spins at 0 %)");
        // the helper's status line
        let s = parse_status(r#"{"fans":[{"id":"a","rpmId":"s1"}],"rpms":[{"id":"s1","hw":"x","name":"Fan #1","rpm":800}],"job":{"seq":4,"kind":"pair","id":"a","step":1,"steps":2,"duty":100,"done":false,"error":"","paired":"","start":null,"stop":null,"max":null,"points":[]}}"#).unwrap();
        assert_eq!((s.fans[0].rpm_id.as_str(), s.rpms[0].rpm, s.job.as_ref().map(|j| j.duty)), ("s1", Some(800.0), Some(Some(100.0))));
    }

    #[test]
    fn fan_pump_choices_are_global() {
        let mut store = ProfileStore::default();
        let mut a = Profile::default();
        a.pumps.insert("/x/cpu".into(), true);
        a.pumps.insert("/x/pump".into(), false);
        store.profiles.insert("A".into(), a);
        store.profiles.insert("B".into(), Profile::default());
        let mut t = Fan::new_with(store, true, false);
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

    /// Editing a curve: points are added in order, removed down to 2, and typed values stay inside the rules.
    #[test]
    fn fan_curve_point_editing() {
        let mut pts = vec![[30.0, 25.0], [50.0, 40.0], [85.0, 100.0]];
        // add: lands in temperature order, rounded, never under the floor
        assert_eq!(add_point(&mut pts, 61.4, 10.2, 20.0), Some(2));
        assert_eq!(pts[2], [61.0, 20.0]);
        assert_eq!(add_point(&mut pts, 95.0, 150.0, 20.0), Some(4));
        assert_eq!(pts[4], [95.0, 100.0]);
        assert_eq!(add_point(&mut pts, 10.0, 30.0, 20.0), Some(0), "left of the graph: clamped to 20 °C");
        assert_eq!(pts[0], [20.0, 30.0]);
        // too close to another point, or not a number: nothing added
        let before = pts.clone();
        assert_eq!(add_point(&mut pts, 51.0, 50.0, 20.0), None);
        assert_eq!(add_point(&mut pts, f32::NAN, 50.0, 20.0), None);
        assert_eq!(pts, before);
        // typed values: temperature stays 2 °C from the neighbours, speed between floor and 100
        assert!(set_point(&mut pts, 2, 90.0, 5.0, 20.0));
        assert_eq!(pts[2], [59.0, 20.0], "clamped under the next point (61 °C) and to the floor");
        assert!(set_point(&mut pts, 2, 0.0, 55.4, 20.0));
        assert_eq!(pts[2], [32.0, 55.0], "clamped above the previous point (30 °C)");
        assert!(!set_point(&mut pts, 2, 32.0, 55.0, 20.0), "same value: no change");
        assert!(!set_point(&mut pts, 99, 50.0, 50.0, 20.0));
        assert!(set_point(&mut pts, 5, 200.0, 100.0, 20.0));
        assert_eq!(pts[5][0], 100.0, "last point may go to the right edge");
        // an edited curve is already in the form the loader and the helper expect
        assert_eq!(normalize_points(&pts).unwrap(), pts);
        assert_eq!(command_for("f", &FanSetting::Curve { source: Source::Cpu, points: pts.clone() }), "curve f cpu 20:30,30:25,32:55,61:20,85:100,100:100");
        // remove: down to 2 points, no further
        assert!(!remove_point(&mut pts, 6));
        while pts.len() > MIN_POINTS {
            assert!(remove_point(&mut pts, 0));
        }
        assert!(!remove_point(&mut pts, 0));
        assert_eq!(pts.len(), 2);
        // full curve: no more points
        let mut full: Vec<[f32; 2]> = (0..MAX_POINTS).map(|i| [20.0 + 4.0 * i as f32, 50.0]).collect();
        assert_eq!(add_point(&mut full, 99.0, 50.0, 20.0), None);
        assert_eq!(full.len(), MAX_POINTS);
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
        let t = Fan::new_with(ProfileStore::default(), false, false);
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
        let mut t = Fan::new_with(ProfileStore::default(), false, false);
        assert!(!t.drove && !t.stuck && !t.reclaim_tried && t.detached.is_none() && !t.persist);
        t.set_stuck(true);
        assert!(t.stuck);
        t.set_stuck(false);
        assert!(!t.stuck);
    }

    /// The demo screen (README screenshots): duty agrees with the slider and with the curve's live point.
    #[test]
    fn fan_demo_numbers_agree() {
        let t = Fan::new_with(ProfileStore::default(), true, true);
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
        assert_eq!(temp_color(50.0), Tone::Accent);
        assert_eq!(temp_color(75.0), Tone::Warn);
        assert_eq!(temp_color(88.0), Tone::Error);
    }
}
