// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.
// Ported from LibreHardwareMonitor (Interop/NvApi.cs, Hardware/Gpu/NvidiaGpu.cs),
// Copyright (C) LibreHardwareMonitor and Contributors.

//! NVIDIA GPU through NVAPI (nvapi64.dll, from the graphics driver) - READ ONLY in stage 1:
//! GPU temperatures and the fans' RPM / level. Struct layouts and version fields as LHM 0.9.6 uses them.

use std::ffi::c_void;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const OK: i32 = 0;
const MAX_PHYSICAL_GPUS: usize = 64;
const MAX_THERMAL_SENSORS_PER_GPU: usize = 3;
const THERMAL_TARGET_ALL: i32 = 15;
const THERMAL_TARGET_GPU: i32 = 1;
const MAX_FAN_COOLERS: usize = 32;

const ID_INITIALIZE: u32 = 0x0150_E828;
const ID_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC_921F;
const ID_GET_FULL_NAME: u32 = 0xCEEE_8E9F;
const ID_GET_THERMAL_SETTINGS: u32 = 0xE364_0A56;
const ID_GET_THERMAL_SENSORS: u32 = 0x65FE_3AAD;
const ID_FAN_COOLERS_GET_STATUS: u32 = 0x35AE_D5E8;
const ID_FAN_COOLERS_GET_CONTROL: u32 = 0x814B_209F;
const ID_FAN_COOLERS_SET_CONTROL: u32 = 0xA589_71A5;
const MODE_AUTO: u32 = 0;
const MODE_MANUAL: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct NvFanCoolerControlItem {
    cooler_id: u32,
    level: u32,
    control_mode: u32,
    reserved: [u32; 8],
}

#[repr(C)]
struct NvFanCoolerControl {
    version: u32,
    reserved: u32,
    count: u32,
    reserved2: [u32; 8],
    items: [NvFanCoolerControlItem; MAX_FAN_COOLERS],
}

type FnCoolersControl = unsafe extern "C" fn(*mut c_void, *mut NvFanCoolerControl) -> i32;

pub const ID_PREFIX: &str = "/gpu-nvidia/0";

#[repr(C)]
#[derive(Clone, Copy)]
struct NvSensor {
    controller: i32,
    default_min: u32,
    default_max: u32,
    current: u32,
    target: i32,
}

#[repr(C)]
struct NvThermalSettings {
    version: u32,
    count: u32,
    sensor: [NvSensor; MAX_THERMAL_SENSORS_PER_GPU],
}

