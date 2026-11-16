//! Processes tab: background processes that can be switched off or given a lower priority without
//! breaking Windows or getting in the way of games, plus a before / after measurement.
//!
//! Only the processes in `CATALOG` are ever changed. Every change writes registry values only:
//!   - "Lower": Image File Execution Options\<exe>\PerfOptions  CpuPriorityClass = 5 (below normal),
//!     IoPriority = 1 (low). Windows applies it each time the program starts; the running copy is
//!     lowered at once as well.
//!   - "Off": the feature's own setting (the same one the Settings app writes) + "Lower", and the
//!     running copy is closed.
//! Each change keeps what it replaced, so Undo puts back exactly what was there (or removes what
//! was added). No file is deleted or renamed and no service is touched.

use crate::common::{self, Job, Tone};
use crate::sysinfo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use winreg::enums::*;
use winreg::RegKey;

pub const HISTORY: &str = "processes-history.json";
pub const MEASURES: &str = "processes-measure.json";
pub const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
/// Below normal (5) CPU priority and low (1) disk priority. Never anything higher than normal.
pub const CPU_BELOW_NORMAL: u32 = 5;
pub const IO_LOW: u32 = 1;
/// Seconds one measurement samples for.
pub const MEASURE_SECS: u64 = 30;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// The feature can be switched off (and the process is lowered too, in case Windows starts it anyway).
    Off,
    /// Kept, but at below normal CPU and low disk priority.
    Lower,
    /// Never touched: Windows or games need it. Listed so the user knows why.
    Keep,
}

/// A feature setting the Settings app writes (HKCU), set to `off` to switch the feature off.
pub struct Setting {
    pub path: &'static str,
    pub name: &'static str,
    pub off: u32,
}

pub struct Entry {
    /// Process name without ".exe", as Get-Process shows it.
    pub exe: &'static str,
    pub title: &'static str,
    /// What it does, in plain English.
    pub what: &'static str,
    /// What you lose when it is switched off / lowered (Keep: why it is left alone).
    pub loses: &'static str,
    pub kind: Kind,
    pub settings: &'static [Setting],
}

const RESUME: &str = r"Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration";
const TABTIP: &str = r"Software\Microsoft\TabletTip\1.7";

