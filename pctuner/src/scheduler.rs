//! CPU Scheduler tab - the Multimedia Class Scheduler (MMCSS) settings, edited directly:
//! HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile and its Tasks.
//! Shows what is set now, what differs from the Windows default, and what each save changed.

use crate::common::{self, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use winreg::enums::*;
use winreg::RegKey;

const SYS: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
const SYS_REG: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
const HISTORY: &str = "mmcss-history.json";
const DISABLED: u32 = 0xFFFF_FFFF;

/// Pseudo task names for the two settings outside the multimedia profile.
const PRIO: &str = "#prio"; // HKLM ...\PriorityControl  Win32PrioritySeparation
const FG: &str = "#fg"; // HKCU Control Panel\Desktop  ForegroundLockTimeout
const PRIO_KEY: &str = r"SYSTEM\CurrentControlSet\Control\PriorityControl";
const FG_KEY: &str = r"Control Panel\Desktop";
const PRIO_NAME: &str = "Win32PrioritySeparation";
const FG_NAME: &str = "ForegroundLockTimeout";

/// The Windows tasks, in the order Windows lists them.
const TASKS: &[&str] = &["Audio", "Capture", "DisplayPostProcessing", "Distribution", "Games", "Playback", "Pro Audio", "Window Manager"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegVal {
    D(u32),
    S(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    Dword,
    Text,
}

/// (registry value name, label, type)
const SYS_FIELDS: &[(&str, &str, Ty)] = &[("NetworkThrottlingIndex", "Network throttling index", Ty::Dword), ("SystemResponsiveness", "System responsiveness reserved CPU", Ty::Dword)];
const TASK_FIELDS: &[(&str, &str, Ty)] = &[
    ("Affinity", "Affinity", Ty::Dword),
    ("Clock Rate", "Clock rate", Ty::Dword),
    ("GPU Priority", "GPU priority", Ty::Dword),
    ("Priority", "Priority", Ty::Dword),
    ("Scheduling Category", "Scheduling category", Ty::Text),
    ("SFIO Priority", "I/O priority", Ty::Text),
    ("Background Only", "Background only", Ty::Text),
    ("BackgroundPriority", "Background priority", Ty::Dword),
];

/// A setting: "" = SystemProfile itself, otherwise the task name; then the value name.
type Key = (String, &'static str);

/// Windows defaults (Windows 10 / 11).
fn default_of(task: &str, name: &str) -> Option<RegVal> {
    let d = |v: u32| Some(RegVal::D(v));
    let s = |v: &str| Some(RegVal::S(v.to_string()));
    match task {
        PRIO => return d(2),
        FG => return d(200000),
        _ => {}
    }
    if task.is_empty() {
        return match name {
            "NetworkThrottlingIndex" => d(10),
            "SystemResponsiveness" => d(20),
            _ => None,
        };
    }
    let (prio, cat, bg_only, bg_prio) = match task {
        "Audio" => (6, "Medium", "True", None),
        "Capture" => (5, "Medium", "True", None),
        "DisplayPostProcessing" => (8, "High", "True", Some(8)),
        "Distribution" => (4, "Medium", "True", None),
        "Games" => (2, "Medium", "False", None),
        "Playback" => (3, "Medium", "False", Some(4)),
        "Pro Audio" => (1, "High", "False", None),
        "Window Manager" => (5, "Medium", "True", None),
        _ => return None,
    };
    match name {
        "Affinity" => d(0),
        "Clock Rate" => d(10000),
        "GPU Priority" => d(8),
        "Priority" => d(prio),
        "Scheduling Category" => s(cat),
        "SFIO Priority" => s("Normal"),
        "Background Only" => s(bg_only),
        "BackgroundPriority" => bg_prio.map(RegVal::D),
        _ => None,
    }
}

/// How a value is shown.
fn show(name: &str, v: &Option<RegVal>) -> String {
    match (name, v) {
        (_, None) => "(not set)".into(),
        ("NetworkThrottlingIndex", Some(RegVal::D(DISABLED))) => "Disabled".into(),
        ("SystemResponsiveness", Some(RegVal::D(n))) => format!("{n}%"),
        ("Affinity", Some(RegVal::D(0))) => "all".into(),
        ("Affinity", Some(RegVal::D(m))) => format!("0x{m:X}"),
        (PRIO_NAME, Some(RegVal::D(m))) => format!("0x{m:X}"),
        (FG_NAME, Some(RegVal::D(n))) => format!("{n} ms"),
        (_, Some(RegVal::D(n))) => n.to_string(),
        (_, Some(RegVal::S(s))) => s.clone(),
    }
}

/// The values a drop-down offers (the current value is always added when it is not in the list).
fn choices(name: &str) -> Vec<Option<RegVal>> {
    let d = |r: std::ops::RangeInclusive<u32>| r.map(|n| Some(RegVal::D(n))).collect::<Vec<_>>();
    let s = |l: &[&str]| l.iter().map(|x| Some(RegVal::S(x.to_string()))).collect::<Vec<_>>();
    match name {
        "NetworkThrottlingIndex" => std::iter::once(Some(RegVal::D(DISABLED))).chain([1, 5, 10, 20, 30, 40, 50, 60, 70].into_iter().map(|n| Some(RegVal::D(n)))).collect(),
        "SystemResponsiveness" => (0..=10).map(|n| Some(RegVal::D(n * 10))).collect(),
        "GPU Priority" => d(0..=31),
        "Priority" => d(1..=8),
        "Scheduling Category" => s(&["High", "Medium", "Low"]),
        "SFIO Priority" => s(&["High", "Normal", "Low", "Idle"]),
        "Background Only" => s(&["True", "False"]),
        "BackgroundPriority" => std::iter::once(None).chain(d(1..=8)).collect(),
        _ => vec![],
    }
}

fn path_of(task: &str) -> String {
    match task {
        "" => SYS.to_string(),
        PRIO => PRIO_KEY.to_string(),
        FG => FG_KEY.to_string(),
        t => format!(r"{SYS}\Tasks\{t}"),
    }
}

/// ForegroundLockTimeout is per user (HKCU); everything else is machine-wide.
fn hive_of(task: &str) -> RegKey {
    RegKey::predef(if task == FG { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE })
}

/// How a setting is named in lists ("Games › Priority", or just the value name).
fn place(task: &str, name: &str) -> String {
    if task.is_empty() || task.starts_with('#') { name.to_string() } else { format!("{task} › {name}") }
}

/// Win32PrioritySeparation parts: (interval 0 auto / 1 long / 2 short, length 0 auto / 1 variable / 2 fixed,
/// foreground boost 0 = 1:1, 1 = 2:1, 2 = 3:1).
fn prio_parts(v: u32) -> (u32, u32, u32) {
    let fix = |x: u32| if x == 3 { 0 } else { x };
    (fix((v >> 4) & 3), fix((v >> 2) & 3), ((v & 3).min(2)))
}

fn prio_value(interval: u32, length: u32, boost: u32) -> u32 {
    (interval << 4) | (length << 2) | boost
}

fn read_one(k: &RegKey, name: &str, ty: Ty) -> Option<RegVal> {
    match ty {
        Ty::Dword => k.get_value::<u32, _>(name).ok().map(RegVal::D),
        Ty::Text => k.get_value::<String, _>(name).ok().map(RegVal::S),
    }
}

/// Every setting as it is in the registry now, plus the task names found.
fn read_all() -> (BTreeMap<Key, Option<RegVal>>, Vec<String>) {
    let hk = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut m = BTreeMap::new();
    for (task, name) in [(PRIO, PRIO_NAME), (FG, FG_NAME)] {
        let v = hive_of(task).open_subkey(path_of(task)).ok().and_then(|k| read_one(&k, name, Ty::Dword));
        m.insert((task.to_string(), name), v);
    }
    if let Ok(k) = hk.open_subkey(SYS) {
        for (n, _, ty) in SYS_FIELDS {
            m.insert((String::new(), *n), read_one(&k, n, *ty));
        }
    }
    // the known tasks first, in order, then any others this PC has
    let mut tasks: Vec<String> = TASKS.iter().map(|s| s.to_string()).collect();
    if let Ok(t) = hk.open_subkey(format!(r"{SYS}\Tasks")) {
        for name in t.enum_keys().flatten() {
            if !tasks.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
                tasks.push(name);
            }
        }
    }
    tasks.retain(|t| hk.open_subkey(path_of(t)).is_ok());
    for t in &tasks {
        if let Ok(k) = hk.open_subkey(path_of(t)) {
            for (n, _, ty) in TASK_FIELDS {
                m.insert((t.clone(), *n), read_one(&k, n, *ty));
            }
        }
    }
    (m, tasks)
}

fn write_one(task: &str, name: &str, v: &Option<RegVal>) -> Result<(), String> {
    let k = hive_of(task).open_subkey_with_flags(path_of(task), KEY_READ | KEY_SET_VALUE).map_err(|e| e.to_string())?;
    match v {
        Some(RegVal::D(n)) => k.set_value(name, n).map_err(|e| e.to_string()),
        Some(RegVal::S(s)) => k.set_value(name, s).map_err(|e| e.to_string()),
        None => match k.delete_value(name) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Change {
    task: String,
    name: String,
    old: Option<RegVal>,
    new: Option<RegVal>,
}

impl Change {
    fn line(&self) -> String {
        format!("{}: {} → {}", place(&self.task, &self.name), show(&self.name, &self.old), show(&self.name, &self.new))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Save {
    time: String,
    changes: Vec<Change>,
    undone: bool,
}

/// Status next to a value: pending change, changed from the Windows default, or default.
fn status(name: &str, cur: &Option<RegVal>, v: &Option<RegVal>, def: &Option<RegVal>) -> (String, egui::Color32) {
    if v != cur {
        (format!("will change (now {})", show(name, cur)), theme::ACCENT)
    } else if def.is_some() && cur != def {
        (format!("changed - default {}", show(name, def)), theme::OK)
    } else if def.is_some() {
        ("default".to_string(), theme::TEXT_FAINT)
    } else {
        (String::new(), theme::TEXT_FAINT)
    }
}

/// Edits that differ from what is in the registry now.
fn diff(cur: &BTreeMap<Key, Option<RegVal>>, edit: &BTreeMap<Key, Option<RegVal>>) -> Vec<Change> {
    edit.iter()
        .filter(|(k, v)| cur.get(*k) != Some(*v))
        .map(|((t, n), v)| Change { task: t.clone(), name: n.to_string(), old: cur.get(&(t.clone(), *n)).cloned().flatten(), new: v.clone() })
        .collect()
}

/// Write a batch; returns the changes that worked and the error lines of those that did not.
fn write_all(changes: &[Change]) -> (Vec<Change>, Vec<String>) {
    let mut ok = vec![];
    let mut bad = vec![];
    for c in changes {
        match write_one(&c.task, &c.name, &c.new) {
            Ok(()) => ok.push(c.clone()),
            Err(e) => bad.push(format!("{} - {e}", c.line())),
        }
    }
    (ok, bad)
}

/// The two parts of the page, each edited and saved on its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    /// Win32PrioritySeparation + ForegroundLockTimeout
    Processor,
    /// The multimedia scheduler (MMCSS) profile and its tasks
    Multimedia,
}

fn section_of(task: &str) -> Section {
    if task == PRIO || task == FG { Section::Processor } else { Section::Multimedia }
}

pub struct SchedulerTab {
    cur: BTreeMap<Key, Option<RegVal>>,
    edit: BTreeMap<Key, Option<RegVal>>,
    tasks: Vec<String>,
    task: usize,
    history: Vec<Save>,
    /// The part of the page shown.
    section: Section,
    /// What the last action did (saved changes, undo, backup) and in which section, shown under the buttons.
    report: Vec<(String, egui::Color32)>,
    report_for: Section,
}

impl SchedulerTab {
    pub fn new() -> Self {
        let (cur, tasks) = read_all();
        let task = tasks.iter().position(|t| t == "Games").unwrap_or(0);
        SchedulerTab { edit: cur.clone(), cur, tasks, task, history: common::load_json(HISTORY), section: Section::Processor, report: vec![], report_for: Section::Processor }
    }

    /// Read the registry again. Unsaved edits in the other section are kept.
    fn reload(&mut self, section: Section) {
        let (cur, tasks) = read_all();
        let mut edit = cur.clone();
        for (k, v) in &self.edit {
            if section_of(&k.0) != section && edit.contains_key(k) {
                edit.insert(k.clone(), v.clone());
            }
        }
        self.cur = cur;
        self.edit = edit;
        self.tasks = tasks;
        self.task = self.task.min(self.tasks.len().saturating_sub(1));
    }

    /// Edits in one section that differ from the registry.
    fn pending(&self, section: Section) -> Vec<Change> {
        diff(&self.cur, &self.edit).into_iter().filter(|c| section_of(&c.task) == section).collect()
    }

    fn backup(&self, section: Section) -> Result<String, String> {
        let stamp = common::now().replace([':', ' '], "-");
        let (key, file) = match section {
            Section::Processor => (format!(r"HKLM\{PRIO_KEY}"), format!("priority-backup-{stamp}.reg")),
            Section::Multimedia => (SYS_REG.to_string(), format!("mmcss-backup-{stamp}.reg")),
        };
        let p = common::data_dir().join(file).to_string_lossy().to_string();
        match common::run("reg.exe", &["export", &key, &p, "/y"]) {
            Ok((0, _)) => Ok(p),
            Ok((code, out)) => Err(format!("reg export failed ({code}): {}", out.trim())),
            Err(e) => Err(e),
        }
    }

    fn save(&mut self, section: Section) {
        let changes = self.pending(section);
        if changes.is_empty() {
            return;
        }
        self.report.clear();
        self.report_for = section;
        // first save in this section: keep a backup of the original values
        if !self.history.iter().any(|s| s.changes.iter().any(|c| section_of(&c.task) == section)) {
            match self.backup(section) {
                Ok(p) => self.report.push((format!("Backup of the original values: {p}"), theme::TEXT_DIM)),
                Err(e) => self.report.push((format!("No backup made: {e}"), theme::GAMING)),
            }
        }
        let (ok, bad) = write_all(&changes);
        if !ok.is_empty() {
            self.report.insert(0, (format!("Applied {} change(s):", ok.len()), theme::OK));
            for c in &ok {
                self.report.push((format!("  {}", c.line()), theme::TEXT));
            }
            self.history.push(Save { time: common::now(), changes: ok, undone: false });
            common::save_json(HISTORY, &self.history);
        }
        for b in bad {
            self.report.push((format!("Not changed: {b}"), theme::LOCKED));
        }
        self.reload(section);
    }

    /// The newest save in a section that is not undone yet.
    fn last_save(&self, section: Section) -> Option<usize> {
        self.history.iter().rposition(|s| !s.undone && s.changes.iter().any(|c| section_of(&c.task) == section))
    }

    fn undo_last(&mut self, section: Section) {
        let Some(i) = self.last_save(section) else { return };
        self.report_for = section;
        let back: Vec<Change> = self.history[i].changes.iter().rev().map(|c| Change { task: c.task.clone(), name: c.name.clone(), old: c.new.clone(), new: c.old.clone() }).collect();
        let (ok, bad) = write_all(&back);
        self.report.clear();
        self.report.push((format!("Undid {} change(s) from {}:", ok.len(), self.history[i].time), theme::OK));
        for c in &ok {
            self.report.push((format!("  {}", c.line()), theme::TEXT));
        }
        for b in &bad {
            self.report.push((format!("Not changed: {b}"), theme::LOCKED));
        }
        if bad.is_empty() {
            self.history[i].undone = true;
        }
        common::save_json(HISTORY, &self.history);
        self.reload(section);
    }

    /// One setting row: label | control | status.
    fn row(&mut self, ui: &mut egui::Ui, task: &str, name: &'static str, label: &str, can: bool) {
        let key: Key = (task.to_string(), name);
        let cur = self.cur.get(&key).cloned().flatten();
        let mut v = self.edit.get(&key).cloned().flatten();
        let def = default_of(task, name);
        ui.label(label);
        ui.add_enabled_ui(can, |ui| match name {
            "Clock Rate" => {
                let mut n = match &v {
                    Some(RegVal::D(n)) => *n,
                    _ => 10000,
                };
                if ui.add(egui::DragValue::new(&mut n).range(0..=1_000_000).speed(100.0)).changed() {
                    v = Some(RegVal::D(n));
                }
            }
            "Affinity" => {
                let mask = match &v {
                    Some(RegVal::D(m)) => *m,
                    _ => 0,
                };
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 20.0], egui::Label::new(RichText::new(show(name, &v)).monospace()));
                    ui.menu_button("select ...", |ui| {
                        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).min(32);
                        let full = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
                        let mut all = mask == 0;
                        if ui.checkbox(&mut all, "All CPUs").changed() {
                            v = Some(RegVal::D(if all { 0 } else { 1 }));
                        }
                        ui.separator();
                        egui::Grid::new(("aff", task)).num_columns(4).show(ui, |ui| {
                            for c in 0..n {
                                let mut on = mask == 0 || mask & (1 << c) != 0;
                                if ui.checkbox(&mut on, format!("CPU {c}")).changed() {
                                    let base = if mask == 0 { full } else { mask };
                                    let m = if on { base | (1 << c) } else { base & !(1 << c) };
                                    // all CPUs = 0; never an empty mask
                                    if m != 0 {
                                        v = Some(RegVal::D(if m & full == full { 0 } else { m }));
                                    }
                                }
                                if c % 4 == 3 {
                                    ui.end_row();
                                }
                            }
                        });
                    });
                });
            }
            _ => {
                let mut opts = choices(name);
                if !opts.contains(&v) {
                    opts.push(v.clone());
                }
                egui::ComboBox::from_id_salt(("mmcss", task, name)).width(130.0).selected_text(show(name, &v)).show_ui(ui, |ui| {
                    for o in opts {
                        let text = show(name, &o);
                        ui.selectable_value(&mut v, o, text);
                    }
                });
            }
        });
        let (text, c) = status(name, &cur, &v, &def);
        ui.label(RichText::new(text).size(12.0).color(c));
        ui.end_row();
        self.edit.insert(key, v);
    }

    /// Processor scheduling (Win32PrioritySeparation) and Foreground Lock Timeout.
    fn prio_card(&mut self, ui: &mut egui::Ui, can: bool) {
        let pk: Key = (PRIO.to_string(), PRIO_NAME);
        let fk: Key = (FG.to_string(), FG_NAME);
        let cur_p = self.cur.get(&pk).cloned().flatten();
        let mut p = match self.edit.get(&pk).cloned().flatten() {
            Some(RegVal::D(v)) => v,
            _ => 2,
        };
        let before = prio_parts(p);
        let (mut interval, mut length, mut boost) = before;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(can, |ui| {
                theme::section(ui, "Interval length");
                ui.radio_value(&mut interval, 0, "Let Windows choose (short on this PC)");
                ui.radio_value(&mut interval, 2, "Short");
                ui.radio_value(&mut interval, 1, "Long");
                ui.add_space(6.0);
                theme::section(ui, "Fixed / variable intervals");
                ui.radio_value(&mut length, 0, "Let Windows choose (variable on this PC)");
                ui.radio_value(&mut length, 2, "Fixed length");
                ui.radio_value(&mut length, 1, "Variable length");
                ui.indent("boost", |ui| {
                    ui.add_enabled_ui(length != 2, |ui| {
                        ui.radio_value(&mut boost, 0, "No foreground boost (1:1)");
                        ui.radio_value(&mut boost, 1, "Foreground double length intervals (2:1)");
                        ui.radio_value(&mut boost, 2, "Foreground triple length intervals (3:1)");
                    });
                });
                ui.add_space(6.0);
                theme::section(ui, "Foreground lock timeout");
                let cur_f = self.cur.get(&fk).cloned().flatten();
                let mut f = self.edit.get(&fk).cloned().flatten();
                ui.horizontal(|ui| {
                    let mut n = match &f {
                        Some(RegVal::D(n)) => *n,
                        _ => 200000,
                    };
                    if ui.add(egui::DragValue::new(&mut n).range(0..=2_000_000).speed(1000.0).suffix(" ms")).changed() {
                        f = Some(RegVal::D(n));
                    }
                    let (text, c) = status(FG_NAME, &cur_f, &f, &default_of(FG, FG_NAME));
                    ui.label(RichText::new(text).size(12.0).color(c));
                });
                self.edit.insert(fk.clone(), f);
            });
            // only a changed choice rewrites the value (an odd stored value such as 0x3F is kept as is)
            if (interval, length, boost) != before {
                p = prio_value(interval, length, boost);
            }
            let v = Some(RegVal::D(p));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Bitmask (hex):");
                ui.label(RichText::new(format!("{p:X}")).monospace().strong());
                let (text, c) = status(PRIO_NAME, &cur_p, &v, &default_of(PRIO, PRIO_NAME));
                ui.label(RichText::new(text).size(12.0).color(c));
                if ui.add_enabled(can, egui::Button::new("Restore defaults")).on_hover_text("Bitmask 2 and 200000 ms. Click Apply to apply.").clicked() {
                    p = 2;
                    self.edit.insert(fk.clone(), default_of(FG, FG_NAME));
                }
            });
        });
        self.edit.insert(pk, Some(RegVal::D(p)));
    }

    /// Settings in a section that differ from the Windows default (what is applied now).
    fn applied(&self, section: Section) -> Vec<String> {
        self.cur
            .iter()
            .filter(|((t, _), _)| section_of(t) == section)
            .filter_map(|((t, n), v)| {
                let def = default_of(t, n);
                (def.is_some() && *v != def).then(|| format!("{}: {} (default {})", place(t, n), show(n, v), show(n, &def)))
            })
            .collect()
    }
}

