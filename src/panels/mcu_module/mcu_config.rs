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
//!     (id: "i2c_1", kind: GenericInterfaceI2c, …),
//! ]
//!
//! @clock
//! hse=8000000
//! hse_on=0
//! …
//! ```

use super::clock::model::Stm32f1Clock;
use super::clock::persist as clock_persist;
use super::mcu::{AutoBuild, Runtime};
use super::modules::{ApiStyle, VirtualModule};
use crate::panels::mcu_module::pins::logic::pin::{Edge, GpioMode};

const MODULES_HEADER: &str = "@modules";
const CLOCK_HEADER: &str = "@clock";
const RUNTIME_HEADER: &str = "@runtime";
const GPIO_HEADER: &str = "@gpio";
const AUTOBUILD_HEADER: &str = "@autobuild";
const STRICT_HEADER: &str = "@strict";
const DEBUGBUILD_HEADER: &str = "@debugbuild";
const ROTATION_HEADER: &str = "@rotation";
const CLOCK_MANUAL_HEADER: &str = "@clockmanual";
const IOPINS_HEADER: &str = "@iopins";
const IRQ_HEADER: &str = "@irq";
const IOMODE_HEADER: &str = "@iomode";
const WATCHDOG_HEADER: &str = "@watchdog";
const COMP_HEADER: &str = "@comp";

/// The `@autobuild` section text (or "" for the default `Check`) — appended by
/// `Mcu::mcu_config_text` after [`serialize`]. Kept separate so `serialize`'s
/// signature (and its many test call-sites) stays put; it's a workflow setting,
/// not part of the module/clock/runtime config.
pub fn autobuild_section(auto_build: AutoBuild) -> String {
    if auto_build == AutoBuild::Check {
        String::new()
    } else {
        format!("{AUTOBUILD_HEADER}\n{}\n", auto_build.as_token())
    }
}

/// The auto-build preference recorded in `@autobuild`; a missing section is the
/// default `Check`.
pub fn parse_autobuild(text: &str) -> AutoBuild {
    section_body(text, AUTOBUILD_HEADER)
        .map(|b| AutoBuild::from_token(&b))
        .unwrap_or_default()
}

/// The `@strict` section text (or "" for the default OFF) — the strict-lints
/// Clippy preference. Appended like `@autobuild`.
pub fn strict_section(strict: bool) -> String {
    if strict {
        format!("{STRICT_HEADER}\non\n")
    } else {
        String::new()
    }
}

/// The `@watchdog` section — the Configuration tab's IWDG/WWDG settings.
///
/// Durations in microseconds, one line per watchdog, absent when not enabled:
///
/// ```text
/// @watchdog
/// iwdg 32768000
/// wwdg 41472 0
/// ```
///
/// These are CODEGEN input, not a view preference: they decide whether
/// `pins/configs/{iwdg,wwdg}.rs` exist at all, so they travel with the project.
pub fn watchdog_section(w: &crate::panels::mcu_module::watchdog::WatchdogSettings) -> String {
    let mut body = String::new();
    if let Some(i) = w.iwdg {
        body.push_str(&format!(
            "iwdg {}
",
            i.timeout_us
        ));
    }
    if let Some(x) = w.wwdg {
        body.push_str(&format!(
            "wwdg {} {}
",
            x.timeout_us, x.window_us
        ));
    }
    // The ESP three. Written on their own keys rather than reusing `iwdg`,
    // because a project carried from an STM32 to an ESP keeps both sets and
    // neither should be read as the other: 26 seconds of IWDG is not 26
    // seconds of RWDT, and the tab shows only the pair its family uses.
    for (key, cfg) in [("rwdt", w.rwdt), ("mwdt0", w.mwdt0), ("mwdt1", w.mwdt1)] {
        if let Some(c) = cfg {
            body.push_str(&format!(
                "{key} {}
",
                c.timeout_us
            ));
        }
    }
    if body.is_empty() {
        String::new()
    } else {
        format!(
            "{WATCHDOG_HEADER}
{body}"
        )
    }
}

