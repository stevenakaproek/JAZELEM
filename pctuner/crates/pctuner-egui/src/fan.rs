//! Fan Tuner tab - drawing only. The helper process, profiles, curves, safety watch and the engine
//! thread live in pctuner_core::fan.

use crate::common::{self, Tab};
use crate::theme;
use eframe::egui::{self, RichText};
use pctuner_core::fan::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Duration;

pub struct FanTab {
    e: FanEngine,
}

impl FanTab {
    pub fn new() -> Self {
        FanTab { e: FanEngine::new(Fan::new()) }
    }
}

impl Tab for FanTab {
    fn title(&self) -> &'static str {
        "Fan Tuner"
    }

    fn on_exit(&mut self) {
        self.e.shutdown();
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        // the engine thread waits while the page is drawn (a few milliseconds)
        let mut guard = self.e.lock();
        page(&mut guard, ui);
    }
}

fn page(f: &mut Fan, ui: &mut egui::Ui) {
    {
        f.ensure_started();
        f.watch();
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        let can = f.can();

        let mut give_back = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| theme::page_header(ui, "Fan Tuner", "Live fan speeds and temperatures. Fans always go back to the BIOS when PC Tuner closes."));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                let b = theme::danger_button("⟲  Give all fans back to BIOS");
                if ui
                    .add_enabled(can && f.any_controlled(), b)
                    .on_hover_text("Every fan goes back to its BIOS / graphics-driver curve. Own settings are cleared and no profile is applied at startup until you pick one.")
                    .on_disabled_hover_text(if can { "No fan is controlled by PC Tuner." } else { "Run PC Tuner as administrator to control fans." })
                    .clicked()
                {
                    give_back = true;
                }
            });
        });
        if give_back {
            f.give_all_back();
        }
        if !common::is_admin() && !f.demo {
            theme::banner(ui, theme::LOCKED, "Run PC Tuner as administrator to see the motherboard fans and the CPU temperature, and to control fans (PawnIO driver). Only the graphics card is visible now.");
        }
        if let Some(e) = &f.error {
            theme::banner(ui, theme::LOCKED, e);
        }
        let (status, rpm_hist, cpu_hist, gpu_hist, stale_for) = f.snapshot();
        let afterburner = f.afterburner_running();
        let mut actions: Vec<(String, f32, CardAction)> = vec![];
        let mut dismiss = None;
        // a detached (hung) helper may still own fans: never "BIOS control" while it runs
        let helper_gone = f.helper_is_gone();
        let stuck = f.fans_stuck();
        egui::ScrollArea::vertical().id_salt("fan-page").auto_shrink([false, false]).show(ui, |ui| {
            if let Some(age) = stale_for {
                // old numbers are not shown as live (and a stale 90 °C banner never stays up)
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (head, sub) = stale_text(helper_gone, stuck, age.as_secs());
                    ui.label(RichText::new(head).size(18.0).strong());
                    ui.label(RichText::new(sub).color(theme::TEXT_DIM));
                });
                return;
            }
            let Some(s) = status else {
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if helper_gone {
                        let (head, sub) = stale_text(true, stuck, 0);
                        ui.label(RichText::new(head).size(18.0).strong());
                        ui.label(RichText::new(sub).color(theme::TEXT_DIM));
                    } else {
                        ui.label(RichText::new("Reading sensors...").size(18.0).strong());
                        ui.add_space(6.0);
                        theme::sweep(ui, ui.available_width());
                    }
                });
                return;
            };
            if s.blind {
                theme::banner(ui, theme::GAMING, "Temperatures cannot be read - every fan is on BIOS control until they can be read again. Your settings come back by themselves.");
            }
            // the helper's latest notice stays until dismissed
            if !s.msg.is_empty() && s.msg_seq != f.msg_dismissed {
                ui.horizontal(|ui| {
                    if ui.small_button("Dismiss").clicked() {
                        dismiss = Some(s.msg_seq);
                    }
                    ui.colored_label(theme::GAMING, &s.msg);
                });
                ui.add_space(4.0);
            }
            hero_card(ui, &s, &cpu_hist, &gpu_hist);
            profile_bar(f, ui, &s.fans, can);
            let view = View {
                settings: &f.settings,
                pinned: &f.store.pinned,
                can,
                afterburner,
                profile: f.active.as_deref(),
                handed_back: &f.handed_back,
            };
            fans_and_temps(ui, &s, &rpm_hist, &view, &mut actions);
        });
        if let Some(seq) = dismiss {
            f.msg_dismissed = seq;
        }
        for (id, floor, a) in actions {
            f.apply(id, floor, a);
        }
        f.sync();
    }
}

