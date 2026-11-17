//! Performance - Windows settings that make games and the desktop feel faster, chosen from
//! researched evidence.
//!
//! Honest scope: Halo Infinite on this PC is GPU-bound, so almost nothing in Windows changes its fps.
//! What helps is lower latency in borderless DX10/11 games, a snappier desktop, and checks that the
//! important settings (Game Mode, GPU scheduling, refresh rate) are where they should be.
//!
//! Logic only: the settings table, reading the current state, the apply / undo scripts, the history and
//! the operation window's contents. The UI draws `Perf` and forwards the window's events to `on_event`.

use crate::common::{self, Job, Stream};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::tabbackup;
use serde::{Deserialize, Serialize};

pub const HISTORY: &str = "perf-history.json";
pub const DX_KEY: &str = r"HKCU:\Software\Microsoft\DirectX\UserGpuPreferences";
pub const ADV_KEY: &str = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
pub const SERIALIZE_KEY: &str = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize";
pub const CLASSIC_MENU_KEY: &str = r"HKCU:\Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}";
pub const SUB_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
pub const PERFEPP: &str = "36687f9e-e3a5-4dbf-b1dc-15eb381c6863";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// One `Key=Value;` pair inside HKCU\...\DirectX\UserGpuPreferences\DirectXUserGlobalSettings.
    DxPair(&'static str),
    /// MenuShowDelay through SystemParametersInfo (applies at once).
    MenuDelay,
    /// MouseHoverTime through SystemParametersInfo.
    HoverTime,
    /// Enhance pointer precision: "threshold1,threshold2,speed" through SPI_SETMOUSE.
    Mouse,
    /// A DWORD under HKCU\...\Explorer\Advanced (Explorer reads it at start).
    AdvDword(&'static str),
    /// Explorer\Serialize StartupDelayInMSec + WaitForIdleState.
    StartupDelay,
    /// The Windows 10 style right-click menu (CLSID override).
    ClassicMenu,
    /// Processor energy performance preference on AC, current power plan (powercfg).
    Epp,
    /// Hibernation and its hiberfil.sys (powercfg /hibernate).
    Hibernate,
    /// Virtualization-based security: "vbs|hello" = DeviceGuard EnableVirtualizationBasedSecurity and
    /// Scenarios\WindowsHello\Enabled (Enhanced Sign-in Security keeps VBS running). Restart needed.
    Vbs,
    /// Background apps (Store apps): policy AppPrivacy LetAppsRunInBackground + _ForceAllowTheseApps.
    /// Value "policy|app;app;..." (an empty part = absent).
    BgApps,
}

impl Kind {
    /// Explorer has to restart before the change shows.
    pub fn needs_explorer(self) -> bool {
        matches!(self, Kind::AdvDword(_) | Kind::ClassicMenu)
    }
}

pub struct Tweak {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub kind: Kind,
    pub target: &'static str,
    pub optional: bool,
    pub why: &'static str,
}

pub const G_GAMES: &str = "Games";
pub const G_DESKTOP: &str = "Snappier desktop";
pub const G_POWER: &str = "Power, mouse and disk";

pub const TWEAKS: &[Tweak] = &[
    Tweak { id: "windowed", name: "Optimizations for windowed games", group: G_GAMES, kind: Kind::DxPair("SwapEffectUpgradeEnable"), target: "1", optional: false,
        why: "Older DX10/DX11 games in a borderless window switch to the modern 'flip' model: lower latency and working VRR. Halo (DX12) already has it; helps older Steam games. Applies at the next game launch." },
    Tweak { id: "vrr", name: "Windows variable refresh rate", group: G_GAMES, kind: Kind::DxPair("VRROptimizeEnable"), target: "1", optional: true,
        why: "Lets old DX11 games without their own VRR support use G-SYNC. Small: most games already handle VRR. Turn it back off if an old game misbehaves." },
    Tweak { id: "menu", name: "Faster submenus", group: G_DESKTOP, kind: Kind::MenuDelay, target: "100", optional: false,
        why: "Submenus (Send to, Open with, menu bars) open after 100 ms instead of 400 ms. You feel it; no effect on fps. Applies at once." },
    Tweak { id: "hover", name: "Faster tooltips", group: G_DESKTOP, kind: Kind::HoverTime, target: "200", optional: true,
        why: "Hover pop-ups in classic apps appear after 200 ms instead of 400 ms. Many modern apps use their own timing." },
    Tweak { id: "taskanim", name: "Taskbar animations", group: G_DESKTOP, kind: Kind::AdvDword("TaskbarAnimations"), target: "0", optional: true,
        why: "Taskbar buttons stop sliding and fading. Small; needs an Explorer restart." },
    Tweak { id: "thumbhover", name: "Faster taskbar thumbnails", group: G_DESKTOP, kind: Kind::AdvDword("ExtendedUIHoverTime"), target: "150", optional: true,
        why: "Window previews appear sooner when you hover a taskbar button. Unofficial value - may do nothing on some builds. Needs an Explorer restart." },
    Tweak { id: "startdelay", name: "No startup-app delay", group: G_DESKTOP, kind: Kind::StartupDelay, target: "0|0", optional: true,
        why: "Tray apps start straight after sign-in instead of a few seconds later. Unofficial; the first seconds after sign-in can feel busier. Next sign-in." },
    Tweak { id: "classicmenu", name: "Classic right-click menu", group: G_DESKTOP, kind: Kind::ClassicMenu, target: "on", optional: true,
        why: "Right-click shows the full menu straight away instead of 'Show more options'. A preference; unsupported by Microsoft, so an update could remove it. Needs an Explorer restart." },
    Tweak { id: "epp", name: "CPU responds faster (energy preference)", group: G_POWER, kind: Kind::Epp, target: "10", optional: true,
        why: "Plugged-in energy preference 25 -> 10, the same value Windows' 'Best performance' power mode uses: clocks ramp up sooner. A few watts more at light load; no fps change when the GPU is the limit." },
    Tweak { id: "mouseaccel", name: "Mouse acceleration off", group: G_POWER, kind: Kind::Mouse, target: "0,0,0", optional: true,
        why: "'Enhance pointer precision' off: the cursor moves 1:1 with your hand. A preference - Halo uses raw input and is not affected. You may want a higher mouse DPI." },
    Tweak { id: "hibernate", name: "Hibernation off (frees disk space)", group: G_POWER, kind: Kind::Hibernate, target: "0", optional: true,
        why: "Deletes hiberfil.sys (about 40 % of your RAM in size). Hibernate disappears from the power menu; Fast Startup is already off on this PC. Disk space only, not speed." },
    Tweak { id: "bgapps", name: "Background apps off (Xbox apps allowed)", group: G_POWER, kind: Kind::BgApps, target: BG_TARGET, optional: true,
        why: "Store apps (Weather, Photos, Phone Link, Teams...) can no longer run in the background. The Xbox app, Game Bar, Xbox sign-in, Gaming Services and the Store are exempt, so invites, Game Pass downloads and sign-in keep working. Normal desktop programs are not affected. Small gain: less background activity and RAM." },
    Tweak { id: "vbs", name: "Virtualization-based security (VBS) off", group: G_POWER, kind: Kind::Vbs, target: "0|0", optional: true,
        why: "Stops Windows running its security layer in a hidden virtual machine. Memory Integrity is already off here, so the gain is small (0-5 % in some games). Costs security: Windows Hello enhanced sign-in, Credential Guard-style protection and Memory Integrity stop working. Restart needed. Hyper-V / WSL keep working." },
];

pub fn tweak(id: &str) -> Option<&'static Tweak> {
    TWEAKS.iter().find(|t| t.id == id)
}

/// Changes for the selected rows: each selected tweak that is available and not applied yet gets its
/// value. Selected rows that are already applied need nothing.
pub fn selected_changes(state: &State, selected: &[bool]) -> Vec<Change> {
    TWEAKS
        .iter()
        .zip(selected)
        .filter(|(t, sel)| **sel && state.applicable(t.kind) && !state.is_set(t))
        .map(|(t, _)| Change { id: t.id.into(), prev: state.value(t.kind), new: Some(t.target.into()) })
        .collect()
}

/// Backups: every setting whose value can be read now.
pub fn backup_items(state: &State) -> Vec<tabbackup::Item> {
    TWEAKS
        .iter()
        .filter(|t| state.applicable(t.kind))
        .map(|t| {
            let v = state.value(t.kind);
            let shown = fmt(t.kind, v.as_deref());
            tabbackup::item(t.id, t.name, v, shown)
        })
        .collect()
}

