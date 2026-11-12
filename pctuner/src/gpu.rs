//! RTX 5080 Profiles tab - port of RTX5080-ProfileManager.ps1 (curve building from RTX5080-Profile.ps1).
//! Switches the card between undervolt presets stored in MSI Afterburner profile slots and shows which one is
//! applied right now (live: memory offset from nvidia-smi; saved: the [Startup] section Afterburner restores).

use crate::common::{self, Job, Tab};
use crate::opwin::{OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use winreg::enums::*;
use winreg::RegKey;

pub struct Preset {
    pub name: &'static str,
    pub slot: u32,
    pub target_mhz: i32,
    pub target_mv: i32,
    pub mem: i32,
    pub power: i32,
    pub text: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Stock", slot: 3, target_mhz: 0, target_mv: 0, mem: 0, power: 100,
        text: "Factory behaviour - no undervolt, no memory overclock. Use this to reset or to compare." },
    Preset { name: "Efficiency", slot: 2, target_mhz: 2700, target_mv: 900, mem: 1000, power: 100,
        text: "Coolest and quietest. 2700 MHz at 900 mV, memory +1000." },
    Preset { name: "Balanced", slot: 1, target_mhz: 2800, target_mv: 925, mem: 1500, power: 100,
        text: "Everyday gaming. 2800 MHz at 925 mV, memory +1500." },
    Preset { name: "Performance", slot: 4, target_mhz: 2900, target_mv: 950, mem: 2000, power: 100,
        text: "Most fps. 2900 MHz at 950 mV, memory +2000. Runs warmest of the three undervolts." },
];

/// MHz: Afterburner re-saves curves with slightly different base points.
const CLOCK_TOLERANCE: f64 = 30.0;

// ------------------------------------------------------------------ Afterburner file access

pub fn ab_dir() -> PathBuf {
    let d = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\WOW6432Node\MSI\Afterburner")
        .and_then(|k| k.get_value::<String, _>("InstallDir"))
        .unwrap_or_default();
    if d.trim().is_empty() {
        PathBuf::from(r"C:\Program Files (x86)\MSI Afterburner")
    } else {
        PathBuf::from(d.trim())
    }
}

pub fn ab_exe() -> PathBuf {
    ab_dir().join("MSIAfterburner.exe")
}

/// Backups go next to the original scripts when that folder exists, else into %LOCALAPPDATA%\PCTuner\gpu-backups.
pub fn backup_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    let script_dir = home.join(r"Documents\RTX5080-Balanced\backups");
    if script_dir.is_dir() {
        script_dir
    } else {
        common::data_dir().join("gpu-backups")
    }
}

/// Profiles\VEN_10DE&DEV_2C02*.cfg (first by name, like Get-ChildItem).
pub fn cfg_file() -> Option<PathBuf> {
    let dir = ab_dir().join("Profiles");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_uppercase();
            p.is_file() && n.starts_with("VEN_10DE&DEV_2C02") && n.ends_with(".CFG")
        })
        .collect();
    names.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    names.into_iter().next()
}

pub struct Cfg {
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub crlf: bool,
}

/// key=value pairs of one [section], in file order.
pub struct Section(pub Vec<(String, String)>);

impl Section {
    /// Case-insensitive like a PowerShell hashtable; the last duplicate wins.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
    pub fn from_lines(lines: &[String]) -> Section {
        Section(
            lines
                .iter()
                .map(|l| match l.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (l.clone(), String::new()),
                })
                .collect(),
        )
    }
}

impl Cfg {
    pub fn parse(path: PathBuf, bytes: &[u8]) -> Cfg {
        // Get-Content reads it as ANSI; the file is plain ASCII, so a byte-per-char decode is exact.
        let text: String = bytes.iter().map(|&b| b as char).collect();
        let text = text.trim_start_matches(['\u{ef}', '\u{bb}', '\u{bf}']);
        let crlf = text.contains("\r\n") || !text.contains('\n');
        let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
        if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        Cfg { path, lines, crlf }
    }

    pub fn read() -> Option<Cfg> {
        let path = cfg_file()?;
        let bytes = std::fs::read(&path).ok()?;
        Some(Cfg::parse(path, &bytes))
    }

    fn header_index(&self, name: &str) -> Option<usize> {
        let h = format!("[{name}]");
        self.lines.iter().position(|l| *l == h)
    }

    /// Body lines of [name] up to the next line starting with '['.
    fn section_range(&self, name: &str) -> Option<(usize, usize)> {
        let s = self.header_index(name)?;
        let mut e = s + 1;
        while e < self.lines.len() && !self.lines[e].starts_with('[') {
            e += 1;
        }
        Some((s, e))
    }

    pub fn section(&self, name: &str) -> Option<Section> {
        let (s, e) = self.section_range(name)?;
        Some(Section::from_lines(&self.lines[s + 1..e]))
    }

    #[allow(dead_code)] // used by the tests
    pub fn section_names(&self) -> Vec<String> {
        self.lines.iter().filter(|l| l.starts_with('[') && l.ends_with(']')).map(|l| l[1..l.len() - 1].to_string()).collect()
    }

    /// Like Set-Content -Encoding ASCII: non-ASCII becomes '?', every line ends with the file's newline.
    pub fn to_bytes(&self) -> Vec<u8> {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut out = Vec::new();
        for l in &self.lines {
            out.extend(l.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }));
            out.extend_from_slice(nl.as_bytes());
        }
        out
    }
}

// ------------------------------------------------------------------ VF curve

#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub off: usize,
    pub mv: f32,
    pub base: f32,
    pub ofs: f32,
}

