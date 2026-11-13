//! Services tab - port of ServiceManager.ps1.
//! Change service startup types with full undo, gaming-dependency warnings, locked core services,
//! Safe / Safe + Optional presets. History lives in %LOCALAPPDATA%\PCTuner\services-history.json
//! (imported once from the PowerShell tool's %LOCALAPPDATA%\ServiceManager\history.json).

use crate::common::{self, Job, Tab};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use winreg::enums::*;
use winreg::RegKey;

const SVC_ROOT: &str = r"SYSTEM\CurrentControlSet\Services";
const HISTORY: &str = "services-history.json";

const NOTE_LOCKED: &str = "Core Windows (locked)";
const NOTE_GAMING: &str = "Gaming dependency";
const NOTE_PER_USER: &str = "Per-user service";

// ---------------------------------------------------------------- lists (copied from ServiceManager.ps1)

/// Xbox / Game Pass / Steam / anti-cheat and everything they depend on
const GAMING: &[&str] = &[
    "GamingServices", "GamingServicesNet", "XblAuthManager", "XblGameSave", "XboxNetApiSvc", "XboxGipSvc",
    "XboxService", "GameInputSvc", "GameInputRedistService", "BcastDVRUserService", "Steam Client Service",
    "EasyAntiCheat_EOS", "EasyAntiCheat", "BEService", "InstallService", "ClipSVC", "LicenseManager", "wlidsvc",
    "TokenBroker", "AppXSvc", "iphlpsvc", "IKEEXT", "KeyIso", "WinHttpAutoProxySvc", "DoSvc", "BITS", "UsoSvc",
    "wuauserv", "hidserv", "bthserv", "Audiosrv", "AudioEndpointBuilder", "SSDPSRV", "upnphost", "QWAVE",
    "WpnService", "NVDisplay.ContainerLocalSystem", "amd3dvcacheSvc", "CoworkVMServiceStore", "HvHost", "SharedAccess",
    "DusmSvc",       // Store / Xbox app downloads can hang at "Pending" without it
    "NcbService",    // Network Connection Broker - Xbox app notifications / invites
    "TimeBrokerSvc", // brokers background tasks for Store / Xbox apps
    "RmSvc",         // Radio Management - Bluetooth on/off and airplane mode (Bluetooth controllers / headsets)
];

/// Changing these can stop Windows from booting or logging in - locked.
const CRITICAL: &[&str] = &[
    "RpcSs", "RpcEptMapper", "DcomLaunch", "LSM", "PlugPlay", "Power", "ProfSvc", "EventLog", "BrokerInfrastructure",
    "SamSs", "CryptSvc", "Winmgmt", "CoreMessagingRegistrar", "StateRepository", "UserManager", "SystemEventsBroker",
    "gpsvc", "Schedule", "BFE", "mpssvc", "Dhcp", "Dnscache", "nsi", "WinDefend", "SecurityHealthService", "wscsvc",
    "MDCoreSvc", "WdNisSvc", "EventSystem", "TrustedInstaller", "msiserver", "AppIDSvc", "Appinfo", "camsvc",
    "TextInputManagementService", "DispBrokerDesktopSvc", "Themes", "FontCache", "NgcSvc", "NgcCtnrSvc", "VaultSvc",
    "KeyIso", "LanmanWorkstation", "Netman", "netprofm", "NlaSvc", "Wcmsvc", "sppsvc", "WaaSMedicSvc", "DeviceInstall",
    "DsmSvc", "Audiosrv", "AudioEndpointBuilder", "SENS", "ShellHWDetection",
];

// ---- Presets (same lists as Optimize-Services.ps1). None of these affect Xbox / Steam.
const PRESET_SAFE: &[(&str, &str)] = &[
    // telemetry / ads / demo
    ("DiagTrack", "Disabled"), ("dmwappushservice", "Disabled"), ("wuqisvc", "Disabled"), ("RetailDemo", "Disabled"),
    ("MapsBroker", "Disabled"), ("lfsvc", "Disabled"), ("wisvc", "Disabled"), ("InventorySvc", "Manual"),
    ("PcaSvc", "Manual"), ("TrkWks", "Disabled"), ("WerSvc", "Manual"),
    // printing / scanning / phone (no printer or camera on this PC)
    ("Spooler", "Disabled"), ("PrintNotify", "Disabled"), ("PrintDeviceConfigurationService", "Disabled"),
    ("PrintScanBrokerService", "Disabled"), ("StiSvc", "Disabled"), ("WiaRpc", "Disabled"), ("TapiSrv", "Disabled"),
    ("PhoneSvc", "Disabled"), ("SmsRouter", "Disabled"),
    // remote access you don't host
    ("TermService", "Disabled"), ("SessionEnv", "Disabled"), ("UmRdpService", "Disabled"), ("WinRM", "Disabled"),
    ("RemoteRegistry", "Disabled"), ("WebClient", "Disabled"), ("RasAuto", "Disabled"),
    // networking roles not used (wired Ethernet, not domain-joined)
    ("icssvc", "Disabled"), ("WwanSvc", "Disabled"), ("autotimesvc", "Disabled"), ("WlanSvc", "Manual"),
    ("PeerDistSvc", "Disabled"), ("MSiSCSI", "Disabled"), ("lltdsvc", "Disabled"), ("NcdAutoSetup", "Disabled"),
    ("wcncsvc", "Disabled"), ("Netlogon", "Disabled"), ("CscService", "Disabled"),
    // misc consumer features
    ("SEMgrSvc", "Disabled"), ("WalletService", "Disabled"), ("WpcMonSvc", "Disabled"), ("WMPNetworkSvc", "Disabled"),
    ("fhsvc", "Disabled"), ("SensorService", "Disabled"), ("SensorDataService", "Disabled"), ("SensrSvc", "Disabled"),
    ("edgeupdate", "Manual"), ("MicrosoftEdgeElevationService", "Disabled"), ("MicrosoftCopilotElevationService", "Disabled"),
    ("StorSvc", "Manual"), // Storage Service: trigger-starts on demand (Store / Game Pass installs still start it) - never Disabled
    // Hyper-V *guest* integration (this PC is a host)
    ("vmicguestinterface", "Disabled"), ("vmicheartbeat", "Disabled"), ("vmickvpexchange", "Disabled"),
    ("vmicrdv", "Disabled"), ("vmicshutdown", "Disabled"), ("vmictimesync", "Disabled"),
    ("vmicvmsession", "Disabled"), ("vmicvss", "Disabled"),
];

/// Entries here override PRESET_SAFE when both apply.
const PRESET_OPTIONAL: &[(&str, &str)] = &[
    ("WSearch", "Disabled"), ("SysMain", "Manual"), ("LanmanServer", "Manual"),
    // Diagnostics trio: troubleshooters, problem detection and Task Manager "App history" stop working
    ("DPS", "Disabled"), ("WdiServiceHost", "Disabled"), ("WdiSystemHost", "Disabled"),
    ("WSAIFabricSvc", "Manual"), ("CDPSvc", "Manual"),
    ("lmhosts", "Disabled"), // NetBIOS name lookup of old LAN devices (DNS keeps working); trigger restarts it if Manual
    ("PcaSvc", "Disabled"),  // Program Compatibility Assistant prompts (overrides the Safe preset's Manual)
    ("logi_lamparray_service", "Manual"), ("WbioSrvc", "Disabled"), ("SCardSvr", "Disabled"),
    ("ScDeviceEnum", "Disabled"), ("SCPolicySvc", "Disabled"), ("CertPropSvc", "Disabled"),
    ("whesvc", "Manual"), // Windows Health and Optimized Experiences: slowness traces + feedback toasts
];

// ---------------------------------------------------------------- helpers (Get-BaseName, Get-Template, Get-Note, Get-StartType ...)

fn contains_ci(list: &[&str], s: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(s))
}

/// Per-user services look like CDPUserSvc_70560 -> CDPUserSvc
fn base_name(name: &str) -> &str {
    if let Some(i) = name.rfind('_') {
        let (prefix, suffix) = (&name[..i], &name[i + 1..]);
        if !prefix.is_empty() && (4..=8).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
            return prefix;
        }
    }
    name
}

fn svc_key(name: &str) -> Option<RegKey> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!(r"{SVC_ROOT}\{name}")).ok()
}

/// Everything apply / undo read or change on the system. `System` is the real PC; the tests use
/// an in-memory registry so apply -> undo round trips can be checked without touching anything.
trait Backend {
    /// Names of all keys under HKLM\SYSTEM\CurrentControlSet\Services.
    fn keys(&self) -> Vec<String>;
    fn exists(&self, name: &str) -> bool;
    /// (Start, DelayedAutostart - None when the value is absent)
    fn read(&self, name: &str) -> Option<(u32, Option<u32>)>;
    fn set_start(&self, name: &str, start: u32) -> Result<(), String>;
    /// Some(v) writes DelayedAutostart, None deletes it (absent already = Ok).
    fn set_delayed(&self, name: &str, v: Option<u32>) -> Result<(), String>;
    /// Start type through the Service Control Manager (`sc_arg` as sc.exe names it: disabled / demand / auto /
    /// delayed-auto); Err(reason) on failure.
    fn sc_config(&self, name: &str, sc_arg: &str) -> Result<(), String>;
    fn is_running(&self, name: &str) -> Option<bool>;
    fn stop(&self, name: &str) -> Result<(), String>;
    fn start(&self, name: &str);
}

struct System;

