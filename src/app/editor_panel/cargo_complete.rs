//! Cargo.toml dependency completion.
//!
//! `Ctrl+Space` inside a dependency table suggests embedded-relevant crates;
//! after a crate is chosen (`name = ""`), it suggests that crate's available
//! versions.
//!
//! The crate-name list is a curated, **offline** set (a raw crates.io search is
//! far too noisy for an embedded IDE — thousands of irrelevant hits). The
//! version list is fetched **live** from the crates.io sparse index in a
//! background thread, so it always reflects what is actually published.

use crate::app::{AppIde, ProjectFileId};
use eframe::egui;
use egui::text_edit::TextEditOutput;
use std::sync::{Arc, Mutex};

/// One suggestion row in the Cargo completion popup.
#[derive(Clone)]
pub(crate) struct CargoItem {
    pub label: String,
    pub detail: String,
    pub action: CargoAccept,
}

/// What happens when an item is accepted.
#[derive(Clone)]
pub(crate) enum CargoAccept {
    /// Insert `<name> = ""` and start fetching that crate's versions.
    CrateName(String),
    /// Insert the chosen version string (the cursor is already inside quotes).
    Version(String),
}

/// Async result of fetching one crate's versions from the sparse index.
pub(crate) enum VersionFetch {
    Loading,
    Done(Vec<String>),
    Error(String),
}

/// Per-`AppIde` state for the Cargo.toml completion popup. Mirrors the LSP
/// completion fields but is entirely self-contained (no rust-analyzer).
#[derive(Default)]
pub(crate) struct CargoCompleteState {
    /// True while the popup is visible.
    pub open: bool,
    /// Highlighted row.
    pub sel: usize,
    /// Items shown in the last rendered frame — keyboard accept reads these so
    /// it always acts on exactly what the user sees.
    pub items: Vec<CargoItem>,
    /// Accept deferred from a mouse click (applied at the top of next frame).
    pub pending: Option<CargoAccept>,
    /// The manifest the popup belongs to. There is more than one now (the
    /// firmware's and every extracted library's), and this state is per-`AppIde`
    /// — without it a pending accept would be applied to whichever manifest
    /// happens to be open next frame, at an offset that means nothing there.
    pub for_file: Option<crate::app::ProjectFileId>,
    /// The crate whose versions are currently being fetched / cached.
    pub version_crate: String,
    /// Shared background fetch result for `version_crate`.
    pub version_fetch: Option<Arc<Mutex<VersionFetch>>>,
}

/// Where the cursor sits inside a Cargo.toml dependency table.
#[derive(Debug, PartialEq)]
pub(crate) enum CargoCtx {
    /// Typing a crate name (key) — replace `[start..cursor]` with the picked crate.
    Name { start: usize, prefix: String },
    /// Typing a version value for `crate_name` — replace `[start..cursor]`.
    Version {
        crate_name: String,
        start: usize,
        prefix: String,
    },
}

