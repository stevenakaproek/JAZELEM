//! PC Tuner - the Tauri app. A thin shell over pctuner-core: every tab is a `Backend` that owns its core
//! state, a background thread pumps them and sends each tab's view to the web page when it changed, and the
//! page sends actions back. Fan control and other background work run here in Rust, also while the window
//! is hidden in the tray (the web view is released then).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod shell;
mod tabs;

use pctuner_core::common;
use shell::Shell;
use tauri::{Manager, RunEvent, WindowEvent};

fn main() {
    // windows_subsystem = "windows" hides stderr: keep panic messages in %LOCALAPPDATA%\PCTuner\panic.log
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(common::data_dir().join("panic.log")).and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{} {info}", common::now())
        });
        default_hook(info);
    }));
    // `PCTunerNext.exe --gpu-guard [--dry-run]`: the sign-in check of the GPU trial (no window).
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gpu-guard") {
        std::process::exit(pctuner_core::gpu::guard_main(args.iter().any(|a| a == "--dry-run")));
    }

    let app = tauri::Builder::default()
        .manage(Shell::new())
        .invoke_handler(tauri::generate_handler![shell::tab_state, shell::tab_action, shell::shell_info, shell::shell_action])
        .setup(|app| {
            let handle = app.handle().clone();
            // no tray icon during self-tests
            if shell::selftest_secs().is_none() {
                shell::create_tray(&handle)?;
            }
            shell::open_window(&handle)?;
            shell::start_pump(handle);
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // closing while a change is applied: wait for it, so its undo record is written first
            WindowEvent::CloseRequested { api, .. } => {
                let app = window.app_handle();
                let shell = app.state::<Shell>();
                if shell.close_requested(app) {
                    api.prevent_close();
                }
            }
            // minimize = hide to the tray: the window (and its web view) is released, Rust keeps running
            WindowEvent::Resized(_) => {
                if window.is_minimized().unwrap_or(false) {
                    let _ = window.destroy();
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building PC Tuner");

    app.run(|app, event| match event {
        // no window is not the end: PC Tuner lives in the tray until "Quit"
        RunEvent::ExitRequested { api, .. } => {
            if !app.state::<Shell>().quitting() {
                api.prevent_exit();
            }
        }
        // e.g. the Fan Tuner hands every fan back to the BIOS
        RunEvent::Exit => app.state::<Shell>().on_exit(),
        _ => {}
    });
}
