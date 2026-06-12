//! Clock-tab GUI.
//!
//! Phase 3 (current): a functional control panel — presets, dropdowns for every
//! configurable node, a live computed-frequency table, and a validation list.
//! Phase 4 will add the faithful Figure-2 vector diagram above these controls
//! (sibling modules `layout` / `draw` / `interact`).

pub mod diagram;

use eframe::egui;

use super::compute::ClockFrequencies;
use super::graph::graph_frequencies;
use super::model::{
    ClockLimits, Mco, PllSrc, Stm32f1Clock, SysclkSrc, UsbPre, ADC_PRESCALERS, AHB_PRESCALERS,
    APB_PRESCALERS, PLL_MUL_MAX, PLL_MUL_MIN,
};
use super::presets::{stm32f1_presets, ClockPreset};
use super::validate::{warnings, Severity};

/// Render the Clock tab for an STM32F1 config. Returns `true` if anything
/// changed (the caller relies on `init_frame` to regenerate `main.rs`).
///
/// `limits` are the chip's datasheet ceilings; `presets` the chip-specific
/// one-click configs (empty → the built-in family presets).
///
/// Layout: a thin **Presets** bar on top, a **fixed 3-zone footer** (always
/// visible — Configuration | Frequencies | Info), and in between the
/// **interactive diagram** in its own vertical scroll area.
pub fn draw_clock_tree(
    ui: &mut egui::Ui,
    c: &mut Stm32f1Clock,
    limits: &ClockLimits,
    presets: &[ClockPreset],
) -> bool {
    let mut changed = false;

    // ── Presets (thin top bar) ───────────────────────────────────────────────
    let family_presets;
    let presets: &[ClockPreset] = if presets.is_empty() {
        family_presets = stm32f1_presets();
        &family_presets
    } else {
        presets
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Presets:").strong());
        for p in presets {
            if ui.button(&p.name).on_hover_text(&p.description).clicked() {
                *c = p.config.clone();
                changed = true;
            }
        }
    });
    ui.separator();

    // ── Fixed footer: 3 always-visible zones (25% | 25% | 50%) ───────────────
    egui::TopBottomPanel::bottom("clock_footer")
        .resizable(true)
        .default_height(230.0)
        .min_height(120.0)
        .show_inside(ui, |ui| {
            let f = graph_frequencies(c);
            let total = ui.available_width();
            let h = ui.available_height();
            ui.horizontal_top(|ui| {
                // Zone 1 — Configuration (all nodes)
                ui.allocate_ui_with_layout(
                    egui::vec2(total * 0.25, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("Configuration (all nodes)");
                        egui::ScrollArea::vertical().id_salt("cfg_scroll").show(ui, |ui| {
                            changed |= draw_controls_grid(ui, c);
                        });
                    },
                );
                ui.separator();
                // Zone 2 — Frequencies
                ui.allocate_ui_with_layout(
                    egui::vec2(total * 0.23, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("Frequencies");
                        egui::ScrollArea::vertical().id_salt("freq_scroll").show(ui, |ui| {
                            freq_table(ui, &f, limits);
                        });
                    },
                );
                ui.separator();
                // Zone 3 — Info / validation (≈ 50% width)
                ui.allocate_ui_with_layout(
                    egui::vec2(total * 0.48, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("Info");
                        egui::ScrollArea::vertical().id_salt("info_scroll").show(ui, |ui| {
                            info_zone(ui, c, &f, limits);
                        });
                    },
                );
            });
        });

    // ── Zoom toolbar ─────────────────────────────────────────────────────────
    let zoom_id = egui::Id::new("clock_diagram_zoom");
    let mut zoom = ui.data(|d| d.get_temp::<f32>(zoom_id).unwrap_or(1.0));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Diagram").strong());
        ui.separator();
        ui.label("Zoom:");
        if ui.small_button("−").clicked() {
            zoom = (zoom / 1.15).max(0.4);
        }
        if ui.small_button(format!("{:.0}%", zoom * 100.0)).on_hover_text("Reset to 100%").clicked() {
            zoom = 1.0;
        }
        if ui.small_button("+").clicked() {
            zoom = (zoom * 1.15).min(3.0);
        }
    });
    ui.data_mut(|d| d.insert_temp(zoom_id, zoom));

    // ── Diagram (fits the viewport at zoom 1.0; scrolls when zoomed in) ──────
    let avail_w = ui.available_width();
    let avail_h = ui.available_height();
    egui::ScrollArea::both()
        .id_salt("clock_diagram_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            changed |= diagram::draw(ui, c, limits, avail_w, avail_h, zoom);
        });

    changed
}