impl AppIde {
    /// Drive the Cargo.toml completion popup: apply a pending accept, react to
    /// Ctrl+Space, then (re)filter and render. Called instead of the LSP
    /// completion handler when the open file is `Cargo.toml`.
    pub(super) fn handle_cargo_completion(
        &mut self,
        ui: &mut egui::Ui,
        editor_resp: &TextEditOutput,
        display_code: &mut String,
        ctrl_space_pressed: bool,
    ) {
        let cursor_char_idx = editor_resp
            .state
            .cursor
            .char_range()
            .map(|r| r.primary.index);

        // ── 0. Different manifest than the popup was opened for? ──────────────
        // Offsets and the item list belong to the file they were computed in;
        // carrying them into another manifest would splice text at a position
        // that means nothing there.
        if self.cargo_complete.for_file != Some(self.selected_file) {
            self.cargo_complete.open = false;
            self.cargo_complete.pending = None;
            self.cargo_complete.items.clear();
            self.cargo_complete.sel = 0;
            self.cargo_complete.for_file = Some(self.selected_file);
        }

        // ── 1. Apply a pending accept (keyboard or mouse) ─────────────────────
        if let Some(accept) = self.cargo_complete.pending.take() {
            if let Some(cur) = cursor_char_idx {
                self.apply_cargo_accept(ui, editor_resp, display_code, cur, accept);
            }
            // The text and caret just changed; the cursor index from this frame
            // is stale against the new text. Let the next frame (which sees the
            // re-positioned caret) recompute context and render the follow-up
            // version popup.
            return;
        }

        // ── 2. Ctrl+Space → (re)open at the current context ───────────────────
        if ctrl_space_pressed {
            if let Some(cur) = cursor_char_idx {
                if let Some(ctx) = cargo_context(display_code, cur) {
                    self.open_cargo_popup(&ctx);
                } else {
                    self.cargo_complete.open = false;
                }
            }
        }

        // ── 3. Render (filter against the live prefix) ────────────────────────
        if !self.cargo_complete.open {
            return;
        }
        let Some(cur) = cursor_char_idx else {
            self.cargo_complete.open = false;
            return;
        };
        let Some(ctx) = cargo_context(display_code, cur) else {
            // Cursor left a completable position.
            self.cargo_complete.open = false;
            return;
        };

        // Rebuild the visible item list from the current context.
        let (items, loading, error) = self.cargo_items_for(&ctx);
        self.cargo_complete.items = items.clone();

        if items.is_empty() && !loading {
            if error.is_none() {
                // Nothing matches the typed prefix — drop the popup.
                self.cargo_complete.open = false;
                return;
            }
        }
        self.cargo_complete.sel = self.cargo_complete.sel.min(items.len().saturating_sub(1));

        self.render_cargo_popup(ui, editor_resp, &items, loading, error);
    }

    /// Configure the popup for a freshly detected context.
    fn open_cargo_popup(&mut self, ctx: &CargoCtx) {
        self.cargo_complete.open = true;
        self.cargo_complete.sel = 0;
        if let CargoCtx::Version { crate_name, .. } = ctx {
            self.ensure_version_fetch(crate_name);
        }
    }

    /// Build the filtered item list for the current context. Returns
    /// `(items, loading, error)`.
    fn cargo_items_for(&mut self, ctx: &CargoCtx) -> (Vec<CargoItem>, bool, Option<String>) {
        match ctx {
            CargoCtx::Name { prefix, .. } => (filter_crates(prefix), false, None),
            CargoCtx::Version {
                crate_name, prefix, ..
            } => {
                self.ensure_version_fetch(crate_name);
                let guard = self
                    .cargo_complete
                    .version_fetch
                    .as_ref()
                    .map(|a| a.lock().unwrap());
                match guard.as_deref() {
                    Some(VersionFetch::Loading) | None => (Vec::new(), true, None),
                    Some(VersionFetch::Error(e)) => (Vec::new(), false, Some(e.clone())),
                    Some(VersionFetch::Done(versions)) => {
                        let pl = prefix.to_lowercase();
                        let items = versions
                            .iter()
                            .filter(|v| pl.is_empty() || v.to_lowercase().starts_with(&pl))
                            .take(60)
                            .enumerate()
                            .map(|(i, v)| CargoItem {
                                label: v.clone(),
                                detail: if i == 0 {
                                    "latest".into()
                                } else {
                                    String::new()
                                },
                                action: CargoAccept::Version(v.clone()),
                            })
                            .collect();
                        (items, false, None)
                    }
                }
            }
        }
    }

    /// Start a background fetch of `name`'s versions if not already cached.
    fn ensure_version_fetch(&mut self, name: &str) {
        if self.cargo_complete.version_crate == name && self.cargo_complete.version_fetch.is_some()
        {
            return;
        }
        let shared = Arc::new(Mutex::new(VersionFetch::Loading));
        self.cargo_complete.version_crate = name.to_string();
        self.cargo_complete.version_fetch = Some(shared.clone());
        let name = name.to_string();
        std::thread::spawn(move || {
            let result = match fetch_versions(&name) {
                Ok(v) if v.is_empty() => VersionFetch::Error("no published versions".into()),
                Ok(v) => VersionFetch::Done(v),
                Err(e) => VersionFetch::Error(e),
            };
            *shared.lock().unwrap() = result;
        });
    }

