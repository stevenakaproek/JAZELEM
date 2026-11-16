//! USB Port Finder - port of USB-PortFinder.ps1.
//! Shows which USB controller every device is plugged into, ranked by how close that controller sits to the CPU,
//! plus a plug-in test that tells you the tier of the port a device was just plugged into. Changes nothing.
//!
//! Logic only: the device tree read, the tier ranking, the plug-in test and the page state.

use crate::common::{self, Job};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const PATH_REFRESH: &str = "$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')";

pub fn tier_text(t: u8) -> &'static str {
    match t {
        1 => "Tier 1  CPU (built into the CPU)",
        2 => "Tier 2  CPU lanes (via add-on USB4 chip)",
        3 => "Tier 3  Chipset",
        4 => "Tier 4  Second chipset (furthest)",
        _ => "Unknown",
    }
}

/// Short tier label for chips.
pub fn tier_short(t: u8) -> &'static str {
    match t {
        1 => "Tier 1  ·  CPU",
        2 => "Tier 2  ·  CPU lanes",
        3 => "Tier 3  ·  Chipset",
        4 => "Tier 4  ·  2nd chipset",
        _ => "Unknown",
    }
}


// ------------------------------------------------------------------ device tree

#[derive(Deserialize, Clone, Debug)]
pub struct RawNode {
    #[serde(rename = "Id", default)]
    pub id: Option<String>,
    #[serde(rename = "Name", default)]
    pub name: Option<String>,
    #[serde(rename = "Class", default)]
    pub class: Option<String>,
    #[serde(rename = "Parent", default)]
    pub parent: Option<String>,
    #[serde(rename = "LocInfo", default)]
    pub loc_info: Option<String>,
    #[serde(rename = "LocPath", default)]
    pub loc_path: Option<String>,
    #[serde(rename = "BusDesc", default)]
    pub bus_desc: Option<String>,
    /// Device Manager problem code (22 = disabled)
    #[serde(rename = "Problem", default)]
    pub problem: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub class: String,
    pub parent: String,
    pub loc_info: String,
    pub loc_path: String,
    pub bus_desc: String,
    /// switched off in Device Manager (problem code 22)
    pub disabled: bool,
}

/// All present PnP devices. Lookups are case-insensitive (keys upper-cased), like PowerShell hashtables.
pub struct Tree {
    pub nodes: Vec<Node>,
    pub by_id: HashMap<String, usize>,
    pub children: HashMap<String, Vec<usize>>,
}

pub const TREE_SCRIPT: &str = r#"
$all = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue)
# One CIM session and one GetDeviceProperties call per device: about 1.5 s for 200 devices, where piping
# them into Get-PnpDeviceProperty took 12-30 s (it is slow per pipeline item). Same data.
$keys = [string[]]@('DEVPKEY_Device_Parent', 'DEVPKEY_Device_LocationInfo', 'DEVPKEY_Device_LocationPaths', 'DEVPKEY_Device_BusReportedDeviceDesc')
$sp = @{}
try { $sp.CimSession = New-CimSession -ErrorAction Stop } catch {}
foreach ($d in $all) {
    $v = @{}
    try {
        $r = Invoke-CimMethod @sp -InputObject $d -MethodName GetDeviceProperties -Arguments @{ devicePropertyKeys = $keys } -ErrorAction Stop
        foreach ($p in @($r.deviceProperties)) { if ($p -and $p.KeyName) { $v[[string]$p.KeyName] = $p.Data } }
    } catch {}
    $i = $d.InstanceId
    [pscustomobject]@{ Id = [string]$i; Name = [string]$d.FriendlyName; Class = [string]$d.Class; Parent = [string]$v['DEVPKEY_Device_Parent']; LocInfo = [string]$v['DEVPKEY_Device_LocationInfo']; LocPath = [string](@($v['DEVPKEY_Device_LocationPaths'])[0]); BusDesc = [string]$v['DEVPKEY_Device_BusReportedDeviceDesc']; Problem = [int]$d.ConfigManagerErrorCode }
}
if ($sp.CimSession) { Remove-CimSession $sp.CimSession }
"#;

