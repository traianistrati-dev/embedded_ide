//! Clock-tab GUI.
//!
//! Phase 3 (current): a functional control panel — presets, dropdowns for every
//! configurable node, a live computed-frequency table, and a validation list.
//! Phase 4 will add the faithful Figure-2 vector diagram above these controls
//! (sibling modules `layout` / `draw` / `interact`).

pub mod diagram;

use eframe::egui;

use super::compute::{frequencies, ClockFrequencies};
use super::model::{
    Mco, PllSrc, Stm32f1Clock, SysclkSrc, UsbPre, ADC_PRESCALERS, AHB_PRESCALERS, APB_PRESCALERS,
    PLL_MUL_MAX, PLL_MUL_MIN,
};
use super::presets::stm32f1_presets;
use super::validate::{warnings, Severity};

/// Render the Clock tab for an STM32F1 config. Returns `true` if anything
/// changed (the caller relies on `init_frame` to regenerate `main.rs`).
pub fn draw_clock_tree(ui: &mut egui::Ui, c: &mut Stm32f1Clock) -> bool {
    let mut changed = false;

    // ── Presets ──────────────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Presets:").strong());
        for p in stm32f1_presets() {
            if ui.button(p.name).on_hover_text(p.description).clicked() {
                *c = p.config;
                changed = true;
            }
        }
    });

    ui.add_space(6.0);

    // ── Figure-2 interactive diagram ─────────────────────────────────────────
    changed |= diagram::draw(ui, c);

    ui.add_space(6.0);
    ui.separator();

    // ── Configurable nodes (precise dropdowns; mirror the diagram) ───────────
    let mut grid_changed = false;
    egui::CollapsingHeader::new("Configuration (all nodes)")
        .default_open(false)
        .show(ui, |ui| {
            grid_changed = draw_controls_grid(ui, c);
        });
    changed |= grid_changed;

    ui.add_space(6.0);
    ui.separator();

    // ── Derived frequencies ──────────────────────────────────────────────────
    let f = frequencies(c);
    ui.heading("Frequencies");
    freq_table(ui, &f);

    ui.add_space(6.0);
    ui.separator();

    // ── Validation ───────────────────────────────────────────────────────────
    let ws = warnings(c, &f);
    if ws.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(90, 200, 110),
            "✔  Configuration is valid.",
        );
    } else {
        for w in ws {
            let (color, icon) = match w.severity {
                Severity::Error => (egui::Color32::from_rgb(230, 90, 80), "✖"),
                Severity::Warning => (egui::Color32::from_rgb(225, 185, 60), "⚠"),
            };
            ui.colored_label(color, format!("{icon}  {}", w.msg));
        }
    }

    changed
}

/// The full grid of node dropdowns (collapsible under the diagram).
fn draw_controls_grid(ui: &mut egui::Ui, c: &mut Stm32f1Clock) -> bool {
    let mut changed = false;
    egui::Grid::new("clock_controls")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            // HSE crystal
            ui.label("HSE crystal");
            ui.horizontal(|ui| {
                let mut mhz = c.hse_hz as f64 / 1e6;
                if ui
                    .add(
                        egui::DragValue::new(&mut mhz)
                            .range(1.0..=25.0)
                            .speed(0.1)
                            .suffix(" MHz"),
                    )
                    .changed()
                {
                    c.hse_hz = (mhz * 1e6).round() as u32;
                    changed = true;
                }
                changed |= ui.checkbox(&mut c.hse_enabled, "enabled").changed();
            });
            ui.end_row();

            // SYSCLK source
            ui.label("SYSCLK source (SW)");
            changed |= combo_enum(
                ui,
                "sysclk_src",
                &mut c.sysclk_src,
                &[
                    (SysclkSrc::Hsi, "HSI (8 MHz)"),
                    (SysclkSrc::Hse, "HSE"),
                    (SysclkSrc::Pll, "PLL"),
                ],
            );
            ui.end_row();

            // PLL source
            ui.label("PLL source (PLLSRC)");
            changed |= combo_enum(
                ui,
                "pll_src",
                &mut c.pll_src,
                &[
                    (PllSrc::HsiDiv2, "HSI / 2"),
                    (PllSrc::Hse, "HSE"),
                    (PllSrc::HseDiv2, "HSE / 2 (PLLXTPRE)"),
                ],
            );
            ui.end_row();

            // PLL multiplier
            ui.label("PLL multiplier (PLLMUL)");
            changed |= combo_u8(
                ui,
                "pll_mul",
                &mut c.pll_mul,
                &(PLL_MUL_MIN..=PLL_MUL_MAX).collect::<Vec<_>>(),
                |v| format!("×{v}"),
            );
            ui.end_row();

            // AHB prescaler
            ui.label("AHB prescaler (HPRE)");
            changed |= combo_u16(ui, "ahb", &mut c.ahb_pre, AHB_PRESCALERS, |v| format!("/ {v}"));
            ui.end_row();

            // APB1 prescaler
            ui.label("APB1 prescaler (PPRE1)");
            changed |= combo_u8(ui, "apb1", &mut c.apb1_pre, APB_PRESCALERS, |v| format!("/ {v}"));
            ui.end_row();

            // APB2 prescaler
            ui.label("APB2 prescaler (PPRE2)");
            changed |= combo_u8(ui, "apb2", &mut c.apb2_pre, APB_PRESCALERS, |v| format!("/ {v}"));
            ui.end_row();

            // ADC prescaler
            ui.label("ADC prescaler");
            changed |= combo_u8(ui, "adc", &mut c.adc_pre, ADC_PRESCALERS, |v| format!("/ {v}"));
            ui.end_row();

            // USB prescaler
            ui.label("USB prescaler");
            changed |= combo_enum(
                ui,
                "usb",
                &mut c.usb_pre,
                &[(UsbPre::Div1_5, "/ 1.5"), (UsbPre::Div1, "/ 1")],
            );
            ui.end_row();

            // MCO
            ui.label("MCO output");
            changed |= combo_enum(
                ui,
                "mco",
                &mut c.mco,
                &[
                    (Mco::None, "disabled"),
                    (Mco::Sysclk, "SYSCLK"),
                    (Mco::Hsi, "HSI"),
                    (Mco::Hse, "HSE"),
                    (Mco::PllDiv2, "PLL / 2"),
                ],
            );
            ui.end_row();
        });

    changed
}

