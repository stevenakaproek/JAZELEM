//! PC Tuner core: all tuning logic, state and background work, with no UI types.
//! The egui app (pctuner-egui) and the Tauri app are thin layers on top of this crate.

pub mod bloat;
pub mod common;
pub mod defender;
pub mod fan;
pub mod gpolicy;
pub mod gpu;
pub mod network;
pub mod opwin;
pub mod perf;
pub mod ramled;
pub mod scheduler;
pub mod services;
pub mod startup;
pub mod svcbackup;
pub mod tweaks;
pub mod usb;

/// The PowerShell scripts built into the app.
pub mod scripts {
    pub const REMOVE_BLOAT: &str = include_str!("../assets/Remove-Bloat.ps1");
    pub const POST_INSTALL_TWEAKS: &str = include_str!("../assets/PostInstall-Tweaks.ps1");
    /// Restarts Windows Explorer (used by the Startup Cleaner and the Performance page).
    pub const EXPLORER_RESTART: &str = "Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force\nStart-Sleep -Seconds 2\nif (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }";
}

/// The fan / RAM LED helper (fanhelper/out), embedded at build time. Empty when it has not been built.
pub mod fan_files {
    include!(concat!(env!("OUT_DIR"), "/fan_files.rs"));

    /// Unpack the embedded helper to %LOCALAPPDATA%\PCTuner\fan and return the exe path.
    pub fn extract_helper() -> Result<std::path::PathBuf, String> {
        if FILES.is_empty() {
            return Err("The fan helper was not built into this copy of PC Tuner (run fanhelper\\build.ps1, then rebuild).".into());
        }
        let dir = crate::common::data_dir().join("fan");
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        for (name, bytes) in FILES {
            let p = dir.join(name);
            let same = std::fs::read(&p).map(|old| old == *bytes).unwrap_or(false);
            if !same {
                // a file can be locked by a helper that is still shutting down; the exe check below catches real problems
                let _ = std::fs::write(&p, bytes);
            }
        }
        let exe = dir.join("PCTunerFan.exe");
        if exe.exists() {
            Ok(exe)
        } else {
            Err(format!("could not unpack the fan helper to {}", dir.display()))
        }
    }
}
