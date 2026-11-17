//! Shared helpers for every tab. The logic (PowerShell bridge, background jobs, JSON history files,
//! notices) lives in pctuner-core and is re-exported here; this file adds the egui-only pieces:
//! the message box, the admin banner, status colours and the Tab trait.

pub use pctuner_core::common::*;

// ------------------------------------------------------------------ UI helpers

/// Status colours (defined in the theme).
pub mod color {
    pub use crate::theme::{DIM, GAMING, JUNK, LOCKED, OK, OPTIONAL};
}

/// A simple message box (OK only). Keep `Option<String>` in the tab.
pub fn message_ui(ctx: &eframe::egui::Context, title: &str, slot: &mut Option<String>) {
    use eframe::egui;
    let mut close = false;
    if let Some(text) = slot.as_ref() {
        crate::theme::dim_backdrop(ctx, "message");
        egui::Window::new("message-dialog")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(crate::theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(500.0);
                ui.label(egui::RichText::new(title).size(20.0).strong());
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(crate::theme::TEXT_DIM));
                });
                ui.add_space(12.0);
                if ui.add(crate::theme::primary_button("   OK   ")).clicked() {
                    close = true;
                }
            });
    }
    if close {
        *slot = None;
    }
}

/// Banner shown on tabs that need admin to change anything.
pub fn admin_banner(ui: &mut eframe::egui::Ui) {
    if !is_admin() {
        crate::theme::banner(ui, color::LOCKED, "View only: start PC Tuner as administrator to apply changes.");
    }
}

/// Every tab implements this.
pub trait Tab {
    fn title(&self) -> &'static str;
    fn ui(&mut self, ui: &mut eframe::egui::Ui);
    /// Called every frame for every tab, visible or not (background work such as the fan profile).
    fn tick(&mut self) {}
    /// Called once when PC Tuner closes (e.g. to hand fans back to the BIOS).
    fn on_exit(&mut self) {}
    /// True while the tab is applying a change; closing PC Tuner then waits for it.
    fn busy(&self) -> bool {
        false
    }
}