pub const CATALOG: &[Entry] = &[
    Entry {
        exe: "CrossDeviceResume",
        title: "Resume from phone",
        what: "Offers to continue on this PC what you were doing in an app or OneDrive file on your phone.",
        loses: "The \"resume on PC\" pop-ups from your phone (Settings > Apps > Resume). Phone Link itself is not affected.",
        kind: Kind::Off,
        settings: &[Setting { path: RESUME, name: "IsResumeAllowed", off: 0 }, Setting { path: RESUME, name: "IsOneDriveResumeAllowed", off: 0 }],
    },
    Entry {
        exe: "TabTip",
        title: "Touch keyboard",
        what: "The on-screen keyboard for touchscreens and tablets.",
        loses: "The touch keyboard no longer pops up by itself (Settings > Time & language > Typing > Touch keyboard: Never). Your real keyboard, emoji panel (Win + .) and the Accessibility on-screen keyboard still work.",
        kind: Kind::Off,
        settings: &[Setting { path: TABTIP, name: "TouchKeyboardTapInvoke", off: 0 }, Setting { path: TABTIP, name: "EnableDesktopModeAutoInvoke", off: 0 }],
    },
    Entry {
        exe: "AppActions",
        title: "App actions",
        what: "Suggests actions from apps for what is on screen (Click to Do).",
        loses: "Nothing - suggestions may appear a moment later.",
        kind: Kind::Lower,
        settings: &[],
    },
    Entry {
        exe: "WidgetBoard",
        title: "Widgets board",
        what: "The news and weather Widgets panel (Win + W).",
        loses: "Nothing - Widgets may open a little slower. (Remove Bloat can remove Widgets completely.)",
        kind: Kind::Lower,
        settings: &[],
    },
    Entry {
        exe: "WidgetService",
        title: "Widgets service",
        what: "Feeds data to the Widgets panel.",
        loses: "Nothing - widget data may refresh a little later.",
        kind: Kind::Lower,
        settings: &[],
    },
    Entry {
        exe: "SecurityHealthSystray",
        title: "Windows Security tray icon",
        what: "Only the shield icon in the taskbar corner. Defender itself is a different process and keeps full priority.",
        loses: "Nothing - the icon reacts a moment later.",
        kind: Kind::Lower,
        settings: &[],
    },
    Entry {
        exe: "PhoneExperienceHost",
        title: "Phone Link",
        what: "Phone Link's background part (notifications and messages from your phone).",
        loses: "Nothing - phone notifications may arrive a moment later.",
        kind: Kind::Lower,
        settings: &[],
    },
    Entry {
        exe: "vctip",
        title: "Visual Studio build telemetry",
        what: "Sends compiler usage data to Microsoft while you build code.",
        loses: "Nothing.",
        kind: Kind::Lower,
        settings: &[],
    },
    // ---------------------------------------------------------------- never touched
    Entry { exe: "HaloInfinite", title: "Your games", what: "Games and their launchers.", loses: "A game gets the priority Windows and Game Mode give it; anti-cheat can flag a game whose priority is changed by another program.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "EasyAntiCheat_EOS", title: "Easy Anti-Cheat", what: "Anti-cheat for online games.", loses: "Changing it can get you kicked from matches.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "GameBar", title: "Xbox Game Bar", what: "Game Bar, captures and the Xbox overlay.", loses: "Needed for Game Mode, captures and Xbox party chat.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "gamingservices", title: "Gaming Services", what: "Installs and runs Game Pass / Xbox games.", loses: "Xbox and Game Pass games stop starting or updating.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "XboxPcAppFT", title: "Xbox app", what: "The Xbox app's background helper.", loses: "Game Pass downloads and sign-in depend on it.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "GameInputRedistService", title: "GameInput", what: "Controller and input handling for games.", loses: "Controllers and input in games.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "NVDisplay.Container", title: "NVIDIA display container", what: "The NVIDIA driver's helper (control panel, overlay, G-SYNC settings).", loses: "Driver features and the NVIDIA overlay.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "audiodg", title: "Windows audio", what: "Plays all sound.", loses: "A lower priority causes crackling audio.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "dwm", title: "Desktop Window Manager", what: "Draws every window and the game's frames to the screen.", loses: "Lowering it causes stutter.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "MsMpEng", title: "Microsoft Defender", what: "Antivirus.", loses: "Protected by Windows; also your protection.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "explorer", title: "Explorer", what: "The taskbar, Start button and file windows.", loses: "The desktop itself.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "ctfmon", title: "Text input (ctfmon)", what: "Keyboard input and language switching in every app.", loses: "Typing in some apps stops working.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "TextInputHost", title: "Text input host", what: "Emoji panel, clipboard history and input methods.", loses: "Win + . and Win + V stop working.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "SearchHost", title: "Start search", what: "Search in the Start menu.", loses: "Typing in Start would lag.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "StartMenuExperienceHost", title: "Start menu", what: "The Start menu.", loses: "Start would open slowly.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "RuntimeBroker", title: "Runtime Broker", what: "Checks app permissions, including for Store games.", loses: "Apps and Store games wait on it.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "sihost", title: "Shell infrastructure host", what: "Runs parts of the desktop (notifications, action center).", loses: "Desktop features break.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "svchost", title: "Service hosts", what: "Windows services (change them on the Services page instead).", loses: "Many services share one process name.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "msedgewebview2", title: "WebView2", what: "The web engine inside apps such as Claude, Widgets and Xbox.", loses: "Every app that uses it.", kind: Kind::Keep, settings: &[] },
    Entry { exe: "ProcessLasso", title: "Process Lasso", what: "Your process priority manager.", loses: "Its own rules decide; if it has a rule for a process here, its rule wins.", kind: Kind::Keep, settings: &[] },
];

pub fn entry(exe: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.exe.eq_ignore_ascii_case(exe))
}

// ------------------------------------------------------------------ registry with backups

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Hive {
    Hklm,
    Hkcu,
}

impl Hive {
    fn root(self) -> RegKey {
        RegKey::predef(match self {
            Hive::Hklm => HKEY_LOCAL_MACHINE,
            Hive::Hkcu => HKEY_CURRENT_USER,
        })
    }
    fn label(self) -> &'static str {
        match self {
            Hive::Hklm => "HKLM",
            Hive::Hkcu => "HKCU",
        }
    }
}

/// One DWORD write and what it replaced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Write {
    pub hive: Hive,
    pub path: String,
    pub name: String,
    pub value: u32,
    /// The value before (None = there was none).
    pub old: Option<u32>,
    /// Keys this write created (deepest last), removed again by Undo when they are empty.
    pub created: Vec<String>,
}

impl Write {
    pub fn new(hive: Hive, path: &str, name: &str, value: u32) -> Self {
        Write { hive, path: path.into(), name: name.into(), value, old: None, created: vec![] }
    }
    pub fn line(&self) -> String {
        format!("{}\\{}  {} = {}", self.hive.label(), self.path, self.name, self.value)
    }
}

fn read_dword(hive: Hive, path: &str, name: &str) -> Option<u32> {
    hive.root().open_subkey(path).ok()?.get_value::<u32, _>(name).ok()
}

