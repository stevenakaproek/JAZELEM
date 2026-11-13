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

/// Written by the script before the first Apply changes anything; -Undo puts back what it records.
const STATE_FILE: &str = "tweaks-state.json";
/// The Startup Cleaner's history: tasks it switched off are left off by Undo (the script reads it too).
const STARTUP_HISTORY: &str = "startup-history.json";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Apply,
    ApplyOneDrive,
    Undo,
    /// Apply just one tweak (a group: G_TASKS, G_EDGE, G_XBOX or G_ONEDRIVE).
    Only(&'static str),
}

/// The script's -Only name for a group.
fn only_name(group: &str) -> &'static str {
    match group {
        G_TASKS => "Tasks",
        G_EDGE => "Edge",
        G_XBOX => "Teredo",
        _ => "OneDrive",
    }
}

impl Mode {
    fn args(self, state: &std::path::Path) -> Vec<String> {
        let mut a: Vec<String> = match self {
            Mode::Apply => vec![],
            Mode::ApplyOneDrive => vec!["-RemoveOneDrive".into()],
            Mode::Undo => vec!["-Undo".into()],
            Mode::Only(g) => vec!["-Only".into(), only_name(g).into()],
        };
        a.extend(["-StatePath".to_string(), state.display().to_string()]);
        a
    }
    fn title(self) -> &'static str {
        match self {
            Mode::Apply => "Apply tweaks",
            Mode::ApplyOneDrive => "Apply + remove OneDrive",
            Mode::Undo => "Undo tweaks",
            Mode::Only(G_TASKS) => "Disable the scheduled tasks",
            Mode::Only(G_EDGE) => "Stop Edge preloading",
            Mode::Only(G_XBOX) => "Turn on Teredo",
            Mode::Only(_) => "Remove OneDrive",
        }
    }
    fn groups(self) -> Vec<&'static str> {
        match self {
            Mode::ApplyOneDrive => vec![G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE],
            Mode::Only(g) => vec![g],
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

/// What Apply tweaks saved in tweaks-state.json before it changed anything.
#[derive(Default, Debug, Clone)]
struct Saved {
    when: String,
    /// (task path, state before Apply: "Ready", "Disabled", ...)
    tasks: Vec<(String, String)>,
    values: Vec<SavedValue>,
    /// `netsh interface teredo show state` Type before Apply, e.g. "default" or "client (Group Policy)".
    teredo_type: String,
    /// Not the state before the tweaks (taken after an older PC Tuner applied them, or the file was unreadable):
    /// the script's -Undo resets to the Windows defaults instead.
    inexact: bool,
    /// The file is there but could not be read (the script moves it aside).
    unreadable: bool,
}

#[derive(Debug, Clone)]
struct SavedValue {
    key: String,
    present: bool,
}

/// A JSON array, or a single object PowerShell wrote without the array around it.
fn json_list(v: &serde_json::Value) -> Vec<&serde_json::Value> {
    match v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![v],
        _ => vec![],
    }
}

fn json_str(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn parse_state(json: &str) -> Option<Saved> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    // same rule as the script: an object with a Tasks entry, anything else is not a saved state
    if !v.is_object() || v.get("Tasks").is_none() {
        return None;
    }
    let tasks = json_list(&v["Tasks"]).into_iter().map(|t| (json_str(t, "Path"), json_str(t, "State"))).filter(|t| !t.0.is_empty()).collect();
    let values = json_list(&v["Values"])
        .into_iter()
        .map(|x| SavedValue { key: json_str(x, "Key"), present: x["Present"].as_bool().unwrap_or(false) })
        .collect();
    let inexact = v["Inexact"].as_bool().unwrap_or(false);
    Some(Saved { when: json_str(&v, "Saved"), tasks, values, teredo_type: json_str(&v["Teredo"], "Type"), inexact, unreadable: false })
}

/// The saved state as -Undo will use it: None when there is no file, an inexact record when it cannot be read.
fn saved_from_file(text: Option<String>) -> Option<Saved> {
    let text = text?;
    Some(parse_state(&text).unwrap_or(Saved { inexact: true, unreadable: true, ..Default::default() }))
}

