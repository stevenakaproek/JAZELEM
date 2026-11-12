//! Post-install tweaks tab - runs PostInstall-Tweaks.ps1 (embedded) and shows Teredo / Widgets status.

use crate::common::{self, Stream, Tab};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};

const SCRIPT: &str = include_str!("../assets/PostInstall-Tweaks.ps1");

const G_TASKS: &str = "Scheduled tasks";
const G_EDGE: &str = "Edge";
const G_XBOX: &str = "Xbox networking";
const G_ONEDRIVE: &str = "OneDrive";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Apply,
    ApplyOneDrive,
    Undo,
}

impl Mode {
    fn args(self) -> Vec<String> {
        match self {
            Mode::Apply => vec![],
            Mode::ApplyOneDrive => vec!["-RemoveOneDrive".into()],
            Mode::Undo => vec!["-Undo".into()],
        }
    }
    fn title(self) -> &'static str {
        match self {
            Mode::Apply => "Apply tweaks",
            Mode::ApplyOneDrive => "Apply + remove OneDrive",
            Mode::Undo => "Undo tweaks",
        }
    }
    fn groups(self) -> Vec<&'static str> {
        match self {
            Mode::ApplyOneDrive => vec![G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE],
            _ => vec![G_TASKS, G_EDGE, G_XBOX],
        }
    }
}

#[derive(Default, Clone)]
struct Status {
    teredo: String,
    teredo_state: String,
    widgets_policy: String,
    widgets_running: bool,
    edge_boost: String,
}

fn read_status() -> Status {
    let mut teredo_state = String::new();
    let teredo = common::run("netsh", &["interface", "teredo", "show", "state"])
        .map(|(_, t)| {
            let get = |k: &str| t.lines().find(|l| l.trim_start().starts_with(k)).and_then(|l| l.split(':').nth(1)).map(|s| s.trim().to_string()).unwrap_or_default();
            teredo_state = get("State");
            format!("type {}, state {}", get("Type"), get("State"))
        })
        .unwrap_or_else(|e| e);
    let hklm = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
    let widgets_policy = hklm
        .open_subkey(r"SOFTWARE\Policies\Microsoft\Dsh")
        .and_then(|k| k.get_value::<u32, _>("AllowNewsAndInterests"))
        .map(|v| if v == 0 { "off by policy".to_string() } else { format!("policy = {v}") })
        .unwrap_or_else(|_| "not set".into());
    let edge_boost = hklm
        .open_subkey(r"SOFTWARE\Policies\Microsoft\Edge")
        .and_then(|k| k.get_value::<u32, _>("StartupBoostEnabled"))
        .map(|v| if v == 0 { "off".to_string() } else { "on".to_string() })
        .unwrap_or_else(|_| "Windows default (on)".into());
    let widgets_running = common::run("tasklist", &["/FI", "IMAGENAME eq WidgetBoard.exe", "/NH"])
        .map(|(_, t)| t.contains("WidgetBoard.exe"))
        .unwrap_or(false);
    Status { teredo, teredo_state, widgets_policy, widgets_running, edge_boost }
}

// ------------------------------------------------------------------ what the script does / did

/// The scheduled-task paths listed in the embedded script (`$Tasks = @( ... )`), so the preview never drifts from it.
fn script_tasks() -> Vec<String> {
    SCRIPT
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("$Tasks = @("))
        .skip(1)
        .take_while(|l| l.trim() != ")")
        .map(|l| l.trim().trim_matches('\'').to_string())
        .filter(|l| l.starts_with('\\'))
        .collect()
}

/// "\Microsoft\Windows\Maps\MapsToastTask" -> ("MapsToastTask", "Maps")
fn task_parts(path: &str) -> (String, String) {
    let mut it = path.trim_end_matches('\\').rsplit('\\');
    let name = it.next().unwrap_or(path).to_string();
    let folder = it.next().unwrap_or("").to_string();
    (name, folder)
}

