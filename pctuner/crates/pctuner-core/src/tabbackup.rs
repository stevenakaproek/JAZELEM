//! Backups for the tabs that change settings: Startup Cleaner, Optimization (Performance, Network,
//! Windows tweaks), Group Policy and Processes.
//!
//! A backup is the current value of every setting the tab manages, saved as JSON in
//! Documents\PCTuner-Backups\<tab>. Restoring never writes anything itself: the tab compares the
//! backup with the PC now and puts the differing settings back through its own apply path, so a
//! restore gets the tab's usual preview window and goes into its history (undoable like any change).
//!
//! Logic only: saving, listing and comparing. Each tab turns its state into `Item`s and back.

use crate::common;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One setting's value at backup time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable key the tab uses to find the setting again.
    pub id: String,
    /// Plain-English name.
    pub title: String,
    /// The value in the tab's own words (None = not set / value absent).
    pub value: Option<String>,
    /// The value as the page shows it ("On", "Disabled", "Not configured"...).
    pub shown: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Backup {
    pub time: String,
    #[serde(default)]
    pub note: String,
    /// What the values belong to when that matters (the network adapter); empty otherwise.
    #[serde(default)]
    pub scope: String,
    pub items: Vec<Item>,
}

/// A setting that differs from the backup: what the backup has and what the PC has now.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub backup: Item,
    pub now: Item,
}

/// Documents\PCTuner-Backups\<folder> (created on demand).
pub fn dir(folder: &str) -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    home.join("Documents").join("PCTuner-Backups").join(folder)
}

/// "Group Policy" -> "group-policy" (the file name prefix).
pub fn prefix(folder: &str) -> String {
    folder.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
}

/// Save a backup into `folder`. Returns the file written.
pub fn create(folder: &str, note: &str, scope: &str, items: Vec<Item>) -> Result<PathBuf, String> {
    create_in(&dir(folder), &prefix(folder), note, scope, items)
}

pub fn create_in(dir: &std::path::Path, prefix: &str, note: &str, scope: &str, items: Vec<Item>) -> Result<PathBuf, String> {
    if items.is_empty() {
        return Err("nothing was read yet - the backup was not saved".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    // two backups in the same second: never overwrite the first
    let mut n = 1;
    while dir.join(format!("{prefix}-{stamp}.json")).exists() {
        n += 1;
        stamp = format!("{}-{n}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    }
    let b = Backup { time: common::now(), note: note.to_string(), scope: scope.to_string(), items };
    let path = dir.join(format!("{prefix}-{stamp}.json"));
    let text = serde_json::to_string_pretty(&b).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Saved backups in `folder`, newest first.
pub fn list(folder: &str) -> Vec<(PathBuf, Backup)> {
    list_in(&dir(folder))
}

pub fn list_in(dir: &std::path::Path) -> Vec<(PathBuf, Backup)> {
    let mut v: Vec<(PathBuf, Backup)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .filter_map(|p| std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Backup>(t.trim_start_matches('\u{feff}')).ok()).map(|b| (p, b)))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.time.cmp(&a.1.time).then_with(|| b.0.cmp(&a.0)));
    v
}

/// Settings whose value differs between the backup and now. Settings that exist only on one side
/// (added or removed since) are left out: there is nothing to compare them with.
pub fn differences(backup: &Backup, now: &[Item]) -> Vec<Diff> {
    backup
        .items
        .iter()
        .filter_map(|b| {
            let n = now.iter().find(|n| n.id == b.id)?;
            (n.value != b.value).then(|| Diff { backup: b.clone(), now: n.clone() })
        })
        .collect()
}

/// Settings in the PC now that the backup does not know (new since the backup).
pub fn new_since(backup: &Backup, now: &[Item]) -> usize {
    now.iter().filter(|n| !backup.items.iter().any(|b| b.id == n.id)).count()
}

pub fn item(id: impl Into<String>, title: impl Into<String>, value: Option<String>, shown: impl Into<String>) -> Item {
    Item { id: id.into(), title: title.into(), value, shown: shown.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabbackup_save_list_compare() {
        let dir = std::env::temp_dir().join(format!("pctuner-tabbackup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(create_in(&dir, "x", "", "", vec![]).is_err(), "an empty backup is refused");
        let items = vec![item("a", "A", Some("1".into()), "On"), item("b", "B", None, "Not set"), item("c", "C", Some("x".into()), "x")];
        let p1 = create_in(&dir, "group-policy", "first", "", items.clone()).unwrap();
        let p2 = create_in(&dir, "group-policy", "second", "eth", items.clone()).unwrap();
        assert_ne!(p1, p2, "a second backup in the same second gets its own file");
        let l = list_in(&dir);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1.items, items);
        assert!(l.iter().any(|(_, b)| b.scope == "eth"));

        // now: a changed, b set, c gone, d new
        let now = vec![item("a", "A", Some("0".into()), "Off"), item("b", "B", Some("5".into()), "5"), item("d", "D", None, "")];
        let d = differences(&l[0].1, &now);
        assert_eq!(d.iter().map(|d| d.backup.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(d[1].backup.value, None, "an absent value is restored as absent");
        assert_eq!(new_since(&l[0].1, &now), 1);
        assert!(differences(&l[0].1, &items).is_empty());
        assert_eq!(prefix("Group Policy"), "group-policy");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