/// The profile bar: pick, save, save as, reset, delete.
fn profile_bar(f: &mut Fan, ui: &mut egui::Ui, fans: &[FanInfo], can: bool) {
    {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Profile");
                ui.add_space(8.0);
                let names = f.profile_names();
                ui.add_enabled_ui(can, |ui| {
                    for n in names {
                        let on = f.active.as_deref() == Some(n.as_str());
                        if ui.selectable_label(on, format!("  {n}  ")).clicked() {
                            f.load_profile(&n, fans);
                        }
                    }
                });
            });
            ui.add_space(4.0);
            let modified = f.modified();
            ui.horizontal(|ui| {
                match &f.active {
                    Some(n) if modified => {
                        ui.label(RichText::new(n.as_str()).strong());
                        ui.colored_label(theme::GAMING, "changed - not saved");
                    }
                    Some(n) => {
                        ui.label(RichText::new(n.as_str()).strong());
                        ui.colored_label(theme::TEXT_DIM, "saved");
                    }
                    None => {
                        ui.colored_label(theme::TEXT_DIM, "No profile - pick one, or set fans below and save them as a profile.");
                    }
                }
                ui.add_space(10.0);
                ui.add_enabled_ui(can, |ui| {
                    if let Some(n) = f.active.clone() {
                        if ui.add_enabled(modified, egui::Button::new("Save changes")).on_hover_text(format!("Store the fans as they are now in {n}.")).clicked() {
                            f.save_changes();
                        }
                        if BUILTIN.contains(&n.as_str()) {
                            if ui.button("Reset to default").on_hover_text(format!("Throw away your edits to {n} and use PC Tuner's default curves.")).clicked() {
                                f.reset_builtin(&n, fans);
                            }
                        } else if ui.button("Delete").clicked() {
                            f.delete_profile(&n);
                        }
                    }
                    ui.add_space(10.0);
                    ui.add(egui::TextEdit::singleline(&mut f.new_name).hint_text("New profile name").desired_width(150.0));
                    let name = f.new_name.trim().to_string();
                    let (ok, why) = new_name_check(&name, &f.store);
                    let r = ui.add_enabled(ok, egui::Button::new("Save as new"));
                    if let Some(why) = &why {
                        if !name.is_empty() {
                            ui.label(RichText::new(why.as_str()).size(11.5).color(theme::GAMING));
                        }
                    }
                    if r.on_disabled_hover_text(why.unwrap_or_else(|| "Type a name for the new profile.".into())).clicked() {
                        f.save_as_new();
                    }
                });
            });
            if let Some(n) = &f.note {
                ui.label(RichText::new(n.as_str()).size(12.0).color(theme::TEXT_DIM));
            }
        });
        ui.add_space(6.0);
    }
}

/// Read-only view of the tab state the page needs.
struct View<'a> {
    settings: &'a HashMap<String, FanSetting>,
    /// fans with their own setting (profiles leave them alone)
    pinned: &'a BTreeMap<String, FanSetting>,
    can: bool,
    afterburner: bool,
    profile: Option<&'a str>,
    /// fans the helper reports back on the BIOS although PC Tuner asked to drive them
    handed_back: &'a HashSet<String>,
}

