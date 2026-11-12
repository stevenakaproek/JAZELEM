//! USB Port Finder tab - port of USB-PortFinder.ps1.
//! Shows which USB controller every device is plugged into, ranked by how close that controller sits to the CPU,
//! plus a plug-in test that tells you the tier of the port a device was just plugged into. Changes nothing.

use crate::common::{self, color, Job, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const PATH_REFRESH: &str = "$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')";

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

pub fn tier_color(t: u8) -> egui::Color32 {
    match t {
        1 => color::OK,
        2 => color::OPTIONAL,
        3 => color::GAMING,
        4 => color::JUNK,
        _ => color::DIM,
    }
}

// ------------------------------------------------------------------ device tree

#[derive(Deserialize, Clone, Debug)]
struct RawNode {
    #[serde(rename = "Id", default)]
    id: Option<String>,
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "Class", default)]
    class: Option<String>,
    #[serde(rename = "Parent", default)]
    parent: Option<String>,
    #[serde(rename = "LocInfo", default)]
    loc_info: Option<String>,
    #[serde(rename = "LocPath", default)]
    loc_path: Option<String>,
    #[serde(rename = "BusDesc", default)]
    bus_desc: Option<String>,
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
}

/// All present PnP devices. Lookups are case-insensitive (keys upper-cased), like PowerShell hashtables.
pub struct Tree {
    pub nodes: Vec<Node>,
    by_id: HashMap<String, usize>,
    children: HashMap<String, Vec<usize>>,
}

const TREE_SCRIPT: &str = r#"
$all = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue)
$props = @($all | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_Parent, DEVPKEY_Device_LocationInfo, DEVPKEY_Device_LocationPaths, DEVPKEY_Device_BusReportedDeviceDesc -ErrorAction SilentlyContinue)
$parent = @{}; $locInfo = @{}; $locPath = @{}; $busDesc = @{}
foreach ($p in $props) {
    switch ($p.KeyName) {
        'DEVPKEY_Device_Parent'               { $parent[$p.InstanceId] = [string]$p.Data }
        'DEVPKEY_Device_LocationInfo'         { $locInfo[$p.InstanceId] = [string]$p.Data }
        'DEVPKEY_Device_LocationPaths'        { $locPath[$p.InstanceId] = [string](@($p.Data)[0]) }
        'DEVPKEY_Device_BusReportedDeviceDesc'{ $busDesc[$p.InstanceId] = [string]$p.Data }
    }
}
foreach ($d in $all) {
    $i = $d.InstanceId
    [pscustomobject]@{ Id = [string]$i; Name = [string]$d.FriendlyName; Class = [string]$d.Class; Parent = [string]$parent[$i]; LocInfo = [string]$locInfo[$i]; LocPath = [string]$locPath[$i]; BusDesc = [string]$busDesc[$i] }
}
"#;

