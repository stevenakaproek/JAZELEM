// Release builds ask for administrator rights (UAC) when started, like the PowerShell tools did.
// Debug builds run without elevation so the app can be tested in view-only mode.
// The fan helper (fanhelper/out, built with `dotnet publish`) is embedded into the exe.
use embed_manifest::{embed_manifest, manifest::ExecutionLevel, new_manifest};
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let level = if std::env::var("PROFILE").as_deref() == Ok("release") {
            ExecutionLevel::RequireAdministrator
        } else {
            ExecutionLevel::AsInvoker
        };
        embed_manifest(new_manifest("PCTuner").requested_execution_level(level)).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
    embed_fan_helper();
}

/// Writes $OUT_DIR/fan_files.rs: `pub const FILES: &[(&str, &[u8])]` with every file of fanhelper/out.
/// Empty when the helper has not been built (the Fan Tuner tab then says so).
fn embed_fan_helper() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("fanhelper").join("out");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut code = String::from("pub const FILES: &[(&str, &[u8])] = &[\n");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
        files.sort();
        for p in files {
            println!("cargo:rerun-if-changed={}", p.display());
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let _ = writeln!(code, "    ({name:?}, include_bytes!({:?})),", p.display().to_string());
        }
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("fan_files.rs");
    std::fs::write(out, code).expect("write fan_files.rs");
}