impl Tree {
    pub fn from_nodes(nodes: Vec<Node>) -> Tree {
        let mut by_id = HashMap::new();
        let mut children: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, n) in nodes.iter().enumerate() {
            by_id.insert(n.id.to_uppercase(), i);
            if !n.parent.is_empty() {
                children.entry(n.parent.to_uppercase()).or_default().push(i);
            }
        }
        Tree { nodes, by_id, children }
    }

    /// One PowerShell call (2-4 s including PowerShell start-up): every present device plus parent / location / bus description.
    pub fn load() -> Result<Tree, String> {
        let raw: Vec<RawNode> = common::ps_json(&format!("{PATH_REFRESH}\n{TREE_SCRIPT}"))?;
        let nodes: Vec<Node> = raw
            .into_iter()
            .filter_map(|r| {
                let id = r.id.unwrap_or_default();
                if id.is_empty() {
                    return None;
                }
                Some(Node {
                    id,
                    name: r.name.unwrap_or_default(),
                    class: r.class.unwrap_or_default(),
                    parent: r.parent.unwrap_or_default(),
                    loc_info: r.loc_info.unwrap_or_default(),
                    loc_path: r.loc_path.unwrap_or_default(),
                    bus_desc: r.bus_desc.unwrap_or_default(),
                    disabled: r.problem == Some(PROBLEM_DISABLED),
                })
            })
            .collect();
        if nodes.is_empty() {
            return Err("Get-PnpDevice returned no devices.".into());
        }
        Ok(Tree::from_nodes(nodes))
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.by_id.get(&id.to_uppercase()).map(|&i| &self.nodes[i])
    }

    /// Parent chain, nearest first (max 40 hops).
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut chain = vec![];
        let mut cur = id.to_string();
        while chain.len() < 40 {
            match self.get(&cur) {
                Some(n) if !n.parent.is_empty() => {
                    cur = n.parent.clone();
                    chain.push(cur.clone());
                }
                _ => break,
            }
        }
        chain
    }
}

pub fn has(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(&needle.to_lowercase())
}

pub fn has_any(text: &str, needles: &[&str]) -> bool {
    let t = text.to_lowercase();
    needles.iter().any(|n| t.contains(&n.to_lowercase()))
}

/// Case-insensitive whole-word match: a non-word character (or the start / end) on both sides of the needle.
pub fn has_word(text: &str, needle: &str) -> bool {
    let t = text.to_lowercase();
    let n = needle.to_lowercase();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = 0;
    while let Some(p) = t[start..].find(&n) {
        let at = start + p;
        let left_ok = t[..at].chars().next_back().map_or(true, |c| !is_word(c));
        let right_ok = t[at + n.len()..].chars().next().map_or(true, |c| !is_word(c));
        if left_ok && right_ok {
            return true;
        }
        start = at + 1;
        while !t.is_char_boundary(start) {
            start += 1;
        }
    }
    false
}

pub fn get_kind(text: &str) -> &'static str {
    // Specific words only: a bare "Controller" also matched "HID-compliant system controller" (mice, keyboards)
    // and "Wireless Adapter" matched Wi-Fi dongles.
    if has_any(text, &["Xbox", "Game Controller", "Wireless Controller", "Gamepad", "XINPUT", "DualSense", "DualShock", "8BitDo", "Joystick"]) {
        return "GAME CONTROLLER";
    }
    if has_any(text, &["Audio", "Headset", "Headphone", "Arctis", "Speaker", "Sound", "Microphone"]) || has_word(text, "Mic") || has_word(text, "DAC") {
        return "AUDIO";
    }
    if has(text, "Mouse") {
        return "mouse";
    }
    if has(text, "Keyboard") {
        return "keyboard";
    }
    // Brand-only hints come after mouse / keyboard: HyperX and SteelSeries also make mice and keyboards.
    if has_any(text, &["HyperX", "SteelSeries", "Logitech G"]) || has_word(text, "Astro") {
        return "AUDIO";
    }
    if has_any(text, &["Mass Storage", "Disk", "Flash", "Card Reader"]) || has_word(text, "UAS") {
        return "storage";
    }
    if has(text, "Bluetooth") {
        return "bluetooth";
    }
    if has(text, "Hub") {
        return "hub";
    }
    ""
}

#[derive(Clone, Debug)]
pub struct Controller {
    pub id: String,
    pub name: String,
    pub tier: u8,
    pub path: String,
    /// switched off in Device Manager (by PC Tuner or by hand)
    pub disabled: bool,
}

/// Device Manager problem code of a disabled device.
pub const PROBLEM_DISABLED: i64 = 22;

/// AMD Promontory chipset upstream ports above the controller: 2 = tier 4, 1 = tier 3; otherwise PCI depth decides.
pub fn controller_tier(tree: &Tree, ctl_id: &str) -> u8 {
    let anc = tree.ancestors(ctl_id);
    let hops = anc.iter().filter(|a| has(a, "VEN_1022&DEV_43F4")).count();
    let path = tree.get(ctl_id).map(|n| n.loc_path.as_str()).unwrap_or("");
    let depth = if path.is_empty() { 99 } else { path.to_uppercase().matches("#PCI(").count() };
    if hops >= 2 {
        4
    } else if hops == 1 {
        3
    } else if depth <= 2 {
        1
    } else if depth < 99 {
        2
    } else {
        9
    }
}

