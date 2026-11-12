//! Fan Tuner tab - live fan speeds and temperatures, driven by the PCTunerFan helper
//! (LibreHardwareMonitorLib + PawnIO). The helper holds the safety rules (floors, 90 C override,
//! fans back to the BIOS on exit/crash); this side shows the data and sends requests.

use crate::common::{self, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
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
    pub msg: String,
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
    pub pumps: BTreeMap<String, bool>,
}

/// %LOCALAPPDATA%\PCTuner\fan-profiles.json
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct ProfileStore {
    /// Applied when PC Tuner starts.
    pub last: Option<String>,
    pub profiles: BTreeMap<String, Profile>,
}

fn preset_points(name: &str) -> Option<[[f32; 2]; 5]> {
    match name {
        "Silent" => Some([[30.0, 20.0], [50.0, 25.0], [65.0, 40.0], [75.0, 60.0], [85.0, 100.0]]),
        "Balanced" => Some([[30.0, 30.0], [50.0, 40.0], [65.0, 60.0], [75.0, 80.0], [85.0, 100.0]]),
        "Performance" => Some([[30.0, 40.0], [50.0, 55.0], [65.0, 75.0], [75.0, 90.0], [80.0, 100.0]]),
        _ => None,
    }
}

/// A built-in profile for the fans this PC has: a curve per controllable fan (never under its floor).
/// Silent leaves the graphics card on its own curve, which can stop its fans at idle.
pub fn builtin_profile(name: &str, fans: &[FanInfo], pumps: &HashMap<String, bool>) -> Option<Profile> {
    let pts = preset_points(name)?;
    let mut p = Profile::default();
    for f in fans.iter().filter(|f| f.controllable) {
        let pump = pumps.get(&f.id).copied().unwrap_or(f.pump);
        if name == "Silent" && f.gpu {
            continue;
        }
        let floor = floor_for(pump, f.min);
        let FanSetting::Curve { source, .. } = default_curve(&f.name, f.gpu, pump, floor) else { continue };
        let points = pts.iter().map(|q| [q[0], q[1].max(floor)]).collect();
        p.fans.insert(f.id.clone(), FanSetting::Curve { source, points });
    }
    for (id, v) in pumps {
        p.pumps.insert(id.clone(), *v);
    }
    Some(p)
}

/// Settings without the fans left on BIOS (so "BIOS" and "not listed" compare equal).
pub fn normalized(settings: &HashMap<String, FanSetting>) -> BTreeMap<String, FanSetting> {
    settings.iter().filter(|(_, s)| **s != FanSetting::Bios).map(|(k, v)| (k.clone(), v.clone())).collect()
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
}

impl Shared {
    fn take_status(&mut self, s: Status) {
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
    stdin: Arc<Mutex<ChildStdin>>,
    shared: Arc<Mutex<Shared>>,
    stopped: bool,
}

impl Helper {
    fn start(shared: Arc<Mutex<Shared>>) -> Result<Helper, String> {
        let exe = extract_helper()?;
        let mut child = Command::new(&exe)
            .args(["--parent", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(common::CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("could not start the fan helper: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
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
        {
            let shared = shared.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    if let Ok(mut sh) = shared.lock() {
                        sh.log.push(line);
                        if sh.log.len() > 50 {
                            sh.log.remove(0);
                        }
                    }
                }
            });
        }
        // keep-alive: the helper hands fans back to the BIOS if it hears nothing for 10 s
        {
            let stdin = Arc::downgrade(&stdin);
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(s) = stdin.upgrade() else { break };
                let ok = s.lock().map(|mut w| writeln!(w, "ping").and_then(|_| w.flush()).is_ok()).unwrap_or(false);
                if !ok {
                    break;
                }
            });
        }
        Ok(Helper { child, stdin, shared, stopped: false })
    }