pub struct Curve {
    pub bytes: Vec<u8>,
    pub points: Vec<Point>,
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// VFCurve hex: 12-byte header (u32 version 0x00020001, u32 point count, reserved) then {f32 mV, f32 base MHz, f32 offset MHz}.
pub fn decode_curve(hex: &str) -> Option<Curve> {
    let hex = hex.trim();
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.is_ascii() {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>().ok()?;
    if bytes.len() < 12 || u32_at(&bytes, 0) != 0x0002_0001 {
        return None;
    }
    let n = u32_at(&bytes, 4) as usize;
    if !(50..=255).contains(&n) || bytes.len() < 12 + n * 12 {
        return None;
    }
    let points = (0..n)
        .map(|k| {
            let o = 12 + k * 12;
            Point { off: o, mv: f32_at(&bytes, o), base: f32_at(&bytes, o + 4), ofs: f32_at(&bytes, o + 8) }
        })
        .collect();
    Some(Curve { bytes, points })
}

/// PowerShell [int]"x" / [int]([double]"x"): banker's rounding; empty/missing = 0; unparseable = None.
fn ps_int(v: Option<&str>) -> Option<i64> {
    let s = v.unwrap_or("").trim();
    if s.is_empty() {
        return Some(0);
    }
    s.parse::<f64>().ok().map(|f| f.round_ties_even() as i64)
}

/// Does a saved section (Startup / ProfileN) contain this preset?
pub fn section_is_preset(sec: Option<&Section>, p: &Preset) -> bool {
    let Some(sec) = sec else { return false };
    let Some(vf) = sec.get("VFCurve").filter(|v| !v.is_empty()) else { return false };
    let mem = match sec.get("MemClkBoost").unwrap_or("").trim() {
        "" => 0,
        s => match s.parse::<f64>() {
            Ok(f) => (f / 1000.0).round_ties_even() as i64,
            Err(_) => return false,
        },
    };
    if mem != p.mem as i64 {
        return false;
    }
    if let Some(pl) = sec.get("PowerLimit").filter(|v| !v.is_empty()) {
        if ps_int(Some(pl)) != Some(p.power as i64) {
            return false;
        }
    }
    let Some(c) = decode_curve(vf) else { return false };
    if p.target_mhz == 0 {
        let max_ofs = c.points.iter().map(|pt| pt.ofs.abs() as f64).fold(f64::MIN, f64::max);
        return max_ofs < 1.0 && ps_int(sec.get("CoreClkBoost")) == Some(0);
    }
    let max_eff = c.points.iter().map(|pt| pt.base as f64 + pt.ofs as f64).fold(f64::MIN, f64::max);
    (max_eff - p.target_mhz as f64).abs() <= CLOCK_TOLERANCE
}

pub fn find_preset(sec: Option<&Section>) -> Option<usize> {
    PRESETS.iter().position(|p| section_is_preset(sec, p))
}

/// Build a preset's [ProfileN] section from the card's own stock curve in [Startup].
pub fn preset_section_lines(cfg: &Cfg, p: &Preset) -> Result<Vec<String>, String> {
    let startup = cfg.section("Startup").ok_or("No [Startup] section in the Afterburner profile. Open Afterburner once, close it, and retry.")?;
    let vf = startup.get("VFCurve").filter(|v| !v.is_empty()).ok_or("No VFCurve in [Startup]. Open Afterburner once, close it, and retry.")?;
    let c = decode_curve(vf).ok_or("Unexpected curve format in the Afterburner profile. Not touching anything.")?;
    let pts = &c.points;
    let mut b = c.bytes.clone();
    let max_base = pts.iter().map(|pt| pt.base).fold(f32::MIN, f32::max);
    if pts[0].mv < 300.0 || pts[0].mv > 700.0 || pts[pts.len() - 1].mv <= pts[0].mv || max_base > 5000.0 {
        return Err("Curve data does not look like a voltage/frequency table. Not touching anything.".into());
    }
    if p.target_mhz == 0 {
        for pt in pts {
            b[pt.off + 8..pt.off + 12].copy_from_slice(&0f32.to_le_bytes());
        }
    } else {
        let anchor = pts.iter().find(|pt| pt.mv as f64 >= p.target_mv as f64).ok_or(format!("No curve point at or above {} mV.", p.target_mv))?;
        let lift = p.target_mhz as f64 - anchor.base as f64;
        if lift > 300.0 {
            return Err(format!("Preset needs +{lift} MHz over this card's stock curve at {} mV - too aggressive. Not writing it.", anchor.mv));
        }
        for pt in pts {
            let base = pt.base as f64;
            let want = if pt.mv < anchor.mv { (base + lift).min(p.target_mhz as f64) } else { p.target_mhz as f64 };
            b[pt.off + 8..pt.off + 12].copy_from_slice(&((want - base) as f32).to_le_bytes());
        }
    }
    let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
    let mut lines = vec![
        format!("[Profile{}]", p.slot),
        "Format=2".to_string(),
        "CoreVoltageBoost=0".to_string(),
        format!("PowerLimit={}", p.power),
        "CoreClkBoost=0".to_string(),
        format!("VFCurve={hex}"),
        format!("MemClkBoost={}", p.mem * 1000),
    ];
    for k in ["FanMode", "FanSpeed", "FanMode2", "FanSpeed2"] {
        if let Some(v) = startup.get(k) {
            lines.push(format!("{k}={v}"));
        }
    }
    Ok(lines)
}

/// Replace (or append) [ProfileN] in memory.
pub fn with_section(cfg: &Cfg, slot: u32, section: Vec<String>) -> Vec<String> {
    let mut lines = cfg.lines.clone();
    if let Some((s, e)) = cfg.section_range(&format!("Profile{slot}")) {
        lines.drain(s..e);
    }
    lines.extend(section);
    lines
}

// ------------------------------------------------------------------ processes

pub fn ab_running() -> bool {
    common::run("tasklist", &["/FI", "IMAGENAME eq MSIAfterburner.exe", "/NH"])
        .map(|(_, o)| o.to_lowercase().contains("msiafterburner.exe"))
        .unwrap_or(false)
}

fn kill_ab() {
    if ab_running() {
        let _ = common::run("taskkill", &["/F", "/IM", "MSIAfterburner.exe"]);
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn launch_ab(args: &[String]) -> Result<(), String> {
    let exe = ab_exe();
    std::process::Command::new(&exe).args(args).spawn().map(|_| ()).map_err(|e| format!("could not start {}: {e}", exe.display()))
}

/// Close Afterburner, back up the cfg, rewrite slot N with the preset. Returns the backup path.
fn write_preset_slot(p: &Preset) -> Result<PathBuf, String> {
    // Build once before closing Afterburner so a bad curve never leaves it closed (the PS original checks after).
    let pre = Cfg::read().ok_or_else(|| format!("No RTX 5080 profile file found in {}\\Profiles. Open Afterburner once, close it, and retry.", ab_dir().display()))?;
    preset_section_lines(&pre, p)?;
    kill_ab();
    let cfg = Cfg::read().ok_or_else(|| format!("No RTX 5080 profile file found in {}\\Profiles. Open Afterburner once, close it, and retry.", ab_dir().display()))?;
    let dir = backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let fname = cfg.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let backup = dir.join(format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), fname));
    std::fs::copy(&cfg.path, &backup).map_err(|e| format!("backup failed: {e}"))?;
    let section = preset_section_lines(&cfg, p)?;
    let out = Cfg { path: cfg.path.clone(), lines: with_section(&cfg, p.slot, section), crlf: cfg.crlf };
    std::fs::write(&out.path, out.to_bytes()).map_err(|e| format!("could not write {} (run as administrator): {e}", out.path.display()))?;
    Ok(backup)
}

fn last_backup() -> Option<PathBuf> {
    std::fs::read_dir(backup_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("cfg")).unwrap_or(false))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Result of one step of an operation: Ok(note, may be empty) or Err(reason).
type StepResult = Result<String, String>;

const STEP_WRITE: &str = "write";
const STEP_START: &str = "start";
const STEP_CONFIRM: &str = "confirm";
const STEP_TRIAL: &str = "trial";
const STEP_END_TRIAL: &str = "endtrial";
const STEP_COPY: &str = "copy";

/// Put a backup back (runs in a Job); every step is reported as it completes.
fn restore_backup(last: &Path, report: &dyn Fn(&str, StepResult)) {
    let Some(cfg) = cfg_file() else {
        report(STEP_COPY, Err("No RTX 5080 profile file to restore over.".into()));
        return;
    };
    kill_ab();
    if let Err(e) = std::fs::copy(last, &cfg) {
        report(STEP_COPY, Err(format!("could not copy the backup (run as administrator): {e}")));
        return;
    }
    report(STEP_COPY, Ok(String::new()));
    report(STEP_START, launch_ab(&[]).map(|_| String::new()));
}

// ------------------------------------------------------------------ live GPU readout

#[derive(Clone, Debug)]
pub struct Live {
    pub core: i64,
    pub mem: i64,
    pub mem_stock: i64,
    pub mem_offset: i64,
    pub power: f64,
    pub temp: i64,
    pub pstate: String,
    pub util: i64,
    /// memory only runs at full speed in P0
    pub full_clocks: bool,
}

pub fn parse_smi(out: &str) -> Option<Live> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() < 7 {
        return None;
    }
    let int = |s: &str| s.parse::<f64>().ok().map(|v| v.round_ties_even() as i64);
    let mem = int(f[1])?;
    let mem_stock = int(f[2])?;
    let pstate = f[5].to_string();
    Some(Live {
        core: int(f[0]).unwrap_or(0),
        mem,
        mem_stock,
        mem_offset: mem - mem_stock,
        power: f[3].parse().unwrap_or(0.0),
        temp: int(f[4]).unwrap_or(0),
        full_clocks: pstate == "P0" && mem >= mem_stock,
        pstate,
        util: int(f[6]).unwrap_or(0),
    })
}

pub fn gpu_live() -> Option<Live> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let smi = PathBuf::from(root).join(r"System32\nvidia-smi.exe");
    let exe = if smi.exists() { smi.display().to_string() } else { "nvidia-smi".to_string() };
    let (code, out) = common::run(
        &exe,
        &["--query-gpu=clocks.gr,clocks.mem,clocks.max.memory,power.draw,temperature.gpu,pstate,utilization.gpu", "--format=csv,noheader,nounits"],
    )
    .ok()?;
    if code != 0 {
        return None;
    }
    parse_smi(&out)
}

