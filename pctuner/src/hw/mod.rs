//! Fan Tuner hardware access in Rust (replacing the C# helper in stages).
//! Stage 1: read-only sensors for this PC's hardware - Nuvoton NCT6687D-R board chip and AMD Zen CPU through the
//! PawnIO driver, the NVIDIA GPU through NVAPI. `read()` produces the same `Status` (ids, names, order) the C#
//! helper sends, so the rest of Fan Tuner and the saved profiles work unchanged.

pub mod amdcpu;
pub mod nct6687;
pub mod nvgpu;
pub mod pawnio;

use crate::fan::{FanInfo, Status, TempInfo};

/// The open hardware; any part can be missing (reported in `errors`).
pub struct Sensors {
    board: Option<nct6687::Nct6687>,
    cpu: Option<amdcpu::AmdCpu>,
    gpu: Option<nvgpu::NvGpu>,
    pub errors: Vec<String>,
}

/// LibreHardwareMonitor's pump rule.
pub fn is_pump_name(n: &str) -> bool {
    let l = n.to_lowercase();
    l.contains("pump") || l.contains("aio")
}

impl Sensors {
    pub fn open() -> Sensors {
        fn keep<T>(r: Result<T, String>, what: &str, errors: &mut Vec<String>) -> Option<T> {
            r.map_err(|e| errors.push(format!("{what}: {e}"))).ok()
        }
        let mut errors = vec![];
        let board = keep(nct6687::Nct6687::open(), "motherboard", &mut errors);
        let cpu = keep(amdcpu::AmdCpu::open(), "CPU", &mut errors);
        let gpu = keep(nvgpu::NvGpu::open(), "GPU", &mut errors);
        Sensors { board, cpu, gpu, errors }
    }

