//! Health: Windows' own repair tools with live progress, and the drives' health.
//!
//!   Disk health  - Get-PhysicalDisk + storage reliability counters, free space on C: (seconds, read only)
//!   CHKDSK       - `chkdsk C: /scan` online; problems can be fixed at the next restart (/spotfix)
//!   DISM         - /CheckHealth -> /ScanHealth -> /RestoreHealth only when the store is repairable
//!   SFC          - `sfc /scannow`, after DISM (Microsoft's order)
//!
//! Why these used to look "stuck at a %":
//!   * DISM, SFC and CHKDSK redraw their percentage with '\r'. They run through
//!     `Stream::run_progress`, so every redraw arrives at once.
//!   * DISM /RestoreHealth really waits (62.3 %, 84.9 %) while it fetches repair files. It only runs
//!     when /ScanHealth found repairable damage, and a live activity line (CPU / disk of the worker
//!     processes) shows it is working; a "looks stuck" warning with Stop appears only when the % has
//!     not moved AND the workers have been idle for STALL_SECS.
//!   * DISM and SFC hang or fail when the services they use are Disabled (the Services tab can do
//!     that). Those are set to Manual for the run and always put back afterwards (also after a
//!     crash: the original types are kept in PREP_FILE until they are restored).
//!
//! Logic only; the UI draws `Health`.

use crate::common::{self, Job, Stream};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

pub const HISTORY: &str = "health-history.json";
/// Services set to Manual for a run, with their original startup type, until they are put back.
pub const PREP_FILE: &str = "health-prep.json";
/// No % change and idle workers for this long = "looks stuck".
pub const STALL_SECS: u64 = 600;
/// A pause this long gets the "still working" note.
pub const PAUSE_NOTE_SECS: u64 = 30;
/// Services DISM / SFC need (Windows Modules Installer, Windows Update, BITS, Cryptographic Services).
pub const NEEDED: [(&str, &str); 4] = [
    ("TrustedInstaller", "Windows Modules Installer"),
    ("wuauserv", "Windows Update"),
    ("BITS", "Background Intelligent Transfer Service"),
    ("CryptSvc", "Cryptographic Services"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum StepId {
    Chkdsk,
    Dism,
    Sfc,
}

pub const STEPS: [StepId; 3] = [StepId::Chkdsk, StepId::Dism, StepId::Sfc];

impl StepId {
    pub fn key(self) -> &'static str {
        match self {
            StepId::Chkdsk => "chkdsk",
            StepId::Dism => "dism",
            StepId::Sfc => "sfc",
        }
    }
    pub fn from_key(k: &str) -> Option<StepId> {
        STEPS.iter().copied().find(|s| s.key() == k)
    }
    pub fn title(self) -> &'static str {
        match self {
            StepId::Chkdsk => "CHKDSK - file system on C:",
            StepId::Dism => "DISM - Windows component store",
            StepId::Sfc => "SFC - Windows system files",
        }
    }
    /// DISM and SFC use Windows Modules Installer and friends.
    pub fn needs_services(self) -> bool {
        self != StepId::Chkdsk
    }
}

/// DISM's three passes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Single,
    Check,
    Scan,
    Restore,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Single => "",
            Phase::Check => "Quick check (1 of 3)",
            Phase::Scan => "Scanning the component store (2 of 3)",
            Phase::Restore => "Repairing from Windows Update (3 of 3)",
        }
    }
}

pub fn command(step: StepId, phase: Phase) -> (String, Vec<String>) {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()) + r"\System32\";
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match (step, phase) {
        (StepId::Chkdsk, _) => (sys + "chkdsk.exe", v(&["C:", "/scan"])),
        (StepId::Sfc, _) => (sys + "sfc.exe", v(&["/scannow"])),
        (StepId::Dism, Phase::Scan) => (sys + "Dism.exe", v(&["/Online", "/Cleanup-Image", "/ScanHealth"])),
        (StepId::Dism, Phase::Restore) => (sys + "Dism.exe", v(&["/Online", "/Cleanup-Image", "/RestoreHealth"])),
        (StepId::Dism, _) => (sys + "Dism.exe", v(&["/Online", "/Cleanup-Image", "/CheckHealth"])),
    }
}

// ------------------------------------------------------------------ reading the tools' output

/// The percentage in a progress line: CHKDSK's "Total: 30%" when present, otherwise the number just
/// before the first '%' (DISM "[==  62.3%  ]", SFC "Verification 45% complete.").
pub fn parse_pct(line: &str) -> Option<f32> {
    let num_before = |s: &str| -> Option<f32> {
        let i = s.find('%')?;
        let head = s[..i].trim_end();
        let start = head.rfind(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).map(|j| j + 1).unwrap_or(0);
        head[start..].replace(',', ".").parse::<f32>().ok().filter(|p| (0.0..=100.0).contains(p))
    };
    if let Some(i) = line.find("Total:") {
        return num_before(&line[i..]);
    }
    num_before(line)
}