fn live_preset(l: &Live) -> Option<usize> {
    if !l.full_clocks {
        return None;
    }
    PRESETS.iter().position(|p| (l.mem_offset - p.mem as i64).abs() <= 5)
}

pub struct State {
    pub cfg: Option<PathBuf>,
    pub saved: Option<usize>,
    /// does slot N already hold preset i
    pub slot_ok: Vec<bool>,
    pub live: Option<Live>,
    pub live_preset: Option<usize>,
    pub ab_running: bool,
}

pub fn current_state() -> State {
    let cfg = Cfg::read();
    let saved = cfg.as_ref().and_then(|c| find_preset(c.section("Startup").as_ref()));
    let slot_ok = PRESETS
        .iter()
        .map(|p| cfg.as_ref().map(|c| section_is_preset(c.section(&format!("Profile{}", p.slot)).as_ref(), p)).unwrap_or(false))
        .collect();
    let live = gpu_live();
    let live_preset = live.as_ref().and_then(live_preset);
    State { cfg: cfg.map(|c| c.path), saved, slot_ok, live, live_preset, ab_running: ab_running() }
}

/// The preset shown as "currently applied". Under load the live reading decides alone (None = custom
/// settings); the saved [Startup] preset only stands in while the GPU is idle or nvidia-smi is missing.
fn current_preset(s: &State) -> Option<usize> {
    match &s.live {
        Some(l) if l.full_clocks => s.live_preset,
        _ => s.saved,
    }
}

fn wait_for_live(p: &Preset) -> StepResult {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(l) = gpu_live() {
            if l.full_clocks && (l.mem_offset - p.mem as i64).abs() <= 5 {
                return Ok(String::new());
            }
        }
    }
    match gpu_live() {
        Some(l) if !l.full_clocks => Ok("Sent - the GPU is idle, so the live check confirms it once a game is running.".into()),
        _ => Err("Sent, but the live memory offset does not match yet - check Afterburner.".into()),
    }
}

/// Apply a preset (runs in a Job): optionally write its slot first, then tell Afterburner to load it.
/// Every step is reported as it completes. Returns true when Afterburner was started with the preset.
/// `before_start` runs after the optional slot write and right before Afterburner is started (the safety
/// trial is armed there, so a crash at the moment of applying is already covered); `start_failed` runs when
/// Afterburner could not be started.
fn apply_preset(idx: usize, write: bool, report: &dyn Fn(&str, StepResult), before_start: &dyn Fn(), start_failed: &dyn Fn()) -> bool {
    let p = &PRESETS[idx];
    if write {
        match write_preset_slot(p) {
            Ok(backup) => report(STEP_WRITE, Ok(format!("Backup: {}", backup.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))),
            Err(e) => {
                report(STEP_WRITE, Err(e));
                return false;
            }
        }
    }
    before_start();
    if let Err(e) = launch_ab(&[format!("-Profile{}", p.slot)]) {
        report(STEP_START, Err(e));
        start_failed();
        return false;
    }
    report(STEP_START, Ok(String::new()));
    report(STEP_CONFIRM, wait_for_live(p));
    true
}

// ------------------------------------------------------------------ trial + auto-revert
//
// Applying a preset the user has not confirmed yet starts a TRIAL: gpu-trial.json (pending) plus a logon
// task that runs `PCTuner.exe --gpu-guard`. If the PC crashed after the trial started, the guard puts the
// GPU back on Stock at the next sign-in. Keep / Revert / applying Stock end the trial.

const TRIAL_FILE: &str = "gpu-trial.json";
const CONFIRMED_FILE: &str = "gpu-confirmed.json";
const GUARD_LOG: &str = "gpu-guard.log";
const TASK_PATH: &str = r"\PCTuner\";
const TASK_NAME: &str = "GpuTrialGuard";
/// Seconds: Win32_OperatingSystem.LastBootUpTime can shift slightly when the clock is synchronised.
const BOOT_TOLERANCE: i64 = 90;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Trial {
    /// preset being tested
    #[serde(default)]
    pub preset: String,
    /// preset to go back to with "Revert now" (the last confirmed one, or Stock)
    #[serde(default)]
    pub previous: String,
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
}

