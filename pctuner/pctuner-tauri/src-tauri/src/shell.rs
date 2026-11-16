//! The app shell: the tab registry, the pump thread, the window, the tray and the close guard.

use crate::backend::Backend;
use crate::tabs;
use pctuner_core::common;
use pctuner_core::power::Choice;
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// The longest notice text kept for the page (older paragraphs stay, new ones are dropped past this).
const NOTICE_MAX: usize = 8000;

pub struct Shell {
    tabs: Vec<(&'static str, Mutex<Box<dyn Backend>>)>,
    /// The tabs the page is showing: only their views are built and sent.
    active: Mutex<Vec<String>>,
    /// Hash of the last view sent per tab, so a view is only sent when it changed.
    sent: Mutex<HashMap<&'static str, u64>>,
    /// The window was closed while a change was running: quit as soon as it has finished.
    close_pending: AtomicBool,
    quitting: AtomicBool,
    exited: AtomicBool,
    /// Messages from background work (crashed task, unreadable history file, ...).
    notice: Mutex<Option<String>>,
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            tabs: tabs::all().into_iter().map(|(n, b)| (n, Mutex::new(b))).collect(),
            active: Mutex::new(vec![]),
            sent: Mutex::new(HashMap::new()),
            close_pending: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            notice: Mutex::new(None),
        }
    }

    fn tab(&self, name: &str) -> Result<&Mutex<Box<dyn Backend>>, String> {
        self.tabs.iter().find(|(n, _)| *n == name).map(|(_, t)| t).ok_or_else(|| format!("no tab named {name}"))
    }

    /// A change is being applied right now (a tab says so, or a PowerShell script is running).
    pub fn busy(&self) -> bool {
        common::scripts_running() > 0 || self.tabs.iter().any(|(_, t)| lock(t).busy())
    }

    pub fn quitting(&self) -> bool {
        self.quitting.load(Ordering::Relaxed)
    }

    /// The window's close button. Returns true when the close must wait for a running change.
    pub fn close_requested(&self, app: &AppHandle) -> bool {
        if self.busy() {
            self.close_pending.store(true, Ordering::Relaxed);
            self.emit_shell(app);
            return true;
        }
        self.quit(app);
        false
    }

    pub fn quit(&self, app: &AppHandle) {
        self.quitting.store(true, Ordering::Relaxed);
        app.exit(0);
    }

    /// Runs once when the app ends: every tab cleans up (the Fan Tuner hands the fans back to the BIOS).
    pub fn on_exit(&self) {
        if self.exited.swap(true, Ordering::Relaxed) {
            return;
        }
        for (_, t) in &self.tabs {
            lock(t).on_exit();
        }
    }

    fn info(&self) -> Value {
        json!({
            "admin": common::is_admin(),
            "version": env!("CARGO_PKG_VERSION"),
            "closePending": self.close_pending.load(Ordering::Relaxed),
            "notice": lock(&self.notice).clone(),
            "selftest": selftest_secs(),
        })
    }

    fn emit_shell(&self, app: &AppHandle) {
        let _ = app.emit("shell", self.info());
    }

    /// Add a background notice as a new paragraph (a paragraph already shown is not repeated).
    fn add_notice(&self, n: &str) -> bool {
        let n = common::cap_text(n, common::NOTICE_MAX_CHARS);
        let mut cur = lock(&self.notice);
        match cur.as_mut() {
            None => *cur = Some(n),
            Some(old) => {
                if old.split("\n\n").any(|p| p == n) || (old.ends_with("...") && old.chars().count() >= NOTICE_MAX) {
                    return false;
                }
                *old = common::cap_text(&format!("{old}\n\n{n}"), NOTICE_MAX);
            }
        }
        true
    }

    /// One round of the pump thread.
    fn pump(&self, app: &AppHandle) {
        let has_window = app.get_webview_window("main").is_some();
        let active = lock(&self.active).clone();
        for (name, tab) in &self.tabs {
            let mut t = lock(tab);
            t.pump();
            if !has_window || !active.iter().any(|a| a == name) {
                continue;
            }
            let view = t.view();
            drop(t);
            self.send_view(app, name, view, false);
        }
        let mut changed = false;
        for n in common::take_notices() {
            changed |= self.add_notice(&n);
        }
        if changed {
            self.emit_shell(app);
        }
        if self.close_pending.load(Ordering::Relaxed) && !self.busy() {
            self.close_pending.store(false, Ordering::Relaxed);
            self.quit(app);
        }
    }

    fn send_view(&self, app: &AppHandle, name: &'static str, view: Value, always: bool) {
        let mut h = DefaultHasher::new();
        view.to_string().hash(&mut h);
        let hash = h.finish();
        let old = lock(&self.sent).insert(name, hash);
        if always || old != Some(hash) {
            let _ = app.emit(&format!("tab:{name}"), view);
        }
    }
}

/// The view of one tab (the page asks when it opens a tab; later views arrive as `tab:<name>` events).
#[tauri::command(async)]
pub fn tab_state(shell: State<'_, Shell>, tab: String) -> Result<Value, String> {
    let view = lock(shell.tab(&tab)?).view();
    if selftest_secs().is_some() {
        selftest_log(&format!("state {tab}: {} bytes{}", view.to_string().len(), if view.is_null() { " (null view)" } else { "" }));
    }
    Ok(view)
}

/// Something the user did on a tab. Returns the tab's view after the action.
#[tauri::command(async)]
pub fn tab_action(shell: State<'_, Shell>, tab: String, action: Value) -> Result<Value, String> {
    let mut t = lock(shell.tab(&tab)?);
    t.act(action)?;
    Ok(t.view())
}

