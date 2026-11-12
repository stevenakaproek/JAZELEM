//! Design system: one dark palette with a single accent, rounded "glass" cards, type scale,
//! and the small widgets every tab shares (card, hero number, chip, buttons, sparkline, bars).

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, Rounding, Stroke, TextStyle, Vec2};
use eframe::epaint::{Shadow, Shape};

// ------------------------------------------------------------------ palette

pub const BG: Color32 = Color32::from_rgb(12, 14, 17); // app background
pub const NAV: Color32 = Color32::from_rgb(16, 18, 22); // sidebar
pub const SURFACE: Color32 = Color32::from_rgb(22, 25, 30); // cards, windows
pub const SURFACE2: Color32 = Color32::from_rgb(31, 35, 42); // inputs, buttons
pub const SURFACE3: Color32 = Color32::from_rgb(42, 47, 56); // hover
pub const BORDER: Color32 = Color32::from_rgb(46, 51, 60);

/// The one accent colour (electric green).
pub const ACCENT: Color32 = Color32::from_rgb(61, 220, 132);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(30, 122, 72);
pub const ON_ACCENT: Color32 = Color32::from_rgb(6, 22, 12); // text on an accent-filled button

pub const TEXT: Color32 = Color32::from_rgb(232, 236, 241);
pub const TEXT_DIM: Color32 = Color32::from_rgb(152, 160, 172);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(100, 108, 120);

// Status colours (slightly muted so the accent stays dominant).
pub const GAMING: Color32 = Color32::from_rgb(232, 196, 92); // keep / gaming dependency
pub const LOCKED: Color32 = Color32::from_rgb(236, 112, 112); // locked, failed, danger
pub const JUNK: Color32 = Color32::from_rgb(245, 150, 70); // safe to switch off
pub const OPTIONAL: Color32 = Color32::from_rgb(120, 172, 250); // costs a convenience
pub const OK: Color32 = ACCENT; // success
pub const DIM: Color32 = Color32::from_rgb(112, 120, 132); // off / disabled

pub const R_CARD: f32 = 12.0;
pub const R_WIDGET: f32 = 8.0;

/// Translucent tint of a colour (for chips and banners).
pub fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

// ------------------------------------------------------------------ global style

/// Call once at start-up.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);

    // Segoe UI for text; egui's built-in fonts stay as fallback (they carry the icon glyphs).
    let mut fonts = FontDefinitions::default();
    for (name, file) in [("segoe", "segoeui.ttf")] {
        let path = std::path::Path::new(&std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts").join(file);
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.to_owned(), FontData::from_owned(bytes));
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, name.to_owned());
        }
    }
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(10.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.window_margin = egui::Margin::same(18.0);
        s.spacing.indent = 18.0;
        s.animation_time = 0.14;

        let v = &mut s.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0_f32,BORDER);
        v.window_rounding = Rounding::same(14.0);
        v.menu_rounding = Rounding::same(10.0);
        v.window_shadow = Shadow { offset: Vec2::new(0.0, 14.0), blur: 36.0, spread: 0.0, color: Color32::from_black_alpha(150) };
        v.popup_shadow = Shadow { offset: Vec2::new(0.0, 8.0), blur: 20.0, spread: 0.0, color: Color32::from_black_alpha(120) };
        v.extreme_bg_color = Color32::from_rgb(8, 9, 11); // text boxes
        v.faint_bg_color = Color32::from_rgb(18, 21, 25); // striped rows
        v.code_bg_color = SURFACE2;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = tint(ACCENT, 70);
        v.selection.stroke = Stroke::new(1.0_f32,ACCENT);
        v.warn_fg_color = GAMING;
        v.error_fg_color = LOCKED;
        v.slider_trailing_fill = true;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32,BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32,TEXT);
        w.noninteractive.rounding = Rounding::same(R_WIDGET);

        w.inactive.bg_fill = SURFACE2;
        w.inactive.weak_bg_fill = SURFACE2;
        // a thin outline keeps empty checkboxes and quiet buttons visible on cards
        w.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(64, 70, 82));
        w.inactive.fg_stroke = Stroke::new(1.0_f32,TEXT);
        w.inactive.rounding = Rounding::same(R_WIDGET);

        w.hovered.bg_fill = SURFACE3;
        w.hovered.weak_bg_fill = SURFACE3;
        w.hovered.bg_stroke = Stroke::new(1.0_f32,tint(ACCENT, 160));
        w.hovered.fg_stroke = Stroke::new(1.0_f32,Color32::WHITE);
        w.hovered.rounding = Rounding::same(R_WIDGET);
        w.hovered.expansion = 1.0;

        w.active.bg_fill = ACCENT_DARK;
        w.active.weak_bg_fill = ACCENT_DARK;
        w.active.bg_stroke = Stroke::new(1.0_f32,ACCENT);
        w.active.fg_stroke = Stroke::new(1.0_f32,Color32::WHITE);
        w.active.rounding = Rounding::same(R_WIDGET);

        w.open.bg_fill = SURFACE2;
        w.open.weak_bg_fill = SURFACE3;
        w.open.bg_stroke = Stroke::new(1.0_f32,BORDER);
        w.open.rounding = Rounding::same(R_WIDGET);
    });
}

// ------------------------------------------------------------------ containers

/// A raised "glass" panel.
pub fn card_frame() -> egui::Frame {
    egui::Frame::none().fill(SURFACE).rounding(R_CARD).stroke(Stroke::new(1.0_f32,BORDER)).inner_margin(14.0)
}

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame().show(ui, add).inner
}

