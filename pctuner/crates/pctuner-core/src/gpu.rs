//! RTX 5080 Profiles - port of RTX5080-ProfileManager.ps1 (curve building from RTX5080-Profile.ps1).
//! Switches the card between undervolt presets stored in MSI Afterburner profile slots and shows which one is
//! applied right now (live: memory offset from nvidia-smi; saved: the [Startup] section Afterburner restores).
//!
//! Logic only: the Afterburner profile file, the presets, the live readout, the trial with its
//! sign-in guard (`guard_main`, run by `--gpu-guard`) and the page state. The UI draws `Gpu` and
//! forwards the operation window's events to `on_event`.

use crate::common::{self, Job, Tone};
use crate::opwin::{OpEvent, OpItem, OpWindow, Stage};
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
pub const CLOCK_TOLERANCE: f64 = 30.0;

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

/// Afterburner's per-card file names start with this (VEN_10DE = NVIDIA, DEV_2C02 = RTX 5080).
pub const CARD_PREFIX: &str = "VEN_10DE&DEV_2C02";

/// "VEN_10DE&DEV_2C02&SUBSYS_89D71043&REV_A1" for every RTX 5080 in Win32_VideoController's PNPDeviceID lines
/// ("PCI\VEN_10DE&DEV_2C02&SUBSYS_89D71043&REV_A1\6&...").
pub fn card_ids(pnp: &str) -> Vec<String> {
    pnp.lines()
        .filter_map(|l| {
            let l = l.trim().to_uppercase();
            let id = l.strip_prefix("PCI\\")?.split('\\').next()?.to_string();
            id.starts_with(CARD_PREFIX).then_some(id)
        })
        .collect()
}

/// The RTX 5080 cards in this PC right now (read once per run - the card does not change while PC Tuner runs).
pub fn present_cards() -> &'static [String] {
    static CARDS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CARDS.get_or_init(|| common::ps("Get-CimInstance Win32_VideoController | ForEach-Object { $_.PNPDeviceID }").map(|o| card_ids(&o)).unwrap_or_default())
}

/// Which Profiles\VEN_10DE&DEV_2C02*.cfg belongs to the installed card. Afterburner keeps one file per card it
/// has ever seen, so after a card swap the old card's file is still there (and may sort first by name): the file
/// named after a present card wins, otherwise the one Afterburner saved last.
pub fn pick_cfg(files: &[(PathBuf, std::time::SystemTime)], cards: &[String]) -> Option<PathBuf> {
    let name = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_uppercase();
    let rtx: Vec<&(PathBuf, std::time::SystemTime)> = files.iter().filter(|(p, _)| { let n = name(p); n.starts_with(CARD_PREFIX) && n.ends_with(".CFG") }).collect();
    for card in cards {
        let want = format!("{card}&");
        if let Some((p, _)) = rtx.iter().find(|(p, _)| name(p).starts_with(&want)) {
            return Some(p.clone());
        }
    }
    rtx.iter().max_by_key(|(_, t)| *t).map(|(p, _)| p.clone())
}

