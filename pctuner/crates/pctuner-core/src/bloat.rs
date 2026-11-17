//! Remove Bloat: runs Remove-Bloat.ps1 (embedded) in depth for the apps the user ticked.
//! Revo-style leftover cleanup with quarantine, re-check task, full restore.
//!
//! Logic only: the target list, the installed-apps scan, the script run, the log summary and the
//! contents of the remove pop-up. The UI draws `Bloat` and forwards the pop-up's events to `on_event`.

use crate::common::{self, Stream};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};

pub const SCRIPT: &str = crate::scripts::REMOVE_BLOAT;

/// The script's `Kind` for a target: appx -> App, capability -> Capability, system -> System.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    App,
    Capability,
    System,
}

pub struct Target {
    pub name: &'static str,
    pub kind: Kind,
    pub optional: bool,
    pub loses: &'static str,
}

pub const TARGETS: &[Target] = &[
    Target { name: "Microsoft.MicrosoftOfficeHub", kind: Kind::App, optional: false, loses: "Copilot app (desktop + identity) and the Copilot key action" },
    Target { name: "Clipchamp.Clipchamp", kind: Kind::App, optional: false, loses: "Clipchamp video editor" },
    Target { name: "Microsoft.BingNews", kind: Kind::App, optional: false, loses: "Microsoft News" },
    Target { name: "Microsoft.BingWeather", kind: Kind::App, optional: false, loses: "Weather" },
    Target { name: "Microsoft.BingSearch", kind: Kind::App, optional: false, loses: "Bing web results in Start search" },
    Target { name: "Microsoft.MicrosoftSolitaireCollection", kind: Kind::App, optional: false, loses: "Solitaire" },
    Target { name: "Microsoft.PowerAutomateDesktop", kind: Kind::App, optional: false, loses: "Power Automate" },
    Target { name: "Microsoft.Windows.DevHome", kind: Kind::App, optional: false, loses: "nothing (retired stub)" },
    Target { name: "Microsoft.WindowsCamera", kind: Kind::App, optional: false, loses: "Camera app" },
    Target { name: "Microsoft.WindowsFeedbackHub", kind: Kind::App, optional: false, loses: "Feedback Hub" },
    Target { name: "Microsoft.StartExperiencesApp", kind: Kind::App, optional: false, loses: "MSN widgets" },
    Target { name: "Microsoft.Ink.Handwriting.Main.en-US.1.0.1", kind: Kind::App, optional: false, loses: "pen handwriting (removed with its language data)" },
    Target { name: "App.StepsRecorder~", kind: Kind::Capability, optional: false, loses: "Steps Recorder" },
    Target { name: "Hello.Face.", kind: Kind::Capability, optional: false, loses: "Windows Hello face (PIN unaffected)" },
    Target { name: "Print.Management.Console~", kind: Kind::Capability, optional: false, loses: "Print Management console" },
    Target { name: "Language.Handwriting~", kind: Kind::Capability, optional: false, loses: "pen handwriting language data" },
    Target { name: "Recall", kind: Kind::System, optional: false, loses: "nothing (Recall off by policy)" },
    Target { name: "OneDrive leftovers", kind: Kind::System, optional: false, loses: "nothing (stops OneDrive returning)" },
    Target { name: "MicrosoftWindows.Client.WebExperience", kind: Kind::App, optional: true, loses: "Widgets board" },
    Target { name: "Microsoft.WidgetsPlatformRuntime", kind: Kind::App, optional: true, loses: "third-party widgets" },
    Target { name: "MSTeams", kind: Kind::App, optional: true, loses: "Teams + Office meeting add-in" },
    Target { name: "Microsoft.OutlookForWindows", kind: Kind::App, optional: true, loses: "new Outlook" },
    Target { name: "Microsoft.YourPhone", kind: Kind::App, optional: true, loses: "Phone Link" },
    Target { name: "MicrosoftWindows.CrossDevice", kind: Kind::App, optional: true, loses: "Mobile devices page" },
    Target { name: "Microsoft.Todos", kind: Kind::App, optional: true, loses: "To Do" },
    Target { name: "Microsoft.MicrosoftStickyNotes", kind: Kind::App, optional: true, loses: "Sticky Notes" },
    Target { name: "Microsoft.WindowsAlarms", kind: Kind::App, optional: true, loses: "Clock / timers" },
    Target { name: "Microsoft.WindowsSoundRecorder", kind: Kind::App, optional: true, loses: "Sound Recorder" },
    Target { name: "MicrosoftCorporationII.QuickAssist", kind: Kind::App, optional: true, loses: "Quick Assist" },
    Target { name: "Microsoft.GetHelp", kind: Kind::App, optional: true, loses: "Get Help + Settings troubleshooters" },
    Target { name: "Microsoft.XboxSpeechToTextOverlay", kind: Kind::App, optional: true, loses: "Game Bar speech-to-text overlay" },
    Target { name: "Media.WindowsMediaPlayer~", kind: Kind::Capability, optional: true, loses: "Windows Media Player Legacy" },
    Target { name: "Browser.InternetExplorer~", kind: Kind::Capability, optional: true, loses: "old IE engine" },
    Target { name: "MathRecognizer~", kind: Kind::Capability, optional: true, loses: "ink math input" },
    Target { name: "Microsoft.Windows.PowerShell.ISE~", kind: Kind::Capability, optional: true, loses: "PowerShell ISE" },
    Target { name: "Language.Speech~", kind: Kind::Capability, optional: true, loses: "voice typing / speech" },
    Target { name: "Language.OCR~", kind: Kind::Capability, optional: true, loses: "copy text from images" },
];

