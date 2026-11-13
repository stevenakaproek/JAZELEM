//! Startup Cleaner tab - port of StartupCleaner.ps1.
//!
//! Covers what Autoruns shows on its main tabs: Run keys, Startup folders, scheduled tasks,
//! Store app startup tasks, Explorer extensions (icon overlays + right-click handlers) and a set
//! of Windows policies that stop background features. Nothing is ever deleted: items are switched
//! off the way Task Manager / Autoruns do it, and every change goes into an undoable history.

use crate::common::{self, Job, Tab};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use winreg::enums::*;
use winreg::types::FromRegValue;
use winreg::{RegKey, RegValue};

const HISTORY: &str = "startup-history.json";
const APPROVED_BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
const STORE_BASE: &str = r"HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
const BLOCKED_KEY: &str = r"HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Shell Extensions\Blocked";
const PATH_REFRESH: &str =
    "$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')\n";

// ------------------------------------------------------------------ lists (copied from StartupCleaner.ps1)

/// Windows scheduled tasks that are telemetry / tips / unused features - safe to switch off.
const JUNK_TASKS: &[&str] = &[
    r"\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser",
    r"\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser Exp",
    r"\Microsoft\Windows\Application Experience\MareBackup",
    r"\Microsoft\Windows\Application Experience\PcaPatchDbTask",
    r"\Microsoft\Windows\Application Experience\ProgramDataUpdater",
    r"\Microsoft\Windows\Application Experience\StartupAppTask",
    r"\Microsoft\Windows\Customer Experience Improvement Program\Consolidator",
    r"\Microsoft\Windows\Customer Experience Improvement Program\UsbCeip",
    r"\Microsoft\Windows\Feedback\Siuf\DmClient",
    r"\Microsoft\Windows\Feedback\Siuf\DmClientOnScenarioDownload",
    r"\Microsoft\Windows\DiskDiagnostic\Microsoft-Windows-DiskDiagnosticDataCollector",
    r"\Microsoft\Windows\Autochk\Proxy",
    r"\Microsoft\Windows\Windows Error Reporting\QueueReporting",
    r"\Microsoft\Windows\Maps\MapsToastTask",
    r"\Microsoft\Windows\Maps\MapsUpdateTask",
    r"\Microsoft\Windows\Shell\FamilySafetyMonitor",
    r"\Microsoft\Windows\Shell\FamilySafetyRefreshTask",
    // added after the full autoruns audit
    r"\Microsoft\Windows\Device Information\Device",
    r"\Microsoft\Windows\Device Information\Device User",
    r"\Microsoft\Windows\Power Efficiency Diagnostics\AnalyzeSystem",
    r"\Microsoft\Windows\Sustainability\PowerGridForecastTask",
    r"\Microsoft\Windows\Sustainability\SustainabilityTelemetry",
    r"\Microsoft\Windows\DiskFootprint\Diagnostics",
    r"\Microsoft\Windows\Diagnosis\RecommendedTroubleshootingScanner",
    r"\Microsoft\Windows\Diagnosis\UnexpectedCodepath",
    r"\Microsoft\Windows\AccountHealth\RecoverabilityToastTask",
    r"\Microsoft\Windows\WwanSvc\OobeDiscovery",
    r"\Microsoft\Windows\WwanSvc\NotificationTask",
    r"\Microsoft\Windows\RetailDemo\CleanupOfflineContent",
    r"\Microsoft\Windows\Printing\PrinterCleanupTask",
    r"\Microsoft\Windows\Printing\PrintJobCleanupTask",
    r"\Microsoft\Windows\Maintenance\WinSAT",
];

/// Windows tasks that are safe to switch off but cost a small convenience.
const OPTIONAL_TASKS: &[(&str, &str)] = &[
    (r"\Microsoft\Windows\WlanSvc\CDSSync", "Optional - Wi-Fi sync (you use Ethernet; switch back on if you use Wi-Fi)"),
    (r"\Microsoft\Windows\AppListBackup\Backup", "Optional - Windows Backup app list to your Microsoft account"),
    (r"\Microsoft\Windows\AppListBackup\BackupNonMaintenance", "Optional - Windows Backup app list to your Microsoft account"),
    (r"\Microsoft\Windows\CloudRestore\Backup", "Optional - settings backup to your Microsoft account"),
    (r"\Microsoft\Windows\Shell\ThemesSyncedImageDownload", "Optional - downloads synced theme wallpapers"),
];

const KEEP_TASKS: &[(&str, &str)] = &[(r"\Microsoft\XblGameSave\XblGameSaveTask", "Keep - syncs Xbox / Game Pass cloud saves")];

struct TweakDef {
    name: &'static str,
    cat: &'static str,
    advice: &'static str,
    /// (PowerShell-style key path, value name, switched-off DWORD)
    values: &'static [(&'static str, &'static str, i64)],
}

const CDM: &str = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";

