//! Bloat tab - runs the proven Remove-Bloat.ps1 (embedded) with live output.
//! Deprovision + uninstall for all users, Deprovisioned marker, UScheduler_Oobe reinstall entries,
//! Revo-style leftover cleanup with quarantine, re-check task, full restore.

use crate::common::{self, Stream, Tab};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};

const SCRIPT: &str = include_str!("../assets/Remove-Bloat.ps1");

struct Target {
    name: &'static str,
    optional: bool,
    loses: &'static str,
}

const TARGETS: &[Target] = &[
    Target { name: "Microsoft.MicrosoftOfficeHub", optional: false, loses: "Copilot app (desktop + identity) and the Copilot key action" },
    Target { name: "Clipchamp.Clipchamp", optional: false, loses: "Clipchamp video editor" },
    Target { name: "Microsoft.BingNews", optional: false, loses: "Microsoft News" },
    Target { name: "Microsoft.BingWeather", optional: false, loses: "Weather" },
    Target { name: "Microsoft.BingSearch", optional: false, loses: "Bing web results in Start search" },
    Target { name: "Microsoft.MicrosoftSolitaireCollection", optional: false, loses: "Solitaire" },
    Target { name: "Microsoft.PowerAutomateDesktop", optional: false, loses: "Power Automate" },
    Target { name: "Microsoft.Windows.DevHome", optional: false, loses: "nothing (retired stub)" },
    Target { name: "Microsoft.WindowsCamera", optional: false, loses: "Camera app" },
    Target { name: "Microsoft.WindowsFeedbackHub", optional: false, loses: "Feedback Hub" },
    Target { name: "Microsoft.StartExperiencesApp", optional: false, loses: "MSN widgets" },
    Target { name: "Microsoft.Ink.Handwriting.Main.en-US.1.0.1", optional: false, loses: "pen handwriting" },
    Target { name: "App.StepsRecorder~", optional: false, loses: "Steps Recorder" },
    Target { name: "Hello.Face.", optional: false, loses: "Windows Hello face (PIN unaffected)" },
    Target { name: "Print.Management.Console~", optional: false, loses: "Print Management console" },
    Target { name: "Language.Handwriting~", optional: false, loses: "pen handwriting language data" },
    Target { name: "Recall", optional: false, loses: "nothing (Recall off by policy)" },
    Target { name: "OneDrive leftovers", optional: false, loses: "nothing (stops OneDrive returning)" },
    Target { name: "MicrosoftWindows.Client.WebExperience", optional: true, loses: "Widgets board" },
    Target { name: "Microsoft.WidgetsPlatformRuntime", optional: true, loses: "third-party widgets" },
    Target { name: "MSTeams", optional: true, loses: "Teams + Office meeting add-in" },
    Target { name: "Microsoft.OutlookForWindows", optional: true, loses: "new Outlook" },
    Target { name: "Microsoft.YourPhone", optional: true, loses: "Phone Link" },
    Target { name: "MicrosoftWindows.CrossDevice", optional: true, loses: "Mobile devices page" },
    Target { name: "Microsoft.Todos", optional: true, loses: "To Do" },
    Target { name: "Microsoft.MicrosoftStickyNotes", optional: true, loses: "Sticky Notes" },
    Target { name: "Microsoft.WindowsAlarms", optional: true, loses: "Clock / timers" },
    Target { name: "Microsoft.WindowsSoundRecorder", optional: true, loses: "Sound Recorder" },
    Target { name: "MicrosoftCorporationII.QuickAssist", optional: true, loses: "Quick Assist" },
    Target { name: "Microsoft.GetHelp", optional: true, loses: "Get Help + Settings troubleshooters" },
    Target { name: "Microsoft.XboxSpeechToTextOverlay", optional: true, loses: "Game Bar speech-to-text overlay" },
    Target { name: "Media.WindowsMediaPlayer~", optional: true, loses: "Windows Media Player Legacy" },
    Target { name: "Browser.InternetExplorer~", optional: true, loses: "old IE engine" },
    Target { name: "MathRecognizer~", optional: true, loses: "ink math input" },
    Target { name: "Microsoft.Windows.PowerShell.ISE~", optional: true, loses: "PowerShell ISE" },
    Target { name: "Language.Speech~", optional: true, loses: "voice typing / speech" },
    Target { name: "Language.OCR~", optional: true, loses: "copy text from images" },
];

