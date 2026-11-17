// The fan helper (fanhelper/out, built with `dotnet publish`) is embedded into the core library.
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    embed_fan_helper();
}

/// Writes $OUT_DIR/fan_files.rs: `pub const FILES: &[(&str, &[u8])]` with every file of fanhelper/out.
/// Empty when the helper has not been built (the Fan Tuner tab then says so).
fn embed_fan_helper() {
    // crates/pctuner-core -> pctuner/fanhelper/out
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("..").join("..").join("fanhelper").join("out");
    let dir = dir.canonicalize().unwrap_or(dir);
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
