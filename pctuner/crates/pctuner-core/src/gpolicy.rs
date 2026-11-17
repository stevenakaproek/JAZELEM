//! Group Policy - a curated list of gaming / privacy policies in plain English.
//! On Pro / Enterprise / Education the local Group Policy object is edited exactly like gpedit does it
//! (Registry.pol + gpt.ini, then gpupdate), so changes show in gpedit and get past Windows' script
//! block on values such as Widgets. Windows Home has no local Group Policy: the policy registry keys
//! are written directly instead. Every change is recorded in %LOCALAPPDATA%\PCTuner\gpolicy-history.json
//! and can be undone; the original policy files are backed up once before the first change.
//!
//! Keys, value names and values are taken from Windows' own ADMX files (C:\Windows\PolicyDefinitions).
//!
//! Logic only: the curated policy list, Registry.pol / gpt.ini handling, applying and undoing, and the
//! page state. The UI draws `Gpolicy` and forwards the operation window's events to `on_event`.

use crate::common::{self, Job};
use crate::opwin::{OpEvent, OpItem, OpWindow};
use crate::tabbackup;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use winreg::enums::*;
use winreg::{RegKey, RegValue};

pub const HISTORY: &str = "gpolicy-history.json";

// ------------------------------------------------------------------ the curated list

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scope {
    Machine,
    User,
}

/// One choice of a policy's drop-down (an ADMX enum element).
pub struct Choice {
    pub label: &'static str,
    pub value: u32,
}