// ------------------------------------------------------------------ friendly result summary

#[derive(Clone, PartialEq)]
enum Res {
    Working,
    Removed,
    WouldRemove,
    AlreadyGone,
    Kept,
    Applied,
    Restored,
    Failed(String),
}

struct ResultItem {
    name: String,
    res: Res,
}

/// Plain-English label for a package / capability name, using the target list.
fn friendly(name: &str) -> String {
    let t = TARGETS.iter().find(|t| {
        let base = t.name.trim_end_matches(['~', '.']);
        name.eq_ignore_ascii_case(t.name) || name.to_ascii_lowercase().starts_with(&base.to_ascii_lowercase())
    });
    match t {
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
fn summarize(lines: &[String]) -> (Vec<ResultItem>, Vec<String>) {
    let mut items: Vec<ResultItem> = vec![];
    let mut notes: Vec<String> = vec![];
    let after = |s: &str, p: &str| s[p.len()..].trim().to_string();
    let name_part = |s: &str| s.split("  -  ").next().unwrap_or(s).trim().to_string();
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
            items.push(ResultItem { name: name_part(rest), res: Res::Applied });
        } else if let Some(rest) = l.strip_prefix("Reinstalling ") {
            items.push(ResultItem { name: rest.split(" from the Store").next().unwrap_or(rest).trim().to_string(), res: Res::Working });
        } else if let Some(rest) = l.strip_prefix("Re-adding capability ") {
            items.push(ResultItem { name: rest.split(" (needs").next().unwrap_or(rest).trim().to_string(), res: Res::Working });
        } else if t.starts_with("not installed:") {
            items.push(ResultItem { name: after(t, "not installed:"), res: Res::AlreadyGone });
        } else if let Some(rest) = t.strip_prefix("KEEP ") {
            items.push(ResultItem { name: rest.split(" (").next().unwrap_or(rest).to_string(), res: Res::Kept });
        } else if t.starts_with("would ") {
            if let Some(i) = last { if i.res == Res::Working || i.res == Res::Applied { i.res = Res::WouldRemove; } }
        } else if t.starts_with("done:") || t == "done" {
            if let Some(i) = last { i.res = if i.res == Res::Working && lines.iter().any(|x| x.contains("Reinstalling") || x.contains("Re-adding")) && t == "done" { Res::Restored } else { Res::Removed }; }
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
        } else if t.contains(": unblocked") {
            let n = t.split(": unblocked").next().unwrap_or(t);
            items.push(ResultItem { name: n.into(), res: Res::Restored });
        } else if t.starts_with("Restore point") || t.starts_with("Registered re-check task") || t.starts_with("Preview only") || t.starts_with("Restore finished") || t.starts_with("Skipping:") || t.starts_with("Restart the PC") {
            notes.push(t.to_string());
        }
    }
    (items, notes)
}

// ------------------------------------------------------------------ tab

/// A real Store app (its installed state can be read from Get-AppxPackage); capabilities and system items cannot.
fn is_app(t: &Target) -> bool {
    !t.name.ends_with('~') && !t.name.ends_with('.') && t.name.contains('.')
}

/// What the script will be started with once the preview is confirmed.
struct Launch {
    args: Vec<String>,
    title: &'static str,
    /// Items the script was asked to process (0 = not known, e.g. Restore).
    targeted: usize,
}

pub struct BloatTab {
    skip: Vec<bool>,
    installed: Option<Vec<String>>,
    scan: Option<common::Job<Vec<String>>>,
    stream: Option<Stream>,
    last_title: String,
    targeted: usize,
    op: Option<OpWindow>,
    pending: Option<Launch>,
    show_details: bool,
    /// A run has just ended: read the installed apps again (once).
    rescan_due: bool,
    /// Screenshot demo: a canned log shown in the result card instead of a real run.
    demo: Option<(Vec<String>, Option<i32>)>,
}

#[derive(serde::Deserialize)]
struct Name {
    #[serde(rename = "Name")]
    name: String,
}

