//! The state of ONE code editor view.
//!
//! Everything here is a property of a VIEW, not of a file: which popup is open,
//! where the caret was, what the find bar holds. `AppIde` carries two of them —
//! the main editor's and the Reference editor's — and swaps them around the
//! second editor's frame (see [`AppIde::with_editor`]).
//!
//! That swap is what makes a second full-featured editor affordable. The ~470
//! places that read this state all mean "the editor being drawn right now", and
//! the two views never run at the same time: the main panel renders first, the
//! Reference tab later inside the MCU panel. So one swap point replaces ~470
//! individual decisions about which view is meant.
//!
//! What is NOT here is as deliberate as what is. State keyed by FILE stays on
//! `AppIde` and stays shared — breakpoints, folds, the git diff cache, the LSP
//! connection. A breakpoint is a property of the file, not of the window
//! looking at it, and splitting those would break the case that already works:
//! the same file's marks agreeing in both views.

use crate::app::{PinHighlight, ProjectFileId, editor_panel, lsp};
use eframe::egui;
use egui_code_editor::{Completer, Syntax};

#[derive(Default)]
pub(crate) struct EditorState {
    /// The code editor's egui widget id, captured after each render. Needed a
    /// frame LATER, and before the widget exists: a folded editor is
    /// non-interactive and therefore unfocused, so the keystroke that unfolds it
    /// must also hand focus back — otherwise the file stays untypable until the
    /// user clicks into it.
    pub(crate) editor_widget_id: Option<egui::Id>,

    /// Set by a fold toggle: `(rel path, the block's header line, the screen y
    /// it had BEFORE the toggle)`. The next frame re-anchors the scroll offset
    /// so that line stays exactly where it was — folding 200 lines otherwise
    /// slides the whole page under the pointer.
    pub(crate) fold_anchor: Option<(String, usize, f32)>,

    /// Code-completion engine — stores the trie, current prefix and popup state.
    /// Must live in the App (not a local) so state is preserved across frames.
    pub(crate) completer: Completer,

    /// True when the LSP completion popup is visible.
    pub(crate) completion_open: bool,

    /// Transient note shown at the cursor when a completion request came back
    /// EMPTY — a silent popup flash was undiagnosable. Carries the reason
    /// (e.g. "the file has no `mod …;` declaration") + when it appeared.
    pub(crate) completion_note: Option<(String, std::time::Instant)>,

    /// Index of the currently highlighted row in the completion popup.
    pub(crate) completion_sel: usize,

    /// Character-offset in the editor text where completion was triggered.
    /// Used to compute the live prefix for filtering and to close the popup
    /// when the cursor moves away.
    pub(crate) completion_trigger_idx: usize,

    /// Completion item deferred from a mouse-click on a popup row.
    /// Applied at the start of the next frame (before the editor renders);
    /// carries the whole item so snippet expansion sees `insert_is_snippet`.
    pub(crate) completion_pending_insert: Option<lsp::CompletionItem>,

    /// Filtered completion list from the last rendered frame.
    /// Key handlers (Tab / Enter / Arrow) use this so they always operate
    /// on the same slice the user sees, not the full unfiltered LSP list.
    pub(crate) completion_filtered_items: Vec<lsp::CompletionItem>,

    /// Cargo.toml dependency-completion popup (crate names + live crates.io
    /// versions). Independent of rust-analyzer.
    pub(crate) cargo_complete: editor_panel::cargo_complete::CargoCompleteState,

    /// Primary caret char-index from the previous frame, used to scroll the
    /// editor so the caret stays in view when it moves off-screen (e.g.
    /// Shift+Up/Down selection past the visible area).
    pub(crate) last_caret_idx: Option<usize>,

    /// Pending "jump to this diagnostic": the target file and its 1-based line.
    /// Set when a row in the Cargo Check / rust-analyzer tab is clicked; applied
    /// once the editor is displaying that file (scrolls the line to row ~10).
    pub(crate) pending_scroll_to_line: Option<(ProjectFileId, usize)>,

    /// The file + 1-based line + band colour of the last-clicked diagnostic.
    /// Highlighted with a translucent band (colour keyed by severity, see
    /// `diag_highlight_color`) in the editor until another diagnostic is clicked.
    pub(crate) highlighted_error_line: Option<(ProjectFileId, usize, egui::Color32)>,

