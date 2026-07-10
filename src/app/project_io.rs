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

        // The RA workspace content is about to change wholesale — drop the
        // flush hash cache so the first flush re-writes every file.
        self.flushed_hashes.lock().unwrap().clear();

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

        // ── Detect which chip this project targets ───────────────────────────
        // Switching the MCU type BEFORE restoring pin state ensures the correct
        // pin diagram + clock graph are active when parse_main_rs() is applied.
        //
        // Two signals, in priority order:
        //   1. The `// embedded-ide:mcu=<id>` marker in src/main.rs — written by
        //      our own codegen. This pins the EXACT definition (incl. imported
        //      chips that share a HAL crate with a built-in, which step 2 can't
        //      disambiguate — e.g. "esp32c3-graph" vs "esp32c3").
        //   2. Fallback: match the Cargo.toml HAL crate (first token of each
        //      definition's `hal_dep`) for older projects without the marker.
        let main_rs_path = root.join("src").join("main.rs");
        let main_rs_source = std::fs::read_to_string(&main_rs_path).ok();

        let detected_id: Option<String> = main_rs_source
            .as_deref()
            .and_then(crate::panels::mcu_module::codegen::parse_mcu_id)
            .filter(|id| self.mcu_registry.iter().any(|d| &d.id == id))
            .or_else(|| {
                let cargo = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
                self.mcu_registry
                    .iter()
                    .find(|d| {
                        let crate_name = d.project.hal_dep.split_whitespace().next().unwrap_or("");
                        !crate_name.is_empty() && cargo.contains(crate_name)
                    })
                    .map(|d| d.id.clone())
            });

        if let Some(id) = detected_id {
            if id != self.selected_mcu_id {
                self.selected_mcu_id = id;
                self.mcu = Self::build_mcu_for(&self.mcu_registry, &self.selected_mcu_id);
                // Reset LSP — it was attached to the previous chip's workspace.
                self.lsp_state.lock().unwrap().reset();
                self.lsp_selected_diagnostic = None;
            }
        }
        // Opening a project replaces the workspace deps → drop the stale lock so
        // the first check re-resolves for this project (later saves keep it).
        self.reset_workspace_lock();

        // Restore the Structure diagram's dragged positions (`@structure_layout`
        // in mcu.config) — read independently of the MCU restore below since
        // the diagram is chip-agnostic. Missing file/section → automatic layout.
        {
            use crate::panels::mcu_module::mcu_config;
            self.structure_overrides = std::fs::read_to_string(root.join(mcu_config::FILE_NAME))
                .map(|t| mcu_config::parse_structure_layout(&t))
                .unwrap_or_default();
            // Force the next Structure-tab frame to rebuild + re-apply them
            // even when the content hash happens to match the cached graph.
            self.structure_cache = None;
        }

        // ── Restore pin state from src/main.rs ───────────────────────────────
        // Parse the GEN_BEGIN…GEN_END block and apply every recognised pin
        // assignment back to the MCU diagram.  If no markers are found (e.g.
        // an ESP32-C3 project or a hand-written main.rs) this is a silent no-op.
        if let Some(source) = main_rs_source {
            use crate::panels::mcu_module::codegen;
            use crate::panels::mcu_module::mcu_config;

            // Restore virtual modules + clock-tree config from the project-root
            // `mcu.config` file (must happen before update_main_rs below, so the
            // restored clock drives the regenerated chain). Older projects
            // without that file fall back to the legacy `@modules` / `@clock`
            // comment markers that used to live in main.rs.
            match std::fs::read_to_string(root.join(mcu_config::FILE_NAME)) {
                Ok(cfg) => {
                    if let Some(mcu) = &mut self.mcu {
                        mcu.apply_mcu_config(&cfg);
                    }
                }
                Err(_) => {
                    use crate::panels::mcu_module::clock::persist as clock_persist;
                    if let Some(clock) = clock_persist::parse_from_source(&source) {
                        if let Some(mcu) = &mut self.mcu {
                            mcu.apply_saved_clock(clock);
                        }
                    }
                    let restored =
                        crate::panels::mcu_module::modules::persist::parse_from_source(&source);
                    if !restored.is_empty() {
                        if let Some(mcu) = &mut self.mcu {
                            mcu.modules = restored;
                        }
                    }
                }
            }

            let saved = codegen::parse_main_rs(&source);
            if !saved.is_empty() {
                if let Some(mcu) = &mut self.mcu {
                    mcu.apply_saved_pins(&saved);
                    // Restore the per-pin user labels (the `_<label>` suffix on a
                    // binding) — after apply_saved_pins, which would clear them.
                    mcu.apply_saved_pin_labels(&codegen::parse_pin_labels(&source));
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

        // ── Restore editable config files from disk ──────────────────────────
        // Read each generated config file the project carries and refresh its
        // `<<< GENERATED >>>` block from the (now-selected) chip, preserving any
        // edits the user made outside the block. Missing files are generated
        // fresh; files a toolchain doesn't use (memory.x/build.rs on ESP) stay
        // empty.
        if let Some((cfg, tc)) = self.selected_build_cfg() {
            use crate::panels::mcu_module::project_gen::{gen_config, splice_config, ConfigFile};
            let load = |file: ConfigFile, path: std::path::PathBuf| -> String {
                match std::fs::read_to_string(&path) {
                    Ok(disk) => splice_config(file, &disk, &cfg, &tc),
                    Err(_) => gen_config(file, &cfg, &tc),
                }
            };
            self.cargo_toml = load(ConfigFile::CargoToml, root.join("Cargo.toml"));
            self.cargo_config =
                load(ConfigFile::CargoConfig, root.join(".cargo").join("config.toml"));
            self.memory_x = load(ConfigFile::MemoryX, root.join("memory.x"));
            self.build_rs = load(ConfigFile::BuildRs, root.join("build.rs"));
            self.gitignore = load(ConfigFile::GitIgnore, root.join(".gitignore"));
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
                        // Directories must be tracked as FOLDERS, and unreadable
                        // paths skipped — the old unconditional push-as-file made
                        // a fresh folder land in `user_src_files` as a phantom
                        // ("folder1", "") entry that then OVERWROTE the folder's
                        // node in `build_tree` (same map key), rendering the
                        // whole folder as one extension-less "file" until the
                        // project was reopened.
                        let is_dir = abs.is_dir();
                        let content = if is_dir {
                            None
                        } else {
                            std::fs::read_to_string(abs).ok()
                        };
                        apply_fs_create(
                            &mut self.project_tree.user_src_files,
                            &mut self.project_tree.user_src_folders,
                            &rel,
                            is_dir,
                            content,
                        );
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

impl AppIde {
    /// Start a git operation on a worker thread (signal handler for the Git
    /// tab's buttons and the tree's context menu). Guards: needs a saved
    /// project (`project_dir`), no overlap with a running op, and no save in
    /// flight (git would read a half-written tree).
    pub(super) fn run_git_op(&mut self, op: crate::git::GitOp) {
        let Some(dir) = self.project_dir.clone() else {
            return; // the tab shows the "save first" hint instead
        };
        if self.git.is_busy() {
            return;
        }
        if self.save_in_progress.is_some() {
            self.git
                .state
                .lock()
                .unwrap()
                .lines
                .push((crate::git::GitLine::Notice, "[busy] save in progress — retry in a moment".into()));
            return;
        }
        let msg = self.git.commit_msg.trim().to_owned();
        let remote = self.git.remote_url_draft.trim().to_owned();
        // Checkbox selection: everything checked → plain `add -A`; otherwise
        // stage only the checked changed files. All-unchecked never spawns.
        let add_paths = if self.git.excluded.is_empty() {
            None
        } else {
            let picked: Vec<String> = self
                .git
                .state
                .lock()
                .unwrap()
                .status
                .changes
                .iter()
                .map(|c| c.path.clone())
                .filter(|p| !self.git.excluded.contains(p))
                .collect();
            if picked.is_empty() && matches!(op, crate::git::GitOp::Commit | crate::git::GitOp::CommitPush)
            {
                self.git.state.lock().unwrap().lines.push((
                    crate::git::GitLine::Notice,
                    "[info] no files checked — check what you want in the commit".into(),
                ));
                return;
            }
            Some(picked)
        };
        crate::git::run_op(
            op,
            msg,
            remote,
            add_paths,
            dir,
            self.git_disk_snapshot(),
            std::sync::Arc::clone(&self.git.state),
            std::sync::Arc::clone(&self.activity),
            self.egui_ctx.clone(),
        );
    }

    /// The in-memory project content, keyed by project-relative path — the
    /// exact file set `write_project` persists. The git worker compares it
    /// against disk for the "unsaved changes" warning (commits are strictly
    /// what's ON DISK; this only powers the warning).
    fn git_disk_snapshot(&self) -> Vec<(String, String)> {
        let files = self.current_project_files();
        let mut snap = vec![
            ("src/main.rs".to_owned(), files.main_rs),
            ("Cargo.toml".to_owned(), files.cargo_toml),
            (".cargo/config.toml".to_owned(), files.cargo_config),
            (".gitignore".to_owned(), files.gitignore),
        ];
        if !files.memory_x.is_empty() {
            snap.push(("memory.x".to_owned(), files.memory_x));
        }
        if !files.build_rs.is_empty() {
            snap.push(("build.rs".to_owned(), files.build_rs));
        }
        let mcu_cfg = self.mcu_config_text();
        if !mcu_cfg.trim().is_empty() {
            snap.push((
                crate::panels::mcu_module::mcu_config::FILE_NAME.to_owned(),
                mcu_cfg,
            ));
        }
        for (rel, content) in &self.project_tree.user_src_files {
            snap.push((format!("src/{rel}"), content.clone()));
        }
        snap
    }
}

/// Apply one watcher CREATE event to the tree state. A directory is tracked as
/// a FOLDER (never a file); a file needs readable `content` (`None` — deleted
/// meanwhile, or unreadable — is skipped, NOT pushed as an empty phantom).
/// Duplicates of already-tracked entries are ignored. Pure, so the
/// directory-pushed-as-file regression stays covered by tests.
pub(super) fn apply_fs_create(
    user_src_files: &mut Vec<(String, String)>,
    user_src_folders: &mut Vec<String>,
    rel: &str,
    is_dir: bool,
    content: Option<String>,
) {
    if is_dir {
        if !user_src_folders.iter().any(|f| f == rel) {
            user_src_folders.push(rel.to_owned());
        }
        return;
    }
    let Some(content) = content else {
        return;
    };
    if !user_src_files.iter().any(|(p, _)| p == rel) {
        user_src_files.push((rel.to_owned(), content));
    }
}

#[cfg(test)]
mod fs_create_tests {
    use super::apply_fs_create;

    /// The reported bug: creating `folder1` fired a watcher CREATE that was
    /// pushed into `user_src_files` — the phantom file then shadowed the
    /// folder node in the tree (same map key), showing an extension-less
    /// "file" instead of the folder until the project was reopened.
    #[test]
    fn directory_create_is_tracked_as_folder_not_file() {
        let mut files = Vec::new();
        let mut folders = Vec::new();
        apply_fs_create(&mut files, &mut folders, "folder1", true, None);
        assert!(files.is_empty(), "a directory must never become a file entry");
        assert_eq!(folders, vec!["folder1".to_owned()]);
        // Re-delivered event (or our own create + the watcher's) → no dupe.
        apply_fs_create(&mut files, &mut folders, "folder1", true, None);
        assert_eq!(folders.len(), 1);
    }

    #[test]
    fn file_create_adds_once_with_content() {
        let mut files = Vec::new();
        let mut folders = Vec::new();
        apply_fs_create(&mut files, &mut folders, "folder1/file1.rs", false, Some("// x\n".into()));
        assert_eq!(files, vec![("folder1/file1.rs".to_owned(), "// x\n".to_owned())]);
        // The IDE's own inline-create already tracked it → the watcher's echo
        // must not duplicate (or overwrite newer in-memory content).
        apply_fs_create(&mut files, &mut folders, "folder1/file1.rs", false, Some("stale".into()));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, "// x\n");
        assert!(folders.is_empty());
    }

    #[test]
    fn unreadable_file_is_skipped_not_pushed_empty() {
        let mut files = Vec::new();
        let mut folders = Vec::new();
        apply_fs_create(&mut files, &mut folders, "ghost.rs", false, None);
        assert!(files.is_empty(), "no phantom (\"ghost.rs\", \"\") entries");
        assert!(folders.is_empty());
    }
}