const DEMO_LOG: &[&str] = &[
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

impl BloatTab {
    pub fn new() -> Self {
        let mut tab = BloatTab {
            skip: vec![false; TARGETS.len()],
            installed: None,
            scan: None,
            stream: None,
            last_title: String::new(),
            targeted: 0,
            op: None,
            pending: None,
            show_details: false,
            rescan_due: false,
            demo: None,
        };
        if let Some(stage) = opwin::demo_stage("bloat") {
            // samples for screenshots - the script is not started
            match stage {
                Stage::Preview => tab.op = Some(tab.remove_window(false).0),
                Stage::Running | Stage::Done => {
                    let done = stage == Stage::Done;
                    let n = if done { DEMO_LOG.len() } else { DEMO_LOG.len() - 5 };
                    tab.demo = Some((DEMO_LOG[..n].iter().map(|s| s.to_string()).collect(), if done { Some(0) } else { None }));
                    tab.last_title = "Remove Safe".into();
                    let all: Vec<String> = DEMO_LOG.iter().map(|s| s.to_string()).collect();
                    tab.targeted = summarize(&all).0.len();
                }
            }
        }
        tab
    }

    fn start_scan(&mut self) {
        self.scan = Some(common::Job::spawn(|| {
            common::ps_json::<Name>("Get-AppxPackage | Select-Object @{n='Name';e={$_.Name}}")
                .map(|v| v.into_iter().map(|n| n.name).collect())
                .unwrap_or_default()
        }));
    }

    fn skip_arg(&self, optional: bool) -> Vec<String> {
        let names: Vec<&str> = TARGETS
            .iter()
            .zip(&self.skip)
            .filter(|(t, s)| **s && (optional || !t.optional))
            .map(|(t, _)| t.name)
            .collect();
        if names.is_empty() { vec![] } else { vec!["-Skip".into(), names.join(",")] }
    }

    /// Targets a run would process: the chosen lists minus the ticked (skipped) items.
    fn selected(&self, optional: bool) -> Vec<&'static Target> {
        TARGETS.iter().zip(&self.skip).filter(|(t, s)| !**s && (optional || !t.optional)).map(|(t, _)| t).collect()
    }

    /// Some(true) = installed, Some(false) = removed, None = not known (still scanning, or not a Store app).
    fn is_installed(&self, t: &Target) -> Option<bool> {
        match (&self.installed, is_app(t)) {
            (Some(list), true) => Some(list.iter().any(|n| n == t.name)),
            _ => None,
        }
    }

    fn launch(&mut self, l: Launch) {
        let path = common::write_script("Remove-Bloat.ps1", SCRIPT);
        self.last_title = l.title.to_string();
        self.targeted = l.targeted;
        self.stream = Some(Stream::run_ps_file(&path, &l.args));
    }

    /// Preview window + launch data for "Remove Safe" / "Remove Safe + Optional".
    fn remove_window(&self, optional: bool) -> (OpWindow, Launch) {
        let sel = self.selected(optional);
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
        w.warning = Some("A restore point is created first. Everything can be undone with Restore.".into());
        w.reassure = "Never touches Xbox, Game Pass, Store, Security, Edge/WebView2, Photos, Notepad or codecs.".into();
        w.go_label = "Remove".into();
        w.empty_text = "Every item in this list is ticked to be skipped.".into();
        let mut args = vec!["-Apply".to_string()];
        if optional {
            args.push("-Optional".into());
        }
        args.push("-Yes".into());
        args.extend(self.skip_arg(optional));
        (w, Launch { args, title: if optional { "Remove Safe + Optional" } else { "Remove Safe" }, targeted: sel.len() })
    }

    fn restore_window() -> (OpWindow, Launch) {
        let items = vec![
            OpItem::new("apps", "Reinstall every app this tool removed", "Restore").detail("from the Microsoft Store"),
            OpItem::new("reg", "Undo its registry and policy changes", "Restore"),
            OpItem::new("task", "Remove the re-check task", "Restore"),
        ];
        let mut w = OpWindow::new("REMOVE BLOAT", "run", "done", items).noun("restore step", "restore steps");
        w.preview_text = "Puts back everything Remove Bloat changed on this PC.".into();
        w.reassure = "Needs internet: the apps are downloaded again from the Microsoft Store.".into();
        w.go_label = "Restore".into();
        (w, Launch { args: vec!["-Restore".into(), "-Yes".into()], title: "Restore", targeted: 0 })
    }
}

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

