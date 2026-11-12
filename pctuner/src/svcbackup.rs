//! Services backups: a full snapshot of every service's startup type, with three independent ways back.
//!   1. Restore in PC Tuner - only the services that differ; each one via sc.exe config, then a direct
//!      registry write, then (for services Windows protects even from admins) the same write as SYSTEM.
//!   2. RESTORE-services-<time>.cmd next to the backup - double-click as administrator; works in Safe Mode.
//!   3. services-<time>.reg - a full export of HKLM\SYSTEM\CurrentControlSet\Services (last resort).
//! Restore only changes Start / DelayedAutostart of services that exist, never paths or accounts.

use crate::common::{self, Job, Stream};
use crate::opwin::{OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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

fn sc_type(start: u32, delayed: Option<u32>) -> &'static str {
    match (start, delayed) {
        (2, Some(1)) => "delayed-auto",
        (2, _) => "auto",
        (3, _) => "demand",
        _ => "disabled",
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
    let _ = common::run("reg.exe", &["export", &format!(r"HKLM\{SERVICES}"), &reg.display().to_string(), "/y"]);
    Ok(json)
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

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Restore script: per service sc.exe config, else registry write; then the leftovers as SYSTEM.
/// One RESULT|index|ok|err line per service.
fn restore_script(diffs: &[(Svc, Svc)], work: &str) -> String {
    let mut s = String::from("$failed = New-Object System.Collections.ArrayList\n");
    for (i, (b, _)) in diffs.iter().enumerate() {
        let key = format!(r"HKLM:\{SERVICES}\{}", b.name);
        let delayed = match b.delayed {
            Some(d) => format!("Set-ItemProperty -LiteralPath {k} -Name DelayedAutostart -Type DWord -Value {d} -ErrorAction SilentlyContinue", k = q(&key)),
            None => format!("Remove-ItemProperty -LiteralPath {k} -Name DelayedAutostart -ErrorAction SilentlyContinue", k = q(&key)),
        };
        s.push_str(&format!(
            "$ok = $false; $o = sc.exe config {n} start= {t} 2>&1; if ($LASTEXITCODE -eq 0) {{ $ok = $true }} else {{ try {{ Set-ItemProperty -LiteralPath {k} -Name Start -Type DWord -Value {v} -ErrorAction Stop; $ok = $true }} catch {{ }} }}\n\
if ($ok) {{ {delayed}; Write-Output 'RESULT|{i}|ok|' }} else {{ [void]$failed.Add(@{{ i = {i}; k = {k}; v = {v}; d = '{d}' }}) }}\n",
            n = q(&b.name),
            t = sc_type(b.start, b.delayed),
            k = q(&key),
            v = b.start,
            d = b.delayed.map(|d| d.to_string()).unwrap_or_default(),
        ));
    }
    // protected services: write the same registry values as SYSTEM through a one-time task
    s.push_str(&format!(
        r#"if ($failed.Count -gt 0) {{
  $lines = foreach ($f in $failed) {{
    "Set-ItemProperty -LiteralPath '$($f.k)' -Name Start -Type DWord -Value $($f.v)"
    if ($f.d -ne '') {{ "Set-ItemProperty -LiteralPath '$($f.k)' -Name DelayedAutostart -Type DWord -Value $($f.d)" }} else {{ "Remove-ItemProperty -LiteralPath '$($f.k)' -Name DelayedAutostart -ErrorAction SilentlyContinue" }}
  }}
  $file = Join-Path {work} 'restore-services-system.ps1'
  Set-Content -LiteralPath $file -Value $lines -Encoding UTF8
  try {{
    $a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$file`""
    $p = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    Register-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'RestoreServices' -Action $a -Principal $p -Force | Out-Null
    Start-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'RestoreServices'
    $t0 = Get-Date
    do {{ Start-Sleep -Milliseconds 500; $st = (Get-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'RestoreServices').State }} while ($st -eq 'Running' -and ((Get-Date) - $t0).TotalSeconds -lt 30)
  }} catch {{ }}
  Unregister-ScheduledTask -TaskPath '\PCTuner\' -TaskName 'RestoreServices' -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue
  foreach ($f in $failed) {{
    $now = (Get-ItemProperty -LiteralPath $f.k -Name Start -ErrorAction SilentlyContinue).Start
    if ($now -eq $f.v) {{ Write-Output "RESULT|$($f.i)|ok|restored as SYSTEM (protected service)" }} else {{ Write-Output "RESULT|$($f.i)|err|Windows protects this service - restore it with the RESTORE .cmd in Safe Mode" }}
  }}
}}
"#,
        work = q(work)
    ));
    s
}

fn parse_result(line: &str) -> Option<(usize, Result<String, String>)> {
    let rest = line.trim().strip_prefix("RESULT|")?;
    let mut p = rest.splitn(3, '|');
    let i = p.next()?.parse().ok()?;
    let ok = p.next()? == "ok";
    let msg = p.next().unwrap_or("").trim().to_string();
    Some((i, if ok { Ok(msg) } else { Err(if msg.is_empty() { "Failed.".into() } else { msg }) }))
}

/// The Backups window (opened from the Services tab).
pub struct BackupsWindow {
    backups: Vec<(PathBuf, Backup)>,
    job: Option<Job<Result<PathBuf, String>>>,
    op: Option<OpWindow>,
    pending: Option<Vec<(Svc, Svc)>>,
    run: Option<(Stream, usize)>,
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
        if let Some((stream, seen)) = &mut self.run {
            let running = stream.pump();
            while *seen < stream.lines.len() {
                let line = stream.lines[*seen].clone();
                *seen += 1;
                if let (Some((i, r)), Some(op)) = (parse_result(&line), &mut self.op) {
                    if let Some(it) = op.items.get_mut(i) {
                        it.result = Some(r);
                    }
                }
            }
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
                        match common::secure_dir() {
                            Ok(d) => {
                                let script = restore_script(&diffs, &d.display().to_string());
                                self.run = Some((common::ps_stream(&script), 0));
                                op.stage = Stage::Running;
                            }
                            Err(e) => {
                                self.msg = Some((format!("Restore could not start: {e}"), false));
                                self.op = None;
                            }
                        }
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
                    let _ = common::run("explorer.exe", &[&d.display().to_string()]);
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
        assert_eq!(sc_type(2, Some(1)), "delayed-auto");
        assert_eq!(sc_type(3, None), "demand");
    }

    #[test]
    fn svcbackup_scripts() {
        let b = Backup { time: "t".into(), note: String::new(), services: vec![s("Spooler", 2, None), s("SysMain", 2, Some(1))] };
        let c = restore_cmd(&b);
        assert!(c.contains(r#"reg add "HKLM\SYSTEM\CurrentControlSet\Services\Spooler" /v Start /t REG_DWORD /d 2 /f"#));
        assert!(c.contains(r#"reg query "HKLM\SYSTEM\CurrentControlSet\Services\Spooler" /v Start 2>nul | find "0x2" >nul ||"#), "unchanged services are skipped");
        assert!(c.contains(r#"Services\SysMain" /v DelayedAutostart /t REG_DWORD /d 1"#) && c.contains(r#"Services\Spooler" /v DelayedAutostart /f"#));
        assert!(c.contains("net session"), "asks for administrator");
        let r = restore_script(&[(s("Spooler", 2, None), s("Spooler", 4, None))], r"C:\ProgramData\PCTuner");
        assert!(r.contains("sc.exe config 'Spooler' start= auto") && r.contains("RESULT|0|ok|") && r.contains("UserId 'SYSTEM'"));
        assert!(r.contains("Unregister-ScheduledTask"), "the SYSTEM task is always removed");
        // the generated script must parse
        let check = format!("$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$e); $e.Count", q(&r));
        assert_eq!(crate::common::ps(&check).unwrap().trim(), "0", "restore script has PowerShell syntax errors");
        assert_eq!(parse_result("RESULT|0|err|x"), Some((0, Err("x".into()))));
        // the generated script must parse
        let check = format!("$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$e); $e.Count", q(&r));
        assert_eq!(crate::common::ps(&check).unwrap().trim(), "0", "restore script has PowerShell syntax errors");
    }

    #[test]
    fn svcbackup_reads_this_pc() {
        let v = read_current().expect("service list");
        assert!(v.len() > 100, "a Windows install has well over 100 services");
        assert!(v.iter().all(|x| (2..=4).contains(&x.start)));
        assert!(v.iter().any(|x| x.name.eq_ignore_ascii_case("Winmgmt")));
    }
}