/// The keys along `path` that do not exist yet (outermost first).
fn missing_keys(hive: Hive, path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('\\').collect();
    (1..=parts.len()).map(|n| parts[..n].join("\\")).filter(|p| hive.root().open_subkey(p).is_err()).collect()
}

/// Do the write, filling in `old` and `created`.
fn apply_write(w: &mut Write) -> Result<(), String> {
    w.old = read_dword(w.hive, &w.path, &w.name);
    w.created = missing_keys(w.hive, &w.path);
    let (k, _) = w.hive.root().create_subkey(&w.path).map_err(|e| format!("cannot open {}\\{}: {e}", w.hive.label(), w.path))?;
    k.set_value(&w.name, &w.value).map_err(|e| format!("cannot set {}: {e}", w.line()))
}

/// Put back what the write replaced.
fn undo_write(w: &Write) -> Result<(), String> {
    let root = w.hive.root();
    match w.old {
        Some(v) => {
            let (k, _) = root.create_subkey(&w.path).map_err(|e| format!("cannot open {}\\{}: {e}", w.hive.label(), w.path))?;
            k.set_value(&w.name, &v).map_err(|e| format!("cannot restore {}: {e}", w.name))?;
        }
        None => {
            if let Ok(k) = root.open_subkey_with_flags(&w.path, KEY_SET_VALUE) {
                let _ = k.delete_value(&w.name);
            }
        }
    }
    // keys this write created, deepest first, only while empty
    for p in w.created.iter().rev() {
        let empty = root.open_subkey(p).map(|k| k.enum_keys().next().is_none() && k.enum_values().next().is_none()).unwrap_or(false);
        if empty {
            let _ = root.delete_subkey(p);
        }
    }
    Ok(())
}

/// The writes for an entry: its feature settings (Off only) and the priority values.
pub fn writes_for(e: &Entry) -> Vec<Write> {
    let mut v: Vec<Write> = vec![];
    if e.kind == Kind::Keep {
        return v;
    }
    if e.kind == Kind::Off {
        v.extend(e.settings.iter().map(|s| Write::new(Hive::Hkcu, s.path, s.name, s.off)));
    }
    let perf = format!(r"{IFEO}\{}.exe\PerfOptions", e.exe);
    v.push(Write::new(Hive::Hklm, &perf, "CpuPriorityClass", CPU_BELOW_NORMAL));
    v.push(Write::new(Hive::Hklm, &perf, "IoPriority", IO_LOW));
    v
}

/// What is set now for an entry.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    /// IFEO priority below normal or lower is in place.
    pub lowered: bool,
    /// Every feature setting is at its "off" value (false for entries without settings).
    pub off: bool,
    /// Another program (e.g. Process Lasso) set a priority that is not ours.
    pub other_priority: Option<u32>,
}

pub fn read_setup(e: &Entry) -> Setup {
    let perf = format!(r"{IFEO}\{}.exe\PerfOptions", e.exe);
    let cpu = read_dword(Hive::Hklm, &perf, "CpuPriorityClass");
    let off = !e.settings.is_empty() && e.settings.iter().all(|s| read_dword(Hive::Hkcu, s.path, s.name) == Some(s.off));
    Setup {
        lowered: matches!(cpu, Some(1) | Some(5)),
        off,
        other_priority: cpu.filter(|c| *c != 1 && *c != 5),
    }
}

