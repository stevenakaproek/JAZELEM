// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.
// Ported from LibreHardwareMonitor (Hardware/Motherboard/Lpc/LpcIO.cs, LpcPort.cs, Nct677X.cs and the NCT6687DR
// defaults in SuperIOHardware.cs), Copyright (C) LibreHardwareMonitor and Contributors.

//! Nuvoton NCT6687D-R (MSI AM5 / LGA1851 800-series boards, e.g. MAG X870E TOMAHAWK WIFI) - READ ONLY.
//! Stage 1 of the Rust fan backend: temperatures, fan RPMs and duty. Nothing is written to the chip's settings
//! (the Super I/O configuration enter/exit and the EC page handshake are port selects, as for any read).

use super::pawnio::{BusMutex, Module, ISA_BUS, LPC_IO};
use std::time::{Duration, Instant};

/// Chip id 0xD5 revision 0x92 at Super I/O port 0x2E / 0x4E, hardware monitor in logical device 0x0B.
const CHIP_ID: u8 = 0xD5;
const CHIP_REVISION: u8 = 0x92;
const HWM_LDN: u8 = 0x0B;
const REGISTER_PORTS: [(u16, u64); 2] = [(0x2E, 0), (0x4E, 1)];

// EC space access (offsets from the hardware monitor base address)
const EC_PAGE: u16 = 0x04;
const EC_INDEX: u16 = 0x05;
const EC_DATA: u16 = 0x06;
const EC_PAGE_SELECT: u8 = 0xFF;

/// Bit 7 set = the monitor is running (LHM sets it on start; read-only Rust only checks it).
const INIT_REGISTER: u16 = 0x180;

pub const HW_NAME: &str = "Nuvoton NCT6687D-R";
pub const ID_PREFIX: &str = "/lpc/nct6687dr/0";

/// Temperature (index in LHM's list, name) - the sensors LHM names on this board.
pub const TEMPS: [(usize, &str); 5] = [(0, "CPU Core"), (1, "System"), (2, "VRM MOS"), (3, "Chipset"), (4, "CPU Socket")];
/// Register of temperature i: 0x100 + 2 i.
fn temp_register(i: usize) -> u16 {
    0x100 + 2 * i as u16
}

/// Fan RPM registers (index = LHM's fan / control index).
const FAN_RPM: [u16; 16] = [0x140, 0x142, 0x144, 0x146, 0x148, 0x14A, 0x14C, 0x14E, 0x150, 0x152, 0x15E, 0x15C, 0x15A, 0x158, 0x156, 0x154];
/// Current duty (0-255) per control index.
const FAN_PWM_OUT: [u16; 16] = [0x160, 0x161, 0x162, 0x163, 0x164, 0x165, 0x166, 0x167, 0xFFF, 0xC93, 0xE05, 0xE04, 0xE03, 0xE02, 0xE01, 0xE00];
/// The headers LHM names on this chip, in its order: (index, RPM sensor name, control name).
pub const HEADERS: [(usize, &str, &str); 10] = [
    (0, "CPU Fan", "CPU Fan"),
    (1, "Pump Fan #1", "Pump Fan"),
    (2, "Chipset Fan", "Chipset Fan"),
    (10, "System Fan #1", "System Fan #1"),
    (11, "System Fan #2", "System Fan #2"),
    (12, "System Fan #3", "System Fan #3"),
    (13, "System Fan #4", "System Fan #4"),
    (14, "System Fan #5", "System Fan #5"),
    (15, "System Fan #6", "System Fan #6"),
    (3, "EZ-Connect Fan", "EZ-Connect Fan"),
];

/// One reading of the chip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// (LHM temperature index, °C)
    pub temps: Vec<(usize, f32)>,
    /// (header index, RPM, duty %)
    pub fans: Vec<(usize, f32, f32)>,
}

pub struct Nct6687 {
    lpc: Module,
    isa: BusMutex,
    /// Hardware monitor base address.
    base: u16,
}

/// Temperature from the two registers: signed whole degrees, bit 7 of the next register = +0.5.
pub fn temperature(whole: u8, half: u8) -> f32 {
    whole as i8 as f32 + if half & 0x80 != 0 { 0.5 } else { 0.0 }
}

/// Duty % shown for a PWM byte (LHM rounds value / 2.55).
pub fn duty_percent(raw: u8) -> f32 {
    (raw as f32 / 2.55).round()
}

