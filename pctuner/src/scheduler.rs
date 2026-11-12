//! CPU Scheduler tab - Win32PrioritySeparation presets (port of TweakScheduler.ps1).

use crate::common::{self, Job, Tab};
use crate::opwin::{self, OpEvent, OpItem, OpWindow, Stage};
use crate::theme;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use winreg::enums::*;
use winreg::RegKey;

const KEY: &str = r"SYSTEM\CurrentControlSet\Control\PriorityControl";
const VALUE: &str = "Win32PrioritySeparation";
const HISTORY: &str = "scheduler-history.json";
const RESTART_NOTE: &str = "Restart the PC to be certain it is in effect.";

struct Preset {
    name: &'static str,
    tag: &'static str,
    value: u32,
    text: &'static str,
}

const PRESETS: &[Preset] = &[
    Preset { name: "Windows default", tag: "recommended for Halo Infinite", value: 0x02,
        text: "What Windows ships with: the game in focus already gets turns 3x longer than background apps plus the maximum wake-up boost. No controlled benchmark shows any other value beating it." },
    Preset { name: "Game", tag: "community favourite", value: 0x2A,
        text: "The value most gaming guides call the \"game\" setting. The game gets exactly what the default gives it; background apps get equally long turns. No measured benefit - an A/B test only." },
    Preset { name: "Long turns", tag: "alternate to test", value: 0x16,
        text: "Keeps the 3:1 advantage and the wake-up boost but doubles every turn. Sound in theory, unmeasured in practice." },
];

#[derive(Serialize, Deserialize, Clone)]
struct HistEntry {
    time: String,
    from: u32,
    to: u32,
    undone: bool,
}

pub struct Decoded {
    pub interval: &'static str,
    pub kind: &'static str,
    pub boost: u32,
    pub ratio: String,
    pub fg_ms: f64,
    pub bg_ms: f64,
    pub signature: (bool, bool, u32),
}

/// Bits 5-4 interval (1 long, 2 short, 0/3 default=short), bits 3-2 type (1 variable, 2 fixed, 0/3 default=variable),
/// bits 1-0 foreground boost (3 = 2). Quantum units: 3 units = one 15.625 ms tick.
pub fn decode(value: u32) -> Decoded {
    let v = value & 0x3F;
    let i = (v >> 4) & 3;
    let t = (v >> 2) & 3;
    let mut b = v & 3;
    if b == 3 {
        b = 2;
    }
    let long = i == 1;
    let fixed = t == 2;
    let table: [f64; 3] = match (fixed, long) {
        (true, true) => [36.0, 36.0, 36.0],
        (true, false) => [18.0, 18.0, 18.0],
        (false, true) => [12.0, 24.0, 36.0],
        (false, false) => [6.0, 12.0, 18.0],
    };
    let ms = |u: f64| (u / 3.0 * 15.625 * 10.0).round() / 10.0;
    Decoded {
        interval: match i { 1 => "Long", 2 => "Short", _ => "Short (Windows default)" },
        kind: match t { 2 => "Fixed", 1 => "Variable", _ => "Variable (Windows default)" },
        boost: b,
        ratio: if fixed { "1:1 (equal turns)".into() } else { format!("{}:1", b + 1) },
        fg_ms: ms(table[b as usize]),
        bg_ms: ms(table[0]),
        signature: (long, fixed, b),
    }
}

fn read_value() -> u32 {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(KEY)
        .and_then(|k| k.get_value::<u32, _>(VALUE))
        .unwrap_or(2)
}

fn write_value(v: u32) -> Result<(), String> {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(KEY, KEY_SET_VALUE)
        .map_err(|e| format!("cannot open the registry key (run as administrator): {e}"))?;
    k.set_value(VALUE, &v).map_err(|e| e.to_string())
}

/// Preset whose schedule matches `value` (same interval, type and boost).
fn preset_of(value: u32) -> Option<&'static Preset> {
    let sig = decode(value).signature;
    PRESETS.iter().find(|p| decode(p.value).signature == sig)
}

