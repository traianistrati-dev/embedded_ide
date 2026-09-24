//! "Flow" tab driver — parses the file that is open in the editor, lays its
//! selected function out as a flowchart (or lists every element of the file,
//! "All — whole file"), maps clicks back into the editor, and scrolls to the
//! editor's caret when it moves.
//!
//! Scope, phase 1: the file the CodeEditor is showing. That is one variable and
//! not two — `AppIde::selected_file` is both "the file selected in the project
//! tree" and the file the editor renders, so following the editor and following
//! the tree are the same thing. Folder and whole-project scope (parallel lanes
//! per entry point) is phase 2.
//!
//! The buffers this reads (`generated_code`, `user_src_files`) are the LIVE
//! ones, so the chart follows typing rather than waiting for a save. That is
//! also why a parse failure has to be survivable: half a keystroke into an `if`
//! the file does not parse, and blanking the panel on every other character
//! would make the tab unusable. The last good charts stay on screen and the
//! toolbar says which line stopped the parser.

use super::{AppIde, ProjectFileId};
use crate::panels::flow_map::{compose, gui, layout, parse};
use eframe::egui;

/// One file's elements and charts at one content hash, plus the layout of
/// whichever chart is being shown.
pub(super) struct FlowCache {
    /// Hash of the text the model was built from.
    hash: u64,
    /// The file it belongs to — switching files must not show stale charts.
    file: ProjectFileId,
    model: parse::FileModel,
    /// Set when the LAST parse attempt failed; `model` then still holds the
    /// last one that worked.
    error: Option<parse::SyntaxError>,
    /// `(chart key, its layout)` — laying out is cheap, but not free, and this
    /// runs every frame the tab is open.
    laid_out: Option<(String, layout::FlowLayout)>,
    /// The whole file drawn as a page, and `(element key, canvas)` of the one
    /// element drawn as a page (a container, a type, a function too tall to
    /// fit). Two slots, so visiting a function does not throw away the
    /// whole-file canvas - on a big file, the expensive one. Built the first
    /// frame each is shown and kept for as long as `model` is: a syntax error
    /// keeps the last good model, and with it these, so typing half a line
    /// rebuilds nothing.
    whole: Option<compose::Composed>,
    scoped: Option<(String, compose::Composed)>,
}