pub fn controllers(tree: &Tree) -> Vec<Controller> {
    tree.nodes
        .iter()
        .filter(|n| {
            n.id.to_uppercase().starts_with("PCI\\")
                && (n.class.eq_ignore_ascii_case("USB") || has(&n.name, "USB4"))
                && has_any(&n.name, &["Host Controller", "Host Router"])
        })
        .map(|n| Controller { id: n.id.clone(), name: n.name.clone(), tier: controller_tier(tree, &n.id), path: n.loc_path.clone(), disabled: n.disabled })
        .collect()
}

/// A USB4 host router is not a place devices plug into: USB devices on the USB4 ports enumerate under the
/// add-on chip's own xHCI controller, so the router itself always has 0 devices.
pub fn is_router(c: &Controller) -> bool {
    has(&c.name, "Host Router")
}

#[derive(Clone, Debug)]
pub struct UsbDevice {
    pub name: String,
    pub kind: &'static str,
    pub onboard: bool,
    pub tier: u8,
    pub controller: String,
    pub ctl_id: String,
    pub port: String,
    pub hubs: usize,
    pub id: String,
    pub functions: String,
}

/// Every physical USB device (USB\VID_ node that is not a hub) with its controller, tier, port and hub count.
pub fn usb_devices(tree: &Tree, ctls: &[Controller]) -> Vec<UsbDevice> {
    let ctl_by_id: HashMap<String, &Controller> = ctls.iter().map(|c| (c.id.to_uppercase(), c)).collect();
    let mut out = vec![];
    for d in &tree.nodes {
        let up = d.id.to_uppercase();
        // Only real plugs: a physical USB device always reports a "Port_#xxxx.Hub_#yyyy" location.
        if !up.starts_with("USB\\VID_") || up.contains("&MI_") || has(&d.name, "Root Hub") || !d.loc_info.to_lowercase().starts_with("port_#") {
            continue;
        }
        let anc = tree.ancestors(&d.id);
        let ctl = anc.iter().find_map(|a| ctl_by_id.get(&a.to_uppercase()).copied());
        // External / onboard hubs between the device and the controller's root hub
        let hubs = anc
            .iter()
            .filter(|a| a.to_uppercase().starts_with("USB\\VID_") && tree.get(a).map(|n| has(&n.name, "Hub")).unwrap_or(false))
            .count();
        // Child function names (composite devices show their real function in the children)
        let children: Vec<String> = tree
            .children
            .get(&up)
            .map(|v| v.iter().map(|&i| tree.nodes[i].name.clone()).collect())
            .unwrap_or_default();
        let name = if d.bus_desc.is_empty() { d.name.clone() } else { d.bus_desc.clone() };
        let kind = get_kind(&format!("{} {} {}", name, d.name, children.join(" ")));
        if kind == "hub" {
            continue;
        }
        // Built into the motherboard: MSI's own devices (VID 0DB0) and the onboard Wi-Fi/Bluetooth card.
        let onboard = up.contains("VID_0DB0&") || kind == "bluetooth";
        let mut uniq: Vec<String> = vec![];
        for c in children {
            if !uniq.iter().any(|u| u.eq_ignore_ascii_case(&c)) {
                uniq.push(c);
            }
        }
        out.push(UsbDevice {
            name,
            kind,
            onboard,
            tier: ctl.map(|c| c.tier).unwrap_or(9),
            controller: ctl.map(|c| c.name.clone()).unwrap_or_else(|| "?".into()),
            ctl_id: ctl.map(|c| c.id.clone()).unwrap_or_default(),
            port: d.loc_info.clone(),
            hubs,
            id: d.id.clone(),
            functions: uniq.join(", "),
        });
    }
    out
}

/// Devices plugged in below a controller right now.
pub fn devices_on(m: &Map, ctl_id: &str) -> usize {
    m.devices.iter().filter(|d| d.ctl_id.eq_ignore_ascii_case(ctl_id)).count()
}

/// Why a controller may not be switched off (None: it may). Only an empty, ordinary controller can be:
/// a USB4 router always shows 0 devices although devices on its ports run through it.
pub fn why_not_disable(m: &Map, c: &Controller) -> Option<&'static str> {
    if c.disabled {
        Some("It is already disabled.")
    } else if is_router(c) {
        Some("A USB4 router carries devices that show under another controller - it is never disabled.")
    } else if devices_on(m, &c.id) > 0 {
        Some("Something is plugged into it. Only controllers with no devices can be disabled.")
    } else {
        None
    }
}

/// Switch a USB controller on or off with Windows' own pnputil (administrator). Ok = a note for the user.
pub fn set_controller(id: &str, enable: bool) -> Result<String, String> {
    let verb = if enable { "/enable-device" } else { "/disable-device" };
    let (code, out) = common::run("pnputil", &[verb, id])?;
    match code {
        0 => Ok(String::new()),
        // ERROR_SUCCESS_REBOOT_REQUIRED
        3010 => Ok("Windows finishes this at the next restart.".into()),
        _ => Err(format!("pnputil {verb} failed (code {code}): {}", out.trim())),
    }
}