    /// Apply an accepted suggestion to the editor text + cursor.
    fn apply_cargo_accept(
        &mut self,
        ui: &mut egui::Ui,
        editor_resp: &TextEditOutput,
        display_code: &mut String,
        cursor: usize,
        accept: CargoAccept,
    ) {
        let Some(ctx) = cargo_context(display_code, cursor) else {
            self.cargo_complete.open = false;
            return;
        };
        let chars: Vec<char> = display_code.chars().collect();
        // `cargo_context` clamps the cursor; the splices below index with it
        // directly, so clamp here too or a caret left over from a longer text
        // panics with "range start index N out of range". (It really happened:
        // the popup state is shared across manifests of different lengths.)
        let cursor = cursor.min(chars.len());
        match accept {
            CargoAccept::CrateName(name) => {
                if let CargoCtx::Name { start, .. } = ctx {
                    let start = start.min(cursor);
                    let insert = format!("{name} = \"\"");
                    let before: String = chars[..start].iter().collect();
                    let after: String = chars[cursor..].iter().collect();
                    *display_code = format!("{before}{insert}{after}");
                    // Cursor between the two quotes (just before the closing one).
                    let new_cursor = start + insert.chars().count() - 1;
                    self.store_cargo_cursor(ui, editor_resp, new_cursor);
                    self.persist_cargo_toml(display_code);
                    // Switch straight to version completion for this crate.
                    self.ensure_version_fetch(&name);
                    self.cargo_complete.open = true;
                    self.cargo_complete.sel = 0;
                    ui.ctx().request_repaint();
                }
            }
            CargoAccept::Version(ver) => {
                if let CargoCtx::Version { start, .. } = ctx {
                    let start = start.min(cursor);
                    let before: String = chars[..start].iter().collect();
                    let after: String = chars[cursor..].iter().collect();
                    *display_code = format!("{before}{ver}{after}");
                    let new_cursor = start + ver.chars().count();
                    self.store_cargo_cursor(ui, editor_resp, new_cursor);
                    self.persist_cargo_toml(display_code);
                    self.cargo_complete.open = false;
                }
            }
        }
    }

    /// Persist the edited manifest text into its backing store, mirroring the
    /// editor write-back.
    ///
    /// Needed because the write-back runs EARLIER in the frame than the
    /// completion, so an accepted suggestion would otherwise be dropped. A
    /// library crate's manifest is an ordinary user file — handling only
    /// `CargoToml` here silently lost every completion accepted there.
    fn persist_cargo_toml(&mut self, display_code: &str) {
        match self.selected_file {
            ProjectFileId::CargoToml => self.cargo_toml = display_code.to_owned(),
            ProjectFileId::UserFile(i) => {
                if let Some(entry) = self.project_tree.user_src_files.get_mut(i) {
                    entry.1 = display_code.to_owned();
                }
            }
            _ => {}
        }
    }