/// Windows settings / policies that stop background features. "On" = Windows default.
const TWEAKS: &[TweakDef] = &[
    TweakDef {
        name: "Widgets (news feed board)",
        cat: "Junk",
        advice: "Junk - stops WidgetBoard / WidgetService / Start feed running in the background (sign out to apply)",
        values: &[
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Dsh", "AllowNewsAndInterests", 0),
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Dsh", "DisableWidgetsOnLockScreen", 1),
            (r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Lock Screen", "LockScreenWidgetsEnabled", 0),
        ],
    },
    TweakDef {
        name: "Suggested apps, silent app installs and tips",
        cat: "Junk",
        advice: "Junk - stops Windows installing promoted apps and showing tips / Start recommendations",
        values: &[
            (CDM, "SilentInstalledAppsEnabled", 0),
            (CDM, "SystemPaneSuggestionsEnabled", 0),
            (CDM, "SoftLandingEnabled", 0),
            (CDM, "OemPreInstalledAppsEnabled", 0),
            (CDM, "PreInstalledAppsEnabled", 0),
            (CDM, "SubscribedContent-338389Enabled", 0),
            (CDM, "SubscribedContent-338388Enabled", 0),
            (CDM, "SubscribedContent-310093Enabled", 0),
            (CDM, "SubscribedContent-338393Enabled", 0),
            (CDM, "SubscribedContent-353694Enabled", 0),
            (CDM, "SubscribedContent-353696Enabled", 0),
            (r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "Start_IrisRecommendations", 0),
        ],
    },
    TweakDef {
        name: "Bing web results in Start search",
        cat: "Junk",
        advice: "Junk - Start search stays local (apps, files, settings); restart Explorer to apply",
        values: &[
            (r"HKCU:\Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions", 1),
            (r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Search", "BingSearchEnabled", 0),
        ],
    },
    TweakDef {
        name: "Diagnostic data: lock to Required (minimum)",
        cat: "Junk",
        advice: "Junk - stops Windows raising telemetry again after updates; no feedback pop-ups",
        values: &[
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection", "AllowTelemetry", 1),
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection", "DoNotShowFeedbackNotifications", 1),
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection", "LimitDiagnosticLogCollection", 1),
        ],
    },
    TweakDef {
        name: "Activity history collection / upload",
        cat: "Junk",
        advice: "Junk - Timeline is gone in Windows 11; stops activity database writes and uploads",
        values: &[
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\System", "EnableActivityFeed", 0),
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\System", "PublishUserActivities", 0),
            (r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\System", "UploadUserActivities", 0),
        ],
    },
    TweakDef {
        name: "\"Finish setting up your device\" nag",
        cat: "Junk",
        advice: "Junk - stops the setup/upsell screen (UserOOBEBroker)",
        values: &[(r"HKCU:\Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement", "ScoobeSystemSettingEnabled", 0)],
    },
    TweakDef {
        name: "Delivery Optimization: share updates with LAN PCs",
        cat: "Junk",
        advice: "Junk - HTTP-only downloads (single PC, nothing to share with); Store / Game Pass downloads unaffected",
        values: &[(r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization", "DODownloadMode", 0)],
    },
    TweakDef {
        name: "Cross-device Resume (CrossDeviceResume.exe)",
        cat: "Optional",
        advice: "Optional - resume phone apps on the PC; sign out to apply",
        values: &[
            (r"HKCU:\Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration", "IsResumeAllowed", 0),
            (r"HKCU:\Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration", "IsOneDriveResumeAllowed", 0),
        ],
    },
    TweakDef {
        name: "Windows Spotlight on the lock screen",
        cat: "Optional",
        advice: "Optional - rotating Bing lock-screen pictures and tips; also pick \"Picture\" in Settings > Lock screen",
        values: &[
            (CDM, "RotatingLockScreenEnabled", 0),
            (CDM, "RotatingLockScreenOverlayEnabled", 0),
            (CDM, "SubscribedContent-338387Enabled", 0),
        ],
    },
    TweakDef {
        name: "Windows Spotlight on the desktop",
        cat: "Optional",
        advice: "Optional - rotating Bing wallpaper; also pick \"Picture\" in Settings > Personalization > Background",
        values: &[(r"HKCU:\Software\Policies\Microsoft\Windows\CloudContent", "DisableSpotlightCollectionOnDesktop", 1)],
    },
];

/// One alternation of the PS regex. A leading `\b` means "word boundary before"; everything else is a
/// literal, case-insensitive substring (the PS patterns use nothing else).
struct Rule {
    alts: &'static [&'static str],
    name_only: bool,
    cat: &'static str,
    advice: &'static str,
}

/// First matching rule wins. Matched against "name command location" (or just the name when name_only).
const RULES: &[Rule] = &[
    Rule { alts: &["MicrosoftEdgeUpdate", "EdgeUpdate"], name_only: false, cat: "Keep", advice: "Keep - also keeps WebView2 (used by many apps) patched" },
    Rule { alts: &["nv3dappshext"], name_only: false, cat: "Junk", advice: "Junk - NVIDIA \"Run with graphics processor\" menu; does nothing on a single-GPU PC" },
    Rule { alts: &["SecurityHealthSystray"], name_only: false, cat: "Optional", advice: "Optional - Windows Security tray icon only; Defender keeps running and still alerts" },
    Rule {
        alts: &[
            r"\bsteam", "xbox", "xbl", "gamingapp", "gamingservices", "gameinput", "easyanticheat", "battleye", "vanguard", "nvidia", "nvcpl",
            "nvcontainer", "nvdisplay", r"\amd\", "radeon", "realtek", "rtkaud", "logitech", "lghub", "logi_", "razer", "corsair", "steelseries",
            "securityhealth",
        ],
        name_only: false,
        cat: "Keep",
        advice: "Keep - gaming / hardware / security",
    },
    Rule {
        alts: &["onedrive", "MicrosoftEdgeAutoLaunch", "softlanding", "MSTeams", "TeamsTfwStartupTask", "MicrosoftOfficeHub", "WebViewHostStartupId", "copilot", "cortana"],
        name_only: false,
        cat: "Junk",
        advice: "Junk - safe to switch off",
    },
    Rule { alts: &[r"Windows Defender\shellext"], name_only: false, cat: "Optional", advice: "Optional - \"Scan with Microsoft Defender\" right-click item" },
    Rule { alts: &["update", "updater"], name_only: true, cat: "Review", advice: "Review - updater (most apps also update when you open them)" },
];

/// Right-click handler classes, in the PS order.
const SHELL_CLASSES: &[(&str, &str)] = &[
    ("*", "Right-click (files)"),
    ("AllFilesystemObjects", "Right-click (files)"),
    ("Directory", "Right-click (folders)"),
    (r"Directory\Background", "Right-click (background)"),
    ("Folder", "Right-click (folders)"),
    ("Drive", "Right-click (drives)"),
    ("exefile", "Right-click (.exe)"),
    ("lnkfile", "Right-click (shortcuts)"),
    ("DesktopBackground", "Right-click (desktop)"),
];

// ------------------------------------------------------------------ data types

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Run,
    StartupFolder,
    Task,
    StoreApp,
    ShellExt,
    Tweak,
}

/// Combo-box order (same as the PS tool).
const KINDS: [Kind; 6] = [Kind::Run, Kind::StartupFolder, Kind::Task, Kind::StoreApp, Kind::ShellExt, Kind::Tweak];

impl Kind {
    /// $KindNames
    fn label(self) -> &'static str {
        match self {
            Kind::Run => "Logon (Run key)",
            Kind::StartupFolder => "Logon (Startup folder)",
            Kind::Task => "Scheduled task",
            Kind::StoreApp => "Store app startup",
            Kind::ShellExt => "Explorer extension",
            Kind::Tweak => "Windows setting",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Kind::Run => "Run",
            Kind::StartupFolder => "StartupFolder",
            Kind::Task => "Task",
            Kind::StoreApp => "StoreApp",
            Kind::ShellExt => "ShellExt",
            Kind::Tweak => "Tweak",
        }
    }
}

/// Accept a JSON array, a single object (PowerShell unrolls 1-element arrays) or null.
fn one_or_many<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(vec![]),
        serde_json::Value::Array(a) => a
            .into_iter()
            .filter(|x| !x.is_null())
            .map(|x| serde_json::from_value(x).map_err(serde::de::Error::custom))
            .collect(),
        o => Ok(vec![serde_json::from_value(o).map_err(serde::de::Error::custom)?]),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct TweakVal {
    #[serde(rename = "P")]
    p: String,
    #[serde(rename = "N")]
    n: String,
    #[serde(rename = "V")]
    v: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Snap {
    #[serde(rename = "P")]
    p: String,
    #[serde(rename = "N")]
    n: String,
    #[serde(rename = "Exists", default)]
    exists: bool,
    #[serde(rename = "Data", default)]
    data: serde_json::Value,
    /// Registry value type and raw bytes (hex), so undo restores the exact value. Missing in older
    /// records and in the PS tool's history: those fall back to `data`.
    #[serde(rename = "VType", default, skip_serializing_if = "Option::is_none")]
    vtype: Option<u32>,
    #[serde(rename = "Raw", default, skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
}

/// Everything needed to toggle an entry. Same field names as the PS tool's `Loc` hashtables, so
/// its history.json can be read as-is. Registry paths are PowerShell style ("HKCU:\...").
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct Loc {
    #[serde(rename = "Approved", default, skip_serializing_if = "Option::is_none")]
    approved: Option<String>,
    #[serde(rename = "Value", default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(rename = "TaskPath", default, skip_serializing_if = "Option::is_none")]
    task_path: Option<String>,
    #[serde(rename = "TaskName", default, skip_serializing_if = "Option::is_none")]
    task_name: Option<String>,
    #[serde(rename = "TaskFull", default, skip_serializing_if = "Option::is_none")]
    task_full: Option<String>,
    #[serde(rename = "Pkg", default, skip_serializing_if = "Option::is_none")]
    pkg: Option<String>,
    #[serde(rename = "TaskId", default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(rename = "Clsid", default, skip_serializing_if = "Option::is_none")]
    clsid: Option<String>,
    #[serde(rename = "Values", default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
    values: Vec<TweakVal>,
    #[serde(rename = "Prev", default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
    prev: Vec<Snap>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    id: String,
    kind: Kind,
    name: String,
    location: String,
    command: String,
    publisher: String,
    enabled: bool,
    category: &'static str,
    advice: &'static str,
    loc: Loc,
}

/// A scheduled task of Windows itself (path under \Microsoft\). Many of these are protected.
fn is_windows_task(e: &Entry) -> bool {
    e.kind == Kind::Task && e.loc.task_path.as_deref().unwrap_or(&e.location).to_lowercase().starts_with(r"\microsoft\")
}

/// One history record (same shape as the PS tool's history.json).
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Hist {
    #[serde(rename = "Batch")]
    batch: u32,
    #[serde(rename = "Time", default)]
    time: String,
    #[serde(rename = "Kind")]
    kind: Kind,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Location", default)]
    location: String,
    #[serde(rename = "Loc", default)]
    loc: Loc,
    #[serde(rename = "From")]
    from: bool,
    #[serde(rename = "To")]
    to: bool,
    #[serde(rename = "Undone", default)]
    undone: bool,
}

// ------------------------------------------------------------------ small helpers

/// [Environment]::ExpandEnvironmentVariables - unknown variables are left as they are.
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('%') {
            let name = &after[..j];
            if !name.is_empty() {
                if let Ok(v) = std::env::var(name) {
                    out.push_str(&v);
                    rest = &after[j + 1..];
                    continue;
                }
            }
        }
        out.push('%');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn is_path_rooted(p: &str) -> bool {
    let b = p.as_bytes();
    matches!(b.first(), Some(b'\\') | Some(b'/')) || (b.len() >= 2 && b[1] == b':')
}

fn has_extension(p: &str) -> bool {
    let start = p.rfind(['\\', '/', ':']).map(|i| i + 1).unwrap_or(0);
    let leaf = &p[start..];
    matches!(leaf.rfind('.'), Some(i) if i + 1 < leaf.len())
}

/// `^(.+?\.(exe|dll|cmd|bat|lnk|ps1|vbs))(\s|,|$)` (case-insensitive) - returns group 1.
fn ext_prefix(c: &str) -> Option<String> {
    const EXTS: [&str; 7] = ["exe", "dll", "cmd", "bat", "lnk", "ps1", "vbs"];
    let lc = c.to_ascii_lowercase();
    for (i, _) in lc.match_indices('.') {
        if i == 0 {
            continue;
        }
        for e in EXTS {
            if lc[i + 1..].starts_with(e) {
                let end = i + 1 + e.len();
                let ok = match c[end..].chars().next() {
                    None => true,
                    Some(ch) => ch.is_whitespace() || ch == ',',
                };
                if ok {
                    return Some(c[..end].to_string());
                }
            }
        }
    }
    None
}

/// Get-ExePath: the file a command line points at, or None when it can't be told.
#[cfg(test)]
fn get_exe_path(cmd: &str) -> Option<String> {
    get_exe_path_sure(cmd).map(|(p, _)| p)
}

/// Get-ExePath plus whether the path is certain (quoted, a known extension, or resolved on disk).
/// `false` means it was guessed from the first word of an unquoted command, which cuts paths with
/// spaces short - such a path not existing proves nothing.
fn get_exe_path_sure(cmd: &str) -> Option<(String, bool)> {
    if cmd.is_empty() || cmd.starts_with('(') {
        return None;
    }
    let c = expand_env(cmd.trim());
    let (p, sure) = if let Some(rest) = c.strip_prefix('"') {
        match rest.find('"') {
            Some(end) if end > 0 => (rest[..end].to_string(), true),
            _ => (c.trim_matches('"').to_string(), true),
        }
    } else if let Some(m) = ext_prefix(&c) {
        (m, true)
    } else {
        let first = c.split_whitespace().next().unwrap_or("").to_string();
        let whole = first.len() == c.len();
        (first, whole)
    };
    if p.is_empty() {
        return None;
    }
    if !is_path_rooted(&p) {
        let leaf = if has_extension(&p) { p.clone() } else { format!("{p}.exe") };
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for dir in [format!(r"{root}\System32"), root.clone()] {
            let full = format!(r"{}\{}", dir.trim_end_matches('\\'), leaf);
            if Path::new(&full).exists() {
                return Some((full, true));
            }
        }
        return None; // bare command name we can't resolve - don't call it "missing"
    }
    Some((p, sure))
}

/// Case-insensitive match of one rule's alternations.
fn rule_match(hay: &str, alts: &[&str]) -> bool {
    let h = hay.to_lowercase();
    alts.iter().any(|a| {
        let (wb, lit) = match a.strip_prefix(r"\b") {
            Some(rest) => (true, rest),
            None => (false, *a),
        };
        let lit = lit.to_lowercase();
        if !wb {
            return h.contains(&lit);
        }
        h.match_indices(&lit)
            .any(|(i, _)| h[..i].chars().next_back().map_or(true, |c| !(c.is_alphanumeric() || c == '_')))
    })
}

/// Get-Advice
fn get_advice(kind: Kind, name: &str, command: &str, location: &str, task_full: Option<&str>) -> (&'static str, &'static str) {
    if kind == Kind::Task {
        let tf = task_full.unwrap_or("");
        if let Some((_, a)) = KEEP_TASKS.iter().find(|(k, _)| k.eq_ignore_ascii_case(tf)) {
            return ("Keep", a);
        }
        if JUNK_TASKS.iter().any(|k| k.eq_ignore_ascii_case(tf)) {
            return ("Junk", "Junk - Windows telemetry / tips / unused feature");
        }
        if let Some((_, a)) = OPTIONAL_TASKS.iter().find(|(k, _)| k.eq_ignore_ascii_case(tf)) {
            return ("Optional", a);
        }
        if tf.to_lowercase().starts_with(r"\microsoft\") {
            return ("Windows", "Windows task - leave unless you know it");
        }
    }
    let hay = format!("{name} {command} {location}");
    for r in RULES {
        let target = if r.name_only { name } else { hay.as_str() };
        if rule_match(target, r.alts) {
            return (r.cat, r.advice);
        }
    }
    if kind == Kind::ShellExt {
        return ("Review", "Review - third-party Explorer add-on; switch off if you never use its menu item / icon");
    }
    ("Review", "Review - starts with Windows; switch off if you do not need it at login")
}

fn new_entry(kind: Kind, name: &str, location: &str, command: &str, enabled: bool, loc: Loc, publisher: &str) -> Entry {
    let (category, advice) = get_advice(kind, name, command, location, loc.task_full.as_deref());
    Entry {
        id: format!("{}|{}|{}", kind.key(), location, name),
        kind,
        name: name.to_string(),
        location: location.to_string(),
        command: command.to_string(),
        publisher: publisher.to_string(),
        enabled,
        category,
        advice,
        loc,
    }
}

// ------------------------------------------------------------------ registry helpers

fn split_ps_path(p: &str) -> Option<(RegKey, &str)> {
    let (hive, rest) = p.split_once(":\\")?;
    let h = match hive.to_ascii_uppercase().as_str() {
        "HKCU" => HKEY_CURRENT_USER,
        "HKLM" => HKEY_LOCAL_MACHINE,
        _ => return None,
    };
    Some((RegKey::predef(h), rest))
}

fn open_read(p: &str) -> Option<RegKey> {
    let (root, sub) = split_ps_path(p)?;
    root.open_subkey_with_flags(sub, KEY_READ | KEY_WOW64_64KEY).ok()
}

/// Open for writing, creating the key when needed (New-Item -Force).
fn open_write(p: &str) -> Result<RegKey, String> {
    let (root, sub) = split_ps_path(p).ok_or_else(|| format!("bad registry path {p}"))?;
    root.create_subkey_with_flags(sub, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY).map(|(k, _)| k).map_err(|e| e.to_string())
}

/// Remove-ItemProperty -ErrorAction SilentlyContinue
fn remove_value_quiet(p: &str, name: &str) {
    if let Some((root, sub)) = split_ps_path(p) {
        if let Ok(k) = root.open_subkey_with_flags(sub, KEY_SET_VALUE | KEY_WOW64_64KEY) {
            let _ = k.delete_value(name);
        }
    }
}

/// How PowerShell would stringify RegistryKey.GetValue (REG_EXPAND_SZ expanded, DWORD as Int32).
fn value_string(v: &RegValue) -> Option<String> {
    match v.vtype {
        REG_DWORD | REG_DWORD_BIG_ENDIAN => u32::from_reg_value(v).ok().map(|x| (x as i32).to_string()),
        REG_QWORD => u64::from_reg_value(v).ok().map(|x| (x as i64).to_string()),
        REG_EXPAND_SZ => String::from_reg_value(v).ok().map(|s| expand_env(&s)),
        REG_SZ => String::from_reg_value(v).ok(),
        REG_MULTI_SZ => Vec::<String>::from_reg_value(v).ok().map(|x| x.join(" ")),
        _ => Some(v.bytes.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(" ")),
    }
}

fn value_json(v: &RegValue) -> serde_json::Value {
    match v.vtype {
        REG_DWORD | REG_DWORD_BIG_ENDIAN => u32::from_reg_value(v).map(|x| serde_json::json!(x as i32)).unwrap_or_default(),
        REG_QWORD => u64::from_reg_value(v).map(|x| serde_json::json!(x as i64)).unwrap_or_default(),
        _ => value_string(v).map(serde_json::Value::String).unwrap_or_default(),
    }
}

fn read_value(p: &str, name: &str) -> Option<RegValue> {
    open_read(p)?.get_raw_value(name).ok()
}

/// Get-ApprovedEnabled: first byte odd = disabled; key/value missing = enabled.
fn approved_enabled(approved: &str, name: &str) -> bool {
    match read_value(approved, name) {
        Some(v) if matches!(v.vtype, REG_BINARY | REG_NONE) && !v.bytes.is_empty() => v.bytes[0] & 1 == 0,
        _ => true,
    }
}

fn filetime_now() -> u64 {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() + 11_644_473_600) * 10_000_000 + (d.subsec_nanos() / 100) as u64
}

// ------------------------------------------------------------------ PowerShell helpers

/// ps_json with a list of input lines passed through a temp file (the command line would overflow otherwise).
/// The script sees them as `$__lines`.
fn ps_json_lines<T: DeserializeOwned>(script: &str, lines: &[String]) -> Result<Vec<T>, String> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("pctuner-startup-{}-{}.txt", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&path, lines.join("\r\n")).map_err(|e| format!("could not write a temporary file: {e}"))?;
    let s = format!(
        // [string] strips the PSPath/PSDrive note properties Get-Content attaches (they would end up in the JSON)
        "{PATH_REFRESH}$__lines = @(Get-Content -LiteralPath {} -Encoding UTF8 | Where-Object {{ $_ }} | ForEach-Object {{ [string]$_ }})\n{}",
        common::psq(&path.display().to_string()),
        script
    );
    let r = common::ps_json(&s);
    let _ = std::fs::remove_file(&path);
    r
}

#[derive(Deserialize)]
struct StoreRow {
    #[serde(rename = "Pfn", default)]
    pfn: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "TaskId", default)]
    task_id: String,
    #[serde(rename = "Def", default)]
    def: String,
    #[serde(rename = "Pub", default)]
    publisher: String,
}

#[derive(Deserialize)]
struct TaskRow {
    #[serde(rename = "P", default)]
    path: String,
    #[serde(rename = "N", default)]
    name: String,
    #[serde(rename = "S", default)]
    state: String,
    #[serde(rename = "C", default)]
    cmd: String,
}

#[derive(Deserialize)]
struct LnkRow {
    #[serde(rename = "P", default)]
    p: String,
    #[serde(rename = "T", default)]
    t: String,
    #[serde(rename = "A", default)]
    a: String,
}

#[derive(Deserialize)]
struct PubRow {
    #[serde(rename = "P", default)]
    p: String,
    #[serde(rename = "C", default)]
    c: Option<String>,
}

#[derive(Deserialize)]
struct TaskOpRow {
    #[serde(rename = "I")]
    i: usize,
    #[serde(rename = "E", default)]
    e: Option<String>,
}

const STORE_SCRIPT: &str = r#"
foreach ($pkg in Get-AppxPackage -ErrorAction SilentlyContinue) {
    if (-not $pkg.InstallLocation) { continue }
    $m = Join-Path $pkg.InstallLocation 'AppxManifest.xml'
    if (-not (Test-Path -LiteralPath $m)) { continue }
    try { $raw = [IO.File]::ReadAllText($m) } catch { continue }
    if ($raw -notmatch 'StartupTask') { continue }
    try { $x = [xml]$raw } catch { continue }
    $pubNode = $x.SelectSingleNode("//*[local-name()='Properties']/*[local-name()='PublisherDisplayName']")
    $pub = if ($pubNode) { $pubNode.InnerText } else { '' }
    foreach ($st in $x.SelectNodes("//*[local-name()='StartupTask']")) {
        $tid = $st.GetAttribute('TaskId')
        if (-not $tid) { continue }
        [pscustomobject]@{ Pfn = [string]$pkg.PackageFamilyName; Name = [string]$pkg.Name; TaskId = [string]$tid; Def = [string]$st.GetAttribute('Enabled'); Pub = [string]$pub }
    }
}
"#;

const TASKS_SCRIPT: &str = r#"
foreach ($t in Get-ScheduledTask -ErrorAction SilentlyContinue) {
    $a = @($t.Actions)[0]
    $cmd = if ($a -and $a.Execute) { "$($a.Execute) $($a.Arguments)".Trim() } else { '(COM handler)' }
    [pscustomobject]@{ P = [string]$t.TaskPath; N = [string]$t.TaskName; S = [string]$t.State; C = [string]$cmd }
}
"#;

const LNK_SCRIPT: &str = r#"
$sh = New-Object -ComObject WScript.Shell
foreach ($p in $__lines) {
    try { $l = $sh.CreateShortcut($p); [pscustomobject]@{ P = $p; T = [string]$l.TargetPath; A = [string]$l.Arguments } } catch { }
}
"#;

const PUB_SCRIPT: &str = r#"
foreach ($p in $__lines) {
    $c = ''
    try { $c = [string](Get-Item -LiteralPath $p -ErrorAction Stop).VersionInfo.CompanyName } catch { }
    [pscustomobject]@{ P = $p; C = $c }
}
"#;

const TASK_OP_SCRIPT: &str = r#"
foreach ($l in $__lines) {
    $f = $l -split "`t"
    $err = ''
    try {
        if ($f[1] -eq '1') { Enable-ScheduledTask -TaskPath $f[2] -TaskName $f[3] -ErrorAction Stop | Out-Null }
        else { Disable-ScheduledTask -TaskPath $f[2] -TaskName $f[3] -ErrorAction Stop | Out-Null }
    } catch { $err = $_.Exception.Message; if (-not $err) { $err = [string]$_ } }
    [pscustomobject]@{ I = [int]$f[0]; E = [string]$err }
}
"#;

pub const EXPLORER_SCRIPT: &str ="Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force\nStart-Sleep -Seconds 2\nif (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }";

// ------------------------------------------------------------------ scanners

pub struct ScanResult {
    entries: Vec<Entry>,
    warnings: Vec<String>,
}

/// Entries plus a flag "publisher still to be looked up".
type Raw = Vec<(Entry, bool)>;

