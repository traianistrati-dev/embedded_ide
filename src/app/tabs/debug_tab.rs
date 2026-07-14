//! Debug tab — on-target debugging via `probe-rs dap-server`.
//! Toolbar (start/stop/continue/pause/step) + three panes: console (build +
//! DAP output), stack frames (click → jump + show its variables), variables
//! (locals + registers). See [`crate::debugger::Debugger`].

use super::terminal_tab::render_scrollback;
use crate::debugger::{DebugPhase, Debugger, Frame};
use eframe::egui;
use egui_phosphor::regular as ph;

pub fn show_debug_tab(
    ui: &mut egui::Ui,
    dbg: &mut Debugger,
    // Set when Start is clicked; the caller writes the project and calls
    // `start_debug` (the `build_go` signal pattern).
    debug_go: &mut bool,
    can_run: bool,
    chip: &str,
) {
    let phase = dbg.phase();
    let busy = dbg.is_busy();
    let stopped = matches!(phase, DebugPhase::Stopped(_));
    let running = matches!(phase, DebugPhase::Running);

    // ── Toolbar ───────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy && can_run,
                egui::Button::new(
                    egui::RichText::new(format!("{} Debug", ph::BUG))
                        .size(10.5)
                        .color(if !busy && can_run {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else {
                            egui::Color32::GRAY
                        }),
                ),
            )
            .on_hover_text(
                "Start a debug session: cargo build --release, flash, then \
                 attach via `probe-rs dap-server`.\nSet breakpoints by clicking \
                 left of a line number in the editor (red dot) — before or \
                 during the session.",
            )
            .clicked()
        {
            *debug_go = true;
        }
        ui.add_enabled_ui(busy, |ui| {
            if ui
                .button(
                    egui::RichText::new(format!("{} Stop", ph::STOP_CIRCLE))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(230, 120, 110)),
                )
                .clicked()
            {
                dbg.stop();
            }
        });

        ui.separator();

        // Execution controls — Continue/Pause swap on state; steps need a halt.
        if stopped {
            if ui
                .button(egui::RichText::new(format!("{} Continue", ph::PLAY)).size(10.5))
                .on_hover_text("Resume execution (F5-style)")
                .clicked()
            {
                dbg.continue_run();
            }
        } else if ui
            .add_enabled(
                running,
                egui::Button::new(egui::RichText::new(format!("{} Pause", ph::PAUSE)).size(10.5)),
            )
            .on_hover_text("Halt the target where it is")
            .clicked()
        {
            dbg.pause();
        }
        ui.add_enabled_ui(stopped, |ui| {
            if ui
                .button(egui::RichText::new(format!("{} Over", ph::ARROW_BEND_DOWN_RIGHT)).size(10.5))
                .on_hover_text("Step over — next line, calls run through")
                .clicked()
            {
                dbg.step_over();
            }
            if ui
                .button(egui::RichText::new(format!("{} In", ph::ARROW_LINE_DOWN)).size(10.5))
                .on_hover_text("Step into the call on this line")
                .clicked()
            {
                dbg.step_in();
            }
            if ui
                .button(egui::RichText::new(format!("{} Out", ph::ARROW_LINE_UP)).size(10.5))
                .on_hover_text("Run until the current function returns")
                .clicked()
            {
                dbg.step_out();
            }
        });

        if ui
            .button(egui::RichText::new(format!("{} Clear", ph::BROOM)).size(10.5))
            .clicked()
        {
            dbg.clear_console();
        }

        ui.separator();
        ui.label(
            egui::RichText::new("Chip:")
                .size(10.5)
                .color(egui::Color32::GRAY),
        );
        ui.label(
            egui::RichText::new(if chip.is_empty() { "—" } else { chip })
                .size(10.5)
                .monospace()
                .color(egui::Color32::from_rgb(120, 160, 200)),
        );

        // Phase status, right-aligned.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match &phase {
                DebugPhase::Idle => ("—".to_owned(), egui::Color32::GRAY),
                DebugPhase::Building => (
                    "building…".to_owned(),
                    egui::Color32::from_rgb(220, 180, 60),
                ),
                DebugPhase::Launching => (
                    "flashing + attaching…".to_owned(),
                    egui::Color32::from_rgb(220, 180, 60),
                ),
                DebugPhase::Running => (
                    format!("{} running", ph::PLAY),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                DebugPhase::Stopped(r) => (
                    format!("{} stopped: {r}", ph::PAUSE),
                    egui::Color32::from_rgb(230, 180, 60),
                ),
                DebugPhase::Error(e) => (
                    format!("{} {}", ph::X_CIRCLE, e.lines().next().unwrap_or("error")),
                    egui::Color32::from_rgb(230, 90, 80),
                ),
            };
            let label = ui.label(egui::RichText::new(text).size(10.5).color(color));
            if let DebugPhase::Error(e) = &phase {
                label.on_hover_text(e);
            }
            if matches!(phase, DebugPhase::Building | DebugPhase::Launching) {
                crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
            }
        });
    });

    ui.separator();

    // ── Empty-state hint ──────────────────────────────────────────────────────
    let no_content = {
        let st = dbg.state.lock().unwrap();
        st.stack.is_empty() && dbg.console.lock().unwrap().lines.is_empty()
    };
    if no_content {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "On-target debugger (probe-rs).\n\
                 1. Click left of a line number in the editor to set a breakpoint (red dot).\n\
                 2. Debug — builds --release, flashes, attaches.\n\
                 3. When a breakpoint hits: stack + locals + registers appear here,\n\
                    the editor jumps to the line. Continue / Step Over / In / Out above.",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
        return;
    }

    // ── Three panes: console | stack | variables ──────────────────────────────
    let avail_h = ui.available_height();
    let total_w = ui.available_width();
    let stack_w = (total_w * 0.26).clamp(160.0, 340.0);
    let vars_w = (total_w * 0.30).clamp(180.0, 400.0);
    let console_w = (total_w - stack_w - vars_w - 24.0).max(160.0);

    // Snapshot the state once (short lock) — the panes render from the copy.
    let (stack, locals, registers, sel_frame) = {
        let st = dbg.state.lock().unwrap();
        (
            st.stack.clone(),
            st.locals.clone(),
            st.registers.clone(),
            st.sel_frame,
        )
    };
    let mut select: Option<Frame> = None;

    ui.horizontal_top(|ui| {
        // Console.
        ui.vertical(|ui| {
            ui.set_width(console_w);
            pane_title(ui, "Console");
            render_scrollback(ui, &dbg.console, "debug_console", avail_h - 24.0);
        });
        ui.separator();

        // Stack frames.
        ui.vertical(|ui| {
            ui.set_width(stack_w);
            pane_title(ui, "Call stack");
            egui::ScrollArea::vertical()
                .id_salt("debug_stack")
                .max_height(avail_h - 24.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if stack.is_empty() {
                        ui.label(
                            egui::RichText::new(if running {
                                "running — Pause or wait for a breakpoint"
                            } else {
                                "no frames"
                            })
                            .size(10.5)
                            .color(egui::Color32::from_gray(110)),
                        );
                    }
                    for f in &stack {
                        let selected = sel_frame == Some(f.id);
                        let loc = match &f.file_rel {
                            Some(rel) => format!("{rel}:{}", f.line),
                            None => "(no source)".to_owned(),
                        };
                        let color = if f.file_rel.is_some() {
                            egui::Color32::from_gray(210)
                        } else {
                            egui::Color32::from_gray(120)
                        };
                        let text = egui::RichText::new(format!("{}  {loc}", f.name))
                            .size(10.5)
                            .monospace()
                            .color(color);
                        if ui.selectable_label(selected, text).clicked() {
                            select = Some(f.clone());
                        }
                    }
                });
        });
        ui.separator();

        // Variables: locals then registers.
        ui.vertical(|ui| {
            ui.set_width(vars_w);
            pane_title(ui, "Variables");
            egui::ScrollArea::vertical()
                .id_salt("debug_vars")
                .max_height(avail_h - 24.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if locals.is_empty() && registers.is_empty() {
                        ui.label(
                            egui::RichText::new("halt on a breakpoint to inspect")
                                .size(10.5)
                                .color(egui::Color32::from_gray(110)),
                        );
                    }
                    for v in &locals {
                        var_row(ui, v, egui::Color32::from_rgb(120, 170, 240));
                    }
                    if !registers.is_empty() {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Registers")
                                .size(10.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                        for v in &registers {
                            var_row(ui, v, egui::Color32::from_rgb(190, 150, 90));
                        }
                    }
                });
        });
    });

    if let Some(f) = select {
        dbg.select_frame(&f);
    }
}

fn pane_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(10.0)
            .color(egui::Color32::from_gray(140)),
    );
}

/// `name value  (type on hover)` — one variable/register row.
fn var_row(ui: &mut egui::Ui, v: &crate::debugger::VarRow, name_color: egui::Color32) {
    let resp = ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(&v.name)
                .size(10.5)
                .monospace()
                .color(name_color),
        );
        ui.label(
            egui::RichText::new(&v.value)
                .size(10.5)
                .monospace()
                .color(egui::Color32::from_gray(200)),
        );
    });
    if let Some(ty) = &v.ty {
        resp.response.on_hover_text(ty);
    }
}