/// Banners plus the hero card: the hottest of CPU and GPU, with live graphs.
fn hero_card(ui: &mut egui::Ui, s: &Status, cpu_hist: &VecDeque<f32>, gpu_hist: &VecDeque<f32>) {
    if s.emergency {
        theme::banner(ui, theme::LOCKED, "Over 90 °C - every controlled fan is at 100 % until the CPU and GPU are back under 80 °C.");
    }
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        let hottest = s.cpu.into_iter().chain(s.gpu).fold(f32::NAN, f32::max);
        ui.horizontal(|ui| {
            if hottest.is_nan() {
                theme::hero(ui, "-", "no temperature", "CPU and GPU temperatures could not be read", theme::TEXT_DIM);
            } else {
                let v = theme::count_up(ui.ctx(), "fan-hot", hottest);
                let which = if s.gpu.unwrap_or(f32::MIN) >= s.cpu.unwrap_or(f32::MIN) { "GPU" } else { "CPU" };
                theme::hero(ui, &format!("{v:.0} °C"), "hottest", &format!("{which} is the warmest right now - 90 °C sends every fan to 100 %"), theme::tone(temp_color(hottest)));
            }
            ui.add_space(28.0);
            let t = |v: Option<f32>| v.map(|x| format!("{x:.0} °C")).unwrap_or_else(|| "-".into());
            theme::stat(ui, &t(s.cpu), "CPU", s.cpu.map(|t| theme::tone(temp_color(t))).unwrap_or(theme::TEXT_DIM));
            ui.add_space(22.0);
            theme::stat(ui, &t(s.gpu), "GPU", s.gpu.map(|t| theme::tone(temp_color(t))).unwrap_or(theme::TEXT_DIM));
            ui.add_space(22.0);
            theme::stat(ui, &s.fans.len().to_string(), "fans found", theme::TEXT);
            ui.add_space(22.0);
            let ctl = s.fans.iter().filter(|f| f.controllable).count();
            theme::stat(ui, &ctl.to_string(), "can be controlled", theme::TEXT);
        });
        ui.add_space(6.0);
        // two graphs side by side, with room for the gap between them
        let w = ((ui.available_width() - 2.0 * ui.spacing().item_spacing.x - 16.0) / 2.0).max(120.0);
        ui.horizontal(|ui| {
            for (label, h) in [("CPU temperature", cpu_hist), ("GPU temperature", gpu_hist)] {
                ui.vertical(|ui| {
                    ui.label(RichText::new(label).size(12.0).color(theme::TEXT_FAINT));
                    let v: Vec<f32> = h.iter().cloned().collect();
                    theme::sparkline(ui, &v, egui::vec2(w, 34.0), v.last().map(|t| theme::tone(temp_color(*t))).unwrap_or(theme::ACCENT));
                });
                ui.add_space(8.0);
            }
        });
    });
    ui.add_space(6.0);
}

fn fans_and_temps(ui: &mut egui::Ui, s: &Status, rpm_hist: &HashMap<String, VecDeque<f32>>, view: &View, actions: &mut Vec<(String, f32, CardAction)>) {
    // ---- one card per fan, three to a row
    theme::section(ui, "Fans");
    if s.fans.is_empty() {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label("No fans were found.");
        });
    }
    // headers that have never spun (nothing plugged in) go into a folded group at the end
    let (shown, empty): (Vec<&FanInfo>, Vec<&FanInfo>) = s.fans.iter().partition(|f| {
        f.gpu
            || view.settings.get(&f.id).map(|x| *x != FanSetting::Bios).unwrap_or(false)
            || rpm_hist.get(&f.id).map(|h| h.iter().any(|v| *v > 0.0)).unwrap_or(false)
            || f.rpm.unwrap_or(0.0) > 0.0
    });
    let mut cards = |ui: &mut egui::Ui, list: &[&FanInfo]| {
        for row in list.chunks(3) {
            ui.columns(3, |cols| {
                for (i, f) in row.iter().enumerate() {
                    let setting = view.settings.get(&f.id).cloned().unwrap_or_default();
                    if let Some(a) = fan_card(&mut cols[i], f, rpm_hist.get(&f.id), &setting, view, s) {
                        actions.push((f.id.clone(), floor_for(f.pump, f.min), a));
                    }
                }
            });
            ui.add_space(4.0);
        }
    };
    cards(ui, &shown);
    if !empty.is_empty() {
        let names: Vec<&str> = empty.iter().map(|f| f.name.as_str()).collect();
        egui::CollapsingHeader::new(RichText::new(format!("{} headers at 0 RPM - probably nothing plugged in", empty.len())).color(theme::TEXT_DIM))
            .id_salt("fan-empty")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new(names.join(", ")).size(12.0).color(theme::TEXT_FAINT));
                ui.add_space(4.0);
                cards(ui, &empty);
            });
    }
    ui.add_space(6.0);

    // ---- all temperatures
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, "Temperatures");
        egui::Grid::new("fan-temps").num_columns(3).spacing([24.0, 6.0]).show(ui, |ui| {
            for t in &s.temps {
                let v = t.value.unwrap_or(0.0);
                ui.label(RichText::new(&t.hw).color(theme::TEXT_DIM));
                ui.label(&t.name);
                ui.label(RichText::new(format!("{v:.0} °C")).strong().color(theme::tone(temp_color(v))));
                ui.end_row();
            }
        });
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new("Sensors: LibreHardwareMonitor with the PawnIO driver, updated every second. Speeds go as low as you set them; at 90 °C every controlled fan goes to 100 %.")
            .size(12.0)
            .color(theme::TEXT_FAINT),
    );
}

