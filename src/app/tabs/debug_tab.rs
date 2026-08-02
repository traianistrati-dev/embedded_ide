//! Debug tab — on-target debugging via `probe-rs dap-server`.
//! Toolbar (start/stop/continue/pause/step) + three panes: console (build +
//! DAP output), stack frames (click → jump + show its variables), variables
//! (locals + registers). See [`crate::debugger::Debugger`].

use super::terminal_tab::render_scrollback;
use crate::app::helpers::help_panel;
use crate::debugger::{DebugPhase, Debugger, Frame};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Memory key of this tab's help panel.
const HELP_ID: &str = "debug";

pub fn show_debug_tab(
    ui: &mut egui::Ui,
    dbg: &mut Debugger,
    // Set when Start is clicked; the caller writes the project and calls
    // `start_debug` (the `build_go` signal pattern).
    debug_go: &mut bool,
    can_run: bool,
    chip: &str,
    // Shared probe selector (RTT + Debug): the scanned list, the chosen
    // `--probe` selector, a "scan" click signal, and the last scan error.
    probes: &[crate::probe::ProbeInfo],
    selected_probe: &mut Option<String>,
    probe_scan: &mut bool,
    probe_scan_err: Option<&str>,
) {
    let phase = dbg.phase();
    let busy = dbg.is_busy();
    let stopped = matches!(phase, DebugPhase::Stopped(_));
    let running = matches!(phase, DebugPhase::Running);

    // ── Toolbar ───────────────────────────────────────────────────────────────
    // Wrapped, not `ui.horizontal`: a plain row keeps allocating past the edge
    // on a narrow panel, and the Code Editor side panel adopts its content's
    // width — an overflowing row would widen the panel every frame.
    ui.horizontal_wrapped(|ui| {
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
        help_panel::toggle_button(ui, HELP_ID);

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

        ui.separator();
        super::probe_selector_ui(ui, probes, selected_probe, probe_scan, probe_scan_err);

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

    // ── Help panel (toggled from the toolbar) ─────────────────────────────────
    help_panel::show_panel(
        ui,
        HELP_ID,
        &[
            (
                "Debug",
                egui::Color32::from_rgb(100, 220, 100),
                "Starts a session: `cargo build --release`, flash the chip, then \
                 attach through `probe-rs dap-server`. The target starts running \
                 and halts as soon as it reaches a breakpoint — the editor jumps \
                 to that line and the panes below fill in.",
            ),
            (
                "Breakpoints",
                egui::Color32::from_rgb(220, 70, 60),
                "Click left of a line number in the editor: a red dot appears \
                 (click it again to remove). They can be set before OR during a \
                 session — changes are pushed to the running session \
                 immediately. Only Rust source files can hold one.",
            ),
            (
                "Continue",
                egui::Color32::from_rgb(200, 210, 230),
                "Resumes a halted target until it hits the next breakpoint. \
                 Shown while the target is stopped; while it runs, the same slot \
                 offers Pause.",
            ),
            (
                "Pause",
                egui::Color32::from_rgb(200, 210, 230),
                "Halts the running target wherever it currently is — the way to \
                 find out where a program that seems stuck is spinning.",
            ),
            (
                "Over / In / Out",
                egui::Color32::from_rgb(200, 210, 230),
                "Only while halted. Over = run the next line, executing any call \
                 on it fully. In = enter the call on the line. Out = run until \
                 the current function returns to its caller.",
            ),
            (
                "Stop",
                egui::Color32::from_rgb(230, 120, 110),
                "Ends the session, kills the DAP server and releases the probe. \
                 The firmware stays on the chip and keeps running. Stop the \
                 session before using the RTT tab — one process at a time may \
                 hold the probe.",
            ),
            (
                "Clear",
                egui::Color32::from_gray(170),
                "Empties the console. It does not touch the target or the \
                 session.",
            ),
        ],
        &[
            "Call stack: the frames of the halted target, innermost first — \
             click a frame to jump to its source line and show its variables. \
             Frames without source (HAL internals, assembly) are greyed.",
            "Variables: the locals of the selected frame plus the core \
             registers; hover a row to see its type.",
            "Console: build progress, probe-rs messages, and the target's \
             RTT/defmt output during the session.",
            "Requires `probe-rs` in PATH (cargo install probe-rs-tools) and a \
             probe: ST-Link / J-Link / CMSIS-DAP, or the built-in USB-JTAG on \
             ESP32-C3.",
            "Probe: with several probes attached, click Scan and pick one — the \
             DAP launch then targets that exact probe (VID:PID:Serial). Auto \
             lets probe-rs choose (fine with a single probe). Shared with the \
             RTT tab.",
            "Debug info comes from the release build (`debug = true` in the \
             generated Cargo.toml). Optimised code can make lines jump around \
             or variables read as <optimized out> — that is the compiler, not \
             the debugger.",
        ],
    );

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
    // The row is allocated EXACTLY the visible size and the panes are placed
    // inside it at fixed rects. Nothing here may ask for more width than the
    // panel has: the Code Editor is an egui side panel, which stores the rect
    // its CONTENT produced as its new width (`PanelState { rect }`), so a row
    // that overflows by a few pixels re-widens the panel every frame — that
    // was this tab's runaway growth, squeezing the MCU / Project panels to
    // their minimum. See `pane_widths`.
    let avail_h = ui.available_height();
    let total_w = ui.available_width();
    let gap = ui.spacing().item_spacing.x.max(4.0);
    // Four panes → three gaps. Widths come from the draggable split boundaries.
    let usable = (total_w - 3.0 * gap).max(0.0);
    let splits = clamp_splits(dbg.pane_splits);
    let [console_w, stack_w, vars_w, watch_w] = split_widths(usable, splits);
    let body_h = (avail_h - 18.0).max(24.0); // minus the pane title row

    // Snapshot the state once (short lock) — the panes render from the copy.
    let (stack, locals, registers, sel_frame, watches) = {
        let st = dbg.state.lock().unwrap();
        (
            st.stack.clone(),
            st.locals.clone(),
            st.registers.clone(),
            st.sel_frame,
            st.watches.clone(),
        )
    };
    let mut select: Option<Frame> = None;
    let mut watch_add: Option<String> = None;
    let mut watch_remove: Option<usize> = None;

    let (row, _) =
        ui.allocate_exact_size(egui::vec2(total_w, avail_h), egui::Sense::hover());
    // Pane left edges: Console | Call stack | Variables | Watch.
    let x0 = row.left();
    let x1 = x0 + console_w + gap;
    let x2 = x1 + stack_w + gap;
    let x3 = x2 + vars_w + gap;
    // Child uis at explicit rects: their content never feeds back into the
    // parent's min_rect (unlike `ui.vertical`, which grows it).
    let pane = |ui: &mut egui::Ui, x: f32, w: f32| -> egui::Ui {
        let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(w, avail_h));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child
    };

    // Console.
    {
        let ui = &mut pane(ui, x0, console_w);
        pane_title(ui, "Console");
        render_scrollback(ui, &dbg.console, "debug_console", body_h);
    }

    // Stack frames.
    {
        let ui = &mut pane(ui, x1, stack_w);
        pane_title(ui, "Call stack");
        egui::ScrollArea::vertical()
            .id_salt("debug_stack")
            .max_height(body_h)
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
    }

    // Variables: locals then registers.
    {
        let ui = &mut pane(ui, x2, vars_w);
        pane_title(ui, "Variables");
        egui::ScrollArea::vertical()
            .id_salt("debug_vars")
            .max_height(body_h)
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
    }

    // Watch: user expressions evaluated against the selected frame.
    {
        let ui = &mut pane(ui, x3, watch_w);
        pane_title(ui, "Watch");
        // Add row: "+" button and an expression field (Enter also adds).
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new(ph::PLUS).size(11.0))
                .on_hover_text("Add a watch expression")
                .clicked()
                && !dbg.watch_draft.trim().is_empty()
            {
                watch_add = Some(std::mem::take(&mut dbg.watch_draft));
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut dbg.watch_draft)
                    .hint_text("expression…")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
            if r.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !dbg.watch_draft.trim().is_empty()
            {
                watch_add = Some(std::mem::take(&mut dbg.watch_draft));
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("debug_watch")
            .max_height((body_h - 24.0).max(24.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if watches.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Right-click a variable in the editor → Add to Watch, \
                             or type an expression above.",
                        )
                        .size(10.0)
                        .color(egui::Color32::from_gray(110)),
                    );
                }
                for (i, w) in watches.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(egui::RichText::new(ph::X).size(10.0))
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            watch_remove = Some(i);
                        }
                        ui.label(
                            egui::RichText::new(&w.expr)
                                .size(10.5)
                                .monospace()
                                .color(egui::Color32::from_rgb(200, 180, 120)),
                        );
                        let val_color = if w.error {
                            egui::Color32::from_gray(110)
                        } else {
                            egui::Color32::from_gray(205)
                        };
                        let resp = ui.label(
                            egui::RichText::new(if w.value.is_empty() { "—" } else { &w.value })
                                .size(10.5)
                                .monospace()
                                .color(val_color),
                        );
                        if let Some(ty) = &w.ty {
                            resp.on_hover_text(ty);
                        }
                    });
                }
            });
    }

    // Draggable separators in the gaps (drawn on top of the panes). Dragging
    // separator k shifts the boundary between pane k and k+1.
    let sep = ui.visuals().widgets.noninteractive.bg_stroke;
    let accent = egui::Color32::from_rgb(90, 140, 210);
    let mut new_splits = splits;
    for (k, sx) in [x1 - gap * 0.5, x2 - gap * 0.5, x3 - gap * 0.5]
        .into_iter()
        .enumerate()
    {
        let handle = egui::Rect::from_min_max(
            egui::pos2(sx - 3.0, row.top()),
            egui::pos2(sx + 3.0, row.bottom()),
        );
        let resp = ui.interact(
            handle,
            ui.id().with(("debug_pane_sep", k)),
            egui::Sense::drag(),
        );
        let hot = resp.hovered() || resp.dragged();
        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let stroke = if hot {
            egui::Stroke::new(2.0, accent)
        } else {
            sep
        };
        ui.painter().vline(sx, row.y_range(), stroke);
        if resp.dragged() && usable > 0.0 {
            new_splits[k] = splits[k] + resp.drag_delta().x / usable;
        }
    }
    if new_splits != splits {
        dbg.pane_splits = clamp_splits(new_splits);
    }

    if let Some(e) = watch_add {
        dbg.add_watch(e);
    }
    if let Some(i) = watch_remove {
        dbg.remove_watch(i);
    }
    if let Some(f) = select {
        dbg.select_frame(&f);
    }
}

