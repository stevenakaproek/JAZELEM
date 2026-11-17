//! CPU Scheduler tab - drawing only. The settings, defaults, registry access and history live in
//! pctuner_core::scheduler.

use crate::common::{self, Tab, Tone};
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::scheduler::*;

pub struct SchedulerTab {
    s: Scheduler,
    /// The task shown on the Multimedia scheduler part.
    task: usize,
    /// The part of the page shown.
    section: Section,
}

impl SchedulerTab {
    pub fn new() -> Self {
        let s = Scheduler::new();
        let task = s.tasks.iter().position(|t| t == "Games").unwrap_or(0);
        SchedulerTab { s, task, section: Section::Processor }
    }

    /// One setting row: label | control | status.
    /// One setting row: label | control | status.
    fn row(&mut self, ui: &mut egui::Ui, task: &str, name: &'static str, label: &str, can: bool) {
        let key: Key = (task.to_string(), name);
        let cur = self.s.cur.get(&key).cloned().flatten();
        let mut v = self.s.edit.get(&key).cloned().flatten();
        let def = default_of(task, name);
        ui.label(label);
        ui.add_enabled_ui(can, |ui| match name {
            "Clock Rate" => {
                let mut n = match &v {
                    Some(RegVal::D(n)) => *n,
                    _ => 10000,
                };
                if ui.add(egui::DragValue::new(&mut n).range(0..=1_000_000).speed(100.0)).changed() {
                    v = Some(RegVal::D(n));
                }
            }
            "Affinity" => {
                let mask = match &v {
                    Some(RegVal::D(m)) => *m,
                    _ => 0,
                };
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 20.0], egui::Label::new(RichText::new(show(name, &v)).monospace()));
                    ui.menu_button("select ...", |ui| {
                        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).min(32);
                        let full = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
                        let mut all = mask == 0;
                        if ui.checkbox(&mut all, "All CPUs").changed() {
                            v = Some(RegVal::D(if all { 0 } else { 1 }));
                        }
                        ui.separator();
                        egui::Grid::new(("aff", task)).num_columns(4).show(ui, |ui| {
                            for c in 0..n {
                                let mut on = mask == 0 || mask & (1 << c) != 0;
                                if ui.checkbox(&mut on, format!("CPU {c}")).changed() {
                                    let base = if mask == 0 { full } else { mask };
                                    let m = if on { base | (1 << c) } else { base & !(1 << c) };
                                    // all CPUs = 0; never an empty mask
                                    if m != 0 {
                                        v = Some(RegVal::D(if m & full == full { 0 } else { m }));
                                    }
                                }
                                if c % 4 == 3 {
                                    ui.end_row();
                                }
                            }
                        });
                    });
                });
            }
            _ => {
                let mut opts = choices(name);
                if !opts.contains(&v) {
                    opts.push(v.clone());
                }
                egui::ComboBox::from_id_salt(("mmcss", task, name)).width(130.0).selected_text(show(name, &v)).show_ui(ui, |ui| {
                    for o in opts {
                        let text = show(name, &o);
                        ui.selectable_value(&mut v, o, text);
                    }
                });
            }
        });
        let (text, c) = status(name, &cur, &v, &def);
        ui.label(RichText::new(text).size(12.0).color(theme::tone(c)));
        ui.end_row();
        self.s.edit.insert(key, v);
    }

    /// Processor scheduling (Win32PrioritySeparation) and Foreground Lock Timeout.
    fn prio_card(&mut self, ui: &mut egui::Ui, can: bool) {
        let pk: Key = (PRIO.to_string(), PRIO_NAME);
        let fk: Key = (FG.to_string(), FG_NAME);
        let cur_p = self.s.cur.get(&pk).cloned().flatten();
        let mut p = match self.s.edit.get(&pk).cloned().flatten() {
            Some(RegVal::D(v)) => v,
            _ => 2,
        };
        let before = prio_parts(p);
        let (mut interval, mut length, mut boost) = before;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(can, |ui| {
                theme::section(ui, "Interval length");
                ui.radio_value(&mut interval, 0, "Let Windows choose (short on this PC)");
                ui.radio_value(&mut interval, 2, "Short");
                ui.radio_value(&mut interval, 1, "Long");
                ui.add_space(6.0);
                theme::section(ui, "Fixed / variable intervals");
                ui.radio_value(&mut length, 0, "Let Windows choose (variable on this PC)");
                ui.radio_value(&mut length, 2, "Fixed length");
                ui.radio_value(&mut length, 1, "Variable length");
                ui.indent("boost", |ui| {
                    ui.add_enabled_ui(length != 2, |ui| {
                        ui.radio_value(&mut boost, 0, "No foreground boost (1:1)");
                        ui.radio_value(&mut boost, 1, "Foreground double length intervals (2:1)");
                        ui.radio_value(&mut boost, 2, "Foreground triple length intervals (3:1)");
                    });
                });
                ui.add_space(6.0);
                theme::section(ui, "Foreground lock timeout");
                let cur_f = self.s.cur.get(&fk).cloned().flatten();
                let mut f = self.s.edit.get(&fk).cloned().flatten();
                ui.horizontal(|ui| {
                    let mut n = match &f {
                        Some(RegVal::D(n)) => *n,
                        _ => 200000,
                    };
                    if ui.add(egui::DragValue::new(&mut n).range(0..=2_000_000).speed(1000.0).suffix(" ms")).changed() {
                        f = Some(RegVal::D(n));
                    }
                    let (text, c) = status(FG_NAME, &cur_f, &f, &default_of(FG, FG_NAME));
                    ui.label(RichText::new(text).size(12.0).color(theme::tone(c)));
                });
                self.s.edit.insert(fk.clone(), f);
            });
            // only a changed choice rewrites the value (an odd stored value such as 0x3F is kept as is)
            if (interval, length, boost) != before {
                p = prio_value(interval, length, boost);
            }
            let v = Some(RegVal::D(p));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Bitmask (hex):");
                ui.label(RichText::new(format!("{p:X}")).monospace().strong());
                let (text, c) = status(PRIO_NAME, &cur_p, &v, &default_of(PRIO, PRIO_NAME));
                ui.label(RichText::new(text).size(12.0).color(theme::tone(c)));
                if ui.add_enabled(can, egui::Button::new("Restore defaults")).on_hover_text("Bitmask 2 and 200000 ms. Click Apply to apply.").clicked() {
                    p = 2;
                    self.s.edit.insert(fk.clone(), default_of(FG, FG_NAME));
                }
            });
        });
        self.s.edit.insert(pk, Some(RegVal::D(p)));
    }
}