// ------------------------------------------------------------------ state

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct State {
    pub dx: Option<String>,
    pub menu: Option<String>,
    pub hover: Option<String>,
    pub mouse: Option<String>,
    pub taskanim: Option<u32>,
    pub thumbhover: Option<u32>,
    /// "delay|idle" (an empty part = absent)
    pub startdelay: Option<String>,
    pub classicmenu: bool,
    /// active power plan GUID (EPP is changed on it)
    pub scheme: String,
    pub epp: Option<u32>,
    /// Settings > Power mode overlay GUID; empty = Balanced (no overlay)
    pub overlay: String,
    pub hibernate: Option<u32>,
    /// "vbs|hello" (an empty part = absent)
    pub vbs: Option<String>,
    /// Win32_DeviceGuard VirtualizationBasedSecurityStatus: 0 off, 1 configured, 2 running
    pub vbs_status: Option<u32>,
    /// "policy|app;app" (an empty part = absent)
    pub bgapps: Option<String>,
    pub hiberfil_gb: Option<f32>,
    // checks (read only)
    pub game_mode: Option<u32>,
    pub hags: Option<u32>,
    pub app_capture: Option<u32>,
    pub gamedvr: Option<u32>,
    pub transparency: Option<u32>,
    pub min_animate: Option<String>,
    pub client_anim: Option<bool>,
    pub usb_suspend: Option<u32>,
    pub pcie_aspm: Option<u32>,
    pub hz_now: Option<u32>,
    pub hz_max: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// name of the monitor whose refresh rate is shown (filled in by `read_state`)
    pub monitor: String,
    /// device interface id of the active monitor on the primary display, as EnumDisplayDevices gives it
    pub display_id: String,
    /// every monitor WMI knows about
    pub monitors: Vec<Monitor>,
    pub error: Option<String>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Monitor {
    /// WmiMonitorID InstanceName, e.g. `DISPLAY\AUS274F\5&148b3217&1&UID4353_0`
    pub instance: String,
    pub name: String,
    pub active: bool,
}

/// `\\?\DISPLAY#AUS274F#5&148b3217&1&UID4353#{e6f07b5f-...}` -> `DISPLAY\AUS274F\5&148B3217&1&UID4353`.
pub fn display_key(id: &str) -> Option<String> {
    let s = id.trim();
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let s = match s.rfind("#{") {
        Some(i) => &s[..i],
        None => s,
    };
    let key = s.replace('#', "\\").to_ascii_uppercase();
    key.starts_with("DISPLAY\\").then_some(key)
}

/// Name of the monitor on the measured display. Falls back to an active PG27AQWP (so its 540 Hz
/// note can still show), otherwise empty - never an unrelated first monitor.
pub fn pick_monitor(display_id: &str, monitors: &[Monitor]) -> String {
    if let Some(key) = display_key(display_id) {
        let hit = monitors.iter().find(|m| {
            let inst = m.instance.to_ascii_uppercase();
            inst.strip_prefix(&key).is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
        });
        if let Some(m) = hit {
            return m.name.clone();
        }
    }
    monitors.iter().find(|m| m.active && m.name.to_ascii_uppercase().contains("PG27AQWP")).map(|m| m.name.clone()).unwrap_or_default()
}

/// Value of one `Key=Value;` pair in the DirectX settings string.
pub fn dx_pair(s: Option<&str>, key: &str) -> Option<String> {
    s?.split(';').filter_map(|p| p.split_once('=')).find(|(k, _)| k.trim().eq_ignore_ascii_case(key)).map(|(_, v)| v.trim().to_string())
}

impl State {
    pub fn value(&self, kind: Kind) -> Option<String> {
        match kind {
            Kind::DxPair(k) => dx_pair(self.dx.as_deref(), k),
            Kind::MenuDelay => Some(self.menu.clone().unwrap_or_else(|| "400".into())),
            Kind::HoverTime => Some(self.hover.clone().unwrap_or_else(|| "400".into())),
            Kind::Mouse => self.mouse.clone(),
            Kind::AdvDword("TaskbarAnimations") => self.taskanim.map(|v| v.to_string()),
            Kind::AdvDword(_) => self.thumbhover.map(|v| v.to_string()),
            Kind::StartupDelay => self.startdelay.clone(),
            Kind::ClassicMenu => self.classicmenu.then(|| "on".to_string()),
            // "plan-guid:value" so undo goes back to the same power plan
            Kind::Epp => self.epp.map(|v| if self.scheme.is_empty() { v.to_string() } else { format!("{}:{v}", self.scheme) }),
            Kind::Hibernate => self.hibernate.map(|v| v.to_string()),
            Kind::Vbs => self.vbs.clone(),
            Kind::BgApps => self.bgapps.clone(),
        }
    }

    /// A Power mode other than Balanced is active (it overrides the plan's EPP).
    pub fn overlay_active(&self) -> bool {
        let o = self.overlay.trim();
        !o.is_empty() && o != "00000000-0000-0000-0000-000000000000"
    }

    pub fn applicable(&self, kind: Kind) -> bool {
        // nothing can be changed safely when the read failed: "before" values would be guesses
        if self.error.is_some() {
            return false;
        }
        match kind {
            Kind::Epp => self.epp.is_some() && !self.overlay_active(),
            Kind::Mouse => self.mouse.is_some(),
            // only when the VBS state could be read (it is HKLM; nothing to compare otherwise)
            Kind::Vbs => self.vbs_status.is_some(),
            _ => true,
        }
    }

    /// Why a row cannot be changed.
    pub fn blocked_reason(&self, kind: Kind) -> &'static str {
        match kind {
            Kind::Epp if self.overlay_active() => "Settings > Power mode is not 'Balanced' - it overrides this value. Set Power mode back to Balanced to use it.",
            Kind::Epp => "The power plan's energy preference could not be read.",
            Kind::Mouse => "The mouse settings could not be read.",
            _ => "The current value could not be read.",
        }
    }

    pub fn is_set(&self, t: &Tweak) -> bool {
        is_target(t, self.value(t.kind).as_deref())
    }
}

/// Does this value count as the tweak being applied?
pub fn is_target(t: &Tweak, v: Option<&str>) -> bool {
    match t.kind {
        // pointer precision is off whenever the speed (third number) is 0
        Kind::Mouse => v.map(|m| m.rsplit(',').next().map(|s| s.trim() == "0").unwrap_or(false)).unwrap_or(false),
        Kind::Epp => v.map(epp_number) == Some(t.target),
        _ => v == Some(t.target),
    }
}

/// "guid:25" or "25" -> "25".
pub fn epp_number(v: &str) -> &str {
    v.rsplit(':').next().unwrap_or(v).trim()
}

/// "delay|idle" -> (delay, idle); an empty part = absent.
pub fn split_pair(v: &str) -> (Option<&str>, Option<&str>) {
    let mut it = v.splitn(2, '|');
    let a = it.next().map(str::trim).filter(|s| !s.is_empty());
    let b = it.next().map(str::trim).filter(|s| !s.is_empty());
    (a, b)
}

impl State {
}