/// The items shown in the preview: one per thing the script will touch.
fn preview_items(mode: Mode) -> Vec<OpItem> {
    let undo = mode == Mode::Undo;
    let mut v: Vec<OpItem> = script_tasks()
        .iter()
        .map(|p| {
            let (name, folder) = task_parts(p);
            OpItem::new(p.clone(), name, G_TASKS).detail(folder)
        })
        .collect();
    v.push(OpItem::new("softlanding", "Windows tips tasks", G_TASKS).detail("SoftLanding"));
    if undo {
        v.push(OpItem::new("edge", "Edge policies", G_EDGE).detail("removed - Edge back to its defaults"));
        v.push(OpItem::new("teredo", "Teredo policy", G_XBOX).detail("removed - Teredo back to the Windows default"));
    } else {
        v.push(OpItem::new("edge-boost", "Startup Boost", G_EDGE).detail("off - Edge stays installed"));
        v.push(OpItem::new("edge-bg", "Background mode", G_EDGE).detail("off"));
        v.push(OpItem::new("teredo", "Teredo", G_XBOX).detail("enterpriseclient, kept on by Group Policy"));
    }
    if mode == Mode::ApplyOneDrive {
        v.push(OpItem::new("onedrive", "OneDrive", G_ONEDRIVE).detail("uninstalled, its tasks disabled"));
    }
    v
}

fn preview_window(mode: Mode) -> OpWindow {
    let (verb, done_word) = if mode == Mode::Undo { ("undo", "undone") } else { ("apply", "applied") };
    let mut w = OpWindow::new("POST-INSTALL TWEAKS", verb, done_word, preview_items(mode)).noun("tweak", "tweaks");
    w.go_label = mode.title().into();
    match mode {
        Mode::Apply => {
            w.preview_text = "Telemetry and tips tasks off, Edge no longer preloads, Teredo on for Xbox online play.".into();
            w.reassure = "Xbox and Steam tasks are kept. Everything can be put back with \"Undo tweaks\".".into();
        }
        Mode::ApplyOneDrive => {
            w.preview_text = "The same tweaks, and OneDrive is uninstalled.".into();
            w.warning = Some("OneDrive will be uninstalled. \"Undo tweaks\" does not bring it back - reinstall it from the Microsoft Store if you want it again.".into());
            w.reassure = "Xbox and Steam tasks are kept. The other tweaks can be put back with \"Undo tweaks\".".into();
        }
        Mode::Undo => {
            w.preview_text = "Tasks are re-enabled and the Edge and Teredo policies are removed.".into();
            w.warning = Some("Teredo goes back to the Windows default. Xbox party chat and multiplayer may stop connecting until you apply the tweaks again.".into());
        }
    }
    w
}

/// The steps shown while the script runs (one per section of its output).
fn step_items(mode: Mode) -> Vec<OpItem> {
    mode.groups()
        .into_iter()
        .map(|g| {
            let name = match g {
                G_TASKS => "Scheduled tasks",
                G_EDGE => "Edge background launching",
                G_XBOX => "Xbox networking (Teredo)",
                _ => "OneDrive",
            };
            OpItem::new(g, name, g)
        })
        .collect()
}

struct LogItem {
    group: &'static str,
    name: String,
    detail: String,
    result: Result<String, String>,
}

#[derive(Default)]
struct Parsed {
    items: Vec<LogItem>,
    /// Section headers seen so far, in order.
    sections: Vec<&'static str>,
    /// The closing "Done." line was printed.
    finished: bool,
}

