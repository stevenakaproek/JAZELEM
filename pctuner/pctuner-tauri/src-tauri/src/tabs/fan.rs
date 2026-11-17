//! Fan Tuner. The engine (helper process, safety watch, settings) runs on its own thread in pctuner-core,
//! whether or not a window is open; this backend only builds the view and forwards what the user did.

use crate::backend::{parse, Backend};
use pctuner_core::common::{self, Tone};
use pctuner_core::fan::*;
use serde_json::{json, Value};
use std::collections::VecDeque;

pub struct FanTab {
    e: FanEngine,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    GiveBack,
    Dismiss { seq: u64 },
    /// Silent / Balanced / Performance for one fan.
    Preset { id: String, name: String },
    /// BIOS / Manual / Curve / Trigger for one fan (starts from the same defaults as the classic app).
    Mode { id: String, mode: String },
    Manual { id: String, percent: f32 },
    Source { id: String, source: Source },
    CurvePoint { id: String, index: usize, t: f32, d: f32 },
    CurveAdd { id: String, t: f32, d: f32 },
    CurveRemove { id: String, index: usize },
    Trigger { id: String, idle_temp: f32, load_temp: f32, idle: f32, load: f32 },
    Pump { id: String, on: bool },
    /// Choose the speed sensor of a fan ("auto" = the built-in pairing).
    PairSensor { id: String, sensor: String },
    /// Start a fan test: "calibrate" or "pair".
    Test { id: String, test: String },
    CancelTest,
}

fn hist(h: Option<&VecDeque<f32>>) -> Vec<f32> {
    h.map(|h| h.iter().copied().collect()).unwrap_or_default()
}

impl FanTab {
    pub fn new() -> Self {
        let mut fan = Fan::new();
        // presets are set per fan here: fans that followed a profile keep their setting as their own
        fan.adopt_profile_settings();
        // the telemetry band shows the CPU at all times: as administrator the helper starts with the app (it only
        // reads sensors until a fan is set)
        if common::is_admin() {
            fan.ensure_started();
        }
        FanTab { e: FanEngine::new(fan) }
    }

    fn fan_card(f: &Fan, s: &Status, info: &FanInfo, rpm: Option<&VecDeque<f32>>, can: bool) -> Value {
        let setting = f.settings.get(&info.id).cloned().unwrap_or_default();
        let ours = setting != FanSetting::Bios;
        let handed_back = f.handed_back.contains(&info.id);
        let chip = if info.controllable {
            let (text, tone) = fan_chip(info, &setting, can, s.emergency, s.blind, handed_back);
            json!({ "text": text, "tone": tone })
        } else {
            json!({ "text": "read-only", "tone": Tone::Off })
        };
        let notice = if !can && ours {
            Some("Saved setting below - not applied (view-only).")
        } else if can && ours && handed_back {
            Some("The helper did not take this setting, so the BIOS drives this fan. PC Tuner tries again every few seconds.")
        } else {
            None
        };
        let presets: Vec<Value> = BUILTIN
            .iter()
            .filter(|n| fan_preset(n, info, &f.store.pumps).is_some())
            .map(|n| json!({ "name": n, "active": preset_of(&setting, info, &f.store.pumps) == Some(*n) }))
            .collect();
        let source = match &setting {
            FanSetting::Curve { source, .. } | FanSetting::Trigger { source, .. } => Some(*source),
            _ => None,
        };
        let live = source.and_then(|src| src.read(s.cpu, s.gpu));
        // where the fan is on its curve right now
        let live_duty = match (&setting, live) {
            (FanSetting::Curve { points, .. }, Some(t)) => Some(curve_duty(points, t.clamp(GRAPH_T.0, GRAPH_T.1)).max(floor_for(info.pump, info.min))),
            (FanSetting::Trigger { load_temp, idle, load, .. }, Some(t)) => Some(info.percent.unwrap_or(if t >= *load_temp { *load } else { *idle })),
            _ => None,
        };
        json!({
            "id": info.id,
            "name": info.name,
            "hw": info.hw,
            "rpm": info.rpm,
            "percent": info.percent,
            "controllable": info.controllable,
            "pump": info.pump,
            "gpu": info.gpu,
            "chip": chip,
            "hist": hist(rpm),
            "floor": floor_for(info.pump, info.min),
            "setting": setting,
            "notice": notice,
            "presets": presets,
            "live": live,
            "liveDuty": live_duty,
            "afterburner": info.gpu && ours && f.afterburner_running(),
            "tools": Self::tools(f, s, info, can),
        })
    }

