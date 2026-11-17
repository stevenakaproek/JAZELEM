//! NVIDIA's management library (nvml.dll, installed with every GeForce driver - nvidia-smi is built on it).
//! Loaded at run time, so PC Tuner still starts on a PC without an NVIDIA card. Only documented NVML calls
//! are used: clock offsets, locked clocks, power limit and live readings. Setters need administrator rights.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::sync::{Mutex, OnceLock};

type Ret = c_int;
type Dev = *mut c_void;

const NVML_SUCCESS: Ret = 0;
pub const CLOCK_GRAPHICS: c_uint = 0;
pub const CLOCK_MEM: c_uint = 2;
pub const PSTATE_0: c_uint = 0;

/// nvmlClockOffset_t (version 1).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct ClockOffset {
    version: c_uint,
    clock_type: c_uint,
    pstate: c_uint,
    offset: c_int,
    min: c_int,
    max: c_int,
}
const CLOCK_OFFSET_V1: c_uint = std::mem::size_of::<ClockOffset>() as c_uint | (1 << 24);

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

struct Api {
    error_string: unsafe extern "C" fn(Ret) -> *const c_char,
    driver_version: unsafe extern "C" fn(*mut c_char, c_uint) -> Ret,
    handle_by_index: unsafe extern "C" fn(c_uint, *mut Dev) -> Ret,
    name: unsafe extern "C" fn(Dev, *mut c_char, c_uint) -> Ret,
    get_offsets: unsafe extern "C" fn(Dev, *mut ClockOffset) -> Ret,
    set_offsets: unsafe extern "C" fn(Dev, *mut ClockOffset) -> Ret,
    lock_clocks: unsafe extern "C" fn(Dev, c_uint, c_uint) -> Ret,
    reset_clocks: unsafe extern "C" fn(Dev) -> Ret,
    power_constraints: unsafe extern "C" fn(Dev, *mut c_uint, *mut c_uint) -> Ret,
    power_default: unsafe extern "C" fn(Dev, *mut c_uint) -> Ret,
    power_limit: unsafe extern "C" fn(Dev, *mut c_uint) -> Ret,
    set_power_limit: unsafe extern "C" fn(Dev, c_uint) -> Ret,
    power_usage: unsafe extern "C" fn(Dev, *mut c_uint) -> Ret,
    clock: unsafe extern "C" fn(Dev, c_uint, *mut c_uint) -> Ret,
    max_clock: unsafe extern "C" fn(Dev, c_uint, *mut c_uint) -> Ret,
    min_max_pstate: unsafe extern "C" fn(Dev, c_uint, c_uint, *mut c_uint, *mut c_uint) -> Ret,
    temperature: unsafe extern "C" fn(Dev, c_uint, *mut c_uint) -> Ret,
    pstate: unsafe extern "C" fn(Dev, *mut c_uint) -> Ret,
    utilization: unsafe extern "C" fn(Dev, *mut Utilization) -> Ret,
    memory: unsafe extern "C" fn(Dev, *mut Memory) -> Ret,
    fan_speed: unsafe extern "C" fn(Dev, *mut c_uint) -> Ret,
}

/// nvmlMemory_t (v1): bytes.
#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

// the function pointers come from a library that stays loaded for the life of the process
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Result<Api, String>> = OnceLock::new();
/// NVML calls are made one at a time (the UI thread and the apply job can both ask).
static LOCK: Mutex<()> = Mutex::new(());