/// Turn the script's output into one row per thing it did.
fn parse_log(lines: &[String]) -> Parsed {
    let mut p = Parsed::default();
    let mut cur: Option<&'static str> = None;
    for raw in lines {
        let t = raw.trim();
        if t.is_empty() || p.finished {
            continue;
        }
        let header = match t {
            "Scheduled tasks:" => Some(G_TASKS),
            "Edge background launching:" => Some(G_EDGE),
            "Xbox networking (Teredo):" => Some(G_XBOX),
            "OneDrive:" => Some(G_ONEDRIVE),
            _ => None,
        };
        if let Some(h) = header {
            cur = Some(h);
            p.sections.push(h);
            continue;
        }
        if t.starts_with("Done.") {
            p.finished = true;
            continue;
        }
        match cur {
            Some(G_TASKS) => {
                let (word, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
                let rest = rest.trim();
                let mut push = |path: &str, detail: &str, result: Result<String, String>| {
                    let (name, folder) = task_parts(path);
                    let detail = if detail.is_empty() { folder } else { detail.to_string() };
                    p.items.push(LogItem { group: G_TASKS, name, detail, result });
                };
                match word {
                    "skip" => push(rest.trim_end_matches("(not present)").trim(), "", Ok("Not on this PC - nothing to do.".into())),
                    "disabled" | "enabled" => push(rest, word, Ok(String::new())),
                    "FAIL" => {
                        let (path, why) = rest.split_once(" : ").unwrap_or((rest, "the task could not be changed"));
                        push(path.trim(), "", Err(why.trim().to_string()));
                    }
                    _ => {}
                }
            }
            Some(G_EDGE) => {
                if t.starts_with("Startup Boost and background mode turned off") {
                    p.items.push(LogItem { group: G_EDGE, name: "Startup Boost and background mode".into(), detail: "off".into(), result: Ok("Edge shows \"Managed by your organization\" - expected.".into()) });
                } else if t.starts_with("Edge policies removed") {
                    p.items.push(LogItem { group: G_EDGE, name: "Edge policies".into(), detail: "removed".into(), result: Ok(String::new()) });
                }
            }
            Some(G_XBOX) => {
                if let Some(rest) = t.strip_prefix("Teredo: ") {
                    let detail = rest.split_once("-> ").map(|x| x.1).unwrap_or(rest).replace('\'', "");
                    p.items.push(LogItem { group: G_XBOX, name: "Teredo".into(), detail, result: Ok(String::new()) });
                } else if t.starts_with("Teredo policy removed") {
                    p.items.push(LogItem { group: G_XBOX, name: "Teredo policy".into(), detail: "removed, Windows default".into(), result: Ok(String::new()) });
                } else if t.starts_with("Not qualified yet") || t.starts_with("Tip:") {
                    let text = if t.starts_with("Tip:") { t.trim_start_matches("Tip:").trim().to_string() } else { "Not qualified yet - check again in a minute.".to_string() };
                    if let Some(LogItem { group: G_XBOX, result: Ok(note), .. }) = p.items.last_mut() {
                        if !note.is_empty() {
                            note.push(' ');
                        }
                        note.push_str(&text);
                    }
                }
            }
            Some(G_ONEDRIVE) => {
                if t.starts_with("OneDrive installer not found") {
                    p.items.push(LogItem { group: G_ONEDRIVE, name: "OneDrive".into(), detail: String::new(), result: Err("Installer not found - uninstall it from Settings > Apps.".into()) });
                } else if t.starts_with("OneDrive removed") && !p.items.iter().any(|i| i.group == G_ONEDRIVE) {
                    p.items.push(LogItem { group: G_ONEDRIVE, name: "OneDrive".into(), detail: "uninstalled".into(), result: Ok("Reinstall any time from the Microsoft Store.".into()) });
                }
            }
            _ => {}
        }
    }
    p
}

/// Result rows for the Done stage. A script that died without output becomes one failed row.
fn result_items(lines: &[String], exit: i32) -> Vec<OpItem> {
    let mut items: Vec<OpItem> = parse_log(lines)
        .items
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let mut it = OpItem::new(format!("r{i}"), l.name, l.group).detail(l.detail);
            it.result = Some(l.result);
            it
        })
        .collect();
    if exit != 0 || items.is_empty() {
        let last = lines.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("no output");
        let mut it = OpItem::new("script", "PostInstall-Tweaks.ps1", "Script");
        it.result = Some(Err(format!("The script stopped early (exit code {exit}): {last}")));
        items.push(it);
    }
    items
}