/// The installed RTX 5080's Afterburner profile file (see pick_cfg).
pub fn cfg_file() -> Option<PathBuf> {
    let files: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(ab_dir().join("Profiles"))
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| (e.path(), e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)))
        .collect();
    pick_cfg(&files, present_cards())
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

    pub fn header_index(&self, name: &str) -> Option<usize> {
        let h = format!("[{name}]");
        self.lines.iter().position(|l| *l == h)
    }

    /// Body lines of [name] up to the next line starting with '['.
    pub fn section_range(&self, name: &str) -> Option<(usize, usize)> {
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

pub fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn u32_at(b: &[u8], o: usize) -> u32 {
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
pub fn ps_int(v: Option<&str>) -> Option<i64> {
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

/// Afterburner only fills [Startup] while "apply overclocking at system startup" is on (EraseStartupSettings=1
/// leaves it empty), so a new card may have no curve anywhere until a profile slot is saved.
pub const NO_CURVE: &str = "Afterburner has not saved this card's voltage/frequency curve yet. Open Afterburner, leave every setting at stock, click Save and then any profile slot (1-5), close Afterburner and retry.";

/// Sections a stock curve can be read from, in order of preference. Every curve point stores the card's own stock
/// clock (base) apart from the offset, so any saved section gives the stock curve.
pub const CURVE_SOURCES: [&str; 6] = ["Startup", "Profile1", "Profile2", "Profile3", "Profile4", "Profile5"];

/// The card's stock curve: from [Startup] when Afterburner keeps one, otherwise from the first saved profile slot.
pub fn stock_curve(cfg: &Cfg) -> Result<Curve, String> {
    let saved: Vec<String> = CURVE_SOURCES.iter().filter_map(|n| cfg.section(n)?.get("VFCurve").filter(|v| !v.is_empty()).map(str::to_string)).collect();
    if saved.is_empty() {
        return Err(NO_CURVE.into());
    }
    saved.iter().find_map(|v| decode_curve(v)).ok_or_else(|| "Unexpected curve format in the Afterburner profile. Not touching anything.".into())
}

/// Build a preset's [ProfileN] section from the card's own stock curve (see stock_curve).
pub fn preset_section_lines(cfg: &Cfg, p: &Preset) -> Result<Vec<String>, String> {
    let c = stock_curve(cfg)?;
    let startup = cfg.section("Startup");
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
        if let Some(v) = startup.as_ref().and_then(|s| s.get(k)) {
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

/// Close Afterburner. Returns true when it was running (and so has to be started again if the change fails).
pub fn kill_ab() -> bool {
    if ab_running() {
        let _ = common::run("taskkill", &["/F", "/IM", "MSIAfterburner.exe"]);
        std::thread::sleep(Duration::from_secs(2));
        true
    } else {
        false
    }
}

pub fn launch_ab(args: &[String]) -> Result<(), String> {
    let exe = ab_exe();
    std::process::Command::new(&exe).args(args).spawn().map(|_| ()).map_err(|e| format!("could not start {}: {e}", exe.display()))
}

/// What to add to the error when a change failed after Afterburner was closed. `relaunch`: start it again
/// unchanged (the GUI); otherwise it is left closed (the sign-in guard: Afterburner's own startup settings
/// may hold the very trial preset that crashed the PC).
pub fn after_failed_change(was_running: bool, relaunch: bool) -> String {
    if !was_running {
        String::new()
    } else if !relaunch {
        " Afterburner was left closed so the trial preset is not re-applied.".into()
    } else {
        match launch_ab(&[]) {
            Ok(()) => " Afterburner was restarted unchanged.".into(),
            Err(e) => format!(" Afterburner could not be restarted - start it manually: {e}."),
        }
    }
}

/// Close Afterburner, back up the cfg, rewrite slot N with the preset. Returns the backup path.
/// `relaunch_on_error`: start Afterburner again unchanged when a step after closing it fails.
pub fn write_preset_slot(p: &Preset, relaunch_on_error: bool) -> Result<PathBuf, String> {
    let missing = || format!("No RTX 5080 profile file found in {}\\Profiles. Open Afterburner once, close it, and retry.", ab_dir().display());
    // Build once before closing Afterburner so a bad curve never leaves it closed (the PS original checks after).
    let pre = Cfg::read().ok_or_else(missing)?;
    preset_section_lines(&pre, p)?;
    let dir = backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let was_running = kill_ab();
    let write = || -> Result<PathBuf, String> {
        // read again: Afterburner may have saved the file while closing
        let cfg = Cfg::read().ok_or_else(missing)?;
        let fname = cfg.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let backup = dir.join(format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), fname));
        std::fs::copy(&cfg.path, &backup).map_err(|e| format!("backup failed: {e}"))?;
        let section = preset_section_lines(&cfg, p)?;
        let out = Cfg { path: cfg.path.clone(), lines: with_section(&cfg, p.slot, section), crlf: cfg.crlf };
        std::fs::write(&out.path, out.to_bytes()).map_err(|e| format!("could not write {} (run as administrator): {e}", out.path.display()))?;
        Ok(backup)
    };
    write().map_err(|e| e + &after_failed_change(was_running, relaunch_on_error))
}

pub fn last_backup() -> Option<PathBuf> {
    std::fs::read_dir(backup_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("cfg")).unwrap_or(false))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Result of one step of an operation: Ok(note, may be empty) or Err(reason).
pub type StepResult = Result<String, String>;

pub const STEP_WRITE: &str = "write";
pub const STEP_START: &str = "start";
pub const STEP_CONFIRM: &str = "confirm";
pub const STEP_TRIAL: &str = "trial";
pub const STEP_END_TRIAL: &str = "endtrial";
pub const STEP_COPY: &str = "copy";

/// Put a backup back (runs in a Job); every step is reported as it completes. `stop_trial`: a safety trial
/// is open and ends once the restored profiles are running (it stays as it is when a step fails).
pub fn restore_backup(last: &Path, stop_trial: bool, report: &dyn Fn(&str, StepResult)) {
    let Some(cfg) = cfg_file() else {
        report(STEP_COPY, Err("No RTX 5080 profile file to restore over.".into()));
        return;
    };
    let was_running = kill_ab();
    if let Err(e) = std::fs::copy(last, &cfg) {
        report(STEP_COPY, Err(format!("could not copy the backup (run as administrator): {e}.{}", after_failed_change(was_running, true))));
        return;
    }
    report(STEP_COPY, Ok(String::new()));
    let started = launch_ab(&[]).map(|_| String::new());
    let ok = started.is_ok();
    report(STEP_START, started);
    if ok && stop_trial {
        report(STEP_END_TRIAL, end_trial());
    }
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

pub fn live_preset(l: &Live) -> Option<usize> {
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
/// `live` is the reading on screen (the newest sample), so the hero and the live-check chip always agree.
pub fn current_preset(saved: Option<usize>, live: Option<&Live>) -> Option<usize> {
    match live {
        Some(l) if l.full_clocks => live_preset(l),
        _ => saved,
    }
}

/// How sure we are that Afterburner really loaded a preset.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Applied {
    /// Afterburner was not started with it (or the slot could not be written)
    Failed,
    /// the GPU runs at full clocks with the preset's memory offset
    Confirmed,
    /// sent, but the GPU is idle or nvidia-smi gave no reading: nothing to compare yet
    Unverified,
    /// the GPU runs at full clocks with a different memory offset
    Mismatch,
}

/// Compare one nvidia-smi reading with the preset.
pub fn live_verdict(l: Option<&Live>, p: &Preset) -> Applied {
    match l {
        Some(l) if l.full_clocks && (l.mem_offset - p.mem as i64).abs() <= 5 => Applied::Confirmed,
        Some(l) if l.full_clocks => Applied::Mismatch,
        _ => Applied::Unverified,
    }
}

pub fn wait_for_live(p: &Preset) -> (Applied, StepResult) {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(1));
        if live_verdict(gpu_live().as_ref(), p) == Applied::Confirmed {
            return (Applied::Confirmed, Ok(String::new()));
        }
    }
    let l = gpu_live();
    match (live_verdict(l.as_ref(), p), &l) {
        (Applied::Confirmed, _) => (Applied::Confirmed, Ok(String::new())),
        (Applied::Unverified, Some(_)) => (Applied::Unverified, Ok("Sent - the GPU is idle, so the live check confirms it once a game is running.".into())),
        (Applied::Unverified, None) => (Applied::Unverified, Err("Sent, but the live memory offset could not be read - check Afterburner.".into())),
        (v, _) => (v, Err("Sent, but the live memory offset does not match yet - check Afterburner.".into())),
    }
}

/// Apply a preset (runs in a Job): optionally write its slot first, then tell Afterburner to load it.
/// Every step is reported as it completes. Returns Failed when Afterburner was not started with the preset,
/// otherwise what the live check saw.
/// `before_start` runs after the optional slot write and right before Afterburner is started (the safety
/// trial is armed there, so a crash at the moment of applying is already covered); `start_failed` runs when
/// Afterburner could not be started. `relaunch_on_error`: see write_preset_slot.
pub fn apply_preset(idx: usize, write: bool, relaunch_on_error: bool, report: &dyn Fn(&str, StepResult), before_start: &dyn Fn(), start_failed: &dyn Fn()) -> Applied {
    let p = &PRESETS[idx];
    if write {
        match write_preset_slot(p, relaunch_on_error) {
            Ok(backup) => report(STEP_WRITE, Ok(format!("Backup: {}", backup.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))),
            Err(e) => {
                report(STEP_WRITE, Err(e));
                return Applied::Failed;
            }
        }
    }
    before_start();
    if let Err(e) = launch_ab(&[format!("-Profile{}", p.slot)]) {
        report(STEP_START, Err(e));
        start_failed();
        return Applied::Failed;
    }
    report(STEP_START, Ok(String::new()));
    let (verdict, r) = wait_for_live(p);
    report(STEP_CONFIRM, r);
    verdict
}

// ------------------------------------------------------------------ trial + auto-revert
//
// Applying a preset the user has not confirmed yet starts a TRIAL: gpu-trial.json (pending) plus a logon
// task that runs `PCTuner.exe --gpu-guard`. If the PC crashed after the trial started, the guard puts the
// GPU back on Stock at the next sign-in. Keep / Revert / applying Stock end the trial.

pub const TRIAL_FILE: &str = "gpu-trial.json";
pub const CONFIRMED_FILE: &str = "gpu-confirmed.json";
pub const GUARD_LOG: &str = "gpu-guard.log";
pub const TASK_PATH: &str = r"\PCTuner\";
pub const TASK_NAME: &str = "GpuTrialGuard";
/// Seconds: Win32_OperatingSystem.LastBootUpTime can shift slightly when the clock is synchronised.
pub const BOOT_TOLERANCE: i64 = 90;

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
    /// "reverted" only: the guard saw Stock confirmed (live reading or Afterburner's saved settings).
    /// false = Stock was sent but not confirmed, and the sign-in check stays to try again.
    #[serde(default)]
    pub verified: bool,
    /// "reverted" only: how many times the sign-in check has sent Stock for this trial
    #[serde(default)]
    pub attempts: u32,
    /// "reverted" only: Stock actually reached Afterburner at one attempt at least (false = every attempt failed
    /// to load it, so the GPU may still run the crashing preset)
    #[serde(default)]
    pub sent: bool,
    /// "reverted" only: how many of the attempts actually handed Stock to Afterburner
    #[serde(default)]
    pub sends: u32,
    /// "reverted" only: Stock was sent MAX_REVERT_ATTEMPTS times without confirmation and the sign-in check stopped
    #[serde(default)]
    pub gave_up: bool,
    /// "reverted" only: what went wrong at the last attempt (empty when Stock was confirmed)
    #[serde(default)]
    pub last_error: String,
}

/// The sign-in check sends Stock at most this many times after a crash, then removes itself and says so.
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
    /// the PC has not restarted since the preset was applied
    StillSameBoot,
    /// restarted without a crash: the trial stays pending
    CleanReboot,
    /// a crash was logged after the preset was applied: go back to Stock
    RevertAfterCrash,
    /// an earlier revert sent Stock but could not confirm it: send it again
    RetryRevert,
}