// ------------------------------------------------------------------ history

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum What {
    Off,
    Lower,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub exe: String,
    pub what: What,
    pub time: String,
    pub writes: Vec<Write>,
    pub undone: bool,
    /// "closed the running copy" / "lowered the running copy" / error notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Apply Off or Lower to an entry: registry writes (rolled back on a failure), then the running copy.
pub fn apply(e: &Entry, what: What) -> Result<Change, String> {
    if e.kind == Kind::Keep || (what == What::Off && e.kind != Kind::Off) {
        return Err(format!("{} is not changed by PC Tuner", e.title));
    }
    let mut writes: Vec<Write> = writes_for(e).into_iter().filter(|w| what == What::Off || w.hive == Hive::Hklm).collect();
    for i in 0..writes.len() {
        if let Err(err) = apply_write(&mut writes[i]) {
            for w in writes[..i].iter().rev() {
                let _ = undo_write(w);
            }
            return Err(err);
        }
    }
    let mut notes = vec![];
    let running = running_count(e.exe);
    if running > 0 {
        if what == What::Off {
            match common::run("taskkill.exe", &["/F", "/IM", &format!("{}.exe", e.exe)]) {
                Ok((0, _)) => notes.push("closed the running copy".into()),
                Ok((_, out)) => notes.push(format!("could not close the running copy: {}", out.trim())),
                Err(err) => notes.push(format!("could not close the running copy: {err}")),
            }
        } else {
            let script = format!("Get-Process -Name {} -ErrorAction SilentlyContinue | ForEach-Object {{ $_.PriorityClass = 'BelowNormal' }}", common::psq(e.exe));
            match common::ps(&script) {
                Ok(_) => notes.push("lowered the running copy".into()),
                Err(err) => notes.push(format!("the running copy keeps its priority until it restarts ({})", err.lines().next().unwrap_or("").trim())),
            }
        }
    }
    Ok(Change { exe: e.exe.into(), what, time: common::now(), writes, undone: false, notes })
}

/// Windows starts some switched-off processes at sign-in anyway (sihost starts Resume from phone, the
/// text input service starts the touch keyboard). This task closes them shortly after every sign-in.
pub const CLOSE_TASK_PATH: &str = r"\PCTuner\";
pub const CLOSE_TASK_NAME: &str = "CloseSwitchedOff";

/// The PowerShell that makes the sign-in task match `exes` (removed when the list is empty).
pub fn close_task_script(exes: &[String]) -> String {
    // only catalog names, so nothing from outside reaches the command line
    let exes: Vec<&str> = exes.iter().filter_map(|x| entry(x)).filter(|e| e.kind == Kind::Off).map(|e| e.exe).collect();
    let unregister = format!("Unregister-ScheduledTask -TaskName {} -TaskPath {} -Confirm:$false -ErrorAction SilentlyContinue", common::psq(CLOSE_TASK_NAME), common::psq(CLOSE_TASK_PATH));
    if exes.is_empty() {
        return unregister;
    }
    let ims: Vec<String> = exes.iter().map(|e| format!("/IM {e}.exe")).collect();
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $a = New-ScheduledTaskAction -Execute 'taskkill.exe' -Argument {}\n\
         $t1 = New-ScheduledTaskTrigger -AtLogOn; $t1.Delay = 'PT30S'\n\
         $t2 = New-ScheduledTaskTrigger -AtLogOn; $t2.Delay = 'PT2M'\n\
         $p = New-ScheduledTaskPrincipal -UserId 'S-1-5-18' -RunLevel Highest\n\
         $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 1)\n\
         Register-ScheduledTask -TaskName {} -TaskPath {} -Description 'PC Tuner: closes the processes switched off on the Processes page, after sign-in. Undo on that page removes it.' -Action $a -Trigger $t1,$t2 -Principal $p -Settings $s -Force | Out-Null",
        common::psq(&format!("/F {}", ims.join(" "))),
        common::psq(CLOSE_TASK_NAME),
        common::psq(CLOSE_TASK_PATH),
    )
}

/// Make the sign-in task close exactly `exes`.
pub fn sync_close_task(exes: &[String]) -> Result<(), String> {
    common::ps(&close_task_script(exes)).map(|_| ()).map_err(|e| format!("the sign-in task could not be updated: {}", e.lines().next().unwrap_or("").trim()))
}

/// Is the sign-in task there?
pub fn close_task_exists() -> bool {
    common::run("schtasks.exe", &["/Query", "/TN", &format!("{CLOSE_TASK_PATH}{CLOSE_TASK_NAME}")]).map(|(c, _)| c == 0).unwrap_or(false)
}

pub fn undo(c: &Change) -> Result<(), String> {
    let mut errs = vec![];
    for w in c.writes.iter().rev() {
        if let Err(e) = undo_write(w) {
            errs.push(e);
        }
    }
    if errs.is_empty() { Ok(()) } else { Err(errs.join("; ")) }
}

// ------------------------------------------------------------------ live processes and measuring

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ProcUse {
    pub count: u32,
    pub mb: f64,
    /// CPU seconds used so far (all copies), None when Windows does not let us read it.
    pub cpu_s: Option<f64>,
}

#[derive(Deserialize)]
struct PsProc {
    #[serde(rename = "N")]
    n: String,
    #[serde(rename = "W")]
    w: f64,
    #[serde(rename = "C")]
    c: Option<f64>,
}

fn names_list() -> String {
    CATALOG.iter().map(|e| common::psq(e.exe)).collect::<Vec<_>>().join(",")
}

/// Memory and CPU time of every catalog process that is running.
pub fn snapshot() -> BTreeMap<String, ProcUse> {
    let script = format!("Get-Process -Name {} -ErrorAction SilentlyContinue | ForEach-Object {{ [pscustomobject]@{{ N = $_.ProcessName; W = [double]$_.WorkingSet64; C = $_.CPU }} }}", names_list());
    let mut m: BTreeMap<String, ProcUse> = BTreeMap::new();
    for p in common::ps_json::<PsProc>(&script).unwrap_or_default() {
        let Some(e) = entry(&p.n) else { continue };
        let u = m.entry(e.exe.to_string()).or_default();
        u.count += 1;
        u.mb += p.w / 1_048_576.0;
        u.cpu_s = match (u.cpu_s, p.c) {
            (Some(a), Some(b)) => Some(a + b),
            (None, b) if u.count == 1 => b,
            (a, _) => a,
        };
    }
    m
}

