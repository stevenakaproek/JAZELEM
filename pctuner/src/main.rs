#![windows_subsystem = "windows"]
//! PC Tuner - one program for everything built in the PowerShell toolkit:
//! Services, Startup, CPU Scheduler, Bloat removal, Post-install tweaks, USB Port Finder, RTX 5080 profiles.

mod bloat;
mod common;
mod gpu;
mod network;
mod opwin;
mod scheduler;
mod services;
mod startup;
mod theme;
mod tweaks;
mod usb;

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
        "CPU Scheduler" => "⏱",
        "RTX 5080 Profiles" => "🎮",
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
                Box::new(scheduler::SchedulerTab::new()),
                Box::new(gpu::GpuTab::new()),
                Box::new(usb::UsbTab::new()),
            ],
            // PCTUNER_TAB=<index> opens on that tab (used with the self-test to screenshot one tab)
            active: std::env::var("PCTUNER_TAB").ok().and_then(|v| v.parse::<usize>().ok()).filter(|i| *i < 8).unwrap_or(0),
            admin: common::is_admin(),
            switched_at: 0.0,
            selftest: std::env::var_os("PCTUNER_SELFTEST").map(|_| std::time::Instant::now()),
        }
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
        // Background jobs report through channels; keep the UI polling them.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));

        let now = ctx.input(|i| i.time);
        let mut go: Option<usize> = None;
        let gpu_index = self.tabs.iter().position(|t| t.title() == "RTX 5080 Profiles");

        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(216.0)
            .frame(egui::Frame::none().fill(theme::NAV).inner_margin(egui::Margin::symmetric(12.0, 16.0)))
            .show(ctx, |ui| {
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
                    if Self::nav_row(ui, tab_icon(t.title()), t.title(), self.active == i) {
                        go = Some(i);
                    }
                }

                // pinned at the bottom: safety button + admin state
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        if self.admin {
                            theme::chip(ui, "Administrator", theme::OK);
                        } else {
                            theme::chip(ui, "View only (not admin)", theme::LOCKED);
                        }
                    });
                    let b = theme::danger_button("⟲  Reset GPU to stock").min_size(egui::vec2(ui.available_width(), 34.0));
                    if ui.add(b).on_hover_text("Puts the RTX 5080 back on the Stock preset: no undervolt, no memory overclock.").clicked() {
                        common::GPU_RESET_REQUEST.store(true, std::sync::atomic::Ordering::Relaxed);
                        go = gpu_index;
                    }
                    ui.separator();
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

        self.selftest_step(ctx);
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
            log("selftest finished".into());
            self.selftest = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if idx != self.active {
            log(format!("ok: {}", self.tabs[self.active].title()));
            self.active = idx;
        }
        // one screenshot near the end of each tab's slot
        let into = secs - idx as f32 * per_tab;
        if into > per_tab - 2.0 && into < per_tab - 1.5 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
    }
}

/// Minimal 24-bit BMP writer (no image crate needed).
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
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gpu-guard") {
        std::process::exit(gpu::guard_main(args.iter().any(|a| a == "--dry-run")));
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PC Tuner")
            .with_inner_size([1250.0, 780.0])
            .with_min_inner_size([950.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PC Tuner",
        opts,
        Box::new(|cc| {
            // Dark theme, accent colour, fonts, spacing (pinned: does not follow the Windows light theme).
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