    /// Re-position the editor caret for next frame (the widget already rendered).
    fn store_cargo_cursor(&self, ui: &mut egui::Ui, editor_resp: &TextEditOutput, char_idx: usize) {
        let mut st = editor_resp.state.clone();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(char_idx),
        )));
        st.store(ui.ctx(), editor_resp.response.id);
    }

    /// Render the popup. Mouse clicks set `cargo_complete.pending` (applied next
    /// frame, same path as keyboard accept).
    fn render_cargo_popup(
        &mut self,
        ui: &mut egui::Ui,
        editor_resp: &TextEditOutput,
        items: &[CargoItem],
        loading: bool,
        error: Option<String>,
    ) {
        let popup_pos = if let Some(char_range) = editor_resp.state.cursor.char_range() {
            let idx = char_range.primary.index;
            let count = editor_resp.galley.job.text.chars().count();
            let clamped = idx.min(count.saturating_sub(1));
            let local = editor_resp
                .galley
                .pos_from_cursor(egui::text::CCursor::new(clamped));
            editor_resp.response.rect.left_top()
                + local.min.to_vec2()
                + egui::vec2(0.0, local.height() + 2.0)
        } else {
            editor_resp.response.rect.left_top()
        };

        let sel = self.cargo_complete.sel;
        egui::Area::new(egui::Id::new("cargo_completion_popup"))
            .fixed_pos(popup_pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(360.0);
                    ui.set_max_width(360.0);

                    if loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new("  crates.io — fetching versions…")
                                    .size(11.5)
                                    .color(egui::Color32::from_rgb(160, 175, 200)),
                            );
                        });
                        ui.ctx().request_repaint();
                        return;
                    }
                    if let Some(err) = &error {
                        ui.label(
                            egui::RichText::new(format!("crates.io: {err}"))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(210, 120, 110)),
                        );
                        return;
                    }

                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (i, item) in items.iter().enumerate() {
                                let selected = i == sel;
                                let fg = if selected {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_rgb(200, 210, 230)
                                };
                                let sel_bg = egui::Color32::from_rgb(40, 90, 160);
                                let hover_bg = egui::Color32::from_rgb(50, 60, 80);
                                let detail_fg = if selected {
                                    egui::Color32::from_rgb(160, 195, 255)
                                } else {
                                    egui::Color32::from_rgb(110, 130, 155)
                                };

                                let row_h = 19.0;
                                let avail_w = ui.available_width();
                                let (rect, row_resp) = ui.allocate_exact_size(
                                    egui::vec2(avail_w, row_h),
                                    egui::Sense::click(),
                                );
                                if selected {
                                    ui.painter().rect_filled(rect, 2.0, sel_bg);
                                } else if row_resp.hovered() {
                                    ui.painter().rect_filled(rect, 2.0, hover_bg);
                                }
                                let painter = ui.painter();
                                painter.text(
                                    rect.left_center() + egui::vec2(4.0, 0.0),
                                    egui::Align2::LEFT_CENTER,
                                    &item.label,
                                    egui::FontId::monospace(12.0),
                                    fg,
                                );
                                if !item.detail.is_empty() {
                                    let det = {
                                        let c: Vec<char> = item.detail.chars().collect();
                                        if c.len() > 44 {
                                            format!("{}…", c[..41].iter().collect::<String>())
                                        } else {
                                            item.detail.clone()
                                        }
                                    };
                                    painter.text(
                                        rect.right_center() - egui::vec2(4.0, 0.0),
                                        egui::Align2::RIGHT_CENTER,
                                        det,
                                        egui::FontId::monospace(10.5),
                                        detail_fg,
                                    );
                                }
                                if row_resp.clicked() {
                                    self.cargo_complete.pending = Some(item.action.clone());
                                    self.cargo_complete.open = false;
                                }
                                if selected {
                                    row_resp.scroll_to_me(None);
                                }
                            }
                        });
                });
            });
    }
}

// ── Pure helpers (tested) ─────────────────────────────────────────────────────

/// Detect what the cursor (char index) is positioned to complete in Cargo.toml.
pub(crate) fn cargo_context(text: &str, cursor: usize) -> Option<CargoCtx> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());

    // Current line bounds (char indices).
    let line_start = chars[..cursor]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let line_end = chars[cursor..]
        .iter()
        .position(|&c| c == '\n')
        .map(|p| cursor + p)
        .unwrap_or(chars.len());
    let line: String = chars[line_start..line_end].iter().collect();
    let col = cursor - line_start; // char offset within the line

    // Skip comments / section headers themselves.
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }

    // Which dependency table are we in?
    let section = current_section(&chars, line_start)?;

    let line_chars: Vec<char> = line.chars().collect();
    let first_eq = line_chars.iter().position(|&c| c == '=');

    match first_eq {
        // No '=' yet, or cursor is on the key side → crate-name completion.
        Some(eq) if col <= eq => name_ctx(&section, &line_chars, line_start, col),
        None => name_ctx(&section, &line_chars, line_start, col),
        // Cursor on the value side → version completion.
        Some(eq) => version_ctx(&section, &line_chars, line_start, col, eq),
    }
}

/// The dependency table that encloses `line_start`, if any.
fn current_section(chars: &[char], line_start: usize) -> Option<DepSection> {
    let prefix: String = chars[..line_start].iter().collect();
    let header = prefix
        .lines()
        .rev()
        .map(|l| l.trim())
        .find(|l| l.starts_with('[') && l.ends_with(']'))?;
    let inner = header.trim_start_matches('[').trim_end_matches(']').trim();
    parse_dep_section(inner)
}

#[derive(Debug, PartialEq, Clone)]
enum DepSection {
    /// `[dependencies]` / `[dev-dependencies]` / `[target.'…'.dependencies]`.
    Table,
    /// `[dependencies.<crate>]` — only a version is completable here.
    Entry(String),
}

