//! Live "usages" analysis for the currently displayed `.rs` file: every fn/
//! struct/enum/const/static/trait/method/field/… is checked against rust-
//! analyzer's `references` so never-used items can be faded in the editor and
//! used items get a small "N refs" indicator that pops up their call sites.
//!
//! Driven entirely by rust-analyzer's own in-memory analysis (`documentSymbol` +
//! `references`) — NOT Cargo Check/Clippy's `dead_code` lint, which needs a full
//! compile and only runs on demand (see the Clippy tab). `references` is a much
//! cheaper, purely semantic query RA already answers live, so this stays fresh
//! without a Build/Clippy run — confirmed by testing `rust-analyzer diagnostics`
//! directly: RA's own diagnostic pass does NOT report `dead_code`, but
//! `references` is a separate, simpler query it has always supported (it backs
//! Ctrl+R rename and F12 already in this codebase).
//!
//! Recomputed (debounced ~1.2 s idle) whenever the displayed file's text settles,
//! and immediately on switching files. While the live text differs from the
//! snapshot `items` were computed against, nothing is drawn — the same "hide
//! until fresh" rule the inline diagnostic overlay already uses, so an edit
//! never leaves a fade/pill at a stale (now wrong) position.

use crate::app::AppIde;
use crate::editor::gui::text_pos::{lsp_line_end_char_idx, lsp_pos_to_char_idx};
use crate::lsp::LspStatus;
use eframe::egui;
use std::time::{Duration, Instant};

/// How long the displayed file's text must sit unchanged before a fresh
/// documentSymbol/references pass is (re)requested. Raised from 1.2s: each
/// pass costs one whole-crate find-all-references per symbol (serialized, see
/// `pump_references`), so re-running it on every brief typing pause piled load
/// onto rust-analyzer.
const DEBOUNCE: Duration = Duration::from_millis(2500);

/// Key-space stride separating reference-request generations: the request key
/// is `refs_run * STRIDE + item_index`, so a late response from a superseded
/// run can be recognised and discarded instead of landing on the wrong item.
const REFS_RUN_STRIDE: usize = 100_000;

/// One usage site: an absolute filesystem path + 0-based LSP position.
#[derive(Clone, Debug)]
struct UsageRef {
    path: String,
    line: u32,
}

/// One tracked item (fn/struct/enum/const/…) in the displayed file.
#[derive(Clone, Debug)]
struct UsageItem {
    name: String,
    /// LSP `SymbolKind` — part of the identity key for carrying a previous
    /// run's references over to the new run (instant fade/pill continuity).
    kind: u8,
    // Whole-item span (0-based LSP) — the "fade" range when unused.
    start_line: u32,
    start_char: u32,
    end_line: u32,
    end_char: u32,
    // Name position — anchors the "N refs" indicator and is the reference
    // query point.
    sel_line: u32,
    sel_char: u32,
    /// `None` while the reference lookup for this item hasn't resolved yet
    /// (nothing is drawn for it until it does).
    references: Option<Vec<UsageRef>>,
}

#[derive(Default)]
pub struct UsagesState {
    /// The file (`"src/main.rs"` / `"src/foo.rs"`) `items` are for.
    rel_path: String,
    items: Vec<UsageItem>,
    /// Exact text snapshot `items`' positions match. Empty = nothing computed
    /// yet. The instant the live text no longer matches this, the overlay
    /// draws nothing until a fresh pass lands (positions would otherwise point
    /// at the wrong code after an edit).
    computed_for_text: String,
    /// The text a documentSymbol request is currently in flight for, so a
    /// still-unchanged buffer isn't re-requested every frame while waiting.
    pending_text: Option<String>,
    /// The text seen on the PREVIOUS tick — compared against the current tick's
    /// text to detect an actual edit. (Comparing against `computed_for_text` /
    /// `pending_text` instead would be wrong: those stay "old" for the whole
    /// debounce+request+response window, so every frame before the first fetch
    /// completes would look like "just changed" and the timer would never
    /// elapse — which is exactly what happened before this field existed.)
    last_seen_text: String,
    /// Debounce timer, reset whenever the live text changes since the last tick.
    last_change_at: Option<Instant>,
    /// Index into `items` whose "references" popup is open, if any.
    open_popup: Option<usize>,
    /// Item indices whose `references` still need fetching, drained ONE AT A
    /// TIME by [`pump_references`]. Firing all of them at once (the old
    /// behaviour) meant N concurrent whole-crate find-all-references queries
    /// per settle (~40 for a big file) — rust-analyzer's queue backed up more
    /// and more over a session, degrading everything LSP-based.
    refs_queue: std::collections::VecDeque<usize>,
    /// Generation counter for reference requests (see [`REFS_RUN_STRIDE`]).
    refs_run: u64,
    /// `true` while one reference request is in flight (next is sent on reply).
    refs_inflight: bool,
}

