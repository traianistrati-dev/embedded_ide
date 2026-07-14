//! LSP completion driver + inline diagnostics, run after the editor widget.
//!
//! Consumes the editor's `TextEditOutput` (cursor + galley) and the text the
//! user just typed.  Applies an accepted completion, detects new triggers
//! (`.`, `::`, Ctrl+Space), renders the completion popup, and finally draws
//! the inline diagnostic overlays.

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::{show_diagnostics_overlay, show_inlay_hint};
use crate::editor::gui::text_pos::{
    diags_for_file, lsp_completion_prefix, lsp_cursor_pos, lsp_kind_icon, lsp_line_end_char_idx,
    lsp_word_start, selected_file_rel_path,
};
use crate::lsp;
use eframe::egui;
use egui::text_edit::TextEditOutput;

// The **inline** diagnostic overlay — squiggles and inline message text drawn
// over the code — is gated at the call site by `self.inline_errors_enabled`
// (toggled from the editor toolbar, default on).
//
// Only **errors and info** are drawn inline; warnings (and hints) are filtered
// out at the call site below and remain in the bottom panel (Cargo Check /
// rust-analyzer tabs) to keep the code view uncluttered. Diagnostics refresh on
// the LSP debounce (3 s after typing stops, or on Project Save — see
// `app::init_frame`), so their positions no longer lag behind active typing.

impl AppIde {
    /// Apply/trigger LSP completion and draw diagnostics, after the editor.
    ///
    /// `display_code` is the current editor text (already written back); it is
    /// taken by value because nothing after this stage reads it again.
    pub(super) fn handle_editor_completion(
        &mut self,
        ui: &mut egui::Ui,
        editor_resp: &TextEditOutput,
        editor_clip: egui::Rect,
        mut display_code: String,
        lsp_accepted: Option<lsp::CompletionItem>,
        ctrl_space_pressed: bool,
        copy_requested: bool,
        ctrl_r_pressed: bool,
        f12_pressed: bool,
        ctrl_f12_pressed: bool,
        // (1-based line, band colour) of the clicked diagnostic to highlight, if
        // in this file (colour keyed by severity).
        highlight: Option<(u32, egui::Color32)>,
        // 1-based line of the F12 definition to highlight (yellow), if in this file.
        def_line: Option<u32>,
    ) {
        // ── LSP completion: post-editor apply + trigger + popup ───────
        let cursor_char_idx = editor_resp
            .state
            .cursor
            .char_range()
            .map(|r| r.primary.index);

        // Apply accepted completion: replace [word_start..cursor] with the
        // item's text. Snippet items (functions/methods with `snippetSupport`)
        // expand to the full call — `foo(a, b)` — and the caret selects the
        // first parameter; plain items land the caret after the inserted text.
        if let Some(item) = lsp_accepted {
            if let Some(cur_idx) = cursor_char_idx {
                let chars: Vec<char> = display_code.chars().collect();
                // Clamp against a stale cursor (text may have shrunk since the
                // cursor was recorded) so the slices below can't panic.
                let cur_idx = cur_idx.min(chars.len());
                let word_start = lsp_word_start(&display_code, cur_idx).min(cur_idx);
                let (mut insert_text, first_stop) = if item.insert_is_snippet {
                    super::snippet::expand(&item.insert_text)
                } else {
                    (item.insert_text.clone(), None)
                };

                // `let name = ` context: a call accepted right after the `=`
                // completes the whole statement — the binding gets the fn's
                // return type and the line is closed:
                //   let my_value: Option<u32> = get_param_value(tx, rx, …);
                // Elsewhere the plain call is inserted, unchanged.
                let mut annotation: Option<(usize, String)> = None;
                if super::let_annotation::is_callable_kind(item.kind)
                    && insert_text.ends_with(')')
                {
                    if let Some(ann_at) = super::let_annotation::let_context(&chars, word_start) {
                        if let Some(ret) = super::let_annotation::return_type(&item.detail) {
                            annotation = Some((ann_at, format!(": {ret}")));
                            // Close the statement only when nothing follows on
                            // the line (don't double up an existing `;`).
                            let line_rest_empty = chars[cur_idx..]
                                .iter()
                                .take_while(|&&c| c != '\n')
                                .all(|c| c.is_whitespace());
                            if line_rest_empty {
                                insert_text.push(';');
                            }
                        }
                    }
                }
                let (ann_at, ann) = annotation.unwrap_or((word_start, String::new()));
                let ann_len = ann.chars().count();

                let before: String = chars[..ann_at].iter().collect();
                let mid: String = chars[ann_at..word_start].iter().collect();
                let after: String = chars[cur_idx..].iter().collect();
                display_code = format!("{}{}{}{}{}", before, ann, mid, insert_text, after);

                // Caret for next frame: select the first-parameter placeholder
                // (typing replaces it), else sit right after the insert. All
                // offsets shift by the `: Type` annotation inserted before it.
                let end = word_start + ann_len + insert_text.chars().count();
                let (sel_start, sel_end) = first_stop
                    .map(|(s, e)| (word_start + ann_len + s, word_start + ann_len + e))
                    .unwrap_or((end, end));
                let mut st = editor_resp.state.clone();
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(sel_start),
                    egui::text::CCursor::new(sel_end),
                )));
                st.store(ui.ctx(), editor_resp.response.id);
                // Mouse accepts move focus to the popup — hand it back so the
                // user can type the argument straight away.
                ui.ctx().memory_mut(|m| m.request_focus(editor_resp.response.id));

