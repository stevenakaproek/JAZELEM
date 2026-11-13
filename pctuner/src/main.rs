#![windows_subsystem = "windows"]
//! PC Tuner - one program for everything built in the PowerShell toolkit:
//! Services, Startup, CPU Scheduler, Bloat removal, Post-install tweaks, USB Port Finder, RTX 5080 profiles.

mod bloat;
mod common;
mod fan;
mod gpu;
mod network;
mod opwin;
mod defender;
mod perf;
mod svcbackup;
mod power;
mod tray;
mod scheduler;
mod services;
mod startup;
mod theme;
mod tweaks;
mod usb;
mod winsvc;
mod hw;

use common::Tab;
use eframe::egui;

/// Sidebar icon for a tab (glyphs from egui's built-in icon font).
fn tab_icon(title: &str) -> &'static str {
    match title {
        "Services" => "⚙",
        "Startup Cleaner" => "🚀",
        "Remove Bloat" => "🗑",
        "Post-install Tweaks" => "🔧",
        "Network" => "🌐",
        "Performance" => "⚡",
        "CPU Scheduler" => "⏱",
        "RTX 5080 Profiles" => "🎮",
        "Fan Tuner" => "🌀",
        "USB Port Finder" => "🔌",
        _ => "•",
    }
}

struct App {
    tabs: Vec<Box<dyn Tab>>,
    active: usize,
    admin: bool,
    /// UI time of the last tab switch (for the fade-in).
    switched_at: f64,
    /// PCTUNER_SELFTEST=1: visit every tab for a few seconds, write a log, then exit (used to test builds).
    selftest: Option<std::time::Instant>,
    /// The Power window (restart / shut down into the BIOS) while it is open.
    power: Option<power::PowerDialog>,
    /// Tray icon (minimize hides PC Tuner to the tray). None if the tray could not be created.
    tray: Option<tray::Tray>,
    /// Self-test: the tab whose screenshot has been requested (one per tab).
    selftest_shot: Option<usize>,
    /// The tab the sidebar last scrolled into view.
    nav_shown: Option<usize>,
    /// The window was closed while a change was running: close as soon as it has finished.
    close_pending: bool,
    /// Close without waiting (self-test end, or "Close anyway").
    force_close: bool,
    /// Messages from background work (crashed task, unreadable history file, ...).
    notice: Option<String>,
}

impl App {
    fn new() -> Self {
        App {
            tabs: vec![
                Box::new(services::ServicesTab::new()),
                Box::new(startup::StartupTab::new()),
                Box::new(bloat::BloatTab::new()),
                Box::new(tweaks::TweaksTab::new()),
                Box::new(network::NetworkTab::new()),
                Box::new(perf::PerfTab::new()),
                Box::new(scheduler::SchedulerTab::new()),
                Box::new(gpu::GpuTab::new()),
                Box::new(fan::FanTab::new()),
                Box::new(usb::UsbTab::new()),
            ],
            // PCTUNER_TAB=<index> opens on that tab (used with the self-test to screenshot one tab)
            active: std::env::var("PCTUNER_TAB").ok().and_then(|v| v.parse::<usize>().ok()).filter(|i| *i < 10).unwrap_or(0),
            admin: common::is_admin(),
            switched_at: 0.0,
            selftest: std::env::var_os("PCTUNER_SELFTEST").map(|_| std::time::Instant::now()),
            // PCTUNER_DEMO=power opens the window for a screenshot (its buttons are never pressed)
            tray: None,
            power: std::env::var("PCTUNER_DEMO").ok().filter(|v| v.eq_ignore_ascii_case("power")).map(|_| power::PowerDialog::new()),
            selftest_shot: None,
            nav_shown: None,
            close_pending: false,
            force_close: false,
            notice: None,
        }
    }

    /// A change is being applied right now (a tab says so, a PowerShell script is running, or an
    /// operation window is in its working stage).
    fn busy(&self, now: f64) -> bool {
        self.tabs.iter().any(|t| t.busy()) || common::scripts_running() > 0 || opwin::running_recently(now)
    }