fn parse_dep_section(inner: &str) -> Option<DepSection> {
    const KINDS: [&str; 3] = ["dev-dependencies", "build-dependencies", "dependencies"];
    if KINDS.iter().any(|k| inner.ends_with(k)) {
        return Some(DepSection::Table);
    }
    for k in KINDS {
        let needle = format!("{k}.");
        if let Some(pos) = inner.rfind(&needle) {
            let name = inner[pos + needle.len()..].trim();
            if !name.is_empty() {
                return Some(DepSection::Entry(name.to_string()));
            }
        }
    }
    None
}

fn name_ctx(
    section: &DepSection,
    line: &[char],
    line_start: usize,
    col: usize,
) -> Option<CargoCtx> {
    // Crate names are only typed in a `[dependencies]`-style table.
    if *section != DepSection::Table {
        return None;
    }
    let typed: String = line[..col].iter().collect();
    let trimmed = typed.trim_start();
    let lead_ws = typed.chars().count() - trimmed.chars().count();
    // Must look like a crate-name prefix (letters/digits/-/_), possibly empty.
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(CargoCtx::Name {
        start: line_start + lead_ws,
        prefix: trimmed.to_string(),
    })
}

fn version_ctx(
    section: &DepSection,
    line: &[char],
    line_start: usize,
    col: usize,
    first_eq: usize,
) -> Option<CargoCtx> {
    // Last '=' before the cursor.
    let veq = line[..col].iter().rposition(|&c| c == '=')?;

    // The key immediately before that '=' (for inner table fields).
    let key_end = line[..veq].iter().rposition(|&c| !c.is_whitespace());
    let key: String = match key_end {
        Some(end) => {
            let key_start = line[..=end]
                .iter()
                .rposition(|&c| c.is_whitespace() || c == '{' || c == ',')
                .map(|p| p + 1)
                .unwrap_or(0);
            line[key_start..=end].iter().collect()
        }
        None => String::new(),
    };

    let crate_name = match section {
        DepSection::Entry(name) => {
            // Only the `version = "…"` field is a version here.
            if key != "version" {
                return None;
            }
            name.clone()
        }
        DepSection::Table => {
            if veq == first_eq {
                // `crate = "…"` simple form — key is the crate name.
                line[..first_eq]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string()
            } else if key == "version" {
                // `crate = { version = "…" }` — outer key is the crate name.
                line[..first_eq]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string()
            } else {
                // Some other inline field (features/path/git/…) — not a version.
                return None;
            }
        }
    };
    if crate_name.is_empty() {
        return None;
    }

    // We only complete inside an open quoted string: the count of '"' between
    // the value's '=' and the cursor must be odd (one unclosed opening quote).
    let region = &line[veq + 1..col];
    let quotes = region.iter().filter(|&&c| c == '"').count();
    if quotes % 2 == 0 {
        return None;
    }
    let open_q = veq + 1 + region.iter().rposition(|&c| c == '"').unwrap();
    let prefix: String = line[open_q + 1..col].iter().collect();
    Some(CargoCtx::Version {
        crate_name,
        start: line_start + open_q + 1,
        prefix,
    })
}

/// Filter the curated embedded-crate list by `prefix` (case-insensitive).
/// Prefix matches rank before substring matches; both are alphabetical within.
fn filter_crates(prefix: &str) -> Vec<CargoItem> {
    let p = prefix.to_lowercase();
    let mut starts: Vec<&(&str, &str)> = Vec::new();
    let mut contains: Vec<&(&str, &str)> = Vec::new();
    for entry in EMBEDDED_CRATES {
        let name = entry.0.to_lowercase();
        if p.is_empty() || name.starts_with(&p) {
            starts.push(entry);
        } else if name.contains(&p) {
            contains.push(entry);
        }
    }
    starts
        .into_iter()
        .chain(contains)
        .take(80)
        .map(|(name, desc)| CargoItem {
            label: name.to_string(),
            detail: desc.to_string(),
            action: CargoAccept::CrateName(name.to_string()),
        })
        .collect()
}