/// Index of the preset whose schedule matches `value` (0x26 selects the same card as 0x02).
fn preset_index(value: u32) -> Option<usize> {
    let sig = decode(value).signature;
    PRESETS.iter().position(|p| decode(p.value).signature == sig)
}

/// History bookkeeping once a write is back: a successful change adds an entry, a successful
/// undo marks its entry undone, and a failed write leaves the history as it was (so a failed
/// undo can be retried). Returns true when the history changed and must be saved.
fn apply_result(history: &mut Vec<HistEntry>, from: u32, to: u32, undo_of: Option<usize>, ok: bool, time: String) -> bool {
    if !ok {
        return false;
    }
    match undo_of {
        None => {
            history.push(HistEntry { time, from, to, undone: false });
            true
        }
        Some(i) => match history.get_mut(i) {
            Some(h) => {
                h.undone = true;
                true
            }
            None => false,
        },
    }
}

fn label_of(value: u32) -> String {
    preset_of(value).map(|p| p.name.to_string()).unwrap_or_else(|| format!("Custom value 0x{value:02X}"))
}

/// The one-row operation window for a change `from` -> `to`.
fn op_window(from: u32, to: u32, undo: bool) -> OpWindow {
    let item = OpItem::new("sep", label_of(to), "Registry").detail(format!("0x{from:02X} -> 0x{to:02X}"));
    let (verb, done_word) = if undo { ("undo", "undone") } else { ("change", "changed") };
    let mut w = OpWindow::new("CPU SCHEDULER", verb, done_word, vec![item]).noun("scheduler setting", "scheduler settings");
    w.preview_text = "Win32PrioritySeparation: how Windows shares CPU time between the game and background apps.".into();
    w.running_text = "Writing the registry value...".into();
    w.go_label = if undo { "Undo".into() } else { "Apply".into() };
    if undo {
        w.reassure = "Puts back the value that was set before your last change.".into();
    } else {
        w.reassure = "\"Undo last change\" puts the previous value back.".into();
        if preset_of(to).map(|p| p.value != PRESETS[0].value).unwrap_or(true) {
            w.warning = Some("No controlled benchmark shows this beating the Windows default. Treat it as an A/B test.".into());
        }
    }
    w
}

/// A registry write in progress (shown as the Running stage).
struct Run {
    job: Job<Result<(), String>>,
    result: Option<Result<(), String>>,
    from: u32,
    to: u32,
    /// Some(index into history) when this run undoes that entry.
    undo_of: Option<usize>,
    t0: f64,
}

pub struct SchedulerTab {
    current: u32,
    selected: Option<usize>,
    history: Vec<HistEntry>,
    op: Option<OpWindow>,
    /// (from, to, history index when it is an undo) of the window that is open in the preview stage.
    pending: Option<(u32, u32, Option<usize>)>,
    run: Option<Run>,
    demo: bool,
}

impl SchedulerTab {
    pub fn new() -> Self {
        let current = read_value();
        let selected = preset_index(current);
        let mut tab = SchedulerTab { current, selected, history: common::load_json(HISTORY), op: None, pending: None, run: None, demo: false };
        if let Some(stage) = opwin::demo_stage("scheduler") {
            // sample window for screenshots - nothing is written
            let mut w = op_window(0x02, 0x2A, false);
            match stage {
                Stage::Preview => {}
                Stage::Running => w.stage = Stage::Running,
                Stage::Done => {
                    w.can_undo = true;
                    w.undo_label = "Undo".into();
                    w.finish(|_| Some(Ok(RESTART_NOTE.to_string())));
                }
            }
            tab.op = Some(w);
            tab.demo = true;
        }
        tab
    }

    fn start(&mut self, from: u32, to: u32, undo_of: Option<usize>, now: f64) {
        self.run = Some(Run { job: Job::spawn(move || write_value(to)), result: None, from, to, undo_of, t0: now });
    }