/// sc.exe's start names -> (Start value, Delayed start to set). Like sc.exe, only "auto" and "delayed-auto"
/// touch Delayed start.
fn scm_start(sc_arg: &str) -> Option<(u32, Option<bool>)> {
    match sc_arg {
        "disabled" => Some((4, None)),
        "demand" => Some((3, None)),
        "auto" => Some((2, Some(false))),
        "delayed-auto" => Some((2, Some(true))),
        _ => None,
    }
}

impl System {
    fn writable(name: &str) -> Result<RegKey, String> {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(format!(r"{SVC_ROOT}\{name}"), KEY_SET_VALUE)
            .map_err(|e| format!("cannot open registry key {name}: {e}"))
    }
}

impl Backend for System {
    fn keys(&self) -> Vec<String> {
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(SVC_ROOT).map(|k| k.enum_keys().filter_map(Result::ok).collect()).unwrap_or_default()
    }
    fn exists(&self, name: &str) -> bool {
        svc_key(name).is_some()
    }
    fn read(&self, name: &str) -> Option<(u32, Option<u32>)> {
        let k = svc_key(name)?;
        let start: u32 = k.get_value("Start").ok()?;
        Some((start, k.get_value::<u32, _>("DelayedAutostart").ok()))
    }
    fn set_start(&self, name: &str, start: u32) -> Result<(), String> {
        System::writable(name)?.set_value("Start", &start).map_err(|e| e.to_string())
    }
    fn set_delayed(&self, name: &str, v: Option<u32>) -> Result<(), String> {
        let key = System::writable(name)?;
        match v {
            Some(v) => key.set_value("DelayedAutostart", &v).map_err(|e| e.to_string()),
            None => match key.delete_value("DelayedAutostart") {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            },
        }
    }
    fn sc_config(&self, name: &str, sc_arg: &str) -> Result<(), String> {
        let (start, delayed) = scm_start(sc_arg).ok_or_else(|| format!("unknown start type {sc_arg}"))?;
        crate::winsvc::set_start(name, start, delayed)
    }
    fn is_running(&self, name: &str) -> Option<bool> {
        crate::winsvc::is_running(name)
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        crate::winsvc::stop(name)
    }
    fn start(&self, name: &str) {
        let _ = crate::winsvc::start(name);
    }
}

/// Template of a per-user service (CDPUserSvc_70560 -> CDPUserSvc), when that template exists.
/// Works even after a reboot has replaced the instance with one that has a new suffix.
fn template_in(b: &dyn Backend, name: &str) -> Option<String> {
    let base = base_name(name);
    if base != name && b.exists(base) {
        Some(base.to_string())
    } else {
        None
    }
}

fn template(name: &str) -> Option<String> {
    template_in(&System, name)
}

/// Current per-user instances of a template (their suffix changes after every reboot / sign-in).
fn instances(b: &dyn Backend, base: &str) -> Vec<String> {
    b.keys().into_iter().filter(|k| !k.eq_ignore_ascii_case(base) && base_name(k).eq_ignore_ascii_case(base)).collect()
}

fn note_for(name: &str) -> &'static str {
    let base = base_name(name);
    if contains_ci(CRITICAL, name) || contains_ci(CRITICAL, base) {
        return NOTE_LOCKED;
    }
    if contains_ci(GAMING, name) || contains_ci(GAMING, base) {
        return NOTE_GAMING;
    }
    if template(name).is_some() {
        return NOTE_PER_USER;
    }
    ""
}

/// For a per-user service this is the template's startup type: that is what PC Tuner changes and
/// what Windows copies into every new instance (the instance can differ, e.g. lose Delayed start).
fn start_type(b: &dyn Backend, name: &str) -> String {
    let p = match template_in(b, name) {
        Some(t) => b.read(&t).or_else(|| b.read(name)),
        None => b.read(name),
    };
    let Some((start, delayed)) = p else { return "Unknown".into() };
    let t = match start {
        0 => "Boot",
        1 => "System",
        2 => "Automatic",
        3 => "Manual",
        4 => "Disabled",
        _ => return "Unknown".into(),
    };
    if t == "Automatic" && delayed == Some(1) {
        return "Automatic (Delayed)".into();
    }
    t.into()
}

fn set_registry_start(b: &dyn Backend, name: &str, ty: &str) -> Result<(), String> {
    let num: u32 = match ty {
        "Disabled" => 4,
        "Manual" => 3,
        "Automatic" | "Automatic (Delayed)" => 2,
        _ => return Err(format!("Cannot set start type '{ty}'.")),
    };
    b.set_start(name, num)?;
    match ty {
        "Automatic (Delayed)" => b.set_delayed(name, Some(1))?,
        // plain Automatic = no delay; remove the value rather than adding DelayedAutostart=0
        "Automatic" => b.set_delayed(name, None)?,
        _ => {}
    }
    Ok(())
}

/// Returns a short note on how it was applied ("ok" when the service manager did it).
fn set_start_type(b: &dyn Backend, name: &str, ty: &str) -> Result<&'static str, String> {
    let sc_arg = match ty {
        "Disabled" => "disabled",
        "Manual" => "demand",
        "Automatic" => "auto",
        "Automatic (Delayed)" => "delayed-auto",
        _ => return Err(format!("Cannot set start type '{ty}'.")),
    };

    // Per-user services: Windows recreates the instance from its template at each logon,
    // so change the template (registry only - the service manager refuses templates). The instance itself is
    // best effort: after a reboot the recorded instance name is gone, which is fine.
    if let Some(t) = template_in(b, name) {
        set_registry_start(b, &t, ty)?;
        if b.exists(name) {
            let _ = set_registry_start(b, name, ty);
        }
        return Ok("per-user, applies after sign-out/reboot");
    }

    let sc_text = match b.sc_config(name, sc_arg) {
        Ok(()) => return Ok("ok"),
        Err(t) => t,
    };

    // Some services are ACL-protected from the service manager; the registry usually still works.
    match set_registry_start(b, name, ty) {
        Ok(()) => Ok("via registry, applies after reboot"),
        Err(e) => Err(format!("service manager: {sc_text} / registry: {e}")),
    }
}

// ---------------------------------------------------------------- data

#[derive(Clone)]
struct Svc {
    name: String,
    display: String,
    start: String,
    state: String,
    note: &'static str,
    name_lc: String,
    display_lc: String,
}