fn fan_card(ui: &mut egui::Ui, f: &FanInfo, hist: Option<&VecDeque<f32>>, setting: &FanSetting, view: &View, s: &Status) -> Option<CardAction> {
    let emergency = s.emergency;
    let mut action = None;
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        // the column layout is justified, which would stretch wrapped text
        ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&f.name).size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if f.controllable {
                    let (text, c) = fan_chip(f, setting, view.can, emergency, s.blind, view.handed_back.contains(&f.id));
                    theme::chip(ui, text, theme::tone(c));
                } else {
                    theme::chip(ui, "read-only", theme::DIM);
                }
                if f.pump {
                    theme::chip(ui, "pump", theme::OPTIONAL);
                }
                if f.controllable && view.pinned.contains_key(&f.id) {
                    theme::chip(ui, "own setting", theme::GAMING);
                }
            });
        });
        ui.label(RichText::new(&f.hw).size(12.0).color(theme::TEXT_FAINT));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let rpm = f.rpm.map(|r| format!("{r:.0}")).unwrap_or_else(|| "-".into());
            ui.label(RichText::new(rpm).size(28.0).strong().color(theme::TEXT));
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("RPM").strong());
            });
            if let Some(p) = f.percent {
                ui.add_space(14.0);
                theme::stat(ui, &format!("{p:.0} %"), "duty", theme::TEXT);
            }
        });
        let v: Vec<f32> = hist.map(|h| h.iter().cloned().collect()).unwrap_or_default();
        theme::sparkline(ui, &v, egui::vec2(ui.available_width(), 30.0), theme::ACCENT);
        if !f.controllable {
            ui.label(RichText::new("Shown only - this header cannot be controlled by the library on this board.").size(11.5).color(theme::TEXT_FAINT));
            return;
        }

        // ---- control
        ui.add_space(6.0);
        let floor = floor_for(f.pump, f.min);
        if !view.can && *setting != FanSetting::Bios {
            ui.label(RichText::new("Saved setting below - not applied (view-only).").size(11.5).color(theme::GAMING));
        } else if view.can && *setting != FanSetting::Bios && view.handed_back.contains(&f.id) {
            ui.label(RichText::new("The helper did not take this setting, so the BIOS drives this fan. PC Tuner tries again every few seconds.").size(11.5).color(theme::GAMING));
        }
        // own setting vs following the profile
        // plain labels wrap inside the card (a horizontal row would widen it past its column)
        ui.add_enabled_ui(view.can, |ui| {
            if view.pinned.contains_key(&f.id) {
                ui.label(RichText::new("Own setting - profiles leave this fan alone.").size(11.5).color(theme::GAMING));
                let tip = match view.profile {
                    Some(p) => format!("Use {p}'s setting for this fan again."),
                    None => "Let the next profile you pick set this fan.".to_string(),
                };
                if ui.small_button("Follow profile").on_hover_text(tip).clicked() {
                    action = Some(CardAction::FollowProfile);
                }
            } else {
                let who = view.profile.map(|p| format!("Follows {p}.")).unwrap_or_else(|| "Follows the profile.".into());
                ui.label(RichText::new(format!("{who} Change it here to give this fan its own setting.")).size(11.5).color(theme::TEXT_FAINT));
            }
        });
        ui.add_enabled_ui(view.can, |ui| {
            ui.horizontal(|ui| {
                let bios = *setting == FanSetting::Bios;
                if ui.selectable_label(bios, "  BIOS  ").on_hover_text("Hands off: the BIOS (or graphics driver) curve decides.").clicked() && !bios {
                    action = Some(CardAction::Set(FanSetting::Bios));
                }
                let manual = matches!(setting, FanSetting::Manual { .. });
                if ui.selectable_label(manual, "  Manual  ").on_hover_text("A fixed speed you choose with the slider.").clicked() && !manual {
                    let start = f.percent.unwrap_or(50.0).max(floor);
                    action = Some(CardAction::Set(FanSetting::Manual { percent: start.round() }));
                }
                let curve = matches!(setting, FanSetting::Curve { .. });
                if ui.selectable_label(curve, "  Curve  ").on_hover_text("Speed follows a temperature: drag the points on the graph.").clicked() && !curve {
                    action = Some(CardAction::Set(default_curve(&f.name, f.gpu, f.pump, floor)));
                }
                let trigger = matches!(setting, FanSetting::Trigger { .. });
                if ui
                    .selectable_label(trigger, "  Trigger  ")
                    .on_hover_text("Two steady speeds for gaming: quiet until the temperature reaches the gaming point, then one fixed speed until it falls back to the idle point. No revving on short temperature spikes.")
                    .clicked()
                    && !trigger
                {
                    action = Some(CardAction::Set(default_trigger(&f.name, f.gpu, f.pump, floor)));
                }
            });
            if let FanSetting::Trigger { source, idle_temp, load_temp, idle, load } = setting {
                let mut src = *source;
                egui::ComboBox::from_id_salt(("fan-trg-src", &f.id)).selected_text(src.label()).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    for s in [Source::Cpu, Source::Gpu, Source::Hottest] {
                        ui.selectable_value(&mut src, s, s.label());
                    }
                });
                let mut v = (*idle_temp, *load_temp, *idle, *load);
                let live = src.read(s.cpu, s.gpu);
                let changed = trigger_editor(ui, &mut v, floor, live, f.percent);
                if changed || src != *source {
                    let (idle_temp, load_temp, idle, load) = v;
                    action = Some(CardAction::Set(FanSetting::Trigger { source: src, idle_temp, load_temp, idle, load }));
                }
            }
            if let FanSetting::Curve { source, points } = setting {
                let mut src = *source;
                egui::ComboBox::from_id_salt(("fan-src", &f.id)).selected_text(src.label()).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    for s in [Source::Cpu, Source::Gpu, Source::Hottest] {
                        ui.selectable_value(&mut src, s, s.label());
                    }
                });
                let mut pts = points.clone();
                let live = src.read(s.cpu, s.gpu);
                let changed = curve_editor(ui, &f.id, &mut pts, floor, live);
                if changed || src != *source {
                    action = Some(CardAction::Set(FanSetting::Curve { source: src, points: pts }));
                }
            }
            if let FanSetting::Manual { percent } = setting {
                let mut v = percent.max(floor);
                // leave room for the number box beside the slider so the card keeps its column width
                ui.spacing_mut().slider_width = (ui.available_width() - 96.0).max(60.0);
                let r = ui.add(egui::Slider::new(&mut v, floor..=100.0).step_by(1.0).fixed_decimals(0).suffix(" %"));
                if r.changed() {
                    action = Some(CardAction::Set(FanSetting::Manual { percent: v }));
                }
                ui.label(RichText::new(format!("Lowest allowed: {floor:.0} %")).size(11.5).color(theme::TEXT_FAINT));
            }
            if !f.gpu {
                let mut pump = f.pump;
                if ui.checkbox(&mut pump, "This is a pump").on_hover_text("Its default curve follows the CPU temperature.").changed() {
                    action = Some(CardAction::Pump(pump));
                }
            }
        })
        .response
        .on_disabled_hover_text("Run PC Tuner as administrator to control fans.");
        if f.gpu && *setting != FanSetting::Bios && view.afterburner {
            ui.label(
                RichText::new("MSI Afterburner is running. Turn off its fan control (Settings > Fan > untick \"Enable user defined software automatic fan control\") so the two don't fight.")
                    .size(11.5)
                    .color(theme::GAMING),
            );
        }
        });
    });
    action
}


