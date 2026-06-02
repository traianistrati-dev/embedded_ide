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
