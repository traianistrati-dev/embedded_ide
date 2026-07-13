//! The `mcu.config` file — out-of-source persistence of the MCU's virtual
//! modules (`@modules`) and clock-tree config (`@clock`). Written to the project
//! root (next to Cargo.toml), created automatically, and NOT shown in the
//! project tree. Replaces the old one-line `// @modules` / `// @clock` comment
//! markers that lived inside `main.rs`.
//!
//! Format — each section header on its own line, the body multi-line:
//! ```text
//! @modules
//! [
//!     (id: "gi_i2c_1", kind: GenericInterfaceI2c, …),
//! ]
//!
//! @clock
//! hse=8000000
//! hse_on=0
//! …
//! ```

use super::clock::model::Stm32f1Clock;
use super::clock::persist as clock_persist;
use super::modules::VirtualModule;

const MODULES_HEADER: &str = "@modules";
const CLOCK_HEADER: &str = "@clock";
const STRUCTURE_HEADER: &str = "@structure_layout";

/// File name written at the project root.
pub const FILE_NAME: &str = "mcu.config";

/// Manually dragged node positions of the Structure diagram, keyed by the
/// module's workspace-relative file (`"mw_radar/utils.rs"`, `"main.rs"`) —
/// stable across graph rebuilds, unlike node indices. BTreeMap so the
/// serialized section is deterministic (the mtime-stable project writes rely
/// on unchanged content staying byte-identical).
pub type StructurePositions = std::collections::BTreeMap<String, (f32, f32)>;

/// Structure-tab view options persisted per project:
/// `(show_calls, call_depth, path_style as u8, show_externals)`.
pub type StructureViewPersist = (bool, Option<usize>, u8, bool);
const VIEW_HEADER: &str = "@structure_view";

/// The `@structure_view` section text (always emitted — tiny and stable).
pub fn structure_view_section(v: &StructureViewPersist) -> String {
    let body = ron::to_string(v).unwrap_or_default();
    format!("{VIEW_HEADER}\n{body}\n")
}

/// Parse the `@structure_view` section back (absent/garbled → `None`, the
/// caller keeps its defaults — projects saved before this feature).
pub fn parse_structure_view(text: &str) -> Option<StructureViewPersist> {
    section_body(text, VIEW_HEADER)
        .and_then(|body| ron::from_str::<StructureViewPersist>(body.trim()).ok())
}

/// The `@structure_layout` section text for `positions` (empty map → "").
/// Appended to [`serialize`]'s output by the app (the diagram isn't MCU state).
pub fn structure_layout_section(positions: &StructurePositions) -> String {
    if positions.is_empty() {
        return String::new();
    }
    let body = ron::ser::to_string_pretty(positions, ron::ser::PrettyConfig::new())
        .unwrap_or_else(|_| ron::to_string(positions).unwrap_or_else(|_| "{}".into()));
    format!("{STRUCTURE_HEADER}\n{body}\n")
}

/// Parse the `@structure_layout` section back (absent/garbled → empty map, so
/// projects saved before this feature load unchanged).
pub fn parse_structure_layout(text: &str) -> StructurePositions {
    section_body(text, STRUCTURE_HEADER)
        .and_then(|body| ron::from_str::<StructurePositions>(body.trim()).ok())
        .unwrap_or_default()
}

/// Build the `mcu.config` text from the MCU's `modules` and (STM32-only) clock.
/// Returns an empty string when there is nothing to persist (no modules and no
/// clock), so the caller can skip writing the file.
pub fn serialize(modules: &[VirtualModule], clock: Option<&Stm32f1Clock>) -> String {
    let mut out = String::new();

    if !modules.is_empty() {
        // Pretty RON: one module per block, each field on its own line (ron's
        // default omits struct names, matching the documented format).
        let pretty = ron::ser::to_string_pretty(&modules, ron::ser::PrettyConfig::new())
            .unwrap_or_else(|_| ron::to_string(&modules).unwrap_or_else(|_| "[]".into()));
        out.push_str(MODULES_HEADER);
        out.push('\n');
        out.push_str(&pretty);
        out.push('\n');
    }

    if let Some(c) = clock {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(CLOCK_HEADER);
        out.push('\n');
        out.push_str(&clock_persist::to_config_block(c));
        out.push('\n');
    }

    out
}