/// Send the next queued reference lookup, if none is in flight — one at a time
/// so rust-analyzer is never flooded. Free function (not a method) so callers
/// can hold the `lsp_state` lock and borrow `usages` mutably at the same time.
fn pump_references(usages: &mut UsagesState, lsp: &mut crate::lsp::LspState) {
    if usages.refs_inflight {
        return;
    }
    while let Some(idx) = usages.refs_queue.pop_front() {
        if let Some(item) = usages.items.get(idx) {
            let key = usages.refs_run as usize * REFS_RUN_STRIDE + idx;
            lsp.request_references(&usages.rel_path, item.sel_line, item.sel_char, key);
            usages.refs_inflight = true;
            return;
        }
    }
}

/// Naming-convention / attribute markers of items invoked from OUTSIDE the
/// crate's own source (the embedded runtime, an interrupt vector, the linker,
/// the test harness, …) rather than by a normal Rust call. `references`
/// legitimately comes back empty for these, so they must never be treated as
/// dead code even though nothing in the file calls them by name.
const EXTERNALLY_INVOKED_MARKERS: [&str; 8] = [
    "#[entry]",         // cortex-m-rt binary entry point
    "#[interrupt",      // cortex-m-rt / PAC interrupt handler
    "#[exception",      // cortex-m-rt CPU exception handler
    "#[no_mangle]",     // called by its C-ABI symbol name, not a Rust call
    "#[export_name",    // same, custom exported symbol name
    "#[test]",          // invoked by the test harness
    "#[panic_handler]", // invoked by the runtime on panic
    "#[global_allocator]",
];

