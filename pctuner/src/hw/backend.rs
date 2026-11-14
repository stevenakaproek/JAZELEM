//! The real fans for the controller: drives NCT6687D-R headers and the NVIDIA GPU fan, and always knows how to
//! hand each one back.
//!
//! Before a board header is first driven, its BIOS registers are saved to %LOCALAPPDATA%\PCTuner\fan-bios-state.json
//! (with the boot time). Handing it back writes exactly those registers. If PC Tuner is killed while driving
//! fans, the next start in the same boot finds the file and restores the BIOS state first - so nothing is
//! ever stranded (the C# helper could not restore a header an earlier process had driven). A file from an
//! earlier boot is dropped: the BIOS set everything up again when the PC started.

use super::controller::FanIo;
use super::{is_pump_name, nct6687, nvgpu, Sensors};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const STATE_FILE: &str = "fan-bios-state.json";

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BiosState {
    /// fan::boot_time() when the state was saved.
    pub boot: u64,
    /// Board headers being driven, with their BIOS registers.
    pub headers: BTreeMap<usize, nct6687::HeaderState>,
    /// GPU coolers being driven (handing back = the driver's Auto mode).
    pub gpu: BTreeSet<u32>,
}

impl BiosState {
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.gpu.is_empty()
    }
}

/// Which fan an id is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Board(usize),
    Gpu(u32),
}

pub fn target(id: &str) -> Option<Target> {
    if let Some(n) = id.strip_prefix(&format!("{}/control/", nct6687::ID_PREFIX)) {
        return n.parse().ok().map(Target::Board);
    }
    if let Some(n) = id.strip_prefix(&format!("{}/control/", nvgpu::ID_PREFIX)) {
        return n.parse().ok().map(Target::Gpu);
    }
    None
}

pub struct Hardware {
    pub sensors: Sensors,
    state: BiosState,
    path: PathBuf,
    /// What happened at start (restored from a file left by a killed PC Tuner, ...).
    pub notes: Vec<String>,
}

impl Hardware {
    pub fn open() -> Hardware {
        Self::with_file(Sensors::open(), crate::common::data_dir().join(STATE_FILE))
    }

    fn with_file(sensors: Sensors, path: PathBuf) -> Hardware {
        let mut hw = Hardware { sensors, state: BiosState::default(), path, notes: vec![] };
        hw.recover();
        hw
    }

    /// A state file left behind: same boot -> put the BIOS state back now; earlier boot -> drop it.
    fn recover(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return };
        let Ok(old) = serde_json::from_str::<BiosState>(&text) else {
            self.notes.push("An unreadable fan state file was moved aside.".into());
            let _ = std::fs::rename(&self.path, self.path.with_extension("corrupt.json"));
            return;
        };
        if crate::fan::boot_time().abs_diff(old.boot) > crate::fan::SAME_BOOT_SECS {
            let _ = std::fs::remove_file(&self.path);
            return;
        }
        self.state = old;
        let n = self.state.headers.len() + self.state.gpu.len();
        let failed = self.hand_back_all();
        self.notes.push(if failed.is_empty() {
            format!("PC Tuner was closed while driving {n} fan(s) - they were put back exactly as the BIOS had them.")
        } else {
            format!("PC Tuner was closed while driving fans; these could not be put back yet: {}", failed.join(", "))
        });
    }

    fn save(&mut self) -> Result<(), String> {
        if self.state.is_empty() {
            let _ = std::fs::remove_file(&self.path);
            return Ok(());
        }
        self.state.boot = crate::fan::boot_time();
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.state).map_err(|e| e.to_string())?).map_err(|e| format!("cannot save the fan state: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("cannot save the fan state: {e}"))
    }

    /// Every driven fan back to the BIOS / driver. Returns the ones that failed (they stay recorded).
    pub fn hand_back_all(&mut self) -> Vec<String> {
        let ids: Vec<String> = self
            .state
            .headers
            .keys()
            .map(|i| format!("{}/control/{i}", nct6687::ID_PREFIX))
            .chain(self.state.gpu.iter().map(|c| format!("{}/control/{c}", nvgpu::ID_PREFIX)))
            .collect();
        ids.into_iter().filter(|id| self.set_default(id).is_err()).collect()
    }

    pub fn driven(&self) -> usize {
        self.state.headers.len() + self.state.gpu.len()
    }
}

impl FanIo for Hardware {
    fn controllable(&self, id: &str) -> bool {
        match target(id) {
            Some(Target::Board(i)) => self.sensors.board.is_some() && nct6687::controllable(i),
            Some(Target::Gpu(c)) => self.sensors.gpu.as_ref().is_some_and(|g| g.coolers().iter().any(|x| x.id == c)),
            None => false,
        }
    }

