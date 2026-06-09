//! Project I/O â loading an existing Cargo project from disk and polling
//! the filesystem watcher for external file changes.
//!
//! Both are inherent methods on AppIde (child module of app), so they can
//! mutate the many self fields involved in project + tree state.

use super::AppIde;
use super::ProjectFileId;
use crate::project_tree::ProjectTreeState;
use notify::Watcher as _;

impl AppIde {
    // ── Project load ──────────────────────────────────────────────────────────

    /// Loads user source files from an existing Cargo project at `root`.
    /// Only files in `root/src/` are imported; `main.rs` is always skipped
    /// (it is regenerated from MCU pin state).
    /// Any previous user files are replaced.
    pub(super) fn load_project_from_dir(&mut self, root: &std::path::Path) {
        // Load files and folders via ProjectTreeState
        self.project_tree = ProjectTreeState::load_from_dir(root);

        self.selected_file = ProjectFileId::MainRs;
        self.renaming_file = None;
        self.renaming_folder = None;
        self.new_src_name = None;
        self.new_src_folder_name = None;
        self.new_file_parent_folder = None;
        self.new_folder_parent_folder = None;
        self.new_file_in_folder = None;
        self.project_name = root.file_name().and_then(|n| n.to_str()).map(String::from);
        self.project_dir = Some(root.to_path_buf());

        // ── Detect MCU type from Cargo.toml ──────────────────────────────────
        // Read the dependency block to determine which chip this project targets.
        // Switching the MCU type BEFORE restoring pin state ensures the correct
        // pin diagram is active when parse_main_rs() is applied.
        let cargo_toml = root.join("Cargo.toml");
        if let Ok(cargo) = std::fs::read_to_string(&cargo_toml) {
            // Match the project's HAL crate against each registered MCU's hal_dep
            // (the crate name = first whitespace token of `hal_dep`).
            let detected_id: Option<String> = self
                .mcu_registry
                .iter()
                .find(|d| {
                    let crate_name = d.project.hal_dep.split_whitespace().next().unwrap_or("");
                    !crate_name.is_empty() && cargo.contains(crate_name)
                })
                .map(|d| d.id.clone());

            if let Some(id) = detected_id {
                if id != self.selected_mcu_id {
                    self.selected_mcu_id = id;
                    self.mcu = Self::build_mcu_for(&self.mcu_registry, &self.selected_mcu_id);
                    // Reset LSP — it was attached to the previous chip's workspace.
                    self.lsp_state.lock().unwrap().reset();
                    self.lsp_selected_diagnostic = None;
                }
            }
        }

        // ── Restore pin state from src/main.rs ───────────────────────────────
        // Parse the GEN_BEGIN…GEN_END block and apply every recognised pin
        // assignment back to the MCU diagram.  If no markers are found (e.g.
        // an ESP32-C3 project or a hand-written main.rs) this is a silent no-op.
        let main_rs_path = root.join("src").join("main.rs");
        if let Ok(source) = std::fs::read_to_string(&main_rs_path) {
            use crate::panels::mcu_module::clock::persist as clock_persist;
            use crate::panels::mcu_module::codegen;

            // Restore the clock-tree config first, so any regeneration below
            // uses it (otherwise a custom clock would reset to the 72 MHz default).
            if let Some(clock) = clock_persist::parse_from_source(&source) {
                if let Some(mcu) = &mut self.mcu {
                    mcu.apply_saved_clock(clock);
                }
            }

            let saved = codegen::parse_main_rs(&source);
            if !saved.is_empty() {
                if let Some(mcu) = &mut self.mcu {
                    mcu.apply_saved_pins(&saved);
                    // Rebuild generated_code from the restored pin state while
                    // keeping the user's loop body from the existing file.
                    self.generated_code = mcu.update_main_rs(&source);
                }
            } else {
                // No parseable pins (blank STM32 project, ESP32-C3, or
                // hand-written main.rs).  Always reset the MCU diagram so
                // pins configured in the previously-open project do not
                // bleed into this one.
                if let Some(mcu) = &mut self.mcu {
                    mcu.reset_all_pins();
                }
                self.generated_code = source;
            }
        }
    }

    // ── Filesystem watcher polling ────────────────────────────────────────────
    /// Drains the notify channel and applies any relevant Create / Remove /
    /// Rename events to `project_tree`.
    ///
    /// Rules:
    /// - Only files inside `workspace/src/` are tracked.
    /// - `src/main.rs` is always excluded (it is the generated file).
    /// - Create: add if not already present (avoids duplicates from our own writes).
    /// - Remove: drop from the list (IDE-initiated removes are already gone).
    /// - Rename: atomically update the stored path.
    pub(super) fn poll_fs_events(&mut self) {
        use crate::project_tree::logic::FsEventKind;
        use notify::EventKind::*;
        use notify::event::{ModifyKind, RenameMode};

        let workspace_src = std::env::temp_dir()
            .join("embedded_ide_0_check")
            .join("src");

        // If the watcher hasn't started watching yet (dir didn't exist on
        // startup), try to attach now that write_project may have created it.
        if let (Some(w), true) = (self._fs_watcher.as_mut(), workspace_src.exists()) {
            // `watch` is idempotent for already-watched paths.
            let _ = w.watch(&workspace_src, notify::RecursiveMode::Recursive);
        }

        let Some(rx) = self.fs_rx.as_ref() else {
            return;
        };

        let mut events = Vec::new();

        for event in rx.try_iter().flatten() {
            match event.kind {
                Create(_) => {
                    for abs in &event.paths {
                        let Ok(rel) = abs.strip_prefix(&workspace_src) else {
                            continue;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if rel == "main.rs" {
                            continue;
                        }
                        // Only add if not already tracked (avoids duplicates from our own writes)
                        if !self
                            .project_tree
                            .user_src_files
                            .iter()
                            .any(|(p, _)| p == &rel)
                        {
                            // Read the file content so the editor shows it correctly
                            let content = std::fs::read_to_string(abs).unwrap_or_default();
                            self.project_tree.user_src_files.push((rel, content));
                        }
                    }
                }
                Remove(_) => {
                    for abs in &event.paths {
                        let Ok(rel) = abs.strip_prefix(&workspace_src) else {
                            continue;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        events.push((rel, FsEventKind::Remove));
                    }
                }
                Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                    let old = &event.paths[0];
                    let new = &event.paths[1];
                    let Ok(old_rel) = old.strip_prefix(&workspace_src) else {
                        continue;
                    };
                    let Ok(new_rel) = new.strip_prefix(&workspace_src) else {
                        continue;
                    };
                    let old_rel = old_rel.to_string_lossy().replace('\\', "/");
                    let new_rel = new_rel.to_string_lossy().replace('\\', "/");
                    events.push((
                        old_rel.clone(),
                        FsEventKind::Rename {
                            old_rel: old_rel.clone(),
                            new_rel,
                        },
                    ));
                }
                _ => {}
            }
        }

        // Delegate Remove and Rename events to ProjectTreeState
        if !events.is_empty() {
            self.project_tree.handle_fs_events(events);
        }
    }
}
