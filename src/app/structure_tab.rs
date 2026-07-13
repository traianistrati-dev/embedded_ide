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
            let mut lay = layout::layout(&graph);
            layout::apply_overrides(&mut lay, &graph, &self.structure_overrides);
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
                        layout::apply_overrides(lay, graph, &self.structure_overrides);
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

        // ── Draw + handle clicks / drags ───────────────────────────────────
        let Some((_, graph, lay)) = self.structure_cache.as_mut() else {
            return;
        };
        let call_edges: &[calls::CallEdge] = self
            .structure_calls
            .as_ref()
            .map(|p| p.edges.as_slice())
            .unwrap_or(&[]);
        // Focused module for the call-edge filter = the currently selected
        // file's node (main by default — also for config files like
        // Cargo.toml, which have no node). Clicking a diagram node opens its
        // file, so the focus follows node clicks too.
        let focus_node = match self.selected_file {
            ProjectFileId::UserFile(i) => graph
                .nodes
                .iter()
                .position(|n| n.file == Some(i))
                .unwrap_or(0),
            _ => 0, // main.rs / config files → main
        };
        // Per-node error flags (rust-analyzer + flycheck diagnostics, keyed by
        // the workspace-relative path) — nodes with errors blink a red border.
        let node_errors: Vec<bool> = {
            let lsp = self.lsp_state.lock().unwrap();
            graph
                .nodes
                .iter()
                .map(|n| lsp.error_count_for(&format!("src/{}", n.file_rel)) > 0)
                .collect()
        };
        let result = gui::show(
            ui,
            &*graph,
            lay,
            &mut self.structure_view,
            call_edges,
            &calls_status,
            focus_node,
            &node_errors,
        );

        // A header drag ended → pin that node's position (keyed by its file,
        // which survives graph rebuilds). Saved with the project (mcu.config).
        if let Some(i) = result.moved {
            if let Some(node) = graph.nodes.get(i) {
                self.structure_overrides
                    .insert(node.file_rel.clone(), (lay.pos[i].x, lay.pos[i].y));
            }
        }

        // "Auto layout" → drop every pin and re-run the automatic arrangement
        // (with the current call pairs, when the pass has delivered them).
        if result.reset_layout {
            self.structure_overrides.clear();
            let pairs: Vec<(usize, usize)> = self
                .structure_calls
                .as_ref()
                .filter(|p| p.hash == hash && !p.running())
                .map(|p| {
                    let set: std::collections::BTreeSet<(usize, usize)> =
                        p.edges.iter().map(|e| (e.from_node, e.to_node)).collect();
                    set.into_iter().collect()
                })
                .unwrap_or_default();
            *lay = layout::layout_with_calls(&*graph, &pairs);
            self.structure_layout_calls = pairs.len();
        }

        if let Some(click) = result.click {
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
            let pass = calls::CallPass::new(graph, hash);
            crate::lsp::debug_log(&format!(
                "CALLS_PASS start total={} hash={hash:x}",
                pass.total
            ));
            self.structure_calls = Some(pass);
        }
        let pass = self.structure_calls.as_mut().unwrap();

        let mut lsp = self.lsp_state.lock().unwrap();

        // 1) Receive the completed lookup (stale keys from a superseded pass
        //    are simply dropped by the key match).
        for (key, locs) in lsp.take_calls_reference_results() {
            if pass.in_flight.map(|(k, _, _)| k) == Some(key) {
                let (_, node, row) = pass.in_flight.take().unwrap();
                pass.add_references(graph, node, row, &locs);
                pass.log_once(format!(
                    "CALLS_RESP key={key} sites={} done={}/{} edges={}",
                    locs.len(),
                    pass.done,
                    pass.total,
                    pass.edges.len()
                ));
            }
        }

        // 2) Fire the next lookup — scanning the WHOLE queue for the first
        //    fireable symbol, so one open-but-edited file can't freeze the
        //    rest of the pass behind it (head-of-line blocking).
        let mut blocked: Option<&'static str> = None;
        if pass.in_flight.is_none() && !pass.queue.is_empty() {
            if !matches!(lsp.status, crate::lsp::LspStatus::Ready) {
                blocked = Some("waiting for rust-analyzer…");
            } else if lsp.references_busy() {
                blocked = Some("waiting for the usages pass…");
            } else {
                // Sync gate per symbol: RA must hold THIS text, or the symbol
                // positions (and the reply's site lines) would be stale. Never
                // a did_change (version bumps cancel other requests and
                // re-trigger flycheck) — but a file RA hasn't opened AT ALL
                // (fresh app start: docs only open on Save/completion) is
                // seeded with did_open: a FIRST open bumps nothing, cancels
                // nothing, runs no flycheck. Open-but-EDITED files wait for
                // the next Project Save (and are skipped over meanwhile).
                let mut fired = false;
                for qi in 0..pass.queue.len() {
                    let (node_i, row) = pass.queue[qi];
                    let node = &graph.nodes[node_i];
                    let content: &str = match node.file {
                        None => &self.generated_code,
                        Some(i) => &self.project_tree.user_src_files[i].1,
                    };
                    let rel = format!("src/{}", node.file_rel);
                    let synced = lsp.last_sent_matches(&rel, content);
                    let seed_open = !synced && !lsp.is_file_open(&rel);
                    if !(synced || seed_open) {
                        continue; // open but edited — retry after the next save
                    }
                    if seed_open {
                        lsp.did_open(&rel, content);
                    }
                    pass.queue.remove(qi);
                    let sym = &node.symbols[row];
                    let key = pass.take_key();
                    pass.log_once(format!(
                        "CALLS_REQ key={key} file={rel} line={} col={} sym={} seed={seed_open}",
                        sym.line - 1,
                        sym.col,
                        sym.name
                    ));
                    lsp.request_references_for_calls(
                        &rel,
                        (sym.line - 1) as u32,
                        sym.col as u32,
                        key,
                    );
                    pass.in_flight = Some((key, node_i, row));
                    pass.waiting_sync = false;
                    fired = true;
                    break;
                }
                if !fired {
                    pass.waiting_sync = true;
                }
            }
        }

        // 3) Toolbar status — always says WHY nothing is moving.
        let status = if pass.running() {
            if let Some(b) = blocked {
                b.to_owned()
            } else if pass.waiting_sync {
                "unsaved changes — Save the project to update the call graph".to_owned()
            } else {
                format!("analyzing calls {}/{}…", pass.done, pass.total)
            }
        } else if !pass.edges.is_empty() {
            format!("{} call edges", pass.edges.len())
        } else {
            // Distinguish "finished, none found" from "not running" — an
            // empty diagram with silent status made failures undiagnosable.
            "no cross-module calls found".to_owned()
        };
        pass.log_once(format!("CALLS_STATE {status}"));
        status
    }
}