fn running_count(exe: &str) -> usize {
    let script = format!("@(Get-Process -Name {} -ErrorAction SilentlyContinue).Count", common::psq(exe));
    common::ps(&script).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ProcMeasure {
    pub count: u32,
    pub mb: f64,
    /// Average CPU use over the measurement, % of the whole processor.
    pub cpu_pct: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Measure {
    pub time: String,
    pub secs: u64,
    pub procs: BTreeMap<String, ProcMeasure>,
    /// Whole-PC CPU use (average) and memory in use at the end.
    pub pc_cpu: Option<f64>,
    pub ram_used_mb: Option<u64>,
    /// Changes in effect when it was taken: "TabTip off", "AppActions lowered"...
    #[serde(default)]
    pub setup: Vec<String>,
    /// Minutes since Windows started.
    #[serde(default)]
    pub uptime_min: Option<u64>,
}

/// Processes that mean a game is running (the game itself, or an anti-cheat that only runs with one).
pub const GAME_SIGNS: &[&str] = &["HaloInfinite", "EasyAntiCheat_EOS"];

/// Why two measurements are not a fair before / after comparison (empty = they are).
pub fn unlike(before: &Measure, after: &Measure) -> Vec<String> {
    let mut v = vec![];
    let game = |m: &Measure| GAME_SIGNS.iter().any(|g| m.procs.contains_key(*g));
    match (game(before), game(after)) {
        (true, false) => v.push("A game was running during the first measurement but not the second, so the whole-PC numbers mostly show the game closing.".to_string()),
        (false, true) => v.push("A game was running during the second measurement but not the first, so the whole-PC numbers mostly show the game.".to_string()),
        _ => {}
    }
    if let (Some(a), Some(b)) = (before.ram_used_mb, after.ram_used_mb) {
        let d = a.abs_diff(b);
        if d > 3000 && !v.iter().any(|s| s.contains("game")) {
            v.push(format!("Memory in use differs by {:.1} GB, so the PC was doing different things in the two measurements.", d as f64 / 1024.0));
        }
    }
    if after.uptime_min.is_some_and(|u| u < 5) {
        v.push("The second measurement was taken within 5 minutes of starting Windows; some processes start later, so measure again after a few minutes.".to_string());
    }
    v
}

fn uptime_min() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() / 60_000 }
}

impl Measure {
    /// Memory of the listed changeable processes (Off + Lower), MB.
    pub fn listed_mb(&self) -> f64 {
        self.procs.iter().filter(|(k, _)| entry(k).is_some_and(|e| e.kind != Kind::Keep)).map(|(_, p)| p.mb).sum()
    }
    /// CPU of the listed changeable processes, %.
    pub fn listed_cpu(&self) -> f64 {
        self.procs.iter().filter(|(k, _)| entry(k).is_some_and(|e| e.kind != Kind::Keep)).filter_map(|(_, p)| p.cpu_pct).sum()
    }
}

/// Turn two snapshots `secs` apart into a measurement.
pub fn measure_from(a: &BTreeMap<String, ProcUse>, b: &BTreeMap<String, ProcUse>, secs: f64, cores: f64) -> BTreeMap<String, ProcMeasure> {
    b.iter()
        .map(|(k, u)| {
            let cpu = match (a.get(k).and_then(|x| x.cpu_s), u.cpu_s) {
                (Some(x), Some(y)) if secs > 0.0 && cores > 0.0 => Some(((y - x).max(0.0) / secs / cores * 100.0).min(100.0)),
                _ => None,
            };
            (k.clone(), ProcMeasure { count: u.count, mb: u.mb, cpu_pct: cpu })
        })
        .collect()
}

fn cores() -> f64 {
    std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0)
}

/// Sample for `secs` seconds (blocking; run it in a Job).
pub fn measure(secs: u64, setup: Vec<String>) -> Measure {
    let mut load = sysinfo::CpuLoad::default();
    let _ = load.sample();
    let start = Instant::now();
    let a = snapshot();
    let mut cpu = vec![];
    while start.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(c) = load.sample() {
            cpu.push(c as f64);
        }
    }
    let b = snapshot();
    let took = start.elapsed().as_secs_f64();
    Measure {
        time: common::now(),
        secs,
        procs: measure_from(&a, &b, took, cores()),
        pc_cpu: if cpu.is_empty() { None } else { Some(cpu.iter().sum::<f64>() / cpu.len() as f64) },
        ram_used_mb: sysinfo::memory().map(|(u, _)| u),
        setup,
        uptime_min: Some(uptime_min()),
    }
}

