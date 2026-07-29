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
        // LF-normalized, like every user file (see `scan_src_dir`): the git
        // gutter compares this text against an LF baseline, and a CRLF body
        // from a Windows checkout showed as a permanent phantom diff.
        let main_rs_source = std::fs::read_to_string(&main_rs_path)
            .ok()
            .map(|s| s.replace("\r\n", "\n"));

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

        // Restore the Structure diagram's dragged positions from
        // `project_structure.config` — read independently of the MCU restore
        // below since the diagram is chip-agnostic. Missing file/section →
        // automatic layout. `load` falls back to `mcu.config`, where this state
        // lived before it got its own file.
        {
            use crate::panels::mcu_module::structure_config;
            let (positions, view) = structure_config::load(root);
            self.structure_overrides = positions;
            // View options (Calls / depth / path style / externals) — absent
            // section (older projects) keeps the defaults.
            if let Some((show_calls, depth, style, externals)) = view {
                self.structure_view.show_calls = show_calls;
                self.structure_view.call_depth = depth;
                self.structure_view.path_style =
                    crate::panels::structure_map::gui::PathStyle::from_u8(style);
                self.structure_view.show_externals = externals;
            }
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
                    // LF-normalized like every buffer (phantom-gutter rule).
                    Ok(disk) => splice_config(file, &disk.replace("\r\n", "\n"), &cfg, &tc),
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

        // Verify the project still loads for cargo (and therefore rust-analyzer)
        // — a workspace member left in a bad state (e.g. an incompatible library
        // added by hand) would otherwise fail silently as a stuck "Checking…".
        self.recheck_workspace_health();
    }

    // ── Project rename ────────────────────────────────────────────────────────

    /// Rename the saved project's FOLDER (leaf only — always the same drive)
    /// and update `project_dir` / `project_name`. The Cargo package name is
    /// deliberately untouched: it is per-chip (the flash pipeline looks the ELF
    /// up by it), not per-project. Everything else follows automatically — the
    /// Git tab reads `project_dir` per command, the fs watcher watches the temp
    /// RA workspace, and no project file stores the folder name.
    pub(super) fn rename_project(&mut self, new_name: &str) -> Result<(), String> {
        let old_dir = self
            .project_dir
            .clone()
            .ok_or("No saved project to rename")?;
        let new_dir = rename_project_dir(&old_dir, new_name)?;
        let name = new_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| new_name.trim().to_owned());
        self.project_dir = Some(new_dir);
        self.project_name = Some(name);
        Ok(())
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

        // Paths are made relative to the workspace ROOT (tree paths are
        // project-root-relative). The WATCH itself still covers only `src/`:
        // watching the root would pull in `target/`, which churns constantly
        // during a build and would flood the channel.
        let workspace_root = std::env::temp_dir().join("embedded_ide_0_check");
        let workspace_src = workspace_root.join("src");

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
                        let Ok(rel) = abs.strip_prefix(&workspace_root) else {
                            continue;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if rel == "src/main.rs" {
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
                            // LF-normalized (phantom-gutter rule).
                            std::fs::read_to_string(abs)
                                .ok()
                                .map(|s| s.replace("\r\n", "\n"))
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
                        let Ok(rel) = abs.strip_prefix(&workspace_root) else {
                            continue;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        events.push((rel, FsEventKind::Remove));
                    }
                }
                Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                    let old = &event.paths[0];
                    let new = &event.paths[1];
                    let Ok(old_rel) = old.strip_prefix(&workspace_root) else {
                        continue;
                    };
                    let Ok(new_rel) = new.strip_prefix(&workspace_root) else {
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
    /// The directory git commands run in: the project root, or a library's own
    /// repository when the Git tab is pointed at one.
    pub(super) fn git_dir(&self) -> Option<std::path::PathBuf> {
        let root = self.project_dir.as_ref()?;
        Some(self.git.target.dir(root))
    }

    /// A path as git reported it — relative to the ACTIVE repo root — turned
    /// into the project-root-relative key the IDE's buffers and tree use.
    ///
    /// These are the same string only while the target is the project. Inside a
    /// library repo git says `src/lib.rs` where the IDE stores
    /// `mw_radar/src/lib.rs`, and using the raw value would look up a file that
    /// does not exist (silently reverting nothing).
    pub(super) fn git_path_to_project(&self, git_path: &str) -> String {
        format!("{}{}", self.git.target.prefix(), git_path)
    }

    /// Start a git operation on a worker thread (signal handler for the Git
    /// tab's buttons and the tree's context menu). Guards: needs a saved
    /// project (`project_dir`), no overlap with a running op, and no save in
    /// flight (git would read a half-written tree).
    pub(super) fn run_git_op(&mut self, op: crate::git::GitOp) {
        let Some(dir) = self.git_dir() else {
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
        // A switch/delete takes its target branch from the header picker; a new
        // branch from the text field. Both flow through `run_op`'s single
        // `branch` argument.
        let branch = if matches!(
            op,
            crate::git::GitOp::SwitchBranch | crate::git::GitOp::DeleteBranch
        ) {
            self.git.switch_target.take().unwrap_or_default()
        } else {
            self.git.branch_draft.trim().to_owned()
        };
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
            branch,
            add_paths,
            dir,
            crate::git::snapshot_for_target(self.git_disk_snapshot(), &self.git.target),
            // Mirror only when committing a LIBRARY and the option is on; the
            // project root is where the second commit runs.
            match (&self.git.target, self.git.mirror_to_project) {
                (crate::git::RepoTarget::Library(lib), true) => self
                    .project_dir
                    .clone()
                    .map(|root| (lib.clone(), root)),
                _ => None,
            },
            std::sync::Arc::clone(&self.git.state),
            std::sync::Arc::clone(&self.activity),
            self.egui_ctx.clone(),
        );
    }

    /// Start a "Clone from git" library import into `<project>/<dir>` — a plain
    /// clone (independent repo) or a submodule. The worker runs git + validates;
    /// `finish_clone_library` wires it in on completion.
    pub(super) fn start_clone_library(&mut self, url: String, dir: String, as_submodule: bool) {
        let Some(root) = self.project_dir.clone() else {
            return; // dialog shows the "save first" hint
        };
        if self.git.is_busy() {
            return;
        }
        crate::git::run_clone_library(
            url,
            dir,
            as_submodule,
            root,
            std::sync::Arc::clone(&self.git.state),
            self.egui_ctx.clone(),
        );
    }

    /// Wire a freshly-imported library into the project: scan its files into the
    /// tree, and (for an INDEPENDENT clone) gitignore it. It is deliberately NOT
    /// added to `[workspace] members` here — an external crate that can't resolve
    /// for the firmware's target would break `cargo metadata`, which rust-analyzer
    /// runs to load the WHOLE workspace, killing RA (no inline errors, no
    /// Structure edges, a stuck "Checking…"). The user promotes it explicitly via
    /// "Add to workspace" ([`add_detached_lib_to_workspace`]), which runs a
    /// `cargo metadata` pre-check first. Until then it shows as a DETACHED library.
    ///
    /// For an INDEPENDENT clone (`is_submodule == false`) it also GITIGNORES the
    /// folder — the clone is its own repo, and tracking its files would gitlink it
    /// and break a fresh checkout. A SUBMODULE is left tracked (git records it via
    /// `.gitmodules` + a pinned commit — that's the whole point).
    pub(super) fn finish_clone_library(&mut self, dir: String, is_submodule: bool) {
        let Some(root) = self.project_dir.clone() else {
            return;
        };
        if !is_submodule {
            let entry = format!("{dir}/");
            let already = self
                .gitignore
                .lines()
                .any(|l| matches!(l.trim(), t if t == dir || t == entry || t == format!("/{dir}")));
            if !already {
                if !self.gitignore.is_empty() && !self.gitignore.ends_with('\n') {
                    self.gitignore.push('\n');
                }
                self.gitignore.push_str(&format!("{entry}\n"));
            }
        }
        // Bring its files into the tree WITHOUT a full reload (preserves other
        // in-memory buffers); `load_from_dir` already treats members this way.
        self.project_tree.add_member_dir(&root, &dir);
        self.cached_project_files = None;
        self.request_save = true;
    }

    /// Start promoting the DETACHED library `dir` into the workspace as BOTH a
    /// `[workspace] member` AND a `[dependencies.<name>]` path dependency, so the
    /// firmware can actually `use` it. A cargo-metadata PRE-CHECK runs first
    /// (async), because an incompatible crate (its own workspace, conflicting
    /// deps, unresolvable path deps) would break `cargo metadata` for the WHOLE
    /// workspace and kill rust-analyzer. The change is applied only if that check
    /// passes ([`poll_workspace_add`]).
    pub(super) fn add_detached_lib_to_workspace(&mut self, dir: String) {
        let Some(root) = self.project_dir.clone() else {
            return;
        };
        if self.workspace_add.is_some() {
            return; // one check at a time
        }
        // The dependency table key is the crate's PACKAGE name (from its own
        // Cargo.toml), which can differ from the directory name — read it, and
        // fall back to the dir name if the manifest can't be parsed.
        let name = self
            .project_tree
            .user_src_files
            .iter()
            .find(|(p, _)| *p == format!("{dir}/Cargo.toml"))
            .and_then(|(_, c)| crate::git::parse_package_name(c))
            .unwrap_or_else(|| dir.clone());
        // The manifest we WOULD write. The worker trials it before we commit.
        let tentative =
            crate::project_tree::extract_crate::patch_root_manifest(&self.cargo_toml, &dir, &name);
        let state = std::sync::Arc::new(std::sync::Mutex::new(None));
        run_metadata_precheck(
            root,
            tentative.clone(),
            std::sync::Arc::clone(&state),
            self.egui_ctx.clone(),
        );
        self.workspace_add = Some(WorkspaceAdd { dir, tentative, state });
    }

    /// Consume a finished "Add to workspace" pre-check. On success the tentative
    /// manifest becomes the live one and a Save is requested (RA reloads with the
    /// member). On failure the cargo error is stored for the modal. Idempotent —
    /// a no-op until the worker posts a result.
    pub(super) fn poll_workspace_add(&mut self) {
        let done = self
            .workspace_add
            .as_ref()
            .and_then(|w| w.state.lock().unwrap().take());
        let Some(result) = done else {
            return;
        };
        let w = self.workspace_add.take().unwrap();
        match result {
            Ok(()) => {
                self.cargo_toml = w.tentative;
                self.cached_project_files = None;
                self.request_save = true;
                // The workspace gained a member — restart RA so it re-runs
                // `cargo metadata` cleanly and analyzes the new crate (a wedged
                // RA won't pick it up from a didChange alone).
                self.restart_lsp();
            }
            Err(e) => self.workspace_add_error = Some((w.dir, e)),
        }
    }

    /// Remove library `dir` from `[workspace] members` (and any path dependency)
    /// WITHOUT deleting its files — the inverse of "Add to workspace". Used both
    /// as the LIBRARIES "Detach" action and as the one-click recovery when an
    /// incompatible member has broken the workspace load.
    pub(super) fn detach_lib_from_workspace(&mut self, dir: &str) {
        self.cargo_toml =
            crate::project_tree::extract_crate::remove_workspace_member(&self.cargo_toml, dir);
        // A detach can only FIX a broken workspace, so clear the stale banner
        // now, then re-verify — the recheck re-flags it if something else is
        // still wrong.
        self.workspace_load_error = None;
        self.cached_project_files = None;
        self.request_save = true;
        // Removing a member changes the workspace graph — and if an incompatible
        // member had wedged RA, only a restart recovers it (a manifest edit alone
        // won't un-stick a dead analyzer, the reported "stuck Checking…").
        self.restart_lsp();
        self.recheck_workspace_health();
    }

    /// Restart rust-analyzer: `reset()` kills the child + bumps the generation +
    /// marks it `Stopped`; the LSP lifecycle in `init_frame` then re-writes the
    /// workspace and spawns a fresh RA next frame. The single entry point for
    /// the Analyzer-tab "Restart" button and every workspace-structure change.
    pub(super) fn restart_lsp(&mut self) {
        self.lsp_state.lock().unwrap().reset();
        self.lsp_selected_diagnostic = None;
    }

    /// Start a background `cargo metadata` HEALTH CHECK of the project exactly as
    /// it is on disk — the same load rust-analyzer performs. A failure means RA
    /// will not load either (no inline errors, no Structure edges, a stuck
    /// "Checking…"), so [`poll_workspace_health`] surfaces it as a banner rather
    /// than leaving the user with a silently-dead analyzer. Runs on open and
    /// after a detach; no-op without a saved project dir.
    pub(super) fn recheck_workspace_health(&mut self) {
        let Some(root) = self.project_dir.clone() else {
            self.workspace_load_error = None;
            return;
        };
        let state = std::sync::Arc::new(std::sync::Mutex::new(None));
        run_metadata_healthcheck(root, std::sync::Arc::clone(&state), self.egui_ctx.clone());
        self.workspace_health = Some(state);
    }

    /// Consume a finished health check: store the error (banner) or clear it.
    pub(super) fn poll_workspace_health(&mut self) {
        let done = self
            .workspace_health
            .as_ref()
            .and_then(|s| s.lock().unwrap().take());
        let Some(result) = done else {
            return;
        };
        self.workspace_health = None;
        self.workspace_load_error = result.err();
    }
}

/// In-flight state for the "Add to workspace" cargo-metadata pre-check.
pub(super) struct WorkspaceAdd {
    /// The library directory being promoted (project-root-relative).
    pub dir: String,
    /// The manifest to commit if the check passes.
    pub tentative: String,
    /// `None` while the worker runs; `Some(Ok/Err)` once it finishes.
    pub state: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
}

/// Run `cargo metadata` against a TRIAL manifest to see whether adding a member
/// keeps the workspace loadable — the same resolution rust-analyzer does on
/// load. Runs on a worker thread; posts `Ok(())` / `Err(cargo error)` into
/// `state`. RA-safe: RA watches the TEMP check workspace, never the project dir,
/// so briefly swapping the project's `Cargo.toml` here can't disturb it. The
/// original `Cargo.toml` (and `Cargo.lock`) are restored on every exit path.
fn run_metadata_precheck(
    project_dir: std::path::PathBuf,
    tentative: String,
    state: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
    ctx: eframe::egui::Context,
) {
    use std::process::Command;
    std::thread::spawn(move || {
        let manifest = project_dir.join("Cargo.toml");
        let lock = project_dir.join("Cargo.lock");
        let orig_toml = std::fs::read(&manifest).ok();
        let orig_lock = std::fs::read(&lock).ok();

        let result = (|| -> Result<(), String> {
            std::fs::write(&manifest, tentative.as_bytes())
                .map_err(|e| format!("could not write trial Cargo.toml: {e}"))?;
            let mut cmd = Command::new("cargo");
            crate::build::no_window(&mut cmd)
                .args(["metadata", "--format-version", "1"])
                .current_dir(&project_dir)
                // A git path-dep could otherwise open a terminal credential
                // prompt that hangs a headless process forever.
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(std::process::Stdio::null());
            let out = cmd
                .output()
                .map_err(|e| format!("could not run cargo: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(clean_cargo_error(&String::from_utf8_lossy(&out.stderr)))
            }
        })();

        // Restore the project's manifest + lock no matter what.
        match orig_toml {
            Some(o) => {
                let _ = std::fs::write(&manifest, o);
            }
            None => {
                let _ = std::fs::remove_file(&manifest);
            }
        }
        match orig_lock {
            Some(o) => {
                let _ = std::fs::write(&lock, o);
            }
            None => {
                let _ = std::fs::remove_file(&lock);
            }
        }

        *state.lock().unwrap() = Some(result);
        ctx.request_repaint();
    });
}

/// Run `cargo metadata` on the project AS-IS (no modification) to check whether
/// the workspace still loads — the resolution rust-analyzer does on start. Posts
/// `Ok(())` / `Err(cargo error)` into `state`. Read-only: unlike the pre-check it
/// never writes the manifest, so it is safe to fire on project open.
fn run_metadata_healthcheck(
    project_dir: std::path::PathBuf,
    state: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
    ctx: eframe::egui::Context,
) {
    use std::process::Command;
    std::thread::spawn(move || {
        // No manifest → nothing to load (e.g. a chip with no export). Treat as
        // healthy so we never show a spurious banner.
        if !project_dir.join("Cargo.toml").is_file() {
            *state.lock().unwrap() = Some(Ok(()));
            ctx.request_repaint();
            return;
        }
        let mut cmd = Command::new("cargo");
        crate::build::no_window(&mut cmd)
            .args(["metadata", "--format-version", "1"])
            .current_dir(&project_dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null());
        let result = match cmd.output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(clean_cargo_error(&String::from_utf8_lossy(&out.stderr))),
            // cargo missing → the health check is meaningless, not a load error.
            Err(_) => Ok(()),
        };
        *state.lock().unwrap() = Some(result);
        ctx.request_repaint();
    });
}

/// Trim cargo's `error:`/`Caused by:` stderr into a short, readable reason.
/// Keeps the first `error:` line and the most specific `Caused by:` tail, so the
/// modal shows "failed to select a version…" not the whole backtrace.
fn clean_cargo_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return "cargo metadata failed (no output)".to_owned();
    }
    let head = lines
        .iter()
        .find(|l| l.starts_with("error"))
        .copied()
        .unwrap_or(lines[0]);
    // The LAST `Caused by:` entry is usually the root cause.
    let cause = lines
        .iter()
        .rposition(|l| l.starts_with("Caused by:"))
        .and_then(|i| lines.get(i + 1))
        .copied();
    match cause {
        Some(c) if c != head => format!("{head}\n{c}"),
        _ => head.to_owned(),
    }
}