/// Every service from the Service Control Manager (the same set Win32_Service lists).
fn load_services() -> Result<Vec<Svc>, String> {
    Ok(crate::winsvc::list()?
        .into_iter()
        .map(|(name, display, state)| Svc {
            start: start_type(&System, &name),
            note: note_for(&name),
            state,
            name_lc: name.to_lowercase(),
            display_lc: display.to_lowercase(),
            name,
            display,
        })
        .collect())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct HistEntry {
    #[serde(default)]
    batch: u64,
    #[serde(default)]
    time: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    was_running: bool,
    #[serde(default)]
    undone: bool,
}

/// The PowerShell tool's history.json: PascalCase fields, a JSON array or a single object.
fn parse_ps_history(text: &str) -> Vec<HistEntry> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}').trim()) else { return vec![] };
    let items = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(_) => vec![v],
        _ => vec![],
    };
    let s = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    items
        .iter()
        .filter_map(|o| {
            let name = s(o, "Name");
            if name.is_empty() {
                return None;
            }
            let batch = o.get("Batch").and_then(|b| b.as_u64().or_else(|| b.as_f64().map(|f| f as u64)).or_else(|| b.as_str().and_then(|x| x.parse().ok()))).unwrap_or(0);
            let display = s(o, "DisplayName");
            Some(HistEntry {
                batch,
                time: s(o, "Time"),
                display_name: if display.is_empty() { name.clone() } else { display },
                name,
                from: s(o, "From"),
                to: s(o, "To"),
                was_running: o.get("WasRunning").and_then(|x| x.as_bool()).unwrap_or(false),
                undone: o.get("Undone").and_then(|x| x.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}

/// True only on a real first run: no history of ours, and no damaged one moved aside earlier
/// (`services-history.json.corrupt-*`). After a damage the PS history's Undone flags are stale,
/// so importing it would offer changes that were already undone for undo again.
fn first_run(data: &std::path::Path) -> bool {
    if data.join(HISTORY).exists() {
        return false;
    }
    let prefix = format!("{HISTORY}.corrupt-");
    !std::fs::read_dir(data).map(|rd| rd.filter_map(Result::ok).any(|f| f.file_name().to_string_lossy().starts_with(&prefix))).unwrap_or(false)
}

fn load_history() -> Vec<HistEntry> {
    if !first_run(&common::data_dir()) {
        return common::load_json(HISTORY);
    }
    // First run: import the PowerShell Service Manager's history.
    let old = std::env::var_os("LOCALAPPDATA").map(|b| std::path::PathBuf::from(b).join("ServiceManager").join("history.json"));
    let imported = old.and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_ps_history(&t)).unwrap_or_default();
    if !imported.is_empty() {
        common::save_json(HISTORY, &imported);
    }
    imported
}

/// Safe (+ Optional) preset as an ordered (name, target) list; Optional overrides Safe.
/// Belt and braces: never let a preset touch a gaming or core service.
fn preset_plan(include_optional: bool) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = Vec::new();
    let mut add = |n: &str, t: &str| {
        if let Some(e) = all.iter_mut().find(|(x, _)| x.eq_ignore_ascii_case(n)) {
            e.1 = t.to_string();
        } else {
            all.push((n.to_string(), t.to_string()));
        }
    };
    for (n, t) in PRESET_SAFE {
        add(n, t);
    }
    if include_optional {
        for (n, t) in PRESET_OPTIONAL {
            add(n, t);
        }
    }
    all.into_iter().filter(|(n, _)| !matches!(note_for(n), NOTE_GAMING | NOTE_LOCKED)).collect()
}

/// The preset plan as it is applied and counted on the hero chip. Optional overrides Safe, so the
/// Safe preset leaves a service alone when it is already at its Safe + Optional target (e.g.
/// PcaSvc Disabled) - applying Safe never reverts an Optional choice.
fn live_preset_plan(b: &dyn Backend, include_optional: bool) -> Vec<(String, String)> {
    let plan = preset_plan(include_optional);
    if include_optional {
        return plan;
    }
    plan.into_iter()
        .filter(|(n, _)| {
            let opt = PRESET_OPTIONAL.iter().find(|(o, _)| o.eq_ignore_ascii_case(n)).map(|(_, t)| *t);
            !opt.is_some_and(|t| start_type(b, n) == t)
        })
        .collect()
}

/// The plan without the services unticked in the preview (preview item ids are service names).
fn picked_plan(plan: Vec<(String, String)>, unpicked: &HashSet<String>) -> Vec<(String, String)> {
    plan.into_iter().filter(|(n, _)| !unpicked.contains(n)).collect()
}

struct Work {
    name: String,
    from: String,
    to: String,
}

/// Keeps only entries for installed services whose startup type would actually change.
fn pending_plan(b: &dyn Backend, plan: &[(String, String)]) -> Vec<Work> {
    plan.iter()
        .filter(|(n, _)| b.exists(n))
        .filter_map(|(n, to)| {
            let from = start_type(b, n);
            if from == *to || (to == "Automatic" && from == "Automatic (Delayed)") {
                None
            } else {
                Some(Work { name: n.clone(), from, to: to.clone() })
            }
        })
        .collect()
}

/// What happened to one service in an apply / undo run.
struct ItemResult {
    /// Index into the history (undo runs only).
    index: usize,
    name: String,
    from: String,
    to: String,
    /// Ok(note) - the note is empty when there is nothing to add; Err(reason).
    result: Result<String, String>,
    /// Undo only: the entry was left alone because the service changed since (or is gone);
    /// it is marked undone anyway so it no longer blocks 'Undo last change'.
    retire: bool,
}

struct ApplyOutcome {
    /// Installed services that were already at the target.
    skipped: usize,
    /// Plan entries that are not installed on this PC.
    missing: usize,
    results: Vec<ItemResult>,
    entries: Vec<HistEntry>,
}

/// Applies a list of (name, target) as ONE history batch (runs on a background thread).
fn apply_plan(b: &dyn Backend, plan: Vec<(String, String)>, batch: u64, stop_when_disabling: bool, info: HashMap<String, (String, bool)>) -> ApplyOutcome {
    let installed: Vec<(String, String)> = plan.iter().filter(|(n, _)| b.exists(n)).cloned().collect();
    let work = pending_plan(b, &installed);
    let mut out = ApplyOutcome { skipped: installed.len() - work.len(), missing: plan.len() - installed.len(), results: vec![], entries: vec![] };
    for w in work {
        let n = &w.name;
        let (display, cached_running) = info.get(n).cloned().unwrap_or_else(|| (n.clone(), false));
        let was_running = b.is_running(n).unwrap_or(cached_running);
        let how = match set_start_type(b, n, &w.to) {
            Ok(h) => h,
            Err(e) => {
                out.results.push(ItemResult { index: 0, name: w.name, from: w.from, to: w.to, result: Err(e), retire: false });
                continue;
            }
        };
        let mut notes: Vec<String> = vec![];
        if how != "ok" {
            notes.push(how.to_string());
        }
        out.entries.push(HistEntry {
            batch,
            time: common::now(),
            name: n.clone(),
            display_name: if display.is_empty() { n.clone() } else { display },
            from: w.from.clone(),
            to: w.to.clone(),
            was_running,
            undone: false,
        });
        if w.to == "Disabled" && stop_when_disabling && was_running {
            if let Err(e) = b.stop(n) {
                notes.push(format!("changed, but could not be stopped now ({e})"));
            }
        }
        out.results.push(ItemResult { index: 0, name: w.name, from: w.from, to: w.to, result: Ok(notes.join("; ")), retire: false });
    }
    out
}

/// Puts one history entry back. Per-user services: the recorded instance (NPSMSvc_5fec7) is
/// replaced by a new suffix after a reboot, so the template and the CURRENT instances are restored.
fn undo_one(b: &dyn Backend, h: &HistEntry) -> Result<String, String> {
    let how = set_start_type(b, &h.name, &h.from)?;
    let targets: Vec<String> = match template_in(b, &h.name) {
        Some(base) => {
            let mut t: Vec<String> = vec![];
            for k in instances(b, &base) {
                if !k.eq_ignore_ascii_case(&h.name) {
                    let _ = set_registry_start(b, &k, &h.from); // best effort, like the instance in set_start_type
                }
                t.push(k);
            }
            t
        }
        None => vec![h.name.clone()],
    };
    if h.was_running {
        for k in &targets {
            b.start(k);
        }
    }
    Ok(if how == "ok" { String::new() } else { how.to_string() })
}

/// Reverts entries (already sorted newest first). One result per entry, `index` = history index.
/// An entry is only reverted while the service is still at the type it set (`to`); anything changed
/// since - by a newer entry that could not be undone, or outside PC Tuner - is left alone.
fn undo_entries(b: &dyn Backend, entries: Vec<(usize, HistEntry)>) -> Vec<ItemResult> {
    let mut blocked: HashSet<String> = HashSet::new();
    entries
        .into_iter()
        .map(|(i, h)| {
            let svc = base_name(&h.name).to_lowercase();
            let mut retire = false;
            let result = if blocked.contains(&svc) {
                Err(format!("skipped - a newer change to {} could not be undone", h.name))
            } else {
                let now = start_type(b, &h.name);
                if now == "Unknown" {
                    retire = true;
                    Err("no longer installed - left alone".to_string())
                } else if now != h.to {
                    retire = true;
                    Err(format!("now {now}, not {} - changed since; left alone", h.to))
                } else {
                    undo_one(b, &h)
                }
            };
            if result.is_err() && !retire {
                blocked.insert(svc);
            }
            ItemResult { index: i, name: h.name, from: h.to, to: h.from, result, retire }
        })
        .collect()
}

/// Not-yet-undone entries for these history indices plus every newer active entry for the same
/// service (undoing an older change first would leave the newer one pointing at the wrong type),
/// newest first so stacked changes unwind in order.
fn undo_chain(history: &[HistEntry], idx: &[usize]) -> Vec<(usize, HistEntry)> {
    let key = |i: usize| (history[i].batch, history[i].time.as_str(), i);
    let mut set: Vec<usize> = idx.iter().copied().filter(|&i| history.get(i).is_some_and(|h| !h.undone)).collect();
    for i in set.clone() {
        let svc = base_name(&history[i].name);
        for j in 0..history.len() {
            if !history[j].undone && base_name(&history[j].name).eq_ignore_ascii_case(svc) && key(j) > key(i) {
                set.push(j);
            }
        }
    }
    set.sort_by(|&a, &b| key(b).cmp(&key(a)));
    set.dedup();
    set.into_iter().map(|i| (i, history[i].clone())).collect()
}

// ---------------------------------------------------------------- operation windows

fn arrow(from: &str, to: &str) -> String {
    format!("{from} -> {to}")
}

fn preset_name(include_optional: bool) -> &'static str {
    if include_optional {
        "Safe + Optional"
    } else {
        "Safe"
    }
}

fn base_window(kicker: &str, items: Vec<OpItem>) -> OpWindow {
    let mut w = OpWindow::new(kicker, "change", "changed", items).noun("service", "services");
    w.running_text = "Changing services...".into();
    w.undo_label = "Undo these changes".into();
    w
}

/// Tick boxes need an outline and a square corner to read as check boxes on a card
/// (the theme's 8 px widget rounding turns a 14 px box into a circle).
fn outline_checks(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for s in [&mut w.inactive, &mut w.hovered, &mut w.active] {
        s.rounding = egui::Rounding::same(3.0);
    }
    w.inactive.bg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_DIM);
}

/// Preview for "set the selected services to `ty`". `names` = selection without the locked ones.
/// `delayed_kept` = selected services left alone by "Automatic" because they already start
/// automatically with Delayed start (the delay is kept).
fn change_window(ty: &str, names: &[String], locked: &[String], work: &[Work], stop_on_disable: bool, delayed_kept: usize) -> OpWindow {
    let items = work.iter().map(|w| OpItem::new(&w.name, &w.name, format!("Set to {}", w.to)).detail(arrow(&w.from, &w.to))).collect();
    let mut win = base_window("Services", items);
    let mut text = match ty {
        "Disabled" => "They will not be able to start at all.".to_string(),
        "Manual" => "They will start only when something needs them.".to_string(),
        _ => "They will start together with Windows.".to_string(),
    };
    if ty == "Disabled" && stop_on_disable {
        text += " Running ones are stopped right away.";
    }
    let already = names.len().saturating_sub(work.len()).saturating_sub(delayed_kept);
    if already > 0 {
        text += &format!(" {already} already set - left alone.");
    }
    if delayed_kept > 0 {
        text += &format!(" {delayed_kept} already start automatically (Delayed start kept) - left alone.");
    }
    win.preview_text = text;
    win.go_label = format!("Set to {ty}");
    win.reassure = "Every change is recorded - 'Undo last change' puts it back.".into();

    let mut warn: Vec<String> = vec![];
    let gaming: Vec<&str> = names.iter().filter(|n| note_for(n) == NOTE_GAMING).map(|s| s.as_str()).collect();
    if !gaming.is_empty() && ty != "Automatic" {
        warn.push(format!("These are needed by Xbox / Game Pass / Steam / anti-cheat: {}. Set them to {ty} anyway?", gaming.join(", ")));
    }
    if !locked.is_empty() {
        warn.push(format!("These are core Windows services and are locked (they will be skipped): {}", locked.join(", ")));
    }
    if !warn.is_empty() {
        win.warning = Some(warn.join("\n"));
    }
    if names.is_empty() {
        win.empty_title = "Nothing was changed".into();
        win.empty_text = format!("These are core Windows services and cannot be changed: {}", locked.join(", "));
    } else {
        win.empty_title = "Nothing to change".into();
        win.empty_text = if delayed_kept > 0 {
            "Everything you selected already starts automatically (some with Delayed start, which is kept).".to_string()
        } else {
            format!("Everything you selected is already set to {ty}.")
        };
        if !locked.is_empty() {
            win.empty_text += &format!(" Locked core Windows services were skipped: {}", locked.join(", "));
        }
    }
    win
}