/// A drop-down shown while the policy is Enabled; its value is written next to the main value.
pub struct Opt {
    pub name: &'static str,
    pub label: &'static str,
    pub choices: &'static [Choice],
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Rec {
    /// Leave it to the user (each choice costs a convenience): no recommendation.
    Leave,
    Enabled,
    Disabled,
    /// Enabled with these option values (one per Opt, in order).
    EnabledWith(&'static [u32]),
}

pub struct Policy {
    pub group: &'static str,
    /// Plain-English name.
    pub name: &'static str,
    /// The policy's title in gpedit.
    pub title: &'static str,
    /// Where it sits in gpedit (under Administrative Templates).
    pub path: &'static str,
    pub scope: Scope,
    pub key: &'static str,
    /// Main value written when Enabled / Disabled. None: the policy only has drop-down values.
    pub value: Option<&'static str>,
    pub on: u32,
    /// Value written when Disabled; None = the value is deleted (ADMX default when none is given).
    pub off: Option<u32>,
    pub opts: &'static [Opt],
    pub rec: Rec,
    pub advice: &'static str,
}

pub const G_AI: &str = "AI & Copilot";
pub const G_SEARCH: &str = "Widgets, Search & taskbar";
pub const G_PRIVACY: &str = "Privacy & telemetry";
pub const G_TIPS: &str = "Tips & promoted apps";
pub const G_UPDATE: &str = "Windows Update";
pub const G_GAMING: &str = "Gaming & background apps";
pub const G_OTHER: &str = "Other";
pub const GROUPS: &[&str] = &[G_AI, G_SEARCH, G_PRIVACY, G_TIPS, G_UPDATE, G_GAMING, G_OTHER];

pub const HOURS: &[Choice] = &[
    Choice { label: "12 AM", value: 0 }, Choice { label: "1 AM", value: 1 }, Choice { label: "2 AM", value: 2 }, Choice { label: "3 AM", value: 3 },
    Choice { label: "4 AM", value: 4 }, Choice { label: "5 AM", value: 5 }, Choice { label: "6 AM", value: 6 }, Choice { label: "7 AM", value: 7 },
    Choice { label: "8 AM", value: 8 }, Choice { label: "9 AM", value: 9 }, Choice { label: "10 AM", value: 10 }, Choice { label: "11 AM", value: 11 },
    Choice { label: "12 PM", value: 12 }, Choice { label: "1 PM", value: 13 }, Choice { label: "2 PM", value: 14 }, Choice { label: "3 PM", value: 15 },
    Choice { label: "4 PM", value: 16 }, Choice { label: "5 PM", value: 17 }, Choice { label: "6 PM", value: 18 }, Choice { label: "7 PM", value: 19 },
    Choice { label: "8 PM", value: 20 }, Choice { label: "9 PM", value: 21 }, Choice { label: "10 PM", value: 22 }, Choice { label: "11 PM", value: 23 },
];

pub const AI: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsAI";
pub const WSEARCH: &str = r"SOFTWARE\Policies\Microsoft\Windows\Windows Search";
pub const DATA: &str = r"Software\Policies\Microsoft\Windows\DataCollection";
pub const CLOUD: &str = r"Software\Policies\Microsoft\Windows\CloudContent";
pub const SYSTEM: &str = r"Software\Policies\Microsoft\Windows\System";
pub const APPCOMPAT: &str = r"Software\Policies\Microsoft\Windows\AppCompat";
pub const WU: &str = r"Software\Policies\Microsoft\Windows\WindowsUpdate";
pub const WU_AU: &str = r"Software\Policies\Microsoft\Windows\WindowsUpdate\AU";

/// Shorthand for the common on = 1 / off = 0 policy.
#[allow(clippy::too_many_arguments)]
pub const fn p(group: &'static str, name: &'static str, title: &'static str, path: &'static str, scope: Scope, key: &'static str, value: &'static str, rec: Rec, advice: &'static str) -> Policy {
    Policy { group, name, title, path, scope, key, value: Some(value), on: 1, off: Some(0), opts: &[], rec, advice }
}

pub const POLICIES: &[Policy] = &[
    // ---- AI
    p(G_AI, "Turn off Copilot in Windows", "Turn off Windows Copilot", r"User > Windows Components > Windows Copilot", Scope::User,
      r"SOFTWARE\Policies\Microsoft\Windows\WindowsCopilot", "TurnOffWindowsCopilot", Rec::Enabled,
      "Hides the Copilot sidebar and taskbar button. (The newer Copilot app is removed in Remove Bloat.)"),
    p(G_AI, "Recall never saves snapshots", "Turn off saving snapshots for use with Recall", r"Computer > Windows Components > Windows AI", Scope::Machine,
      AI, "DisableAIDataAnalysis", Rec::Enabled, "Recall never takes screenshots of what you do."),
    p(G_AI, "Recall cannot be switched on", "Allow Recall to be enabled", r"Computer > Windows Components > Windows AI", Scope::Machine,
      AI, "AllowRecallEnablement", Rec::Disabled, "Disabled = the Recall feature is removed and cannot be turned on (Copilot+ PCs)."),
    p(G_AI, "Turn off Click to Do", "Disable Click to Do", r"Computer > Windows Components > Windows AI", Scope::Machine,
      AI, "DisableClickToDo", Rec::Enabled, "No AI actions on screen content (Copilot+ PCs; does nothing elsewhere)."),
    // ---- Widgets / search / taskbar
    p(G_SEARCH, "Widgets", "Allow widgets", r"Computer > Windows Components > Widgets", Scope::Machine,
      r"SOFTWARE\Policies\Microsoft\Dsh", "AllowNewsAndInterests", Rec::Disabled,
      "Disabled = no Widgets board and none of its background processes. Windows blocks scripts from this one; the policy file gets past that. Sign out to apply."),
    Policy { group: G_SEARCH, name: "No web results in Start search", title: "Don't search the web or display web results in Search",
      path: r"Computer > Windows Components > Search", scope: Scope::Machine, key: WSEARCH, value: Some("ConnectedSearchUseWeb"), on: 0, off: Some(1),
      opts: &[], rec: Rec::Enabled, advice: "Start search stays on your PC: apps, files and settings, no Bing." },
    Policy { group: G_SEARCH, name: "No web search", title: "Do not allow web search", path: r"Computer > Windows Components > Search",
      scope: Scope::Machine, key: WSEARCH, value: Some("DisableWebSearch"), on: 1, off: None, opts: &[], rec: Rec::Enabled,
      advice: "Companion to the one above: search never goes online." },
    p(G_SEARCH, "Search highlights", "Allow search highlights", r"Computer > Windows Components > Search", Scope::Machine,
      WSEARCH, "EnableDynamicContentInWSB", Rec::Disabled, "Disabled = no trending topics or doodles in the search box and Start search."),
    p(G_SEARCH, "Cortana", "Allow Cortana", r"Computer > Windows Components > Search", Scope::Machine,
      WSEARCH, "AllowCortana", Rec::Disabled, "Cortana is retired; Disabled keeps it out of search for good."),
    p(G_SEARCH, "No recent searches / web suggestions", "Turn off display of recent search entries in the File Explorer search box",
      r"User > Windows Components > File Explorer", Scope::User, r"Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions",
      Rec::Enabled, "No recent-search drop-down in Explorer, and no web suggestions in Start search."),
    Policy { group: G_SEARCH, name: "Chat (Teams) icon on the taskbar", title: "Configures the Chat icon on the taskbar",
      path: r"Computer > Windows Components > Chat", scope: Scope::Machine, key: r"Software\Policies\Microsoft\Windows\Windows Chat",
      value: None, on: 0, off: None,
      opts: &[Opt { name: "ChatIcon", label: "Icon", choices: &[Choice { label: "Show", value: 1 }, Choice { label: "Hide", value: 2 }, Choice { label: "Disabled", value: 3 }] }],
      rec: Rec::EnabledWith(&[3]), advice: "Enabled > Disabled removes the Chat icon from the taskbar for good." },
    // ---- privacy
    Policy { group: G_PRIVACY, name: "Diagnostic data level", title: "Allow Diagnostic Data",
      path: r"Computer > Windows Components > Data Collection and Preview Builds", scope: Scope::Machine, key: DATA, value: None, on: 0, off: None,
      opts: &[Opt { name: "AllowTelemetry", label: "Level", choices: &[
          Choice { label: "Off (Enterprise / Education only)", value: 0 },
          Choice { label: "Required", value: 1 },
          Choice { label: "Optional", value: 3 },
      ] }],
      rec: Rec::EnabledWith(&[1]), advice: "Required is the minimum Windows Pro allows ('Off' only works on Enterprise / Education; Pro treats it as Required)." },
    p(G_PRIVACY, "No feedback pop-ups", "Do not show feedback notifications", r"Computer > Windows Components > Data Collection and Preview Builds",
      Scope::Machine, DATA, "DoNotShowFeedbackNotifications", Rec::Enabled, "Windows stops asking you to rate it."),
    p(G_PRIVACY, "Turn off the advertising ID", "Turn off the advertising ID", r"Computer > System > User Profiles", Scope::Machine,
      r"Software\Policies\Microsoft\Windows\AdvertisingInfo", "DisabledByGroupPolicy", Rec::Enabled, "Apps can't use an ID to track you for ads."),
    p(G_PRIVACY, "No tailored experiences", "Do not use diagnostic data for tailored experiences", r"User > Windows Components > Cloud Content",
      Scope::User, CLOUD, "DisableTailoredExperiencesWithDiagnosticData", Rec::Enabled, "Your diagnostic data isn't used for tips, ads and recommendations."),
    p(G_PRIVACY, "Activity feed", "Enables Activity Feed", r"Computer > System > OS Policies", Scope::Machine,
      SYSTEM, "EnableActivityFeed", Rec::Disabled, "Disabled = no activity history database (Timeline is gone in Windows 11 anyway)."),
    p(G_PRIVACY, "Publish activities", "Allow publishing of User Activities", r"Computer > System > OS Policies", Scope::Machine,
      SYSTEM, "PublishUserActivities", Rec::Disabled, "Disabled = apps don't record what you do into the activity history."),
    p(G_PRIVACY, "Upload activities", "Allow upload of User Activities", r"Computer > System > OS Policies", Scope::Machine,
      SYSTEM, "UploadUserActivities", Rec::Disabled, "Disabled = activity history is never uploaded to Microsoft."),
    p(G_PRIVACY, "No activation telemetry", "Turn off KMS Client Online AVS Validation", r"Computer > Windows Components > Software Protection Platform",
      Scope::Machine, r"Software\Policies\Microsoft\Windows NT\CurrentVersion\Software Protection Platform", "NoGenTicket", Rec::Enabled,
      "Stops the activation ticket being sent to Microsoft. Activation itself is not affected."),
    Policy { group: G_PRIVACY, name: "No Inventory Collector", title: "Turn off Inventory Collector",
      path: r"Computer > Windows Components > Application Compatibility", scope: Scope::Machine, key: APPCOMPAT, value: Some("DisableInventory"),
      on: 1, off: None, opts: &[], rec: Rec::Enabled, advice: "Stops the background inventory of your apps and drivers sent with telemetry." },
    p(G_PRIVACY, "No Steps Recorder", "Turn off Steps Recorder", r"Computer > Windows Components > Application Compatibility", Scope::Machine,
      APPCOMPAT, "DisableUAR", Rec::Enabled, "Steps Recorder (a retired troubleshooting tool) can't record your clicks."),
    p(G_PRIVACY, "Clipboard sync across devices", "Allow Clipboard synchronization across devices", r"Computer > System > OS Policies", Scope::Machine,
      SYSTEM, "AllowCrossDeviceClipboard", Rec::Leave, "Disabled = your clipboard never syncs to the cloud. Leave it if you copy between PC and phone."),
    p(G_PRIVACY, "Turn off location", "Turn off location", r"Computer > Windows Components > Location and Sensors", Scope::Machine,
      r"Software\Policies\Microsoft\Windows\LocationAndSensors", "DisableLocation", Rec::Leave,
      "Enabled = no app can use location (Weather, Maps, Find My Device, automatic time zone stop using it)."),
    // ---- tips / promoted content
    p(G_TIPS, "No promoted apps", "Turn off Microsoft consumer experiences", r"Computer > Windows Components > Cloud Content", Scope::Machine,
      CLOUD, "DisableWindowsConsumerFeatures", Rec::Enabled, "Stops Windows installing promoted apps and games. Windows Pro may ignore it (Enterprise / Education only)."),
    p(G_TIPS, "No Windows tips", "Do not show Windows tips", r"Computer > Windows Components > Cloud Content", Scope::Machine,
      CLOUD, "DisableSoftLanding", Rec::Enabled, "No tips pop-ups. Windows Pro may ignore it (Enterprise / Education only)."),
    p(G_TIPS, "No cloud optimized content", "Turn off cloud optimized content", r"Computer > Windows Components > Cloud Content", Scope::Machine,
      CLOUD, "DisableCloudOptimizedContent", Rec::Enabled, "No Microsoft-pushed content in Start and the taskbar."),
    p(G_TIPS, "Turn off Windows Spotlight", "Turn off all Windows spotlight features", r"User > Windows Components > Cloud Content", Scope::User,
      CLOUD, "DisableWindowsSpotlightFeatures", Rec::Leave, "Enabled = no Spotlight pictures, facts and suggestions on the lock screen and desktop."),
    // ---- Windows Update
    p(G_UPDATE, "Keep your own drivers", "Do not include drivers with Windows Updates", r"Computer > Windows Components > Windows Update > Manage updates offered from Windows Update",
      Scope::Machine, WU, "ExcludeWUDriversInQualityUpdate", Rec::Enabled,
      "Windows Update stops replacing your GPU / chipset drivers. Install those from NVIDIA / AMD / your board maker."),
    Policy { group: G_UPDATE, name: "No update restarts while you play", title: "Turn off auto-restart for updates during active hours",
      path: r"Computer > Windows Components > Windows Update > Manage end user experience", scope: Scope::Machine, key: WU, value: Some("SetActiveHours"),
      on: 1, off: Some(0),
      opts: &[Opt { name: "ActiveHoursStart", label: "From", choices: HOURS }, Opt { name: "ActiveHoursEnd", label: "To", choices: HOURS }],
      rec: Rec::EnabledWith(&[8, 2]), advice: "Windows never restarts for updates inside these hours (at most 18 hours)." },
    p(G_UPDATE, "No restart while signed in", "No auto-restart with logged on users for scheduled automatic updates installations",
      r"Computer > Windows Components > Windows Update > Legacy Policies", Scope::Machine, WU_AU, "NoAutoRebootWithLoggedOnUsers", Rec::Enabled,
      "Windows asks instead of restarting while you are signed in (scheduled installs)."),
    Policy { group: G_UPDATE, name: "How updates install", title: "Configure Automatic Updates",
      path: r"Computer > Windows Components > Windows Update > Manage end user experience", scope: Scope::Machine, key: WU_AU,
      value: Some("NoAutoUpdate"), on: 0, off: Some(1),
      opts: &[Opt { name: "AUOptions", label: "Mode", choices: &[
          Choice { label: "Notify before download", value: 2 },
          Choice { label: "Download, ask to install", value: 3 },
          Choice { label: "Download, ask to install and restart", value: 7 },
      ] }],
      rec: Rec::Leave, advice: "Enabled > 'Download, ask to install' = nothing installs in the middle of a game. Disabled turns automatic updates off (not recommended)." },
    Policy { group: G_UPDATE, name: "Delivery Optimization", title: "Download Mode", path: r"Computer > Windows Components > Delivery Optimization",
      scope: Scope::Machine, key: r"SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization", value: None, on: 0, off: None,
      opts: &[Opt { name: "DODownloadMode", label: "Mode", choices: &[
          Choice { label: "HTTP only", value: 0 }, Choice { label: "LAN", value: 1 }, Choice { label: "Group", value: 2 },
          Choice { label: "Internet", value: 3 }, Choice { label: "Simple", value: 99 }, Choice { label: "Bypass", value: 100 },
      ] }],
      rec: Rec::EnabledWith(&[0]), advice: "HTTP only = your PC never uploads updates to other PCs. Store / Game Pass downloads are unaffected." },
    // ---- gaming / background
    p(G_GAMING, "Game Bar recording", "Enables or disables Windows Game Recording and Broadcasting", r"Computer > Windows Components > Windows Game Recording and Broadcasting",
      Scope::Machine, r"Software\Policies\Microsoft\Windows\GameDVR", "AllowGameDVR", Rec::Leave,
      "Disabled = no Game Bar clips or background recording (saves a little performance). Leave it if you use Win+Alt+R."),
    Policy { group: G_GAMING, name: "Store apps in the background", title: "Let Windows apps run in the background",
      path: r"Computer > Windows Components > App Privacy", scope: Scope::Machine, key: r"Software\Policies\Microsoft\Windows\AppPrivacy",
      value: None, on: 0, off: None,
      opts: &[Opt { name: "LetAppsRunInBackground", label: "Apps", choices: &[
          Choice { label: "User is in control", value: 0 }, Choice { label: "Force allow", value: 1 }, Choice { label: "Force deny", value: 2 },
      ] }],
      rec: Rec::Leave, advice: "Force deny stops Store apps running in the background - Xbox app / Game Pass notifications and Phone Link stop too." },
    Policy { group: G_GAMING, name: "Windows Ink Workspace", title: "Allow Windows Ink Workspace", path: r"Computer > Windows Components > Windows Ink Workspace",
      scope: Scope::Machine, key: r"Software\Policies\Microsoft\WindowsInkWorkspace", value: None, on: 0, off: None,
      opts: &[Opt { name: "AllowWindowsInkWorkspace", label: "Ink", choices: &[
          Choice { label: "Disabled", value: 0 }, Choice { label: "On, not above lock", value: 1 }, Choice { label: "On", value: 2 },
      ] }],
      rec: Rec::EnabledWith(&[0]), advice: "Enabled > Disabled removes the pen workspace (no use without a pen)." },
    p(G_GAMING, "Program Compatibility Assistant", "Turn off Program Compatibility Assistant", r"Computer > Windows Components > Application Compatibility",
      Scope::Machine, APPCOMPAT, "DisablePCA", Rec::Leave,
      "Enabled = no 'this program might not have installed correctly' prompts; very old games lose their automatic fixes."),
    // ---- other
    Policy { group: G_OTHER, name: "Turn off AutoPlay", title: "Turn off Autoplay", path: r"Computer > Windows Components > AutoPlay Policies",
      scope: Scope::Machine, key: r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer", value: None, on: 0, off: None,
      opts: &[Opt { name: "NoDriveTypeAutoRun", label: "On", choices: &[Choice { label: "CD-ROM and removable drives", value: 181 }, Choice { label: "All drives", value: 255 }] }],
      rec: Rec::EnabledWith(&[255]), advice: "Nothing pops up or runs by itself when a USB stick or disc goes in (also safer)." },
    p(G_OTHER, "OneDrive file sync", "Prevent the usage of OneDrive for file storage", r"Computer > Windows Components > OneDrive", Scope::Machine,
      r"Software\Policies\Microsoft\Windows\OneDrive", "DisableFileSyncNGSC", Rec::Leave,
      "Enabled = OneDrive can't sync at all. Don't use it if your Documents / Desktop live in OneDrive."),
    p(G_OTHER, "Find My Device", "Turn On/Off Find My Device", r"Computer > Windows Components > Find My Device", Scope::Machine,
      r"SOFTWARE\Policies\Microsoft\FindMyDevice", "AllowFindMyDevice", Rec::Leave, "Disabled = the PC can't be located from your Microsoft account (fine for a desktop)."),
];

// ------------------------------------------------------------------ policy state

/// A policy's setting, as gpedit shows it.
#[derive(Clone, PartialEq, Debug)]
pub enum St {
    NotConfigured,
    Enabled(Vec<u32>),
    Disabled,
    /// Values present that match neither Enabled nor Disabled (set by something else).
    Other,
}

/// A setting as saved in a backup: "nc", "off", "on", "on:1,2" or "other".
pub fn st_key(s: &St) -> String {
    match s {
        St::NotConfigured => "nc".into(),
        St::Disabled => "off".into(),
        St::Other => "other".into(),
        St::Enabled(v) if v.is_empty() => "on".into(),
        St::Enabled(v) => format!("on:{}", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
    }
}

/// The setting a backup value puts back (None for "other": it cannot be written back).
pub fn st_parse(k: &str) -> Option<St> {
    match k {
        "nc" => Some(St::NotConfigured),
        "off" => Some(St::Disabled),
        "on" => Some(St::Enabled(vec![])),
        _ => {
            let v: Result<Vec<u32>, _> = k.strip_prefix("on:")?.split(',').map(|x| x.parse()).collect();
            v.ok().map(St::Enabled)
        }
    }
}

impl Policy {
    pub fn rec_state(&self) -> Option<St> {
        match self.rec {
            Rec::Leave => None,
            Rec::Enabled => Some(St::Enabled(vec![])),
            Rec::Disabled => Some(St::Disabled),
            Rec::EnabledWith(v) => Some(St::Enabled(v.to_vec())),
        }
    }

    /// Default option values when switching to Enabled by hand.
    pub fn default_opts(&self) -> Vec<u32> {
        match self.rec {
            Rec::EnabledWith(v) => v.to_vec(),
            _ => self.opts.iter().map(|o| o.choices[0].value).collect(),
        }
    }

    pub fn label(&self, s: &St) -> String {
        match s {
            St::NotConfigured => "Not configured".into(),
            St::Disabled => "Disabled".into(),
            St::Other => "Set by something else".into(),
            St::Enabled(v) if v.is_empty() || self.opts.is_empty() => "Enabled".into(),
            St::Enabled(v) => {
                let parts: Vec<&str> = self
                    .opts
                    .iter()
                    .zip(v)
                    .map(|(o, x)| o.choices.iter().find(|c| c.value == *x).map(|c| c.label).unwrap_or("?"))
                    .collect();
                format!("Enabled ({})", parts.join(" - "))
            }
        }
    }

    /// All value names this policy owns.
    pub fn names(&self) -> Vec<&'static str> {
        self.value.into_iter().chain(self.opts.iter().map(|o| o.name)).collect()
    }

    /// Writes that put this policy into `s` (None = remove the value from the policy).
    pub fn writes(&self, s: &St) -> Vec<Write> {
        let user = self.scope == Scope::User;
        let w = |name: &str, v: Option<PolVal>| Write { user, key: self.key.to_string(), name: name.to_string(), val: v };
        match s {
            St::NotConfigured | St::Other => self.names().into_iter().map(|n| w(n, None)).collect(),
            St::Enabled(v) => {
                let mut out: Vec<Write> = self.value.map(|n| w(n, Some(PolVal::Dword(self.on)))).into_iter().collect();
                let opts = if v.len() == self.opts.len() { v.clone() } else { self.default_opts() };
                out.extend(self.opts.iter().zip(opts).map(|(o, x)| w(o.name, Some(PolVal::Dword(x)))));
                out
            }
            St::Disabled => {
                let mut out: Vec<Write> = self.value.map(|n| w(n, Some(self.off.map(PolVal::Dword).unwrap_or(PolVal::Del)))).into_iter().collect();
                out.extend(self.opts.iter().map(|o| w(o.name, Some(PolVal::Del))));
                out
            }
        }
    }

    /// Read the setting through `get(name)` (from the policy file, or the registry on Home).
    pub fn state(&self, get: &dyn Fn(&str) -> Option<PolVal>) -> St {
        let opt_vals: Vec<Option<PolVal>> = self.opts.iter().map(|o| get(o.name)).collect();
        let opts_dword = || -> Option<Vec<u32>> {
            opt_vals.iter().map(|v| if let Some(PolVal::Dword(x)) = v { Some(*x) } else { None }).collect()
        };
        match self.value {
            Some(n) => match get(n) {
                Some(PolVal::Dword(x)) if x == self.on => match opts_dword() {
                    Some(v) => St::Enabled(v),
                    None if self.opts.is_empty() => St::Enabled(vec![]),
                    None => St::Other,
                },
                Some(PolVal::Dword(x)) if Some(x) == self.off => St::Disabled,
                Some(PolVal::Del) if self.off.is_none() => St::Disabled,
                None => St::NotConfigured,
                _ => St::Other,
            },
            None => {
                if opt_vals.iter().all(|v| v.is_none()) {
                    St::NotConfigured
                } else if opt_vals.iter().all(|v| matches!(v, Some(PolVal::Del))) {
                    St::Disabled
                } else {
                    opts_dword().map(St::Enabled).unwrap_or(St::Other)
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Registry.pol

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum PolVal {
    Dword(u32),
    /// "**del.<name>": the policy deletes the value (gpedit's Disabled for some policies).
    Del,
    Raw { ty: u32, data: Vec<u8> },
}

#[derive(Clone, PartialEq, Debug)]
pub struct PolEntry {
    pub key: String,
    pub name: String,
    pub val: PolVal,
}

/// One change: set (Some) or remove from the policy (None).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Write {
    pub user: bool,
    pub key: String,
    pub name: String,
    pub val: Option<PolVal>,
}

pub const REG_SZ_T: u32 = 1;
pub const REG_DWORD_T: u32 = 4;

pub fn wide(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
}

/// Parse a Registry.pol file ("PReg", version 1, then [key;value;type;size;data] in UTF-16LE).
pub fn parse_pol(b: &[u8]) -> Result<Vec<PolEntry>, String> {
    if b.is_empty() {
        return Ok(vec![]);
    }
    if b.len() < 8 || &b[0..4] != b"PReg" || u32::from_le_bytes([b[4], b[5], b[6], b[7]]) != 1 {
        return Err("not a Registry.pol file".into());
    }
    let bad = || "Registry.pol is damaged".to_string();
    let mut i = 8;
    let u16_at = |i: usize| -> Option<u16> { b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]])) };
    let expect = |i: &mut usize, c: u16| -> Result<(), String> {
        if u16_at(*i) == Some(c) {
            *i += 2;
            Ok(())
        } else {
            Err(bad())
        }
    };
    let string = |i: &mut usize| -> Result<String, String> {
        let mut units = vec![];
        loop {
            let u = u16_at(*i).ok_or_else(bad)?;
            *i += 2;
            if u == 0 {
                break;
            }
            units.push(u);
        }
        Ok(String::from_utf16_lossy(&units))
    };
    let dword = |i: &mut usize| -> Result<u32, String> {
        let x = b.get(*i..*i + 4).ok_or_else(bad)?;
        *i += 4;
        Ok(u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
    };
    let mut out = vec![];
    while i < b.len() {
        expect(&mut i, '[' as u16)?;
        let key = string(&mut i)?;
        expect(&mut i, ';' as u16)?;
        let name = string(&mut i)?;
        expect(&mut i, ';' as u16)?;
        let ty = dword(&mut i)?;
        expect(&mut i, ';' as u16)?;
        let size = dword(&mut i)? as usize;
        expect(&mut i, ';' as u16)?;
        let data = b.get(i..i + size).ok_or_else(bad)?.to_vec();
        i += size;
        expect(&mut i, ']' as u16)?;
        let lower = name.to_ascii_lowercase();
        let (name, val) = if lower.starts_with("**del.") {
            (name[6..].to_string(), PolVal::Del)
        } else if ty == REG_DWORD_T && data.len() == 4 {
            (name, PolVal::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])))
        } else {
            (name, PolVal::Raw { ty, data })
        };
        out.push(PolEntry { key, name, val });
    }
    Ok(out)
}

