//! Performance tab - Windows settings that make games and the desktop feel faster, chosen from
//! researched evidence (see LEFT_ALONE for the popular tweaks that do nothing or cause harm).
//!
//! Honest scope: Halo Infinite on this PC is GPU-bound, so almost nothing in Windows changes its fps.
//! What helps is lower latency in borderless DX10/11 games, a snappier desktop, and checks that the
//! important settings (Game Mode, GPU scheduling, refresh rate) are where they should be.

use crate::common::{self, Job, Stream, Tab};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

const HISTORY: &str = "perf-history.json";
const DX_KEY: &str = r"HKCU:\Software\Microsoft\DirectX\UserGpuPreferences";
const ADV_KEY: &str = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const SERIALIZE_KEY: &str = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize";
const CLASSIC_MENU_KEY: &str = r"HKCU:\Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}";
const SUB_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
const PERFEPP: &str = "36687f9e-e3a5-4dbf-b1dc-15eb381c6863";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
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
}

impl Kind {
    /// Explorer has to restart before the change shows.
    fn needs_explorer(self) -> bool {
        matches!(self, Kind::AdvDword(_) | Kind::ClassicMenu)
    }
}

struct Tweak {
    id: &'static str,
    name: &'static str,
    group: &'static str,
    kind: Kind,
    target: &'static str,
    optional: bool,
    why: &'static str,
}

const G_GAMES: &str = "Games";
const G_DESKTOP: &str = "Snappier desktop";
const G_POWER: &str = "Power, mouse and disk";

const TWEAKS: &[Tweak] = &[
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
];

/// Popular tweaks this tab does not offer, and why (from the research).
const LEFT_ALONE: &[(&str, &str)] = &[
    ("Ultimate / High performance power plan", "AMD recommends Balanced for X3D CPUs; the other plans add nothing measurable on a 9800X3D."),
    ("Minimum processor state 100 %, core parking off", "Already fully unparked when plugged in; forcing it only raises idle power."),
    ("HPET / useplatformclock / disabledynamictick", "Slower timing and possible stutter; modern Windows already uses the best timer."),
    ("Timer resolution tools", "Games ask Windows for the timer resolution they need; global changes do nothing on Windows 11."),
    ("Disabling MPO (multiplane overlays)", "Only a fix for specific flicker bugs; it can raise latency."),
    ("Fullscreen optimizations off", "Needed for the flip model and VRR; no measured gain from turning it off."),
    ("Game Mode off / GPU scheduling (HAGS) off", "Both are right as they are: Game Mode stops update interruptions, HAGS is needed for DLSS Frame Generation."),
    ("TdrDelay / TdrLevel", "A GPU hang would freeze the whole PC instead of recovering."),
    ("Spectre / Meltdown mitigations off", "A real security risk for a tiny gain at best."),
    ("LargeSystemCache, DisablePagingExecutive, fixed page file", "Server-era settings; on a desktop they do nothing or make memory pressure worse."),
    ("MMCSS 'Games' priorities, SystemResponsiveness", "Game render threads do not use them. SystemResponsiveness is already 10 here (the lowest that counts)."),
    ("MSI-mode tools", "The RTX 5080 already uses MSI interrupts."),
    ("Thumbnail cache off, ForegroundLockTimeout 0, AutoEndTasks", "Slower folders, focus stealing, or lost unsaved work."),
];

fn tweak(id: &str) -> Option<&'static Tweak> {
    TWEAKS.iter().find(|t| t.id == id)
}