impl Tree {
    fn from_nodes(nodes: Vec<Node>) -> Tree {
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

    /// One PowerShell call (~8 s): every present device plus parent / location / bus description.
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
                })
            })
            .collect();
        if nodes.is_empty() {
            return Err("Get-PnpDevice returned no devices.".into());
        }
        Ok(Tree::from_nodes(nodes))
    }

    fn get(&self, id: &str) -> Option<&Node> {
        self.by_id.get(&id.to_uppercase()).map(|&i| &self.nodes[i])
    }

    /// Parent chain, nearest first (max 40 hops).
    fn ancestors(&self, id: &str) -> Vec<String> {
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

fn has(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(&needle.to_lowercase())
}

fn has_any(text: &str, needles: &[&str]) -> bool {
    let t = text.to_lowercase();
    needles.iter().any(|n| t.contains(&n.to_lowercase()))
}

/// Case-insensitive "word\b" match: the needle followed by a non-word character or the end.
fn has_word_end(text: &str, needle: &str) -> bool {
    let t = text.to_lowercase();
    let n = needle.to_lowercase();
    let mut start = 0;
    while let Some(p) = t[start..].find(&n) {
        let end = start + p + n.len();
        match t[end..].chars().next() {
            None => return true,
            Some(c) if !(c.is_alphanumeric() || c == '_') => return true,
            _ => {}
        }
        start = start + p + 1;
        while !t.is_char_boundary(start) {
            start += 1;
        }
    }
    false
}

pub fn get_kind(text: &str) -> &'static str {
    if has_any(text, &["Xbox", "Controller", "Gamepad", "XINPUT", "DualSense", "DualShock", "8BitDo", "Joystick", "Wireless Adapter"]) {
        return "GAME CONTROLLER";
    }
    if has_any(text, &["Audio", "Headset", "Headphone", "Arctis", "DAC", "Speaker", "Sound", "Microphone", "HyperX", "Astro", "SteelSeries", "Logitech G"])
        || has_word_end(text, "Mic")
    {
        return "AUDIO";
    }
    if has(text, "Mouse") {
        return "mouse";
    }
    if has(text, "Keyboard") {
        return "keyboard";
    }
    if has_any(text, &["Mass Storage", "Disk", "Flash", "Card Reader", "UAS"]) {
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
}

/// AMD Promontory chipset upstream ports above the controller: 2 = tier 4, 1 = tier 3; otherwise PCI depth decides.
fn controller_tier(tree: &Tree, ctl_id: &str) -> u8 {
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
        .map(|n| Controller { id: n.id.clone(), name: n.name.clone(), tier: controller_tier(tree, &n.id), path: n.loc_path.clone() })
        .collect()
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

fn load_map() -> Result<Map, String> {
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

/// Present USB\VID_* ids (no &MI_ interfaces), upper-cased.
fn present_usb_ids() -> Result<HashSet<String>, String> {
    #[derive(Deserialize)]
    struct Id {
        #[serde(rename = "Id", default)]
        id: Option<String>,
    }
    let script = format!(
        "{PATH_REFRESH}\nGet-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object {{ $_.InstanceId -like 'USB\\VID_*' -and $_.InstanceId -notmatch '&MI_' }} | ForEach-Object {{ [pscustomobject]@{{ Id = [string]$_.InstanceId }} }}"
    );
    let v: Vec<Id> = common::ps_json(&script)?;
    Ok(v.into_iter().filter_map(|i| i.id).map(|s| s.to_uppercase()).collect())
}

struct LogEntry {
    n: usize,
    name: String,
    tier: u8,
    hubs: usize,
    controller: String,
    port: String,
    /// UI time the hit arrived (drives the highlight fade).
    at: f64,
}

struct TickResult {
    seen: HashSet<String>,
    found: Vec<UsbDevice>,
    map: Option<Map>,
    error: Option<String>,
}

/// One watch tick: compare the present ids with `seen`; for new ones wait, rebuild the tree and report them.
fn watch_tick(mut seen: HashSet<String>, first: bool) -> TickResult {
    let now = match present_usb_ids() {
        Ok(n) => n,
        Err(e) => return TickResult { seen, found: vec![], map: None, error: Some(e) },
    };
    if first {
        return TickResult { seen: now, found: vec![], map: None, error: None };
    }
    let new: Vec<String> = now.iter().filter(|i| !seen.contains(*i)).cloned().collect();
    seen.retain(|k| now.contains(k)); // unplugged -> forget, so re-plugging is detected
    if new.is_empty() {
        return TickResult { seen, found: vec![], map: None, error: None };
    }
    std::thread::sleep(Duration::from_millis(800)); // let Windows finish enumerating the device
    let (found, map, error) = match Tree::load() {
        Ok(tree) => {
            let map = build_map(&tree);
            let found: Vec<UsbDevice> = map.devices.iter().filter(|d| new.contains(&d.id.to_uppercase())).cloned().collect();
            (found, Some(map), None)
        }
        Err(e) => (vec![], None, Some(e)),
    };
    seen.extend(new);
    TickResult { seen, found, map, error }
}

struct Watch {
    seen: HashSet<String>,
    seeded: bool,
    job: Option<Job<TickResult>>,
    next: Instant,
    count: usize,
}

// ------------------------------------------------------------------ tab

pub struct UsbTab {
    map: Option<Map>,
    map_job: Option<Job<Result<Map, String>>>,
    error: Option<String>,
    watch: Option<Watch>,
    log: Vec<LogEntry>,
    watch_error: Option<String>,
}

/// Seconds a new plug-in test hit stays highlighted.
const HIT_GLOW: f64 = 2.5;

/// A chip in a fixed-width column, so the names after it line up.
fn chip_col(ui: &mut egui::Ui, width: f32, text: &str, c: egui::Color32) {
    ui.allocate_ui_with_layout(egui::vec2(width, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        theme::chip(ui, text, c);
    });
}

/// A hero-row stat that is left out when the window is too narrow for it (instead of wrapping letter by letter).
fn stat_fit(ui: &mut egui::Ui, gap: f32, value: &str, label: &str, c: egui::Color32) {
    if ui.available_width() < gap + 7.0 * label.chars().count().max(value.chars().count() * 2) as f32 {
        return;
    }
    ui.add_space(gap);
    theme::stat(ui, value, label, c);
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max.saturating_sub(3)).collect::<String>().trim_end())
    }
}