/// Trigger editor: a picture of the two speeds and four typed values. `v` = (idle temp, gaming temp,
/// idle %, gaming %). `now` is the speed the helper reports for this fan. Returns true when a value changed.
fn trigger_editor(ui: &mut egui::Ui, v: &mut (f32, f32, f32, f32), floor: f32, live: Option<f32>, now: Option<f32>) -> bool {
    let size = egui::vec2(ui.available_width(), 110.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let plot = rect.shrink2(egui::vec2(6.0, 8.0));
    let x = |t: f32| plot.left() + plot.width() * (t.clamp(GRAPH_T.0, GRAPH_T.1) - GRAPH_T.0) / (GRAPH_T.1 - GRAPH_T.0);
    let y = |d: f32| plot.bottom() - plot.height() * d.clamp(0.0, 100.0) / 100.0;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, theme::tint(egui::Color32::WHITE, 6));
    let (it, lt, idle, load) = *v;
    // between the two temperatures the fan keeps whichever speed it had
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x(it), plot.top()), egui::pos2(x(lt), plot.bottom())), 0.0, theme::tint(theme::ACCENT, 14));
    let faint = egui::Stroke::new(1.0_f32, theme::tint(egui::Color32::WHITE, 10));
    for d in [25.0_f32, 50.0, 75.0] {
        painter.line_segment([egui::pos2(plot.left(), y(d)), egui::pos2(plot.right(), y(d))], faint);
    }
    let x90 = x(90.0);
    painter.line_segment([egui::pos2(x90, plot.top()), egui::pos2(x90, plot.bottom())], egui::Stroke::new(1.0_f32, theme::tint(theme::LOCKED, 160)));
    // idle level up to the gaming point, the jump, gaming level down to the idle point (dashed: the way back)
    let s = egui::Stroke::new(2.0_f32, theme::ACCENT);
    painter.line_segment([egui::pos2(plot.left(), y(idle)), egui::pos2(x(lt), y(idle))], s);
    painter.line_segment([egui::pos2(x(lt), y(idle)), egui::pos2(x(lt), y(load))], s);
    painter.line_segment([egui::pos2(x(lt), y(load)), egui::pos2(plot.right(), y(load))], s);
    painter.add(egui::Shape::dashed_line(&[egui::pos2(x(it), y(load)), egui::pos2(x(lt), y(load))], s, 4.0, 3.0));
    painter.add(egui::Shape::dashed_line(&[egui::pos2(x(it), y(load)), egui::pos2(x(it), y(idle))], s, 4.0, 3.0));
    let small = egui::FontId::proportional(10.5);
    painter.text(egui::pos2(x(lt) + 4.0, y(load) - 2.0), egui::Align2::LEFT_BOTTOM, format!("gaming {load:.0} %"), small.clone(), theme::TEXT_DIM);
    painter.text(egui::pos2(plot.left() + 2.0, y(idle) - 2.0), egui::Align2::LEFT_BOTTOM, format!("idle {idle:.0} %"), small.clone(), theme::TEXT_DIM);
    painter.text(plot.left_bottom() + egui::vec2(2.0, -2.0), egui::Align2::LEFT_BOTTOM, "20 °C", small.clone(), theme::TEXT_FAINT);
    painter.text(egui::pos2(x90 + 3.0, plot.top()), egui::Align2::LEFT_TOP, "90°", small, theme::tint(theme::LOCKED, 200));
    if let Some(t) = live {
        let d = now.unwrap_or(if t >= lt { load } else { idle });
        painter.circle_filled(egui::pos2(x(t), y(d)), 5.0, theme::GAMING);
        let label = format!("now {t:.0} °C → {d:.0} %");
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.5), theme::GAMING);
        let pos = plot.left_top() + egui::vec2(2.0, 2.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(3.0, 1.0));
        painter.rect_filled(bg, 3.0, egui::Color32::from_rgba_unmultiplied(theme::BG.r(), theme::BG.g(), theme::BG.b(), 200));
        painter.galley(pos, galley, theme::GAMING);
    }

    let mut n = *v;
    let mut changed = false;
    fn dv<'a>(val: &'a mut f32, lo: f32, hi: f32, suffix: &'static str) -> egui::DragValue<'a> {
        egui::DragValue::new(val).range(lo..=hi).speed(0.25).fixed_decimals(0).suffix(suffix)
    }
    egui::Grid::new(ui.id().with("trigger-values")).num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new("Gaming from").size(12.0).color(theme::TEXT_DIM));
        changed |= ui.add(dv(&mut n.1, GRAPH_T.0 + 2.0, GRAPH_T.1, " °C")).on_hover_text("At this temperature the fan steps up to the gaming speed.").changed();
        ui.label(RichText::new("at").size(12.0).color(theme::TEXT_DIM));
        changed |= ui.add(dv(&mut n.3, floor, 100.0, " %")).on_hover_text("Gaming speed.").changed();
        ui.end_row();
        ui.label(RichText::new("Idle below").size(12.0).color(theme::TEXT_DIM));
        changed |= ui.add(dv(&mut n.0, GRAPH_T.0, GRAPH_T.1 - 2.0, " °C")).on_hover_text("The fan drops back to the idle speed only when the temperature falls to this point (at least 2 °C under the gaming point).").changed();
        ui.label(RichText::new("at").size(12.0).color(theme::TEXT_DIM));
        changed |= ui.add(dv(&mut n.2, floor, 100.0, " %")).on_hover_text("Idle speed.").changed();
        ui.end_row();
    });
    if changed {
        // keep the gaming point above the idle point, moving whichever one was not just edited
        if n.1 != v.1 && n.1 < n.0 + 2.0 {
            n.0 = n.1 - 2.0;
        }
        let fixed = fix_trigger(n.0, n.1, n.2, n.3, floor);
        changed = fixed != *v;
        *v = fixed;
    }
    ui.label(RichText::new("Steady speeds: short temperature spikes in games do not rev the fan up and down.").size(11.5).color(theme::TEXT_FAINT));
    changed
}