pub fn serialize_pol(entries: &[PolEntry]) -> Vec<u8> {
    let mut b = b"PReg".to_vec();
    b.extend(1u32.to_le_bytes());
    let ch = |c: char| (c as u16).to_le_bytes();
    for e in entries {
        let (name, ty, data) = match &e.val {
            PolVal::Dword(x) => (e.name.clone(), REG_DWORD_T, x.to_le_bytes().to_vec()),
            PolVal::Del => (format!("**del.{}", e.name), REG_SZ_T, wide(" ")),
            PolVal::Raw { ty, data } => (e.name.clone(), *ty, data.clone()),
        };
        b.extend(ch('['));
        b.extend(wide(&e.key));
        b.extend(ch(';'));
        b.extend(wide(&name));
        b.extend(ch(';'));
        b.extend(ty.to_le_bytes());
        b.extend(ch(';'));
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(ch(';'));
        b.extend(data);
        b.extend(ch(']'));
    }
    b
}

pub fn same(e: &PolEntry, key: &str, name: &str) -> bool {
    e.key.eq_ignore_ascii_case(key) && e.name.eq_ignore_ascii_case(name)
}

pub fn pol_get(entries: &[PolEntry], key: &str, name: &str) -> Option<PolVal> {
    entries.iter().rev().find(|e| same(e, key, name)).map(|e| e.val.clone())
}

