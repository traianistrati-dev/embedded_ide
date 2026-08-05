//! Profile tab — code efficiency. Two modes:
//! - **Static** (`cargo bloat`): `.text`/Flash size per function or crate.
//! - **Runtime** (on-target flamegraph): probe-rs stack-sampling of the running
//!   firmware (see [`crate::flamegraph`]).

use crate::flamegraph::{FlameNode, FlameState};
use crate::profile::{BloatRow, ProfileMode, ProfileState};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};

#[allow(clippy::too_many_arguments)]
pub fn show_profile_tab(
    ui: &mut egui::Ui,
    mode: &mut ProfileMode,
    // ── Static (cargo bloat) ──
    state: &Arc<Mutex<ProfileState>>,
    by_crate: &mut bool,
    run_clicked: &mut bool,
    // ── Runtime (flamegraph) ──
    flame_state: &Arc<Mutex<FlameState>>,
    sample_clicked: &mut bool,
    // The probe-rs chip name (for the runtime hover), and whether a chip config
    // exists (gates both modes).
    chip: &str,
    can_run: bool,
    // Shared probe-rs probe row (same as Debug / RTT / Flash) — shown in the
    // Runtime view so the probe can be scanned/picked from here too.
    probe_list: &[crate::probe::ProbeInfo],
    selected_probe: &mut Option<String>,
    probe_scan: &mut bool,
    probe_scan_err: Option<&str>,
    toolchain: &crate::panels::mcu_module::mcu_catalog::ToolchainKind,
) {
    // ── Mode switch ─────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.selectable_value(mode, ProfileMode::Static, "Static (size)")
            .on_hover_text("cargo bloat: Flash (.text) size per function / crate. No target needed.");
        ui.selectable_value(mode, ProfileMode::Runtime, "Runtime (flamegraph)")
            .on_hover_text(
                "On-target statistical profiler: halt-samples the running firmware's call \
                 stack via probe-rs. Needs the firmware flashed + running.",
            );
    });
    ui.separator();

    match mode {
        ProfileMode::Static => static_view(ui, state, by_crate, run_clicked, can_run),
        ProfileMode::Runtime => runtime_view(
            ui,
            flame_state,
            sample_clicked,
            chip,
            can_run,
            probe_list,
            selected_probe,
            probe_scan,
            probe_scan_err,
            toolchain,
        ),
    }
}

// ── Static (cargo bloat) ──────────────────────────────────────────────────────

fn static_view(
    ui: &mut egui::Ui,
    state: &Arc<Mutex<ProfileState>>,
    by_crate: &mut bool,
    run_clicked: &mut bool,
    can_run: bool,
) {
    let st = state.lock().unwrap().clone();
    let busy = st.is_busy();

    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !busy && can_run,
                egui::Button::new(
                    egui::RichText::new(format!("{} Analyze", ph::CHART_BAR))
                        .size(10.5)
                        .color(if !busy && can_run {
                            egui::Color32::from_rgb(120, 190, 230)
                        } else {
                            egui::Color32::GRAY
                        }),
                ),
            )
            .on_hover_text(
                "cargo bloat --release: build, then report the .text (Flash) size of each \
                 function.\nNeeds cargo-bloat: cargo install cargo-bloat",
            )
            .clicked()
        {
            *run_clicked = true;
        }
        let toggle = ui
            .selectable_label(*by_crate, egui::RichText::new("by crate").size(10.5))
            .on_hover_text("Group the sizes by CRATE instead of by function.");
        if toggle.clicked() {
            *by_crate = !*by_crate;
            if matches!(st, ProfileState::Done(_)) && can_run {
                *run_clicked = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match &st {
                ProfileState::Idle => ("—".to_owned(), egui::Color32::GRAY),
                ProfileState::Running => (
                    "building + analyzing…".to_owned(),
                    egui::Color32::from_rgb(220, 180, 60),
                ),
                ProfileState::Done(r) => (
                    format!("{} {}", r.rows.len(), if r.by_crate { "crates" } else { "fns" }),
                    egui::Color32::from_rgb(120, 190, 120),
                ),
                ProfileState::Failed(_) => {
                    ("failed".to_owned(), egui::Color32::from_rgb(220, 110, 100))
                }
            };
            ui.label(egui::RichText::new(text).size(10.5).color(color));
            if busy {
                ui.spinner();
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
            }
        });
    });
    ui.separator();

    match &st {
        ProfileState::Idle => {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Click Analyze to break down the Flash (.text) size per function. \
                     Biggest first — toggle \"by crate\" to group by dependency.",
                )
                .size(11.0)
                .color(egui::Color32::GRAY),
            );
        }
        ProfileState::Running => {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Building --release and running cargo bloat…").size(11.0).color(egui::Color32::from_rgb(200, 180, 120)));
        }
        ProfileState::Failed(e) => {
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(egui::RichText::new(e).monospace().size(11.0).color(egui::Color32::from_rgb(230, 130, 120)));
            });
        }
        ProfileState::Done(res) => {
            let max = res.rows.iter().map(|r| r.size_bytes).max().unwrap_or(1).max(1);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if !res.summary.is_empty() {
                    ui.label(egui::RichText::new(&res.summary).size(11.0).color(egui::Color32::from_gray(160)));
                    ui.add_space(4.0);
                }
                if res.rows.is_empty() {
                    ui.label(egui::RichText::new(&res.raw).monospace().size(10.5).color(egui::Color32::from_gray(190)));
                }
                for r in &res.rows {
                    bloat_row(ui, r, max);
                }
            });
        }
    }
}