fn load() -> Result<Api, String> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let paths = [format!(r"{sys}\System32\nvml.dll"), r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll".to_string()];
    let mut lib = std::ptr::null_mut();
    for p in &paths {
        let w: Vec<u16> = p.encode_utf16().chain(Some(0)).collect();
        lib = unsafe { LoadLibraryW(w.as_ptr()) };
        if !lib.is_null() {
            break;
        }
    }
    if lib.is_null() {
        return Err("The NVIDIA driver's management library (nvml.dll) was not found - is the NVIDIA driver installed?".into());
    }
    macro_rules! f {
        ($name:literal) => {{
            let p = unsafe { GetProcAddress(lib, concat!($name, "\0").as_ptr()) };
            match p {
                Some(p) => unsafe { std::mem::transmute::<_, _>(p) },
                None => return Err(format!("This NVIDIA driver is too old: nvml.dll has no {}. Update the driver.", $name)),
            }
        }};
    }
    let init: unsafe extern "C" fn() -> Ret = f!("nvmlInit_v2");
    let api = Api {
        error_string: f!("nvmlErrorString"),
        driver_version: f!("nvmlSystemGetDriverVersion"),
        handle_by_index: f!("nvmlDeviceGetHandleByIndex_v2"),
        name: f!("nvmlDeviceGetName"),
        get_offsets: f!("nvmlDeviceGetClockOffsets"),
        set_offsets: f!("nvmlDeviceSetClockOffsets"),
        lock_clocks: f!("nvmlDeviceSetGpuLockedClocks"),
        reset_clocks: f!("nvmlDeviceResetGpuLockedClocks"),
        power_constraints: f!("nvmlDeviceGetPowerManagementLimitConstraints"),
        power_default: f!("nvmlDeviceGetPowerManagementDefaultLimit"),
        power_limit: f!("nvmlDeviceGetPowerManagementLimit"),
        set_power_limit: f!("nvmlDeviceSetPowerManagementLimit"),
        power_usage: f!("nvmlDeviceGetPowerUsage"),
        clock: f!("nvmlDeviceGetClockInfo"),
        max_clock: f!("nvmlDeviceGetMaxClockInfo"),
        min_max_pstate: f!("nvmlDeviceGetMinMaxClockOfPState"),
        temperature: f!("nvmlDeviceGetTemperature"),
        pstate: f!("nvmlDeviceGetPerformanceState"),
        utilization: f!("nvmlDeviceGetUtilizationRates"),
        memory: f!("nvmlDeviceGetMemoryInfo"),
        fan_speed: f!("nvmlDeviceGetFanSpeed"),
    };
    let r = unsafe { init() };
    if r != NVML_SUCCESS {
        return Err(format!("The NVIDIA driver did not start its management library: {}", err_text(&api, r)));
    }
    Ok(api)
}

fn err_text(api: &Api, r: Ret) -> String {
    let p = unsafe { (api.error_string)(r) };
    let s = if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
    match r {
        3 => "this card or driver does not support it (Not Supported)".into(),
        4 => "administrator rights are needed (No Permission)".into(),
        2 => format!("the driver refused the value ({s})"),
        _ => format!("{s} (NVML error {r})"),
    }
}

/// The NVIDIA card (the first one NVML lists) with the library ready to use.
pub struct Gpu {
    api: &'static Api,
    dev: Dev,
}

fn check(api: &Api, what: &str, r: Ret) -> Result<(), String> {
    if r == NVML_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: {}", err_text(api, r)))
    }
}

fn cstring(buf: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// A clock offset with the range the driver allows.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Offset {
    pub mhz: i32,
    pub min: i32,
    pub max: i32,
}

/// What NVML reads from the card in one go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub core_mhz: u32,
    pub mem_mhz: u32,
    pub max_mem_mhz: u32,
    pub power_w: f32,
    pub temp_c: u32,
    pub pstate: u32,
    pub util: u32,
    /// video memory in use / installed, MiB
    pub vram_used_mb: u32,
    pub vram_total_mb: u32,
    /// fan speed, % of maximum (0 when the fans are stopped or it cannot be read)
    pub fan_pct: u32,
}

impl Gpu {
    /// Open the first NVIDIA card. Fails with a plain-English reason (no driver, no card).
    pub fn open() -> Result<Gpu, String> {
        let api = match API.get_or_init(load) {
            Ok(a) => a,
            Err(e) => return Err(e.clone()),
        };
        let mut dev: Dev = std::ptr::null_mut();
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        check(api, "No NVIDIA card found", unsafe { (api.handle_by_index)(0, &mut dev) })?;
        Ok(Gpu { api, dev })
    }

