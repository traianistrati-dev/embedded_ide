//! "Flow" tab driver — parses the file that is open in the editor, lays its
//! selected function out as a flowchart, and maps clicks back into the editor.
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
use crate::panels::flow_map::{gui, layout, parse};
use eframe::egui;

/// Parsed charts for one file at one content hash, plus the layout of whichever
/// one is being shown.
pub(super) struct FlowCache {
    /// Hash of the text the charts were built from.
    hash: u64,
    /// The file they belong to — switching files must not show stale charts.
    file: ProjectFileId,
    charts: Vec<parse::Chart>,
    /// Set when the LAST parse attempt failed; `charts` then still holds the
    /// last ones that worked.
    error: Option<parse::SyntaxError>,
    /// `(chart name, its layout)` — laying out is cheap, but not free, and this
    /// runs every frame the tab is open.
    laid_out: Option<(String, layout::FlowLayout)>,
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
            match parse::charts_of(&source) {
                Ok(charts) => {
                    self.flow_cache = Some(FlowCache {
                        hash,
                        file: self.selected_file,
                        charts,
                        error: None,
                        laid_out: None,
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
                            charts: Vec::new(),
                            error: Some(e),
                            laid_out: None,
                        });
                    }
                },
            }
            if switched {
                self.flow_view.zoom = 1.0;
                self.flow_view.pan = egui::Vec2::ZERO;
            }
        }

        let Some(cache) = self.flow_cache.as_mut() else {
            return;
        };

        // ── Choose the chart ──────────────────────────────────────────────
        // Restore the persisted choice only for the file it was made in, then
        // fall back to the first ENTRY POINT — `main` is what the reader wants
        // first, not whichever helper happens to be at the top of the file.
        if !cache
            .charts
            .iter()
            .any(|c| c.name == self.flow_view.selected)
        {
            let persisted = (self.flow_selected.0 == rel)
                .then(|| {
                    cache
                        .charts
                        .iter()
                        .find(|c| c.name == self.flow_selected.1)
                        .map(|c| c.name.clone())
                })
                .flatten();
            self.flow_view.selected = persisted
                .or_else(|| {
                    cache
                        .charts
                        .iter()
                        .find(|c| c.kind.is_entry())
                        .map(|c| c.name.clone())
                })
                .or_else(|| cache.charts.first().map(|c| c.name.clone()))
                .unwrap_or_default();
            cache.laid_out = None;
        }

        if cache
            .laid_out
            .as_ref()
            .is_none_or(|(name, _)| *name != self.flow_view.selected)
        {
            cache.laid_out = cache
                .charts
                .iter()
                .find(|c| c.name == self.flow_view.selected)
                .map(|c| (c.name.clone(), layout::layout(c)));
        }

        let status = match (&cache.error, cache.charts.is_empty()) {
            (Some(e), true) => format!("cannot parse this file — line {}: {}", e.line, e.message),
            (Some(e), false) => format!(
                "showing the last good chart — line {} does not parse",
                e.line
            ),
            (None, true) => "no functions in this file".to_string(),
            (None, false) => String::new(),
        };

        let empty = layout::FlowLayout::default();
        let lay = cache.laid_out.as_ref().map(|(_, l)| l).unwrap_or(&empty);
        let result = gui::show(ui, &cache.charts, lay, &mut self.flow_view, &status);

        // Remember the choice for this file (written with the project).
        self.flow_selected = (rel, self.flow_view.selected.clone());

        // ── Clicks ────────────────────────────────────────────────────────
        if let Some(name) = result.open_chart {
            self.flow_view.selected = name;
            self.flow_view.zoom = 1.0;
            self.flow_view.pan = egui::Vec2::ZERO;
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
