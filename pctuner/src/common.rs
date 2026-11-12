//! Shared helpers for every tab: PowerShell bridge, background jobs, JSON history files,
//! confirmation dialogs and colours.

use base64::Engine;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// Hide the console window of child processes.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ------------------------------------------------------------------ environment

/// %LOCALAPPDATA%\PCTuner - history files live here.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("PCTuner");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ------------------------------------------------------------------ PowerShell bridge

fn encode(script: &str) -> String {
    let full = format!(
        "$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[Text.Encoding]::UTF8;\n{}",
        script
    );
    let bytes: Vec<u8> = full.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Run a PowerShell script (Windows PowerShell 5.1, hidden). Ok(stdout) on exit code 0, Err(stderr+stdout) otherwise.
pub fn ps(script: &str) -> Result<String, String> {
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encode(script)])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not start PowerShell: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(format!("{}{}", String::from_utf8_lossy(&out.stderr), stdout).trim().to_string())
    }
}

/// Run a PowerShell script whose pipeline output is converted to a JSON array and deserialised.
/// The script's output objects must have properties matching T's fields.
pub fn ps_json<T: DeserializeOwned>(script: &str) -> Result<Vec<T>, String> {
    let wrapped = format!(
        "$ErrorActionPreference='Continue'\n$__r = @(& {{\n{}\n}})\nConvertTo-Json -InputObject $__r -Depth 6 -Compress",
        script
    );
    let out = ps(&wrapped)?;
    let t = out.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return Ok(vec![]);
    }
    // The JSON is the last line; earlier lines can be stray warnings.
    let json = t.lines().rev().find(|l| l.trim_start().starts_with('[')).unwrap_or(t);
    serde_json::from_str(json).map_err(|e| format!("could not read PowerShell output ({e}): {}", &json[..json.len().min(300)]))
}

/// Run an inline PowerShell script hidden, streaming its output line by line.
pub fn ps_stream(script: &str) -> Stream {
    Stream::run(
        "powershell.exe",
        vec!["-NoProfile".into(), "-NonInteractive".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-EncodedCommand".into(), encode(script)],
    )
}

/// Quote a string for use inside a single-quoted PowerShell literal.
pub fn psq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Run an external program hidden and capture (exit code, stdout+stderr).
pub fn run(exe: &str, args: &[&str]) -> Result<(i32, String), String> {
    let out = Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not start {exe}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), text))
}

// ------------------------------------------------------------------ background work

/// A one-shot background job. Poll it every frame; it yields its result once.
pub struct Job<T> {
    rx: mpsc::Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn<F: FnOnce() -> T + Send + 'static>(f: F) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        Job { rx }
    }
    pub fn poll(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

/// Live output of a long-running script, line by line.
pub enum StreamMsg {
    Line(String),
    Exit(i32),
}

/// Some Windows tools (icacls, netsh, ...) print UTF-16 when piped, which shows up as letters with
/// NUL characters between them ("C : \ P r o g r a m"). Drop NULs and stray carriage returns.
fn clean_line(l: &str) -> String {
    l.chars().filter(|c| *c != '\0' && *c != '\r' && *c != '\u{feff}').collect()
}

pub struct Stream {
    rx: mpsc::Receiver<StreamMsg>,
    pub lines: Vec<String>,
    pub exit: Option<i32>,
}

impl Stream {
    /// Run a .ps1 file with arguments, streaming its stdout/stderr.
    pub fn run_ps_file(path: &std::path::Path, args: &[String]) -> Self {
        let mut a: Vec<String> = vec!["-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), path.display().to_string()];
        a.extend(args.iter().cloned());
        Self::run("powershell.exe", a)
    }

    pub fn run(exe: &str, args: Vec<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let exe = exe.to_string();
        thread::spawn(move || {
            let child = Command::new(&exe)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(StreamMsg::Line(format!("could not start {exe}: {e}")));
                    let _ = tx.send(StreamMsg::Exit(-1));
                    return;
                }
            };
            let err = child.stderr.take();
            let tx2 = tx.clone();
            let errt = thread::spawn(move || {
                if let Some(e) = err {
                    for l in BufReader::new(e).lines().map_while(Result::ok) {
                        let _ = tx2.send(StreamMsg::Line(clean_line(&l)));
                    }
                }
            });
            if let Some(o) = child.stdout.take() {
                for l in BufReader::new(o).lines().map_while(Result::ok) {
                    let _ = tx.send(StreamMsg::Line(clean_line(&l)));
                }
            }
            let _ = errt.join();
            let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            let _ = tx.send(StreamMsg::Exit(code));
        });
        Stream { rx, lines: vec![], exit: None }
    }

    /// Pull new output; returns true while still running.
    pub fn pump(&mut self) -> bool {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                StreamMsg::Line(l) => self.lines.push(l),
                StreamMsg::Exit(c) => self.exit = Some(c),
            }
        }
        self.exit.is_none()
    }
}

// ------------------------------------------------------------------ JSON files

pub fn load_json<T: DeserializeOwned + Default>(name: &str) -> T {
    std::fs::read_to_string(data_dir().join(name))
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save_json<T: Serialize>(name: &str, v: &T) {
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(data_dir().join(name), s);
    }
}

/// Write an embedded script to %LOCALAPPDATA%\PCTuner\<name> and return its path.
pub fn write_script(name: &str, body: &str) -> PathBuf {
    let p = data_dir().join(name);
    // UTF-8 with BOM so Windows PowerShell 5.1 reads non-ASCII text correctly
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(body.as_bytes());
    let _ = std::fs::write(&p, bytes);
    p
}

// ------------------------------------------------------------------ UI helpers

/// Status colours (defined in the theme).
pub mod color {
    pub use crate::theme::{DIM, GAMING, JUNK, LOCKED, OK, OPTIONAL};
}

/// Set by the sidebar's "Reset GPU to stock" button; the GPU tab picks it up and opens its reset window.
pub static GPU_RESET_REQUEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A simple message box (OK only). Keep `Option<String>` in the tab.
pub fn message_ui(ctx: &eframe::egui::Context, title: &str, slot: &mut Option<String>) {
    use eframe::egui;
    let mut close = false;
    if let Some(text) = slot.as_ref() {
        crate::theme::dim_backdrop(ctx, "message");
        egui::Window::new("message-dialog")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(crate::theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(500.0);
                ui.label(egui::RichText::new(title).size(20.0).strong());
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(crate::theme::TEXT_DIM));
                });
                ui.add_space(12.0);
                if ui.add(crate::theme::primary_button("   OK   ")).clicked() {
                    close = true;
                }
            });
    }
    if close {
        *slot = None;
    }
}

/// Banner shown on tabs that need admin to change anything.
pub fn admin_banner(ui: &mut eframe::egui::Ui) {
    if !is_admin() {
        crate::theme::banner(ui, color::LOCKED, "View only: start PC Tuner as administrator to apply changes.");
    }
}

/// Every tab implements this.
pub trait Tab {
    fn title(&self) -> &'static str;
    fn ui(&mut self, ui: &mut eframe::egui::Ui);
    /// Called every frame for every tab, visible or not (background work such as the fan profile).
    fn tick(&mut self) {}
    /// Called once when PC Tuner closes (e.g. to hand fans back to the BIOS).
    fn on_exit(&mut self) {}
}