/// One size bar: width ∝ size, coloured small→large, `size · crate · name` on it.
fn bloat_row(ui: &mut egui::Ui, r: &BloatRow, max: u64) {
    let frac = (r.size_bytes as f32 / max as f32).clamp(0.0, 1.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(38));
    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac, rect.height()));
    let col = egui::Color32::from_rgb(
        (70.0 + frac * 155.0) as u8,
        (170.0 - frac * 70.0) as u8,
        (150.0 - frac * 100.0) as u8,
    );
    painter.rect_filled(bar, 2.0, col);
    let label = if r.name.is_empty() {
        format!("{}   {}", r.size_label, r.crate_name)
    } else if r.crate_name.is_empty() {
        format!("{}   {}", r.size_label, r.name)
    } else {
        format!("{}   {}::{}", r.size_label, r.crate_name, r.name)
    };
    painter.text(rect.left_center() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::monospace(11.0), egui::Color32::from_gray(235));
    resp.on_hover_text(format!("{:.1}% of .text · {} bytes", r.text_pct, r.size_bytes));
    ui.add_space(2.0);
}

// ── Runtime (flamegraph) ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn runtime_view(
    ui: &mut egui::Ui,
    flame_state: &Arc<Mutex<FlameState>>,
    sample_clicked: &mut bool,
    chip: &str,
    can_run: bool,
    probe_list: &[crate::probe::ProbeInfo],
    selected_probe: &mut Option<String>,
    probe_scan: &mut bool,
    probe_scan_err: Option<&str>,
    toolchain: &crate::panels::mcu_module::mcu_catalog::ToolchainKind,
) {
    let st = flame_state.lock().unwrap().clone();
    let busy = st.is_busy();

    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !busy && can_run,
                egui::Button::new(
                    egui::RichText::new(format!("{} Sample", ph::FLAME))
                        .size(10.5)
                        .color(if !busy && can_run {
                            egui::Color32::from_rgb(230, 150, 90)
                        } else {
                            egui::Color32::GRAY
                        }),
                ),
            )
            .on_hover_text(
                "Halt-sample the RUNNING firmware's call stack via probe-rs, then fold it \
                 into a flamegraph.\nThe firmware must already be flashed + running. \
                 Intrusive (each halt stops the CPU) → a statistical view.",
            )
            .clicked()
        {
            *sample_clicked = true;
        }
        ui.separator();
        ui.label(egui::RichText::new("Chip:").size(10.5).color(egui::Color32::GRAY));
        ui.label(
            egui::RichText::new(if chip.is_empty() { "—" } else { chip })
                .size(10.5)
                .monospace()
                .color(egui::Color32::from_rgb(120, 160, 200)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match &st {
                FlameState::Sampling(done, total) => {
                    ui.label(egui::RichText::new(format!("{done}/{total}")).size(10.5).color(egui::Color32::from_rgb(220, 180, 60)));
                    ui.spinner();
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                }
                FlameState::Building => {
                    ui.label(egui::RichText::new("building…").size(10.5).color(egui::Color32::from_rgb(220, 180, 60)));
                    ui.spinner();
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
                }
                FlameState::Done(r) => {
                    ui.label(egui::RichText::new(format!("{} samples", r.samples)).size(10.5).color(egui::Color32::from_rgb(120, 190, 120)));
                }
                FlameState::Failed(_) => {
                    ui.label(egui::RichText::new("failed").size(10.5).color(egui::Color32::from_rgb(220, 110, 100)));
                }
                FlameState::Idle => {}
            }
        });
    });
    // Shared probe row — pick the SAME probe as the Debug / RTT / Flash tabs.
    ui.horizontal_wrapped(|ui| {
        super::probe_selector_ui(
            ui,
            probe_list,
            selected_probe,
            probe_scan,
            probe_scan_err,
            toolchain,
        );
    });
    ui.separator();

    match &st {
        FlameState::Idle => {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Flash + run your firmware, then click Sample. probe-rs halt-samples the \
                     call stack; the flamegraph shows where the CPU spends time (wider = more \
                     samples). Statistical + intrusive — not cycle-accurate.",
                )
                .size(11.0)
                .color(egui::Color32::GRAY),
            );
        }
        FlameState::Building => {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Building --release (for symbols)…").size(11.0).color(egui::Color32::from_rgb(200, 180, 120)));
        }
        FlameState::Sampling(done, total) => {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(format!("Sampling the call stack… {done}/{total}")).size(11.0).color(egui::Color32::from_rgb(200, 180, 120)));
        }
        FlameState::Failed(e) => {
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(egui::RichText::new(e).monospace().size(11.0).color(egui::Color32::from_rgb(230, 130, 120)));
            });
        }
        FlameState::Done(res) => {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                flame_widget(ui, &res.root);
            });
        }
    }
}