    /// Speed sensor pairing and calibration for one controllable fan (null for read-only fans and pumps).
    fn tools(f: &Fan, s: &Status, info: &FanInfo, can: bool) -> Value {
        if !info.controllable || info.pump {
            return Value::Null;
        }
        let label = |r: &RpmInfo| format!("{} ({}){}", r.name, r.hw, r.rpm.map(|v| format!(" - {v:.0} RPM")).unwrap_or_default());
        let mut sensors: Vec<Value> = s.rpms.iter().map(|r| json!({ "id": r.id, "label": label(r) })).collect();
        sensors.insert(0, json!({ "id": "none", "label": "None (no speed reading)" }));
        let running = s.job.as_ref().filter(|j| !j.done);
        let mine = running.filter(|j| j.id == info.id);
        let last = s.job.as_ref().filter(|j| j.done && j.id == info.id).map(|j| {
            let ok = j.error.is_empty();
            let text = if !ok {
                j.error.clone()
            } else if j.kind == "pair" {
                let name = s.rpms.iter().find(|r| r.id == j.paired).map(|r| r.name.clone()).unwrap_or_else(|| j.paired.clone());
                format!("Paired with {name}.")
            } else {
                "Calibration finished.".to_string()
            };
            json!({ "ok": ok, "text": text })
        });
        let calib = f.store.calib.get(&info.id).map(|c| json!({ "summary": c.summary(), "points": c.points, "when": c.when }));
        json!({
            "sensors": sensors,
            "sensor": if info.rpm_id.is_empty() { "none".to_string() } else { info.rpm_id.clone() },
            "manual": f.store.pairs.contains_key(&info.id),
            "calib": calib,
            "test": mine.map(|j| json!({ "kind": j.kind, "step": j.step, "steps": j.steps, "duty": j.duty })),
            "otherRunning": running.is_some() && mine.is_none(),
            "result": last,
            "canTest": can && common::is_admin() && running.is_none() && !s.emergency && !s.blind,
        })
    }

    fn floor(f: &Fan, id: &str) -> Option<(f32, FanInfo)> {
        let (status, ..) = f.snapshot();
        let info = status?.fans.into_iter().find(|x| x.id == id)?;
        Some((floor_for(info.pump, info.min), info))
    }
}

impl Backend for FanTab {
    // the engine thread does the periodic work

    fn on_exit(&mut self) {
        self.e.shutdown();
    }

    fn view(&self) -> Value {
        let mut f = self.e.lock();
        // the page is open: start the helper the first time (view-only shows the graphics card)
        f.ensure_started();
        let can = f.can();
        let (status, rpm_hist, _, _, stale_for) = f.snapshot();
        let (helper_gone, stuck) = (f.helper_is_gone(), f.fans_stuck());
        let sources: Vec<Value> = [Source::Cpu, Source::Gpu, Source::Hottest].iter().map(|s| json!({ "key": s, "label": s.label() })).collect();
        let mut v = json!({
            "can": can,
            "viewOnly": !common::is_admin() && !f.demo,
            "error": f.error,
            "anyControlled": f.any_controlled(),
            "note": f.note,
            "sources": sources,
            "graph": { "tMin": (GRAPH_T.0), "tMax": (GRAPH_T.1), "minPoints": MIN_POINTS, "maxPoints": MAX_POINTS },
        });
        if let Some(age) = stale_for {
            // old numbers are not shown as live (and a stale 90 C banner never stays up)
            let (head, sub) = stale_text(helper_gone, stuck, age.as_secs());
            v["stale"] = json!({ "head": head, "sub": sub });
            return v;
        }
        let Some(s) = status else {
            if helper_gone {
                let (head, sub) = stale_text(true, stuck, 0);
                v["stale"] = json!({ "head": head, "sub": sub });
            } else {
                v["reading"] = json!(true);
            }
            return v;
        };
        // headers that have never spun (nothing plugged in) go into a folded group at the end
        let (shown, empty): (Vec<&FanInfo>, Vec<&FanInfo>) = s.fans.iter().partition(|x| {
            x.gpu
                || f.settings.get(&x.id).map(|st| *st != FanSetting::Bios).unwrap_or(false)
                || rpm_hist.get(&x.id).map(|h| h.iter().any(|v| *v > 0.0)).unwrap_or(false)
                || x.rpm.unwrap_or(0.0) > 0.0
        });
        let cards = |list: &[&FanInfo]| -> Vec<Value> { list.iter().map(|x| Self::fan_card(&f, &s, x, rpm_hist.get(&x.id), can)).collect() };
        v["status"] = json!({
            "emergency": s.emergency,
            "blind": s.blind,
            "msg": if !s.msg.is_empty() && s.msg_seq != f.msg_dismissed { json!({ "seq": s.msg_seq, "text": s.msg }) } else { Value::Null },
            "fansFound": s.fans.len(),
            "controllable": s.fans.iter().filter(|x| x.controllable).count(),
            "fans": cards(&shown),
            "empty": cards(&empty),
        });
        v
    }