/// The game controller(s) and headset / audio devices on USB (not the ones built into the motherboard).
fn key_devices(m: &Map) -> Vec<&UsbDevice> {
    m.devices.iter().filter(|d| (d.kind == "GAME CONTROLLER" || d.kind == "AUDIO") && !d.onboard).collect()
}

impl UsbTab {
    pub fn new() -> Self {
        let mut tab = UsbTab { map: None, map_job: None, error: None, watch: None, log: vec![], watch_error: None };
        if crate::opwin::demo_stage("usb").is_some() {
            // sample plug-in test hits for screenshots (the newest one stays highlighted)
            let ctl = "AMD USB 3.10 eXtensible Host Controller - 1.20 (Microsoft)";
            tab.log.push(LogEntry { n: 1, name: "SanDisk Ultra".into(), tier: 3, hubs: 1, controller: ctl.replace("1.20", "1.10"), port: "Port_#0002.Hub_#0005".into(), at: 0.0 });
            tab.log.push(LogEntry { n: 2, name: "SanDisk Ultra".into(), tier: 1, hubs: 0, controller: ctl.into(), port: "Port_#0003.Hub_#0002".into(), at: f64::MAX });
        }
        tab
    }

    fn start_scan(&mut self) {
        self.error = None;
        self.map_job = Some(Job::spawn(load_map));
    }

