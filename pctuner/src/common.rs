//! Shared helpers for every tab: PowerShell bridge, background jobs, JSON history files,
//! confirmation dialogs and colours.

use base64::Engine;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};
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

/// %ProgramData%\PCTuner without creating or hardening it.
fn secure_base() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("PCTuner")
}

/// %ProgramData%\PCTuner with an admin-only ACL: owned by Administrators, no inheritance,
/// SYSTEM + Administrators full control, Users read & execute (the same lock-down as
/// Initialize-DataDir in Remove-Bloat.ps1). Files that run elevated live here so an unelevated
/// process cannot swap them between the write and the start. Err when the folder cannot be
/// created or locked down; the caller must then not run anything from it. Needs administrator.
pub fn secure_dir() -> Result<PathBuf, String> {
    let d = secure_base();
    std::fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    // a junction or link planted by another user would redirect the writes
    check_plain_dir(&d)?;
    let ds = d.display().to_string();
    for a in lockdown_steps(&ds) {
        let a: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        match run("icacls.exe", &a)? {
            (0, _) => {}
            (code, out) => return Err(format!("could not lock down {} (icacls exit code {code}): {}", d.display(), out.trim())),
        }
    }
    // now only administrators can rename or replace the folder: check it again, then empty it
    check_plain_dir(&d)?;
    clear_children(&d)?;
    Ok(d)
}

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Err unless `d` is a real folder (not a junction, symbolic link or other reparse point).
fn check_plain_dir(d: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    let md = std::fs::symlink_metadata(d).map_err(|e| format!("cannot read {}: {e}", d.display()))?;
    if !md.is_dir() || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!("{} is a link or junction, not a plain folder, so it is not used", d.display()));
    }
    Ok(())
}

/// The icacls calls that lock down the folder itself. Never /T (icacls follows junctions planted
/// inside, which would rewrite the owner and ACL of whatever they point at) and always /L (act on
/// a link itself, not its target, should the folder be swapped for one between the check and the call).
fn lockdown_steps(ds: &str) -> Vec<Vec<String>> {
    [
        // take the folder away from its creator (a non-admin may have made it first)
        vec![ds, "/setowner", "*S-1-5-32-544", "/L", "/C", "/Q"],
        vec![ds, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-32-545:(OI)(CI)RX", "/L", "/Q"],
    ]
    .iter()
    .map(|s| s.iter().map(|a| a.to_string()).collect())
    .collect()
}

/// Delete everything directly inside `d` without following links: junctions and symbolic links
/// are removed as links (their targets are not touched), files are deleted (they may be hard links
/// or carry someone else's ACL; write_script writes its file fresh anyway). A real subfolder is
/// never created by PC Tuner, so it is an error rather than something to delete recursively.
fn clear_children(d: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    let rd = std::fs::read_dir(d).map_err(|e| format!("cannot list {}: {e}", d.display()))?;
    for e in rd {
        let p = e.map_err(|e| format!("cannot list {}: {e}", d.display()))?.path();
        let md = std::fs::symlink_metadata(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        let attr = md.file_attributes();
        let is_dir = attr & FILE_ATTRIBUTE_DIRECTORY != 0;
        let r = if attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            // remove_dir on a directory junction/symlink deletes the link only
            if is_dir { std::fs::remove_dir(&p) } else { std::fs::remove_file(&p) }
        } else if is_dir {
            return Err(format!("{} contains an unexpected folder {}, so it is not used", d.display(), p.display()));
        } else {
            std::fs::remove_file(&p)
        };
        r.map_err(|e| format!("cannot remove {} from {}: {e}", p.display(), d.display()))?;
    }
    Ok(())
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
        Err(ps_error_text(&String::from_utf8_lossy(&out.stderr), &stdout))
    }
}

/// The message of a failed ps(): CLIXML error records decoded, and PowerShell's position lines
/// ("At line:3 char:1", "+ CategoryInfo ...") dropped so only the readable text is left.
fn ps_error_text(stderr: &str, stdout: &str) -> String {
    let err = decode_clixml(stderr);
    let kept: Vec<&str> = err
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("+ ") || t == "+" || (t.starts_with("At ") && t.contains(" char:")))
        })
        .collect();
    let err = if kept.iter().any(|l| !l.trim().is_empty()) { kept.join("\n") } else { err };
    format!("{}\n{}", err.trim(), stdout.trim()).trim().to_string()
}