/// Set (Some) or remove (None) one value; Del and normal entries of the same name replace each other.
pub fn pol_set(entries: &mut Vec<PolEntry>, key: &str, name: &str, val: Option<PolVal>) {
    entries.retain(|e| !same(e, key, name));
    if let Some(val) = val {
        entries.push(PolEntry { key: key.to_string(), name: name.to_string(), val });
    }
}

// ------------------------------------------------------------------ gpt.ini

/// Registry client-side extension, and the Administrative Templates tool GUIDs gpedit writes with it.
pub const CSE_REGISTRY: &str = "{35378EAC-683F-11D2-A89A-00C04FBBCFA2}";
pub const TOOL_MACHINE: &str = "{0F6B957D-509E-11D1-A7CC-0000F87571E3}";
pub const TOOL_USER: &str = "{0F6B957E-509E-11D1-A7CC-0000F87571E3}";

/// Make sure "[{cse}{tool}...]" lists the registry extension with gpedit's tool GUID (sorted, like gpedit).
pub fn add_ext(list: &str, tool: &str) -> String {
    let mut groups: Vec<Vec<String>> = list
        .split(']')
        .map(|g| g.trim().trim_start_matches('['))
        .filter(|g| !g.is_empty())
        .map(|g| g.split('}').filter(|s| !s.trim().is_empty()).map(|s| format!("{}}}", s.trim())).collect())
        .collect();
    match groups.iter_mut().find(|g| g.first().is_some_and(|c| c.eq_ignore_ascii_case(CSE_REGISTRY))) {
        Some(g) => {
            if !g.iter().skip(1).any(|t| t.eq_ignore_ascii_case(tool)) {
                g.push(tool.to_string());
                g[1..].sort_by_key(|s| s.to_ascii_uppercase());
            }
        }
        None => groups.push(vec![CSE_REGISTRY.to_string(), tool.to_string()]),
    }
    groups.sort_by_key(|g| g.first().map(|s| s.to_ascii_uppercase()).unwrap_or_default());
    groups.iter().map(|g| format!("[{}]", g.concat())).collect()
}