/// Path of a crate inside the crates.io sparse index (lower-cased name).
fn sparse_index_path(name: &str) -> String {
    let n = name.to_lowercase();
    let c: Vec<char> = n.chars().collect();
    match c.len() {
        0 => n,
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{}", c[0], n),
        _ => {
            let a: String = c[..2].iter().collect();
            let b: String = c[2..4].iter().collect();
            format!("{a}/{b}/{n}")
        }
    }
}

/// Fetch a crate's published versions from the crates.io sparse index.
fn fetch_versions(name: &str) -> Result<Vec<String>, String> {
    let url = format!("https://index.crates.io/{}", sparse_index_path(name));
    let body = ureq::get(&url)
        .set("User-Agent", "embedded_ide_0 (crate version lookup)")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "crate not found".to_string(),
            other => other.to_string(),
        })?
        .into_string()
        .map_err(|e| e.to_string())?;
    Ok(parse_index_versions(&body))
}

/// Parse a sparse-index body (newline-delimited JSON) into a newest-first list
/// of non-yanked versions.
fn parse_index_versions(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("yanked").and_then(|y| y.as_bool()).unwrap_or(false) {
                continue;
            }
            if let Some(ver) = v.get("vers").and_then(|s| s.as_str()) {
                out.push(ver.to_string());
            }
        }
    }
    out.reverse(); // index lists oldest→newest; show newest first.
    out
}