fn preset_window(include_optional: bool, work: &[Work]) -> OpWindow {
    let name = preset_name(include_optional);
    let items = work.iter().map(|w| OpItem::new(&w.name, &w.name, format!("Set to {}", w.to)).detail(arrow(&w.from, &w.to))).collect();
    let mut win = base_window(&format!("{name} preset"), items);
    win.preview_text = if include_optional {
        "Safe preset plus Windows Search, SysMain, file sharing, diagnostics, Windows AI and smart card / biometrics.".into()
    } else {
        "Telemetry, printing, remote desktop host, unused networking and consumer extras.".into()
    };
    win.reassure = "No Xbox / Steam services are included. 'Undo last change' reverts everything this applies.".into();
    win.go_label = "Apply".into();
    // every service the preset would change is ticked; untick any to leave it as it is
    win.pickable = true;
    win.running_text = format!("Applying the {name} preset...");
    win.empty_title = "Already applied".into();
    win.empty_text = format!("The {name} preset is already fully applied.");
    win
}

/// Preview for reverting history entries (already sorted newest first).
/// `picked` = the history indices the user chose; other entries are newer changes pulled in by undo_chain.
fn undo_window(entries: &[(usize, HistEntry)], picked: &[usize]) -> OpWindow {
    let items = entries.iter().map(|(i, h)| OpItem::new(i.to_string(), &h.name, format!("Back to {}", h.from)).detail(arrow(&h.to, &h.from))).collect();
    let mut win = OpWindow::new("Undo", "restore", "restored", items).noun("service", "services");
    win.preview_text = "Each service goes back to the startup type it had before the change.".into();
    let extra: Vec<&HistEntry> = entries.iter().filter(|(i, _)| !picked.contains(i)).map(|(_, h)| h).collect();
    if !extra.is_empty() {
        let mut names: Vec<&str> = extra.iter().map(|h| h.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        win.preview_text += &format!(" Also reverts {} newer change(s) to {} first, so it ends where it started.", extra.len(), names.join(", "));
    }
    win.reassure = "Services that were running before are started again.".into();
    win.go_label = "Undo".into();
    win.running_text = "Restoring services...".into();
    win.empty_title = "Nothing to undo".into();
    win.empty_text = "These changes were already undone.".into();
    win
}

/// Sample window for screenshots (PCTUNER_DEMO=services:<stage>). Touches nothing.
fn demo_window(stage: Stage) -> OpWindow {
    let sample = [
        ("DiagTrack", "Automatic", "Disabled"),
        ("MapsBroker", "Automatic (Delayed)", "Disabled"),
        ("Spooler", "Automatic", "Disabled"),
        ("TermService", "Manual", "Disabled"),
        ("PhoneSvc", "Manual", "Disabled"),
        ("lfsvc", "Manual", "Disabled"),
        ("WMPNetworkSvc", "Manual", "Disabled"),
        ("TrkWks", "Automatic", "Disabled"),
        ("PcaSvc", "Automatic", "Manual"),
        ("StorSvc", "Automatic (Delayed)", "Manual"),
        ("WlanSvc", "Automatic", "Manual"),
    ];
    let work: Vec<Work> = sample.iter().map(|(n, f, t)| Work { name: n.to_string(), from: f.to_string(), to: t.to_string() }).collect();
    let mut win = preset_window(false, &work);
    match stage {
        Stage::Preview => {}
        Stage::Running => win.stage = Stage::Running,
        Stage::Done => demo_finish(&mut win),
    }
    win
}

fn demo_finish(win: &mut OpWindow) {
    win.finish(|it| match it.id.as_str() {
        "Spooler" => Some(Ok("changed, but could not be stopped now (a dependent service is still running)".into())),
        "StorSvc" => Some(Ok("via registry, applies after reboot".into())),
        "TermService" => Some(Err("service manager: ChangeServiceConfig failed: Access is denied. (os error 5) / registry: Access is denied. (os error 5)".into())),
        _ => None,
    });
    win.can_undo = true;
    win.notes = vec!["Restart the PC to finish.".into()];
}

// ---------------------------------------------------------------- tab

/// What "Go" in the open operation window starts.
enum OpKind {
    Apply { plan: Vec<(String, String)>, verb: String, preset: Option<&'static str> },
    Undo(Vec<usize>),
    Demo,
}

enum JobResult {
    Applied { outcome: ApplyOutcome, verb: String, preset: Option<&'static str> },
    Undone(Vec<ItemResult>),
}

struct Loaded {
    list: Vec<Svc>,
    /// Changes the Safe preset would still make.
    safe_pending: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum SortCol {
    Name,
    Display,
    Startup,
    Status,
    Note,
}

const ROW_H: f32 = 27.0;
const HEADER_H: f32 = 26.0;

pub struct ServicesTab {
    list: Vec<Svc>,
    load_job: Option<Job<Result<Loaded, String>>>,
    load_error: Option<String>,
    safe_pending: Option<usize>,
    selected: HashSet<String>,
    search: String,
    running_only: bool,
    hide_locked: bool,
    stop_on_disable: bool,
    sort: SortCol,
    sort_asc: bool,
    history: Vec<HistEntry>,
    job: Option<Job<JobResult>>,
    last_action: String,
    op: Option<OpWindow>,
    op_kind: Option<OpKind>,
    /// History batch written by the operation shown in the result stage (for its Undo button).
    op_batch: Option<u64>,
    show_history: bool,
    /// The Backups window while it is open.
    backups: Option<crate::svcbackup::BackupsWindow>,
    hist_sel: HashSet<usize>,
}

impl ServicesTab {
    pub fn new() -> Self {
        let mut t = Self::with_history(load_history());
        t.start_load();
        if std::env::var("PCTUNER_DEMO").is_ok_and(|v| v.eq_ignore_ascii_case("services:history")) {
            // screenshot hook for the history window (read-only)
            t.show_history = true;
        } else if let Some(stage) = opwin::demo_stage("services") {
            t.op = Some(demo_window(stage));
            t.op_kind = Some(OpKind::Demo);
        }
        t
    }

    /// The tab with nothing loaded or running yet.
    fn with_history(history: Vec<HistEntry>) -> Self {
        ServicesTab {
            list: vec![],
            load_job: None,
            load_error: None,
            safe_pending: None,
            selected: HashSet::new(),
            search: String::new(),
            running_only: false,
            hide_locked: false,
            stop_on_disable: true,
            sort: SortCol::Display,
            sort_asc: true,
            history,
            job: None,
            last_action: String::new(),
            op: None,
            op_kind: None,
            op_batch: None,
            show_history: false,
            backups: None,
            hist_sel: HashSet::new(),
        }
    }

    fn start_load(&mut self) {
        self.load_job = Some(Job::spawn(|| {
            let list = load_services()?;
            Ok(Loaded { list, safe_pending: pending_plan(&System, &live_preset_plan(&System, false)).len() })
        }));
    }

    fn next_batch(&self) -> u64 {
        self.history.iter().map(|h| h.batch).max().unwrap_or(0) + 1
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<usize> = (0..self.list.len())
            .filter(|&i| {
                let s = &self.list[i];
                (q.is_empty() || s.name_lc.contains(&q) || s.display_lc.contains(&q))
                    && (!self.running_only || s.state == "Running")
                    && (!self.hide_locked || s.note != NOTE_LOCKED)
            })
            .collect();
        let l = &self.list;
        v.sort_by(|&a, &b| {
            let (a, b) = (&l[a], &l[b]);
            let o = match self.sort {
                SortCol::Name => a.name_lc.cmp(&b.name_lc),
                SortCol::Display => a.display_lc.cmp(&b.display_lc),
                SortCol::Startup => a.start.cmp(&b.start),
                SortCol::Status => a.state.cmp(&b.state),
                SortCol::Note => a.note.cmp(b.note),
            }
            .then_with(|| a.display_lc.cmp(&b.display_lc));
            if self.sort_asc { o } else { o.reverse() }
        });
        v
    }

    /// Selected services that are currently shown (filters hide rows from the action, like the PS grid).
    fn selected_visible(&self) -> Vec<String> {
        self.visible().into_iter().map(|i| &self.list[i].name).filter(|n| self.selected.contains(*n)).cloned().collect()
    }

    fn open_op(&mut self, win: OpWindow, kind: OpKind) {
        self.op = Some(win);
        self.op_kind = Some(kind);
        self.op_batch = None;
    }

    fn request_change(&mut self, ty: &'static str) {
        let names = self.selected_visible();
        if names.is_empty() {
            self.last_action = "Select one or more services first.".into();
            return;
        }
        let (locked, names): (Vec<String>, Vec<String>) = names.into_iter().partition(|n| note_for(n) == NOTE_LOCKED);
        let plan: Vec<(String, String)> = names.iter().map(|n| (n.clone(), ty.to_string())).collect();
        let work = pending_plan(&System, &plan);
        let delayed_kept = if ty == "Automatic" {
            names.iter().filter(|n| !work.iter().any(|w| w.name == **n) && start_type(&System, n) == "Automatic (Delayed)").count()
        } else {
            0
        };
        let win = change_window(ty, &names, &locked, &work, self.stop_on_disable, delayed_kept);
        self.open_op(win, OpKind::Apply { plan, verb: format!("set to {ty}"), preset: None });
    }

    fn request_preset(&mut self, include_optional: bool) {
        let name = preset_name(include_optional);
        let plan = live_preset_plan(&System, include_optional);
        let work = pending_plan(&System, &plan);
        let win = preset_window(include_optional, &work);
        self.open_op(win, OpKind::Apply { plan, verb: format!("changed by {name} preset"), preset: Some(name) });
    }

    fn start_apply(&mut self, plan: Vec<(String, String)>, verb: String, preset: Option<&'static str>) {
        let batch = self.next_batch();
        let stop = self.stop_on_disable;
        let info: HashMap<String, (String, bool)> = self.list.iter().map(|s| (s.name.clone(), (s.display.clone(), s.state == "Running"))).collect();
        self.last_action = "Applying...".into();
        self.job = Some(Job::spawn(move || {
            // a full backup first, if there is none yet (one of the ways back)
            let _ = crate::svcbackup::ensure_backup();
            let outcome = apply_plan(&System, plan, batch, stop, info);
            JobResult::Applied { outcome, verb, preset }
        }));
    }

    /// See undo_chain: the picked entries plus newer active changes to the same services, newest first.
    fn undo_order(&self, idx: &[usize]) -> Vec<(usize, HistEntry)> {
        undo_chain(&self.history, idx)
    }

    fn request_undo_last(&mut self) {
        let Some(last) = self.history.iter().filter(|h| !h.undone).map(|h| h.batch).max() else {
            self.last_action = "Nothing to undo.".into();
            return;
        };
        let idx: Vec<usize> = (0..self.history.len()).filter(|&i| !self.history[i].undone && self.history[i].batch == last).collect();
        self.request_undo(idx);
    }

    fn request_undo(&mut self, idx: Vec<usize>) {
        let win = undo_window(&self.undo_order(&idx), &idx);
        self.open_op(win, OpKind::Undo(idx));
    }

    /// "Undo" in the result stage: revert the batch that was just applied (no second preview).
    fn undo_op_batch(&mut self) {
        let Some(batch) = self.op_batch.take() else {
            if matches!(self.op_kind, Some(OpKind::Demo)) {
                self.op = None;
            }
            return;
        };
        let idx: Vec<usize> = (0..self.history.len()).filter(|&i| !self.history[i].undone && self.history[i].batch == batch).collect();
        let mut win = undo_window(&self.undo_order(&idx), &idx);
        if !win.items.is_empty() {
            win.stage = Stage::Running;
        }
        self.op = Some(win);
        self.op_kind = None;
        self.start_undo(idx);
    }

    fn start_undo(&mut self, idx: Vec<usize>) {
        let entries = self.undo_order(&idx);
        if entries.is_empty() {
            self.last_action = "Nothing to undo.".into();
            if let Some(op) = self.op.as_mut() {
                op.finish(|_| Some(Ok("already undone".into())));
            }
            return;
        }
        self.last_action = "Undoing...".into();
        self.job = Some(Job::spawn(move || JobResult::Undone(undo_entries(&System, entries))));
    }

    fn finish_job(&mut self, r: JobResult) {
        match r {
            JobResult::Applied { outcome: o, verb, preset } => {
                let ok = o.results.iter().filter(|r| r.result.is_ok()).count();
                let failed = o.results.len() - ok;
                let batch = o.entries.first().map(|e| e.batch);
                if o.results.is_empty() {
                    self.last_action = "Nothing to change - all already set".to_string();
                    if o.missing > 0 {
                        self.last_action += &format!(" ({} not installed)", o.missing);
                    }
                    self.last_action += ".";
                } else {
                    self.history.extend(o.entries);
                    common::save_json(HISTORY, &self.history);
                    self.start_load();
                    let mut summary = format!("{ok} {verb}");
                    if o.skipped > 0 {
                        summary += &format!(", {} already set", o.skipped);
                    }
                    if failed > 0 {
                        summary += &format!(", {failed} failed");
                    }
                    if o.missing > 0 {
                        summary += &format!(", {} not installed", o.missing);
                    }
                    self.last_action = summary + ".";
                }
                self.op_batch = batch;
                if let Some(op) = self.op.as_mut() {
                    let find = |id: &str| o.results.iter().find(|r| r.name.eq_ignore_ascii_case(id));
                    for it in op.items.iter_mut() {
                        if let Some(r) = find(&it.id) {
                            it.detail = arrow(&r.from, &r.to);
                        }
                    }
                    op.finish(|it| Some(find(&it.id).map(|r| r.result.clone()).unwrap_or_else(|| Ok("already set".into()))));
                    op.can_undo = batch.is_some();
                    if preset.is_some() && ok > 0 {
                        op.notes = vec!["Restart the PC to finish.".into()];
                    }
                }
            }
            JobResult::Undone(results) => {
                let mut done = 0;
                let mut retired = 0;
                for r in &results {
                    if r.result.is_ok() || r.retire {
                        if r.result.is_ok() {
                            done += 1;
                        } else {
                            retired += 1;
                        }
                        if let Some(h) = self.history.get_mut(r.index) {
                            h.undone = true;
                        }
                    }
                }
                common::save_json(HISTORY, &self.history);
                self.start_load();
                self.last_action = format!("Undid {done} change(s).");
                if retired > 0 {
                    self.last_action += &format!(" {retired} left alone (changed since).");
                }
                if results.len() > done + retired {
                    self.last_action += &format!(" {} could not be undone.", results.len() - done - retired);
                }
                if let Some(op) = self.op.as_mut() {
                    let find = |id: &str| results.iter().find(|r| r.index.to_string() == id);
                    for it in op.items.iter_mut() {
                        if let Some(r) = find(&it.id) {
                            it.name = r.name.clone();
                            it.detail = arrow(&r.from, &r.to);
                        }
                    }
                    op.finish(|it| find(&it.id).map(|r| r.result.clone()));
                    op.can_undo = false;
                }
            }
        }
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let rows = self.visible();
        let mut sort = self.sort;
        let mut asc = self.sort_asc;
        let mut toggled: Option<String> = None;
        ui.spacing_mut().item_spacing.y = 3.0;
        let avail = (ui.available_height() - HEADER_H - ui.spacing().item_spacing.y).max(80.0);

        TableBuilder::new(ui)
            .id_salt("svc-table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(0.0)
            .max_scroll_height(avail)
            .auto_shrink([false, false])
            .column(Column::initial(190.0).at_least(60.0).clip(true))
            .column(Column::initial(310.0).at_least(80.0).clip(true))
            .column(Column::initial(150.0).at_least(60.0).clip(true))
            .column(Column::initial(90.0).at_least(50.0).clip(true))
            .column(Column::remainder().at_least(60.0).clip(true))
            .header(HEADER_H, |mut header| {
                for (col, label) in [
                    (SortCol::Name, "Name"),
                    (SortCol::Display, "Display name"),
                    (SortCol::Startup, "Startup"),
                    (SortCol::Status, "Status"),
                    (SortCol::Note, "Note"),
                ] {
                    header.col(|ui| {
                        let active = sort == col;
                        let txt = if active { format!("{label}  {}", if asc { "⏶" } else { "⏷" }) } else { label.to_string() };
                        let rt = egui::RichText::new(txt).size(12.5).strong().color(if active { theme::ACCENT } else { theme::TEXT_DIM });
                        let r = ui.add(egui::Label::new(rt).selectable(false).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clicked() {
                            if active {
                                asc = !asc;
                            } else {
                                sort = col;
                                asc = true;
                            }
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let s = &self.list[rows[row.index()]];
                    row.set_selected(self.selected.contains(&s.name));
                    let c = match s.note {
                        NOTE_GAMING => Some(theme::GAMING),
                        NOTE_LOCKED => Some(theme::LOCKED),
                        _ if s.start == "Disabled" => Some(theme::DIM),
                        _ => None,
                    };
                    let start_c = if s.start == "Disabled" { Some(theme::DIM) } else { c };
                    let cell = |ui: &mut egui::Ui, t: &str, c: Option<egui::Color32>| {
                        let mut rt = egui::RichText::new(t);
                        if let Some(c) = c {
                            rt = rt.color(c);
                        }
                        ui.add(egui::Label::new(rt).selectable(false).truncate());
                    };
                    row.col(|ui| cell(ui, &s.name, c));
                    row.col(|ui| cell(ui, &s.display, c));
                    row.col(|ui| cell(ui, &s.start, start_c));
                    row.col(|ui| cell(ui, &s.state, c.or(if s.state == "Running" { None } else { Some(theme::TEXT_DIM) })));
                    row.col(|ui| cell(ui, s.note, c));
                    if row.response().clicked() {
                        toggled = Some(s.name.clone());
                    }
                });
            });

        self.sort = sort;
        self.sort_asc = asc;
        if let Some(n) = toggled {
            if !self.selected.remove(&n) {
                self.selected.insert(n);
            }
        }
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        let mut close = false;
        let mut undo_sel = false;
        let mut clear = false;
        let can = common::is_admin() && !self.busy();
        let active = self.history.iter().filter(|h| !h.undone).count();
        theme::dim_backdrop(ctx, "svc-history");
        egui::Window::new("svc-history-window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width((ctx.screen_rect().width() - 80.0).min(930.0));
                theme::section(ui, "Services");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Change history").size(26.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        chip(ui, &format!("{active} can be undone"), if active > 0 { theme::ACCENT } else { theme::DIM });
                        chip(ui, &format!("{} recorded", self.history.len()), theme::DIM);
                    });
                });
                ui.label(egui::RichText::new("Tick the changes you want to take back, newest first.").color(theme::TEXT_DIM));
                ui.add_space(6.0);
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if self.history.is_empty() {
                        ui.label(egui::RichText::new("No changes recorded yet.").color(theme::TEXT_DIM));
                        return;
                    }
                    egui::ScrollArea::both().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                        egui::Grid::new("svc-history").striped(true).num_columns(7).min_row_height(ROW_H - 4.0).spacing([16.0, 4.0]).show(ui, |ui| {
                            // the tick boxes need an outline to be visible on the card
                            outline_checks(ui);
                            ui.label("");
                            for h in ["Time", "Name", "Display name", "From", "To", "State"] {
                                ui.label(egui::RichText::new(h).size(12.5).strong().color(theme::TEXT_DIM));
                            }
                            ui.end_row();
                            for i in (0..self.history.len()).rev() {
                                let h = &self.history[i];
                                let mut on = self.hist_sel.contains(&i);
                                if ui.add_enabled(!h.undone, egui::Checkbox::without_text(&mut on)).changed() {
                                    if on {
                                        self.hist_sel.insert(i);
                                    } else {
                                        self.hist_sel.remove(&i);
                                    }
                                }
                                let c = if h.undone { theme::DIM } else { theme::TEXT };
                                let short = |t: &str, max: usize| -> String {
                                    if t.chars().count() > max {
                                        format!("{}...", t.chars().take(max - 3).collect::<String>())
                                    } else {
                                        t.to_string()
                                    }
                                };
                                ui.colored_label(c, &h.time);
                                ui.colored_label(c, short(&h.name, 26));
                                ui.colored_label(c, short(&h.display_name, 34)).on_hover_text(&h.display_name);
                                ui.colored_label(c, &h.from);
                                ui.colored_label(c, &h.to);
                                ui.colored_label(if h.undone { theme::DIM } else { theme::ACCENT }, if h.undone { "Undone" } else { "Active" });
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let n = self.hist_sel.len();
                    let label = if n > 0 { format!("   Undo selected ({n})   ") } else { "   Undo selected   ".to_string() };
                    if ui.add_enabled(can && n > 0, theme::primary_button(&label)).clicked() {
                        undo_sel = true;
                    }
                    if ui.add_enabled(!self.busy(), egui::Button::new("Clear undone entries")).on_hover_text("Removes already-undone rows from this list. Changes nothing on the PC.").clicked() {
                        clear = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    if self.busy() {
                        ui.spinner();
                    }
                    if !common::is_admin() {
                        ui.colored_label(theme::LOCKED, "View only - start PC Tuner as administrator.");
                    }
                });
            });
        if undo_sel {
            let idx: Vec<usize> = self.hist_sel.drain().collect();
            self.request_undo(idx);
        }
        if clear {
            self.history.retain(|h| !h.undone);
            self.hist_sel.clear();
            common::save_json(HISTORY, &self.history);
        }
        if close {
            self.show_history = false;
        }
    }

    /// Runs the open operation window and reacts to its buttons.
    fn op_window(&mut self, ctx: &egui::Context, can: bool) {
        let Some(op) = self.op.as_mut() else { return };
        match op.show(ctx, can) {
            Some(OpEvent::Go) => match self.op_kind.take() {
                Some(OpKind::Apply { plan, verb, preset }) => {
                    // services the user unticked in the preview are left as they are
                    let unpicked = self.op.as_ref().map(|o| o.unpicked.clone()).unwrap_or_default();
                    let plan = picked_plan(plan, &unpicked);
                    self.start_apply(plan, verb, preset)
                }
                Some(OpKind::Undo(idx)) => self.start_undo(idx),
                Some(OpKind::Demo) => {
                    if let Some(op) = self.op.as_mut() {
                        demo_finish(op);
                    }
                    self.op_kind = Some(OpKind::Demo);
                }
                None => self.op = None,
            },
            Some(OpEvent::Close) => {
                self.op = None;
                self.op_kind = None;
                self.op_batch = None;
            }
            Some(OpEvent::Undo) => self.undo_op_batch(),
            Some(OpEvent::Extra(_)) | None => {}
        }
    }
}

/// Same look as theme::chip, but painted at its natural size (a framed chip stretches to the
/// row height when it sits in a tall row such as the hero).
fn chip(ui: &mut egui::Ui, text: &str, c: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.5), c);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(20.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 99.0, theme::tint(c, 30));
    ui.painter().galley(rect.min + egui::vec2(10.0, 4.0), galley, c);
}

/// Small coloured dot + caption (table legend).
fn legend(ui: &mut egui::Ui, c: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, c);
        ui.label(egui::RichText::new(text).size(12.0).color(theme::TEXT_DIM));
    });
}