fn scan_run(out: &mut Raw) {
    let sources = [
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run", "HKCU", "Run", "HKCU Run"),
        (HKEY_LOCAL_MACHINE, r"Software\Microsoft\Windows\CurrentVersion\Run", "HKLM", "Run", "HKLM Run"),
        (HKEY_LOCAL_MACHINE, r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run", "HKLM", "Run32", "HKLM Run (32-bit)"),
    ];
    for (hive, key, hname, appr, label) in sources {
        let Ok(k) = RegKey::predef(hive).open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY) else { continue };
        let approved = format!(r"{hname}:\{APPROVED_BASE}\{appr}");
        for (vn, val) in k.enum_values().filter_map(Result::ok) {
            if vn.is_empty() {
                continue;
            }
            let cmd = value_string(&val).unwrap_or_default();
            let loc = Loc { approved: Some(approved.clone()), value: Some(vn.clone()), ..Default::default() };
            out.push((new_entry(Kind::Run, &vn, label, &cmd, approved_enabled(&approved, &vn), loc, ""), true));
        }
    }
}

fn shell_folder(hive: winreg::HKEY, name: &str) -> Option<String> {
    let k = RegKey::predef(hive)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders", KEY_READ | KEY_WOW64_64KEY)
        .ok()?;
    let v = k.get_raw_value(name).ok()?;
    value_string(&v).filter(|s| !s.is_empty())
}

/// One file in a Startup folder. Shortcuts keep the bare path as their command (the .lnk resolver
/// below looks them up by it); any other file (.url, .ahk, .js, ...) is quoted, because the folder
/// path contains spaces ("Start Menu") and the file itself is what starts.
fn folder_entry(name: &str, full: &str, approved: &str, label: &str) -> Entry {
    let cmd = if name.to_ascii_lowercase().ends_with(".lnk") { full.to_string() } else { format!("\"{full}\"") };
    let loc = Loc { approved: Some(approved.to_string()), value: Some(name.to_string()), ..Default::default() };
    new_entry(Kind::StartupFolder, name, label, &cmd, approved_enabled(approved, name), loc, "")
}

fn scan_startup_folders(out: &mut Raw, warnings: &mut Vec<String>) {
    let user = shell_folder(HKEY_CURRENT_USER, "Startup").or_else(|| {
        std::env::var("APPDATA").ok().map(|a| format!(r"{a}\Microsoft\Windows\Start Menu\Programs\Startup"))
    });
    let common_dir = shell_folder(HKEY_LOCAL_MACHINE, "Common Startup").or_else(|| {
        std::env::var("ProgramData").ok().map(|a| format!(r"{a}\Microsoft\Windows\Start Menu\Programs\StartUp"))
    });
    let sources = [
        (user, format!(r"HKCU:\{APPROVED_BASE}\StartupFolder"), "Startup folder (you)"),
        (common_dir, format!(r"HKLM:\{APPROVED_BASE}\StartupFolder"), "Startup folder (all users)"),
    ];
    let start = out.len();
    let mut lnks: Vec<String> = vec![];
    for (dir, approved, label) in sources {
        let Some(dir) = dir else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for f in rd.filter_map(Result::ok) {
            if !f.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let name = f.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let full = f.path().display().to_string();
            if name.to_ascii_lowercase().ends_with(".lnk") {
                lnks.push(full.clone());
            }
            out.push((folder_entry(&name, &full, &approved, label), true));
        }
    }
    if lnks.is_empty() {
        return;
    }
    // Resolve shortcut targets in one PowerShell call, then recompute advice with the real command.
    match ps_json_lines::<LnkRow>(LNK_SCRIPT, &lnks) {
        Ok(rows) => {
            let map: HashMap<String, String> = rows
                .into_iter()
                .filter(|r| !r.t.is_empty())
                .map(|r| (r.p.to_lowercase(), format!("\"{}\" {}", r.t, r.a).trim().to_string()))
                .collect();
            for (e, _) in out[start..].iter_mut() {
                if let Some(cmd) = map.get(&e.command.to_lowercase()) {
                    let rebuilt = new_entry(e.kind, &e.name, &e.location, cmd, e.enabled, e.loc.clone(), "");
                    *e = rebuilt;
                }
            }
        }
        Err(err) => warnings.push(format!("Startup-folder shortcuts could not be resolved: {err}")),
    }
}

fn scan_store() -> Result<Vec<Entry>, String> {
    let rows: Vec<StoreRow> = common::ps_json(&format!("{PATH_REFRESH}{STORE_SCRIPT}"))?;
    let mut out = vec![];
    for r in rows {
        if r.task_id.is_empty() {
            continue;
        }
        let key = format!(r"{STORE_BASE}\{}\{}", r.pfn, r.task_id);
        let state = read_value(&key, "State").and_then(|v| value_string(&v));
        let enabled = match state {
            Some(s) => matches!(s.trim().parse::<i64>(), Ok(2) | Ok(4)),
            None => r.def.eq_ignore_ascii_case("true"),
        };
        let loc = Loc { pkg: Some(r.pfn.clone()), task_id: Some(r.task_id.clone()), ..Default::default() };
        let mut e = new_entry(Kind::StoreApp, &r.name, "Store app", &r.task_id, enabled, loc, &r.publisher);
        // PS used "StoreApp|Store app|<package>", which hid the 2nd startup task of a package. Include the task id.
        e.id = format!("StoreApp|Store app|{}|{}", r.name, r.task_id);
        out.push(e);
    }
    Ok(out)
}

fn scan_tasks() -> Result<Vec<Entry>, String> {
    let rows: Vec<TaskRow> = common::ps_json(&format!("{PATH_REFRESH}{TASKS_SCRIPT}"))?;
    Ok(rows
        .into_iter()
        .map(|t| {
            let full = format!("{}{}", t.path, t.name);
            let loc = Loc { task_path: Some(t.path.clone()), task_name: Some(t.name.clone()), task_full: Some(full), ..Default::default() };
            new_entry(Kind::Task, &t.name, &t.path, &t.cmd, t.state != "Disabled", loc, "")
        })
        .collect())
}

fn is_clsid(s: &str) -> bool {
    s.len() > 2 && s.starts_with('{') && s.ends_with('}') && s[1..s.len() - 1].chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Resolve-Clsid: InprocServer32 default value from HKCU classes, HKLM classes, WOW6432Node.
fn resolve_clsid(clsid: &str) -> String {
    let roots = [
        (HKEY_CURRENT_USER, r"Software\Classes\CLSID"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Classes\CLSID"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Classes\CLSID"),
    ];
    for (hive, base) in roots {
        if let Ok(k) = RegKey::predef(hive).open_subkey_with_flags(format!(r"{base}\{clsid}\InprocServer32"), KEY_READ | KEY_WOW64_64KEY) {
            let v = k.get_raw_value("").ok().and_then(|v| value_string(&v)).unwrap_or_default();
            return expand_env(&v);
        }
    }
    String::new()
}

/// `\\Windows\\(?!System32\\DriverStore\\)|^%SystemRoot%|^%windir%` - Windows' own handlers.
/// Vendor DLLs installed into the DriverStore (e.g. NVIDIA) are third-party and stay in the list.
fn is_windows_dll(dll: &str) -> bool {
    let l = dll.to_lowercase();
    if l.starts_with("%systemroot%") || l.starts_with("%windir%") {
        return true;
    }
    l.match_indices(r"\windows\").any(|(i, m)| !l[i + m.len()..].starts_with(r"system32\driverstore\"))
}

fn scan_shell_ext(out: &mut Raw) {
    let blocked: HashSet<String> = open_read(BLOCKED_KEY)
        .map(|k| k.enum_values().filter_map(Result::ok).map(|(n, _)| n.to_lowercase()).collect())
        .unwrap_or_default();
    let overlay = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers";
    let mut sources: Vec<(winreg::HKEY, String, &str)> =
        vec![(HKEY_LOCAL_MACHINE, overlay.to_string(), "Icon overlay"), (HKEY_CURRENT_USER, overlay.to_string(), "Icon overlay")];
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for (c, label) in SHELL_CLASSES {
            sources.push((hive, format!(r"SOFTWARE\Classes\{c}\shellex\ContextMenuHandlers"), label));
        }
    }
    let mut seen: Vec<Entry> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    for (hive, path, label) in sources {
        let Ok(parent) = RegKey::predef(hive).open_subkey_with_flags(&path, KEY_READ | KEY_WOW64_64KEY) else { continue };
        for sk in parent.enum_keys().filter_map(Result::ok) {
            let default = parent
                .open_subkey_with_flags(&sk, KEY_READ | KEY_WOW64_64KEY)
                .ok()
                .and_then(|k| k.get_raw_value("").ok())
                .and_then(|v| value_string(&v))
                .unwrap_or_default();
            let clsid = if default.starts_with('{') { default } else { sk.clone() };
            if !is_clsid(&clsid) {
                continue;
            }
            let dll = resolve_clsid(&clsid);
            if dll.is_empty() || is_windows_dll(&dll) {
                continue;
            }
            let key = clsid.to_uppercase();
            if let Some(&i) = index.get(&key) {
                if !seen[i].location.to_lowercase().contains(&label.to_lowercase()) {
                    seen[i].location.push_str(&format!(", {label}"));
                }
                continue;
            }
            let enabled = !blocked.contains(&clsid.to_lowercase());
            let mut e = new_entry(Kind::ShellExt, sk.trim(), label, &dll, enabled, Loc { clsid: Some(clsid.clone()), ..Default::default() }, "");
            e.id = format!("ShellExt|{key}");
            index.insert(key, seen.len());
            seen.push(e);
        }
    }
    out.extend(seen.into_iter().map(|e| (e, true)));
}

fn tweak_values(t: &TweakDef) -> Vec<TweakVal> {
    t.values.iter().map(|(p, n, v)| TweakVal { p: p.to_string(), n: n.to_string(), v: *v }).collect()
}

fn scan_tweaks(out: &mut Raw) {
    for t in TWEAKS {
        // "On" (Windows default) unless every value already matches the switched-off value
        let off = t.values.iter().all(|(p, n, v)| {
            read_value(p, n).and_then(|x| value_string(&x)).map_or(false, |cur| cur.eq_ignore_ascii_case(&v.to_string()))
        });
        let cmd = t
            .values
            .iter()
            .map(|(p, n, v)| format!(r"{}\{}={}", p.replacen(":\\", "\\", 1), n, v))
            .collect::<Vec<_>>()
            .join(" ; ");
        out.push((
            Entry {
                id: format!("Tweak|{}", t.name),
                kind: Kind::Tweak,
                name: t.name.to_string(),
                location: "Registry policy / setting".into(),
                command: cmd,
                publisher: "Windows".into(),
                enabled: !off,
                category: t.cat,
                advice: t.advice,
                loc: Loc { values: tweak_values(t), ..Default::default() },
            },
            false,
        ));
    }
}

/// Fill in publishers with ONE PowerShell call and apply the "file no longer exists" rule.
fn fill_publishers(raw: &mut Raw, warnings: &mut Vec<String>) {
    const BAD: [char; 6] = ['<', '>', '|', '"', '*', '?'];
    let mut paths: HashMap<String, String> = HashMap::new(); // lower -> original
    let mut exe_of: Vec<Option<(String, bool)>> = Vec::with_capacity(raw.len());
    for (e, need) in raw.iter() {
        let p = if *need { get_exe_path_sure(&e.command) } else { None };
        if let Some((p, _)) = &p {
            if !p.contains(BAD) && Path::new(p).is_file() {
                paths.entry(p.to_lowercase()).or_insert_with(|| p.clone());
            }
        }
        exe_of.push(p);
    }
    let mut company: HashMap<String, String> = HashMap::new();
    if !paths.is_empty() {
        let list: Vec<String> = paths.values().cloned().collect();
        match ps_json_lines::<PubRow>(PUB_SCRIPT, &list) {
            Ok(rows) => {
                for r in rows {
                    company.insert(r.p.to_lowercase(), r.c.unwrap_or_default());
                }
            }
            Err(err) => warnings.push(format!("Publishers could not be read: {err}")),
        }
    }
    for ((e, need), exe) in raw.iter_mut().zip(exe_of) {
        if !*need {
            continue;
        }
        e.publisher = match exe {
            None => String::new(),
            Some((p, _)) if p.contains(BAD) => String::new(),
            // a guessed path (first word of an unquoted command) not existing proves nothing
            Some((p, false)) if !Path::new(&p).is_file() => String::new(),
            Some((p, _)) if !Path::new(&p).is_file() => "(file missing)".into(),
            Some((p, _)) => company.get(&p.to_lowercase()).cloned().unwrap_or_default(),
        };
        if e.publisher == "(file missing)" && e.category != "Keep" && matches!(e.kind, Kind::Run | Kind::StartupFolder | Kind::Task) {
            if is_windows_task(e) {
                // e.g. \Microsoft\Windows\UpdateOrchestrator\USO_UxBroker -> MusNotification.exe on newer builds:
                // Windows protects these tasks (switching off fails even as admin) and they cannot run anyway
                e.category = "Windows";
                e.advice = "Windows task - its file no longer exists, so it cannot run; protected by Windows (harmless, leave it)";
            } else {
                e.category = "Junk";
                e.advice = "Junk - points to a file that no longer exists";
            }
        }
    }
}

/// Load-Entries: every scanner, in the PS order; first entry per Id wins.
fn scan() -> ScanResult {
    let mut warnings = vec![];
    let store_job = std::thread::spawn(scan_store);
    let task_job = std::thread::spawn(scan_tasks);

    let mut raw: Raw = vec![];
    scan_run(&mut raw);
    scan_startup_folders(&mut raw, &mut warnings);
    let mut shell: Raw = vec![];
    scan_shell_ext(&mut shell);
    let mut tweaks: Raw = vec![];
    scan_tweaks(&mut tweaks);

    match store_job.join() {
        Ok(Ok(v)) => raw.extend(v.into_iter().map(|e| (e, false))),
        Ok(Err(e)) => warnings.push(format!("Store app startup tasks could not be read: {e}")),
        Err(_) => warnings.push("Store app scan crashed".into()),
    }
    raw.extend(shell);
    raw.extend(tweaks);
    match task_job.join() {
        Ok(Ok(v)) => raw.extend(v.into_iter().map(|e| (e, true))),
        Ok(Err(e)) => warnings.push(format!("Scheduled tasks could not be read: {e}")),
        Err(_) => warnings.push("Scheduled task scan crashed".into()),
    }

    fill_publishers(&mut raw, &mut warnings);

    let mut ids = HashSet::new();
    let entries = raw.into_iter().map(|(e, _)| e).filter(|e| ids.insert(e.id.clone())).collect();
    ScanResult { entries, warnings }
}

// ------------------------------------------------------------------ state changes

/// What is stored at `p\n` right now, type and bytes included.
fn snap_value(p: &str, n: &str) -> Snap {
    let d = read_value(p, n);
    Snap {
        p: p.to_string(),
        n: n.to_string(),
        exists: d.is_some(),
        data: d.as_ref().map(value_json).unwrap_or_default(),
        vtype: d.as_ref().map(|x| x.vtype.clone() as u32),
        raw: d.as_ref().map(|x| x.bytes.iter().map(|b| format!("{b:02x}")).collect()),
    }
}

fn tweak_snapshot(values: &[TweakVal]) -> Vec<Snap> {
    values.iter().map(|v| snap_value(&v.p, &v.n)).collect()
}

fn reg_type(t: u32) -> Option<RegType> {
    [
        REG_NONE,
        REG_SZ,
        REG_EXPAND_SZ,
        REG_BINARY,
        REG_DWORD,
        REG_DWORD_BIG_ENDIAN,
        REG_LINK,
        REG_MULTI_SZ,
        REG_RESOURCE_LIST,
        REG_FULL_RESOURCE_DESCRIPTOR,
        REG_RESOURCE_REQUIREMENTS_LIST,
        REG_QWORD,
    ]
    .into_iter()
    .find(|x| x.clone() as u32 == t)
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Put one snapshot back exactly: the original type and bytes, or no value at all when there was none.
/// Records without type/bytes (older history, the PS tool's history) are restored as a DWORD.
fn restore_snap(s: &Snap) -> Result<(), String> {
    if !s.exists {
        remove_value_quiet(&s.p, &s.n);
        return Ok(());
    }
    if let (Some(t), Some(raw)) = (s.vtype, s.raw.as_deref()) {
        let vtype = reg_type(t).ok_or_else(|| format!("unknown registry type {t}"))?;
        let bytes = hex_bytes(raw).ok_or("damaged snapshot bytes")?;
        return open_write(&s.p)?.set_raw_value(&s.n, &RegValue { bytes, vtype }).map_err(|e| e.to_string());
    }
    let d = json_to_dword(&s.data)?;
    open_write(&s.p)?.set_value(&s.n, &d).map_err(|e| e.to_string())
}

fn find_snap<'a>(loc: &'a Loc, p: &str, n: &str) -> Option<&'a Snap> {
    loc.prev.iter().find(|s| s.p.eq_ignore_ascii_case(p) && s.n.eq_ignore_ascii_case(n))
}