    /// The file + 1-based line of the last F12 go-to-definition that landed in a
    /// project file. Highlighted with a translucent yellow band (like the
    /// Definition tab) until the next F12.
    pub(crate) highlighted_def_line: Option<(ProjectFileId, usize)>,

    /// The pulsing "here is your pin" highlight, or `None` when none is running.
    pub(crate) highlighted_pin_lines: Option<PinHighlight>,

    /// Live "usages" analysis (fn/struct/enum/const/… fade-if-unused + a
    /// "references" popup) for whichever `.rs` file is currently displayed. See
    /// `editor_panel::usages`.
    pub(crate) usages: editor_panel::usages::UsagesState,

    /// Extra caret positions for Ctrl+Shift+Up/Down multi-cursor editing (char
    /// indices into the displayed file, in the order they were added — last
    /// added is popped first by Ctrl+Shift+Down). See `editor_panel::multi_cursor`.
    pub(crate) extra_cursors: Vec<editor_panel::multi_cursor::ExtraCaret>,

    /// Which file `extra_cursors` belongs to — cleared on a file switch so
    /// stale positions never leak into an unrelated file.
    pub(crate) extra_cursors_file: Option<ProjectFileId>,

    /// The primary caret's char index at the end of the previous frame — lets
    /// multi-cursor replay tell a Backspace (deletes BEFORE the cursor) apart
    /// from a Delete-key press (deletes AFTER it) — and, since it stores the
    /// whole `(anchor, head)` selection, typing OVER a selection apart from
    /// either, because then each caret replaces its OWN span.
    pub(crate) mc_prev_primary_sel: Option<(usize, usize)>,

    /// Did the code editor hold keyboard focus last frame? egui surrenders the
    /// focused widget on Escape before any of our code runs, so this is the
    /// only way to know whether the caret that just vanished was OURS — and
    /// therefore whether to take the focus back.
    pub(crate) editor_was_focused: bool,

    // ── Rename symbol (Ctrl+R → textDocument/rename) ─────────────────────────
    /// While `true`, the rename input popup is shown.
    pub(crate) rename_active: bool,

    /// The new name being typed in the rename popup (pre-filled with the symbol).
    pub(crate) rename_input: String,

    /// The symbol's name BEFORE the rename, captured when the popup opens, so
    /// the applied edits can be audited for occurrences RA did not reach.
    pub(crate) rename_old_name: String,

    /// The name submitted in the rename popup, so leftovers can be offered the
    /// same target.
    pub(crate) rename_new_name: String,

    /// File + 0-based (line, char) where the rename was triggered.
    pub(crate) rename_rel: String,

    pub(crate) rename_line: u32,

    pub(crate) rename_char: u32,

    /// Screen position to anchor the rename popup at.
    pub(crate) rename_popup_pos: egui::Pos2,

    /// `true` after a rename request was sent, until RA's edits are applied.
    pub(crate) rename_in_flight: bool,

    // ── Code actions (Ctrl+Enter — RA assists / quick-fixes) ─────────────────
    /// `true` after a codeAction request, until the list arrives.
    pub(crate) code_action_in_flight: bool,

    /// `true` after a codeAction/resolve request, until its edits arrive.
    pub(crate) code_action_resolve_in_flight: bool,

    /// The code actions to choose from (popup shown when > 1).
    pub(crate) code_actions: Vec<lsp::CodeAction>,

    /// Whether the chooser popup is open.
    pub(crate) code_action_popup_open: bool,

    /// Highlighted row in the chooser popup.
    pub(crate) code_action_sel: usize,

    /// Screen anchor for the chooser popup (the cursor rect when triggered).
    pub(crate) code_action_popup_pos: egui::Pos2,

    /// Chooser selection deferred to next frame's `init_frame` (so the edit
    /// applies at frame TOP, avoiding the display_code write-back revert).
    pub(crate) code_action_choice: Option<usize>,

    /// The crate identifier the "Add dependency" row offers, for the caret the
    /// last Ctrl+Enter was fired on. `Some` puts an extra row at the TOP of the
    /// code-action list — rust-analyzer never produces it, because it does not
    /// know Cargo.toml exists.
    pub(crate) code_action_add_dep: Option<String>,

