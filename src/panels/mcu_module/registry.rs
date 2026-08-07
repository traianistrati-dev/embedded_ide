//! MCU registry loader (Phase 5).
//!
//! Assembles the list of available [`McuDefinition`]s from two sources:
//! 1. **Built-ins** bundled in the binary via `include_str!`
//!    ([`builtins::builtin_definitions`]).
//! 2. **User imports** — `.ron` files dropped into a per-user `mcus/` folder
//!    (`%APPDATA%/embedded_ide_0/mcus` on Windows, `$XDG_CONFIG_HOME` /
//!    `~/.config` elsewhere). These load at startup with **no recompile**, so
//!    new chips inside an already-supported family are pure data.
//!
//! User definitions with the same `id` as a built-in **override** it, letting
//! a user patch a bundled chip without touching the binary.

use std::path::{Path, PathBuf};

use super::builtins;
use super::mcu_def::McuDefinition;

/// Per-user folder scanned for imported `.ron` definitions.
///
/// Returns `None` only if no suitable home/config dir can be resolved (rare);
/// callers then fall back to built-ins only.
pub fn user_mcus_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA") // Windows
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)) // Linux
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))); // Unix
    base.map(|b| b.join("embedded_ide_0").join("mcus"))
}

/// Build the full registry: built-ins first, then user `.ron` imports merged on
/// top (same-`id` user files override the built-in entry).
pub fn load_registry() -> Vec<McuDefinition> {
    load_registry_from(user_mcus_dir().as_deref())
}

/// Core loader, parameterised on the scan directory so it is unit-testable
/// without touching the real per-user folder. `None` (or a missing dir) yields
/// the built-ins only.
fn load_registry_from(user_dir: Option<&Path>) -> Vec<McuDefinition> {
    let mut defs = builtins::builtin_definitions();

    if let Some(dir) = user_dir {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("ron") {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(def) = parse_definition(&text) {
                        merge_def(&mut defs, def);
                    }
                    // Silently skip files that fail to parse — a malformed user
                    // file must never break startup.
                }
            }
        }
    }

    defs
}

/// Parse + validate a definition from RON text. Pure (no I/O) so it is unit
/// testable and reused by both [`load_registry`] and [`import_file`].
pub fn parse_definition(text: &str) -> Result<McuDefinition, String> {
    let def: McuDefinition = ron::from_str(text).map_err(|e| format!("RON parse error: {e}"))?;
    if def.id.trim().is_empty() {
        return Err("definition has an empty `id`".to_owned());
    }
    if def.display_name.trim().is_empty() {
        return Err("definition has an empty `display_name`".to_owned());
    }
    Ok(def)
}

/// Insert `def` into `defs`, overriding any existing entry with the same `id`.
pub fn merge_def(defs: &mut Vec<McuDefinition>, def: McuDefinition) {
    if let Some(slot) = defs.iter_mut().find(|d| d.id == def.id) {
        *slot = def;
    } else {
        defs.push(def);
    }
}

/// Open the user `mcus/` folder in the OS file manager, creating it first so
/// the user always has a place to drop `.ron` files. Best-effort: any failure
/// is ignored (the path is also shown in the UI for manual navigation).
pub fn open_user_mcus_dir() {
    if let Some(dir) = user_mcus_dir() {
        let _ = std::fs::create_dir_all(&dir);
        open_in_file_manager(&dir);
    }
}

#[cfg(target_os = "windows")]
fn open_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

#[cfg(target_os = "macos")]
fn open_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("open").arg(path).spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Import a `.ron` file chosen by the user: parse it and persist a copy into the
/// user `mcus/` folder so it survives restarts. Returns the parsed definition
/// (the caller merges it into the live registry).
///
/// Persistence failures are non-fatal: the chip is still usable this session.
pub fn import_file(path: &Path) -> Result<McuDefinition, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    let def = parse_definition(&text)?;

    if let Some(dir) = user_mcus_dir() {
        if std::fs::create_dir_all(&dir).is_ok() {
            // Name the persisted copy by id so re-imports overwrite cleanly.
            let dest = dir.join(format!("{}.ron", def.id));
            let _ = std::fs::write(&dest, &text);
        }
    }

    Ok(def)
}