#[repr(C)]
struct NvThermalSensors {
    version: u32,
    mask: u32,
    reserved: [i32; 8],
    temperatures: [i32; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvFanCoolersStatusItem {
    cooler_id: u32,
    current_rpm: u32,
    current_min_level: u32,
    current_max_level: u32,
    current_level: u32,
    reserved: [u32; 8],
}

#[repr(C)]
struct NvFanCoolersStatus {
    version: u32,
    count: u32,
    reserved: [u64; 4],
    items: [NvFanCoolersStatusItem; MAX_FAN_COOLERS],
}

/// MAKE_NVAPI_VERSION: struct size | version << 16.
const fn version<T>(v: u32) -> u32 {
    std::mem::size_of::<T>() as u32 | (v << 16)
}

type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type FnInit = unsafe extern "C" fn() -> i32;
type FnEnum = unsafe extern "C" fn(*mut [*mut c_void; MAX_PHYSICAL_GPUS], *mut i32) -> i32;
type FnName = unsafe extern "C" fn(*mut c_void, *mut [u8; 64]) -> i32;
type FnThermalSettings = unsafe extern "C" fn(*mut c_void, i32, *mut NvThermalSettings) -> i32;
type FnThermalSensors = unsafe extern "C" fn(*mut c_void, *mut NvThermalSensors) -> i32;
type FnCoolersStatus = unsafe extern "C" fn(*mut c_void, *mut NvFanCoolersStatus) -> i32;

/// One GPU fan: (cooler id, RPM, level %, lowest level the driver allows).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cooler {
    pub id: u32,
    pub rpm: u32,
    pub level: u32,
    pub min: u32,
}

/// The first NVIDIA GPU.
pub struct NvGpu {
    handle: *mut c_void,
    pub name: String,
    thermal_settings: Option<FnThermalSettings>,
    thermal_sensors: Option<FnThermalSensors>,
    coolers: Option<FnCoolersStatus>,
    get_control: Option<FnCoolersControl>,
    set_control: Option<FnCoolersControl>,
    /// Highest working thermal-sensor mask (LHM probes it once), 0 = none.
    sensor_mask: u32,
    /// Sensors in the thermal settings (the memory junction sits at that index + 2).
    settings_count: u32,
}

// NVAPI handles are process-wide; PC Tuner reads them from one thread at a time
unsafe impl Send for NvGpu {}

/// "NVIDIA GeForce RTX 5080" (LHM adds "NVIDIA " when the driver leaves it out).
pub fn full_name(raw: &str) -> String {
    let n = raw.trim();
    if n.to_ascii_uppercase().starts_with("NVIDIA") {
        n.to_string()
    } else {
        format!("NVIDIA {n}")
    }
}

impl NvGpu {
    pub fn open() -> Result<NvGpu, String> {
        unsafe {
            let dll: Vec<u16> = "nvapi64.dll".encode_utf16().chain([0]).collect();
            let lib = LoadLibraryW(dll.as_ptr());
            if lib.is_null() {
                return Err("nvapi64.dll not found (no NVIDIA driver)".into());
            }
            let qi = GetProcAddress(lib, b"nvapi_QueryInterface\0".as_ptr()).ok_or("nvapi_QueryInterface missing")?;
            let qi: QueryInterface = std::mem::transmute(qi);
            let get = |id: u32| -> Option<*mut c_void> {
                let p = qi(id);
                (!p.is_null()).then_some(p)
            };
            let init: FnInit = std::mem::transmute(get(ID_INITIALIZE).ok_or("NvAPI_Initialize missing")?);
            if init() != OK {
                return Err("NvAPI_Initialize failed".into());
            }
            let enum_gpus: FnEnum = std::mem::transmute(get(ID_ENUM_PHYSICAL_GPUS).ok_or("NvAPI_EnumPhysicalGPUs missing")?);
            let mut handles = [std::ptr::null_mut(); MAX_PHYSICAL_GPUS];
            let mut count = 0i32;
            if enum_gpus(&mut handles, &mut count) != OK || count < 1 {
                return Err("no NVIDIA GPU".into());
            }
            let handle = handles[0];
            let mut name_buf = [0u8; 64];
            let name = match get(ID_GET_FULL_NAME) {
                Some(f) if std::mem::transmute::<_, FnName>(f)(handle, &mut name_buf) == OK => {
                    let end = name_buf.iter().position(|b| *b == 0).unwrap_or(64);
                    full_name(&String::from_utf8_lossy(&name_buf[..end]))
                }
                _ => "NVIDIA GPU".into(),
            };
            let mut gpu = NvGpu {
                handle,
                name,
                thermal_settings: get(ID_GET_THERMAL_SETTINGS).map(|p| std::mem::transmute(p)),
                thermal_sensors: get(ID_GET_THERMAL_SENSORS).map(|p| std::mem::transmute(p)),
                coolers: get(ID_FAN_COOLERS_GET_STATUS).map(|p| std::mem::transmute(p)),
                get_control: get(ID_FAN_COOLERS_GET_CONTROL).map(|p| std::mem::transmute(p)),
                set_control: get(ID_FAN_COOLERS_SET_CONTROL).map(|p| std::mem::transmute(p)),
                sensor_mask: 0,
                settings_count: 0,
            };
            gpu.settings_count = gpu.read_settings().map(|s| s.count).unwrap_or(0);
            // LHM: the highest mask bit that still answers OK
            if gpu.thermal_sensors.is_some() {
                let mut found = false;
                for bit in 0..32 {
                    let mask = 1u32 << bit;
                    gpu.sensor_mask = mask;
                    if gpu.read_sensors(mask).is_some() {
                        found = true;
                        continue;
                    }
                    gpu.sensor_mask = mask - 1;
                    break;
                }
                if !found {
                    gpu.sensor_mask = 0;
                }
            }
            Ok(gpu)
        }
    }