/// The full picture, sorted the way the script prints it.
pub struct Map {
    pub controllers: Vec<Controller>,
    pub devices: Vec<UsbDevice>,
    pub time: String,
}

pub fn build_map(tree: &Tree) -> Map {
    let mut controllers = controllers(tree);
    let mut devices = usb_devices(tree, &controllers);
    controllers.sort_by(|a, b| a.tier.cmp(&b.tier).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    devices.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(a.kind.to_lowercase().cmp(&b.kind.to_lowercase()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Map { controllers, devices, time: common::now() }
}

pub fn load_map() -> Result<Map, String> {
    Tree::load().map(|t| build_map(&t))
}

pub fn verdict(d: &UsbDevice) -> String {
    let mut v = match d.tier {
        1 => "already on a CPU port - nothing to do",
        2 => "on CPU lanes - fine",
        _ => "on a chipset port - move it to a Tier 1 port if one is free",
    }
    .to_string();
    if d.hubs > 0 {
        v += "; it is going through a hub - plug it into the PC directly";
    }
    v
}

// ------------------------------------------------------------------ plug-in test

/// Present USB\VID_* ids (no &MI_ interfaces), upper-cased, each with its parent (hub / root hub) at this moment.
/// The parent is recorded at detection time so a device that is moved while its port is being read is noticed.
pub fn present_usb_ids() -> Result<HashMap<String, String>, String> {
    #[derive(Deserialize)]
    struct Id {
        #[serde(rename = "Id", default)]
        id: Option<String>,
        #[serde(rename = "Parent", default)]
        parent: Option<String>,
    }
    let script = format!(
        "{PATH_REFRESH}\n$sp = @{{}}; try {{ $sp.CimSession = New-CimSession -ErrorAction Stop }} catch {{}}\nGet-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object {{ $_.InstanceId -like 'USB\\VID_*' -and $_.InstanceId -notmatch '&MI_' }} | ForEach-Object {{ $par = ''; try {{ $r = Invoke-CimMethod @sp -InputObject $_ -MethodName GetDeviceProperties -Arguments @{{ devicePropertyKeys = [string[]]@('DEVPKEY_Device_Parent') }} -ErrorAction Stop; $par = [string](@($r.deviceProperties)[0].Data) }} catch {{}}; [pscustomobject]@{{ Id = [string]$_.InstanceId; Parent = $par }} }}\nif ($sp.CimSession) {{ Remove-CimSession $sp.CimSession }}"
    );
    let v: Vec<Id> = common::ps_json(&script)?;
    Ok(v.into_iter().filter_map(|i| Some((i.id?.to_uppercase(), i.parent.unwrap_or_default().to_uppercase()))).collect())
}

pub struct LogEntry {
    pub n: usize,
    pub name: String,
    pub tier: u8,
    pub hubs: usize,
    pub controller: String,
    pub port: String,
    /// UI time the hit arrived (drives the highlight fade).
    pub at: f64,
}

pub struct TickResult {
    pub seen: HashSet<String>,
    pub found: Vec<UsbDevice>,
    pub map: Option<Map>,
    pub error: Option<String>,
}

/// What a tick does with a fresh id snapshot: either it is finished, or it has new ids (with their parent at
/// detection time) whose port must be read. Pure, so it can be tested without real devices.
pub enum Step {
    Done(TickResult),
    Lookup { seen: HashSet<String>, new: Vec<(String, String)> },
}

pub fn snapshot_step(mut seen: HashSet<String>, now: HashMap<String, String>, first: bool) -> Step {
    let done = |seen, error: Option<&str>| Step::Done(TickResult { seen, found: vec![], map: None, error: error.map(String::from) });
    if now.is_empty() {
        // The device being tested is always plugged in somewhere, so an empty read is a glitch, not "everything
        // was unplugged". On the first tick it must not become the baseline (every device would then look new).
        return if first { done(seen, Some("No USB devices were found yet - retrying the snapshot...")) } else { done(seen, None) };
    }
    if first {
        return done(now.into_keys().collect(), None);
    }
    let mut new: Vec<(String, String)> = now.iter().filter(|(i, _)| !seen.contains(*i)).map(|(i, p)| (i.clone(), p.clone())).collect();
    new.sort();
    seen.retain(|k| now.contains_key(k)); // unplugged -> forget, so re-plugging is detected
    if new.is_empty() {
        return done(seen, None);
    }
    Step::Lookup { seen, new }
}

/// Match new ids against a freshly read tree. Returns the devices to report and the ids that are settled.
/// An id is settled only when the tree shows it under the same parent it had when it was detected; one that is
/// gone (unplugged in transit) or sits under a different parent (moved while the tree was read) is left out, so
/// the next tick tries it again and its row gets the port it is really in.
pub fn resolve_hits(new: &[(String, String)], tree: &Tree, map: &Map) -> (Vec<UsbDevice>, Vec<String>) {
    let mut found = vec![];
    let mut settled = vec![];
    for (id, parent) in new {
        let Some(node) = tree.get(id) else { continue };
        if !parent.is_empty() && !node.parent.eq_ignore_ascii_case(parent) {
            continue;
        }
        settled.push(id.clone());
        if let Some(d) = map.devices.iter().find(|d| d.id.eq_ignore_ascii_case(id)) {
            found.push(d.clone());
        }
    }
    (found, settled)
}

/// One watch tick: compare the present ids with `seen`; for new ones wait, read the tree and report them.
/// `reading` is raised while the port of a new device is being read, so the UI can say so.
pub fn watch_tick(seen: HashSet<String>, first: bool, reading: Arc<AtomicBool>) -> TickResult {
    let now = match present_usb_ids() {
        Ok(n) => n,
        Err(e) => return TickResult { seen, found: vec![], map: None, error: Some(e) },
    };
    let (mut seen, new) = match snapshot_step(seen, now, first) {
        Step::Done(r) => return r,
        Step::Lookup { seen, new } => (seen, new),
    };
    reading.store(true, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(800)); // let Windows finish enumerating the device
    let r = match Tree::load() {
        Ok(tree) => {
            let map = build_map(&tree);
            let (found, settled) = resolve_hits(&new, &tree, &map);
            seen.extend(settled);
            TickResult { seen, found, map: Some(map), error: None }
        }
        Err(e) => TickResult { seen, found: vec![], map: None, error: Some(e) },
    };
    reading.store(false, Ordering::Relaxed);
    r
}

pub struct Watch {
    pub seen: HashSet<String>,
    pub seeded: bool,
    pub job: Option<Job<TickResult>>,
    pub next: Instant,
    pub count: usize,
    /// Set by the tick thread while it reads the port of a device that was just plugged in.
    pub reading: Arc<AtomicBool>,
}

impl Watch {
    /// The tick thread is reading the port of a device that was just plugged in.
    pub fn is_reading(&self) -> bool {
        self.reading.load(Ordering::Relaxed)
    }
}

// ------------------------------------------------------------------ page state

/// The USB Port Finder page's state: the device map, the running scan and the plug-in test.
pub struct Usb {
    pub map: Option<Map>,
    pub map_job: Option<Job<Result<Map, String>>>,
    /// When the running full scan started (drives the live seconds in the hero card).
    pub scan_started: Option<Instant>,
    pub error: Option<String>,
    pub watch: Option<Watch>,
    pub log: Vec<LogEntry>,
    pub watch_error: Option<String>,
    /// a controller being switched on / off: (its name, enable, the job)
    pub change: Option<(String, bool, Job<Result<String, String>>)>,
    /// the result of the last switch
    pub change_note: Option<(bool, String)>,
}

pub fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max.saturating_sub(3)).collect::<String>().trim_end())
    }
}