    /// The crate chooser that row opens.
    pub(crate) add_dep: editor_panel::add_dep::AddDepState,

    /// Ctrl+Alt+Insert: the "move these lines into a new function" popup.
    pub(crate) extract: editor_panel::extract_fn::ExtractFnState,

    /// The inferred-type hint to draw as ghost text after an untyped `let` on
    /// the cursor's line, if any (its `text_edits` insert the type on Tab).
    /// Cleared when the caret leaves an untyped `let`.
    pub(crate) inlay_hint: Option<lsp::InlayHint>,

    /// `(rel_path, 0-based line)` the last inlay request was fired for — so we
    /// re-request when the caret moves to a different `let` line, or after RA
    /// re-syncs (the request key is reset while the file is dirty).
    pub(crate) inlay_requested: Option<(String, u32)>,

    /// Set when Tab is pressed while the ghost hint shows; the type is inserted
    /// at frame TOP next `init_frame` (like code actions, to dodge the revert).
    pub(crate) inlay_accept_pending: bool,

    /// Request keyboard focus for the rename input on the frame it opens.
    pub(crate) rename_focus: bool,

    // ── Find / Replace (Ctrl+F / Ctrl+H / Ctrl+Shift+F / Ctrl+Shift+H) ───────
    /// Search bar state: mode, query/replacement text, results, match cursor.
    pub(crate) find: editor_panel::find_replace::FindReplace,

    /// Full-definition highlight set by a triple-click on a `{`/`}` — `(file,
    /// start, close)` inclusive char range, kept until the selection changes.
    pub(crate) full_block_selection: Option<(ProjectFileId, usize, usize)>,
    /// Live git diff vs HEAD for the file THIS view is showing.
    /// Per view, not shared: it caches exactly one file's hunks, so two
    /// views on different files would recompute over each other every
    /// frame.
    /// Editor gutter diff (live in-memory text vs HEAD) + revert-hunk state.
    pub(crate) diff_gutter: editor_panel::diff_gutter::DiffGutter,
}

impl EditorState {
    /// A fresh view.
    ///
    /// Not `Default::default()`: the keyword completer has to be built with the
    /// Rust syntax so it has a word list, and `Default` cannot supply one.
    pub(crate) fn new() -> Self {
        Self {
            diff_gutter: editor_panel::diff_gutter::DiffGutter::default(),
            editor_widget_id: None,
            fold_anchor: None,
            // Completer: seeded with Rust keywords/types + learns words from code
            completer: Completer::new_with_syntax(&Syntax::rust())
                .with_auto_indent()
                .with_user_words(),
            completion_open: false,
            completion_note: None,
            completion_sel: 0,
            completion_trigger_idx: 0,
            completion_pending_insert: None,
            completion_filtered_items: Vec::new(),
            cargo_complete: editor_panel::cargo_complete::CargoCompleteState::default(),
            last_caret_idx: None,
            pending_scroll_to_line: None,
            highlighted_error_line: None,
            highlighted_def_line: None,
            highlighted_pin_lines: None,
            usages: editor_panel::usages::UsagesState::default(),
            extra_cursors: Vec::new(),
            extra_cursors_file: None,
            mc_prev_primary_sel: None,
            editor_was_focused: false,
            rename_active: false,
            rename_input: String::new(),
            rename_old_name: String::new(),
            rename_new_name: String::new(),
            rename_rel: String::new(),
            rename_line: 0,
            rename_char: 0,
            rename_popup_pos: egui::Pos2::ZERO,
            rename_in_flight: false,
            code_action_in_flight: false,
            code_action_resolve_in_flight: false,
            code_actions: Vec::new(),
            code_action_popup_open: false,
            code_action_sel: 0,
            code_action_popup_pos: egui::Pos2::ZERO,
            code_action_choice: None,
            code_action_add_dep: None,
            add_dep: editor_panel::add_dep::AddDepState::default(),
            extract: editor_panel::extract_fn::ExtractFnState::default(),
            inlay_hint: None,
            inlay_requested: None,
            inlay_accept_pending: false,
            rename_focus: false,
            find: editor_panel::find_replace::FindReplace::default(),
            full_block_selection: None,
        }
    }
}
