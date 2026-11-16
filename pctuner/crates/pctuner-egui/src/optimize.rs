//! Optimization tab: Performance, Network and Windows tweaks in one place, one sub-tab each.
//! The three parts are the existing pages (perf.rs, network.rs, tweaks.rs), shown without their own header.

use crate::common::{self, Tab};
use crate::theme;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Performance,
    Network,
    Windows,
}

const PARTS: [(Part, &str); 3] = [(Part::Performance, "Performance"), (Part::Network, "Network"), (Part::Windows, "Windows tweaks")];

pub struct OptimizeTab {
    perf: crate::perf::PerfTab,
    network: crate::network::NetworkTab,
    tweaks: crate::tweaks::TweaksTab,
    part: Part,
}

impl OptimizeTab {
    pub fn new() -> Self {
        OptimizeTab { perf: crate::perf::PerfTab::new(), network: crate::network::NetworkTab::new(), tweaks: crate::tweaks::TweaksTab::new(), part: Part::Performance }
    }

    fn get(&mut self, p: Part) -> &mut dyn Tab {
        match p {
            Part::Performance => &mut self.perf,
            Part::Network => &mut self.network,
            Part::Windows => &mut self.tweaks,
        }
    }

    fn part_busy(&self, p: Part) -> bool {
        match p {
            Part::Performance => self.perf.busy(),
            Part::Network => self.network.busy(),
            Part::Windows => self.tweaks.busy(),
        }
    }
}

impl Tab for OptimizeTab {
    fn title(&self) -> &'static str {
        "Optimization"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        theme::page_header(ui, "Optimization", "");
        common::admin_banner(ui);
        // sub-tabs (a dot marks a part that is applying a change)
        ui.horizontal(|ui| {
            for (p, name) in PARTS {
                let on = self.part == p;
                let text = if self.part_busy(p) { format!("  {name}  •  ") } else { format!("  {name}  ") };
                let rt = RichText::new(text).size(15.0).strong().color(if on { theme::ACCENT } else { theme::TEXT_DIM });
                if ui.selectable_label(on, rt).clicked() {
                    self.part = p;
                }
            }
        });
        ui.add_space(4.0);
        let p = self.part;
        self.get(p).ui(ui);
    }

    fn tick(&mut self) {
        for (p, _) in PARTS {
            self.get(p).tick();
        }
    }

    fn on_exit(&mut self) {
        for (p, _) in PARTS {
            self.get(p).on_exit();
        }
    }

    fn busy(&self) -> bool {
        PARTS.iter().any(|(p, _)| self.part_busy(*p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimize_parts() {
        let mut t = OptimizeTab::new();
        assert_eq!(t.title(), "Optimization");
        assert!(!t.busy(), "nothing runs at start");
        assert_eq!(t.get(Part::Performance).title(), "Performance");
        assert_eq!(t.get(Part::Network).title(), "Network");
        assert_eq!(t.get(Part::Windows).title(), "Post-install Tweaks");
    }
}