impl AppIde {
    /// Render the Flow tab (called from the MCU-panel tab dispatch).
    pub(super) fn show_flow_tab(&mut self, ui: &mut egui::Ui) {
        let (source, rel) = self.flow_source();
        let Some(source) = source else {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(
                    "The Flow tab charts Rust source. Select a `.rs` file in the project tree.",
                )
                .size(12.0)
                .color(egui::Color32::from_rgb(150, 150, 160)),
            );
            return;
        };

        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            source.hash(&mut h);
            h.finish()
        };

        // ── Re-parse when the file or its text changed ─────────────────────
        let stale = self
            .flow_cache
            .as_ref()
            .is_none_or(|c| c.hash != hash || c.file != self.selected_file);
        if stale {
            let switched = self
                .flow_cache
                .as_ref()
                .is_none_or(|c| c.file != self.selected_file);
            match parse::parse_file(&source) {
                Ok(model) => {
                    self.flow_cache = Some(FlowCache {
                        hash,
                        file: self.selected_file,
                        model,
                        error: None,
                        laid_out: None,
                        whole: None,
                        scoped: None,
                    });
                }
                Err(e) => match (&mut self.flow_cache, switched) {
                    // Same file, still being typed in: keep what was drawn and
                    // report the line, rather than flashing an empty panel.
                    (Some(c), false) => {
                        c.hash = hash;
                        c.error = Some(e);
                    }
                    // A DIFFERENT file that does not parse has no last-good
                    // chart to fall back on — showing the previous file's would
                    // be worse than showing none.
                    _ => {
                        self.flow_cache = Some(FlowCache {
                            hash,
                            file: self.selected_file,
                            model: parse::FileModel::default(),
                            error: Some(e),
                            laid_out: None,
                            whole: None,
                            scoped: None,
                        });
                    }
                },
            }
            if switched {
                self.flow_view.reset_file();
            }
        }

        let Some(cache) = self.flow_cache.as_mut() else {
            return;
        };

        // ── Follow the editor's caret ─────────────────────────────────────
        // Only while the model is this text's own: the last good version kept
        // through a syntax error has lines that no longer match the ones
        // being typed. A click in Flow jumps the editor without moving its
        // caret, so following never feeds back into itself.
        if let Some(idx) = crate::panels::flow_map::caret_to_follow(
            &mut self.flow_caret,
            self.ed.caret_at,
            self.selected_file,
        ) && cache.error.is_none()
        {
            self.flow_view.reveal_line = Some(crate::panels::flow_map::line_of_char(&source, idx));
        }

        // ── Choose what to show ───────────────────────────────────────────
        // Kept up to date in the whole-file view too: `selected` is what
        // leaving it goes back to. The persisted choice counts only for the
        // file it was made in.
        let model = &cache.model;
        if !model
            .elements
            .iter()
            .any(|e| e.key == self.flow_view.selected && e.openable())
        {
            let persisted = (self.flow_selected.0 == rel).then_some(self.flow_selected.1.as_str());
            self.flow_view.selected = crate::panels::flow_map::choose_selection(
                model,
                &self.flow_view.selected,
                persisted,
            );
            cache.laid_out = None;
        }
        let scope = gui::scope_of(model, &self.flow_view);

        // A function's chart, laid out once per selection.
        if let gui::Scope::Chart(i) = scope
            && cache
                .laid_out
                .as_ref()
                .is_none_or(|(key, _)| *key != self.flow_view.selected)
        {
            cache.laid_out = model.elements[i].chart.map(|c| {
                (
                    model.charts[c].key.clone(),
                    layout::layout(&model.charts[c]),
                )
            });
        }

        let status = crate::panels::flow_map::status_line(
            &cache.model,
            cache.error.as_ref(),
            scope.shows_elements(),
        );

        // The canvas of whatever is drawn as a page: the whole file or a
        // container in Implementation, a type's card, and a function (for when
        // it is too tall to read fitted whole - the driver cannot know that,
        // it depends on the panel, and one chart's canvas is cheap).
        let implementation = self.flow_view.implementation;
        let canvas = match scope {
            gui::Scope::Whole if implementation => {
                if cache.whole.is_none() {
                    cache.whole = Some(compose::compose(&cache.model));
                }
                cache.whole.as_ref()
            }
            gui::Scope::Container(i) | gui::Scope::Card(i) | gui::Scope::Chart(i)
                if implementation || !matches!(scope, gui::Scope::Container(_)) =>
            {
                let key = &cache.model.elements[i].key;
                if cache.scoped.as_ref().is_none_or(|(k, _)| k != key) {
                    let canvas = compose::compose_scope(&cache.model, Some(i));
                    cache.scoped = Some((key.clone(), canvas));
                }
                cache.scoped.as_ref().map(|(_, c)| c)
            }
            _ => None,
        };

        let empty = layout::FlowLayout::default();
        let lay = cache.laid_out.as_ref().map(|(_, l)| l).unwrap_or(&empty);
        let result = gui::show(ui, &cache.model, lay, canvas, &mut self.flow_view, &status);

        // Remember the choice for this file (written with the project).
        self.flow_selected = (rel, self.flow_view.selected.clone());

        // ── Clicks ────────────────────────────────────────────────────────
        // Opening a function - from a subroutine box, or a double click in the
        // whole-file list - always lands on its chart.
        if let Some(key) = result.open_chart {
            self.flow_view.open(key);
        }
        if let Some(line) = result.goto_line {
            let id = self.selected_file;
            self.ed.pending_scroll_to_line = Some((id, line));
            self.ed.highlighted_def_line = Some((id, line));
        }
    }

    /// The Rust text to chart, and the file's project-root-relative path.
    ///
    /// `None` for anything that is not Rust: `Cargo.toml` and `memory.x` have
    /// no control flow, and handing them to `syn` would only produce a syntax
    /// error that says nothing useful.
    fn flow_source(&self) -> (Option<String>, String) {
        match self.selected_file {
            ProjectFileId::MainRs => (Some(self.generated_code.clone()), "src/main.rs".to_string()),
            ProjectFileId::UserFile(i) => match self.project_tree.user_src_files.get(i) {
                Some((path, text)) if path.ends_with(".rs") => (Some(text.clone()), path.clone()),
                Some((path, _)) => (None, path.clone()),
                None => (None, String::new()),
            },
            _ => (None, String::new()),
        }
    }
}