/// A line that is only a progress redraw (kept out of the stage text and thinned in the log).
pub fn is_progress(line: &str) -> bool {
    let t = line.trim();
    (t.starts_with('[') && t.contains('%')) || t.starts_with("Verification ") && t.ends_with("complete.") || t.starts_with("Progress:")
}

/// A line worth showing as "what it is doing now".
pub fn stage_text(line: &str) -> Option<String> {
    let t = line.trim();
    if t.is_empty() || is_progress(t) {
        return None;
    }
    let keep = t.starts_with("Stage ") || t.starts_with("Beginning ") || t.contains("phase");
    keep.then(|| common::cap_text(t.trim_end_matches(['.', ' ']), 120))
}

#[derive(Clone, Debug, PartialEq)]
pub enum DismOut {
    Healthy,
    Repairable,
    NotRepairable,
    Repaired,
    Failed(String),
}

pub fn dism_outcome(lines: &[String], code: i32) -> DismOut {
    let has = |s: &str| lines.iter().any(|l| l.contains(s));
    if let Some(i) = lines.iter().position(|l| l.trim_start().starts_with("Error:")) {
        let next = lines[i + 1..].iter().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_default();
        return DismOut::Failed(format!("{} {next}", lines[i].trim()).trim().to_string());
    }
    if has("restore operation completed successfully") {
        DismOut::Repaired
    } else if has("cannot be repaired") {
        DismOut::NotRepairable
    } else if has("is repairable") {
        DismOut::Repairable
    } else if has("No component store corruption detected") {
        DismOut::Healthy
    } else if code == 0 {
        // other languages: a clean exit with none of the English phrases
        DismOut::Healthy
    } else {
        DismOut::Failed(format!("DISM ended with code {code}."))
    }
}