    fn read_settings(&self) -> Option<NvThermalSettings> {
        let f = self.thermal_settings?;
        let mut s: NvThermalSettings = unsafe { std::mem::zeroed() };
        s.version = version::<NvThermalSettings>(2);
        s.count = MAX_THERMAL_SENSORS_PER_GPU as u32;
        (unsafe { f(self.handle, THERMAL_TARGET_ALL, &mut s) } == OK).then_some(s)
    }

    fn read_sensors(&self, mask: u32) -> Option<NvThermalSensors> {
        let f = self.thermal_sensors?;
        let mut s: NvThermalSensors = unsafe { std::mem::zeroed() };
        s.version = version::<NvThermalSensors>(2);
        s.mask = mask;
        (unsafe { f(self.handle, &mut s) } == OK).then_some(s)
    }

    /// (LHM temperature index, name, °C): GPU Core, and on RTX 50 / 40 cards the memory junction.
    pub fn temperatures(&self) -> Vec<(usize, String, f32)> {
        let mut out = vec![];
        let mut core = self.read_settings().and_then(|s| {
            (0..(s.count as usize).min(MAX_THERMAL_SENSORS_PER_GPU)).find(|i| s.sensor[*i].target == THERMAL_TARGET_GPU).map(|i| s.sensor[i].current as f32)
        });
        if self.sensor_mask > 0 {
            if let Some(s) = self.read_sensors(self.sensor_mask) {
                let at = |i: usize| s.temperatures[i] as f32 / 256.0;
                let upper = self.name.to_ascii_uppercase();
                let junction = if upper.starts_with("NVIDIA GEFORCE RTX 50") {
                    core = Some(at(1));
                    at(2)
                } else if upper.starts_with("NVIDIA GEFORCE RTX 40") {
                    at(7)
                } else {
                    at(9)
                };
                if junction != 0.0 {
                    out.push((self.settings_count as usize + 2, "GPU Memory Junction".to_string(), junction));
                }
            }
        }
        if let Some(c) = core {
            out.insert(0, (0, "GPU Core".to_string(), c));
        }
        out
    }

    /// Set one cooler: Some(level %) = manual at that level, None = back to the driver (Auto). The driver's
    /// current control block is read and only this cooler is changed.
    fn control(&self, cooler_id: u32, level: Option<u32>) -> Result<(), String> {
        let (get, set) = match (self.get_control, self.set_control) {
            (Some(g), Some(s)) => (g, s),
            _ => return Err("this driver has no fan-cooler control".into()),
        };
        let mut c: NvFanCoolerControl = unsafe { std::mem::zeroed() };
        c.version = version::<NvFanCoolerControl>(1);
        let r = unsafe { get(self.handle, &mut c) };
        if r != OK {
            return Err(format!("NvAPI_GPU_ClientFanCoolersGetControl failed ({r})"));
        }
        let n = (c.count as usize).min(MAX_FAN_COOLERS);
        let item = c.items[..n].iter_mut().find(|i| i.cooler_id == cooler_id).ok_or_else(|| format!("no GPU cooler {cooler_id}"))?;
        match level {
            Some(l) => {
                item.control_mode = MODE_MANUAL;
                item.level = l.min(100);
            }
            None => {
                item.control_mode = MODE_AUTO;
                item.level = 0;
            }
        }
        let r = unsafe { set(self.handle, &mut c) };
        if r != OK {
            return Err(format!("NvAPI_GPU_ClientFanCoolersSetControl failed ({r})"));
        }
        Ok(())
    }