pub const READ_SCRIPT: &str = r#"
function RegV($p, $n) { try { (Get-ItemProperty -LiteralPath $p -Name $n -ErrorAction Stop).$n } catch { $null } }
# tolerant reads: one odd value (wrong type, negative DWORD) must not break the whole page
function RegS($p, $n) { $v = RegV $p $n; if ($null -eq $v) { $null } else { [string]$v } }
function RegD($p, $n) {
  $v = RegV $p $n
  if ($v -is [int]) { return [BitConverter]::ToUInt32([BitConverter]::GetBytes([int]$v), 0) }
  $u = 0; if ($null -ne $v -and [uint32]::TryParse([string]$v, [ref]$u)) { return $u }
  $null
}
function AcIndex($sub, $set) {
  $out = powercfg /qh SCHEME_CURRENT $sub $set 2>$null
  if ($LASTEXITCODE -ne 0 -or -not $out) { return $null }
  # the last two hex values are the AC and DC "Current ... Setting Index" lines (language independent)
  $hex = @([regex]::Matches(($out -join "`n"), '0x[0-9a-fA-F]+') | ForEach-Object { $_.Value })
  if ($hex.Count -lt 2) { return $null }
  [Convert]::ToUInt32($hex[$hex.Count - 2], 16)
}
Add-Type -Namespace PCTRead -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SystemParametersInfo(uint a, uint u, ref int p, uint f);
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct DEVMODE {
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
  public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra; public int dmFields;
  public int dmPositionX, dmPositionY; public int dmDisplayOrientation, dmDisplayFixedOutput;
  public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
  public short dmLogPixels; public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
  public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2, dmPanningWidth, dmPanningHeight; }
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettings(string dev, int mode, ref DEVMODE dm);
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct DISPLAY_DEVICE {
  public int cb;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string DeviceName;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string DeviceString;
  public int StateFlags;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string DeviceID;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string DeviceKey; }
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplayDevices(string dev, uint i, ref DISPLAY_DEVICE dd, uint flags);
'@
$o = [ordered]@{}
$o.dx = RegS 'HKCU:\Software\Microsoft\DirectX\UserGpuPreferences' 'DirectXUserGlobalSettings'
$o.menu = RegS 'HKCU:\Control Panel\Desktop' 'MenuShowDelay'
$o.hover = RegS 'HKCU:\Control Panel\Mouse' 'MouseHoverTime'
$t1 = RegS 'HKCU:\Control Panel\Mouse' 'MouseThreshold1'; $t2 = RegS 'HKCU:\Control Panel\Mouse' 'MouseThreshold2'; $sp = RegS 'HKCU:\Control Panel\Mouse' 'MouseSpeed'
$o.mouse = if ($null -ne $sp) { '{0},{1},{2}' -f $t1, $t2, $sp } else { $null }
$adv = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced'
$o.taskanim = RegD $adv 'TaskbarAnimations'
$o.thumbhover = RegD $adv 'ExtendedUIHoverTime'
# "delay|idle", an empty part = that value is absent (both are restored separately on undo)
$ser = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize'
$sd = RegD $ser 'StartupDelayInMSec'; $si = RegD $ser 'WaitForIdleState'
$o.startdelay = if ($null -eq $sd -and $null -eq $si) { $null } else { '{0}|{1}' -f $sd, $si }
$o.classicmenu = Test-Path -LiteralPath 'HKCU:\Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32'
# EPP is changed on the active plan; remember which plan, so undo goes back to the same one
$sch = (powercfg /getactivescheme) -join ' '
$o.scheme = if ($sch -match '([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})') { $matches[1].ToLower() } else { '' }
$o.epp = AcIndex '54533251-82be-4824-96c1-47b60b740d00' '36687f9e-e3a5-4dbf-b1dc-15eb381c6863'
# Settings > Power mode other than Balanced puts its own EPP on top of the plan
$o.overlay = [string](RegS 'HKLM:\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes' 'ActiveOverlayAcPowerScheme')
$o.hibernate = RegD 'HKLM:\SYSTEM\CurrentControlSet\Control\Power' 'HibernateEnabled'
if ($null -eq $o.hibernate) { $o.hibernate = RegD 'HKLM:\SYSTEM\CurrentControlSet\Control\Power' 'HibernateEnabledDefault' }
# list the folder instead of opening the (locked) file
$hf = Get-ChildItem -LiteralPath 'C:\' -Force -Filter 'hiberfil.sys' -ErrorAction SilentlyContinue | Select-Object -First 1
$o.hiberfil_gb = if ($hf) { [math]::Round($hf.Length / 1GB, 1) } else { $null }
$dgk = 'HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard'
$v1 = RegD $dgk 'EnableVirtualizationBasedSecurity'; $v2 = RegD "$dgk\Scenarios\WindowsHello" 'Enabled'
$o.vbs = '{0}|{1}' -f $v1, $v2
$bgk = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\AppPrivacy'
$bp = RegD $bgk 'LetAppsRunInBackground'; $ba = RegV $bgk 'LetAppsRunInBackground_ForceAllowTheseApps'
$o.bgapps = '{0}|{1}' -f $bp, ((@($ba) | Where-Object { $_ }) -join ';')
try { $o.vbs_status = [uint32](Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard -ErrorAction Stop).VirtualizationBasedSecurityStatus } catch { $o.vbs_status = $null }
$o.game_mode = RegD 'HKCU:\Software\Microsoft\GameBar' 'AutoGameModeEnabled'
$o.hags = RegD 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers' 'HwSchMode'
$o.app_capture = RegD 'HKCU:\Software\Microsoft\Windows\CurrentVersion\GameDVR' 'AppCaptureEnabled'
$o.gamedvr = RegD 'HKCU:\System\GameConfigStore' 'GameDVR_Enabled'
$o.transparency = RegD 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' 'EnableTransparency'
$o.min_animate = RegS 'HKCU:\Control Panel\Desktop\WindowMetrics' 'MinAnimate'
$ca = 0; $o.client_anim = if ([PCTRead.Native]::SystemParametersInfo(0x1042, 0, [ref]$ca, 0)) { $ca -ne 0 } else { $null }
$o.usb_suspend = AcIndex '2a737441-1930-4402-8d77-b2bebba308a3' '48e6b7a6-50f5-4782-a5d4-53bb8f07e226'
$o.pcie_aspm = AcIndex '501a4d13-42af-4429-9fd1-a8218c268e20' 'ee12f906-d277-404b-b6da-e5fa1a576df5'
$dm = New-Object PCTRead.Native+DEVMODE; $dm.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($dm)
# [NullString]::Value: PowerShell would pass $null as "" (no such display)
$nodev = [NullString]::Value
# the primary display (attached 0x1 + primary 0x4): its refresh rate and its monitor come from one device
$disp = $nodev; $o.display_id = ''
$dd = New-Object PCTRead.Native+DISPLAY_DEVICE; $dd.cb = [Runtime.InteropServices.Marshal]::SizeOf($dd)
for ($a = 0; $a -lt 64 -and [PCTRead.Native]::EnumDisplayDevices($nodev, [uint32]$a, [ref]$dd, 0); $a++) {
  if (($dd.StateFlags -band 5) -eq 5) { $disp = $dd.DeviceName; break }
}
if ($disp -is [string] -and $disp) {
  $md = New-Object PCTRead.Native+DISPLAY_DEVICE; $md.cb = $dd.cb
  # EDD_GET_DEVICE_INTERFACE_NAME = 1; the first active (0x1) monitor on that output
  for ($j = 0; $j -lt 16 -and [PCTRead.Native]::EnumDisplayDevices($disp, [uint32]$j, [ref]$md, 1); $j++) {
    if ($md.StateFlags -band 1) { $o.display_id = [string]$md.DeviceID; break }
  }
}
if ([PCTRead.Native]::EnumDisplaySettings($disp, -1, [ref]$dm)) {
  $o.hz_now = $dm.dmDisplayFrequency; $o.width = $dm.dmPelsWidth; $o.height = $dm.dmPelsHeight
  $max = 0; $i = 0; $m = New-Object PCTRead.Native+DEVMODE; $m.dmSize = $dm.dmSize
  while ([PCTRead.Native]::EnumDisplaySettings($disp, $i, [ref]$m)) {
    if ($m.dmPelsWidth -eq $dm.dmPelsWidth -and $m.dmPelsHeight -eq $dm.dmPelsHeight -and $m.dmDisplayFrequency -gt $max) { $max = $m.dmDisplayFrequency }
    $i++; if ($i -gt 2000) { break }
  }
  $o.hz_max = $max
}
# every monitor; Rust picks the one whose InstanceName matches display_id
$o.monitors = @()
try {
  $o.monitors = @(Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorID -ErrorAction Stop | ForEach-Object {
    [pscustomobject]@{ instance = [string]$_.InstanceName; active = [bool]$_.Active; name = (($_.UserFriendlyName | Where-Object { $_ -ne 0 } | ForEach-Object { [char]$_ }) -join '').Trim() }
  })
} catch { }
[pscustomobject]$o
"#;

pub fn read_state() -> State {
    match common::ps_json::<State>(READ_SCRIPT) {
        Ok(v) => {
            let mut s = v.into_iter().next().unwrap_or_else(|| State { error: Some("The settings could not be read.".into()), ..Default::default() });
            s.monitor = pick_monitor(&s.display_id, &s.monitors);
            s
        }
        Err(e) => State { error: Some(format!("The settings could not be read: {e}")), ..Default::default() },
    }
}

// ------------------------------------------------------------------ changes

/// Human-readable value.
pub fn fmt(kind: Kind, v: Option<&str>) -> String {
    match (kind, v) {
        (Kind::DxPair(_), Some("1")) => "On".into(),
        (Kind::DxPair(_), Some("0")) => "Off".into(),
        (Kind::DxPair("SwapEffectUpgradeEnable"), None) => "Windows default".into(),
        (Kind::DxPair(_), None) => "Off (default)".into(),
        (Kind::MenuDelay | Kind::HoverTime, Some(ms)) => format!("{ms} ms"),
        (Kind::MenuDelay | Kind::HoverTime, None) => "400 ms (default)".into(),
        (Kind::Mouse, Some(m)) if m.rsplit(',').next().map(|s| s.trim() == "0").unwrap_or(false) => "Off".into(),
        (Kind::Mouse, Some(_)) => "On".into(),
        (Kind::Mouse, None) => "On (default)".into(),
        (Kind::AdvDword("TaskbarAnimations"), Some("0")) => "Off".into(),
        (Kind::AdvDword("TaskbarAnimations"), Some(_)) => "On".into(),
        (Kind::AdvDword("TaskbarAnimations"), None) => "On (default)".into(),
        (Kind::AdvDword(_), Some(ms)) => format!("{ms} ms"),
        (Kind::AdvDword(_), None) => "About 400 ms (default)".into(),
        (Kind::StartupDelay, Some(p)) => match split_pair(p).0 {
            Some("0") => "No delay".into(),
            Some(ms) => format!("{ms} ms"),
            None => "A few seconds (default)".into(),
        },
        (Kind::StartupDelay, None) => "A few seconds (default)".into(),
        (Kind::ClassicMenu, Some(_)) => "Classic menu".into(),
        (Kind::ClassicMenu, None) => "Windows 11 menu".into(),
        (Kind::Epp, Some(v)) => epp_number(v).to_string(),
        (Kind::Epp, None) => "-".into(),
        (Kind::Vbs, Some(p)) => match split_pair(p) {
            (Some("0"), Some("0") | None) => "Off".into(),
            _ => "On".into(),
        },
        (Kind::Vbs, None) => "On (default)".into(),
        (Kind::BgApps, Some(p)) => match split_pair(p).0 {
            Some("2") => "Off (Xbox apps allowed)".into(),
            Some("1") => "Always on (policy)".into(),
            _ => "On (Windows default)".into(),
        },
        (Kind::BgApps, None) => "On (Windows default)".into(),
        (Kind::Hibernate, Some("0")) => "Off".into(),
        (Kind::Hibernate, _) => "On".into(),
        (_, Some(x)) => x.to_string(),
    }
}