/// Tick the running steps off as the script's output moves from section to section.
fn update_steps(op: &mut OpWindow, parsed: &Parsed, tasks_total: usize) {
    for step in op.items.iter_mut() {
        let Some(pos) = parsed.sections.iter().position(|s| *s == step.group) else { continue };
        let group = step.group.clone();
        let in_group = parsed.items.iter().filter(|i| i.group == group);
        // a step is ticked when the script has moved past it; what failed inside it is listed in the result stage
        let failed = in_group.clone().filter(|i| i.result.is_err()).count();
        let problems = if failed == 0 { String::new() } else { format!("{failed} failed") };
        step.detail = if group == G_TASKS {
            let count = format!("{} / {}", in_group.count().min(tasks_total), tasks_total);
            if failed == 0 { count } else { format!("{count}, {problems}") }
        } else {
            problems
        };
        if parsed.finished || pos + 1 < parsed.sections.len() {
            step.result = Some(Ok(String::new()));
        }
    }
}

const DEMO_LOG: &[&str] = &[
    "Scheduled tasks:",
    r"  disabled  \Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser",
    r"  disabled  \Microsoft\Windows\Application Experience\ProgramDataUpdater",
    r"  disabled  \Microsoft\Windows\Customer Experience Improvement Program\Consolidator",
    r"  FAIL      \Microsoft\Windows\Customer Experience Improvement Program\UsbCeip : Access is denied.",
    r"  disabled  \Microsoft\Windows\Feedback\Siuf\DmClient",
    r"  disabled  \Microsoft\Windows\Windows Error Reporting\QueueReporting",
    r"  disabled  \Microsoft\Windows\Maps\MapsToastTask",
    r"  disabled  \Microsoft\Windows\Maps\MapsUpdateTask",
    r"  skip      \Microsoft\Windows\RetailDemo\CleanupOfflineContent (not present)",
    r"  disabled  \Microsoft\Windows\Maintenance\WinSAT",
    "",
    "Edge background launching:",
    "  Startup Boost and background mode turned off (Edge shows \"Managed by your organization\" - expected)",
    "",
    "Xbox networking (Teredo):",
    "  Teredo: enterpriseclient via win1910.ipv6.microsoft.com -> state 'qualified', router UPnP 'Yes'",
    "",
    "Done. Widgets: Windows blocks scripts from switching it off - use gpedit.msc >",
];

// ------------------------------------------------------------------ tab

/// A hero-row stat that is left out when the window is too narrow for it (instead of wrapping letter by letter).
fn stat_fit(ui: &mut egui::Ui, gap: f32, value: &str, label: &str, c: egui::Color32) {
    if ui.available_width() < gap + 7.0 * label.chars().count().max(value.chars().count() * 2) as f32 {
        return;
    }
    ui.add_space(gap);
    theme::stat(ui, value, label, c);
}

/// Give empty tick boxes an outline so they can be seen on a card.
fn visible_checkboxes(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for w in [&mut widgets.inactive, &mut widgets.hovered] {
        w.bg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_FAINT);
        w.rounding = egui::Rounding::same(5.0);
    }
}

pub struct TweaksTab {
    status: Option<Status>,
    job: Option<common::Job<Status>>,
    stream: Option<Stream>,
    /// What the running / last script was started as.
    mode: Option<Mode>,
    op: Option<OpWindow>,
    /// Mode of the window that is open in the preview stage.
    pending: Option<Mode>,
    show_details: bool,
    demo: bool,
}

impl TweaksTab {
    pub fn new() -> Self {
        let mut tab = TweaksTab { status: None, job: None, stream: None, mode: None, op: None, pending: None, show_details: false, demo: false };
        if let Some(stage) = opwin::demo_stage("tweaks") {
            // sample window for screenshots - the script is not started
            let mut w = preview_window(Mode::Apply);
            let log: Vec<String> = DEMO_LOG.iter().map(|s| s.to_string()).collect();
            match stage {
                Stage::Preview => {}
                Stage::Running => {
                    Self::to_running(&mut w, Mode::Apply);
                    let upto = log.iter().position(|l| l.starts_with("Xbox")).unwrap_or(log.len());
                    let mut parsed = parse_log(&log[..=upto]);
                    for _ in parsed.items.iter().filter(|i| i.group == G_TASKS).count()..script_tasks().len() {
                        parsed.items.push(LogItem { group: G_TASKS, name: String::new(), detail: String::new(), result: Ok(String::new()) });
                    }
                    update_steps(&mut w, &parsed, script_tasks().len());
                }
                Stage::Done => {
                    w.items = result_items(&log, 0);
                    w.can_undo = true;
                    w.undo_label = "Undo tweaks".into();
                    w.done();
                }
            }
            tab.op = Some(w);
            tab.demo = true;
        }
        tab
    }

