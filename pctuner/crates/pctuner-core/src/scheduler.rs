//! CPU Scheduler tab - the Multimedia Class Scheduler (MMCSS) settings, edited directly:
//! HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile and its Tasks.
//! Shows what is set now, what differs from the Windows default, and what each save changed.

use crate::common::{self, Tone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use winreg::enums::*;
use winreg::RegKey;

pub const SYS: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
pub const SYS_REG: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
pub const HISTORY: &str = "mmcss-history.json";
pub const DISABLED: u32 = 0xFFFF_FFFF;

/// Pseudo task names for the two settings outside the multimedia profile.
pub const PRIO: &str = "#prio"; // HKLM ...\PriorityControl  Win32PrioritySeparation
pub const FG: &str = "#fg"; // HKCU Control Panel\Desktop  ForegroundLockTimeout
pub const PRIO_KEY: &str = r"SYSTEM\CurrentControlSet\Control\PriorityControl";
pub const FG_KEY: &str = r"Control Panel\Desktop";
pub const PRIO_NAME: &str = "Win32PrioritySeparation";
pub const FG_NAME: &str = "ForegroundLockTimeout";

/// The Windows tasks, in the order Windows lists them.
pub const TASKS: &[&str] = &["Audio", "Capture", "DisplayPostProcessing", "Distribution", "Games", "Playback", "Pro Audio", "Window Manager"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegVal {
    D(u32),
    S(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Ty {
    Dword,
    Text,
}

/// (registry value name, label, type)
pub const SYS_FIELDS: &[(&str, &str, Ty)] = &[("NetworkThrottlingIndex", "Network throttling index", Ty::Dword), ("SystemResponsiveness", "System responsiveness reserved CPU", Ty::Dword)];
pub const TASK_FIELDS: &[(&str, &str, Ty)] = &[
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
pub type Key = (String, &'static str);

/// Windows defaults (Windows 10 / 11).
pub fn default_of(task: &str, name: &str) -> Option<RegVal> {
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
pub fn show(name: &str, v: &Option<RegVal>) -> String {
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
pub fn choices(name: &str) -> Vec<Option<RegVal>> {
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

pub fn path_of(task: &str) -> String {
    match task {
        "" => SYS.to_string(),
        PRIO => PRIO_KEY.to_string(),
        FG => FG_KEY.to_string(),
        t => format!(r"{SYS}\Tasks\{t}"),
    }
}

/// ForegroundLockTimeout is per user (HKCU); everything else is machine-wide.
pub fn hive_of(task: &str) -> RegKey {
    RegKey::predef(if task == FG { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE })
}

/// How a setting is named in lists ("Games › Priority", or just the value name).
pub fn place(task: &str, name: &str) -> String {
    if task.is_empty() || task.starts_with('#') { name.to_string() } else { format!("{task} › {name}") }
}

/// Win32PrioritySeparation parts: (interval 0 auto / 1 long / 2 short, length 0 auto / 1 variable / 2 fixed,
/// foreground boost 0 = 1:1, 1 = 2:1, 2 = 3:1).
pub fn prio_parts(v: u32) -> (u32, u32, u32) {
    let fix = |x: u32| if x == 3 { 0 } else { x };
    (fix((v >> 4) & 3), fix((v >> 2) & 3), ((v & 3).min(2)))
}

pub fn prio_value(interval: u32, length: u32, boost: u32) -> u32 {
    (interval << 4) | (length << 2) | boost
}

pub fn read_one(k: &RegKey, name: &str, ty: Ty) -> Option<RegVal> {
    match ty {
        Ty::Dword => k.get_value::<u32, _>(name).ok().map(RegVal::D),
        Ty::Text => k.get_value::<String, _>(name).ok().map(RegVal::S),
    }
}

/// Every setting as it is in the registry now, plus the task names found.
pub fn read_all() -> (BTreeMap<Key, Option<RegVal>>, Vec<String>) {
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

pub fn write_one(task: &str, name: &str, v: &Option<RegVal>) -> Result<(), String> {
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
pub struct Change {
    pub task: String,
    pub name: String,
    pub old: Option<RegVal>,
    pub new: Option<RegVal>,
}

impl Change {
    pub fn line(&self) -> String {
        format!("{}: {} → {}", place(&self.task, &self.name), show(&self.name, &self.old), show(&self.name, &self.new))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Save {
    pub time: String,
    pub changes: Vec<Change>,
    pub undone: bool,
}

/// Status next to a value: pending change, changed from the Windows default, or default.
pub fn status(name: &str, cur: &Option<RegVal>, v: &Option<RegVal>, def: &Option<RegVal>) -> (String, Tone) {
    if v != cur {
        (format!("will change (now {})", show(name, cur)), Tone::Accent)
    } else if def.is_some() && cur != def {
        (format!("changed - default {}", show(name, def)), Tone::Ok)
    } else if def.is_some() {
        ("default".to_string(), Tone::Faint)
    } else {
        (String::new(), Tone::Faint)
    }
}

/// Edits that differ from what is in the registry now.
pub fn diff(cur: &BTreeMap<Key, Option<RegVal>>, edit: &BTreeMap<Key, Option<RegVal>>) -> Vec<Change> {
    edit.iter()
        .filter(|(k, v)| cur.get(*k) != Some(*v))
        .map(|((t, n), v)| Change { task: t.clone(), name: n.to_string(), old: cur.get(&(t.clone(), *n)).cloned().flatten(), new: v.clone() })
        .collect()
}

/// Write a batch; returns the changes that worked and the error lines of those that did not.
pub fn write_all(changes: &[Change]) -> (Vec<Change>, Vec<String>) {
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Section {
    /// Win32PrioritySeparation + ForegroundLockTimeout
    Processor,
    /// The multimedia scheduler (MMCSS) profile and its tasks
    Multimedia,
}

pub fn section_of(task: &str) -> Section {
    if task == PRIO || task == FG { Section::Processor } else { Section::Multimedia }
}

/// The scheduler settings: what the registry holds, the user's unsaved edits, and the save history.
pub struct Scheduler {
    pub cur: BTreeMap<Key, Option<RegVal>>,
    pub edit: BTreeMap<Key, Option<RegVal>>,
    pub tasks: Vec<String>,
    pub history: Vec<Save>,
    /// What the last action did (saved changes, undo, backup) and in which section.
    pub report: Vec<(String, Tone)>,
    pub report_for: Section,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        let (cur, tasks) = read_all();
        Scheduler { edit: cur.clone(), cur, tasks, history: common::load_json(HISTORY), report: vec![], report_for: Section::Processor }
    }

    /// Read the registry again. Unsaved edits in the other section are kept.
    pub fn reload(&mut self, section: Section) {
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
    }

    /// Edits in one section that differ from the registry.
    pub fn pending(&self, section: Section) -> Vec<Change> {
        diff(&self.cur, &self.edit).into_iter().filter(|c| section_of(&c.task) == section).collect()
    }

    pub fn backup(&self, section: Section) -> Result<String, String> {
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

    pub fn save(&mut self, section: Section) {
        let changes = self.pending(section);
        if changes.is_empty() {
            return;
        }
        self.report.clear();
        self.report_for = section;
        // first save in this section: keep a backup of the original values
        if !self.history.iter().any(|s| s.changes.iter().any(|c| section_of(&c.task) == section)) {
            match self.backup(section) {
                Ok(p) => self.report.push((format!("Backup of the original values: {p}"), Tone::Dim)),
                Err(e) => self.report.push((format!("No backup made: {e}"), Tone::Warn)),
            }
        }
        let (ok, bad) = write_all(&changes);
        if !ok.is_empty() {
            self.report.insert(0, (format!("Applied {} change(s):", ok.len()), Tone::Ok));
            for c in &ok {
                self.report.push((format!("  {}", c.line()), Tone::Text));
            }
            self.history.push(Save { time: common::now(), changes: ok, undone: false });
            common::save_json(HISTORY, &self.history);
        }
        for b in bad {
            self.report.push((format!("Not changed: {b}"), Tone::Error));
        }
        self.reload(section);
    }

    /// The newest save in a section that is not undone yet.
    pub fn last_save(&self, section: Section) -> Option<usize> {
        self.history.iter().rposition(|s| !s.undone && s.changes.iter().any(|c| section_of(&c.task) == section))
    }

    pub fn undo_last(&mut self, section: Section) {
        let Some(i) = self.last_save(section) else { return };
        self.report_for = section;
        let back: Vec<Change> = self.history[i].changes.iter().rev().map(|c| Change { task: c.task.clone(), name: c.name.clone(), old: c.new.clone(), new: c.old.clone() }).collect();
        let (ok, bad) = write_all(&back);
        self.report.clear();
        self.report.push((format!("Undid {} change(s) from {}:", ok.len(), self.history[i].time), Tone::Ok));
        for c in &ok {
            self.report.push((format!("  {}", c.line()), Tone::Text));
        }
        for b in &bad {
            self.report.push((format!("Not changed: {b}"), Tone::Error));
        }
        if bad.is_empty() {
            self.history[i].undone = true;
        }
        common::save_json(HISTORY, &self.history);
        self.reload(section);
    }

    /// Settings in a section that differ from the Windows default (what is applied now).
    pub fn applied(&self, section: Section) -> Vec<String> {
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
        let tab = Scheduler { cur, edit, tasks: vec!["Games".into()], history: vec![], report: vec![], report_for: Section::Processor };
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