/// gpt.ini with the version bumped for the changed halves (user = high word, computer = low word)
/// and the registry extension listed, so gpupdate picks the change up.
pub fn bump_gpt(text: &str, machine: bool, user: bool) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if !lines.iter().any(|l| l.trim().eq_ignore_ascii_case("[General]")) {
        lines.insert(0, "[General]".into());
    }
    let get = |lines: &Vec<String>, k: &str| lines.iter().position(|l| l.split('=').next().is_some_and(|x| x.trim().eq_ignore_ascii_case(k)));
    let value = |l: &str| l.split_once('=').map(|x| x.1.trim().to_string()).unwrap_or_default();
    let set = |lines: &mut Vec<String>, k: &str, v: String| match get(lines, k) {
        Some(i) => lines[i] = format!("{k}={v}"),
        None => {
            let at = lines.iter().position(|l| l.trim().eq_ignore_ascii_case("[General]")).map(|i| i + 1).unwrap_or(lines.len());
            lines.insert(at, format!("{k}={v}"));
        }
    };
    let ver: u32 = get(&lines, "Version").and_then(|i| value(&lines[i]).parse().ok()).unwrap_or(0);
    let (mut hi, mut lo) = (ver >> 16, ver & 0xFFFF);
    if machine {
        lo = (lo + 1) & 0xFFFF;
    }
    if user {
        hi = (hi + 1) & 0xFFFF;
    }
    set(&mut lines, "Version", ((hi << 16) | lo).to_string());
    if machine {
        let cur = get(&lines, "gPCMachineExtensionNames").map(|i| value(&lines[i])).unwrap_or_default();
        set(&mut lines, "gPCMachineExtensionNames", add_ext(&cur, TOOL_MACHINE));
    }
    if user {
        let cur = get(&lines, "gPCUserExtensionNames").map(|i| value(&lines[i])).unwrap_or_default();
        set(&mut lines, "gPCUserExtensionNames", add_ext(&cur, TOOL_USER));
    }
    lines.join("\r\n") + "\r\n"
}

// ------------------------------------------------------------------ where policies live

pub fn gp_dir() -> PathBuf {
    let win = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    win.join("System32").join("GroupPolicy")
}

pub fn pol_path(user: bool) -> PathBuf {
    gp_dir().join(if user { "User" } else { "Machine" }).join("Registry.pol")
}

pub fn read_pol(user: bool) -> Result<Vec<PolEntry>, String> {
    match std::fs::read(pol_path(user)) {
        Ok(b) => parse_pol(&b).map_err(|e| format!("{} ({e})", pol_path(user).display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{}: {e}", pol_path(user).display())),
    }
}

/// Windows edition: Home ("Core...") has no local Group Policy, so policies go to the registry.
pub fn edition() -> (String, bool) {
    let id: String = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|k| k.get_value("EditionID"))
        .unwrap_or_default();
    let home = id.to_ascii_lowercase().starts_with("core");
    (id, !home)
}

pub fn hive(user: bool) -> RegKey {
    RegKey::predef(if user { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE })
}

pub fn reg_get(user: bool, key: &str, name: &str) -> Option<PolVal> {
    let v = hive(user).open_subkey(key).ok()?.get_raw_value(name).ok()?;
    Some(if matches!(v.vtype, REG_DWORD) && v.bytes.len() == 4 {
        PolVal::Dword(u32::from_le_bytes([v.bytes[0], v.bytes[1], v.bytes[2], v.bytes[3]]))
    } else {
        PolVal::Raw { ty: v.vtype.clone() as u32, data: v.bytes }
    })
}

pub fn reg_type(t: u32) -> RegType {
    [REG_NONE, REG_SZ, REG_EXPAND_SZ, REG_BINARY, REG_DWORD, REG_DWORD_BIG_ENDIAN, REG_LINK, REG_MULTI_SZ, REG_QWORD]
        .into_iter()
        .find(|x| x.clone() as u32 == t)
        .unwrap_or(REG_BINARY)
}

/// Write one value straight to the registry (Home, and cleanup of values left behind).
pub fn reg_set(user: bool, key: &str, name: &str, val: Option<&PolVal>) -> Result<(), String> {
    match val {
        None | Some(PolVal::Del) => match hive(user).open_subkey_with_flags(key, KEY_SET_VALUE) {
            Ok(k) => match k.delete_value(name) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("could not delete {name}: {e}")),
            },
            Err(_) => Ok(()), // key missing: nothing to delete
        },
        Some(v) => {
            let (k, _) = hive(user).create_subkey(key).map_err(|e| format!("could not open {key}: {e}"))?;
            let rv = match v {
                PolVal::Dword(x) => RegValue { bytes: x.to_le_bytes().to_vec(), vtype: REG_DWORD },
                PolVal::Raw { ty, data } => RegValue { bytes: data.clone(), vtype: reg_type(*ty) },
                PolVal::Del => unreachable!(),
            };
            k.set_raw_value(name, &rv).map_err(|e| {
                if e.raw_os_error() == Some(5) {
                    "Windows refused the change (this value is protected from scripts)".to_string()
                } else {
                    format!("could not write {name}: {e}")
                }
            })
        }
    }
}

/// The value Windows actually uses for a write (to check it took effect).
pub fn effective_ok(w: &Write) -> bool {
    let now = reg_get(w.user, &w.key, &w.name);
    match &w.val {
        None | Some(PolVal::Del) => now.is_none(),
        Some(v) => now.as_ref() == Some(v),
    }
}

// ------------------------------------------------------------------ applying

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snap {
    pub user: bool,
    pub key: String,
    pub name: String,
    /// What was there before (None = nothing).
    pub old: Option<PolVal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub batch: u32,
    pub time: String,
    /// true = written to the local Group Policy object, false = straight to the registry (Home)
    pub gpo: bool,
    pub titles: Vec<String>,
    pub snaps: Vec<Snap>,
    #[serde(default)]
    pub undone: bool,
}

/// One policy's part of an operation.
pub struct Item {
    pub id: String,
    pub writes: Vec<Write>,
}

pub struct Applied {
    /// Before-values of everything written (for the history), empty when nothing was written.
    pub snaps: Vec<Snap>,
    pub results: Vec<(String, Result<String, String>)>,
}

/// Copy the original policy files once, before PC Tuner first changes them.
pub fn backup_originals() {
    let dst = common::data_dir().join("gpo-original");
    if dst.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dst.join("Machine"));
    let _ = std::fs::create_dir_all(dst.join("User"));
    for (src, to) in [
        (gp_dir().join("gpt.ini"), dst.join("gpt.ini")),
        (pol_path(false), dst.join("Machine").join("Registry.pol")),
        (pol_path(true), dst.join("User").join("Registry.pol")),
    ] {
        if src.exists() {
            let _ = std::fs::copy(&src, &to);
        }
    }
}

pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Apply the items (runs in a background job).
pub fn apply(gpo: bool, items: Vec<Item>) -> Applied {
    let all: Vec<&Write> = items.iter().flat_map(|i| &i.writes).collect();
    if all.is_empty() {
        return Applied { snaps: vec![], results: items.into_iter().map(|i| (i.id, Ok("already set".into()))).collect() };
    }
    if !gpo {
        // Home: straight to the registry, one value at a time
        let mut snaps = vec![];
        let mut results = vec![];
        for it in &items {
            let mut err = None;
            for w in &it.writes {
                let old = reg_get(w.user, &w.key, &w.name);
                match reg_set(w.user, &w.key, &w.name, w.val.as_ref()) {
                    Ok(()) => snaps.push(Snap { user: w.user, key: w.key.clone(), name: w.name.clone(), old }),
                    Err(e) => err = Some(e),
                }
            }
            results.push((it.id.clone(), err.map(Err).unwrap_or_else(|| Ok("set in the registry".into()))));
        }
        return Applied { snaps, results };
    }

    // Pro and up: edit the local Group Policy object like gpedit, then let Windows apply it
    let fail = |why: String| Applied { snaps: vec![], results: items.iter().map(|i| (i.id.clone(), Err(why.clone()))).collect() };
    let (mut mach, mut user) = match (read_pol(false), read_pol(true)) {
        (Ok(m), Ok(u)) => (m, u),
        (Err(e), _) | (_, Err(e)) => return fail(format!("could not read the policy file {e}")),
    };
    let mut snaps = vec![];
    for w in &all {
        let pol = if w.user { &mut user } else { &mut mach };
        snaps.push(Snap { user: w.user, key: w.key.clone(), name: w.name.clone(), old: pol_get(pol, &w.key, &w.name) });
        pol_set(pol, &w.key, &w.name, w.val.clone());
    }
    let (do_m, do_u) = (all.iter().any(|w| !w.user), all.iter().any(|w| w.user));
    backup_originals();
    let gpt = gp_dir().join("gpt.ini");
    let gpt_text = std::fs::read(&gpt).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let mut writes: Vec<(PathBuf, Vec<u8>)> = vec![];
    if do_m {
        writes.push((pol_path(false), serialize_pol(&mach)));
    }
    if do_u {
        writes.push((pol_path(true), serialize_pol(&user)));
    }
    writes.push((gpt, bump_gpt(&gpt_text, do_m, do_u).into_bytes()));
    for (path, bytes) in &writes {
        if let Err(e) = write_file(path, bytes) {
            return fail(format!("could not save {e}"));
        }
    }
    // gpupdate reads the files and writes the registry (as SYSTEM, so protected values work too)
    let mut gp_err = None;
    for (on, target) in [(do_m, "/target:computer"), (do_u, "/target:user")] {
        if on {
            match common::run("gpupdate.exe", &[target, "/force", "/wait:120"]) {
                Ok((0, _)) => {}
                Ok((code, out)) => gp_err = Some(format!("gpupdate exit code {code}: {}", out.trim().lines().last().unwrap_or(""))),
                Err(e) => gp_err = Some(e),
            }
        }
    }
    let results = items
        .iter()
        .map(|it| {
            // "Not configured": Windows leaves a value it no longer manages behind in some cases - clear it
            for w in it.writes.iter().filter(|w| w.val.is_none()) {
                if !effective_ok(w) {
                    let _ = reg_set(w.user, &w.key, &w.name, None);
                }
            }
            let ok = it.writes.iter().all(effective_ok);
            let r = if ok {
                Ok("applied".to_string())
            } else if let Some(e) = &gp_err {
                Err(format!("saved in Group Policy, but Windows has not applied it yet ({e}). Restart to apply."))
            } else {
                Err("saved in Group Policy; Windows applies it at the next sign-in / restart".to_string())
            };
            (it.id.clone(), r)
        })
        .collect();
    Applied { snaps, results }
}