impl Trial {
    fn pending(&self) -> bool {
        self.status == "pending"
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Confirmed {
    #[serde(default)]
    preset: String,
}

fn load_trial() -> Option<Trial> {
    common::load_json::<Option<Trial>>(TRIAL_FILE)
}

fn delete_trial() {
    let _ = std::fs::remove_file(common::data_dir().join(TRIAL_FILE));
}

fn load_confirmed() -> Option<String> {
    Some(common::load_json::<Confirmed>(CONFIRMED_FILE).preset).filter(|p| !p.is_empty())
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Decision {
    /// no trial file, or the trial is not pending
    NoTrial,
    /// the PC has not restarted since the preset was applied
    StillSameBoot,
    /// restarted without a crash: the trial stays pending
    CleanReboot,
    /// a crash was logged after the preset was applied: go back to Stock
    RevertAfterCrash,
}

/// What the sign-in check does. Pure: all inputs are passed in. Boot times are unix seconds, 0 = unknown.
pub fn guard_decision(trial: Option<&Trial>, current_boot_time: i64, crash_events_after_apply: bool) -> Decision {
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
fn boot_time() -> i64 {
    common::ps("([DateTimeOffset](Get-CimInstance Win32_OperatingSystem).LastBootUpTime).ToUnixTimeSeconds()")
        .ok()
        .and_then(|o| o.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// First crash event in the System log after `since` (unix seconds): Kernel-Power 41, EventLog 6008 or a
/// BugCheck / WER-SystemErrorReporting 1001. Ok(None) = no crash. Read-only.
fn crash_event_after(since: i64) -> Result<Option<String>, String> {
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

/// PowerShell that registers the sign-in check: this exe with --gpu-guard, 30 s after the current user
/// signs in, elevated.
fn register_task_script(exe: &str) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute {exe} -Argument '--gpu-guard'
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$trigger.Delay = 'PT30S'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskPath {path} -TaskName {name} -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null"#,
        exe = common::psq(exe),
        path = common::psq(TASK_PATH),
        name = common::psq(TASK_NAME),
    )
}

fn register_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program's path: {e}"))?;
    common::ps(&register_task_script(&exe.display().to_string())).map(|_| ())
}

fn unregister_task_script() -> String {
    format!(
        "Unregister-ScheduledTask -TaskPath {p} -TaskName {n} -Confirm:$false -ErrorAction SilentlyContinue\nif (Get-ScheduledTask -TaskPath {p} -TaskName {n} -ErrorAction SilentlyContinue) {{ throw 'the sign-in check task is still registered (run as administrator)' }}",
        p = common::psq(TASK_PATH),
        n = common::psq(TASK_NAME),
    )
}

fn unregister_task() -> Result<(), String> {
    common::ps(&unregister_task_script()).map(|_| ())
}

/// Start a trial: register the sign-in check and write gpu-trial.json (pending). The file records whether
/// the check is really armed, so the tab never promises a revert that cannot happen.
fn start_trial(preset: &str, previous: &str) -> StepResult {
    let registered = register_task();
    let trial = Trial {
        preset: preset.to_string(),
        previous: previous.to_string(),
        applied_at: chrono::Utc::now().timestamp(),
        boot_time: boot_time(),
        status: "pending".into(),
        reason: String::new(),
        unguarded: registered.is_err(),
    };
    common::save_json(TRIAL_FILE, &trial);
    if load_trial().as_ref() != Some(&trial) {
        // without the file the sign-in check has nothing to act on: do not leave a useless task behind
        if registered.is_ok() {
            let _ = unregister_task();
        }
        return Err(format!("The trial could not be saved to {} - no automatic revert.", common::data_dir().join(TRIAL_FILE).display()));
    }
    registered.map_err(|e| format!("Trial noted, but the sign-in check could not be registered - no automatic revert: {e}"))?;
    Ok("If the PC crashes, the GPU goes back to Stock at your next sign-in.".into())
}

/// End a pending trial: delete its file and remove the sign-in check. A "reverted" notice is left for the user to read.
fn end_trial() -> StepResult {
    if load_trial().map(|t| t.pending()).unwrap_or(false) {
        delete_trial();
    }
    unregister_task().map(|_| String::new())
}

fn guard_log(line: &str) {
    use std::io::Write;
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(common::data_dir().join(GUARD_LOG)).and_then(|mut f| writeln!(f, "{}  {line}", common::now()));
}

/// Sign-in check of the GPU trial (`PCTuner.exe --gpu-guard [--dry-run]`). Returns the process exit code.
/// `dry_run` only logs the decision; nothing is changed.
pub fn guard_main(dry_run: bool) -> i32 {
    let mode = if dry_run { "dry-run" } else { "guard" };
    let trial = load_trial();
    let Some(t) = trial.as_ref().filter(|t| t.pending()) else {
        let why = match &trial {
            Some(t) => format!("trial status is \"{}\"", t.status),
            None => "no trial file".into(),
        };
        guard_log(&format!("[{mode}] decision={:?} ({why})", guard_decision(trial.as_ref(), 0, false)));
        return 0;
    };
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
    if decision != Decision::RevertAfterCrash || dry_run {
        return 0;
    }

    // Crash during the trial: make sure slot 3 holds Stock (written with a backup if not) and load it.
    let stock = &PRESETS[0];
    let slot_ok = Cfg::read().map(|c| section_is_preset(c.section(&format!("Profile{}", stock.slot)).as_ref(), stock)).unwrap_or(false);
    let report = |id: &str, r: StepResult| guard_log(&format!("[{mode}]   step {id}: {r:?}"));
    if !apply_preset(0, !slot_ok, &report, &|| {}, &|| {}) {
        guard_log(&format!("[{mode}] revert FAILED - the trial stays pending and is checked again at the next sign-in"));
        return 1;
    }
    let mut done = t.clone();
    done.status = "reverted".into();
    done.reason = crash_text;
    common::save_json(TRIAL_FILE, &done);
    match unregister_task() {
        Ok(()) => guard_log(&format!("[{mode}] GPU set back to Stock; sign-in check removed")),
        Err(e) => guard_log(&format!("[{mode}] GPU set back to Stock; sign-in check NOT removed: {e}")),
    }
    0
}

// ------------------------------------------------------------------ tab

/// What happens to the trial once Afterburner has been started with the preset.
#[derive(Clone, Debug, PartialEq)]
enum After {
    Nothing,
    StartTrial { previous: String },
    /// `visible`: shown as a step (a trial is pending); otherwise the task is removed quietly.
    EndTrial { visible: bool },
}

enum Action {
    Apply { idx: usize, write: bool, after: After },
    Restore(PathBuf),
}

/// Trial bookkeeping for applying preset `idx`, given what the user confirmed and whether a trial is pending.
fn plan_after(idx: usize, confirmed: Option<&str>, trial_pending: bool) -> After {
    if idx == 0 {
        After::EndTrial { visible: trial_pending }
    } else if confirmed == Some(PRESETS[idx].name) {
        if trial_pending {
            After::EndTrial { visible: true }
        } else {
            After::Nothing
        }
    } else {
        After::StartTrial { previous: confirmed.unwrap_or(PRESETS[0].name).to_string() }
    }
}

/// The whole Apply operation (runs in a Job).
fn run_apply(idx: usize, write: bool, after: &After, report: &dyn Fn(&str, StepResult)) {
    // A new trial is armed BEFORE Afterburner gets the preset: an unstable tune usually crashes the PC the
    // moment it is applied, and the sign-in check must already exist then.
    let prior = load_trial().filter(|t| t.pending());
    let arm = || {
        if let After::StartTrial { previous } = after {
            report(STEP_TRIAL, start_trial(PRESETS[idx].name, previous));
        }
    };
    // Afterburner never got the preset: no trial for it. An earlier trial that is still pending stays as it was.
    let disarm = || {
        if let After::StartTrial { .. } = after {
            match &prior {
                Some(t) => common::save_json(TRIAL_FILE, t),
                None => {
                    let _ = end_trial();
                }
            }
        }
    };
    if !apply_preset(idx, write, report, &arm, &disarm) {
        return;
    }
    match after {
        After::Nothing | After::StartTrial { .. } => {}
        After::EndTrial { visible } => {
            let r = end_trial();
            if *visible {
                report(STEP_END_TRIAL, r);
            }
        }
    }
}

/// The operation window for applying a preset: one item per step.
fn apply_window(kicker: &str, idx: usize, write: bool, after: &After) -> OpWindow {
    let p = &PRESETS[idx];
    let mut items = vec![];
    if write {
        items.push(OpItem::new(STEP_WRITE, format!("Write preset into slot {} (backup first)", p.slot), ""));
    }
    // armed before Afterburner gets the preset, so a crash at the moment of applying is covered
    if let After::StartTrial { .. } = after {
        items.push(OpItem::new(STEP_TRIAL, "Arm the safety trial (auto-revert after a crash)", ""));
    }
    items.push(OpItem::new(STEP_START, "Start MSI Afterburner with the preset", "").detail(format!("-Profile{}", p.slot)));
    items.push(OpItem::new(STEP_CONFIRM, "Confirm on the GPU (live memory offset)", ""));
    if let After::EndTrial { visible: true } = after {
        items.push(OpItem::new(STEP_END_TRIAL, "End the safety trial (remove the sign-in check)", ""));
    }
    let mut w = OpWindow::new(kicker, "run", "done", items).noun("step", "steps");
    w.show_steps = true;
    w.fail_phrase = "did not complete".into();
    w.preview_text = format!("{}: {}", p.name, p.text);
    w.running_text = format!("Applying {}...", p.name);
    w.go_label = if idx == 0 { "Reset to stock".into() } else { format!("Apply {}", p.name) };
    let mut warn: Vec<String> = vec![];
    if write {
        warn.push(format!(
            "Afterburner slot {} does not hold the {} preset yet. PC Tuner closes Afterburner, backs up its profile file to {} and rewrites the slot.",
            p.slot,
            p.name,
            backup_dir().display()
        ));
    }
    if p.name == "Performance" {
        warn.push("Runs warmest - test stability before long sessions.".into());
    }
    if !warn.is_empty() {
        w.warning = Some(warn.join("\n"));
    }
    w.reassure = match after {
        After::StartTrial { previous } => format!("Starts as a trial: Keep it or Revert to {previous} afterwards. After a crash the GPU goes back to Stock at your next sign-in."),
        _ if idx == 0 => "Stock is the factory behaviour: no undervolt, no memory overclock.".into(),
        _ => "This is the preset you already confirmed.".into(),
    };
    w
}

fn restore_window(last: &Path) -> OpWindow {
    let when = std::fs::metadata(last)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    let name = last.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let items = vec![
        OpItem::new(STEP_COPY, "Close Afterburner and put the backup back", "").detail(name),
        OpItem::new(STEP_START, "Restart MSI Afterburner", ""),
    ];
    let mut w = OpWindow::new("RESTORE LAST BACKUP", "run", "done", items).noun("step", "steps");
    w.show_steps = true;
    w.fail_phrase = "did not complete".into();
    w.preview_text = format!("Puts back the Afterburner profile file saved on {when}.");
    w.running_text = "Restoring the backup...".into();
    w.go_label = "Restore".into();
    w.warning = Some("All Afterburner profile slots go back to what they held when this backup was made.".into());
    w
}

/// A running operation: step results arrive on `rx`, `job` yields when everything is over.
struct Work {
    rx: mpsc::Receiver<(String, StepResult)>,
    job: Job<()>,
}

const HISTORY: usize = 60;

pub struct GpuTab {
    state: Option<State>,
    refresh: Option<Job<State>>,
    next_refresh: Instant,
    /// light ~1 s sampler feeding the sparklines
    sampler: Option<Job<Option<Live>>>,
    next_sample: Instant,
    history: VecDeque<Live>,
    selected: Option<usize>,
    op: Option<OpWindow>,
    op_action: Option<Action>,
    work: Option<Work>,
    /// removing the sign-in check after "Keep"
    chore: Option<Job<Result<(), String>>>,
    status: String,
    error: Option<String>,
    trial: Option<Trial>,
    confirmed: Option<String>,
    /// PCTUNER_DEMO sample data on screen: nothing is written or run
    demo: bool,
}

enum Click {
    Reset,
    Apply,
    Restore,
    Refresh,
    Keep,
    Revert,
    Dismiss,
}

fn hhmm(unix: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(unix, 0).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

fn notice_frame(c: egui::Color32) -> egui::Frame {
    theme::card_frame().fill(theme::tint(c, 20)).stroke(egui::Stroke::new(1.0_f32, theme::tint(c, 130)))
}

/// One live stat with its graph.
fn stat_tile(ui: &mut egui::Ui, key: &str, value: f32, unit: &str, label: &str, series: &[f32]) {
    let v = theme::count_up(ui.ctx(), ("gpu-stat", key), value);
    theme::stat(ui, &format!("{v:.0} {unit}"), label, theme::TEXT);
    let w = (ui.available_width() - 12.0).max(40.0);
    theme::sparkline(ui, series, egui::vec2(w, 38.0), theme::ACCENT);
}

/// A selectable preset card. Returns true when clicked.
fn preset_card(ui: &mut egui::Ui, i: usize, p: &Preset, selected: bool, current: bool, needs_write: bool) -> bool {
    let id = egui::Id::new(("gpu-preset", i));
    let hovered = ui.ctx().read_response(id).map(|r| r.hovered()).unwrap_or(false);
    let t = ui.ctx().animate_bool_with_time(id.with("hover"), hovered, 0.12);
    let fill = theme::SURFACE.lerp_to_gamma(theme::SURFACE2, t);
    let stroke = if selected { egui::Stroke::new(1.5_f32, theme::ACCENT) } else { egui::Stroke::new(1.0_f32, theme::BORDER) };
    let r = theme::card_frame().fill(fill).stroke(stroke).show(ui, |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        ui.set_width(ui.available_width());
        ui.set_min_height(148.0);
        // columns justify their content; inside the card text is plain left-aligned
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.name).size(18.0).strong().color(if selected { theme::ACCENT } else { theme::TEXT }));
                if current {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::chip(ui, "current", theme::OK));
                }
            });
            ui.label(RichText::new(format!("Afterburner slot {}", p.slot)).size(12.0).color(theme::TEXT_FAINT));
            ui.label(RichText::new(p.text).color(theme::TEXT_DIM));
            if needs_write {
                ui.label(RichText::new("Slot needs writing (backup first)").size(12.0).color(theme::GAMING));
            }
        });
    });
    ui.interact(r.response.rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

impl GpuTab {
    pub fn new() -> Self {
        let mut tab = GpuTab {
            state: None,
            refresh: None,
            next_refresh: Instant::now(),
            sampler: None,
            next_sample: Instant::now(),
            history: VecDeque::with_capacity(HISTORY),
            selected: None,
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
    fn demo_setup(&mut self) {
        let v = std::env::var("PCTUNER_DEMO").unwrap_or_default().to_lowercase();
        let sample = |status: &str, reason: &str| Trial {
            preset: "Performance".into(),
            previous: "Balanced".into(),
            applied_at: chrono::Utc::now().timestamp() - 25 * 60,
            boot_time: 0,
            status: status.into(),
            reason: reason.into(),
            unguarded: false,
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
                let mut w = apply_window("APPLY PERFORMANCE", 3, true, &After::StartTrial { previous: "Balanced".into() });
                match stage {
                    Stage::Preview => {}
                    Stage::Running => {
                        w.items[0].result = Some(Ok(String::new()));
                        w.items[1].result = Some(Ok(String::new()));
                        w.stage = Stage::Running;
                    }
                    Stage::Done => {
                        w.finish(|it| match it.id.as_str() {
                            STEP_WRITE => Some(Ok("Backup: 20261111-214512-VEN_10DE&DEV_2C02&SUBSYS_53521458&REV_A1&BUS_1&DEV_0&FN_0.cfg".into())),
                            STEP_CONFIRM => Some(Err("Sent, but the live memory offset does not match yet - check Afterburner.".into())),
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

    fn reload_trial(&mut self) {
        if !self.demo {
            self.trial = load_trial();
            self.confirmed = load_confirmed();
        }
    }

    fn pump(&mut self) {
        if let Some(j) = &self.refresh {
            if let Some(s) = j.poll() {
                if self.selected.is_none() {
                    self.selected = s.live_preset.or(s.saved);
                }
                self.state = Some(s);
                self.refresh = None;
                self.next_refresh = Instant::now() + Duration::from_secs(3);
            }
        }
        if self.refresh.is_none() && Instant::now() >= self.next_refresh {
            self.refresh = Some(Job::spawn(current_state));
        }

        // light sampler: one nvidia-smi reading about once per second
        if let Some(j) = &self.sampler {
            if let Some(l) = j.poll() {
                if let Some(l) = l {
                    if self.history.len() >= HISTORY {
                        self.history.pop_front();
                    }
                    self.history.push_back(l);
                }
                self.sampler = None;
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
                    if ok(STEP_TRIAL) && ok(STEP_START) {
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
                    self.error = Some(format!("The sign-in check could not be removed:\n{e}"));
                }
                self.chore = None;
            }
        }
    }

    /// Open the operation window (preview) for applying preset `idx`.
    fn open_apply(&mut self, idx: usize, kicker: String) {
        let p = &PRESETS[idx];
        let Some(cfg) = Cfg::read() else {
            self.error = Some(format!("No RTX 5080 profile file in {}\\Profiles. Open Afterburner once, close it, and retry.", ab_dir().display()));
            return;
        };
        let slot_ok = section_is_preset(cfg.section(&format!("Profile{}", p.slot)).as_ref(), p);
        if !slot_ok {
            if let Err(e) = preset_section_lines(&cfg, p) {
                self.error = Some(format!("Could not apply {}:\n{e}", p.name));
                return;
            }
        }
        let pending = self.trial.as_ref().map(|t| t.pending()).unwrap_or(false);
        let after = plan_after(idx, self.confirmed.as_deref(), pending);
        self.op = Some(apply_window(&kicker, idx, !slot_ok, &after));
        self.op_action = Some(Action::Apply { idx, write: !slot_ok, after });
    }

    /// "Go" was clicked in the operation window: start the background work.
    fn go(&mut self) {
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
                Action::Apply { idx, write, after } => run_apply(idx, write, &after, &report),
                Action::Restore(path) => restore_backup(&path, &report),
            }
        });
        self.work = Some(Work { rx, job });
    }

    fn click(&mut self, c: Click) {
        match c {
            Click::Reset => self.open_apply(0, "RESET GPU TO STOCK".into()),
            Click::Apply => match self.selected {
                Some(idx) => self.open_apply(idx, format!("APPLY {}", PRESETS[idx].name.to_uppercase())),
                None => self.status = "Pick a preset first.".into(),
            },
            Click::Restore => match last_backup() {
                None => self.status = format!("No backups in {}.", backup_dir().display()),
                Some(last) => {
                    self.op = Some(restore_window(&last));
                    self.op_action = Some(Action::Restore(last));
                }
            },
            Click::Refresh => self.next_refresh = Instant::now(),
            Click::Keep => {
                let Some(t) = self.trial.take() else { return };
                self.status = format!("{} kept as your confirmed tune.", t.preset);
                if self.demo {
                    return;
                }
                common::save_json(CONFIRMED_FILE, &Confirmed { preset: t.preset.clone() });
                self.confirmed = Some(t.preset);
                delete_trial();
                self.chore = Some(Job::spawn(unregister_task));
            }
            Click::Revert => {
                let prev = self.trial.as_ref().map(|t| t.previous.clone()).unwrap_or_default();
                let idx = PRESETS.iter().position(|p| p.name == prev).unwrap_or(0);
                self.open_apply(idx, format!("REVERT TO {}", PRESETS[idx].name.to_uppercase()));
            }
            Click::Dismiss => {
                self.trial = None;
                if !self.demo {
                    delete_trial();
                }
            }
        }
    }

    /// Trial / crash notice at the top of the tab.
    fn trial_banner(&self, ui: &mut egui::Ui, can: bool, click: &mut Option<Click>) {
        let Some(t) = &self.trial else { return };
        if t.pending() {
            notice_frame(theme::GAMING).show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Safety trial");
                ui.label(RichText::new(format!("Testing {} - keep this tune?", t.preset)).size(18.0).strong().color(theme::TEXT));
                if t.unguarded {
                    ui.label(RichText::new(format!("Applied at {}.", hhmm(t.applied_at))).color(theme::TEXT_DIM));
                    theme::banner(ui, theme::LOCKED, "Automatic revert is NOT armed (the sign-in check could not be registered) - revert manually if it is unstable.");
                } else {
                    ui.label(
                        RichText::new(format!(
                            "Applied at {}. If the PC crashes while you test it, the GPU is set back to Stock at your next sign-in.",
                            hhmm(t.applied_at)
                        ))
                        .color(theme::TEXT_DIM),
                    );
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, theme::primary_button("   Keep   ")).on_hover_text("Stable: make this the confirmed tune and end the trial.").clicked() {
                        *click = Some(Click::Keep);
                    }
                    if ui.add_enabled(self.work.is_none(), egui::Button::new("Revert now")).on_hover_text(format!("Go back to {}.", t.previous)).clicked() {
                        *click = Some(Click::Revert);
                    }
                    ui.label(RichText::new(format!("Revert goes back to {}.", t.previous)).size(12.0).color(theme::TEXT_FAINT));
                });
            });
            ui.add_space(4.0);
        } else if t.status == "reverted" {
            notice_frame(theme::LOCKED).show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Trial reverted");
                ui.label(RichText::new(format!("Unexpected shutdown during the {} trial", t.preset)).size(18.0).strong().color(theme::TEXT));
                ui.label(
                    RichText::new(format!(
                        "Windows logged an unexpected shutdown during the {} trial, so the GPU was set back to Stock at sign-in. If it was not a power cut, try a milder preset.",
                        t.preset
                    ))
                    .color(theme::TEXT_DIM),
                );
                if !t.reason.is_empty() {
                    ui.label(RichText::new(format!("Windows logged: {}", t.reason)).size(12.0).color(theme::TEXT_FAINT));
                }
                ui.add_space(2.0);
                if ui.button("   Dismiss   ").clicked() {
                    *click = Some(Click::Dismiss);
                }
            });
            ui.add_space(4.0);
        }
    }

    /// The most important thing on this screen: which preset is applied, with live graphs.
    fn hero_card(&self, ui: &mut egui::Ui, current: Option<usize>, click: &mut Option<Click>) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(s) = &self.state else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reading the GPU...").color(theme::TEXT_DIM));
                });
                return;
            };
            theme::section(ui, "Currently applied");
            ui.horizontal(|ui| {
                let (name, c) = match current {
                    Some(i) => (PRESETS[i].name, theme::ACCENT),
                    None => ("Custom", theme::JUNK),
                };
                let saved = s.saved.map(|i| PRESETS[i].name).unwrap_or("custom, not a preset");
                theme::hero(ui, name, if current.is_some() { "preset" } else { "settings" }, &format!("Afterburner saved settings: {saved}"), c);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui
                        .add_enabled(self.work.is_none(), theme::danger_button("⟲  Reset to stock"))
                        .on_hover_text("Puts the RTX 5080 back on the Stock preset: no undervolt, no memory overclock.")
                        .clicked()
                    {
                        *click = Some(Click::Reset);
                    }
                });
            });

            let live = self.history.back().or(s.live.as_ref());
            ui.horizontal_wrapped(|ui| {
                match (live, s.live_preset) {
                    (Some(_), Some(i)) => theme::chip(ui, &format!("Live check: memory offset matches {}", PRESETS[i].name), theme::OK),
                    (Some(l), None) if l.full_clocks => theme::chip(ui, "Live check: memory offset matches no preset", theme::JUNK),
                    (Some(l), None) => theme::chip(ui, &format!("Live check: GPU idle in {} - confirms under load", l.pstate), theme::DIM),
                    (None, _) => theme::chip(ui, "Live check: nvidia-smi not available", theme::DIM),
                }
                if s.ab_running {
                    theme::chip(ui, "Afterburner running", theme::OK);
                } else {
                    theme::chip(ui, "Afterburner not running - presets only hold while it runs", theme::JUNK);
                }
                if let Some(l) = live.filter(|l| l.full_clocks) {
                    theme::chip(ui, &format!("Memory {} MHz ({:+} over stock {})", l.mem, l.mem_offset, l.mem_stock), theme::DIM);
                }
            });
            if s.cfg.is_none() {
                theme::banner(ui, theme::LOCKED, &format!("No RTX 5080 profile file (VEN_10DE&DEV_2C02*.cfg) in {}\\Profiles.", ab_dir().display()));
            }

            if let Some(l) = live {
                ui.add_space(6.0);
                let series = |f: &dyn Fn(&Live) -> f32| -> Vec<f32> { self.history.iter().map(f).collect() };
                let (core, temp, power, util) = (series(&|l| l.core as f32), series(&|l| l.temp as f32), series(&|l| l.power as f32), series(&|l| l.util as f32));
                ui.columns(4, |c| {
                    stat_tile(&mut c[0], "core", l.core as f32, "MHz", "Core clock", &core);
                    stat_tile(&mut c[1], "temp", l.temp as f32, "°C", "Temperature", &temp);
                    stat_tile(&mut c[2], "power", l.power as f32, "W", "Power draw", &power);
                    stat_tile(&mut c[3], "util", l.util as f32, "%", "GPU load", &util);
                });
            }
        });
    }
}

