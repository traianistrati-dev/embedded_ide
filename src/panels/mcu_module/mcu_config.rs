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
use super::mcu::Runtime;
use super::modules::VirtualModule;

const MODULES_HEADER: &str = "@modules";
const CLOCK_HEADER: &str = "@clock";
const RUNTIME_HEADER: &str = "@runtime";

/// File name written at the project root.
pub const FILE_NAME: &str = "mcu.config";

// The Structure tab's `@structure_layout` / `@structure_view` sections used to
// live here too. They moved to `project_structure.config` (see
// [`super::structure_config`]) because they change on every node drag, which
// made this file — real, reviewable configuration — permanently dirty in Git.
// `section_body` stays shared: both files use the same `@section` layout, and
// the migration path reads the old sections straight out of this one.

/// Build the `mcu.config` text from the MCU's `modules`, (STM32-only) clock and
/// `runtime`. Returns an empty string when there is nothing to persist (no
/// modules, no clock, and the default Blocking runtime), so the caller can skip
/// writing the file.
pub fn serialize(
    modules: &[VirtualModule],
    clock: Option<&Stm32f1Clock>,
    runtime: Runtime,
) -> String {
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

    // Only persist a non-default runtime — a Blocking project keeps the file
    // free of the section, so old projects (and the common case) round-trip
    // byte-identically.
    if runtime != Runtime::Blocking {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(RUNTIME_HEADER);
        out.push('\n');
        out.push_str(runtime.as_token());
        out.push('\n');
    }

    out
}

/// Parse an `mcu.config` file back into `(modules, clock)`. A missing or garbled
/// section yields an empty module list / `None` clock. (Runtime is read
/// separately via [`parse_runtime`].)
pub fn parse(text: &str) -> (Vec<VirtualModule>, Option<Stm32f1Clock>) {
    let modules = section_body(text, MODULES_HEADER)
        .and_then(|body| ron::from_str::<Vec<VirtualModule>>(body.trim()).ok())
        .unwrap_or_default();
    let clock = section_body(text, CLOCK_HEADER).map(|b| clock_persist::from_config_block(&b));
    (modules, clock)
}

/// The project [`Runtime`] recorded in `@runtime`; a missing section (any
/// pre-async project) is the default [`Runtime::Blocking`].
pub fn parse_runtime(text: &str) -> Runtime {
    section_body(text, RUNTIME_HEADER)
        .map(|b| Runtime::from_token(&b))
        .unwrap_or_default()
}

/// The lines belonging to `header`: everything after the header line up to (but
/// excluding) the next `@`-prefixed section header, or EOF.
pub(super) fn section_body(text: &str, header: &str) -> Option<String> {
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
        let text = serialize(&modules, Some(&clock), Runtime::Blocking);

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
        assert_eq!(serialize(&[], None, Runtime::Blocking), "");
    }

    #[test]
    fn clock_only_when_no_modules() {
        let text = serialize(&[], Some(&Stm32f1Clock::default()), Runtime::Blocking);
        assert!(!text.contains("@modules"));
        assert!(text.starts_with("@clock\n"));
        let (m, c) = parse(&text);
        assert!(m.is_empty());
        assert_eq!(c, Some(Stm32f1Clock::default()));
    }

    #[test]
    fn runtime_round_trips_and_defaults_to_blocking() {
        // Default runtime writes NO section — old projects stay byte-identical.
        assert_eq!(serialize(&[], None, Runtime::Blocking), "");
        assert_eq!(parse_runtime(""), Runtime::Blocking);

        // Async is persisted and parsed back, even with no modules/clock.
        let text = serialize(&[], None, Runtime::Async);
        assert!(text.contains("@runtime\n"));
        assert!(text.contains("Async"));
        assert_eq!(parse_runtime(&text), Runtime::Async);

        // …and it coexists with modules + clock.
        let text = serialize(&[sample_module()], Some(&Stm32f1Clock::default()), Runtime::Async);
        let (m, c) = parse(&text);
        assert_eq!(m.len(), 1);
        assert!(c.is_some());
        assert_eq!(parse_runtime(&text), Runtime::Async);
    }

    #[test]
    fn parse_ignores_garbage() {
        let (m, c) = parse("not a config file\n");
        assert!(m.is_empty());
        assert!(c.is_none());
    }

    /// The Structure sections moved to their own file, but a `mcu.config` that
    /// still carries them (saved before the split) must keep parsing.
    #[test]
    fn legacy_structure_sections_do_not_break_parsing() {
        use crate::panels::mcu_module::structure_config;
        let mut pos = structure_config::StructurePositions::new();
        pos.insert("main.rs".into(), (14.0, 14.0));
        pos.insert("mw_radar/utils.rs".into(), (321.5, 208.0));

        // A legacy file: MCU sections followed by the old Structure ones.
        let mut text = serialize(&[sample_module()], Some(&Stm32f1Clock::default()), Runtime::Blocking);
        text.push('\n');
        text.push_str(&structure_config::serialize(&pos, &(true, Some(2), 0, false)));

        let (m, c) = parse(&text);
        assert_eq!(m.len(), 1, "modules unaffected by the extra sections");
        assert!(c.is_some(), "clock unaffected by the extra sections");
        // And the migration reader still finds the positions in there.
        assert_eq!(structure_config::parse_layout(&text), pos);
    }
}