impl Nct6687 {
    /// Find the chip. Err when PawnIO is unavailable, the chip is something else, or the monitor is not running.
    pub fn open() -> Result<Nct6687, String> {
        let isa = BusMutex::open(ISA_BUS).ok_or("cannot open the ISA bus mutex")?;
        for (port, slot) in REGISTER_PORTS {
            let lpc = Module::load(LPC_IO)?;
            lpc.select_slot(slot)?;
            let found = isa
                .with(100, || -> Result<Option<u16>, String> {
                    // Winbond / Nuvoton / Fintek configuration mode
                    lpc.pio_outb(port, 0x87)?;
                    lpc.pio_outb(port, 0x87)?;
                    let id = lpc.superio_inb(0x20)?;
                    let rev = lpc.superio_inb(0x21)?;
                    let mut base = None;
                    if id == CHIP_ID && rev == CHIP_REVISION {
                        lpc.find_bars()?;
                        lpc.superio_outb(0x07, HWM_LDN)?; // select the hardware monitor logical device
                        let a = lpc.superio_inw(0x60)?;
                        std::thread::sleep(Duration::from_millis(1));
                        let verify = lpc.superio_inw(0x60)?;
                        if a == verify && a >= 0x100 && a & 0xF007 == 0 {
                            base = Some(a);
                        }
                    }
                    lpc.pio_outb(port, 0xAA)?; // leave configuration mode
                    Ok(base)
                })
                .ok_or("the ISA bus is busy")??;
            if let Some(base) = found {
                let chip = Nct6687 { lpc, isa, base };
                let init = chip.locked(|c| c.read(INIT_REGISTER))?;
                if init & 0x80 == 0 {
                    return Err("the NCT6687D-R monitor is not started yet (open Fan Tuner with the C# helper once)".into());
                }
                return Ok(chip);
            }
        }
        Err("no Nuvoton NCT6687D-R found at Super I/O 0x2E / 0x4E".into())
    }

    fn locked<R>(&self, f: impl FnOnce(&Self) -> Result<R, String>) -> Result<R, String> {
        self.isa.with(100, || f(self)).ok_or_else(|| "the ISA bus is busy".to_string())?
    }

    /// One EC-space byte (page = high byte, index = low byte), with the access handshake LHM uses.
    fn read(&self, address: u16) -> Result<u8, String> {
        let page_port = self.base + EC_PAGE;
        let timeout = Instant::now() + Duration::from_millis(500);
        let mut access = self.lpc.pio_inb(page_port)?;
        while access != EC_PAGE_SELECT && Instant::now() < timeout {
            std::thread::sleep(Duration::from_millis(1));
            access = self.lpc.pio_inb(page_port)?;
        }
        if access != EC_PAGE_SELECT {
            self.lpc.pio_outb(page_port, EC_PAGE_SELECT)?; // force access
        }
        self.lpc.pio_outb(page_port, (address >> 8) as u8)?;
        self.lpc.pio_outb(self.base + EC_INDEX, (address & 0xFF) as u8)?;
        let v = self.lpc.pio_inb(self.base + EC_DATA)?;
        self.lpc.pio_outb(page_port, EC_PAGE_SELECT)?; // free access for other programs
        Ok(v)
    }

    /// Every named temperature and header.
    pub fn read_all(&self) -> Result<Reading, String> {
        self.locked(|c| {
            let mut r = Reading::default();
            for (i, _) in TEMPS {
                let reg = temp_register(i);
                r.temps.push((i, temperature(c.read(reg)?, c.read(reg + 1)?)));
            }
            for (i, _, _) in HEADERS {
                let rpm = ((c.read(FAN_RPM[i])? as u32) << 8 | c.read(FAN_RPM[i] + 1)? as u32) as f32;
                let duty = duty_percent(c.read(FAN_PWM_OUT[i])?);
                r.fans.push((i, rpm, duty));
            }
            Ok(r)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nct6687_conversions() {
        assert_eq!(temperature(55, 0x00), 55.0);
        assert_eq!(temperature(32, 0x80), 32.5);
        assert_eq!(temperature(0xFB, 0x00), -5.0);
        assert_eq!(duty_percent(0), 0.0);
        assert_eq!(duty_percent(255), 100.0);
        assert_eq!(duty_percent(133), 52.0); // CPU Fan 52 % in the C# helper's dump
        assert_eq!(temp_register(4), 0x108);
        // every named header has registers; index 8 is the unused 0xFFF slot
        assert!(HEADERS.iter().all(|(i, _, _)| FAN_PWM_OUT[*i] != 0xFFF));
        assert_eq!(FAN_RPM[14], 0x156, "System Fan #5");
    }
}