    fn pump(&mut self, now: f64) {
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
                w.seeded = true;
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
            w.job = Some(Job::spawn(move || watch_tick(seen, first)));
        }
    }

    /// Hero card: the tier your controller and headset sit on.
    fn hero_ui(&self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(m) = &self.map else {
                if let Some(e) = &self.error {
                    theme::hero(ui, "?", "controller and headset", "The USB tree could not be read", theme::LOCKED);
                    ui.colored_label(theme::LOCKED, e);
                } else {
                    theme::hero(ui, "...", "controller and headset", "Reading the USB tree... (about 8 seconds)", theme::TEXT_FAINT);
                    theme::sweep(ui, ui.available_width());
                }
                return;
            };
            let key = key_devices(m);
            ui.horizontal(|ui| {
                if key.is_empty() {
                    theme::hero(ui, "-", "no controller or headset on USB", "A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.", theme::TEXT_DIM);
                } else {
                    let worst = key.iter().map(|d| d.tier).max().unwrap_or(9);
                    let same = key.iter().all(|d| d.tier == worst);
                    let has_pad = key.iter().any(|d| d.kind == "GAME CONTROLLER");
                    let has_audio = key.iter().any(|d| d.kind == "AUDIO");
                    let who = match (has_pad, has_audio) {
                        (true, true) => "controller and headset",
                        (true, false) => "game controller",
                        _ => "headset / audio",
                    };
                    let mut caption = match worst {
                        1 => "on CPU ports - nothing to do",
                        2 => "on CPU lanes - fine",
                        3 | 4 => "on a chipset port - move it to a Tier 1 port if one is free",
                        _ => "on a controller this tool does not recognise",
                    }
                    .to_string();
                    if !same {
                        caption = format!("the slowest one is {caption}");
                    }
                    if key.iter().any(|d| d.hubs > 0) {
                        caption += "  ·  one goes through a hub";
                    }
                    let value = if worst <= 4 { format!("Tier {worst}") } else { "Tier ?".to_string() };
                    theme::hero(ui, &value, who, &caption, tier_color(worst));
                }
                ui.add_space(8.0);
                for d in key.iter().take(2) {
                    stat_fit(ui, 22.0, &format!("Tier {}", d.tier), &shorten(&d.name, 24), tier_color(d.tier));
                }
                stat_fit(ui, 22.0, &m.controllers.len().to_string(), "USB controllers", theme::TEXT);
                stat_fit(ui, 22.0, &m.devices.len().to_string(), "USB devices", theme::TEXT);
            });
        });
    }

    /// Plug-in test card: instructions, start / stop, and one highlighted row per port tried.
    fn test_ui(&mut self, ui: &mut egui::Ui, now: f64) {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Plug-in test");
            ui.label(RichText::new("Find out which physical socket is which tier: plug any USB device (a flash drive or your controller) into a port, wait 2 seconds, then move it to the next port.").color(theme::TEXT_DIM));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let watching = self.watch.is_some();
                let clicked = if watching { ui.button("Stop plug-in test").clicked() } else { ui.add(theme::primary_button("   Start plug-in test   ")).clicked() };
                if clicked {
                    if watching {
                        self.watch = None; // a running tick finishes in the background and is dropped
                    } else {
                        self.log.clear();
                        self.watch_error = None;
                        self.watch = Some(Watch { seen: HashSet::new(), seeded: false, job: None, next: Instant::now(), count: 0 });
                    }
                }
                match &self.watch {
                    Some(w) => {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new(if w.seeded { "Watching for new devices..." } else { "Taking a snapshot of the connected devices..." }).color(theme::TEXT_DIM));
                    }
                    None if !self.log.is_empty() => {
                        ui.label(RichText::new(format!("{} port(s) tested", self.log.len())).color(theme::TEXT_DIM));
                    }
                    None => {}
                }
            });
            if let Some(e) = &self.watch_error {
                ui.colored_label(theme::LOCKED, e);
            }
            // newest first; a fresh hit glows in its tier colour and fades
            for l in self.log.iter().rev() {
                let glow = (1.0 - (now - l.at) / HIT_GLOW).clamp(0.0, 1.0) as f32;
                let c = tier_color(l.tier);
                egui::Frame::none()
                    .fill(theme::tint(c, (12.0 + 22.0 * glow) as u8))
                    .stroke(egui::Stroke::new(1.0_f32, theme::tint(c, (50.0 + 170.0 * glow) as u8)))
                    .rounding(theme::R_WIDGET)
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("#{}", l.n)).color(theme::TEXT_DIM).strong());
                            ui.label(RichText::new(&l.name).strong().color(theme::TEXT));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("this port = {}", tier_text(l.tier))).strong().color(c.lerp_to_gamma(theme::TEXT, 0.85 * glow)));
                            });
                        });
                        let hub = if l.hubs > 0 { format!("  ·  through {} hub(s) - a front-panel or monitor port", l.hubs) } else { String::new() };
                        ui.label(RichText::new(format!("{}  ·  {}{}", l.controller, l.port, hub)).size(12.0).color(theme::TEXT_DIM));
                    });
                if glow > 0.0 {
                    ui.ctx().request_repaint();
                }
            }
        });
    }

    fn map_ui(&self, ui: &mut egui::Ui, m: &Map) {
        // ---- the devices that matter for latency
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Your game controller and audio");
            let key = key_devices(m);
            if key.is_empty() {
                ui.label(RichText::new("None found on USB. (A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.)").color(theme::TEXT_DIM));
            }
            for d in key {
                ui.horizontal(|ui| {
                    chip_col(ui, 74.0, &format!("Tier {}", d.tier), tier_color(d.tier));
                    ui.label(RichText::new(&d.name).strong().color(theme::TEXT));
                    ui.label(RichText::new(verdict(d)).color(theme::TEXT_DIM));
                });
            }
        });
        ui.add_space(4.0);

        // ---- controllers
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "USB controllers");
                ui.label(RichText::new("closest to the CPU first").size(12.0).color(theme::TEXT_DIM));
            });
            for c in &m.controllers {
                let n = m.devices.iter().filter(|d| d.ctl_id.eq_ignore_ascii_case(&c.id)).count();
                ui.horizontal(|ui| {
                    chip_col(ui, 150.0, tier_short(c.tier), tier_color(c.tier));
                    ui.label(RichText::new(&c.name).color(theme::TEXT));
                    ui.label(RichText::new(if n == 1 { "1 device".to_string() } else { format!("{n} devices") }).color(if n == 0 { theme::TEXT_FAINT } else { theme::TEXT_DIM }));
                })
                .response
                .on_hover_text(format!("{}\n{}", c.id, c.path));
            }
        });
        ui.add_space(4.0);

        // ---- devices
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Connected devices");
            for d in &m.devices {
                ui.horizontal(|ui| {
                    chip_col(ui, 74.0, &format!("Tier {}", d.tier), tier_color(d.tier));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&d.name).color(theme::TEXT));
                            if !d.kind.is_empty() {
                                let key = d.kind == "GAME CONTROLLER" || d.kind == "AUDIO";
                                theme::chip(ui, &d.kind.to_lowercase(), if key { theme::ACCENT } else { theme::DIM });
                            }
                            if d.hubs > 0 {
                                theme::chip(ui, &format!("via {} hub(s)", d.hubs), theme::JUNK);
                            }
                            if d.onboard {
                                theme::chip(ui, "built in - cannot be moved", theme::DIM);
                            }
                        });
                        ui.label(RichText::new(format!("{}  ·  {}", d.controller, d.port)).size(12.0).color(theme::TEXT_FAINT));
                    });
                })
                .response
                .on_hover_text(format!("{}\n{}", d.id, d.functions));
                ui.add_space(2.0);
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new("Good to know: the latency difference between tiers is a few microseconds. What matters more is plugging straight into the PC (no hub, front panel or monitor port) and not sharing a port group with a busy webcam or drive.")
                .size(12.5)
                .color(theme::TEXT_DIM),
        );
    }
}

