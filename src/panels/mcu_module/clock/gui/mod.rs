//! Clock-tab GUI — the data-driven graph is the only clock model.
//!
//! [`draw_graph_clock`] renders an imported/built-in [`GraphClock`]:
//! a presets bar, a fixed footer (Frequencies | Info with validation), a zoom
//! toolbar, and the interactive diagram (shared static renderer + widget
//! overlay editing graph node states). The old typed `Stm32f1Clock` UI
//! (`draw_clock_tree`, `interactive_nodes`) was retired — its visuals live on
//! as data in `stm32f1_layout`.

pub mod diagram;

use eframe::egui;

use super::compute::frequencies;
use super::graph::layout::ValueSrc;
use super::graph::validate::ceiling_for;
use super::graph::{evaluate, graph_to_stm32f1, over_limits, stm32f1_graph, value_from_graph, GraphClock};
use super::model::ClockLimits;
use super::presets::{stm32f1_presets, ClockPreset};
use super::validate::{warnings, Severity};

/// Render the Clock tab for a graph clock. Returns `true` if anything changed
/// (the caller relies on `init_frame` to regenerate `main.rs`).
///
/// `limits` are the chip's datasheet ceilings; `presets` the chip-specific
/// one-click configs (empty + `family == "stm32f1"` → built-in F103 presets);
/// `family` gates the family-specific extras (presets fallback + footnote
/// validation via the `graph_to_stm32f1` bridge).
pub fn draw_graph_clock(
    ui: &mut egui::Ui,
    gc: &mut GraphClock,
    limits: &ClockLimits,
    presets: &[ClockPreset],
    family: &str,
) -> bool {
    let mut changed = false;
    let is_stm32f1 = family == "stm32f1";

    // ── Presets (thin top bar) ───────────────────────────────────────────────
    // Presets are typed `Stm32f1Clock` configs; applying one expands it to
    // graph node states and adopts them by id (no-op on non-F103 graphs).
    let family_presets;
    let presets: &[ClockPreset] = if presets.is_empty() && is_stm32f1 {
        family_presets = stm32f1_presets();
        &family_presets
    } else {
        presets
    };
    if !presets.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Presets:").strong());
            for p in presets {
                if ui.button(&p.name).on_hover_text(&p.description).clicked() {
                    gc.graph.adopt_states(&stm32f1_graph(&p.config));
                    changed = true;
                }
            }
        });
        ui.separator();
    }

    // Evaluated AFTER any preset click so the footer reflects the new state.
    let freqs = evaluate(&gc.graph);

    // ── Fixed footer: Frequencies | Info (always visible) ───────────────────
    egui::TopBottomPanel::bottom("graph_clock_footer")
        .resizable(true)
        .default_height(190.0)
        .min_height(100.0)
        .show_inside(ui, |ui| {
            let total = ui.available_width();
            let h = ui.available_height();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(total * 0.30, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("Frequencies");
                        egui::ScrollArea::vertical().id_salt("gfreq_scroll").show(ui, |ui| {
                            graph_freq_table(ui, gc, limits, &freqs);
                        });
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(total * 0.66, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("Info");
                        egui::ScrollArea::vertical().id_salt("ginfo_scroll").show(ui, |ui| {
                            graph_info_zone(ui, gc, limits, &freqs, is_stm32f1);
                        });
                    },
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

    // ── Diagram + interactive widgets (graph-backed values) ──────────────────
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

// ── Footer zones ──────────────────────────────────────────────────────────────

/// Delivered-clock table: prefers the layout's labelled output boxes; falls
/// back to every Output / limit-bearing graph node.
fn graph_freq_table(
    ui: &mut egui::Ui,
    gc: &GraphClock,
    limits: &ClockLimits,
    freqs: &std::collections::BTreeMap<String, u32>,
) {
    let row = |ui: &mut egui::Ui, name: &str, hz: u32, over: bool| {
        ui.label(name);
        let color = if over {
            egui::Color32::from_rgb(230, 90, 80)
        } else {
            egui::Color32::from_rgb(150, 200, 160)
        };
        ui.colored_label(color, fmt_mhz(hz));
        ui.end_row();
    };

    egui::Grid::new("graph_clock_freqs")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            if !gc.layout.outputs.is_empty() {
                for o in &gc.layout.outputs {
                    let hz = value_from_graph(&o.src, freqs);
                    let over = o
                        .limit
                        .and_then(|k| ceiling_for(k, limits))
                        .map_or(false, |l| hz > l);
                    row(ui, &o.label, hz, over);
                }
            } else {
                use super::graph::model::NodeKind;
                for node in &gc.graph.nodes {
                    if !matches!(node.kind, NodeKind::Output) && node.limit.is_none() {
                        continue;
                    }
                    let hz = freqs.get(&node.id).copied().unwrap_or(0);
                    let over = node
                        .limit
                        .and_then(|k| ceiling_for(k, limits))
                        .map_or(false, |l| hz > l);
                    row(ui, &node.id, hz, over);
                }
            }
        });
}

/// Validation + legend. STM32F1-family graphs get the FULL typed validation
/// (datasheet footnotes — USB = 48 MHz, HSI→PLL cap, ADC-1 µs APB2 set) via the
/// `graph_to_stm32f1` bridge; other families get the generic ceiling check.
fn graph_info_zone(
    ui: &mut egui::Ui,
    gc: &GraphClock,
    l: &ClockLimits,
    freqs: &std::collections::BTreeMap<String, u32>,
    is_stm32f1: bool,
) {
    if is_stm32f1 {
        let c = graph_to_stm32f1(&gc.graph);
        let ws = warnings(&c, &frequencies(&c), l);
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
    } else {
        let issues = over_limits(&gc.graph, l, freqs);
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
    }

    ui.add_space(6.0);
    ui.separator();
    let dim = egui::Color32::from_rgb(150, 158, 172);
    let mut lines = vec![format!(
        "Limits: SYSCLK {} · HCLK {} · PCLK1 {} · PCLK2 {} · ADC {} · USB {} MHz",
        mhz_num(l.sysclk_max),
        mhz_num(l.hclk_max),
        mhz_num(l.pclk1_max),
        mhz_num(l.pclk2_max),
        mhz_num(l.adcclk_max),
        mhz_num(l.usbclk_hz),
    )];
    if is_stm32f1 {
        lines.insert(0, "HSE = high-speed external · HSI = high-speed internal".to_owned());
        lines.insert(1, "LSE = low-speed external · LSI = low-speed internal".to_owned());
        lines.push(format!(
            "USB needs HSE+PLL with USBCLK = {} MHz · ADC 1 µs needs PCLK2 ∈ {{14,28,56}}",
            mhz_num(l.usbclk_hz)
        ));
    }
    for line in lines {
        ui.label(egui::RichText::new(line).size(11.0).color(dim));
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

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