    /// Open the undo window for the last change that has not been undone.
    fn undo_window(&mut self) -> bool {
        let Some(i) = self.history.iter().rposition(|h| !h.undone) else { return false };
        let to = self.history[i].from;
        self.pending = Some((self.current, to, Some(i)));
        self.op = Some(op_window(self.current, to, true));
        true
    }

    /// Poll the registry write; when it is back (and the window had a moment to show it) fill in the result.
    fn pump(&mut self, now: f64) {
        let Some(run) = &mut self.run else { return };
        if run.result.is_none() {
            run.result = run.job.poll();
        }
        if run.result.is_none() || now - run.t0 < 0.6 {
            return;
        }
        let run = self.run.take().unwrap();
        let result = run.result.unwrap();
        if apply_result(&mut self.history, run.from, run.to, run.undo_of, result.is_ok(), common::now()) {
            common::save_json(HISTORY, &self.history);
        }
        // re-read after a failure too, so `current` and the next window's "from" match the registry
        self.current = read_value();
        self.selected = preset_index(self.current);
        if let Some(op) = &mut self.op {
            let failed_undo = result.is_err() && run.undo_of.is_some();
            op.can_undo = (result.is_ok() && run.undo_of.is_none()) || failed_undo;
            if failed_undo {
                // the entry is still not undone, so the Undo event picks the same one again
                op.undo_label = "Retry undo".into();
            }
            op.finish(|_| Some(match &result {
                Ok(()) => Ok(RESTART_NOTE.to_string()),
                Err(e) => Err(format!("Could not change it: {e}")),
            }));
        }
    }

    fn window(&mut self, ctx: &egui::Context, now: f64) {
        let Some(op) = &mut self.op else { return };
        let can = common::is_admin() && self.run.is_none() && !self.demo;
        match op.show(ctx, can) {
            Some(OpEvent::Go) => {
                if let Some((from, to, undo_of)) = self.pending.take() {
                    self.start(from, to, undo_of, now);
                }
            }
            Some(OpEvent::Undo) => {
                if self.undo_window() {
                    if let (Some(op), Some((from, to, undo_of))) = (&mut self.op, self.pending.take()) {
                        op.stage = Stage::Running;
                        self.start(from, to, undo_of, now);
                    }
                }
            }
            Some(OpEvent::Close) => {
                self.op = None;
                self.pending = None;
            }
            _ => {}
        }
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

/// Small number + label pair used inside the preset cards.
fn mini(ui: &mut egui::Ui, value: &str, label: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(value).size(16.0).strong().color(theme::TEXT));
        ui.label(RichText::new(label).size(11.5).color(theme::TEXT_FAINT));
    });
}