/// Rows that are ticked and unticked together.
pub const PAIRED: &[&[&str]] = &[&["Microsoft.Ink.Handwriting.Main.en-US.1.0.1", "Language.Handwriting~"]];

// ------------------------------------------------------------------ friendly result summary

#[derive(Clone, PartialEq, Debug)]
pub enum Res {
    Working,
    Removed,
    WouldRemove,
    AlreadyGone,
    Kept,
    Applied,
    /// A system item that had nothing to change (already in place, or a preview with nothing to do).
    AlreadySet,
    Restored,
    Failed(String),
}

pub struct ResultItem {
    pub name: String,
    pub res: Res,
}

/// Index in TARGETS of the target a package / capability name belongs to.
pub fn target_of(name: &str) -> Option<usize> {
    TARGETS.iter().position(|t| {
        let base = t.name.trim_end_matches(['~', '.']);
        name.eq_ignore_ascii_case(t.name) || name.to_ascii_lowercase().starts_with(&base.to_ascii_lowercase())
    })
}

/// Plain-English label for a package / capability name, using the target list.
pub fn friendly(name: &str) -> String {
    match target_of(name).map(|i| &TARGETS[i]) {
        // items whose description is "nothing ..." get a proper name instead
        Some(t) if t.name == "Recall" => "Recall (AI screen snapshots)".into(),
        Some(t) if t.name == "OneDrive leftovers" => "OneDrive leftovers".into(),
        Some(t) if t.name == "Microsoft.Windows.DevHome" => "Dev Home (retired)".into(),
        Some(t) => {
            let mut s = t.loses.to_string();
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            s
        }
        // not in the list (e.g. a protected app): drop the "Microsoft." prefix and version suffix
        None => name.trim_start_matches("Microsoft.").split("~~~").next().unwrap_or(name).to_string(),
    }
}

/// Turn the script's log into one line per app: removed / already gone / kept / failed.
/// `preview`: the run was a -WhatIf preview, so nothing in it can be "done".
pub fn summarize(lines: &[String], preview: bool) -> (Vec<ResultItem>, Vec<String>) {
    let mut items: Vec<ResultItem> = vec![];
    let mut notes: Vec<String> = vec![];
    let after = |s: &str, p: &str| s[p.len()..].trim().to_string();
    let name_part = |s: &str| s.split("  -  ").next().unwrap_or(s).trim().to_string();
    let restoring = lines.iter().any(|x| ["Reinstalling ", "Re-adding ", "Re-enabling ", "Re-registering "].iter().any(|p| x.contains(p)));
    for raw in lines {
        // strip the "HH:MM:SS  " timestamp
        let l = if raw.len() > 10 && raw.as_bytes()[2] == b':' && raw.as_bytes()[5] == b':' { &raw[10..] } else { raw.as_str() };
        let t = l.trim();
        let last = items.last_mut();
        if let Some(rest) = l.strip_prefix("Removing capability ") {
            items.push(ResultItem { name: name_part(rest), res: Res::Working });
        } else if let Some(rest) = l.strip_prefix("Removing ") {
            items.push(ResultItem { name: name_part(rest), res: Res::Working });
        } else if let Some(rest) = l.strip_prefix("Applying: ") {
            // in a preview it stays "nothing to change" unless a "would ..." line follows
            items.push(ResultItem { name: name_part(rest), res: if preview { Res::AlreadySet } else { Res::Applied } });
        } else if t == "already set" {
            if let Some(i) = last { if i.res == Res::Applied || i.res == Res::AlreadySet { i.res = Res::AlreadySet; } }
        } else if let Some(rest) = l.strip_prefix("Re-enabling ").or_else(|| l.strip_prefix("Re-registering ")) {
            items.push(ResultItem { name: rest.trim().trim_end_matches("...").to_string(), res: Res::Working });
        } else if let Some(rest) = l.strip_prefix("Reinstalling ") {
            items.push(ResultItem { name: rest.split(" from the Store").next().unwrap_or(rest).trim().to_string(), res: Res::Working });
        } else if let Some(rest) = l.strip_prefix("Re-adding capability ") {
            items.push(ResultItem { name: rest.split(" (needs").next().unwrap_or(rest).trim().to_string(), res: Res::Working });
        } else if t.starts_with("not installed:") {
            items.push(ResultItem { name: after(t, "not installed:"), res: Res::AlreadyGone });
        } else if let Some(rest) = t.strip_prefix("KEEP ") {
            items.push(ResultItem { name: rest.split(" (").next().unwrap_or(rest).to_string(), res: Res::Kept });
        } else if t.starts_with("would ") {
            if let Some(i) = last { if matches!(i.res, Res::Working | Res::Applied | Res::AlreadySet) { i.res = Res::WouldRemove; } }
        } else if t.starts_with("done:") || t == "done" {
            if let Some(i) = last { i.res = if i.res == Res::Working && restoring && t == "done" { Res::Restored } else { Res::Removed }; }
        } else if t == "reinstalled" {
            if let Some(i) = last { i.res = Res::Restored; }
        } else if t.starts_with("WARNING:") {
            if let Some(i) = last { i.res = Res::Failed("still installed for some user".into()); }
        } else if t.starts_with("uninstall failed:") || t.starts_with("failed:") {
            let msg = t.split_once(':').map(|x| x.1.trim().to_string()).unwrap_or_default();
            if let Some(i) = last { i.res = Res::Failed(msg); }
        } else if let Some(rest) = t.strip_prefix("ERROR on ") {
            let (n, msg) = rest.split_once(':').unwrap_or((rest, ""));
            match items.iter_mut().rev().find(|i| i.name == n.trim()) {
                Some(i) => i.res = Res::Failed(msg.trim().into()),
                None => items.push(ResultItem { name: n.trim().into(), res: Res::Failed(msg.trim().into()) }),
            }
        } else if t.starts_with("winget exit code") {
            if let Some(i) = last { i.res = Res::Failed("reinstall it from the Microsoft Store by name".into()); }
        } else if let Some(rest) = t.strip_prefix("NOT restored:") {
            // unblocked only: the app has to be reinstalled by hand, so this is not a success
            let (n, why) = rest.split_once(" - ").unwrap_or((rest, "reinstall it from the Microsoft Store by name"));
            items.push(ResultItem { name: n.trim().into(), res: Res::Failed(format!("not reinstalled ({})", why.trim())) });
        } else if t.starts_with("Restore point") || t.starts_with("Registered re-check task") || t.starts_with("Preview only") || t.starts_with("Restore finished") || t.starts_with("Skipping:") || t.starts_with("Restart the PC") {
            notes.push(t.to_string());
        }
    }
    (items, notes)
}