// ── Frequency table ───────────────────────────────────────────────────────────

fn freq_table(ui: &mut egui::Ui, f: &ClockFrequencies) {
    egui::Grid::new("clock_freqs")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, name: &str, hz: u32, limit: Option<u32>| {
                ui.label(name);
                let over = limit.map(|l| hz > l).unwrap_or(false);
                let color = if over {
                    egui::Color32::from_rgb(230, 90, 80)
                } else {
                    egui::Color32::from_rgb(150, 200, 160)
                };
                ui.colored_label(color, fmt_mhz(hz));
                ui.end_row();
            };
            const M: u32 = 1_000_000;
            row(ui, "SYSCLK", f.sysclk, Some(72 * M));
            row(ui, "HCLK (AHB / core)", f.hclk, Some(72 * M));
            row(ui, "PCLK1 (APB1)", f.pclk1, Some(36 * M));
            row(ui, "PCLK2 (APB2)", f.pclk2, Some(72 * M));
            row(ui, "TIM2/3/4 clk", f.tim_apb1, None);
            row(ui, "TIM1 clk", f.tim_apb2, None);
            row(ui, "ADCCLK", f.adcclk, Some(14 * M));
            row(ui, "USBCLK", f.usbclk, None);
            row(ui, "SysTick (HCLK/8)", f.systick, None);
            row(ui, "PLLCLK", f.pllclk, Some(72 * M));
        });
}

/// Format Hz as a trimmed MHz string, e.g. 36_000_000 → "36 MHz", 12_500_000 → "12.5 MHz".
fn fmt_mhz(hz: u32) -> String {
    let mhz = hz as f64 / 1_000_000.0;
    if (mhz.fract()).abs() < 1e-6 {
        format!("{} MHz", mhz as u32)
    } else {
        format!("{mhz:.2} MHz")
    }
}

// ── ComboBox helpers ──────────────────────────────────────────────────────────

/// Dropdown over an enum's labelled variants. Returns `true` if the value changed.
fn combo_enum<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| *l)
        .unwrap_or("?");
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (v, label) in options {
                if ui.selectable_label(value == v, *label).clicked() {
                    *value = *v;
                    changed = true;
                }
            }
        });
    changed
}

/// Dropdown over a list of `u8` divider/multiplier options.
fn combo_u8(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut u8,
    options: &[u8],
    label: impl Fn(u8) -> String,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(*value))
        .show_ui(ui, |ui| {
            for &opt in options {
                if ui.selectable_label(*value == opt, label(opt)).clicked() {
                    *value = opt;
                    changed = true;
                }
            }
        });
    changed
}

/// Dropdown over a list of `u16` divider options.
fn combo_u16(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut u16,
    options: &[u16],
    label: impl Fn(u16) -> String,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(*value))
        .show_ui(ui, |ui| {
            for &opt in options {
                if ui.selectable_label(*value == opt, label(opt)).clicked() {
                    *value = opt;
                    changed = true;
                }
            }
        });
    changed
}