/// Icicle flamegraph: root at the top (full width), callees stacked below, each
/// as wide as its share of the parent's samples. Warm per-symbol colours.
/// **Clicking** a segment opens a 90%-wide details popup (function, samples, %,
/// and the callee breakdown) — richer than a hover tooltip and it stays put.
fn flame_widget(ui: &mut egui::Ui, root: &FlameNode) {
    let total = root.count.max(1);
    const ROW_H: f32 = 17.0;
    let depth = tree_depth(root);
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(w, (ROW_H * depth as f32).max(ROW_H)),
        egui::Sense::click(),
    );
    let painter = ui.painter().with_clip_rect(rect);
    // The click position picks exactly one segment (its row + x span).
    let click = resp.clicked().then(|| resp.interact_pointer_pos()).flatten();
    let mut hit: Option<FlameNode> = None;
    draw_node(&painter, root, rect.left(), rect.top(), w, ROW_H, click, &mut hit);

    let sel_id = ui.id().with("flame_selected");
    if let Some(node) = hit {
        ui.data_mut(|d| d.insert_temp(sel_id, (node, total)));
    }
    if let Some((node, total)) = ui.data(|d| d.get_temp::<(FlameNode, usize)>(sel_id)) {
        let mut open = true;
        let pw = w * 0.9; // 90% of the flamegraph panel width
        egui::Window::new("Frame details")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_min_width(pw);
                flame_details(ui, &node, total);
            });
        if !open {
            ui.data_mut(|d| d.remove::<(FlameNode, usize)>(sel_id));
        }
    }
}

/// The details popup body for a clicked flame segment.
fn flame_details(ui: &mut egui::Ui, node: &FlameNode, total: usize) {
    let pct = node.count as f32 / total as f32 * 100.0;
    ui.label(
        egui::RichText::new(&node.name)
            .monospace()
            .size(13.0)
            .strong(),
    );
    ui.label(
        egui::RichText::new(format!("{} samples · {:.1}% of all", node.count, pct))
            .color(egui::Color32::from_rgb(120, 190, 120)),
    );
    if node.children.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("leaf — no callees sampled below this frame")
                .italics()
                .color(egui::Color32::GRAY),
        );
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("Callees (share of this frame):").size(11.5));
    let parent = node.count.max(1) as f32;
    let mut kids: Vec<&FlameNode> = node.children.iter().collect();
    kids.sort_by(|a, b| b.count.cmp(&a.count));
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            for c in kids {
                let cp = c.count as f32 / parent * 100.0;
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [46.0, 14.0],
                        egui::Label::new(
                            egui::RichText::new(format!("{cp:>5.1}%")).monospace().size(10.5),
                        ),
                    );
                    // mini share bar
                    let (r, _) = ui.allocate_exact_size(egui::vec2(80.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(r.min, egui::vec2((80.0 * cp / 100.0).max(1.0), 10.0)),
                        1.0,
                        node_color(&c.name),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}  {}", c.count, c.name))
                            .monospace()
                            .size(10.5),
                    );
                });
            }
        });
}

fn tree_depth(node: &FlameNode) -> usize {
    1 + node.children.iter().map(tree_depth).max().unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    painter: &egui::Painter,
    node: &FlameNode,
    x: f32,
    y: f32,
    w: f32,
    row_h: f32,
    click: Option<egui::Pos2>,
    hit: &mut Option<FlameNode>,
) {
    if w < 1.0 {
        return;
    }
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2((w - 1.0).max(0.5), row_h - 1.0));
    painter.rect_filled(rect, 1.0, node_color(&node.name));
    if w > 26.0 {
        painter.text(
            rect.left_center() + egui::vec2(3.0, 0.0),
            egui::Align2::LEFT_CENTER,
            elide(&node.name, w),
            egui::FontId::monospace(10.5),
            egui::Color32::from_rgb(25, 20, 15),
        );
    }
    // The click lands in exactly one row, so this matches a single segment.
    if let Some(p) = click {
        if rect.contains(p) {
            *hit = Some(node.clone());
        }
    }
    let mut cx = x;
    let parent = node.count.max(1) as f32;
    for c in &node.children {
        let cw = w * (c.count as f32 / parent);
        draw_node(painter, c, cx, y + row_h, cw, row_h, click, hit);
        cx += cw;
    }
}

/// Warm per-symbol colour (classic flamegraph palette), stable per name.
fn node_color(name: &str) -> egui::Color32 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    let x = h.finish();
    egui::Color32::from_rgb(
        205 + (x % 50) as u8,
        90 + ((x >> 8) % 120) as u8,
        45 + ((x >> 16) % 40) as u8,
    )
}

/// Truncate a symbol to roughly fit `w` px of monospace (≈6 px/char at 10.5).
fn elide(name: &str, w: f32) -> String {
    let max = ((w - 6.0) / 6.0).floor().max(1.0) as usize;
    if name.chars().count() <= max {
        name.to_owned()
    } else {
        let keep = max.saturating_sub(1);
        format!("{}…", name.chars().take(keep).collect::<String>())
    }
}