/// What the sign-in check does. Pure: all inputs are passed in. Boot times are unix seconds, 0 = unknown.
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

/// PowerShell that registers the sign-in check: this exe with --gpu-guard, 30 s after the current user
/// signs in, elevated.
pub fn register_task_script(exe: &str) -> String {
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

pub fn register_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program's path: {e}"))?;
    common::ps(&register_task_script(&exe.display().to_string())).map(|_| ())
}

pub fn unregister_task_script() -> String {
    format!(
        "Unregister-ScheduledTask -TaskPath {p} -TaskName {n} -Confirm:$false -ErrorAction SilentlyContinue\nif (Get-ScheduledTask -TaskPath {p} -TaskName {n} -ErrorAction SilentlyContinue) {{ throw 'the sign-in check task is still registered (run as administrator)' }}",
        p = common::psq(TASK_PATH),
        n = common::psq(TASK_NAME),
    )
}

pub fn unregister_task() -> Result<(), String> {
    common::ps(&unregister_task_script()).map(|_| ())
}

/// Start a trial: register the sign-in check and write gpu-trial.json (pending). The file records whether
/// the check is really armed, so the tab never promises a revert that cannot happen.
pub fn start_trial(preset: &str, previous: &str) -> StepResult {
    let registered = register_task();
    let trial = Trial {
        preset: preset.to_string(),
        previous: previous.to_string(),
        applied_at: chrono::Utc::now().timestamp(),
        boot_time: boot_time(),
        status: "pending".into(),
        reason: String::new(),
        unguarded: registered.is_err(),
        ..Default::default()
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

/// End a trial the sign-in check still acts on (pending, or a revert not confirmed yet): delete its file and
/// remove the sign-in check. The user has now chosen the GPU state, so a task that survives a failed
/// unregister finds nothing to act on. A settled "reverted" notice is left for the user to read.
pub fn end_trial() -> StepResult {
    if load_trial().map(|t| t.armed()).unwrap_or(false) {
        delete_trial();
    }
    unregister_task().map(|_| String::new())
}

pub fn guard_log(line: &str) {
    use std::io::Write;
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(common::data_dir().join(GUARD_LOG)).and_then(|mut f| writeln!(f, "{}  {line}", common::now()));
}

/// Sign-in check of the GPU trial (`PCTuner.exe --gpu-guard [--dry-run]`). Returns the process exit code.
/// `dry_run` only logs the decision; nothing is changed.
/// The trial file after the guard sent Stock following a crash (or a retry). Whatever happened, the trial is
/// "reverted" from now on, so the tab never offers Keep on a preset that crashed the PC. `startup_is_stock`:
/// Afterburner's saved [Startup] settings hold Stock (only asked for when the live check was idle).
/// Not confirmed: the sign-in check stays to send Stock again, until MAX_REVERT_ATTEMPTS sends, then it gives up.
pub fn revert_outcome(t: &Trial, crash_text: String, applied: Applied, startup_is_stock: bool) -> Trial {
    let verified = match applied {
        Applied::Confirmed => true,
        Applied::Unverified => startup_is_stock,
        Applied::Failed | Applied::Mismatch => false,
    };
    let attempts = t.attempts.saturating_add(1);
    let sent_now = applied != Applied::Failed;
    let last_error = match applied {
        _ if verified => String::new(),
        Applied::Failed => "Stock could not be loaded into Afterburner".into(),
        Applied::Mismatch => "the GPU ran something other than Stock (live memory offset differs)".into(),
        _ => "Stock was sent but not confirmed (GPU idle, Afterburner's saved settings not Stock yet)".into(),
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

pub fn guard_main(dry_run: bool) -> i32 {
    let mode = if dry_run { "dry-run" } else { "guard" };
    let trial = load_trial();
    let Some(t) = trial.as_ref().filter(|t| t.armed()) else {
        let why = match &trial {
            Some(t) => format!("trial status is \"{}\"", t.status),
            None => "no trial file".into(),
        };
        guard_log(&format!("[{mode}] decision={:?} ({why})", guard_decision(trial.as_ref(), 0, false)));
        return 0;
    };
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
    if !matches!(decision, Decision::RevertAfterCrash | Decision::RetryRevert) || dry_run {
        return 0;
    }

    // Crash during the trial: make sure slot 3 holds Stock (written with a backup if not) and load it.
    // Afterburner is not restarted "unchanged" when the slot write fails: its startup settings may hold the trial preset.
    let stock = &PRESETS[0];
    let slot_ok = Cfg::read().map(|c| section_is_preset(c.section(&format!("Profile{}", stock.slot)).as_ref(), stock)).unwrap_or(false);
    let report = |id: &str, r: StepResult| guard_log(&format!("[{mode}]   step {id}: {r:?}"));
    let applied = apply_preset(0, !slot_ok, false, &report, &|| {}, &|| {});
    let startup_is_stock = || {
        // give Afterburner a moment to save the profile it loaded as its startup settings
        std::thread::sleep(Duration::from_secs(5));
        live_verdict(gpu_live().as_ref(), stock) == Applied::Confirmed || Cfg::read().and_then(|c| find_preset(c.section("Startup").as_ref())) == Some(0)
    };
    let startup = applied == Applied::Unverified && startup_is_stock();
    let done = revert_outcome(t, crash_text, applied, startup);
    save_trial(&done);
    let code = if matches!(applied, Applied::Failed | Applied::Mismatch) { 1 } else { 0 };
    let what = if done.verified {
        "GPU set back to Stock (confirmed)".to_string()
    } else if done.gave_up {
        format!("revert NOT confirmed after {} attempts ({}); giving up", done.attempts, done.last_error)
    } else {
        guard_log(&format!(
            "[{mode}] revert NOT confirmed, attempt {}/{MAX_REVERT_ATTEMPTS} ({}); sign-in check kept to try again",
            done.attempts, done.last_error
        ));
        return code;
    };
    match unregister_task() {
        Ok(()) => guard_log(&format!("[{mode}] {what}; sign-in check removed")),
        Err(e) => guard_log(&format!("[{mode}] {what}; sign-in check NOT removed: {e}")),
    }
    code
}

// ------------------------------------------------------------------ tab

/// What happens to the trial once Afterburner has been started with the preset.
#[derive(Clone, Debug, PartialEq)]
pub enum After {
    Nothing,
    StartTrial { previous: String },
    /// `visible`: shown as a step (a trial is pending); otherwise the task is removed quietly.
    EndTrial { visible: bool },
}

pub enum Action {
    Apply { idx: usize, write: bool, after: After },
    /// `end_trial`: a safety trial is open and ends with the restore
    Restore { path: PathBuf, end_trial: bool },
}

/// Trial bookkeeping for applying preset `idx`, given what the user confirmed and whether a trial is pending.
pub fn plan_after(idx: usize, confirmed: Option<&str>, trial_pending: bool) -> After {
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
pub fn run_apply(idx: usize, write: bool, after: &After, report: &dyn Fn(&str, StepResult)) {
    // A new trial is armed BEFORE Afterburner gets the preset: an unstable tune usually crashes the PC the
    // moment it is applied, and the sign-in check must already exist then.
    // an earlier trial the sign-in check still acts on (pending, or a crash revert not confirmed yet)
    let prior = load_trial().filter(|t| t.armed());
    let arm = || {
        if let After::StartTrial { previous } = after {
            report(STEP_TRIAL, start_trial(PRESETS[idx].name, previous));
        }
    };
    // Afterburner never got the preset: no trial for it. An earlier armed trial is written back as it was, and
    // the sign-in check start_trial just (re)registered stays for it.
    let disarm = || {
        if let After::StartTrial { .. } = after {
            match &prior {
                Some(t) => save_trial(t),
                None => {
                    let _ = end_trial();
                }
            }
        }
    };
    if apply_preset(idx, write, true, report, &arm, &disarm) == Applied::Failed {
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
pub fn apply_window(kicker: &str, idx: usize, write: bool, after: &After) -> OpWindow {
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

/// `trial`: the preset of a safety trial that is open (it ends with the restore).
pub fn restore_window(last: &Path, trial: Option<&str>) -> OpWindow {
    let when = std::fs::metadata(last)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    let name = last.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut items = vec![
        OpItem::new(STEP_COPY, "Close Afterburner and put the backup back", "").detail(name),
        OpItem::new(STEP_START, "Restart MSI Afterburner", ""),
    ];
    if trial.is_some() {
        items.push(OpItem::new(STEP_END_TRIAL, "End the safety trial (remove the sign-in check)", ""));
    }
    let mut w = OpWindow::new("RESTORE LAST BACKUP", "run", "done", items).noun("step", "steps");
    w.show_steps = true;
    w.fail_phrase = "did not complete".into();
    w.preview_text = format!("Puts back the Afterburner profile file saved on {when}.");
    w.running_text = "Restoring the backup...".into();
    w.go_label = "Restore".into();
    let mut warn = "All Afterburner profile slots go back to what they held when this backup was made.".to_string();
    if let Some(p) = trial {
        warn.push_str(&format!("\nThe {p} trial ends: the restored profiles are not a trial tune."));
    }
    w.warning = Some(warn);
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
    pub selected: Option<usize>,
    pub op: Option<OpWindow>,
    pub op_action: Option<Action>,
    pub work: Option<Work>,
    /// removing the sign-in check after "Keep"
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
    Restore,
    Refresh,
    Keep,
    Revert,
    Dismiss,
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
            "Windows logged an unexpected shutdown during the {} trial. Stock could NOT be loaded into Afterburner at sign-in (see below), so the GPU may still run the {} preset; use Reset to stock ({next}). If it was not a power cut, try a milder preset.",
            t.preset, t.preset
        )
    } else if t.gave_up {
        let n = t.sends.max(1);
        format!(
            "Windows logged an unexpected shutdown during the {} trial. Stock was sent to Afterburner at {n} sign-in{} but never confirmed, so the sign-in check has stopped. Check the live readout under load, or use Reset to stock. If it was not a power cut, try a milder preset.",
            t.preset,
            if n == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Windows logged an unexpected shutdown during the {} trial. Stock was sent to Afterburner at sign-in - not yet confirmed; check the live readout (the sign-in check tries again next time). If it was not a power cut, try a milder preset.",
            t.preset
        )
    }
}

/// `t` confirmed by a live reading: Some(trial marked verified) when it is a revert not confirmed yet and the
/// GPU runs at full clocks with the Stock memory offset.
pub fn settled_revert(t: &Trial, live: Option<&Live>) -> Option<Trial> {
    let stock = live.filter(|l| l.full_clocks).and_then(live_preset) == Some(0);
    (t.revert_unverified() && stock).then(|| Trial { verified: true, last_error: String::new(), ..t.clone() })
}

/// Hero of "Currently applied": (name, unit, caption, colour). Without an Afterburner profile file nothing
/// is managed and the card runs its factory settings - not "custom settings".
pub fn hero_text(cfg_found: bool, saved: Option<usize>, current: Option<usize>) -> (&'static str, &'static str, String, Tone) {
    let caption = match (cfg_found, saved) {
        (false, _) => "No Afterburner profile - the card runs its factory settings (confirmed under load)".to_string(),
        (true, Some(i)) => format!("Afterburner saved settings: {}", PRESETS[i].name),
        (true, None) => "Afterburner saved settings: custom, not a preset".to_string(),
    };
    match current {
        Some(i) => (PRESETS[i].name, "preset", caption, Tone::Accent),
        None if !cfg_found => ("Not managed", "by Afterburner", caption, Tone::Off),
        None => ("Custom", "settings", caption, Tone::Junk),
    }
}

/// The live-check chip, from the one reading on screen: (text, colour).
pub fn live_chip(live: Option<&Live>) -> (String, Tone) {
    match (live, live.and_then(live_preset)) {
        (Some(_), Some(i)) => (format!("Live check: memory offset matches {}", PRESETS[i].name), Tone::Ok),
        (Some(l), None) if l.full_clocks => ("Live check: memory offset matches no preset".into(), Tone::Junk),
        (Some(l), None) => (format!("Live check: GPU idle in {} - confirms under load", l.pstate), Tone::Off),
        (None, _) => ("Live check: nvidia-smi not available".into(), Tone::Off),
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

    /// The nvidia-smi reading on screen: the newest 1 s sample, else the last state refresh.
    pub fn live_shown(&self) -> Option<&Live> {
        self.history.back().or(self.state.as_ref().and_then(|s| s.live.as_ref()))
    }

    pub fn reload_trial(&mut self) {
        if !self.demo {
            self.trial = load_trial();
            self.confirmed = load_confirmed();
        }
    }

    /// A crash revert the sign-in check could not confirm is settled once the tab sees Stock under load:
    /// the notice then reads as confirmed and the sign-in check is removed.
    pub fn settle_revert(&mut self) {
        if self.demo || self.work.is_some() || !self.trial.as_ref().map(|t| t.revert_unverified()).unwrap_or(false) {
            return;
        }
        // decide from the file on disk, which the sign-in check may have changed
        let Some(done) = load_trial().and_then(|t| settled_revert(&t, self.history.back())) else { return };
        save_trial(&done);
        self.trial = Some(done);
        if self.chore.is_none() {
            self.chore = Some(Job::spawn(unregister_task));
        }
    }

    pub fn pump(&mut self) {
        if let Some(s) = self.refresh.as_ref().and_then(|j| j.poll()) {
            if self.selected.is_none() {
                self.selected = s.live_preset.or(s.saved);
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
    pub fn open_apply(&mut self, idx: usize, kicker: String) {
        // plan from the file on disk: the sign-in check may have changed it
        self.reload_trial();
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
        // a revert that is not confirmed yet also keeps the sign-in check: applying a preset settles it
        let pending = self.trial.as_ref().map(|t| t.armed()).unwrap_or(false);
        let after = plan_after(idx, self.confirmed.as_deref(), pending);
        self.op = Some(apply_window(&kicker, idx, !slot_ok, &after));
        self.op_action = Some(Action::Apply { idx, write: !slot_ok, after });
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
                Action::Apply { idx, write, after } => run_apply(idx, write, &after, &report),
                Action::Restore { path, end_trial } => restore_backup(&path, end_trial, &report),
            }
        });
        self.work = Some(Work { rx, job });
    }

    pub fn click(&mut self, c: Click) {
        match c {
            Click::Reset => self.open_apply(0, "RESET GPU TO STOCK".into()),
            Click::Apply => match self.selected {
                Some(idx) => self.open_apply(idx, format!("APPLY {}", PRESETS[idx].name.to_uppercase())),
                None => self.status = "Pick a preset first.".into(),
            },
            Click::Restore => match last_backup() {
                None => self.status = format!("No backups in {}.", backup_dir().display()),
                Some(last) => {
                    self.reload_trial();
                    let trial = self.trial.as_ref().filter(|t| t.armed()).map(|t| t.preset.clone());
                    self.op = Some(restore_window(&last, trial.as_deref()));
                    self.op_action = Some(Action::Restore { path: last, end_trial: trial.is_some() });
                }
            },
            Click::Refresh => self.next_refresh = Instant::now(),
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
                        self.chore = Some(Job::spawn(unregister_task));
                    }
                }
            }
            Click::Revert => {
                self.reload_trial();
                let prev = self.trial.as_ref().map(|t| t.previous.clone()).unwrap_or_default();
                let idx = PRESETS.iter().position(|p| p.name == prev).unwrap_or(0);
                self.open_apply(idx, format!("REVERT TO {}", PRESETS[idx].name.to_uppercase()));
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
                    // the sign-in check was kept for this revert; without the file it has nothing to do
                    self.chore = Some(Job::spawn(unregister_task));
                }
            }
        }
    }

    /// Trial / crash notice at the top of the tab.
    /// An apply / restore is running.
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

    #[test]
    fn profile_file_is_the_installed_cards() {
        // this PC: an old Gigabyte RTX 5080's file (sorts first) is still there after the swap to an ASUS one
        let pnp = "PCI\\VEN_1002&DEV_13C0&SUBSYS_7E591462&REV_CB\\4&1\r\nPCI\\VEN_10DE&DEV_2C02&SUBSYS_89D71043&REV_A1\\69208F05A92DB04800\r\n";
        let cards = card_ids(pnp);
        assert_eq!(cards, vec!["VEN_10DE&DEV_2C02&SUBSYS_89D71043&REV_A1".to_string()]);
        let t = |s: u64| std::time::UNIX_EPOCH + Duration::from_secs(s);
        let old = PathBuf::from("VEN_10DE&DEV_2C02&SUBSYS_418B1458&REV_A1&BUS_1&DEV_0&FN_0.cfg");
        let new = PathBuf::from("VEN_10DE&DEV_2C02&SUBSYS_89D71043&REV_A1&BUS_1&DEV_0&FN_0.cfg");
        let files = vec![
            (PathBuf::from("MSIAfterburner.cfg"), t(900)),
            (PathBuf::from("VEN_1002&DEV_13C0&SUBSYS_7E591462&REV_CB&BUS_121&DEV_0&FN_0.cfg"), t(800)),
            (old.clone(), t(500)),
            (new.clone(), t(100)),
        ];
        // the present card's file wins even when it is older and sorts last
        assert_eq!(pick_cfg(&files, &cards), Some(new.clone()));
        // card not detected (or no file named after it): the one Afterburner saved last
        assert_eq!(pick_cfg(&files, &[]), Some(old.clone()));
        assert_eq!(pick_cfg(&files, &["VEN_10DE&DEV_2C02&SUBSYS_00000000&REV_A1".into()]), Some(old));
        // a different revision is a different card
        assert_eq!(pick_cfg(&files[..3], &["VEN_10DE&DEV_2C02&SUBSYS_89D71043&REV_A".into()]).unwrap().to_string_lossy().contains("418B1458"), true);
        assert_eq!(pick_cfg(&files[..2], &cards), None);
    }

    #[test]
    fn smi_parse() {
        let l = parse_smi("2895, 17001, 15001, 310.52, 61, P0, 97\r\n").unwrap();
        assert_eq!(l.mem_offset, 2000);
        assert!(l.full_clocks);
        assert_eq!(live_preset(&l), Some(3));
        let idle = parse_smi("210, 405, 15001, 25.10, 40, P8, 2").unwrap();
        assert!(!idle.full_clocks);
    }

    /// Live check, read-only: parses the real Afterburner profile, builds every preset in memory (never
    /// written) and prints the live nvidia-smi reading (`cargo test gpu_cfg_live -- --ignored --nocapture`).
    /// Fails when there is no RTX 5080 profile file; the fixture tests below cover machines without one.
    #[test]
    #[ignore]
    fn gpu_cfg_live() {
        println!("Afterburner: {}  (exe exists: {})", ab_dir().display(), ab_exe().exists());
        println!("backup dir:  {}", backup_dir().display());
        let cfg = Cfg::read().expect("no RTX 5080 profile file found - this live check needs Afterburner's VEN_10DE&DEV_2C02*.cfg");
        println!("cfg: {}  ({} lines, CRLF: {})", cfg.path.display(), cfg.lines.len(), cfg.crlf);
        // the file must round-trip byte for byte when nothing is changed
        let orig = std::fs::read(&cfg.path).unwrap();
        assert_eq!(orig, cfg.to_bytes(), "unchanged re-serialisation differs from the file");
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
                Err(e) => panic!("build {} FAILED: {e}", p.name),
            }
        }
        match gpu_live() {
            Some(l) => println!("live: {:?}\n  -> live preset: {:?}", l, live_preset(&l).map(|i| PRESETS[i].name)),
            None => println!("live: nvidia-smi not available"),
        }
        println!("Afterburner running: {}", ab_running());
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
        assert_eq!(current_preset(Some(2), Some(&live("2895, 17001, 15001, 310, 61, P0, 97"))), Some(3));
        assert_eq!(current_preset(Some(2), Some(&live("2895, 15700, 15001, 310, 61, P0, 97"))), None);
        assert_eq!(current_preset(Some(2), Some(&live("210, 405, 15001, 25, 40, P8, 2"))), Some(2));
        assert_eq!(current_preset(Some(2), None), Some(2));
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

    // ------------------------------------------------------------- fixtures (no Afterburner needed)

    /// Points in the fixture curve and the trailing bytes after them (the real RTX 5080 blob is 3228 bytes for
    /// 127 points: 12 + 127*12 = 1536 bytes of header and points, then 1692 bytes the writer must keep).
    const N: usize = 127;
    const TRAIL: usize = 1692;

    fn trail_byte(i: usize) -> u8 {
        (i * 7 + 3) as u8
    }

    /// A VFCurve hex string: header (version, count, reserved), points {mV, base MHz, offset MHz}, filler.
    fn curve_hex_with(version: u32, n: usize, mv: &dyn Fn(usize) -> f32, base: &dyn Fn(usize) -> f32, ofs: &dyn Fn(usize) -> f32, trailing: usize) -> String {
        let mut b = vec![];
        b.extend_from_slice(&version.to_le_bytes());
        b.extend_from_slice(&(n as u32).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        for k in 0..n {
            b.extend_from_slice(&mv(k).to_le_bytes());
            b.extend_from_slice(&base(k).to_le_bytes());
            b.extend_from_slice(&ofs(k).to_le_bytes());
        }
        b.extend((0..trailing).map(trail_byte));
        b.iter().map(|x| format!("{x:02X}")).collect()
    }

    /// 450 mV upward in 5 mV steps, base 2000 MHz rising to about 2820: every preset is within +300 MHz.
    fn fixture_mv(k: usize) -> f32 {
        450.0 + 5.0 * k as f32
    }
    fn fixture_base(k: usize) -> f32 {
        2000.0 + 6.5 * k as f32
    }
    fn fixture_curve() -> String {
        curve_hex_with(0x0002_0001, N, &fixture_mv, &fixture_base, &|_| 0.0, TRAIL)
    }

    /// A profile file with [Profile2] in the middle (followed by [Profile3] and another section).
    fn fixture_text(vf: &str, nl: &str) -> String {
        [
            "[Settings]",
            "LastUsed=1",
            "[Startup]",
            "Format=2",
            &format!("VFCurve={vf}"),
            "FanMode=1",
            "FanSpeed=45",
            "[Profile2]",
            "Format=2",
            "MemClkBoost=0",
            "VFCurve=00",
            "[Profile3]",
            "Format=2",
            "CoreClkBoost=0",
            "[Defaults]",
            "Foo=bar",
        ]
        .iter()
        .map(|l| format!("{l}{nl}"))
        .collect()
    }

    fn fixture_cfg() -> Cfg {
        Cfg::parse(PathBuf::new(), fixture_text(&fixture_curve(), "\r\n").as_bytes())
    }

    fn cfg_with_startup(vf: &str) -> Cfg {
        Cfg::parse(PathBuf::new(), format!("[Startup]\r\nVFCurve={vf}\r\n").as_bytes())
    }

    fn sec(pairs: &[(&str, &str)]) -> Section {
        Section(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    #[test]
    fn gpu_decode_curve() {
        let c = decode_curve(&fixture_curve()).expect("fixture decodes");
        assert_eq!(c.points.len(), N);
        assert_eq!(c.bytes.len(), 12 + N * 12 + TRAIL);
        assert_eq!((c.points[0].mv, c.points[0].base, c.points[0].off), (450.0, 2000.0, 12));
        assert_eq!(c.points[N - 1].off, 12 + (N - 1) * 12);
        // lower-case hex and surrounding whitespace are fine
        assert!(decode_curve(&format!("  {}\r\n", fixture_curve().to_lowercase())).is_some());

        assert!(decode_curve("").is_none());
        assert!(decode_curve("ABC").is_none(), "odd length");
        assert!(decode_curve(&format!("{}Z", &fixture_curve()[..fixture_curve().len() - 1])).is_none(), "not hex");
        assert!(decode_curve(&curve_hex_with(0x0001_0001, N, &fixture_mv, &fixture_base, &|_| 0.0, 0)).is_none(), "wrong version");
        assert!(decode_curve(&curve_hex_with(0x0002_0001, 49, &fixture_mv, &fixture_base, &|_| 0.0, 0)).is_none(), "n=49");
        assert!(decode_curve(&curve_hex_with(0x0002_0001, 50, &fixture_mv, &fixture_base, &|_| 0.0, 0)).is_some(), "n=50");
        assert!(decode_curve(&curve_hex_with(0x0002_0001, 255, &fixture_mv, &fixture_base, &|_| 0.0, 0)).is_some(), "n=255");
        assert!(decode_curve(&curve_hex_with(0x0002_0001, 256, &fixture_mv, &fixture_base, &|_| 0.0, 0)).is_none(), "n=256");
        // header says 127 points, but the buffer stops one point short
        let full = curve_hex_with(0x0002_0001, N, &fixture_mv, &fixture_base, &|_| 0.0, 0);
        assert!(decode_curve(&full[..(12 + (N - 1) * 12) * 2]).is_none(), "short buffer");
        assert!(decode_curve(&full[..16]).is_none(), "shorter than the header");
    }

    #[test]
    fn gpu_preset_build_fixture() {
        let cfg = fixture_cfg();
        let input = decode_curve(&fixture_curve()).unwrap();
        for (i, p) in PRESETS.iter().enumerate() {
            let lines = preset_section_lines(&cfg, p).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            assert_eq!(lines[0], format!("[Profile{}]", p.slot));
            let s = Section::from_lines(&lines[1..]);
            assert_eq!(find_preset(Some(&s)), Some(i), "{} detected as itself", p.name);
            assert_eq!(s.get("MemClkBoost"), Some((p.mem * 1000).to_string().as_str()));
            assert_eq!(s.get("CoreClkBoost"), Some("0"));
            assert_eq!(s.get("PowerLimit"), Some(p.power.to_string().as_str()));
            // fan settings come from [Startup]
            assert_eq!((s.get("FanMode"), s.get("FanSpeed")), (Some("1"), Some("45")));
            let out = decode_curve(s.get("VFCurve").unwrap()).unwrap();
            assert_eq!(out.bytes.len(), input.bytes.len());
            assert_eq!(out.bytes[12 + N * 12..], input.bytes[12 + N * 12..], "{}: trailing bytes kept", p.name);
            assert_eq!(out.bytes[..12], input.bytes[..12], "header kept");
            for (a, b) in out.points.iter().zip(&input.points) {
                assert_eq!((a.mv, a.base), (b.mv, b.base), "only offsets change");
            }
            if p.target_mhz == 0 {
                assert!(out.points.iter().all(|pt| pt.ofs == 0.0), "Stock zeroes every offset");
            } else {
                let max_eff = out.points.iter().map(|pt| pt.base + pt.ofs).fold(f32::MIN, f32::max);
                assert!((max_eff - p.target_mhz as f32).abs() < 0.01, "{}: flat at the target", p.name);
                // the anchor is the first point at or above target_mv; from there on the curve is flat
                let anchor = out.points.iter().position(|pt| pt.mv >= p.target_mv as f32).unwrap();
                assert!(out.points[anchor..].iter().all(|pt| (pt.base + pt.ofs - p.target_mhz as f32).abs() < 0.01));
                assert!(out.points[..anchor].iter().all(|pt| pt.base + pt.ofs <= p.target_mhz as f32 + 0.01));
            }
        }
        // Stock leaves an existing offset curve at 0 again
        let tuned = cfg_with_startup(&curve_hex_with(0x0002_0001, N, &fixture_mv, &fixture_base, &|_| 150.0, TRAIL));
        let s = Section::from_lines(&preset_section_lines(&tuned, &PRESETS[0]).unwrap()[1..]);
        assert!(decode_curve(s.get("VFCurve").unwrap()).unwrap().points.iter().all(|pt| pt.ofs == 0.0));
    }

    #[test]
    fn gpu_preset_build_rejects() {
        let eff = &PRESETS[1];
        // stock curve too low: Efficiency would need far more than +300 MHz at 900 mV
        let low = cfg_with_startup(&curve_hex_with(0x0002_0001, N, &fixture_mv, &|k| 1500.0 + 6.5 * k as f32, &|_| 0.0, TRAIL));
        assert!(preset_section_lines(&low, eff).unwrap_err().contains("too aggressive"));
        // Stock never lifts anything, so the same curve is fine for it
        assert!(preset_section_lines(&low, &PRESETS[0]).is_ok());
        // not a voltage/frequency table: first point under 300 mV, or a base clock above 5000 MHz
        let low_mv = cfg_with_startup(&curve_hex_with(0x0002_0001, N, &|k| 250.0 + 5.0 * k as f32, &fixture_base, &|_| 0.0, 0));
        assert!(preset_section_lines(&low_mv, eff).unwrap_err().contains("does not look like"));
        let fast = cfg_with_startup(&curve_hex_with(0x0002_0001, N, &fixture_mv, &|k| if k == 10 { 6000.0 } else { fixture_base(k) }, &|_| 0.0, 0));
        assert!(preset_section_lines(&fast, eff).unwrap_err().contains("does not look like"));
        // every point below the preset's voltage
        let short = cfg_with_startup(&curve_hex_with(0x0002_0001, N, &|k| 450.0 + k as f32, &fixture_base, &|_| 0.0, 0));
        assert_eq!(preset_section_lines(&short, eff).unwrap_err(), "No curve point at or above 900 mV.");
        // missing pieces
        assert!(preset_section_lines(&Cfg::parse(PathBuf::new(), b"[Settings]\r\n"), eff).unwrap_err().contains("has not saved this card's voltage/frequency curve"));
        assert!(preset_section_lines(&Cfg::parse(PathBuf::new(), b"[Startup]\r\nVFCurve=\r\n"), eff).unwrap_err().contains("has not saved this card's voltage/frequency curve"));
        assert!(preset_section_lines(&cfg_with_startup("0102"), eff).unwrap_err().contains("Unexpected curve format"));
    }

    #[test]
    fn stock_curve_from_a_saved_slot() {
        // this PC: [Startup] stays empty (EraseStartupSettings=1), the user saved the stock settings to slot 1
        let tuned = curve_hex_with(0x0002_0001, N, &fixture_mv, &fixture_base, &|k| 50.0 + k as f32, TRAIL);
        let text = format!("[Startup]\r\nFormat=2\r\nVFCurve=\r\nFanMode=\r\n[Profile1]\r\nFormat=2\r\nVFCurve={tuned}\r\nFanMode=1\r\nFanSpeed=30\r\n");
        let from_slot = Cfg::parse(PathBuf::new(), text.as_bytes());
        let from_startup = cfg_with_startup(&fixture_curve());
        for p in PRESETS {
            let a = preset_section_lines(&from_slot, p).unwrap();
            let b = preset_section_lines(&from_startup, p).unwrap();
            // same curve: the slot's offsets are replaced, only its stock base clocks are used
            assert_eq!(a.iter().find(|l| l.starts_with("VFCurve=")), b.iter().find(|l| l.starts_with("VFCurve=")), "{}", p.name);
            // fan settings still come from [Startup] only, never from the slot that gave the curve
            assert!(a.contains(&"FanMode=".to_string()) && !a.iter().any(|l| l.starts_with("FanSpeed")));
        }
        // [Startup] wins when it has a curve; a broken slot is skipped when another one is fine
        let both = format!("[Startup]\r\nVFCurve={}\r\n[Profile1]\r\nVFCurve=0102\r\n[Profile2]\r\nVFCurve={tuned}\r\n", fixture_curve());
        assert!(preset_section_lines(&Cfg::parse(PathBuf::new(), both.as_bytes()), &PRESETS[1]).is_ok());
        let broken = format!("[Startup]\r\nVFCurve=\r\n[Profile1]\r\nVFCurve=0102\r\n[Profile3]\r\nVFCurve={tuned}\r\n");
        assert!(preset_section_lines(&Cfg::parse(PathBuf::new(), broken.as_bytes()), &PRESETS[1]).is_ok());
    }

    #[test]
    fn gpu_section_is_preset_edges() {
        let eff = &PRESETS[1];
        // one point peaks at 2700 + x MHz, the rest stay far lower
        let peak = |x: f32| curve_hex_with(0x0002_0001, N, &fixture_mv, &|k| if k == 100 { 2700.0 } else { 2000.0 }, &move |k| if k == 100 { x } else { 0.0 }, TRAIL);
        let with = |vf: &str, mem: &str| sec(&[("VFCurve", vf), ("MemClkBoost", mem), ("PowerLimit", "100"), ("CoreClkBoost", "0")]);
        assert!(section_is_preset(Some(&with(&peak(0.0), "1000000")), eff));
        assert!(section_is_preset(Some(&with(&peak(CLOCK_TOLERANCE as f32), "1000000")), eff), "+tolerance accepted");
        assert!(section_is_preset(Some(&with(&peak(-CLOCK_TOLERANCE as f32), "1000000")), eff), "-tolerance accepted");
        assert!(!section_is_preset(Some(&with(&peak(CLOCK_TOLERANCE as f32 + 0.5), "1000000")), eff), "just over");
        assert!(!section_is_preset(Some(&with(&peak(-CLOCK_TOLERANCE as f32 - 0.5), "1000000")), eff), "just under");
        // MemClkBoost is in kHz: 1000000 = +1000 MHz; anything else, or text, is not this preset
        assert!(!section_is_preset(Some(&with(&peak(0.0), "1500000")), eff));
        assert!(!section_is_preset(Some(&with(&peak(0.0), "abc")), eff));
        assert_eq!(find_preset(Some(&with(&peak(0.0), "1000000"))), Some(1));
        // a different power limit
        let pl = sec(&[("VFCurve", &peak(0.0)), ("MemClkBoost", "1000000"), ("PowerLimit", "90")]);
        assert!(!section_is_preset(Some(&pl), eff));
        // no VFCurve, or none at all
        assert!(!section_is_preset(Some(&sec(&[("MemClkBoost", "1000000")])), eff));
        assert!(!section_is_preset(None, eff));
        // Stock: no offsets and no CoreClkBoost
        let flat = curve_hex_with(0x0002_0001, N, &fixture_mv, &fixture_base, &|_| 0.0, TRAIL);
        let stock = |core: &str| sec(&[("VFCurve", &flat), ("MemClkBoost", "0"), ("CoreClkBoost", core)]);
        assert!(section_is_preset(Some(&stock("0")), &PRESETS[0]));
        assert!(section_is_preset(Some(&stock("")), &PRESETS[0]), "missing CoreClkBoost counts as 0");
        assert!(!section_is_preset(Some(&stock("50")), &PRESETS[0]));
        let small_ofs = curve_hex_with(0x0002_0001, N, &fixture_mv, &fixture_base, &|k| if k == 3 { 1.5 } else { 0.0 }, TRAIL);
        assert!(!section_is_preset(Some(&sec(&[("VFCurve", &small_ofs), ("MemClkBoost", "0")])), &PRESETS[0]));
    }

    #[test]
    fn gpu_cfg_parse_round_trip() {
        for nl in ["\r\n", "\n"] {
            let text = fixture_text(&fixture_curve(), nl);
            let c = Cfg::parse(PathBuf::new(), text.as_bytes());
            assert_eq!(c.crlf, nl == "\r\n");
            assert_eq!(c.to_bytes(), text.as_bytes(), "unchanged file round-trips byte for byte ({nl:?})");
            assert_eq!(c.section_names(), vec!["Settings", "Startup", "Profile2", "Profile3", "Defaults"]);
            assert_eq!(c.section("Profile3").unwrap().get("coreclkboost"), Some("0"), "keys are case-insensitive");
        }
        // pinned current behaviour: a UTF-8 BOM is dropped, a missing final newline is added
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"[A]\r\nx=1\r\n");
        assert_eq!(Cfg::parse(PathBuf::new(), &bom).to_bytes(), b"[A]\r\nx=1\r\n");
        assert_eq!(Cfg::parse(PathBuf::new(), b"[A]\r\nx=1").to_bytes(), b"[A]\r\nx=1\r\n");
        assert_eq!(Cfg::parse(PathBuf::new(), b"[A]\nx=1").to_bytes(), b"[A]\nx=1\n");
        // one line without any newline is written with CRLF
        assert_eq!(Cfg::parse(PathBuf::new(), b"[A]").to_bytes(), b"[A]\r\n");
    }

    #[test]
    fn gpu_with_section_middle() {
        let cfg = fixture_cfg();
        let built = preset_section_lines(&cfg, &PRESETS[1]).unwrap();
        let out = Cfg { path: PathBuf::new(), lines: with_section(&cfg, 2, built.clone()), crlf: cfg.crlf };
        let re = Cfg::parse(PathBuf::new(), &out.to_bytes());
        assert_eq!(re.lines.iter().filter(|l| *l == "[Profile2]").count(), 1, "old [Profile2] removed");
        assert!(!re.lines.iter().any(|l| l == "VFCurve=00"), "old body dropped");
        assert_eq!(re.section_names(), vec!["Settings", "Startup", "Profile3", "Defaults", "Profile2"]);
        // the neighbours are untouched
        assert_eq!(re.section("Profile3").unwrap().0, cfg.section("Profile3").unwrap().0);
        assert_eq!(re.section("Defaults").unwrap().0, cfg.section("Defaults").unwrap().0);
        assert_eq!(re.section("Startup").unwrap().0, cfg.section("Startup").unwrap().0);
        assert_eq!(find_preset(re.section("Profile2").as_ref()), Some(1));
        assert_eq!(find_preset(re.section("Startup").as_ref()), Some(0), "the stock [Startup] is Stock");
        // a slot that does not exist yet is appended
        let out = with_section(&cfg, 4, preset_section_lines(&cfg, &PRESETS[3]).unwrap());
        assert_eq!(out.len(), cfg.lines.len() + built.len());
        assert_eq!(find_preset(Cfg::parse(PathBuf::new(), &Cfg { path: PathBuf::new(), lines: out, crlf: true }.to_bytes()).section("Profile4").as_ref()), Some(3));
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
        // only a confirmed Stock (live, or saved as Afterburner's startup settings) removes the sign-in check
        let crashed = trial("pending", 1);
        let out = |t: &Trial, a: Applied, startup: bool| revert_outcome(t, "event 41".into(), a, startup);
        let ok = out(&crashed, Applied::Confirmed, false);
        assert!(ok.verified && !ok.armed() && ok.status == "reverted" && ok.reason == "event 41" && ok.last_error.is_empty());
        assert!(out(&crashed, Applied::Unverified, true).verified);
        // whatever happened after a crash, the trial is "reverted": Keep is never offered on the crashing preset
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
        assert_eq!(guard_decision(Some(&t), 0, false), Decision::NoTrial, "a leftover task does nothing");
        assert!(reverted_text(&t).contains("sign-in check has stopped") && reverted_text(&t).contains("at 3 sign-ins"));
        // Stock never reached Afterburner: the notice must not say it was sent, before or after giving up
        let failed = out(&crashed, Applied::Failed, false);
        assert!(!failed.sent && failed.sends == 0 && failed.armed());
        assert!(!reverted_text(&failed).contains("was sent") && reverted_text(&failed).contains("could NOT be loaded"));
        let mut f = crashed.clone();
        for _ in 0..MAX_REVERT_ATTEMPTS {
            f = out(&f, Applied::Failed, false);
        }
        assert!(f.gave_up && !f.sent && f.attempts == MAX_REVERT_ATTEMPTS);
        assert!(!reverted_text(&f).contains("was sent") && reverted_text(&f).contains("could NOT be loaded"));
        // a failed attempt, one that reached Afterburner, another failed one: only the real send is counted
        let mixed = out(&out(&out(&crashed, Applied::Failed, false), Applied::Unverified, false), Applied::Failed, false);
        assert!(mixed.gave_up && mixed.sent && mixed.sends == 1);
        assert!(reverted_text(&mixed).contains("at 1 sign-in but never confirmed"));
        // a later confirmation still settles it
        assert!(out(&Trial { attempts: 1, ..out(&crashed, Applied::Failed, false) }, Applied::Confirmed, false).verified);
        let l = |line: &str| parse_smi(line).unwrap();
        // the tab settles an unconfirmed revert only from a full-clocks Stock reading
        let unv = out(&crashed, Applied::Unverified, false);
        let settled = settled_revert(&unv, Some(&l("2895, 15001, 15001, 310, 61, P0, 97"))).unwrap();
        assert!(settled.verified && !settled.armed() && settled.last_error.is_empty());
        assert_eq!(settled_revert(&unv, Some(&l("210, 405, 15001, 25, 40, P8, 2"))), None, "idle");
        assert_eq!(settled_revert(&unv, Some(&l("2895, 17001, 15001, 310, 61, P0, 97"))), None, "not Stock");
        assert_eq!(settled_revert(&unv, None), None);
        assert_eq!(settled_revert(&crashed, Some(&l("2895, 15001, 15001, 310, 61, P0, 97"))), None, "pending trials are not touched");
        assert_eq!(live_verdict(Some(&l("2895, 15001, 15001, 310, 61, P0, 97")), &PRESETS[0]), Applied::Confirmed);
        assert_eq!(live_verdict(Some(&l("2895, 17001, 15001, 310, 61, P0, 97")), &PRESETS[0]), Applied::Mismatch);
        assert_eq!(live_verdict(Some(&l("210, 405, 15001, 25, 40, P8, 2")), &PRESETS[0]), Applied::Unverified);
        assert_eq!(live_verdict(None, &PRESETS[0]), Applied::Unverified);
        // the notice only states Stock as fact when it was confirmed
        let t = Trial { sent: true, sends: 1, attempts: 1, ..trial("reverted", 1) };
        assert!(reverted_text(&t).contains("not yet confirmed"));
        assert!(reverted_text(&Trial { verified: true, ..t }).contains("was set back to Stock at sign-in"));
        // older "reverted" files without the field read as unconfirmed
        assert!(!serde_json::from_str::<Trial>(r#"{"preset":"Balanced","status":"reverted"}"#).unwrap().verified);
    }

    #[test]
    fn gpu_hero_and_live_chip() {
        // no Afterburner profile at idle: not "custom settings"
        let (name, unit, cap, c) = hero_text(false, None, None);
        assert_eq!((name, unit, c), ("Not managed", "by Afterburner", Tone::Off));
        assert!(cap.starts_with("No Afterburner profile"));
        // ... but a live P0 reading at stock memory still says Stock
        assert_eq!(hero_text(false, None, Some(0)).0, "Stock");
        let (name, _, cap, _) = hero_text(true, None, None);
        assert_eq!((name, cap.as_str()), ("Custom", "Afterburner saved settings: custom, not a preset"));
        assert_eq!(hero_text(true, Some(2), Some(2)).2, "Afterburner saved settings: Balanced");

        // the chip, the memory chip and the hero all come from the same reading
        let perf = parse_smi("2895, 17001, 15001, 310, 61, P0, 97").unwrap();
        let (text, c) = live_chip(Some(&perf));
        assert_eq!((text.as_str(), c), ("Live check: memory offset matches Performance", Tone::Ok));
        assert_eq!(current_preset(None, Some(&perf)), Some(3));
        assert_eq!(live_chip(Some(&parse_smi("2895, 15700, 15001, 310, 61, P0, 97").unwrap())).0, "Live check: memory offset matches no preset");
        assert!(live_chip(Some(&parse_smi("210, 405, 15001, 25, 40, P8, 2").unwrap())).0.contains("idle in P8"));
        assert!(live_chip(None).0.contains("not available"));
    }

    #[test]
    fn gpu_restore_window_trial() {
        let w = restore_window(Path::new("x.cfg"), None);
        assert_eq!(w.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![STEP_COPY, STEP_START]);
        assert!(!w.warning.unwrap().contains("trial"));
        let w = restore_window(Path::new("x.cfg"), Some("Performance"));
        assert_eq!(w.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![STEP_COPY, STEP_START, STEP_END_TRIAL]);
        assert!(w.warning.unwrap().contains("The Performance trial ends"));
    }
}
