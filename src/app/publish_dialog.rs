//! "Publish…" on a library crate: check, rehearse, then upload.
//!
//! Four things in one window, because they are one question:
//!
//! * what the registry would refuse, checked HERE rather than delegated to
//!   `cargo publish --dry-run`, which only warns about the two fields that
//!   actually get an upload rejected (see [`crate::publish`]);
//! * the missing `[package]` fields, editable in place — the extract-crate
//!   dialog is create-only, so until now the only way to fill in `repository`
//!   or a blank `description` was to hand-edit the manifest in the tree;
//! * where it goes and with what credentials (see
//!   [`crate::publish_target`] for why a typed index URL travels in the
//!   environment and never on the command line);
//! * `cargo publish`, streamed live — as a rehearsal first, and then for real.
//!
//! The upload is the one irreversible thing this IDE does: a published version
//! can never be overwritten and its code cannot be deleted, only yanked. So it
//! is gated on the crate's own name being typed, on every blocker being clear,
//! and on the project being saved — cargo packages what is on disk, and
//! publishing a stale file cannot be taken back.

use super::AppIde;
use crate::publish::{self, Severity};
use crate::publish_target::{self, Target};
use crate::terminal::{LineKind, TerminalState};
use eframe::egui;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// The open dialog. `None` on `AppIde` means closed.
pub(super) struct PublishDialog {
    /// Project-root-relative directory of the library crate.
    pub dir: String,
    /// Draft values for the `[package]` fields, keyed by the same names as
    /// [`publish::EDITABLE_FIELDS`]. Seeded from the manifest on open.
    pub fields: Vec<(&'static str, String)>,
    /// Live output of the dry run.
    pub log: Arc<Mutex<TerminalState>>,
    pub stop: Arc<AtomicBool>,
    /// Set while a cargo run is in flight, so the buttons disable themselves.
    pub running: Arc<AtomicBool>,
    /// The cargo child, so closing the window can kill it. Without this the
    /// process outlived the dialog: `stop` only makes the readers drop their
    /// pipes, and an upload kept going with nowhere to report.
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub error: Option<String>,

    // ── Phase B: where, with what credentials, and are you sure ─────────────
    /// Registries offered in the picker: crates.io, whatever the user's own
    /// cargo config already defines, and a custom index.
    pub targets: Vec<Target>,
    pub target: usize,
    /// The custom index URL, kept even while another target is selected so
    /// flipping back does not lose it.
    pub custom_index: String,
    /// Paste a token, or leave `use_saved_token` on and pass none at all.
    ///
    /// Held ONLY here, for the life of this window. Never written to disk: the
    /// IDE's existing secret store (the AI key) is plain text in the config
    /// folder, which is not a bar a publishing credential should clear.
    pub token: String,
    pub show_token: bool,
    /// Let cargo use the credentials it already has (`cargo login`).
    pub use_saved_token: bool,
    /// Typed confirmation. A publish cannot be undone, so the crate's own name
    /// has to be typed - a checkbox is too easy to click past.
    pub confirm: String,
}

impl PublishDialog {
    pub fn new(dir: String, manifest: &str, targets: Vec<Target>) -> Self {
        // An inherited field has no literal value to edit here - its text
        // lives in `[workspace.package]`. Seeding the box with the sentinel
        // let the user type into it and write that sentence into the manifest.
        let fields = publish::EDITABLE_FIELDS
            .iter()
            .map(|(key, _)| {
                let v = publish::package_field(manifest, key)
                    .filter(|_| !publish::is_workspace_inherited(manifest, key))
                    .unwrap_or_default();
                (*key, v)
            })
            .collect();
        Self {
            dir,
            fields,
            log: Arc::new(Mutex::new(TerminalState::default())),
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            targets,
            target: 0,
            custom_index: String::new(),
            token: String::new(),
            show_token: false,
            use_saved_token: true,
            confirm: String::new(),
            error: None,
        }
    }
}

impl AppIde {
    /// The registries to offer: crates.io, every one the user's own cargo
    /// config already defines, and a custom index.
    ///
    /// Reading their config is what makes the picker useful rather than a
    /// guess at a vendor list — and a registry cargo already knows needs no
    /// index from us at all, just its name.
    pub(super) fn publish_targets(&self) -> Vec<Target> {
        let mut out = vec![Target::CratesIo];
        let mut configs: Vec<std::path::PathBuf> = Vec::new();
        // The user's cargo home, honouring CARGO_HOME.
        if let Some(home) = std::env::var_os("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join(".cargo"))
            })
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cargo"))
            })
        {
            configs.push(home.join("config.toml"));
            // Cargo still reads the extension-less name.
            configs.push(home.join("config"));
        }
        // …and the project's own, which is where a per-project registry lives.
        if let Some(root) = &self.project_dir {
            configs.push(root.join(".cargo").join("config.toml"));
        }
        for path in configs {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (name, index) in crate::publish_target::parse_registries(&text) {
                if !out
                    .iter()
                    .any(|t| matches!(t, Target::Configured { name: n, .. } if *n == name))
                {
                    out.push(Target::Configured { name, index });
                }
            }
        }
        out.push(Target::Custom {
            index: String::new(),
        });
        out
    }

    /// The library's `Cargo.toml`, project-root-relative.
    fn lib_manifest_path(dir: &str) -> String {
        format!("{dir}/Cargo.toml")
    }

    /// Read a library's manifest out of the in-memory tree.
    fn lib_manifest(&self, dir: &str) -> Option<String> {
        let path = Self::lib_manifest_path(dir);
        self.project_tree
            .user_src_files
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, c)| c.clone())
    }

    pub(super) fn show_publish_dialog(&mut self, ui: &egui::Ui) {
        let Some(dir) = self.move_or_publish_dir() else {
            return;
        };
        let Some(manifest) = self.lib_manifest(&dir) else {
            // The crate lost its manifest while the window was open.
            self.publish_dialog = None;
            return;
        };
        // Which siblings are already on a registry is not knowable offline, so
        // every path dependency is reported as a caution naming the crate.
        let findings = publish::check_manifest(&manifest, |_| None);
        let blockers = findings
            .iter()
            .filter(|f| f.severity == Severity::Blocker)
            .count();

        let mut close = false;
        let mut apply: Option<(&'static str, String)> = None;
        let mut start_dry_run: Option<(Target, Option<String>)> = None;
        let mut start_publish: Option<(Target, Option<String>)> = None;

        egui::Window::new(format!("Publish {dir}"))
            .id(egui::Id::new("publish_dialog"))
            .collapsible(false)
            .resizable(true)
            .default_size([620.0, 520.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                let dlg = self.publish_dialog.as_mut().expect("checked above");

                ui.label(
                    egui::RichText::new(
                        "Check, rehearse, then upload. The upload at the bottom is permanent.",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_gray(150)),
                );
                ui.add_space(8.0);

                // ── What would be refused ───────────────────────────────────
                ui.label(egui::RichText::new("Before publishing").size(11.5).strong());
                ui.add_space(4.0);
                if findings.is_empty() {
                    ui.label(
                        egui::RichText::new("Nothing missing.")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(140, 200, 150)),
                    );
                }
                for f in &findings {
                    let (icon, color) = match f.severity {
                        Severity::Blocker => (
                            egui_phosphor::regular::X_CIRCLE,
                            egui::Color32::from_rgb(220, 90, 80),
                        ),
                        Severity::Advice => (
                            egui_phosphor::regular::WARNING,
                            egui::Color32::from_rgb(220, 180, 60),
                        ),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(icon).size(11.0).color(color));
                        ui.label(egui::RichText::new(&f.message).size(10.5));
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                // ── The metadata, editable here ─────────────────────────────
                ui.label(egui::RichText::new("Package metadata").size(11.5).strong())
                    .on_hover_text(
                        "Written straight into the library's Cargo.toml. The crate-creation \
                     dialog only ever writes five fields and never reopens, so this is \
                     where the rest get filled in.",
                    );
                ui.add_space(4.0);
                egui::Grid::new("publish_fields")
                    .num_columns(3)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        for (i, (key, hint)) in publish::EDITABLE_FIELDS.iter().enumerate() {
                            let required = i < 2;
                            let label = if required {
                                egui::RichText::new(format!("{key} *"))
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(230, 190, 120))
                            } else {
                                egui::RichText::new(*key).size(11.0)
                            };
                            ui.label(label).on_hover_text(*hint);
                            let value = &mut dlg.fields[i].1;
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .desired_width(330.0)
                                    .hint_text(*hint),
                            );
                            let inherited = publish::is_workspace_inherited(&manifest, key);
                            let stored = publish::package_field(&manifest, key)
                                .filter(|_| !inherited)
                                .unwrap_or_default();
                            let changed = stored != *value;
                            if ui
                                .add_enabled(
                                    changed && !value.trim().is_empty() && !inherited,
                                    egui::Button::new(egui::RichText::new("Write").size(10.5)),
                                )
                                .on_hover_text("Save this field into the library's Cargo.toml")
                                .on_disabled_hover_text(if inherited {
                                    "Inherited from [workspace.package] - change it there"
                                } else if changed {
                                    "Type a value first"
                                } else {
                                    "Already what the manifest says"
                                })
                                .clicked()
                            {
                                apply = Some((key, value.clone()));
                            }
                            ui.end_row();
                        }
                    });
                ui.label(
                    egui::RichText::new("* required by crates.io")
                        .size(9.5)
                        .color(egui::Color32::from_gray(140)),
                );

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                // ── Where it goes ───────────────────────────────────────────
                ui.label(egui::RichText::new("Registry").size(11.5).strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("publish_target")
                        .selected_text(
                            egui::RichText::new(dlg.targets[dlg.target].label()).size(11.0),
                        )
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for (i, t) in dlg.targets.iter().enumerate() {
                                ui.selectable_value(
                                    &mut dlg.target,
                                    i,
                                    egui::RichText::new(t.label()).size(11.0),
                                );
                            }
                        });
                    if let Target::Configured { index, .. } = &dlg.targets[dlg.target] {
                        ui.label(
                            egui::RichText::new(index)
                                .size(10.0)
                                .monospace()
                                .color(egui::Color32::from_gray(150)),
                        );
                    }
                });
                if matches!(dlg.targets[dlg.target], Target::Custom { .. }) {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Index URL").size(11.0));
                        ui.add(
                            egui::TextEdit::singleline(&mut dlg.custom_index)
                                .desired_width(400.0)
                                .hint_text("sparse+https://…/index/"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Templates:")
                                .size(10.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                        for (name, url) in publish_target::INDEX_TEMPLATES {
                            if ui
                                .add(egui::Button::new(egui::RichText::new(*name).size(10.0)).frame(false))
                                .on_hover_text(*url)
                                .clicked()
                            {
                                dlg.custom_index = (*url).to_owned();
                            }
                        }
                    });
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Credentials").size(11.5).strong());
                ui.add_space(4.0);
                ui.checkbox(
                    &mut dlg.use_saved_token,
                    egui::RichText::new("Use the credentials cargo already has").size(11.0),
                )
                .on_hover_text(
                    "Passes no token at all - cargo reads the one `cargo login` stored. Untick to paste a token for this publish only; it is kept in memory and never written to disk.",
                );
                if !dlg.use_saved_token {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Token").size(11.0));
                        ui.add(
                            egui::TextEdit::singleline(&mut dlg.token)
                                .desired_width(360.0)
                                .password(!dlg.show_token),
                        );
                        ui.checkbox(&mut dlg.show_token, egui::RichText::new("show").size(10.0));
                    });
                    dlg.token.retain(|c| !c.is_whitespace());
                    ui.label(
                        egui::RichText::new(
                            "Passed to cargo through the environment, never as a command-line argument - an argument would be readable in the process list.",
                        )
                        .size(9.5)
                        .color(egui::Color32::from_gray(140)),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                let crate_name = dir.rsplit('/').next().unwrap_or(&dir).to_owned();
                let token = (!dlg.use_saved_token).then(|| dlg.token.clone());
                let chosen = match &dlg.targets[dlg.target] {
                    Target::Custom { .. } => Target::Custom {
                        index: dlg.custom_index.clone(),
                    },
                    other => other.clone(),
                };
                let target_problem = publish_target::target_blocker(&chosen, token.as_deref());
                let confirmed = dlg.confirm.trim() == crate_name;

                // ── The rehearsal ───────────────────────────────────────────
                let busy = dlg.running.load(std::sync::atomic::Ordering::Relaxed);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(
                                egui::RichText::new("Run cargo publish --dry-run").size(11.5),
                            ),
                        )
                        .on_hover_text(
                            "Packages and builds the crate exactly as a publish would, and \
                             stops before the upload. It does NOT check the two fields \
                             crates.io rejects on - that is what the list above is for.",
                        )
                        .on_disabled_hover_text("Already running")
                        .clicked()
                    {
                        // The rehearsal uses the SELECTED registry, not
                        // crates.io: a dry run against a different index is a
                        // rehearsal of a different publish.
                        start_dry_run = Some((chosen.clone(), token.clone()));
                    }
                    if busy {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new("Close").size(11.5)).clicked() {
                            close = true;
                        }
                    });
                });
                // ── The real thing ──────────────────────────────────────────
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Publish for real")
                        .size(11.5)
                        .strong()
                        .color(egui::Color32::from_rgb(230, 150, 120)),
                );
                ui.label(
                    egui::RichText::new(
                        "A publish is permanent: the version can never be overwritten and the code cannot be deleted. Yanking stops NEW dependents, it does not remove anything.",
                    )
                    .size(10.0)
                    .color(egui::Color32::from_gray(150)),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Type `{crate_name}` to confirm")).size(11.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut dlg.confirm)
                            .desired_width(200.0)
                            .hint_text(&crate_name),
                    );
                });
                ui.horizontal(|ui| {
                    let ready = !busy && blockers == 0 && target_problem.is_none() && confirmed;
                    let why = if busy {
                        "Already running"
                    } else if blockers > 0 {
                        "Fix the blockers listed above first"
                    } else if let Some(p) = &target_problem {
                        p.as_str()
                    } else {
                        "Type the crate name to confirm"
                    };
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(
                                egui::RichText::new(format!("Publish {crate_name}"))
                                    .size(11.5)
                                    .color(egui::Color32::from_rgb(240, 200, 190)),
                            )
                            .fill(egui::Color32::from_rgb(120, 50, 45)),
                        )
                        .on_hover_text("Uploads the crate. This cannot be undone.")
                        .on_disabled_hover_text(why)
                        .clicked()
                    {
                        start_publish = Some((chosen.clone(), token.clone()));
                    }
                    if !confirmed && !dlg.confirm.trim().is_empty() {
                        ui.label(
                            egui::RichText::new("that is not the crate name")
                                .size(10.0)
                                .color(egui::Color32::from_rgb(220, 150, 120)),
                        );
                    }
                });

                if blockers > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{blockers} blocker(s) above would still make the real upload fail."
                        ))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(220, 150, 120)),
                    );
                }

                if let Some(e) = &dlg.error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(e)
                            .size(10.5)
                            .color(egui::Color32::from_rgb(230, 130, 115)),
                    );
                }

                ui.add_space(6.0);
                let log = Arc::clone(&dlg.log);
                egui::ScrollArea::vertical()
                    .id_salt("publish_log")
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let guard = log.lock().unwrap();
                        for line in &guard.lines {
                            // A terminal line is coloured SPANS (cargo's ANSI
                            // is parsed on the way in); the kind's colour is
                            // the fallback for a span that carries none.
                            let fallback = match line.kind {
                                LineKind::Input => egui::Color32::from_rgb(140, 190, 240),
                                LineKind::Stderr => egui::Color32::from_rgb(220, 150, 120),
                                LineKind::Notice => egui::Color32::from_rgb(150, 200, 150),
                                LineKind::Stdout => egui::Color32::from_gray(190),
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (text, color) in &line.spans {
                                    ui.label(
                                        egui::RichText::new(text)
                                            .size(10.5)
                                            .monospace()
                                            .color(color.unwrap_or(fallback)),
                                    );
                                }
                            });
                        }
                    });
            });

        // Applied outside the window closure: both borrow `self`.
        if let Some((key, value)) = apply {
            let path = Self::lib_manifest_path(&dir);
            // `None` = the manifest is not parseable right now (half-typed in
            // the editor). Refused rather than written over.
            let Some(updated) = publish::set_package_field(&manifest, key, &value) else {
                if let Some(d) = &mut self.publish_dialog {
                    d.error = Some(format!(
                        "`{path}` is not valid TOML at the moment - fix it in the editor first."
                    ));
                }
                return;
            };
            match self
                .project_tree
                .user_src_files
                .iter_mut()
                .find(|(p, _)| *p == path)
            {
                Some(entry) => {
                    entry.1 = updated;
                    self.workspace_write_requested = true;
                    if let Some(d) = &mut self.publish_dialog {
                        d.error = None;
                    }
                }
                None => {
                    if let Some(d) = &mut self.publish_dialog {
                        d.error = Some(format!("`{path}` is no longer in the project."));
                    }
                }
            }
        }
        if let Some((target, token)) = start_dry_run {
            self.start_cargo_publish(&dir, target, token, true, ui.ctx());
        }
        if let Some((target, token)) = start_publish {
            self.start_cargo_publish(&dir, target, token, false, ui.ctx());
        }
        if close {
            if let Some(d) = &self.publish_dialog {
                d.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                // Kill it, do not merely stop reading it: an upload left
                // running has nowhere to report and cannot be cancelled.
                if let Some(mut c) = d.child.lock().unwrap().take() {
                    let _ = c.kill();
                }
            }
            self.publish_dialog = None;
        }
    }

    /// The library directory the publish dialog is open on.
    fn move_or_publish_dir(&self) -> Option<String> {
        self.publish_dialog.as_ref().map(|d| d.dir.clone())
    }

    /// Launch cargo on the library, streaming into the dialog's log.
    ///
    /// One function for both the rehearsal and the real thing, because the only
    /// differences are the flags and the environment — and keeping them in one
    /// place is what makes it impossible for the real publish to pick up
    /// `--allow-dirty` by accident (see [`publish_target::publish_command`]).
    ///
    /// Runs against the SAVED project folder, not the scratch workspace: cargo
    /// packages what is on disk, and the scratch copy carries no `.git`, which
    /// would silently change the dirty-tree answer.
    fn start_cargo_publish(
        &mut self,
        dir: &str,
        target: Target,
        token: Option<String>,
        dry_run: bool,
        ctx: &egui::Context,
    ) {
        let Some(root) = self.project_dir.clone() else {
            if let Some(d) = &mut self.publish_dialog {
                d.error =
                    Some("Save the project first - cargo packages what is on disk.".to_owned());
            }
            return;
        };
        // Unsaved edits are in the buffers, not on disk, and cargo reads disk.
        // Publishing a stale file is not a mistake that can be taken back.
        if !dry_run && !self.unsaved_files().is_empty() {
            if let Some(d) = &mut self.publish_dialog {
                d.error = Some(
                    "Save the project first - cargo would package the last saved text.".to_owned(),
                );
            }
            return;
        }
        let Some(d) = &self.publish_dialog else {
            return;
        };
        let (log, stop, running, child_slot) = (
            Arc::clone(&d.log),
            Arc::clone(&d.stop),
            Arc::clone(&d.running),
            Arc::clone(&d.child),
        );
        let (args, env) = publish_target::publish_command(&target, token.as_deref(), dry_run);
        stop.store(false, std::sync::atomic::Ordering::Relaxed);
        running.store(true, std::sync::atomic::Ordering::Relaxed);
        {
            let mut g = log.lock().unwrap();
            g.lines.clear();
            // The echoed line names the variables but NEVER their values - a
            // token pasted into this window must not end up in a log the user
            // may screenshot.
            let shown_env: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
            let suffix = if shown_env.is_empty() {
                String::new()
            } else {
                format!("   [env: {}]", shown_env.join(", "))
            };
            g.push_plain(
                LineKind::Input,
                format!("> cargo {}{suffix}", args.join(" ")),
            );
        }
        let crate_dir = root.join(dir);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("cargo");
            crate::build::no_window(&mut cmd)
                .current_dir(&crate_dir)
                .args(&args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            for (k, v) in &env {
                cmd.env(k, v);
            }
            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    running.store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(mut g) = log.lock() {
                        g.push_plain(LineKind::Notice, format!("could not launch cargo: {e}"));
                    }
                    ctx.request_repaint();
                    return;
                }
            };
            let mut child = child;
            let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            // Cargo says everything interesting on stderr, progress included.
            if let Some(err) = stderr {
                crate::terminal::spawn_reader(
                    err,
                    LineKind::Stdout,
                    Arc::clone(&log),
                    Arc::clone(&stop),
                    ctx.clone(),
                    Arc::clone(&done),
                );
            }
            if let Some(out) = stdout {
                crate::terminal::spawn_reader(
                    out,
                    LineKind::Stdout,
                    Arc::clone(&log),
                    Arc::clone(&stop),
                    ctx.clone(),
                    Arc::clone(&done),
                );
            }
            // Parked so Close can kill it; taken back OUT before waiting, so
            // the wait never happens while the lock is held.
            *child_slot.lock().unwrap() = Some(child);
            let taken = child_slot.lock().unwrap().take();
            let status = match taken {
                Some(mut c) => c.wait(),
                None => {
                    // Close killed it while we were handing it over.
                    running.store(false, std::sync::atomic::Ordering::Relaxed);
                    ctx.request_repaint();
                    return;
                }
            };
            let msg = match (status, dry_run) {
                (Ok(s), true) if s.success() => {
                    "dry run passed - the crate packages and builds. It does NOT mean the registry would accept it; see the checks above."
                        .to_owned()
                }
                (Ok(s), false) if s.success() => {
                    "published. This version is now permanent - it cannot be overwritten or deleted, only yanked."
                        .to_owned()
                }
                (Ok(s), _) => format!("cargo exited with {s}."),
                (Err(e), _) => format!("could not wait for cargo: {e}"),
            };
            // Cleared BEFORE touching the shared log: the reader threads hold
            // that same mutex, and one of them panicking would poison it, so an
            // `unwrap()` here would kill this thread with the flag still set -
            // both buttons then read "Already running" for good.
            running.store(false, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut g) = log.lock() {
                g.push_plain(LineKind::Notice, msg);
            }
            ctx.request_repaint();
        });
    }
}