#[tauri::command]
pub fn shell_info(shell: State<'_, Shell>) -> Value {
    shell.info()
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShellAction {
    /// The tabs the page shows now.
    Active { tabs: Vec<String> },
    /// The sidebar's "Reset GPU to stock": the GPU tab picks it up and opens its reset window.
    ResetGpu,
    DismissNotice,
    /// Close guard: "Keep PC Tuner open".
    KeepOpen,
    /// Close guard: "Close anyway (the change may not be recorded)".
    CloseAnyway,
    /// Self-test: a line for the log (what the page rendered, script errors).
    Log { text: String },
    /// Self-test: every tab was visited.
    SelftestDone,
}

#[tauri::command]
pub fn shell_action(app: AppHandle, shell: State<'_, Shell>, action: ShellAction) -> Result<(), String> {
    match action {
        ShellAction::Active { tabs } => *lock(&shell.active) = tabs,
        ShellAction::ResetGpu => common::GPU_RESET_REQUEST.store(true, Ordering::Relaxed),
        ShellAction::DismissNotice => *lock(&shell.notice) = None,
        ShellAction::KeepOpen => shell.close_pending.store(false, Ordering::Relaxed),
        ShellAction::Log { text } => {
            selftest_log(&format!("page: {text}"));
            return Ok(());
        }
        ShellAction::SelftestDone => {
            selftest_log("selftest finished");
            shell.quit(&app);
            return Ok(());
        }
        ShellAction::CloseAnyway => {
            shell.close_pending.store(false, Ordering::Relaxed);
            shell.quit(&app);
            return Ok(());
        }
    }
    shell.emit_shell(&app);
    Ok(())
}

/// The pump thread: polls every tab about five times a second and sends changed views to the page.
pub fn start_pump(app: AppHandle) {
    let _ = std::thread::Builder::new().name("pump".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        let shell = app.state::<Shell>();
        if shell.quitting() {
            break;
        }
        shell.pump(&app);
    });
}

/// Open the main window, or bring it back when it exists. Must run on the main thread.
pub fn open_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    // PCTUNER_HEIGHT=<px>: taller window for screenshots of long pages
    let height = std::env::var("PCTUNER_HEIGHT").ok().and_then(|v| v.parse::<f64>().ok()).filter(|h| (520.0..=4000.0).contains(h)).unwrap_or(780.0);
    let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("PC Tuner")
        .inner_size(1250.0, height)
        .min_inner_size(950.0, 520.0)
        .theme(Some(tauri::Theme::Dark))
        .transparent(true)
        .effects(EffectsBuilder::new().effect(Effect::Mica).build());
    // PCTUNER_SELFTEST_HIDDEN=1 (with PCTUNER_SELFTEST): the self-test window opens far off-screen, without
    // taking the focus or a taskbar button, so it does not interrupt work on this PC.
    if selftest_secs().is_some() && std::env::var_os("PCTUNER_SELFTEST_HIDDEN").is_some() {
        b = b.position(-30000.0, -30000.0).focused(false).skip_taskbar(true);
    }
    b.build()?;
    Ok(())
}

fn power(c: Choice) {
    if common::is_admin() {
        std::thread::spawn(move || {
            let _ = common::run("shutdown.exe", &c.args());
        });
    }
}

/// The tray icon: Show PC Tuner / Restart into BIOS / Shut down (BIOS on next start) / Quit.
pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let admin = common::is_admin();
    let show = MenuItemBuilder::with_id("show", "Show PC Tuner").build(app)?;
    let restart = MenuItemBuilder::with_id("restart", "Restart into BIOS").enabled(admin).build(app)?;
    let shutdown = MenuItemBuilder::with_id("shutdown", "Shut down (BIOS on next start)").enabled(admin).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit PC Tuner").build(app)?;
    let menu = MenuBuilder::new(app).item(&show).separator().item(&restart).item(&shutdown).separator().item(&quit).build()?;
    let mut tray = TrayIconBuilder::with_id("main").tooltip("PC Tuner").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, e| match e.id().as_ref() {
        "show" => {
            let _ = open_window(app);
        }
        "restart" => power(Choice::RestartToBios),
        "shutdown" => power(Choice::ShutdownToBios),
        "quit" => {
            let shell = app.state::<Shell>();
            if shell.busy() {
                // show the window so the close guard can be seen; it quits once the change has finished
                let _ = open_window(app);
                shell.close_pending.store(true, Ordering::Relaxed);
                shell.emit_shell(app);
            } else {
                shell.quit(app);
            }
        }
        _ => {}
    })
    .on_tray_icon_event(|tray, e| {
        let left_click = matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. });
        if left_click || matches!(e, TrayIconEvent::DoubleClick { .. }) {
            let _ = open_window(tray.app_handle());
        }
    })
    .build(app)?;
    Ok(())
}

/// PCTUNER_SELFTEST=<seconds per tab>: the page visits every tab, the results go to
/// %LOCALAPPDATA%\PCTuner\selftest\next.log, then the app quits. Used to check builds.
pub fn selftest_secs() -> Option<f64> {
    let v = std::env::var("PCTUNER_SELFTEST").ok()?;
    Some(v.parse::<f64>().ok().filter(|s| *s >= 0.5).unwrap_or(3.0))
}

pub fn selftest_log(line: &str) {
    let dir = common::data_dir().join("selftest");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("next.log")).and_then(|mut f| {
        use std::io::Write;
        writeln!(f, "{} {line}", common::now())
    });
}