    /// Closing while busy: cancel the close and close by itself once the change has finished, so its
    /// undo record is written first.
    fn close_guard(&mut self, ctx: &egui::Context, now: f64) {
        let busy = self.busy(now);
        if ctx.input(|i| i.viewport().close_requested()) && !self.force_close && busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_pending = true;
        }
        if !self.close_pending {
            return;
        }
        if !busy {
            self.close_pending = false;
            self.force_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        theme::dim_backdrop(ctx, "close-wait");
        egui::Window::new("close-wait-dialog")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("A change is still being applied").size(20.0).strong());
                });
                ui.label(egui::RichText::new("PC Tuner will close when it finishes, so the change is recorded and can be undone.").color(theme::TEXT_DIM));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary_button("   Keep PC Tuner open   ")).clicked() {
                        self.close_pending = false;
                    }
                    if ui.button("Close anyway (the change may not be recorded)").clicked() {
                        self.close_pending = false;
                        self.force_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }

    fn go_to(&mut self, i: usize, now: f64) {
        if i != self.active {
            self.active = i;
            self.switched_at = now;
        }
    }

    /// One sidebar row: icon + label, accent bar when active, soft hover fade.
    fn nav_row(ui: &mut egui::Ui, icon: &str, label: &str, active: bool) -> bool {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
        let p = ui.painter();
        if active {
            p.rect_filled(rect, 8.0, theme::tint(theme::ACCENT, 26));
            p.rect_filled(egui::Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 8.0), egui::vec2(3.0, rect.height() - 16.0)), 2.0, theme::ACCENT);
        } else if hover > 0.0 {
            p.rect_filled(rect, 8.0, theme::tint(egui::Color32::WHITE, (14.0 * hover) as u8));
        }
        let text_c = if active { theme::TEXT } else { theme::TEXT_DIM.lerp_to_gamma(theme::TEXT, hover) };
        let icon_c = if active { theme::ACCENT } else { text_c };
        p.text(rect.left_center() + egui::vec2(14.0, 0.0), egui::Align2::LEFT_CENTER, icon, egui::FontId::proportional(15.0), icon_c);
        p.text(rect.left_center() + egui::vec2(40.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(14.5), text_c);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // minimize = hide to the tray (the tray menu brings it back)
        if self.tray.is_some() && ctx.input(|i| i.viewport().minimized) == Some(true) && !tray::HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
            tray::HIDDEN.store(true, std::sync::atomic::Ordering::Relaxed);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // Background jobs report through channels; keep the UI polling them.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));

        let now = ctx.input(|i| i.time);
        for t in self.tabs.iter_mut() {
            t.tick();
        }
        let mut go: Option<usize> = None;
        let gpu_index = self.tabs.iter().position(|t| t.title() == "RTX 5080 Profiles");

        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(216.0)
            .frame(egui::Frame::none().fill(theme::NAV).inner_margin(egui::Margin::symmetric(12.0, 16.0)))
            .show(ctx, |ui| {
                // pinned at the bottom in its own area (drawn first so the tab list can never run under it)
                egui::TopBottomPanel::bottom("nav_bottom").frame(egui::Frame::none()).show_separator_line(false).show_inside(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.separator();
                    let p = egui::Button::new(egui::RichText::new("🖥  Power / BIOS").color(theme::TEXT)).min_size(egui::vec2(ui.available_width(), 34.0));
                    if ui.add(p).on_hover_text("Restart into the BIOS, or shut down so the next start opens the BIOS.").clicked() {
                        self.power = Some(power::PowerDialog::new());
                    }
                    let b = theme::danger_button("⟲  Reset GPU to stock").min_size(egui::vec2(ui.available_width(), 34.0));
                    if ui.add(b).on_hover_text("Puts the RTX 5080 back on the Stock preset: no undervolt, no memory overclock.").clicked() {
                        common::GPU_RESET_REQUEST.store(true, std::sync::atomic::Ordering::Relaxed);
                        go = gpu_index;
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        if self.admin {
                            theme::chip(ui, "Administrator", theme::OK);
                        } else {
                            theme::chip(ui, "View only (not admin)", theme::LOCKED);
                        }
                    });
                });
                // header + tabs scroll when the window is too short for all of them
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("PC").size(22.0).strong().color(theme::ACCENT));
                        ui.label(egui::RichText::new("Tuner").size(22.0).strong().color(theme::TEXT));
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("gaming-safe Windows tuning").size(11.5).color(theme::TEXT_FAINT));
                    });
                    ui.add_space(14.0);
                    for (i, t) in self.tabs.iter().enumerate() {
                        let top = ui.cursor().min;
                        if Self::nav_row(ui, tab_icon(t.title()), t.title(), self.active == i) {
                            go = Some(i);
                        }
                        // keep the active tab in view when it changes (e.g. opened by the Reset GPU button)
                        if self.active == i && self.nav_shown != Some(i) {
                            let clip = ui.clip_rect();
                            if top.y >= clip.top() && top.y + 36.0 <= clip.bottom() {
                                self.nav_shown = Some(i);
                            } else {
                                ui.scroll_to_rect(egui::Rect::from_min_size(top, egui::vec2(1.0, 36.0)), None);
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                });
            });
        if let Some(i) = go {
            self.go_to(i, now);
        }

        egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG).inner_margin(egui::Margin::symmetric(22.0, 18.0))).show(ctx, |ui| {
            // short fade-in when the tab changes
            let fade = (((now - self.switched_at) / 0.16) as f32).clamp(0.0, 1.0);
            if fade < 1.0 {
                ui.multiply_opacity(0.25 + 0.75 * fade);
                ctx.request_repaint();
            }
            self.tabs[self.active].ui(ui);
        });

        if let Some(p) = &mut self.power {
            if !p.show(ctx) {
                self.power = None;
            }
        }

        for n in common::take_notices() {
            self.notice = Some(add_notice(self.notice.take(), &n));
        }
        common::message_ui(ctx, "PC Tuner", &mut self.notice);

        self.close_guard(ctx, now);
        self.selftest_step(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // e.g. the Fan Tuner hands every fan back to the BIOS
        for t in self.tabs.iter_mut() {
            t.on_exit();
        }
    }
}