// ------------------------------------------------------------------ tab

pub struct Loaded {
    pub states: Vec<St>,
    /// Does the registry match the policy (has Windows applied it)?
    pub live: Vec<bool>,
    /// The setting as found in the registry (differs from `states` when something else set it).
    pub reg: Vec<St>,
    pub error: Option<String>,
}

pub fn load(gpo: bool) -> Loaded {
    let mut error = None;
    let (mach, user) = if gpo {
        match (read_pol(false), read_pol(true)) {
            (Ok(m), Ok(u)) => (m, u),
            (Err(e), _) | (_, Err(e)) => {
                error = Some(format!("Could not read the policy file {e}. Showing the registry instead."));
                (vec![], vec![])
            }
        }
    } else {
        (vec![], vec![])
    };
    let from_pol = gpo && error.is_none();
    let mut states = vec![];
    let mut live = vec![];
    let mut reg = vec![];
    for p in POLICIES {
        let u = p.scope == Scope::User;
        let in_reg = p.state(&|n| reg_get(u, p.key, n));
        let st = if from_pol {
            let pol = if u { &user } else { &mach };
            p.state(&|n| pol_get(pol, p.key, n))
        } else {
            in_reg.clone()
        };
        live.push(p.writes(&st).iter().all(effective_ok));
        states.push(st);
        reg.push(in_reg);
    }
    Loaded { states, live, reg, error }
}

pub enum Pending {
    Apply { items: Vec<(usize, St)> },
    Undo { batch: u32 },
}

/// The Group Policy page's state: what is set, what the user wants changed, and the running change.
pub struct Gpolicy {
    pub gpo: bool,
    pub edition: String,
    pub states: Vec<St>,
    pub live: Vec<bool>,
    pub reg: Vec<St>,
    pub want: Vec<Option<St>>,
    pub loader: Option<Job<Loaded>>,
    pub job: Option<Job<Applied>>,
    pub running: Option<Pending>,
    pub history: Vec<Batch>,
    pub op: Option<OpWindow>,
    pub pending: Option<Pending>,
    pub last_action: String,
    pub load_error: Option<String>,
    pub search: String,
    /// Rows the user clicked (shown green); a right-click choice applies to all of them.
    pub picked: std::collections::HashSet<usize>,
}

impl Default for Gpolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpolicy {
    pub fn new() -> Self {
        let (edition, gpo) = edition();
        let mut t = Gpolicy {
            gpo,
            edition,
            states: vec![],
            live: vec![],
            reg: vec![],
            want: vec![None; POLICIES.len()],
            loader: None,
            job: None,
            running: None,
            history: common::load_json(HISTORY),
            op: None,
            pending: None,
            last_action: String::new(),
            load_error: None,
            search: String::new(),
            picked: Default::default(),
        };
        t.reload();
        t
    }

    pub fn reload(&mut self) {
        let gpo = self.gpo;
        self.loader = Some(Job::spawn(move || load(gpo)));
    }

    pub fn busy_job(&self) -> bool {
        self.job.is_some()
    }

    /// What a policy will be after Apply.
    pub fn target(&self, i: usize) -> St {
        self.want[i].clone().unwrap_or_else(|| self.states.get(i).cloned().unwrap_or(St::NotConfigured))
    }

    pub fn changes(&self) -> Vec<(usize, St)> {
        (0..POLICIES.len()).filter_map(|i| self.want[i].clone().map(|s| (i, s))).collect()
    }

    pub fn select_recommended(&mut self) {
        let mut n = 0;
        for (i, p) in POLICIES.iter().enumerate() {
            if let Some(r) = p.rec_state() {
                if self.states.get(i) != Some(&r) {
                    self.want[i] = Some(r);
                    n += 1;
                }
            }
        }
        self.last_action = if n == 0 {
            "Everything recommended is already set.".into()
        } else {
            format!("{n} recommended changes selected. Review them below (change or reset any row), then Apply.")
        };
    }

    pub fn open_apply(&mut self) {
        let ch = self.changes();
        let items: Vec<OpItem> = ch
            .iter()
            .map(|(i, s)| {
                let p = &POLICIES[*i];
                OpItem::new(i.to_string(), p.name, p.group).detail(format!("{} -> {}", p.label(&self.states[*i]), p.label(s))).optional(p.rec == Rec::Leave)
            })
            .collect();
        let mut w = OpWindow::new("GROUP POLICY", "change", "changed", items).noun("policy", "policies");
        w.preview_text = if self.gpo {
            "Saved in the local Group Policy (the same place gpedit uses), then applied with gpupdate. You will see these in gpedit.msc.".into()
        } else {
            format!("Windows {} has no Group Policy editor: the policy values are written straight to the registry.", self.edition)
        };
        w.reassure = "Every change goes into the history - 'Undo last change' puts it all back.".into();
        w.go_label = "Apply".into();
        w.running_text = if self.gpo { "Applying (gpupdate can take a minute)...".into() } else { "Applying...".into() };
        self.op = Some(w);
        self.pending = Some(Pending::Apply { items: ch });
    }

    /// Restore a backup: the policies that differ from it are set to their backed-up setting and the
    /// usual Apply preview opens (recorded in the history, so "Undo last change" reverts the restore).
    /// Policies the backup found "set by something else" cannot be written back and are left out.
    pub fn open_restore(&mut self, b: &tabbackup::Backup) -> Result<(), String> {
        if self.busy_job() || self.op.is_some() {
            return Err("Another change is still open - finish it first.".into());
        }
        if self.states.len() != POLICIES.len() {
            return Err("The policies are still being read.".into());
        }
        let mut want = vec![None; POLICIES.len()];
        let mut n = 0;
        for d in tabbackup::differences(b, &self.backup_items()) {
            let Some(i) = POLICIES.iter().position(|p| p.name == d.backup.id) else { continue };
            if let Some(s) = d.backup.value.as_deref().and_then(st_parse) {
                want[i] = Some(s);
                n += 1;
            }
        }
        if n == 0 {
            return Err("Nothing to restore - every policy already matches this backup.".into());
        }
        self.want = want;
        self.picked.clear();
        self.open_apply();
        if let Some(w) = &mut self.op {
            w.kicker = "RESTORE GROUP POLICY BACKUP".into();
            w.preview_text = format!("Puts these policies back to the backup from {}. {}", b.time, w.preview_text);
        }
        Ok(())
    }

    /// Backups: every policy's setting as the page shows it.
    pub fn backup_items(&self) -> Vec<tabbackup::Item> {
        POLICIES.iter().zip(&self.states).map(|(p, s)| tabbackup::item(p.name, p.name, Some(st_key(s)), p.label(s))).collect()
    }

