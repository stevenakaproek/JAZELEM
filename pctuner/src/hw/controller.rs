//! The fan safety loop, ported from the old C# helper's Controller / Safety (fanhelper/Program.cs, removed), now
//! in-process.
//! It has no hardware or clock of its own (tests drive it with a fake), and takes the same command lines
//! Fan Tuner already sends: set / curve / auto / autoall / pump / ping.
//!
//! Rules (unchanged from the helper):
//!   - fans never below 20 %, pumps never below 50 %
//!   - any CPU or GPU temperature >= 90 °C: every controlled fan 100 % until both are below 80 °C
//!   - temperatures unreadable for > 5 s: every fan back to the BIOS until they read again (settings kept)
//!   - curves follow their temperature with a 3 °C hysteresis on the way down
//! The helper's watchdog and stale-default handling are gone: the controller lives inside PC Tuner, and the
//! BIOS state of each header is saved to disk before it is first driven (see nct6687), so it can always be
//! put back exactly.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const FAN_FLOOR: f32 = 20.0;
pub const PUMP_FLOOR: f32 = 50.0;
pub const HOT_ON: f32 = 90.0;
pub const HOT_OFF: f32 = 80.0;
pub const HYSTERESIS: f32 = 3.0;
pub const BLIND_AFTER: Duration = Duration::from_secs(5);

/// The fans as the controller sees them (the real one is hw::Hardware; tests use a fake).
pub trait FanIo {
    fn controllable(&self, id: &str) -> bool;
    /// The header's name says it is a pump.
    fn named_pump(&self, id: &str) -> bool;
    /// Drive the fan at this duty (already floored by the controller).
    fn set_software(&mut self, id: &str, duty: f32) -> Result<(), String>;
    /// Hand the fan back to the BIOS / graphics driver (exactly as it was before it was first driven).
    fn set_default(&mut self, id: &str) -> Result<(), String>;
}

/// Requested duty -> the duty that is allowed.
pub fn clamp(percent: f32, pump: bool) -> f32 {
    let p = if percent.is_nan() { 100.0 } else { percent };
    p.min(100.0).max(if pump { PUMP_FLOOR } else { FAN_FLOOR })
}

/// Emergency latch: on at >= 90 °C, off only when every known temperature is below 80 °C.
pub fn emergency(latched: bool, cpu: Option<f32>, gpu: Option<f32>) -> bool {
    if cpu.unwrap_or(0.0) >= HOT_ON || gpu.unwrap_or(0.0) >= HOT_ON {
        return true;
    }
    if !latched {
        return false;
    }
    let cool = (cpu.is_some() || gpu.is_some()) && cpu.unwrap_or(0.0) < HOT_OFF && gpu.unwrap_or(0.0) < HOT_OFF;
    !cool
}

/// Temperature a curve follows: rises at once, falls only after a 3 °C drop.
pub fn hysteresis(last: Option<f32>, temp: f32) -> f32 {
    match last {
        Some(l) if temp < l && l - temp < HYSTERESIS => l,
        _ => temp,
    }
}

/// "30:25,50:40" -> points sorted by temperature (stable); None when it cannot be read (1-16 points).
pub fn parse_points(s: &str) -> Option<Vec<[f32; 2]>> {
    let mut v = vec![];
    for p in s.split(',').filter(|p| !p.is_empty()) {
        let (t, d) = p.split_once(':')?;
        let (t, d): (f32, f32) = (t.trim().parse().ok()?, d.trim().parse().ok()?);
        if t.is_nan() || d.is_nan() {
            return None;
        }
        v.push([t, d.clamp(0.0, 100.0)]);
    }
    if v.is_empty() || v.len() > 16 {
        return None;
    }
    v.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Some(v)
}

struct Curve {
    source: String,
    points: Vec<[f32; 2]>,
    last: Option<f32>,
}

pub struct Controller {
    requested: BTreeMap<String, f32>,
    curves: BTreeMap<String, Curve>,
    touched: BTreeSet<String>,
    pump_override: BTreeSet<String>,
    not_pump: BTreeSet<String>,
    last_temps: Instant,
    pub emergency: bool,
    pub blind: bool,
    pub message: String,
    pub message_seq: u64,
}

impl Controller {
    pub fn new(now: Instant) -> Controller {
        Controller {
            requested: BTreeMap::new(),
            curves: BTreeMap::new(),
            touched: BTreeSet::new(),
            pump_override: BTreeSet::new(),
            not_pump: BTreeSet::new(),
            last_temps: now,
            emergency: false,
            blind: false,
            message: String::new(),
            message_seq: 0,
        }
    }