impl App {
    /// Self-test: 20 s per tab (lets background scans finish), a screenshot of each tab, then exit.
    fn selftest_step(&mut self, ctx: &egui::Context) {
        let Some(start) = self.selftest else { return };
        // PCTUNER_SELFTEST=<seconds per tab>; anything that is not a number means 20 s
        let per_tab = std::env::var("PCTUNER_SELFTEST").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v >= 3.0).unwrap_or(20.0);
        let secs = start.elapsed().as_secs_f32();
        // with PCTUNER_TAB only that one tab is visited
        let only = std::env::var("PCTUNER_TAB").ok().and_then(|v| v.parse::<usize>().ok()).filter(|i| *i < self.tabs.len());
        let idx = match only {
            Some(t) if secs < per_tab => t,
            Some(_) => self.tabs.len(),
            None => (secs / per_tab) as usize,
        };
        let secs = if only.is_some() { secs + idx.min(self.tabs.len() - 1) as f32 * per_tab } else { secs };
        let dir = common::data_dir().join("selftest");
        let _ = std::fs::create_dir_all(&dir);
        let log = |line: String| {
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("selftest.log")).and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "{line}")
            });
        };
        // save any screenshot that arrived
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.raw.events.iter().filter_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }).collect()
        });
        for img in shots {
            let name = format!("{}-{}.bmp", self.active, self.tabs[self.active].title().replace(' ', "_"));
            let _ = save_bmp(&dir.join(&name), &img);
            log(format!("screenshot: {name}"));
        }
        if idx >= self.tabs.len() {
            log(format!("ok: {}", self.tabs[self.active].title()));
            log("selftest finished".into());
            self.selftest = None;
            self.force_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if idx != self.active {
            log(format!("ok: {}", self.tabs[self.active].title()));
            self.active = idx;
        }
        // one screenshot per tab, near the end of its slot (a slow frame past that point still gets it)
        let into = secs - idx as f32 * per_tab;
        if into > per_tab - 2.0 && self.selftest_shot != Some(idx) {
            self.selftest_shot = Some(idx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
    }
}

/// Minimal 24-bit BMP writer (no image crate needed).
/// The open notice text with `n` added as a new paragraph: a paragraph already shown is not
/// repeated, and the whole text stays at most NOTICE_DIALOG_MAX characters.
const NOTICE_DIALOG_MAX: usize = 8000;
fn add_notice(old: Option<String>, n: &str) -> String {
    let n = common::cap_text(n, common::NOTICE_MAX_CHARS);
    let Some(old) = old else { return n };
    if old.split("\n\n").any(|p| p == n) || (old.ends_with("...") && old.chars().count() >= NOTICE_DIALOG_MAX) {
        return old;
    }
    common::cap_text(&format!("{old}\n\n{n}"), NOTICE_DIALOG_MAX)
}

#[cfg(test)]
mod notice_tests {
    use super::add_notice;

    #[test]
    fn main_notice_dialog_deduplicates_and_caps() {
        let a = add_notice(None, "file could not be read");
        let b = add_notice(Some(a.clone()), "file could not be read");
        assert_eq!(a, b);
        let c = add_notice(Some(b), "other");
        assert_eq!(c, "file could not be read\n\nother");
        let mut t = None;
        for i in 0..1000 {
            t = Some(add_notice(t, &format!("notice number {i} {}", "x".repeat(50))));
        }
        let t = t.unwrap();
        assert!(t.chars().count() <= super::NOTICE_DIALOG_MAX);
        assert!(t.ends_with("..."));
        assert!(add_notice(None, &"y".repeat(10_000)).chars().count() <= crate::common::NOTICE_MAX_CHARS);
    }
}

fn save_bmp(path: &std::path::Path, img: &egui::ColorImage) -> std::io::Result<()> {
    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut b: Vec<u8> = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    for v in [size, 0, 54, 40] { b.extend_from_slice(&v.to_le_bytes()); }
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    for v in [0u32, row * h, 2835, 2835, 0, 0] { b.extend_from_slice(&v.to_le_bytes()); }
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.pixels[(y * w + x) as usize];
            b.extend_from_slice(&[p.b(), p.g(), p.r()]);
        }
        for _ in 0..(row - w * 3) { b.push(0); }
    }
    std::fs::write(path, b)
}