/// Fan curve editor. Returns true when the curve changed.
/// Drag a point to move it, click the graph to add one, right-click a point to remove it; the row under
/// the graph takes typed values for the selected point.
/// x = temperature 20-100 °C, y = duty 0-100 %. The shaded band is below the fan's floor; the red line is the 90 °C override.
fn curve_editor(ui: &mut egui::Ui, id: &str, points: &mut Vec<[f32; 2]>, floor: f32, live: Option<f32>) -> bool {
    let size = egui::vec2(ui.available_width(), 150.0);
    // the graph itself takes clicks (add a point); the points are registered after it, so they win
    let (rect, graph) = ui.allocate_exact_size(size, egui::Sense::click());
    let plot = rect.shrink2(egui::vec2(6.0, 8.0));
    let to_screen = |t: f32, d: f32| {
        egui::pos2(
            plot.left() + plot.width() * (t - GRAPH_T.0) / (GRAPH_T.1 - GRAPH_T.0),
            plot.bottom() - plot.height() * d / 100.0,
        )
    };
    let from_screen = |p: egui::Pos2| {
        let t = GRAPH_T.0 + (p.x - plot.left()) / plot.width() * (GRAPH_T.1 - GRAPH_T.0);
        let d = (plot.bottom() - p.y) / plot.height() * 100.0;
        (t, d)
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, theme::tint(egui::Color32::WHITE, 6));
    // grid: every 10 °C and every 25 %
    for t in (30..=90).step_by(10) {
        let x = to_screen(t as f32, 0.0).x;
        painter.line_segment([egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())], egui::Stroke::new(1.0_f32, theme::tint(egui::Color32::WHITE, 10)));
    }
    for d in [25.0_f32, 50.0, 75.0] {
        let y = to_screen(0.0, d).y;
        painter.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], egui::Stroke::new(1.0_f32, theme::tint(egui::Color32::WHITE, 10)));
    }
    // below the floor: not allowed
    let fy = to_screen(0.0, floor).y;
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(plot.left(), fy), plot.right_bottom()), 0.0, theme::tint(theme::LOCKED, 9));
    painter.line_segment([egui::pos2(plot.left(), fy), egui::pos2(plot.right(), fy)], egui::Stroke::new(1.0_f32, theme::tint(theme::LOCKED, 90)));
    // 90 °C: every fan to 100 %
    let x90 = to_screen(90.0, 0.0).x;
    // under the floor line, ending left of the 90° line (the curve is always above the floor)
    if floor > 0.0 {
        painter.text(egui::pos2(x90.min(plot.right()) - 4.0, fy + 2.0), egui::Align2::RIGHT_TOP, format!("min {floor:.0} %"), egui::FontId::proportional(10.5), theme::tint(theme::LOCKED, 200));
    }
    painter.line_segment([egui::pos2(x90, plot.top()), egui::pos2(x90, plot.bottom())], egui::Stroke::new(1.0_f32, theme::tint(theme::LOCKED, 160)));
    painter.text(egui::pos2(x90 + 3.0, plot.top()), egui::Align2::LEFT_TOP, "90°", egui::FontId::proportional(10.5), theme::tint(theme::LOCKED, 200));
    painter.text(plot.left_bottom() + egui::vec2(2.0, -2.0), egui::Align2::LEFT_BOTTOM, "20 °C", egui::FontId::proportional(10.5), theme::TEXT_FAINT);
    painter.text(plot.left_top() + egui::vec2(2.0, 0.0), egui::Align2::LEFT_TOP, "100 %", egui::FontId::proportional(10.5), theme::TEXT_FAINT);

    // the selected point (the one the value boxes under the graph edit) is remembered per fan
    let sel_id = ui.id().with(("fan-sel", id));
    let mut sel: Option<usize> = ui.data(|d| d.get_temp::<usize>(sel_id)).filter(|i| *i < points.len());

    // drag the points (temperatures stay in order, 2 °C apart); click selects, right-click removes
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let mut hovered: Option<usize> = None;
    for i in 0..points.len() {
        let p = to_screen(points[i][0], points[i][1]);
        let hit = egui::Rect::from_center_size(p, egui::vec2(18.0, 18.0));
        let resp = ui.interact(hit, ui.id().with(("fan-pt", id, i)), egui::Sense::click_and_drag());
        if resp.clicked() || resp.drag_started_by(egui::PointerButton::Primary) {
            sel = Some(i);
        }
        if resp.secondary_clicked() {
            remove = Some(i);
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (t, d) = from_screen(pos);
                changed |= set_point(points, i, t, d, floor);
            }
        }
        if resp.hovered() || resp.dragged() {
            hovered = Some(i);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
    if let Some(i) = remove {
        if remove_point(points, i) {
            changed = true;
            sel = None;
            hovered = None;
        }
    } else if graph.clicked() {
        if let Some(pos) = graph.interact_pointer_pos() {
            let (t, d) = from_screen(pos);
            if let Some(i) = add_point(points, t, d, floor) {
                changed = true;
                sel = Some(i);
            }
        }
    }

    // the curve, flat beyond the first and last points
    let mut line = vec![to_screen(GRAPH_T.0, points.first().map(|p| p[1]).unwrap_or(100.0).max(floor))];
    line.extend(points.iter().map(|p| to_screen(p[0], p[1].max(floor))));
    line.push(to_screen(GRAPH_T.1, points.last().map(|p| p[1]).unwrap_or(100.0).max(floor)));
    painter.add(egui::Shape::line(line, egui::Stroke::new(2.0_f32, theme::ACCENT)));
    for (i, p) in points.iter().enumerate() {
        let c = to_screen(p[0], p[1].max(floor));
        let hot = ui.ctx().is_being_dragged(ui.id().with(("fan-pt", id, i)));
        let fill = if sel == Some(i) { theme::ACCENT } else { theme::BG };
        painter.circle(c, if hot { 7.0 } else { 5.5 }, fill, egui::Stroke::new(2.0_f32, theme::ACCENT));
        if hot || hovered == Some(i) {
            painter.text(c + egui::vec2(0.0, -12.0), egui::Align2::CENTER_BOTTOM, format!("{:.0} °C → {:.0} %", p[0], p[1]), egui::FontId::proportional(11.5), theme::TEXT);
        }
    }

    // where the fan is right now
    if let Some(t) = live {
        let duty = curve_duty(points, t.clamp(GRAPH_T.0, GRAPH_T.1)).max(floor);
        let c = to_screen(t.clamp(GRAPH_T.0, GRAPH_T.1), duty);
        painter.circle_filled(c, 5.0, theme::GAMING);
        // pinned to the top-left corner (under "100 %"), where curves rarely go, on a backdrop so grid
        // lines and points never run through it
        let label = format!("now {t:.0} °C → {duty:.0} %");
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.5), theme::GAMING);
        let pos = plot.left_top() + egui::vec2(2.0, 15.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(3.0, 1.0));
        painter.rect_filled(bg, 3.0, egui::Color32::from_rgba_unmultiplied(theme::BG.r(), theme::BG.g(), theme::BG.b(), 200));
        painter.galley(pos, galley, theme::GAMING);
    }

    // exact values for the selected point
    ui.horizontal(|ui| match sel {
        Some(i) => {
            ui.label(RichText::new(format!("Point {} of {}", i + 1, points.len())).size(12.0).color(theme::TEXT_DIM));
            let (lo, hi) = temp_range(points, i);
            let mut t = points[i][0];
            let mut d = points[i][1].max(floor);
            let rt = ui
                .add(egui::DragValue::new(&mut t).range(lo..=hi).speed(0.25).fixed_decimals(0).suffix(" °C"))
                .on_hover_text(format!("Temperature of this point: type a value ({lo:.0}-{hi:.0} °C, 2 °C away from its neighbours)."));
            let rd = ui
                .add(egui::DragValue::new(&mut d).range(floor..=100.0).speed(0.25).fixed_decimals(0).suffix(" %"))
                .on_hover_text(format!("Fan speed at that temperature ({floor:.0}-100 %)."));
            if (rt.changed() || rd.changed()) && set_point(points, i, t, d, floor) {
                changed = true;
            }
            if ui.add_enabled(points.len() > MIN_POINTS, egui::Button::new("Remove point").small()).on_disabled_hover_text("A curve needs at least 2 points.").clicked() && remove_point(points, i) {
                changed = true;
                sel = None;
            }
        }
        None => {
            ui.label(RichText::new("Click a point to type its exact temperature and speed.").size(11.5).color(theme::TEXT_FAINT));
        }
    });
    let full = points.len() >= MAX_POINTS;
    ui.label(
        RichText::new(if full { "Right-click a point to remove it (the curve has the most points it can hold)." } else { "Click the graph to add a point, right-click a point to remove it." })
            .size(11.5)
            .color(theme::TEXT_FAINT),
    );
    ui.data_mut(|d| match sel {
        Some(i) => d.insert_temp(sel_id, i),
        None => d.remove::<usize>(sel_id),
    });
    changed
}