impl Tab for SchedulerTab {
    fn title(&self) -> &'static str {
        "CPU Scheduler"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        theme::page_header(ui, "CPU Scheduler", "");
        common::admin_banner(ui);
        let can = common::is_admin();

        // ---- section tabs (a dot marks a section with unsaved changes)
        ui.horizontal(|ui| {
            for (s, name) in [(Section::Processor, "Processor scheduling"), (Section::Multimedia, "Multimedia scheduler")] {
                let on = self.section == s;
                let dirty = !self.pending(s).is_empty();
                let text = if dirty { format!("  {name}  •  ") } else { format!("  {name}  ") };
                let rt = RichText::new(text).size(15.0).strong().color(if on { theme::ACCENT } else { theme::TEXT_DIM });
                if ui.selectable_label(on, rt).clicked() {
                    self.section = s;
                }
            }
        });
        ui.add_space(6.0);

        let section = self.section;
        egui::ScrollArea::vertical().id_salt(("sched-page", section == Section::Processor)).auto_shrink([false, false]).show(ui, |ui| {
            // ---- what is applied now in this section
            let applied = self.applied(section);
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                if applied.is_empty() {
                    ui.label(RichText::new("Everything is at the Windows default.").color(theme::TEXT_DIM));
                } else {
                    theme::section(ui, &format!("Applied now - {} changed from default", applied.len()));
                    for a in &applied {
                        ui.label(RichText::new(a).color(theme::OK));
                    }
                }
            });
            ui.add_space(6.0);

