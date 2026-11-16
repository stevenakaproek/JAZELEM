//! Cheap, read-only system readings for the telemetry band that need no administrator rights and no helper:
//! memory in use, total CPU load and the processor's name.

use winreg::enums::HKEY_LOCAL_MACHINE;
use winreg::RegKey;

/// Physical memory: (used MB, total MB). None when Windows does not answer.
pub fn memory() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return None;
    }
    let mb = 1024 * 1024;
    Some(((m.ullTotalPhys - m.ullAvailPhys) / mb, m.ullTotalPhys / mb))
}

/// The processor's name as Windows shows it ("AMD Ryzen 7 9800X3D 8-Core Processor" -> without the tail).
pub fn cpu_name() -> String {
    let raw: String = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|k| k.get_value("ProcessorNameString"))
        .unwrap_or_default();
    short_cpu_name(&raw)
}

pub fn short_cpu_name(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for cut in [" with ", " @ "] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    let words: Vec<&str> = s.split_whitespace().filter(|w| !w.ends_with("-Core") && !matches!(*w, "Processor" | "CPU" | "(R)" | "(TM)")).collect();
    let s = words.join(" ").replace("(R)", "").replace("(TM)", "");
    if s.is_empty() {
        "Processor".into()
    } else {
        s
    }
}

/// Total CPU load between two calls (percent). The first call returns None.
#[derive(Default)]
pub struct CpuLoad {
    last: Option<(u64, u64)>,
}

fn times() -> Option<(u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut idle, mut kernel, mut user) = (z, z, z);
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let v = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    // kernel time includes idle time
    Some((v(idle), v(kernel) + v(user)))
}

/// Load from two (idle, total) readings.
pub fn load_between(a: (u64, u64), b: (u64, u64)) -> Option<f32> {
    let total = b.1.checked_sub(a.1)?;
    let idle = b.0.checked_sub(a.0)?;
    (total > 0).then(|| (100.0 * (total.saturating_sub(idle)) as f32 / total as f32).clamp(0.0, 100.0))
}

impl CpuLoad {
    pub fn sample(&mut self) -> Option<f32> {
        let now = times()?;
        let r = self.last.and_then(|a| load_between(a, now));
        self.last = Some(now);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysinfo_readings() {
        let (used, total) = memory().expect("memory");
        assert!(total > 1000 && used > 0 && used < total);
        assert_eq!(short_cpu_name("AMD Ryzen 7 9800X3D 8-Core Processor           "), "AMD Ryzen 7 9800X3D");
        assert_eq!(short_cpu_name("Intel(R) Core(TM) i7-13700K CPU @ 3.40GHz"), "Intel Core i7-13700K");
        assert_eq!(short_cpu_name(""), "Processor");
        assert!(!cpu_name().is_empty());
        assert_eq!(load_between((100, 1000), (150, 1200)), Some(75.0));
        assert_eq!(load_between((0, 10), (0, 10)), None);
        let mut l = CpuLoad::default();
        assert_eq!(l.sample(), None);
    }
}
