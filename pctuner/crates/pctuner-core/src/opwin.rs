//! The shared "operation window": every action in the app goes preview -> working -> result here.
//!
//!   Preview  what is about to change, grouped, with an optional warning banner, and the Go button
//!   Running  a sweep bar (or a step list ticking off when `show_steps` is on)
//!   Done     rows reveal one by one with a counter, problems first, then chips, notes and buttons
//!
//! A tab keeps `Option<OpWindow>`, has the UI draw it every frame and reacts to the returned `OpEvent`:
//! on `Go` it starts its background job (the window is already in the Running stage), and when the
//! job ends it writes each item's `result` and calls `finish()`.
//!
//! This is the data model only; each UI draws it its own way.

use crate::common::Tone;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    Preview,
    Running,
    Done,
}

pub struct OpItem {
    pub id: String,
    pub name: String,
    /// Group label used for the preview sections and the result chips ("Scheduled tasks", "Services", ...).
    pub group: String,
    /// Short extra text shown dim after the name, e.g. "Automatic -> Disabled".
    pub detail: String,
    pub optional: bool,
    /// None = not done yet. Ok(note) = worked (note may be empty). Err(reason) = failed.
    pub result: Option<Result<String, String>>,
}

impl OpItem {
    pub fn new(id: impl Into<String>, name: impl Into<String>, group: impl Into<String>) -> Self {
        OpItem { id: id.into(), name: name.into(), group: group.into(), detail: String::new(), optional: false, result: None }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    pub fn optional(mut self, o: bool) -> Self {
        self.optional = o;
        self
    }
    pub fn failed(&self) -> bool {
        matches!(self.result, Some(Err(_)))
    }
    pub fn ok(&self) -> bool {
        matches!(self.result, Some(Ok(_)))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OpEvent {
    /// "Go" was clicked in the preview. The window has switched itself to Running.
    Go,
    Close,
    /// "Undo" in the result stage.
    Undo,
    /// One of the `extras` buttons in the result stage (index).
    Extra(usize),
}

pub struct OpWindow {
    /// Small caption at the top ("CLEAN JUNK", "SERVICES").
    pub kicker: String,
    /// Words used to build the headlines: "Ready to {verb} {n} {noun}", "{n} {noun} {done_word}".
    pub verb: String,
    pub done_word: String,
    pub noun: (String, String),
    /// Line under the preview headline.
    pub preview_text: String,
    /// Headline while working ("Cleaning...").
    pub running_text: String,
    /// Yellow banner in the preview (risk the user should read before continuing).
    pub warning: Option<String>,
    /// Reassurance line above the buttons in the preview.
    pub reassure: String,
    pub go_label: String,
    pub stage: Stage,
    pub items: Vec<OpItem>,
    /// Running stage: list the items as steps and tick them off as their results arrive.
    pub show_steps: bool,
    /// Result stage: plain lines under the chips ("Sign out or restart Windows to finish.").
    pub notes: Vec<String>,
    /// Result stage: one coloured line under the chips (e.g. "Junk still switched on: 0").
    pub footer: Option<(String, Tone)>,
    /// Result stage: extra buttons; clicking one returns `OpEvent::Extra(index)`.
    pub extras: Vec<String>,
    /// Result stage: show "Undo" (label in `undo_label`).
    pub can_undo: bool,
    pub undo_label: String,
    /// Shown when the window is opened with nothing to do.
    pub empty_title: String,
    pub empty_text: String,
    /// False when the empty state is a refusal (e.g. every selected item is locked): no green tick.
    pub empty_ok: bool,
    /// Wording for failures in the result headline ("could not be changed", "failed").
    pub fail_phrase: String,
    /// Running stage: bar position 0..=1 for script-driven work (overrides results / items).
    pub progress: Option<f32>,
    /// UI clock time at which the result stage was first drawn (drives the row-by-row reveal).
    pub t_done: Option<f64>,
}

const REVEAL_PER_SEC: f64 = 20.0;

/// UI time (f64 bits) when a window was last drawn in the Running stage; u64::MAX = never.
static RUNNING_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// True when an operation window was in the Running stage within the last half second, i.e. a
/// change is being applied right now (closing PC Tuner waits for it).
pub fn running_recently(now: f64) -> bool {
    let bits = RUNNING_AT.load(std::sync::atomic::Ordering::Relaxed);
    bits != u64::MAX && ran_within(f64::from_bits(bits), now)
}

/// The UI calls this each time it draws a window in the Running stage (`now` = its clock, in seconds).
pub fn mark_running(now: f64) {
    RUNNING_AT.store(now.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

fn ran_within(at: f64, now: f64) -> bool {
    (now - at).abs() < 0.5
}

/// How many result rows are visible `elapsed` seconds into the reveal.
pub fn revealed(count: usize, elapsed: f64) -> usize {
    ((elapsed.max(0.0) * REVEAL_PER_SEC) as usize + 1).min(count)
}

/// (group, items in group, failed in group) in order of first appearance.
pub fn group_counts(items: &[OpItem]) -> Vec<(String, usize, usize)> {
    let mut out: Vec<(String, usize, usize)> = vec![];
    for it in items {
        match out.iter_mut().find(|g| g.0 == it.group) {
            Some(g) => {
                g.1 += 1;
                g.2 += it.failed() as usize;
            }
            None => out.push((it.group.clone(), 1, it.failed() as usize)),
        }
    }
    out
}

pub fn plural<'a>(n: usize, noun: &'a (String, String)) -> &'a str {
    if n == 1 {
        &noun.0
    } else {
        &noun.1
    }
}

impl OpWindow {
    /// `verb`/`done_word` e.g. ("clean", "cleaned"), ("change", "changed"), ("undo", "undone").
    pub fn new(kicker: &str, verb: &str, done_word: &str, items: Vec<OpItem>) -> Self {
        let stage = if items.is_empty() { Stage::Done } else { Stage::Preview };
        OpWindow {
            kicker: kicker.to_string(),
            verb: verb.to_string(),
            done_word: done_word.to_string(),
            noun: ("item".into(), "items".into()),
            preview_text: String::new(),
            running_text: "Working...".into(),
            warning: None,
            reassure: String::new(),
            go_label: "Apply".into(),
            stage,
            items,
            show_steps: false,
            notes: vec![],
            footer: None,
            extras: vec![],
            can_undo: false,
            undo_label: "Undo".into(),
            empty_title: "Nothing to do".into(),
            empty_text: "Everything is already the way you asked.".into(),
            empty_ok: true,
            fail_phrase: "could not be changed".into(),
            progress: None,
            t_done: None,
        }
    }

    pub fn noun(mut self, one: &str, many: &str) -> Self {
        self.noun = (one.into(), many.into());
        self
    }

    /// Fill each item's result from `f` (items `f` returns None for count as done without a note)
    /// and switch to the result stage.
    pub fn finish(&mut self, mut f: impl FnMut(&OpItem) -> Option<Result<String, String>>) {
        for it in self.items.iter_mut() {
            let r = f(it).unwrap_or(Ok(String::new()));
            it.result = Some(r);
        }
        self.stage = Stage::Done;
        self.t_done = None;
    }

    /// Go straight to the result stage with the results already on the items.
    pub fn done(&mut self) {
        self.stage = Stage::Done;
        self.t_done = None;
    }

    pub fn ok_count(&self) -> usize {
        self.items.iter().filter(|i| i.ok()).count()
    }
}

/// Demo hook for screenshots: `PCTUNER_DEMO=<tab>:<preview|running|done>` (nothing on the PC is touched).
pub fn demo_stage(tab: &str) -> Option<Stage> {
    let v = std::env::var("PCTUNER_DEMO").ok()?;
    let (t, s) = v.split_once(':').unwrap_or((v.as_str(), "done"));
    if !t.eq_ignore_ascii_case(tab) {
        return None;
    }
    Some(match s {
        "preview" => Stage::Preview,
        "running" => Stage::Running,
        _ => Stage::Done,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opwin_reveal_and_groups() {
        assert_eq!(revealed(0, 5.0), 0);
        assert_eq!(revealed(24, 0.0), 1);
        assert_eq!(revealed(24, 0.5), 11);
        assert_eq!(revealed(24, 60.0), 24);
        assert_eq!(revealed(24, -1.0), 1);

        let mut items = vec![
            OpItem::new("a", "A", "Tasks"),
            OpItem::new("b", "B", "Settings"),
            OpItem::new("c", "C", "Tasks").detail("Automatic -> Disabled"),
        ];
        items[1].result = Some(Err("blocked".into()));
        let mut w = OpWindow::new("TEST", "clean", "cleaned", items);
        assert_eq!(w.stage, Stage::Preview);
        assert_eq!(group_counts(&w.items), vec![("Tasks".to_string(), 2, 0), ("Settings".to_string(), 1, 1)]);
        w.finish(|it| if it.id == "b" { Some(Err("blocked".into())) } else { None });
        assert_eq!(w.stage, Stage::Done);
        assert_eq!(w.ok_count(), 2);
        // an empty operation opens straight in the "nothing to do" result state
        assert_eq!(OpWindow::new("TEST", "clean", "cleaned", vec![]).stage, Stage::Done);
        assert_eq!(plural(1, &w.noun), "item");
        assert_eq!(plural(2, &w.noun), "items");
    }

    #[test]
    fn opwin_running_window_counts_as_busy() {
        assert!(ran_within(10.0, 10.2));
        assert!(!ran_within(10.0, 11.0));
        // a new app run starts its UI clock at 0 again
        assert!(!ran_within(10.0, 0.0));
    }
}