/// PowerShell that sets `kind` to `value` (`None` = back to the Windows default / absent).
pub fn set_stmt(kind: Kind, value: Option<&str>) -> String {
    let q = common::psq;
    let num = |v: Option<&str>, default: u32| v.and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(default);
    match kind {
        Kind::DxPair(key) => {
            let action = match value {
                Some(v) => format!("$pairs[{}] = {}", q(key), q(v)),
                None => format!("$pairs.Remove({})", q(key)),
            };
            format!(
                "$k = {k}; if (-not (Test-Path -LiteralPath $k)) {{ New-Item -Path $k -Force | Out-Null }}; \
$s = [string](Get-ItemProperty -LiteralPath $k -Name DirectXUserGlobalSettings -ErrorAction SilentlyContinue).DirectXUserGlobalSettings; \
$pairs = [ordered]@{{}}; foreach ($p in $s.Split(';')) {{ if ($p -match '^\\s*([^=]+?)\\s*=\\s*(.*?)\\s*$') {{ $pairs[$matches[1]] = $matches[2] }} }}; \
{action}; \
if ($pairs.Count -eq 0) {{ Remove-ItemProperty -LiteralPath $k -Name DirectXUserGlobalSettings -ErrorAction SilentlyContinue }} \
else {{ Set-ItemProperty -LiteralPath $k -Name DirectXUserGlobalSettings -Type String -Value ((@($pairs.Keys) | ForEach-Object {{ \"$_=$($pairs[$_]);\" }}) -join '') -ErrorAction Stop }}",
                k = q(DX_KEY)
            )
        }
        Kind::MenuDelay => format!("if (-not [PCT.Spi]::SystemParametersInfo(0x006B, {}, [IntPtr]::Zero, 3)) {{ throw 'SystemParametersInfo failed' }}", num(value, 400)),
        Kind::HoverTime => format!("if (-not [PCT.Spi]::SystemParametersInfo(0x0067, {}, [IntPtr]::Zero, 3)) {{ throw 'SystemParametersInfo failed' }}", num(value, 400)),
        Kind::Mouse => {
            let triple: Vec<i32> = value.unwrap_or("6,10,1").split(',').map(|s| s.trim().parse::<i32>().unwrap_or(0)).collect();
            let t = if triple.len() == 3 { triple } else { vec![6, 10, 1] };
            format!("if (-not [PCT.Spi]::SystemParametersInfo(0x0004, 0, [int[]]({}, {}, {}), 3)) {{ throw 'SystemParametersInfo failed' }}", t[0], t[1], t[2])
        }
        Kind::AdvDword(name) => match value {
            Some(v) => format!("Set-ItemProperty -LiteralPath {} -Name {} -Type DWord -Value {} -ErrorAction Stop", q(ADV_KEY), q(name), num(Some(v), 0)),
            None => format!("Remove-ItemProperty -LiteralPath {} -Name {} -ErrorAction SilentlyContinue", q(ADV_KEY), q(name)),
        },
        Kind::StartupDelay => {
            // "delay|idle": each value is set, or removed when its part is empty
            let (delay, idle) = value.map(split_pair).unwrap_or((None, None));
            let one = |name: &str, v: Option<&str>| match v {
                Some(n) => format!("Set-ItemProperty -LiteralPath $k -Name {name} -Type DWord -Value {} -ErrorAction Stop", num(Some(n), 0)),
                None => format!("Remove-ItemProperty -LiteralPath $k -Name {name} -ErrorAction SilentlyContinue"),
            };
            format!(
                "$k = {k}; if (-not (Test-Path -LiteralPath $k)) {{ New-Item -Path $k -Force | Out-Null }}; {a}; {b}; \
$left = Get-Item -LiteralPath $k -ErrorAction SilentlyContinue; if ($left -and $left.ValueCount -eq 0 -and $left.SubKeyCount -eq 0) {{ Remove-Item -LiteralPath $k -ErrorAction SilentlyContinue }}",
                k = q(SERIALIZE_KEY),
                a = one("StartupDelayInMSec", delay),
                b = one("WaitForIdleState", idle),
            )
        }
        Kind::ClassicMenu => match value {
            Some(_) => format!(
                "$k = {}; if (-not (Test-Path -LiteralPath $k)) {{ New-Item -Path $k -Force | Out-Null }}; Set-Item -LiteralPath $k -Value '' -ErrorAction Stop",
                q(&format!(r"{CLASSIC_MENU_KEY}\InprocServer32"))
            ),
            // remove only what the tweak adds; the CLSID key itself only when nothing else is left in it
            None => format!(
                "$p = {p}; Remove-Item -LiteralPath \"$p\\InprocServer32\" -Recurse -Force -ErrorAction SilentlyContinue; \
$left = Get-Item -LiteralPath $p -ErrorAction SilentlyContinue; if ($left -and $left.ValueCount -eq 0 -and $left.SubKeyCount -eq 0) {{ Remove-Item -LiteralPath $p -ErrorAction SilentlyContinue }}",
                p = q(CLASSIC_MENU_KEY)
            ),
        },
        Kind::Epp => {
            // "plan-guid:value" (undo: the plan that was changed) or "value" (apply: the active plan)
            let v = value.unwrap_or("25");
            let n = epp_number(v).parse::<u32>().unwrap_or(25).min(100);
            let guid = v.rsplit_once(':').map(|(g, _)| g.trim()).filter(|g| g.len() == 36 && g.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            let plan = guid.unwrap_or("SCHEME_CURRENT");
            let activate = match guid {
                // re-apply only if that plan is still the active one (never switch the user's plan)
                Some(g) => format!("if (((powercfg /getactivescheme) -join ' ') -match '{g}') {{ $o = powercfg /setactive {g}; if ($LASTEXITCODE -ne 0) {{ throw (($o | Out-String).Trim()) }} }}"),
                None => "$o = powercfg /setactive SCHEME_CURRENT; if ($LASTEXITCODE -ne 0) { throw (($o | Out-String).Trim()) }".to_string(),
            };
            format!("$o = powercfg /setacvalueindex {plan} {SUB_PROCESSOR} {PERFEPP} {n}; if ($LASTEXITCODE -ne 0) {{ throw (($o | Out-String).Trim()) }}; {activate}")
        }
        Kind::Hibernate => {
            let onoff = if value == Some("0") { "off" } else { "on" };
            format!("$o = powercfg /hibernate {onoff}; if ($LASTEXITCODE -ne 0) {{ throw (($o | Out-String).Trim()) }}")
        }
        Kind::Vbs => {
            // "vbs|hello": each value set, or removed when its part is empty (exact undo)
            let (vbs, hello) = value.map(split_pair).unwrap_or((None, None));
            let one = |key: &str, name: &str, v: Option<&str>| match v {
                Some(n) => format!(
                    "$k = {k}; if (-not (Test-Path -LiteralPath $k)) {{ New-Item -Path $k -Force | Out-Null }}; Set-ItemProperty -LiteralPath $k -Name {name} -Type DWord -Value {n} -ErrorAction Stop",
                    k = q(key),
                    n = num(Some(n), 0)
                ),
                None => format!("Remove-ItemProperty -LiteralPath {} -Name {name} -ErrorAction SilentlyContinue", q(key)),
            };
            format!(
                "{}; {}",
                one(VBS_KEY, "EnableVirtualizationBasedSecurity", vbs),
                one(&format!(r"{VBS_KEY}\Scenarios\WindowsHello"), "Enabled", hello)
            )
        }
        Kind::BgApps => {
            // "policy|app;app": each value set, or removed when its part is empty (exact undo)
            let (pol, apps) = value.map(split_pair).unwrap_or((None, None));
            let p = match pol {
                Some(n) => format!("Set-ItemProperty -LiteralPath $k -Name LetAppsRunInBackground -Type DWord -Value {} -ErrorAction Stop", num(Some(n), 0)),
                None => "Remove-ItemProperty -LiteralPath $k -Name LetAppsRunInBackground -ErrorAction SilentlyContinue".to_string(),
            };
            let a = match apps {
                Some(list) => {
                    let items: Vec<String> = list.split(';').map(str::trim).filter(|s| !s.is_empty()).map(q).collect();
                    format!("Set-ItemProperty -LiteralPath $k -Name LetAppsRunInBackground_ForceAllowTheseApps -Type MultiString -Value @({}) -ErrorAction Stop", items.join(", "))
                }
                None => "Remove-ItemProperty -LiteralPath $k -Name LetAppsRunInBackground_ForceAllowTheseApps -ErrorAction SilentlyContinue".to_string(),
            };
            format!(
                "$k = {k}; if (-not (Test-Path -LiteralPath $k)) {{ New-Item -Path $k -Force | Out-Null }}; {p}; {a}; \
$left = Get-Item -LiteralPath $k -ErrorAction SilentlyContinue; if ($left -and $left.ValueCount -eq 0 -and $left.SubKeyCount -eq 0) {{ Remove-Item -LiteralPath $k -ErrorAction SilentlyContinue }}",
                k = q(BG_KEY)
            )
        }
    }
}

pub const VBS_KEY: &str = r"HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard";
pub const BG_KEY: &str = r"HKLM:\SOFTWARE\Policies\Microsoft\Windows\AppPrivacy";
/// Force Deny (2), with the Xbox / gaming / Store apps allowed to keep running in the background.
pub const BG_TARGET: &str = "2|Microsoft.GamingApp_8wekyb3d8bbwe;Microsoft.XboxGamingOverlay_8wekyb3d8bbwe;Microsoft.XboxIdentityProvider_8wekyb3d8bbwe;Microsoft.Xbox.TCUI_8wekyb3d8bbwe;Microsoft.XboxSpeechToTextOverlay_8wekyb3d8bbwe;Microsoft.GamingServices_8wekyb3d8bbwe;Microsoft.WindowsStore_8wekyb3d8bbwe";

pub const SPI_TYPE: &str = "Add-Type -Namespace PCT -Name Spi -MemberDefinition '[DllImport(\"user32.dll\", SetLastError = true)] public static extern bool SystemParametersInfo(uint a, uint u, IntPtr p, uint f); [DllImport(\"user32.dll\", SetLastError = true)] public static extern bool SystemParametersInfo(uint a, uint u, int[] p, uint f);'\n";

/// One script for a batch: one RESULT line per change.
pub fn build_script(changes: &[(String, Kind, Option<String>)]) -> String {
    let mut s = String::new();
    if changes.iter().any(|c| matches!(c.1, Kind::MenuDelay | Kind::HoverTime | Kind::Mouse)) {
        s.push_str(SPI_TYPE);
    }
    for (id, kind, value) in changes {
        let stmt = set_stmt(*kind, value.as_deref());
        s.push_str(&format!(
            "try {{ {stmt}; Write-Output 'RESULT|{id}|ok|' }} catch {{ Write-Output ('RESULT|{id}|err|' + ($_.Exception.Message -replace '\\s+', ' ')) }}\nStart-Sleep -Milliseconds 120\n"
        ));
    }
    s
}

/// "RESULT|id|ok|note" / "RESULT|id|err|message" -> (id, result).
pub fn parse_result(line: &str) -> Option<(String, Result<String, String>)> {
    let rest = line.trim().strip_prefix("RESULT|")?;
    let mut p = rest.splitn(3, '|');
    let id = p.next()?.to_string();
    let status = p.next()?;
    let msg = p.next().unwrap_or("").trim().to_string();
    Some((id, if status == "ok" { Ok(msg) } else { Err(if msg.is_empty() { "Failed.".into() } else { msg }) }))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Change {
    pub id: String,
    pub prev: Option<String>,
    pub new: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct HistEntry {
    pub time: String,
    pub changes: Vec<Change>,
    pub undone: bool,
}

// ------------------------------------------------------------------ page state

/// A change being applied: the script's output stream and what it is changing.
pub struct Run {
    pub stream: Stream,
    pub seen: usize,
    pub changes: Vec<Change>,
    pub undo_of: Option<usize>,
}

/// The Performance page's state: what was read, what is selected, the running change and its window.
pub struct Perf {
    pub state: Option<State>,
    pub loading: Option<Job<State>>,
    pub selected: Vec<bool>,
    pub history: Vec<HistEntry>,
    pub op: Option<OpWindow>,
    pub pending: Option<(Vec<Change>, Option<usize>)>,
    pub run: Option<Run>,
    pub explorer: Option<Job<Result<String, String>>>,
    /// `op` is the "Restart Explorer" preview, not a settings change
    pub explorer_preview: bool,
    pub demo: bool,
    pub note: Option<String>,
}

/// Preview window for restarting Explorer (same wording as the Startup tab).
pub fn explorer_window() -> OpWindow {
    let items = vec![OpItem::new("explorer", "Windows Explorer", "Explorer").detail("taskbar, desktop and folder windows")];
    let mut win = OpWindow::new("Restart Explorer", "restart", "restarted", items).noun("program", "programs");
    win.preview_text = "Shows taskbar and right-click changes.".into();
    win.warning = Some("The taskbar blinks and open folder windows close. Not during a game.".into());
    win.go_label = "Restart now".into();
    win.running_text = "Restarting Explorer...".into();
    win
}

impl Default for Perf {
    fn default() -> Self {
        Self::new()
    }
}

impl Perf {
    pub fn new() -> Self {
        let mut tab = Perf {
            state: None,
            loading: Some(Job::spawn(read_state)),
            // nothing is pre-selected: the user clicks the settings they want
            selected: vec![false; TWEAKS.len()],
            history: common::load_json(HISTORY),
            op: None,
            pending: None,
            run: None,
            explorer: None,
            explorer_preview: false,
            demo: false,
            note: None,
        };
        if let Some(stage) = opwin::demo_stage("perf") {
            let changes = vec![
                Change { id: "windowed".into(), prev: None, new: Some("1".into()) },
                Change { id: "menu".into(), prev: Some("400".into()), new: Some("100".into()) },
                Change { id: "taskanim".into(), prev: None, new: Some("0".into()) },
            ];
            let mut w = Self::op_window(&changes, false);
            match stage {
                Stage::Preview => {}
                Stage::Running => {
                    w.stage = Stage::Running;
                    w.items[0].result = Some(Ok(String::new()));
                }
                Stage::Done => {
                    w.can_undo = true;
                    w.finish(|_| Some(Ok(String::new())));
                }
            }
            tab.op = Some(w);
            tab.demo = true;
        }
        tab
    }

    pub fn refresh(&mut self) {
        if self.loading.is_none() {
            self.loading = Some(Job::spawn(read_state));
        }
    }

    pub fn op_window(changes: &[Change], undo: bool) -> OpWindow {
        let mut items = vec![];
        for c in changes {
            let Some(t) = tweak(&c.id) else { continue };
            items.push(
                OpItem::new(t.id, t.name, t.group)
                    .detail(format!("{} -> {}", fmt(t.kind, c.prev.as_deref()), fmt(t.kind, c.new.as_deref())))
                    .optional(t.optional),
            );
        }
        let (kicker, verb, done) = if undo { ("UNDO PERFORMANCE CHANGE", "undo", "undone") } else { ("PERFORMANCE", "change", "changed") };
        let mut w = OpWindow::new(kicker, verb, done, items).noun("setting", "settings");
        w.show_steps = true;
        w.preview_text = "Each change is saved so it can be undone.".into();
        w.running_text = if undo { "Putting the old settings back...".into() } else { "Applying...".into() };
        w.go_label = if undo { "Undo".into() } else { "Apply".into() };
        w.reassure = if undo { "Restores the values from before that change.".into() } else { "\"Undo last change\" puts every value back.".into() };
        let kinds: Vec<Kind> = changes.iter().filter_map(|c| tweak(&c.id).map(|t| t.kind)).collect();
        if kinds.iter().any(|k| matches!(k, Kind::Hibernate)) {
            w.warning = Some("Hibernation changes delete or re-create C:\\hiberfil.sys.".into());
        }
        if kinds.iter().any(|k| matches!(k, Kind::Vbs)) {
            w.warning = Some("VBS off lowers Windows security (Windows Hello enhanced sign-in and Memory Integrity stop working). Only the restart applies it.".into());
            w.notes.push("Restart the PC for the VBS change to take effect. System Information then shows 'Virtualization-based security: Not enabled'.".into());
        }
        if kinds.iter().any(|k| k.needs_explorer()) {
            w.notes.push("Taskbar and right-click changes show after Explorer restarts (or after signing out).".into());
            w.extras.push("Restart Explorer".into());
        }
        if kinds.iter().any(|k| matches!(k, Kind::DxPair(_))) {
            w.notes.push("Game settings apply the next time a game starts.".into());
        }
        if kinds.iter().any(|k| matches!(k, Kind::StartupDelay)) {
            w.notes.push("The startup-app delay changes at your next sign-in.".into());
        }
        w
    }

    /// "Apply selected": the selected settings that are not applied yet get their value.
    pub fn open_apply(&mut self) {
        let Some(state) = self.state.as_ref().filter(|s| s.error.is_none()) else { return };
        let changes = selected_changes(state, &self.selected);
        if changes.is_empty() {
            return;
        }
        self.op = Some(Self::op_window(&changes, false));
        self.pending = Some((changes, None));
    }

    /// Restore a backup: the settings that differ from it get their backed-up value, through the
    /// normal preview window (recorded in the history, so "Undo last change" reverts the restore).
    pub fn open_restore(&mut self, b: &tabbackup::Backup) -> Result<(), String> {
        if self.run.is_some() || self.op.is_some() {
            return Err("Another change is still open - finish it first.".into());
        }
        let Some(state) = self.state.as_ref().filter(|s| s.error.is_none()) else { return Err("The settings are still being read.".into()) };
        let changes: Vec<Change> = tabbackup::differences(b, &backup_items(state))
            .into_iter()
            .map(|d| Change { id: d.backup.id, prev: d.now.value, new: d.backup.value })
            .collect();
        if changes.is_empty() {
            return Err("Nothing to restore - every setting already matches this backup.".into());
        }
        let mut w = Self::op_window(&changes, false);
        w.kicker = "RESTORE PERFORMANCE BACKUP".into();
        w.preview_text = format!("Puts these settings back to the backup from {}. Each change is saved so it can be undone.", b.time);
        self.op = Some(w);
        self.pending = Some((changes, None));
        Ok(())
    }

    /// The value a setting goes back to when it is switched off: what it was before PC Tuner's newest
    /// change that switched it on, or the Windows default (None) when PC Tuner never did.
    pub fn off_value(&self, id: &str) -> Option<String> {
        let t = tweak(id)?;
        self.history
            .iter()
            .rev()
            .filter(|h| !h.undone)
            .flat_map(|h| h.changes.iter())
            .find(|c| c.id == id && is_target(t, c.new.as_deref()))
            .and_then(|c| c.prev.clone())
            .filter(|p| !is_target(t, Some(p)))
    }

    /// The switch on a row: one change (on = the tweak's value, off = `off_value`) through the usual
    /// preview. Recorded like any change, so "Undo last change" reverts it too.
    pub fn open_switch(&mut self, i: usize, on: bool) {
        if self.run.is_some() || self.op.is_some() {
            return;
        }
        let (Some(state), Some(t)) = (self.state.as_ref().filter(|s| s.error.is_none()), TWEAKS.get(i)) else { return };
        if !state.applicable(t.kind) || state.is_set(t) == on {
            return;
        }
        let new = if on { Some(t.target.to_string()) } else { self.off_value(t.id) };
        let changes = vec![Change { id: t.id.into(), prev: state.value(t.kind), new }];
        let mut w = Self::op_window(&changes, false);
        if !on {
            w.kicker = "SWITCH OFF".into();
            w.go_label = "Switch off".into();
            w.preview_text = "Puts back the value from before PC Tuner changed it (or the Windows default). Saved, so \"Undo last change\" switches it on again.".into();
        }
        self.selected[i] = false;
        self.op = Some(w);
        self.pending = Some((changes, None));
    }

    pub fn undo_changes(&self) -> Option<(Vec<Change>, usize)> {
        let i = self.history.iter().rposition(|h| !h.undone)?;
        let changes = self.history[i].changes.iter().rev().map(|c| Change { id: c.id.clone(), prev: c.new.clone(), new: c.prev.clone() }).collect();
        Some((changes, i))
    }

    pub fn open_undo(&mut self, start_now: bool) {
        let Some((changes, i)) = self.undo_changes() else { return };
        self.op = Some(Self::op_window(&changes, true));
        if start_now {
            self.start(changes, Some(i));
        } else {
            self.pending = Some((changes, Some(i)));
        }
    }

    pub fn start(&mut self, changes: Vec<Change>, undo_of: Option<usize>) {
        let batch: Vec<(String, Kind, Option<String>)> = changes.iter().filter_map(|c| tweak(&c.id).map(|t| (c.id.clone(), t.kind, c.new.clone()))).collect();
        self.run = Some(Run { stream: common::ps_stream(&build_script(&batch)), seen: 0, changes, undo_of });
        if let Some(op) = &mut self.op {
            op.stage = Stage::Running;
        }
    }

    pub fn pump(&mut self) {
        if let Some(j) = &self.loading {
            if let Some(s) = j.poll() {
                self.state = Some(s);
                self.loading = None;
            }
        }
        if let Some(j) = &self.explorer {
            if let Some(r) = j.poll() {
                self.explorer = None;
                self.note = Some(match r {
                    Ok(_) => "Explorer restarted.".into(),
                    Err(e) => format!("Explorer could not be restarted: {e}"),
                });
                self.refresh();
            }
        }
        let Some(run) = &mut self.run else { return };
        let running = run.stream.pump();
        while run.seen < run.stream.lines.len() {
            let line = run.stream.lines[run.seen].clone();
            run.seen += 1;
            if let (Some((id, r)), Some(op)) = (parse_result(&line), &mut self.op) {
                if let Some(it) = op.items.iter_mut().find(|i| i.id == id) {
                    it.result = Some(r);
                }
            }
        }
        if running {
            return;
        }
        let run = self.run.take().unwrap();
        let code = run.stream.exit.unwrap_or(-1);
        let mut ok_ids = vec![];
        if let Some(op) = &mut self.op {
            for it in op.items.iter_mut() {
                if it.result.is_none() {
                    let tail = run.stream.lines.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default();
                    it.result = Some(Err(format!("No result - the script stopped early (exit code {code}). {tail}").trim().to_string()));
                }
                if matches!(it.result, Some(Ok(_))) {
                    ok_ids.push(it.id.clone());
                }
            }
        }
        let mut can_undo = false;
        match run.undo_of {
            None => {
                let changes: Vec<Change> = run.changes.into_iter().filter(|c| ok_ids.contains(&c.id)).collect();
                if !changes.is_empty() {
                    self.history.push(HistEntry { time: common::now(), changes, undone: false });
                    common::save_json(HISTORY, &self.history);
                    can_undo = true;
                }
            }
            Some(i) => {
                // only what was really put back leaves the history; failed parts can be undone again
                if let Some(h) = self.history.get_mut(i) {
                    h.changes.retain(|c| !ok_ids.contains(&c.id));
                    h.undone = h.changes.is_empty();
                    can_undo = !h.undone;
                }
                common::save_json(HISTORY, &self.history);
            }
        }
        if let Some(op) = &mut self.op {
            op.can_undo = can_undo;
            op.done();
        }
        self.selected = vec![false; TWEAKS.len()];
        self.refresh();
    }

    /// May the window's buttons be pressed now? Restarting Explorer never needed admin; settings changes do.
    pub fn can_act(&self) -> bool {
        if self.explorer_preview {
            self.run.is_none() && self.explorer.is_none() && !self.demo
        } else {
            common::is_admin() && self.run.is_none() && !self.demo
        }
    }

    /// React to a button of the operation window (`can` = `can_act()` when the window was drawn).
    pub fn on_event(&mut self, ev: OpEvent, can: bool) {
        match ev {
            OpEvent::Go if self.explorer_preview => {
                if can {
                    self.restart_explorer();
                }
                self.close_window();
            }
            OpEvent::Go => {
                if let Some((changes, undo_of)) = self.pending.take() {
                    self.start(changes, undo_of);
                }
            }
            OpEvent::Undo => self.open_undo(true),
            OpEvent::Extra(_) => self.open_explorer_preview(),
            OpEvent::Close => self.close_window(),
        }
    }

    /// True while a settings change is being applied.
    pub fn busy(&self) -> bool {
        self.run.is_some()
    }

    pub fn close_window(&mut self) {
        self.op = None;
        self.pending = None;
        self.explorer_preview = false;
    }

    /// Restart Explorer: shows a preview first (open folder windows close).
    pub fn open_explorer_preview(&mut self) {
        self.op = Some(explorer_window());
        self.pending = None;
        self.explorer_preview = true;
    }

    pub fn restart_explorer(&mut self) {
        if self.explorer.is_none() && !self.demo {
            self.note = Some("Restarting Explorer...".into());
            self.explorer = Some(Job::spawn(|| common::ps(crate::scripts::EXPLORER_RESTART)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_switch_off_puts_back_the_value_from_before() {
        let st = State { menu: Some("100".into()), taskanim: Some(0), ..Default::default() };
        let hist = vec![
            HistEntry { time: "a".into(), changes: vec![Change { id: "menu".into(), prev: Some("250".into()), new: Some("100".into()) }], undone: false },
            HistEntry { time: "b".into(), changes: vec![Change { id: "taskanim".into(), prev: None, new: Some("0".into()) }], undone: true },
        ];
        let mut p = Perf { state: Some(st), loading: None, selected: vec![false; TWEAKS.len()], history: hist, op: None, pending: None, run: None, explorer: None, explorer_preview: false, demo: false, note: None };
        assert_eq!(p.off_value("menu").as_deref(), Some("250"), "the value PC Tuner replaced");
        assert_eq!(p.off_value("taskanim"), None, "an undone change does not count: the Windows default");
        let menu = TWEAKS.iter().position(|t| t.id == "menu").unwrap();
        p.open_switch(menu, true);
        assert!(p.op.is_none(), "already on: nothing to do");
        p.open_switch(menu, false);
        let (c, undo_of) = p.pending.clone().unwrap();
        assert_eq!((c[0].prev.as_deref(), c[0].new.as_deref(), undo_of), (Some("100"), Some("250"), None));
        assert_eq!(p.op.as_ref().unwrap().go_label, "Switch off");
        let mut on = Perf { state: Some(State::default()), op: None, pending: None, ..p };
        let hover = TWEAKS.iter().position(|t| t.id == "hover").unwrap();
        on.open_switch(hover, true);
        assert_eq!(on.pending.unwrap().0[0].new.as_deref(), Some("200"));
        assert!(is_target(tweak("mouseaccel").unwrap(), Some("0,0,0")) && !is_target(tweak("mouseaccel").unwrap(), Some("6,10,1")));
    }

    #[test]
    fn perf_backup_restore_opens_the_preview() {
        let now = State { menu: Some("100".into()), taskanim: Some(0), ..Default::default() };
        let items = backup_items(&now);
        assert!(items.iter().any(|i| i.id == "menu" && i.value.as_deref() == Some("100") && i.shown == fmt(Kind::MenuDelay, Some("100"))));
        assert!(!items.iter().any(|i| i.id == "epp" || i.id == "mouseaccel"), "settings that could not be read are not backed up");
        let b = tabbackup::Backup { time: "then".into(), items: backup_items(&State::default()), ..Default::default() };
        let mut p = Perf { state: Some(now), loading: None, selected: vec![false; TWEAKS.len()], history: vec![], op: None, pending: None, run: None, explorer: None, explorer_preview: false, demo: false, note: None };
        p.open_restore(&b).unwrap();
        let (changes, undo_of) = p.pending.clone().unwrap();
        assert!(undo_of.is_none(), "a restore is a new change, so Undo last change reverts it");
        let got: Vec<(&str, Option<&str>, Option<&str>)> = changes.iter().map(|c| (c.id.as_str(), c.prev.as_deref(), c.new.as_deref())).collect();
        assert_eq!(got, vec![("menu", Some("100"), Some("400")), ("taskanim", Some("0"), None)], "an absent value goes back to absent");
        assert_eq!(p.op.as_ref().unwrap().items.len(), 2);
        assert!(p.open_restore(&b).is_err(), "never over an open window");
        p.op = None;
        p.pending = None;
        let same = tabbackup::Backup { items: backup_items(p.state.as_ref().unwrap()), ..Default::default() };
        assert!(p.open_restore(&same).unwrap_err().starts_with("Nothing to restore"));
    }

    #[test]
    fn perf_dx_pairs() {
        assert_eq!(dx_pair(Some("VRROptimizeEnable=0;"), "VRROptimizeEnable").as_deref(), Some("0"));
        assert_eq!(dx_pair(Some("VRROptimizeEnable=0;SwapEffectUpgradeEnable=1;"), "SwapEffectUpgradeEnable").as_deref(), Some("1"));
        assert_eq!(dx_pair(Some("VRROptimizeEnable=0;"), "SwapEffectUpgradeEnable"), None);
        assert_eq!(dx_pair(None, "VRROptimizeEnable"), None);
        assert_eq!(dx_pair(Some(" swapeffectupgradeenable = 1 ;"), "SwapEffectUpgradeEnable").as_deref(), Some("1"));
    }

    #[test]
    fn perf_state_and_targets() {
        let s = State { dx: Some("VRROptimizeEnable=0;".into()), menu: Some("400".into()), mouse: Some("6,10,1".into()), epp: Some(25), hibernate: Some(1), ..Default::default() };
        let t = |id| tweak(id).unwrap();
        assert!(!s.is_set(t("windowed")));
        assert!(!s.is_set(t("vrr")));
        assert!(!s.is_set(t("menu")));
        assert!(!s.is_set(t("mouseaccel")));
        let s2 = State { mouse: Some("0,0,0".into()), menu: Some("100".into()), ..s.clone() };
        assert!(s2.is_set(t("mouseaccel")) && s2.is_set(t("menu")));
        assert_eq!(s.value(Kind::MenuDelay).as_deref(), Some("400"));
        assert_eq!(State::default().value(Kind::MenuDelay).as_deref(), Some("400"), "absent = Windows default 400 ms");
        assert!(!State::default().applicable(Kind::Epp), "EPP needs a readable value");
        // a failed read blocks every change (the "before" values would be guesses)
        let broken = State { error: Some("x".into()), ..s.clone() };
        assert!(TWEAKS.iter().all(|t| !broken.applicable(t.kind)));
        // a Power mode overlay overrides EPP
        let overlay = State { overlay: "ded574b5-45a0-4f42-8737-46345c09c238".into(), ..s.clone() };
        assert!(!overlay.applicable(Kind::Epp) && s.applicable(Kind::Epp));
        // EPP value carries the plan, "set" compares only the number
        let planned = State { scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".into(), epp: Some(10), ..s.clone() };
        assert_eq!(planned.value(Kind::Epp).as_deref(), Some("381b4222-f694-41f0-9685-ff5bb260df2e:10"));
        assert!(planned.is_set(t("epp")));
        // startup delay is "set" only when both values are 0
        assert!(State { startdelay: Some("0|0".into()), ..s.clone() }.is_set(t("startdelay")));
        assert!(!State { startdelay: Some("0|".into()), ..s.clone() }.is_set(t("startdelay")));
        assert_eq!(fmt(Kind::StartupDelay, Some("10000|1")), "10000 ms");
        // ids unique
        let mut ids: Vec<&str> = TWEAKS.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TWEAKS.len());
    }

    #[test]
    fn perf_statements() {
        let s = set_stmt(Kind::DxPair("SwapEffectUpgradeEnable"), Some("1"));
        assert!(s.contains("$pairs['SwapEffectUpgradeEnable'] = '1'") && s.contains("DirectXUserGlobalSettings"));
        assert!(set_stmt(Kind::DxPair("VRROptimizeEnable"), None).contains("$pairs.Remove('VRROptimizeEnable')"));
        assert!(set_stmt(Kind::MenuDelay, Some("100")).contains("0x006B, 100,"));
        assert!(set_stmt(Kind::MenuDelay, None).contains("0x006B, 400,"), "undo to absent = 400 ms");
        assert!(set_stmt(Kind::Mouse, Some("6,10,1")).contains("[int[]](6, 10, 1)"));
        assert!(set_stmt(Kind::Mouse, Some("garbage")).contains("[int[]](6, 10, 1)") || set_stmt(Kind::Mouse, Some("garbage")).contains("[int[]](0"), "never panics");
        assert!(set_stmt(Kind::AdvDword("TaskbarAnimations"), Some("0")).contains("-Name 'TaskbarAnimations' -Type DWord -Value 0"));
        assert!(set_stmt(Kind::AdvDword("ExtendedUIHoverTime"), None).starts_with("Remove-ItemProperty"));
        assert!(set_stmt(Kind::StartupDelay, None).contains("ValueCount -eq 0"), "the Serialize key is only removed when empty");
        assert!(set_stmt(Kind::ClassicMenu, Some("on")).contains(r"InprocServer32"));
        assert!(set_stmt(Kind::Epp, Some("10")).contains(&format!("{SUB_PROCESSOR} {PERFEPP} 10")));
        assert!(set_stmt(Kind::Epp, Some("250")).contains(" 100;"), "EPP is clamped to 100");
        assert!(set_stmt(Kind::Hibernate, Some("0")).contains("/hibernate off"));
        // VBS: both values set; undo restores each one (an empty part = remove it)
        let s = set_stmt(Kind::Vbs, Some("0|0"));
        assert!(s.contains("-Name EnableVirtualizationBasedSecurity -Type DWord -Value 0") && s.contains(r"Scenarios\WindowsHello") && s.contains("-Name Enabled -Type DWord -Value 0"));
        let s = set_stmt(Kind::Vbs, Some("1|"));
        assert!(s.contains("EnableVirtualizationBasedSecurity -Type DWord -Value 1") && s.contains("Remove-ItemProperty") && s.contains("-Name Enabled"));
        assert_eq!(fmt(Kind::Vbs, Some("1|1")), "On");
        assert_eq!(fmt(Kind::Vbs, Some("0|0")), "Off");
        // background apps: Force Deny with the Xbox apps exempt; undo removes what was absent
        let s = set_stmt(Kind::BgApps, Some(BG_TARGET));
        assert!(s.contains("LetAppsRunInBackground -Type DWord -Value 2") && s.contains("-Type MultiString -Value @('Microsoft.GamingApp_8wekyb3d8bbwe'"));
        assert!(BG_TARGET.contains("Microsoft.GamingServices_8wekyb3d8bbwe") && BG_TARGET.contains("Microsoft.XboxIdentityProvider_8wekyb3d8bbwe"));
        let s = set_stmt(Kind::BgApps, Some("|"));
        assert!(s.matches("Remove-ItemProperty").count() == 2 && s.contains("ValueCount -eq 0"));
        assert_eq!(fmt(Kind::BgApps, Some("|")), "On (Windows default)");
        assert!(set_stmt(Kind::Hibernate, None).contains("/hibernate on"));
        // EPP: apply on the active plan; undo on the plan that was changed, without switching plans
        let g = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let undo = set_stmt(Kind::Epp, Some(&format!("{g}:25")));
        assert!(undo.contains(&format!("/setacvalueindex {g} {SUB_PROCESSOR} {PERFEPP} 25")));
        assert!(undo.contains(&format!("-match '{g}'")), "re-activates only if that plan is still active");
        assert!(!set_stmt(Kind::Epp, Some("x; rm -rf:10")).contains("rm -rf"), "a bad plan id is ignored");
        // startup delay: both values set / removed separately
        let s = set_stmt(Kind::StartupDelay, Some("0|0"));
        assert!(s.contains("-Name StartupDelayInMSec -Type DWord -Value 0") && s.contains("-Name WaitForIdleState -Type DWord -Value 0"));
        let s = set_stmt(Kind::StartupDelay, Some("10000|"));
        assert!(s.contains("StartupDelayInMSec -Type DWord -Value 10000") && s.contains("Remove-ItemProperty -LiteralPath $k -Name WaitForIdleState"));
        // classic menu undo removes InprocServer32, and the CLSID key only when it is empty
        let s = set_stmt(Kind::ClassicMenu, None);
        assert!(s.contains("InprocServer32") && s.contains("ValueCount -eq 0") && !s.contains(&format!("Remove-Item -LiteralPath '{CLASSIC_MENU_KEY}' -Recurse")));
        let script = build_script(&[("menu".into(), Kind::MenuDelay, Some("100".into())), ("taskanim".into(), Kind::AdvDword("TaskbarAnimations"), Some("0".into()))]);
        assert!(script.starts_with("Add-Type") && script.contains("RESULT|menu|ok|") && script.contains("RESULT|taskanim|ok|"));
        assert!(!build_script(&[("epp".into(), Kind::Epp, Some("10".into()))]).contains("Add-Type"), "SPI type only when needed");
        assert_eq!(parse_result("RESULT|menu|err|Access is denied."), Some(("menu".into(), Err("Access is denied.".into()))));
    }

    /// Runs the real DirectX-pair and Serialize statements against a scratch key (never the real ones).
    /// `cargo test -- --ignored perf_registry_roundtrip`
    #[test]
    #[ignore]
    fn perf_registry_roundtrip() {
        const TEST: &str = r"HKCU:\Software\PCTunerSelfTest";
        let on_test = |s: String| {
            s.replace(DX_KEY, &format!(r"{TEST}\Dx")).replace(SERIALIZE_KEY, &format!(r"{TEST}\Serialize")).replace(CLASSIC_MENU_KEY, &format!(r"{TEST}\Clsid"))
        };
        let read = |name: &str, value: &str| {
            common::ps(&format!("(Get-ItemProperty -LiteralPath '{TEST}\\{name}' -Name {value} -ErrorAction SilentlyContinue).{value}")).unwrap_or_default().trim().to_string()
        };
        let run = |s: String| common::ps(&on_test(s)).expect("statement runs");
        let _ = common::ps(&format!("Remove-Item -LiteralPath '{TEST}' -Recurse -Force -ErrorAction SilentlyContinue"));
        // start like this PC: another pair already in the string
        run(format!("New-Item -Path '{TEST}\\Dx' -Force | Out-Null; Set-ItemProperty -LiteralPath '{TEST}\\Dx' -Name DirectXUserGlobalSettings -Value 'VRROptimizeEnable=0;'"));
        run(set_stmt(Kind::DxPair("SwapEffectUpgradeEnable"), Some("1")));
        assert_eq!(read("Dx", "DirectXUserGlobalSettings"), "VRROptimizeEnable=0;SwapEffectUpgradeEnable=1;", "adds the pair, keeps the other");
        run(set_stmt(Kind::DxPair("VRROptimizeEnable"), Some("1")));
        assert_eq!(read("Dx", "DirectXUserGlobalSettings"), "VRROptimizeEnable=1;SwapEffectUpgradeEnable=1;", "changes one pair in place");
        run(set_stmt(Kind::DxPair("SwapEffectUpgradeEnable"), None));
        run(set_stmt(Kind::DxPair("VRROptimizeEnable"), Some("0")));
        assert_eq!(read("Dx", "DirectXUserGlobalSettings"), "VRROptimizeEnable=0;", "undo gives back the exact original string");
        run(set_stmt(Kind::DxPair("VRROptimizeEnable"), None));
        assert_eq!(read("Dx", "DirectXUserGlobalSettings"), "", "an empty string is removed, not left behind");
        // startup delay: create, then remove the key it created
        run(set_stmt(Kind::StartupDelay, Some("0|0")));
        assert_eq!(read("Serialize", "StartupDelayInMSec"), "0");
        assert_eq!(read("Serialize", "WaitForIdleState"), "0");
        run(set_stmt(Kind::StartupDelay, None));
        let gone = common::ps(&format!("Test-Path -LiteralPath '{TEST}\\Serialize'")).unwrap();
        assert_eq!(gone.trim(), "False", "the empty Serialize key is removed on undo");
        // startup delay undo restores each old value on its own (delay 10000, idle absent)
        run(set_stmt(Kind::StartupDelay, Some("10000|")));
        run(set_stmt(Kind::StartupDelay, Some("0|0")));
        run(set_stmt(Kind::StartupDelay, Some("10000|")));
        assert_eq!(read("Serialize", "StartupDelayInMSec"), "10000");
        assert_eq!(read("Serialize", "WaitForIdleState"), "", "WaitForIdleState goes back to absent, not 10000");
        // classic menu: empty (Default); undo keeps a CLSID key that has other content
        run(set_stmt(Kind::ClassicMenu, Some("on")));
        let default = common::ps(&format!("(Get-Item -LiteralPath '{TEST}\\Clsid\\InprocServer32').GetValue('', $null) -eq ''")).unwrap();
        assert_eq!(default.trim(), "True", "InprocServer32 (Default) is an empty string");
        run(format!("Set-ItemProperty -LiteralPath '{TEST}\\Clsid' -Name Mine -Value 1"));
        run(set_stmt(Kind::ClassicMenu, None));
        assert_eq!(read("Clsid", "Mine"), "1", "the user's own value in the CLSID key survives undo");
        let sub = common::ps(&format!("Test-Path -LiteralPath '{TEST}\\Clsid\\InprocServer32'")).unwrap();
        assert_eq!(sub.trim(), "False");
        let _ = common::ps(&format!("Remove-Item -LiteralPath '{TEST}' -Recurse -Force -ErrorAction SilentlyContinue"));
    }

    /// Prints what the app reads on this PC (read-only). `cargo test -- --ignored perf_print_state --nocapture`
    #[test]
    #[ignore]
    fn perf_print_state() {
        let s = read_state();
        println!("{s:?}");
        assert!(s.error.is_none(), "{:?}", s.error);
    }

    fn two_monitors() -> Vec<Monitor> {
        vec![
            Monitor { instance: r"DISPLAY\SAM7350\5&148b3217&1&UID4352_0".into(), name: "S90H".into(), active: true },
            Monitor { instance: r"DISPLAY\AUS274F\5&148b3217&1&UID4353_0".into(), name: "PG27AQWP-W".into(), active: true },
        ]
    }

    #[test]
    fn perf_monitor_matches_measured_display() {
        let mons = two_monitors();
        let asus = r"\\?\DISPLAY#AUS274F#5&148b3217&1&UID4353#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(display_key(asus).as_deref(), Some(r"DISPLAY\AUS274F\5&148B3217&1&UID4353"));
        assert_eq!(pick_monitor(asus, &mons), "PG27AQWP-W", "the TV listed first is not picked");
        let tv = r"\\?\DISPLAY#SAM7350#5&148b3217&1&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(pick_monitor(tv, &mons), "S90H");
        // no match: fall back to an active PG27AQWP, never an unrelated first monitor
        assert_eq!(pick_monitor("", &mons), "PG27AQWP-W");
        assert_eq!(pick_monitor("", &mons[..1]), "");
        // UID4353 must not match UID43530
        let near = vec![Monitor { instance: r"DISPLAY\AUS274F\5&148b3217&1&UID43530_0".into(), name: "Other".into(), active: true }];
        assert_eq!(pick_monitor(asus, &near), "");
    }

    #[test]
    fn perf_explorer_needs_preview() {
        let mut tab = Perf {
            state: None,
            loading: None,
            selected: vec![],
            history: vec![],
            op: None,
            pending: None,
            run: None,
            explorer: None,
            explorer_preview: false,
            demo: true,
            note: None,
        };
        tab.open_explorer_preview();
        assert!(tab.op.is_some() && tab.explorer_preview, "a preview opens first");
        assert!(tab.explorer.is_none(), "nothing runs before Go");
        assert!(tab.op.as_ref().unwrap().warning.as_deref().unwrap_or("").contains("folder windows close"));
        tab.close_window();
        assert!(tab.op.is_none() && !tab.explorer_preview);
    }
}