impl Tab for ServicesTab {
    fn title(&self) -> &'static str {
        "Services"
    }

    /// A service change is running: closing PC Tuner waits for it.
    fn busy(&self) -> bool {
        self.job.is_some() || self.backups.as_ref().map(|b| b.busy()).unwrap_or(false)
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        // background work
        if let Some(r) = self.load_job.as_ref().and_then(|j| j.poll()) {
            self.load_job = None;
            match r {
                Ok(l) => {
                    self.list = l.list;
                    self.safe_pending = Some(l.safe_pending);
                    self.load_error = None;
                }
                Err(e) => self.load_error = Some(e),
            }
        }
        if let Some(r) = self.job.as_ref().and_then(|j| j.poll()) {
            self.job = None;
            self.finish_job(r);
        }

        let ctx = ui.ctx().clone();
        let can = common::is_admin() && !self.busy();
        self.op_window(&ctx, can);
        if let Some(b) = &mut self.backups {
            let (keep, changed) = b.show(&ctx);
            if !keep {
                self.backups = None;
            }
            if changed {
                self.start_load();
            }
        }
        if self.op.is_none() && self.backups.is_none() {
            self.history_window(&ctx);
        }

        // header, with the two view-only buttons on the title row
        let top = ui.cursor().min;
        let right = ui.max_rect().right();
        theme::page_header(ui, "Services", "Change startup types with full undo. Gaming dependencies warn first; core Windows services are locked.");
        {
            let r = egui::Rect::from_min_max(egui::pos2(right - 380.0, top.y), egui::pos2(right, top.y + 32.0));
            let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)));
            if bar.add_enabled(self.load_job.is_none(), egui::Button::new("Refresh")).on_hover_text("Read the service list again.").clicked() {
                self.start_load();
            }
            if bar.button("Backups").on_hover_text("Save or restore the startup type of every service.").clicked() {
                self.backups = Some(crate::svcbackup::BackupsWindow::new());
            }
            if bar.button("History").on_hover_text("Every change made here, with undo per entry.").clicked() {
                self.show_history = true;
            }
            if self.load_job.is_some() {
                bar.spinner();
            }
        }
        common::admin_banner(ui);

        // hero
        let running = self.list.iter().filter(|s| s.state == "Running").count();
        let disabled = self.list.iter().filter(|s| s.start == "Disabled").count();
        let undoable = self.history.iter().filter(|h| !h.undone).count();
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let shown = theme::count_up(ui.ctx(), "svc-running", running as f32).round() as usize;
                theme::hero(ui, &shown.to_string(), "running", "services active right now", theme::ACCENT);
                ui.add_space(30.0);
                theme::stat(ui, &disabled.to_string(), "disabled", theme::DIM);
                ui.add_space(22.0);
                theme::stat(ui, &self.list.len().to_string(), "total", theme::TEXT);
                ui.add_space(22.0);
                theme::stat(ui, &undoable.to_string(), "changes that can be undone", if undoable > 0 { theme::TEXT } else { theme::DIM });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match self.safe_pending {
                    None => chip(ui, "Safe preset: checking...", theme::DIM),
                    Some(0) => chip(ui, "Safe preset: applied", theme::OK),
                    Some(n) => chip(ui, &format!("Safe preset: {n} pending"), theme::JUNK),
                });
            });
        });

        // actions for the selection (laid out before the table so the table gets the rest)
        egui::TopBottomPanel::bottom("svc-bottom").frame(egui::Frame::none()).show_separator_line(false).show_inside(ui, |ui| {
            ui.add_space(8.0);
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // the actions only act on selected rows that are shown (see selected_visible)
                    let vis = self.selected_visible().len();
                    let total = self.selected.len();
                    let has = vis > 0;
                    if total == 0 {
                        chip(ui, "Click rows to select", theme::DIM);
                    } else if vis == total {
                        chip(ui, &format!("{vis} selected"), theme::ACCENT);
                    } else if vis > 0 {
                        chip(ui, &format!("{vis} selected ({} hidden by filter)", total - vis), theme::ACCENT);
                    } else {
                        chip(ui, &format!("{total} selected, all hidden by filter"), theme::DIM);
                    }
                    if ui.add_enabled(can && has, theme::danger_button("Disable")).on_hover_text("Set selected services to Disabled (they cannot start at all).").clicked() {
                        self.request_change("Disabled");
                    }
                    if ui.add_enabled(can && has, egui::Button::new("Manual")).on_hover_text("Set selected services to Manual (start only when something needs them).").clicked() {
                        self.request_change("Manual");
                    }
                    if ui.add_enabled(can && has, egui::Button::new("Automatic")).on_hover_text("Set selected services to start automatically at boot.").clicked() {
                        self.request_change("Automatic");
                    }
                    if total > 0 && ui.button("Clear").on_hover_text("Clear the selection.").clicked() {
                        self.selected.clear();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let has_undo = self.history.iter().any(|h| !h.undone);
                        if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).on_hover_text("Revert the most recent batch of changes.").clicked() {
                            self.request_undo_last();
                        }
                        if self.busy() {
                            ui.spinner();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    legend(ui, theme::GAMING, "Gaming dependency");
                    legend(ui, theme::LOCKED, "Locked core service");
                    legend(ui, theme::DIM, "Disabled");
                    ui.add_space(10.0);
                    ui.scope(|ui| {
                        outline_checks(ui);
                        ui.checkbox(&mut self.stop_on_disable, "Stop when disabling").on_hover_text("Also stop a running service right away when it is set to Disabled.");
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(e) = &self.load_error {
                            ui.add(egui::Label::new(egui::RichText::new(format!("Could not load services: {e}")).size(12.0).color(theme::LOCKED)).truncate());
                        } else if !self.last_action.is_empty() {
                            ui.add(egui::Label::new(egui::RichText::new(&self.last_action).size(12.0).color(theme::TEXT_DIM)).truncate());
                        }
                    });
                });
            });
        });

        // toolbar + table
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 560.0).clamp(130.0, 260.0);
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search services").desired_width(w));
                ui.scope(|ui| {
                    outline_checks(ui);
                    ui.checkbox(&mut self.running_only, "Running only");
                    ui.checkbox(&mut self.hide_locked, "Hide locked").on_hover_text("Hide core Windows services (they cannot be changed).");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(can, egui::Button::new("Safe + Optional"))
                        .on_hover_text("Safe preset plus: Windows Search, SysMain, file sharing, diagnostics, Windows AI, Phone Link, Logitech RGB, smart card / biometrics.")
                        .clicked()
                    {
                        self.request_preset(true);
                    }
                    if ui
                        .add_enabled(can, theme::primary_button("Apply Safe preset"))
                        .on_hover_text("Telemetry, printing, remote desktop host, unused networking and consumer extras. No gaming impact.")
                        .clicked()
                    {
                        self.request_preset(false);
                    }
                });
            });
            ui.add_space(2.0);
            if self.list.is_empty() && self.load_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Loading services...").color(theme::TEXT_DIM));
                });
                ui.allocate_space(egui::vec2(0.0, ui.available_height()));
            } else {
                self.table(ui);
            }
        });
    }
}

