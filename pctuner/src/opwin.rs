//! The shared "operation window": every action in the app goes preview -> working -> result here.
//!
//!   Preview  what is about to change, grouped, with an optional warning banner, and the Go button
//!   Running  a sweep bar (or a step list ticking off when `show_steps` is on)
//!   Done     rows reveal one by one with a counter, problems first, then chips, notes and buttons
//!
//! A tab keeps `Option<OpWindow>`, calls `show()` every frame and reacts to the returned `OpEvent`:
//! on `Go` it starts its background job (the window is already in the Running stage), and when the
//! job ends it writes each item's `result` and calls `finish()`.

use crate::theme;
use eframe::egui::{self, RichText};

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
    fn failed(&self) -> bool {
        matches!(self.result, Some(Err(_)))
    }
    fn ok(&self) -> bool {
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
    pub footer: Option<(String, egui::Color32)>,
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
    t_done: Option<f64>,
}

const REVEAL_PER_SEC: f64 = 20.0;

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

fn plural<'a>(n: usize, noun: &'a (String, String)) -> &'a str {
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

    /// Draw the window. `can` = the user may press Go / Undo / extras (admin and not busy).
    pub fn show(&mut self, ctx: &egui::Context, can: bool) -> Option<OpEvent> {
        let now = ctx.input(|i| i.time);
        let mut ev: Option<OpEvent> = None;
        theme::dim_backdrop(ctx, "opwin");

        egui::Window::new("operation-window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                const W: f32 = 580.0;
                ui.set_width(W);
                theme::section(ui, &self.kicker);
                let n = self.items.len();
                match self.stage {
                    Stage::Preview => {
                        ui.label(RichText::new(format!("Ready to {} {n} {}", self.verb, plural(n, &self.noun))).size(26.0).strong());
                        if !self.preview_text.is_empty() {
                            ui.label(RichText::new(&self.preview_text).color(theme::TEXT_DIM));
                        }
                        ui.add_space(6.0);
                        if let Some(w) = &self.warning {
                            theme::banner(ui, theme::GAMING, w);
                            ui.add_space(4.0);
                        }
                        let groups = group_counts(&self.items);
                        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                            ui.set_width(W - 12.0);
                            let single = groups.len() == 1 && n <= 8;
                            for (label, total, _) in &groups {
                                let body = |ui: &mut egui::Ui| {
                                    for it in self.items.iter().filter(|i| &i.group == label) {
                                        ui.horizontal(|ui| {
                                            ui.colored_label(if it.optional { theme::OPTIONAL } else { theme::ACCENT }, "•");
                                            ui.label(&it.name);
                                            if !it.detail.is_empty() {
                                                ui.label(RichText::new(&it.detail).color(theme::TEXT_DIM));
                                            }
                                            if it.optional {
                                                ui.label(RichText::new("(optional)").color(theme::TEXT_FAINT));
                                            }
                                        });
                                    }
                                };
                                if single {
                                    // few items of one kind: just list them
                                    body(ui);
                                } else {
                                    // show the names straight away unless the list is very long
                                    egui::CollapsingHeader::new(RichText::new(format!("{label}    {total}")).strong()).id_salt(label).default_open(groups.len() == 1 || n <= 30).show(ui, body);
                                }
                            }
                        });
                        ui.add_space(6.0);
                        if !self.reassure.is_empty() {
                            ui.label(RichText::new(&self.reassure).color(theme::TEXT_DIM));
                        }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(can, theme::primary_button(&format!("   {}   ", self.go_label))).clicked() {
                                ev = Some(OpEvent::Go);
                            }
                            if ui.button("Cancel").clicked() {
                                ev = Some(OpEvent::Close);
                            }
                            if !crate::common::is_admin() {
                                ui.colored_label(theme::LOCKED, "View only - start PC Tuner as administrator.");
                            }
                        });
                    }
                    Stage::Running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(&self.running_text).size(26.0).strong());
                        });
                        let done = self.items.iter().filter(|i| i.result.is_some()).count();
                        if self.show_steps {
                            ui.add_space(4.0);
                            theme::progress(ui, self.progress.unwrap_or(done as f32 / n.max(1) as f32), W);
                            ui.add_space(6.0);
                            let mut current_marked = false;
                            egui::ScrollArea::vertical().id_salt("op-steps").max_height(260.0).show(ui, |ui| {
                            ui.set_width(W - 12.0);
                            for it in &self.items {
                                ui.horizontal(|ui| {
                                    match &it.result {
                                        Some(Ok(_)) => {
                                            ui.colored_label(theme::OK, RichText::new("✔").strong());
                                            ui.label(&it.name);
                                        }
                                        Some(Err(_)) => {
                                            ui.colored_label(theme::LOCKED, RichText::new("✖").strong());
                                            ui.label(&it.name);
                                        }
                                        None if !current_marked => {
                                            current_marked = true;
                                            ui.add(egui::Spinner::new().size(13.0));
                                            ui.label(RichText::new(&it.name).strong());
                                        }
                                        None => {
                                            ui.colored_label(theme::TEXT_FAINT, "•");
                                            ui.label(RichText::new(&it.name).color(theme::TEXT_FAINT));
                                        }
                                    }
                                    if !it.detail.is_empty() {
                                        ui.label(RichText::new(&it.detail).color(theme::TEXT_FAINT).small());
                                    }
                                });
                            }
                            });
                        } else {
                            ui.label(RichText::new(format!("{n} {}", plural(n, &self.noun))).color(theme::TEXT_DIM));
                            ui.add_space(8.0);
                            theme::sweep(ui, W);
                        }
                        ui.add_space(4.0);
                        ctx.request_repaint();
                    }
                    Stage::Done if n == 0 => {
                        // a green tick only when "nothing to do" is good news (not for a refused action)
                        let (icon, c) = if self.empty_ok { ("✔  ", theme::OK) } else { ("", theme::TEXT) };
                        ui.label(RichText::new(format!("{icon}{}", self.empty_title)).size(26.0).strong().color(c));
                        ui.label(RichText::new(&self.empty_text).color(theme::TEXT_DIM));
                        ui.add_space(12.0);
                        if ui.add(theme::primary_button("   Close   ")).clicked() {
                            ev = Some(OpEvent::Close);
                        }
                    }
                    Stage::Done => {
                        let t0 = *self.t_done.get_or_insert(now);
                        let shown = revealed(n, now - t0);
                        let finished = shown >= n;
                        let ok_shown = self.items[..shown].iter().filter(|i| i.ok()).count();
                        let ok_total = self.ok_count();
                        let failed_total = n - ok_total;

                        ui.horizontal(|ui| {
                            ui.label(RichText::new(ok_shown.to_string()).size(46.0).strong().color(theme::OK));
                            ui.vertical(|ui| {
                                ui.add_space(12.0);
                                let text = if finished { format!("{} {}", plural(ok_total, &self.noun), self.done_word) } else { "working...".to_string() };
                                ui.label(RichText::new(text).size(20.0).strong());
                                if finished && failed_total > 0 {
                                    ui.colored_label(theme::LOCKED, format!("{failed_total} {}", self.fail_phrase));
                                }
                            });
                        });
                        theme::progress(ui, shown as f32 / n as f32, W);
                        ui.add_space(6.0);

                        // While revealing follow the newest row; when finished go back to the top, problems first.
                        let mut rows: Vec<&OpItem> = self.items[..shown].iter().collect();
                        if finished {
                            rows.sort_by_key(|i| !i.failed());
                        }
                        egui::ScrollArea::vertical().id_salt(if finished { "op-final" } else { "op-reveal" }).max_height(250.0).stick_to_bottom(!finished).show(ui, |ui| {
                            ui.set_width(W - 12.0);
                            for it in rows {
                                let row = |ui: &mut egui::Ui, icon: &str, c: egui::Color32, strong: bool| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(c, RichText::new(icon).strong());
                                        let name = RichText::new(&it.name);
                                        ui.label(if strong { name.strong() } else { name });
                                        if !it.detail.is_empty() {
                                            ui.label(RichText::new(&it.detail).color(theme::TEXT_DIM));
                                        }
                                        ui.label(RichText::new(&it.group).color(theme::TEXT_FAINT).small());
                                    });
                                };
                                match &it.result {
                                    Some(Err(why)) => {
                                        row(ui, "✖", theme::LOCKED, true);
                                        ui.indent(("why", &it.id), |ui| {
                                            ui.colored_label(theme::LOCKED, why);
                                        });
                                    }
                                    Some(Ok(note)) if !note.is_empty() => {
                                        row(ui, "✔", theme::OK, false);
                                        ui.indent(("note", &it.id), |ui| {
                                            ui.label(RichText::new(note).color(theme::GAMING).small());
                                        });
                                    }
                                    _ => row(ui, "✔", theme::OK, false),
                                }
                            }
                        });

                        if !finished {
                            ctx.request_repaint();
                            return;
                        }
                        ui.add_space(8.0);
                        let groups = group_counts(&self.items);
                        if groups.len() > 1 {
                            ui.horizontal_wrapped(|ui| {
                                for (label, total, failed) in &groups {
                                    theme::chip(ui, &format!("{label}  {}/{total}", total - failed), if *failed == 0 { theme::OK } else { theme::JUNK });
                                }
                            });
                        }
                        if let Some((text, c)) = &self.footer {
                            ui.label(RichText::new(text).color(*c));
                        }
                        for nline in &self.notes {
                            ui.label(RichText::new(nline).color(theme::TEXT_DIM));
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui.add(theme::primary_button("   Close   ")).clicked() {
                                ev = Some(OpEvent::Close);
                            }
                            for (i, label) in self.extras.iter().enumerate() {
                                if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                                    ev = Some(OpEvent::Extra(i));
                                }
                            }
                            if self.can_undo && ui.add_enabled(can, egui::Button::new(&self.undo_label)).clicked() {
                                ev = Some(OpEvent::Undo);
                            }
                        });
                    }
                }
            });

        if ev == Some(OpEvent::Go) {
            self.stage = Stage::Running;
        }
        ev
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
}