/// What DISM does after a pass (None = done).
pub fn dism_next(phase: Phase, out: &DismOut) -> Option<Phase> {
    match (phase, out) {
        (Phase::Check, DismOut::Healthy) => Some(Phase::Scan),
        (Phase::Check | Phase::Scan, DismOut::Repairable) => Some(Phase::Restore),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SfcOut {
    Clean,
    Repaired,
    Unfixed,
    RebootPending,
    CouldNotStart,
    CouldNotPerform,
    Unknown(i32),
}

pub fn sfc_outcome(lines: &[String], code: i32) -> SfcOut {
    let has = |s: &str| lines.iter().any(|l| l.contains(s));
    if has("did not find any integrity violations") {
        SfcOut::Clean
    } else if has("successfully repaired") {
        SfcOut::Repaired
    } else if has("unable to fix") {
        SfcOut::Unfixed
    } else if has("repair pending") {
        SfcOut::RebootPending
    } else if has("could not start the repair service") {
        SfcOut::CouldNotStart
    } else if has("could not perform") {
        SfcOut::CouldNotPerform
    } else {
        SfcOut::Unknown(code)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChkOut {
    Clean,
    Problems,
    Failed(String),
}

pub fn chkdsk_outcome(lines: &[String], code: i32) -> ChkOut {
    let has = |s: &str| lines.iter().any(|l| l.contains(s));
    if has("found no problems") {
        ChkOut::Clean
    } else if has("found problems") || has("/spotfix") || has("/SPOTFIX") || code == 3 {
        ChkOut::Problems
    } else if code == 0 {
        ChkOut::Clean
    } else {
        let why = lines.iter().rev().find(|l| !l.trim().is_empty() && !is_progress(l)).map(|l| l.trim().to_string()).unwrap_or_default();
        ChkOut::Failed(if why.is_empty() { format!("CHKDSK ended with code {code}.") } else { why })
    }
}

// ------------------------------------------------------------------ results

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StepResult {
    pub time: String,
    pub level: Level,
    pub summary: String,
    #[serde(default)]
    pub detail: Vec<String>,
    pub secs: u64,
    #[serde(default)]
    pub log: String,
    /// CHKDSK found problems: "Fix at next restart" is offered.
    #[serde(default)]
    pub can_fix: bool,
}

fn res(level: Level, summary: &str) -> StepResult {
    StepResult { time: common::now(), level, summary: summary.into(), detail: vec![], secs: 0, log: String::new(), can_fix: false }
}

pub fn dism_result(out: &DismOut, ran_restore: bool) -> StepResult {
    match out {
        DismOut::Healthy => res(Level::Ok, if ran_restore { "Repaired - the component store is healthy now" } else { "No corruption found - nothing needed repairing" }),
        DismOut::Repaired => res(Level::Ok, "Damage found and repaired from Windows Update"),
        DismOut::Repairable => res(Level::Warn, "Damage found that can be repaired - run DISM again to repair it"),
        DismOut::NotRepairable => {
            let mut r = res(Level::Error, "The component store is damaged beyond what DISM can repair");
            r.detail.push("Next step: an in-place repair install (run setup.exe from a Windows 11 ISO and keep your files and apps).".into());
            r
        }
        DismOut::Failed(why) => {
            let mut r = res(Level::Error, "DISM could not finish");
            r.detail.push(why.clone());
            if why.contains("0x800f081f") || why.contains("source files could not be found") {
                r.detail.push("Windows Update could not supply the repair files. Check that Windows Update works, then try again.".into());
            }
            r
        }
    }
}

pub fn sfc_result(out: &SfcOut) -> StepResult {
    match out {
        SfcOut::Clean => res(Level::Ok, "No damaged system files"),
        SfcOut::Repaired => res(Level::Ok, "Damaged system files were found and repaired - restart the PC"),
        SfcOut::Unfixed => res(Level::Error, "Some damaged system files could not be repaired - run DISM, restart, then SFC again"),
        SfcOut::RebootPending => res(Level::Warn, "A repair is waiting for a restart - restart the PC, then run SFC again"),
        SfcOut::CouldNotStart => res(Level::Error, "SFC could not start the repair service (Windows Modules Installer)"),
        SfcOut::CouldNotPerform => res(Level::Error, "SFC could not run - try again after a restart, or in Safe Mode"),
        SfcOut::Unknown(c) => res(if *c == 0 { Level::Ok } else { Level::Warn }, if *c == 0 { "SFC finished" } else { "SFC ended without a clear result - see the log" }),
    }
}

pub fn chkdsk_result(out: &ChkOut) -> StepResult {
    match out {
        ChkOut::Clean => res(Level::Ok, "No file system problems on C:"),
        ChkOut::Problems => {
            let mut r = res(Level::Warn, "Problems found on C: - they can be fixed at the next restart (takes seconds)");
            r.can_fix = true;
            r
        }
        ChkOut::Failed(why) => {
            let mut r = res(Level::Error, "CHKDSK could not scan C:");
            r.detail.push(why.clone());
            r
        }
    }
}

/// SFC's own record of what it repaired or could not repair (the [SR] lines of CBS.log).
pub fn cbs_lines(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter(|l| l.contains("[SR]") && (l.contains("Cannot repair") || l.contains("Repaired file") || l.contains("Repairing corrupted file")))
        .map(|l| common::cap_text(l.split("[SR]").nth(1).unwrap_or(l).trim(), 200))
        .collect();
    v.dedup();
    let n = v.len();
    v.drain(..n.saturating_sub(10));
    v
}

fn read_cbs() -> Vec<String> {
    let p = std::path::Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"Logs\CBS\CBS.log");
    // CBS.log can be large: only its end matters
    let Ok(b) = std::fs::read(&p) else { return vec![] };
    let tail = &b[b.len().saturating_sub(4_000_000)..];
    cbs_lines(&String::from_utf8_lossy(tail))
}

// ------------------------------------------------------------------ the services DISM / SFC need

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Prepped {
    pub name: String,
    /// Startup type before (4 = Disabled).
    pub start: u32,
}

/// Startup type (2 auto, 3 manual, 4 disabled) of a service; None when it does not exist.
pub fn start_type(name: &str) -> Option<u32> {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey(format!(r"SYSTEM\CurrentControlSet\Services\{name}")).ok()?.get_value("Start").ok()
}

/// The needed services that are Disabled now: (name, display name).
pub fn disabled_needed(start: impl Fn(&str) -> Option<u32>) -> Vec<(&'static str, &'static str)> {
    NEEDED.iter().copied().filter(|(n, _)| start(n) == Some(4)).collect()
}

fn sc_config(name: &str, start: u32) -> Result<(), String> {
    let t = match start {
        2 => "auto",
        3 => "demand",
        _ => "disabled",
    };
    let (code, out) = common::run("sc.exe", &["config", name, "start=", t])?;
    if code == 0 { Ok(()) } else { Err(format!("{name}: {}", out.trim())) }
}

/// Set the Disabled needed services to Manual; the originals are saved first (PREP_FILE).
pub fn prep() -> (Vec<Prepped>, Vec<String>) {
    let mut done: Vec<Prepped> = common::load_json(PREP_FILE);
    let mut errs = vec![];
    for (name, _) in disabled_needed(start_type) {
        if done.iter().any(|p| p.name == name) {
            continue;
        }
        done.push(Prepped { name: name.into(), start: 4 });
        common::save_json(PREP_FILE, &done);
        if let Err(e) = sc_config(name, 3) {
            errs.push(e);
            done.retain(|p| p.name != name);
            common::save_json(PREP_FILE, &done);
        }
    }
    (done, errs)
}

/// Put back what `prep` changed. Returns the problems (the file keeps what could not be put back).
pub fn unprep() -> Vec<String> {
    let list: Vec<Prepped> = common::load_json(PREP_FILE);
    let mut left = vec![];
    let mut errs = vec![];
    for p in list {
        if let Err(e) = sc_config(&p.name, p.start) {
            errs.push(e);
            left.push(p);
        }
    }
    common::save_json(PREP_FILE, &left);
    errs
}

// ------------------------------------------------------------------ disk health

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Drive {
    pub name: String,
    pub media: String,
    pub bus: String,
    pub size_gb: f64,
    pub health: String,
    pub status: String,
    pub wear: Option<u32>,
    pub temp: Option<u32>,
    pub temp_max: Option<u32>,
    pub read_errors: Option<u64>,
    pub write_errors: Option<u64>,
    pub hours: Option<u64>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DiskReport {
    pub time: String,
    pub drives: Vec<Drive>,
    pub c_free_gb: Option<f64>,
    pub c_size_gb: Option<f64>,
    pub error: Option<String>,
}

const DISK_SCRIPT: &str = r#"
$c = Get-Volume -DriveLetter C -ErrorAction SilentlyContinue
$drives = @(Get-PhysicalDisk -ErrorAction SilentlyContinue | ForEach-Object {
  $r = $_ | Get-StorageReliabilityCounter -ErrorAction SilentlyContinue
  [pscustomobject]@{
    name = [string]$_.FriendlyName; media = [string]$_.MediaType; bus = [string]$_.BusType
    size_gb = [math]::Round($_.Size / 1GB, 0); health = [string]$_.HealthStatus; status = (@($_.OperationalStatus) -join ', ')
    wear = $(if ($r -and $r.Wear -ne $null) { [int]$r.Wear } else { $null })
    temp = $(if ($r -and $r.Temperature) { [int]$r.Temperature } else { $null })
    temp_max = $(if ($r -and $r.TemperatureMax) { [int]$r.TemperatureMax } else { $null })
    read_errors = $(if ($r -and $r.ReadErrorsTotal -ne $null) { [long]$r.ReadErrorsTotal } else { $null })
    write_errors = $(if ($r -and $r.WriteErrorsTotal -ne $null) { [long]$r.WriteErrorsTotal } else { $null })
    hours = $(if ($r -and $r.PowerOnHours -ne $null) { [long]$r.PowerOnHours } else { $null })
  }
})
[pscustomobject]@{ drives = $drives; c_free_gb = $(if ($c) { [math]::Round($c.SizeRemaining / 1GB, 1) } else { $null }); c_size_gb = $(if ($c) { [math]::Round($c.Size / 1GB, 1) } else { $null }) }
"#;

pub fn read_disks() -> DiskReport {
    match common::ps_json::<DiskReport>(DISK_SCRIPT) {
        Ok(mut v) if !v.is_empty() => {
            let mut r = v.remove(0);
            r.time = common::now();
            r
        }
        Ok(_) => DiskReport { time: common::now(), error: Some("Windows returned no drive information.".into()), ..Default::default() },
        Err(e) => DiskReport { time: common::now(), error: Some(e), ..Default::default() },
    }
}

/// Warnings for one drive (empty = healthy).
pub fn drive_warnings(d: &Drive) -> Vec<String> {
    let mut w = vec![];
    if !d.health.is_empty() && !d.health.eq_ignore_ascii_case("Healthy") {
        w.push(format!("Windows reports this drive as {}", d.health));
    }
    if !d.status.is_empty() && !d.status.eq_ignore_ascii_case("OK") {
        w.push(format!("Status: {}", d.status));
    }
    if d.wear.is_some_and(|x| x >= 80) {
        w.push(format!("{} % of its rated life is used - plan to replace it", d.wear.unwrap_or(0)));
    }
    if d.read_errors.is_some_and(|x| x > 0) || d.write_errors.is_some_and(|x| x > 0) {
        w.push(format!("{} read / {} write errors recorded", d.read_errors.unwrap_or(0), d.write_errors.unwrap_or(0)));
    }
    if let (Some(t), Some(m)) = (d.temp, d.temp_max) {
        if m > 0 && t + 5 >= m {
            w.push(format!("Running hot: {t} °C (limit {m} °C)"));
        }
    }
    w
}

/// C: below 10 % free.
pub fn low_space(r: &DiskReport) -> Option<String> {
    let (f, s) = (r.c_free_gb?, r.c_size_gb?);
    (s > 0.0 && f / s < 0.10).then(|| format!("Only {f:.0} GB free on C: ({:.0} %) - Windows updates and repairs need room", f / s * 100.0))
}

// ------------------------------------------------------------------ live activity of the workers

#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub name: String,
    /// % of the whole CPU
    pub cpu: f32,
    pub mb_s: f32,
}

/// "TICK|DismHost,12.5,3145728;TiWorker,3,0" -> activity per process.
pub fn parse_tick(line: &str) -> Option<Vec<Act>> {
    let rest = line.trim().strip_prefix("TICK|")?;
    Some(
        rest.split(';')
            .filter_map(|p| {
                let mut f = p.split(',');
                let name = f.next()?.trim().to_string();
                let cpu = f.next()?.trim().parse::<f32>().ok()?;
                let bps = f.next()?.trim().parse::<f64>().ok()?;
                (!name.is_empty()).then(|| Act { name: name.split('#').next().unwrap_or(&name).to_string(), cpu, mb_s: (bps / 1_048_576.0) as f32 })
            })
            .collect(),
    )
}

/// Doing real work (not the idle trickle of a waiting process).
pub fn is_busy(acts: &[Act]) -> bool {
    acts.iter().map(|a| a.cpu).sum::<f32>() >= 1.0 || acts.iter().map(|a| a.mb_s).sum::<f32>() >= 0.05
}

/// "Looks stuck": the % has not moved AND the workers have been idle, both for STALL_SECS.
pub fn stalled(since_move: u64, since_busy: u64) -> bool {
    since_move >= STALL_SECS && since_busy >= STALL_SECS
}

fn sampler_script() -> String {
    format!(
        r#"$parent = {pid}
$n = [Environment]::ProcessorCount
while ($true) {{
  if (-not (Get-Process -Id $parent -ErrorAction SilentlyContinue)) {{ break }}
  $p = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "Name like 'Dism%' or Name like 'TiWorker%' or Name like 'TrustedInstaller%' or Name like 'sfc%' or Name like 'chkdsk%'" -ErrorAction SilentlyContinue
  $out = foreach ($x in $p) {{ "$($x.Name),$([math]::Round($x.PercentProcessorTime / $n, 1)),$($x.IODataBytesPersec)" }}
  "TICK|" + ($out -join ';')
  Start-Sleep -Seconds 2
}}
"#,
        pid = std::process::id()
    )
}

// ------------------------------------------------------------------ the tab's state

pub struct Running {
    pub step: StepId,
    pub phase: Phase,
    pub stream: Stream,
    pub seen: usize,
    pub started: Instant,
    pub pct: Option<f32>,
    pub last_move: Instant,
    pub stage: String,
    /// every line of every pass, for the log
    pub log: Vec<String>,
    pub ran_restore: bool,
    pub stopping: bool,
    pub prep_errors: Vec<String>,
}

pub struct Health {
    pub history: BTreeMap<String, StepResult>,
    pub disk: Option<DiskReport>,
    pub disk_job: Option<Job<DiskReport>>,
    pub run: Option<Running>,
    pub queue: Vec<StepId>,
    pub sampler: Option<Stream>,
    pub sampler_seen: usize,
    pub acts: Vec<Act>,
    pub last_busy: Instant,
    pub fix_job: Option<Job<Result<String, String>>>,
    pub note: Option<(bool, String)>,
    /// Time of the last "Run all" that finished.
    pub last_all: Option<String>,
    all_running: bool,
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default)]
    steps: BTreeMap<String, StepResult>,
    #[serde(default)]
    last_all: Option<String>,
}

impl Health {
    pub fn new() -> Self {
        let saved: Saved = common::load_json(HISTORY);
        let mut h = Health {
            history: saved.steps,
            disk: None,
            disk_job: None,
            run: None,
            queue: vec![],
            sampler: None,
            sampler_seen: 0,
            acts: vec![],
            last_busy: Instant::now(),
            fix_job: None,
            note: None,
            last_all: saved.last_all,
            all_running: false,
        };
        // a run that never finished (PC Tuner was closed or crashed) left services at Manual
        if common::is_admin() && !common::load_json::<Vec<Prepped>>(PREP_FILE).is_empty() {
            let errs = unprep();
            if !errs.is_empty() {
                h.note = Some((false, format!("Could not put back the services set to Manual for the last check: {}", errs.join("; "))));
            }
        }
        h
    }

    fn save(&self) {
        common::save_json(HISTORY, &Saved { steps: self.history.clone(), last_all: self.last_all.clone() });
    }

    pub fn busy(&self) -> bool {
        self.run.is_some() || self.fix_job.is_some()
    }

    pub fn refresh_disk(&mut self) {
        if self.disk_job.is_none() {
            self.disk_job = Some(Job::spawn(read_disks));
        }
    }

    pub fn run_all(&mut self) {
        if self.busy() {
            return;
        }
        self.refresh_disk();
        self.all_running = true;
        self.queue = STEPS.to_vec();
        self.next();
    }

    pub fn run_one(&mut self, step: StepId) {
        if self.busy() {
            return;
        }
        self.all_running = false;
        self.queue = vec![step];
        self.next();
    }

    fn next(&mut self) {
        if self.queue.is_empty() {
            if self.all_running {
                self.all_running = false;
                self.last_all = Some(common::now());
                self.save();
            }
            self.stop_sampler();
            return;
        }
        let step = self.queue.remove(0);
        let mut prep_errors = vec![];
        if step.needs_services() {
            prep_errors = prep().1;
        }
        let phase = if step == StepId::Dism { Phase::Check } else { Phase::Single };
        let (exe, args) = command(step, phase);
        self.note = None;
        self.run = Some(Running {
            step,
            phase,
            stream: Stream::run_progress(&exe, args),
            seen: 0,
            started: Instant::now(),
            pct: None,
            last_move: Instant::now(),
            stage: String::new(),
            log: vec![format!("> {exe}")],
            ran_restore: false,
            stopping: false,
            prep_errors,
        });
        self.start_sampler();
    }

    fn start_sampler(&mut self) {
        if self.sampler.is_none() {
            self.sampler = Some(common::ps_stream_readonly(&sampler_script()));
            self.sampler_seen = 0;
            self.last_busy = Instant::now();
        }
    }

    fn stop_sampler(&mut self) {
        if let Some(s) = self.sampler.take() {
            let _ = s.kill();
        }
        self.acts.clear();
    }

    /// Stop the running tool (and the rest of "Run all").
    pub fn stop(&mut self) {
        self.queue.clear();
        self.all_running = false;
        if let Some(r) = &mut self.run {
            r.stopping = true;
            let _ = r.stream.kill();
        }
    }

    /// Fix CHKDSK's problems at the next restart (asks Windows to run /spotfix when C: is free).
    pub fn fix_chkdsk(&mut self) {
        if self.busy() {
            return;
        }
        self.fix_job = Some(Job::spawn(|| {
            let (code, out) = common::run("cmd.exe", &["/d", "/c", "echo Y| chkdsk C: /spotfix"])?;
            let text = out.to_lowercase();
            if text.contains("next time the system restarts") || text.contains("will be checked") || code == 0 {
                Ok("CHKDSK will fix C: the next time the PC restarts (a few seconds at boot).".to_string())
            } else {
                Err(common::cap_text(out.trim(), 300))
            }
        }));
    }

    pub fn pump(&mut self) {
        if let Some(j) = &self.disk_job {
            if let Some(r) = j.poll() {
                self.disk = Some(r);
                self.disk_job = None;
            }
        }
        if let Some(j) = &self.fix_job {
            if let Some(r) = j.poll() {
                self.fix_job = None;
                match r {
                    Ok(t) => {
                        if let Some(c) = self.history.get_mut(StepId::Chkdsk.key()) {
                            c.can_fix = false;
                            c.detail.push(t.clone());
                        }
                        self.save();
                        self.note = Some((true, t));
                    }
                    Err(e) => self.note = Some((false, format!("The fix could not be scheduled: {e}"))),
                }
            }
        }
        if let Some(s) = &mut self.sampler {
            s.pump();
            while self.sampler_seen < s.lines.len() {
                let l = s.lines[self.sampler_seen].clone();
                self.sampler_seen += 1;
                if let Some(a) = parse_tick(&l) {
                    if is_busy(&a) {
                        self.last_busy = Instant::now();
                    }
                    self.acts = a;
                }
            }
        }
        let Some(r) = &mut self.run else { return };
        let running = r.stream.pump();
        while r.seen < r.stream.lines.len() {
            let l = r.stream.lines[r.seen].clone();
            r.seen += 1;
            if let Some(p) = parse_pct(&l) {
                if r.pct.map_or(true, |o| (o - p).abs() >= 0.05) {
                    r.last_move = Instant::now();
                    // the log keeps whole-number steps only
                    if r.pct.map_or(true, |o| o.floor() != p.floor()) {
                        r.log.push(l.trim().to_string());
                    }
                }
                r.pct = Some(p);
            } else {
                if let Some(s) = stage_text(&l) {
                    r.stage = s;
                    r.last_move = Instant::now();
                }
                r.log.push(l);
            }
        }
        if running {
            return;
        }
        let code = r.stream.exit.unwrap_or(-1);
        let pass: Vec<String> = r.stream.lines.clone();
        if r.stopping {
            let mut out = res(Level::Warn, "Stopped by you - nothing was finished");
            out.secs = r.started.elapsed().as_secs();
            return self.finish(out);
        }
        match r.step {
            StepId::Dism => {
                let out = dism_outcome(&pass, code);
                if let Some(next) = dism_next(r.phase, &out) {
                    let (exe, args) = command(StepId::Dism, next);
                    r.log.push(format!("> {exe} {}", args.join(" ")));
                    r.phase = next;
                    r.ran_restore |= next == Phase::Restore;
                    r.stream = Stream::run_progress(&exe, args);
                    r.seen = 0;
                    r.pct = None;
                    r.stage = String::new();
                    r.last_move = Instant::now();
                    return;
                }
                let ran = r.ran_restore;
                self.finish(dism_result(&out, ran));
            }
            StepId::Sfc => {
                let o = sfc_outcome(&pass, code);
                let mut out = sfc_result(&o);
                if matches!(o, SfcOut::Repaired | SfcOut::Unfixed) {
                    out.detail.extend(read_cbs());
                }
                if o == SfcOut::CouldNotStart && !r.prep_errors.is_empty() {
                    out.detail.push(format!("Setting the services it needs to Manual failed: {}", r.prep_errors.join("; ")));
                }
                self.finish(out);
            }
            StepId::Chkdsk => {
                let out = chkdsk_result(&chkdsk_outcome(&pass, code));
                self.finish(out);
            }
        }
    }

    fn finish(&mut self, mut out: StepResult) {
        let Some(r) = self.run.take() else { return };
        if out.secs == 0 {
            out.secs = r.started.elapsed().as_secs();
        }
        if r.step.needs_services() {
            let errs = unprep();
            if !errs.is_empty() {
                out.detail.push(format!("Could not put these services back to Disabled: {}", errs.join("; ")));
            }
        }
        // the full output, for "Open log"
        let dir = common::data_dir().join("health");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}-{}.log", chrono::Local::now().format("%Y%m%d-%H%M%S"), r.step.key()));
        let mut text = r.log.join("\r\n");
        text.push_str(&format!("\r\n\r\nResult: {}\r\n", out.summary));
        if std::fs::write(&path, text).is_ok() {
            out.log = path.display().to_string();
        }
        self.history.insert(r.step.key().into(), out);
        self.save();
        self.next();
    }

    /// The step running now, its pass and how long since the % moved.
    pub fn since_move(&self) -> u64 {
        self.run.as_ref().map(|r| r.last_move.elapsed().as_secs()).unwrap_or(0)
    }

    pub fn since_busy(&self) -> u64 {
        self.last_busy.elapsed().as_secs()
    }

    /// PC Tuner is closing: stop the sampler; services set to Manual are put back.
    pub fn on_exit(&mut self) {
        self.stop_sampler();
        if self.run.is_none() && !common::load_json::<Vec<Prepped>>(PREP_FILE).is_empty() {
            let _ = unprep();
        }
    }
}

