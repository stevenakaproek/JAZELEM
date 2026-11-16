//! Network tab - adapter and Windows settings that matter for online play, plus a live latency test.
//!
//! Honest scope: Halo Infinite's match traffic is UDP over a wired link. What helps is keeping the
//! adapter awake and responsive; most "ping tweaks" on the internet are TCP-only or ignored by
//! Windows 10/11. The biggest real-world cause of lag spikes (bufferbloat) lives on the router.

use crate::common::{self, Job, Stream, Tab};
use crate::opwin::{self, OpWindowUi as _, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use winreg::enums::*;
use winreg::RegKey;

const HISTORY: &str = "network-history.json";
const QOS_NAME: &str = "PCTuner Halo Infinite";
const THROTTLE_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
const IF_KEY: &str = r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// Network adapter advanced property (registry keyword).
    Adv(&'static str),
    /// MMCSS NetworkThrottlingIndex (DWORD).
    Throttle,
    /// Per-interface TcpAckFrequency (DWORD).
    Ack,
    /// Per-interface TCPNoDelay (DWORD).
    NoDelay,
    /// Local QoS policy that tags HaloInfinite.exe with DSCP 46.
    Qos,
    /// UDP receive segment coalescing offload (netsh int udp).
    Uro,
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

const G_POWER: &str = "Adapter power saving";
const G_LAT: &str = "Adapter latency";
const G_WIN: &str = "Windows network stack";
const G_OPT: &str = "Optional";

const TWEAKS: &[Tweak] = &[
    Tweak { id: "eee", name: "Energy-Efficient Ethernet", group: G_POWER, kind: Kind::Adv("*EEE"), target: "0", optional: false,
        why: "Stops the link dozing between packets, which adds a wake-up delay to the next one." },
    Tweak { id: "adveee", name: "Advanced EEE", group: G_POWER, kind: Kind::Adv("AdvancedEEE"), target: "0", optional: false,
        why: "Realtek's extra power saving on top of EEE." },
    Tweak { id: "green", name: "Green Ethernet", group: G_POWER, kind: Kind::Adv("EnableGreenEthernet"), target: "0", optional: false,
        why: "Lowers transmit power on short cables. No benefit on a desktop." },
    Tweak { id: "psm", name: "Power Saving Mode", group: G_POWER, kind: Kind::Adv("PowerSavingMode"), target: "0", optional: false,
        why: "Lets the adapter slow down when traffic is light - exactly the pattern of game packets." },
    Tweak { id: "gigalite", name: "Gigabit Lite", group: G_POWER, kind: Kind::Adv("GigaLite"), target: "0", optional: false,
        why: "Drops the link to 500 Mbps to save power." },
    Tweak { id: "ss", name: "Selective Suspend", group: G_POWER, kind: Kind::Adv("*SelectiveSuspend"), target: "0", optional: false,
        why: "Lets Windows put the adapter to sleep when it looks idle." },
    Tweak { id: "im", name: "Interrupt Moderation", group: G_LAT, kind: Kind::Adv("*InterruptModeration"), target: "0", optional: false,
        why: "Packets reach Windows at once instead of being batched. Saves well under a millisecond and costs a little CPU - the 9800X3D has plenty." },
    Tweak { id: "fc", name: "Flow Control", group: G_LAT, kind: Kind::Adv("*FlowControl"), target: "0", optional: false,
        why: "Stops the router or switch from pausing your adapter. Pauses are rare at home; off is the usual gaming setting." },
    Tweak { id: "throttle", name: "Network throttling (MMCSS)", group: G_WIN, kind: Kind::Throttle, target: "4294967295", optional: false,
        why: "Windows caps non-media packets while audio or video plays. Off removes the cap. Small effect, no downside." },
    Tweak { id: "ack", name: "TCP delayed ACK", group: G_WIN, kind: Kind::Ack, target: "1", optional: false,
        why: "Acknowledges every TCP packet at once. TCP only: Halo's match traffic is UDP, so this helps chat and menus, not hit registration." },
    Tweak { id: "nodelay", name: "Nagle's algorithm", group: G_WIN, kind: Kind::NoDelay, target: "1", optional: false,
        why: "Sends small TCP packets straight away instead of bundling them. TCP only, same note as above." },
    Tweak { id: "dscp", name: "Tag Halo Infinite as game traffic", group: G_OPT, kind: Kind::Qos, target: "on", optional: true,
        why: "Marks HaloInfinite.exe packets DSCP 46 (high priority). Only helps if your router prioritises DSCP - most home routers ignore it." },
    Tweak { id: "uro", name: "UDP receive coalescing (URO)", group: G_OPT, kind: Kind::Uro, target: "disabled", optional: true,
        why: "Windows 11 can merge incoming UDP packets to save CPU. No measured effect on games either way - an A/B test." },
    Tweak { id: "wol", name: "Wake on magic packet", group: G_OPT, kind: Kind::Adv("*WakeOnMagicPacket"), target: "0", optional: true,
        why: "The PC can no longer be woken over the network. No speed effect - just less running." },
    Tweak { id: "wolp", name: "Wake on pattern match", group: G_OPT, kind: Kind::Adv("*WakeOnPattern"), target: "0", optional: true,
        why: "Stops ordinary network traffic waking the PC from sleep." },
    Tweak { id: "s5wol", name: "Shutdown Wake-on-LAN", group: G_OPT, kind: Kind::Adv("S5WakeOnLan"), target: "0", optional: true,
        why: "Keeps the network port powered while the PC is off. Off saves standby power." },
];

fn tweak(id: &str) -> Option<&'static Tweak> {
    TWEAKS.iter().find(|t| t.id == id)
}

// ------------------------------------------------------------------ state

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AdapterInfo {
    name: String,
    desc: String,
    speed: String,
    media: String,
    guid: String,
    gateway: String,
    adv: HashMap<String, String>,
    qos: bool,
    uro: String,
}

#[derive(Default, Clone)]
struct State {
    info: Option<AdapterInfo>,
    error: Option<String>,
    throttle: Option<u32>,
    ack: Option<u32>,
    nodelay: Option<u32>,
}

impl State {
    fn adapter(&self) -> &str {
        self.info.as_ref().map(|i| i.name.as_str()).unwrap_or("")
    }

    fn value(&self, kind: Kind) -> Option<String> {
        let info = self.info.as_ref();
        match kind {
            Kind::Adv(kw) => info.and_then(|i| i.adv.get(kw).cloned()),
            Kind::Throttle => self.throttle.map(|v| v.to_string()),
            Kind::Ack => self.ack.map(|v| v.to_string()),
            Kind::NoDelay => self.nodelay.map(|v| v.to_string()),
            Kind::Qos => info.filter(|i| i.qos).map(|_| "on".to_string()),
            Kind::Uro => info.map(|i| i.uro.to_ascii_lowercase()).filter(|u| !u.is_empty()),
        }
    }

    fn applicable(&self, kind: Kind) -> bool {
        let info = self.info.as_ref();
        match kind {
            Kind::Adv(kw) => info.map(|i| i.adv.contains_key(kw)).unwrap_or(false),
            Kind::Ack | Kind::NoDelay => info.map(|i| !i.guid.is_empty()).unwrap_or(false),
            Kind::Uro => info.map(|i| !i.uro.is_empty()).unwrap_or(false),
            Kind::Throttle | Kind::Qos => true,
        }
    }

    fn is_set(&self, t: &Tweak) -> bool {
        self.value(t.kind).as_deref() == Some(t.target)
    }
}

const READ_SCRIPT: &str = r#"
$a = Get-NetAdapter -Physical -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'Up' } |
     Sort-Object @{e={$_.MediaType -ne '802.3'}}, @{e={$_.ReceiveLinkSpeed}; Descending=$true} | Select-Object -First 1