    fn say(&mut self, m: &str) {
        if m != self.message {
            self.message = m.to_string();
            self.message_seq += 1;
        }
    }

    pub fn requested(&self, id: &str) -> Option<f32> {
        self.requested.get(id).copied()
    }
    pub fn has_curve(&self, id: &str) -> bool {
        self.curves.contains_key(id)
    }
    pub fn is_pump(&self, io: &dyn FanIo, id: &str) -> bool {
        !self.not_pump.contains(id) && (io.named_pump(id) || self.pump_override.contains(id))
    }
    /// The controller is driving this fan right now.
    pub fn is_touched(&self, id: &str) -> bool {
        self.touched.contains(id)
    }
    /// The user said this named pump is not a pump.
    pub fn is_not_pump(&self, id: &str) -> bool {
        self.not_pump.contains(id)
    }

    fn give_back(&mut self, io: &mut dyn FanIo, id: &str) {
        self.requested.remove(id);
        self.curves.remove(id);
        if self.touched.remove(id) && io.controllable(id) {
            if let Err(e) = io.set_default(id) {
                self.say(&format!("could not hand {id} back to the BIOS: {e}"));
            }
        }
    }

    /// One command line from Fan Tuner.
    pub fn handle(&mut self, io: &mut dyn FanIo, line: &str) {
        let parts: Vec<&str> = line.trim_matches(['\u{feff}', ' ', '\t', '\r', '\n']).split(' ').filter(|p| !p.is_empty()).collect();
        match parts.as_slice() {
            [] | ["ping", ..] => {}
            ["set", id, pct, ..] => {
                if !io.controllable(id) {
                    self.say(&format!("not controllable: {id}"));
                } else if let Ok(p) = pct.parse::<f32>() {
                    self.curves.remove(*id);
                    self.requested.insert(id.to_string(), p);
                } else {
                    self.say(&format!("bad percent: {pct}"));
                }
            }
            ["curve", id, src, pts, ..] => {
                if !io.controllable(id) {
                    self.say(&format!("not controllable: {id}"));
                } else if let Some(points) = parse_points(pts) {
                    let source = if *src == "cpu" || *src == "gpu" { src.to_string() } else { "hottest".into() };
                    let c = self.curves.entry(id.to_string()).or_insert(Curve { source: String::new(), points: vec![], last: None });
                    c.source = source;
                    c.points = points;
                } else {
                    self.say(&format!("bad curve: {pts}"));
                }
            }
            ["auto", id, ..] => self.give_back(io, id),
            ["autoall", ..] => {
                let all: BTreeSet<String> = self.touched.iter().chain(self.requested.keys()).chain(self.curves.keys()).cloned().collect();
                for id in all {
                    self.give_back(io, &id);
                }
            }
            ["pump", id, v, ..] => {
                if *v == "1" {
                    self.pump_override.insert(id.to_string());
                    self.not_pump.remove(*id);
                } else {
                    self.pump_override.remove(*id);
                    if io.named_pump(id) {
                        self.not_pump.insert(id.to_string());
                    }
                }
            }
            [other, ..] => self.say(&format!("unknown command: {other}")),
        }
    }

    fn run_curves(&mut self, cpu: Option<f32>, gpu: Option<f32>) {
        let hottest = if cpu.is_some() || gpu.is_some() { Some(cpu.unwrap_or(0.0).max(gpu.unwrap_or(0.0))) } else { None };
        for (id, c) in self.curves.iter_mut() {
            let src = match c.source.as_str() {
                "cpu" => cpu,
                "gpu" => gpu,
                _ => hottest,
            }
            .or(hottest); // the chosen sensor is missing: follow whichever is known
            let Some(t) = src else { continue }; // none at all: the blind rule hands the fan back
            let t = hysteresis(c.last, t);
            c.last = Some(t);
            self.requested.insert(id.clone(), crate::fan::curve_duty(&c.points, t));
        }
    }

    fn apply_all(&mut self, io: &mut dyn FanIo) {
        if self.blind {
            return;
        }
        let reqs: Vec<(String, f32)> = self.requested.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (id, v) in reqs {
            if !io.controllable(&id) {
                continue;
            }
            let duty = if self.emergency { 100.0 } else { clamp(v, self.is_pump(io, &id)) };
            match io.set_software(&id, duty) {
                Ok(()) => {
                    self.touched.insert(id);
                }
                Err(e) => self.say(&format!("control failed: {e}")),
            }
        }
    }

