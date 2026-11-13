// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.
// Ported from LibreHardwareMonitor (Hardware/Cpu/Amd17Cpu.cs), Copyright (C) LibreHardwareMonitor and Contributors.

//! AMD Zen (family 17h and later) CPU temperatures through PawnIO's AMDFamily17 module (SMN reads only).

use super::pawnio::{BusMutex, Module, AMD_FAMILY_17, PCI_BUS};

const THM_TCON_CUR_TMP: u32 = 0x0005_9800;
const TEMP_OFFSET_FLAG: u32 = 0x80000;
const M70H_CCD1_TEMP: u32 = 0x0005_9954;
/// Raphael (model 0x61) and Granite Ridge (0x44, e.g. the 9800X3D)
const M61H_CCD1_TEMP: u32 = 0x0005_9B08;
const MAX_CCDS: u32 = 8;

pub const ID_PREFIX: &str = "/amdcpu/0";
/// LHM's temperature indexes: Tctl 0, Tdie 1, Tctl/Tdie 2, then each CCD found, from 3.
const TCTL_TDIE_INDEX: usize = 2;
const FIRST_CCD_INDEX: usize = 3;

/// (family, model) from CPUID, extended fields included.
pub fn family_model() -> (u32, u32) {
    #[cfg(target_arch = "x86_64")]
    {
        let r = core::arch::x86_64::__cpuid(1);
        let base_family = (r.eax >> 8) & 0xF;
        let family = if base_family == 0xF { base_family + ((r.eax >> 20) & 0xFF) } else { base_family };
        let model = ((r.eax >> 4) & 0xF) | (((r.eax >> 16) & 0xF) << 4);
        (family, model)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        (0, 0)
    }
}

/// "AMD Ryzen 7 9800X3D" (the processor name Windows stores, trimmed like LHM's).
pub fn cpu_name() -> String {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|k| k.get_value::<String, _>("ProcessorNameString"))
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ").replace(" 8-Core Processor", "").replace(" Processor", ""))
        .unwrap_or_default()
}

/// Tctl/Tdie from THM_TCON_CUR_TMP: bits 31:21 in 1/8 °C; bit 19 = reported 49 °C high.
pub fn tctl(raw: u32) -> f32 {
    let t = (raw >> 21) as f32 * 0.125;
    if raw & TEMP_OFFSET_FLAG != 0 {
        t - 49.0
    } else {
        t
    }
}

/// A CCD temperature (12 bits, 1/8 °C from -305 / 8); None when the CCD is absent.
pub fn ccd(raw: u32) -> Option<f32> {
    let r = raw & 0xFFF;
    let t = (r as f32 * 125.0 - 305_000.0) * 0.001;
    (r > 0 && t < 125.0).then_some(t)
}

pub struct AmdCpu {
    smn: Module,
    pci: BusMutex,
    model: u32,
    pub name: String,
}

/// (LHM temperature index, name, °C)
pub type Temp = (usize, String, f32);

impl AmdCpu {
    /// Zen only (family 17h or later). Err otherwise, or when PawnIO is not available.
    pub fn open() -> Result<AmdCpu, String> {
        let (family, model) = family_model();
        if family < 0x17 {
            return Err(format!("not an AMD Zen CPU (family {family:#x})"));
        }
        Ok(AmdCpu { smn: Module::load(AMD_FAMILY_17)?, pci: BusMutex::open(PCI_BUS).ok_or("cannot open the PCI bus mutex")?, model, name: cpu_name() })
    }

    pub fn read(&self) -> Result<Vec<Temp>, String> {
        self.pci
            .with(100, || -> Result<Vec<Temp>, String> {
                let mut out = vec![(TCTL_TDIE_INDEX, "Core (Tctl/Tdie)".to_string(), tctl(self.smn.read_smn(THM_TCON_CUR_TMP)?))];
                let first = if self.model == 0x61 || self.model == 0x44 { M61H_CCD1_TEMP } else { M70H_CCD1_TEMP };
                let mut next = FIRST_CCD_INDEX;
                for i in 0..MAX_CCDS {
                    if let Some(t) = ccd(self.smn.read_smn(first + i * 4)?) {
                        out.push((next, format!("CCD{} (Tdie)", i + 1), t));
                        next += 1;
                    }
                }
                Ok(out)
            })
            .ok_or_else(|| "the PCI bus is busy".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amdcpu_conversions() {
        // 55 °C = 440 eighths in bits 31:21
        assert_eq!(tctl(440 << 21), 55.0);
        assert_eq!(tctl((832 << 21) | TEMP_OFFSET_FLAG), 55.0, "the +49 °C range");
        // 45.875 °C: (r * 125 - 305000) / 1000 -> r = 2807
        assert!((ccd(2807).unwrap() - 45.875).abs() < 0.001);
        assert_eq!(ccd(0), None, "absent CCD");
        assert_eq!(ccd(0xFFF), None, "out of range");
        // this PC: family 1Ah (Zen 5), model 44h (Granite Ridge)
        let (f, _) = family_model();
        assert!(f > 0);
        assert!(!cpu_name().is_empty());
    }
}