/// Render an **imported graph clock** (`ClockConfig::Graph`): the data-driven
/// diagram (drawn by the *same* `draw_static_diagram` the F103 path uses, with
/// values resolved from the graph) plus a validation footer. Read-only for now
/// (interactive editing of graph node states is the next step) → returns `false`.
pub fn draw_graph_clock(
    ui: &mut egui::Ui,
    gc: &mut super::graph::GraphClock,
    limits: &ClockLimits,
) -> bool {
    use super::graph::layout::ValueSrc;
    use super::graph::{evaluate, over_limits, value_from_graph};

    let freqs = evaluate(&gc.graph);
    let mut changed = false;

    // ── Validation footer (always visible) ───────────────────────────────────
    egui::TopBottomPanel::bottom("graph_clock_footer")
        .resizable(true)
        .default_height(110.0)
        .min_height(56.0)
        .show_inside(ui, |ui| {
            ui.strong("Validation");
            egui::ScrollArea::vertical().id_salt("graph_clock_info").show(ui, |ui| {
                let issues = over_limits(&gc.graph, limits, &freqs);
                if issues.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(90, 200, 110),
                        "✔  All clocks within datasheet limits.",
                    );
                } else {
                    for o in issues {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 90, 80),
                            format!("✖  {} = {} exceeds {}", o.node, fmt_mhz(o.hz), fmt_mhz(o.limit)),
                        );
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Imported data-driven clock graph (read-only diagram for now).",
                    )
                    .size(11.0)
                    .color(egui::Color32::from_rgb(150, 158, 172)),
                );
            });
        });

    // ── Zoom toolbar ─────────────────────────────────────────────────────────
    let zoom_id = egui::Id::new("graph_clock_zoom");
    let mut zoom = ui.data(|d| d.get_temp::<f32>(zoom_id).unwrap_or(1.0));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Diagram").strong());
        ui.separator();
        ui.label("Zoom:");
        if ui.small_button("−").clicked() {
            zoom = (zoom / 1.15).max(0.4);
        }
        if ui.small_button(format!("{:.0}%", zoom * 100.0)).on_hover_text("Reset to 100%").clicked() {
            zoom = 1.0;
        }
        if ui.small_button("+").clicked() {
            zoom = (zoom * 1.15).min(3.0);
        }
    });
    ui.data_mut(|d| d.insert_temp(zoom_id, zoom));

    // ── Diagram + interactive dropdowns — shared static renderer + generic
    //    `interactive_graph` editing node states. ─────────────────────────────
    let avail_w = ui.available_width();
    let avail_h = ui.available_height();
    egui::ScrollArea::both()
        .id_salt("graph_clock_diagram")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, tf) = {
                let resolve = |src: &ValueSrc| value_from_graph(src, &freqs);
                diagram::draw_static_diagram(ui, &gc.layout, limits, avail_w, avail_h, zoom, resolve)
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                if diagram::interactive_graph(ui, &tf, &mut gc.graph, &gc.layout.widgets) {
                    changed = true;
                }
            });
        });

    changed
}

/// The Info zone: validation messages plus the datasheet notes/legend.
fn info_zone(ui: &mut egui::Ui, c: &Stm32f1Clock, f: &ClockFrequencies, l: &ClockLimits) {
    let ws = warnings(c, f, l);
    if ws.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(90, 200, 110), "✔  Configuration is valid.");
    } else {
        for w in ws {
            let (color, icon) = match w.severity {
                Severity::Error => (egui::Color32::from_rgb(230, 90, 80), "✖"),
                Severity::Warning => (egui::Color32::from_rgb(225, 185, 60), "⚠"),
            };
            ui.colored_label(color, format!("{icon}  {}", w.msg));
        }
    }
    ui.add_space(6.0);
    ui.separator();
    let dim = egui::Color32::from_rgb(150, 158, 172);
    for line in [
        "HSE = high-speed external · HSI = high-speed internal".to_owned(),
        "LSE = low-speed external · LSI = low-speed internal".to_owned(),
        format!(
            "Limits: SYSCLK {} · HCLK {} · PCLK1 {} · PCLK2 {} · ADC {} · USB {} MHz",
            mhz_num(l.sysclk_max),
            mhz_num(l.hclk_max),
            mhz_num(l.pclk1_max),
            mhz_num(l.pclk2_max),
            mhz_num(l.adcclk_max),
            mhz_num(l.usbclk_hz),
        ),
        format!(
            "USB needs HSE+PLL with USBCLK = {} MHz · ADC 1 µs needs PCLK2 ∈ {{14,28,56}}",
            mhz_num(l.usbclk_hz)
        ),
    ] {
        ui.label(egui::RichText::new(line).size(11.0).color(dim));
    }
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

fn freq_table(ui: &mut egui::Ui, f: &ClockFrequencies, l: &ClockLimits) {
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
            row(ui, "SYSCLK", f.sysclk, Some(l.sysclk_max));
            row(ui, "HCLK (AHB / core)", f.hclk, Some(l.hclk_max));
            row(ui, "PCLK1 (APB1)", f.pclk1, Some(l.pclk1_max));
            row(ui, "PCLK2 (APB2)", f.pclk2, Some(l.pclk2_max));
            row(ui, "TIM2/3/4 clk", f.tim_apb1, None);
            row(ui, "TIM1 clk", f.tim_apb2, None);
            row(ui, "ADCCLK", f.adcclk, Some(l.adcclk_max));
            row(ui, "USBCLK", f.usbclk, None);
            row(ui, "SysTick (HCLK/8)", f.systick, None);
            row(ui, "PLLCLK", f.pllclk, Some(l.sysclk_max));
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

/// Bare MHz number for the compact legend lines, e.g. 72_000_000 → "72".
fn mhz_num(hz: u32) -> String {
    let mhz = hz as f64 / 1_000_000.0;
    if (mhz.fract()).abs() < 1e-6 {
        format!("{}", mhz as u32)
    } else {
        format!("{mhz:.1}")
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