// ------------------------------------------------------------------ the tab's state

pub struct Procs {
    pub history: Vec<Change>,
    pub measures: Vec<Measure>,
    pub setups: Vec<Setup>,
    pub live: BTreeMap<String, ProcUse>,
    pub scan: Option<Job<(Vec<Setup>, BTreeMap<String, ProcUse>, bool)>>,
    /// The sign-in task is there (read with every scan).
    pub task: Option<bool>,
    pub task_tried: bool,
    pub work: Option<(String, Job<Result<Option<Change>, String>>)>,
    pub measuring: Option<(Instant, Job<Measure>)>,
    pub note: Option<(bool, String)>,
}

impl Default for Procs {
    fn default() -> Self {
        Self::new()
    }
}

impl Procs {
    pub fn new() -> Self {
        Procs {
            history: common::load_json(HISTORY),
            measures: common::load_json(MEASURES),
            setups: vec![],
            live: BTreeMap::new(),
            scan: None,
            task: None,
            task_tried: false,
            work: None,
            measuring: None,
            note: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.work.is_some()
    }

    pub fn rescan(&mut self) {
        if self.scan.is_none() {
            self.scan = Some(Job::spawn(|| (CATALOG.iter().map(read_setup).collect(), snapshot(), close_task_exists())));
        }
    }

    pub fn pump(&mut self) {
        if let Some(j) = &self.scan {
            if let Some((s, l, t)) = j.poll() {
                self.setups = s;
                self.live = l;
                self.task = Some(t);
                self.scan = None;
                // processes switched off before the sign-in task existed (or the task was deleted): add it
                let offs = self.off_list();
                if !t && !offs.is_empty() && common::is_admin() && self.work.is_none() && !self.task_tried {
                    // once per run, so a failure does not retry on every scan
                    self.task_tried = true;
                    self.work = Some(("Sign-in task added".into(), Job::spawn(move || sync_close_task(&offs).map(|_| None))));
                }
            }
        }
        if let Some((label, j)) = &self.work {
            if let Some(r) = j.poll() {
                let label = label.clone();
                self.work = None;
                match r {
                    Ok(Some(c)) => {
                        let extra = if c.notes.is_empty() { String::new() } else { format!(" ({})", c.notes.join(", ")) };
                        self.note = Some((true, format!("{label}: done{extra}.")));
                        self.history.push(c);
                    }
                    Ok(None) => self.note = Some((true, format!("{label}: done."))),
                    Err(e) => self.note = Some((false, format!("{label}: {e}"))),
                }
                common::save_json(HISTORY, &self.history);
                self.rescan();
            }
        }
        if let Some((_, j)) = &self.measuring {
            if let Some(m) = j.poll() {
                self.measuring = None;
                self.measures.push(m);
                // keep the last 20
                let n = self.measures.len();
                if n > 20 {
                    self.measures.drain(..n - 20);
                }
                common::save_json(MEASURES, &self.measures);
                self.rescan();
            }
        }
    }

    /// The active (not undone) change for a process, if any.
    pub fn active(&self, exe: &str) -> Option<usize> {
        self.history.iter().rposition(|c| !c.undone && c.exe.eq_ignore_ascii_case(exe))
    }

    pub fn apply(&mut self, exe: &str, what: What) {
        if self.busy() {
            return;
        }
        let Some(e) = entry(exe) else { return };
        if self.active(exe).is_some() {
            self.note = Some((false, format!("{} is already changed - undo it first.", e.title)));
            return;
        }
        let label = format!("{} {}", e.title, if what == What::Off { "switched off" } else { "lowered" });
        let mut offs = self.off_list();
        if what == What::Off {
            offs.push(e.exe.to_string());
        }
        self.work = Some((
            label,
            Job::spawn(move || {
                let mut c = apply(e, what)?;
                if what == What::Off {
                    match sync_close_task(&offs) {
                        Ok(()) => c.notes.push("closed after every sign-in from now on".into()),
                        Err(err) => c.notes.push(err),
                    }
                }
                Ok(Some(c))
            }),
        ));
    }

    /// The processes switched off now (not undone).
    pub fn off_list(&self) -> Vec<String> {
        self.history.iter().filter(|c| !c.undone && c.what == What::Off).map(|c| c.exe.clone()).collect()
    }

    pub fn undo(&mut self, exe: &str) {
        if self.busy() {
            return;
        }
        let Some(i) = self.active(exe) else { return };
        self.history[i].undone = true;
        let c = self.history[i].clone();
        let offs = self.off_list();
        let title = entry(exe).map(|e| e.title).unwrap_or(exe);
        self.work = Some((
            format!("{title} undone"),
            Job::spawn(move || {
                undo(&c)?;
                if c.what == What::Off {
                    sync_close_task(&offs)?;
                }
                Ok(None)
            }),
        ));
    }

    pub fn undo_all(&mut self) {
        if self.busy() {
            return;
        }
        let list: Vec<Change> = self.history.iter_mut().filter(|c| !c.undone).map(|c| {
            c.undone = true;
            c.clone()
        }).collect();
        if list.is_empty() {
            return;
        }
        self.work = Some(("Undo all".into(), Job::spawn(move || {
            let mut errs: Vec<String> = list.iter().rev().filter_map(|c| undo(c).err()).collect();
            if let Err(e) = sync_close_task(&[]) {
                errs.push(e);
            }
            if errs.is_empty() { Ok(None) } else { Err(errs.join("; ")) }
        })));
    }

    /// The changes in effect, as short words for a measurement.
    pub fn setup_words(&self) -> Vec<String> {
        self.history.iter().filter(|c| !c.undone).map(|c| format!("{} {}", c.exe, if c.what == What::Off { "off" } else { "lowered" })).collect()
    }

    pub fn start_measure(&mut self) {
        if self.measuring.is_none() {
            let setup = self.setup_words();
            self.measuring = Some((Instant::now(), Job::spawn(move || measure(MEASURE_SECS, setup))));
        }
    }
}

/// Text and colour of a row's state. `running`: the process is running right now.
pub fn state_text(e: &Entry, s: Option<&Setup>, active: Option<&Change>, running: bool) -> (String, Tone) {
    if e.kind == Kind::Keep {
        return ("left alone".into(), Tone::Dim);
    }
    match (active.map(|c| c.what), s) {
        // Windows started it again (at sign-in, or when something asked for it): say so
        (Some(What::Off), _) if running => ("off, but running again (low priority)".into(), Tone::Warn),
        (Some(What::Off), _) => ("switched off".into(), Tone::Ok),
        (Some(What::Lower), _) => ("lower priority".into(), Tone::Ok),
        (None, Some(s)) if s.off => ("off (set elsewhere)".into(), Tone::Dim),
        (None, Some(s)) if s.lowered => ("low priority (set elsewhere)".into(), Tone::Dim),
        (None, Some(s)) if s.other_priority.is_some() => ("priority set by another program".into(), Tone::Warn),
        _ => ("as Windows set it".into(), Tone::Text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_and_windows_essentials_are_never_changed() {
        let never = [
            "HaloInfinite", "EasyAntiCheat_EOS", "GameBar", "gamingservices", "XboxPcAppFT", "GameInputRedistService", "XboxGameBarWidgets", "NVDisplay.Container",
            "audiodg", "dwm", "MsMpEng", "explorer", "ctfmon", "TextInputHost", "SearchHost", "StartMenuExperienceHost", "RuntimeBroker", "sihost", "svchost",
            "csrss", "lsass", "winlogon", "services", "msedgewebview2", "ProcessLasso", "claude",
        ];
        for e in CATALOG.iter().filter(|e| e.kind != Kind::Keep) {
            assert!(!never.iter().any(|n| n.eq_ignore_ascii_case(e.exe)), "{} must never be changed", e.exe);
        }
        for e in CATALOG.iter().filter(|e| e.kind == Kind::Keep) {
            assert!(writes_for(e).is_empty(), "{} has writes", e.exe);
            assert!(apply(e, What::Lower).is_err());
        }
    }

    #[test]
    fn priority_is_only_ever_lowered() {
        for e in CATALOG.iter().filter(|e| e.kind != Kind::Keep) {
            for w in writes_for(e).iter().filter(|w| w.hive == Hive::Hklm) {
                assert!(w.path.starts_with(IFEO) && w.path.ends_with(&format!(r"\{}.exe\PerfOptions", e.exe)), "{}", w.path);
                match w.name.as_str() {
                    "CpuPriorityClass" => assert!(w.value == 5 || w.value == 1, "CPU priority {} is not below normal", w.value),
                    "IoPriority" => assert!(w.value <= 1, "I/O priority {} is not low", w.value),
                    n => panic!("unexpected value {n}"),
                }
            }
            // feature settings live in the user's own hive only, and only for Off entries
            let user: Vec<_> = writes_for(e).into_iter().filter(|w| w.hive == Hive::Hkcu).collect();
            assert_eq!(user.len(), if e.kind == Kind::Off { e.settings.len() } else { 0 });
        }
        // exe names are unique
        let mut names: Vec<_> = CATALOG.iter().map(|e| e.exe.to_ascii_lowercase()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), CATALOG.len());
    }

    /// Writes and undoes in a scratch HKCU key: undo puts back an old value, removes a new one and
    /// deletes only the keys it created.
    #[test]
    fn undo_restores_exactly_what_was_there() {
        let base = format!(r"Software\PCTunerTest-{}", std::process::id());
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let (k, _) = root.create_subkey(&base).unwrap();
        k.set_value("Kept", &7u32).unwrap();
        // an existing value is replaced, then restored
        let mut w1 = Write::new(Hive::Hkcu, &base, "Kept", 0);
        apply_write(&mut w1).unwrap();
        assert_eq!(read_dword(Hive::Hkcu, &base, "Kept"), Some(0));
        assert_eq!(w1.old, Some(7));
        assert!(w1.created.is_empty());
        // a new value in new keys is removed together with those keys
        let deep = format!(r"{base}\A\B");
        let mut w2 = Write::new(Hive::Hkcu, &deep, "New", 5);
        apply_write(&mut w2).unwrap();
        assert_eq!(w2.old, None);
        assert_eq!(w2.created, vec![format!(r"{base}\A"), deep.clone()]);
        undo_write(&w2).unwrap();
        assert!(root.open_subkey(format!(r"{base}\A")).is_err(), "created keys are removed");
        undo_write(&w1).unwrap();
        assert_eq!(read_dword(Hive::Hkcu, &base, "Kept"), Some(7));
        assert!(root.open_subkey(&base).is_ok(), "a key that was there stays");
        let _ = root.delete_subkey_all(&base);
    }

    #[test]
    fn sign_in_task_closes_only_switched_off_catalog_processes() {
        let s = close_task_script(&["TabTip".into(), "CrossDeviceResume".into(), "AppActions".into(), "calc & evil".into(), "HaloInfinite".into()]);
        assert!(s.contains("'/F /IM TabTip.exe /IM CrossDeviceResume.exe'"), "{s}");
        assert!(!s.contains("AppActions") && !s.contains("evil") && !s.contains("Halo"), "only Off entries from the catalog: {s}");
        assert!(s.contains("Register-ScheduledTask") && s.contains("S-1-5-18") && s.contains("-AtLogOn"));
        // nothing switched off: the task is removed
        let none = close_task_script(&[]);
        assert!(none.starts_with("Unregister-ScheduledTask") && !none.contains("Register-ScheduledTask -"), "{none}");
        assert_eq!(close_task_script(&["AppActions".into()]), none, "a lowered process never gets the task");
    }

    #[test]
    fn switched_off_but_running_again_is_not_called_off() {
        let e = entry("TabTip").unwrap();
        let c = Change { exe: "TabTip".into(), what: What::Off, time: String::new(), writes: vec![], undone: false, notes: vec![] };
        assert_eq!(state_text(e, None, Some(&c), false).1, Tone::Ok);
        let (t, tone) = state_text(e, None, Some(&c), true);
        assert_eq!(tone, Tone::Warn);
        assert!(t.contains("running again"));
    }

    #[test]
    fn unlike_measurements_are_flagged() {
        let with_game = |game: bool, ram: u64, up: u64| {
            let mut procs = BTreeMap::new();
            if game {
                procs.insert("HaloInfinite".to_string(), ProcMeasure { count: 1, mb: 4800.0, cpu_pct: Some(3.8) });
            }
            Measure { procs, ram_used_mb: Some(ram), uptime_min: Some(up), ..Default::default() }
        };
        // the user's real case: Halo in the first, a fresh restart in the second
        let w = unlike(&with_game(true, 17039, 300), &with_game(false, 5315, 1));
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("game") && w[1].contains("5 minutes"));
        assert!(unlike(&with_game(false, 5000, 60), &with_game(false, 5200, 90)).is_empty());
        assert!(unlike(&with_game(false, 5000, 60), &with_game(false, 9000, 90))[0].contains("GB"));
    }

    #[test]
    fn measurement_turns_cpu_seconds_into_percent() {
        let mut a = BTreeMap::new();
        let mut b = BTreeMap::new();
        a.insert("TabTip".to_string(), ProcUse { count: 1, mb: 90.0, cpu_s: Some(10.0) });
        b.insert("TabTip".to_string(), ProcUse { count: 1, mb: 96.0, cpu_s: Some(13.0) });
        b.insert("AppActions".to_string(), ProcUse { count: 1, mb: 40.0, cpu_s: None });
        let m = measure_from(&a, &b, 30.0, 16.0);
        // 3 CPU seconds in 30 s on 16 threads = 0.625 %
        assert!((m["TabTip"].cpu_pct.unwrap() - 0.625).abs() < 1e-9);
        assert_eq!(m["TabTip"].mb, 96.0);
        assert_eq!(m["AppActions"].cpu_pct, None);
        let meas = Measure { procs: m, ..Default::default() };
        assert_eq!(meas.listed_mb(), 136.0);
    }
}