    /// Switch a window to the step list shown while the script runs.
    fn to_running(w: &mut OpWindow, mode: Mode) {
        w.items = step_items(mode);
        w.show_steps = true;
        w.running_text = if mode == Mode::Undo { "Undoing the tweaks...".into() } else { "Applying the tweaks...".into() };
        w.stage = Stage::Running;
    }

    fn launch(&mut self, mode: Mode) {
        let path = common::write_script("PostInstall-Tweaks.ps1", SCRIPT);
        self.mode = Some(mode);
        self.stream = Some(Stream::run_ps_file(&path, &mode.args()));
        if let Some(op) = &mut self.op {
            Self::to_running(op, mode);
        }
    }

    fn pump(&mut self) {
        if self.status.is_none() && self.job.is_none() {
            self.job = Some(common::Job::spawn(read_status));
        }
        if let Some(j) = &self.job {
            if let Some(s) = j.poll() {
                self.status = Some(s);
                self.job = None;
            }
        }
        let Some(s) = &mut self.stream else { return };
        let was_running = s.exit.is_none();
        s.pump();
        if !was_running {
            return;
        }
        let Some(op) = &mut self.op else { return };
        match s.exit {
            None => update_steps(op, &parse_log(&s.lines), script_tasks().len()),
            Some(code) => {
                self.status = None; // refresh after a run
                op.items = result_items(&s.lines, code);
                op.can_undo = self.mode != Some(Mode::Undo);
                op.undo_label = "Undo tweaks".into();
                if self.mode == Some(Mode::Undo) {
                    (op.verb, op.done_word) = ("undo".into(), "undone".into());
                }
                op.done();
            }
        }
    }

    fn window(&mut self, ctx: &egui::Context) {
        let Some(op) = &mut self.op else { return };
        let running = self.stream.as_ref().map(|s| s.exit.is_none()).unwrap_or(false);
        let can = common::is_admin() && !running && !self.demo;
        match op.show(ctx, can) {
            Some(OpEvent::Go) => {
                if let Some(mode) = self.pending.take() {
                    self.launch(mode);
                }
            }
            // same preview (with its warning) as the page's "Undo tweaks" button - never a one-click undo
            Some(OpEvent::Undo) => self.ask(Mode::Undo),
            Some(OpEvent::Close) => {
                self.op = None;
                self.pending = None;
            }
            _ => {}
        }
    }

    fn ask(&mut self, mode: Mode) {
        self.pending = Some(mode);
        self.op = Some(preview_window(mode));
    }
}

impl Tab for TweaksTab {
    fn title(&self) -> &'static str {
        "Post-install Tweaks"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.pump();
        self.window(ui.ctx());