    /// One second of the safety loop.
    pub fn tick(&mut self, io: &mut dyn FanIo, now: Instant, cpu: Option<f32>, gpu: Option<f32>) {
        if cpu.is_some() || gpu.is_some() {
            self.last_temps = now;
        }
        self.emergency = emergency(self.emergency, cpu, gpu);
        self.run_curves(cpu, gpu);
        let blind = now.duration_since(self.last_temps) > BLIND_AFTER;
        if blind && !self.blind {
            self.blind = true;
            let controlling = !self.touched.is_empty() || !self.requested.is_empty() || !self.curves.is_empty();
            for id in std::mem::take(&mut self.touched) {
                let _ = io.set_default(&id);
            }
            if controlling {
                self.say("Temperatures could not be read - fans were given back to the BIOS until they can be read again.");
            }
        } else if !blind && self.blind {
            self.blind = false;
            if !self.requested.is_empty() || !self.curves.is_empty() {
                self.say("Temperatures can be read again - PC Tuner controls the fans again.");
            }
        }
        self.apply_all(io);
    }

    /// Every touched fan back to the BIOS; every request dropped (exit, panic, giving everything back).
    pub fn reset_all(&mut self, io: &mut dyn FanIo) {
        for id in std::mem::take(&mut self.touched) {
            let _ = io.set_default(&id);
        }
        self.requested.clear();
        self.curves.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Fake fans: a header's set_default restores what it had before its first set_software (like the real
    /// backend, which saves the BIOS registers first); never driven = nothing written.
    #[derive(Default)]
    struct Fake {
        ctl: HashSet<String>,
        pumps: HashSet<String>,
        duty: HashMap<String, f32>,
        defaulted: HashSet<String>,
        saved: HashMap<String, Option<f32>>,
    }

    impl FanIo for Fake {
        fn controllable(&self, id: &str) -> bool {
            self.ctl.contains(id)
        }
        fn named_pump(&self, id: &str) -> bool {
            self.pumps.contains(id)
        }
        fn set_software(&mut self, id: &str, duty: f32) -> Result<(), String> {
            if !self.saved.contains_key(id) {
                self.saved.insert(id.into(), self.duty.get(id).copied());
            }
            self.duty.insert(id.into(), duty);
            self.defaulted.remove(id);
            Ok(())
        }
        fn set_default(&mut self, id: &str) -> Result<(), String> {
            let Some(back) = self.saved.remove(id) else { return Ok(()) };
            match back {
                Some(v) => {
                    self.duty.insert(id.into(), v);
                }
                None => {
                    self.duty.remove(id);
                    self.defaulted.insert(id.into());
                }
            }
            Ok(())
        }
    }

    fn set<'a>(v: &[&'a str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn controller_rules() {
        assert_eq!(clamp(5.0, false), 20.0);
        assert_eq!(clamp(5.0, true), 50.0);
        assert_eq!(clamp(150.0, false), 100.0);
        assert_eq!(clamp(f32::NAN, false), 100.0);
        assert_eq!(clamp(65.0, false), 65.0);
        assert!(!emergency(false, Some(89.9), Some(60.0)));
        assert!(emergency(false, Some(90.0), Some(60.0)));
        assert!(emergency(false, Some(50.0), Some(91.0)));
        assert!(emergency(true, Some(85.0), Some(60.0)), "stays on above 80");
        assert!(!emergency(true, Some(79.0), Some(70.0)));
        assert!(emergency(true, None, None), "stays on when unreadable");
        assert!(!emergency(true, Some(79.0), None));
        let p = parse_points("70:70,30:25,50:40,85:100").unwrap();
        assert_eq!(p[0], [30.0, 25.0]);
        assert!(parse_points("30:x").is_none() && parse_points("").is_none() && parse_points("30").is_none());
        let dup = parse_points("28:82,28:54").unwrap();
        assert_eq!((dup[0][1], dup[1][1]), (82.0, 54.0), "stable sort");
        assert_eq!(hysteresis(None, 60.0), 60.0);
        assert_eq!(hysteresis(Some(60.0), 65.0), 65.0);
        assert_eq!(hysteresis(Some(65.0), 63.0), 65.0);
        assert_eq!(hysteresis(Some(65.0), 62.0), 62.0);
        assert!(parse_points(&(0..17).map(|i| format!("{}:50", 20 + i * 2)).collect::<Vec<_>>().join(",")).is_none(), "more than 16");
    }

    /// The C# helper's ControllerTests, same steps and expectations (minus its watchdog / reclaim, which an
    /// in-process controller no longer needs).
    #[test]
    fn controller_loop() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut io = Fake { ctl: set(&["fan", "pump", "other"]), pumps: set(&["pump"]), ..Default::default() };
        let mut c = Controller::new(t0);
        c.handle(&mut io, "set fan 5");
        c.handle(&mut io, "set pump 5");
        c.handle(&mut io, "set nope 50");
        c.tick(&mut io, at(1.0), Some(50.0), Some(50.0));
        assert_eq!((io.duty["fan"], io.duty["pump"]), (20.0, 50.0), "floors");
        assert_eq!((c.message.as_str(), c.message_seq), ("not controllable: nope", 1));
        c.tick(&mut io, at(2.0), Some(50.0), Some(50.0));
        assert_eq!(c.message_seq, 1, "the notice stays");
        c.handle(&mut io, "pump fan 1");
        c.handle(&mut io, "pump pump 0");
        c.tick(&mut io, at(3.0), Some(50.0), Some(50.0));
        assert_eq!((io.duty["fan"], io.duty["pump"]), (50.0, 20.0), "pump ticks move the floor both ways");
        c.handle(&mut io, "pump fan 0");
        c.handle(&mut io, "pump pump 1");
        c.handle(&mut io, "set fan 40");
        c.handle(&mut io, "set pump 60");
        c.tick(&mut io, at(4.0), Some(90.0), Some(50.0));
        assert!(c.emergency && io.duty["fan"] == 100.0 && io.duty["pump"] == 100.0, "90 °C -> 100 %");
        c.tick(&mut io, at(5.0), Some(85.0), Some(50.0));
        assert!(c.emergency && io.duty["fan"] == 100.0, "stays above 80");
        c.tick(&mut io, at(6.0), Some(79.0), Some(70.0));
        assert!(!c.emergency && io.duty["fan"] == 40.0 && io.duty["pump"] == 60.0, "back to the requested duties");
        c.handle(&mut io, "curve other cpu 28:82,28:54");
        c.tick(&mut io, at(7.0), Some(20.0), Some(50.0));
        assert!(io.duty["other"] == 82.0 && c.has_curve("other"), "a curve drives its fan");

        // blind: > 5 s without temperatures -> BIOS, settings kept, back when temperatures return
        c.tick(&mut io, at(10.0), None, None);
        assert!(!c.blind && io.duty["fan"] == 40.0, "3 s is not blind yet");
        let seq = c.message_seq;
        c.tick(&mut io, at(13.0), None, None);
        assert!(c.blind && io.duty.is_empty() && io.defaulted == set(&["fan", "pump", "other"]), "blind -> every fan to the BIOS");
        assert!(c.message_seq == seq + 1 && c.message.contains("could not be read"));
        assert!(c.requested("fan") == Some(40.0) && c.has_curve("other"), "settings kept");
        c.tick(&mut io, at(14.0), None, None);
        assert!(io.duty.is_empty(), "nothing driven while blind");
        c.tick(&mut io, at(15.0), Some(50.0), None);
        assert!(!c.blind && io.duty["fan"] == 40.0 && io.duty["pump"] == 60.0 && io.duty["other"] == 54.0, "settings come back");
        assert!(c.message.contains("again"));

        c.reset_all(&mut io);
        assert!(io.duty.is_empty() && io.defaulted == set(&["fan", "pump", "other"]) && c.requested("fan").is_none() && !c.has_curve("other"));

        // auto / autoall
        let mut io2 = Fake { ctl: set(&["a", "b"]), ..Default::default() };
        let mut c2 = Controller::new(t0);
        c2.handle(&mut io2, "set a 40");
        c2.handle(&mut io2, "curve b gpu 30:30,80:100");
        c2.tick(&mut io2, at(1.0), Some(50.0), Some(50.0));
        c2.handle(&mut io2, "auto a");
        assert!(io2.defaulted.contains("a") && c2.requested("a").is_none());
        c2.handle(&mut io2, "autoall");
        assert!(io2.duty.is_empty() && !c2.has_curve("b") && io2.defaulted.contains("b"));
        // a fan never driven: auto writes nothing
        let mut io3 = Fake { ctl: set(&["x"]), ..Default::default() };
        io3.duty.insert("x".into(), 33.0);
        let mut c3 = Controller::new(t0);
        c3.handle(&mut io3, "auto x");
        assert_eq!(io3.duty["x"], 33.0);
    }
}
