//! The Backups sheet shared by Startup Cleaner, Optimization (each part), Group Policy and Processes.
//! Saving, listing and comparing is pctuner_core::tabbackup; each tab supplies its settings as items
//! and puts a chosen backup back through its own apply path.

use pctuner_core::common;
use pctuner_core::tabbackup::{self, differences, new_since, Backup, Item};
use serde_json::{json, Value};
use std::path::PathBuf;

/// The sheet's buttons (the same action names on every tab).
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackupAction {
    Backups,
    BackupsClose,
    BackupCreate,
    BackupFolder,
    BackupRestore { index: usize },
    BackupDelete { index: usize },
}

/// A Backups action, if this is one (tabs check this before their own actions).
pub fn action(a: &Value) -> Option<BackupAction> {
    serde_json::from_value(a.clone()).ok()
}

pub struct Backups {
    folder: &'static str,
    about: &'static str,
    pub open: bool,
    list: Vec<(PathBuf, Backup)>,
    /// (true = good news, text)
    pub msg: Option<(bool, String)>,
}

impl Backups {
    pub fn new(folder: &'static str, about: &'static str) -> Self {
        Backups { folder, about, open: false, list: vec![], msg: None }
    }

    /// Handle a sheet button. `now` gives (scope, the settings now) when a backup is made; None = not read
    /// yet. Returns the backup to restore when Restore was clicked; the tab then puts it back and calls
    /// `restored` with what happened.
    pub fn act(&mut self, a: BackupAction, now: Option<(String, Vec<Item>)>) -> Option<Backup> {
        match a {
            BackupAction::Backups => {
                self.list = tabbackup::list(self.folder);
                self.msg = None;
                self.open = true;
            }
            BackupAction::BackupsClose => self.open = false,
            BackupAction::BackupCreate => {
                self.msg = Some(match now {
                    None => (false, "The settings are still being read - try again in a moment.".into()),
                    Some((scope, items)) => match tabbackup::create(self.folder, "", &scope, items) {
                        Ok(p) => (true, format!("Backup saved: {}", p.file_name().and_then(|f| f.to_str()).unwrap_or(""))),
                        Err(e) => (false, format!("Backup failed: {e}")),
                    },
                });
                self.list = tabbackup::list(self.folder);
            }
            BackupAction::BackupFolder => {
                let d = tabbackup::dir(self.folder);
                let _ = std::fs::create_dir_all(&d);
                let _ = common::run("explorer.exe", &[&d.display().to_string()]);
            }
            BackupAction::BackupRestore { index } => return self.list.get(index).map(|(_, b)| b.clone()),
            BackupAction::BackupDelete { index } => {
                if let Some((path, b)) = self.list.get(index).cloned() {
                    self.msg = Some(match tabbackup::delete(&path) {
                        Ok(()) => (true, format!("Backup from {} moved to the Recycle Bin.", b.time)),
                        Err(e) => (false, format!("Could not delete the backup from {}: {e}", b.time)),
                    });
                    self.list = tabbackup::list(self.folder);
                }
            }
        }
        None
    }

    /// After a restore was handed to the tab: Ok = its preview window is open (the sheet closes so it
    /// shows), Err = why nothing was opened (shown in the sheet).
    pub fn restored(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => self.open = false,
            Err(e) => self.msg = Some((false, e)),
        }
    }

    /// The sheet (null when closed). `now` = the settings now (None while reading), `can_restore` = the
    /// tab could apply a change right now.
    pub fn view(&self, now: Option<&[Item]>, can_restore: bool) -> Value {
        if !self.open {
            return Value::Null;
        }
        let list: Vec<Value> = self
            .list
            .iter()
            .enumerate()
            .map(|(index, (_, b))| {
                let (diffs, new) = match now {
                    Some(n) => (differences(b, n), new_since(b, n)),
                    None => (vec![], 0),
                };
                json!({
                    "index": index,
                    "time": b.time,
                    "detail": format!("{} settings{}{}", b.items.len(), if b.scope.is_empty() { String::new() } else { format!(" · {}", b.scope) }, if b.note.is_empty() { String::new() } else { format!(" - {}", b.note) }),
                    "known": now.is_some(),
                    "diffs": diffs.iter().map(|d| json!({ "title": d.backup.title, "now": d.now.shown, "backup": d.backup.shown })).collect::<Vec<_>>(),
                    "newSince": new,
                    "canRestore": can_restore && !diffs.is_empty(),
                })
            })
            .collect();
        json!({
            "about": self.about,
            "folder": tabbackup::dir(self.folder).display().to_string(),
            "msg": self.msg.as_ref().map(|(ok, t)| json!({ "ok": ok, "text": t })),
            "canCreate": now.is_some(),
            "admin": common::is_admin(),
            "list": list,
        })
    }
}
