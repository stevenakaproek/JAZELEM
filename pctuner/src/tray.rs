//! System tray icon: minimizing PC Tuner hides it to the tray; right-click the icon for
//! Show PC Tuner / Restart into BIOS / Shut down (BIOS on next start) / Quit.
//! Menu clicks arrive through Windows' message loop, so they work while the window is hidden.

use crate::common;
use crate::power::Choice;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// The window is hidden in the tray right now.
pub static HIDDEN: AtomicBool = AtomicBool::new(false);

pub struct Tray {
    _icon: TrayIcon,
}

/// A small green "PC" disc in the app's accent colour (generated, no image file needed).
fn icon() -> Option<Icon> {
    const N: u32 = 32;
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let d = (dx * dx + dy * dy).sqrt();
            let a = ((15.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
            // a darker ring inside the disc
            let ring = (d - 9.0).abs() < 1.6;
            let (r, g, b) = if ring { (6, 22, 12) } else { (61, 220, 132) };
            px.extend_from_slice(&[r, g, b, a]);
        }
    }
    Icon::from_rgba(px, N, N).ok()
}

/// Bring the hidden window back (Win32, because a hidden eframe window does not run its update loop).
pub fn show_window(ctx: &egui::Context) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    let title: Vec<u16> = "PC Tuner".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
    HIDDEN.store(false, Ordering::Relaxed);
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.request_repaint();
}

fn power(c: Choice) {
    if common::is_admin() {
        std::thread::spawn(move || {
            let _ = common::run("shutdown.exe", &c.args());
        });
    }
}

/// Create the tray icon. None when the tray cannot be created (the app then minimizes normally).
pub fn create(ctx: &egui::Context) -> Option<Tray> {
    let show = MenuItem::new("Show PC Tuner", true, None);
    let admin = common::is_admin();
    let restart = MenuItem::new("Restart into BIOS", admin, None);
    let shutdown = MenuItem::new("Shut down (BIOS on next start)", admin, None);
    let quit = MenuItem::new("Quit PC Tuner", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &PredefinedMenuItem::separator(), &restart, &shutdown, &PredefinedMenuItem::separator(), &quit]).ok()?;
    let (show_id, restart_id, shutdown_id, quit_id) = (show.id().clone(), restart.id().clone(), shutdown.id().clone(), quit.id().clone());

    let c = ctx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == show_id {
            show_window(&c);
        } else if e.id == restart_id {
            power(Choice::RestartToBios);
        } else if e.id == shutdown_id {
            power(Choice::ShutdownToBios);
        } else if e.id == quit_id {
            // show first so eframe runs its normal close (fans back to the BIOS, running changes finish)
            show_window(&c);
            c.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }));
    let c = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let left_click = matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. });
        if left_click || matches!(e, TrayIconEvent::DoubleClick { .. }) {
            show_window(&c);
        }
    }));

    let mut b = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("PC Tuner");
    if let Some(i) = icon() {
        b = b.with_icon(i);
    }
    b.build().ok().map(|i| Tray { _icon: i })
}