/// One selectable preset card. Returns true when it was clicked.
fn preset_card(ui: &mut egui::Ui, index: usize, p: &Preset, selected: bool, is_current: bool) -> bool {
    let id = ui.id().with(("preset", index));
    let hovered = ui.ctx().read_response(id).map(|r| r.hovered()).unwrap_or(false);
    let hover = ui.ctx().animate_bool_with_time(id.with("hover"), hovered, 0.12);
    let sel = ui.ctx().animate_bool_with_time(id.with("sel"), selected, 0.14);
    let border = theme::BORDER.lerp_to_gamma(theme::TEXT_FAINT, hover).lerp_to_gamma(theme::ACCENT, sel);
    let fill = theme::SURFACE.lerp_to_gamma(theme::SURFACE2, 0.5 * hover).lerp_to_gamma(theme::ACCENT, 0.07 * sel);
    let d = decode(p.value);
    let inner = theme::card_frame().fill(fill).stroke(egui::Stroke::new(1.0 + 0.5 * sel, border)).show(ui, |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        ui.set_width(ui.available_width());
        ui.set_min_height(250.0);
        // the columns are justified layouts, which would stretch the words of wrapped text
        ui.vertical(|ui| {
        ui.horizontal(|ui| {
            theme::section(ui, &format!("Preset {}", index + 1));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_current {
                    theme::chip(ui, "current", theme::ACCENT);
                }
                if index == 0 {
                    theme::chip(ui, "recommended", theme::ACCENT);
                }
            });
        });
        ui.label(RichText::new(p.name).size(20.0).strong().color(if selected { theme::ACCENT } else { theme::TEXT }));
        ui.label(RichText::new(p.tag).color(theme::TEXT_DIM));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            mini(ui, &format!("{} ms", d.fg_ms), "game turn");
            mini(ui, &format!("{} ms", d.bg_ms), "background turn");
            mini(ui, &format!("+{}", d.boost), "boost");
        });
        ui.add_space(6.0);
        ui.label(RichText::new(p.text).size(13.0).color(theme::TEXT_DIM));
        ui.add_space(2.0);
        ui.label(RichText::new(format!("0x{:02X}  ·  {}, {}", p.value, d.interval.split(" (").next().unwrap_or(d.interval), d.kind.split(" (").next().unwrap_or(d.kind))).size(12.0).color(theme::TEXT_FAINT));
        });
    });
    ui.interact(inner.response.rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

impl Tab for SchedulerTab {
    fn title(&self) -> &'static str {
        "CPU Scheduler"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        self.pump(now);
        self.window(ui.ctx(), now);

        theme::page_header(ui, "CPU Scheduler", "How long a thread may run before Windows lets another in, and how much the game in focus is favoured.");
        common::admin_banner(ui);
        egui::ScrollArea::vertical().id_salt("scheduler-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui));
    }
}

impl SchedulerTab {
    fn page(&mut self, ui: &mut egui::Ui) {

        // ---- hero: the schedule that is active now
        let d = decode(self.current);
        let matched = preset_of(self.current);
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::hero(
                    ui,
                    matched.map(|p| p.name).unwrap_or("Custom"),
                    "current schedule",
                    &format!("value 0x{:02X}  ·  game : background = {}", self.current, d.ratio),
                    if matched.is_some() { theme::ACCENT } else { theme::JUNK },
                );
                stat_fit(ui, 30.0, &format!("{} ms", d.fg_ms), "game turn", theme::TEXT);
                stat_fit(ui, 22.0, &format!("{} ms", d.bg_ms), "background turn", theme::TEXT);
                stat_fit(ui, 22.0, &if d.boost == 0 { "none".to_string() } else { format!("+{}", d.boost) }, "foreground boost", theme::TEXT);
            });
        });
        ui.add_space(6.0);

        // ---- presets
        theme::section(ui, "Presets");
        let current_sig = d.signature;
        ui.columns(PRESETS.len(), |cols| {
            for (i, p) in PRESETS.iter().enumerate() {
                let is_current = decode(p.value).signature == current_sig;
                if preset_card(&mut cols[i], i, p, self.selected == Some(i), is_current) {
                    self.selected = Some(i);
                }
            }
        });
        ui.add_space(6.0);

        // ---- actions
        ui.horizontal(|ui| {
            let can = common::is_admin() && self.run.is_none();
            if ui.add_enabled(can && self.selected.is_some(), theme::primary_button("   Apply selected preset   ")).clicked() {
                let p = &PRESETS[self.selected.unwrap()];
                if decode(p.value).signature == decode(self.current).signature {
                    // nothing to write (same schedule, maybe stored as another value): the window opens in its "nothing to do" state
                    let mut w = OpWindow::new("CPU SCHEDULER", "change", "changed", vec![]);
                    w.empty_title = "Already set".into();
                    w.empty_text = if p.value == self.current {
                        format!("The scheduler is already on {} (0x{:02X}).", p.name, p.value)
                    } else {
                        format!("The scheduler is already on {} (stored as 0x{:02X}, the same schedule as 0x{:02X}).", p.name, self.current, p.value)
                    };
                    self.pending = None;
                    self.op = Some(w);
                } else {
                    self.pending = Some((self.current, p.value, None));
                    self.op = Some(op_window(self.current, p.value, false));
                }
            }
            let has_undo = self.history.iter().any(|h| !h.undone);
            if ui.add_enabled(can && has_undo, egui::Button::new("Undo last change")).clicked() {
                self.undo_window();
            }
            if ui.button("Refresh").clicked() {
                self.current = read_value();
                self.selected = preset_index(self.current);
            }
        });
        ui.add_space(2.0);
        ui.label(RichText::new("Turn lengths are nominal and only matter when threads share a core. This is the same value Bitsum TweakScheduler edits - use one tool or the other.").size(12.5).color(theme::TEXT_DIM));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_known_values() {
        // (value, game turn ms, background turn ms, (long, fixed, boost), ratio)
        let cases: &[(u32, f64, f64, (bool, bool, u32), &str)] = &[
            (0x02, 93.8, 31.3, (false, false, 2), "3:1"),
            (0x26, 93.8, 31.3, (false, false, 2), "3:1"),
            (0x2A, 93.8, 93.8, (false, true, 2), "1:1 (equal turns)"),
            (0x16, 187.5, 62.5, (true, false, 2), "3:1"),
            (0x00, 31.3, 31.3, (false, false, 0), "1:1"),
            (0x3F, 93.8, 31.3, (false, false, 2), "3:1"),
        ];
        for &(v, fg, bg, sig, ratio) in cases {
            let d = decode(v);
            assert_eq!(d.fg_ms, fg, "game turn 0x{v:02X}");
            assert_eq!(d.bg_ms, bg, "background turn 0x{v:02X}");
            assert_eq!(d.signature, sig, "signature 0x{v:02X}");
            assert_eq!(d.boost, sig.2, "boost 0x{v:02X}");
            assert_eq!(d.ratio, ratio, "ratio 0x{v:02X}");
        }
        assert_eq!(decode(0x2A).kind, "Fixed");
        assert_eq!(decode(0x16).interval, "Long");
        assert_eq!(decode(0x26).interval, "Short");
    }

    #[test]
    fn preset_matching_by_schedule() {
        for (i, p) in PRESETS.iter().enumerate() {
            assert_eq!(preset_index(p.value), Some(i));
            assert_eq!(preset_of(p.value).map(|q| q.value), Some(p.value));
        }
        // 0x26 and 0x3F store the default schedule in other bits
        assert_eq!(preset_index(0x26), Some(0));
        assert_eq!(preset_index(0x3F), Some(0));
        assert_eq!(label_of(0x26), "Windows default");
        // long + fixed + no boost matches no preset
        assert_eq!(preset_index(0x18), None);
        assert!(preset_of(0x18).is_none());
        assert_eq!(label_of(0x18), "Custom value 0x18");
    }

    #[test]
    fn history_rules() {
        let mut h = Vec::new();
        // a failed change records nothing
        assert!(!apply_result(&mut h, 0x02, 0x16, None, false, "t".into()));
        assert!(h.is_empty());
        // successful changes are recorded
        assert!(apply_result(&mut h, 0x02, 0x16, None, true, "t".into()));
        assert!(apply_result(&mut h, 0x16, 0x2A, None, true, "t".into()));
        assert_eq!(h.len(), 2);
        // a failed undo keeps the entry as not undone, so the next undo picks it again
        assert!(!apply_result(&mut h, 0x2A, 0x16, Some(1), false, "t".into()));
        assert!(!h[1].undone);
        assert_eq!(h.iter().rposition(|e| !e.undone), Some(1));
        // a successful undo marks it; the next undo goes to the older change
        assert!(apply_result(&mut h, 0x2A, 0x16, Some(1), true, "t".into()));
        assert!(h[1].undone && !h[0].undone);
        assert_eq!(h.iter().rposition(|e| !e.undone), Some(0));
        // an index that is not in the history changes nothing
        assert!(!apply_result(&mut h, 0x16, 0x02, Some(9), true, "t".into()));
        assert_eq!(h.len(), 2);
    }
}