/// Read `@watchdog` back. A malformed or partial line is DROPPED rather than
/// defaulted: a watchdog the user cannot see in the tab must not end up in the
/// generated firmware, and silently substituting a number would do exactly that.
pub fn parse_watchdog(text: &str) -> crate::panels::mcu_module::watchdog::WatchdogSettings {
    use crate::panels::mcu_module::watchdog::{
        EspWdtConfig, IwdgConfig, WatchdogSettings, WwdgConfig,
    };
    let mut out = WatchdogSettings::default();
    let Some(body) = section_body(text, WATCHDOG_HEADER) else {
        return out;
    };
    for line in body.lines() {
        let mut it = line.split_whitespace();
        match (it.next(), it.next().and_then(|v| v.parse().ok())) {
            (Some("iwdg"), Some(timeout_us)) => out.iwdg = Some(IwdgConfig { timeout_us }),
            (Some("rwdt"), Some(timeout_us)) => out.rwdt = Some(EspWdtConfig { timeout_us }),
            (Some("mwdt0"), Some(timeout_us)) => out.mwdt0 = Some(EspWdtConfig { timeout_us }),
            (Some("mwdt1"), Some(timeout_us)) => out.mwdt1 = Some(EspWdtConfig { timeout_us }),
            (Some("wwdg"), Some(timeout_us)) => {
                // The window is required: without it the pair is meaningless,
                // and defaulting it to 0 would quietly change the behaviour the
                // user configured.
                if let Some(window_us) = it.next().and_then(|v| v.parse().ok()) {
                    out.wwdg = Some(WwdgConfig {
                        timeout_us,
                        window_us,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// The `@comp` section — one line per ENABLED comparator:
///
/// ```text
/// @comp
/// 3 HighSpeed Mv20 NotInverted HalfVref None
/// ```
///
/// Positional rather than `key=value`: five fields that always appear in the
/// same order, and a line that lost one is dropped whole (below) rather than
/// half-applied.
pub fn comp_section(c: &crate::panels::mcu_module::comparator::CompSettings) -> String {
    let mut body = String::new();
    for (n, cfg) in c {
        body.push_str(&format!(
            "{n} {} {} {} {} {}\n",
            cfg.power_mode.token(),
            cfg.hysteresis.token(),
            cfg.output_polarity.token(),
            cfg.inverting_input.token(),
            cfg.blanking_source.token(),
        ));
    }
    if body.is_empty() {
        String::new()
    } else {
        format!("{COMP_HEADER}\n{body}")
    }
}

/// Read `@comp` back. Same policy as `@watchdog`: a line that does not parse
/// COMPLETELY is dropped, because a comparator the user cannot see in the tab
/// must not reach the generated firmware, and defaulting a field would quietly
/// change what it compares against.
pub fn parse_comp(text: &str) -> crate::panels::mcu_module::comparator::CompSettings {
    use crate::panels::mcu_module::comparator::{
        BlankingSource, CompConfig, CompSettings, Hysteresis, InvertingInput, OutputPolarity,
        PowerMode,
    };
    let mut out = CompSettings::new();
    let Some(body) = section_body(text, COMP_HEADER) else {
        return out;
    };
    // The token spellings are the generator's own, so a round trip cannot drift
    // from what the templates emit.
    let by_token = |tok: &str, all: &[&str]| all.iter().position(|t| *t == tok);
    for line in body.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 6 {
            continue;
        }
        let Ok(n) = f[0].parse::<u8>() else { continue };
        let power = PowerMode::ALL.iter().find(|v| v.token() == f[1]).copied();
        let hyst = Hysteresis::ALL.iter().find(|v| v.token() == f[2]).copied();
        let pol = OutputPolarity::ALL
            .iter()
            .find(|v| v.token() == f[3])
            .copied();
        let inm = InvertingInput::ALL
            .iter()
            .find(|v| v.token() == f[4])
            .copied();
        let blank = BlankingSource::ALL
            .iter()
            .find(|v| v.token() == f[5])
            .copied();
        let _ = by_token;
        if let (
            Some(power_mode),
            Some(hysteresis),
            Some(output_polarity),
            Some(inverting_input),
            Some(blanking_source),
        ) = (power, hyst, pol, inm, blank)
        {
            out.insert(
                n,
                CompConfig {
                    power_mode,
                    hysteresis,
                    output_polarity,
                    inverting_input,
                    blanking_source,
                },
            );
        }
    }
    out
}

/// The strict-lints preference recorded in `@strict`; missing / anything but
/// `on` is OFF (the default).
pub fn parse_strict(text: &str) -> bool {
    section_body(text, STRICT_HEADER).as_deref() == Some("on")
}

/// The `@debugbuild` section text (or "" for the default OFF) — the Debug tab's
/// "Debug-friendly build" toggle, which relaxes `[profile.release]` so every
/// source line can hold a breakpoint. Appended like `@autobuild`.
pub fn debug_build_section(debug_build: bool) -> String {
    if debug_build {
        format!("{DEBUGBUILD_HEADER}\non\n")
    } else {
        String::new()
    }
}

/// The debug-build preference recorded in `@debugbuild`; missing / anything but
/// `on` is OFF (the optimised profile that gets flashed).
pub fn parse_debug_build(text: &str) -> bool {
    section_body(text, DEBUGBUILD_HEADER).as_deref() == Some("on")
}

/// The `@rotation` section text (or "" for the default un-rotated) — the diagram
/// rotation toggle. Appended like `@autobuild`.
pub fn rotation_section(rotated: bool) -> String {
    if rotated {
        format!("{ROTATION_HEADER}\non\n")
    } else {
        String::new()
    }
}

/// The `@clockmanual` section text (or "" when the clock is generated) — the
/// hand-written-clock switch. In `mcu.config` rather than the view-state file
/// because it CHANGES THE GENERATED CODE, so it belongs in Git with the rest of
/// the project's configuration. Appended like `@autobuild`.
pub fn clock_manual_section(manual: bool) -> String {
    if manual {
        format!(
            "{CLOCK_MANUAL_HEADER}
on
"
        )
    } else {
        String::new()
    }
}

/// The hand-written-clock preference recorded in `@clockmanual`.
///
/// A MISSING section is not simply "off": a chip whose family has no RCC recipe
/// defaults to manual, and that default is applied by the caller — this only
/// reports what the file says.
pub fn parse_clock_manual(text: &str) -> Option<bool> {
    section_body(text, CLOCK_MANUAL_HEADER).map(|b| b == "on")
}

/// The diagram-rotation preference recorded in `@rotation`; missing / anything
/// but `on` is un-rotated (the default).
pub fn parse_rotation(text: &str) -> bool {
    section_body(text, ROTATION_HEADER).as_deref() == Some("on")
}

/// The `@iopins` section — manual in/out field positions, one `num=x,y` per
/// line — or "" when none are placed.
pub fn iopins_section(pos: &std::collections::BTreeMap<usize, (f32, f32)>) -> String {
    if pos.is_empty() {
        return String::new();
    }
    let mut s = String::from(IOPINS_HEADER);
    s.push('\n');
    for (num, (x, y)) in pos {
        s.push_str(&format!("{num}={x},{y}\n"));
    }
    s
}

/// The `@irq` section — one `num=Edge` per interrupt-enabled input pin — or ""
/// when none are armed, so a project that uses no interrupts round-trips without
/// the section at all.
pub fn irq_section(irqs: &std::collections::BTreeMap<usize, Edge>) -> String {
    if irqs.is_empty() {
        return String::new();
    }
    let mut s = String::from(IRQ_HEADER);
    s.push('\n');
    for (num, e) in irqs {
        s.push_str(&format!("{num}={}\n", e.as_token()));
    }
    s
}

/// The `@iomode` section — one `num=Mode` per GPIO pin whose drive/pull mode the
/// user changed from the backend default — or "" when every pin is on its
/// default, so a project that never touched a mode round-trips without it.
pub fn iomode_section(modes: &std::collections::BTreeMap<usize, GpioMode>) -> String {
    if modes.is_empty() {
        return String::new();
    }
    let mut s = String::from(IOMODE_HEADER);
    s.push('\n');
    for (num, m) in modes {
        s.push_str(&format!("{num}={}\n", m.as_token()));
    }
    s
}

/// Parse `@iomode` back into `pin -> GpioMode`; malformed lines are skipped.
pub fn parse_iomode(text: &str) -> std::collections::BTreeMap<usize, GpioMode> {
    let mut map = std::collections::BTreeMap::new();
    let Some(body) = section_body(text, IOMODE_HEADER) else {
        return map;
    };
    for line in body.lines() {
        if let Some((n, m)) = line.trim().split_once('=') {
            if let (Ok(num), Some(mode)) = (n.trim().parse::<usize>(), GpioMode::from_token(m)) {
                map.insert(num, mode);
            }
        }
    }
    map
}

/// Parse `@irq` back into `pin -> Edge`; malformed lines are skipped.
pub fn parse_irq(text: &str) -> std::collections::BTreeMap<usize, Edge> {
    let mut map = std::collections::BTreeMap::new();
    let Some(body) = section_body(text, IRQ_HEADER) else {
        return map;
    };
    for line in body.lines() {
        if let Some((n, e)) = line.trim().split_once('=') {
            if let (Ok(num), Some(edge)) = (n.trim().parse::<usize>(), Edge::from_token(e)) {
                map.insert(num, edge);
            }
        }
    }
    map
}

/// Parse the `@iopins` section back into the `pin → (x,y)` map; malformed lines
/// are skipped.
pub fn parse_iopins(text: &str) -> std::collections::BTreeMap<usize, (f32, f32)> {
    let mut map = std::collections::BTreeMap::new();
    let Some(body) = section_body(text, IOPINS_HEADER) else {
        return map;
    };
    for line in body.lines() {
        let line = line.trim();
        if let Some((n, xy)) = line.split_once('=') {
            if let (Ok(num), Some((xs, ys))) = (n.trim().parse::<usize>(), xy.split_once(',')) {
                if let (Ok(x), Ok(y)) = (xs.trim().parse::<f32>(), ys.trim().parse::<f32>()) {
                    map.insert(num, (x, y));
                }
            }
        }
    }
    map
}

/// The token for a GPIO api style (`@gpio` section): "Native" or "Portable".
fn gpio_token(s: ApiStyle) -> &'static str {
    match s {
        ApiStyle::Native => "Native",
        ApiStyle::Portable => "Portable",
    }
}

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
    gpio_api: ApiStyle,
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

    // Only persist a non-default (Native) GPIO api — Portable (the default,
    // with the io.rs bridge) keeps the file free of the section.
    if gpio_api != ApiStyle::Portable {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(GPIO_HEADER);
        out.push('\n');
        out.push_str(gpio_token(gpio_api));
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

/// The GPIO api style recorded in `@gpio`; a missing section is the default
/// `Portable` (the io.rs embedded-hal bridge).
pub fn parse_gpio_api(text: &str) -> ApiStyle {
    match section_body(text, GPIO_HEADER).as_deref().map(str::trim) {
        Some("Native") => ApiStyle::Native,
        _ => ApiStyle::Portable,
    }
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
            id: "_i2c_1".into(),
            kind: ModuleKind::GenericInterfaceI2c,
            name: "I2C1".into(),
            pos: (0.0, 0.0),
            config: ModuleConfig::I2c(cfg),
            connections: vec![
                Connection {
                    signal: ModuleSignal::Scl,
                    mcu_pin: 45,
                },
                Connection {
                    signal: ModuleSignal::Sda,
                    mcu_pin: 46,
                },
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
        let text = serialize(
            &modules,
            Some(&clock),
            Runtime::Blocking,
            ApiStyle::Portable,
        );

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
        assert_eq!(
            serialize(&[], None, Runtime::Blocking, ApiStyle::Portable),
            ""
        );
    }

    #[test]
    fn rotation_round_trips() {
        assert_eq!(rotation_section(false), "");
        assert!(!parse_rotation(""));
        let t = rotation_section(true);
        assert!(t.contains("@rotation"));
        assert!(parse_rotation(&t));
    }

    #[test]
    fn iopins_round_trip() {
        let mut m = std::collections::BTreeMap::new();
        assert_eq!(iopins_section(&m), "");
        assert!(parse_iopins("").is_empty());
        m.insert(13usize, (12.5_f32, -8.0_f32));
        m.insert(45usize, (100.0_f32, 40.0_f32));
        let t = iopins_section(&m);
        assert!(t.starts_with("@iopins\n"), "{t}");
        assert_eq!(parse_iopins(&t), m);
    }

    #[test]
    fn clock_only_when_no_modules() {
        let text = serialize(
            &[],
            Some(&Stm32f1Clock::default()),
            Runtime::Blocking,
            ApiStyle::Portable,
        );
        assert!(!text.contains("@modules"));
        assert!(text.starts_with("@clock\n"));
        let (m, c) = parse(&text);
        assert!(m.is_empty());
        assert_eq!(c, Some(Stm32f1Clock::default()));
    }

    #[test]
    fn runtime_round_trips_and_defaults_to_blocking() {
        // Default runtime writes NO section — old projects stay byte-identical.
        assert_eq!(
            serialize(&[], None, Runtime::Blocking, ApiStyle::Portable),
            ""
        );
        assert_eq!(parse_runtime(""), Runtime::Blocking);

        // Async is persisted and parsed back, even with no modules/clock.
        let text = serialize(&[], None, Runtime::Async, ApiStyle::Portable);
        assert!(text.contains("@runtime\n"));
        assert!(text.contains("Async"));
        assert_eq!(parse_runtime(&text), Runtime::Async);

        // Native round-trips too.
        let text = serialize(&[], None, Runtime::Native, ApiStyle::Portable);
        assert!(text.contains("@runtime\n") && text.contains("Native"));
        assert_eq!(parse_runtime(&text), Runtime::Native);

        // …and it coexists with modules + clock.
        let text = serialize(
            &[sample_module()],
            Some(&Stm32f1Clock::default()),
            Runtime::Async,
            ApiStyle::Portable,
        );
        let (m, c) = parse(&text);
        assert_eq!(m.len(), 1);
        assert!(c.is_some());
        assert_eq!(parse_runtime(&text), Runtime::Async);
    }

    #[test]
    fn gpio_api_round_trips_and_defaults_to_portable() {
        // Default (Portable) writes NO @gpio section.
        assert_eq!(
            serialize(&[], None, Runtime::Blocking, ApiStyle::Portable),
            ""
        );
        assert_eq!(parse_gpio_api(""), ApiStyle::Portable);

        // Native is persisted + parsed back, independent of runtime.
        let text = serialize(&[], None, Runtime::Blocking, ApiStyle::Native);
        assert!(text.contains("@gpio\n") && text.contains("Native"));
        assert_eq!(parse_gpio_api(&text), ApiStyle::Native);
        assert_eq!(
            parse_runtime(&text),
            Runtime::Blocking,
            "gpio section doesn't disturb runtime"
        );

        // Coexists with the runtime section.
        let text = serialize(&[], None, Runtime::Async, ApiStyle::Native);
        assert_eq!(parse_runtime(&text), Runtime::Async);
        assert_eq!(parse_gpio_api(&text), ApiStyle::Native);
    }

    #[test]
    fn autobuild_round_trips_and_defaults_to_check() {
        use crate::panels::mcu_module::mcu::AutoBuild;
        // Default (Check) writes NO section; missing section parses as Check.
        assert_eq!(autobuild_section(AutoBuild::Check), "");
        assert_eq!(parse_autobuild(""), AutoBuild::Check);
        // Off / Release persist + parse back, independent of the other sections.
        for mode in [AutoBuild::Off, AutoBuild::Release] {
            let text = format!(
                "{}{}",
                serialize(&[], None, Runtime::Async, ApiStyle::Native),
                {
                    let s = autobuild_section(mode);
                    // (mcu_config_text joins with a blank line; a leading one here
                    //  is harmless for the section parser.)
                    format!("\n{s}")
                }
            );
            assert!(text.contains("@autobuild\n"));
            assert_eq!(parse_autobuild(&text), mode);
            // …and it doesn't disturb the other sections.
            assert_eq!(parse_runtime(&text), Runtime::Async);
            assert_eq!(parse_gpio_api(&text), ApiStyle::Native);
        }
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
        let mut text = serialize(
            &[sample_module()],
            Some(&Stm32f1Clock::default()),
            Runtime::Blocking,
            ApiStyle::Portable,
        );
        text.push('\n');
        use structure_config::CLOCK_VIEW_DEFAULT;
        text.push_str(&structure_config::serialize(
            &pos,
            &(true, Some(2), 0, false),
            &Default::default(),
            &CLOCK_VIEW_DEFAULT,
        ));

        let (m, c) = parse(&text);
        assert_eq!(m.len(), 1, "modules unaffected by the extra sections");
        assert!(c.is_some(), "clock unaffected by the extra sections");
        // And the migration reader still finds the positions in there.
        assert_eq!(structure_config::parse_layout(&text), pos);
    }
}

#[cfg(test)]
mod watchdog_section_tests {
    use super::*;
    use crate::panels::mcu_module::watchdog::{
        EspWdtConfig, IwdgConfig, WatchdogSettings, WwdgConfig,
    };

    #[test]
    fn both_watchdogs_round_trip() {
        let w = WatchdogSettings {
            iwdg: Some(IwdgConfig {
                timeout_us: 32_768_000,
            }),
            wwdg: Some(WwdgConfig {
                timeout_us: 41_472,
                window_us: 5_000,
            }),
            ..Default::default()
        };
        assert_eq!(parse_watchdog(&watchdog_section(&w)), w);
    }

    /// The ESP three round-trip on their own keys, and do NOT come back as the
    /// STM32 pair — which is what reusing `iwdg` for the RWDT would have done.
    #[test]
    fn the_esp_watchdogs_round_trip_on_their_own_keys() {
        let w = WatchdogSettings {
            rwdt: Some(EspWdtConfig {
                timeout_us: 2_000_000,
            }),
            mwdt0: Some(EspWdtConfig {
                timeout_us: 750_000,
            }),
            mwdt1: Some(EspWdtConfig { timeout_us: 15 }),
            ..Default::default()
        };
        let text = watchdog_section(&w);
        assert_eq!(parse_watchdog(&text), w, "{text}");
        assert!(!text.contains("iwdg"), "{text}");

        // And an STM32 project keeps writing nothing for them.
        let stm = WatchdogSettings {
            iwdg: Some(IwdgConfig { timeout_us: 1_000 }),
            ..Default::default()
        };
        let text = watchdog_section(&stm);
        assert!(!text.contains("wdt"), "{text}");
        assert_eq!(parse_watchdog(&text), stm);
    }

    #[test]
    fn each_one_alone_round_trips_too() {
        for w in [
            WatchdogSettings {
                iwdg: Some(IwdgConfig { timeout_us: 1_000 }),
                ..Default::default()
            },
            WatchdogSettings {
                wwdg: Some(WwdgConfig {
                    timeout_us: 900,
                    window_us: 0,
                }),
                ..Default::default()
            },
            WatchdogSettings {
                mwdt1: Some(EspWdtConfig {
                    timeout_us: 5_000_000,
                }),
                ..Default::default()
            },
        ] {
            assert_eq!(parse_watchdog(&watchdog_section(&w)), w);
        }
    }

    #[test]
    fn nothing_enabled_writes_no_section() {
        // An empty section would leave `@watchdog` in every project file
        // that never touched the tab.
        assert_eq!(watchdog_section(&WatchdogSettings::default()), "");
        assert_eq!(parse_watchdog(""), WatchdogSettings::default());
    }

    #[test]
    fn a_malformed_line_is_dropped_not_defaulted() {
        // Substituting a number would put a watchdog in the firmware that
        // the user cannot see in the tab - the worst possible outcome for a
        // peripheral whose whole job is resetting the board.
        let w = parse_watchdog(
            "@watchdog
iwdg
wwdg 500
",
        );
        assert_eq!(w, WatchdogSettings::default(), "partial lines must vanish");
        // …and a WWDG without its window is partial, not a 0-window one.
        assert_eq!(
            parse_watchdog(
                "@watchdog
wwdg 500
"
            )
            .wwdg,
            None
        );
    }
}

#[cfg(test)]
mod comp_section_tests {
    use super::{comp_section, parse_comp};
    use crate::panels::mcu_module::comparator::{
        BlankingSource, CompConfig, CompSettings, Hysteresis, InvertingInput, OutputPolarity,
        PowerMode,
    };

    #[test]
    fn a_comparator_survives_the_round_trip() {
        let mut c = CompSettings::new();
        c.insert(
            2,
            CompConfig {
                power_mode: PowerMode::MediumSpeed,
                hysteresis: Hysteresis::Mv40,
                output_polarity: OutputPolarity::Inverted,
                inverting_input: InvertingInput::Dac2,
                blanking_source: BlankingSource::Blank1,
            },
        );
        c.insert(7, CompConfig::default());
        let text = comp_section(&c);
        assert_eq!(parse_comp(&text), c);
        // Nothing configured writes NO section, so an untouched project's
        // mcu.config does not grow an empty header.
        assert!(comp_section(&CompSettings::new()).is_empty());
        assert!(parse_comp("").is_empty());
    }

    /// Same policy as `@watchdog`: half a line is no line. A comparator the tab
    /// cannot show must not reach the firmware, and defaulting a field would
    /// quietly change what it compares against.
    #[test]
    fn an_incomplete_line_is_dropped_whole() {
        for bad in [
            "@comp\n2 MediumSpeed Mv40 Inverted Dac2\n",
            "@comp\n2 MediumSpeed Mv40 Inverted Dac2 Blank1 extra\n",
            "@comp\nx MediumSpeed Mv40 Inverted Dac2 Blank1\n",
            "@comp\n2 Turbo Mv40 Inverted Dac2 Blank1\n",
            "@comp\n2 MediumSpeed Mv999 Inverted Dac2 Blank1\n",
        ] {
            assert!(parse_comp(bad).is_empty(), "{bad}");
        }
        // ...while a good line right after a bad one still lands.
        let mixed = "@comp\n2 Turbo Mv40 Inverted Dac2 Blank1\n7 HighSpeed None NotInverted HalfVref None\n";
        assert_eq!(parse_comp(mixed).len(), 1);
        assert!(parse_comp(mixed).contains_key(&7));
    }
}

#[cfg(test)]
mod lpuart_persist_tests {
    use super::*;
    use crate::panels::mcu_module::modules::{
        Connection, ModuleConfig, ModuleKind, ModuleSignal, UsartModuleConfig, VirtualModule,
    };

    /// The LPUART variant survives `@modules` — it shares the USART's settings
    /// struct, so the only thing that can go wrong is the variant itself being
    /// read back as a USART, which would silently move the module to the wrong
    /// peripheral.
    #[test]
    fn an_lpuart_module_round_trips() {
        let mut cfg = UsartModuleConfig::new(1);
        cfg.baud_rate = 9600;
        let m = VirtualModule {
            id: "lpuart_1".into(),
            kind: ModuleKind::GenericInterfaceLpuart,
            name: "LPUART1".into(),
            pos: (0.0, 0.0),
            config: ModuleConfig::Lpuart(cfg),
            connections: vec![
                Connection {
                    signal: ModuleSignal::LpTx,
                    mcu_pin: 21,
                },
                Connection {
                    signal: ModuleSignal::LpRx,
                    mcu_pin: 22,
                },
            ],
        };
        let text = serialize(&[m], None, Runtime::Blocking, ApiStyle::Portable);
        let (back, _) = parse(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].kind, ModuleKind::GenericInterfaceLpuart);
        assert!(matches!(back[0].config, ModuleConfig::Lpuart(_)));
        assert_eq!(back[0].instance(), 1);
        assert_eq!(back[0].pin_for(ModuleSignal::LpTx), Some(21));
    }
}