/// The registry value a Run / Startup folder / Store app change writes (None for other kinds).
fn state_value(kind: Kind, loc: &Loc) -> Option<(String, String)> {
    match kind {
        Kind::Run | Kind::StartupFolder => Some((loc.approved.clone()?, loc.value.clone()?)),
        Kind::StoreApp => Some((format!(r"{STORE_BASE}\{}\{}", loc.pkg.as_deref()?, loc.task_id.as_deref()?), "State".into())),
        _ => None,
    }
}

fn json_to_dword(d: &serde_json::Value) -> Result<u32, String> {
    match d {
        serde_json::Value::Number(n) => n.as_i64().map(|x| x as u32).ok_or_else(|| format!("cannot restore {n} as a DWORD")),
        serde_json::Value::String(s) => s.trim().parse::<i64>().map(|x| x as u32).map_err(|_| format!("cannot restore '{s}' as a DWORD")),
        serde_json::Value::Bool(b) => Ok(*b as u32),
        other => Err(format!("cannot restore {other} as a DWORD")),
    }
}

/// Set-EntryEnabled for everything except scheduled tasks (those go through PowerShell in a batch).
/// `undo`: put back the value saved in `loc.prev` when the change was made (exact undo), if there is one.
fn set_native(kind: Kind, loc: &Loc, on: bool, undo: bool) -> Result<(), String> {
    if undo {
        if let Some((p, n)) = state_value(kind, loc) {
            if let Some(s) = find_snap(loc, &p, &n) {
                return restore_snap(s);
            }
        }
    }
    match kind {
        Kind::Run | Kind::StartupFolder => {
            let approved = loc.approved.as_deref().ok_or("missing StartupApproved key")?;
            let name = loc.value.as_deref().ok_or("missing value name")?;
            // Same 12-byte format Task Manager uses: 02 = enabled, 03 + timestamp = disabled
            let mut bytes = vec![0u8; 12];
            if on {
                bytes[0] = 2;
            } else {
                bytes[0] = 3;
                bytes[4..].copy_from_slice(&filetime_now().to_le_bytes());
            }
            open_write(approved)?.set_raw_value(name, &RegValue { bytes, vtype: REG_BINARY }).map_err(|e| e.to_string())
        }
        Kind::StoreApp => {
            let pkg = loc.pkg.as_deref().ok_or("missing package")?;
            let tid = loc.task_id.as_deref().ok_or("missing task id")?;
            let k = open_write(&format!(r"{STORE_BASE}\{pkg}\{tid}"))?;
            k.set_value("State", &(if on { 2u32 } else { 1u32 })).map_err(|e| e.to_string())
        }
        Kind::ShellExt => {
            let clsid = loc.clsid.as_deref().ok_or("missing CLSID")?;
            if on {
                remove_value_quiet(BLOCKED_KEY, clsid);
                Ok(())
            } else {
                open_write(BLOCKED_KEY)?.set_value(clsid, &String::new()).map_err(|e| e.to_string())
            }
        }
        Kind::Tweak => {
            let mut failed = vec![];
            for v in &loc.values {
                if on {
                    // Restore what was there before we switched it off; otherwise remove our value (= Windows default).
                    // Keep going on a failure so one bad value doesn't leave the rest switched off.
                    let r = match find_snap(loc, &v.p, &v.n) {
                        Some(s) => restore_snap(s),
                        None => {
                            remove_value_quiet(&v.p, &v.n);
                            Ok(())
                        }
                    };
                    if let Err(e) = r {
                        failed.push(format!(r"{}\{}: {e}", v.p, v.n));
                    }
                } else {
                    open_write(&v.p)?.set_value(&v.n, &(v.v as u32)).map_err(|e| e.to_string())?;
                }
            }
            if failed.is_empty() {
                Ok(())
            } else {
                Err(format!("{} of {} values could not be restored: {}", failed.len(), loc.values.len(), failed.join("; ")))
            }
        }
        Kind::Task => Err("scheduled tasks are changed through PowerShell".into()),
    }
}

struct Op {
    kind: Kind,
    name: String,
    loc: Loc,
    on: bool,
    /// A change from the main list: snapshot what is there first so undo can put it back exactly.
    /// Tweaks being switched off are also rolled back on failure.
    snapshot: bool,
    /// Undo of a history record: restore the snapshot in `loc.prev` when there is one.
    undo: bool,
}

/// Turn a raw error into the PS tool's friendlier wording.
fn explain(op: &Op, err: &str) -> String {
    let low = err.to_lowercase();
    if op.kind == Kind::Task
        && low.contains("access is denied")
        && op.loc.task_path.as_deref().unwrap_or("").to_lowercase().starts_with(r"\microsoft\")
    {
        return "protected by Windows, so it cannot be changed (this is normal and harmless)".into();
    }
    if op.kind == Kind::Tweak && (low.contains("unauthorized") || low.contains("denied")) {
        let mut why = "Windows' UCPD protection blocks this value from scripts. Set it with Group Policy instead (gpedit.msc).".to_string();
        if op.name.starts_with("Widgets") {
            why.push_str("\n    Computer Configuration > Administrative Templates > Windows Components > Widgets: 'Allow widgets' = Disabled and 'Disable widgets on the lock screen' = Enabled, then sign out.");
        }
        return why;
    }
    err.to_string()
}

/// Apply every op; returns the (possibly updated) Loc per op, or the reason it failed.
fn apply_ops(ops: &[Op]) -> Vec<Result<Loc, String>> {
    let mut results: Vec<Option<Result<Loc, String>>> = (0..ops.len()).map(|_| None).collect();
    let mut task_lines = vec![];
    for (i, op) in ops.iter().enumerate() {
        if op.kind == Kind::Task {
            let (p, n) = (op.loc.task_path.clone().unwrap_or_default(), op.loc.task_name.clone().unwrap_or_default());
            task_lines.push(format!("{}\t{}\t{}\t{}", i, if op.on { 1 } else { 0 }, p, n));
            continue;
        }
        let mut loc = op.loc.clone();
        if op.kind == Kind::Tweak && op.snapshot {
            loc.prev = tweak_snapshot(&loc.values); // so undo restores exactly
        }
        if op.snapshot {
            if let Some((p, n)) = state_value(op.kind, &loc) {
                // StartupApproved bytes / Store app State as they are now (or absent), for an exact undo
                loc.prev = vec![snap_value(&p, &n)];
            }
        }
        let r = match set_native(op.kind, &loc, op.on, op.undo) {
            Ok(()) => Ok(loc),
            Err(e) => {
                if op.kind == Kind::Tweak && op.snapshot && !op.on {
                    // Don't leave a half-applied setting behind: put back whatever was written before the failure
                    let _ = set_native(Kind::Tweak, &loc, true, true);
                }
                Err(explain(op, &e))
            }
        };
        results[i] = Some(r);
    }
    if !task_lines.is_empty() {
        match ps_json_lines::<TaskOpRow>(TASK_OP_SCRIPT, &task_lines) {
            Ok(rows) => {
                for r in rows {
                    if r.i < ops.len() {
                        let e = r.e.unwrap_or_default();
                        results[r.i] = Some(if e.is_empty() { Ok(ops[r.i].loc.clone()) } else { Err(explain(&ops[r.i], &e)) });
                    }
                }
            }
            Err(e) => {
                for l in &task_lines {
                    if let Some(i) = l.split('\t').next().and_then(|s| s.parse::<usize>().ok()) {
                        results[i] = Some(Err(e.clone()));
                    }
                }
            }
        }
    }
    results.into_iter().map(|r| r.unwrap_or_else(|| Err("no result".into()))).collect()
}

// ------------------------------------------------------------------ history

fn parse_ps_history(text: &str) -> Result<Vec<Hist>, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).map_err(|e| e.to_string())?;
    let items = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Null => vec![],
        o => vec![o],
    };
    let mut out = vec![];
    for (i, it) in items.into_iter().enumerate() {
        if it.is_null() {
            continue;
        }
        out.push(serde_json::from_value::<Hist>(it).map_err(|e| format!("record {}: {e}", i + 1))?);
    }
    Ok(out)
}

/// Write the history through a temp file, so a crash or power loss mid-write never leaves a
/// truncated file behind (rename replaces the old file in one step).
fn save_history_to(data: &Path, v: &[Hist]) -> Result<(), String> {
    use std::io::Write;
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    let target = data.join(HISTORY);
    let tmp = data.join(format!("{HISTORY}.tmp"));
    let r = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(|e| e.to_string())
}

/// Result of reading the history at start-up.
struct HistLoad {
    hist: Vec<Hist>,
    /// Shown as the last-action text (e.g. "Imported 12 changes ...").
    note: Option<String>,
    /// Shown as a warning banner (the file could not be read).
    warning: Option<String>,
    /// false: the file is there but could neither be read nor backed up - never overwrite it.
    writable: bool,
}

/// Load our history; on first run import the PowerShell tool's history (same record format).
fn load_history() -> HistLoad {
    let base = std::env::var_os("LOCALAPPDATA");
    load_history_in(&common::data_dir(), base.as_deref().map(Path::new))
}

/// `data`: our data folder; `ps_base`: the folder holding the PS tool's StartupCleaner\history.json.
fn load_history_in(data: &Path, ps_base: Option<&Path>) -> HistLoad {
    let own = data.join(HISTORY);
    match std::fs::read(&own) {
        Ok(bytes) => {
            let parsed = std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(parse_ps_history);
            match parsed {
                Ok(v) => return HistLoad { hist: v, note: None, warning: None, writable: true },
                Err(e) => {
                    // Keep the bytes: rename it out of the way (or copy it when that fails), never overwrite it.
                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    let backup = format!("startup-history.corrupt-{secs}.json");
                    let kept = std::fs::rename(&own, data.join(&backup)).is_ok() || std::fs::write(data.join(&backup), &bytes).is_ok();
                    if !kept {
                        let w = format!("Could not read {HISTORY} ({e}) and could not back it up, so it is left untouched. Changes made now are not saved and can only be undone until PC Tuner closes.");
                        return HistLoad { hist: vec![], note: None, warning: Some(w), writable: false };
                    }
                    // Never fall back to the PowerShell history here: its Undone flags are older than
                    // the damaged file, so changes already undone would be offered for undo again.
                    let w = format!("Could not read {HISTORY} ({e}); it was kept as {backup}. Earlier changes cannot be undone from this tab.");
                    return HistLoad { hist: vec![], note: None, warning: Some(w), writable: true };
                }
            }
        }
        // Import only on a true first run: a moved-aside damaged file means we already had our own history.
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound && has_corrupt_backup(data) => {
            return HistLoad { hist: vec![], note: None, warning: None, writable: true };
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            let w = format!("Could not read {HISTORY} ({e}), so it is left untouched. Changes made now are not saved and can only be undone until PC Tuner closes.");
            return HistLoad { hist: vec![], note: None, warning: Some(w), writable: false };
        }
    }
    let empty = || HistLoad { hist: vec![], note: None, warning: None, writable: true };
    let Some(base) = ps_base else { return empty() };
    let ps_file = base.join("StartupCleaner").join("history.json");
    let Ok(text) = std::fs::read_to_string(&ps_file) else { return empty() };
    match parse_ps_history(&text) {
        Ok(v) => {
            let n = v.len();
            let mut note = format!("Imported {n} change(s) from the PowerShell Startup Cleaner history.");
            if let Err(e) = save_history_to(data, &v) {
                note.push_str(&format!(" (could not save {HISTORY}: {e})"));
            }
            HistLoad { hist: v, note: Some(note), warning: None, writable: true }
        }
        Err(e) => HistLoad { hist: vec![], note: Some(format!("Could not import {}: {e}", ps_file.display())), warning: None, writable: true },
    }
}

/// A damaged history of ours was moved aside earlier (startup-history.corrupt-*).
fn has_corrupt_backup(data: &Path) -> bool {
    std::fs::read_dir(data)
        .map(|rd| rd.filter_map(Result::ok).any(|f| f.file_name().to_string_lossy().starts_with("startup-history.corrupt-")))
        .unwrap_or(false)
}

// ------------------------------------------------------------------ operation windows
//
// Every change (clean, switch on/off, undo, Explorer restart) is shown in the shared operation
// window: preview -> working -> per-item result.

/// Groups in the preview / result, in the order they are shown.
const GROUPS: [&str; 5] = ["Windows settings", "Scheduled tasks", "Startup programs", "Store app startup", "Explorer add-ons"];

fn group_label(k: Kind) -> &'static str {
    match k {
        Kind::Run | Kind::StartupFolder => "Startup programs",
        Kind::Task => "Scheduled tasks",
        Kind::StoreApp => "Store app startup",
        Kind::ShellExt => "Explorer add-ons",
        Kind::Tweak => "Windows settings",
    }
}

fn group_rank(k: Kind) -> usize {
    GROUPS.iter().position(|g| *g == group_label(k)).unwrap_or(GROUPS.len())
}

/// Small hint under a result row that worked.
fn ok_note(kind: Kind) -> String {
    if kind == Kind::ShellExt {
        "Applies after Explorer restarts".into()
    } else {
        String::new()
    }
}

const EXPLORER_EXTRA: &str = "Restart Explorer";
const SPOTLIGHT_NOTE: &str = "Spotlight: also choose 'Picture' in Settings > Personalization > Background / Lock screen.";

/// What the open operation window is doing.
enum What {
    Clean { spotlight: bool },
    Switch { on: bool },
    Undo { idx: Vec<usize> },
    Explorer,
    /// Sample data for screenshots (PCTUNER_DEMO) - never touches the PC.
    Demo,
}

struct OpShow {
    win: OpWindow,
    what: What,
    /// History batch written by this operation, for "Undo" in the result stage.
    batch: Option<u32>,
}

fn entry_item(e: &Entry) -> OpItem {
    OpItem::new(e.id.as_str(), e.name.as_str(), group_label(e.kind)).optional(e.category == "Optional")
}

