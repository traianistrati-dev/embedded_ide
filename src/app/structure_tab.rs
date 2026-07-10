//! "Structure" tab driver — caches the parsed module graph + layout, drives
//! the Phase-3 call-graph pass, and maps node/row clicks into the editor.
//!
//! The graph is rebuilt only when the project content changes (hash over every
//! file's path + text), so keeping the tab open costs one hash pass per frame
//! and nothing else. The graph itself is parse-based (no LSP); only the
//! OPTIONAL call-edge pass talks to rust-analyzer, under strict discipline —
//! see `structure_map::calls` for the rules (serialized, no did_change, sync-
//! gated, dedicated reply channel).

use super::{AppIde, ProjectFileId};
use crate::panels::structure_map::{calls, gui, layout, parse};
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
            self.structure_layout_calls = 0; // fresh layout knows no call edges
        }

        // ── Drive the call-graph pass (only while the tab is open) ─────────
        let calls_status = self.tick_structure_calls(hash);

        // ── Re-layout once the call pass settles ───────────────────────────
        // The initial layout only knows module edges; when the finished pass
        // contributes call pairs, one re-layout lets the ordering + transpose
        // minimize call-edge crossings too (see `layout_with_calls`).
        if let Some(pass) = &self.structure_calls {
            if pass.hash == hash && !pass.running() {
                let pairs: std::collections::BTreeSet<(usize, usize)> = pass
                    .edges
                    .iter()
                    .map(|e| (e.from_node, e.to_node))
                    .collect();
                if pairs.len() != self.structure_layout_calls {
                    if let Some((_, graph, lay)) = self.structure_cache.as_mut() {
                        let pairs: Vec<(usize, usize)> = pairs.into_iter().collect();
                        *lay = layout::layout_with_calls(graph, &pairs);
                        self.structure_layout_calls = pairs.len();
                    }
                }
            }
        }
        // Keep frames coming while the pass works or waits for a save — LSP
        // replies repaint on arrival, but the NEXT request fires from here.
        if self
            .structure_calls
            .as_ref()
            .is_some_and(|p| p.running())
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }

        // ── Draw + handle a node / symbol-row click ────────────────────────
        let Some((_, graph, lay)) = &self.structure_cache else {
            return;
        };
        let call_edges: &[calls::CallEdge] = self
            .structure_calls
            .as_ref()
            .map(|p| p.edges.as_slice())
            .unwrap_or(&[]);
        if let Some(click) = gui::show(
            ui,
            graph,
            lay,
            &mut self.structure_view,
            call_edges,
            &calls_status,
        ) {
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

    /// One step of the call-graph pass: receive the in-flight reply, then fire
    /// the next symbol's references lookup. Returns the toolbar status text.
    ///
    /// Discipline (the rules that keep saves fast and Ctrl+Enter alive):
    /// NO did_change — a symbol is queried only while RA already holds its
    /// file's current text; one request in flight, and none while the usages
    /// pass runs its own search (`references_busy`).
    fn tick_structure_calls(&mut self, hash: u64) -> String {
        let Some((_, graph, _)) = &self.structure_cache else {
            return String::new();
        };
        if !self.structure_view.show_calls {
            return String::new(); // toggle off → don't spend requests
        }
        // (Re)start the pass when the content hash moved.
        if self.structure_calls.as_ref().map(|p| p.hash) != Some(hash) {
            self.structure_calls = Some(calls::CallPass::new(graph, hash));
        }
        let pass = self.structure_calls.as_mut().unwrap();

        let mut lsp = self.lsp_state.lock().unwrap();

        // 1) Receive the completed lookup (stale keys from a superseded pass
        //    are simply dropped by the key match).
        for (key, locs) in lsp.take_calls_reference_results() {
            if pass.in_flight.map(|(k, _, _)| k) == Some(key) {
                let (_, node, row) = pass.in_flight.take().unwrap();
                pass.add_references(graph, node, row, &locs);
            }
        }

        // 2) Fire the next lookup.
        if pass.in_flight.is_none() && !pass.queue.is_empty() {
            let ready = matches!(lsp.status, crate::lsp::LspStatus::Ready);
            if ready && !lsp.references_busy() {
                let &(node_i, row) = pass.queue.front().unwrap();
                let node = &graph.nodes[node_i];
                let content: &str = match node.file {
                    None => &self.generated_code,
                    Some(i) => &self.project_tree.user_src_files[i].1,
                };
                let rel = format!("src/{}", node.file_rel);
                // Sync gate: RA must hold THIS text, or the symbol positions
                // (and the reply's site lines) would be stale. No did_change —
                // the pass just waits for the next Project Save.
                if lsp.last_sent_matches(&rel, content) {
                    pass.queue.pop_front();
                    let sym = &node.symbols[row];
                    let key = pass.take_key();
                    lsp.request_references_for_calls(
                        &rel,
                        (sym.line - 1) as u32,
                        sym.col as u32,
                        key,
                    );
                    pass.in_flight = Some((key, node_i, row));
                    pass.waiting_sync = false;
                } else {
                    pass.waiting_sync = true;
                }
            }
        }

        // 3) Toolbar status.
        if pass.waiting_sync && pass.running() {
            "unsaved changes — Save the project to update the call graph".to_owned()
        } else if pass.running() {
            format!("analyzing calls {}/{}…", pass.done, pass.total)
        } else if !pass.edges.is_empty() {
            format!("{} call edges", pass.edges.len())
        } else {
            String::new()
        }
    }
}