/// A real Store app (its installed state can be read from Get-AppxPackage); capabilities and system items cannot.
pub fn is_app(t: &Target) -> bool {
    t.kind == Kind::App
}

/// What the script will be started with once the preview is confirmed.
pub struct Launch {
    pub args: Vec<String>,
    pub title: &'static str,
    /// Items the script was asked to process (0 = not known, e.g. Restore).
    pub targeted: usize,
}

/// The Remove Bloat page's state: what is ticked, what is installed, the running script and its pop-up.
pub struct Bloat {
    /// Ticked by hand = removed by the next "Remove selected".
    pub pick: Vec<bool>,
    pub installed: Option<Vec<String>>,
    pub scan: Option<common::Job<Vec<String>>>,
    pub stream: Option<Stream>,
    pub last_title: String,
    pub targeted: usize,
    pub op: Option<OpWindow>,
    pub pending: Option<Launch>,
    /// A run has just ended: read the installed apps again (once).
    pub rescan_due: bool,
    /// The full script log while its window is open ("Show technical details" in the pop-up).
    pub log: Option<String>,
    /// Screenshot demo: the pop-up shows a canned log, nothing runs.
    pub demo: bool,
}

#[derive(serde::Deserialize)]
pub struct Name {
    #[serde(rename = "Name")]
    pub name: String,
}

pub const DEMO_LOG: &[&str] = &[
    "20:15:22  Restore point created.",
    "20:15:22  Removing Microsoft.MicrosoftOfficeHub  -  Copilot app (desktop + identity) and the Copilot key action",
    "20:15:24    done: Microsoft.MicrosoftOfficeHub removed",
    "20:15:25    not installed: Clipchamp.Clipchamp",
    "20:15:25  Removing Microsoft.BingNews  -  Microsoft News",
    "20:15:26    done: Microsoft.BingNews removed",
    "20:15:26  Removing Microsoft.BingWeather  -  Weather",
    "20:15:27    done: Microsoft.BingWeather removed",
    "20:15:27    not installed: Microsoft.BingSearch",
    "20:15:28  Removing Microsoft.MicrosoftSolitaireCollection  -  Solitaire",
    "20:15:29    done: Microsoft.MicrosoftSolitaireCollection removed",
    "20:15:29    not installed: Microsoft.PowerAutomateDesktop",
    "20:15:30  Removing Microsoft.WindowsCamera  -  Camera app",
    "20:15:30    uninstall failed: Access is denied.",
    "20:15:31  Removing Microsoft.WindowsFeedbackHub  -  Feedback Hub",
    "20:15:32    done: Microsoft.WindowsFeedbackHub removed",
    "20:15:33  Removing capability Print.Management.Console~~~~0.0.1.0  -  Print Management console",
    // the lines below only appear in the finished sample
    "20:15:36    done: Print.Management.Console~~~~0.0.1.0",
    "20:15:37  Applying: Recall  -  Nothing (Recall is turned off by policy).",
    "20:15:38  Applying: OneDrive leftovers  -  Nothing (stops OneDrive returning).",
    "20:15:39  Registered re-check task \\Debloat\\ReapplyDebloat (startup + daily) - removes anything updates bring back.",
    "Restart the PC. Undo any time with:  .\\Remove-Bloat.ps1 -Restore",
];

impl Default for Bloat {
    fn default() -> Self {
        Self::new()
    }
}

impl Bloat {
    pub fn new() -> Self {
        let mut tab = Bloat {
            pick: vec![false; TARGETS.len()],
            installed: None,
            scan: None,
            stream: None,
            last_title: String::new(),
            targeted: 0,
            op: None,
            pending: None,
            rescan_due: false,
            log: None,
            demo: false,
        };
        if let Some(stage) = opwin::demo_stage("bloat") {
            // samples for screenshots - the script is not started
            tab.pick = TARGETS.iter().map(|t| !t.optional).collect();
            let mut w = tab.remove_window().0;
            if stage != Stage::Preview {
                let done = stage == Stage::Done;
                let n = if done { DEMO_LOG.len() } else { DEMO_LOG.len() - 5 };
                let lines: Vec<String> = DEMO_LOG[..n].iter().map(|s| s.to_string()).collect();
                w.stage = Stage::Running;
                w.show_steps = true;
                fill_window(&mut w, &lines, if done { Some(0) } else { None });
            }
            tab.op = Some(w);
            tab.demo = true;
        }
        tab
    }

    pub fn start_scan(&mut self) {
        self.scan = Some(common::Job::spawn(|| {
            common::ps_json::<Name>("Get-AppxPackage | Select-Object @{n='Name';e={$_.Name}}")
                .map(|v| v.into_iter().map(|n| n.name).collect())
                .unwrap_or_default()
        }));
    }