/// Invoke-CleanJunk: everything marked junk (and optional, when asked) that is still switched on.
fn clean_op(entries: &[Entry], include_optional: bool) -> OpShow {
    let mut junk: Vec<&Entry> = entries.iter().filter(|e| e.enabled && (e.category == "Junk" || (include_optional && e.category == "Optional"))).collect();
    junk.sort_by_cached_key(|e| (group_rank(e.kind), e.category, e.name.to_lowercase()));
    let spotlight = junk.iter().any(|e| e.name.starts_with("Windows Spotlight"));
    let title = if include_optional { "Clean junk + optional" } else { "Clean junk" };
    let mut win = OpWindow::new(title, "clean", "cleaned", junk.iter().map(|e| entry_item(e)).collect());
    win.preview_text = "These get switched off. Nothing is deleted.".into();
    win.reassure = "Gaming and hardware items are never included. One click undoes the whole cleanup.".into();
    win.go_label = "Clean now".into();
    win.running_text = "Cleaning...".into();
    win.undo_label = "Undo this cleanup".into();
    win.empty_title = "Already clean".into();
    win.empty_text = "Everything on this list is already switched off.".into();
    // every item is ticked; untick any to leave it switched on (Go acts on the items left in the window)
    win.pickable = true;
    OpShow { win, what: What::Clean { spotlight }, batch: None }
}

/// Result-stage extras of a cleanup: what is left, what to do next.
fn clean_done(win: &mut OpWindow, junk_left: usize, spotlight: bool) {
    win.footer = Some((format!("Junk still switched on: {junk_left}"), if junk_left == 0 { theme::OK } else { theme::JUNK }));
    if win.ok_count() > 0 {
        win.notes.push("Sign out or restart Windows to finish.".into());
        if spotlight {
            win.notes.push(SPOTLIGHT_NOTE.into());
        }
    }
}

/// Invoke-Switch: the selected items that are not yet in the wanted state.
fn switch_op(sel: &[Entry], on: bool) -> OpShow {
    let mut work: Vec<&Entry> = sel.iter().filter(|e| e.enabled != on).collect();
    work.sort_by_cached_key(|e| (group_rank(e.kind), e.name.to_lowercase()));
    let (verb, done) = if on { ("switch on", "switched on") } else { ("switch off", "switched off") };
    let mut win = OpWindow::new(verb, verb, done, work.iter().map(|e| entry_item(e)).collect());
    win.preview_text = if on { "These get switched back on.".into() } else { "These get switched off. Nothing is deleted.".into() };
    win.reassure = "Every change goes into the history and can be undone.".into();
    win.go_label = if on { "Switch on".into() } else { "Switch off".into() };
    win.running_text = if on { "Switching on...".into() } else { "Switching off...".into() };
    if !on {
        let keep: Vec<&str> = work.iter().filter(|e| e.category == "Keep" || e.category == "Windows").map(|e| e.name.as_str()).collect();
        if !keep.is_empty() {
            const MAX: usize = 10;
            let mut names = keep.iter().take(MAX).copied().collect::<Vec<_>>().join("\n");
            if keep.len() > MAX {
                names.push_str(&format!("\n... and {} more", keep.len() - MAX));
            }
            win.warning = Some(format!("These are marked KEEP (gaming / hardware / security / Windows):\n\n{names}\n\nSwitch them off anyway?"));
        }
    }
    OpShow { win, what: What::Switch { on }, batch: None }
}

/// Undo-Entries: put history records back the way they were.
fn undo_op(history: &[Hist], idx: Vec<usize>, title: &str) -> OpShow {
    let mut idx: Vec<usize> = idx.into_iter().filter(|&i| i < history.len() && !history[i].undone).collect();
    idx.sort_by_cached_key(|&i| (group_rank(history[i].kind), history[i].name.to_lowercase()));
    let items = idx
        .iter()
        .map(|&i| {
            let h = &history[i];
            OpItem::new(format!("h{i}"), h.name.as_str(), group_label(h.kind)).detail(if h.from { "back on" } else { "back off" })
        })
        .collect();
    let mut win = OpWindow::new(title, "undo", "undone", items).noun("change", "changes");
    win.preview_text = "Each item goes back to the state it had before the change.".into();
    win.go_label = "Undo".into();
    win.running_text = "Undoing...".into();
    win.empty_title = "Nothing to undo".into();
    win.empty_text = "There are no active changes in the history.".into();
    OpShow { win, what: What::Undo { idx }, batch: None }
}

fn explorer_op() -> OpShow {
    let items = vec![OpItem::new("explorer", "Windows Explorer", "Explorer").detail("taskbar, desktop and folder windows")];
    let mut win = OpWindow::new("Restart Explorer", "restart", "restarted", items).noun("program", "programs");
    win.preview_text = "Needed for Explorer add-on changes to take effect.".into();
    win.warning = Some("The taskbar blinks and open folder windows close.".into());
    win.go_label = "Restart now".into();
    win.running_text = "Restarting Explorer...".into();
    OpShow { win, what: What::Explorer, batch: None }
}

/// Sample result for the demo window: one failure, notes on the Explorer add-ons.
fn demo_result(it: &OpItem) -> Option<Result<String, String>> {
    if it.name.starts_with("Widgets") {
        return Some(Err("Windows' UCPD protection blocks this value from scripts. Set it with Group Policy instead (gpedit.msc).".into()));
    }
    if it.group == group_label(Kind::ShellExt) {
        return Some(Ok(ok_note(Kind::ShellExt)));
    }
    None
}

fn demo_done(win: &mut OpWindow) {
    win.finish(demo_result);
    clean_done(win, 1, true);
    win.extras = vec![EXPLORER_EXTRA.into()];
    win.can_undo = true;
}

/// Sample window for PCTUNER_DEMO=startup:<stage> (screenshots only - nothing on the PC is touched).
fn demo_op(stage: Stage) -> OpShow {
    let mk = |name: &str, kind: Kind, optional: bool| OpItem::new(name, name, group_label(kind)).optional(optional);
    let mut items = vec![
        mk("Activity history collection / upload", Kind::Tweak, false),
        mk("Bing web results in Start search", Kind::Tweak, false),
        mk("Suggested apps, silent app installs and tips", Kind::Tweak, false),
        mk("Widgets (news feed board)", Kind::Tweak, false),
        mk("Windows Spotlight on the lock screen", Kind::Tweak, true),
    ];
    for t in ["Consolidator", "Device", "Device User", "DmClient", "MapsUpdateTask", "PcaPatchDbTask", "QueueReporting", "UsbCeip", "WinSAT"] {
        items.push(mk(t, Kind::Task, false));
    }
    items.push(mk("OneDrive", Kind::Run, false));
    items.push(mk("NvAppShExt", Kind::ShellExt, false));
    items.push(mk("OpenGLShExt", Kind::ShellExt, false));
    let mut op = clean_op(&[], true);
    op.what = What::Demo;
    op.win.items = items;
    op.win.stage = stage;
    if stage == Stage::Done {
        demo_done(&mut op.win);
    }
    op
}

// ------------------------------------------------------------------ the tab

enum Work {
    Rescan,
    Toggle { items: Vec<Entry>, on: bool, verb: String, skipped: usize, batch: u32 },
    Undo { idx: Vec<usize> },
}

struct WorkOut {
    results: Vec<Result<Loc, String>>,
    scan: ScanResult,
}

pub struct StartupTab {
    entries: Vec<Entry>,
    scanned: bool,
    job: Option<Job<WorkOut>>,
    work: Option<Work>,
    explorer_job: Option<Job<Result<String, String>>>,
    history: Vec<Hist>,
    selected: HashSet<String>,
    search: String,
    kind_filter: Option<Kind>,
    enabled_only: bool,
    show_windows: bool,
    last_action: String,
    warnings: Vec<String>,
    show_history: bool,
    hist_sel: HashSet<usize>,
    /// Shown inside the history window (e.g. "Nothing to undo.").
    hist_status: String,
    /// The history file could not be read or saved (warning banner).
    history_warning: Option<String>,
    /// false: the history file could not be read nor backed up, so it is never overwritten.
    history_writable: bool,
    /// The open operation window (preview -> working -> result), if any.
    op: Option<OpShow>,
}

impl StartupTab {
    pub fn new() -> Self {
        Self::from_load(load_history())
    }

    fn from_load(h: HistLoad) -> Self {
        let HistLoad { hist: history, note, warning, writable } = h;
        StartupTab {
            entries: vec![],
            scanned: false,
            job: None,
            work: None,
            explorer_job: None,
            history,
            selected: HashSet::new(),
            search: String::new(),
            kind_filter: None,
            enabled_only: false,
            show_windows: false,
            last_action: note.unwrap_or_default(),
            warnings: vec![],
            // screenshot hook for the history window (read-only): PCTUNER_DEMO=startup:history
            show_history: demo_history(),
            hist_sel: HashSet::new(),
            hist_status: String::new(),
            history_warning: warning,
            history_writable: writable,
            op: if demo_history() { None } else { opwin::demo_stage("startup").map(demo_op) },
        }
    }

    fn start(&mut self, work: Work, ops: Vec<Op>) {
        self.scanned = true;
        self.job = Some(Job::spawn(move || WorkOut { results: apply_ops(&ops), scan: scan() }));
        self.work = Some(work);
    }

    fn rescan(&mut self) {
        if !self.busy() {
            self.start(Work::Rescan, vec![]);
        }
    }

    /// Save the history; a failure is shown as a banner instead of being dropped.
    fn save_history(&mut self) {
        if !self.history_writable {
            return; // the unreadable file is kept as it is (the banner already says so)
        }
        if let Err(e) = save_history_to(&common::data_dir(), &self.history) {
            self.history_warning = Some(format!("Could not save {HISTORY} ({e}). The changes were made, but may not be undoable after PC Tuner closes."));
        }
    }

    fn next_batch(&self) -> u32 {
        self.history.iter().map(|h| h.batch).max().unwrap_or(0) + 1
    }

    fn by_ids(&self, ids: &[String]) -> Vec<Entry> {
        let set: HashSet<&String> = ids.iter().collect();
        self.entries.iter().filter(|e| set.contains(&e.id)).cloned().collect()
    }

    /// Active history records of one batch.
    fn batch_idx(&self, batch: u32) -> Vec<usize> {
        (0..self.history.len()).filter(|&i| !self.history[i].undone && self.history[i].batch == batch).collect()
    }

    /// Invoke-Toggle: switch a set of entries on/off as ONE history batch.
    fn toggle(&mut self, items: Vec<Entry>, on: bool, verb: &str) {
        let total = items.len();
        let work: Vec<Entry> = items.into_iter().filter(|e| e.enabled != on).collect();
        if work.is_empty() {
            self.last_action = format!("Nothing to change - already switched {}.", if on { "on" } else { "off" });
            return;
        }
        let ops = work
            .iter()
            .map(|e| {
                let mut loc = e.loc.clone();
                if e.kind == Kind::Tweak && on {
                    // Use the snapshot taken when this tool switched it off, if there is one.
                    if let Some(h) = self.history.iter().rev().find(|h| h.kind == Kind::Tweak && h.name == e.name && !h.to && !h.undone) {
                        loc.prev = h.loc.prev.clone();
                    }
                }
                let snapshot = matches!(e.kind, Kind::Run | Kind::StartupFolder | Kind::StoreApp) || (e.kind == Kind::Tweak && !on);
                Op { kind: e.kind, name: e.name.clone(), loc, on, snapshot, undo: false }
            })
            .collect();
        let skipped = total - work.len();
        let batch = self.next_batch();
        self.start(Work::Toggle { items: work, on, verb: verb.to_string(), skipped, batch }, ops);
    }

    /// Undo-Entries
    fn undo(&mut self, idx: Vec<usize>) {
        let mut idx: Vec<usize> = idx.into_iter().filter(|&i| i < self.history.len() && !self.history[i].undone).collect();
        if idx.is_empty() {
            self.last_action = "Nothing to undo.".into();
            return;
        }
        // newest first (Sort-Object Batch, Time -Descending)
        idx.sort_by(|&a, &b| {
            let (x, y) = (&self.history[a], &self.history[b]);
            (y.batch, &y.time, b).cmp(&(x.batch, &x.time, a))
        });
        let ops = idx
            .iter()
            .map(|&i| {
                let h = &self.history[i];
                Op { kind: h.kind, name: h.name.clone(), loc: h.loc.clone(), on: h.from, snapshot: false, undo: true }
            })
            .collect();
        self.start(Work::Undo { idx }, ops);
    }