                // Persist the change in memory so the write-back picks it up; the
                // debounced LSP flush (3 s idle / Project Save) handles disk + RA.
                if let ProjectFileId::UserFile(i) = self.selected_file {
                    if let Some(entry) = self.project_tree.user_src_files.get_mut(i) {
                        entry.1 = display_code.clone();
                    }
                } else if self.selected_file == ProjectFileId::MainRs {
                    self.generated_code = display_code.clone();
                }
            }
        }

        // Trigger detection
        // LSP completions are available for any .rs file open in RA.
        let lsp_file_tracked = matches!(
            self.selected_file,
            ProjectFileId::MainRs | ProjectFileId::UserFile(_)
        );
        // Compute the relative path for the currently edited file.
        // Used for all LSP requests (did_change, request_completion, etc.)
        let current_rel_path: Option<String> =
            selected_file_rel_path(&self.selected_file, &self.project_tree.user_src_files);
        {
            let lsp_ready = lsp_file_tracked
                && current_rel_path.is_some()
                && matches!(self.lsp_state.lock().unwrap().status, lsp::LspStatus::Ready);
            if lsp_ready {
                let rel = current_rel_path.as_deref().unwrap_or("src/main.rs");
                // Manual Ctrl+Space
                if ctrl_space_pressed {
                    if let Some(idx) = cursor_char_idx {
                        let (line, col) = lsp_cursor_pos(&display_code, idx);
                        // Sync the latest editor text to RA BEFORE the
                        // completion request — the frame's did_change (sent
                        // at the top of update()) used last frame's code.
                        {
                            let mut lsp = self.lsp_state.lock().unwrap();
                            lsp.did_change(rel, &display_code, false);
                            lsp.request_completion(rel, line, col, None);
                        }
                        self.completion_trigger_idx = idx;
                        self.completion_sel = 0;
                        self.completion_open = true;
                        self.completion_note = None;
                    }
                }

                // Auto-trigger on `.`  (method / field access)
                let dot_trigger = editor_resp.response.changed()
                    && cursor_char_idx
                        .map(|idx| {
                            let chars: Vec<char> = display_code.chars().collect();
                            idx > 0 && chars.get(idx - 1) == Some(&'.')
                        })
                        .unwrap_or(false);
                if dot_trigger && !ctrl_space_pressed {
                    if let Some(idx) = cursor_char_idx {
                        let (line, col) = lsp_cursor_pos(&display_code, idx);
                        {
                            let mut lsp = self.lsp_state.lock().unwrap();
                            lsp.did_change(rel, &display_code, false);
                            lsp.request_completion(rel, line, col, Some('.'));
                        }
                        self.completion_trigger_idx = idx;
                        self.completion_sel = 0;
                        self.completion_open = true;
                    }
                }

                // Auto-trigger on `::` (Rust path separator)
                let colon_trigger = !dot_trigger
                    && !ctrl_space_pressed
                    && editor_resp.response.changed()
                    && cursor_char_idx
                        .map(|idx| {
                            let chars: Vec<char> = display_code.chars().collect();
                            idx >= 2
                                && chars.get(idx - 1) == Some(&':')
                                && chars.get(idx - 2) == Some(&':')
                        })
                        .unwrap_or(false);
                if colon_trigger {
                    if let Some(idx) = cursor_char_idx {
                        let (line, col) = lsp_cursor_pos(&display_code, idx);
                        {
                            let mut lsp = self.lsp_state.lock().unwrap();
                            lsp.did_change(rel, &display_code, false);
                            lsp.request_completion(rel, line, col, Some(':'));
                        }
                        self.completion_trigger_idx = idx;
                        self.completion_sel = 0;
                        self.completion_open = true;
                    }
                }
            }

            // Close popup if cursor moved back past the trigger point,
            // or too far ahead (user navigated away from the trigger word).
            if self.completion_open {
                if let Some(idx) = cursor_char_idx {
                    let cursor = idx as isize;
                    let trigger = self.completion_trigger_idx as isize;
                    let delta = cursor - trigger;
                    // delta < 0  → user deleted back past trigger point
                    // delta > 80 → user moved far forward (switched context)
                    if delta < 0 || delta > 80 {
                        self.completion_open = false;
                    }
                }
            }
        }

        // ── Rename (Ctrl+R): capture the symbol + open the rename popup ──
        if ctrl_r_pressed && lsp_file_tracked {
            if let (Some(idx), Some(rel)) = (cursor_char_idx, current_rel_path.clone()) {
                let word = super::rename::identifier_at(&display_code, idx);
                if !word.is_empty() {
                    let (line, col) = lsp_cursor_pos(&display_code, idx);
                    self.rename_active = true;
                    self.rename_focus = true;
                    self.rename_input = word;
                    self.rename_rel = rel;
                    self.rename_line = line;
                    self.rename_char = col;
                    // Anchor the popup just below the cursor.
                    self.rename_popup_pos = editor_resp
                        .state
                        .cursor
                        .char_range()
                        .map(|cr| {
                            let clamped = cr.primary.index.min(
                                editor_resp.galley.job.text.chars().count().saturating_sub(1),
                            );
                            let local = editor_resp
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(clamped));
                            editor_resp.response.rect.left_top()
                                + local.min.to_vec2()
                                + egui::vec2(0.0, local.height() + 4.0)
                        })
                        .unwrap_or_else(|| editor_resp.response.rect.left_top());
                }
            }
        }

        // ── F12 / Ctrl+F12: go to definition / implementation ─────────────────
        // Both funnel through the same result slot and navigation pipeline;
        // Ctrl+F12 resolves the `impl … for …` site where F12 on a trait
        // method would land on the trait's declaration.
        if (f12_pressed || ctrl_f12_pressed) && lsp_file_tracked {
            if let (Some(idx), Some(rel)) = (cursor_char_idx, current_rel_path.clone()) {
                let (line, col) = lsp_cursor_pos(&display_code, idx);
                let mut lsp = self.lsp_state.lock().unwrap();
                lsp.did_change(&rel, &display_code, false);
                if ctrl_f12_pressed {
                    lsp.request_implementation(&rel, line, col);
                } else {
                    lsp.request_definition(&rel, line, col);
                }
                drop(lsp);
                self.definition_in_flight = true;
            }
        }

        // ── LSP completion popup ───────────────────────────────────────
        if self.completion_open {
            let all_items = self.lsp_state.lock().unwrap().completion_items.clone();

            if !all_items.is_empty() {
                // ── Prefix-first ordering ────────────────────────────────
                // The identifier word ending at the cursor (e.g. the "n" typed
                // before pressing Ctrl+Space). `lsp_word_start` stops at `.` / `:`
                // / whitespace, so a `.`/`::` trigger keeps only the part after the
                // separator. Using the whole word (not just text typed after the
                // trigger) is what lets the list lead with what's already typed.
                let prefix = cursor_char_idx
                    .map(|cur| {
                        let word_start = lsp_word_start(&display_code, cur);
                        lsp_completion_prefix(&display_code, word_start, cur)
                    })
                    .unwrap_or_default();

                let filtered = order_by_prefix(all_items, &prefix);

                // Persist filtered list so next frame's key handlers see
                // exactly the same items the user sees right now.
                self.completion_filtered_items = filtered.clone();

                if filtered.is_empty() {
                    // Nothing matches the current prefix — hide the popup.
                    self.completion_open = false;
                } else {
                    // Items on screen — any earlier "why empty" note is stale.
                    self.completion_note = None;
                    // Clamp selection into the visible filtered range.
                    self.completion_sel = self.completion_sel.min(filtered.len() - 1);
                    let sel = self.completion_sel;

                    // ── Popup screen position ────────────────────────────
                    let popup_pos = if let Some(char_range) = editor_resp.state.cursor.char_range()
                    {
                        let cursor_idx = char_range.primary.index;
                        let text_char_count = editor_resp.galley.job.text.chars().count();
                        let clamped = cursor_idx.min(text_char_count.saturating_sub(1));
                        let cursor_local = editor_resp
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(clamped));
                        let offset = egui::vec2(0.0, cursor_local.height() + 2.0);
                        editor_resp.response.rect.left_top() + cursor_local.min.to_vec2() + offset
                    } else {
                        editor_resp.response.rect.left_top()
                    };

                    // ── Render popup ─────────────────────────────────────
                    // `interactable` defaults to true → mouse clicks work.
                    egui::Area::new(egui::Id::new("lsp_completion_popup"))
                        .fixed_pos(popup_pos)
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                ui.set_min_width(440.0);
                                ui.set_max_width(440.0);

                                egui::ScrollArea::vertical()
                                    .max_height(300.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        for (i, item) in filtered.iter().enumerate() {
                                            let selected = i == sel;

                                            let fg = if selected {
                                                egui::Color32::WHITE
                                            } else {
                                                egui::Color32::from_rgb(200, 210, 230)
                                            };
                                            let sel_bg = egui::Color32::from_rgb(40, 90, 160);
                                            let hover_bg = egui::Color32::from_rgb(50, 60, 80);
                                            let detail_fg = if selected {
                                                egui::Color32::from_rgb(160, 195, 255)
                                            } else {
                                                egui::Color32::from_rgb(110, 130, 155)
                                            };

                                            // Allocate the full row width for hit-testing.
                                            let row_h = 19.0;
                                            let avail_w = ui.available_width();
                                            let (rect, row_resp) = ui.allocate_exact_size(
                                                egui::vec2(avail_w, row_h),
                                                egui::Sense::click(),
                                            );

                                            // Background (selected / hovered).
                                            if selected {
                                                ui.painter().rect_filled(rect, 2.0, sel_bg);
                                            } else if row_resp.hovered() {
                                                ui.painter().rect_filled(rect, 2.0, hover_bg);
                                            }

                                            let painter = ui.painter();
                                            let icon = lsp_kind_icon(item.kind);
                                            let label = format!("{} {}", icon, item.label);

                                            // Icon + label — left-aligned.
                                            painter.text(
                                                rect.left_center() + egui::vec2(4.0, 0.0),
                                                egui::Align2::LEFT_CENTER,
                                                &label,
                                                egui::FontId::monospace(12.0),
                                                fg,
                                            );

                                            // Detail (type signature) — right-aligned,
                                            // smaller and dimmer, truncated if needed.
                                            if !item.detail.is_empty() {
                                                let det = {
                                                    let chars: Vec<char> =
                                                        item.detail.chars().collect();
                                                    if chars.len() > 38 {
                                                        format!(
                                                            "{}…",
                                                            chars[..35].iter().collect::<String>()
                                                        )
                                                    } else {
                                                        item.detail.clone()
                                                    }
                                                };
                                                painter.text(
                                                    rect.right_center() - egui::vec2(4.0, 0.0),
                                                    egui::Align2::RIGHT_CENTER,
                                                    det,
                                                    egui::FontId::monospace(10.5),
                                                    detail_fg,
                                                );
                                            }

                                            // Mouse click → deferred insert.
                                            if row_resp.clicked() {
                                                self.completion_pending_insert =
                                                    Some(item.clone());
                                                self.completion_open = false;
                                            }

                                            // Scroll selected item into view.
                                            if selected {
                                                row_resp.scroll_to_me(None);
                                            }

                                            // Hover tooltip: documentation first,
                                            // then full detail as fallback.
                                            if !item.documentation.is_empty() {
                                                row_resp.on_hover_text(
                                                    egui::RichText::new(&item.documentation)
                                                        .size(11.5),
                                                );
                                            } else if !item.detail.is_empty() {
                                                row_resp.on_hover_text(
                                                    egui::RichText::new(&item.detail)
                                                        .monospace()
                                                        .size(11.0),
                                                );
                                            }
                                        }
                                    }); // ScrollArea
                            }); // Frame
                        }); // Area
                }
            }
            // all_items is empty: either RA hasn't responded yet, or
            // it responded with no completions / an error.
            else if lsp_file_tracked {
                let (resp_received, timed_out) = {
                    let lsp = self.lsp_state.lock().unwrap();
                    let received = lsp.completion_response_received;
                    let timeout = lsp
                        .completion_request_sent_at
                        .map(|t| t.elapsed().as_secs() > 6)
                        .unwrap_or(false);
                    (received, timeout)
                };

                if resp_received || timed_out {
                    // RA answered (empty) or request is stale — close the
                    // popup, but SAY WHY at the cursor: the silent one-frame
                    // flash ("apare și dispare") was undiagnosable. The most
                    // common real cause is a file that no `mod …;` declares —
                    // rust-analyzer detaches it and answers `null` to every
                    // completion request in it.
                    self.completion_open = false;
                    let note = if timed_out && !resp_received {
                        "rust-analyzer did not answer (busy / indexing) — try again".to_owned()
                    } else {
                        self.unlinked_module_hint()
                            .unwrap_or_else(|| "no suggestions here".to_owned())
                    };
                    self.completion_note = Some((note, std::time::Instant::now()));
                } else {
                    // Still waiting — show a small spinner popup.
                    let popup_pos = cursor_char_idx.and_then(|_| {
                        editor_resp.state.cursor.char_range().map(|cr| {
                            let clamped = cr.primary.index.min(
                                editor_resp
                                    .galley
                                    .job
                                    .text
                                    .chars()
                                    .count()
                                    .saturating_sub(1),
                            );
                            let local = editor_resp
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(clamped));
                            editor_resp.response.rect.left_top()
                                + local.min.to_vec2()
                                + egui::vec2(0.0, local.height() + 2.0)
                        })
                    });
                    if let Some(pos) = popup_pos {
                        egui::Area::new(egui::Id::new("lsp_completion_loading"))
                            .fixed_pos(pos)
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new("  rust-analyzer…")
                                                .size(11.5)
                                                .color(egui::Color32::from_rgb(160, 175, 200)),
                                        );
                                    });
                                    ui.add_space(2.0);
                                });
                            });
                        ui.ctx().request_repaint();
                    }
                }
            }
        }

        // ── "Why was the list empty?" note ─────────────────────────────────
        // Shown at the cursor for a few seconds after a completion request came
        // back with nothing (most often: the file has no `mod …;` declaration,
        // so rust-analyzer does not analyze it at all). Cleared by its timeout,
        // by typing, or by the next successful popup.
        if let Some((note, at)) = self.completion_note.clone() {
            if at.elapsed().as_secs_f32() > 6.0 || editor_resp.response.changed() {
                self.completion_note = None;
            } else {
                let pos = editor_resp
                    .state
                    .cursor
                    .char_range()
                    .map(|cr| {
                        let clamped = cr.primary.index.min(
                            editor_resp.galley.job.text.chars().count().saturating_sub(1),
                        );
                        let local = editor_resp
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(clamped));
                        editor_resp.response.rect.left_top()
                            + local.min.to_vec2()
                            + egui::vec2(0.0, local.height() + 2.0)
                    })
                    .unwrap_or_else(|| editor_resp.response.rect.left_top());
                egui::Area::new(egui::Id::new("lsp_completion_note"))
                    .fixed_pos(pos)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(
                                egui::RichText::new(&note)
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(230, 190, 90)),
                            );
                        });
                    });
                // Keep frames coming so the timeout fires without input.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(400));
            }
        }

        // ── Diagnostic overlays ───────────────────────────────────────
        if lsp_file_tracked && self.inline_errors_enabled {
            // Only draw the inline overlay when RA holds the CURRENT text for the
            // displayed file (per-file, not a global check). With pending edits
            // the diagnostics are stale — their line/col cling to a row that was
            // moved or deleted, so a squiggle/message "sticks" after the bad line
            // is gone. They reappear (refreshed) once the LSP debounce re-verifies
            // (3 s idle / Project Save). Errors that live inside `#[entry] fn main`
            // come from cargo-check (RA can't expand the entry macro), so they
            // surface a moment after that check completes.
            let diags: Vec<lsp::LspDiagnostic> = current_rel_path
                .as_deref()
                .map(|rel| {
                    let lsp = self.lsp_state.lock().unwrap();
                    // Show the diagnostics only when RA holds the CURRENT text for
                    // this file AND has re-published since the last edit was sent.
                    // `last_sent_matches` hides them the instant you type (before
                    // the flush); `diagnostics_fresh` then keeps them hidden in the
                    // window between the flush (didChange) and RA's fresh publish —
                    // otherwise the OLD diagnostics paint over the NEW text at
                    // shifted line/cols (a fixed error lingering on the wrong line).
                    if lsp.last_sent_matches(rel, &display_code) && lsp.diagnostics_fresh(rel) {
                        // Flycheck (cargo check) diagnostics keep rustc's old
                        // line/cols until the next check finishes, so they'd paint
                        // on stale/commented lines after an edit. Hide them until a
                        // fresh check completes; RA's native diagnostics are
                        // re-mapped per edit and stay visible — this ALSO includes
                        // numbered hard errors (`E0425`, …), which RA computes
                        // natively regardless of `source == "rustc"` (see
                        // `LspDiagnostic::is_rustc_error_code`).
                        let flycheck_stale = lsp.flycheck_stale();
                        diags_for_file(&lsp.diagnostics, rel)
                            .into_iter()
                            // Inline overlay shows only errors and info; warnings +
                            // hints are left to the bottom diagnostics panel.
                            .filter(|d| {
                                matches!(
                                    d.severity,
                                    lsp::DiagSeverity::Error | lsp::DiagSeverity::Info
                                )
                            })
                            .filter(|d| {
                                d.source == "rust-analyzer" || d.is_rustc_error_code() || !flycheck_stale
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                })
                .unwrap_or_default();

            // Clip strictly to the VISIBLE editor area. The code editor wraps the
            // text in nested scroll areas, so `text_clip_rect` (and the editor's
            // response rect) cover the *full* galley — every line, even scrolled-
            // off ones — which is why clipping to those let squiggles/messages for
            // off-screen lines bleed into the bottom panel. `editor_clip` is the
            // editor's on-screen region (captured before it filled the space; its
            // bottom edge is the top of the diagnostics panel), so it bounds the
            // overlay to what's actually visible.
            let visible_clip = editor_clip;
            show_diagnostics_overlay(
                ui,
                editor_resp.galley_pos,
                visible_clip,
                &editor_resp.galley,
                &diags,
                &display_code,
                copy_requested,
                highlight,
                def_line,
            );
        }

        // ── Inferred-type ghost hint (cursor line only) ────────────────
        // Independent of the inline-errors toggle: request/clear the hint for
        // the caret's untyped `let`, then draw it as dim ghost text at the END
        // of the line (Tab to insert — handled in `mod.rs`, applied in
        // `init_frame`). End-of-line, not inline after the name: an overlay
        // can't push the real code aside, so an inline hint overlapped the ` =
        // initializer` — drawing after the line keeps both readable.
        let inlay_line =
            self.update_inlay_hint(&display_code, cursor_char_idx, current_rel_path.as_deref());
        if let (Some(line), Some(hint)) = (inlay_line, self.inlay_hint.as_ref()) {
            // Only draw a hint that still belongs to the caret's current line.
            if hint.line == line {
                let eol_idx = lsp_line_end_char_idx(&display_code, hint.line + 1);
                show_inlay_hint(
                    ui,
                    editor_resp.galley_pos,
                    editor_clip,
                    &editor_resp.galley,
                    eol_idx,
                    &hint.label,
                    self.editor_font_size,
                );
            }
        }
    }

    /// If the displayed file is NOT declared by its parent module (`mod x;`
    /// missing in the folder's `mod.rs`, or in `main.rs` for top-level files),
    /// return a hint naming the exact missing line. rust-analyzer detaches
    /// such files — no completions, no diagnostics — and every completion
    /// request in them answers `null`, which used to read as a popup that
    /// "appears and instantly disappears".
    fn unlinked_module_hint(&self) -> Option<String> {
        let ProjectFileId::UserFile(i) = self.selected_file else {
            return None; // main.rs (and config files) are always linked
        };
        let (name, _) = self.project_tree.user_src_files.get(i)?;
        let stem = name.rsplit('/').next()?.strip_suffix(".rs")?.to_owned();
        if stem == "mod" {
            return None; // a mod.rs is declared by ITS parent — keep it simple
        }
        let (parent_label, parent_text) = match name.rsplit_once('/') {
            Some((dir, _)) => {
                let parent_rel = format!("{dir}/mod.rs");
                let text = self
                    .project_tree
                    .user_src_files
                    .iter()
                    .find(|(n, _)| *n == parent_rel)?
                    .1
                    .as_str();
                (format!("src/{parent_rel}"), text)
            }
            None => ("src/main.rs".to_owned(), self.generated_code.as_str()),
        };
        (!mod_declared_in(parent_text, &stem)).then(|| {
            format!(
                "no suggestions — this file is not in the module tree: add \
                 `mod {stem};` to {parent_label} (rust-analyzer skips \
                 undeclared files entirely)"
            )
        })
    }
}

/// `true` when `parent_text` declares the child module `stem` — accepts
/// `mod x;`, `pub mod x;`, `pub(crate) mod x;` and `mod x {`; comment lines
/// don't count. Used by [`AppIde::unlinked_module_hint`].
fn mod_declared_in(parent_text: &str, stem: &str) -> bool {
    parent_text.lines().any(|l| {
        let l = l.trim();
        if l.starts_with("//") {
            return false;
        }
        let mut words = l.split_whitespace().peekable();
        while let Some(w) = words.next() {
            if w == "mod" {
                if let Some(&next) = words.peek() {
                    let ident = next.trim_end_matches([';', '{']).trim();
                    return ident == stem;
                }
            }
        }
        false
    })
}

/// Order completion items so those whose label starts with `prefix` (case-
/// insensitive) come first, keeping each group in the server's original order,
/// then the rest — so the popup leads with what the user has already typed.
/// An empty prefix returns the list unchanged (the server's relevance order).
fn order_by_prefix(items: Vec<lsp::CompletionItem>, prefix: &str) -> Vec<lsp::CompletionItem> {
    if prefix.is_empty() {
        return items;
    }
    let pl = prefix.to_lowercase();
    let (mut starts, rest): (Vec<_>, Vec<_>) = items
        .into_iter()
        .partition(|it| it.label.to_lowercase().starts_with(&pl));
    starts.extend(rest);
    starts
}

#[cfg(test)]
mod tests {
    use super::{mod_declared_in, order_by_prefix};
    use crate::lsp::CompletionItem;

    /// The unlinked-file detector: every accepted `mod` declaration shape
    /// counts, comments and other modules don't.
    #[test]
    fn mod_declaration_shapes_are_recognised() {
        let parent = "// New file\n\
                      pub mod data;\n\
                      mod radar;\n\
                      pub(crate) mod send_models;\n\
                      mod inline { }\n\
                      // mod commented_out;\n\
                      pub use radar::*;\n";
        assert!(mod_declared_in(parent, "data"));
        assert!(mod_declared_in(parent, "radar"));
        assert!(mod_declared_in(parent, "send_models"));
        assert!(mod_declared_in(parent, "inline"));
        assert!(!mod_declared_in(parent, "commented_out"));
        // The real bug that motivated this: the declaration simply missing.
        assert!(!mod_declared_in(parent, "read_report_admin"));
        // `use radar::*` alone must NOT count as declaring `radar`… checked
        // via a parent that only re-exports:
        assert!(!mod_declared_in("pub use radar::*;\n", "radar"));
    }

    fn items(labels: &[&str]) -> Vec<CompletionItem> {
        labels
            .iter()
            .map(|l| CompletionItem {
                label: (*l).to_string(),
                ..Default::default()
            })
            .collect()
    }

    fn labels(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn prefix_matches_lead_then_the_rest() {
        // RA's order mixes matches and non-matches; "n" items should bubble up
        // first (in their original order), the rest follow (in their order).
        let got = order_by_prefix(items(&["len", "new", "abs", "next", "map"]), "n");
        assert_eq!(labels(got), ["new", "next", "len", "abs", "map"]);
    }

    #[test]
    fn case_insensitive_and_empty_prefix_unchanged() {
        let got = order_by_prefix(items(&["Node", "abs", "new"]), "n");
        assert_eq!(labels(got), ["Node", "new", "abs"]);
        // Empty prefix preserves the server's order.
        let got = order_by_prefix(items(&["b", "a", "c"]), "");
        assert_eq!(labels(got), ["b", "a", "c"]);
    }
}