/// Parse an `mcu.config` file back into `(modules, clock)`. A missing or garbled
/// section yields an empty module list / `None` clock.
pub fn parse(text: &str) -> (Vec<VirtualModule>, Option<Stm32f1Clock>) {
    let modules = section_body(text, MODULES_HEADER)
        .and_then(|body| ron::from_str::<Vec<VirtualModule>>(body.trim()).ok())
        .unwrap_or_default();
    let clock = section_body(text, CLOCK_HEADER).map(|b| clock_persist::from_config_block(&b));
    (modules, clock)
}

/// The lines belonging to `header`: everything after the header line up to (but
/// excluding) the next `@`-prefixed section header, or EOF.
fn section_body(text: &str, header: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == header)?;
    let mut body: Vec<&str> = Vec::new();
    for &l in &lines[start + 1..] {
        if l.trim_start().starts_with('@') {
            break;
        }
        body.push(l);
    }
    Some(body.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::model::{Stm32f1Clock, SysclkSrc};
    use crate::panels::mcu_module::modules::{
        Connection, I2cModuleConfig, ModuleConfig, ModuleKind, ModuleSignal, VirtualModule,
    };

    fn sample_module() -> VirtualModule {
        let mut cfg = I2cModuleConfig::new(1);
        cfg.custom_label = "128x32 display".into();
        VirtualModule {
            id: "gi_i2c_1".into(),
            kind: ModuleKind::GenericInterfaceI2c,
            name: "GI_I2C1".into(),
            pos: (0.0, 0.0),
            config: ModuleConfig::I2c(cfg),
            connections: vec![
                Connection { signal: ModuleSignal::Scl, mcu_pin: 45 },
                Connection { signal: ModuleSignal::Sda, mcu_pin: 46 },
            ],
        }
    }

    #[test]
    fn modules_and_clock_round_trip() {
        let modules = vec![sample_module()];
        let clock = Stm32f1Clock {
            sysclk_src: SysclkSrc::Hsi,
            ..Stm32f1Clock::default()
        };
        let text = serialize(&modules, Some(&clock));

        // Headers + multi-line layout present.
        assert!(text.contains("@modules\n"));
        assert!(text.contains("@clock\n"));
        assert!(text.contains("hse=8000000"));
        assert!(text.lines().count() > 10, "must be multi-line:\n{text}");

        let (m2, c2) = parse(&text);
        assert_eq!(m2, modules, "modules round-trip");
        assert_eq!(c2, Some(clock), "clock round-trip");
    }

    #[test]
    fn empty_when_nothing_to_persist() {
        assert_eq!(serialize(&[], None), "");
    }

    #[test]
    fn clock_only_when_no_modules() {
        let text = serialize(&[], Some(&Stm32f1Clock::default()));
        assert!(!text.contains("@modules"));
        assert!(text.starts_with("@clock\n"));
        let (m, c) = parse(&text);
        assert!(m.is_empty());
        assert_eq!(c, Some(Stm32f1Clock::default()));
    }

    #[test]
    fn parse_ignores_garbage() {
        let (m, c) = parse("not a config file\n");
        assert!(m.is_empty());
        assert!(c.is_none());
    }

    #[test]
    fn structure_layout_round_trips_and_coexists() {
        let mut pos = StructurePositions::new();
        pos.insert("main.rs".into(), (14.0, 14.0));
        pos.insert("mw_radar/utils.rs".into(), (321.5, 208.0));

        // Appended after the MCU sections, every section still parses.
        let mut text = serialize(&[sample_module()], Some(&Stm32f1Clock::default()));
        text.push('\n');
        text.push_str(&structure_layout_section(&pos));

        assert_eq!(parse_structure_layout(&text), pos, "positions round-trip");
        let (m, c) = parse(&text);
        assert_eq!(m.len(), 1, "modules unaffected by the extra section");
        assert!(c.is_some(), "clock unaffected by the extra section");

        // Absent section (older projects) → empty map; empty map → no section.
        assert!(parse_structure_layout("@modules\n[]\n").is_empty());
        assert_eq!(structure_layout_section(&StructurePositions::new()), "");
    }
}