/// `true` when the item named `name`, starting at 0-based line `start_line` in
/// `text`, is externally invoked (see [`EXTERNALLY_INVOKED_MARKERS`]) — either
/// `fn main` itself (the `#[entry]`-annotated binary entry point in this
/// `no_std` project; excluded by name too since a plain `fn main` is never
/// called from source even without the attribute) or preceded by one of the
/// marker attributes. Scans upward from the item's own line (inclusive, since
/// rust-analyzer's `documentSymbol` range sometimes starts at a leading
/// attribute and sometimes at the `fn`/`pub` keyword — checking both is cheap),
/// skipping blank lines, `//` doc comments, and other attributes, stopping at
/// the first real code line above the item's own.
fn is_externally_invoked(name: &str, start_line: u32, text: &str) -> bool {
    if name == "main" {
        return true;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut i = (start_line as usize + 1).min(lines.len());
    while i > 0 {
        i -= 1;
        let l = lines.get(i).map(|s| s.trim()).unwrap_or("");
        if l.is_empty() || l.starts_with("//") {
            continue;
        }
        if l.starts_with("#[") {
            if EXTERNALLY_INVOKED_MARKERS.iter().any(|m| l.contains(m)) {
                return true;
            }
            continue;
        }
        // Real code: on the item's own line (first iteration) this is just its
        // signature — keep scanning above for attributes. Once we've moved
        // above that line, real code means we've run out of attributes.
        if i < start_line as usize {
            break;
        }
    }
    false
}

/// The fade span for an `unused_variables` diagnostic, self-verified against
/// the CURRENT text: `None` unless the identifier named in the diagnostic's
/// message (rustc's `"unused variable: \`x\`"` format) still appears at its
/// reported position. Guards against a Build/Clippy result gone stale after a
/// later edit shifted the code — the diagnostic just silently stops applying
/// rather than fading the wrong span.
fn unused_variable_range(d: &crate::build::Diagnostic, display_code: &str) -> Option<(usize, usize)> {
    let name = d.message.split('`').nth(1)?;
    if name.is_empty() {
        return None;
    }
    let start = lsp_pos_to_char_idx(display_code, d.line?, d.col?);
    let chars: Vec<char> = display_code.chars().collect();
    let end = (start + name.chars().count()).min(chars.len());
    if end <= start || chars[start..end].iter().collect::<String>() != name {
        return None; // stale — the code at that position no longer matches
    }
    Some((start, end))
}

impl AppIde {
    /// Snapshot every `.rs` file's CURRENT content into `build_text_snapshot`
    /// (`"src/main.rs"` / `"src/{rel}"` → text) — call right alongside
    /// `write_project` whenever a Cargo Check / Clippy run is kicked off, so
    /// the snapshot always matches exactly what got compiled. Lets
    /// `usages_dead_ranges` tell whether that run's `unused_variables`
    /// diagnostics still apply to the live text (exact match) or have gone
    /// stale from a later edit (any edit anywhere in the file, not just at the
    /// diagnostic's own position — e.g. adding a use of the variable elsewhere
    /// doesn't move the `let`, but must still invalidate the "unused" fade).
    pub(super) fn snapshot_build_text(&mut self) {
        self.build_text_snapshot.clear();
        self.build_text_snapshot
            .insert("src/main.rs".to_owned(), self.generated_code.clone());
        for (rel, content) in &self.project_tree.user_src_files {
            self.build_text_snapshot
                .insert(format!("src/{rel}"), content.clone());
        }
    }

    /// Advance the usages pipeline for the displayed `.rs` file: reset on a
    /// file switch, poll any in-flight LSP responses, and — once the text has
    /// settled and RA is `Ready` — (re)issue `documentSymbol` for it. Call once
    /// per frame, before rendering the editor (so `usages_dead_ranges` below
    /// reflects this frame's poll).
    pub(super) fn tick_usages(&mut self, rel_path: &str, text: &str) {
        if self.usages.rel_path != rel_path {
            self.usages = UsagesState {
                rel_path: rel_path.to_owned(),
                ..Default::default()
            };
        }
        self.poll_usages();

        if self.usages.last_seen_text != text {
            self.usages.last_seen_text = text.to_owned();
            self.usages.last_change_at = Some(Instant::now());
        }

        let settled = self
            .usages
            .last_change_at
            .is_none_or(|t| t.elapsed() > DEBOUNCE);
        let stale = self.usages.computed_for_text != text;
        let already_pending = self.usages.pending_text.as_deref() == Some(text);
        if stale && settled && !already_pending {
            let ready = matches!(self.lsp_state.lock().unwrap().status, LspStatus::Ready);
            if ready {
                self.usages.pending_text = Some(text.to_owned());
                self.lsp_state
                    .lock()
                    .unwrap()
                    .request_document_symbols(rel_path);
            }
        }
    }

    /// Drain completed `documentSymbol` / `references` LSP responses into
    /// `self.usages`. A fresh symbol list immediately fires one `references`
    /// request per item (concurrent — keyed by its index in the new list).
    fn poll_usages(&mut self) {
        let symbols = {
            let mut lsp = self.lsp_state.lock().unwrap();
            lsp.take_document_symbols_result()
        };
        if let Some((file, syms)) = symbols {
            if file == self.usages.rel_path {
                let text = self.usages.pending_text.take().unwrap_or_default();
                // Carry the previous run's resolved references over by (name,
                // kind) so the fade/pill stays continuous while the serialized
                // refresh below re-verifies each item one by one.
                let cache: std::collections::HashMap<(String, u8), Vec<UsageRef>> =
                    std::mem::take(&mut self.usages.items)
                        .into_iter()
                        .filter_map(|it| Some(((it.name, it.kind), it.references?)))
                        .collect();
                self.usages.items = syms
                    .into_iter()
                    // Entry points / externally-invoked items (`fn main`, an
                    // `#[interrupt]` handler, …) are never called from the
                    // crate's own source — `references` legitimately comes back
                    // empty for them, so they must be excluded here rather than
                    // shown as "dead code".
                    .filter(|s| !is_externally_invoked(&s.name, s.start_line, &text))
                    .map(|s| UsageItem {
                        references: cache.get(&(s.name.clone(), s.kind)).cloned(),
                        name: s.name,
                        kind: s.kind,
                        start_line: s.start_line,
                        start_char: s.start_char,
                        end_line: s.end_line,
                        end_char: s.end_char,
                        sel_line: s.sel_line,
                        sel_char: s.sel_char,
                    })
                    .collect();
                self.usages.computed_for_text = text;
                self.usages.open_popup = None;
                // Refresh every item's references — SERIALIZED (one in-flight
                // request; the next goes out when the reply lands), never the
                // old fire-all-at-once flood. A fresh run supersedes any
                // still-queued indices from the previous one.
                self.usages.refs_run += 1;
                self.usages.refs_queue = (0..self.usages.items.len()).collect();
                self.usages.refs_inflight = false;
                let mut lsp = self.lsp_state.lock().unwrap();
                pump_references(&mut self.usages, &mut lsp);
            }
            // A response for a file we've since navigated away from — drop it;
            // `take_document_symbols_result` already removed it from `lsp_state`.
        }

        let refs = {
            let mut lsp = self.lsp_state.lock().unwrap();
            lsp.take_reference_results()
        };
        if !refs.is_empty() {
            for (key, locs) in refs {
                // Any reply frees the in-flight slot; only replies from the
                // CURRENT run are applied (a superseded run's key would land on
                // the wrong item).
                self.usages.refs_inflight = false;
                if key / REFS_RUN_STRIDE == self.usages.refs_run as usize {
                    let idx = key % REFS_RUN_STRIDE;
                    if let Some(item) = self.usages.items.get_mut(idx) {
                        item.references = Some(
                            locs.into_iter()
                                .map(|r| UsageRef { path: r.path, line: r.line })
                                .collect(),
                        );
                    }
                }
            }
            let mut lsp = self.lsp_state.lock().unwrap();
            pump_references(&mut self.usages, &mut lsp);
        }
    }

    /// Char-index `[start, end)` ranges to dim in the currently displayed file:
    /// never-referenced items (fn/struct/enum/const/…, from the live RA
    /// `references` analysis above) PLUS never-read local `let` bindings.
    ///
    /// Local variables aren't file-level items, so `documentSymbol` never sees
    /// them, and — confirmed by probing `rust-analyzer` directly with this
    /// app's own `checkOnSave: false` setting — RA's native analysis never
    /// reports `unused_variables` without a real compile (`source` comes back
    /// `"rustc"`, i.e. flycheck-only; two live probes returned zero diagnostics
    /// with checkOnSave off). So, for JUST this lint, we fall back to whatever
    /// the last Cargo Check / Clippy run found (same source the Clippy tab
    /// already shows it in) — gated on `build_text_snapshot`: that run's
    /// diagnostics are used ONLY while the file's live text still matches
    /// EXACTLY what was compiled (an edit anywhere in the file — not just at
    /// the diagnostic's own position — must invalidate them: adding a use of
    /// the variable elsewhere doesn't move the `let`, but clears the "unused"
    /// verdict all the same). The per-diagnostic identifier re-check below is
    /// extra insurance on top of that, not a substitute for it.
    pub(super) fn usages_dead_ranges(&self, rel_path: &str, display_code: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();

        if self.usages.rel_path == rel_path && self.usages.computed_for_text == display_code {
            ranges.extend(self.usages.items.iter().filter_map(|item| {
                let refs = item.references.as_ref()?;
                if !refs.is_empty() {
                    return None;
                }
                let start =
                    lsp_pos_to_char_idx(display_code, item.start_line + 1, item.start_char + 1);
                let end = lsp_pos_to_char_idx(display_code, item.end_line + 1, item.end_char + 1);
                Some((start, end.max(start)))
            }));
        }

        if self.build_text_snapshot.get(rel_path).map(String::as_str) == Some(display_code) {
            let results = [&self.build_state, &self.clippy_state]
                .into_iter()
                .filter_map(|s| match &*s.lock().unwrap() {
                    crate::build::BuildState::Done(r) => Some(r.clone()),
                    _ => None,
                });
            for result in results {
                ranges.extend(
                    result
                        .diagnostics
                        .iter()
                        .filter(|d| {
                            d.file.as_deref() == Some(rel_path)
                                && d.code.as_deref() == Some("unused_variables")
                        })
                        .filter_map(|d| unused_variable_range(d, display_code)),
                );
            }
        }

        ranges
    }

    /// Paint the "N refs" indicator on every used item's signature line and
    /// handle its click (open/close a floating popup listing call sites, itself
    /// clickable to navigate there). A no-op unless the usages analysis is
    /// fresh for the exact `display_code` passed in (same guard as
    /// `usages_dead_ranges` — keeps the two in lock-step).
    pub(super) fn show_usages_overlay(
        &mut self,
        ui: &egui::Ui,
        galley_pos: egui::Pos2,
        clip: egui::Rect,
        galley: &egui::text::Galley,
        display_code: &str,
        rel_path: &str,
    ) {
        if self.usages.rel_path != rel_path || self.usages.computed_for_text != display_code {
            return;
        }

        const PILL_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(40, 58, 76, 220);
        const PILL_FG: egui::Color32 = egui::Color32::from_rgb(150, 195, 235);

        // Pass 1 (read-only borrow of `self.usages.items`): paint every pill and
        // remember its rect + click state. Nothing here mutates `self` — the
        // popup (which navigates on click) is handled in pass 2, after this
        // borrow ends.
        let mut clicked: Option<usize> = None;
        let mut pill_rects: std::collections::HashMap<usize, egui::Rect> = Default::default();
        {
            let total_chars = display_code.chars().count();
            let painter = ui.painter().with_clip_rect(clip);
            let gp = galley_pos;
            for (i, item) in self.usages.items.iter().enumerate() {
                let Some(refs) = &item.references else {
                    continue; // still resolving
                };
                if refs.is_empty() {
                    continue; // unused → faded by the highlighter, no pill
                }

                let sel_ci =
                    lsp_pos_to_char_idx(display_code, item.sel_line + 1, item.sel_char + 1)
                        .min(total_chars);
                let eol_ci =
                    lsp_line_end_char_idx(display_code, item.sel_line + 1).min(total_chars);
                let loc_sel = galley.pos_from_cursor(egui::text::CCursor::new(sel_ci));
                let loc_eol = galley.pos_from_cursor(egui::text::CCursor::new(eol_ci));
                let y_top = gp.y + loc_sel.min.y;
                let y_bot = gp.y + loc_sel.max.y;
                if y_bot < clip.top() || y_top > clip.bottom() {
                    continue; // scrolled out of view
                }

                let label = format!("{} {}", refs.len(), if refs.len() == 1 { "ref" } else { "refs" });
                let font = egui::FontId::proportional(10.0);
                let galley_txt = painter.layout_no_wrap(label.clone(), font.clone(), PILL_FG);
                let pad = 3.0;
                let x0 = gp.x + loc_eol.min.x + 14.0;
                let h = (y_bot - y_top).max(galley_txt.size().y + pad);
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x0, y_top + (y_bot - y_top - h).max(0.0) * 0.5),
                    egui::vec2(galley_txt.size().x + pad * 2.0, h),
                );
                painter.rect_filled(rect, 3.0, PILL_BG);
                painter.text(
                    rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    font,
                    PILL_FG,
                );
                pill_rects.insert(i, rect);

                let resp = ui.interact(
                    rect,
                    egui::Id::new("usages_pill").with(rel_path).with(i),
                    egui::Sense::click(),
                );
                if resp.clicked() {
                    clicked = Some(i);
                }
                resp.on_hover_text(format!("`{}` — {label}, click to see call sites", item.name));
            }
        }

        if let Some(i) = clicked {
            self.usages.open_popup = if self.usages.open_popup == Some(i) {
                None
            } else {
                Some(i)
            };
        }

        // Pass 2: the floating popup (needs `&mut self` to navigate on click),
        // now that pass 1's borrow of `self.usages.items` has ended.
        if let Some(idx) = self.usages.open_popup {
            match pill_rects.get(&idx) {
                Some(&anchor) => self.show_usage_popup(ui, anchor, idx),
                None => self.usages.open_popup = None, // pill scrolled off-screen
            }
        }
    }

    /// The floating "References to `name`" popup anchored below a pill; each
    /// row navigates to that call site (opens the file, scrolls, highlights).
    fn show_usage_popup(&mut self, ui: &egui::Ui, anchor: egui::Rect, idx: usize) {
        let Some(item) = self.usages.items.get(idx) else {
            self.usages.open_popup = None;
            return;
        };
        let name = item.name.clone();
        let refs = item.references.clone().unwrap_or_default();

        let mut close = false;
        let mut nav_to: Option<(String, u32)> = None;

        egui::Area::new(egui::Id::new("usages_popup"))
            .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 4.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.set_min_width(240.0);
                    ui.label(
                        egui::RichText::new(format!("References to `{name}`"))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 180, 200)),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            for r in &refs {
                                let short = crate::app::short_path(&r.path);
                                let label = format!("{short}:{}", r.line + 1);
                                let resp = ui.add(
                                    egui::Label::new(egui::RichText::new(label).size(11.0))
                                        .sense(egui::Sense::click()),
                                );
                                if resp.clicked() {
                                    nav_to = Some((r.path.clone(), r.line));
                                    close = true;
                                }
                            }
                        });
                    ui.add_space(2.0);
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Close").size(10.0),
                        ))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        if let Some((path, line)) = nav_to {
            if let Some(id) = crate::app::project_file_for_def(&path, &self.project_tree.user_src_files) {
                self.selected_file = id;
                self.pending_scroll_to_line = Some((id, line as usize + 1));
                self.highlighted_def_line = Some((id, line as usize + 1));
            }
        }
        if close {
            self.usages.open_popup = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_externally_invoked, unused_variable_range};

    fn diag(line: u32, col: u32, message: &str) -> crate::build::Diagnostic {
        crate::build::Diagnostic {
            level: "warning".into(),
            message: message.into(),
            rendered: String::new(),
            file: Some("src/main.rs".into()),
            line: Some(line),
            col: Some(col),
            code: Some("unused_variables".into()),
            fixes: Vec::new(),
            rename: None,
        }
    }

    #[test]
    fn unused_variable_range_matches_live_position() {
        let text = "fn main() {\n    let x = 100;\n}\n";
        // rustc reports 1-based line 2, col 9 (the `x`).
        let d = diag(2, 9, "unused variable: `x`");
        let (start, end) = unused_variable_range(&d, text).expect("position still matches");
        let got: String = text.chars().skip(start).take(end - start).collect();
        assert_eq!(got, "x");
    }

    #[test]
    fn unused_variable_range_rejects_stale_position() {
        // The diagnostic was computed for `x`, but the live text at that
        // position now reads something else — must not fade the wrong span.
        let text = "fn main() {\n    let y = 100;\n}\n";
        let d = diag(2, 9, "unused variable: `x`");
        assert!(unused_variable_range(&d, text).is_none());
    }

    #[test]
    fn unused_variable_range_needs_a_quoted_name() {
        let d = diag(1, 1, "no backticks here");
        assert!(unused_variable_range(&d, "let x = 1;").is_none());
    }

    #[test]
    fn plain_fn_main_is_excluded_by_name_alone() {
        // No attribute at all — `fn main` is still never called from source.
        let text = "fn main() {\n    loop {}\n}\n";
        assert!(is_externally_invoked("main", 0, text));
    }

    #[test]
    fn entry_annotated_main_is_excluded() {
        let text = "#[entry]\nfn main() -> ! {\n    loop {}\n}\n";
        assert!(is_externally_invoked("main", 1, text));
    }

    #[test]
    fn interrupt_handler_is_excluded() {
        let text = "#[interrupt]\nfn USART1() {\n    // handle it\n}\n";
        assert!(is_externally_invoked("USART1", 1, text));
    }

    #[test]
    fn stacked_attributes_still_find_the_marker() {
        // #[allow(...)] sits between the real marker and the fn line.
        let text = "#[interrupt]\n#[allow(non_snake_case)]\nfn TIM2() {}\n";
        assert!(is_externally_invoked("TIM2", 2, text));
    }

    #[test]
    fn ordinary_function_is_not_excluded() {
        let text = "pub fn helper() -> u8 {\n    42\n}\n";
        assert!(!is_externally_invoked("helper", 0, text));
    }

    #[test]
    fn unrelated_attribute_does_not_exclude() {
        // #[allow(dead_code)] is not in the marker list — a genuinely unused
        // fn wearing it must still be eligible for fading.
        let text = "#[allow(dead_code)]\nfn unused_helper() {}\n";
        assert!(!is_externally_invoked("unused_helper", 1, text));
    }

    #[test]
    fn doc_comment_between_attribute_and_fn_is_skipped() {
        let text = "#[no_mangle]\n/// Doc comment.\npub extern \"C\" fn ffi_thing() {}\n";
        assert!(is_externally_invoked("ffi_thing", 2, text));
    }
}