$adv = @{}
$gw = ''
if ($a) {
  Get-NetAdapterAdvancedProperty -Name $a.Name -ErrorAction SilentlyContinue | ForEach-Object {
    if ($_.RegistryKeyword) { $adv[$_.RegistryKeyword] = (@($_.RegistryValue) -join ',') }
  }
  $r = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -InterfaceIndex $a.InterfaceIndex -ErrorAction SilentlyContinue | Sort-Object RouteMetric | Select-Object -First 1
  if ($r) { $gw = [string]$r.NextHop }
}
$qos = [bool](Get-NetQosPolicy -Name 'PCTuner Halo Infinite' -ErrorAction SilentlyContinue)
$uro = ''
$line = netsh int udp show global | Where-Object { $_ -match 'Receive Offload' } | Select-Object -First 1
if ($line) { $uro = ($line -split ':')[-1].Trim() }
[pscustomobject]@{
  name = [string]$a.Name; desc = [string]$a.InterfaceDescription; speed = [string]$a.LinkSpeed
  media = [string]$a.MediaType; guid = [string]$a.InterfaceGuid; gateway = $gw; adv = $adv; qos = $qos; uro = $uro
}
"#;

fn reg_u32(path: &str, name: &str) -> Option<u32> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(path).ok()?.get_value::<u32, _>(name).ok()
}

fn read_state() -> State {
    let mut s = State { throttle: reg_u32(THROTTLE_KEY, "NetworkThrottlingIndex"), ..Default::default() };
    match common::ps_json::<AdapterInfo>(READ_SCRIPT) {
        Ok(v) => {
            let info = v.into_iter().next().filter(|i| !i.name.is_empty());
            if info.is_none() {
                s.error = Some("No connected network adapter was found.".into());
            }
            if let Some(i) = &info {
                let key = format!(r"{IF_KEY}\{}", i.guid);
                s.ack = reg_u32(&key, "TcpAckFrequency");
                s.nodelay = reg_u32(&key, "TCPNoDelay");
            }
            s.info = info;
        }
        Err(e) => s.error = Some(format!("Could not read the network settings: {e}")),
    }
    s
}