    /// Tick or untick a row. Pen handwriting and its language data go together: Windows puts the app
    /// back from the language data, so ticking either ticks both (and unticking either unticks both).
    pub fn toggle(&mut self, index: usize) {
        let Some(p) = self.pick.get_mut(index) else { return };
        *p = !*p;
        self.sync_paired(index);
    }

    /// Give the rows paired with `index` the same tick as it has.
    pub fn sync_paired(&mut self, index: usize) {
        let Some(&on) = self.pick.get(index) else { return };
        for group in PAIRED {
            if group.contains(&TARGETS[index].name) {
                for (i, t) in TARGETS.iter().enumerate() {
                    if group.contains(&t.name) {
                        self.pick[i] = on;
                    }
                }
            }
        }
    }

    /// Targets a run would process: the ticked items.
    pub fn selected(&self) -> Vec<&'static Target> {
        TARGETS.iter().zip(&self.pick).filter(|(_, p)| **p).map(|(t, _)| t).collect()
    }

    /// Some(true) = installed, Some(false) = removed, None = not known (still scanning, or not a Store app).
    pub fn is_installed(&self, t: &Target) -> Option<bool> {
        match (&self.installed, is_app(t)) {
            (Some(list), true) => Some(list.iter().any(|n| n == t.name)),
            _ => None,
        }
    }

    pub fn launch(&mut self, l: Launch) {
        let path = common::write_script("Remove-Bloat.ps1", SCRIPT);
        self.last_title = l.title.to_string();
        self.targeted = l.targeted;
        self.stream = Some(Stream::run_ps_file(&path, &l.args));
    }

    /// Preview window + launch data for "Remove selected".
    pub fn remove_window(&self) -> (OpWindow, Launch) {
        let sel = self.selected();
        let items: Vec<OpItem> = sel
            .iter()
            .map(|t| {
                OpItem::new(t.name, friendly(t.name), if t.optional { "Optional list" } else { "Safe list" })
                    .detail(if self.is_installed(t) == Some(false) { "already gone" } else { "" })
                    .optional(t.optional)
            })
            .collect();
        let mut w = OpWindow::new("REMOVE BLOAT", "remove", "removed", items);
        w.preview_text = "Removed for every user, leftovers cleaned up, and blocked from coming back with updates.".into();
        if !sel.is_empty() {
            // The shared window lists a long group collapsed, so name every item up front:
            // nobody should have to expand anything to see what is about to be removed.
            let names: Vec<String> = sel.iter().map(|t| friendly(t.name)).collect();
            w.preview_text.push_str(&format!("\n\nRemoves: {}.", names.join(", ")));
        }
        w.warning = Some("A Windows restore point is created first.".into());
        w.reassure = "Never touches Xbox, Game Pass, Store, Security, Edge/WebView2, Photos, Notepad or codecs.".into();
        w.go_label = "Remove".into();
        w.empty_text = "Nothing is ticked. Tick the apps to remove in the list.".into();
        let mut args = vec!["-Apply".to_string(), "-Yes".to_string()];
        args.extend(pick_args(&self.pick));
        (w, Launch { args, title: "Remove selected", targeted: sel.len() })
    }

    /// Is the script running now?
    pub fn running(&self) -> bool {
        self.stream.as_ref().map(|s| s.exit.is_none()).unwrap_or(false)
    }

    /// Poll the installed-apps scan and the running script. Call often.
    pub fn pump(&mut self) {
        if self.installed.is_none() && self.scan.is_none() {
            self.start_scan();
        }
        if let Some(j) = &self.scan {
            if let Some(r) = j.poll() {
                self.installed = Some(r);
                self.scan = None;
            }
        }
        if let Some(s) = &mut self.stream {
            // only the frame on which the run ends asks for a refresh of the installed status
            let was_running = s.exit.is_none();
            s.pump();
            if was_running {
                // the pop-up shows the progress and, at the end, the results
                if let Some(op) = &mut self.op {
                    fill_window(op, &s.lines, s.exit);
                }
                if s.exit.is_some() {
                    self.rescan_due = true;
                }
            }
        }
        if self.rescan_due && self.scan.is_none() {
            // a scan that was still in flight when the run ended has finished by now: read again
            self.rescan_due = false;
            self.installed = None;
        }
    }

    /// May the pop-up's buttons be pressed now?
    pub fn can_act(&self) -> bool {
        common::is_admin() && !self.running() && !self.demo
    }

    /// React to a button of the remove pop-up: confirm starts the script and the pop-up stays open,
    /// ticks off each app as it goes and shows the results ("Show technical details" = the full log).
    pub fn on_event(&mut self, ev: OpEvent) {
        match ev {
            OpEvent::Go => {
                if let Some(l) = self.pending.take() {
                    if let Some(op) = &mut self.op {
                        op.stage = Stage::Running;
                        op.show_steps = true;
                        op.running_text = "Removing...".into();
                    }
                    self.launch(l);
                }
            }
            OpEvent::Extra(0) => {
                let lines = self.stream.as_ref().map(|s| s.lines.clone()).unwrap_or_else(|| DEMO_LOG.iter().map(|s| s.to_string()).collect());
                self.log = Some(lines.join("\n"));
            }
            OpEvent::Close => {
                self.op = None;
                self.pending = None;
            }
            _ => {}
        }
    }

    /// "Remove selected": open the confirmation pop-up for the ticked apps.
    pub fn open_remove(&mut self) {
        let (w, l) = self.remove_window();
        self.pending = if w.items.is_empty() { None } else { Some(l) };
        self.op = Some(w);
    }

    /// The summary of the running / last run, one item per app.
    pub fn last_run(&self) -> Option<Vec<ResultItem>> {
        self.stream.as_ref().map(|s| summarize(&s.lines, self.last_title.starts_with("Preview")).0)
    }
}

