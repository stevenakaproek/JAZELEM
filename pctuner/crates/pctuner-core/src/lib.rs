//! PC Tuner core: all tuning logic, state and background work, with no UI types.
//! The egui app (pctuner-egui) and the Tauri app are thin layers on top of this crate.

pub mod common;

/// The PowerShell scripts built into the app.
pub mod scripts {
    pub const REMOVE_BLOAT: &str = include_str!("../assets/Remove-Bloat.ps1");
    pub const POST_INSTALL_TWEAKS: &str = include_str!("../assets/PostInstall-Tweaks.ps1");
}

/// The fan / RAM LED helper (fanhelper/out), embedded at build time. Empty when it has not been built.
pub mod fan_files {
    include!(concat!(env!("OUT_DIR"), "/fan_files.rs"));
}