impl AppIde {
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
            snap.push((rel.clone(), content.clone()));
        }
        snap
    }

    /// Project-relative paths whose in-memory content differs from disk — what
    /// closing the app right now would lose. Empty = everything is saved.
    ///
    /// For a project that was NEVER saved there is nothing to diff against, so
    /// it counts as unsaved only when the user actually built something
    /// (configured a pin, added a module or a source file). Otherwise a
    /// pristine start-up state would nag on every exit.
    pub(super) fn unsaved_files(&self) -> Vec<String> {
        let snapshot = self.git_disk_snapshot();
        match &self.project_dir {
            Some(dir) => crate::git::unsaved_changes(dir, &snapshot),
            None => {
                use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
                let has_content = !self.project_tree.user_src_files.is_empty()
                    || self.mcu.as_ref().is_some_and(|m| {
                        !m.modules.is_empty()
                            || m.iter_all_pins()
                                .any(|p| p.selected_function != PinFunction::Unset)
                    });
                if has_content {
                    snapshot.into_iter().map(|(p, _)| p).collect()
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Reverse ONE hunk of a changed file — the Git diff view's per-hunk revert
    /// (Phase B). Reconstructs that hunk's patch from the open diff and applies
    /// it in reverse on disk, then refreshes the file's in-memory buffer so the
    /// next Save doesn't clobber it. Returns true when a buffer changed (the
    /// caller then refreshes the editor's `display_code`). IDE-managed files are
    /// refused (main.rs is better reverted hunk-by-hunk in the editor gutter).
    pub(super) fn apply_hunk_revert(&mut self, path: &str, hunk_row: usize) -> bool {
        let Some(dir) = self.git_dir() else {
            return false;
        };
        // `path` came from git and is relative to the ACTIVE repo; the tree and
        // the IDE-managed rules are keyed from the project root.
        let key = self.git_path_to_project(path);
        if crate::app::tabs::git_tab::is_ide_managed(&key) {
            self.git.state.lock().unwrap().lines.push((
                crate::git::GitLine::Notice,
                format!("[skip] {path} is IDE-managed — change it via the UI (for main.rs, use the editor gutter to revert hunks)"),
            ));
            return false;
        }
        // Build the single-hunk patch from the currently-open diff.
        let patch = {
            let st = self.git.state.lock().unwrap();
            match &st.diff {
                Some(d) if d.path == path => crate::git::hunk_patch(path, &d.rows, hunk_row),
                _ => None,
            }
        };
        let Some(patch) = patch else {
            self.git.state.lock().unwrap().lines.push((
                crate::git::GitLine::Notice,
                format!("[error] couldn't build the revert patch for {path}"),
            ));
            return false;
        };

        match crate::git::apply_reverse_patch(&dir, &patch) {
            Ok(()) => {
                // Refresh the in-memory buffer from the now-reverted disk file.
                // Only user files reach here (managed ones are refused), and
                // `path` is already project-root-relative — the same key the
                // tree uses.
                let mut changed = false;
                if let Ok(disk) = std::fs::read_to_string(dir.join(path)) {
                    let disk = disk.replace("\r\n", "\n");
                    if let Some(entry) = self
                        .project_tree
                        .user_src_files
                        .iter_mut()
                        .find(|(p, _)| *p == key)
                    {
                        entry.1 = disk;
                        changed = true;
                    }
                }
                {
                    let mut st = self.git.state.lock().unwrap();
                    st.op_gen += 1; // refresh the editor gutter's HEAD baseline marks
                    st.lines
                        .push((crate::git::GitLine::Notice, format!("reverted a hunk in {path}")));
                }
                // Re-open the diff so the remaining hunks show (or it closes if
                // the file now matches HEAD).
                crate::git::run_diff(
                    path.to_owned(),
                    false,
                    dir,
                    std::sync::Arc::clone(&self.git.state),
                    self.egui_ctx.clone(),
                );
                changed
            }
            Err(e) => {
                self.git.state.lock().unwrap().lines.push((
                    crate::git::GitLine::Notice,
                    format!("[error] git apply --reverse failed: {e}"),
                ));
                false
            }
        }
    }

    /// Discard a whole file's changes — Phase A. A TRACKED file is restored to
    /// its HEAD version (disk + in-memory buffer); an UNTRACKED file is deleted
    /// (disk + buffer + tree selection remap). Returns true when the open file's
    /// buffer was updated in place (the caller then refreshes `display_code`).
    /// IDE-managed files are refused; the caller confirms via a dialog first.
    /// Restore ONE file to its content at `sha` (History view).
    ///
    /// Scoped on purpose: only this file's buffer is refreshed, so unsaved
    /// edits elsewhere survive and no project reload is needed. HEAD does not
    /// move — the result is an ordinary uncommitted change, visible in the
    /// Changes view and undoable with Discard.
    ///
    /// Returns `true` when a buffer changed, so the caller can refresh the
    /// editor's working copy (otherwise the end-of-frame write-back would put
    /// the old text straight back).
    pub(super) fn apply_restore_from_commit(&mut self, sha: &str, path: &str) -> bool {
        let Some(dir) = self.git_dir() else {
            return false;
        };
        // git reports paths from the ACTIVE repo root; buffers are keyed from
        // the project root.
        let key = self.git_path_to_project(path);
        if crate::app::tabs::git_tab::is_ide_managed(&key) {
            self.git_note(format!(
                "[skip] {path} is IDE-managed — it is regenerated from the MCU configuration"
            ));
            return false;
        }
        let content = match crate::git::restore_file_at(&dir, sha, path) {
            Ok(c) => c,
            Err(e) => {
                self.git_note(format!("[error] restore {path}: {e}"));
                return false;
            }
        };

        // Update the in-memory buffer — and ADD the file when it isn't tracked
        // any more (it was deleted after `sha`). Writing it to disk WITHOUT
        // registering it would let `write_project`'s stale-file prune delete it
        // again at the next save.
        if let Some(e) = self
            .project_tree
            .user_src_files
            .iter_mut()
            .find(|(p, _)| *p == key)
        {
            e.1 = content;
        } else {
            self.project_tree.user_src_files.push((key.clone(), content));
        }
        self.cached_project_files = None;
        {
            let mut st = self.git.state.lock().unwrap();
            st.op_gen += 1; // refresh the editor gutter's HEAD baseline marks
            st.lines.push((
                crate::git::GitLine::Notice,
                format!("[ok] restored {path} from {}", &sha[..sha.len().min(7)]),
            ));
        }
        self.request_save = true;
        true
    }

    pub(super) fn apply_discard_file(&mut self, path: &str) -> bool {
        let Some(dir) = self.git_dir() else {
            return false;
        };
        let key = self.git_path_to_project(path);
        if crate::app::tabs::git_tab::is_ide_managed(&key) {
            self.git_note(format!(
                "[skip] {path} is IDE-managed — change it via the UI (for main.rs, use the editor gutter)"
            ));
            return false;
        }
        let untracked = self
            .git
            .state
            .lock()
            .unwrap()
            .status
            .changes
            .iter()
            .any(|c| c.path == path && c.code == "??");

        let changed = if untracked {
            // Untracked → delete the file (irreversible) + drop its buffer/tree.
            match std::fs::remove_file(dir.join(path)) {
                Ok(()) => {
                    {
                        if let Some(idx) = self
                            .project_tree
                            .user_src_files
                            .iter()
                            .position(|(p, _)| *p == key)
                        {
                            self.project_tree.user_src_files.remove(idx);
                            // Remap the index-based selection across the removal.
                            if let ProjectFileId::UserFile(sel) = self.selected_file {
                                self.selected_file = if sel == idx {
                                    ProjectFileId::MainRs
                                } else if sel > idx {
                                    ProjectFileId::UserFile(sel - 1)
                                } else {
                                    ProjectFileId::UserFile(sel)
                                };
                            }
                        }
                    }
                    self.git_note(format!("Deleted untracked {path}"));
                    false
                }
                Err(e) => {
                    self.git_note(format!("[error] couldn't delete {path}: {e}"));
                    false
                }
            }
        } else {
            // Tracked → restore its HEAD version on disk + refresh the buffer.
            match crate::git::restore_file_to_head(&dir, path) {
                Ok(content) => {
                    let mut c = false;
                    {
                        if let Some(entry) = self
                            .project_tree
                            .user_src_files
                            .iter_mut()
                            .find(|(p, _)| *p == key)
                        {
                            entry.1 = content;
                            c = true;
                        }
                    }
                    self.git_note(format!("Restored {path} to HEAD"));
                    c
                }
                Err(e) => {
                    self.git_note(format!("[error] couldn't discard {path}: {e}"));
                    false
                }
            }
        };

        self.git.state.lock().unwrap().op_gen += 1; // refresh gutter baseline
        self.run_git_op(crate::git::GitOp::Refresh); // status + clears the diff
        changed
    }

    /// Discard ALL uncommitted changes back to HEAD — Phase C. `git reset
    /// --hard` + `git clean -fd`, then RELOAD the whole project from disk
    /// (`load_project_from_dir` re-reads main.rs/pins, config files, user files,
    /// mcu.config) so every in-memory buffer matches the reset tree. The caller
    /// confirms first and gates on `has_commits`.
    pub(super) fn apply_discard_all(&mut self) {
        let Some(dir) = self.git_dir() else {
            return;
        };
        match crate::git::discard_all_to_head(&dir) {
            Ok(()) => {
                // Rebuild every buffer from the now-reset disk. Reload the
                // PROJECT, never `dir`: when a LIBRARY repo is the selected
                // target, `dir` is the library's folder — loading THAT as the
                // project would repoint `project_dir` at the library and
                // re-detect the chip from its manifest (the reported "Discard
                // all doesn't work for the selected repo"). The project reload
                // rescans src/ + members + detached libs, so a discarded
                // library's files refresh either way.
                let reload = self.project_dir.clone().unwrap_or(dir);
                self.load_project_from_dir(&reload);
                self.git_note("Discarded all changes (reset --hard + clean)".into());
            }
            Err(e) => self.git_note(format!("[error] discard-all failed: {e}")),
        }
        self.git.state.lock().unwrap().op_gen += 1; // refresh gutter baseline
        self.run_git_op(crate::git::GitOp::Refresh);
    }

    /// Push a one-off notice line into the Git tab's output scrollback.
    fn git_note(&self, msg: String) {
        self.git
            .state
            .lock()
            .unwrap()
            .lines
            .push((crate::git::GitLine::Notice, msg));
    }
}

/// Validate a project FOLDER name for Windows: non-empty, no reserved
/// characters, no trailing dot/space, not a reserved device name.
pub(super) fn valid_project_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if n.chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20)
    {
        return Err(r#"Name cannot contain < > : " / \ | ? *"#.into());
    }
    if n.ends_with('.') || n.ends_with(' ') {
        return Err("Name cannot end with a dot or a space".into());
    }
    let stem = n.split('.').next().unwrap_or(n).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        return Err(format!("\"{n}\" is a reserved Windows name"));
    }
    Ok(())
}

/// Rename `old_dir`'s LEAF to `new_name` (same parent → same drive, so
/// `fs::rename` always applies). Returns the new path. A case-only change is
/// allowed (the `exists()` collision check would false-positive on Windows'
/// case-insensitive filesystem); renaming to the identical name is a no-op.
pub(super) fn rename_project_dir(
    old_dir: &std::path::Path,
    new_name: &str,
) -> Result<std::path::PathBuf, String> {
    let new_name = new_name.trim();
    valid_project_name(new_name)?;
    let parent = old_dir
        .parent()
        .ok_or("Project folder has no parent directory")?;
    let new_dir = parent.join(new_name);
    let old_leaf = old_dir
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    if old_leaf == new_name {
        return Ok(old_dir.to_path_buf()); // unchanged
    }
    let case_only = old_leaf.eq_ignore_ascii_case(new_name);
    if !case_only && new_dir.exists() {
        return Err(format!("\"{new_name}\" already exists here"));
    }
    std::fs::rename(old_dir, &new_dir).map_err(|e| format!("Rename failed: {e}"))?;
    Ok(new_dir)
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
mod rename_project_tests {
    use super::{rename_project_dir, valid_project_name};

    #[test]
    fn name_validation() {
        assert!(valid_project_name("my_project-2").is_ok());
        assert!(valid_project_name("  ").is_err(), "empty");
        assert!(valid_project_name("a/b").is_err(), "path separator");
        assert!(valid_project_name("a:b").is_err(), "colon");
        assert!(valid_project_name("done.").is_err(), "trailing dot");
        assert!(valid_project_name("CON").is_err(), "reserved device");
        assert!(valid_project_name("com3").is_err(), "reserved device");
        assert!(valid_project_name("COMET").is_ok(), "COM prefix but not COMn");
    }

    #[test]
    fn renames_leaf_and_reports_collisions() {
        let base = std::env::temp_dir().join(format!("eide_rename_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("proj_a");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("marker.txt"), "x").unwrap();

        // Plain rename moves the folder and its contents.
        let newp = rename_project_dir(&old, "proj_b").unwrap();
        assert_eq!(newp, base.join("proj_b"));
        assert!(newp.join("marker.txt").exists());
        assert!(!old.exists());

        // Renaming to an existing sibling is refused.
        std::fs::create_dir_all(base.join("proj_c")).unwrap();
        assert!(rename_project_dir(&newp, "proj_c").is_err());

        // Same name → no-op; case-only change is allowed on Windows.
        assert_eq!(rename_project_dir(&newp, "proj_b").unwrap(), newp);
        let cased = rename_project_dir(&newp, "Proj_B").unwrap();
        assert!(cased.join("marker.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
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
