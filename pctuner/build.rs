// Release builds ask for administrator rights (UAC) when started, like the PowerShell tools did.
// Debug builds run without elevation so the app can be tested in view-only mode.
use embed_manifest::{embed_manifest, manifest::ExecutionLevel, new_manifest};

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
}