/// The "it often waits here" note for a pause (None while it moves).
pub fn pause_note(step: StepId, phase: Phase, pct: Option<f32>, since_move: u64) -> Option<String> {
    if since_move < PAUSE_NOTE_SECS {
        return None;
    }
    let p = pct.unwrap_or(0.0);
    Some(match (step, phase) {
        (StepId::Dism, Phase::Restore) if (60.0..=63.0).contains(&p) || (84.0..=86.0).contains(&p) => {
            "DISM often waits here for many minutes while it downloads repair files from Windows Update.".into()
        }
        (StepId::Dism, _) => "DISM can stay on one % for several minutes while it checks the component store.".into(),
        (StepId::Sfc, _) => "SFC often pauses for several minutes (commonly around 60-70 %) while it checks large files.".into(),
        (StepId::Chkdsk, _) => "CHKDSK can pause on large folders (for example the Steam or WinSxS folders).".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn health_progress_parsing() {
        assert_eq!(parse_pct("[=====                      62.3%                          ]"), Some(62.3));
        assert_eq!(parse_pct("Verification 45% complete."), Some(45.0));
        assert_eq!(parse_pct("Progress: 123456 of 789012 done; Stage:  45%; Total:  30%; ETA:   0:00:10 .."), Some(30.0));
        assert_eq!(parse_pct("Stage 1: Examining basic file system structure ..."), None);
        assert!(is_progress("[==  10.0%  ]") && is_progress("Verification 45% complete.") && !is_progress("Beginning system scan."));
        assert_eq!(stage_text("Stage 2: Examining file name linkage ...").as_deref(), Some("Stage 2: Examining file name linkage"));
        assert_eq!(stage_text("[==  10.0%  ]"), None);
    }

    #[test]
    fn health_dism_runs_restore_only_when_repairable() {
        let healthy = l(&["Image Version: 10.0.26100.1", "No component store corruption detected.", "The operation completed successfully."]);
        let repairable = l(&["[==========================100.0%==========================] The component store is repairable.", "The operation completed successfully."]);
        assert_eq!(dism_outcome(&healthy, 0), DismOut::Healthy);
        assert_eq!(dism_outcome(&repairable, 0), DismOut::Repairable);
        assert_eq!(dism_next(Phase::Check, &DismOut::Healthy), Some(Phase::Scan));
        assert_eq!(dism_next(Phase::Scan, &DismOut::Healthy), None, "healthy after the scan: the slow repair never runs");
        assert_eq!(dism_next(Phase::Scan, &DismOut::Repairable), Some(Phase::Restore));
        assert_eq!(dism_next(Phase::Check, &DismOut::Repairable), Some(Phase::Restore), "known damage skips the scan");
        assert_eq!(dism_next(Phase::Restore, &DismOut::Repaired), None);
        let err = l(&["[===  62.3%  ]", "Error: 0x800f081f", "", "The source files could not be found."]);
        let o = dism_outcome(&err, 0x800f081fu32 as i32);
        assert_eq!(o, DismOut::Failed("Error: 0x800f081f The source files could not be found.".into()));
        assert_eq!(dism_result(&o, true).detail.len(), 2, "explains the missing repair files");
        assert_eq!(dism_outcome(&l(&["The component store cannot be repaired."]), 0), DismOut::NotRepairable);
        assert_eq!(dism_outcome(&l(&["The restore operation completed successfully."]), 0), DismOut::Repaired);
    }

    #[test]
    fn health_sfc_and_chkdsk_results() {
        let s = |t: &str| sfc_outcome(&l(&["Beginning system scan.", "Verification 100% complete.", t]), 0);
        assert_eq!(s("Windows Resource Protection did not find any integrity violations."), SfcOut::Clean);
        assert_eq!(s("Windows Resource Protection found corrupt files and successfully repaired them."), SfcOut::Repaired);
        assert_eq!(s("Windows Resource Protection found corrupt files but was unable to fix some of them."), SfcOut::Unfixed);
        assert_eq!(s("Windows Resource Protection could not start the repair service."), SfcOut::CouldNotStart);
        assert_eq!(s("There is a system repair pending which requires reboot to complete."), SfcOut::RebootPending);
        assert_eq!(sfc_result(&SfcOut::Clean).level, Level::Ok);

        let clean = l(&["Stage 1: Examining basic file system structure ...", "Windows has scanned the file system and found no problems.", "No further action is required."]);
        assert_eq!(chkdsk_outcome(&clean, 0), ChkOut::Clean);
        let bad = l(&["Windows has scanned the file system and found problems.", "Run CHKDSK with the /spotfix option to fix the problems."]);
        assert_eq!(chkdsk_outcome(&bad, 3), ChkOut::Problems);
        assert!(chkdsk_result(&ChkOut::Problems).can_fix, "problems offer the fix at restart");
        assert!(matches!(chkdsk_outcome(&l(&["Access Denied as you do not have sufficient privileges."]), 3), ChkOut::Problems | ChkOut::Failed(_)));

        let cbs = "2026-11-16 10:00:00, Info CSI 00000001 [SR] Repairing corrupted file \\??\\C:\\Windows\\a.dll\n2026-11-16, Info CSI [SR] Cannot repair member file [l:10]b.dll\nother line";
        assert_eq!(cbs_lines(cbs).len(), 2);
    }

    #[test]
    fn health_services_prep_and_stall_rule() {
        let starts = |n: &str| match n {
            "TrustedInstaller" => Some(4),
            "BITS" => Some(4),
            "wuauserv" => Some(3),
            _ => None,
        };
        let d = disabled_needed(starts);
        assert_eq!(d.iter().map(|x| x.0).collect::<Vec<_>>(), vec!["TrustedInstaller", "BITS"], "only Disabled ones are touched");
        // the rule needs both: no % change AND idle workers
        assert!(!stalled(STALL_SECS + 1, 5), "working but not moving the % is not stuck");
        assert!(!stalled(5, STALL_SECS + 1));
        assert!(stalled(STALL_SECS, STALL_SECS));
        let a = parse_tick("TICK|DismHost,12.5,3145728;TiWorker#1,0,0").unwrap();
        assert_eq!(a[0], Act { name: "DismHost".into(), cpu: 12.5, mb_s: 3.0 });
        assert_eq!(a[1].name, "TiWorker");
        assert!(is_busy(&a) && !is_busy(&parse_tick("TICK|TiWorker,0.2,1000").unwrap()) && parse_tick("TICK|").unwrap().is_empty());
        assert!(pause_note(StepId::Dism, Phase::Restore, Some(62.3), 120).unwrap().contains("Windows Update"));
        assert!(pause_note(StepId::Sfc, Phase::Single, Some(65.0), 5).is_none());
    }

    #[test]
    fn health_disk_warnings() {
        let ok = Drive { name: "Samsung 990 PRO".into(), health: "Healthy".into(), status: "OK".into(), wear: Some(2), temp: Some(40), temp_max: Some(82), read_errors: Some(0), write_errors: Some(0), ..Default::default() };
        assert!(drive_warnings(&ok).is_empty());
        let bad = Drive { health: "Warning".into(), wear: Some(91), read_errors: Some(3), ..ok.clone() };
        assert_eq!(drive_warnings(&bad).len(), 3);
        let r = DiskReport { c_free_gb: Some(40.0), c_size_gb: Some(1000.0), ..Default::default() };
        assert!(low_space(&r).is_some());
        assert!(low_space(&DiskReport { c_free_gb: Some(400.0), c_size_gb: Some(1000.0), ..Default::default() }).is_none());
    }
}
