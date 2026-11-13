//! Services backups: a full snapshot of every service's startup type, with three independent ways back.
//!   1. Restore in PC Tuner - only the services that differ; each one through the Service Control Manager,
//!      then a direct registry write, then (for services Windows protects even from admins) the same write as
//!      SYSTEM. All in Rust: no sc.exe, no PowerShell, no scheduled task.
//!   2. RESTORE-services-<time>.cmd next to the backup - double-click as administrator; works in Safe Mode
//!      (a file for the user to run by hand when PC Tuner cannot; PC Tuner itself never runs it).
//!   3. services-<time>.reg - a full export of HKLM\SYSTEM\CurrentControlSet\Services (last resort), written
//!      by PC Tuner itself in regedit's format.
//! Restore only changes Start / DelayedAutostart of services that exist, never paths or accounts.

use crate::common::{self, Job};
use crate::opwin::{OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc;
use winreg::enums::*;
use winreg::RegKey;

const SERVICES: &str = r"SYSTEM\CurrentControlSet\Services";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Svc {
    pub name: String,
    #[serde(default)]
    pub display: String,
    /// 2 automatic, 3 manual, 4 disabled
    pub start: u32,
    /// DelayedAutostart (1 = Automatic (Delayed)); None = value absent
    #[serde(default)]
    pub delayed: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Backup {
    pub time: String,
    #[serde(default)]
    pub note: String,
    pub services: Vec<Svc>,
}

/// Documents\PCTuner-Backups\Services (created on demand).
pub fn backup_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    home.join("Documents").join("PCTuner-Backups").join("Services")
}

/// Every Win32 service (own / shared process, per-user templates) with its startup type.
/// Kernel drivers and per-user instances (name_xxxx, recreated at sign-in) are left out.
pub fn read_current() -> Result<Vec<Svc>, String> {
    let root = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(SERVICES).map_err(|e| format!("cannot read the service list: {e}"))?;
    let mut out = vec![];
    for name in root.enum_keys().filter_map(Result::ok) {
        let Ok(k) = root.open_subkey(&name) else { continue };
        let ty: u32 = k.get_value("Type").unwrap_or(0);
        let start: Result<u32, _> = k.get_value("Start");
        // 0x10 own process, 0x20 shared, 0x40 per-user template; 0x80 = per-user instance
        if ty & 0x30 == 0 || ty & 0x80 != 0 {
            continue;
        }
        let Ok(start) = start else { continue };
        if !(2..=4).contains(&start) {
            continue;
        }
        let display: String = k.get_value("DisplayName").unwrap_or_default();
        let delayed: Option<u32> = k.get_value("DelayedAutostart").ok();
        out.push(Svc { name, display, start, delayed });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn type_name(start: u32, delayed: Option<u32>) -> &'static str {
    match (start, delayed) {
        (2, Some(1)) => "Automatic (Delayed)",
        (2, _) => "Automatic",
        (3, _) => "Manual",
        (4, _) => "Disabled",
        _ => "Other",
    }
}

/// What the Service Control Manager is asked to set for a saved startup type: (Start, Delayed start).
/// Like `sc config start= auto / delayed-auto`, only Automatic touches Delayed start.
fn scm_type(start: u32, delayed: Option<u32>) -> (u32, Option<bool>) {
    match (start, delayed) {
        (2, Some(1)) => (2, Some(true)),
        (2, _) => (2, Some(false)),
        (3, _) => (3, None),
        _ => (4, None),
    }
}

/// Services whose startup type differs from the backup (and that still exist): (backup, current).
pub fn differences(backup: &[Svc], current: &[Svc]) -> Vec<(Svc, Svc)> {
    let mut d = vec![];
    for b in backup {
        if let Some(c) = current.iter().find(|c| c.name.eq_ignore_ascii_case(&b.name)) {
            let delayed_differs = b.start == 2 && b.delayed.unwrap_or(0) != c.delayed.unwrap_or(0);
            if b.start != c.start || delayed_differs {
                d.push((b.clone(), c.clone()));
            }
        }
    }
    d
}

/// Double-click restore script (cmd, registry writes only, so it also works in Safe Mode).
pub fn restore_cmd(b: &Backup) -> String {
    let mut s = String::from("@echo off\r\nrem PC Tuner services backup - puts every service's startup type back.\r\n");
    s.push_str(&format!("rem Backup from {}. Right-click this file > Run as administrator, then restart.\r\n", b.time));
    s.push_str("net session >nul 2>&1 || (echo Please right-click this file and choose \"Run as administrator\". & pause & exit /b 1)\r\nset failed=0\r\nset changed=0\r\n");
    // only services that differ are written: Windows protects some keys (Defender, RPC...) even from
    // administrators, and writing an unchanged value there would only produce false "could not" lines
    for v in &b.services {
        let key = format!(r"HKLM\{SERVICES}\{}", v.name);
        s.push_str(&format!(
            "reg query \"{key}\" /v Start 2>nul | find \"0x{st}\" >nul || (reg add \"{key}\" /v Start /t REG_DWORD /d {st} /f >nul 2>&1 && (echo restored {n} & set changed=1) || (echo could not restore {n} - Windows protects it & set failed=1))\r\n",
            st = v.start,
            n = v.name
        ));
        match v.delayed {
            Some(d) => s.push_str(&format!("reg query \"{key}\" /v DelayedAutostart 2>nul | find \"0x{d}\" >nul || reg add \"{key}\" /v DelayedAutostart /t REG_DWORD /d {d} /f >nul 2>&1\r\n")),
            None => s.push_str(&format!("reg query \"{key}\" /v DelayedAutostart >nul 2>&1 && reg delete \"{key}\" /v DelayedAutostart /f >nul 2>&1\r\n")),
        }
    }
    s.push_str("echo.\r\nif %failed%==1 (echo Some changed services are protected by Windows - use Restore in PC Tuner, or import the .reg file next to this one.) else if %changed%==1 (echo The services above were put back.) else (echo Nothing to restore - every service already matches this backup.)\r\necho Restart the PC to finish.\r\npause\r\n");
    s
}

/// Save a backup: JSON + restore .cmd + full .reg export. Returns the JSON path.
pub fn create(note: &str) -> Result<PathBuf, String> {
    let services = read_current()?;
    if services.is_empty() {
        return Err("no services were read - the backup was not saved".into());
    }
    let dir = backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let b = Backup { time: common::now(), note: note.to_string(), services };
    let json = dir.join(format!("services-{stamp}.json"));
    let text = serde_json::to_string_pretty(&b).map_err(|e| e.to_string())?;
    std::fs::write(&json, text).map_err(|e| format!("cannot write {}: {e}", json.display()))?;
    let cmd = dir.join(format!("RESTORE-services-{stamp}.cmd"));
    let _ = std::fs::write(&cmd, restore_cmd(&b));
    let reg = dir.join(format!("services-{stamp}.reg"));
    let _ = export_reg(&reg);
    Ok(json)
}

// ------------------------------------------------------------------ .reg export (regedit's format)

/// `"name"` for a value name, `@` for the default value; \ and " escaped as regedit does.
fn reg_name(name: &str) -> String {
    if name.is_empty() {
        "@".into()
    } else {
        format!("\"{}\"", name.replace('\\', r"\\").replace('"', "\\\""))
    }
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(",")
}

/// One value line as regedit writes it: strings quoted, DWORDs as dword:, everything else as hex(type):.
fn reg_value_line(name: &str, vtype: u32, bytes: &[u8]) -> String {
    let data = match vtype {
        // REG_SZ: UTF-16 text without its terminating null
        1 => {
            let u: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let end = u.iter().position(|c| *c == 0).unwrap_or(u.len());
            format!("\"{}\"", String::from_utf16_lossy(&u[..end]).replace('\\', r"\\").replace('"', "\\\""))
        }
        4 if bytes.len() == 4 => format!("dword:{:08x}", u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        3 => format!("hex:{}", hex_bytes(bytes)),
        t => format!("hex({t:x}):{}", hex_bytes(bytes)),
    };
    format!("{}={data}", reg_name(name))
}

/// Append `[path]` and its values, then its subkeys (keys that cannot be read are left out, like regedit).
fn export_key(out: &mut String, key: &RegKey, path: &str) {
    out.push_str(&format!("\r\n[{path}]\r\n"));
    for (name, v) in key.enum_values().filter_map(Result::ok) {
        out.push_str(&reg_value_line(&name, v.vtype as u32, &v.bytes));
        out.push_str("\r\n");
    }
    for sub in key.enum_keys().filter_map(Result::ok) {
        if let Ok(k) = key.open_subkey(&sub) {
            export_key(out, &k, &format!(r"{path}\{sub}"));
        }
    }
}

/// HKLM\SYSTEM\CurrentControlSet\Services as a .reg file (UTF-16 with BOM, like `reg export`).
pub fn export_reg(path: &std::path::Path) -> Result<(), String> {
    let root = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(SERVICES).map_err(|e| format!("cannot read the services key: {e}"))?;
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
    export_key(&mut text, &root, &format!(r"HKEY_LOCAL_MACHINE\{SERVICES}"));
    text.push_str("\r\n");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Saved backups, newest first: (path, backup).
pub fn list() -> Vec<(PathBuf, Backup)> {
    let mut v: Vec<(PathBuf, Backup)> = std::fs::read_dir(backup_dir())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|p| std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Backup>(t.trim_start_matches('\u{feff}')).ok()).map(|b| (p, b)))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// Make a backup when there is none yet (called before the first service change).
pub fn ensure_backup() -> Option<String> {
    if list().is_empty() {
        return create("automatic, before the first change in PC Tuner").err();
    }
    None
}

// ------------------------------------------------------------------ restore (all in Rust)

/// Write a saved startup type straight into the service's registry key (Start, and DelayedAutostart set or
/// removed). Used when the Service Control Manager refuses, and - as SYSTEM - for protected services.
fn write_registry(name: &str, start: u32, delayed: Option<u32>) -> Result<(), String> {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(format!(r"{SERVICES}\{name}"), KEY_SET_VALUE)
        .map_err(|e| format!("cannot open the registry key: {e}"))?;
    k.set_value("Start", &start).map_err(|e| e.to_string())?;
    match delayed {
        Some(d) => k.set_value("DelayedAutostart", &d).map_err(|e| e.to_string())?,
        None => {
            let _ = k.delete_value("DelayedAutostart");
        }
    }
    Ok(())
}

/// The DelayedAutostart value after a successful change, as the old script did (errors ignored).
fn write_delayed(name: &str, delayed: Option<u32>) {
    if let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(format!(r"{SERVICES}\{name}"), KEY_SET_VALUE) {
        let _ = match delayed {
            Some(d) => k.set_value("DelayedAutostart", &d),
            None => k.delete_value("DelayedAutostart"),
        };
    }
}

fn read_start(name: &str) -> Option<u32> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!(r"{SERVICES}\{name}")).ok()?.get_value("Start").ok()
}

/// Put each service back (runs in a Job), reporting (index, result) as it goes: the Service Control Manager
/// first, then a registry write, then the services that refused both are written as SYSTEM and checked.
fn restore(diffs: Vec<(Svc, Svc)>, tx: mpsc::Sender<(usize, Result<String, String>)>) {
    let mut failed: Vec<(usize, String, u32, Option<u32>)> = vec![];
    for (i, (b, _)) in diffs.iter().enumerate() {
        let (start, delayed) = scm_type(b.start, b.delayed);
        let done = crate::winsvc::set_start(&b.name, start, delayed).or_else(|_| write_registry(&b.name, b.start, b.delayed));
        match done {
            Ok(()) => {
                write_delayed(&b.name, b.delayed);
                let _ = tx.send((i, Ok(String::new())));
            }
            Err(_) => failed.push((i, b.name.clone(), b.start, b.delayed)),
        }
    }
    if failed.is_empty() {
        return;
    }
    // protected services: the same registry write as SYSTEM, in-process (no task, no script)
    let work = failed.clone();
    let as_system = crate::winsvc::as_system(move || {
        for (_, name, start, delayed) in &work {
            let _ = write_registry(name, *start, *delayed);
        }
    });
    for (i, name, start, _) in failed {
        let r = if read_start(&name) == Some(start) {
            Ok("restored as SYSTEM (protected service)".to_string())
        } else {
            let why = as_system.as_ref().err().map(|e| format!(" ({e})")).unwrap_or_default();
            Err(format!("Windows protects this service - restore it with the RESTORE .cmd in Safe Mode{why}"))
        };
        let _ = tx.send((i, r));
    }
}

/// A restore in progress: results arrive on `rx`; the channel closes when the Job is done.
struct Running {
    rx: mpsc::Receiver<(usize, Result<String, String>)>,
    _job: Job<()>,
}

/// The Backups window (opened from the Services tab).
pub struct BackupsWindow {
    backups: Vec<(PathBuf, Backup)>,
    job: Option<Job<Result<PathBuf, String>>>,
    op: Option<OpWindow>,
    pending: Option<Vec<(Svc, Svc)>>,
    run: Option<Running>,
    msg: Option<(String, bool)>,
    /// the service list now (read when the window opens and after each backup / restore)
    current: Vec<Svc>,
}

impl BackupsWindow {
    pub fn new() -> Self {
        // rewrite the RESTORE .cmd of every saved backup, so older ones get the current script
        for (json, b) in list() {
            if let Some(stem) = json.file_stem().and_then(|s| s.to_str()) {
                if let Some(stamp) = stem.strip_prefix("services-") {
                    let _ = std::fs::write(json.with_file_name(format!("RESTORE-services-{stamp}.cmd")), restore_cmd(&b));
                }
            }
        }
        BackupsWindow { backups: list(), job: None, op: None, pending: None, run: None, msg: None, current: read_current().unwrap_or_default() }
    }

    pub fn busy(&self) -> bool {
        self.run.is_some() || self.job.is_some()
    }

    /// Draw; returns (keep open, services changed so the list should reload).
    pub fn show(&mut self, ctx: &egui::Context) -> (bool, bool) {
        let mut changed = false;
        if let Some(j) = &self.job {
            if let Some(r) = j.poll() {
                self.job = None;
                self.msg = Some(match r {
                    Ok(p) => (format!("Backup saved: {}", p.display()), true),
                    Err(e) => (format!("Backup failed: {e}"), false),
                });
                self.backups = list();
                self.current = read_current().unwrap_or_default();
            }
        }
        if let Some(run) = &self.run {
            // results as they come; the channel closes when the restore has finished
            let mut running = true;
            loop {
                match run.rx.try_recv() {
                    Ok((i, r)) => {
                        if let Some(it) = self.op.as_mut().and_then(|op| op.items.get_mut(i)) {
                            it.result = Some(r);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        running = false;
                        break;
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            if !running {
                self.run = None;
                if let Some(op) = &mut self.op {
                    for it in op.items.iter_mut().filter(|i| i.result.is_none()) {
                        it.result = Some(Err("No result - the restore stopped early.".into()));
                    }
                    op.notes.push("Restart the PC so every service starts the way the backup says.".into());
                    op.done();
                }
                changed = true;
                self.current = read_current().unwrap_or_default();
            }
        }
        if let Some(op) = &mut self.op {
            let can = common::is_admin() && self.run.is_none();
            match op.show(ctx, can) {
                Some(OpEvent::Go) => {
                    if let Some(diffs) = self.pending.take() {
                        let (tx, rx) = mpsc::channel();
                        self.run = Some(Running { rx, _job: Job::spawn(move || restore(diffs, tx)) });
                        op.stage = Stage::Running;
                    }
                }
                Some(OpEvent::Close) => {
                    self.op = None;
                    self.pending = None;
                }
                _ => {}
            }
            return (true, changed);
        }

        let mut keep = true;
        let admin = common::is_admin();
        theme::dim_backdrop(ctx, "svc-backups");
        egui::Window::new("svc-backups").title_bar(false).collapsible(false).resizable(false).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).frame(theme::modal_frame(ctx)).show(ctx, |ui| {
            ui.set_width(620.0);
            theme::section(ui, "Services");
            ui.label(RichText::new("Backups").size(24.0).strong());
            ui.label(RichText::new("A backup saves the startup type of every service. Each one has three ways back: Restore here, a double-click RESTORE .cmd (works in Safe Mode), and a full .reg export.").color(theme::TEXT_DIM));
            ui.add_space(6.0);
            if let Some((m, ok)) = &self.msg {
                theme::banner(ui, if *ok { theme::OK } else { theme::LOCKED }, m);
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(admin && self.job.is_none(), theme::primary_button("   Create backup now   ")).clicked() {
                    self.job = Some(Job::spawn(|| create("made by you")));
                }
                if self.job.is_some() {
                    ui.spinner();
                }
                if ui.button("Open backup folder").clicked() {
                    let d = backup_dir();
                    let _ = std::fs::create_dir_all(&d);
                    crate::winsvc::open_folder(&d);
                }
            });
            if !admin {
                ui.colored_label(theme::LOCKED, "Run PC Tuner as administrator to create or restore backups.");
            }
            ui.add_space(6.0);
            if self.backups.is_empty() {
                ui.label(RichText::new("No backups yet. PC Tuner also makes one by itself before the first service change.").color(theme::TEXT_DIM));
            }
            let current = self.current.clone();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (path, b) in self.backups.clone() {
                    let diffs = differences(&b.services, &current);
                    theme::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&b.time).strong());
                                let note = if b.note.is_empty() { String::new() } else { format!(" - {}", b.note) };
                                ui.label(RichText::new(format!("{} services{note}", b.services.len())).size(12.0).color(theme::TEXT_DIM));
                                let d = if diffs.is_empty() { "matches the PC now".to_string() } else { format!("{} differ from now", diffs.len()) };
                                ui.label(RichText::new(d).size(12.0).color(if diffs.is_empty() { theme::OK } else { theme::GAMING }));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let btn = if diffs.is_empty() { egui::Button::new("Restore this backup") } else { theme::primary_button("  Restore this backup  ") };
                                if ui.add_enabled(admin && self.run.is_none(), btn).on_hover_text("Puts every service back to the startup type in this backup.").clicked() {
                                    let items = diffs.iter().map(|(bk, cur)| OpItem::new(bk.name.clone(), if bk.display.is_empty() { bk.name.clone() } else { format!("{} ({})", bk.display, bk.name) }, "Startup type").detail(format!("{} -> {}", type_name(cur.start, cur.delayed), type_name(bk.start, bk.delayed)))).collect();
                                    let mut w = OpWindow::new("RESTORE SERVICES BACKUP", "restore", "restored", items).noun("service", "services");
                                    w.show_steps = true;
                                    w.preview_text = format!("Back to the backup from {}.", b.time);
                                    w.go_label = "Restore".into();
                                    w.running_text = "Restoring services...".into();
                                    w.reassure = "Only startup types change. Protected services are restored as SYSTEM; a restart finishes it.".into();
                                    // with nothing to change the window opens in its "nothing to do" state
                                    w.empty_title = "Nothing to restore".into();
                                    w.empty_text = format!("Every service already has the startup type saved in the backup from {}.", b.time);
                                    self.op = Some(w);
                                    self.pending = if diffs.is_empty() { None } else { Some(diffs.clone()) };
                                }
                                let _ = &path;
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
            });
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                keep = false;
            }
        });
        (keep, changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: &str, start: u32, delayed: Option<u32>) -> Svc {
        Svc { name: n.into(), display: String::new(), start, delayed }
    }

    #[test]
    fn svcbackup_differences() {
        let backup = vec![s("A", 2, Some(1)), s("B", 3, None), s("C", 4, None), s("Gone", 2, None)];
        let now = vec![s("a", 2, Some(1)), s("B", 4, None), s("C", 4, None), s("New", 3, None)];
        let d = differences(&backup, &now);
        assert_eq!(d.len(), 1, "only B differs; missing services are skipped");
        assert_eq!(d[0].0.name, "B");
        let d = differences(&[s("A", 2, Some(1))], &[s("A", 2, None)]);
        assert_eq!(d.len(), 1, "Automatic (Delayed) vs Automatic counts");
        assert_eq!(type_name(2, Some(1)), "Automatic (Delayed)");
        // what the Service Control Manager is asked for: only Automatic touches Delayed start
        assert_eq!(scm_type(2, Some(1)), (2, Some(true)));
        assert_eq!(scm_type(2, None), (2, Some(false)));
        assert_eq!(scm_type(3, Some(1)), (3, None));
        assert_eq!(scm_type(4, None), (4, None));
    }

    /// The .reg file is written in regedit's own format.
    #[test]
    fn svcbackup_reg_format() {
        let w = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect() };
        assert_eq!(reg_value_line("Start", 4, &2u32.to_le_bytes()), r#""Start"=dword:00000002"#);
        assert_eq!(reg_value_line("DisplayName", 1, &w(r#"My "quoted" C:\path"#)), r#""DisplayName"="My \"quoted\" C:\\path""#);
        assert_eq!(reg_value_line("", 1, &w("x")), r#"@="x""#);
        assert_eq!(reg_value_line("ImagePath", 2, &w("%a%")), r#""ImagePath"=hex(2):25,00,61,00,25,00,00,00"#);
        assert_eq!(reg_value_line("B", 3, &[1, 0xab]), r#""B"=hex:01,ab"#);
        assert_eq!(reg_value_line("M", 7, &[0, 0]), r#""M"=hex(7):00,00"#);
        assert_eq!(reg_value_line("Q", 11, &[1, 0, 0, 0, 0, 0, 0, 0]), r#""Q"=hex(b):01,00,00,00,00,00,00,00"#);
        assert_eq!(reg_name(r#"a\b"#), r#""a\\b""#);
    }

    /// Opt-in (`cargo test -- --ignored svcbackup_reg_same_as`): our .reg against `reg export` of the same key -
    /// the same keys and the same values (regedit wraps long hex lines; they are joined before comparing).
    #[test]
    #[ignore]
    fn svcbackup_reg_same_as_reg_exe() {
        let dir = std::env::temp_dir();
        let (ours, theirs) = (dir.join("pctuner-ours.reg"), dir.join("pctuner-reg-exe.reg"));
        export_reg(&ours).unwrap();
        let (code, _) = crate::common::run("reg.exe", &["export", &format!(r"HKLM\{SERVICES}"), &theirs.display().to_string(), "/y"]).unwrap();
        assert_eq!(code, 0);
        let read = |p: &std::path::Path| -> Vec<String> {
            let b = std::fs::read(p).unwrap();
            let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let t = String::from_utf16_lossy(&u).replace("\\\r\n  ", "");
            t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
        };
        let (a, b) = (read(&ours), read(&theirs));
        let _ = (std::fs::remove_file(&ours), std::fs::remove_file(&theirs));
        let sa: std::collections::HashSet<&String> = a.iter().collect();
        let sb: std::collections::HashSet<&String> = b.iter().collect();
        let only_ours: Vec<&&String> = sa.difference(&sb).take(5).collect();
        let only_reg: Vec<&&String> = sb.difference(&sa).take(5).collect();
        println!("lines: ours {} / reg.exe {}; first only-ours {only_ours:?}; first only-reg.exe {only_reg:?}", a.len(), b.len());
        // keys or values can change between the two reads (a service's state data); a few lines at most
        assert!(sa.symmetric_difference(&sb).count() <= 10, "the same keys and values");
    }

    /// Read-only on the registry: the real Services key exported to a temporary file.
    #[test]
    fn svcbackup_reg_export_this_pc() {
        let path = std::env::temp_dir().join(format!("pctuner-test-services-{}.reg", std::process::id()));
        export_reg(&path).expect("export");
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16 with BOM, like reg export");
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16(&u).unwrap();
        assert!(text.starts_with("Windows Registry Editor Version 5.00\r\n"));
        assert!(text.contains(r"[HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\Winmgmt]"));
        assert!(text.contains("\"Start\"=dword:0000000"));
        assert!(text.matches("\r\n[HKEY_LOCAL_MACHINE").count() > 300, "every service key and its subkeys");
    }

    #[test]
    fn svcbackup_scripts() {
        let b = Backup { time: "t".into(), note: String::new(), services: vec![s("Spooler", 2, None), s("SysMain", 2, Some(1))] };
        let c = restore_cmd(&b);
        assert!(c.contains(r#"reg add "HKLM\SYSTEM\CurrentControlSet\Services\Spooler" /v Start /t REG_DWORD /d 2 /f"#));
        assert!(c.contains(r#"reg query "HKLM\SYSTEM\CurrentControlSet\Services\Spooler" /v Start 2>nul | find "0x2" >nul ||"#), "unchanged services are skipped");
        assert!(c.contains(r#"Services\SysMain" /v DelayedAutostart /t REG_DWORD /d 1"#) && c.contains(r#"Services\Spooler" /v DelayedAutostart /f"#));
        assert!(c.contains("net session"), "asks for administrator");
        assert!(!c.to_lowercase().contains("powershell"), "the hand-run file uses cmd and reg only");
    }

    #[test]
    fn svcbackup_reads_this_pc() {
        let v = read_current().expect("service list");
        assert!(v.len() > 100, "a Windows install has well over 100 services");
        assert!(v.iter().all(|x| (2..=4).contains(&x.start)));
        assert!(v.iter().any(|x| x.name.eq_ignore_ascii_case("Winmgmt")));
    }
}