/// Undo the XML escapes used in CLIXML text: entities and `_xHHHH_` character codes.
fn clixml_unescape(s: &str) -> String {
    let s = s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find("_x") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let code = tail.get(2..6).filter(|h| h.chars().all(|c| c.is_ascii_hexdigit())).and_then(|h| u32::from_str_radix(h, 16).ok());
        match (code, tail.get(6..7)) {
            (Some(c), Some("_")) => {
                if let Some(ch) = char::from_u32(c) {
                    out.push(ch);
                }
                rest = &tail[7..];
            }
            _ => {
                out.push_str("_x");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    out.replace("\r\n", "\n")
}

/// Windows PowerShell started with -EncodedCommand writes its error stream as CLIXML
/// ("#< CLIXML" + XML). Return the readable error text (warnings prefixed "WARNING: ", progress
/// records skipped). Text that is not CLIXML comes back unchanged.
pub fn decode_clixml(s: &str) -> String {
    let t = s.trim_start_matches('\u{feff}').trim_start();
    let Some(body) = t.strip_prefix("#< CLIXML") else { return s.to_string() };
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find("<S S=\"") {
        rest = &rest[i + 6..];
        let Some(q) = rest.find('"') else { break };
        let kind = &rest[..q];
        let Some(gt) = rest[q..].find('>') else { break };
        let after = &rest[q + gt + 1..];
        let Some(end) = after.find("</S>") else { break };
        let text = clixml_unescape(&after[..end]);
        rest = &after[end + 4..];
        if kind.eq_ignore_ascii_case("error") {
            out.push_str(&text);
        } else if kind.eq_ignore_ascii_case("warning") {
            for l in text.lines() {
                out.push_str("WARNING: ");
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    out.trim().to_string()
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
    serde_json::from_str(json).map_err(|e| format!("could not read PowerShell output ({e}): {}", snippet(json, 300)))
}

/// The first `max` characters of `s` (never cuts a multi-byte character in half).
fn snippet(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Run an inline PowerShell script hidden, streaming its output line by line.
pub fn ps_stream(script: &str) -> Stream {
    Stream::run(
        "powershell.exe",
        vec!["-NoProfile".into(), "-NonInteractive".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-EncodedCommand".into(), encode(script)],
    )
}

/// Like `ps_stream`, for a script that only reads or measures (e.g. the latency test): closing
/// PC Tuner does not wait for it.
#[allow(dead_code)]
pub fn ps_stream_readonly(script: &str) -> Stream {
    Stream::spawn(
        "powershell.exe",
        vec!["-NoProfile".into(), "-NonInteractive".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-EncodedCommand".into(), encode(script)],
        false,
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
    let text = format!("{}{}", decode_bytes(&out.stdout, oem_cp()), decode_bytes(&out.stderr, oem_cp()));
    Ok((out.status.code().unwrap_or(-1), text))
}

// ------------------------------------------------------------------ text decoding

#[link(name = "kernel32")]
extern "system" {
    fn GetOEMCP() -> u32;
    fn MultiByteToWideChar(codepage: u32, flags: u32, src: *const u8, srclen: i32, dst: *mut u16, dstlen: i32) -> i32;
}

/// The system's console (OEM) code page, e.g. 437 (US) or 850 (Western Europe), which child
/// console programs write in. Read from the registry: PC Tuner's own manifest sets its active code
/// page to UTF-8, so GetOEMCP() in this process answers 65001 instead.
fn oem_cp() -> u32 {
    static CP: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *CP.get_or_init(|| {
        winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
            .open_subkey(r"SYSTEM\CurrentControlSet\Control\Nls\CodePage")
            .and_then(|k| k.get_value::<String, _>("OEMCP"))
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|cp| *cp > 0)
            .unwrap_or_else(|| unsafe { GetOEMCP() })
    })
}

/// Bytes from a child process as text: UTF-8 when valid, otherwise the given code page
/// (console tools such as ping.exe, and Windows PowerShell started with -File, write the OEM code
/// page), and as a last resort UTF-8 with replacement characters. Never fails.
fn decode_bytes(b: &[u8], codepage: u32) -> String {
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    if b.len() < i32::MAX as usize {
        unsafe {
            let n = MultiByteToWideChar(codepage, 0, b.as_ptr(), b.len() as i32, std::ptr::null_mut(), 0);
            if n > 0 {
                let mut w = vec![0u16; n as usize];
                let m = MultiByteToWideChar(codepage, 0, b.as_ptr(), b.len() as i32, w.as_mut_ptr(), n);
                if m > 0 {
                    return String::from_utf16_lossy(&w[..m as usize]);
                }
            }
        }
    }
    String::from_utf8_lossy(b).into_owned()
}

/// Call `f` with every line of `r` (line ends removed, decoded with `decode_bytes`). Unlike
/// `BufRead::lines` this never stops at a line that is not valid UTF-8; only end of input or a
/// read error ends it.
fn for_each_line<R: Read>(r: R, codepage: u32, mut f: impl FnMut(String)) {
    let mut r = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let mut end = buf.len();
                while end > 0 && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r') {
                    end -= 1;
                }
                f(clean_line(&decode_bytes(&buf[..end], codepage)));
            }
        }
    }
}

/// Turns PowerShell's CLIXML error stream ("#< CLIXML" followed by one XML line) back into
/// readable lines; any other line passes through unchanged.
#[derive(Default)]
struct ClixmlLines {
    seen_header: bool,
}

impl ClixmlLines {
    fn feed(&mut self, l: String) -> Vec<String> {
        if l.trim() == "#< CLIXML" {
            self.seen_header = true;
            return vec![];
        }
        if self.seen_header && l.trim_start().starts_with("<Objs") {
            return decode_clixml(&format!("#< CLIXML\n{l}")).lines().filter(|d| !d.trim().is_empty()).map(|d| d.to_string()).collect();
        }
        vec![l]
    }
}

// ------------------------------------------------------------------ background work

/// A one-shot background job. Poll it every frame; it yields its result once.
/// If the job panics, poll() stays None, `failed()` returns the reason and the crash is reported
/// once through `take_notices()` (shown by the main window) instead of a silent endless spinner.
pub struct Job<T> {
    rx: mpsc::Receiver<Result<T, String>>,
    failed: Mutex<Option<String>>,
}

/// Text of a panic payload.
fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "background task crashed".into())
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn<F: FnOnce() -> T + Send + 'static>(f: F) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| panic_text(p.as_ref()));
            let _ = tx.send(r);
        });
        Job { rx, failed: Mutex::new(None) }
    }
    pub fn poll(&self) -> Option<T> {
        let msg = match self.rx.try_recv() {
            Ok(Ok(v)) => return Some(v),
            Ok(Err(why)) => format!("A background task stopped unexpectedly: {why}"),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => "A background task stopped unexpectedly.".to_string(),
        };
        if let Ok(mut f) = self.failed.lock() {
            if f.is_none() {
                notice(format!("{msg}\n\nThe page that started it may keep waiting. Restart PC Tuner if it does."));
                *f = Some(msg);
            }
        }
        None
    }
    /// Some(reason) once poll() has seen the job crash instead of returning a result.
    #[allow(dead_code)]
    pub fn failed(&self) -> Option<String> {
        self.failed.lock().ok().and_then(|f| f.clone())
    }
}