        theme::page_header(ui, "Post-install Tweaks", "Re-applies what a fresh Windows install brings back: telemetry tasks, Edge preloading, and Teredo for Xbox online play.");
        common::admin_banner(ui);
        egui::ScrollArea::vertical().id_salt("tweaks-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl TweaksTab {
    fn page(&mut self, ui: &mut egui::Ui) {
        // ---- hero: is Xbox online play ready?
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            match &self.status {
                None => {
                    theme::hero(ui, "...", "Xbox online play", "Reading the Teredo, Edge and Widgets state", theme::TEXT_FAINT);
                    theme::sweep(ui, ui.available_width());
                }
                Some(s) => {
                    let ready = s.teredo.contains("qualified") || s.teredo.contains("dormant");
                    ui.horizontal(|ui| {
                        let caption = if s.teredo_state.is_empty() { s.teredo.clone() } else { format!("Teredo {}", s.teredo.split(", state").next().unwrap_or(&s.teredo)) };
                        theme::hero(ui, if ready { "Ready" } else { "Off" }, "Xbox online play", &caption, if ready { theme::ACCENT } else { theme::JUNK });
                        if !s.teredo_state.is_empty() {
                            stat_fit(ui, 30.0, &s.teredo_state, "Teredo state", if ready { theme::TEXT } else { theme::JUNK });
                        }
                        let boost_off = s.edge_boost == "off";
                        stat_fit(
                            ui,
                            22.0,
                            if boost_off { "Off" } else { "On" },
                            if s.edge_boost.starts_with("Windows default") { "Edge startup boost  ·  Windows default" } else { "Edge startup boost" },
                            if boost_off { theme::TEXT } else { theme::JUNK },
                        );
                        stat_fit(
                            ui,
                            22.0,
                            if s.widgets_running { "Running" } else { "Not running" },
                            &if s.widgets_policy == "not set" { "Widgets  ·  policy not set".to_string() } else { format!("Widgets  ·  {}", s.widgets_policy) },
                            if s.widgets_running { theme::JUNK } else { theme::TEXT },
                        );
                    });
                }
            }
        });
        if self.status.as_ref().map(|s| s.widgets_running).unwrap_or(false) {
            theme::banner(ui, theme::OPTIONAL, "Widgets is running and Windows blocks scripts from its policy. Remove Widgets in the Remove Bloat tab (Optional list), or open gpedit.msc > Computer Configuration > Administrative Templates > Windows Components > Widgets > Allow widgets = Disabled, then sign out.");
        }
        ui.add_space(6.0);