impl Tab for GpuTab {
    fn title(&self) -> &'static str {
        "RTX 5080 Profiles"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.pump();
        let ctx = ui.ctx().clone();
        let busy = self.work.is_some();
        let can = common::is_admin() && !busy && !self.demo;

        // the sidebar's "Reset GPU to stock" button
        if common::GPU_RESET_REQUEST.swap(false, std::sync::atomic::Ordering::Relaxed) && !busy {
            self.click(Click::Reset);
        }

        match self.op.as_mut().and_then(|op| op.show(&ctx, can)) {
            Some(OpEvent::Go) => self.go(),
            Some(OpEvent::Close) => {
                self.op = None;
                self.op_action = None;
            }
            _ => {}
        }
        common::message_ui(&ctx, "RTX 5080 Profiles", &mut self.error);

        let mut click: Option<Click> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            theme::page_header(ui, "RTX 5080 Profiles", "Switch the card between undervolt presets stored in MSI Afterburner, and see what is applied right now.");
            common::admin_banner(ui);
            self.trial_banner(ui, can, &mut click);

            let st = self.state.as_ref();
            let current = st.and_then(current_preset);
            self.hero_card(ui, current, &mut click);

            ui.add_space(8.0);
            theme::section(ui, "Presets");
            let mut pick = None;
            ui.columns(PRESETS.len(), |cols| {
                for (i, p) in PRESETS.iter().enumerate() {
                    let needs_write = st.map(|s| s.cfg.is_some() && !s.slot_ok[i]).unwrap_or(false);
                    if preset_card(&mut cols[i], i, p, self.selected == Some(i), current == Some(i), needs_write) {
                        pick = Some(i);
                    }
                }
            });
            if pick.is_some() {
                self.selected = pick;
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = match self.selected {
                    Some(i) => format!("   Apply {}   ", PRESETS[i].name),
                    None => "   Apply preset   ".into(),
                };
                if ui.add_enabled(!busy && self.selected.is_some(), theme::primary_button(&label)).clicked() {
                    click = Some(Click::Apply);
                }
                if ui.add_enabled(!busy, egui::Button::new("Restore last backup")).clicked() {
                    click = Some(Click::Restore);
                }
                if ui.add_enabled(self.refresh.is_none(), egui::Button::new("Refresh")).clicked() {
                    click = Some(Click::Refresh);
                }
                if !self.status.is_empty() {
                    ui.label(RichText::new(&self.status).color(theme::TEXT_DIM));
                }
            });
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "A preset you have not kept yet starts as a trial. The automatic revert runs at your next sign-in after a crash (it cannot act during a freeze) and needs MSI Afterburner.",
                )
                .size(12.0)
                .color(theme::TEXT_DIM),
            );
        });

        if let Some(c) = click {
            self.click(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smi_parse() {
        let l = parse_smi("2895, 17001, 15001, 310.52, 61, P0, 97\r\n").unwrap();
        assert_eq!(l.mem_offset, 2000);
        assert!(l.full_clocks);
        assert_eq!(live_preset(&l), Some(3));
        let idle = parse_smi("210, 405, 15001, 25.10, 40, P8, 2").unwrap();
        assert!(!idle.full_clocks);
    }

    /// Read-only: parses the real Afterburner profile, builds every preset in memory (never written) and
    /// prints the live nvidia-smi reading (`cargo test gpu -- --nocapture`).
    #[test]
    fn gpu_cfg_live() {
        println!("Afterburner: {}  (exe exists: {})", ab_dir().display(), ab_exe().exists());
        println!("backup dir:  {}", backup_dir().display());
        let Some(cfg) = Cfg::read() else {
            println!("no RTX 5080 profile file found - skipping cfg checks");
            return;
        };
        println!("cfg: {}  ({} lines, CRLF: {})", cfg.path.display(), cfg.lines.len(), cfg.crlf);
        // the file must round-trip byte for byte when nothing is changed
        let orig = std::fs::read(&cfg.path).unwrap();
        println!("unchanged re-serialisation identical: {}", orig == cfg.to_bytes());
        for name in cfg.section_names() {
            let sec = cfg.section(&name);
            let s = sec.as_ref().unwrap();
            let curve = s.get("VFCurve").and_then(decode_curve);
            let max_eff = curve.as_ref().map(|c| c.points.iter().map(|p| p.base + p.ofs).fold(f32::MIN, f32::max));
            println!(
                "  [{name}]  matches: {:<12} MemClkBoost={:?} PowerLimit={:?} CoreClkBoost={:?} curve: {}",
                find_preset(sec.as_ref()).map(|i| PRESETS[i].name).unwrap_or("-"),
                s.get("MemClkBoost"),
                s.get("PowerLimit"),
                s.get("CoreClkBoost"),
                match (&curve, max_eff) {
                    (Some(c), Some(m)) => format!("{} pts, max {:.0} MHz", c.points.len(), m),
                    _ => "none".into(),
                }
            );
        }
        for (i, p) in PRESETS.iter().enumerate() {
            match preset_section_lines(&cfg, p) {
                Ok(lines) => {
                    let sec = Section::from_lines(&lines[1..]);
                    let back = find_preset(Some(&sec));
                    let slot_now = section_is_preset(cfg.section(&format!("Profile{}", p.slot)).as_ref(), p);
                    // also check the rewritten file parses back to the same section
                    let new_cfg = Cfg { path: cfg.path.clone(), lines: with_section(&cfg, p.slot, lines.clone()), crlf: cfg.crlf };
                    let reparsed = Cfg::parse(cfg.path.clone(), &new_cfg.to_bytes());
                    let back2 = find_preset(reparsed.section(&format!("Profile{}", p.slot)).as_ref());
                    println!(
                        "  build {:<12} -> slot {}  round-trip: {:?} / via file: {:?}   slot already holds it: {}   ({})",
                        p.name,
                        p.slot,
                        back.map(|j| PRESETS[j].name),
                        back2.map(|j| PRESETS[j].name),
                        slot_now,
                        lines[1..].iter().filter(|l| !l.starts_with("VFCurve")).cloned().collect::<Vec<_>>().join(" ")
                    );
                    assert_eq!(back, Some(i));
                    assert_eq!(back2, Some(i));
                }
                Err(e) => println!("  build {:<12} FAILED: {e}", p.name),
            }
        }
        match gpu_live() {
            Some(l) => println!("live: {:?}\n  -> live preset: {:?}", l, live_preset(&l).map(|i| PRESETS[i].name)),
            None => println!("live: nvidia-smi not available"),
        }
        println!("Afterburner running: {}", ab_running());
    }

    fn trial(status: &str, boot: i64) -> Trial {
        Trial { preset: "Performance".into(), previous: "Balanced".into(), applied_at: 1_800_000_000, boot_time: boot, status: status.into(), reason: String::new(), unguarded: false }
    }

    #[test]
    fn gpu_guard_decision() {
        let boot = 1_799_990_000;
        // no file, or a trial that is not pending
        assert_eq!(guard_decision(None, boot, true), Decision::NoTrial);
        assert_eq!(guard_decision(Some(&trial("reverted", boot)), boot + 5000, true), Decision::NoTrial);
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
        let t = Trial { reason: "event 41 (Microsoft-Windows-Kernel-Power) at 2026-11-11 21:47:03".into(), ..trial("reverted", 1_799_990_000) };
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
        assert_eq!(plan_after(2, Some("Balanced"), false), After::Nothing);
        assert_eq!(plan_after(2, Some("Balanced"), true), After::EndTrial { visible: true });
        assert_eq!(plan_after(3, Some("Balanced"), false), After::StartTrial { previous: "Balanced".into() });
        assert_eq!(plan_after(1, None, false), After::StartTrial { previous: "Stock".into() });

        let w = apply_window("APPLY PERFORMANCE", 3, true, &After::StartTrial { previous: "Balanced".into() });
        assert_eq!(w.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![STEP_WRITE, STEP_TRIAL, STEP_START, STEP_CONFIRM]);
        assert!(w.items.iter().all(|i| i.group.is_empty()));
        assert!(w.show_steps && w.stage == Stage::Preview);

        // under load the live reading alone decides what is "currently applied"
        let live = |line: &str| parse_smi(line).unwrap();
        let state = |l: Option<Live>| State { cfg: None, saved: Some(2), slot_ok: vec![], live_preset: l.as_ref().and_then(live_preset), live: l, ab_running: true };
        assert_eq!(current_preset(&state(Some(live("2895, 17001, 15001, 310, 61, P0, 97")))), Some(3));
        assert_eq!(current_preset(&state(Some(live("2895, 15700, 15001, 310, 61, P0, 97")))), None);
        assert_eq!(current_preset(&state(Some(live("210, 405, 15001, 25, 40, P8, 2")))), Some(2));
        assert_eq!(current_preset(&state(None)), Some(2));
        // older trial files have no "unguarded" field: they count as armed
        assert!(!serde_json::from_str::<Trial>(r#"{"preset":"Balanced","status":"pending"}"#).unwrap().unguarded);
        let warn = w.warning.unwrap();
        assert!(warn.contains("slot 4") && warn.contains("Runs warmest"));
        let w = apply_window("RESET GPU TO STOCK", 0, false, &After::EndTrial { visible: false });
        assert_eq!(w.items.len(), 2);
        assert!(w.warning.is_none());

        // the task scripts (never run by the tests)
        let s = register_task_script(r"C:\Apps\It's\PCTuner.exe");
        assert!(s.contains(r"-Execute 'C:\Apps\It''s\PCTuner.exe' -Argument '--gpu-guard'"));
        assert!(s.contains("-AtLogOn") && s.contains("PT30S") && s.contains("-RunLevel Highest") && s.contains("-Force"));
        assert!(s.contains(r"-TaskPath '\PCTuner\' -TaskName 'GpuTrialGuard'"));
        assert!(unregister_task_script().starts_with(r"Unregister-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'GpuTrialGuard'"));
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
}