/// The game controller(s) and headset / audio devices on USB (not the ones built into the motherboard).
pub fn key_devices(m: &Map) -> Vec<&UsbDevice> {
    m.devices.iter().filter(|d| (d.kind == "GAME CONTROLLER" || d.kind == "AUDIO") && !d.onboard).collect()
}

impl Default for Usb {
    fn default() -> Self {
        Self::new()
    }
}

impl Usb {
    pub fn new() -> Self {
        let mut tab = Usb { map: None, map_job: None, scan_started: None, error: None, watch: None, log: vec![], watch_error: None, change: None, change_note: None };
        if crate::opwin::demo_stage("usb").is_some() {
            // sample plug-in test hits for screenshots (the newest one stays highlighted)
            let ctl = "AMD USB 3.10 eXtensible Host Controller - 1.20 (Microsoft)";
            tab.log.push(LogEntry { n: 1, name: "SanDisk Ultra".into(), tier: 3, hubs: 1, controller: ctl.replace("1.20", "1.10"), port: "Port_#0002.Hub_#0005".into(), at: 0.0 });
            tab.log.push(LogEntry { n: 2, name: "SanDisk Ultra".into(), tier: 1, hubs: 0, controller: ctl.into(), port: "Port_#0003.Hub_#0002".into(), at: f64::MAX });
        }
        tab
    }

    pub fn start_scan(&mut self) {
        self.error = None;
        self.scan_started = Some(Instant::now());
        self.map_job = Some(Job::spawn(load_map));
    }