    fn act(&mut self, action: Value) -> Result<Value, String> {
        let action = parse::<Action>(action)?;
        let mut f = self.e.lock();
        if let Action::Dismiss { seq } = action {
            f.msg_dismissed = seq;
            return Ok(Value::Null);
        }
        // a view-only PC Tuner never changes a fan or a profile
        if !f.can() {
            return Ok(Value::Null);
        }
        match action {
            Action::Dismiss { .. } => {}
            Action::GiveBack => f.give_all_back(),
            Action::Preset { id, name } => {
                if let Some((floor, info)) = Self::floor(&f, &id) {
                    if let Some(s) = fan_preset(&name, &info, &f.store.pumps) {
                        f.apply(id, floor, CardAction::Set(s));
                    }
                }
            }
            Action::PairSensor { id, sensor } => {
                let known = sensor == "auto" || sensor == "none" || f.snapshot().0.is_some_and(|s| s.rpms.iter().any(|r| r.id == sensor));
                if known && f.running_test().is_none() {
                    f.pair(&id, &sensor);
                }
            }
            Action::Test { id, test } => {
                if let Err(e) = f.start_test(&test, &id) {
                    f.note = Some(e);
                }
            }
            Action::CancelTest => f.cancel_test(),
            Action::Pump { id, on } => {
                if let Some((floor, _)) = Self::floor(&f, &id) {
                    f.apply(id, floor, CardAction::Pump(on));
                }
            }
            Action::Mode { id, mode } => {
                let Some((floor, info)) = Self::floor(&f, &id) else { return Ok(Value::Null) };
                let old = f.settings.get(&id).cloned().unwrap_or_default();
                let new = match mode.as_str() {
                    "bios" if old != FanSetting::Bios => Some(FanSetting::Bios),
                    "manual" if !matches!(old, FanSetting::Manual { .. }) => Some(FanSetting::Manual { percent: info.percent.unwrap_or(50.0).max(floor).round() }),
                    "curve" if !matches!(old, FanSetting::Curve { .. }) => Some(default_curve(&info.name, info.gpu, info.pump, floor)),
                    "trigger" if !matches!(old, FanSetting::Trigger { .. }) => Some(default_trigger(&info.name, info.gpu, info.pump, floor)),
                    _ => None,
                };
                if let Some(s) = new {
                    f.apply(id, floor, CardAction::Set(s));
                }
            }
            Action::Manual { id, percent } => {
                if let Some((floor, _)) = Self::floor(&f, &id) {
                    if matches!(f.settings.get(&id), Some(FanSetting::Manual { .. })) && percent.is_finite() {
                        f.apply(id, floor, CardAction::Set(FanSetting::Manual { percent: percent.round().clamp(floor, 100.0) }));
                    }
                }
            }
            Action::Source { id, source } => {
                let Some((floor, _)) = Self::floor(&f, &id) else { return Ok(Value::Null) };
                let new = match f.settings.get(&id).cloned() {
                    Some(FanSetting::Curve { points, .. }) => Some(FanSetting::Curve { source, points }),
                    Some(FanSetting::Trigger { idle_temp, load_temp, idle, load, .. }) => Some(FanSetting::Trigger { source, idle_temp, load_temp, idle, load }),
                    _ => None,
                };
                if let Some(s) = new {
                    f.apply(id, floor, CardAction::Set(s));
                }
            }
            Action::CurvePoint { id, index, t, d } => {
                let Some((floor, _)) = Self::floor(&f, &id) else { return Ok(Value::Null) };
                if let Some(FanSetting::Curve { source, mut points }) = f.settings.get(&id).cloned() {
                    if set_point(&mut points, index, t, d, floor) {
                        f.apply(id, floor, CardAction::Set(FanSetting::Curve { source, points }));
                    }
                }
            }
            Action::CurveAdd { id, t, d } => {
                let Some((floor, _)) = Self::floor(&f, &id) else { return Ok(Value::Null) };
                if let Some(FanSetting::Curve { source, mut points }) = f.settings.get(&id).cloned() {
                    if add_point(&mut points, t, d, floor).is_some() {
                        f.apply(id, floor, CardAction::Set(FanSetting::Curve { source, points }));
                    }
                }
            }
            Action::CurveRemove { id, index } => {
                let Some((floor, _)) = Self::floor(&f, &id) else { return Ok(Value::Null) };
                if let Some(FanSetting::Curve { source, mut points }) = f.settings.get(&id).cloned() {
                    if remove_point(&mut points, index) {
                        f.apply(id, floor, CardAction::Set(FanSetting::Curve { source, points }));
                    }
                }
            }
            Action::Trigger { id, idle_temp, load_temp, idle, load } => {
                let Some((floor, _)) = Self::floor(&f, &id) else { return Ok(Value::Null) };
                if let Some(FanSetting::Trigger { source, idle_temp: old_it, load_temp: old_lt, idle: old_i, load: old_l }) = f.settings.get(&id).cloned() {
                    // keep the gaming point above the idle point, moving whichever one was not just edited
                    let mut it = idle_temp;
                    if load_temp != old_lt && load_temp < it + 2.0 {
                        it = load_temp - 2.0;
                    }
                    let fixed = fix_trigger(it, load_temp, idle, load, floor);
                    if fixed != (old_it, old_lt, old_i, old_l) {
                        let (idle_temp, load_temp, idle, load) = fixed;
                        f.apply(id, floor, CardAction::Set(FanSetting::Trigger { source, idle_temp, load_temp, idle, load }));
                    }
                }
            }
        }
        // send the change to the helper now instead of at the engine's next tick
        f.sync();
        Ok(Value::Null)
    }
}