// ------------------------------------------------------------------ state

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct State {
    dx: Option<String>,
    menu: Option<String>,
    hover: Option<String>,
    mouse: Option<String>,
    taskanim: Option<u32>,
    thumbhover: Option<u32>,
    /// "delay|idle" (an empty part = absent)
    startdelay: Option<String>,
    classicmenu: bool,
    /// active power plan GUID (EPP is changed on it)
    scheme: String,
    epp: Option<u32>,
    /// Settings > Power mode overlay GUID; empty = Balanced (no overlay)
    overlay: String,
    hibernate: Option<u32>,
    hiberfil_gb: Option<f32>,
    // checks (read only)
    game_mode: Option<u32>,
    hags: Option<u32>,
    app_capture: Option<u32>,
    gamedvr: Option<u32>,
    transparency: Option<u32>,
    min_animate: Option<String>,
    client_anim: Option<bool>,
    usb_suspend: Option<u32>,
    pcie_aspm: Option<u32>,
    hz_now: Option<u32>,
    hz_max: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    monitor: String,
    error: Option<String>,
}

/// Value of one `Key=Value;` pair in the DirectX settings string.
pub fn dx_pair(s: Option<&str>, key: &str) -> Option<String> {
    s?.split(';').filter_map(|p| p.split_once('=')).find(|(k, _)| k.trim().eq_ignore_ascii_case(key)).map(|(_, v)| v.trim().to_string())
}

impl State {
    fn value(&self, kind: Kind) -> Option<String> {
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
        }
    }

    /// A Power mode other than Balanced is active (it overrides the plan's EPP).
    fn overlay_active(&self) -> bool {
        let o = self.overlay.trim();
        !o.is_empty() && o != "00000000-0000-0000-0000-000000000000"
    }

    fn applicable(&self, kind: Kind) -> bool {
        // nothing can be changed safely when the read failed: "before" values would be guesses
        if self.error.is_some() {
            return false;
        }
        match kind {
            Kind::Epp => self.epp.is_some() && !self.overlay_active(),
            Kind::Mouse => self.mouse.is_some(),
            _ => true,
        }
    }

    /// Why a row cannot be changed.
    fn blocked_reason(&self, kind: Kind) -> &'static str {
        match kind {
            Kind::Epp if self.overlay_active() => "Settings > Power mode is not 'Balanced' - it overrides this value. Set Power mode back to Balanced to use it.",
            Kind::Epp => "The power plan's energy preference could not be read.",
            Kind::Mouse => "The mouse settings could not be read.",
            _ => "The current value could not be read.",
        }
    }

    fn is_set(&self, t: &Tweak) -> bool {
        let v = self.value(t.kind);
        match t.kind {
            // pointer precision is off whenever the speed (third number) is 0
            Kind::Mouse => v.map(|m| m.rsplit(',').next().map(|s| s.trim() == "0").unwrap_or(false)).unwrap_or(false),
            Kind::Epp => v.as_deref().map(epp_number) == Some(t.target),
            _ => v.as_deref() == Some(t.target),
        }
    }
}

/// "guid:25" or "25" -> "25".
fn epp_number(v: &str) -> &str {
    v.rsplit(':').next().unwrap_or(v).trim()
}

/// "delay|idle" -> (delay, idle); an empty part = absent.
fn split_pair(v: &str) -> (Option<&str>, Option<&str>) {
    let mut it = v.splitn(2, '|');
    let a = it.next().map(str::trim).filter(|s| !s.is_empty());
    let b = it.next().map(str::trim).filter(|s| !s.is_empty());
    (a, b)
}

impl State {
}

const READ_SCRIPT: &str = r#"
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
if ([PCTRead.Native]::EnumDisplaySettings($nodev, -1, [ref]$dm)) {
  $o.hz_now = $dm.dmDisplayFrequency; $o.width = $dm.dmPelsWidth; $o.height = $dm.dmPelsHeight
  $max = 0; $i = 0; $m = New-Object PCTRead.Native+DEVMODE; $m.dmSize = $dm.dmSize
  while ([PCTRead.Native]::EnumDisplaySettings($nodev, $i, [ref]$m)) {
    if ($m.dmPelsWidth -eq $dm.dmPelsWidth -and $m.dmPelsHeight -eq $dm.dmPelsHeight -and $m.dmDisplayFrequency -gt $max) { $max = $m.dmDisplayFrequency }
    $i++; if ($i -gt 2000) { break }
  }
  $o.hz_max = $max
}
$o.monitor = ''
try {
  $mon = Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorID -ErrorAction Stop | Where-Object Active | Select-Object -First 1
  if ($mon) { $o.monitor = (($mon.UserFriendlyName | Where-Object { $_ -ne 0 } | ForEach-Object { [char]$_ }) -join '').Trim() }
} catch { }
[pscustomobject]$o
"#;

