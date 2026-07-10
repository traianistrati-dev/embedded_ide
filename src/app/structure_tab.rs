//! "Structure" tab driver — caches the parsed module graph + layout and maps
//! a node click to opening that file in the editor.
//!
//! The graph is rebuilt only when the project content changes (hash over every
//! file's path + text), so keeping the tab open costs one hash pass per frame
//! and nothing else. Parse-based: no LSP requests, no interaction with
//! rust-analyzer's pipeline.

use super::{AppIde, ProjectFileId};
use crate::panels::structure_map::{gui, layout, parse};
use eframe::egui;

impl AppIde {
    /// Render the Structure tab (called from the MCU-panel tab dispatch).
    pub(super) fn show_structure_tab(&mut self, ui: &mut egui::Ui) {
        // ── Rebuild the graph when the project content changed ────────────
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.generated_code.hash(&mut h);
            for (rel, content) in &self.project_tree.user_src_files {
                rel.hash(&mut h);
                content.hash(&mut h);
            }
            h.finish()
        };
        if self.structure_cache.as_ref().map(|(h, _, _)| *h) != Some(hash) {
            let graph =
                parse::build_graph(&self.generated_code, &self.project_tree.user_src_files);
            let lay = layout::layout(&graph);
            self.structure_cache = Some((hash, graph, lay));
        }

        // ── Draw + handle a node click ─────────────────────────────────────
        let Some((_, graph, lay)) = &self.structure_cache else {
            return;
        };
        if let Some(click) = gui::show(ui, graph, lay, &mut self.structure_view) {
            let id = match click.file {
                None => ProjectFileId::MainRs,
                // Guard against a stale index (file list changed this frame).
                Some(i) if i < self.project_tree.user_src_files.len() => {
                    ProjectFileId::UserFile(i)
                }
                Some(_) => return,
            };
            self.selected_file = id;
            // A symbol-row click also jumps to the item's line (same scroll +
            // highlight path the usages popup and F12 navigation use).
            if let Some(line) = click.line {
                self.pending_scroll_to_line = Some((id, line));
                self.highlighted_def_line = Some((id, line));
            }
        }
    }
}
