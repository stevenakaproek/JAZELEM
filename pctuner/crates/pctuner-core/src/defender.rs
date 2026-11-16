//! Defender exclusions for game folders (shown on the Performance page).
//! Finds game libraries (Steam libraries, XboxGames, Epic, EA), adds the ticked ones as Microsoft
//! Defender exclusion folders, and undoes exactly what it added. Exclusions the user already had are
//! never removed. Needs administrator (Defender only shows and changes exclusions for admins).
//!
//! Logic only: the UI draws `Defender` and forwards the operation window's events to `on_event`.

use crate::common::{self, Job, Stream};
use crate::opwin::{OpEvent, OpItem, OpWindow, Stage};
use serde::{Deserialize, Serialize};

const HISTORY: &str = "defender-history.json";

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Scan {
    /// (kind, path) of game folders found on this PC
    pub folders: Vec<Found>,
    /// current exclusion folders; None when they could not be read (not admin / Defender off)
    pub excluded: Option<Vec<String>>,
    /// "Normal", "Passive" (another antivirus is active), "" when unknown
    pub mode: String,
    pub error: String,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Found {
    pub kind: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct HistEntry {
    pub time: String,
    /// paths PC Tuner added (and may remove on undo)
    pub added: Vec<String>,
    pub undone: bool,
}

const SCAN_SCRIPT: &str = r#"
$f = New-Object System.Collections.ArrayList
function AddF($k, $p) { if ($p -and (Test-Path -LiteralPath $p -PathType Container)) { $full = (Resolve-Path -LiteralPath $p).ProviderPath.TrimEnd('\'); if (-not ($f | Where-Object { $_.path -eq $full })) { [void]$f.Add([pscustomobject]@{ kind = $k; path = $full }) } } }
$steam = (Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue).SteamPath
if ($steam) {
  $steam = $steam -replace '/', '\'
  AddF 'Steam' (Join-Path $steam 'steamapps\common')
  $vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
  if (Test-Path -LiteralPath $vdf) {
    foreach ($m in [regex]::Matches((Get-Content -LiteralPath $vdf -Raw), '"path"\s+"([^"]+)"')) { AddF 'Steam' (Join-Path ($m.Groups[1].Value -replace '\\\\', '\') 'steamapps\common') }
  }
}
foreach ($d in Get-PSDrive -PSProvider FileSystem -ErrorAction SilentlyContinue) { if ($d.Root -match '^[A-Z]:\\$') { AddF 'Xbox / Game Pass' (Join-Path $d.Root 'XboxGames') } }
AddF 'Epic Games' (Join-Path $env:ProgramFiles 'Epic Games')
AddF 'EA' (Join-Path $env:ProgramFiles 'EA Games')
AddF 'Battle.net' (Join-Path ${env:ProgramFiles(x86)} 'Battle.net')
$ex = $null; $mode = ''; $err = ''
try { $mode = [string](Get-MpComputerStatus -ErrorAction Stop).AMRunningMode } catch { $err = 'Microsoft Defender is not available: ' + $_.Exception.Message }
try {
  $p = @((Get-MpPreference -ErrorAction Stop).ExclusionPath) | Where-Object { $_ }
  if (-not ($p | Where-Object { $_ -like 'N/A*' })) { $ex = @($p | ForEach-Object { ([string]$_).TrimEnd('\') }) }
} catch { if (-not $err) { $err = $_.Exception.Message } }
[pscustomobject]@{ folders = @($f); excluded = $ex; mode = $mode; error = $err }
"#;

fn scan() -> Scan {
    match common::ps_json::<Scan>(SCAN_SCRIPT) {
        Ok(v) => v.into_iter().next().unwrap_or_default(),
        Err(e) => Scan { error: format!("The game folders could not be read: {e}"), ..Default::default() },
    }
}

/// Same folder? (case-insensitive, trailing backslash ignored)
pub fn same_path(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('\\').eq_ignore_ascii_case(b.trim().trim_end_matches('\\'))
}

/// A folder path a user typed: absolute, a real drive, no wildcards, not a whole drive or Windows itself.
pub fn valid_custom(p: &str) -> Result<String, &'static str> {
    let p = p.trim().trim_matches('"').trim_end_matches('\\').to_string();
    let b = p.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return Err("Type a full folder path, e.g. D:\\Games");
    }
    if p.contains('*') || p.contains('?') {
        return Err("Wildcards are not allowed.");
    }
    if p.len() <= 3 {
        return Err("A whole drive cannot be excluded - pick the games folder on it.");
    }
    let low = p.to_ascii_lowercase();
    if ["\\windows", "\\program files", "\\program files (x86)", "\\programdata", "\\users"].iter().any(|s| low[2..] == **s) {
        return Err("That is a system folder - exclude only the games folder inside it.");
    }
    Ok(p)
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// One script; one RESULT line per folder. `add` = Add-MpPreference, else Remove-MpPreference.
fn script(paths: &[String], add: bool) -> String {
    let mut s = String::new();
    for (i, p) in paths.iter().enumerate() {
        let q = ps_quote(p);
        let (cmd, check) = if add {
            ("Add-MpPreference", "-contains")
        } else {
            ("Remove-MpPreference", "-notcontains")
        };
        s.push_str(&format!(
            "try {{ {cmd} -ExclusionPath {q} -ErrorAction Stop; $now = @((Get-MpPreference).ExclusionPath | ForEach-Object {{ ([string]$_).TrimEnd('\\') }}); \
if ($now {check} {q}) {{ Write-Output 'RESULT|{i}|ok|' }} else {{ Write-Output 'RESULT|{i}|err|Defender did not take the change (another antivirus or a policy may manage it).' }} }} \
catch {{ Write-Output ('RESULT|{i}|err|' + ($_.Exception.Message -replace '\\s+', ' ')) }}\n"
        ));
    }
    s
}

fn parse_result(line: &str) -> Option<(usize, Result<String, String>)> {
    let rest = line.trim().strip_prefix("RESULT|")?;
    let mut p = rest.splitn(3, '|');
    let i = p.next()?.parse().ok()?;
    let ok = p.next()? == "ok";
    let msg = p.next().unwrap_or("").trim().to_string();
    Some((i, if ok { Ok(String::new()) } else { Err(if msg.is_empty() { "Failed.".into() } else { msg }) }))
}

/// A change being applied: the script's output stream and the folders it works on.
pub struct Run {
    pub stream: Stream,
    pub seen: usize,
    pub paths: Vec<String>,
    pub undo_of: Option<usize>,
}

/// The Defender card's state.
pub struct Defender {
    pub scan: Option<Scan>,
    pub loading: Option<Job<Scan>>,
    /// ticked folders (paths)
    pub ticked: Vec<String>,
    pub custom: String,
    pub custom_err: Option<&'static str>,
    pub extra: Vec<String>,
    pub history: Vec<HistEntry>,
    pub op: Option<OpWindow>,
    pub pending: Option<(Vec<String>, Option<usize>)>,
    pub run: Option<Run>,
}

impl Default for Defender {
    fn default() -> Self {
        Self::new()
    }
}

impl Defender {
    pub fn new() -> Self {
        Defender {
            scan: None,
            loading: Some(Job::spawn(scan)),
            ticked: vec![],
            custom: String::new(),
            custom_err: None,
            extra: vec![],
            history: common::load_json(HISTORY),
            op: None,
            pending: None,
            run: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.run.is_some()
    }

    pub fn refresh(&mut self) {
        if self.loading.is_none() {
            self.loading = Some(Job::spawn(scan));
        }
    }

    pub fn excluded(&self, p: &str) -> bool {
        self.scan.as_ref().and_then(|s| s.excluded.as_ref()).map(|e| e.iter().any(|x| same_path(x, p))).unwrap_or(false)
    }

    pub fn window(paths: &[String], undo: bool) -> OpWindow {
        let items = paths.iter().enumerate().map(|(i, p)| OpItem::new(i.to_string(), p.clone(), "Game folders").detail(if undo { "scanned again" } else { "not scanned" })).collect();
        let (k, v, d) = if undo { ("UNDO DEFENDER EXCLUSIONS", "remove", "removed") } else { ("DEFENDER EXCLUSIONS", "exclude", "excluded") };
        let mut w = OpWindow::new(k, v, d, items).noun("folder", "folders");
        w.show_steps = true;
        w.go_label = if undo { "Remove exclusions".into() } else { "Exclude".into() };
        w.running_text = if undo { "Removing exclusions...".into() } else { "Adding exclusions...".into() };
        if undo {
            w.reassure = "Only the exclusions PC Tuner added are removed; Defender scans these folders again.".into();
        } else {
            w.warning = Some("Defender will not scan files in these folders. Only exclude folders that hold games from stores you trust.".into());
            w.reassure = "\"Undo\" removes exactly these exclusions again.".into();
        }
        w
    }

    fn start(&mut self, paths: Vec<String>, undo_of: Option<usize>) {
        let add = undo_of.is_none();
        self.run = Some(Run { stream: common::ps_stream(&script(&paths, add)), seen: 0, paths, undo_of });
        if let Some(op) = &mut self.op {
            op.stage = Stage::Running;
        }
    }

    /// Open the preview for excluding `todo`.
    pub fn open_exclude(&mut self, todo: Vec<String>) {
        self.op = Some(Self::window(&todo, false));
        self.pending = Some((todo, None));
    }

    pub fn has_undo(&self) -> bool {
        self.history.iter().any(|h| !h.undone && !h.added.is_empty())
    }

    pub fn open_undo(&mut self, start_now: bool) {
        let Some(i) = self.history.iter().rposition(|h| !h.undone && !h.added.is_empty()) else { return };
        let paths = self.history[i].added.clone();
        self.op = Some(Self::window(&paths, true));
        if start_now {
            self.start(paths, Some(i));
        } else {
            self.pending = Some((paths, Some(i)));
        }
    }

    /// Poll background work (the scan and a running change). Call often.
    pub fn pump(&mut self) {
        if let Some(j) = &self.loading {
            if let Some(s) = j.poll() {
                self.loading = None;
                // tick every found folder that is not excluded yet (first scan only)
                if self.scan.is_none() {
                    self.ticked = s.folders.iter().map(|f| f.path.clone()).collect();
                }
                self.scan = Some(s);
            }
        }
        if let Some(run) = &mut self.run {
            let running = run.stream.pump();
            while run.seen < run.stream.lines.len() {
                let line = run.stream.lines[run.seen].clone();
                run.seen += 1;
                if let (Some((i, r)), Some(op)) = (parse_result(&line), &mut self.op) {
                    if let Some(it) = op.items.get_mut(i) {
                        it.result = Some(r);
                    }
                }
            }
            if !running {
                let run = self.run.take().unwrap();
                let mut ok: Vec<String> = vec![];
                if let Some(op) = &mut self.op {
                    for (i, it) in op.items.iter_mut().enumerate() {
                        if it.result.is_none() {
                            it.result = Some(Err("No result - the script stopped early.".into()));
                        }
                        if matches!(it.result, Some(Ok(_))) {
                            if let Some(p) = run.paths.get(i) {
                                ok.push(p.clone());
                            }
                        }
                    }
                }
                let mut can_undo = false;
                match run.undo_of {
                    None if !ok.is_empty() => {
                        self.history.push(HistEntry { time: common::now(), added: ok, undone: false });
                        can_undo = true;
                    }
                    None => {}
                    Some(i) => {
                        if let Some(h) = self.history.get_mut(i) {
                            h.added.retain(|p| !ok.iter().any(|o| same_path(o, p)));
                            h.undone = h.added.is_empty();
                            can_undo = !h.undone;
                        }
                    }
                }
                common::save_json(HISTORY, &self.history);
                if let Some(op) = &mut self.op {
                    op.can_undo = can_undo;
                    op.done();
                }
                self.refresh();
            }
        }
    }

    /// May the operation window's buttons be pressed now?
    pub fn can_act(&self) -> bool {
        common::is_admin() && self.run.is_none()
    }

    /// React to a button of the operation window.
    pub fn on_event(&mut self, ev: OpEvent) {
        match ev {
            OpEvent::Go => {
                if let Some((paths, undo_of)) = self.pending.take() {
                    self.start(paths, undo_of);
                }
            }
            OpEvent::Undo => self.open_undo(true),
            OpEvent::Close => {
                self.op = None;
                self.pending = None;
            }
            _ => {}
        }
    }

    /// The rows of the card: (kind, path) of every found folder, then the ones the user added.
    pub fn rows(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = self.scan.iter().flat_map(|s| s.folders.iter()).map(|f| (f.kind.clone(), f.path.clone())).collect();
        for e in &self.extra {
            if !rows.iter().any(|(_, p)| same_path(p, e)) {
                rows.push(("Added by you".into(), e.clone()));
            }
        }
        rows
    }

    pub fn is_ticked(&self, path: &str) -> bool {
        self.ticked.iter().any(|t| same_path(t, path))
    }

    pub fn set_ticked(&mut self, path: &str, on: bool) {
        if on {
            self.ticked.push(path.to_string());
        } else {
            self.ticked.retain(|t| !same_path(t, path));
        }
    }

    /// "Add folder": take the typed path if it is a valid, existing folder.
    pub fn add_custom(&mut self) {
        match valid_custom(&self.custom) {
            Ok(p) if std::path::Path::new(&p).is_dir() => {
                self.custom_err = None;
                if !self.extra.iter().any(|e| same_path(e, &p)) {
                    self.extra.push(p.clone());
                }
                self.ticked.push(p);
                self.custom.clear();
            }
            Ok(_) => self.custom_err = Some("That folder does not exist."),
            Err(e) => self.custom_err = Some(e),
        }
    }

    /// Ticked folders that are not excluded yet.
    pub fn todo(&self, rows: &[(String, String)]) -> Vec<String> {
        rows.iter().map(|(_, p)| p.clone()).filter(|p| !self.excluded(p) && self.is_ticked(p)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defender_paths() {
        assert!(same_path(r"C:\XboxGames\", r"c:\xboxgames"));
        assert!(!same_path(r"C:\XboxGames", r"C:\XboxGames2"));
        assert_eq!(valid_custom(r#""D:\Games\""#), Ok(r"D:\Games".to_string()));
        assert!(valid_custom(r"D:\").is_err(), "whole drive refused");
        assert!(valid_custom(r"C:\Windows").is_err() && valid_custom(r"c:\program files").is_err());
        assert!(valid_custom(r"Games").is_err() && valid_custom(r"D:\*.exe").is_err());
        assert!(valid_custom(r"C:\Program Files\Epic Games").is_ok());
    }

    #[test]
    fn defender_script_and_results() {
        let s = script(&[r"C:\XboxGames".into(), r"D:\It's Games".into()], true);
        assert!(s.contains(r"Add-MpPreference -ExclusionPath 'C:\XboxGames'") && s.contains("'D:\\It''s Games'"));
        assert!(s.contains("RESULT|0|ok|") && s.contains("RESULT|1|ok|") && s.contains("-contains"));
        let u = script(&[r"C:\XboxGames".into()], false);
        assert!(u.contains("Remove-MpPreference -ExclusionPath 'C:\\XboxGames'") && u.contains("-notcontains"));
        assert_eq!(parse_result("RESULT|1|err|Access denied"), Some((1, Err("Access denied".into()))));
        assert_eq!(parse_result("RESULT|0|ok|"), Some((0, Ok(String::new()))));
        assert_eq!(parse_result("noise"), None);
    }
}