    fn finish(&mut self, out: WorkOut) {
        let work = self.work.take();
        self.entries = out.scan.entries;
        self.warnings = out.scan.warnings;
        let ids: HashSet<&String> = self.entries.iter().map(|e| &e.id).collect();
        self.selected.retain(|s| ids.contains(s));
        let junk_left = self.entries.iter().filter(|e| e.enabled && e.category == "Junk").count();
        // Per-item results for the operation window, keyed by item id.
        let mut per_item: HashMap<String, Result<String, String>> = HashMap::new();
        let mut explorer = false;
        let mut ok = 0;
        let mut new_batch = None;
        match work {
            None | Some(Work::Rescan) => return,
            Some(Work::Toggle { items, on, verb, skipped, batch }) => {
                let mut failed = 0;
                let time = common::now();
                for (e, r) in items.iter().zip(out.results) {
                    match r {
                        Ok(loc) => {
                            self.history.push(Hist {
                                batch,
                                time: time.clone(),
                                kind: e.kind,
                                name: e.name.clone(),
                                location: e.location.clone(),
                                loc,
                                from: e.enabled,
                                to: on,
                                undone: false,
                            });
                            explorer |= e.kind == Kind::ShellExt;
                            ok += 1;
                            per_item.insert(e.id.clone(), Ok(ok_note(e.kind)));
                        }
                        Err(why) => {
                            failed += 1;
                            per_item.insert(e.id.clone(), Err(why));
                        }
                    }
                }
                self.save_history();
                let mut summary = format!("{ok} {verb}");
                if skipped > 0 {
                    summary.push_str(&format!(", {skipped} already in that state"));
                }
                if failed > 0 {
                    summary.push_str(&format!(", {failed} failed"));
                }
                self.last_action = format!("{summary}.");
                if ok > 0 {
                    new_batch = Some(batch);
                }
            }
            Some(Work::Undo { idx }) => {
                let mut failed = 0;
                for (i, r) in idx.into_iter().zip(out.results) {
                    match r {
                        Ok(_) => {
                            self.history[i].undone = true;
                            ok += 1;
                            explorer |= self.history[i].kind == Kind::ShellExt;
                            per_item.insert(format!("h{i}"), Ok(ok_note(self.history[i].kind)));
                        }
                        Err(why) => {
                            failed += 1;
                            per_item.insert(format!("h{i}"), Err(why));
                        }
                    }
                }
                self.save_history();
                self.last_action = format!("Undid {ok} change(s).");
                if failed > 0 {
                    self.last_action.push_str(&format!(" {failed} could not be undone."));
                }
            }
        }
        // Report in the operation window that started the work.
        if let Some(op) = self.op.as_mut() {
            op.win.finish(|it| per_item.remove(&it.id));
            op.batch = new_batch;
            op.win.can_undo = new_batch.is_some();
            if explorer {
                op.win.extras = vec![EXPLORER_EXTRA.into()];
            }
            if let What::Clean { spotlight } = op.what {
                clean_done(&mut op.win, junk_left, spotlight);
            }
        }
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                (q.is_empty()
                    || [&e.name, &e.command, &e.publisher, &e.location].iter().any(|s| s.to_lowercase().contains(&q)))
                    && self.kind_filter.map_or(true, |k| e.kind == k)
                    && (!self.enabled_only || e.enabled)
                    && (self.show_windows || e.category != "Windows")
            })
            .map(|(i, _)| i)
            .collect();
        v.sort_by_cached_key(|&i| (self.entries[i].kind.label().to_lowercase(), self.entries[i].name.to_lowercase()));
        v
    }

    /// Invoke-Switch: opens the preview (with the KEEP warning when needed).
    fn switch(&mut self, sel: Vec<Entry>, on: bool) {
        if sel.is_empty() {
            self.last_action = "Select one or more items first.".into();
            return;
        }
        let op = switch_op(&sel, on);
        if op.win.items.is_empty() {
            self.last_action = format!("Nothing to change - already switched {}.", if on { "on" } else { "off" });
            return;
        }
        self.op = Some(op);
    }

    /// Undo-LastBatch: opens the preview of the newest active batch.
    fn undo_last(&mut self) {
        let Some(last) = self.history.iter().filter(|h| !h.undone).map(|h| h.batch).max() else {
            self.last_action = "Nothing to undo.".into();
            return;
        };
        self.op = Some(undo_op(&self.history, self.batch_idx(last), "Undo last change"));
    }

    fn restart_explorer(&mut self) {
        if self.explorer_job.is_none() {
            self.last_action = "Restarting Explorer...".into();
            self.explorer_job = Some(Job::spawn(|| common::ps(EXPLORER_SCRIPT)));
        }
    }

    /// Draw the operation window and act on its buttons.
    fn op_window(&mut self, ctx: &egui::Context) {
        let busy = self.busy();
        let Some(op) = self.op.as_mut() else { return };
        // Restarting Explorer never needed admin; everything else does.
        let can = !busy && (common::is_admin() || matches!(op.what, What::Explorer));
        let Some(ev) = op.win.show(ctx, can) else { return };
        if matches!(op.what, What::Demo) {
            // sample window: play the result, change nothing
            match ev {
                OpEvent::Go => demo_done(&mut op.win),
                _ => self.op = None,
            }
            return;
        }
        match ev {
            OpEvent::Close => self.op = None,
            OpEvent::Go => {
                if !can {
                    op.win.stage = Stage::Preview;
                    return;
                }
                let ids: Vec<String> = op.win.items.iter().map(|i| i.id.clone()).collect();
                match &op.what {
                    What::Clean { .. } => {
                        let items = self.by_ids(&ids);
                        self.toggle(items, false, "items switched off");
                    }
                    What::Switch { on } => {
                        let on = *on;
                        let items = self.by_ids(&ids);
                        self.toggle(items, on, if on { "switched on" } else { "switched off" });
                    }
                    What::Undo { idx } => {
                        let idx = idx.clone();
                        self.undo(idx);
                    }
                    What::Explorer => self.restart_explorer(),
                    What::Demo => {}
                }
                if !self.busy() {
                    // everything was already in that state: show the "nothing to do" result
                    if let Some(op) = self.op.as_mut() {
                        op.win.items.clear();
                        op.win.done();
                    }
                }
            }
            OpEvent::Extra(_) => {
                op.win.extras.clear();
                self.restart_explorer();
            }
            OpEvent::Undo => {
                let batch = op.batch;
                self.op = None;
                let idx = batch.map(|b| self.batch_idx(b)).unwrap_or_default();
                if idx.is_empty() {
                    return;
                }
                // The user already asked for it in the result stage: go straight to work.
                let mut u = undo_op(&self.history, idx.clone(), "Undo");
                u.win.stage = Stage::Running;
                self.op = Some(u);
                self.undo(idx);
                if self.job.is_none() {
                    self.op = None;
                }
            }
        }
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        // hidden while an operation window is open (it comes back afterwards)
        if !self.show_history || self.op.is_some() {
            return;
        }
        enum H {
            UndoSel,
            UndoAll,
            Clear,
        }
        let mut act: Option<H> = None;
        let mut close = false;
        let busy = self.busy();
        let active = self.history.iter().filter(|h| !h.undone).count();
        let any_active = active > 0;
        let mut clicked: Option<usize> = None;
        theme::dim_backdrop(ctx, "startup-history");
        egui::Window::new("startup-history-window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width((ctx.screen_rect().width() - 80.0).min(930.0));
                theme::section(ui, "Startup Cleaner");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Change history").size(26.0).strong().color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::chip(ui, &format!("{active} can be undone"), if any_active { theme::ACCENT } else { theme::DIM });
                        theme::chip(ui, &format!("{} recorded", self.history.len()), theme::DIM);
                    });
                });
                ui.label(RichText::new("Click the changes you want to take back, newest first.").color(theme::TEXT_DIM));
                ui.add_space(6.0);
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if self.history.is_empty() {
                        ui.label(RichText::new("No changes recorded yet.").color(theme::TEXT_DIM));
                        return;
                    }
                    let order: Vec<usize> = (0..self.history.len()).rev().collect();
                    let hist = &self.history;
                    let sel = &self.hist_sel;
                    let max_h = (ctx.screen_rect().height() - 330.0).clamp(140.0, 360.0);
                    TableBuilder::new(ui)
                        .id_salt("startup-history-table")
                        .striped(true)
                        .resizable(false)
                        .sense(egui::Sense::click())
                        .max_scroll_height(max_h)
                        .auto_shrink([false, true])
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::exact(156.0))
                        .column(Column::exact(204.0).clip(true))
                        .column(Column::exact(130.0).clip(true))
                        .column(Column::exact(172.0).clip(true))
                        .column(Column::exact(96.0))
                        .column(Column::remainder().at_least(56.0))
                        .header(24.0, |mut h| {
                            for t in ["Time", "Name", "Type", "Location", "Change", "State"] {
                                h.col(|ui| head(ui, t));
                            }
                        })
                        .body(|body| {
                            body.rows(24.0, order.len(), |mut row| {
                                let i = order[row.index()];
                                let h = &hist[i];
                                row.set_selected(!h.undone && sel.contains(&i));
                                let (c, c2) = if h.undone { (theme::DIM, theme::DIM) } else { (theme::TEXT, theme::TEXT_DIM) };
                                row.col(|ui| cell(ui, &h.time, c2));
                                row.col(|ui| cell(ui, &h.name, c));
                                row.col(|ui| cell(ui, h.kind.label(), c2));
                                row.col(|ui| cell(ui, &h.location, c2));
                                row.col(|ui| cell(ui, if h.to { "Switched on" } else { "Switched off" }, c2));
                                row.col(|ui| cell(ui, if h.undone { "Undone" } else { "Active" }, if h.undone { theme::DIM } else { theme::ACCENT }));
                                if row.response().clicked() {
                                    clicked = Some(i);
                                }
                            });
                        });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    // only active rows count (undone ones can't be selected, this is a safety net)
                    let n = self.hist_sel.iter().filter(|&&i| self.history.get(i).is_some_and(|h| !h.undone)).count();
                    let label = if n > 0 { format!("   Undo selected ({n})   ") } else { "   Undo selected   ".to_string() };
                    if ui.add_enabled(!busy && n > 0, theme::primary_button(&label)).clicked() {
                        act = Some(H::UndoSel);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Clear undone entries"))
                        .on_hover_text("Removes already-undone rows from this list. Changes nothing on the PC.")
                        .clicked()
                    {
                        act = Some(H::Clear);
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    if busy {
                        ui.spinner();
                    } else if !self.hist_status.is_empty() {
                        ui.label(RichText::new(&self.hist_status).color(theme::TEXT_DIM));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!busy && any_active, theme::danger_button("Undo everything"))
                            .on_hover_text("Takes back every change still active in this list. You see the full list before anything happens.")
                            .clicked()
                        {
                            act = Some(H::UndoAll);
                        }
                    });
                });
            });
        if let Some(i) = clicked {
            self.hist_status.clear();
            hist_click(&self.history, &mut self.hist_sel, i);
        }
        if close {
            self.show_history = false;
            self.hist_status.clear();
        }
        if act.is_some() {
            self.hist_status.clear();
        }
        match act {
            Some(H::UndoSel) => {
                let idx: Vec<usize> = self.hist_sel.drain().collect();
                let op = undo_op(&self.history, idx, "Undo selected changes");
                if op.win.items.is_empty() {
                    // shown in this window (the main page is behind the backdrop)
                    self.hist_status = "Nothing to undo - the selected changes were already undone.".into();
                } else {
                    self.op = Some(op);
                }
            }
            Some(H::UndoAll) => {
                let idx: Vec<usize> = (0..self.history.len()).filter(|&i| !self.history[i].undone).collect();
                if !idx.is_empty() {
                    self.op = Some(undo_op(&self.history, idx, "Undo everything"));
                }
            }
            Some(H::Clear) => {
                self.history.retain(|h| !h.undone);
                self.hist_sel.clear();
                self.save_history();
            }
            None => {}
        }
    }
}

/// A click on history row `i`: active rows toggle; undone rows can't be selected (a stale selection is dropped).
fn hist_click(history: &[Hist], sel: &mut HashSet<usize>, i: usize) {
    if history.get(i).is_some_and(|h| !h.undone) {
        if !sel.remove(&i) {
            sel.insert(i);
        }
    } else {
        sel.remove(&i);
    }
}

fn demo_history() -> bool {
    std::env::var("PCTUNER_DEMO").is_ok_and(|v| v.eq_ignore_ascii_case("startup:history"))
}

fn head(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).strong().color(theme::TEXT_DIM));
}

fn cell(ui: &mut egui::Ui, text: &str, c: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(c)).truncate().selectable(false));
}

impl Tab for StartupTab {
    fn title(&self) -> &'static str {
        "Startup Cleaner"
    }

    /// A change or an Explorer restart is running: closing PC Tuner waits for it.
    fn busy(&self) -> bool {
        self.job.is_some() || self.explorer_job.is_some()
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.scanned && self.job.is_none() {
            self.rescan();
        }
        if let Some(out) = self.job.as_ref().and_then(|j| j.poll()) {
            self.job = None;
            self.finish(out);
        }
        if let Some(r) = self.explorer_job.as_ref().and_then(|j| j.poll()) {
            self.explorer_job = None;
            self.last_action = match &r {
                Ok(_) => "Explorer restarted.".into(),
                Err(e) => format!("Explorer restart failed: {e}"),
            };
            if let Some(op) = self.op.as_mut() {
                if matches!(op.what, What::Explorer) {
                    op.win.finish(|_| Some(r.clone().map(|_| String::new())));
                } else if op.win.stage == Stage::Done {
                    op.win.notes.push(self.last_action.clone());
                }
            }
        }

        self.history_window(&ctx);
        self.op_window(&ctx);

        let busy = self.busy();
        let ready =self.scanned && !(self.job.is_some() && self.entries.is_empty());

        // header: title on the left, page-level tools on the right
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui.add_enabled(!busy, egui::Button::new("Restart Explorer")).on_hover_text("Needed for Explorer add-on changes to take effect. Shows a preview first.").clicked() {
                self.op = Some(explorer_op());
            }
            if ui.button("History...").on_hover_text("Every change this tool made, with undo.").clicked() {
                self.show_history = true;
            }
            if ui.add_enabled(!busy, egui::Button::new("Rescan")).clicked() {
                self.rescan();
            }
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                theme::page_header(ui, "Startup Cleaner", "Everything that starts with Windows. Items are switched off, never deleted - every change can be undone.");
            });
        });
        common::admin_banner(ui);

        // hero: junk still on
        let shown: Vec<&Entry> = self.entries.iter().filter(|e| e.category != "Windows").collect();
        let on = shown.iter().filter(|e| e.enabled).count();
        let junk = self.entries.iter().filter(|e| e.enabled && e.category == "Junk").count();
        let optional = self.entries.iter().filter(|e| e.enabled && e.category == "Optional").count();
        let pending = self.history.iter().filter(|h| !h.undone).count();
        let mut clean: Option<bool> = None;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let wide = ui.available_width() >= 900.0;
            let mut buttons = |ui: &mut egui::Ui, rtl: bool| {
                let enabled = ready && !busy;
                // right-to-left layouts place the first widget rightmost: keep the primary button on the left
                for include_optional in if rtl { [true, false] } else { [false, true] } {
                    let (b, tip) = if include_optional {
                        (
                            egui::Button::new("Clean junk + optional").min_size(egui::vec2(0.0, 32.0)),
                            "Junk plus optional items - each optional one costs a small convenience; see the Advice column.",
                        )
                    } else {
                        (theme::primary_button("   Clean junk   "), "Switches off everything marked junk. Shows a preview first. Never touches gaming items.")
                    };
                    if ui.add_enabled(enabled, b).on_hover_text(tip).clicked() {
                        clean = Some(include_optional);
                    }
                }
            };
            ui.horizontal(|ui| {
                if ready {
                    let n = theme::count_up(&ctx, "startup-junk", junk as f32).round() as usize;
                    let (c, caption) =
                        if junk == 0 { (theme::ACCENT, "Nothing left to clean.") } else { (theme::JUNK, "Safe to switch off - nothing is deleted.") };
                    theme::hero(ui, &n.to_string(), "junk still on", caption, c);
                } else {
                    theme::hero(ui, "-", "junk still on", "Scanning startup locations...", theme::TEXT_FAINT);
                }
                ui.add_space(26.0);
                let val = |n: usize| if ready { n.to_string() } else { "-".to_string() };
                theme::stat(ui, &val(on), "items switched on", theme::TEXT);
                ui.add_space(16.0);
                theme::stat(ui, &val(optional), "optional still on", if optional == 0 || !ready { theme::TEXT } else { theme::OPTIONAL });
                ui.add_space(16.0);
                theme::stat(ui, &pending.to_string(), "changes can be undone", theme::TEXT);
                if wide {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| buttons(ui, true));
                }
            });
            if !wide {
                ui.horizontal(|ui| buttons(ui, false));
            }
        });
        if let Some(include_optional) = clean {
            self.op = Some(clean_op(&self.entries, include_optional));
        }
        for w in self.history_warning.iter().chain(&self.warnings) {
            theme::banner(ui, theme::LOCKED, w);
        }

        // list
        let visible = self.visible();
        let sel: Vec<Entry> = visible.iter().map(|&i| &self.entries[i]).filter(|e| self.selected.contains(&e.id)).cloned().collect();
        let mut clicked: Option<String> = None;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search name, publisher, command").desired_width(230.0));
                egui::ComboBox::from_id_salt("startup-kind")
                    .width(170.0)
                    .selected_text(self.kind_filter.map_or("All types", |k| k.label()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.kind_filter, None, "All types");
                        for k in KINDS {
                            ui.selectable_value(&mut self.kind_filter, Some(k), k.label());
                        }
                    });
                ui.checkbox(&mut self.enabled_only, "Enabled only");
                ui.checkbox(&mut self.show_windows, "Show all Windows tasks");
                if busy {
                    ui.spinner();
                    ui.label(RichText::new(if self.job.is_some() { "Working / scanning..." } else { "Restarting Explorer..." }).color(theme::TEXT_DIM));
                } else if !self.last_action.is_empty() {
                    ui.label(RichText::new(&self.last_action).color(theme::TEXT_DIM));
                }
            });
            ui.horizontal_wrapped(|ui| {
                let has_sel = !sel.is_empty() && !busy;
                if ui.add_enabled(has_sel, egui::Button::new("Switch off")).on_hover_text("Switch off the selected items (nothing is deleted). Shows a preview first.").clicked() {
                    self.switch(sel.clone(), false);
                }
                if ui.add_enabled(has_sel, egui::Button::new("Switch on")).on_hover_text("Switch the selected items back on. Shows a preview first.").clicked() {
                    self.switch(sel.clone(), true);
                }
                if ui.add_enabled(!busy && pending > 0, egui::Button::new("Undo last change")).on_hover_text("Shows what goes back before anything changes.").clicked() {
                    self.undo_last();
                }
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{} selected", sel.len())).color(theme::TEXT_DIM));
                if ui.small_button("Select all shown").clicked() {
                    for &i in &visible {
                        self.selected.insert(self.entries[i].id.clone());
                    }
                }
                if ui.small_button("Clear selection").clicked() {
                    self.selected.clear();
                }
                ui.add_space(6.0);
                theme::chip(ui, "Junk", theme::JUNK);
                theme::chip(ui, "Optional", theme::OPTIONAL);
                theme::chip(ui, "Keep", theme::GAMING);
                theme::chip(ui, "Switched off", theme::DIM);
            });
            ui.add_space(2.0);

            let entries = &self.entries;
            let selected = &self.selected;
            TableBuilder::new(ui)
                .id_salt("startup-table")
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .min_scrolled_height(0.0)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(34.0))
                .column(Column::initial(180.0).at_least(60.0).clip(true))
                .column(Column::initial(120.0).at_least(50.0).clip(true))
                .column(Column::initial(115.0).at_least(50.0).clip(true))
                .column(Column::initial(105.0).at_least(50.0).clip(true))
                .column(Column::initial(230.0).at_least(60.0).clip(true))
                .column(Column::remainder().at_least(60.0).clip(true))
                .header(24.0, |mut h| {
                    for t in ["On", "Name", "Type", "Location", "Publisher", "Advice", "Command"] {
                        h.col(|ui| head(ui, t));
                    }
                })
                .body(|body| {
                    body.rows(22.0, visible.len(), |mut row| {
                        let e = &entries[visible[row.index()]];
                        row.set_selected(selected.contains(&e.id));
                        // name / advice carry the category colour; the rest stays quiet
                        let (c, c2) = if !e.enabled {
                            (theme::DIM, theme::TEXT_FAINT)
                        } else {
                            let c = match e.category {
                                "Junk" => theme::JUNK,
                                "Optional" => theme::OPTIONAL,
                                "Keep" => theme::GAMING,
                                _ => theme::TEXT,
                            };
                            (c, theme::TEXT_DIM)
                        };
                        row.col(|ui| cell(ui, if e.enabled { "On" } else { "Off" }, c));
                        row.col(|ui| cell(ui, &e.name, c));
                        row.col(|ui| cell(ui, e.kind.label(), c2));
                        row.col(|ui| cell(ui, &e.location, c2));
                        row.col(|ui| cell(ui, &e.publisher, c2));
                        row.col(|ui| cell(ui, e.advice, c));
                        row.col(|ui| cell(ui, &e.command, c2));
                        if row.response().clicked() {
                            clicked = Some(e.id.clone());
                        }
                    });
                });
        });
        if let Some(id) = clicked {
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
        }
    }
}