            match section {
                Section::Processor => self.prio_card(ui, can),
                Section::Multimedia => {
                    theme::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        theme::section(ui, "Multimedia scheduler");
                        egui::Grid::new("mmcss-sys").num_columns(3).spacing([14.0, 6.0]).show(ui, |ui| {
                            for (n, label, _) in SYS_FIELDS {
                                self.row(ui, "", n, label, can);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    theme::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            theme::section(ui, "Task");
                            let name = self.tasks.get(self.task).cloned().unwrap_or_default();
                            let changed: Vec<String> = self.pending(Section::Multimedia).into_iter().map(|c| c.task).collect();
                            egui::ComboBox::from_id_salt("mmcss-task").width(200.0).selected_text(&name).show_ui(ui, |ui| {
                                for (i, t) in self.tasks.iter().enumerate() {
                                    let label = if changed.contains(t) { format!("{t}  •") } else { t.clone() };
                                    ui.selectable_value(&mut self.task, i, label);
                                }
                            });
                        });
                        ui.add_space(4.0);
                        if let Some(task) = self.tasks.get(self.task).cloned() {
                            egui::Grid::new(("mmcss-task-grid", &task)).num_columns(3).spacing([14.0, 6.0]).show(ui, |ui| {
                                for (n, label, _) in TASK_FIELDS {
                                    self.row(ui, &task, n, label, can);
                                }
                            });
                        }
                    });
                }
            }
            ui.add_space(6.0);

            // ---- actions for this section only
            let pending = self.pending(section);
            ui.horizontal(|ui| {
                if ui.add_enabled(can && !pending.is_empty(), theme::primary_button(&format!("   Apply ({})   ", pending.len()))).clicked() {
                    self.save(section);
                }
                if ui.add_enabled(!pending.is_empty(), egui::Button::new("Cancel")).on_hover_text("Discard the changes on this tab that are not applied yet.").clicked() {
                    for (k, v) in self.cur.iter().filter(|(k, _)| section_of(&k.0) == section) {
                        self.edit.insert(k.clone(), v.clone());
                    }
                }
                if ui.add_enabled(can && self.last_save(section).is_some(), egui::Button::new("Undo last apply")).clicked() {
                    self.undo_last(section);
                }
                if ui.button("Backup registry entries").clicked() {
                    self.report_for = section;
                    self.report = vec![match self.backup(section) {
                        Ok(p) => (format!("Backup saved: {p}"), theme::OK),
                        Err(e) => (format!("Backup failed: {e}"), theme::LOCKED),
                    }];
                }
                if ui.button("Reload").on_hover_text("Read the registry again.").clicked() {
                    self.reload(section);
                }
            });
            if !pending.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Not applied yet:").size(12.5).color(theme::ACCENT));
                for c in &pending {
                    ui.label(RichText::new(format!("  {}", c.line())).size(12.5).color(theme::TEXT_DIM));
                }
            }
            if !self.report.is_empty() && self.report_for == section {
                ui.add_space(4.0);
                for (l, c) in &self.report {
                    ui.label(RichText::new(l).size(12.5).color(*c));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmcss_defaults_and_display() {
        assert_eq!(default_of("", "SystemResponsiveness"), Some(RegVal::D(20)));
        assert_eq!(default_of("Games", "Priority"), Some(RegVal::D(2)));
        assert_eq!(default_of("Games", "Background Only"), Some(RegVal::S("False".into())));
        assert_eq!(default_of("Audio", "BackgroundPriority"), None);
        assert_eq!(default_of("Playback", "BackgroundPriority"), Some(RegVal::D(4)));
        assert_eq!(show("NetworkThrottlingIndex", &Some(RegVal::D(DISABLED))), "Disabled");
        assert_eq!(show("SystemResponsiveness", &Some(RegVal::D(10))), "10%");
        assert_eq!(show("Affinity", &Some(RegVal::D(0))), "all");
        assert_eq!(show("BackgroundPriority", &None), "(not set)");
        assert!(choices("Priority").contains(&Some(RegVal::D(8))));
        assert_eq!(choices("BackgroundPriority")[0], None);
    }

    /// The two tabs are saved apart: each one's pending list holds only its own settings.
    #[test]
    fn sched_sections_apply_separately() {
        assert_eq!(section_of(PRIO), Section::Processor);
        assert_eq!(section_of(FG), Section::Processor);
        assert_eq!(section_of(""), Section::Multimedia);
        assert_eq!(section_of("Games"), Section::Multimedia);
        let mut cur = BTreeMap::new();
        cur.insert((PRIO.to_string(), PRIO_NAME), Some(RegVal::D(2)));
        cur.insert(("Games".to_string(), "Priority"), Some(RegVal::D(2)));
        let mut edit = cur.clone();
        edit.insert((PRIO.to_string(), PRIO_NAME), Some(RegVal::D(0x26)));
        edit.insert(("Games".to_string(), "Priority"), Some(RegVal::D(6)));
        let tab = SchedulerTab { cur, edit, tasks: vec!["Games".into()], task: 0, history: vec![], section: Section::Processor, report: vec![], report_for: Section::Processor };
        let p: Vec<String> = tab.pending(Section::Processor).iter().map(|c| c.line()).collect();
        let m: Vec<String> = tab.pending(Section::Multimedia).iter().map(|c| c.line()).collect();
        assert_eq!(p, vec!["Win32PrioritySeparation: 0x2 → 0x26".to_string()]);
        assert_eq!(m, vec!["Games › Priority: 2 → 6".to_string()]);
    }

    #[test]
    fn mmcss_priority_separation() {
        // 0x2 = Windows default: OS chooses interval and length, 3:1 boost
        assert_eq!(prio_parts(0x2), (0, 0, 2));
        assert_eq!(prio_parts(0x26), (2, 1, 2), "short, variable, 3:1");
        assert_eq!(prio_parts(0x18), (1, 2, 0), "long, fixed, 1:1");
        assert_eq!(prio_parts(0x3F), (0, 0, 2), "3 means default in every field");
        for v in [0x2, 0x26, 0x18, 0x14, 0x29] {
            let (i, l, b) = prio_parts(v);
            assert_eq!(prio_value(i, l, b), v);
        }
        assert_eq!(default_of(PRIO, PRIO_NAME), Some(RegVal::D(2)));
        assert_eq!(default_of(FG, FG_NAME), Some(RegVal::D(200000)));
        assert_eq!(show(PRIO_NAME, &Some(RegVal::D(0x26))), "0x26");
        assert_eq!(show(FG_NAME, &Some(RegVal::D(0))), "0 ms");
        assert_eq!(place(PRIO, PRIO_NAME), PRIO_NAME);
        assert_eq!(place("Games", "Priority"), "Games › Priority");
    }

    #[test]
    fn mmcss_diff_lists_only_edits() {
        let mut cur = BTreeMap::new();
        cur.insert(("Games".to_string(), "Priority"), Some(RegVal::D(2)));
        cur.insert(("Games".to_string(), "Scheduling Category"), Some(RegVal::S("Medium".into())));
        cur.insert((String::new(), "SystemResponsiveness"), Some(RegVal::D(20)));
        let mut edit = cur.clone();
        assert!(diff(&cur, &edit).is_empty());
        edit.insert(("Games".to_string(), "Priority"), Some(RegVal::D(6)));
        edit.insert((String::new(), "SystemResponsiveness"), Some(RegVal::D(10)));
        let d = diff(&cur, &edit);
        assert_eq!(d.len(), 2);
        let lines: Vec<String> = d.iter().map(|c| c.line()).collect();
        assert!(lines.contains(&"Games › Priority: 2 → 6".to_string()));
        assert!(lines.contains(&"SystemResponsiveness: 20% → 10%".to_string()));
    }

    /// Read-only: the registry on this PC can be read and every known task is found.
    #[test]
    #[ignore]
    fn mmcss_live_read() {
        let (m, tasks) = read_all();
        println!("{tasks:?}");
        assert!(tasks.iter().any(|t| t == "Games"));
        assert!(m.contains_key(&("Games".to_string(), "Priority")));
        println!("Win32PrioritySeparation {:?}, ForegroundLockTimeout {:?}", m[&(PRIO.to_string(), PRIO_NAME)], m[&(FG.to_string(), FG_NAME)]);
    }
}
