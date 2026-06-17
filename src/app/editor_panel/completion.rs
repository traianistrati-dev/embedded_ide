//! LSP completion driver + inline diagnostics, run after the editor widget.
//!
//! Consumes the editor's `TextEditOutput` (cursor + galley) and the text the
//! user just typed.  Applies an accepted completion, detects new triggers
//! (`.`, `::`, Ctrl+Space), renders the completion popup, and finally draws
//! the inline diagnostic overlays.

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::show_diagnostics_overlay;
use crate::editor::gui::text_pos::{
    diags_for_file, lsp_completion_prefix, lsp_cursor_pos, lsp_kind_icon, lsp_word_start,
    selected_file_rel_path,
};
use crate::lsp;
use eframe::egui;
use egui::text_edit::TextEditOutput;

/// Master switch for the **inline** diagnostic overlay — the squiggles and
/// message text drawn over the code in the editor.
///
/// Only **errors and info** are drawn inline; warnings (and hints) are filtered
/// out at the call site below and remain in the bottom panel (Cargo Check /
/// rust-analyzer tabs) to keep the code view uncluttered. Diagnostics refresh on
/// the LSP debounce (3 s after typing stops, or on Project Save — see
/// `app::init_frame`), so their positions no longer lag behind active typing.
const SHOW_INLINE_DIAGNOSTICS: bool = true;

impl AppIde {
    /// Apply/trigger LSP completion and draw diagnostics, after the editor.
    ///
    /// `display_code` is the current editor text (already written back); it is
    /// taken by value because nothing after this stage reads it again.
    pub(super) fn handle_editor_completion(
        &mut self,
        ui: &mut egui::Ui,
        editor_resp: &TextEditOutput,
        mut display_code: String,
        lsp_accepted: Option<String>,
        ctrl_space_pressed: bool,
    ) {
        // ── LSP completion: post-editor apply + trigger + popup ───────
        let cursor_char_idx = editor_resp
            .state
            .cursor
            .char_range()
            .map(|r| r.primary.index);

        // Apply accepted completion: replace [word_start..cursor] with insert_text
        if let Some(insert_text) = lsp_accepted {
            if let Some(cur_idx) = cursor_char_idx {
                let word_start = lsp_word_start(&display_code, cur_idx);
                let chars: Vec<char> = display_code.chars().collect();
                let before: String = chars[..word_start].iter().collect();
                let after: String = chars[cur_idx..].iter().collect();
                display_code = format!("{}{}{}", before, insert_text, after);
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
                            lsp.did_change(rel, &display_code);
                            lsp.request_completion(rel, line, col, None);
                        }
                        self.completion_trigger_idx = idx;
                        self.completion_sel = 0;
                        self.completion_open = true;
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
                            lsp.did_change(rel, &display_code);
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
                            lsp.did_change(rel, &display_code);
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

        // ── LSP completion popup ───────────────────────────────────────
        if self.completion_open {
            let all_items = self.lsp_state.lock().unwrap().completion_items.clone();

            if !all_items.is_empty() {
                // ── Live prefix filtering ────────────────────────────────
                // Compute what the user has typed since the trigger point.
                let prefix = cursor_char_idx
                    .map(|cur| {
                        lsp_completion_prefix(&display_code, self.completion_trigger_idx, cur)
                    })
                    .unwrap_or_default();

                let filtered: Vec<lsp::CompletionItem> = if prefix.is_empty() {
                    all_items
                } else {
                    let pl = prefix.to_lowercase();
                    all_items
                        .into_iter()
                        .filter(|it| it.label.to_lowercase().starts_with(&pl))
                        .collect()
                };

                // Persist filtered list so next frame's key handlers see
                // exactly the same items the user sees right now.
                self.completion_filtered_items = filtered.clone();

                if filtered.is_empty() {
                    // Nothing matches the current prefix — hide the popup.
                    self.completion_open = false;
                } else {
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
                                                    Some(item.insert_text.clone());
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
                    // RA answered (empty) or request is stale — close popup.
                    self.completion_open = false;
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

        // ── Diagnostic overlays ───────────────────────────────────────
        if lsp_file_tracked && SHOW_INLINE_DIAGNOSTICS {
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
                    // Show only when (a) RA holds the current text for this file
                    // AND (b) RA has re-published diagnostics since that text was
                    // sent — otherwise the diagnostics are stale (an old error
                    // would linger at a position whose line was edited away).
                    if lsp.last_sent_matches(rel, &display_code) && lsp.diagnostics_fresh(rel) {
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
                            .collect()
                    } else {
                        Vec::new()
                    }
                })
                .unwrap_or_default();

            show_diagnostics_overlay(
                ui,
                editor_resp.galley_pos,
                editor_resp.text_clip_rect,
                &editor_resp.galley,
                &diags,
                &display_code,
            );
        }
    }
}