/// One result row: icon, plain name, what happened.
fn result_row(ui: &mut egui::Ui, i: &ResultItem) {
    let (icon, text, c) = match &i.res {
        Res::Working => ("…", "working".to_string(), theme::OPTIONAL),
        Res::Removed => ("✔", "removed".into(), theme::OK),
        Res::Applied => ("✔", "done".into(), theme::OK),
        Res::Restored => ("✔", "restored".into(), theme::OK),
        Res::WouldRemove => ("•", "would be removed".into(), theme::OPTIONAL),
        Res::AlreadyGone => ("—", "already gone".into(), theme::DIM),
        Res::Kept => ("•", "kept (protected)".into(), theme::GAMING),
        Res::Failed(m) => ("✖", if m.is_empty() { "failed".into() } else { format!("failed: {m}") }, theme::LOCKED),
    };
    ui.horizontal(|ui| {
        if i.res == Res::Working {
            ui.add_sized([18.0, 18.0], egui::Spinner::new().size(13.0));
        } else {
            ui.add_sized([18.0, 18.0], egui::Label::new(RichText::new(icon).color(c).strong()));
        }
        ui.label(RichText::new(friendly(&i.name)).color(if i.res == Res::AlreadyGone { theme::DIM } else { theme::TEXT }));
        ui.label(RichText::new(text).color(c));
    });
}

/// The live result card: status line, progress (items finished / items targeted), rows as they arrive.
fn result_card(ui: &mut egui::Ui, title: &str, run: Option<(&[String], Option<i32>)>, targeted: usize, show_details: &mut bool) {
    ui.horizontal(|ui| {
        theme::section(ui, &if title.is_empty() { "Result".to_string() } else { format!("Result - {title}") });
        if run.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                visible_checkboxes(ui);
                ui.checkbox(show_details, "Show technical details");
            });
        }
    });
    let Some((lines, exit)) = run else {
        ui.label(RichText::new("Run a preview first - it changes nothing.").color(theme::TEXT_DIM));
        ui.allocate_space(ui.available_size());
        return;
    };
    let (mut items, notes) = summarize(lines);
    let running = exit.is_none();
    let count = |f: &dyn Fn(&Res) -> bool| items.iter().filter(|i| f(&i.res)).count();
    let removed = count(&|r| matches!(r, Res::Removed | Res::Applied | Res::Restored));
    let would = count(&|r| matches!(r, Res::WouldRemove));
    let gone = count(&|r| matches!(r, Res::AlreadyGone));
    let failed = count(&|r| matches!(r, Res::Failed(_)));
    let finished = count(&|r| !matches!(r, Res::Working));

    ui.horizontal(|ui| {
        if running {
            ui.spinner();
            let cur = items.iter().rev().find(|i| i.res == Res::Working).map(|i| friendly(&i.name)).unwrap_or_else(|| "Starting...".into());
            ui.label(RichText::new(format!("Working: {cur}")).size(16.0).strong());
        } else if failed > 0 {
            ui.label(RichText::new(format!("Finished with {failed} problem{}", if failed == 1 { "" } else { "s" })).size(16.0).strong().color(theme::LOCKED));
        } else {
            ui.label(RichText::new("✔  Finished").size(16.0).strong().color(theme::OK));
        }
        if targeted > 0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{} / {targeted}", finished.min(targeted))).color(theme::TEXT_DIM));
            });
        }
    });
    let width = ui.available_width();
    if !running {
        theme::progress(ui, 1.0, width);
    } else if targeted > 0 {
        let frac = ui.ctx().animate_value_with_time(egui::Id::new("bloat-progress"), finished as f32 / targeted as f32, 0.3);
        theme::progress(ui, frac, width);
    } else {
        theme::sweep(ui, width);
    }
    ui.horizontal_wrapped(|ui| {
        if would > 0 {
            theme::chip(ui, &format!("{would} would be removed"), theme::OPTIONAL);
        }
        if removed > 0 {
            theme::chip(ui, &format!("{removed} done"), theme::OK);
        }
        theme::chip(ui, &format!("{gone} already gone"), theme::DIM);
        if failed > 0 {
            theme::chip(ui, &format!("{failed} failed"), theme::LOCKED);
        }
    });
    ui.add_space(2.0);

    if !running {
        items.sort_by_key(|i| !matches!(i.res, Res::Failed(_))); // problems first
    }
    egui::ScrollArea::vertical().id_salt(if running { "bloat-live" } else { "bloat-final" }).auto_shrink([false, false]).stick_to_bottom(running).show(ui, |ui| {
        if *show_details {
            for l in lines {
                ui.label(RichText::new(l).monospace().small().color(theme::TEXT_DIM));
            }
            return;
        }
        for i in &items {
            result_row(ui, i);
        }
        if !notes.is_empty() {
            ui.add_space(6.0);
            for n in &notes {
                ui.label(RichText::new(n).size(12.5).color(theme::TEXT_DIM));
            }
        }
        if !running && items.is_empty() && notes.is_empty() {
            ui.label(RichText::new(format!("Finished (exit code {}). Tick \"Show technical details\" to see the full log.", exit.unwrap_or(-1))).color(theme::TEXT_DIM));
        }
    });
}