/// Minimum fraction of the row each of the four panes keeps — the drag can't
/// squeeze one below this, so nothing ever collapses to zero.
const MIN_SPLIT: f32 = 0.08;

/// Sanitize the three split boundaries (fractions of the row, ascending) so the
/// four panes each keep at least [`MIN_SPLIT`] and stay ordered. Idempotent —
/// re-clamping an already-valid array is a no-op.
fn clamp_splits(mut b: [f32; 3]) -> [f32; 3] {
    b[0] = b[0].clamp(MIN_SPLIT, 1.0 - 3.0 * MIN_SPLIT);
    b[1] = b[1].clamp(b[0] + MIN_SPLIT, 1.0 - 2.0 * MIN_SPLIT);
    b[2] = b[2].clamp(b[1] + MIN_SPLIT, 1.0 - MIN_SPLIT);
    b
}

/// The four pane widths for a row `usable` px wide (row minus its three gaps),
/// derived from the split boundaries `b`. They sum to EXACTLY `usable` and are
/// never negative — the panes shrink together on a narrow panel instead of
/// holding a floor, because a row wider than the panel makes egui re-widen the
/// Code Editor side panel from its content rect, forever (note at the call
/// site). Callers pass boundaries already through [`clamp_splits`].
fn split_widths(usable: f32, b: [f32; 3]) -> [f32; 4] {
    let usable = usable.max(0.0);
    [
        b[0] * usable,
        (b[1] - b[0]) * usable,
        (b[2] - b[1]) * usable,
        (1.0 - b[2]) * usable,
    ]
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

#[cfg(test)]
mod tests {
    use super::{clamp_splits, split_widths, MIN_SPLIT};

    /// The four panes must fit the row EXACTLY at every width — the runaway this
    /// replaced came from floors that out-demanded a narrow panel, and the Code
    /// Editor side panel adopts its content's width, so the row re-widened it
    /// every frame until the MCU and Project panels hit their minimum.
    #[test]
    fn panes_always_fit_the_row() {
        let splits = clamp_splits([0.34, 0.56, 0.78]);
        for usable in [0.0_f32, 1.0, 80.0, 200.0, 418.0, 500.0, 900.0, 1600.0, 4000.0] {
            let w = split_widths(usable, splits);
            assert!(w.iter().all(|x| *x >= 0.0), "negative pane at {usable}: {w:?}");
            let sum: f32 = w.iter().sum();
            assert!(
                (sum - usable).abs() < 0.001,
                "row of {usable} demands {sum} ({w:?})"
            );
        }
        // A degenerate/negative row (panel dragged to nothing) stays at zero.
        assert_eq!(split_widths(-50.0, splits), [0.0, 0.0, 0.0, 0.0]);
    }

    /// Clamping keeps every pane ≥ MIN_SPLIT and the boundaries ascending, even
    /// from garbage input (out of order, out of range, all-collapsed).
    #[test]
    fn clamp_keeps_four_panes_alive() {
        for raw in [
            [0.34, 0.56, 0.78],   // already valid
            [0.9, 0.1, 0.5],      // out of order
            [-1.0, 2.0, 0.0],     // out of range
            [0.0, 0.0, 0.0],      // all collapsed left
            [1.0, 1.0, 1.0],      // all collapsed right
        ] {
            let b = clamp_splits(raw);
            // Ascending with a MIN_SPLIT gap between each of the four panes.
            assert!(b[0] >= MIN_SPLIT - 1e-6, "{b:?}");
            assert!(b[1] - b[0] >= MIN_SPLIT - 1e-6, "{b:?}");
            assert!(b[2] - b[1] >= MIN_SPLIT - 1e-6, "{b:?}");
            assert!(1.0 - b[2] >= MIN_SPLIT - 1e-6, "{b:?}");
        }
        // Idempotent.
        let once = clamp_splits([0.9, 0.1, 0.5]);
        assert_eq!(clamp_splits(once), once);
    }
}