/// A coloured banner (warnings, notices). `c` is the accent of the banner.
pub fn banner(ui: &mut egui::Ui, c: Color32, text: &str) {
    egui::Frame::none().fill(tint(c, 26)).stroke(Stroke::new(1.0_f32,tint(c, 120))).rounding(R_WIDGET).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(text).color(c));
    });
}

/// Small section caption above a block.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(11.5).color(TEXT_FAINT).strong());
}

/// Page header: title + one-line description.
pub fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(24.0).strong().color(TEXT));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(TEXT_DIM));
    }
    ui.add_space(4.0);
}

// ------------------------------------------------------------------ numbers

/// Animated number: eases towards `target` (use for counters). Returns the value to display.
pub fn count_up(ctx: &egui::Context, id: impl std::hash::Hash, target: f32) -> f32 {
    ctx.animate_value_with_time(egui::Id::new(id), target, 0.6)
}

/// The most important number on a screen: big value, unit beside it, caption underneath.
pub fn hero(ui: &mut egui::Ui, value: &str, unit: &str, caption: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(value).size(44.0).strong().color(color));
        ui.vertical(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(unit).size(18.0).strong().color(TEXT));
            if !caption.is_empty() {
                ui.label(RichText::new(caption).size(12.5).color(TEXT_DIM));
            }
        });
    });
}

/// A secondary stat: smaller number with a label under it.
pub fn stat(ui: &mut egui::Ui, value: &str, label: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(value).size(22.0).strong().color(color));
        ui.label(RichText::new(label).size(12.0).color(TEXT_DIM));
    });
}

/// Rounded pill label.
pub fn chip(ui: &mut egui::Ui, text: &str, c: Color32) {
    // Painted at its natural size: never stretches to a tall row and never wraps inside the pill.
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.5), c);
    let size = galley.size() + Vec2::new(20.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, size.y / 2.0, tint(c, 30));
    ui.painter().galley(rect.min + Vec2::new(10.0, 4.0), galley, c);
}

// ------------------------------------------------------------------ buttons and bars

/// The main action on a screen (accent fill).
pub fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).strong().color(ON_ACCENT)).fill(ACCENT).rounding(R_WIDGET).min_size(Vec2::new(0.0, 32.0))
}

/// A risky action (red outline).
pub fn danger_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(LOCKED)).fill(tint(LOCKED, 24)).stroke(Stroke::new(1.0_f32,tint(LOCKED, 140))).rounding(R_WIDGET)
}

/// Accent progress bar. `frac` in 0..=1.
pub fn progress(ui: &mut egui::Ui, frac: f32, width: f32) {
    ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(width).desired_height(8.0).rounding(4.0).fill(ACCENT));
}

/// Indeterminate "working" bar: a light sweeps across. Keeps the UI repainting while shown.
pub fn sweep(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, SURFACE2);
    let t = (ui.input(|i| i.time) * 0.9).fract() as f32;
    let w = rect.width() * 0.28;
    let x = rect.left() - w + (rect.width() + w) * t;
    let seg = egui::Rect::from_min_max(egui::pos2(x.max(rect.left()), rect.top()), egui::pos2((x + w).min(rect.right()), rect.bottom()));
    if seg.width() > 0.0 {
        p.rect_filled(seg, 4.0, ACCENT);
    }
    ui.ctx().request_repaint();
}

/// Tiny live graph. `values` oldest -> newest. Draws nothing meaningful with fewer than 2 points.
pub fn sparkline(ui: &mut egui::Ui, values: &[f32], size: Vec2, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, tint(Color32::WHITE, 6));
    if values.len() < 2 {
        return;
    }
    let (lo, hi) = spark_range(values);
    let n = values.len();
    let pts: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
            let y = rect.bottom() - 3.0 - (rect.height() - 6.0) * ((v - lo) / (hi - lo));
            egui::pos2(x, y)
        })
        .collect();
    // soft fill under the line (one convex quad per segment)
    for w in pts.windows(2) {
        p.add(Shape::convex_polygon(vec![w[0], w[1], egui::pos2(w[1].x, rect.bottom()), egui::pos2(w[0].x, rect.bottom())], tint(color, 28), Stroke::NONE));
    }
    p.add(Shape::line(pts.clone(), Stroke::new(1.6_f32, color)));
    if let Some(last) = pts.last() {
        p.circle_filled(*last, 2.6, color);
    }
}

/// Vertical range for a sparkline: padded min/max, never zero-height.
pub fn spark_range(values: &[f32]) -> (f32, f32) {
    let lo = values.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let pad = ((hi - lo) * 0.15).max(hi.abs() * 0.01).max(0.5);
    (lo - pad, hi + pad)
}

/// Dark layer behind a modal window; also stops clicks reaching the page underneath.
pub fn dim_backdrop(ctx: &egui::Context, id: &str) {
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new(("dim", id))).order(egui::Order::Middle).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
        ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(150));
    });
}

/// Frame for modal windows (use with `.order(Order::Foreground)`).
pub fn modal_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.style()).inner_margin(22.0).rounding(16.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_spark_range() {
        let (lo, hi) = spark_range(&[50.0, 60.0, 55.0]);
        assert!(lo < 50.0 && hi > 60.0);
        let (lo, hi) = spark_range(&[70.0, 70.0]); // flat line still has height
        assert!(hi - lo >= 1.0);
        assert_eq!(tint(ACCENT, 30).a(), 30);
    }
}