    /// Drive a GPU fan at `percent` (never under the driver's minimum - the controller floors it too).
    pub fn set_level(&self, cooler_id: u32, percent: f32) -> Result<(), String> {
        let min = self.coolers().iter().find(|c| c.id == cooler_id).map(|c| c.min).unwrap_or(30);
        self.control(cooler_id, Some((percent.round().clamp(0.0, 100.0) as u32).max(min)))
    }

    /// Hand a GPU fan back to the graphics driver's own curve.
    pub fn set_auto(&self, cooler_id: u32) -> Result<(), String> {
        self.control(cooler_id, None)
    }

    /// Is the cooler under manual (software) control right now?
    pub fn is_manual(&self, cooler_id: u32) -> Option<bool> {
        let get = self.get_control?;
        let mut c: NvFanCoolerControl = unsafe { std::mem::zeroed() };
        c.version = version::<NvFanCoolerControl>(1);
        if unsafe { get(self.handle, &mut c) } != OK {
            return None;
        }
        c.items[..(c.count as usize).min(MAX_FAN_COOLERS)].iter().find(|i| i.cooler_id == cooler_id).map(|i| i.control_mode == MODE_MANUAL)
    }

    pub fn coolers(&self) -> Vec<Cooler> {
        let Some(f) = self.coolers else { return vec![] };
        let mut s: NvFanCoolersStatus = unsafe { std::mem::zeroed() };
        s.version = version::<NvFanCoolersStatus>(1);
        if unsafe { f(self.handle, &mut s) } != OK {
            return vec![];
        }
        s.items[..(s.count as usize).min(MAX_FAN_COOLERS)]
            .iter()
            .map(|i| Cooler { id: i.cooler_id, rpm: i.current_rpm, level: i.current_level, min: i.current_min_level })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layouts LHM's marshalling produces (Pack = 8): the driver checks these sizes via the version field.
    #[test]
    fn nvgpu_struct_sizes() {
        assert_eq!(std::mem::size_of::<NvThermalSensors>(), 168);
        assert_eq!(std::mem::size_of::<NvFanCoolersStatusItem>(), 52);
        assert_eq!(std::mem::size_of::<NvFanCoolersStatus>(), 40 + 32 * 52);
        assert_eq!(std::mem::size_of::<NvThermalSettings>(), 8 + 3 * 20);
        assert_eq!(std::mem::size_of::<NvFanCoolerControlItem>(), 44);
        assert_eq!(std::mem::size_of::<NvFanCoolerControl>(), 44 + 32 * 44);
        assert_eq!(version::<NvThermalSensors>(2), 168 | 2 << 16);
        assert_eq!(full_name("GeForce RTX 5080"), "NVIDIA GeForce RTX 5080");
        assert_eq!(full_name("NVIDIA GeForce RTX 5080"), "NVIDIA GeForce RTX 5080");
    }

    /// NVAPI needs no administrator: read the real card (read-only).
    #[test]
    fn nvgpu_reads_this_pc() {
        let g = NvGpu::open().expect("RTX 5080 via NVAPI");
        assert!(g.name.contains("RTX 5080"), "{}", g.name);
        let t = g.temperatures();
        println!("{} temps {t:?} coolers {:?} mask {:#x}", g.name, g.coolers(), g.sensor_mask);
        let core = t.iter().find(|x| x.1 == "GPU Core").expect("GPU Core");
        assert!(core.2 > 10.0 && core.2 < 100.0);
        assert!(t.iter().any(|x| x.1 == "GPU Memory Junction" && x.0 == 3), "id /gpu-nvidia/0/temperature/3");
        let c = g.coolers();
        assert!(!c.is_empty() && c[0].id == 1 && c[0].min == 30, "{c:?}");
    }
}
