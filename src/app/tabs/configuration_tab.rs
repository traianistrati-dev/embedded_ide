//! The **Configuration** tab: peripherals that have no pin.
//!
//! Everything on the Pins and Peripherals tabs is reached through a pin. A
//! watchdog is not — it is a box you switch on and give a duration to — so it
//! needs a home of its own rather than a row that pretends to be a pin function.
//!
//! # Durations, not register fields
//!
//! CubeMX shows prescaler / window value / downcounter. embassy takes
//! `new(peri, timeout_us[, window_us])` and derives the registers itself from
//! the live PCLK1, so those fields cannot be generated even if they were shown.
//! The tab therefore edits time, and spends its effort on the thing time cannot
//! tell you by itself: whether the chip can actually express it. See
//! [`crate::panels::mcu_module::watchdog`].

use crate::app::AppIde;
use crate::panels::mcu_module::watchdog::{self as wdg, IwdgConfig, WatchdogLimits, WwdgConfig};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Muted body text, matching the other tabs' explanatory lines.
fn dim(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(11.0)
        .color(egui::Color32::from_rgb(130, 130, 145))
}

/// A microsecond count as something a person can read at a glance.
///
/// Watchdog periods span five orders of magnitude — 125 µs to 132 s — so a
/// single unit is unreadable at one end or the other.
fn human_us(us: u32) -> String {
    if us >= 1_000_000 {
        format!("{:.3} s", us as f64 / 1_000_000.0)
    } else if us >= 1_000 {
        format!("{:.3} ms", us as f64 / 1_000.0)
    } else {
        format!("{us} us")
    }
}

impl AppIde {
    /// Render the Configuration tab. Called only with a chip selected.
    pub(in crate::app) fn show_configuration_tab(&mut self, ui: &mut egui::Ui) {
        let Some(mcu) = &mut self.mcu else { return };
        let family = mcu.family.clone();
        let limits = wdg::limits_for(&family);
        // The WWDG's whole range is relative to PCLK1, so the Clock tab feeds
        // this one. 0 = no clock model → the range is unknowable, and the tab
        // says so rather than inventing one.
        let pclk1 = match &mcu.clock {
            crate::panels::mcu_module::clock::ClockConfig::Graph(gc) => {
                crate::panels::mcu_module::clock::graph::evaluate(&gc.graph)
                    .get("pclk1")
                    .copied()
                    .unwrap_or(0)
            }
            // No clock graph (F1's own model, or a chip with none): the WWDG
            // range is unknowable and the card says so instead of guessing.
            _ => 0,
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(dim(
                "Peripherals with no pins of their own. Values are durations - the HAL \
                 derives the prescaler and counter from them, so what matters here is \
                 whether the chip can reach the time you ask for.",
            ));
            ui.add_space(10.0);

            // Not wired for every backend yet: better to say so than to let
            // the cards accept settings that reach no generated file.
            if !wdg::codegen_supported(&family) {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  Watchdog code generation is not implemented for this family yet ({family}). The settings below would not reach the project.",
                        ph::WARNING
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(235, 150, 90)),
                );
                return;
            }

            iwdg_card(ui, &mut mcu.watchdog.iwdg, &limits);
            ui.add_space(12.0);
            wwdg_card(ui, &mut mcu.watchdog.wwdg, &limits, pclk1, &family);
            ui.add_space(10.0);
        });
    }
}

/// One duration row: a drag value in microseconds plus its human reading.
fn duration_row(ui: &mut egui::Ui, label: &str, value: &mut u32, range: (u32, u32)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                // Clamped to what the chip can express, so the common way of
                // reaching an invalid value is simply not available. Typing one
                // still is, which is what the warning below is for.
                .range(range.0..=range.1)
                .speed((range.1 as f64 - range.0 as f64) / 500.0)
                .suffix(" us"),
        );
        ui.label(dim(human_us(*value)));
    });
}

