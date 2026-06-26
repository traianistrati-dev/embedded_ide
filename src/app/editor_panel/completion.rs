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
        editor_clip: egui::Rect,
        mut display_code: String,
        lsp_accepted: Option<String>,
        ctrl_space_pressed: bool,
        copy_requested: bool,
        ctrl_r_pressed: bool,
        f12_pressed: bool,
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
                            lsp.did_change(rel, &display_code, false);
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

        // ── F12 go to definition: request; result shown in the Definition tab ──
        if f12_pressed && lsp_file_tracked {
            if let (Some(idx), Some(rel)) = (cursor_char_idx, current_rel_path.clone()) {
                let (line, col) = lsp_cursor_pos(&display_code, idx);
                let mut lsp = self.lsp_state.lock().unwrap();
                lsp.did_change(&rel, &display_code, false);
                lsp.request_definition(&rel, line, col);
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
                        // re-mapped per edit and stay visible.
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
                            .filter(|d| d.source == "rust-analyzer" || !flycheck_stale)
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
    }
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
    use super::order_by_prefix;
    use crate::lsp::CompletionItem;

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