impl Tab for UsbTab {
    fn title(&self) -> &'static str {
        "USB Port Finder"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if self.map.is_none() && self.map_job.is_none() && self.error.is_none() {
            self.start_scan();
        }
        let now = ui.input(|i| i.time);
        self.pump(now);

        theme::page_header(ui, "USB Port Finder", "Which USB controller every device is plugged into, ranked by how close it sits to the CPU. Changes nothing.");
        self.hero_ui(ui);

        let busy = self.map_job.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Rescan")).clicked() {
                self.start_scan();
            }
            if busy {
                if self.map.is_some() {
                    // the first read shows its sweep in the hero card
                    ui.label(RichText::new("Reading the USB tree...").color(theme::TEXT_DIM));
                    theme::sweep(ui, 220.0);
                }
            } else if let Some(m) = &self.map {
                ui.label(RichText::new(format!("read {}", m.time)).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(2.0);

        // screenshots only: PCTUNER_DEMO=usb:done starts scrolled down to the device list
        let mut area = egui::ScrollArea::vertical().id_salt("usb-scroll").auto_shrink([false, false]);
        if crate::opwin::demo_stage("usb") == Some(crate::opwin::Stage::Done) {
            area = area.vertical_scroll_offset(470.0);
        }
        area.show(ui, |ui| {
            self.test_ui(ui, now);
            ui.add_space(4.0);
            if let Some(m) = &self.map {
                self.map_ui(ui, m);
            }
        });
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
    }
}
