//! Find / Replace for the code editor.
//!
//! A single bar above the editor with four modes, opened by:
//! - `Ctrl+F`        — find in the current file
//! - `Ctrl+H`        — replace in the current file
//! - `Ctrl+Shift+F`  — find across the whole project tree
//! - `Ctrl+Shift+H`  — replace across the whole project tree
//!
//! Matching is case-sensitive literal text. Find and Replace also run on `Enter`.
//! In-file find selects the current match in the editor and scrolls to it (via
//! `pending_select` applied after the editor renders, + `pending_scroll_to_line`);
//! project find lists every hit and clicking one opens that file at the line.

use crate::app::{AppIde, ProjectFileId};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Which of the four search/replace modes the bar is in.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum FindMode {
    #[default]
    FindFile,
    ReplaceFile,
    FindProject,
    ReplaceProject,
}

impl FindMode {
    fn is_replace(self) -> bool {
        matches!(self, FindMode::ReplaceFile | FindMode::ReplaceProject)
    }
    fn is_project(self) -> bool {
        matches!(self, FindMode::FindProject | FindMode::ReplaceProject)
    }
    fn title(self) -> &'static str {
        match self {
            FindMode::FindFile => "Find in file",
            FindMode::ReplaceFile => "Replace in file",
            FindMode::FindProject => "Find in project",
            FindMode::ReplaceProject => "Replace in project",
        }
    }
}

/// One project-wide search hit (at most one per line).
pub struct ProjectMatch {
    pub file: ProjectFileId,
    pub path: String,
    pub line: usize, // 1-based
    pub preview: String,
}

/// Find/Replace bar state, stored on `AppIde`.
#[derive(Default)]
pub struct FindReplace {
    pub open: bool,
    pub mode: FindMode,
    pub query: String,
    pub replace: String,
    /// Request focus on the query field next frame.
    focus_query: bool,
    /// Request focus on the replace field next frame — set when a Replace mode
    /// is opened pre-filled with the identifier under the cursor, so the user
    /// edits the new name straight away.
    focus_replace: bool,
    /// Current in-file match index.
    current: usize,
    /// Status text shown in the bar (`3/12`, `No results`, `Replaced 5`, …).
    status: String,
    /// Project-wide search results.
    results: Vec<ProjectMatch>,
    /// `(start, end)` char range to select in the editor after it renders.
    pub pending_select: Option<(usize, usize)>,
}

impl FindReplace {
    /// Open (or re-target) the bar in `mode`, focusing the query field.
    pub fn open_with(&mut self, mode: FindMode) {
        self.open = true;
        self.mode = mode;
        self.focus_query = true;
        self.focus_replace = false;
        self.current = 0;
        self.status.clear();
        self.results.clear();
    }

    /// Open a Replace mode pre-filled with `word` (the identifier under the
    /// cursor): the find field searches for it, the replace field starts from
    /// it (edit to the new name), and focus goes to the replace field. When
    /// `word` is empty this is just [`open_with`].
    pub fn open_replace_with_word(&mut self, mode: FindMode, word: &str) {
        self.open_with(mode);
        if !word.is_empty() {
            self.query = word.to_owned();
            self.replace = word.to_owned();
            self.focus_query = false;
            self.focus_replace = true;
        }
    }
}

/// Non-overlapping char-index start positions of `query` in `text`
/// (case-sensitive, literal).
fn match_starts(text: &str, query: &str) -> Vec<usize> {
    let q: Vec<char> = query.chars().collect();
    let m = q.len();
    if m == 0 {
        return Vec::new();
    }
    let t: Vec<char> = text.chars().collect();
    let n = t.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i + m <= n {
        if t[i..i + m] == q[..] {
            out.push(i);
            i += m;
        } else {
            i += 1;
        }
    }
    out
}

/// 1-based line number of char index `idx` in `text`.
fn line_of(text: &str, idx: usize) -> usize {
    text.chars().take(idx).filter(|&c| c == '\n').count() + 1
}

impl AppIde {
    /// Render the Find/Replace bar (when open) above the editor. Mutates
    /// `display_code` in place for in-file replace; project replace updates the
    /// underlying file buffers (and re-syncs `display_code` so the editor's
    /// write-back doesn't revert the current file).
    pub(super) fn show_find_replace_bar(
        &mut self,
        ui: &mut egui::Ui,
        display_code: &mut String,
        displayed_file: ProjectFileId,
    ) {
        if !self.find.open {
            return;
        }
        // Esc closes the bar.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.find.open = false;
            return;
        }