    fn named_pump(&self, id: &str) -> bool {
        match target(id) {
            Some(Target::Board(i)) => nct6687::HEADERS.iter().find(|h| h.0 == i).is_some_and(|h| is_pump_name(h.2)),
            _ => false,
        }
    }

    fn set_software(&mut self, id: &str, duty: f32) -> Result<(), String> {
        match target(id) {
            Some(Target::Board(i)) => {
                if !self.state.headers.contains_key(&i) {
                    let s = self.sensors.board.as_ref().ok_or("no board chip")?.bios_state(i)?;
                    self.state.headers.insert(i, s);
                    // never drive a header whose way back is not on disk
                    if let Err(e) = self.save() {
                        self.state.headers.remove(&i);
                        return Err(e);
                    }
                }
                self.sensors.board.as_ref().ok_or("no board chip")?.set_duty(i, duty)
            }
            Some(Target::Gpu(c)) => {
                if self.state.gpu.insert(c) {
                    if let Err(e) = self.save() {
                        self.state.gpu.remove(&c);
                        return Err(e);
                    }
                }
                let g = self.sensors.gpu.as_ref().ok_or("no GPU")?;
                g.set_level(c, duty).or_else(|e| {
                    // on any NVAPI error the GPU goes back to the driver
                    let _ = g.set_auto(c);
                    Err(e)
                })
            }
            None => Err(format!("unknown fan {id}")),
        }
    }

    fn set_default(&mut self, id: &str) -> Result<(), String> {
        match target(id) {
            Some(Target::Board(i)) => {
                let Some(s) = self.state.headers.get(&i).cloned() else { return Ok(()) }; // never driven: nothing to write
                self.sensors.board.as_ref().ok_or("no board chip")?.restore(&s)?;
                self.state.headers.remove(&i);
                self.save()
            }
            Some(Target::Gpu(c)) => {
                if !self.state.gpu.contains(&c) {
                    return Ok(());
                }
                self.sensors.gpu.as_ref().ok_or("no GPU")?.set_auto(c)?;
                self.state.gpu.remove(&c);
                self.save()
            }
            None => Ok(()),
        }
    }
}