    pub fn open_undo(&mut self) {
        let Some(b) = self.history.iter().rev().find(|b| !b.undone) else {
            self.last_action = "Nothing to undo.".into();
            return;
        };
        let items = b.titles.iter().enumerate().map(|(i, t)| OpItem::new(format!("u{i}"), t.as_str(), "Undo")).collect();
        let mut w = OpWindow::new("GROUP POLICY", "undo", "undone", items).noun("policy", "policies");
        w.preview_text = format!("Puts these policies back the way they were before {}.", b.time);
        w.go_label = "Undo".into();
        w.running_text = "Undoing...".into();
        self.pending = Some(Pending::Undo { batch: b.batch });
        self.op = Some(w);
    }

    pub fn start(&mut self, p: Pending) {
        let gpo = self.gpo;
        match &p {
            Pending::Apply { items } => {
                let items: Vec<Item> = items.iter().map(|(i, s)| Item { id: i.to_string(), writes: POLICIES[*i].writes(s) }).collect();
                self.job = Some(Job::spawn(move || apply(gpo, items)));
            }
            Pending::Undo { batch } => {
                let Some(b) = self.history.iter().find(|b| b.batch == *batch) else { return };
                // newest write first, so a value written twice ends at its oldest before-value
                let writes: Vec<Write> =
                    b.snaps.iter().rev().map(|s| Write { user: s.user, key: s.key.clone(), name: s.name.clone(), val: s.old.clone() }).collect();
                let gpo = b.gpo;
                let n = b.titles.len();
                self.job = Some(Job::spawn(move || {
                    let mut a = apply(gpo, vec![Item { id: "all".into(), writes }]);
                    // one result line per policy in the window
                    let r = a.results.pop().map(|x| x.1).unwrap_or(Ok(String::new()));
                    a.results = (0..n).map(|i| (format!("u{i}"), r.clone())).collect();
                    a
                }));
            }
        }
        self.running = Some(p);
        self.last_action = "Applying...".into();
    }

    pub fn finish(&mut self, a: Applied) {
        let ok = a.results.iter().filter(|r| r.1.is_ok()).count();
        let failed = a.results.len() - ok;
        match self.running.take() {
            Some(Pending::Apply { items }) => {
                if !a.snaps.is_empty() {
                    let batch = self.history.iter().map(|b| b.batch).max().unwrap_or(0) + 1;
                    let titles = items.iter().map(|(i, s)| format!("{}: {}", POLICIES[*i].name, POLICIES[*i].label(s))).collect();
                    self.history.push(Batch { batch, time: common::now(), gpo: self.gpo, titles, snaps: a.snaps, undone: false });
                    common::save_json(HISTORY, &self.history);
                }
                for (i, _) in &items {
                    self.want[*i] = None;
                }
                self.picked.clear();
                self.last_action = format!("{ok} policies changed{}.", if failed > 0 { format!(", {failed} need attention") } else { String::new() });
            }
            Some(Pending::Undo { batch }) => {
                if failed == 0 {
                    if let Some(b) = self.history.iter_mut().find(|b| b.batch == batch) {
                        b.undone = true;
                    }
                    common::save_json(HISTORY, &self.history);
                }
                self.last_action = if failed == 0 { "Last change undone.".into() } else { "Undo needs attention - see the window.".into() };
            }
            None => {}
        }
        if let Some(w) = self.op.as_mut() {
            let res = &a.results;
            w.finish(|it| res.iter().find(|r| r.0 == it.id).map(|r| r.1.clone()));
            if ok > 0 && self.gpo {
                w.notes.push("Some policies (Widgets, Search) need a sign-out or restart to show.".into());
            }
        }
        self.reload();
    }

    /// Set what a policy becomes on Apply (back to "no change" when it equals the current setting).
    /// Enabled keeps the current drop-down values when the policy is already enabled.
    pub fn set_target(&mut self, i: usize, s: St) {
        let p = &POLICIES[i];
        let cur = self.states.get(i).cloned().unwrap_or(St::NotConfigured);
        let s = match (&cur, s) {
            (St::Enabled(v), St::Enabled(_)) => St::Enabled(v.clone()),
            (_, St::Enabled(v)) if v.len() != p.opts.len() => St::Enabled(p.default_opts()),
            (_, s) => s,
        };
        self.want[i] = if s == cur { None } else { Some(s) };
    }

    /// A row's click or right-click choice. A right-click choice goes to every selected row when the
    /// clicked row is one of them, otherwise to that row alone (which is then selected).
    pub fn row_action(&mut self, i: usize, act: RowAct) {
        let rows: Vec<usize> = if self.picked.contains(&i) { self.picked.iter().copied().collect() } else { vec![i] };
        match act {
            RowAct::None => {}
            RowAct::Toggle => {
                if !self.picked.remove(&i) {
                    self.picked.insert(i);
                }
            }
            RowAct::Set(s) => {
                self.picked.insert(i);
                for r in rows {
                    self.set_target(r, s.clone());
                }
            }
            RowAct::Reset => {
                for r in rows {
                    self.want[r] = None;
                }
            }
        }
    }

    /// Poll the policy read and a running change. Call often.
    pub fn pump(&mut self) {
        if let Some(l) = self.loader.as_ref().and_then(|j| j.poll()) {
            self.loader = None;
            self.states = l.states;
            self.live = l.live;
            self.reg = l.reg;
            self.load_error = l.error;
        }
        if let Some(a) = self.job.as_ref().and_then(|j| j.poll()) {
            self.job = None;
            self.finish(a);
        }
    }

    /// May changes be applied now (administrator, nothing running)?
    pub fn can_act(&self) -> bool {
        common::is_admin() && !self.busy_job()
    }

    /// React to a button of the operation window.
    pub fn on_event(&mut self, ev: OpEvent) {
        match ev {
            OpEvent::Go => {
                if let Some(p) = self.pending.take() {
                    self.start(p);
                }
            }
            OpEvent::Close => {
                self.op = None;
                self.pending = None;
            }
            _ => {}
        }
    }

    /// "Discard changes": forget every pending change and the selection.
    pub fn discard(&mut self) {
        self.want = vec![None; POLICIES.len()];
        self.picked.clear();
        self.last_action.clear();
    }
}

/// What happened on a policy row this frame.
pub enum RowAct {
    None,
    Toggle,
    Set(St),
    Reset,
}