    fn call(&self, what: &str, f: impl FnOnce(&Api, Dev) -> Ret) -> Result<(), String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        check(self.api, what, f(self.api, self.dev))
    }

    pub fn name(&self) -> Result<String, String> {
        let mut b = [0 as c_char; 96];
        self.call("Card name", |a, d| unsafe { (a.name)(d, b.as_mut_ptr(), b.len() as c_uint) })?;
        Ok(cstring(&b))
    }

    pub fn driver(&self) -> Result<String, String> {
        let mut b = [0 as c_char; 80];
        self.call("Driver version", |a, _| unsafe { (a.driver_version)(b.as_mut_ptr(), b.len() as c_uint) })?;
        Ok(cstring(&b))
    }

    /// The current P0 offset of a clock (CLOCK_GRAPHICS or CLOCK_MEM) and the range the driver allows.
    pub fn offset(&self, clock: c_uint) -> Result<Offset, String> {
        let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: clock, pstate: PSTATE_0, ..Default::default() };
        self.call("Read clock offset", |a, d| unsafe { (a.get_offsets)(d, &mut o) })?;
        Ok(Offset { mhz: o.offset, min: o.min, max: o.max })
    }

    /// Set the P0 offset of a clock (administrator).
    pub fn set_offset(&self, clock: c_uint, mhz: i32) -> Result<(), String> {
        let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: clock, pstate: PSTATE_0, offset: mhz, ..Default::default() };
        let what = if clock == CLOCK_MEM { "Set memory offset" } else { "Set core offset" };
        self.call(what, |a, d| unsafe { (a.set_offsets)(d, &mut o) })
    }

    /// Keep the core clock between min and max MHz (administrator).
    pub fn lock_clocks(&self, min: u32, max: u32) -> Result<(), String> {
        self.call("Set the core clock limit", |a, d| unsafe { (a.lock_clocks)(d, min, max) })
    }

    /// Give the core clock range back to the driver (administrator).
    pub fn reset_clocks(&self) -> Result<(), String> {
        self.call("Remove the core clock limit", |a, d| unsafe { (a.reset_clocks)(d) })
    }

    /// Power limit range and default, in watts: (min, max, default, current).
    pub fn power_limits(&self) -> Result<(u32, u32, u32, u32), String> {
        let (mut lo, mut hi, mut def, mut cur) = (0, 0, 0, 0);
        self.call("Read power limits", |a, d| unsafe { (a.power_constraints)(d, &mut lo, &mut hi) })?;
        self.call("Read default power limit", |a, d| unsafe { (a.power_default)(d, &mut def) })?;
        self.call("Read power limit", |a, d| unsafe { (a.power_limit)(d, &mut cur) })?;
        Ok((lo / 1000, hi / 1000, def / 1000, cur / 1000))
    }

    /// Set the power limit in watts (administrator).
    pub fn set_power_limit(&self, watts: u32) -> Result<(), String> {
        self.call("Set the power limit", |a, d| unsafe { (a.set_power_limit)(d, watts * 1000) })
    }

    /// The lowest and highest core clock of P0 (what a clock limit may use).
    pub fn core_range(&self) -> Result<(u32, u32), String> {
        let (mut lo, mut hi) = (0, 0);
        self.call("Read the core clock range", |a, d| unsafe { (a.min_max_pstate)(d, CLOCK_GRAPHICS, PSTATE_0, &mut lo, &mut hi) })?;
        Ok((lo, hi))
    }

    /// Live clocks, power, temperature, P-state and load. Missing values read as 0.
    pub fn read(&self) -> Reading {
        let mut r = Reading::default();
        let mut mw = 0;
        let mut u = Utilization::default();
        let _ = self.call("", |a, d| unsafe { (a.clock)(d, CLOCK_GRAPHICS, &mut r.core_mhz) });
        let _ = self.call("", |a, d| unsafe { (a.clock)(d, CLOCK_MEM, &mut r.mem_mhz) });
        let _ = self.call("", |a, d| unsafe { (a.max_clock)(d, CLOCK_MEM, &mut r.max_mem_mhz) });
        let _ = self.call("", |a, d| unsafe { (a.power_usage)(d, &mut mw) });
        let _ = self.call("", |a, d| unsafe { (a.temperature)(d, 0, &mut r.temp_c) });
        let _ = self.call("", |a, d| unsafe { (a.pstate)(d, &mut r.pstate) });
        let _ = self.call("", |a, d| unsafe { (a.utilization)(d, &mut u) });
        let mut m = Memory::default();
        if self.call("", |a, d| unsafe { (a.memory)(d, &mut m) }).is_ok() {
            r.vram_used_mb = (m.used / (1024 * 1024)) as u32;
            r.vram_total_mb = (m.total / (1024 * 1024)) as u32;
        }
        let _ = self.call("", |a, d| unsafe { (a.fan_speed)(d, &mut r.fan_pct) });
        r.power_w = mw as f32 / 1000.0;
        r.util = u.gpu;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvml_struct_version() {
        assert_eq!(std::mem::size_of::<ClockOffset>(), 24);
        assert_eq!(CLOCK_OFFSET_V1, 24 | (1 << 24));
    }

    /// Read-only look at this PC's card: `cargo test -p pctuner-core -- --ignored gpu_nvml_probe --nocapture`.
    #[test]
    #[ignore]
    fn gpu_nvml_probe() {
        let g = Gpu::open().expect("open");
        println!("card {:?}  driver {:?}", g.name(), g.driver());
        println!("core offset {:?}", g.offset(CLOCK_GRAPHICS));
        println!("mem offset  {:?}", g.offset(CLOCK_MEM));
        println!("P0 core range {:?}", g.core_range());
        println!("power (min, max, default, now) W {:?}", g.power_limits());
        println!("live {:?}", g.read());
    }
}