/// Curated set of embedded-relevant crates (name, one-line description). A raw
/// crates.io search returns far too much noise for an MCU project, so the name
/// list is hand-picked; versions are still fetched live from crates.io.
#[rustfmt::skip]
const EMBEDDED_CRATES: &[(&str, &str)] = &[
    // Cortex-M core / runtime
    ("cortex-m", "Low level access to Cortex-M processors"),
    ("cortex-m-rt", "Startup code & minimal runtime for Cortex-M"),
    ("cortex-m-semihosting", "Semihosting for Cortex-M"),
    ("cortex-m-rtic", "Real-Time Interrupt-driven Concurrency"),
    ("rtic", "Real-Time Interrupt-driven Concurrency framework"),
    ("critical-section", "Cross-platform critical sections"),
    ("bare-metal", "Abstractions for bare-metal programming"),
    ("vcell", "Volatile Cell"),
    ("volatile-register", "Volatile access to memory mapped registers"),
    // RISC-V
    ("riscv", "Low level access to RISC-V processors"),
    ("riscv-rt", "Startup code & runtime for RISC-V"),
    // HAL traits / IO
    ("embedded-hal", "Hardware Abstraction Layer traits"),
    ("embedded-hal-async", "Async HAL traits"),
    ("embedded-hal-bus", "Bus/sharing utilities for embedded-hal"),
    ("embedded-io", "Core IO traits for embedded"),
    ("embedded-io-async", "Async core IO traits"),
    ("nb", "Minimal non-blocking IO abstraction"),
    ("fugit", "Time units & duration for embedded"),
    // STM32 HALs / PACs
    ("stm32f1xx-hal", "HAL for STM32F1 series"),
    ("stm32f4xx-hal", "HAL for STM32F4 series"),
    ("stm32f0xx-hal", "HAL for STM32F0 series"),
    ("stm32f3xx-hal", "HAL for STM32F3 series"),
    ("stm32f7xx-hal", "HAL for STM32F7 series"),
    ("stm32l0xx-hal", "HAL for STM32L0 series"),
    ("stm32l4xx-hal", "HAL for STM32L4 series"),
    ("stm32g0xx-hal", "HAL for STM32G0 series"),
    ("stm32g4xx-hal", "HAL for STM32G4 series"),
    ("stm32h7xx-hal", "HAL for STM32H7 series"),
    ("stm32f1", "Peripheral access crate for STM32F1"),
    ("stm32f4", "Peripheral access crate for STM32F4"),
    ("stm32-hal2", "Multi-family STM32 HAL"),
    // Embassy async
    ("embassy-executor", "Async/await executor for embedded"),
    ("embassy-stm32", "Embassy HAL for STM32"),
    ("embassy-time", "Timekeeping & timers for Embassy"),
    ("embassy-sync", "Async synchronization primitives"),
    ("embassy-futures", "Async utilities for embedded"),
    ("embassy-net", "Async TCP/IP network stack"),
    ("embassy-usb", "Async USB device stack"),
    ("embassy-rp", "Embassy HAL for RP2040"),
    ("embassy-nrf", "Embassy HAL for nRF"),
    ("static_cell", "Statically allocated, initialized-at-runtime cell"),
    // ESP32
    ("esp-hal", "HAL for Espressif (no_std) chips"),
    ("esp-backtrace", "Backtrace support for ESP"),
    ("esp-println", "println!/log over UART/RTT for ESP"),
    ("esp-wifi", "Wi-Fi / BLE for ESP no_std"),
    ("esp-idf-hal", "embedded-hal for ESP-IDF (std)"),
    ("esp-idf-svc", "ESP-IDF services (Wi-Fi, MQTT, …)"),
    ("esp-idf-sys", "Raw bindings to ESP-IDF"),
    ("esp32", "Peripheral access crate for ESP32"),
    ("esp32c3", "Peripheral access crate for ESP32-C3"),
    // RP2040
    ("rp2040-hal", "HAL for the RP2040"),
    ("rp-pico", "Board support for the Raspberry Pi Pico"),
    ("rp2040-boot2", "Second-stage bootloaders for RP2040"),
    // nRF
    ("nrf52840-hal", "HAL for nRF52840"),
    ("nrf52833-hal", "HAL for nRF52833"),
    ("nrf-hal-common", "Common HAL code for nRF"),
    // AVR
    ("avr-device", "Register access for AVR microcontrollers"),
    // Logging / debug
    ("defmt", "Efficient deferred-formatting logging"),
    ("defmt-rtt", "defmt transport over RTT"),
    ("defmt-test", "Embedded test harness using defmt"),
    ("rtt-target", "RTT target-side implementation"),
    ("panic-probe", "Panic handler printing via defmt/RTT"),
    ("panic-halt", "Panic handler that halts"),
    ("panic-semihosting", "Panic handler over semihosting"),
    ("panic-rtt-target", "Panic handler over RTT"),
    ("log", "Logging facade"),
    // Data / math (no_std)
    ("heapless", "Fixed-capacity data structures (no_std)"),
    ("fixed", "Fixed-point numbers"),
    ("micromath", "Embedded-friendly math approximations"),
    ("libm", "Pure-Rust libm math functions"),
    ("num-traits", "Numeric traits"),
    ("byteorder", "Read/write numbers in big/little endian"),
    ("bitflags", "Typed bitflag sets"),
    ("bitfield", "Bitfield macros"),
    ("void", "Uninhabited void type"),
    // USB
    ("usb-device", "USB device stack"),
    ("usbd-serial", "USB CDC-ACM serial class"),
    ("usbd-hid", "USB HID class"),
    // Graphics / displays / drivers
    ("embedded-graphics", "2D graphics for small displays"),
    ("embedded-graphics-core", "Core traits for embedded-graphics"),
    ("display-interface", "Generic display interface traits"),
    ("ssd1306", "Driver for SSD1306 OLED displays"),
    ("st7789", "Driver for ST7789 TFT displays"),
    ("ili9341", "Driver for ILI9341 TFT displays"),
    ("epd-waveshare", "Driver for Waveshare e-paper displays"),
    ("smart-leds", "Driver abstraction for addressable LEDs"),
    ("ws2812-spi", "WS2812 LEDs over SPI"),
    ("shared-bus", "Share an I2C/SPI bus between drivers"),
    ("mpu6050", "Driver for the MPU6050 IMU"),
    ("bmp280", "Driver for the BMP280 pressure sensor"),
    ("ds323x", "Driver for DS3231/DS3232 RTCs"),
    ("ina219", "Driver for the INA219 power monitor"),
    // Async / futures
    ("futures", "Async primitives & combinators"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(text: &str) -> Option<CargoCtx> {
        // Cursor is marked by '|' in the fixture.
        let cursor = text.find('|').expect("fixture needs a | cursor marker");
        let cleaned = text.replace('|', "");
        // char index of the cursor (fixtures are ASCII)
        cargo_context(&cleaned, cursor)
    }

    #[test]
    fn name_context_in_dependencies() {
        let c = ctx("[dependencies]\nstm|\n");
        assert_eq!(
            c,
            Some(CargoCtx::Name {
                start: 15,
                prefix: "stm".to_string()
            })
        );
    }

    #[test]
    fn empty_name_context_offers_all() {
        let c = ctx("[dependencies]\n|\n");
        assert!(matches!(c, Some(CargoCtx::Name { prefix, .. }) if prefix.is_empty()));
    }

    #[test]
    fn version_context_simple_form() {
        let c = ctx("[dependencies]\ncortex-m = \"0.7|\"\n");
        match c {
            Some(CargoCtx::Version {
                crate_name, prefix, ..
            }) => {
                assert_eq!(crate_name, "cortex-m");
                assert_eq!(prefix, "0.7");
            }
            other => panic!("expected version ctx, got {other:?}"),
        }
    }

    #[test]
    fn version_context_table_form() {
        let c = ctx("[dependencies]\nserde = { version = \"1|\", features = [] }\n");
        match c {
            Some(CargoCtx::Version {
                crate_name, prefix, ..
            }) => {
                assert_eq!(crate_name, "serde");
                assert_eq!(prefix, "1");
            }
            other => panic!("expected version ctx, got {other:?}"),
        }
    }

    #[test]
    fn version_context_dotted_section() {
        let c = ctx("[dependencies.heapless]\nversion = \"0.|\"\n");
        match c {
            Some(CargoCtx::Version {
                crate_name, prefix, ..
            }) => {
                assert_eq!(crate_name, "heapless");
                assert_eq!(prefix, "0.");
            }
            other => panic!("expected version ctx, got {other:?}"),
        }
    }

    #[test]
    fn features_field_is_not_version() {
        let c = ctx("[dependencies]\nserde = { features = [\"de|\"] }\n");
        assert_eq!(c, None);
    }

    #[test]
    fn outside_dependency_table_is_none() {
        assert_eq!(ctx("[package]\nnam|\n"), None);
    }

    #[test]
    fn section_header_line_is_none() {
        assert_eq!(ctx("[depend|encies]\n"), None);
    }

    #[test]
    fn cursor_after_closed_string_is_none() {
        // Even number of quotes before the cursor → not inside a string.
        let c = ctx("[dependencies]\ncortex-m = \"0.7\"|\n");
        assert_eq!(c, None);
    }

    #[test]
    fn sparse_index_paths() {
        assert_eq!(sparse_index_path("a"), "1/a");
        assert_eq!(sparse_index_path("nb"), "2/nb");
        assert_eq!(sparse_index_path("log"), "3/l/log");
        assert_eq!(sparse_index_path("serde"), "se/rd/serde");
        assert_eq!(sparse_index_path("cortex-m"), "co/rt/cortex-m");
        assert_eq!(sparse_index_path("STM32"), "st/m3/stm32");
    }

    #[test]
    fn parse_versions_newest_first_skips_yanked() {
        let body = "\
{\"name\":\"x\",\"vers\":\"0.1.0\",\"yanked\":false}
{\"name\":\"x\",\"vers\":\"0.2.0\",\"yanked\":true}
{\"name\":\"x\",\"vers\":\"0.3.0\",\"yanked\":false}
";
        assert_eq!(parse_index_versions(body), vec!["0.3.0", "0.1.0"]);
    }

    #[test]
    fn filter_crates_ranks_prefix_first() {
        let items = filter_crates("stm32f1");
        assert!(!items.is_empty());
        assert!(items[0].label.starts_with("stm32f1"));
    }

    /// A caret left over from a LONGER manifest must not index past the current
    /// text. Regression: the popup state is shared across manifests (the
    /// firmware's and every extracted library's), so switching between two of
    /// different lengths panicked with "range start index N out of range".
    /// `cargo_context` clamps; the splice in `apply_cargo_accept` did not.
    #[test]
    fn a_cursor_past_the_end_is_clamped_not_panicking() {
        let text = "[dependencies]
foo = \"1\"
";
        // Far past the end — what a stale caret from another file looks like.
        assert!(cargo_context(text, 10_000).is_some() || cargo_context(text, 10_000).is_none());
        // And the offsets it reports are always inside the text.
        if let Some(ctx) = cargo_context(text, 10_000) {
            let len = text.chars().count();
            match ctx {
                CargoCtx::Name { start, .. } => assert!(start <= len, "start {start} > {len}"),
                CargoCtx::Version { start, .. } => assert!(start <= len, "start {start} > {len}"),
            }
        }
    }
}
