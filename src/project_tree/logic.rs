//! Project tree logic — file operations, directory scanning, filesystem watching.

use std::path::Path;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

/// State for the project tree (files and folders in src/).
#[derive(Debug, Clone)]
pub struct ProjectTreeState {
    /// `(path_relative_to_src, content)` for every user-created file.
    pub user_src_files: Vec<(String, String)>,
    /// Explicitly-created folders inside src/.
    pub user_src_folders: Vec<String>,
}

impl ProjectTreeState {
    /// Create a new empty project tree state.
    pub fn new() -> Self {
        Self {
            user_src_files: Vec::new(),
            user_src_folders: Vec::new(),
        }
    }

    /// Load project tree state from a project directory.
    pub fn load_from_dir(root: &Path) -> Self {
        let src_dir = root.join("src");
        if !src_dir.exists() {
            return Self::new();
        }

        let mut files = Vec::new();
        let mut folders = Vec::new();
        Self::scan_src_dir(&src_dir, &src_dir, &mut files, &mut folders);

        Self {
            user_src_files: files,
            user_src_folders: folders,
        }
    }

    /// Recursively scan a directory for files and folders (relative to root).
    /// Skips `main.rs` and loads all file types.
    fn scan_src_dir(
        root: &Path,
        dir: &Path,
        files: &mut Vec<(String, String)>,
        folders: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !folders.contains(&rel) {
                    folders.push(rel);
                }
                Self::scan_src_dir(root, &path, files, folders);
            } else if path.is_file() {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel == "main.rs" {
                    continue; // always generated — skip
                }
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                files.push((rel, content));
            }
        }
    }

    /// Handle filesystem events: Create, Remove, and Rename operations.
    pub fn handle_fs_events(&mut self, events: Vec<(String, FsEventKind)>) {
        for (rel, kind) in events {
            match kind {
                FsEventKind::Create => {
                    if !self.user_src_files.iter().any(|(p, _)| p == &rel) {
                        let content = String::new();
                        self.user_src_files.push((rel, content));
                    }
                }
                FsEventKind::Remove => {
                    self.user_src_files.retain(|(p, _)| p != &rel);
                    let dir_rel = rel.trim_end_matches('/').to_string();
                    self.user_src_folders.retain(|f| f != &dir_rel);
                }
                FsEventKind::Rename { old_rel, new_rel } => {
                    // File rename
                    if let Some((p, _)) = self.user_src_files.iter_mut().find(|(p, _)| p == &old_rel)
                    {
                        *p = new_rel.clone();
                    }
                    // Folder rename — update folder list + all child paths
                    if let Some(f) = self.user_src_folders.iter_mut().find(|f| **f == old_rel) {
                        *f = new_rel.clone();
                    }
                    let old_prefix = format!("{old_rel}/");
                    let new_prefix = format!("{new_rel}/");
                    for (path, _) in &mut self.user_src_files {
                        if path.starts_with(&old_prefix) {
                            *path = format!("{new_prefix}{}", &path[old_prefix.len()..]);
                        }
                    }
                }
            }
        }
    }

    /// Synchronize pin files in the pins/ directory.
    /// Removes old pin files, creates new ones, and rebuilds pins/mod.rs.
    pub fn sync_pin_files(&mut self, all_pins: &[(usize, String, PinFunction)]) {
        const MOD_PATH: &str = "pins/mod.rs";

        // Build the authoritative set of configured pins
        let configured: Vec<(String, usize, &str, &PinFunction)> = all_pins
            .iter()
            .filter(|(_, _, f)| *f != PinFunction::Unset)
            .map(|(num, name, func)| {
                // Suffix the file/module name with the selected function type
                // (e.g. `pin2_pc13_out`). Changing a pin's function renames its
                // file accordingly — the old file is dropped in step 3.
                let slug = format!("pin{}_{}_{}", num, name.to_lowercase(), func.file_token());
                (slug, *num, name.as_str(), func)
            })
            .collect();

        let active_slugs: Vec<&str> = configured.iter().map(|(s, ..)| s.as_str()).collect();

        // 1. Ensure pins/ folder is registered
        let folder = "pins".to_string();
        if !self.user_src_folders.contains(&folder) {
            self.user_src_folders.push(folder);
        }

        // 2. Ensure pins/mod.rs exists
        if !self
            .user_src_files
            .iter()
            .any(|(p, _)| p == MOD_PATH)
        {
            self.user_src_files.push((MOD_PATH.to_string(), String::new()));
        }

        // 3. Drop pin files that are no longer configured
        self.user_src_files.retain(|(path, _)| {
            let Some(fname) = path.strip_prefix("pins/") else {
                return true;
            };
            if fname == "mod.rs" {
                return true;
            }
            if !fname.starts_with("pin") || !fname.ends_with(".rs") {
                return true;
            }
            let slug = &fname[..fname.len() - 3];
            active_slugs.contains(&slug)
        });

        // 4. Create or update pin files (with GENERATED marker preservation)
        for (slug, num, name, func) in &configured {
            let file_path = format!("pins/{slug}.rs");
            let generated_content = generate_pin_content(*num, name, func);
            let wrapped_content = format!(
                "// <<< GENERATED>>>\n{}\n// <<< GENERATED END >>>\n",
                generated_content
            );

            if let Some((_, file_content)) = self.user_src_files.iter_mut().find(|(p, _)| p == &file_path) {
                // Update existing file: preserve user code, update only GENERATED section
                let existing = file_content.clone();
                let updated = splice_pin_file(&existing, &wrapped_content);
                *file_content = updated;
            } else {
                // Create new file
                self.user_src_files.push((file_path, wrapped_content));
            }
        }

        // 5. Rebuild mod.rs (preserve custom code outside GENERATED section).
        // Also declare `pub mod configs;` when per-peripheral init modules exist
        // under `pins/configs/` (synced separately by `sync_config_files`).
        let has_configs = self
            .user_src_files
            .iter()
            .any(|(p, _)| p.starts_with("pins/configs/"));
        let mut generated_section: String = configured
            .iter()
            .map(|(slug, ..)| format!("pub mod {slug};\n"))
            .collect();
        if has_configs {
            generated_section.push_str("pub mod configs;\n");
        }

        let generated_with_markers = format!(
            "// <<< GENERATED>>>\n{}\n// <<< GENERATED END >>>\n",
            generated_section.trim()
        );

        if let Some((_, mod_content)) = self
            .user_src_files
            .iter_mut()
            .find(|(p, _)| p == MOD_PATH)
        {
            let existing = mod_content.as_str();
            if let (Some(begin_pos), Some(end_pos)) = (
                existing.find("// <<< GENERATED>>>"),
                existing.find("// <<< GENERATED END >>>"),
            ) {
                let before = &existing[..begin_pos].trim_end();
                let after = &existing[end_pos + "// <<< GENERATED END >>>".len()..].trim_start();
                if before.is_empty() && after.is_empty() {
                    *mod_content = generated_with_markers;
                } else if before.is_empty() {
                    *mod_content = format!("{}\n\n{}", generated_with_markers.trim(), after);
                } else if after.is_empty() {
                    *mod_content = format!("{}\n\n{}", before, generated_with_markers.trim());
                } else {
                    *mod_content = format!(
                        "{}\n\n{}\n\n{}",
                        before,
                        generated_with_markers.trim(),
                        after
                    );
                }
            } else if !existing.trim().is_empty() {
                *mod_content = format!("{}\n\n{}", generated_with_markers.trim(), existing);
            } else {
                *mod_content = generated_with_markers;
            }
        }
    }

    /// Sync the per-peripheral init modules under `src/pins/configs/` from the
    /// codegen output `files = (file_name, generated_body)` (one per configured
    /// USART/SPI/I2C). Mirrors [`sync_pin_files`]: registers the folder, ensures
    /// `configs/mod.rs`, drops orphaned files, splices each file's GENERATED
    /// block (preserving user code outside it), and rebuilds `configs/mod.rs`
    /// with `pub mod <periph>;`. When `files` is empty the whole subtree is
    /// dropped. Call this BEFORE `sync_pin_files` so the latter can add
    /// `pub mod configs;` to `pins/mod.rs`.
    pub fn sync_config_files(&mut self, files: &[(String, String)]) {
        const DIR: &str = "pins/configs";
        const MOD_PATH: &str = "pins/configs/mod.rs";
        const GEN_BEGIN: &str = "// <<< GENERATED>>>";
        const GEN_END: &str = "// <<< GENERATED END >>>";

        if files.is_empty() {
            // No configured peripherals → drop the entire configs/ subtree.
            self.user_src_files
                .retain(|(p, _)| !p.starts_with("pins/configs/"));
            self.user_src_folders.retain(|f| f != DIR);
            return;
        }

        // 1. Register the folder.
        if !self.user_src_folders.iter().any(|f| f == DIR) {
            self.user_src_folders.push(DIR.to_string());
        }
        // 2. Ensure configs/mod.rs exists.
        if !self.user_src_files.iter().any(|(p, _)| p == MOD_PATH) {
            self.user_src_files.push((MOD_PATH.to_string(), String::new()));
        }

        // Active module stems (file names without `.rs`).
        let active: Vec<String> = files
            .iter()
            .map(|(name, _)| name.trim_end_matches(".rs").to_string())
            .collect();

        // 3. Drop config files no longer configured.
        self.user_src_files.retain(|(path, _)| {
            let Some(rest) = path.strip_prefix("pins/configs/") else {
                return true;
            };
            rest == "mod.rs" || active.iter().any(|a| a == rest.trim_end_matches(".rs"))
        });

        // 4. Create / update each config file. The codegen `body` already wraps
        //    ONLY the constants in `// <<< GENERATED>>>` markers; everything below
        //    (use block + get_config/init) is editable. On update we re-splice
        //    just that constants block, so the user's edits to the rest survive.
        for (name, body) in files {
            let file_path = format!("pins/configs/{name}");
            if let Some((_, content)) =
                self.user_src_files.iter_mut().find(|(p, _)| p == &file_path)
            {
                if let Some(block) = extract_gen_block(body) {
                    let existing = content.clone();
                    let updated = splice_pin_file(&existing, &block);
                    if *content != updated {
                        *content = updated;
                    }
                }
            } else {
                // New file: write the full generated content (consts + editable
                // remainder).
                self.user_src_files.push((file_path, body.clone()));
            }
        }

        // 5. Rebuild configs/mod.rs (`pub mod usart1;` …), preserving user code.
        let gen_section: String = active.iter().map(|s| format!("pub mod {s};\n")).collect();
        let wrapped_mod = format!("{GEN_BEGIN}\n{}\n{GEN_END}\n", gen_section.trim());
        if let Some((_, mod_content)) =
            self.user_src_files.iter_mut().find(|(p, _)| p == MOD_PATH)
        {
            let existing = mod_content.clone();
            let updated = splice_pin_file(&existing, &wrapped_mod);
            if *mod_content != updated {
                *mod_content = updated;
            }
        }
    }

    /// Initialize the pins/ scaffold (folder + empty mod.rs).
    pub fn init_pins_scaffold(&mut self) {
        let folder = "pins".to_string();
        let mod_path = "pins/mod.rs".to_string();
        if !self.user_src_folders.contains(&folder) {
            self.user_src_folders.push(folder);
        }
        if !self.user_src_files.iter().any(|(p, _)| p == &mod_path) {
            self.user_src_files.push((mod_path, String::new()));
        }
    }
}