// ------------------------------------------------------------------ changes

/// Human-readable form of a setting's value (`None` = not set, i.e. the Windows default).
fn fmt(kind: Kind, v: Option<&str>) -> String {
    match (kind, v) {
        (Kind::Throttle, None) => "10 packets/ms (default)".into(),
        (Kind::Throttle, Some("4294967295")) => "Off".into(),
        (Kind::Throttle, Some(x)) => format!("{x} packets/ms"),
        (Kind::Ack, None) => "Every 2nd packet (default)".into(),
        (Kind::Ack, Some("1")) => "Every packet".into(),
        (Kind::Ack, Some(x)) => format!("Every {x} packets"),
        (Kind::NoDelay, None) => "On (default)".into(),
        (Kind::NoDelay, Some("1")) => "Off".into(),
        (Kind::NoDelay, Some(_)) => "On".into(),
        (Kind::Qos, None) => "Not tagged".into(),
        (Kind::Qos, Some(_)) => "DSCP 46".into(),
        (Kind::Uro, Some("disabled")) => "Off".into(),
        (Kind::Uro, Some("enabled")) => "On".into(),
        (Kind::Adv(_), Some("0")) => "Off".into(),
        (Kind::Adv(_), Some("1")) => "On".into(),
        (Kind::Adv(_), None) => "Driver default".into(),
        (_, Some(x)) => format!("On ({x})"),
        (_, None) => "-".into(),
    }
}