/// The script runs whole tiers minus -Skip: ask for the Optional tier only when an optional item is
/// ticked, and skip every unticked item of the tiers that run.
pub fn pick_args(pick: &[bool]) -> Vec<String> {
    let optional = TARGETS.iter().zip(pick).any(|(t, p)| *p && t.optional);
    let skip: Vec<&str> = TARGETS.iter().zip(pick).filter(|(t, p)| !**p && (optional || !t.optional)).map(|(t, _)| t.name).collect();
    let mut args = vec![];
    if optional {
        args.push("-Optional".to_string());
    }
    if !skip.is_empty() {
        args.push("-Skip".into());
        args.push(skip.join(","));
    }
    args
}

/// Short form of a capability's full name for telling apart rows of one target,
/// e.g. "Language.Speech~~~en-US~0.0.1.0" -> "Language.Speech en-US".
#[cfg(test)]
pub fn part_name(name: &str) -> String {
    let mut parts: Vec<&str> = name.split('~').filter(|p| !p.is_empty()).collect();
    if parts.len() > 1 && parts.last().is_some_and(|p| p.chars().all(|c| c.is_ascii_digit() || c == '.')) {
        parts.pop();
    }
    parts.join(" ")
}

/// Labels for the result rows: the plain name, plus the part when one target produced several rows.
#[cfg(test)]
pub fn row_labels(items: &[ResultItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| {
            let shared = target_of(&i.name).is_some_and(|t| items.iter().filter(|o| target_of(&o.name) == Some(t)).count() > 1);
            if shared { format!("{} ({})", friendly(&i.name), part_name(&i.name)) } else { friendly(&i.name) }
        })
        .collect()
}

/// How many targets are finished: a target is finished when it has rows and none of them is still working.
/// Rows that belong to no target (e.g. a protected app) count as one item each.
#[cfg(test)]
pub fn finished_targets(items: &[ResultItem]) -> usize {
    let mut done = 0;
    let mut seen: Vec<usize> = vec![];
    for i in items {
        match target_of(&i.name) {
            Some(t) => {
                if !seen.contains(&t) {
                    seen.push(t);
                    if items.iter().all(|o| target_of(&o.name) != Some(t) || o.res != Res::Working) {
                        done += 1;
                    }
                }
            }
            None => done += usize::from(i.res != Res::Working),
        }
    }
    done
}

/// The outcome of one target in the script's log: None while it is still running or has no line yet.
pub fn target_result(items: &[ResultItem], target: usize) -> Option<Result<String, String>> {
    let rows: Vec<&ResultItem> = items.iter().filter(|i| target_of(&i.name) == Some(target)).collect();
    if rows.is_empty() || rows.iter().any(|r| r.res == Res::Working) {
        return None;
    }
    if let Some(Res::Failed(m)) = rows.iter().map(|r| &r.res).find(|r| matches!(r, Res::Failed(_))) {
        return Some(Err(if m.is_empty() { "failed".into() } else { m.clone() }));
    }
    Some(Ok(if rows.iter().any(|r| matches!(r.res, Res::Removed | Res::Applied | Res::Restored)) {
        "removed".into()
    } else if rows.iter().any(|r| r.res == Res::Kept) {
        "kept (protected)".into()
    } else {
        "already gone".into()
    }))
}