// ------------------------------------------------------------------ notices for the main window

static NOTICES: Mutex<NoticeQueue> = Mutex::new(NoticeQueue::new());
/// At most this many notices per run, each at most NOTICE_MAX_CHARS characters.
const NOTICE_MAX_COUNT: usize = 50;
pub const NOTICE_MAX_CHARS: usize = 2000;
const NOTICE_SUPPRESSED: &str = "Further messages were suppressed. Restart PC Tuner to see new ones.";

struct NoticeQueue {
    /// Not yet taken by the main window.
    queue: Vec<String>,
    /// Texts queued with `once` this run (a condition that repeats on a timer, e.g. a history file
    /// that is re-read every few seconds, is reported once, also after the user closed the message).
    seen: Vec<String>,
    /// Notices queued this run; after NOTICE_MAX_COUNT one last "suppressed" notice is queued.
    count: usize,
}

impl NoticeQueue {
    const fn new() -> Self {
        NoticeQueue { queue: Vec::new(), seen: Vec::new(), count: 0 }
    }

    fn push(&mut self, text: String) {
        let text = cap_text(&text, NOTICE_MAX_CHARS);
        if self.queue.contains(&text) {
            return;
        }
        if self.count < NOTICE_MAX_COUNT {
            self.queue.push(text);
        } else if self.count == NOTICE_MAX_COUNT {
            self.queue.push(NOTICE_SUPPRESSED.to_string());
        } else {
            return;
        }
        self.count += 1;
    }