// ------------------------------------------------------------------ tests (no system changes)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpolicy_backup_values_round_trip() {
        for s in [St::NotConfigured, St::Disabled, St::Enabled(vec![]), St::Enabled(vec![2, 0, 15])] {
            assert_eq!(st_parse(&st_key(&s)), Some(s));
        }
        assert_eq!(st_parse(&st_key(&St::Other)), None, "set by something else cannot be written back");
        assert_eq!(st_parse("on:x"), None);
        let names: std::collections::HashSet<&str> = POLICIES.iter().map(|p| p.name).collect();
        assert_eq!(names.len(), POLICIES.len(), "policy names are the backup ids, so they must be unique");
    }

    /// Click selects a row; a right-click choice goes to every selected row (or just the clicked one).
    #[test]
    fn gpolicy_select_and_right_click() {
        let mut t = Gpolicy::new();
        t.states = vec![St::NotConfigured; POLICIES.len()];
        t.row_action(0, RowAct::Toggle);
        t.row_action(1, RowAct::Toggle);
        assert!(t.picked.contains(&0) && t.picked.contains(&1));
        // right-click "Disable" on a selected row: both selected rows change
        t.row_action(1, RowAct::Set(St::Disabled));
        assert_eq!(t.want[0], Some(St::Disabled));
        assert_eq!(t.want[1], Some(St::Disabled));
        assert_eq!(t.want[2], None);
        // right-click on a row that is not selected: only that row changes, and it gets selected
        t.row_action(2, RowAct::Set(St::Enabled(vec![])));
        assert!(matches!(t.want[2], Some(St::Enabled(_))));
        assert!(t.picked.contains(&2));
        if let Some(St::Enabled(v)) = &t.want[2] {
            assert_eq!(v.len(), POLICIES[2].opts.len(), "Enabled gets the policy's drop-down values");
        }
        // choosing what it already is means no change
        t.row_action(2, RowAct::Set(St::NotConfigured));
        assert_eq!(t.want[2], None);
        // "keep as it is" clears the selected rows' changes; clicking again unselects
        t.row_action(0, RowAct::Reset);
        assert!(t.want.iter().all(|w| w.is_none()));
        t.row_action(0, RowAct::Toggle);
        assert!(!t.picked.contains(&0));
    }

    #[test]
    fn gpolicy_pol_roundtrip() {
        let mut e = vec![];
        pol_set(&mut e, r"Software\Policies\X", "A", Some(PolVal::Dword(1)));
        pol_set(&mut e, r"Software\Policies\X", "B", Some(PolVal::Del));
        pol_set(&mut e, r"Software\Policies\Y", "S", Some(PolVal::Raw { ty: 1, data: wide("hi") }));
        let b = serialize_pol(&e);
        assert_eq!(&b[0..4], b"PReg");
        let back = parse_pol(&b).unwrap();
        assert_eq!(back, e);
        // replacing and removing
        pol_set(&mut e, r"software\policies\x", "a", Some(PolVal::Dword(0)));
        assert_eq!(pol_get(&e, r"Software\Policies\X", "A"), Some(PolVal::Dword(0)));
        pol_set(&mut e, r"Software\Policies\X", "B", None);
        assert_eq!(pol_get(&e, r"Software\Policies\X", "B"), None);
        assert!(parse_pol(b"nope").is_err());
        assert!(parse_pol(&b[..b.len() - 3]).is_err());
    }

    #[test]
    fn gpolicy_reads_the_real_policy_files() {
        // read-only: the files on this PC must parse (missing files are fine)
        for u in [false, true] {
            let r = read_pol(u);
            assert!(r.is_ok(), "{r:?}");
            println!("{}: {} entries", pol_path(u).display(), r.unwrap().len());
        }
    }

    #[test]
    fn gpolicy_gpt_ini() {
        let t = bump_gpt("", true, false);
        assert!(t.contains("Version=1"), "{t}");
        assert!(t.contains(&format!("gPCMachineExtensionNames=[{CSE_REGISTRY}{TOOL_MACHINE}]")), "{t}");
        let t = bump_gpt("[General]\r\nVersion=65537\r\ngPCMachineExtensionNames=[{0000-A}{1111}][{35378EAC-683F-11D2-A89A-00C04FBBCFA2}{AAAA}]\r\n", true, true);
        assert!(t.contains(&format!("Version={}", (2 << 16) | 2)), "{t}");
        assert!(t.contains(&format!("[{{0000-A}}{{1111}}][{CSE_REGISTRY}{TOOL_MACHINE}{{AAAA}}]")), "{t}");
        assert!(t.contains(&format!("gPCUserExtensionNames=[{CSE_REGISTRY}{TOOL_USER}]")), "{t}");
        // idempotent extension lists
        let again = bump_gpt(&t, true, true);
        assert_eq!(again.matches(TOOL_MACHINE).count(), 1);
        // the gpt.ini this PC already has (written by gpedit) keeps its user list unchanged
        let real = "[General]\r\ngPCUserExtensionNames=[{35378EAC-683F-11D2-A89A-00C04FBBCFA2}{0F6B957E-509E-11D1-A7CC-0000F87571E3}]\r\nVersion=131072\r\n";
        let t = bump_gpt(real, true, false);
        assert!(t.contains("gPCUserExtensionNames=[{35378EAC-683F-11D2-A89A-00C04FBBCFA2}{0F6B957E-509E-11D1-A7CC-0000F87571E3}]"), "{t}");
        assert!(t.contains("Version=131073"), "{t}");
    }

    /// Live test (needs administrator; changes this PC, then puts it back): `cargo test gpolicy_live -- --ignored`.
    /// Applies "No feedback pop-ups" through the local Group Policy, checks Windows applied it, then undoes it.
    #[test]
    #[ignore]
    fn gpolicy_live_apply_and_undo() {
        // log to a file as well: an elevated run has no console to read
        let log_path = std::env::var("GP_LIVE_LOG").ok();
        let log = |s: String| {
            println!("{s}");
            if let Some(p) = &log_path {
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                    let _ = writeln!(f, "{s}");
                }
            }
        };
        let check = |ok: bool, what: String| {
            log(format!("{} {what}", if ok { "PASS" } else { "FAIL" }));
            ok
        };
        let mut all = check(common::is_admin(), "running as administrator".into());
        let (ed, gpo) = edition();
        let i = POLICIES.iter().position(|p| p.value == Some("DoNotShowFeedbackNotifications")).unwrap();
        let p = &POLICIES[i];
        let name = "DoNotShowFeedbackNotifications";
        log(format!("edition {ed}, gpo mode {gpo}; at start: registry {:?}, policy {:?}", reg_get(false, p.key, name), load(gpo).states[i]));

        // 1. start from Not configured (also cleans up an earlier interrupted run)
        let a = apply(gpo, vec![Item { id: "c".into(), writes: p.writes(&St::NotConfigured) }]);
        log(format!("set Not configured: {:?}", a.results));
        all &= check(reg_get(false, p.key, name).is_none(), "registry value gone".into());
        let pol_base = read_pol(false).unwrap();

        // 2. Enabled
        let a = apply(gpo, vec![Item { id: "t".into(), writes: p.writes(&St::Enabled(vec![])) }]);
        log(format!("apply Enabled: {:?}", a.results));
        all &= check(a.results[0].1.is_ok(), "apply reported ok".into());
        all &= check(reg_get(false, p.key, name) == Some(PolVal::Dword(1)), format!("registry = 1 ({:?})", reg_get(false, p.key, name)));
        let l = load(gpo);
        all &= check(l.states[i] == St::Enabled(vec![]) && l.live[i], format!("tab shows Enabled, applied ({:?}, live {})", l.states[i], l.live[i]));
        log(format!("gpt.ini: {}", std::fs::read_to_string(gp_dir().join("gpt.ini")).unwrap_or_default().replace("\r\n", " | ")));

        // 3. undo = write the snapshots back
        let writes: Vec<Write> = a.snaps.iter().rev().map(|s| Write { user: s.user, key: s.key.clone(), name: s.name.clone(), val: s.old.clone() }).collect();
        let u = apply(gpo, vec![Item { id: "u".into(), writes }]);
        log(format!("undo: {:?}", u.results));
        all &= check(u.results[0].1.is_ok(), "undo reported ok".into());
        all &= check(reg_get(false, p.key, name).is_none(), format!("registry value gone again ({:?})", reg_get(false, p.key, name)));
        all &= check(load(gpo).states[i] == St::NotConfigured, "tab shows Not configured".into());
        all &= check(read_pol(false).unwrap() == pol_base, "policy file back to its entries before the test".into());
        log(if all { "ALL PASSED".into() } else { "SOME CHECKS FAILED".into() });
        assert!(all);
    }

    #[test]
    fn gpolicy_states_roundtrip() {
        for p in POLICIES {
            let mut states = vec![St::NotConfigured, St::Disabled, St::Enabled(p.default_opts())];
            if let Some(r) = p.rec_state() {
                states.push(r);
            }
            for s in states {
                let mut e = vec![];
                for w in p.writes(&s) {
                    pol_set(&mut e, &w.key, &w.name, w.val);
                }
                let s = match s {
                    St::Enabled(v) if p.opts.is_empty() && v.is_empty() => St::Enabled(vec![]),
                    other => other,
                };
                assert_eq!(p.state(&|n| pol_get(&e, p.key, n)), s, "{}", p.name);
            }
            // every recommendation fits the drop-downs
            if let Rec::EnabledWith(v) = p.rec {
                assert_eq!(v.len(), p.opts.len(), "{}", p.name);
                for (o, x) in p.opts.iter().zip(v) {
                    assert!(o.choices.iter().any(|c| c.value == *x), "{}", p.name);
                }
            }
            assert!(GROUPS.contains(&p.group));
        }
    }

    #[test]
    fn gpolicy_policies_exist_in_windows() {
        // each value name is defined in Windows' own ADMX files (read-only check; skipped without them)
        let dir = Path::new(r"C:\Windows\PolicyDefinitions");
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let all: String = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("admx")))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .map(|b| {
                // some ADMX files (Search.admx) are UTF-16 with a BOM
                if b.starts_with(&[0xFF, 0xFE]) {
                    let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    String::from_utf16_lossy(&u)
                } else {
                    String::from_utf8_lossy(&b).into_owned()
                }
            })
            .collect();
        for p in POLICIES {
            for n in p.names() {
                assert!(all.contains(&format!("valueName=\"{n}\"")), "{} ({n}) not in the ADMX files", p.name);
            }
        }
    }
}