/// PowerShell statement that sets `kind` to `value` (`None` removes the value / policy).
/// Removing is idempotent: a value, key or policy that is already gone counts as done.
/// `guid` is the interface GUID for the per-interface TCP values; when it is empty the statement
/// uses `$ifGuid` (resolved by `build_script` from the adapter name) and throws if that is empty
/// too, so nothing is ever written to the Interfaces parent key.
fn set_stmt(kind: Kind, value: Option<&str>, adapter: &str, guid: &str) -> String {
    let q = common::psq;
    // `path` is a PowerShell expression (a quoted literal or a parenthesised string expression)
    let dword = |path: String, name: &str| match value {
        Some(v) => format!(
            "New-ItemProperty -LiteralPath {} -Name {} -PropertyType DWord -Value ([int32]{}) -Force -ErrorAction Stop | Out-Null",
            path, q(name), v.parse::<u32>().unwrap_or(0) as i32
        ),
        None => format!(
            "if ((Get-Item -LiteralPath {p} -ErrorAction SilentlyContinue) -and ($null -ne (Get-ItemProperty -LiteralPath {p} -Name {n} -ErrorAction SilentlyContinue))) {{ Remove-ItemProperty -LiteralPath {p} -Name {n} -ErrorAction Stop }}",
            p = path, n = q(name)
        ),
    };
    let iface_dword = |name: &str| {
        if guid.is_empty() {
            format!(
                "if (-not $ifGuid) {{ throw 'The network adapter could not be found, so its TCP settings were not changed.' }}; {}",
                dword(format!("({} + $ifGuid)", q(&format!(r"HKLM:\{IF_KEY}\"))), name)
            )
        } else {
            dword(q(&format!(r"HKLM:\{IF_KEY}\{guid}")), name)
        }
    };
    match kind {
        Kind::Adv(kw) => match value {
            Some(v) => format!(
                "Set-NetAdapterAdvancedProperty -Name {} -RegistryKeyword {} -RegistryValue {} -NoRestart -ErrorAction Stop",
                q(adapter), q(kw), q(v)
            ),
            // "Remove": back to the network driver's own default
            None => format!("Reset-NetAdapterAdvancedProperty -Name {} -RegistryKeyword {} -NoRestart -ErrorAction Stop", q(adapter), q(kw)),
        },
        Kind::Throttle => dword(q(&format!(r"HKLM:\{THROTTLE_KEY}")), "NetworkThrottlingIndex"),
        Kind::Ack => iface_dword("TcpAckFrequency"),
        Kind::NoDelay => iface_dword("TCPNoDelay"),
        Kind::Qos => match value {
            Some(_) => format!(
                "if (-not (Get-NetQosPolicy -Name {n} -ErrorAction SilentlyContinue)) {{ New-NetQosPolicy -Name {n} -AppPathNameMatchCondition 'HaloInfinite.exe' -IPProtocolMatchCondition Both -DSCPAction 46 -NetworkProfile All -ErrorAction Stop | Out-Null }}",
                n = q(QOS_NAME)
            ),
            None => format!(
                "if (Get-NetQosPolicy -Name {n} -ErrorAction SilentlyContinue) {{ Remove-NetQosPolicy -Name {n} -Confirm:$false -ErrorAction Stop }}",
                n = q(QOS_NAME)
            ),
        },
        Kind::Uro => {
            let v = if value == Some("disabled") { "disabled" } else { "enabled" };
            format!("$o = netsh int udp set global uro={v}; if ($LASTEXITCODE -ne 0) {{ throw (($o | Out-String).Trim()) }}")
        }
    }
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
    adapter: String,
    /// Interface GUID of `adapter` when the change was made (empty in older history files).
    #[serde(default)]
    guid: String,
    changes: Vec<Change>,
    undone: bool,
}

/// Changes for the selected rows: each selected tweak that exists on this adapter and is not applied
/// yet gets its value. Selected rows that are already applied need nothing.
fn selected_changes(state: &State, selected: &[bool]) -> Vec<Change> {
    TWEAKS
        .iter()
        .zip(selected)
        .filter(|(t, sel)| **sel && state.applicable(t.kind) && !state.is_set(t))
        .map(|(t, _)| Change { id: t.id.into(), prev: state.value(t.kind), new: Some(t.target.into()) })
        .collect()
}

/// One script for a batch of changes: one RESULT line per change, then one adapter restart if any
/// adapter property changed (properties are written with -NoRestart).
/// An empty `guid` is resolved from the adapter name at run time (`$ifGuid`).
fn build_script(changes: &[(String, Kind, Option<String>)], adapter: &str, guid: &str) -> String {
    let mut s = String::from("$advChanged = $false\n");
    if guid.is_empty() && changes.iter().any(|c| matches!(c.1, Kind::Ack | Kind::NoDelay)) {
        s.push_str("$ifGuid = ''\n");
        if !adapter.is_empty() {
            s.push_str(&format!(
                "try {{ $ifGuid = [string](Get-NetAdapter -Name {} -ErrorAction Stop).InterfaceGuid }} catch {{ }}\n",
                common::psq(adapter)
            ));
        }
    }
    for (id, kind, value) in changes {
        let stmt = set_stmt(*kind, value.as_deref(), adapter, guid);
        let mark = if matches!(kind, Kind::Adv(_)) { "; $advChanged = $true" } else { "" };
        s.push_str(&format!(
            "try {{ {stmt}{mark}; Write-Output 'RESULT|{id}|ok|' }} catch {{ Write-Output ('RESULT|{id}|err|' + ($_.Exception.Message -replace '\\s+', ' ')) }}\nStart-Sleep -Milliseconds 150\n"
        ));
    }
    if changes.iter().any(|c| matches!(c.1, Kind::Adv(_))) {
        s.push_str(&format!(
            "if ($advChanged) {{ try {{ Restart-NetAdapter -Name {} -Confirm:$false -ErrorAction Stop; Start-Sleep -Seconds 4; Write-Output 'RESULT|restart|ok|Back online.' }} catch {{ Write-Output ('RESULT|restart|err|' + ($_.Exception.Message -replace '\\s+', ' ')) }} }} else {{ Write-Output 'RESULT|restart|ok|Not needed - no adapter setting changed.' }}\n",
            common::psq(adapter)
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

// ------------------------------------------------------------------ tab

struct Run {
    stream: Stream,
    seen: usize,
    changes: Vec<Change>,
    adapter: String,
    guid: String,
    /// Some(history index) when this run undoes that entry.
    undo_of: Option<usize>,
    /// Result of the adapter restart step (not a setting, so it is not an OpItem).
    restart: Option<Result<String, String>>,
}

/// (adapter name, interface GUID) a run works on. An undo goes back to the adapter and interface
/// the change was made on, not whichever adapter is shown now; an old entry without a GUID gets
/// it resolved from the adapter name inside the script (never the current adapter's GUID).
fn run_target(state: &State, history: &[HistEntry], undo_of: Option<usize>) -> (String, String) {
    match undo_of.and_then(|i| history.get(i)) {
        Some(h) => (h.adapter.clone(), h.guid.clone()),
        None => (state.adapter().to_string(), state.info.as_ref().map(|i| i.guid.clone()).unwrap_or_default()),
    }
}

/// True when any change is an adapter property (those are written with -NoRestart, then one restart).
fn has_adapter_change(changes: &[Change]) -> bool {
    changes.iter().any(|c| tweak(&c.id).map(|t| matches!(t.kind, Kind::Adv(_))).unwrap_or(false))
}

/// Puts the outcome of the adapter restart step under the result chips. The restart is a step,
/// not a setting, so it is kept out of the window's items and their "N settings changed" count.
fn show_restart(op: &mut OpWindow, adapter: &str, r: Option<&Result<String, String>>) {
    match r {
        Some(Ok(note)) if note.starts_with("Not needed") => {}
        Some(Ok(_)) => op.notes.insert(0, format!("\"{adapter}\" was restarted and is back online.")),
        Some(Err(e)) => {
            op.footer = Some((format!("Restarting \"{adapter}\" failed - restart it or reboot to finish. {e}").trim().to_string(), common::Tone::Error))
        }
        None => {
            op.footer = Some((format!("\"{adapter}\" may not have been restarted - restart it or reboot to finish."), common::Tone::Error))
        }
    }
}

pub struct NetworkTab {
    state: Option<State>,
    loading: Option<Job<State>>,
    selected: Vec<bool>,
    history: Vec<HistEntry>,
    op: Option<OpWindow>,
    /// Changes of the window open in the preview stage: (changes, undo_of).
    pending: Option<(Vec<Change>, Option<usize>)>,
    run: Option<Run>,
    demo: bool,
}

impl NetworkTab {
    pub fn new() -> Self {
        let mut tab = NetworkTab {
            state: None,
            loading: Some(Job::spawn(read_state)),
            // nothing is pre-selected: the user clicks the settings they want
            selected: vec![false; TWEAKS.len()],
            history: common::load_json(HISTORY),
            op: None,
            pending: None,
            run: None,
            demo: false,
        };
        if let Some(stage) = opwin::demo_stage("network") {
            // sample window for screenshots - nothing is changed
            let ids = ["eee", "im", "throttle", "dscp"];
            let prev = |id: &str| match id {
                "throttle" => Some("10".to_string()),
                "dscp" => None,
                _ => Some("1".to_string()),
            };
            let changes: Vec<Change> = ids.iter().map(|id| Change { id: id.to_string(), prev: prev(id), new: tweak(id).map(|t| t.target.to_string()) }).collect();
            let mut w = Self::op_window(&changes, false, "Ethernet");
            match stage {
                Stage::Preview => {}
                Stage::Running => {
                    w.stage = Stage::Running;
                    w.items[0].result = Some(Ok(String::new()));
                }
                Stage::Done => {
                    w.can_undo = true;
                    w.finish(|it| Some(if it.id == "dscp" { Err("Access is denied.".into()) } else { Ok(String::new()) }));
                    show_restart(&mut w, "Ethernet", Some(&Ok("Back online.".into())));
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

    /// Operation window for a list of changes (apply or undo).
    fn op_window(changes: &[Change], undo: bool, adapter: &str) -> OpWindow {
        let mut items: Vec<OpItem> = vec![];
        for c in changes {
            let Some(t) = tweak(&c.id) else { continue };
            items.push(
                OpItem::new(t.id, t.name, t.group)
                    .detail(format!("{} -> {}", fmt(t.kind, c.prev.as_deref()), fmt(t.kind, c.new.as_deref())))
                    .optional(t.optional),
            );
        }
        // the adapter restart is a step, not a setting: it is not an item (keeps the headline count
        // honest) - the warning below announces it and `show_restart` reports how it went
        let adapter_change = has_adapter_change(changes);
        let (kicker, verb, done_word) = if undo { ("UNDO NETWORK CHANGE", "undo", "undone") } else { ("NETWORK", "change", "changed") };
        let mut w = OpWindow::new(kicker, verb, done_word, items).noun("setting", "settings");
        w.show_steps = true;
        w.fail_phrase = "could not be changed".into();
        w.preview_text = format!("Adapter: {adapter}. Each change is saved so it can be undone.");
        w.running_text = if undo { "Putting the old settings back...".into() } else { "Applying network settings...".into() };
        w.go_label = if undo { "Undo".into() } else { "Apply".into() };
        w.reassure = if undo { "Restores the values from before that change.".into() } else { "\"Undo last change\" puts every value back.".into() };
        if adapter_change {
            w.warning = Some(format!("\"{adapter}\" restarts at the end: your connection drops for about 5 seconds. Don't do this during a match."));
        }
        w
    }

    fn start(&mut self, changes: Vec<Change>, undo_of: Option<usize>) {
        let Some(state) = &self.state else { return };
        let (adapter, guid) = run_target(state, &self.history, undo_of);
        let batch: Vec<(String, Kind, Option<String>)> =
            changes.iter().filter_map(|c| tweak(&c.id).map(|t| (c.id.clone(), t.kind, c.new.clone()))).collect();
        let script = build_script(&batch, &adapter, &guid);
        self.run = Some(Run { stream: common::ps_stream(&script), seen: 0, changes, adapter, guid, undo_of, restart: None });
        if let Some(op) = &mut self.op {
            op.stage = Stage::Running;
        }
    }

    /// "Apply selected": the selected settings that are not applied yet get their tweak value.
    /// Recorded, so "Undo last change" reverts them.
    fn open_apply(&mut self) {
        let Some(state) = &self.state else { return };
        let changes = selected_changes(state, &self.selected);
        if changes.is_empty() {
            return;
        }
        self.op = Some(Self::op_window(&changes, false, state.adapter()));
        self.pending = Some((changes, None));
    }

    /// Changes that put back the last change that has not been undone.
    fn undo_changes(&self) -> Option<(Vec<Change>, usize)> {
        let i = self.history.iter().rposition(|h| !h.undone)?;
        let changes = self.history[i].changes.iter().rev().map(|c| Change { id: c.id.clone(), prev: c.new.clone(), new: c.prev.clone() }).collect();
        Some((changes, i))
    }

    fn open_undo(&mut self, start_now: bool) {
        let Some((changes, i)) = self.undo_changes() else { return };
        let adapter = self.history[i].adapter.clone();
        let mut w = Self::op_window(&changes, true, &adapter);
        let current = self.state.as_ref().map(|s| s.adapter().to_string()).unwrap_or_default();
        if !adapter.is_empty() && adapter != current {
            let shown = if current.is_empty() { "No adapter is connected now".to_string() } else { format!("The adapter shown now is \"{current}\"") };
            let restart = if has_adapter_change(&changes) { ", but its adapter settings and restart may fail if it is not present" } else { "" };
            let note = format!("This change was made on \"{adapter}\". {shown}. The old values are put back on \"{adapter}\"{restart}.");
            w.warning = Some(match w.warning.take() {
                Some(old) => format!("{note} {old}"),
                None => note,
            });
        }
        self.op = Some(w);
        if start_now {
            self.start(changes, Some(i));
        } else {
            self.pending = Some((changes, Some(i)));
        }
    }

    fn pump(&mut self) {
        if let Some(j) = &self.loading {
            if let Some(s) = j.poll() {
                self.state = Some(s);
                self.loading = None;
            }
        }

        let Some(run) = &mut self.run else { return };
        let running = run.stream.pump();
        while run.seen < run.stream.lines.len() {
            let line = run.stream.lines[run.seen].clone();
            run.seen += 1;
            match (parse_result(&line), &mut self.op) {
                (Some((id, r)), _) if id == "restart" => run.restart = Some(r),
                (Some((id, r)), Some(op)) => {
                    if let Some(it) = op.items.iter_mut().find(|i| i.id == id) {
                        it.result = Some(r);
                    }
                }
                _ => {}
            }
        }
        if running {
            return;
        }
        let run = self.run.take().unwrap();
        let code = run.stream.exit.unwrap_or(-1);
        let mut ok_ids: Vec<String> = vec![];
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
        if let Some(op) = &mut self.op {
            if has_adapter_change(&run.changes) {
                show_restart(op, &run.adapter, run.restart.as_ref());
            }
        }
        let mut can_undo = false;
        match run.undo_of {
            None => {
                let changes: Vec<Change> = run.changes.into_iter().filter(|c| ok_ids.contains(&c.id)).collect();
                if !changes.is_empty() {
                    self.history.push(HistEntry { time: common::now(), adapter: run.adapter, guid: run.guid, changes, undone: false });
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
            Some(OpEvent::Close) => {
                self.op = None;
                self.pending = None;
            }
            _ => {}
        }
    }
}

impl Tab for NetworkTab {
    fn title(&self) -> &'static str {
        "Network"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.pump();
        self.window(ui.ctx());
        if self.loading.is_some() || self.run.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }

        // shown inside the Optimization tab, which draws the page header and admin banner
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui.add_enabled(self.loading.is_none(), egui::Button::new("Refresh")).clicked() {
                self.refresh();
            }
        });
        egui::ScrollArea::vertical().id_salt("network-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl NetworkTab {
    fn page(&mut self, ui: &mut egui::Ui) {
        // only while reading, or when no adapter could be read
        match &self.state {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reading your network adapter...").color(theme::TEXT_DIM));
                });
            }
            Some(s) if s.info.is_none() => {
                ui.colored_label(theme::LOCKED, s.error.clone().unwrap_or_default());
            }
            Some(_) => {}
        }

        // ---- settings
        let Some(state) = self.state.clone() else { return };
        if state.info.is_none() {
            return;
        }
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Settings");
                ui.label(RichText::new("click the ones you want, then Apply selected").size(12.0).color(theme::TEXT_FAINT));
            });
            let mut group = "";
            for (i, t) in TWEAKS.iter().enumerate() {
                if t.group != group {
                    group = t.group;
                    ui.add_space(6.0);
                    ui.label(RichText::new(group).size(15.0).strong().color(if t.optional { theme::OPTIONAL } else { theme::TEXT }));
                    ui.add_space(2.0);
                }
                let applicable = state.applicable(t.kind);
                let set = state.is_set(t);
                // an applied setting has nothing to apply, so it cannot be selected
                let selectable = applicable && !set && self.run.is_none();
                let sel = self.selected[i] && selectable;
                let clicked = theme::select_row(ui, ("net", i), sel, selectable, |ui| {
                    ui.horizontal(|ui| {
                        if applicable {
                            theme::applied_badge(ui, set);
                        } else {
                            theme::chip(ui, "not on this adapter", theme::DIM);
                        }
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.set_width((ui.available_width() - 260.0).max(240.0));
                            ui.label(RichText::new(t.name).strong().color(if applicable { theme::TEXT } else { theme::TEXT_FAINT }));
                            ui.label(RichText::new(t.why).size(12.5).color(theme::TEXT_DIM));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if applicable {
                                let now = fmt(t.kind, state.value(t.kind).as_deref());
                                let (text, c) = if set {
                                    (now, theme::OK)
                                } else {
                                    (format!("{now} -> {}", fmt(t.kind, Some(t.target))), if sel { theme::ACCENT } else { theme::TEXT_DIM })
                                };
                                ui.label(RichText::new(text).color(c));
                            }
                        });
                    });
                });
                if clicked {
                    self.selected[i] = !self.selected[i];
                }
                ui.add_space(3.0);
            }
            ui.add_space(8.0);
            let pending = selected_changes(&state, &self.selected).len();
            ui.horizontal(|ui| {
                let can = common::is_admin() && self.run.is_none() && self.loading.is_none();
                if ui
                    .add_enabled(can && pending > 0, theme::primary_button(&format!("   Apply selected ({pending})   ")))
                    .on_disabled_hover_text(if common::is_admin() { "Click the settings you want to apply." } else { "Run PC Tuner as administrator to change settings." })
                    .clicked()
                {
                    self.open_apply();
                }
                if pending > 0 && ui.button("Clear selection").clicked() {
                    self.selected = vec![false; TWEAKS.len()];
                }
                let has_undo = self.history.iter().any(|h| !h.undone);
                if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).clicked() {
                    self.open_undo(false);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses (does not run) the generated scripts with the PowerShell parser.
    #[test]
    #[ignore]
    fn net_scripts_parse() {
        let all = [(String::from("eee"), Kind::Adv("*EEE"), Some("0".to_string())), ("ack".into(), Kind::Ack, None), ("nodelay".into(), Kind::NoDelay, Some("1".into())), ("throttle".into(), Kind::Throttle, None), ("dscp".into(), Kind::Qos, None), ("dscp".into(), Kind::Qos, Some("on".into())), ("uro".into(), Kind::Uro, Some("enabled".into()))];
        for script in [build_script(&all, "Ethernet", "{G}"), build_script(&all, "Ethernet", ""), READ_SCRIPT.to_string()] {
            let check = format!(
                "$e = $null; $null = [System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$e); if ($e) {{ $e | ForEach-Object {{ $_.Message }}; exit 1 }}",
                common::psq(&script)
            );
            if let Err(e) = common::ps(&check) {
                panic!("script does not parse: {e}\n{script}");
            }
        }
    }

    #[test]
    fn net_restart_is_not_a_setting() {
        let changes: Vec<Change> = ["eee", "im", "fc"].iter().map(|id| Change { id: id.to_string(), prev: Some("1".into()), new: Some("0".into()) }).collect();
        let w = NetworkTab::op_window(&changes, false, "Ethernet");
        assert_eq!(w.items.len(), 3, "headline counts 3 settings");
        assert!(w.items.iter().all(|i| i.id != "restart"));
        assert!(w.warning.as_deref().unwrap_or("").contains("\"Ethernet\" restarts"));
        let mut w = NetworkTab::op_window(&changes, false, "Ethernet");
        show_restart(&mut w, "Ethernet", Some(&Err("Access is denied.".into())));
        assert!(w.footer.as_ref().unwrap().0.contains("failed"));
        let mut w = NetworkTab::op_window(&changes, false, "Ethernet");
        show_restart(&mut w, "Ethernet", Some(&Ok("Not needed - no adapter setting changed.".into())));
        assert!(w.footer.is_none() && !w.notes.iter().any(|n| n.contains("restarted")));
    }

    /// "Apply selected" only touches selected settings that exist on the adapter and are not applied yet.
    #[test]
    fn net_apply_selected_only() {
        let adv: HashMap<String, String> = [("*EEE".to_string(), "1".to_string())].into_iter().collect();
        let state = State {
            info: Some(AdapterInfo { name: "Ethernet".into(), guid: "{G}".into(), adv, ..Default::default() }),
            throttle: Some(4294967295),
            ..Default::default()
        };
        let all = vec![true; TWEAKS.len()];
        let ids: Vec<String> = selected_changes(&state, &all).into_iter().map(|c| c.id).collect();
        assert!(ids.contains(&"eee".to_string()), "selected and not applied");
        assert!(!ids.contains(&"throttle".to_string()), "already applied: nothing to do");
        assert!(!ids.contains(&"adveee".to_string()), "not on this adapter");
        assert!(selected_changes(&state, &vec![false; TWEAKS.len()]).is_empty(), "nothing selected, nothing changes");
    }

    #[test]
    fn net_history_without_guid_loads() {
        let h: Vec<HistEntry> = serde_json::from_str(r#"[{"time":"t","adapter":"Ethernet","changes":[],"undone":false}]"#).unwrap();
        assert_eq!(h[0].guid, "");
    }

    #[test]
    fn net_undo_targets_the_changed_adapter() {
        let state = State { info: Some(AdapterInfo { name: "Wi-Fi".into(), guid: "{WIFI}".into(), ..Default::default() }), ..Default::default() };
        let history = vec![
            HistEntry { adapter: "Ethernet".into(), guid: "{ETH}".into(), ..Default::default() },
            HistEntry { adapter: "Ethernet".into(), guid: String::new(), ..Default::default() },
        ];
        assert_eq!(run_target(&state, &history, None), ("Wi-Fi".into(), "{WIFI}".into()));
        assert_eq!(run_target(&state, &history, Some(0)), ("Ethernet".into(), "{ETH}".into()));
        // old entry without a GUID: resolved from "Ethernet" in the script, not the Wi-Fi GUID
        assert_eq!(run_target(&state, &history, Some(1)), ("Ethernet".into(), String::new()));
    }

    #[test]
    fn net_result_lines() {
        assert_eq!(parse_result("RESULT|eee|ok|"), Some(("eee".into(), Ok(String::new()))));
        assert_eq!(parse_result("RESULT|dscp|err|Access is denied."), Some(("dscp".into(), Err("Access is denied.".into()))));
        assert_eq!(parse_result("some other output"), None);
    }

    #[test]
    fn net_statements() {
        let s = set_stmt(Kind::Throttle, Some("4294967295"), "Ethernet", "{G}");
        assert!(s.contains("[int32]-1") && s.contains("NetworkThrottlingIndex") && s.contains("DWord"));
        // removing is idempotent: an absent value / policy is not an error
        let s = set_stmt(Kind::Ack, None, "Ethernet", "{G}");
        assert!(s.contains("Remove-ItemProperty") && s.contains(r"Interfaces\{G}"));
        assert!(s.starts_with("if ((Get-Item -LiteralPath") && s.contains("Get-ItemProperty -LiteralPath") && s.contains("SilentlyContinue"));
        let s = set_stmt(Kind::NoDelay, None, "Ethernet", "{G}");
        assert!(s.starts_with("if ((Get-Item") && s.contains("'TCPNoDelay'"));
        let s = set_stmt(Kind::Qos, None, "", "");
        assert!(s.starts_with("if (Get-NetQosPolicy") && s.contains("Remove-NetQosPolicy"));
        // no GUID: never the Interfaces parent key, the script's $ifGuid or a throw instead
        let s = set_stmt(Kind::Ack, Some("1"), "Ethernet", "");
        assert!(s.starts_with("if (-not $ifGuid) { throw") && s.contains("+ $ifGuid)") && !s.contains(r"Interfaces'"));
        let s = set_stmt(Kind::Adv("*EEE"), Some("0"), "Ether'net", "");
        assert!(s.contains("'Ether''net'") && s.contains("-NoRestart"));
        assert!(set_stmt(Kind::Uro, Some("disabled"), "", "").contains("uro=disabled"));
        let script = build_script(&[("ack".into(), Kind::Ack, None)], "Ethernet", "");
        assert!(script.contains("Get-NetAdapter -Name 'Ethernet'") && script.contains("InterfaceGuid"));
        assert!(!build_script(&[("ack".into(), Kind::Ack, None)], "Ethernet", "{G}").contains("$ifGuid"));
        let script = build_script(&[("eee".into(), Kind::Adv("*EEE"), Some("0".into())), ("ack".into(), Kind::Ack, Some("1".into()))], "Ethernet", "{G}");
        assert!(script.contains("RESULT|eee|ok|") && script.contains("RESULT|ack|ok|") && script.contains("Restart-NetAdapter"));
        let script = build_script(&[("ack".into(), Kind::Ack, Some("1".into()))], "Ethernet", "{G}");
        assert!(!script.contains("Restart-NetAdapter"));
    }

    #[test]
    fn net_format_and_ids() {
        assert_eq!(fmt(Kind::Throttle, Some("4294967295")), "Off");
        assert_eq!(fmt(Kind::Throttle, None), "10 packets/ms (default)");
        assert_eq!(fmt(Kind::Adv("*FlowControl"), Some("3")), "On (3)");
        assert_eq!(fmt(Kind::NoDelay, Some("1")), "Off");
        let mut ids: Vec<&str> = TWEAKS.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TWEAKS.len(), "tweak ids must be unique");
        assert!(!ids.contains(&"restart"));
    }
}