// ---------------------------------------------------------------- tests (read-only)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_preset_preview_is_pickable() {
        let work = vec![
            Work { name: "DiagTrack".into(), from: "Automatic".into(), to: "Disabled".into() },
            Work { name: "Spooler".into(), from: "Automatic".into(), to: "Manual".into() },
        ];
        for opt in [false, true] {
            let w = preset_window(opt, &work);
            assert!(w.pickable && w.unpicked.is_empty(), "every preset service starts ticked");
            assert_eq!(w.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["DiagTrack", "Spooler"]);
        }
        // a manual "set to" change stays all-or-nothing
        assert!(!change_window("Manual", &["Spooler".into()], &[], &work[1..], false, 0).pickable);
        // unticked services are left out of the plan, the rest keep their order and targets
        let plan: Vec<(String, String)> = vec![("DiagTrack".into(), "Disabled".into()), ("Spooler".into(), "Manual".into()), ("Fax".into(), "Disabled".into())];
        let un: HashSet<String> = ["Spooler".to_string()].into();
        assert_eq!(picked_plan(plan.clone(), &un), vec![plan[0].clone(), plan[2].clone()]);
        assert_eq!(picked_plan(plan.clone(), &HashSet::new()), plan);
    }

    #[test]
    fn services_base_name() {
        assert_eq!(base_name("CDPUserSvc_70560"), "CDPUserSvc");
        assert_eq!(base_name("Foo_1a2b3c"), "Foo");
        assert_eq!(base_name("EasyAntiCheat_EOS"), "EasyAntiCheat_EOS");
        assert_eq!(base_name("Foo_123"), "Foo_123");
        assert_eq!(base_name("Foo_123456789"), "Foo_123456789");
        assert_eq!(base_name("_abcd"), "_abcd");
    }

    #[test]
    fn services_busy_through_tab_trait() {
        let mut t = ServicesTab::with_history(vec![]);
        assert!(!(&t as &dyn Tab).busy());
        // a job that never finishes (the parked thread ends with the test process)
        t.job = Some(Job::spawn(|| -> JobResult {
            loop {
                std::thread::park();
            }
        }));
        assert!((&t as &dyn Tab).busy());
    }

    #[test]
    fn services_import_only_on_first_run() {
        let d = std::env::temp_dir().join(format!("pctuner-test-{}-svc-first-run", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(first_run(&d));
        std::fs::write(d.join(HISTORY), "[]").unwrap();
        assert!(!first_run(&d));
        // a damaged file was moved aside: no longer a first run, so the PS history is not re-imported
        std::fs::rename(d.join(HISTORY), d.join(format!("{HISTORY}.corrupt-123"))).unwrap();
        assert!(!first_run(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn services_presets() {
        let safe = preset_plan(false);
        let opt = preset_plan(true);
        let get = |p: &[(String, String)], n: &str| p.iter().find(|(x, _)| x == n).map(|(_, t)| t.clone());
        assert_eq!(get(&safe, "PcaSvc").as_deref(), Some("Manual"));
        assert_eq!(get(&opt, "PcaSvc").as_deref(), Some("Disabled"));
        assert!(get(&safe, "WSearch").is_none());
        assert!(opt.iter().all(|(n, _)| !matches!(note_for(n), NOTE_GAMING | NOTE_LOCKED)));
        println!("safe preset: {} entries, safe+optional: {} entries", safe.len(), opt.len());
    }

    #[test]
    fn services_import_ps_history() {
        let arr = "\u{feff}[{\"Batch\":2,\"Time\":\"2026-01-01 10:00:00\",\"Name\":\"DiagTrack\",\"DisplayName\":\"Connected User Experiences\",\"From\":\"Automatic\",\"To\":\"Disabled\",\"WasRunning\":true,\"Undone\":false}]";
        let one = "{\"Batch\":1,\"Time\":\"t\",\"Name\":\"Spooler\",\"DisplayName\":null,\"From\":\"Automatic\",\"To\":\"Disabled\",\"WasRunning\":false,\"Undone\":true}";
        let a = parse_ps_history(arr);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].batch, 2);
        assert!(a[0].was_running && !a[0].undone);
        let b = parse_ps_history(one);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].display_name, "Spooler");
        assert!(b[0].undone);
    }

    #[test]
    fn services_live_list() {
        let list = load_services().expect("load services");
        let running = list.iter().filter(|s| s.state == "Running").count();
        let disabled = list.iter().filter(|s| s.start == "Disabled").count();
        let unknown = list.iter().filter(|s| s.start == "Unknown").count();
        let count = |n: &str| list.iter().filter(|s| s.note == n).count();
        println!(
            "{} services | {} running | {} disabled | {} unknown start | {} locked | {} gaming | {} per-user",
            list.len(),
            running,
            disabled,
            unknown,
            count(NOTE_LOCKED),
            count(NOTE_GAMING),
            count(NOTE_PER_USER)
        );
        assert!(!list.is_empty());
        for n in ["GamingServices", "RpcSs", "Spooler", "XboxGipSvc", "Audiosrv"] {
            if let Some(s) = list.iter().find(|s| s.name.eq_ignore_ascii_case(n)) {
                println!("  {:<22} {:<22} {:<10} {}", s.name, s.start, s.state, s.note);
            } else {
                println!("  {n}: not installed");
            }
        }
        assert_eq!(note_for("RpcSs"), NOTE_LOCKED);
        assert_eq!(note_for("GamingServices"), NOTE_GAMING);
        for s in list.iter().filter(|s| s.note == NOTE_PER_USER).take(3) {
            println!("  per-user: {} (template {:?}) start {}", s.name, template(&s.name), s.start);
        }
        println!("pending Safe: {} change(s), Safe+Optional: {} change(s)", pending_plan(&System, &live_preset_plan(&System, false)).len(), pending_plan(&System, &live_preset_plan(&System, true)).len());
    }

    // ---- in-memory round trips (no admin, nothing on the PC is touched)

    use std::cell::RefCell;

    #[derive(Default)]
    struct Mock {
        reg: RefCell<HashMap<String, (u32, Option<u32>)>>,
        running: RefCell<HashSet<String>>,
        started: RefCell<Vec<String>>,
        /// the service manager refuses these (registry fallback still works)
        sc_refuses: HashSet<String>,
        /// both the service manager and the registry refuse these
        denied: HashSet<String>,
    }

    impl Mock {
        fn with(entries: &[(&str, u32, Option<u32>)]) -> Mock {
            let m = Mock::default();
            for (n, s, d) in entries {
                m.reg.borrow_mut().insert(n.to_string(), (*s, *d));
            }
            m
        }
        fn get(&self, n: &str) -> Option<(u32, Option<u32>)> {
            self.reg.borrow().get(n).copied()
        }
        fn writable(&self, n: &str) -> Result<(), String> {
            if self.denied.contains(n) {
                return Err("Access is denied.".into());
            }
            if !self.reg.borrow().contains_key(n) {
                return Err(format!("cannot open registry key {n}"));
            }
            Ok(())
        }
    }

    impl Backend for Mock {
        fn keys(&self) -> Vec<String> {
            self.reg.borrow().keys().cloned().collect()
        }
        fn exists(&self, name: &str) -> bool {
            self.reg.borrow().contains_key(name)
        }
        fn read(&self, name: &str) -> Option<(u32, Option<u32>)> {
            self.get(name)
        }
        fn set_start(&self, name: &str, start: u32) -> Result<(), String> {
            self.writable(name)?;
            self.reg.borrow_mut().get_mut(name).unwrap().0 = start;
            Ok(())
        }
        fn set_delayed(&self, name: &str, v: Option<u32>) -> Result<(), String> {
            self.writable(name)?;
            self.reg.borrow_mut().get_mut(name).unwrap().1 = v;
            Ok(())
        }
        fn sc_config(&self, name: &str, sc_arg: &str) -> Result<(), String> {
            if self.sc_refuses.contains(name) {
                return Err("[SC] ChangeServiceConfig FAILED 5".into());
            }
            self.writable(name)?;
            let mut reg = self.reg.borrow_mut();
            let e = reg.get_mut(name).unwrap();
            match sc_arg {
                "disabled" => e.0 = 4,
                "demand" => e.0 = 3,
                "auto" => *e = (2, Some(0)),
                "delayed-auto" => *e = (2, Some(1)),
                _ => return Err("bad arg".into()),
            }
            Ok(())
        }
        fn is_running(&self, name: &str) -> Option<bool> {
            self.exists(name).then(|| self.running.borrow().contains(name))
        }
        fn stop(&self, name: &str) -> Result<(), String> {
            self.running.borrow_mut().remove(name);
            Ok(())
        }
        fn start(&self, name: &str) {
            self.started.borrow_mut().push(name.to_string());
            self.running.borrow_mut().insert(name.to_string());
        }
    }

    fn plan(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
    }

    /// apply, then append to `history` like finish_job does
    fn apply(b: &Mock, history: &mut Vec<HistEntry>, p: Vec<(String, String)>) -> ApplyOutcome {
        let batch = history.iter().map(|h| h.batch).max().unwrap_or(0) + 1;
        let mut o = apply_plan(b, p, batch, true, HashMap::new());
        history.append(&mut o.entries);
        o
    }

    /// undo, then mark entries like finish_job does
    fn undo(b: &Mock, history: &mut [HistEntry], idx: &[usize]) -> Vec<ItemResult> {
        let r = undo_entries(b, undo_chain(history, idx));
        for x in &r {
            if x.result.is_ok() || x.retire {
                history[x.index].undone = true;
            }
        }
        r
    }

    #[test]
    fn services_roundtrip_delayed() {
        for refuse in [false, true] {
            let mut m = Mock::with(&[("MapsBroker", 2, Some(1))]);
            if refuse {
                m.sc_refuses.insert("MapsBroker".into()); // registry fallback path
            }
            m.running.borrow_mut().insert("MapsBroker".into());
            let mut h = vec![];
            let o = apply(&m, &mut h, plan(&[("MapsBroker", "Disabled")]));
            assert!(o.results[0].result.is_ok());
            assert_eq!(h[0].from, "Automatic (Delayed)");
            assert_eq!(m.get("MapsBroker").unwrap().0, 4);
            assert!(!m.running.borrow().contains("MapsBroker"), "stopped when disabling");
            let r = undo(&m, &mut h, &[0]);
            assert!(r[0].result.is_ok(), "{:?}", r[0].result);
            assert_eq!(m.get("MapsBroker"), Some((2, Some(1))));
            assert!(m.started.borrow().contains(&"MapsBroker".to_string()));
        }
        // plain Automatic through the registry removes the delay instead of writing DelayedAutostart=0
        let m = Mock::with(&[("Foo", 4, None)]);
        set_registry_start(&m, "Foo", "Automatic").unwrap();
        assert_eq!(m.get("Foo"), Some((2, None)));
    }

    #[test]
    fn services_roundtrip_per_user_after_reboot() {
        assert_eq!(base_name("NPSMSvc_5fec7"), "NPSMSvc");
        // the instance lost Delayed start; the template is what counts
        let m = Mock::with(&[("cbdhsvc", 2, Some(1)), ("cbdhsvc_7242d", 2, None)]);
        m.running.borrow_mut().insert("cbdhsvc_7242d".into());
        assert_eq!(start_type(&m, "cbdhsvc_7242d"), "Automatic (Delayed)");
        let mut h = vec![];
        let o = apply(&m, &mut h, plan(&[("cbdhsvc_7242d", "Disabled")]));
        assert!(o.results[0].result.is_ok());
        assert_eq!(h[0].from, "Automatic (Delayed)");
        assert_eq!(m.get("cbdhsvc").unwrap().0, 4);
        assert_eq!(m.get("cbdhsvc_7242d").unwrap().0, 4);

        // reboot: the instance comes back from the template with a new suffix
        m.reg.borrow_mut().remove("cbdhsvc_7242d");
        m.reg.borrow_mut().insert("cbdhsvc_9a1b2".into(), (4, Some(1)));
        let r = undo(&m, &mut h, &[0]);
        assert!(r[0].result.is_ok(), "{:?}", r[0].result);
        assert!(h[0].undone);
        assert_eq!(m.get("cbdhsvc"), Some((2, Some(1))));
        assert_eq!(m.get("cbdhsvc_9a1b2"), Some((2, Some(1))));
        assert_eq!(*m.started.borrow(), vec!["cbdhsvc_9a1b2".to_string()]);
        // and a per-user change whose instance key is already gone still succeeds
        assert!(set_start_type(&m, "cbdhsvc_dead1", "Manual").is_ok());
        assert_eq!(m.get("cbdhsvc").unwrap().0, 3);
    }

    #[test]
    fn services_undo_stacked_history() {
        let m = Mock::with(&[("PcaSvc", 3, None)]);
        let mut h = vec![];
        apply(&m, &mut h, plan(&[("PcaSvc", "Disabled")]));
        apply(&m, &mut h, plan(&[("PcaSvc", "Automatic (Delayed)")]));
        // only the older entry is picked: the newer one is pulled in and undone first
        let chain = undo_chain(&h, &[0]);
        assert_eq!(chain.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![1, 0]);
        let r = undo(&m, &mut h, &[0]);
        assert!(r.iter().all(|x| x.result.is_ok()));
        assert_eq!(start_type(&m, "PcaSvc"), "Manual");
        assert!(h.iter().all(|e| e.undone));

        // changed outside PC Tuner since: left alone, retired so Undo last moves on
        let m = Mock::with(&[("PcaSvc", 3, None)]);
        let mut h = vec![];
        apply(&m, &mut h, plan(&[("PcaSvc", "Disabled")]));
        m.reg.borrow_mut().insert("PcaSvc".into(), (2, Some(1)));
        let r = undo(&m, &mut h, &[0]);
        assert!(r[0].result.is_err() && r[0].retire);
        assert_eq!(start_type(&m, "PcaSvc"), "Automatic (Delayed)");

        // a newer entry that fails blocks the older one (not retired, can be retried)
        let mut m = Mock::with(&[("PcaSvc", 3, None)]);
        let mut h = vec![];
        apply(&m, &mut h, plan(&[("PcaSvc", "Disabled")]));
        apply(&m, &mut h, plan(&[("PcaSvc", "Manual")]));
        m.denied.insert("PcaSvc".into());
        let r = undo(&m, &mut h, &[0, 1]);
        assert!(r.iter().all(|x| x.result.is_err() && !x.retire));
        assert!(h.iter().all(|e| !e.undone));
    }

    #[test]
    fn services_safe_after_optional() {
        let all: Vec<(&str, u32, Option<u32>)> = PRESET_SAFE.iter().chain(PRESET_OPTIONAL).map(|(n, _)| (*n, 2, None)).collect();
        let m = Mock::with(&all);
        let mut h = vec![];
        apply(&m, &mut h, live_preset_plan(&m, true));
        assert_eq!(start_type(&m, "PcaSvc"), "Disabled");
        assert!(!live_preset_plan(&m, false).iter().any(|(n, _)| n == "PcaSvc"));
        assert!(pending_plan(&m, &live_preset_plan(&m, false)).is_empty());
        // without Optional applied, Safe still sets PcaSvc to Manual
        let m = Mock::with(&all);
        assert!(pending_plan(&m, &live_preset_plan(&m, false)).iter().any(|w| w.name == "PcaSvc" && w.to == "Manual"));
    }

    #[test]
    fn services_apply_counts_missing() {
        let m = Mock::with(&[("DiagTrack", 4, None), ("Spooler", 2, None)]);
        let mut h = vec![];
        let o = apply(&m, &mut h, plan(&[("DiagTrack", "Disabled"), ("Spooler", "Disabled"), ("WMPNetworkSvc", "Disabled")]));
        assert_eq!((o.results.len(), o.skipped, o.missing), (1, 1, 1));
    }
}