/// Replace the GENERATED section in a pin file while preserving user code.
/// If no markers exist, wraps the new content and appends any existing code.
/// The `// <<< GENERATED>>> … // <<< GENERATED END >>>` block of `content`
/// (markers included), or `None` when absent. Used to splice just the constants
/// block of a config file, leaving the editable remainder untouched.
fn extract_gen_block(content: &str) -> Option<String> {
    const GEN_BEGIN: &str = "// <<< GENERATED>>>";
    const GEN_END: &str = "// <<< GENERATED END >>>";
    let begin = content.find(GEN_BEGIN)?;
    let end = content.find(GEN_END)? + GEN_END.len();
    Some(content[begin..end].to_string())
}

fn splice_pin_file(existing: &str, new_generated: &str) -> String {
    const GEN_BEGIN: &str = "// <<< GENERATED>>>";
    const GEN_END: &str = "// <<< GENERATED END >>>";

    if let (Some(begin_pos), Some(end_pos)) = (existing.find(GEN_BEGIN), existing.find(GEN_END)) {
        let before = &existing[..begin_pos].trim_end();
        let after_end = end_pos + GEN_END.len();
        let after = &existing[after_end..].trim_start();

        // Rebuild with new generated section
        if before.is_empty() && after.is_empty() {
            new_generated.to_string()
        } else if before.is_empty() {
            format!("{}\n\n{}", new_generated.trim(), after)
        } else if after.is_empty() {
            format!("{}\n\n{}", before, new_generated.trim())
        } else {
            format!(
                "{}\n\n{}\n\n{}",
                before,
                new_generated.trim(),
                after
            )
        }
    } else {
        // No markers found: wrap and append existing code
        if existing.trim().is_empty() {
            new_generated.to_string()
        } else {
            format!("{}\n\n{}", new_generated.trim(), existing)
        }
    }
}