    /// Disable (an empty controller) or enable a controller, in the background; the map is read again afterwards.
    pub fn switch_controller(&mut self, id: &str, enable: bool) -> Result<(), String> {
        if !common::is_admin() {
            return Err("Run PC Tuner as administrator to switch USB controllers.".into());
        }
        if self.change.is_some() || self.map_job.is_some() {
            return Err("Wait for the current change or scan to finish.".into());
        }
        let m = self.map.as_ref().ok_or("The USB tree has not been read yet.")?;
        let c = m.controllers.iter().find(|c| c.id.eq_ignore_ascii_case(id)).ok_or("That controller is no longer there - rescan.")?;
        if enable && !c.disabled {
            return Err("It is already on.".into());
        }
        if !enable {
            if let Some(why) = why_not_disable(m, c) {
                return Err(why.into());
            }
        }
        let id = c.id.clone();
        self.change_note = None;
        self.change = Some((c.name.clone(), enable, Job::spawn(move || set_controller(&id, enable))));
        Ok(())
    }

    pub fn pump(&mut self, now: f64) {
        if let Some(r) = self.change.as_ref().and_then(|(_, _, j)| j.poll()) {
            let (name, enable, _) = self.change.take().expect("checked above");
            let what = if enable { "enabled" } else { "disabled" };
            self.change_note = Some(match r {
                Ok(note) => (true, format!("{name} {what}. {note}").trim().to_string()),
                Err(e) => (false, format!("{name} could not be {what}: {e}")),
            });
            if self.map_job.is_none() {
                self.start_scan();
            }
        }
        if let Some(j) = &self.map_job {
            if let Some(r) = j.poll() {
                match r {
                    Ok(m) => self.map = Some(m),
                    Err(e) => self.error = Some(e),
                }
                self.map_job = None;
            }
        }
        let Some(w) = &mut self.watch else { return };
        if let Some(j) = &w.job {
            if let Some(r) = j.poll() {
                w.job = None;
                w.seen = r.seen;
                // only a valid snapshot becomes the baseline; after an error the next tick takes it again
                if r.error.is_none() {
                    w.seeded = true;
                }
                w.next = Instant::now() + Duration::from_millis(1500);
                self.watch_error = r.error;
                for d in r.found {
                    w.count += 1;
                    self.log.push(LogEntry { n: w.count, name: d.name, tier: d.tier, hubs: d.hubs, controller: d.controller, port: d.port, at: now });
                }
                if let Some(m) = r.map {
                    self.map = Some(m);
                }
            }
        }
        if w.job.is_none() && Instant::now() >= w.next {
            let seen = w.seen.clone();
            let first = !w.seeded;
            let reading = w.reading.clone();
            w.job = Some(Job::spawn(move || watch_tick(seen, first, reading)));
        }
    }

    /// "Start plug-in test": forget earlier hits and start watching for newly plugged-in devices.
    pub fn start_watch(&mut self) {
        self.log.clear();
        self.watch_error = None;
        self.watch = Some(Watch { seen: HashSet::new(), seeded: false, job: None, next: Instant::now(), count: 0, reading: Arc::new(AtomicBool::new(false)) });
    }

    /// "Stop plug-in test" (a running tick finishes in the background and is dropped).
    pub fn stop_watch(&mut self) {
        self.watch = None;
    }