/// Full paths (lower case) of the tasks the Startup Cleaner's history says it switched off - same rule as the script.
fn startup_kept_off(json: &str) -> std::collections::HashSet<String> {
    let mut state: std::collections::HashMap<String, bool> = Default::default();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json.trim_start_matches('\u{feff}')) {
        for h in json_list(&v) {
            let name = json_str(&h["Loc"], "TaskName");
            if json_str(h, "Kind") != "Task" || name.is_empty() {
                continue;
            }
            let full = format!("{}\\{}", json_str(&h["Loc"], "TaskPath").trim_end_matches('\\'), name).to_lowercase();
            let b = |k: &str| h[k].as_bool().unwrap_or(false);
            let on = if b("Undone") { b("From") } else { b("To") };
            state.insert(full, !on);
        }
    }
    state.into_iter().filter(|(_, off)| *off).map(|(p, _)| p).collect()
}

fn load_saved() -> Option<Saved> {
    let path = common::data_dir().join(STATE_FILE);
    // a file that is there but not valid UTF-8 counts as unreadable, not missing
    saved_from_file(if path.exists() { Some(std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()) } else { None })
}

fn load_kept_off() -> std::collections::HashSet<String> {
    std::fs::read_to_string(common::data_dir().join(STARTUP_HISTORY)).map(|s| startup_kept_off(&s)).unwrap_or_default()
}

/// The Teredo type the script's -Undo puts back with netsh, or None when it leaves netsh alone
/// (a Teredo policy was there before, or the saved type came from a policy).
fn undo_teredo_type(s: &Saved) -> Option<String> {
    let had_policy = s.values.iter().any(|v| v.key.ends_with("v6Transition") && v.present);
    if had_policy || s.teredo_type.is_empty() || s.teredo_type.contains("Group Policy") {
        return None;
    }
    let t = s.teredo_type.split(" (").next().unwrap_or("").trim().to_string();
    Some(if ["disabled", "client", "enterpriseclient", "natawareclient", "server", "default"].contains(&t.as_str()) { t } else { "default".into() })
}

/// Saved tasks Undo leaves off: (were off before Apply, switched off by the Startup Cleaner).
fn undo_left_off(s: &Saved, kept_off: &std::collections::HashSet<String>) -> (usize, usize) {
    let cleaner = s.tasks.iter().filter(|(p, _)| kept_off.contains(&p.to_lowercase())).count();
    let before = s.tasks.iter().filter(|(p, st)| st == "Disabled" && !kept_off.contains(&p.to_lowercase())).count();
    (before, cleaner)
}

/// The items shown in the preview: one per thing the script will touch.
/// For Undo with a saved state, only what will actually change is listed.
fn preview_items(mode: Mode, saved: Option<&Saved>, kept_off: &std::collections::HashSet<String>) -> Vec<OpItem> {
    let undo = mode == Mode::Undo;
    let task_item = |p: &String| {
        let (name, folder) = task_parts(p);
        OpItem::new(p.clone(), name, G_TASKS).detail(folder)
    };
    // an inexact record is undone like no record at all (reset to the Windows defaults)
    if let (true, Some(s)) = (undo, saved.filter(|s| !s.inexact)) {
        let mut v: Vec<OpItem> = s.tasks.iter().filter(|(p, st)| st != "Disabled" && !kept_off.contains(&p.to_lowercase())).map(|(p, _)| task_item(p)).collect();
        let edge_set = s.values.iter().any(|x| x.key.ends_with("Edge") && x.present);
        v.push(OpItem::new("edge", "Edge policies", G_EDGE).detail(if edge_set { "put back to the values saved before the tweaks" } else { "removed - they were not set before" }));
        let teredo = match undo_teredo_type(s) {
            Some(t) => format!("policy removed, Teredo type back to {t}"),
            None if s.values.iter().any(|x| x.key.ends_with("v6Transition") && x.present) => "policy put back as it was before".into(),
            None => "policy removed - it was not set before".into(),
        };
        v.push(OpItem::new("teredo", "Teredo", G_XBOX).detail(teredo));
        return v;
    }
    let mut v: Vec<OpItem> = script_tasks().iter().filter(|p| !(undo && kept_off.contains(&p.to_lowercase()))).map(task_item).collect();
    v.push(OpItem::new("softlanding", "Windows tips tasks", G_TASKS).detail("SoftLanding"));
    if undo {
        v.push(OpItem::new("edge", "Edge policies", G_EDGE).detail("removed - Edge back to its defaults"));
        v.push(OpItem::new("teredo", "Teredo policy", G_XBOX).detail("removed - Teredo back to the Windows default"));
    } else {
        v.push(OpItem::new("edge-boost", "Startup Boost", G_EDGE).detail("off - Edge stays installed"));
        v.push(OpItem::new("edge-bg", "Background mode", G_EDGE).detail("off"));
        v.push(OpItem::new("teredo", "Teredo", G_XBOX).detail("enterpriseclient, kept on by Group Policy"));
    }
    if matches!(mode, Mode::ApplyOneDrive | Mode::Only(_)) {
        v.push(OpItem::new("onedrive", "OneDrive", G_ONEDRIVE).detail("uninstalled, its tasks disabled"));
    }
    if let Mode::Only(g) = mode {
        v.retain(|i| i.group == g);
    }
    v
}