/// Put the script's progress into the remove pop-up: each app is ticked off as its result arrives;
/// once the script has ended the window shows the results, the notes and a "technical details" button.
pub fn fill_window(op: &mut OpWindow, lines: &[String], exit: Option<i32>) {
    let (items, notes) = summarize(lines, false);
    let result = |it: &OpItem| TARGETS.iter().position(|t| t.name == it.id).and_then(|i| target_result(&items, i));
    match exit {
        None => {
            for it in op.items.iter_mut() {
                if it.result.is_none() {
                    it.result = result(it);
                }
            }
        }
        Some(code) => {
            op.finish(|it| {
                result(it).or_else(|| Some(if code == 0 { Ok("nothing to do".into()) } else { Err(format!("no result - the script stopped (exit code {code})")) }))
            });
            op.notes = notes.into_iter().filter(|n| !n.starts_with("Skipping") && !n.starts_with("Restart the PC")).collect();
            if op.ok_count() > 0 {
                op.notes.push("Restart the PC to finish.".into());
            }
            op.extras = vec!["Show technical details".into()];
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The remove pop-up ticks apps off as the log arrives, then shows the results.
    #[test]
    fn bloat_results_in_the_popup() {
        let mut tab = Bloat::new();
        tab.pick = TARGETS.iter().map(|t| !t.optional).collect();
        let mut w = tab.remove_window().0;
        let n = w.items.len();
        let log: Vec<String> = DEMO_LOG.iter().map(|s| s.to_string()).collect();
        // while running: finished apps have results, the one in progress does not
        w.stage = Stage::Running;
        fill_window(&mut w, &log[..13], None);
        let id = |t: &str| w.items.iter().find(|i| i.id == t).map(|i| i.result.clone()).unwrap();
        assert_eq!(id("Microsoft.BingNews"), Some(Ok("removed".into())));
        assert_eq!(id("Clipchamp.Clipchamp"), Some(Ok("already gone".into())));
        assert_eq!(id("Microsoft.WindowsCamera"), None, "still being removed");
        assert_eq!(w.stage, Stage::Running);
        // finished: every item has a result, the failure keeps its reason, the log button is offered
        fill_window(&mut w, &log, Some(0));
        assert_eq!(w.stage, Stage::Done);
        assert_eq!(w.items.len(), n);
        assert!(w.items.iter().all(|i| i.result.is_some()));
        let cam = w.items.iter().find(|i| i.id == "Microsoft.WindowsCamera").unwrap();
        assert_eq!(cam.result, Some(Err("Access is denied.".into())));
        assert_eq!(w.extras, vec!["Show technical details".to_string()]);
        assert!(w.notes.iter().all(|n| !n.starts_with("Skipping")));
        assert!(w.notes.contains(&"Restart the PC to finish.".to_string()));
    }

    #[test]
    fn bloat_pick_args() {
        let safe: Vec<bool> = TARGETS.iter().map(|t| !t.optional).collect();
        assert!(pick_args(&safe).is_empty(), "the Safe preset as picked runs the Safe tier with nothing skipped");
        // Safe, minus Camera, plus Teams: both tiers run, every other unticked item is skipped
        let mut p = safe.clone();
        let cam = TARGETS.iter().position(|t| t.name == "Microsoft.WindowsCamera").unwrap();
        let teams = TARGETS.iter().position(|t| t.name == "MSTeams").unwrap();
        p[cam] = false;
        p[teams] = true;
        let a = pick_args(&p);
        assert_eq!(a[0], "-Optional");
        assert_eq!(a[1], "-Skip");
        let skip: Vec<&str> = a[2].split(',').collect();
        assert!(skip.contains(&"Microsoft.WindowsCamera"));
        assert!(!skip.contains(&"MSTeams"));
        assert_eq!(skip.len(), TARGETS.len() - p.iter().filter(|x| **x).count());
    }

    #[test]
    fn pen_handwriting_and_its_language_data_tick_together() {
        let mut b = Bloat::new();
        let ink = TARGETS.iter().position(|t| t.name == "Microsoft.Ink.Handwriting.Main.en-US.1.0.1").unwrap();
        let lang = TARGETS.iter().position(|t| t.name == "Language.Handwriting~").unwrap();
        b.toggle(ink);
        assert!(b.pick[ink] && b.pick[lang], "ticking pen handwriting ticks its language data");
        let names: Vec<&str> = b.selected().iter().map(|t| t.name).collect();
        assert_eq!(names, ["Microsoft.Ink.Handwriting.Main.en-US.1.0.1", "Language.Handwriting~"]);
        let skip = pick_args(&b.pick).last().cloned().unwrap_or_default();
        assert!(skip.split(',').all(|s| s != "Language.Handwriting~" && !s.starts_with("Microsoft.Ink")), "neither is skipped");
        b.toggle(lang);
        assert!(!b.pick[ink] && !b.pick[lang], "unticking either unticks both");
        // every paired name is a real target
        for g in PAIRED {
            for n in *g {
                assert!(TARGETS.iter().any(|t| t.name == *n), "{n} is not a target");
            }
        }
    }

    #[test]
    fn bloat_summary_from_log() {
        let log: Vec<String> = [
            "20:15:22  Restore point created.",
            "20:15:22  Removing Microsoft.MicrosoftOfficeHub  -  Copilot app (desktop + identity) and the Copilot key action",
            "20:15:22    uninstalling desktop Copilot...",
            "20:15:24    deprovision failed: The system cannot find the path specified.",
            "20:15:24    uninstalled Microsoft.MicrosoftOfficeHub_157.0.4341.46_neutral__8wekyb3d8bbwe",
            "20:15:24    done: Microsoft.MicrosoftOfficeHub removed",
            "20:15:25    not installed: Clipchamp.Clipchamp",
            "20:15:26  Removing capability Language.Speech~~~en-US~0.0.1.0  -  Voice typing, Voice Access, Game Bar speech-to-text.",
            "20:15:27    done: Language.Speech~~~en-US~0.0.1.0",
            "20:15:28  Removing MSTeams  -  Microsoft Teams (+ its Office meeting add-in).",
            "20:15:28    uninstall failed: Access is denied.",
            "20:15:29    KEEP Microsoft.ZuneMusic (protected)",
            "20:15:30  Applying: Recall  -  Nothing (Recall is turned off by policy).",
            "20:15:31  Registered re-check task \\Debloat\\ReapplyDebloat (startup + daily) - removes anything updates bring back.",
            "C:\\ProgramData\\Debloat\\backup\\...\\e3b0c442: The system cannot find the path specified.",
        ].iter().map(|s| s.to_string()).collect();
        let (items, notes) = summarize(&log, false);
        for i in &items { println!("{:<45} {:?}", friendly(&i.name), match &i.res { Res::Failed(m) => format!("failed: {m}"), r => format!("{}", match r { Res::Removed => "removed", Res::AlreadyGone => "already gone", Res::Kept => "kept", Res::Applied => "done", _ => "other" }) }); }
        for n in &notes { println!("note: {n}"); }
        assert!(matches!(items[0].res, Res::Removed));          // deprovision-failed noise ignored
        assert!(matches!(items[1].res, Res::AlreadyGone));
        assert!(matches!(items[2].res, Res::Removed));          // capability
        assert!(matches!(items[3].res, Res::Failed(_)));
        assert!(matches!(items[4].res, Res::Kept));
        assert!(matches!(items[5].res, Res::Applied));
        assert_eq!(items.len(), 6);                             // icacls noise not turned into items
        assert_eq!(notes.len(), 2);
        assert_eq!(friendly("Clipchamp.Clipchamp"), "Clipchamp video editor");
        assert_eq!(friendly("Language.Speech~~~en-US~0.0.1.0"), "Voice typing / speech");
    }

    fn log(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn preview_with_nothing_to_change_is_not_done() {
        // a preview where the Recall policies and OneDrive leftovers are already in place
        let l = log(&[
            "20:15:30  Applying: Recall  -  Nothing (Recall is turned off by policy).",
            "20:15:31  Applying: OneDrive leftovers  -  Nothing (stops OneDrive returning for new profiles).",
            "20:15:31    would remove OneDriveSetup from the Default user Run key if it is there, backed up (a preview does not open that profile)",
            "20:15:32  Applying: Recall  -  again",
            "20:15:32    would set HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\WindowsAI\\AllowRecallEnablement = 0",
            "Preview only - nothing was changed. Run with -Apply to do it.",
        ]);
        let (items, _) = summarize(&l, true);
        assert_eq!(items[0].res, Res::AlreadySet);
        // the Default user hive is not opened in a preview, so the real run may still change it
        assert_eq!(items[1].res, Res::WouldRemove);
        assert_eq!(items[2].res, Res::WouldRemove);
        assert!(items.iter().all(|i| i.res != Res::Applied));
        // a real run whose hook logged "already set"
        let (items, _) = summarize(&log(&["20:15:30  Applying: Recall  -  x", "20:15:30    already set", "20:15:31  Applying: OneDrive leftovers  -  x", "20:15:31    set HKLM:\\x = 1"]), false);
        assert_eq!(items[0].res, Res::AlreadySet);
        assert_eq!(items[1].res, Res::Applied);
    }

    #[test]
    fn restore_not_reinstalled_is_not_restored() {
        let l = log(&[
            "20:16:00  NOT restored: Microsoft.BingSearch - no Store id; unblocked, reinstall it from the Microsoft Store by name",
            "20:16:01  Reinstalling Microsoft.BingNews from the Store (9WZDNCRFHVFW)...",
            "20:16:09    reinstalled",
            "20:16:10  Reinstalling Copilot (desktop) from the Store (9NHT9RB2F4HD)...",
            "20:16:20    winget exit code 1 - reinstall it from the Store by name",
            "20:16:21  NOT restored: Teams Meeting Add-in - reinstall it by hand (the Teams add-in comes back with Teams)",
            "20:16:22  Re-enabling task ShowFeedbackToast...",
            "20:16:22    done",
            "20:16:23  Re-enabling optional feature Recall...",
            "20:16:23    failed: The requested operation requires elevation.",
            "20:16:24  Re-registering framework Microsoft.Ink.Handwriting.en-US.1.0...",
            "20:16:24    done",
            "20:16:25  Restore finished. Quarantined files are kept under C:\\ProgramData\\Debloat\\backup\\<date>\\quarantine.",
        ]);
        let (items, _) = summarize(&l, false);
        assert!(matches!(&items[0].res, Res::Failed(m) if m.contains("no Store id")));
        assert_eq!(items[0].name, "Microsoft.BingSearch");
        assert_eq!(items[1].res, Res::Restored);
        assert!(matches!(items[2].res, Res::Failed(_)));
        assert!(matches!(items[3].res, Res::Failed(_)));
        assert_eq!(items[3].name, "Teams Meeting Add-in");
        assert_eq!((items[4].name.as_str(), &items[4].res), ("task ShowFeedbackToast", &Res::Restored));
        assert!(matches!(items[5].res, Res::Failed(_)));
        assert_eq!(items[6].res, Res::Restored);
        assert_eq!(items.len(), 7);
        // a later Restore: apps settled on an earlier run are neutral lines, not failures
        let (items, _) = summarize(&log(&[
            "20:17:00  Microsoft.BingSearch: still unblocked from an earlier Restore, reinstall it by hand",
            "20:17:00  Microsoft.Windows.DevHome: installed again - nothing to restore",
            "20:17:01  Restore finished. Quarantined files are kept under C:\\ProgramData\\Debloat\\backup\\<date>\\quarantine.",
        ]), false);
        assert!(items.iter().all(|i| !matches!(i.res, Res::Failed(_))));
    }

    #[test]
    fn progress_counts_targets_not_rows() {
        let l = log(&[
            "20:15:33  Removing capability Hello.Face.20134~~~~0.0.1.0  -  Windows Hello face",
            "20:15:34    done: Hello.Face.20134~~~~0.0.1.0",
            "20:15:34  Removing capability Hello.Face.Migration~~~~0.0.1.0  -  Windows Hello face",
            "20:15:35    done: Hello.Face.Migration~~~~0.0.1.0",
            "20:15:35  Removing capability Language.Speech~~~en-US~0.0.1.0  -  speech",
            "20:15:36    done: Language.Speech~~~en-US~0.0.1.0",
            "20:15:36  Removing capability Language.Speech~~~de-DE~0.0.1.0  -  speech",
            "20:15:36    KEEP Microsoft.ZuneMusic (protected)",
        ]);
        let (items, _) = summarize(&l, false);
        assert_eq!(items.len(), 5);
        // Hello face (2 rows) is finished, speech still has a working row, the protected app counts once
        assert_eq!(finished_targets(&items), 2);
        let labels = row_labels(&items);
        assert_eq!(labels[0], "Windows Hello face (PIN unaffected) (Hello.Face.20134)");
        assert_eq!(labels[2], "Voice typing / speech (Language.Speech en-US)");
        assert_ne!(labels[0], labels[1]);
        // a capability query error becomes a failed row for that target
        let (items, _) = summarize(&log(&["20:15:33    ERROR on Hello.Face.: cannot query capabilities: Class not registered"]), false);
        assert!(matches!(&items[0].res, Res::Failed(m) if m.starts_with("cannot query")));
        assert_eq!(target_of(&items[0].name), TARGETS.iter().position(|t| t.name == "Hello.Face."));
    }

    // ---- guards for the embedded script

    /// The text after `key = '` up to the next quote, in one $Targets line.
    fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        let start = line.find(&format!("{key} = '"))? + key.len() + 4;
        line[start..].split('\'').next()
    }

    /// (Tier, Kind, Name) of every entry in the script's $Targets table.
    fn script_targets() -> Vec<(String, String, String)> {
        let block = SCRIPT.split("$Targets = @(").nth(1).unwrap().split("\n)").next().unwrap();
        block
            .lines()
            .filter(|l| l.trim_start().starts_with("@{"))
            .map(|l| (field(l, "Tier").unwrap().to_string(), field(l, "Kind").unwrap().to_string(), field(l, "Name").unwrap().to_string()))
            .collect()
    }

    fn script_protected() -> Vec<String> {
        let block = SCRIPT.split("$Protected = @(").nth(1).unwrap().split("\n)").next().unwrap();
        block.split('\'').skip(1).step_by(2).map(|s| s.to_string()).collect()
    }

    #[test]
    fn targets_match_the_script() {
        let script = script_targets();
        assert_eq!(script.len(), TARGETS.len());
        for (t, (tier, kind, name)) in TARGETS.iter().zip(&script) {
            assert_eq!(t.name, name);
            assert_eq!(t.optional, tier == "Optional", "{name}");
            let k = match kind.as_str() { "appx" => Kind::App, "capability" => Kind::Capability, "system" => Kind::System, o => panic!("unknown kind {o}") };
            assert_eq!(t.kind, k, "{name}");
        }
        assert_eq!(TARGETS.iter().filter(|t| is_app(t)).count(), 25);
        assert!(is_app(TARGETS.iter().find(|t| t.name == "MSTeams").unwrap()));
    }

    #[test]
    fn gaming_and_protected_apps_are_never_targets() {
        let protected = script_protected();
        let gaming = [
            "Microsoft.GamingApp", "Microsoft.GamingServices", "Microsoft.Xbox.TCUI", "Microsoft.XboxIdentityProvider",
            "Microsoft.XboxGamingOverlay", "Microsoft.XboxGameCallableUI", "Microsoft.254428597CFE2", "Microsoft.WindowsStore",
            "Microsoft.StorePurchaseApp", "Microsoft.DesktopAppInstaller",
        ];
        for g in gaming {
            assert!(protected.iter().any(|p| p == g), "{g} missing from $Protected");
        }
        for t in TARGETS {
            let base = t.name.trim_end_matches(['~', '.']).to_ascii_lowercase();
            for p in &protected {
                // exact names for apps; capability names are prefixes, so check those as prefixes too
                assert!(!p.eq_ignore_ascii_case(t.name) && !(t.kind == Kind::Capability && p.to_ascii_lowercase().starts_with(&base)), "{} is protected", t.name);
            }
        }
    }

    #[test]
    fn preview_never_mounts_a_hive() {
        // every "reg.exe load" must sit in a branch that a -WhatIf preview cannot reach
        let hook = SCRIPT.split("'OneDrive/run' {").nth(1).unwrap().split("default { }").next().unwrap();
        let load = hook.find("reg.exe load").unwrap();
        let guard = hook.find("-not $IsSystem -and $WhatIf").unwrap();
        let other = hook.find("} elseif ((Test-Path $hive) -and -not $IsSystem) {").unwrap();
        assert!(guard < other && other < load);
        assert_eq!(SCRIPT.matches("reg.exe load").count(), 2); // the hook above and Invoke-Restore (never in a preview)
        let restore = SCRIPT.split("function Invoke-Restore {").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(restore.contains("reg.exe load"));
        assert!(SCRIPT.contains("if ($Restore) {") && SCRIPT.find("if ($Restore) {") < SCRIPT.find("foreach ($t in $work)"));
    }

    fn lf(s: &str) -> String {
        s.replace("\r\n", "\n")
    }

    #[test]
    fn root_script_copies_match_the_embedded_ones() {
        assert!(lf(include_str!("../../../../Remove-Bloat.ps1")) == lf(SCRIPT), "Remove-Bloat.ps1: the root copy differs from pctuner/assets");
        assert!(
            lf(include_str!("../../../../PostInstall-Tweaks.ps1")) == lf(crate::scripts::POST_INSTALL_TWEAKS),
            "PostInstall-Tweaks.ps1: the root copy differs from pctuner/assets"
        );
    }

    /// Parses (never runs) a script with the PowerShell parser and returns the number of syntax errors.
    #[cfg(windows)]
    fn parse_errors(name: &str, text: &str) -> usize {
        let path = std::env::temp_dir().join(format!("pctuner-parse-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        let cmd = format!(
            "$e=$null;[void][System.Management.Automation.Language.Parser]::ParseFile('{}',[ref]$null,[ref]$e);$e | ForEach-Object {{ [Console]::Error.WriteLine($_.Message + ' line ' + $_.Extent.StartLineNumber) }};$e.Count",
            path.display()
        );
        let out = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &cmd]).output().expect("powershell");
        let _ = std::fs::remove_file(&path);
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().parse().expect("error count")
    }

    #[cfg(windows)]
    #[test]
    fn embedded_scripts_parse() {
        assert_eq!(parse_errors("Remove-Bloat.ps1", SCRIPT), 0);
        assert_eq!(parse_errors("PostInstall-Tweaks.ps1", crate::scripts::POST_INSTALL_TWEAKS), 0);
    }
}