        // ---- what it does + the actions
        let running = self.stream.as_ref().map(|s| s.exit.is_none()).unwrap_or(false);
        let can = common::is_admin() && !running;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "What the tweaks do");
            let n_tasks = script_tasks().len();
            let rows: [(&str, String); 4] = [
                (G_TASKS, format!("{n_tasks} telemetry, tips, maps and family-safety tasks are disabled. Xbox and Steam tasks are kept.")),
                (G_EDGE, "Startup Boost and background mode off, so Edge no longer preloads. Edge stays installed.".into()),
                (G_XBOX, "Teredo on as \"enterpriseclient\", set as a Group Policy so Windows keeps it.".into()),
                (G_ONEDRIVE, "Uninstalled only when you choose \"Apply + remove OneDrive\".".into()),
            ];
            egui::Grid::new("tweaks-what").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
                for (name, text) in &rows {
                    ui.label(RichText::new(*name).strong().color(theme::TEXT));
                    ui.label(RichText::new(text).color(theme::TEXT_DIM));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(can, theme::primary_button("   Apply tweaks   ")).clicked() {
                    self.ask(Mode::Apply);
                }
                if ui.add_enabled(can, theme::danger_button("Apply + remove OneDrive")).clicked() {
                    self.ask(Mode::ApplyOneDrive);
                }
                if ui.add_enabled(can, egui::Button::new("Undo tweaks")).clicked() {
                    self.ask(Mode::Undo);
                }
                if ui.add_enabled(!running && self.status.is_some(), egui::Button::new("Refresh status")).clicked() {
                    self.status = None;
                }
            });
        });
        ui.add_space(6.0);

        // ---- last run + raw log
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, &match self.mode {
                    Some(m) => format!("Last run - {}", m.title()),
                    None => "Last run".to_string(),
                });
                if self.stream.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        visible_checkboxes(ui);
                        ui.checkbox(&mut self.show_details, "Show technical details");
                    });
                }
            });
            let Some(s) = &self.stream else {
                ui.label(RichText::new("Nothing has run in this session yet.").color(theme::TEXT_DIM));
                return;
            };
            let parsed = parse_log(&s.lines);
            ui.horizontal_wrapped(|ui| {
                match s.exit {
                    None => {
                        ui.spinner();
                        ui.label(RichText::new("Running...").strong());
                    }
                    Some(0) => theme::chip(ui, "finished", theme::OK),
                    Some(c) => theme::chip(ui, &format!("stopped, exit code {c}"), theme::LOCKED),
                }
                for g in [G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE] {
                    let total = parsed.items.iter().filter(|i| i.group == g).count();
                    let failed = parsed.items.iter().filter(|i| i.group == g && i.result.is_err()).count();
                    if total > 0 {
                        theme::chip(ui, &format!("{g}  {}/{total}", total - failed), if failed == 0 { theme::OK } else { theme::JUNK });
                    }
                }
            });
            if self.show_details {
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("tweaks-log").stick_to_bottom(true).max_height(280.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for l in &s.lines {
                        let c = if l.contains("FAIL") { theme::LOCKED } else { theme::TEXT_DIM };
                        ui.label(RichText::new(l).monospace().color(c));
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tweaks_log_to_rows() {
        // the task list comes straight from the embedded script
        let tasks = script_tasks();
        assert_eq!(tasks.len(), 32);
        assert!(tasks.iter().all(|t| t.starts_with(r"\Microsoft\Windows\")));
        assert_eq!(task_parts(r"\Microsoft\Windows\Maps\MapsToastTask"), ("MapsToastTask".to_string(), "Maps".to_string()));
        assert_eq!(preview_items(Mode::Apply).len(), 32 + 1 + 2 + 1);
        assert_eq!(preview_items(Mode::ApplyOneDrive).last().unwrap().group, G_ONEDRIVE);

        let log: Vec<String> = DEMO_LOG.iter().map(|s| s.to_string()).collect();
        let p = parse_log(&log);
        assert!(p.finished);
        assert_eq!(p.sections, vec![G_TASKS, G_EDGE, G_XBOX]);
        assert_eq!(p.items.iter().filter(|i| i.group == G_TASKS).count(), 10);
        let fail = p.items.iter().find(|i| i.result.is_err()).unwrap();
        assert_eq!((fail.name.as_str(), fail.result.clone().unwrap_err().as_str()), ("UsbCeip", "Access is denied."));
        let skip = p.items.iter().find(|i| i.name == "CleanupOfflineContent").unwrap();
        assert!(skip.result.clone().unwrap().starts_with("Not on this PC"));
        let teredo = p.items.last().unwrap();
        assert_eq!((teredo.group, teredo.detail.as_str()), (G_XBOX, "state qualified, router UPnP Yes"));

        // steps tick off as sections pass; the task step shows a live count
        let mut w = preview_window(Mode::Apply);
        TweaksTab::to_running(&mut w, Mode::Apply);
        let upto = log.iter().position(|l| l.starts_with("Edge")).unwrap();
        update_steps(&mut w, &parse_log(&log[..=upto]), 32);
        assert_eq!(w.items[0].detail, "10 / 32, 1 failed");
        assert!(matches!(w.items[0].result, Some(Ok(_))));
        assert!(w.items[1].result.is_none() && w.items[2].result.is_none());

        // undo output, a not-qualified Teredo, and a script that died
        let undo: Vec<String> = ["Scheduled tasks:", r"  enabled   \Microsoft\Windows\Maps\MapsToastTask", "Edge background launching:", "  Edge policies removed (Edge back to defaults)", "Xbox networking (Teredo):", "  Teredo policy removed and Teredo set back to the Windows default", "Done. Widgets"].iter().map(|s| s.to_string()).collect();
        assert_eq!(result_items(&undo, 0).len(), 3);
        let nq: Vec<String> = ["Xbox networking (Teredo):", "  Teredo: enterpriseclient via x -> state 'probe', router UPnP 'No'", "  Not qualified yet - check again in a minute: netsh interface teredo show state", "  Tip: enable UPnP in your router for Open NAT."].iter().map(|s| s.to_string()).collect();
        let note = parse_log(&nq).items[0].result.clone().unwrap();
        assert!(note.contains("Not qualified yet") && note.contains("UPnP"));
        let dead = result_items(&["Run this script from an elevated (Administrator) PowerShell.".to_string()], 1);
        assert!(matches!(dead[0].result, Some(Err(_))));
    }
}