/// Filesystem event type.
#[derive(Debug, Clone)]
pub enum FsEventKind {
    Create,
    Remove,
    Rename { old_rel: String, new_rel: String },
}

/// Generate HAL code for a pin type alias.
fn generate_pin_content(pin_num: usize, pin_name: &str, func: &PinFunction) -> String {
    let Some(mode) = func.hal_gpio_mode() else {
        return String::new();
    };

    // Parse STM32 pin name
    let upper = pin_name.to_uppercase();
    let mut chars = upper.chars();
    if chars.next() != Some('P') {
        return format!(
            "// Pin {pin_num} — {pin_name}\n// Function: {label}\n",
            label = func.label()
        );
    }
    let port = match chars.next() {
        Some(c) => c,
        None => {
            return format!(
                "// Pin {pin_num} — {pin_name}\n// Function: {label}\n",
                label = func.label()
            );
        }
    };
    let idx_str: String = chars.collect();
    let Ok(idx) = idx_str.parse::<u8>() else {
        return format!(
            "// Pin {pin_num} — {pin_name}\n// Function: {label}\n",
            label = func.label()
        );
    };

    let comment = match func {
        PinFunction::GpioInput | PinFunction::GpioOutput => String::new(),
        other => format!(" // {}", other.label()),
    };

    format!(
        "use stm32f1xx_hal::gpio::{{{mode}, Pin}};\n\
         pub type PinType = Pin<'{port}', {idx}, {mode}>;{comment}\n",
    )
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── Helper macros and functions ──────────────────────────────────────────

    macro_rules! setup_temp_project {
        () => {{
            let temp = TempDir::new().unwrap();
            let src = temp.path().join("src");
            fs::create_dir(&src).unwrap();
            (temp, src)
        }};
    }

    fn assert_file_exists(state: &ProjectTreeState, path: &str) {
        assert!(
            state.user_src_files.iter().any(|(p, _)| p == path),
            "File {} not found in state",
            path
        );
    }

    fn assert_file_not_exists(state: &ProjectTreeState, path: &str) {
        assert!(
            !state.user_src_files.iter().any(|(p, _)| p == path),
            "File {} found but shouldn't exist",
            path
        );
    }

    fn assert_file_content(state: &ProjectTreeState, path: &str, expected: &str) {
        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == path)
            .expect(&format!("File {} not found", path));
        assert_eq!(entry.1, expected, "Content mismatch for {}", path);
    }

    fn assert_folder_exists(state: &ProjectTreeState, path: &str) {
        assert!(
            state.user_src_folders.contains(&path.to_string()),
            "Folder {} not found",
            path
        );
    }

    // ── Initialization Tests ─────────────────────────────────────────────────

    #[test]
    fn test_new_empty_state() {
        let state = ProjectTreeState::new();
        assert!(state.user_src_files.is_empty());
        assert!(state.user_src_folders.is_empty());
    }

    #[test]
    fn test_load_empty_directory() {
        let (_temp, src) = setup_temp_project!();
        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);
        assert!(state.user_src_files.is_empty());
        assert!(state.user_src_folders.is_empty());
    }

    #[test]
    fn test_load_nonexistent_path() {
        let nonexistent = PathBuf::from("/nonexistent/path/123456");
        let state = ProjectTreeState::load_from_dir(&nonexistent);
        assert!(state.user_src_files.is_empty());
        assert!(state.user_src_folders.is_empty());
    }

    // ── File Discovery Tests ─────────────────────────────────────────────────

    #[test]
    fn test_load_discovers_single_file() {
        let (_temp, src) = setup_temp_project!();
        fs::write(src.join("utils.rs"), "pub fn helper() {}").unwrap();

        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);

        assert_eq!(state.user_src_files.len(), 1);
        assert_file_exists(&state, "utils.rs");
        assert_file_content(&state, "utils.rs", "pub fn helper() {}");
    }

    #[test]
    fn test_load_discovers_nested_files() {
        let (_temp, src) = setup_temp_project!();
        fs::create_dir(src.join("helpers")).unwrap();
        fs::write(src.join("helpers/math.rs"), "").unwrap();
        fs::write(src.join("helpers/strings.rs"), "").unwrap();

        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);

        assert_file_exists(&state, "helpers/math.rs");
        assert_file_exists(&state, "helpers/strings.rs");
    }

    #[test]
    fn test_load_discovers_all_file_types() {
        let (_temp, src) = setup_temp_project!();
        fs::write(src.join("file.rs"), "").unwrap();
        fs::write(src.join("config.txt"), "").unwrap();
        fs::write(src.join("data.json"), "").unwrap();

        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);

        assert_file_exists(&state, "file.rs");
        assert_file_exists(&state, "config.txt");
        assert_file_exists(&state, "data.json");
    }

    #[test]
    fn test_load_skips_main_rs() {
        let (_temp, src) = setup_temp_project!();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        fs::write(src.join("utils.rs"), "").unwrap();

        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);

        assert_file_not_exists(&state, "main.rs");
        assert_file_exists(&state, "utils.rs");
    }

    #[test]
    fn test_load_registers_folders() {
        let (_temp, src) = setup_temp_project!();
        fs::create_dir(src.join("helpers")).unwrap();
        fs::create_dir(src.join("core")).unwrap();
        fs::write(src.join("helpers/file.txt"), "").unwrap();
        fs::write(src.join("core/file.txt"), "").unwrap();

        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);

        assert_folder_exists(&state, "helpers");
        assert_folder_exists(&state, "core");
    }

    #[test]
    fn test_load_normalizes_paths() {
        let (_temp, src) = setup_temp_project!();
        fs::create_dir(src.join("utils")).unwrap();
        fs::write(src.join("utils/helper.rs"), "").unwrap();

        let parent = src.parent().unwrap();
        let state = ProjectTreeState::load_from_dir(parent);

        // All paths should use forward slashes
        for (path, _) in &state.user_src_files {
            assert!(!path.contains('\\'), "Path contains backslashes: {}", path);
        }
    }

    // ── Filesystem Events Tests ──────────────────────────────────────────────

    #[test]
    fn test_handle_create_event() {
        let mut state = ProjectTreeState::new();
        state.handle_fs_events(vec![("utils.rs".to_string(), FsEventKind::Create)]);

        assert_eq!(state.user_src_files.len(), 1);
        assert_file_exists(&state, "utils.rs");
        assert_file_content(&state, "utils.rs", "");
    }

    #[test]
    fn test_handle_create_event_skips_duplicates() {
        let mut state = ProjectTreeState::new();
        state
            .user_src_files
            .push(("utils.rs".to_string(), "existing content".to_string()));

        state.handle_fs_events(vec![("utils.rs".to_string(), FsEventKind::Create)]);

        assert_eq!(state.user_src_files.len(), 1);
        assert_file_content(&state, "utils.rs", "existing content");
    }

    #[test]
    fn test_handle_remove_file_event() {
        let mut state = ProjectTreeState::new();
        state
            .user_src_files
            .push(("utils.rs".to_string(), "content".to_string()));

        state.handle_fs_events(vec![("utils.rs".to_string(), FsEventKind::Remove)]);

        assert!(state.user_src_files.is_empty());
    }

    #[test]
    fn test_handle_remove_folder_event() {
        let mut state = ProjectTreeState::new();
        state.user_src_folders.push("helpers".to_string());
        state
            .user_src_files
            .push(("helpers/math.rs".to_string(), "".to_string()));
        state
            .user_src_files
            .push(("utils.rs".to_string(), "".to_string()));

        // When a folder is removed, the folder entry itself is removed
        state.handle_fs_events(vec![("helpers".to_string(), FsEventKind::Remove)]);

        assert!(!state.user_src_folders.contains(&"helpers".to_string()));
        // Note: child files are not automatically removed by the current implementation
        // In practice, they are removed by individual Remove events from filesystem watcher
        assert_file_exists(&state, "utils.rs");
    }

    #[test]
    fn test_handle_rename_file() {
        let mut state = ProjectTreeState::new();
        state
            .user_src_files
            .push(("old.rs".to_string(), "content".to_string()));

        state.handle_fs_events(vec![(
            "old.rs".to_string(),
            FsEventKind::Rename {
                old_rel: "old.rs".to_string(),
                new_rel: "new.rs".to_string(),
            },
        )]);

        assert_file_not_exists(&state, "old.rs");
        assert_file_exists(&state, "new.rs");
        assert_file_content(&state, "new.rs", "content");
    }

    #[test]
    fn test_handle_rename_folder_updates_children() {
        let mut state = ProjectTreeState::new();
        state.user_src_folders.push("oldname".to_string());
        state
            .user_src_files
            .push(("oldname/file1.rs".to_string(), "".to_string()));
        state
            .user_src_files
            .push(("oldname/file2.rs".to_string(), "".to_string()));

        state.handle_fs_events(vec![(
            "oldname".to_string(),
            FsEventKind::Rename {
                old_rel: "oldname".to_string(),
                new_rel: "newname".to_string(),
            },
        )]);

        assert_folder_exists(&state, "newname");
        assert_file_not_exists(&state, "oldname/file1.rs");
        assert_file_not_exists(&state, "oldname/file2.rs");
        assert_file_exists(&state, "newname/file1.rs");
        assert_file_exists(&state, "newname/file2.rs");
    }

    #[test]
    fn test_handle_multiple_events_sequence() {
        let mut state = ProjectTreeState::new();

        // Create
        state.handle_fs_events(vec![("a.rs".to_string(), FsEventKind::Create)]);
        assert_eq!(state.user_src_files.len(), 1);

        // Rename
        state.handle_fs_events(vec![(
            "a.rs".to_string(),
            FsEventKind::Rename {
                old_rel: "a.rs".to_string(),
                new_rel: "b.rs".to_string(),
            },
        )]);
        assert_file_not_exists(&state, "a.rs");
        assert_file_exists(&state, "b.rs");

        // Remove
        state.handle_fs_events(vec![("b.rs".to_string(), FsEventKind::Remove)]);
        assert!(state.user_src_files.is_empty());
    }

    // ── Pin Synchronization Tests ────────────────────────────────────────────

    #[test]
    fn test_sync_creates_pins_folder() {
        let mut state = ProjectTreeState::new();
        state.sync_pin_files(&[]);

        assert_folder_exists(&state, "pins");
    }

    #[test]
    fn test_sync_creates_mod_rs() {
        let mut state = ProjectTreeState::new();
        state.sync_pin_files(&[]);

        assert_file_exists(&state, "pins/mod.rs");
    }

    #[test]
    fn test_sync_generates_gpio_output_pin() {
        let mut state = ProjectTreeState::new();
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        assert_file_exists(&state, "pins/pin1_pa0_out.rs");
        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/pin1_pa0_out.rs")
            .unwrap();
        assert!(entry.1.contains("pub type PinType = Pin<'A', 0,"));
    }

    #[test]
    fn test_sync_pin_file_name_includes_type() {
        let mut state = ProjectTreeState::new();
        let pins = vec![
            (2usize, "PC13".to_string(), PinFunction::GpioOutput),
            (10usize, "PA0".to_string(), PinFunction::GpioInput),
            (11usize, "PA1".to_string(), PinFunction::AdcChannel { adc: 1, channel: 1 }),
            (30usize, "PA9".to_string(), PinFunction::TimerPwm { timer: 1, channel: 2 }),
        ];
        state.sync_pin_files(&pins);

        // File names carry the selected function type (e.g. pin2_pc13_out.rs).
        assert_file_exists(&state, "pins/pin2_pc13_out.rs");
        assert_file_exists(&state, "pins/pin10_pa0_in.rs");
        assert_file_exists(&state, "pins/pin11_pa1_adc.rs");
        assert_file_exists(&state, "pins/pin30_pa9_pwm.rs");

        // mod.rs declares the type-suffixed modules.
        let mod_file = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/mod.rs")
            .unwrap();
        assert!(mod_file.1.contains("pub mod pin2_pc13_out;"));
        assert!(mod_file.1.contains("pub mod pin30_pa9_pwm;"));
    }

    #[test]
    fn test_sync_removes_old_pins() {
        let mut state = ProjectTreeState::new();

        // Add old pins (using the type-suffixed naming convention)
        state
            .user_src_files
            .push(("pins/pin1_pa0_out.rs".to_string(), "".to_string()));
        state
            .user_src_files
            .push(("pins/pin2_pa1_out.rs".to_string(), "".to_string()));
        state
            .user_src_folders
            .push("pins".to_string());
        state
            .user_src_files
            .push(("pins/mod.rs".to_string(), "".to_string()));

        // Sync with only pin1 configured
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        assert_file_exists(&state, "pins/pin1_pa0_out.rs");
        assert_file_not_exists(&state, "pins/pin2_pa1_out.rs");
    }

    #[test]
    fn test_sync_preserves_custom_code() {
        let mut state = ProjectTreeState::new();
        let custom_code = "pub mod custom_utils;\npub fn helper() {}";
        let mod_content = format!(
            "// <<< GENERATED>>>\npub mod pin1_pa0_out;\n// <<< GENERATED END >>>\n\n{}",
            custom_code
        );
        state
            .user_src_files
            .push(("pins/mod.rs".to_string(), mod_content));
        state.user_src_folders.push("pins".to_string());

        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        let mod_file = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/mod.rs")
            .unwrap();
        assert!(mod_file.1.contains("pub mod custom_utils;"));
        assert!(mod_file.1.contains("pub fn helper() {}"));
    }

    #[test]
    fn test_sync_ignores_unset_pins() {
        let mut state = ProjectTreeState::new();
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::Unset)];
        state.sync_pin_files(&pins);

        assert_file_not_exists(&state, "pins/pin1_pa0.rs");
        let mod_file = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/mod.rs")
            .unwrap();
        assert!(mod_file.1.trim().is_empty() || !mod_file.1.contains("pub mod"));
    }

    #[test]
    fn test_sync_mod_rs_empty_to_generated() {
        let mut state = ProjectTreeState::new();
        state
            .user_src_files
            .push(("pins/mod.rs".to_string(), "".to_string()));
        state.user_src_folders.push("pins".to_string());

        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        let mod_file = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/mod.rs")
            .unwrap();
        assert!(mod_file.1.contains("// <<< GENERATED>>>"));
        assert!(mod_file.1.contains("pub mod pin1_pa0_out;"));
    }

    #[test]
    fn test_init_pins_scaffold() {
        let mut state = ProjectTreeState::new();
        state.init_pins_scaffold();

        assert_folder_exists(&state, "pins");
        assert_file_exists(&state, "pins/mod.rs");
    }

    #[test]
    fn test_init_pins_scaffold_idempotent() {
        let mut state = ProjectTreeState::new();
        state.init_pins_scaffold();
        let count_before = state.user_src_folders.len() + state.user_src_files.len();

        state.init_pins_scaffold();
        let count_after = state.user_src_folders.len() + state.user_src_files.len();

        assert_eq!(count_before, count_after, "Scaffold should be idempotent");
    }

    #[test]
    fn test_sync_updates_pintype_on_function_change() {
        let mut state = ProjectTreeState::new();

        // Start with PA0 as GPIO Output
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/pin1_pa0_out.rs")
            .unwrap();
        assert!(entry.1.contains("pub type PinType = Pin<'A', 0, Output>;"));

        // Change to ADC (Analog) — the file renames to the new type suffix.
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::AdcChannel { adc: 1, channel: 0 })];
        state.sync_pin_files(&pins);

        assert_file_not_exists(&state, "pins/pin1_pa0_out.rs");
        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/pin1_pa0_adc.rs")
            .unwrap();
        // After function change, the file should be regenerated with new type
        assert!(entry.1.contains("pub type PinType = Pin<'A', 0, Analog>;"));
        assert!(!entry.1.contains("Output"), "Old Output type should be removed");
    }

    #[test]
    fn test_sync_updates_pin_comment_on_function_change() {
        let mut state = ProjectTreeState::new();

        // Start with GPIO Output (no function comment on type alias)
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/pin1_pa0_out.rs")
            .unwrap();
        // GPIO pins should have PinType line without function comment
        assert!(
            entry.1.contains("pub type PinType = Pin<'A', 0, Output>;"),
            "GPIO pins should have no function comment on PinType: {}",
            entry.1
        );

        // Change to ADC with function label
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::AdcChannel { adc: 1, channel: 0 })];
        state.sync_pin_files(&pins);

        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/pin1_pa0_adc.rs")
            .unwrap();
        // ADC should have a comment with the function label on PinType line
        assert!(
            entry.1.contains("pub type PinType = Pin<'A', 0, Analog>; // ADC1  IN0"),
            "ADC pin should have function label comment on PinType: {}",
            entry.1
        );
    }

    /// A config file's constants (inside the GENERATED block) regenerate on a
    /// config change, but the editable remainder (use block + init fns the user
    /// may have edited) is preserved.
    #[test]
    fn config_file_constants_regenerate_body_preserved() {
        let mut state = ProjectTreeState::new();
        let path = "pins/configs/usart1.rs";
        let v1 = "// <<< GENERATED>>>\nconst BAUDRATE: u32 = 115200;\n// <<< GENERATED END >>>\n\nuse foo;\npub fn init() { /* orig */ }\n";
        state.sync_config_files(&[("usart1.rs".to_string(), v1.to_string())]);
        assert!(state.user_src_files.iter().any(|(p, _)| p == path));

        // User edits the EDITABLE part (below the markers).
        {
            let f = state.user_src_files.iter_mut().find(|(p, _)| p == path).unwrap();
            f.1 = f.1.replace("/* orig */", "/* MY EDIT */");
        }

        // Regenerate with a new baud rate (only the constants block changes).
        let v2 = "// <<< GENERATED>>>\nconst BAUDRATE: u32 = 9600;\n// <<< GENERATED END >>>\n\nuse foo;\npub fn init() { /* orig */ }\n";
        state.sync_config_files(&[("usart1.rs".to_string(), v2.to_string())]);

        let body = &state.user_src_files.iter().find(|(p, _)| p == path).unwrap().1;
        assert!(body.contains("const BAUDRATE: u32 = 9600;"), "const updated:\n{body}");
        assert!(!body.contains("115200"), "old const gone");
        assert!(body.contains("/* MY EDIT */"), "user body edit preserved:\n{body}");
    }
}