        let mode = self.find.mode;
        let mut do_next = false;
        let mut do_prev = false;
        let mut do_search = false;
        let mut do_replace_all = false;
        let mut query_changed = false;
        let mut close = false;
        let mut clicked_result: Option<usize> = None;

        let frame = egui::Frame::new()
            .fill(egui::Color32::from_rgb(40, 40, 47))
            .inner_margin(egui::Margin::same(6))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 70, 82)));

        frame.show(ui, |ui| {
            // ── Row 1: title + query + nav + status + close ──
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}  {}", ph::MAGNIFYING_GLASS, mode.title()))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(160, 170, 190)),
                );
                let q = ui.add(
                    egui::TextEdit::singleline(&mut self.find.query)
                        .desired_width(230.0)
                        .hint_text("find"),
                );
                if self.find.focus_query {
                    q.request_focus();
                    self.find.focus_query = false;
                }
                query_changed = q.changed();
                let enter = q.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if !mode.is_replace() && !mode.is_project() {
                    if ui.button(ph::ARROW_UP).on_hover_text("Previous").clicked() {
                        do_prev = true;
                    }
                    if ui.button(ph::ARROW_DOWN).on_hover_text("Next").clicked() {
                        do_next = true;
                    }
                }
                if mode.is_project() && ui.button("Search").clicked() {
                    do_search = true;
                }

                // Enter runs the mode's primary action.
                if enter {
                    match mode {
                        FindMode::FindFile => do_next = true,
                        FindMode::FindProject => do_search = true,
                        FindMode::ReplaceFile | FindMode::ReplaceProject => do_replace_all = true,
                    }
                    self.find.focus_query = true; // keep focus for repeated Enter
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(ph::X).on_hover_text("Close (Esc)").clicked() {
                        close = true;
                    }
                    if !self.find.status.is_empty() {
                        ui.label(
                            egui::RichText::new(&self.find.status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 175)),
                        );
                    }
                });
            });

            // ── Row 2: replacement field + Replace All (replace modes) ──
            if mode.is_replace() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}  with", ph::ARROW_BEND_DOWN_RIGHT))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(160, 170, 190)),
                    );
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.find.replace)
                            .desired_width(230.0)
                            .hint_text("replace with"),
                    );
                    if self.find.focus_replace {
                        r.request_focus();
                        self.find.focus_replace = false;
                    }
                    let renter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Replace All").clicked() || renter {
                        do_replace_all = true;
                    }
                });
            }

            // ── Results list (project modes) ──
            if mode.is_project() && !self.find.results.is_empty() {
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(190.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (idx, m) in self.find.results.iter().enumerate() {
                            let text = format!("{}:{}  {}", m.path, m.line, m.preview);
                            if ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(text).size(11.0).monospace(),
                                    )
                                    .sense(egui::Sense::click())
                                    .truncate(),
                                )
                                .on_hover_text("Open")
                                .clicked()
                            {
                                clicked_result = Some(idx);
                            }
                        }
                    });
            }
        });
        ui.add_space(4.0);

        if close {
            self.find.open = false;
            return;
        }

        // ── Apply actions (outside the closures to avoid borrow tangles) ──
        if let Some(idx) = clicked_result {
            if let Some(m) = self.find.results.get(idx) {
                let (file, line) = (m.file, m.line);
                self.selected_file = file;
                self.pending_scroll_to_line = Some((file, line));
            }
        }

        match mode {
            FindMode::FindProject => {
                if do_search {
                    self.run_project_search();
                }
            }
            FindMode::ReplaceProject => {
                if do_replace_all {
                    self.run_project_replace(displayed_file, display_code);
                }
            }
            FindMode::ReplaceFile => {
                if do_replace_all && !self.find.query.is_empty() {
                    let count = display_code.matches(self.find.query.as_str()).count();
                    *display_code =
                        display_code.replace(self.find.query.as_str(), self.find.replace.as_str());
                    self.find.status = format!("Replaced {count}");
                }
            }
            FindMode::FindFile => {
                let starts = match_starts(display_code, &self.find.query);
                if starts.is_empty() {
                    self.find.status = if self.find.query.is_empty() {
                        String::new()
                    } else {
                        "No results".to_string()
                    };
                } else {
                    if do_next {
                        self.find.current = (self.find.current + 1) % starts.len();
                    } else if do_prev {
                        self.find.current = (self.find.current + starts.len() - 1) % starts.len();
                    } else if query_changed {
                        self.find.current = 0;
                    }
                    self.find.current = self.find.current.min(starts.len() - 1);
                    if do_next || do_prev || query_changed {
                        let start = starts[self.find.current];
                        let end = start + self.find.query.chars().count();
                        self.find.pending_select = Some((start, end));
                        self.pending_scroll_to_line =
                            Some((displayed_file, line_of(display_code, start)));
                    }
                    self.find.status = format!("{}/{}", self.find.current + 1, starts.len());
                }
            }
        }
    }

    /// Overlay-highlight every occurrence of the active find query in the
    /// currently-shown file, so matches stay visible even while the find field
    /// (not the editor) holds focus. The current in-file match is emphasised in
    /// amber; the rest are translucent cyan. Painted after the editor, like
    /// [`AppIde::highlight_selected_word`].
    pub(super) fn paint_find_matches(
        &self,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
        clip: egui::Rect,
        ui: &egui::Ui,
    ) {
        if !self.find.open || self.find.query.is_empty() {
            return;
        }
        let starts = match_starts(display_code, &self.find.query);
        if starts.is_empty() {
            return;
        }
        let wl = self.find.query.chars().count();
        let file_find = matches!(self.find.mode, FindMode::FindFile | FindMode::ReplaceFile);
        let cur_idx = self.find.current.min(starts.len() - 1);
        let base = egui::Color32::from_rgba_unmultiplied(52, 232, 235, 45);
        let current = egui::Color32::from_rgba_unmultiplied(255, 200, 60, 96);

        let gp = editor_resp.galley_pos;
        let galley = &editor_resp.galley;
        let painter = ui.painter().with_clip_rect(clip);
        for (idx, &start) in starts.iter().enumerate() {
            let loc_s = galley.pos_from_cursor(egui::text::CCursor::new(start));
            let loc_e = galley.pos_from_cursor(egui::text::CCursor::new(start + wl));
            let y_top = gp.y + loc_s.min.y;
            let y_bot = gp.y + loc_s.max.y;
            let same_row = (loc_s.min.y - loc_e.min.y).abs() < (y_bot - y_top).max(1.0) * 0.5;
            let x_l = gp.x + loc_s.min.x;
            let x_r = if same_row {
                gp.x + loc_e.min.x
            } else {
                gp.x + galley.rect.width()
            };
            if y_bot >= clip.top() && y_top <= clip.bottom() && x_r > x_l {
                let color = if file_find && idx == cur_idx { current } else { base };
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(x_l, y_top), egui::pos2(x_r, y_bot)),
                    2.0,
                    color,
                );
            }
        }
    }

    /// Every file the project search/replace spans: `(id, display_path, content)`.
    /// Config files that don't apply to the toolchain (empty `memory.x` /
    /// `build.rs` for ESP) are skipped.
    fn searchable_files(&self) -> Vec<(ProjectFileId, String, String)> {
        let mut v = vec![(
            ProjectFileId::MainRs,
            "src/main.rs".to_string(),
            self.generated_code.clone(),
        )];
        for (i, (name, content)) in self.project_tree.user_src_files.iter().enumerate() {
            v.push((ProjectFileId::UserFile(i), name.clone(), content.clone()));
        }
        v.push((ProjectFileId::CargoToml, "Cargo.toml".into(), self.cargo_toml.clone()));
        v.push((
            ProjectFileId::CargoConfig,
            ".cargo/config.toml".into(),
            self.cargo_config.clone(),
        ));
        if !self.memory_x.is_empty() {
            v.push((ProjectFileId::MemoryX, "memory.x".into(), self.memory_x.clone()));
        }
        if !self.build_rs.is_empty() {
            v.push((ProjectFileId::BuildRs, "build.rs".into(), self.build_rs.clone()));
        }
        v.push((ProjectFileId::GitIgnore, ".gitignore".into(), self.gitignore.clone()));
        v
    }

    /// In-memory content of a searchable file by id.
    fn searchable_content(&self, id: ProjectFileId) -> String {
        match id {
            ProjectFileId::MainRs => self.generated_code.clone(),
            ProjectFileId::CargoToml => self.cargo_toml.clone(),
            ProjectFileId::CargoConfig => self.cargo_config.clone(),
            ProjectFileId::MemoryX => self.memory_x.clone(),
            ProjectFileId::BuildRs => self.build_rs.clone(),
            ProjectFileId::GitIgnore => self.gitignore.clone(),
            ProjectFileId::UserFile(i) => self
                .project_tree
                .user_src_files
                .get(i)
                .map(|(_, c)| c.clone())
                .unwrap_or_default(),
        }
    }

    /// Overwrite a searchable file's content by id.
    fn set_searchable_content(&mut self, id: ProjectFileId, content: String) {
        match id {
            ProjectFileId::MainRs => self.generated_code = content,
            ProjectFileId::CargoToml => self.cargo_toml = content,
            ProjectFileId::CargoConfig => self.cargo_config = content,
            ProjectFileId::MemoryX => self.memory_x = content,
            ProjectFileId::BuildRs => self.build_rs = content,
            ProjectFileId::GitIgnore => self.gitignore = content,
            ProjectFileId::UserFile(i) => {
                if let Some(e) = self.project_tree.user_src_files.get_mut(i) {
                    e.1 = content;
                }
            }
        }
    }

    /// Populate `self.find.results` with every line in the project containing the
    /// query (one hit per line; capped to keep the list responsive).
    fn run_project_search(&mut self) {
        self.find.results.clear();
        let query = self.find.query.clone();
        if query.is_empty() {
            self.find.status.clear();
            return;
        }
        const CAP: usize = 1000;
        let mut capped = false;
        for (id, path, content) in self.searchable_files() {
            for (n, line) in content.lines().enumerate() {
                if line.contains(&query) {
                    self.find.results.push(ProjectMatch {
                        file: id,
                        path: path.clone(),
                        line: n + 1,
                        preview: line.trim().chars().take(140).collect(),
                    });
                    if self.find.results.len() >= CAP {
                        capped = true;
                        break;
                    }
                }
            }
            if capped {
                break;
            }
        }
        let n = self.find.results.len();
        self.find.status = match n {
            0 => "No results".to_string(),
            _ if capped => format!("{n}+ matches"),
            _ => format!("{n} matches"),
        };
    }

    /// Replace every occurrence of the query across all project files. Re-syncs
    /// `display_code` to the (possibly edited) current file so the editor's
    /// write-back doesn't revert it.
    fn run_project_replace(&mut self, displayed_file: ProjectFileId, display_code: &mut String) {
        let query = self.find.query.clone();
        let replacement = self.find.replace.clone();
        if query.is_empty() {
            return;
        }
        let mut total = 0usize;
        let mut files = 0usize;
        for (id, _, content) in self.searchable_files() {
            let count = content.matches(query.as_str()).count();
            if count > 0 {
                self.set_searchable_content(id, content.replace(query.as_str(), replacement.as_str()));
                total += count;
                files += 1;
            }
        }
        *display_code = self.searchable_content(displayed_file);
        self.find.results.clear();
        self.find.status = format!("Replaced {total} in {files} file(s)");
    }
}

#[cfg(test)]
mod tests {
    use super::{line_of, match_starts};

    #[test]
    fn finds_non_overlapping_matches() {
        assert_eq!(match_starts("ababab", "ab"), vec![0, 2, 4]);
        assert_eq!(match_starts("aaaa", "aa"), vec![0, 2]); // non-overlapping
        assert_eq!(match_starts("xyz", "q"), Vec::<usize>::new());
        assert_eq!(match_starts("abc", ""), Vec::<usize>::new());
    }

    #[test]
    fn match_starts_are_char_indices() {
        // A multi-byte char before the match: the index is in chars, not bytes.
        let starts = match_starts("é foo foo", "foo");
        assert_eq!(starts, vec![2, 6]);
    }

    #[test]
    fn line_numbers_are_one_based() {
        let text = "a\nbb\nccc";
        assert_eq!(line_of(text, 0), 1);
        assert_eq!(line_of(text, 2), 2); // first char of line 2
        assert_eq!(line_of(text, 5), 3);
    }
}
