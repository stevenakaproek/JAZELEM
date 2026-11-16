//! Read-only: the RTX 5080's own voltage/frequency curve from the NVIDIA driver (nvapi64.dll).
//! NVIDIA does not document this call (it is the one Afterburner uses to show its curve editor); the layout
//! follows the open-source LACT / nvvf research for Blackwell cards. Nothing is ever written through it:
//! PC Tuner only reads the curve to work out each preset's core offset, and every value is sanity-checked.

use std::ffi::c_void;

const ID_INITIALIZE: u32 = 0x0150_E828;
const ID_ENUM_GPUS: u32 = 0xE5AC_921F;
const ID_VF_STATUS: u32 = 0x2153_7AD4;

const STATUS_SIZE: usize = 0x1C28;
const STATUS_VERSION: u32 = (1 << 16) | STATUS_SIZE as u32;
const ENTRIES_AT: usize = 0x48;
const ENTRY_STRIDE: usize = 0x1C;
const ENTRY_COUNT: usize = 128;

/// One point of the curve: the clock the card runs at this voltage with no offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfPoint {
    pub mv: f32,
    pub mhz: f32,
}

type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;

/// Turn the driver's raw answer into curve points (kHz / µV per entry), dropping empty and idle entries.
/// Err when it does not look like a voltage/frequency table.
pub fn parse_status(buf: &[u8]) -> Result<Vec<VfPoint>, String> {
    if buf.len() < ENTRIES_AT + ENTRY_COUNT * ENTRY_STRIDE {
        return Err("The driver's curve answer is too short.".into());
    }
    let u32_at = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    let mut pts: Vec<VfPoint> = (0..ENTRY_COUNT)
        .map(|i| {
            let o = ENTRIES_AT + i * ENTRY_STRIDE;
            VfPoint { mhz: u32_at(o) as f32 / 1000.0, mv: u32_at(o + 4) as f32 / 1000.0 }
        })
        .filter(|p| p.mhz > 0.0 && p.mv > 0.0)
        .collect();
    // the driver also lists idle / low-power entries out of order: keep the rising part of the table
    pts.sort_by(|a, b| a.mv.total_cmp(&b.mv));
    let ok = pts.len() >= 30
        && pts.iter().all(|p| (300.0..=1400.0).contains(&p.mv) && p.mhz <= 5000.0)
        && pts.iter().map(|p| p.mhz).fold(0.0, f32::max) >= 1500.0;
    if !ok {
        return Err("The curve the driver returned does not look like a voltage/frequency table.".into());
    }
    Ok(pts)
}

/// The stock clock at a voltage: the first curve point at or above `mv` (as Afterburner's preset builder did).
pub fn clock_at(curve: &[VfPoint], mv: f32) -> Option<VfPoint> {
    curve.iter().find(|p| p.mv >= mv).copied()
}

/// Read the first NVIDIA card's curve. Read-only.
pub fn read_curve() -> Result<Vec<VfPoint>, String> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let name: Vec<u16> = "nvapi64.dll".encode_utf16().chain(Some(0)).collect();
    let lib = unsafe { LoadLibraryW(name.as_ptr()) };
    if lib.is_null() {
        return Err("nvapi64.dll (part of the NVIDIA driver) was not found.".into());
    }
    let qi = unsafe { GetProcAddress(lib, c"nvapi_QueryInterface".as_ptr() as *const u8) }.ok_or("nvapi64.dll has no nvapi_QueryInterface.")?;
    let qi: QueryInterface = unsafe { std::mem::transmute(qi) };
    let get = |id: u32| {
        let p = unsafe { qi(id) };
        if p.is_null() {
            Err(format!("This NVIDIA driver does not offer call 0x{id:08X}."))
        } else {
            Ok(p)
        }
    };
    let init: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(get(ID_INITIALIZE)?) };
    let enum_gpus: unsafe extern "C" fn(*mut *mut c_void, *mut u32) -> i32 = unsafe { std::mem::transmute(get(ID_ENUM_GPUS)?) };
    let status: unsafe extern "C" fn(*mut c_void, *mut u8) -> i32 = unsafe { std::mem::transmute(get(ID_VF_STATUS)?) };
    let r = unsafe { init() };
    if r != 0 {
        return Err(format!("The NVIDIA driver interface did not start (error {r})."));
    }
    let mut gpus = [std::ptr::null_mut::<c_void>(); 64];
    let mut n = 0u32;
    let r = unsafe { enum_gpus(gpus.as_mut_ptr(), &mut n) };
    if r != 0 || n == 0 {
        return Err(format!("No NVIDIA card found by the driver (error {r})."));
    }
    let mut buf = vec![0u8; STATUS_SIZE];
    buf[0..4].copy_from_slice(&STATUS_VERSION.to_le_bytes());
    for b in &mut buf[4..20] {
        *b = 0xFF;
    }
    buf[0x14..0x18].copy_from_slice(&15u32.to_le_bytes());
    let r = unsafe { status(gpus[0], buf.as_mut_ptr()) };
    if r != 0 {
        return Err(format!("The driver did not return the voltage/frequency curve (error {r})."));
    }
    parse_status(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(points: &[(u32, u32)]) -> Vec<u8> {
        let mut b = vec![0u8; STATUS_SIZE];
        for (i, (khz, uv)) in points.iter().enumerate() {
            let o = ENTRIES_AT + i * ENTRY_STRIDE;
            b[o..o + 4].copy_from_slice(&khz.to_le_bytes());
            b[o + 4..o + 8].copy_from_slice(&uv.to_le_bytes());
        }
        b
    }

    #[test]
    fn nvapi_parse_curve() {
        // 40 rising points from 700 mV / 1500 MHz, plus an empty and an out-of-order idle entry
        let mut pts: Vec<(u32, u32)> = (0..40).map(|k| ((1500 + 30 * k) * 1000, (700 + 10 * k) * 1000)).collect();
        pts.push((0, 0));
        pts.push((405_000, 530_000));
        let c = parse_status(&raw(&pts)).unwrap();
        assert_eq!(c.len(), 41);
        assert_eq!(c[0], VfPoint { mv: 530.0, mhz: 405.0 });
        assert_eq!(clock_at(&c, 925.0), Some(VfPoint { mv: 930.0, mhz: 2190.0 }));
        assert_eq!(clock_at(&c, 2000.0), None);
        // nonsense is refused
        assert!(parse_status(&raw(&[(2_000_000, 900_000)])).is_err(), "too few points");
        assert!(parse_status(&raw(&(0..40).map(|k| (100 * 1000, (700 + k) * 1000)).collect::<Vec<_>>())).is_err(), "no real clocks");
        assert!(parse_status(&[0u8; 16]).is_err());
    }

    /// Read-only look at this PC's curve: `cargo test -p pctuner-core -- --ignored gpu_nvapi_curve --nocapture`.
    #[test]
    #[ignore]
    fn gpu_nvapi_curve() {
        let c = read_curve().expect("curve");
        println!("{} points, {:?} .. {:?}", c.len(), c.first(), c.last());
        for mv in [900.0, 925.0, 950.0] {
            println!("at {mv} mV: {:?}", clock_at(&c, mv));
        }
        for p in c.iter().step_by(8) {
            println!("  {:7.1} mV  {:7.1} MHz", p.mv, p.mhz);
        }
    }
}