impl Tab for BloatTab {
    fn title(&self) -> &'static str {
        "Remove Bloat"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
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
            if was_running && s.exit.is_some() {
                self.rescan_due = true;
            }
        }
        if self.rescan_due && self.scan.is_none() {
            // a scan that was still in flight when the run ended has finished by now: read again
            self.rescan_due = false;
            self.installed = None;
        }
        let running = self.stream.as_ref().map(|s| s.exit.is_none()).unwrap_or(false);
        let can = common::is_admin() && !running;

        // Confirmation: the preview stage of the operation window. On Go the script starts and the window
        // closes, so the live result card underneath shows every item as it happens.
        if let Some(op) = &mut self.op {
            match op.show(ui.ctx(), can && self.pending.is_some()) {
                Some(OpEvent::Go) => {
                    self.op = None;
                    if let Some(l) = self.pending.take() {
                        self.launch(l);
                    }
                }
                Some(OpEvent::Close) => {
                    self.op = None;
                    self.pending = None;
                }
                _ => {}
            }
        }

        theme::page_header(ui, "Remove Bloat", "Removes apps in depth (all users, leftovers, reinstall blocks). Never touches Xbox, Game Pass, Store, Security or Edge.");
        common::admin_banner(ui);

        // ---- hero: how much of the list is gone
        let run: Option<(&[String], Option<i32>)> = match (&self.stream, &self.demo) {
            (Some(s), _) => Some((&s.lines[..], s.exit)),
            (None, Some((lines, exit))) => Some((&lines[..], *exit)),
            _ => None,
        };
        let apps: Vec<&Target> = TARGETS.iter().filter(|t| is_app(t)).collect();
        let last = run.map(|(lines, _)| summarize(lines).0);
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match &self.installed {
                    Some(list) => {
                        let gone = apps.iter().filter(|t| !list.iter().any(|n| n == t.name)).count();
                        let shown = theme::count_up(ui.ctx(), "bloat-hero", gone as f32).round() as usize;
                        theme::hero(ui, &format!("{shown} of {}", apps.len()), "apps removed", "Store apps from the Safe and Optional lists that are gone from this PC", theme::ACCENT);
                    }
                    None => {
                        ui.vertical(|ui| {
                            theme::hero(ui, "...", "apps removed", "Reading the installed apps", theme::TEXT_FAINT);
                            theme::sweep(ui, 260.0);
                        });
                    }
                }
                let n = |f: &dyn Fn(&Res) -> bool| last.as_ref().map(|items| items.iter().filter(|i| f(&i.res)).count());
                let show = |v: Option<usize>| v.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
                let failed = n(&|r| matches!(r, Res::Failed(_)));
                let dim = |on: bool, c: egui::Color32| if on { c } else { theme::TEXT_FAINT };
                stat_fit(ui, 30.0, &show(n(&|r| matches!(r, Res::WouldRemove))), "would remove (after a preview)", dim(last.is_some(), theme::OPTIONAL));
                stat_fit(ui, 22.0, &show(n(&|r| matches!(r, Res::AlreadyGone))), "already gone", dim(last.is_some(), theme::TEXT));
                stat_fit(ui, 22.0, &show(failed), "failed", dim(last.is_some(), if failed.unwrap_or(0) > 0 { theme::LOCKED } else { theme::TEXT }));
            });
        });
        ui.add_space(2.0);

        // ---- actions
        let mut preview: Option<bool> = None;
        let mut remove: Option<bool> = None;
        let mut restore = false;
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(can, theme::primary_button("  Preview Safe  ")).on_hover_text("Shows what would be removed. Changes nothing.").clicked() {
                preview = Some(false);
            }
            if ui.add_enabled(can, egui::Button::new("Preview Safe + Optional")).on_hover_text("Shows what would be removed. Changes nothing.").clicked() {
                preview = Some(true);
            }
            ui.separator();
            if ui.add_enabled(can, theme::danger_button("Remove Safe")).clicked() {
                remove = Some(false);
            }
            if ui.add_enabled(can, theme::danger_button("Remove Safe + Optional")).clicked() {
                remove = Some(true);
            }
            ui.separator();
            if ui.add_enabled(can, egui::Button::new("Restore everything")).clicked() {
                restore = true;
            }
            if ui.add_enabled(!running, egui::Button::new("Rescan")).clicked() {
                self.installed = None;
            }
        });
        ui.add_space(2.0);

        // ---- targets (left) and the live result (right)
        let skip = &mut self.skip;
        let installed = &self.installed;
        let show_details = &mut self.show_details;
        let (title, targeted) = (&self.last_title, self.targeted);
        ui.columns(2, |cols| {
            theme::card(&mut cols[0], |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::section(ui, "Targets");
                    ui.label(RichText::new("tick an item to skip it").size(12.0).color(theme::TEXT_DIM));
                });
                egui::ScrollArea::vertical().id_salt("bloat-list").auto_shrink([false, false]).show(ui, |ui| {
                    visible_checkboxes(ui);
                    for (i, t) in TARGETS.iter().enumerate() {
                        if i > 0 && t.optional && !TARGETS[i - 1].optional {
                            ui.add_space(6.0);
                            ui.label(RichText::new("OPTIONAL - EACH COSTS A CONVENIENCE").size(11.0).color(theme::TEXT_FAINT).strong());
                        }
                        let state = match (installed, is_app(t)) {
                            (Some(list), true) => Some(list.iter().any(|n| n == t.name)),
                            _ => None,
                        };
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add_space(8.0);
                                match state {
                                    Some(true) => theme::chip(ui, "installed", theme::JUNK),
                                    Some(false) => theme::chip(ui, "removed", theme::OK),
                                    None => {}
                                }
                                if t.optional {
                                    theme::chip(ui, "optional", theme::OPTIONAL);
                                }
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    ui.checkbox(&mut skip[i], "").on_hover_text("Tick to skip this item");
                                    let c = if skip[i] || state == Some(false) { theme::DIM } else { theme::TEXT };
                                    let mut name = RichText::new(friendly(t.name)).color(c);
                                    if skip[i] {
                                        name = name.strikethrough();
                                    }
                                    ui.add(egui::Label::new(name).truncate()).on_hover_text(format!("{}\nYou lose: {}", t.name.trim_end_matches(['~', '.']), t.loses));
                                });
                            });
                        });
                    }
                });
            });
            theme::card(&mut cols[1], |ui| {
                ui.set_width(ui.available_width());
                result_card(ui, title, run, targeted, show_details);
            });
        });

        // button clicks are handled last, when nothing borrows the tab any more
        if let Some(optional) = preview {
            let mut args = vec!["-WhatIf".to_string()];
            if optional {
                args.push("-Optional".into());
            }
            args.extend(self.skip_arg(optional));
            let targeted = self.selected(optional).len();
            self.launch(Launch { args, title: if optional { "Preview Safe + Optional" } else { "Preview Safe" }, targeted });
        }
        if let Some(optional) = remove {
            let (w, l) = self.remove_window(optional);
            self.pending = if w.items.is_empty() { None } else { Some(l) };
            self.op = Some(w);
        }
        if restore {
            let (w, l) = Self::restore_window();
            self.op = Some(w);
            self.pending = Some(l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (items, notes) = summarize(&log);
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
}