impl Tab for SchedulerTab {
    fn title(&self) -> &'static str {
        "CPU Scheduler"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        theme::page_header(ui, "CPU Scheduler", "");
        common::admin_banner(ui);
        let can = common::is_admin();
        self.task = self.task.min(self.s.tasks.len().saturating_sub(1));

        // ---- section tabs (a dot marks a section with unsaved changes)
        ui.horizontal(|ui| {
            for (s, name) in [(Section::Processor, "Processor scheduling"), (Section::Multimedia, "Multimedia scheduler")] {
                let on = self.section == s;
                let dirty = !self.s.pending(s).is_empty();
                let text = if dirty { format!("  {name}  •  ") } else { format!("  {name}  ") };
                let rt = RichText::new(text).size(15.0).strong().color(if on { theme::ACCENT } else { theme::TEXT_DIM });
                if ui.selectable_label(on, rt).clicked() {
                    self.section = s;
                }
            }
        });
        ui.add_space(6.0);

        let section = self.section;
        egui::ScrollArea::vertical().id_salt(("sched-page", section == Section::Processor)).auto_shrink([false, false]).show(ui, |ui| {
            // ---- what is applied now in this section
            let applied = self.s.applied(section);
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                if applied.is_empty() {
                    ui.label(RichText::new("Everything is at the Windows default.").color(theme::TEXT_DIM));
                } else {
                    theme::section(ui, &format!("Applied now - {} changed from default", applied.len()));
                    for a in &applied {
                        ui.label(RichText::new(a).color(theme::OK));
                    }
                }
            });
            ui.add_space(6.0);

            match section {
                Section::Processor => self.prio_card(ui, can),
                Section::Multimedia => {
                    theme::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        theme::section(ui, "Multimedia scheduler");
                        egui::Grid::new("mmcss-sys").num_columns(3).spacing([14.0, 6.0]).show(ui, |ui| {
                            for (n, label, _) in SYS_FIELDS {
                                self.row(ui, "", n, label, can);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    theme::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            theme::section(ui, "Task");
                            let name = self.s.tasks.get(self.task).cloned().unwrap_or_default();
                            let changed: Vec<String> = self.s.pending(Section::Multimedia).into_iter().map(|c| c.task).collect();
                            egui::ComboBox::from_id_salt("mmcss-task").width(200.0).selected_text(&name).show_ui(ui, |ui| {
                                for (i, t) in self.s.tasks.iter().enumerate() {
                                    let label = if changed.contains(t) { format!("{t}  •") } else { t.clone() };
                                    ui.selectable_value(&mut self.task, i, label);
                                }
                            });
                        });
                        ui.add_space(4.0);
                        if let Some(task) = self.s.tasks.get(self.task).cloned() {
                            egui::Grid::new(("mmcss-task-grid", &task)).num_columns(3).spacing([14.0, 6.0]).show(ui, |ui| {
                                for (n, label, _) in TASK_FIELDS {
                                    self.row(ui, &task, n, label, can);
                                }
                            });
                        }
                    });
                }
            }
            ui.add_space(6.0);

            // ---- actions for this section only
            let pending = self.s.pending(section);
            ui.horizontal(|ui| {
                if ui.add_enabled(can && !pending.is_empty(), theme::primary_button(&format!("   Apply ({})   ", pending.len()))).clicked() {
                    self.s.save(section);
                }
                if ui.add_enabled(!pending.is_empty(), egui::Button::new("Cancel")).on_hover_text("Discard the changes on this tab that are not applied yet.").clicked() {
                    for (k, v) in self.s.cur.iter().filter(|(k, _)| section_of(&k.0) == section) {
                        self.s.edit.insert(k.clone(), v.clone());
                    }
                }
                if ui.add_enabled(can && self.s.last_save(section).is_some(), egui::Button::new("Undo last apply")).clicked() {
                    self.s.undo_last(section);
                }
                if ui.button("Backup registry entries").clicked() {
                    self.s.report_for = section;
                    self.s.report = vec![match self.s.backup(section) {
                        Ok(p) => (format!("Backup saved: {p}"), Tone::Ok),
                        Err(e) => (format!("Backup failed: {e}"), Tone::Error),
                    }];
                }
                if ui.button("Reload").on_hover_text("Read the registry again.").clicked() {
                    self.s.reload(section);
                }
            });
            if !pending.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Not applied yet:").size(12.5).color(theme::ACCENT));
                for c in &pending {
                    ui.label(RichText::new(format!("  {}", c.line())).size(12.5).color(theme::TEXT_DIM));
                }
            }
            if !self.s.report.is_empty() && self.s.report_for == section {
                ui.add_space(4.0);
                for (l, c) in &self.s.report {
                    ui.label(RichText::new(l).size(12.5).color(theme::tone(*c)));
                }
            }
        });
    }
}