/// Persist a form-authored definition to `<user mcus>/<id>.ron` (the same
/// folder imports live in, so it survives restarts and re-authoring an id
/// overwrites cleanly). Returns the written path. Fails only on I/O errors or
/// when no user dir resolves.
pub fn save_definition(def: &McuDefinition) -> Result<PathBuf, String> {
    let dir = user_mcus_dir().ok_or("could not resolve the user config folder")?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let text = ron::ser::to_string_pretty(def, ron::ser::PrettyConfig::default())
        .map_err(|e| format!("RON serialize error: {e}"))?;
    let dest = dir.join(format!("{}.ron", def.id));
    std::fs::write(&dest, text).map_err(|e| format!("could not write {}: {e}", dest.display()))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_registry_includes_builtins() {
        let defs = load_registry();
        assert!(defs.iter().any(|d| d.id == "stm32f103c8t6"));
        assert!(defs.iter().any(|d| d.id == "esp32c3"));
    }

    #[test]
    fn parse_definition_round_trips_a_builtin() {
        let original = builtins::builtin_for("stm32f103c8t6").unwrap();
        let text = ron::to_string(&original).unwrap();
        let parsed = parse_definition(&text).expect("valid definition parses");
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_definition_rejects_empty_id() {
        let mut def = builtins::builtin_for("esp32c3").unwrap();
        def.id = "  ".to_owned();
        let text = ron::to_string(&def).unwrap();
        assert!(parse_definition(&text).is_err());
    }

    #[test]
    fn parse_definition_rejects_garbage() {
        assert!(parse_definition("not ron at all {{{").is_err());
    }

    #[test]
    fn merge_def_overrides_same_id() {
        let mut defs = builtins::builtin_definitions();
        let before = defs.len();

        // Same id → replace, length unchanged.
        let mut patched = builtins::builtin_for("stm32f103c8t6").unwrap();
        patched.display_name = "PATCHED".to_owned();
        merge_def(&mut defs, patched);
        assert_eq!(defs.len(), before);
        assert_eq!(
            defs.iter()
                .find(|d| d.id == "stm32f103c8t6")
                .unwrap()
                .display_name,
            "PATCHED"
        );
    }

    #[test]
    fn merge_def_appends_new_id() {
        let mut defs = builtins::builtin_definitions();
        let before = defs.len();

        let mut fresh = builtins::builtin_for("stm32f103c8t6").unwrap();
        fresh.id = "stm32f103rb".to_owned();
        merge_def(&mut defs, fresh);
        assert_eq!(defs.len(), before + 1);
        assert!(defs.iter().any(|d| d.id == "stm32f103rb"));
    }

    /// End-to-end scan: a `.ron` dropped into the user folder is discovered,
    /// parsed, merged, and builds into a runtime `Mcu`. Uses a temp dir so the
    /// real per-user folder is never touched.
    #[test]
    fn scan_picks_up_user_ron_file() {
        let dir = tempfile::tempdir().unwrap();

        // A user-authored variant in the already-supported STM32F1 family.
        let mut def = builtins::builtin_for("stm32f103c8t6").unwrap();
        def.id = "stm32f103rb".to_owned();
        def.display_name = "STM32F103RB (imported)".to_owned();
        let text = ron::to_string(&def).unwrap();
        std::fs::write(dir.path().join("stm32f103rb.ron"), &text).unwrap();
        // A non-RON file must be ignored, not break the scan.
        std::fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();
        // A malformed RON must be skipped silently.
        std::fs::write(dir.path().join("broken.ron"), "garbage {{{").unwrap();

        let defs = load_registry_from(Some(dir.path()));

        // Built-ins still present + the imported chip added.
        assert!(defs.iter().any(|d| d.id == "stm32f103c8t6"));
        assert!(defs.iter().any(|d| d.id == "esp32c3"));
        let imported = defs
            .iter()
            .find(|d| d.id == "stm32f103rb")
            .expect("imported chip must appear in the registry");
        assert_eq!(imported.display_name, "STM32F103RB (imported)");

        // And it builds into a usable runtime Mcu.
        let mcu = imported.build_mcu();
        assert!(mcu.iter_all_pins().count() > 0);
    }

    #[test]
    fn missing_user_dir_yields_builtins_only() {
        let baseline = builtins::builtin_definitions().len();
        let defs = load_registry_from(Some(Path::new("Z:/no/such/dir/exists")));
        assert_eq!(defs.len(), baseline);
    }
}