fn preview_window(mode: Mode) -> OpWindow {
    if mode == Mode::Undo {
        preview_window_with(mode, load_saved().as_ref(), &load_kept_off())
    } else {
        preview_window_with(mode, None, &Default::default())
    }
}

fn preview_window_with(mode: Mode, saved: Option<&Saved>, kept_off: &std::collections::HashSet<String>) -> OpWindow {
    let (verb, done_word) = if mode == Mode::Undo { ("undo", "undone") } else { ("apply", "applied") };
    let mut w = OpWindow::new("POST-INSTALL TWEAKS", verb, done_word, preview_items(mode, saved, kept_off)).noun("tweak", "tweaks");
    w.go_label = mode.title().into();
    match mode {
        Mode::Apply => {
            w.preview_text = "Telemetry and tips tasks off, Edge no longer preloads, Teredo on for Xbox online play.".into();
            w.reassure = "Xbox and Steam tasks are kept. The state before is saved first, so \"Undo tweaks\" puts back exactly what was there.".into();
        }
        Mode::ApplyOneDrive => {
            w.preview_text = "The same tweaks, and OneDrive is uninstalled.".into();
            w.warning = Some("OneDrive will be uninstalled. \"Undo tweaks\" does not bring it back - reinstall it from the Microsoft Store if you want it again.".into());
            w.reassure = "Xbox and Steam tasks are kept. The other tweaks can be put back exactly with \"Undo tweaks\".".into();
        }
        Mode::Only(g) => {
            w.preview_text = match g {
                G_TASKS => "Only the telemetry and tips tasks are switched off. Edge, Teredo and OneDrive are left alone.",
                G_EDGE => "Only Edge's Startup Boost and background mode are switched off. The other tweaks are left alone.",
                G_XBOX => "Only Teredo is turned on for Xbox online play. The other tweaks are left alone.",
                _ => "Only OneDrive is uninstalled. The other tweaks are left alone.",
            }
            .into();
            if g == G_ONEDRIVE {
                w.warning = Some("OneDrive will be uninstalled. \"Undo tweaks\" does not bring it back - reinstall it from the Microsoft Store if you want it again.".into());
                w.reassure = "Nothing else is changed.".into();
            } else {
                w.reassure = "The state before is saved first. \"Undo tweaks\" puts back every tweak applied so far, not just this one.".into();
            }
        }
        Mode::Undo => match saved {
            Some(s) if !s.inexact => {
                let (before, cleaner) = undo_left_off(s, kept_off);
                let when = if s.when.is_empty() { String::new() } else { format!(" on {}", s.when) };
                w.preview_text = format!("Puts back the state saved before the tweaks{when}: tasks that were on are switched back on, the Edge and Teredo policies go back to what they were.");
                let mut left = vec![];
                let tasks = |n: usize| if n == 1 { ("task", "stays") } else { ("tasks", "stay") };
                if before > 0 {
                    let (t, stay) = tasks(before);
                    left.push(format!("{before} {t} already off before the tweaks {stay} off"));
                }
                if cleaner > 0 {
                    let (t, stay) = tasks(cleaner);
                    left.push(format!("{cleaner} {t} switched off by the Startup Cleaner {stay} off"));
                }
                if !left.is_empty() {
                    w.reassure = format!("{}.", left.join(", "));
                }
                if let Some(t) = undo_teredo_type(s) {
                    w.warning = Some(format!("Teredo goes back to how it was before the tweaks (type {t}). Xbox party chat and multiplayer may stop connecting until you apply the tweaks again."));
                }
            }
            _ => {
                w.preview_text = match saved {
                    Some(s) if s.unreadable => "The saved state from \"Apply tweaks\" could not be read, so exact undo is unavailable and Undo resets to the Windows defaults. The file is moved aside, not deleted.".into(),
                    Some(_) => "The saved state was taken after an older PC Tuner had already applied the tweaks, so exact undo is unavailable and Undo resets to the Windows defaults.".into(),
                    None => "No saved state from \"Apply tweaks\" was found (they were applied by an older PC Tuner, or not at all), so Undo resets to the Windows defaults.".into(),
                };
                w.warning = Some("Every listed task is switched on, including ones that were already off before, and the Edge and Teredo policies are removed. Tasks the Startup Cleaner switched off are left off. Teredo goes back to the Windows default - Xbox party chat and multiplayer may stop connecting until you apply the tweaks again.".into());
            }
        },
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
        // "FAIL      Edge policies : Access is denied." in the Edge / Xbox / OneDrive sections
        if let (Some(g), Some(rest)) = (cur, t.strip_prefix("FAIL")) {
            if g != G_TASKS {
                let rest = rest.trim();
                let (name, why) = rest.split_once(" : ").unwrap_or((rest, "failed"));
                p.items.push(LogItem { group: g, name: name.trim().to_string(), detail: String::new(), result: Err(why.trim().to_string()) });
                continue;
            }
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
                    // "kept      <path> (was off before the tweaks - left off)"
                    "kept" => {
                        let (path, why) = rest.rsplit_once(" (").map(|(a, b)| (a, b.trim_end_matches(')'))).unwrap_or((rest, "left as it was"));
                        let mut note: String = why.chars().take(1).flat_map(char::to_uppercase).chain(why.chars().skip(1)).collect();
                        note.push('.');
                        push(path.trim(), "", Ok(note));
                    }
                    "NOTE" => {
                        let detail = if rest.contains("could not be read") {
                            "unreadable"
                        } else if rest.contains("older PC Tuner") {
                            "inexact"
                        } else {
                            "not found"
                        };
                        p.items.push(LogItem { group: G_TASKS, name: "Saved state".into(), detail: detail.into(), result: Ok(rest.to_string()) })
                    }
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
                } else if t.starts_with("Edge policies put back") {
                    p.items.push(LogItem { group: G_EDGE, name: "Edge policies".into(), detail: "as before the tweaks".into(), result: Ok(String::new()) });
                }
            }
            Some(G_XBOX) => {
                if let Some(rest) = t.strip_prefix("Teredo: ") {
                    let detail = rest.split_once("-> ").map(|x| x.1).unwrap_or(rest).replace('\'', "");
                    p.items.push(LogItem { group: G_XBOX, name: "Teredo".into(), detail, result: Ok(String::new()) });
                } else if t.starts_with("Teredo policy removed") {
                    p.items.push(LogItem { group: G_XBOX, name: "Teredo policy".into(), detail: "removed, Windows default".into(), result: Ok(String::new()) });
                } else if let Some(rest) = t.strip_prefix("Teredo put back as it was before the tweaks") {
                    let how = rest.trim().trim_start_matches('(').trim_end_matches(')');
                    let detail = if how.is_empty() { "as before the tweaks".to_string() } else { format!("as before the tweaks, {how}") };
                    p.items.push(LogItem { group: G_XBOX, name: "Teredo".into(), detail, result: Ok(String::new()) });
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
                } else if t.starts_with("OneDrive removed") {
                    p.items.push(LogItem { group: G_ONEDRIVE, name: "OneDrive".into(), detail: "uninstalled".into(), result: Ok("Reinstall any time from the Microsoft Store.".into()) });
                } else if t.starts_with("OneDrive not installed") {
                    p.items.push(LogItem { group: G_ONEDRIVE, name: "OneDrive".into(), detail: "not installed".into(), result: Ok("Nothing to do.".into()) });
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
    let (errors, first_error) = ps_errors(lines);
    if exit != 0 || items.is_empty() {
        let last = lines.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("no output");
        let mut it = OpItem::new("script", "PostInstall-Tweaks.ps1", "Script");
        it.result = Some(Err(format!("The script stopped early (exit code {exit}): {}", first_error.as_deref().unwrap_or(last))));
        items.push(it);
    } else if errors > 0 {
        // PowerShell errors the script did not turn into a FAIL row - never hide them behind green ticks
        let mut it = OpItem::new("pserrors", "Other errors", "Script");
        let first = first_error.map(|e| format!(" First: {e}")).unwrap_or_default();
        it.result = Some(Err(format!("PowerShell reported {errors} more error{} - see the technical details.{first}", if errors == 1 { "" } else { "s" })));
        items.push(it);
    }
    items
}

/// PowerShell error records in the output (each ends with "+ FullyQualifiedErrorId ...") and the first one's message
/// (the line above its "At <script>.ps1:<line> char:<n>").
fn ps_errors(lines: &[String]) -> (usize, Option<String>) {
    let count = lines.iter().filter(|l| l.trim_start().starts_with("+ FullyQualifiedErrorId")).count();
    let first = lines
        .iter()
        .position(|l| l.trim_start().starts_with("At ") && l.contains(".ps1:"))
        .filter(|i| *i > 0)
        .map(|i| lines[i - 1].trim().to_string())
        .filter(|m| !m.is_empty());
    (count, first)
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
        self.stream = Some(Stream::run_ps_file(&path, &mode.args(&common::data_dir().join(STATE_FILE))));
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
            let rows: [(&'static str, String); 4] = [
                (G_TASKS, format!("{n_tasks} telemetry, tips, maps and family-safety tasks are disabled. Xbox and Steam tasks are kept.")),
                (G_EDGE, "Startup Boost and background mode off, so Edge no longer preloads. Edge stays installed.".into()),
                (G_XBOX, "Teredo on as \"enterpriseclient\", set as a Group Policy so Windows keeps it.".into()),
                (G_ONEDRIVE, "Uninstalled only when you choose \"Apply + remove OneDrive\" or \"Remove\" here.".into()),
            ];
            egui::Grid::new("tweaks-what").num_columns(3).spacing([18.0, 8.0]).show(ui, |ui| {
                for (name, text) in &rows {
                    ui.label(RichText::new(*name).strong().color(theme::TEXT));
                    ui.label(RichText::new(text).color(theme::TEXT_DIM));
                    // apply just this one tweak
                    let button = if *name == G_ONEDRIVE { theme::danger_button("Remove") } else { egui::Button::new("Apply") };
                    if ui.add_enabled(can, button).on_hover_text(Mode::Only(name).title()).clicked() {
                        self.ask(Mode::Only(name));
                    }
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
        let none = Default::default();
        assert_eq!(preview_items(Mode::Apply, None, &none).len(), 32 + 1 + 2 + 1);
        assert_eq!(preview_items(Mode::ApplyOneDrive, None, &none).last().unwrap().group, G_ONEDRIVE);

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

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tweaks_failures_are_red() {
        // FAIL rows from every section, and the new "kept" rows
        let log = lines(&[
            "Scheduled tasks:",
            r"  FAIL      \SoftLanding\S-1-5-21\SoftLandingTask : Access is denied.",
            r"  kept      \Microsoft\Windows\Maps\MapsToastTask (already off - nothing to do)",
            "Edge background launching:",
            "  FAIL      Edge policies : Requested registry access is not allowed.",
            "Xbox networking (Teredo):",
            "  FAIL      Teredo (netsh) : netsh exit code 1 - The requested operation requires elevation.",
            "  Teredo: enterpriseclient via x -> state 'qualified', router UPnP 'Yes'",
            "Done.",
        ]);
        let p = parse_log(&log);
        let r: Vec<(&str, &str, bool)> = p.items.iter().map(|i| (i.group, i.name.as_str(), i.result.is_ok())).collect();
        assert_eq!(
            r,
            vec![(G_TASKS, "SoftLandingTask", false), (G_TASKS, "MapsToastTask", true), (G_EDGE, "Edge policies", false), (G_XBOX, "Teredo (netsh)", false), (G_XBOX, "Teredo", true)]
        );
        assert_eq!(p.items[1].result.clone().unwrap(), "Already off - nothing to do.");
        assert_eq!(p.items[2].result.clone().unwrap_err(), "Requested registry access is not allowed.");

        // errors PowerShell printed that no FAIL line covers still show up
        let stray = lines(&[
            "Scheduled tasks:",
            r"  disabled  \Microsoft\Windows\Maps\MapsToastTask",
            "Disable-ScheduledTask : Access is denied.",
            r"At C:\x\PostInstall-Tweaks.ps1:71 char:9",
            "    + FullyQualifiedErrorId : HRESULT 0x80070005,Disable-ScheduledTask",
            "Done.",
        ]);
        let items = result_items(&stray, 0);
        let last = items.last().unwrap();
        assert_eq!(last.name, "Other errors");
        assert!(last.result.clone().unwrap().unwrap_err().contains("Access is denied."));
        assert_eq!(result_items(&stray[..2], 0).len(), 1);
    }

    #[test]
    fn tweaks_apply_one_at_a_time() {
        let none = Default::default();
        // each tweak previews only its own items and runs only its own step
        let groups = |m: Mode| preview_items(m, None, &none).iter().map(|i| i.group.clone()).collect::<std::collections::BTreeSet<_>>();
        for g in [G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE] {
            assert_eq!(groups(Mode::Only(g)).into_iter().collect::<Vec<_>>(), vec![g.to_string()]);
            assert_eq!(Mode::Only(g).groups(), vec![g]);
        }
        assert_eq!(preview_items(Mode::Only(G_TASKS), None, &none).len(), 32 + 1);
        assert_eq!(preview_items(Mode::Only(G_EDGE), None, &none).len(), 2);
        assert!(preview_window_with(Mode::Only(G_ONEDRIVE), None, &none).warning.is_some());
        assert!(preview_window_with(Mode::Only(G_EDGE), None, &none).warning.is_none());

        // the app passes -Only with a name the script accepts
        let a = Mode::Only(G_XBOX).args(std::path::Path::new(r"C:\x\s.json"));
        assert_eq!(&a[..2], &["-Only".to_string(), "Teredo".into()]);
        for g in [G_TASKS, G_EDGE, G_XBOX, G_ONEDRIVE] {
            assert!(SCRIPT.contains(&format!("Test-Do '{}'", only_name(g))) || only_name(g) == "OneDrive");
        }
        assert!(SCRIPT.contains("[ValidateSet('Tasks', 'Edge', 'Teredo', 'OneDrive')]"));

        // a run of just one section still parses
        let edge = lines(&["Edge background launching:", "  Startup Boost and background mode turned off (Edge shows \"Managed by your organization\" - expected)", "Done."]);
        let r = result_items(&edge, 0);
        assert_eq!((r.len(), r[0].group.as_str()), (1, G_EDGE));
    }

    #[test]
    fn tweaks_onedrive_outcomes() {
        let one = |l: &str| {
            let p = parse_log(&lines(&["OneDrive:", l]));
            assert_eq!(p.items.len(), 1, "{l}");
            let i = &p.items[0];
            (i.group, i.detail.clone(), i.result.clone())
        };
        assert_eq!(one("  OneDrive removed (reinstall any time from the Microsoft Store or winget)."), (G_ONEDRIVE, "uninstalled".into(), Ok("Reinstall any time from the Microsoft Store.".into())));
        assert_eq!(one("  OneDrive not installed - nothing to do."), (G_ONEDRIVE, "not installed".into(), Ok("Nothing to do.".into())));
        assert_eq!(one("  FAIL      OneDrive : the uninstaller stopped with exit code 5"), (G_ONEDRIVE, String::new(), Err("the uninstaller stopped with exit code 5".into())));
        // the script prints exactly these lines
        assert!(SCRIPT.contains("'  OneDrive not installed - nothing to do.'") && SCRIPT.contains("Write-Fail 'OneDrive' "));
    }

    #[test]
    fn tweaks_undo_uses_saved_state() {
        // what the script writes (PowerShell's ConvertTo-Json, BOM included)
        let state = "\u{feff}{\"Version\":1,\"Saved\":\"2026-11-12 10:00:00\",\
            \"Tasks\":[{\"Path\":\"\\\\Microsoft\\\\Windows\\\\Maps\\\\MapsToastTask\",\"State\":\"Ready\"},\
                       {\"Path\":\"\\\\Microsoft\\\\Windows\\\\Maps\\\\MapsUpdateTask\",\"State\":\"Disabled\"},\
                       {\"Path\":\"\\\\Microsoft\\\\Windows\\\\Maintenance\\\\WinSAT\",\"State\":\"Ready\"},\
                       {\"Path\":\"\\\\Microsoft\\\\Windows\\\\Autochk\\\\Proxy\",\"State\":\"Ready\"}],\
            \"Values\":[{\"Key\":\"HKLM:\\\\SOFTWARE\\\\Policies\\\\Microsoft\\\\Edge\",\"Name\":\"StartupBoostEnabled\",\"Present\":false,\"Kind\":null,\"Data\":null},\
                        {\"Key\":\"HKLM:\\\\SOFTWARE\\\\Policies\\\\Microsoft\\\\Windows\\\\TCPIP\\\\v6Transition\",\"Name\":\"Teredo_State\",\"Present\":false,\"Kind\":null,\"Data\":null}],\
            \"PolicyKeys\":[],\"Teredo\":{\"Type\":\"client\",\"Server\":\"win1910.ipv6.microsoft.com\"}}";
        let s = parse_state(state).unwrap();
        assert_eq!((s.tasks.len(), s.when.as_str(), s.teredo_type.as_str()), (4, "2026-11-12 10:00:00", "client"));
        assert_eq!(undo_teredo_type(&s).as_deref(), Some("client"));

        // the Startup Cleaner switched WinSAT off (and undid its MapsToastTask change)
        let hist = r#"[{"Batch":1,"Kind":"Task","Loc":{"TaskPath":"\\Microsoft\\Windows\\Maintenance\\","TaskName":"WinSAT"},"From":true,"To":false},
                       {"Batch":2,"Kind":"Task","Loc":{"TaskPath":"\\Microsoft\\Windows\\Maps\\","TaskName":"MapsToastTask"},"From":true,"To":false,"Undone":true},
                       {"Batch":2,"Kind":"Run","Name":"x","From":true,"To":false}]"#;
        let kept = startup_kept_off(hist);
        assert_eq!(kept.into_iter().collect::<Vec<_>>(), vec![r"\microsoft\windows\maintenance\winsat".to_string()]);
        let kept = startup_kept_off(hist);

        // only what changes is listed: tasks that were on before (not WinSAT), then Edge and Teredo
        let items = preview_items(Mode::Undo, Some(&s), &kept);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["MapsToastTask", "Proxy", "Edge policies", "Teredo"]);
        assert_eq!(items[3].detail, "policy removed, Teredo type back to client");
        assert_eq!(undo_left_off(&s, &kept), (1, 1));
        let w = preview_window_with(Mode::Undo, Some(&s), &kept);
        assert_eq!(w.reassure, "1 task already off before the tweaks stays off, 1 task switched off by the Startup Cleaner stays off.");
        assert!(w.warning.unwrap().contains("type client"));

        // a Teredo policy that existed before is put back, netsh is left alone
        let mut gp = s.clone();
        gp.values[1].present = true;
        assert_eq!(undo_teredo_type(&gp), None);
        assert!(preview_window_with(Mode::Undo, Some(&gp), &kept).warning.is_none());

        // no saved state: the old reset-to-defaults undo, said plainly, Startup Cleaner tasks left out
        let w = preview_window_with(Mode::Undo, None, &kept);
        assert!(w.preview_text.contains("No saved state") && w.warning.unwrap().contains("already off"));
        assert_eq!(w.items.iter().filter(|i| i.group == G_TASKS).count(), 32 - 1 + 1);

        // the script's undo output
        let undo = lines(&[
            "Scheduled tasks:",
            r"  enabled   \Microsoft\Windows\Maps\MapsToastTask",
            r"  kept      \Microsoft\Windows\Maps\MapsUpdateTask (was off before the tweaks - left off)",
            r"  kept      \Microsoft\Windows\Maintenance\WinSAT (switched off by the Startup Cleaner - left off)",
            "Edge background launching:",
            "  Edge policies put back as they were before the tweaks",
            "Xbox networking (Teredo):",
            "  Teredo put back as it was before the tweaks (type client)",
            "Done.",
        ]);
        let r = result_items(&undo, 0);
        assert!(r.iter().all(|i| matches!(i.result, Some(Ok(_)))));
        assert_eq!(r[2].result.clone().unwrap().unwrap(), "Switched off by the Startup Cleaner - left off.");
        assert_eq!((r[3].detail.as_str(), r[4].detail.as_str()), ("as before the tweaks", "as before the tweaks, type client"));

        // a record taken after an older PC Tuner had applied the tweaks: reset-to-defaults undo, said plainly
        let old = state.replacen("\"Version\":1,", "\"Version\":1,\"Inexact\":true,", 1);
        let si = parse_state(&old).unwrap();
        assert!(si.inexact && !si.unreadable);
        let w = preview_window_with(Mode::Undo, Some(&si), &kept);
        assert!(w.preview_text.contains("exact undo is unavailable") && w.warning.unwrap().contains("already off"));
        assert_eq!(w.items.iter().filter(|i| i.group == G_TASKS).count(), 32 - 1 + 1);
        assert!(!s.inexact);

        // missing file vs unreadable file
        assert!(saved_from_file(None).is_none());
        for bad in ["{\"Version\":1,", "", "null", "[1,2]", "[{\"Version\":1,\"Tasks\":[]}]", "7", "\"x\"", "{\"Version\":1}"] {
            let u = saved_from_file(Some(bad.into())).unwrap();
            assert!(u.inexact && u.unreadable, "{bad}");
            let w = preview_window_with(Mode::Undo, Some(&u), &kept);
            assert!(w.preview_text.contains("could not be read"));
            assert_eq!(w.items.iter().filter(|i| i.group == G_TASKS).count(), 32 - 1 + 1);
        }
        assert!(!saved_from_file(Some(state.into())).unwrap().inexact);

        // the script's notes for these cases
        let notes = lines(&[
            "Scheduled tasks:",
            "  NOTE      The saved state from Apply tweaks could not be read (moved to tweaks-state.corrupt-20261112-100000.json) - exact undo is unavailable. Every task is switched back on and the policies are reset to the Windows defaults.",
            "  NOTE      The saved state was taken after an older PC Tuner had already applied the tweaks - exact undo is unavailable. Every task is switched back on and the policies are reset to the Windows defaults.",
            "  NOTE      No saved state from Apply tweaks - every task is switched back on and the policies are reset to the Windows defaults.",
            "Done.",
        ]);
        let d: Vec<String> = result_items(&notes, 0).into_iter().map(|i| i.detail).collect();
        assert_eq!(d, vec!["unreadable", "inexact", "not found"]);
        assert!(SCRIPT.contains("could not be read ($where) - exact undo is unavailable.") && SCRIPT.contains("had already applied the tweaks - exact undo is unavailable."));
        assert!(SCRIPT.contains("Inexact    = $inexact") && SCRIPT.contains("$State.Inexact"));

        // the app passes the state file to the script, which declares the parameter
        let a = Mode::Undo.args(std::path::Path::new(r"C:\x\tweaks-state.json"));
        assert_eq!(a, vec!["-Undo".to_string(), "-StatePath".into(), r"C:\x\tweaks-state.json".into()]);
        assert!(SCRIPT.contains("[string]$StatePath"));
    }
}