    /// One reading, as a status line (no fan is controlled in stage 1: every fan reads as BIOS "default").
    pub fn read(&self) -> Status {
        let mut s = Status::default();
        let temp = |id: String, hw: &str, kind: &str, name: &str, v: f32| TempInfo { id, hw: hw.into(), kind: kind.into(), name: name.into(), value: Some(v) };
        // the helper lists 0-150 °C only
        let plausible = |v: f32| v > 0.0 && v < 150.0;

        if let Some(b) = &self.board {
            if let Ok(r) = b.read_all() {
                for (i, rpm, duty) in &r.fans {
                    let Some((_, _, name)) = nct6687::HEADERS.iter().find(|h| h.0 == *i) else { continue };
                    s.fans.push(FanInfo {
                        id: format!("{}/control/{i}", nct6687::ID_PREFIX),
                        hw: nct6687::HW_NAME.into(),
                        name: (*name).into(),
                        rpm: Some(*rpm),
                        percent: Some(*duty),
                        controllable: true,
                        min: Some(0.0),
                        mode: "default".into(),
                        pump: is_pump_name(name),
                        gpu: false,
                        ..Default::default()
                    });
                }
                for (i, v) in r.temps.iter().filter(|t| plausible(t.1)) {
                    let name = nct6687::TEMPS.iter().find(|t| t.0 == *i).map(|t| t.1).unwrap_or("Temperature");
                    s.temps.push(temp(format!("{}/temperature/{i}", nct6687::ID_PREFIX), nct6687::HW_NAME, "SuperIO", name, *v));
                }
            }
        }
        if let Some(c) = &self.cpu {
            if let Ok(ts) = c.read() {
                let main: Vec<f32> = ts.iter().filter(|t| plausible(t.2)).map(|t| t.2).collect();
                // the helper: the hottest of the Tctl / Tdie / Package sensors
                s.cpu = main.iter().copied().reduce(f32::max);
                for (i, name, v) in ts.into_iter().filter(|t| plausible(t.2)) {
                    s.temps.push(temp(format!("{}/temperature/{i}", amdcpu::ID_PREFIX), &c.name, "Cpu", &name, v));
                }
            }
        }
        if let Some(g) = &self.gpu {
            let ts = g.temperatures();
            s.gpu = ts.iter().find(|t| t.1 == "GPU Core").map(|t| t.2);
            for (i, name, v) in ts.into_iter().filter(|t| plausible(t.2)) {
                s.temps.push(temp(format!("{}/temperature/{i}", nvgpu::ID_PREFIX), &g.name, "GpuNvidia", &name, v));
            }
            let coolers = g.coolers();
            let many = coolers.len() > 1;
            for (k, c) in coolers.iter().enumerate() {
                let name = if many { format!("GPU Fan {}", k + 1) } else { "GPU Fan".to_string() };
                s.fans.push(FanInfo {
                    id: format!("{}/control/{}", nvgpu::ID_PREFIX, c.id),
                    hw: g.name.clone(),
                    name,
                    rpm: Some(c.rpm as f32),
                    percent: Some(c.level as f32),
                    controllable: true,
                    min: Some(c.min as f32),
                    mode: "default".into(),
                    gpu: true,
                    ..Default::default()
                });
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in, needs an elevated prompt (`cargo test hw_same_as_helper -- --ignored --nocapture`): the Rust
    /// reading next to the C# helper's `--dump` taken right after it - same fans, ids and names, values close.
    /// Read-only on both sides.
    #[test]
    #[ignore]
    fn hw_same_as_helper() {
        let sensors = Sensors::open();
        assert!(sensors.errors.is_empty(), "{:?}", sensors.errors);
        let ours = sensors.read();
        let exe = dirs_helper();
        let rep = std::env::temp_dir().join("pctuner-hw-compare.txt");
        let st = std::process::Command::new(&exe).args(["--dump", &rep.display().to_string()]).output().expect("run the C# helper");
        assert!(st.status.success());
        // read again right after: idle CCD temperatures move several degrees within a second
        let after = sensors.read();
        let span = |id: &str| -> (f32, f32) {
            let v: Vec<f32> = [&ours, &after].iter().filter_map(|s| s.temps.iter().find(|t| t.id == id).and_then(|t| t.value)).collect();
            (v.iter().copied().fold(f32::MAX, f32::min), v.iter().copied().fold(f32::MIN, f32::max))
        };
        let dump = std::fs::read_to_string(&rep).unwrap();
        println!("RUST cpu {:?} gpu {:?}", ours.cpu, ours.gpu);
        let mut bad = vec![];
        // fans: "hw | name | rpm=.. | percent=.. | ... | id=.."
        for line in dump.lines().filter(|l| l.contains(" | id=")) {
            let f: Vec<&str> = line.split(" | ").collect();
            let id = f.last().unwrap().trim_start_matches("id=");
            let num = |k: &str| f.iter().find_map(|x| x.strip_prefix(k)).and_then(|v| v.parse::<f32>().ok());
            match ours.fans.iter().find(|x| x.id == id) {
                None => bad.push(format!("missing fan {id}")),
                Some(o) => {
                    let (r1, r2) = (num("rpm=").unwrap_or(0.0), o.rpm.unwrap_or(0.0));
                    let (p1, p2) = (num("percent=").unwrap_or(0.0), o.percent.unwrap_or(0.0));
                    println!("{id:34} {:16} rpm C# {r1:6} Rust {r2:6}   % C# {p1:4} Rust {p2:4}  name C# {:?}", o.name, f[1]);
                    if o.name != f[1] || o.hw != f[0] {
                        bad.push(format!("{id}: name/hw {:?}/{:?} vs {:?}/{:?}", o.name, o.hw, f[1], f[0]));
                    }
                    if (r1 - r2).abs() > (r1 * 0.05).max(60.0) || (p1 - p2).abs() > 3.0 {
                        bad.push(format!("{id}: rpm {r1} vs {r2}, % {p1} vs {p2}"));
                    }
                }
            }
        }
        // temperatures: "    Temperature  <name>  <value>   <id>"
        for line in dump.lines().filter(|l| l.trim_start().starts_with("Temperature ")) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let id = *parts.last().unwrap();
            let v: f32 = parts[parts.len() - 2].parse().unwrap_or(f32::NAN);
            match ours.temps.iter().find(|t| t.id == id) {
                None => bad.push(format!("missing temperature {id}")),
                Some(t) => {
                    let (lo, hi) = span(id);
                    println!("{id:34} {:22} C# {v:5}  Rust {lo:.1}..{hi:.1}", t.name);
                    if v < lo - 1.0 || v > hi + 1.0 {
                        bad.push(format!("{id}: C# {v} outside Rust {lo:.1}..{hi:.1}"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    fn dirs_helper() -> std::path::PathBuf {
        let built = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"fanhelper\out\PCTunerFan.exe");
        if built.exists() {
            built
        } else {
            crate::common::data_dir().join(r"fan\PCTunerFan.exe")
        }
    }
}