    fn send(&self, cmd: &str) -> bool {
        self.stdin.lock().map(|mut w| writeln!(w, "{cmd}").and_then(|_| w.flush()).is_ok()).unwrap_or(false)
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ask the helper to hand every fan back to the BIOS and exit; kill it if it does not within 3 s.
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Ok(mut w) = self.stdin.lock() {
            let _ = writeln!(w, "quit");
            let _ = w.flush();
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
        msg: String::new(),
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

// ------------------------------------------------------------------ tab

/// What a fan card asks for when the user touches it.
enum CardAction {
    Set(FanSetting),
    Pump(bool),
}

pub struct FanTab {
    shared: Arc<Mutex<Shared>>,
    helper: Option<Helper>,
    error: Option<String>,
    restarts: u32,
    started: bool,
    demo: bool,
    /// What the user asked for, per fan id.
    settings: HashMap<String, FanSetting>,
    /// "This is a pump" choices the user made (overrides the name-based guess).
    pumps: HashMap<String, bool>,
    /// Last command sent per key, so each change is sent once (cleared when the helper restarts).
    sent: HashMap<String, String>,
    afterburner: Arc<AtomicBool>,
    store: ProfileStore,
    /// The profile the settings came from (they may have been changed since).
    active: Option<String>,
    new_name: String,
    note: Option<String>,
}

impl FanTab {
    pub fn new() -> Self {
        let demo = std::env::var("PCTUNER_DEMO").map(|v| v.to_ascii_lowercase().starts_with("fan")).unwrap_or(false);
        let mut tab = FanTab {
            shared: Arc::new(Mutex::new(Shared::default())),
            helper: None,
            error: None,
            restarts: 0,
            started: false,
            demo,
            settings: HashMap::new(),
            pumps: HashMap::new(),
            sent: HashMap::new(),
            afterburner: Arc::new(AtomicBool::new(false)),
            store: if demo { ProfileStore::default() } else { common::load_json(PROFILES_FILE) },
            active: None,
            new_name: String::new(),
            note: None,
        };
        // apply the last profile at startup (only as administrator: otherwise nothing can be controlled)
        if !demo {
            if let Some(name) = tab.store.last.clone() {
                if let Some(p) = tab.store.profiles.get(&name).cloned() {
                    tab.use_profile(&name, &p);
                    tab.note = Some(format!("{name} was applied when PC Tuner started."));
                    if common::is_admin() {
                        tab.start();
                    }
                }
            }
        }
        if demo {
            // the Balanced profile, with the CPU fan changed by hand (shows "changed - not saved")
            if let Some(p) = builtin_profile("Balanced", &demo_status(0.0).fans, &HashMap::new()) {
                tab.store.profiles.insert("Balanced".into(), p.clone());
                tab.use_profile("Balanced", &p);
            }
            tab.settings.insert("/lpc/nct6687d/0/control/0".into(), FanSetting::Manual { percent: 45.0 });
            tab.note = Some("Balanced was applied when PC Tuner started.".into());
            tab.afterburner.store(true, Ordering::Relaxed);
        }
        tab
    }

    fn start(&mut self) {
        self.started = true;
        if self.demo {
            return;
        }
        if let Ok(mut sh) = self.shared.lock() {
            sh.exited = false;
        }
        match Helper::start(self.shared.clone()) {
            Ok(h) => {
                self.helper = Some(h);
                self.error = None;
                // a fresh helper knows nothing: send every setting again
                self.sent.clear();
            }
            Err(e) => self.error = Some(e),
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
    fn sync(&mut self) {
        if !common::is_admin() {
            return;
        }
        let Some(h) = &self.helper else { return };
        let mut cmds: Vec<(String, String)> = vec![];
        for (id, choice) in &self.pumps {
            cmds.push((format!("pump:{id}"), format!("pump {id} {}", if *choice { 1 } else { 0 })));
        }
        for (id, s) in &self.settings {
            cmds.push((id.clone(), command_for(id, s)));
        }
        for (key, cmd) in cmds {
            if self.sent.get(&key) != Some(&cmd) && h.send(&cmd) {
                self.sent.insert(key, cmd);
            }
        }
    }

    fn give_all_back(&mut self) {
        for s in self.settings.values_mut() {
            *s = FanSetting::Bios;
        }
        if let Some(h) = &self.helper {
            h.send("autoall");
        }
    }

    fn any_controlled(&self) -> bool {
        self.settings.values().any(|s| *s != FanSetting::Bios)
    }

    /// Take a profile's settings: listed fans as saved, every other fan back to BIOS.
    fn use_profile(&mut self, name: &str, p: &Profile) {
        for s in self.settings.values_mut() {
            *s = FanSetting::Bios;
        }
        for (id, s) in &p.fans {
            self.settings.insert(id.clone(), s.clone());
        }
        self.pumps = p.pumps.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.active = Some(name.to_string());
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
                let Some(p) = builtin_profile(name, fans, &self.pumps) else { return };
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
        Profile { fans: normalized(&self.settings), pumps: self.pumps.iter().map(|(k, v)| (k.clone(), *v)).collect() }
    }

    fn modified(&self) -> bool {
        match self.active.as_ref().and_then(|n| self.store.profiles.get(n)) {
            Some(p) => normalized(&self.settings) != p.fans,
            None => self.any_controlled(),
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
                    let ok = !name.is_empty() && name.len() <= 32 && !BUILTIN.contains(&name.as_str());
                    if ui.add_enabled(ok, egui::Button::new("Save as new")).clicked() {
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
    fn watch(&mut self) {
        let Some(h) = &mut self.helper else { return };
        if h.alive() {
            return;
        }
        let tail = self.shared.lock().map(|s| s.log.last().cloned().unwrap_or_default()).unwrap_or_default();
        self.helper = None;
        if self.restarts < 1 {
            self.restarts += 1;
            self.start();
            if self.error.is_none() {
                self.error = Some(format!("The fan helper stopped and was restarted. {tail}").trim().to_string());
            }
        } else {
            self.error = Some(format!("The fan helper stopped. Fans are on BIOS control. {tail}").trim().to_string());
        }
    }

    fn snapshot(&self, now: f64) -> (Option<Status>, HashMap<String, VecDeque<f32>>, VecDeque<f32>, VecDeque<f32>) {
        if self.demo {
            let mut sh = Shared::default();
            for i in 0..HISTORY {
                sh.take_status(demo_status(now - (HISTORY - i) as f64));
            }
            return (sh.status, sh.rpm, sh.cpu, sh.gpu);
        }
        match self.shared.lock() {
            Ok(sh) => (sh.status.clone(), sh.rpm.clone(), sh.cpu.clone(), sh.gpu.clone()),
            Err(_) => (None, HashMap::new(), VecDeque::new(), VecDeque::new()),
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
    }

    fn on_exit(&mut self) {
        if let Some(mut h) = self.helper.take() {
            h.stop();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.started {
            self.start();
        }
        self.watch();
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        let now = ui.input(|i| i.time);

        let mut give_back = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| theme::page_header(ui, "Fan Tuner", "Live fan speeds and temperatures. Fans always go back to the BIOS when PC Tuner closes."));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                let b = theme::danger_button("⟲  Give all fans back to BIOS");
                if ui.add_enabled(self.any_controlled(), b).on_hover_text("Every fan goes back to its BIOS / graphics-driver curve.").clicked() {
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
        let (status, rpm_hist, cpu_hist, gpu_hist) = self.snapshot(now);
        let can = common::is_admin() || self.demo;
        let afterburner = self.afterburner.load(Ordering::Relaxed);
        let mut actions: Vec<(String, f32, CardAction)> = vec![];
        egui::ScrollArea::vertical().id_salt("fan-page").auto_shrink([false, false]).show(ui, |ui| {
            let Some(s) = status else {
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Reading sensors...").size(18.0).strong());
                    ui.add_space(6.0);
                    theme::sweep(ui, ui.available_width());
                });
                return;
            };
            hero_card(ui, &s, &cpu_hist, &gpu_hist);
            self.profile_bar(ui, &s.fans, can);
            let view = View { settings: &self.settings, can, afterburner };
            fans_and_temps(ui, &s, &rpm_hist, &view, &mut actions);
        });
        for (id, floor, a) in actions {
            match a {
                CardAction::Set(FanSetting::Manual { percent }) => {
                    self.settings.insert(id, FanSetting::Manual { percent: percent.max(floor) });
                }
                CardAction::Set(FanSetting::Curve { source, points }) => {
                    let points = points.into_iter().map(|p| [p[0], p[1].clamp(floor, 100.0)]).collect();
                    self.settings.insert(id, FanSetting::Curve { source, points });
                }
                CardAction::Set(s) => {
                    self.settings.insert(id, s);
                }
                CardAction::Pump(p) => {
                    if p {
                        match self.settings.get_mut(&id) {
                            Some(FanSetting::Manual { percent }) => *percent = percent.max(PUMP_FLOOR),
                            Some(FanSetting::Curve { points, .. }) => points.iter_mut().for_each(|pt| pt[1] = pt[1].max(PUMP_FLOOR)),
                            _ => {}
                        }
                    }
                    self.pumps.insert(id, p);
                }
            }
        }
        self.sync();
    }
}

/// Read-only view of the tab state the page needs.
struct View<'a> {
    settings: &'a HashMap<String, FanSetting>,
    can: bool,
    afterburner: bool,
}

/// Banners plus the hero card: the hottest of CPU and GPU, with live graphs.
fn hero_card(ui: &mut egui::Ui, s: &Status, cpu_hist: &VecDeque<f32>, gpu_hist: &VecDeque<f32>) {
    if s.emergency {
        theme::banner(ui, theme::LOCKED, "Over 90 °C - every controlled fan is at 100 % until the CPU and GPU are back under 80 °C.");
    }
    if !s.msg.is_empty() {
        theme::banner(ui, theme::GAMING, &s.msg);
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
            ui.add_space(22.0);
            theme::stat(ui, &s.fans.len().to_string(), "fans found", theme::TEXT);
            ui.add_space(22.0);
            let ctl = s.fans.iter().filter(|f| f.controllable).count();
            theme::stat(ui, &ctl.to_string(), "can be controlled", theme::TEXT);
        });
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
    for row in s.fans.chunks(3) {
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
                    let ours = *setting != FanSetting::Bios;
                    if ours && emergency {
                        theme::chip(ui, "100 % - hot", theme::LOCKED);
                    } else if matches!(setting, FanSetting::Curve { .. }) {
                        theme::chip(ui, "Curve", theme::ACCENT);
                    } else if ours {
                        theme::chip(ui, "PC Tuner", theme::ACCENT);
                    } else {
                        theme::chip(ui, "BIOS", theme::DIM);
                    }
                } else {
                    theme::chip(ui, "read-only", theme::DIM);
                }
                if f.pump {
                    theme::chip(ui, "pump", theme::OPTIONAL);
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
                let changed = curve_editor(ui, &f.id, &mut pts, floor, live);
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

/// Draggable fan curve. Returns true when a point moved.
/// x = temperature 20-100 °C, y = duty 0-100 %. The shaded band is below the fan's floor; the red line is the 90 °C override.
fn curve_editor(ui: &mut egui::Ui, id: &str, points: &mut [[f32; 2]], floor: f32, live: Option<f32>) -> bool {
    let size = egui::vec2(ui.available_width(), 150.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
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
    painter.text(egui::pos2(plot.right() - 4.0, fy + 2.0), egui::Align2::RIGHT_TOP, format!("min {floor:.0} %"), egui::FontId::proportional(10.5), theme::tint(theme::LOCKED, 200));
    // 90 °C: every fan to 100 %
    let x90 = to_screen(90.0, 0.0).x;
    painter.line_segment([egui::pos2(x90, plot.top()), egui::pos2(x90, plot.bottom())], egui::Stroke::new(1.0_f32, theme::tint(theme::LOCKED, 160)));
    painter.text(egui::pos2(x90 + 3.0, plot.top()), egui::Align2::LEFT_TOP, "90°", egui::FontId::proportional(10.5), theme::tint(theme::LOCKED, 200));
    painter.text(plot.left_bottom() + egui::vec2(2.0, -2.0), egui::Align2::LEFT_BOTTOM, "20 °C", egui::FontId::proportional(10.5), theme::TEXT_FAINT);
    painter.text(plot.left_top() + egui::vec2(2.0, 0.0), egui::Align2::LEFT_TOP, "100 %", egui::FontId::proportional(10.5), theme::TEXT_FAINT);

    // drag the points (temperatures stay in order, 2 °C apart)
    let mut changed = false;
    let n = points.len();
    for i in 0..n {
        let p = to_screen(points[i][0], points[i][1]);
        let hit = egui::Rect::from_center_size(p, egui::vec2(18.0, 18.0));
        let resp = ui.interact(hit, ui.id().with(("fan-pt", id, i)), egui::Sense::drag());
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (t, d) = from_screen(pos);
                let lo = if i > 0 { points[i - 1][0] + 2.0 } else { GRAPH_T.0 };
                let hi = if i + 1 < n { points[i + 1][0] - 2.0 } else { GRAPH_T.1 };
                let nt = t.round().clamp(lo, hi.max(lo));
                let nd = d.round().clamp(floor, 100.0);
                if nt != points[i][0] || nd != points[i][1] {
                    points[i] = [nt, nd];
                    changed = true;
                }
            }
        }
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    // the curve, flat beyond the first and last points
    let mut line = vec![to_screen(GRAPH_T.0, points.first().map(|p| p[1]).unwrap_or(100.0).max(floor))];
    line.extend(points.iter().map(|p| to_screen(p[0], p[1].max(floor))));
    line.push(to_screen(GRAPH_T.1, points.last().map(|p| p[1]).unwrap_or(100.0).max(floor)));
    painter.add(egui::Shape::line(line, egui::Stroke::new(2.0_f32, theme::ACCENT)));
    for (i, p) in points.iter().enumerate() {
        let c = to_screen(p[0], p[1].max(floor));
        let hot = ui.ctx().is_being_dragged(ui.id().with(("fan-pt", id, i)));
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
        let label = format!("{t:.0} °C → {duty:.0} %");
        let align = if c.x > plot.center().x { egui::Align2::RIGHT_BOTTOM } else { egui::Align2::LEFT_BOTTOM };
        let off = if c.x > plot.center().x { egui::vec2(-8.0, -6.0) } else { egui::vec2(8.0, -6.0) };
        painter.text(c + off, align, label, egui::FontId::proportional(11.5), theme::GAMING);
    }
    changed
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
        let pumps = HashMap::new();
        let silent = builtin_profile("Silent", &fans, &pumps).unwrap();
        let perf = builtin_profile("Performance", &fans, &pumps).unwrap();
        assert!(builtin_profile("Nope", &fans, &pumps).is_none());
        // only controllable fans are listed; Silent leaves the GPU on its own curve
        assert_eq!(perf.fans.len(), fans.iter().filter(|f| f.controllable).count());
        assert!(!silent.fans.contains_key("/gpu-nvidia/0/control/1"));
        assert!(perf.fans.contains_key("/gpu-nvidia/0/control/1"));
        assert!(!perf.fans.contains_key("/lpc/nct6687d/0/fan/2"), "read-only fans are never in a profile");
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

    #[test]
    fn fan_temp_colours() {
        assert_eq!(temp_color(50.0), theme::ACCENT);
        assert_eq!(temp_color(75.0), theme::GAMING);
        assert_eq!(temp_color(88.0), theme::LOCKED);
    }
}