fn main() -> eframe::Result<()> {
    // `PCTuner.exe --gpu-guard [--dry-run]`: the sign-in check of the GPU trial (no window).
    // windows_subsystem = "windows" hides stderr: keep panic messages in %LOCALAPPDATA%\PCTuner\panic.log
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(common::data_dir().join("panic.log")).and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{} {info}", common::now())
        });
        default_hook(info);
    }));
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gpu-guard") {
        std::process::exit(gpu::guard_main(args.iter().any(|a| a == "--dry-run")));
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PC Tuner")
            // PCTUNER_HEIGHT=<px>: taller window for self-test screenshots of long pages
            .with_inner_size([1250.0, std::env::var("PCTUNER_HEIGHT").ok().and_then(|v| v.parse::<f32>().ok()).filter(|h| (520.0..=4000.0).contains(h)).unwrap_or(780.0)])
            .with_min_inner_size([950.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PC Tuner",
        opts,
        Box::new(|cc| {
            // Dark theme, accent colour, fonts, spacing (pinned: does not follow the Windows light theme).
            theme::apply(&cc.egui_ctx);
            let mut app = App::new();
            // no tray icon during self-tests (screenshots)
            if std::env::var_os("PCTUNER_SELFTEST").is_none() {
                app.tray = tray::create(&cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}
