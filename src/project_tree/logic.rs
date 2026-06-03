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
                let slug = format!("pin{}_{}", num, name.to_lowercase());
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

        // 4. Create pin files that don't yet exist
        for (slug, num, name, func) in &configured {
            let file_path = format!("pins/{slug}.rs");
            if !self.user_src_files.iter().any(|(p, _)| p == &file_path) {
                let content = generate_pin_content(*num, name, func);
                self.user_src_files.push((file_path, content));
            }
        }

        // 5. Rebuild mod.rs (preserve custom code outside GENERATED section)
        let generated_section: String = configured
            .iter()
            .map(|(slug, ..)| format!("pub mod {slug};\n"))
            .collect();

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

        assert_file_exists(&state, "pins/pin1_pa0.rs");
        let entry = state
            .user_src_files
            .iter()
            .find(|(p, _)| p == "pins/pin1_pa0.rs")
            .unwrap();
        assert!(entry.1.contains("pub type PinType = Pin<'A', 0,"));
    }

    #[test]
    fn test_sync_removes_old_pins() {
        let mut state = ProjectTreeState::new();

        // Add old pins
        state
            .user_src_files
            .push(("pins/pin1_pa0.rs".to_string(), "".to_string()));
        state
            .user_src_files
            .push(("pins/pin2_pa1.rs".to_string(), "".to_string()));
        state
            .user_src_folders
            .push("pins".to_string());
        state
            .user_src_files
            .push(("pins/mod.rs".to_string(), "".to_string()));

        // Sync with only pin1 configured
        let pins = vec![(1usize, "PA0".to_string(), PinFunction::GpioOutput)];
        state.sync_pin_files(&pins);

        assert_file_exists(&state, "pins/pin1_pa0.rs");
        assert_file_not_exists(&state, "pins/pin2_pa1.rs");
    }

    #[test]
    fn test_sync_preserves_custom_code() {
        let mut state = ProjectTreeState::new();
        let custom_code = "pub mod custom_utils;\npub fn helper() {}";
        let mod_content = format!(
            "// <<< GENERATED>>>\npub mod pin1_pa0;\n// <<< GENERATED END >>>\n\n{}",
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
        assert!(mod_file.1.contains("pub mod pin1_pa0;"));
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
}
