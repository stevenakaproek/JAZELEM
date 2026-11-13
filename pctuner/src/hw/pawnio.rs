// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.
// Ported from LibreHardwareMonitor (PawnIo/PawnIo.cs, PawnIo/LpcIO.cs, PawnIo/AmdFamily17.cs, Hardware/Mutexes.cs),
// Copyright (C) LibreHardwareMonitor and Contributors.

//! The PawnIO driver, spoken to directly (like LibreHardwareMonitor): open `\\?\GLOBALROOT\Device\PawnIO`, load a
//! signed module blob, call its functions. Each loaded module has a handle of its own.
//! The blobs (assets/pawnio, LGPL-2.1, unmodified from the PawnIO.Modules 0.1.6 release) are the same bytes
//! LibreHardwareMonitorLib 0.9.6 embeds.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows_sys::Win32::System::IO::DeviceIoControl;

pub const LPC_IO: &[u8] = include_bytes!("../../assets/pawnio/LpcIO.bin");
pub const AMD_FAMILY_17: &[u8] = include_bytes!("../../assets/pawnio/AMDFamily17.bin");

const DEVICE_TYPE: u32 = 41394 << 16;
const IOCTL_LOAD_BINARY: u32 = DEVICE_TYPE | (0x821 << 2);
const IOCTL_EXECUTE_FN: u32 = DEVICE_TYPE | (0x841 << 2);
const FN_NAME_LENGTH: usize = 32;
const GENERIC_READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;

/// A loaded PawnIO module. Closed on drop.
pub struct Module(HANDLE);

// the handle is only used through &self calls that the driver serialises; PC Tuner also holds the bus mutexes
unsafe impl Send for Module {}

impl Drop for Module {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

impl Module {
    /// Open the driver and load `blob`. Err when PawnIO is not installed, PC Tuner is not administrator, or the
    /// driver refuses the module.
    pub fn load(blob: &[u8]) -> Result<Module, String> {
        let path: Vec<u16> = r"\\?\GLOBALROOT\Device\PawnIO".encode_utf16().chain([0]).collect();
        let h = unsafe {
            CreateFileW(path.as_ptr(), GENERIC_READ_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut())
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(format!("the PawnIO driver cannot be opened (installed? administrator?): {}", std::io::Error::last_os_error()));
        }
        let m = Module(h);
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(m.0, IOCTL_LOAD_BINARY, blob.as_ptr() as *const c_void, blob.len() as u32, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut())
        };
        if ok == 0 {
            return Err(format!("PawnIO refused the module: {}", std::io::Error::last_os_error()));
        }
        Ok(m)
    }

    /// Call `name` with `input`, expecting up to `out_len` results.
    pub fn execute(&self, name: &str, input: &[u64], out_len: usize) -> Result<Vec<u64>, String> {
        let mut inbuf = vec![0u8; FN_NAME_LENGTH + input.len() * 8];
        let n = name.len().min(FN_NAME_LENGTH - 1);
        inbuf[..n].copy_from_slice(&name.as_bytes()[..n]);
        for (i, v) in input.iter().enumerate() {
            inbuf[FN_NAME_LENGTH + i * 8..FN_NAME_LENGTH + i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        let mut out = vec![0u64; out_len.max(1)];
        let mut read = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_EXECUTE_FN,
                inbuf.as_ptr() as *const c_void,
                inbuf.len() as u32,
                out.as_mut_ptr() as *mut c_void,
                (out_len * 8) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!("{name} failed: {}", std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)));
        }
        out.truncate(read as usize / 8);
        Ok(out)
    }

    fn first(&self, name: &str, input: &[u64]) -> Result<u64, String> {
        self.execute(name, input, 1)?.first().copied().ok_or_else(|| format!("{name} returned nothing"))
    }

    // ---- LpcIO.bin (Super I/O)
    pub fn select_slot(&self, slot: u64) -> Result<(), String> {
        self.execute("ioctl_select_slot", &[slot], 0).map(|_| ())
    }
    pub fn find_bars(&self) -> Result<(), String> {
        self.execute("ioctl_find_bars", &[], 0).map(|_| ())
    }
    pub fn pio_inb(&self, port: u16) -> Result<u8, String> {
        self.first("ioctl_pio_inb", &[port as u64]).map(|v| v as u8)
    }
    pub fn pio_outb(&self, port: u16, value: u8) -> Result<(), String> {
        self.execute("ioctl_pio_outb", &[port as u64, value as u64], 0).map(|_| ())
    }
    pub fn superio_inb(&self, reg: u8) -> Result<u8, String> {
        self.first("ioctl_superio_inb", &[reg as u64]).map(|v| v as u8)
    }
    pub fn superio_inw(&self, reg: u8) -> Result<u16, String> {
        self.first("ioctl_superio_inw", &[reg as u64]).map(|v| v as u16)
    }
    pub fn superio_outb(&self, reg: u8, value: u8) -> Result<(), String> {
        self.execute("ioctl_superio_outb", &[reg as u64, value as u64], 0).map(|_| ())
    }

    // ---- AMDFamily17.bin (Zen)
    pub fn read_smn(&self, offset: u32) -> Result<u32, String> {
        self.first("ioctl_read_smn", &[offset as u64]).map(|v| v as u32)
    }
}

/// A named system-wide mutex shared with other hardware monitors (HWiNFO, LibreHardwareMonitor and the old C#
/// helper use the same names), so two programs never interleave port accesses.
pub struct BusMutex(HANDLE);

unsafe impl Send for BusMutex {}

impl Drop for BusMutex {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub const ISA_BUS: &str = r"Global\Access_ISABUS.HTP.Method";
pub const PCI_BUS: &str = r"Global\Access_PCI";

impl BusMutex {
    pub fn open(name: &str) -> Option<BusMutex> {
        let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let h = unsafe { CreateMutexW(std::ptr::null(), 0, w.as_ptr()) };
        (!h.is_null()).then_some(BusMutex(h))
    }

    /// Run `f` while holding the mutex; None when it could not be taken within `ms`.
    pub fn with<R>(&self, ms: u32, f: impl FnOnce() -> R) -> Option<R> {
        let w = unsafe { WaitForSingleObject(self.0, ms) };
        if w != WAIT_OBJECT_0 && w != WAIT_ABANDONED {
            return None;
        }
        let r = f();
        unsafe { ReleaseMutex(self.0) };
        Some(r)
    }
}