// ------------------------------------------------------------------ tests (read-only)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_cleanup_reveal_and_groups() {
        assert_eq!(opwin::revealed(24, 0.0), 1); // first row shows at once
        assert_eq!(opwin::revealed(24, 60.0), 24); // never past the end
        let demo = demo_op(Stage::Done);
        let groups = opwin::group_counts(&demo.win.items);
        println!("{groups:?}");
        assert_eq!(groups[0], ("Windows settings".to_string(), 5, 1)); // Widgets is the sample failure
        assert_eq!(groups[1], ("Scheduled tasks".to_string(), 9, 0));
        assert_eq!(groups.iter().map(|g| g.1).sum::<usize>(), demo.win.items.len());
        assert_eq!(demo.win.stage, Stage::Done);
        assert_eq!(demo.win.ok_count(), demo.win.items.len() - 1);
        assert_eq!(demo.win.extras, vec![EXPLORER_EXTRA.to_string()]);
        assert!(demo.win.can_undo && demo.win.footer.is_some());
        assert!(demo.win.notes.iter().any(|n| n == SPOTLIGHT_NOTE) && demo.win.notes[0] == "Sign out or restart Windows to finish.");
        assert!(demo.win.items.iter().any(|i| matches!(&i.result, Some(Ok(n)) if !n.is_empty()))); // a note
        let pre = demo_op(Stage::Preview);
        assert_eq!(pre.win.stage, Stage::Preview);
        assert!(pre.win.items.iter().all(|i| i.result.is_none()));
    }

    /// The operation windows are built from the right items, in the fixed group order. Builds only - changes nothing.
    #[test]
    fn startup_op_windows() {
        let mk = |kind: Kind, name: &str, enabled: bool, cat: &'static str| {
            let mut e = new_entry(kind, name, "loc", "cmd", enabled, Loc::default(), "");
            e.category = cat;
            e
        };
        let entries = vec![
            mk(Kind::ShellExt, "Ext", true, "Junk"),
            mk(Kind::Run, "OneDrive", true, "Junk"),
            mk(Kind::Task, "Consolidator", true, "Junk"),
            mk(Kind::Tweak, "Windows Spotlight on the desktop", true, "Optional"),
            mk(Kind::Tweak, "Widgets (news feed board)", true, "Junk"),
            mk(Kind::StoreApp, "Teams", false, "Junk"), // already off
            mk(Kind::Run, "Steam", true, "Keep"),
        ];
        // junk only: gaming items and switched-off items are never included
        let c = clean_op(&entries, false);
        let names: Vec<&str> = c.win.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Widgets (news feed board)", "Consolidator", "OneDrive", "Ext"]);
        assert!(matches!(c.what, What::Clean { spotlight: false }));
        assert_eq!(c.win.stage, Stage::Preview);
        // every junk item starts ticked and can be unticked; a manual switch stays all-or-nothing
        assert!(c.win.pickable && c.win.unpicked.is_empty());
        assert!(!switch_op(&entries[..1], false).win.pickable);
        // + optional: the Spotlight hint is armed and the item is marked optional
        let c = clean_op(&entries, true);
        assert!(matches!(c.what, What::Clean { spotlight: true }));
        assert_eq!(c.win.items.len(), 5);
        assert!(c.win.items[1].optional && c.win.items[1].group == "Windows settings");
        let labels: Vec<String> = opwin::group_counts(&c.win.items).into_iter().map(|g| g.0).collect();
        assert_eq!(labels, ["Windows settings", "Scheduled tasks", "Startup programs", "Explorer add-ons"]);
        // nothing to clean -> opens straight in the "Already clean" result
        let c = clean_op(&entries[5..6], true);
        assert_eq!((c.win.stage, c.win.items.len()), (Stage::Done, 0));

        // switch off: KEEP items raise the warning; items already off are left out
        let s = switch_op(&entries, false);
        assert_eq!(s.win.items.len(), 6);
        let w = s.win.warning.expect("keep warning");
        assert!(w.starts_with("These are marked KEEP (gaming / hardware / security / Windows):") && w.contains("Steam") && w.ends_with("Switch them off anyway?"));
        assert!(switch_op(&entries[..5], false).win.warning.is_none());
        let s = switch_op(&entries, true);
        assert_eq!(s.win.items.len(), 1);
        assert!(s.win.warning.is_none() && matches!(s.what, What::Switch { on: true }));

        // undo: only active records, keyed by history index
        let h = |name: &str, kind: Kind, from: bool, undone: bool| Hist { batch: 1, time: String::new(), kind, name: name.into(), location: String::new(), loc: Loc::default(), from, to: !from, undone };
        let hist = vec![h("OneDrive", Kind::Run, true, false), h("Old", Kind::Run, true, true), h("Widgets", Kind::Tweak, true, false)];
        let u = undo_op(&hist, vec![0, 1, 2, 9], "Undo last change");
        let ids: Vec<&str> = u.win.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["h2", "h0"]);
        assert_eq!(u.win.items[0].detail, "back on");
        assert!(matches!(&u.what, What::Undo { idx } if idx == &vec![2, 0]));
    }

    #[test]
    fn startup_windows_tasks_detected() {
        let loc = |p: &str| Loc { task_path: Some(p.into()), task_name: Some("USO_UxBroker".into()), ..Default::default() };
        let uso = new_entry(Kind::Task, "USO_UxBroker", r"\Microsoft\Windows\UpdateOrchestrator\", r"%systemroot%\system32\MusNotification.exe", true, loc(r"\Microsoft\Windows\UpdateOrchestrator\"), "");
        assert!(is_windows_task(&uso), "Windows' own tasks are recognised (they are protected)");
        let brave = new_entry(Kind::Task, "BraveSoftwareUpdateTask", r"\", r"C:\gone\updater.exe", true, loc(r"\"), "");
        assert!(!is_windows_task(&brave), "a third-party task with a missing file is still junk");
    }

    #[test]
    fn startup_exe_path_parsing() {
        let sr = std::env::var("SystemRoot").unwrap();
        assert_eq!(get_exe_path(r#""C:\Program Files\App\app.exe" --min"#).as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(get_exe_path(r"C:\Program Files\App\app.exe --min").as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(get_exe_path(r"C:\x\a.dll,Entry").as_deref(), Some(r"C:\x\a.dll"));
        assert_eq!(get_exe_path("(COM handler)"), None);
        assert_eq!(get_exe_path("rundll32.exe foo").map(|s| s.to_lowercase()), Some(format!(r"{sr}\System32\rundll32.exe").to_lowercase()));
        assert_eq!(get_exe_path("notreal_zzz_cmd /x"), None);
        assert_eq!(get_exe_path(r"%SystemRoot%\system32\cmd.exe /c").map(|s| s.to_lowercase()), Some(format!(r"{sr}\system32\cmd.exe").to_lowercase()));
    }

    /// Unquoted paths with spaces and file types the extension list doesn't know.
    #[test]
    fn startup_exe_path_spaces_and_other_types() {
        let url = r"C:\x\Start Menu\Programs\Startup\Discord.url";
        assert_eq!(get_exe_path(&format!("\"{url}\"")).as_deref(), Some(url));
        assert_eq!(get_exe_path_sure(&format!("\"{url}\"")), Some((url.to_string(), true)));
        // unquoted: the first word is only a guess
        assert_eq!(get_exe_path_sure(url), Some((r"C:\x\Start".to_string(), false)));
        assert_eq!(get_exe_path_sure(r"C:\Program Files\App\app.exe --min").map(|x| x.1), Some(true));
        assert_eq!(get_exe_path_sure(r"C:\gone\tool").map(|x| x.1), Some(true)); // one word: nothing was cut off
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pctuner-startup-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A real .url / .ahk file in a "Start Menu" Startup folder exists, so it is never junk; a missing
    /// one still is. Reads files and publishers only.
    #[test]
    fn startup_folder_non_shortcuts_not_junk() {
        let d = temp_dir("folder");
        let dir = d.join("Start Menu").join("Programs").join("Startup");
        std::fs::create_dir_all(&dir).unwrap();
        let approved = r"HKCU:\Software\PCTunerTestScratch\startup-none"; // never created: read as "enabled"
        let mut raw: Raw = vec![];
        for name in ["Discord.url", "MyMacros.ahk"] {
            let full = dir.join(name);
            std::fs::write(&full, "x").unwrap();
            raw.push((folder_entry(name, &full.display().to_string(), approved, "Startup folder (you)"), true));
        }
        let gone = dir.join("Gone.exe").display().to_string();
        raw.push((folder_entry("Gone.exe", &gone, approved, "Startup folder (you)"), true));
        // an unquoted command cut short at a space proves nothing either
        raw.push((new_entry(Kind::Run, "Spaced", "HKCU Run", &dir.join("Tool.ahk").display().to_string(), true, Loc::default(), ""), true));
        assert!(raw[0].0.command.starts_with('"'));
        let mut warnings = vec![];
        fill_publishers(&mut raw, &mut warnings);
        for (e, _) in &raw[..2] {
            assert_ne!(e.category, "Junk", "{} exists", e.name);
            assert_ne!(e.publisher, "(file missing)");
        }
        assert_eq!((raw[2].0.category, raw[2].0.publisher.as_str()), ("Junk", "(file missing)"));
        assert_ne!(raw[3].0.category, "Junk");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn startup_history_click_skips_undone() {
        let h = |undone: bool| Hist { batch: 1, time: String::new(), kind: Kind::Run, name: "x".into(), location: String::new(), loc: Loc::default(), from: true, to: false, undone };
        let hist = vec![h(false), h(true)];
        let mut sel = HashSet::new();
        hist_click(&hist, &mut sel, 1);
        assert!(sel.is_empty(), "undone rows can't be selected");
        hist_click(&hist, &mut sel, 0);
        assert!(sel.contains(&0));
        hist_click(&hist, &mut sel, 0);
        assert!(sel.is_empty());
        sel.insert(1); // selected before it became undone
        hist_click(&hist, &mut sel, 1);
        assert!(sel.is_empty());
    }

    #[test]
    fn startup_snapshot_bytes_roundtrip() {
        assert_eq!(hex_bytes("0300ff"), Some(vec![3, 0, 255]));
        assert_eq!(hex_bytes("030"), None);
        assert_eq!(hex_bytes("zz"), None);
        assert_eq!(reg_type(REG_SZ as u32), Some(REG_SZ));
        assert_eq!(reg_type(REG_BINARY as u32), Some(REG_BINARY));
        assert_eq!(reg_type(999), None);
        // older records (and the PS tool's) have no type/bytes and still load
        let s: Snap = serde_json::from_str(r#"{"P":"HKCU:\\x","N":"y","Exists":true,"Data":1}"#).unwrap();
        assert_eq!((s.vtype, s.raw), (None, None));
        let loc = Loc { pkg: Some("P".into()), task_id: Some("T".into()), ..Default::default() };
        assert_eq!(state_value(Kind::StoreApp, &loc), Some((format!(r"{STORE_BASE}\P\T"), "State".to_string())));
        assert_eq!(state_value(Kind::Tweak, &loc), None);
    }

    const PS_ONE: &str = r#"[{"Batch":1,"Time":"t","Kind":"Run","Name":"X","Location":"HKCU Run","From":true,"To":false}]"#;

    #[test]
    fn startup_busy_through_tab_trait() {
        let mut t = StartupTab::from_load(HistLoad { hist: vec![], note: None, warning: None, writable: true });
        assert!(!(&t as &dyn Tab).busy());
        // an Explorer restart still running after the op window is done
        t.explorer_job = Some(Job::spawn(|| -> Result<String, String> {
            loop {
                std::thread::park();
            }
        }));
        assert!((&t as &dyn Tab).busy());
    }

    #[test]
    fn startup_history_load_cases() {
        // nothing at all
        let d = temp_dir("hist-none");
        let r = load_history_in(&d, Some(&d));
        assert!(r.hist.is_empty() && r.note.is_none() && r.warning.is_none() && r.writable);

        // PS history only: imported and saved as our own file
        let d = temp_dir("hist-ps");
        std::fs::create_dir_all(d.join("StartupCleaner")).unwrap();
        std::fs::write(d.join("StartupCleaner").join("history.json"), PS_ONE).unwrap();
        let r = load_history_in(&d, Some(&d));
        assert_eq!(r.hist.len(), 1);
        assert!(r.note.as_deref().unwrap().starts_with("Imported 1 change(s)"));
        assert!(d.join(HISTORY).exists() && !d.join(format!("{HISTORY}.tmp")).exists());
        // our own file now takes precedence over the PS one
        std::fs::write(d.join("StartupCleaner").join("history.json"), "[]").unwrap();
        let r = load_history_in(&d, Some(&d));
        assert_eq!((r.hist.len(), r.note.is_none()), (1, true));

        // a damaged PS file: note, and no file of ours is written
        let d = temp_dir("hist-psbad");
        std::fs::create_dir_all(d.join("StartupCleaner")).unwrap();
        std::fs::write(d.join("StartupCleaner").join("history.json"), "[{").unwrap();
        let r = load_history_in(&d, Some(&d));
        assert!(r.hist.is_empty() && r.note.as_deref().unwrap().starts_with("Could not import"));
        assert!(!d.join(HISTORY).exists());

        // our own file truncated: kept as a backup (never overwritten), warning, and the old PS
        // history is NOT re-imported (its Undone flags are stale)
        let d = temp_dir("hist-corrupt");
        let truncated = r#"[{"Batch":1,"Kind":"Run","Na"#;
        std::fs::write(d.join(HISTORY), truncated).unwrap();
        std::fs::create_dir_all(d.join("StartupCleaner")).unwrap();
        std::fs::write(d.join("StartupCleaner").join("history.json"), PS_ONE).unwrap();
        let r = load_history_in(&d, Some(&d));
        let w = r.warning.expect("warning");
        assert!(w.starts_with(&format!("Could not read {HISTORY}")) && w.contains("startup-history.corrupt-"), "{w}");
        assert!(r.hist.is_empty() && r.note.is_none() && r.writable);
        assert!(!d.join(HISTORY).exists());
        // next launch: own file missing but a backup exists, so still no import
        let r = load_history_in(&d, Some(&d));
        assert!(r.hist.is_empty() && r.note.is_none() && r.warning.is_none() && r.writable);
        assert!(!d.join(HISTORY).exists());
        let backups: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|f| f.file_name().to_string_lossy().starts_with("startup-history.corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), truncated);

        // corrupt and no PS history: empty, warning, still writable
        let d2 = temp_dir("hist-corrupt2");
        std::fs::write(d2.join(HISTORY), "").unwrap();
        let r = load_history_in(&d2, None);
        assert!(r.hist.is_empty() && r.warning.is_some() && r.writable && !d2.join(HISTORY).exists());

        for x in ["hist-none", "hist-ps", "hist-psbad", "hist-corrupt", "hist-corrupt2"] {
            let _ = std::fs::remove_dir_all(temp_dir(x));
        }
    }

    /// Deletes one subkey of HKCU\Software\PCTunerTestScratch (and the parent when empty) on drop.
    struct Scratch(&'static str);
    impl Scratch {
        fn path(&self) -> String {
            format!(r"HKCU:\Software\PCTunerTestScratch\{}", self.0)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let _ = hkcu.delete_subkey_all(format!(r"Software\PCTunerTestScratch\{}", self.0));
            let _ = hkcu.delete_subkey(r"Software\PCTunerTestScratch"); // fails while other tests still use it
        }
    }

    fn run_op(kind: Kind, loc: Loc, on: bool, snapshot: bool, undo: bool) -> Result<Loc, String> {
        apply_ops(&[Op { kind, name: "test".into(), loc, on, snapshot, undo }]).remove(0)
    }

    /// StartupApproved: switch off/on and undo put back exactly what was there (absent / 03 + old time).
    #[test]
    #[ignore]
    fn startup_registry_approved_roundtrip() {
        let s = Scratch("startup-approved");
        let loc = Loc { approved: Some(s.path()), value: Some("X".into()), ..Default::default() };
        // (a) no value before
        let changed = run_op(Kind::Run, loc.clone(), false, true, false).unwrap();
        let v = read_value(&s.path(), "X").unwrap();
        assert_eq!(v.bytes[0], 3);
        assert!(v.bytes[4..12].iter().any(|&b| b != 0));
        assert!(!changed.prev[0].exists);
        run_op(Kind::Run, changed, true, false, true).unwrap();
        assert!(read_value(&s.path(), "X").is_none(), "undo removes a value that wasn't there");
        // (b) already off with its own timestamp
        let orig = vec![3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        open_write(&s.path()).unwrap().set_raw_value("X", &RegValue { bytes: orig.clone(), vtype: REG_BINARY }).unwrap();
        let changed = run_op(Kind::StartupFolder, loc.clone(), true, true, false).unwrap();
        assert_eq!(read_value(&s.path(), "X").unwrap().bytes[0], 2);
        run_op(Kind::StartupFolder, changed, false, false, true).unwrap();
        let v = read_value(&s.path(), "X").unwrap();
        assert_eq!((v.bytes, v.vtype), (orig, REG_BINARY));
        // old records without a snapshot keep the 02/03 behaviour
        run_op(Kind::Run, loc, true, false, true).unwrap();
        assert_eq!(read_value(&s.path(), "X").unwrap().bytes[0], 2);
    }

    /// Store app State: a DWORD 0 comes back as 0 (not 1), a missing one as missing.
    #[test]
    #[ignore]
    fn startup_registry_state_snapshot_restore() {
        let s = Scratch("startup-state");
        open_write(&s.path()).unwrap().set_value("State", &0u32).unwrap();
        let snap = snap_value(&s.path(), "State");
        open_write(&s.path()).unwrap().set_value("State", &2u32).unwrap();
        restore_snap(&snap).unwrap();
        let v = read_value(&s.path(), "State").unwrap();
        assert_eq!((u32::from_reg_value(&v).unwrap(), v.vtype), (0, REG_DWORD));
        let missing = snap_value(&s.path(), "Nope");
        open_write(&s.path()).unwrap().set_value("Nope", &2u32).unwrap();
        restore_snap(&missing).unwrap();
        assert!(read_value(&s.path(), "Nope").is_none());
    }

    /// Tweak: undo restores the original type (REG_SZ stays REG_SZ) and removes values that weren't there;
    /// a failure part-way through switching off is rolled back; a bad snapshot doesn't stop the rest.
    #[test]
    #[ignore]
    fn startup_registry_tweak_roundtrip() {
        let s = Scratch("startup-tweak");
        let p = s.path();
        open_write(&p).unwrap().set_value("A", &"1".to_string()).unwrap();
        let loc = Loc {
            values: vec![TweakVal { p: p.clone(), n: "A".into(), v: 0 }, TweakVal { p: p.clone(), n: "B".into(), v: 0 }],
            ..Default::default()
        };
        let changed = run_op(Kind::Tweak, loc.clone(), false, true, false).unwrap();
        assert_eq!(read_value(&p, "A").unwrap().vtype, REG_DWORD);
        assert_eq!(changed.prev.len(), 2);
        run_op(Kind::Tweak, changed.clone(), true, false, true).unwrap();
        let a = read_value(&p, "A").unwrap();
        assert_eq!((a.vtype.clone(), String::from_reg_value(&a).unwrap()), (REG_SZ, "1".to_string()));
        assert!(read_value(&p, "B").is_none());

        // the second value can't be written: the first one goes back to its snapshot
        let bad = Loc {
            values: vec![TweakVal { p: p.clone(), n: "A".into(), v: 0 }, TweakVal { p: r"HKXX:\nowhere".into(), n: "B".into(), v: 0 }],
            ..Default::default()
        };
        assert!(run_op(Kind::Tweak, bad, false, true, false).is_err());
        let a = read_value(&p, "A").unwrap();
        assert_eq!((a.vtype.clone(), String::from_reg_value(&a).unwrap()), (REG_SZ, "1".to_string()));

        // a damaged snapshot for A: B is still restored, and the error names A
        run_op(Kind::Tweak, loc, false, true, false).unwrap();
        let mut broken = changed;
        broken.prev[0].raw = Some("zz".into());
        let e = set_native(Kind::Tweak, &broken, true, true).unwrap_err();
        assert!(e.starts_with("1 of 2 values") && e.contains(r"\A:"), "{e}");
        assert!(read_value(&p, "B").is_none());
    }

    #[test]
    fn startup_rules_match_ps() {
        assert_eq!(get_advice(Kind::Run, "Steam", r#""C:\Steam\steam.exe" -silent"#, "HKCU Run", None).0, "Keep");
        assert_eq!(get_advice(Kind::Run, "MySteamer", "x", "HKCU Run", None).0, "Review"); // \bsteam needs a word boundary
        assert_eq!(get_advice(Kind::Run, "OneDrive", "x", "HKCU Run", None).0, "Junk");
        assert_eq!(get_advice(Kind::Run, "FooUpdater", "x", "HKCU Run", None).0, "Review");
        assert_eq!(get_advice(Kind::ShellExt, "NvAppShExt", r"C:\Windows\System32\DriverStore\FileRepository\nv\nv3dappshext.dll", "Right-click (.exe)", None).0, "Junk");
        assert_eq!(get_advice(Kind::Task, "XblGameSaveTask", "", r"\Microsoft\XblGameSave\", Some(r"\Microsoft\XblGameSave\XblGameSaveTask")).0, "Keep");
        assert_eq!(get_advice(Kind::Task, "Proxy", "", "", Some(r"\microsoft\windows\autochk\proxy")).0, "Junk");
        assert_eq!(get_advice(Kind::Task, "Foo", "", "", Some(r"\Microsoft\Windows\Foo\Foo")).0, "Windows");
        assert_eq!(get_advice(Kind::Run, "x", r"C:\Program Files\AMD\x.exe", "HKLM Run", None).0, "Keep");
        assert!(is_windows_dll(r"C:\Windows\System32\shell32.dll"));
        assert!(!is_windows_dll(r"C:\Windows\System32\DriverStore\FileRepository\nv.inf\nvshext.dll"));
        assert!(!is_windows_dll(r"C:\Program Files\7-Zip\7-zip.dll"));
        assert_eq!(TWEAKS.len(), 10);
        assert_eq!(TWEAKS[1].values.len(), 12);
        assert_eq!(JUNK_TASKS.len(), 32);
    }

    #[test]
    fn startup_ps_history_import_parses() {
        let Some(base) = std::env::var_os("LOCALAPPDATA") else { return };
        let p = Path::new(&base).join("StartupCleaner").join("history.json");
        let Ok(text) = std::fs::read_to_string(&p) else {
            println!("no PS history at {}", p.display());
            return;
        };
        let v = parse_ps_history(&text).expect("PS history must parse");
        let mut kinds: HashMap<&str, usize> = HashMap::new();
        for h in &v {
            *kinds.entry(h.kind.key()).or_default() += 1;
        }
        println!("PS history: {} records, by kind {:?}, active {}", v.len(), kinds, v.iter().filter(|h| !h.undone).count());
        for h in v.iter().filter(|h| h.kind == Kind::Tweak).take(2) {
            println!("  tweak {:?}: {} values, {} snapshot values", h.name, h.loc.values.len(), h.loc.prev.len());
        }
    }

    /// The Startup folders are empty on this PC, so exercise the batched .lnk resolver on Start Menu shortcuts.
    #[test]
    fn startup_lnk_resolver() {
        let dir = format!(r"{}\Microsoft\Windows\Start Menu\Programs", std::env::var("ProgramData").unwrap());
        let lnks: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(Result::ok).map(|f| f.path().display().to_string()).filter(|p| p.to_lowercase().ends_with(".lnk")).take(5).collect())
            .unwrap_or_default();
        if lnks.is_empty() {
            return;
        }
        let rows = ps_json_lines::<LnkRow>(LNK_SCRIPT, &lnks).expect("lnk resolver");
        for r in &rows {
            println!("  {} -> \"{}\" {}", r.p, r.t, r.a);
        }
        assert_eq!(rows.len(), lnks.len());
    }

    /// Full scan of this PC. Reads only - never changes anything.
    #[test]
    fn startup_scan_readonly() {
        let t = std::time::Instant::now();
        let s = scan_store().map(|v| v.len());
        println!("store scan alone: {:.1}s {:?}", t.elapsed().as_secs_f32(), s);
        let t = std::time::Instant::now();
        let s = scan_tasks().map(|v| v.len());
        println!("task scan alone: {:.1}s {:?}", t.elapsed().as_secs_f32(), s);
        let t0 = std::time::Instant::now();
        let r = scan();
        println!("scan took {:.1}s, {} entries", t0.elapsed().as_secs_f32(), r.entries.len());
        for w in &r.warnings {
            println!("WARNING: {w}");
        }
        let mut by_kind: Vec<(&str, usize, usize)> = vec![];
        for k in KINDS {
            let all = r.entries.iter().filter(|e| e.kind == k).count();
            let on = r.entries.iter().filter(|e| e.kind == k && e.enabled).count();
            by_kind.push((k.label(), all, on));
        }
        println!("per kind (total / on):");
        for (k, a, o) in &by_kind {
            println!("  {k:<24} {a:>4} / {o}");
        }
        let mut cats: Vec<&str> = r.entries.iter().map(|e| e.category).collect();
        cats.sort();
        cats.dedup();
        println!("per category (total / on):");
        for c in cats {
            let all = r.entries.iter().filter(|e| e.category == c).count();
            let on = r.entries.iter().filter(|e| e.category == c && e.enabled).count();
            println!("  {c:<10} {all:>4} / {on}");
        }
        let non_windows = r.entries.iter().filter(|e| e.category != "Windows").count();
        println!("items shown by default (not 'Windows'): {non_windows}");
        println!("junk still on:");
        for e in r.entries.iter().filter(|e| e.enabled && e.category == "Junk") {
            println!("  [{}] {}  |  {}  |  {}", e.kind.label(), e.name, e.location, e.advice);
        }
        println!("optional still on:");
        for e in r.entries.iter().filter(|e| e.enabled && e.category == "Optional") {
            println!("  [{}] {}", e.kind.label(), e.name);
        }
        println!("explorer extensions:");
        for e in r.entries.iter().filter(|e| e.kind == Kind::ShellExt) {
            println!("  on={} {} | {} | {} | {}", e.enabled, e.name, e.location, e.publisher, e.command);
        }
        println!("missing files:");
        for e in r.entries.iter().filter(|e| e.publisher == "(file missing)") {
            println!("  [{}] {} -> {}", e.kind.label(), e.name, e.command);
        }
        println!("run / startup folder / store:");
        for e in r.entries.iter().filter(|e| matches!(e.kind, Kind::Run | Kind::StartupFolder | Kind::StoreApp)) {
            println!("  on={} [{}] {} | {} | {} | {}", e.enabled, e.kind.label(), e.name, e.publisher, e.category, e.command);
        }
        println!("tweaks:");
        for e in r.entries.iter().filter(|e| e.kind == Kind::Tweak) {
            println!("  on={} {}", e.enabled, e.name);
        }
        assert!(r.entries.len() > 50, "scan found too few entries");
        assert!(r.entries.iter().filter(|e| e.kind == Kind::Tweak).count() == TWEAKS.len());
    }
}