    fn once(&mut self, text: String) {
        let text = cap_text(&text, NOTICE_MAX_CHARS);
        if self.seen.contains(&text) {
            return;
        }
        self.seen.push(text.clone());
        self.push(text);
    }
}

/// Shorten `s` to at most `max` characters, marking the cut with "...".
pub fn cap_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(3)).collect();
    t.push_str("...");
    t
}

/// Queue a message for the main window (a crashed background task, a failed save, ...).
/// A text still waiting in the queue is not queued twice; once the window has taken it, the same
/// text is reported again (for example a second save that fails the same way).
pub fn notice(text: String) {
    if let Ok(mut n) = NOTICES.lock() {
        n.push(text);
    }
}

/// Like `notice`, but the same text is reported only once per run, also after the user closed it.
/// For conditions that are hit again and again on a timer.
pub fn notice_once(text: String) {
    if let Ok(mut n) = NOTICES.lock() {
        n.once(text);
    }
}

/// Messages queued with `notice` since the last call.
pub fn take_notices() -> Vec<String> {
    NOTICES.lock().map(|mut n| std::mem::take(&mut n.queue)).unwrap_or_default()
}

#[cfg(test)]
fn peek_notices() -> Vec<String> {
    NOTICES.lock().map(|n| n.queue.clone()).unwrap_or_default()
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

/// PowerShell processes started through `Stream` that are still running (they may be changing
/// the PC; closing PC Tuner waits for them so their undo record gets written).
static RUNNING_SCRIPTS: AtomicUsize = AtomicUsize::new(0);

pub fn scripts_running() -> usize {
    RUNNING_SCRIPTS.load(Ordering::SeqCst)
}

struct RunningScript;
impl RunningScript {
    fn start() -> Self {
        RUNNING_SCRIPTS.fetch_add(1, Ordering::SeqCst);
        RunningScript
    }
}
impl Drop for RunningScript {
    fn drop(&mut self) {
        RUNNING_SCRIPTS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Scripts write_script could not put in a safe place: path -> reason (run_ps_file refuses them).
static SCRIPT_ERRORS: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

fn set_script_error(p: &Path, e: Option<String>) {
    if let Ok(mut v) = SCRIPT_ERRORS.lock() {
        v.retain(|(q, _)| q != p);
        if let Some(e) = e {
            v.push((p.to_path_buf(), e));
        }
    }
}

fn script_error(p: &Path) -> Option<String> {
    SCRIPT_ERRORS.lock().ok().and_then(|v| v.iter().find(|(q, _)| q == p).map(|(_, e)| e.clone()))
}

pub struct Stream {
    rx: mpsc::Receiver<StreamMsg>,
    pub lines: Vec<String>,
    pub exit: Option<i32>,
}

impl Stream {
    /// Run a .ps1 file with arguments, streaming its stdout/stderr.
    pub fn run_ps_file(path: &std::path::Path, args: &[String]) -> Self {
        if let Some(e) = script_error(path) {
            return Self::failed(format!("The script was not run: {e}"));
        }
        let mut a: Vec<String> = vec!["-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), path.display().to_string()];
        a.extend(args.iter().cloned());
        Self::run("powershell.exe", a)
    }

    /// A stream that has already ended with exit code -1 and one line of explanation.
    fn failed(why: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(StreamMsg::Line(why));
        let _ = tx.send(StreamMsg::Exit(-1));
        Stream { rx, lines: vec![], exit: None }
    }

    /// Run a program, streaming its output. A PowerShell process counts as a running change
    /// (`scripts_running`); other programs (ping.exe, ...) do not.
    pub fn run(exe: &str, args: Vec<String>) -> Self {
        let counted = exe.to_ascii_lowercase().contains("powershell");
        Self::spawn(exe, args, counted)
    }

    fn spawn(exe: &str, args: Vec<String>, counted: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let exe = exe.to_string();
        let guard = if counted { Some(RunningScript::start()) } else { None };
        thread::spawn(move || {
            let _guard = guard;
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
            let cp = oem_cp();
            let err = child.stderr.take();
            let tx2 = tx.clone();
            let errt = thread::spawn(move || {
                if let Some(e) = err {
                    let mut cx = ClixmlLines::default();
                    for_each_line(e, cp, |l| {
                        for d in cx.feed(l) {
                            let _ = tx2.send(StreamMsg::Line(d));
                        }
                    });
                }
            });
            if let Some(o) = child.stdout.take() {
                for_each_line(o, cp, |l| {
                    let _ = tx.send(StreamMsg::Line(l));
                });
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

/// File bytes as text: UTF-8 (with or without BOM) or UTF-16 LE/BE with BOM.
fn decode_text_file(b: &[u8]) -> Result<String, String> {
    let utf16 = |rest: &[u8], le: bool| -> Result<String, String> {
        let u: Vec<u16> = rest.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16(&u).map_err(|_| "not valid UTF-16 text".to_string())
    };
    if let Some(r) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8(r.to_vec()).map_err(|_| "not valid UTF-8 text".into())
    } else if let Some(r) = b.strip_prefix(&[0xFF, 0xFE]) {
        utf16(r, true)
    } else if let Some(r) = b.strip_prefix(&[0xFE, 0xFF]) {
        utf16(r, false)
    } else {
        String::from_utf8(b.to_vec()).map_err(|_| "not valid UTF-8 text".into())
    }
}

/// Read a JSON file. A missing file gives T::default(). A file that cannot be read as T is
/// renamed to `<name>.corrupt-<unix time>` (so the next save cannot overwrite it), the user is told
/// through `notice`, and T::default() is returned.
fn load_json_at<T: DeserializeOwned + Default>(p: &Path) -> T {
    let bytes = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
        Err(e) => {
            notice_once(format!("{} could not be read ({e}). Changes recorded in it cannot be undone from PC Tuner right now.", p.display()));
            return T::default();
        }
    };
    let parsed = decode_text_file(&bytes).and_then(|s| serde_json::from_str::<T>(s.trim_start_matches('\u{feff}')).map_err(|e| e.to_string()));
    match parsed {
        Ok(v) => v,
        Err(why) => {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let backup = p.with_file_name(format!("{name}.corrupt-{secs}"));
            let kept = match std::fs::rename(p, &backup) {
                Ok(()) => format!("It was saved as {}.", backup.display()),
                Err(e) => format!("It could not be moved aside ({e})."),
            };
            notice_once(format!("{} could not be read ({why}). {kept} Earlier changes recorded in it cannot be undone from here.", p.display()));
            T::default()
        }
    }
}

/// Write a JSON file atomically: `<name>.tmp`, flushed to disk, then renamed over the old file.
fn save_json_at<T: Serialize>(p: &Path, v: &T) -> Result<(), String> {
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = p.with_file_name(format!("{name}.tmp"));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(s.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, p)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{} could not be saved: {e}", p.display())
    })
}

pub fn load_json<T: DeserializeOwned + Default>(name: &str) -> T {
    load_json_at(&data_dir().join(name))
}

pub fn save_json<T: Serialize>(name: &str, v: &T) {
    if let Err(e) = save_json_at(&data_dir().join(name), v) {
        notice(format!("{e}. The last change may not be undoable from PC Tuner."));
    }
}

/// Write an embedded script and return its path. As administrator it goes to the locked-down
/// %ProgramData%\PCTuner (see `secure_dir`) and is read back to check it; if that fails nothing is
/// written there and `Stream::run_ps_file` refuses the path with the reason. Without admin
/// (view only, nothing runs elevated) it goes to %LOCALAPPDATA%\PCTuner.
pub fn write_script(name: &str, body: &str) -> PathBuf {
    // UTF-8 with BOM so Windows PowerShell 5.1 reads non-ASCII text correctly
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(body.as_bytes());
    if !is_admin() {
        let p = data_dir().join(name);
        let _ = std::fs::write(&p, bytes);
        return p;
    }
    let p = secure_base().join(name);
    let r = secure_dir().and_then(|d| write_verified(&d.join(name), &bytes));
    set_script_error(&p, r.err());
    p
}

/// Replace `p` with `bytes` and read it back to make sure the file on disk is exactly that.
fn write_verified(p: &Path, bytes: &[u8]) -> Result<(), String> {
    match std::fs::remove_file(p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot replace {}: {e}", p.display())),
    }
    std::fs::write(p, bytes).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
    match std::fs::read(p) {
        Ok(b) if b == bytes => Ok(()),
        Ok(_) => Err(format!("{} changed right after it was written", p.display())),
        Err(e) => Err(format!("cannot read back {}: {e}", p.display())),
    }
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
    /// True while the tab is applying a change; closing PC Tuner then waits for it.
    fn busy(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pctuner-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn common_oem_ping_lines_survive() {
        // German ping.exe output in CP850: "ü" is 0x81, which is not valid UTF-8
        let mut bytes: Vec<u8> = b"\r\nPing wird ausgef".to_vec();
        bytes.push(0x81);
        bytes.extend_from_slice(b"hrt f\x81r 1.1.1.1 mit 32 Bytes Daten:\r\n");
        bytes.extend_from_slice(b"Antwort von 1.1.1.1: Bytes=32 Zeit=9ms TTL=57\r\n");
        bytes.extend_from_slice(b"Antwort von 1.1.1.1: Bytes=32 Zeit=11ms TTL=57");
        let mut lines = vec![];
        for_each_line(std::io::Cursor::new(bytes), 850, |l| lines.push(l));
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1], "Ping wird ausgeführt für 1.1.1.1 mit 32 Bytes Daten:");
        assert_eq!(lines[2], "Antwort von 1.1.1.1: Bytes=32 Zeit=9ms TTL=57");
        assert_eq!(lines[3], "Antwort von 1.1.1.1: Bytes=32 Zeit=11ms TTL=57");
        // UTF-8 stays UTF-8; garbage never stops the reader
        assert_eq!(decode_bytes("José".as_bytes(), 850), "José");
        let mut n = 0;
        for_each_line(std::io::Cursor::new(b"\xff\xfe\xfd\nok\n".to_vec()), 65001, |_| n += 1);
        assert_eq!(n, 2);
    }

    #[test]
    fn common_stream_keeps_lines_after_non_ascii() {
        // a real -File run: Windows PowerShell writes the OEM code page there
        let d = temp("stream");
        let p = d.join("t.ps1");
        std::fs::write(&p, "\u{feff}Write-Output 'Jos\u{e9} \u{fc}'\nWrite-Output 'after'\n[Console]::Error.WriteLine('err line')\n").unwrap();
        let mut s = Stream::run_ps_file(&p, &[]);
        let t0 = std::time::Instant::now();
        while s.pump() && t0.elapsed().as_secs() < 60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(s.exit, Some(0), "{:?}", s.lines);
        assert!(s.lines.iter().any(|l| l == "José ü"), "{:?}", s.lines);
        assert!(s.lines.iter().any(|l| l == "after"), "{:?}", s.lines);
        assert!(s.lines.iter().any(|l| l == "err line"), "{:?}", s.lines);
        let _ = std::fs::remove_dir_all(&d);
    }

    const CLIXML: &str = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\" xmlns=\"http://schemas.microsoft.com/powershell/2004/04\"><Obj S=\"progress\" RefId=\"0\"><TN RefId=\"0\"><T>System.Management.Automation.PSCustomObject</T><T>System.Object</T></TN><MS><I64 N=\"SourceId\">1</I64><PR N=\"Record\"><AV>Preparing modules for first use.</AV><AI>0</AI><Nil /><PI>-1</PI><PC>-1</PC><T>Completed</T><SR>-1</SR><SD> </SD></PR></MS></Obj><S S=\"warning\">careful &amp; slow_x000D__x000A_</S><S S=\"Error\">my readable error_x000D__x000A_</S><S S=\"Error\">At line:3 char:1_x000D__x000A_</S><S S=\"Error\">+ throw 'my readable error'_x000D__x000A_</S><S S=\"Error\">+ ~~~~~~~~~~~~~~~~~~~~~~~~~_x000D__x000A_</S><S S=\"Error\">    + CategoryInfo          : OperationStopped: (my readable error:String) [], RuntimeException_x000D__x000A_</S><S S=\"Error\">    + FullyQualifiedErrorId : my readable error_x000D__x000A_</S></Objs>";

    #[test]
    fn common_clixml_decoded() {
        let d = decode_clixml(CLIXML);
        assert!(d.starts_with("WARNING: careful & slow\nmy readable error\nAt line:3"), "{d}");
        assert!(!d.contains("Preparing modules"));
        assert!(!d.contains("<Objs"));
        assert_eq!(ps_error_text(CLIXML, ""), "WARNING: careful & slow\nmy readable error");
        assert_eq!(decode_clixml("plain text"), "plain text");
        assert_eq!(clixml_unescape("a_x0009_b _x zz &lt;i&gt;"), "a\tb _x zz <i>");
        // in a stream: the header line goes, the XML line becomes readable lines
        let mut cx = ClixmlLines::default();
        let mut out = vec![];
        for l in CLIXML.lines() {
            out.extend(cx.feed(l.to_string()));
        }
        assert_eq!(out[0], "WARNING: careful & slow");
        assert_eq!(out[1], "my readable error");
        assert!(out.iter().all(|l| !l.contains("CLIXML") && !l.contains("<S ")));
        assert_eq!(ClixmlLines::default().feed("normal".into()), vec!["normal".to_string()]);
    }

    #[test]
    fn common_ps_error_is_readable() {
        let e = ps("throw 'my readable error'").unwrap_err();
        assert_eq!(e, "my readable error");
    }

    #[test]
    fn common_snippet_multibyte() {
        let s = format!("{}ü and more", "x".repeat(299));
        let t = snippet(&s, 300);
        assert_eq!(t.chars().count(), 300);
        assert!(t.ends_with('ü'));
        assert_eq!(snippet("short", 300), "short");
    }

    #[test]
    fn common_job_panic_is_reported() {
        let j: Job<u8> = Job::spawn(|| panic!("boom"));
        let t0 = std::time::Instant::now();
        while j.failed().is_none() && t0.elapsed().as_secs() < 10 {
            assert_eq!(j.poll(), None);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(j.failed().unwrap().contains("boom"));
        assert!(peek_notices().iter().any(|n| n.contains("boom")));
        let ok: Job<u8> = Job::spawn(|| 7);
        let t0 = std::time::Instant::now();
        let mut got = None;
        while got.is_none() && t0.elapsed().as_secs() < 10 {
            got = ok.poll();
        }
        assert_eq!(got, Some(7));
        assert_eq!(ok.failed(), None);
    }

    #[test]
    fn common_json_history_files() {
        let d = temp("json");
        let p = d.join("h.json");
        // missing file: default, nothing created
        let v: Vec<u32> = load_json_at(&p);
        assert!(v.is_empty());
        assert!(!p.exists());
        // UTF-8 with BOM, UTF-16 LE and BE with BOM
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(b"[1,2]");
        std::fs::write(&p, &b).unwrap();
        assert_eq!(load_json_at::<Vec<u32>>(&p), vec![1, 2]);
        let mut le = vec![0xFF, 0xFE];
        le.extend("[3]".encode_utf16().flat_map(|u| u.to_le_bytes()));
        std::fs::write(&p, &le).unwrap();
        assert_eq!(load_json_at::<Vec<u32>>(&p), vec![3]);
        let mut be = vec![0xFE, 0xFF];
        be.extend("[4]".encode_utf16().flat_map(|u| u.to_be_bytes()));
        std::fs::write(&p, &be).unwrap();
        assert_eq!(load_json_at::<Vec<u32>>(&p), vec![4]);
        // truncated and empty files are moved aside, so the next save keeps them
        for bad in ["[1,2,", ""] {
            std::fs::write(&p, bad).unwrap();
            assert!(load_json_at::<Vec<u32>>(&p).is_empty());
            assert!(!p.exists());
            save_json_at(&p, &vec![9u32]).unwrap();
            assert_eq!(load_json_at::<Vec<u32>>(&p), vec![9]);
            let backups: Vec<PathBuf> = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|q| q.to_string_lossy().contains(".corrupt-")).collect();
            assert_eq!(backups.len(), 1);
            assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), bad);
            std::fs::remove_file(&backups[0]).unwrap();
        }
        assert!(peek_notices().iter().any(|n| n.contains("could not be read")));
        // atomic save leaves no temp file
        assert!(!d.join("h.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn common_notice_dedup_and_cap() {
        let mut q = NoticeQueue::new();
        q.once("timer".into());
        q.once("timer".into());
        assert_eq!(q.queue, vec!["timer".to_string()]);
        // after the queue was drained a `once` text does not come back
        q.queue.clear();
        q.once("timer".into());
        assert!(q.queue.is_empty());
        // a plain notice is merged while it waits, but reported again once it was taken
        q.push("save failed".into());
        q.push("save failed".into());
        assert_eq!(q.queue, vec!["save failed".to_string()]);
        q.queue.clear();
        q.push("save failed".into());
        assert_eq!(q.queue, vec!["save failed".to_string()]);
        // past the cap one "suppressed" notice is queued, then nothing
        for i in 0..NOTICE_MAX_COUNT + 5 {
            q.push(format!("n{i}"));
        }
        assert_eq!(q.queue.iter().filter(|n| *n == NOTICE_SUPPRESSED).count(), 1);
        assert_eq!(q.queue.last().map(String::as_str), Some(NOTICE_SUPPRESSED));
        assert_eq!(q.count, NOTICE_MAX_COUNT + 1);
        // the global entry points reach the shared queue
        let t = format!("global notice {}", std::process::id());
        notice_once(t.clone());
        notice_once(t.clone());
        assert_eq!(peek_notices().iter().filter(|n| **n == t).count(), 1);
        assert_eq!(cap_text(&"z".repeat(5000), NOTICE_MAX_CHARS).chars().count(), NOTICE_MAX_CHARS);
        assert_eq!(cap_text("short", 10), "short");
    }

    #[test]
    fn common_lockdown_never_recurses() {
        for s in lockdown_steps(r"C:\ProgramData\PCTuner") {
            assert_eq!(s[0], r"C:\ProgramData\PCTuner");
            assert!(!s.iter().any(|a| a.eq_ignore_ascii_case("/T") || a.contains('*') && !a.starts_with("*S-1-5-")), "{s:?}");
            assert!(s.iter().any(|a| a == "/L"), "{s:?}");
        }
    }

    #[test]
    fn common_clear_children_does_not_follow_links() {
        let d = temp("secure");
        let target = temp("secure-target");
        std::fs::write(target.join("keep.txt"), "keep").unwrap();
        std::fs::write(d.join("old.ps1"), "old").unwrap();
        let j = d.join("j");
        let (code, out) = run("cmd.exe", &["/c", "mklink", "/J", &j.display().to_string(), &target.display().to_string()]).unwrap();
        assert_eq!(code, 0, "{out}");
        check_plain_dir(&d).unwrap();
        assert!(check_plain_dir(&j).is_err());
        clear_children(&d).unwrap();
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        // the junction's target and its contents are untouched
        assert_eq!(std::fs::read_to_string(target.join("keep.txt")).unwrap(), "keep");
        // a real subfolder is refused, not deleted
        std::fs::create_dir(d.join("sub")).unwrap();
        assert!(clear_children(&d).is_err());
        assert!(d.join("sub").is_dir());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn common_refused_script_does_not_run() {
        let p = std::env::temp_dir().join("pctuner-never-written.ps1");
        set_script_error(&p, Some("folder is not safe".into()));
        let mut s = Stream::run_ps_file(&p, &[]);
        s.pump();
        assert_eq!(s.exit, Some(-1));
        assert!(s.lines[0].contains("folder is not safe"));
        set_script_error(&p, None);
        assert_eq!(script_error(&p), None);
    }
}