fn read_state() -> State {
    match common::ps_json::<State>(READ_SCRIPT) {
        Ok(v) => v.into_iter().next().unwrap_or_else(|| State { error: Some("The settings could not be read.".into()), ..Default::default() }),
        Err(e) => State { error: Some(format!("The settings could not be read: {e}")), ..Default::default() },
    }
}

// ------------------------------------------------------------------ changes

/// Human-readable value.
fn fmt(kind: Kind, v: Option<&str>) -> String {
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
        (Kind::Hibernate, Some("0")) => "Off".into(),
        (Kind::Hibernate, _) => "On".into(),
        (_, Some(x)) => x.to_string(),
    }
}

/// PowerShell that sets `kind` to `value` (`None` = back to the Windows default / absent).
fn set_stmt(kind: Kind, value: Option<&str>) -> String {
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
    }
}

const SPI_TYPE: &str = "Add-Type -Namespace PCT -Name Spi -MemberDefinition '[DllImport(\"user32.dll\", SetLastError = true)] public static extern bool SystemParametersInfo(uint a, uint u, IntPtr p, uint f); [DllImport(\"user32.dll\", SetLastError = true)] public static extern bool SystemParametersInfo(uint a, uint u, int[] p, uint f);'\n";

/// One script for a batch: one RESULT line per change.
fn build_script(changes: &[(String, Kind, Option<String>)]) -> String {
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
fn parse_result(line: &str) -> Option<(String, Result<String, String>)> {
    let rest = line.trim().strip_prefix("RESULT|")?;
    let mut p = rest.splitn(3, '|');
    let id = p.next()?.to_string();
    let status = p.next()?;
    let msg = p.next().unwrap_or("").trim().to_string();
    Some((id, if status == "ok" { Ok(msg) } else { Err(if msg.is_empty() { "Failed.".into() } else { msg }) }))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Change {
    id: String,
    prev: Option<String>,
    new: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct HistEntry {
    time: String,
    changes: Vec<Change>,
    undone: bool,
}

// ------------------------------------------------------------------ checks (read only)

#[derive(Clone, Copy, PartialEq, Debug)]
enum Check {
    Good,
    Look,
    Unknown,
}

struct CheckRow {
    name: &'static str,
    state: Check,
    now: String,
    note: String,
}

fn checks(s: &State) -> Vec<CheckRow> {
    let mut v = vec![];
    let on_off = |b: bool| if b { "On".to_string() } else { "Off".to_string() };
    let gm = s.game_mode.unwrap_or(1) != 0;
    v.push(CheckRow { name: "Game Mode", state: if gm { Check::Good } else { Check::Look }, now: on_off(gm),
        note: if gm { "Keeps Windows Update and its pop-ups quiet during games.".into() } else { "Turn it on: Settings > Gaming > Game Mode.".into() } });
    let hags_off = s.hags == Some(1);
    v.push(CheckRow { name: "GPU scheduling (HAGS)", state: if hags_off { Check::Look } else { Check::Good },
        now: match s.hags { Some(2) => "On".into(), Some(1) => "Off".into(), _ => "Windows default (on)".into() },
        note: if hags_off { "Turn it on (Settings > Display > Graphics): DLSS Frame Generation needs it.".into() } else { "Needed for DLSS Frame Generation on the RTX 5080.".into() } });
    let capture = s.app_capture.unwrap_or(0) != 0 || s.gamedvr.unwrap_or(0) != 0;
    v.push(CheckRow { name: "Background game recording", state: if capture { Check::Look } else { Check::Good }, now: on_off(capture),
        note: if capture { "Recording in the background costs a little GPU time; turn it off in Settings > Gaming > Captures if you do not use it.".into() } else { "Off - nothing records in the background.".into() } });
    if let Some(hz) = s.hz_now {
        let max = s.hz_max.unwrap_or(hz);
        let asus540 = s.monitor.to_ascii_uppercase().contains("PG27AQWP") && max < 540;
        let state = if hz < max || asus540 { Check::Look } else { Check::Good };
        let mut note = if hz < max {
            format!("Windows offers {max} Hz at this resolution - pick it in Settings > Display > Advanced display.")
        } else {
            "Running at the highest refresh rate Windows offers.".to_string()
        };
        if asus540 {
            note.push_str(" This monitor can do 540 Hz, but only over DisplayPort 2.1 UHBR20: check the cable (DP80 / UHBR20 certified) and the monitor's DisplayPort setting.");
        }
        let mon = if s.monitor.is_empty() { String::new() } else { format!(" ({})", s.monitor) };
        v.push(CheckRow { name: "Refresh rate", state, now: format!("{hz} Hz{mon}"), note });
    }
    let anim = s.client_anim.unwrap_or(true) || s.min_animate.as_deref() != Some("0");
    v.push(CheckRow { name: "Window animations", state: Check::Good, now: if anim { "On".into() } else { "Off".into() },
        note: if anim { "Turning them off (Settings > Accessibility > Visual effects) makes windows open instantly - a matter of taste.".into() } else { "Off - windows open and close instantly.".into() } });
    let tr = s.transparency.unwrap_or(1) != 0;
    v.push(CheckRow { name: "Transparency effects", state: Check::Good, now: on_off(tr),
        note: if tr { "A matter of taste; costs almost nothing on an RTX 5080.".into() } else { "Off.".into() } });
    v.push(CheckRow { name: "USB selective suspend", state: match s.usb_suspend { Some(0) => Check::Good, Some(_) => Check::Look, None => Check::Unknown },
        now: match s.usb_suspend { Some(0) => "Off".into(), Some(_) => "On".into(), None => "-".into() },
        note: "Off means USB devices (controller, headset) are never put to sleep while plugged in.".into() });
    v.push(CheckRow { name: "PCI Express power saving", state: match s.pcie_aspm { Some(0) => Check::Good, Some(_) => Check::Look, None => Check::Unknown },
        now: match s.pcie_aspm { Some(0) => "Off".into(), Some(1) => "Moderate".into(), Some(2) => "Maximum".into(), _ => "-".into() },
        note: "Off means the graphics card's link never dozes while plugged in.".into() });
    v.push(CheckRow { name: "Do not disturb while gaming", state: Check::Unknown, now: "Check".into(),
        note: "Windows stores this where apps cannot read it safely. Open the setting and tick 'When playing a game'.".into() });
    v
}

// ------------------------------------------------------------------ tab

struct Run {
    stream: Stream,
    seen: usize,
    changes: Vec<Change>,
    undo_of: Option<usize>,
}

pub struct PerfTab {
    state: Option<State>,
    loading: Option<Job<State>>,
    selected: Vec<bool>,
    history: Vec<HistEntry>,
    op: Option<OpWindow>,
    pending: Option<(Vec<Change>, Option<usize>)>,
    run: Option<Run>,
    explorer: Option<Job<Result<String, String>>>,
    demo: bool,
    note: Option<String>,
}

fn open_uri(uri: &'static str) {
    std::thread::spawn(move || {
        let _ = common::run("explorer.exe", &[uri]);
    });
}

impl PerfTab {
    pub fn new() -> Self {
        let mut tab = PerfTab {
            state: None,
            loading: Some(Job::spawn(read_state)),
            selected: TWEAKS.iter().map(|t| !t.optional).collect(),
            history: common::load_json(HISTORY),
            op: None,
            pending: None,
            run: None,
            explorer: None,
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

    fn refresh(&mut self) {
        if self.loading.is_none() {
            self.loading = Some(Job::spawn(read_state));
        }
    }

    fn op_window(changes: &[Change], undo: bool) -> OpWindow {
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

    fn open_apply(&mut self) {
        let Some(state) = self.state.as_ref().filter(|s| s.error.is_none()) else { return };
        let changes: Vec<Change> = TWEAKS
            .iter()
            .zip(&self.selected)
            .filter(|(t, sel)| **sel && state.applicable(t.kind) && !state.is_set(t))
            .map(|(t, _)| Change { id: t.id.into(), prev: state.value(t.kind), new: Some(t.target.into()) })
            .collect();
        if changes.is_empty() {
            let mut w = OpWindow::new("PERFORMANCE", "change", "changed", vec![]);
            w.empty_title = "Already set".into();
            w.empty_text = "Every ticked setting already has the value shown.".into();
            self.op = Some(w);
            self.pending = None;
        } else {
            self.op = Some(Self::op_window(&changes, false));
            self.pending = Some((changes, None));
        }
    }

    fn undo_changes(&self) -> Option<(Vec<Change>, usize)> {
        let i = self.history.iter().rposition(|h| !h.undone)?;
        let changes = self.history[i].changes.iter().rev().map(|c| Change { id: c.id.clone(), prev: c.new.clone(), new: c.prev.clone() }).collect();
        Some((changes, i))
    }

    fn open_undo(&mut self, start_now: bool) {
        let Some((changes, i)) = self.undo_changes() else { return };
        self.op = Some(Self::op_window(&changes, true));
        if start_now {
            self.start(changes, Some(i));
        } else {
            self.pending = Some((changes, Some(i)));
        }
    }

    fn start(&mut self, changes: Vec<Change>, undo_of: Option<usize>) {
        let batch: Vec<(String, Kind, Option<String>)> = changes.iter().filter_map(|c| tweak(&c.id).map(|t| (c.id.clone(), t.kind, c.new.clone()))).collect();
        self.run = Some(Run { stream: common::ps_stream(&build_script(&batch)), seen: 0, changes, undo_of });
        if let Some(op) = &mut self.op {
            op.stage = Stage::Running;
        }
    }

    fn pump(&mut self) {
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
        self.refresh();
    }

    fn window(&mut self, ctx: &egui::Context) {
        let Some(op) = &mut self.op else { return };
        let can = common::is_admin() && self.run.is_none() && !self.demo;
        match op.show(ctx, can) {
            Some(OpEvent::Go) => {
                if let Some((changes, undo_of)) = self.pending.take() {
                    self.start(changes, undo_of);
                }
            }
            Some(OpEvent::Undo) => self.open_undo(true),
            Some(OpEvent::Extra(_)) => {
                self.restart_explorer();
                self.op = None;
            }
            Some(OpEvent::Close) => {
                self.op = None;
                self.pending = None;
            }
            None => {}
        }
    }

    fn restart_explorer(&mut self) {
        if self.explorer.is_none() && !self.demo {
            self.note = Some("Restarting Explorer...".into());
            self.explorer = Some(Job::spawn(|| common::ps(crate::startup::EXPLORER_SCRIPT)));
        }
    }
}

impl Tab for PerfTab {
    fn title(&self) -> &'static str {
        "Performance"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.pump();
        self.window(ui.ctx());
        if self.loading.is_some() || self.run.is_some() || self.explorer.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                theme::page_header(ui, "Performance", "Settings with a real effect on games and on how fast Windows feels. Most 'gaming tweaks' online do nothing - those are listed at the bottom.")
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if ui.add_enabled(self.loading.is_none(), egui::Button::new("Refresh")).clicked() {
                    self.refresh();
                }
            });
        });
        common::admin_banner(ui);
        egui::ScrollArea::vertical().id_salt("perf-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl PerfTab {
    fn page(&mut self, ui: &mut egui::Ui) {
        let Some(state) = self.state.clone() else {
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Reading your settings...").size(18.0).strong());
                ui.add_space(6.0);
                theme::sweep(ui, ui.available_width());
            });
            return;
        };
        if let Some(e) = &state.error {
            // without a reliable read nothing may be changed: the saved "before" values would be guesses
            theme::banner(ui, theme::LOCKED, e);
            ui.label(RichText::new("Nothing can be changed until the settings are read correctly. Press Refresh to try again.").color(theme::TEXT_DIM));
            return;
        }
        let rows = checks(&state);

        // ---- hero
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let rec: Vec<&Tweak> = TWEAKS.iter().filter(|t| !t.optional && state.applicable(t.kind)).collect();
            let set = rec.iter().filter(|t| state.is_set(t)).count();
            let good = rows.iter().filter(|r| r.state == Check::Good).count();
            let look = rows.iter().filter(|r| r.state == Check::Look).count();
            let opt_on = TWEAKS.iter().filter(|t| t.optional && state.is_set(t)).count();
            ui.horizontal(|ui| {
                let v = theme::count_up(ui.ctx(), "perf-set", set as f32).round() as usize;
                theme::hero(ui, &format!("{v} of {}", rec.len()), "recommended settings in place", "Optional ones are your choice - read each note.", if set == rec.len() { theme::ACCENT } else { theme::JUNK });
                ui.add_space(28.0);
                theme::stat(ui, &good.to_string(), "checks fine", theme::TEXT);
                ui.add_space(22.0);
                theme::stat(ui, &look.to_string(), "worth a look", if look > 0 { theme::GAMING } else { theme::TEXT });
                ui.add_space(22.0);
                theme::stat(ui, &opt_on.to_string(), "optional in place", theme::TEXT);
            });
        });
        ui.add_space(6.0);

        // ---- checks
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Checks");
                ui.label(RichText::new("read only - PC Tuner does not change these").size(12.0).color(theme::TEXT_FAINT));
            });
            ui.add_space(4.0);
            for r in &rows {
                ui.horizontal(|ui| {
                    let (chip, c) = match r.state {
                        Check::Good => ("fine", theme::OK),
                        Check::Look => ("look", theme::GAMING),
                        Check::Unknown => ("check", theme::DIM),
                    };
                    theme::chip(ui, chip, c);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(r.name).strong());
                            ui.label(RichText::new(&r.now).color(theme::TEXT_DIM));
                        });
                        ui.label(RichText::new(&r.note).size(12.5).color(theme::TEXT_DIM));
                    });
                });
                ui.add_space(2.0);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Display settings").clicked() {
                    open_uri("ms-settings:display-advanced");
                }
                if ui.button("Notification settings").clicked() {
                    open_uri("ms-settings:notifications");
                }
                if ui.button("Graphics settings").clicked() {
                    open_uri("ms-settings:display-advancedgraphics");
                }
                if ui.button("Gaming settings").clicked() {
                    open_uri("ms-settings:gaming-gamemode");
                }
            });
        });
        ui.add_space(6.0);

        // ---- settings
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Settings");
            let mut group = "";
            for (i, t) in TWEAKS.iter().enumerate() {
                if t.group != group {
                    group = t.group;
                    ui.add_space(6.0);
                    ui.label(RichText::new(group).size(15.0).strong());
                    ui.add_space(2.0);
                }
                let applicable = state.applicable(t.kind);
                let set = state.is_set(t);
                ui.horizontal(|ui| {
                    ui.add_enabled(applicable && self.run.is_none(), egui::Checkbox::without_text(&mut self.selected[i]));
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 330.0).max(260.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t.name).strong().color(if applicable { theme::TEXT } else { theme::TEXT_FAINT }));
                            if t.optional {
                                ui.label(RichText::new("optional").size(11.5).color(theme::OPTIONAL));
                            }
                        });
                        ui.label(RichText::new(t.why).size(12.5).color(theme::TEXT_DIM));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !applicable {
                            theme::chip_response(ui, "not available", theme::DIM).on_hover_text(state.blocked_reason(t.kind));
                        } else if set {
                            theme::chip(ui, "set", theme::OK);
                        } else if self.selected[i] {
                            theme::chip(ui, "will change", theme::JUNK);
                        } else {
                            theme::chip(ui, "not set", theme::DIM);
                        }
                        if applicable {
                            let now = fmt(t.kind, state.value(t.kind).as_deref());
                            let text = if set { now } else { format!("{now} -> {}", fmt(t.kind, Some(t.target))) };
                            ui.label(RichText::new(text).color(theme::TEXT_DIM));
                        }
                    });
                });
                if t.kind == Kind::Hibernate {
                    if let Some(gb) = state.hiberfil_gb {
                        ui.label(RichText::new(format!("        hiberfil.sys is {gb:.1} GB now")).size(11.5).color(theme::TEXT_FAINT));
                    }
                }
                ui.add_space(2.0);
            }
            ui.add_space(8.0);
            let pending = TWEAKS.iter().zip(&self.selected).filter(|(t, s)| **s && state.applicable(t.kind) && !state.is_set(t)).count();
            ui.horizontal(|ui| {
                let can = common::is_admin() && self.run.is_none() && self.loading.is_none();
                let label = match pending {
                    0 => "   Apply   ".to_string(),
                    1 => "   Apply 1 change   ".to_string(),
                    n => format!("   Apply {n} changes   "),
                };
                if ui.add_enabled(can, theme::primary_button(&label)).clicked() {
                    self.open_apply();
                }
                let has_undo = self.history.iter().any(|h| !h.undone);
                if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).clicked() {
                    self.open_undo(false);
                }
                if ui.button("Tick recommended only").clicked() {
                    self.selected = TWEAKS.iter().map(|t| !t.optional).collect();
                }
                if ui.add_enabled(self.explorer.is_none(), egui::Button::new("Restart Explorer")).on_hover_text("Shows taskbar and right-click changes. Not during a game: the taskbar disappears for a moment.").clicked() {
                    self.restart_explorer();
                }
            });
            if let Some(n) = &self.note {
                ui.label(RichText::new(n.as_str()).size(12.0).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(6.0);

        // ---- left alone
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Left alone on purpose");
            ui.label(RichText::new("Popular in 'gaming optimization' guides, but placebo or harmful on Windows 11 with this hardware.").size(12.5).color(theme::TEXT_DIM));
            ui.add_space(4.0);
            egui::Grid::new("perf-left-alone").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
                for (what, why) in LEFT_ALONE {
                    ui.label(RichText::new(*what).strong());
                    ui.label(RichText::new(*why).color(theme::TEXT_DIM));
                    ui.end_row();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn perf_checks() {
        let s = State { game_mode: Some(1), hags: None, app_capture: Some(0), gamedvr: Some(0), hz_now: Some(500), hz_max: Some(500), monitor: "PG27AQWP-W".into(), usb_suspend: Some(0), pcie_aspm: Some(0), ..Default::default() };
        let rows = checks(&s);
        let rr = rows.iter().find(|r| r.name == "Refresh rate").unwrap();
        assert_eq!(rr.state, Check::Look, "540 Hz monitor stuck at 500 Hz is flagged");
        assert!(rr.note.contains("UHBR20"));
        assert_eq!(rows.iter().find(|r| r.name == "Game Mode").unwrap().state, Check::Good);
        assert_eq!(rows.iter().find(|r| r.name == "GPU scheduling (HAGS)").unwrap().state, Check::Good, "absent = on");
        let other = State { hz_now: Some(144), hz_max: Some(165), monitor: "Other".into(), ..Default::default() };
        let rr = checks(&other).into_iter().find(|r| r.name == "Refresh rate").unwrap();
        assert_eq!(rr.state, Check::Look);
        assert!(rr.note.contains("165 Hz"));
    }
}