    /// Start the first scan when nothing has been read yet, then poll. `now` = the UI clock in seconds
    /// (stamps new plug-in test hits so the UI can highlight them for a moment).
    pub fn tick(&mut self, now: f64) {
        if self.map.is_none() && self.map_job.is_none() && self.error.is_none() {
            self.start_scan();
        }
        self.pump(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_keywords() {
        assert_eq!(get_kind("Xbox Wireless Controller"), "GAME CONTROLLER");
        assert_eq!(get_kind("USB Mic"), "AUDIO");
        assert_eq!(get_kind("Mic"), "AUDIO");
        assert_eq!(get_kind("Mickey Mouse"), "mouse");
        assert_eq!(get_kind("Generic USB Hub"), "hub");
        assert_eq!(get_kind("SanDisk Flash"), "storage");
        assert_eq!(get_kind("thing"), "");
        // false positives found in testing
        assert_eq!(get_kind("Microsoft Ergonomic Keyboard"), "keyboard");
        assert_eq!(get_kind("HyperX Pulsefire Haste HID-compliant mouse"), "mouse");
        assert_eq!(get_kind("SteelSeries Apex 7 HID Keyboard Device"), "keyboard");
        assert_eq!(get_kind("TP-Link Wireless Adapter 802.11ac"), "");
        assert_ne!(get_kind("USB Input Device HID-compliant system controller"), "GAME CONTROLLER");
        assert_eq!(get_kind("USB Input Device HID-compliant system controller"), "");
        assert_eq!(get_kind("USB Input Device HID-compliant game controller"), "GAME CONTROLLER");
        assert_eq!(get_kind("Xbox Wireless Adapter for Windows"), "GAME CONTROLLER");
        assert_eq!(get_kind("Sony Wireless Controller"), "GAME CONTROLLER");
        assert_eq!(get_kind("HyperX Cloud Alpha Wireless"), "AUDIO");
        assert_eq!(get_kind("Astro A50 Voice"), "AUDIO");
        assert_eq!(get_kind("FiiO K5 Pro DAC"), "AUDIO");
        assert_eq!(get_kind("Dacia thing"), "");
        assert_eq!(get_kind("USB Attached SCSI (UAS) Device"), "storage");
        assert_eq!(get_kind("Guaspari"), "");
    }

    #[test]
    fn word_boundaries() {
        assert!(has_word("USB Mic", "mic"));
        assert!(has_word("Mic (Arctis)", "Mic"));
        assert!(!has_word("Ergonomic", "Mic"));
        assert!(!has_word("Microsoft", "Mic"));
        assert!(has_word("ergonomic mic", "Mic")); // a later whole-word hit after a partial one
    }

    fn node(id: &str, name: &str, class: &str, parent: &str, loc_info: &str, loc_path: &str) -> Node {
        Node { id: id.into(), name: name.into(), class: class.into(), parent: parent.into(), loc_info: loc_info.into(), loc_path: loc_path.into(), bus_desc: String::new(), disabled: false }
    }

    /// Two xHCI controllers (one CPU, one chipset), a USB4 host router and one flash drive.
    fn sample_tree(drive_parent: &str) -> Tree {
        Tree::from_nodes(vec![
            node("PCI\\ROOT", "PCI root", "System", "", "", "PCIROOT(0)"),
            node("PCI\\VEN_1022&DEV_15B6\\CPU", "AMD USB 3.10 eXtensible Host Controller - 1.10 (Microsoft)", "USB", "PCI\\ROOT", "", "PCIROOT(0)#PCI(0801)#PCI(0003)"),
            node("PCI\\VEN_1022&DEV_43F4\\UP", "Promontory upstream", "System", "PCI\\ROOT", "", "PCIROOT(0)#PCI(0201)"),
            node("PCI\\VEN_1022&DEV_43FD\\CHIP", "AMD USB 3.20 eXtensible Host Controller - 1.20 (Microsoft)", "USB", "PCI\\VEN_1022&DEV_43F4\\UP", "", "PCIROOT(0)#PCI(0201)#PCI(0000)#PCI(0C00)#PCI(0000)"),
            node("PCI\\VEN_8086&DEV_ROUTER\\R", "USB4(TM) Host Router (Microsoft)", "System", "PCI\\ROOT", "", "PCIROOT(0)#PCI(0101)#PCI(0000)"),
            node("USB\\ROOT_HUB30\\A", "USB Root Hub (USB 3.0)", "USB", "PCI\\VEN_1022&DEV_15B6\\CPU", "", ""),
            node("USB\\ROOT_HUB30\\B", "USB Root Hub (USB 3.0)", "USB", "PCI\\VEN_1022&DEV_43FD\\CHIP", "", ""),
            node("USB\\VID_0781&PID_5581\\SERIAL1", "USB Mass Storage Device", "USB", drive_parent, "Port_#0002.Hub_#0001", ""),
        ])
    }

    #[test]
    fn router_is_not_counted_as_a_place_to_plug_in() {
        let m = build_map(&sample_tree("USB\\ROOT_HUB30\\A"));
        assert_eq!(m.controllers.len(), 3);
        assert_eq!(m.controllers.iter().filter(|c| !is_router(c)).count(), 2);
        assert_eq!(m.devices.len(), 1);
        assert_eq!(m.devices[0].tier, 1);
    }

    #[test]
    fn hit_is_settled_only_where_it_was_detected() {
        let id = "USB\\VID_0781&PID_5581\\SERIAL1".to_string();
        // still on the port it was detected on -> reported and settled
        let t = sample_tree("USB\\ROOT_HUB30\\A");
        let m = build_map(&t);
        let (found, settled) = resolve_hits(&[(id.clone(), "USB\\ROOT_HUB30\\A".into())], &t, &m);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tier, 1);
        assert_eq!(settled, vec![id.clone()]);
        // moved to the chipset port while the tree was read -> not reported, retried next tick
        let t = sample_tree("USB\\ROOT_HUB30\\B");
        let m = build_map(&t);
        let (found, settled) = resolve_hits(&[(id.clone(), "USB\\ROOT_HUB30\\A".into())], &t, &m);
        assert!(found.is_empty() && settled.is_empty());
        // unplugged in transit -> not reported, not settled
        let (found, settled) = resolve_hits(&[("USB\\VID_1234&PID_0001\\GONE".into(), String::new())], &t, &m);
        assert!(found.is_empty() && settled.is_empty());
    }

    #[test]
    fn snapshot_rules() {
        let ids = |v: &[&str]| -> HashMap<String, String> { v.iter().map(|s| (s.to_string(), "P".to_string())).collect() };
        // an empty first snapshot is an error and does not become the baseline
        match snapshot_step(HashSet::new(), HashMap::new(), true) {
            Step::Done(r) => assert!(r.error.is_some() && r.seen.is_empty()),
            _ => panic!("expected Done"),
        }
        // a valid first snapshot is the baseline
        let seen = match snapshot_step(HashSet::new(), ids(&["A", "B"]), true) {
            Step::Done(r) => {
                assert!(r.error.is_none());
                r.seen
            }
            _ => panic!("expected Done"),
        };
        assert_eq!(seen.len(), 2);
        // an empty read mid-run keeps the baseline instead of clearing it
        let seen = match snapshot_step(seen, HashMap::new(), false) {
            Step::Done(r) => {
                assert!(r.error.is_none());
                r.seen
            }
            _ => panic!("expected Done"),
        };
        assert_eq!(seen.len(), 2);
        // A unplugged, C plugged in -> C is looked up, A forgotten
        match snapshot_step(seen, ids(&["B", "C"]), false) {
            Step::Lookup { seen, new } => {
                assert_eq!(new, vec![("C".to_string(), "P".to_string())]);
                assert!(seen.contains("B") && !seen.contains("A") && !seen.contains("C"));
            }
            _ => panic!("expected Lookup"),
        }
    }

    /// Read-only: builds the real device tree and prints it (`cargo test usb -- --nocapture`).
    #[test]
    fn usb_tree_live() {
        let t0 = Instant::now();
        let tree = Tree::load().expect("tree");
        let m = build_map(&tree);
        println!("read {} nodes in {:.1} s", tree.nodes.len(), t0.elapsed().as_secs_f32());
        println!("\nCONTROLLERS");
        for c in &m.controllers {
            let n = m.devices.iter().filter(|d| d.ctl_id.eq_ignore_ascii_case(&c.id)).count();
            println!("  {:<42} {}  ({} device(s))\n      {}", tier_text(c.tier), c.name, n, c.id);
        }
        println!("\nDEVICES");
        for d in &m.devices {
            let mut note = if d.hubs > 0 { format!("  via {} hub(s)", d.hubs) } else { String::new() };
            if d.onboard {
                note += "  (onboard)";
            }
            println!("  Tier {}  {:<18} {}\n      {}  {}{}", d.tier, if d.kind.is_empty() { String::new() } else { format!("[{}]", d.kind) }, d.name, d.controller, d.port, note);
        }
        println!("\nKEY");
        for d in m.devices.iter().filter(|d| (d.kind == "GAME CONTROLLER" || d.kind == "AUDIO") && !d.onboard) {
            println!("  {}: {} -> {}", d.kind, d.name, verdict(d));
        }
        assert!(!m.controllers.is_empty());
        // the plug-in test's snapshot: same ids as the tree, each with the parent the tree has
        let t0 = Instant::now();
        let ids = present_usb_ids().expect("ids");
        println!("\nsnapshot of {} USB ids in {:.1} s", ids.len(), t0.elapsed().as_secs_f32());
        assert!(!ids.is_empty());
        for (id, parent) in &ids {
            let n = tree.get(id).expect("id in tree");
            assert!(n.parent.eq_ignore_ascii_case(parent), "{id}: {parent} vs {}", n.parent);
        }
    }

    #[test]
    fn usb_which_controllers_can_be_disabled() {
        let ctl = |id: &str, name: &str, disabled: bool| Controller { id: id.into(), name: name.into(), tier: 3, path: String::new(), disabled };
        let dev = |ctl_id: &str| UsbDevice { name: "Mouse".into(), kind: "mouse", onboard: false, tier: 3, controller: String::new(), ctl_id: ctl_id.into(), port: String::new(), hubs: 0, id: r"USB\VID_1".into(), functions: String::new() };
        let m = Map {
            controllers: vec![ctl(r"PCI\A", "AMD USB 3.10 eXtensible Host Controller", false), ctl(r"PCI\B", "AMD USB 3.10 eXtensible Host Controller", false), ctl(r"PCI\R", "USB4 Host Router", false), ctl(r"PCI\D", "AMD USB 3.20 eXtensible Host Controller", true)],
            devices: vec![dev(r"pci\a")],
            time: String::new(),
        };
        assert_eq!(devices_on(&m, r"PCI\A"), 1, "ids compare without case");
        assert!(why_not_disable(&m, &m.controllers[0]).unwrap().contains("plugged into it"));
        assert_eq!(why_not_disable(&m, &m.controllers[1]), None, "empty: may be disabled");
        assert!(why_not_disable(&m, &m.controllers[2]).unwrap().contains("never disabled"), "USB4 router");
        assert!(why_not_disable(&m, &m.controllers[3]).unwrap().contains("already disabled"));
        // the switch itself needs administrator and a read map (the tests never run pnputil)
        let mut u = Usb::new();
        assert!(u.switch_controller(r"PCI\B", false).is_err());
        assert!(u.change.is_none());
    }
}