fn iwdg_card(ui: &mut egui::Ui, cfg: &mut Option<IwdgConfig>, l: &WatchdogLimits) {
    let (lo, hi) = wdg::iwdg_range_us(l);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let mut on = cfg.is_some();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, "").changed() {
                *cfg = on.then(|| IwdgConfig::default_for(l));
            }
            ui.label(egui::RichText::new(format!("{}  IWDG", ph::SHIELD)).strong());
            ui.label(dim("independent watchdog"));
        });
        ui.label(dim(format!(
            "Runs off the LSI ({} kHz), so its period does NOT move with the Clock tab. \
             Range {} .. {}.",
            l.lsi_hz / 1000,
            human_us(lo),
            human_us(hi)
        )));
        let Some(c) = cfg.as_mut() else { return };
        ui.add_space(6.0);
        duration_row(ui, "Period", &mut c.timeout_us, (lo, hi));
        ui.add_space(4.0);
        ui.label(dim(
            "Configured but NOT started: the generated code calls unleash() nowhere, \
             so you choose when it starts biting.",
        ));
        problem_and_reset(ui, wdg::iwdg_problem(c, l), || {
            *c = IwdgConfig::default_for(l)
        });
    });
}

fn wwdg_card(
    ui: &mut egui::Ui,
    cfg: &mut Option<WwdgConfig>,
    l: &WatchdogLimits,
    pclk1: u32,
    family: &str,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        // Shown disabled rather than hidden: a control that vanishes on one chip
        // is harder to understand than one that says why it cannot be used.
        if !wdg::wwdg_supported(family) {
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Checkbox::new(&mut false, ""));
                ui.label(
                    egui::RichText::new(format!("{}  WWDG", ph::SHIELD))
                        .strong()
                        .color(egui::Color32::GRAY),
                );
                ui.label(dim("window watchdog"));
            });
            ui.label(dim(
                "Not available on this chip: the STM32F1 HAL (stm32f1xx-hal) implements \
                 only the independent watchdog. Every embassy family has both.",
            ));
            return;
        }
        let range = wdg::wwdg_range_us(l, pclk1);
        let mut on = cfg.is_some();
        ui.horizontal(|ui| {
            let can_enable = range.is_some();
            if ui
                .add_enabled(can_enable, egui::Checkbox::new(&mut on, ""))
                .on_disabled_hover_text(
                    "Set the system clock first - every WWDG period is relative to PCLK1",
                )
                .changed()
            {
                *cfg = on.then(|| WwdgConfig::default_for(l, pclk1)).flatten();
            }
            ui.label(egui::RichText::new(format!("{}  WWDG", ph::SHIELD)).strong());
            ui.label(dim("window watchdog"));
        });
        match range {
            Some((lo, hi)) => ui.label(dim(format!(
                "Counts down from PCLK1 ({} MHz), so its period moves with the Clock tab. \
                 Range {} .. {}.",
                pclk1 / 1_000_000,
                human_us(lo),
                human_us(hi)
            ))),
            None => ui.label(dim(
                "PCLK1 is unknown - configure the Clock tab and the achievable range appears here.",
            )),
        };
        let (Some(c), Some((lo, hi))) = (cfg.as_mut(), range) else {
            return;
        };
        ui.add_space(6.0);
        duration_row(ui, "Period", &mut c.timeout_us, (lo, hi));
        // The window may be zero (no restriction) but never as long as the
        // period; the driver asserts strictly less.
        duration_row(ui, "Closed window", &mut c.window_us, (0, hi));
        ui.add_space(4.0);
        ui.label(dim(
            "Starts immediately and cannot be stopped. Petting DURING the closed window \
             resets the chip, exactly like petting too late - leave it at 0 unless you \
             mean it.",
        ));
        problem_and_reset(ui, wdg::wwdg_problem(c, l, pclk1), || {
            if let Some(d) = WwdgConfig::default_for(l, pclk1) {
                *c = d;
            }
        });
    });
}

/// The shared footer: what is wrong (if anything) and the way back to a value
/// that is known good.
///
/// The warning is not decoration. Every problem it reports is a **panic at
/// boot**, not a compile error, so this line is the only place the mistake can
/// be caught before a board resets in a loop.
fn problem_and_reset(ui: &mut egui::Ui, problem: Option<String>, mut reset: impl FnMut()) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .button(format!("{} Reset", ph::ARROW_COUNTER_CLOCKWISE))
            .on_hover_text("Restore the longest period this chip can express, window disabled")
            .clicked()
        {
            reset();
        }
        if let Some(msg) = problem {
            ui.label(
                egui::RichText::new(format!("{}  {msg}", ph::WARNING))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(235, 150, 90)),
            );
        }
    });
}