impl Drop for Hardware {
    /// Closing (or a panic unwinding through the fan thread): every driven fan goes back.
    fn drop(&mut self) {
        let _ = self.hand_back_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::controller::Controller;
    use std::time::{Duration, Instant};

    /// Opt-in, elevated (`cargo test hw_control_one_fan -- --ignored --nocapture`). Drives ONE case fan
    /// (System Fan #5) at 40 % then 70 % through the real controller, hands it back, and checks every BIOS
    /// register is byte-identical to before; then the GPU fan at 50 % and back to the driver. Always hands back.
    #[test]
    #[ignore]
    fn hw_control_one_fan() {
        const FAN: &str = "/lpc/nct6687dr/0/control/14";
        const GPU: &str = "/gpu-nvidia/0/control/1";
        let mut hw = Hardware::open();
        assert!(hw.sensors.errors.is_empty(), "{:?}", hw.sensors.errors);
        println!("start notes: {:?}", hw.notes);
        assert_eq!(hw.driven(), 0, "nothing driven at start");
        let raw = |hw: &Hardware| -> Vec<u8> {
            let b = hw.sensors.board.as_ref().unwrap();
            let mut v: Vec<u8> = (0xC10..=0xC1D).map(|a| b.peek(a).unwrap()).collect(); // every curve byte, odd ones too
            v.push(b.peek(0xA00).unwrap()); // manual-mode bits of headers 0-7
            v
        };
        let fan = |hw: &Hardware, id: &str| -> (f32, f32) {
            let s = hw.sensors.read();
            let f = s.fans.iter().find(|f| f.id == id).unwrap();
            (f.rpm.unwrap_or(0.0), f.percent.unwrap_or(0.0))
        };
        let before = raw(&hw);
        let (rpm0, duty0) = fan(&hw, FAN);
        println!("System Fan #5 before: {rpm0} rpm, {duty0} %  registers {before:02X?}");

        let mut c = Controller::new(Instant::now());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // the chip ramps the duty toward its target: tick until it gets there (at most 30 s), logging each second
            let run = |c: &mut Controller, hw: &mut Hardware, target: f32| {
                for sec in 1..=30 {
                    let s = hw.sensors.read();
                    c.tick(hw, Instant::now(), s.cpu, s.gpu);
                    std::thread::sleep(Duration::from_secs(1));
                    let (rpm, d) = fan(hw, FAN);
                    println!("  {sec:2}s  {rpm:5} rpm  duty {d} %");
                    if (d - target).abs() <= 1.0 && sec >= 3 {
                        break;
                    }
                }
            };
            c.handle(&mut hw, &format!("set {FAN} 40"));
            run(&mut c, &mut hw, 40.0);
            let (rpm40, d40) = fan(&hw, FAN);
            println!("at 40 %: {rpm40} rpm, duty {d40} %  (message {:?})", c.message);
            assert!((d40 - 40.0).abs() <= 1.0, "duty follows 40 %");
            assert!(std::path::Path::new(&hw.path).exists(), "BIOS state saved before driving");
            c.handle(&mut hw, &format!("set {FAN} 70"));
            run(&mut c, &mut hw, 70.0);
            let (rpm70, d70) = fan(&hw, FAN);
            println!("at 70 %: {rpm70} rpm, duty {d70} %");
            assert!((d70 - 70.0).abs() <= 1.0, "duty follows 70 %");
            assert!(rpm70 > rpm40 + 100.0, "the fan speeds up ({rpm40} -> {rpm70} rpm)");
        }));
        c.handle(&mut hw, &format!("auto {FAN}"));
        std::thread::sleep(Duration::from_secs(5));
        let after = raw(&hw);
        let (rpm1, duty1) = fan(&hw, FAN);
        println!("handed back: {rpm1} rpm, {duty1} %  registers {after:02X?}");
        assert_eq!(after, before, "every BIOS register back byte for byte");
        assert_eq!(hw.driven(), 0);
        assert!(!std::path::Path::new(&hw.path).exists(), "state file removed once everything is back");
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }

        // the GPU fan: 50 %, then back to the driver
        let g = hw.sensors.gpu.as_ref().unwrap();
        let was_manual = g.is_manual(1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            hw.set_software(GPU, 50.0).unwrap();
            std::thread::sleep(Duration::from_secs(4));
            let (rpm, level) = fan(&hw, GPU);
            println!("GPU at 50 %: {rpm} rpm, level {level} %");
            assert!((level - 50.0).abs() <= 2.0);
        }));
        hw.set_default(GPU).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let manual = hw.sensors.gpu.as_ref().unwrap().is_manual(1);
        println!("GPU before {was_manual:?}, after {manual:?}: {:?}", fan(&hw, GPU));
        assert_eq!(manual, Some(false), "back on the driver's curve");
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn backend_ids_and_state_file() {
        assert_eq!(target("/lpc/nct6687dr/0/control/14"), Some(Target::Board(14)));
        assert_eq!(target("/gpu-nvidia/0/control/1"), Some(Target::Gpu(1)));
        assert_eq!(target("/lpc/nct6687d/0/control/1"), None, "another chip's id is never driven");
        assert_eq!(target("/gpu-nvidia/0/fan/1"), None);

        // no hardware opened (Sensors with nothing): ids are not controllable, a state file from an earlier
        // boot is dropped, one from this boot is kept when it cannot be restored
        let none = || Sensors { board: None, cpu: None, gpu: None, errors: vec![] };
        let dir = std::env::temp_dir().join(format!("pctuner-backend-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(STATE_FILE);
        let state = |boot| BiosState {
            boot,
            headers: [(14usize, nct6687::HeaderState { index: 14, manual_bit: None, registers: vec![(0xC10, 60)] })].into(),
            gpu: BTreeSet::new(),
        };
        std::fs::write(&path, serde_json::to_string(&state(1)).unwrap()).unwrap();
        let hw = Hardware::with_file(none(), path.clone());
        assert!(!path.exists() && hw.notes.is_empty() && hw.driven() == 0, "earlier boot: dropped");
        drop(hw);
        std::fs::write(&path, serde_json::to_string(&state(crate::fan::boot_time())).unwrap()).unwrap();
        let hw = Hardware::with_file(none(), path.clone());
        assert!(hw.notes[0].contains("could not be put back") && hw.driven() == 1 && path.exists(), "same boot, no chip: kept for later");
        assert!(!hw.controllable("/lpc/nct6687dr/0/control/14"));
        assert!(hw.named_pump("/lpc/nct6687dr/0/control/1") && !hw.named_pump("/lpc/nct6687dr/0/control/0"));
        std::mem::forget(hw); // Drop would try the (missing) chip again
        std::fs::write(&path, "not json").unwrap();
        let hw = Hardware::with_file(none(), path.clone());
        assert!(hw.notes[0].contains("unreadable") && !path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
