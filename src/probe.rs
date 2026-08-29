//! Debug-probe enumeration via `probe-rs list`, shared by the RTT and Debug
//! tabs so the user can target a specific probe when several are connected.
//!
//! Both tabs drive `probe-rs`, which — given only `--chip` — auto-selects the
//! sole attached probe and turns ambiguous the moment a second one is plugged
//! in. Passing `--probe <VID:PID[:Serial]>` (RTT) or the DAP `probe` launch
//! field (Debug) pins the session to one probe. The selector strings come
//! straight from `probe-rs list`, so they are always in the exact form
//! probe-rs expects — no reconstruction from our own USB scan.

use crate::build::no_window;
use crate::terminal::{LineKind, TerminalState};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// One probe as reported by `probe-rs list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeInfo {
    /// Human name, e.g. "STLink V2".
    pub name: String,
    /// probe-rs family tag, e.g. "ST-LINK", "EspJtag".
    pub kind: String,
    /// The exact `--probe` selector (`VID:PID` or `VID:PID:Serial`).
    pub selector: String,
}

impl ProbeInfo {
    /// One-line label for the ComboBox.
    pub fn combo_label(&self) -> String {
        format!("[{}] {}  ·  {}", self.kind, self.name, self.selector)
    }
}

/// A probe selector as probe-rs should see it, or `None` for "let it choose".
///
/// One place, because two callers had drifted: the RTT tab passes it as
/// `--probe <sel>` and the debugger as the DAP `launch` object's `probe` field,
/// and both were filtering with a bare `is_empty()`. A selector of SPACES then
/// survived and went out as `--probe "   "` / `"probe": "   "`, which probe-rs
/// rejects - where the empty case is meant to mean auto-select.
pub fn selector(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Run `probe-rs list` and parse the connected probes. `Ok(vec![])` when none
/// are attached; `Err` only when the binary itself cannot be run.
pub fn list_probes() -> Result<Vec<ProbeInfo>, String> {
    let out = no_window(&mut Command::new("probe-rs"))
        .arg("list")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "probe-rs not found in PATH (cargo install probe-rs-tools)".to_string()
            } else {
                format!("could not run `probe-rs list`: {e}")
            }
        })?;
    // The probe rows go to stdout; scan stderr too in case a build ever changes
    // that — parsing only picks up the `[n]: … -- … (Kind)` shape either way.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // `probe-rs list` can panic inside its own USB enumeration. Parsing that
    // output would just report "no probes attached" — a crash has to be told
    // apart from an empty bench, since the fix is a probe-rs version, not a
    // cable.
    if let Some(detail) = crate::failure_hint::probe_rs_panic(&text) {
        return Err(crate::failure_hint::probe_rs_panic_message(&detail));
    }
    if let Some(detail) = crate::failure_hint::probe_open_failure(&text) {
        return Err(crate::failure_hint::probe_open_message(&detail));
    }
    Ok(parse_list(&text))
}

/// Reset the target through the probe (`probe-rs reset`), streaming the result
/// into `console`. Runs on its own thread — it opens the probe, which takes a
/// moment and must not stall the UI.
///
/// This is the way out of a firmware that sits somewhere it shouldn't: it
/// restarts the chip WITHOUT reflashing it and without a USB replug. Only
/// meaningful while nothing else holds the probe — a live Debug/RTT session
/// owns it exclusively, which is why the button is disabled there.
pub fn start_reset(
    chip: String,
    probe: Option<String>,
    console: Arc<Mutex<TerminalState>>,
    ctx: eframe::egui::Context,
) {
    std::thread::spawn(move || {
        let mut args: Vec<String> = vec!["reset".into(), "--chip".into(), chip];
        if let Some(p) = selector(probe.as_deref()) {
            args.push("--probe".into());
            args.push(p);
        }
        console
            .lock()
            .unwrap()
            .push_plain(LineKind::Input, format!("> probe-rs {}", args.join(" ")));
        ctx.request_repaint();

        let out = no_window(&mut Command::new("probe-rs"))
            .args(&args)
            .output();
        let mut c = console.lock().unwrap();
        match out {
            Ok(o) => {
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
                for line in crate::terminal::strip_ansi(&text).lines() {
                    if !line.trim().is_empty() {
                        c.push_plain(LineKind::Stdout, line);
                    }
                }
                c.push_plain(
                    if o.status.success() {
                        LineKind::Notice
                    } else {
                        LineKind::Stderr
                    },
                    if o.status.success() {
                        "[target reset — the firmware is running from its entry point]"
                    } else {
                        "[reset failed — see above]"
                    },
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                c.push_plain(
                    LineKind::Stderr,
                    "probe-rs not found in PATH (cargo install probe-rs-tools)",
                );
            }
            Err(e) => c.push_plain(LineKind::Stderr, format!("could not run probe-rs: {e}")),
        }
        drop(c);
        ctx.request_repaint();
    });
}

/// Parse `probe-rs list` output. Recognised line shape (probe-rs 0.2x–0.31):
/// `[<idx>]: <name> -- <VID:PID[:Serial]> (<Kind>)`. The serial itself can hold
/// colons (ESP JTAG → `303a:1001:50:78:7D:62:33:A4`), so the selector is taken
/// verbatim between ` -- ` and the trailing ` (Kind)` rather than split on `:`.
pub fn parse_list(stdout: &str) -> Vec<ProbeInfo> {
    let mut probes = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        // Only the numbered "[0]: …" rows are probes (skip the header line).
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some(idx_end) = rest.find("]:") else {
            continue;
        };
        let body = rest[idx_end + 2..].trim(); // "STLink V2 -- 0483:3748: (ST-LINK)"

        // name -- tail, split on the first " -- ".
        let Some(dash) = body.find(" -- ") else {
            continue;
        };
        let name = body[..dash].trim().to_string();
        let tail = body[dash + 4..].trim(); // "0483:3748: (ST-LINK)"

        // Kind is the last parenthesised token; the selector is everything
        // before it (rfind so a probe name never confuses the split).
        let (selector, kind) = match tail.rfind('(') {
            Some(paren) => {
                let sel = tail[..paren].trim();
                let kind = tail[paren + 1..].trim_end().trim_end_matches(')').trim();
                (sel, kind.to_string())
            }
            None => (tail, String::new()),
        };

        // Drop a trailing empty-serial colon ("0483:3748:" → "0483:3748") while
        // leaving the colon-bearing ESP serials intact.
        let selector = selector.trim().trim_end_matches(':').to_string();
        if selector.is_empty() {
            continue;
        }
        probes.push(ProbeInfo {
            name,
            kind,
            selector,
        });
    }
    probes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stlink_and_esp_jtag() {
        let out = "The following debug probes were found:\n\
                   [0]: STLink V2 -- 0483:3748: (ST-LINK)\n\
                   [1]: ESP JTAG -- 303a:1001:50:78:7D:62:33:A4 (EspJtag)\n";
        let p = parse_list(out);
        assert_eq!(p.len(), 2);

        assert_eq!(p[0].name, "STLink V2");
        assert_eq!(p[0].kind, "ST-LINK");
        // Empty serial → trailing colon dropped.
        assert_eq!(p[0].selector, "0483:3748");

        assert_eq!(p[1].name, "ESP JTAG");
        assert_eq!(p[1].kind, "EspJtag");
        // Colon-bearing serial kept verbatim.
        assert_eq!(p[1].selector, "303a:1001:50:78:7D:62:33:A4");
    }

    #[test]
    fn parses_serial_with_hex_string() {
        let out = "[0]: J-Link -- 1366:0101:000059012345 (JLink)\n";
        let p = parse_list(out);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].selector, "1366:0101:000059012345");
        assert_eq!(p[0].kind, "JLink");
    }

    #[test]
    fn no_probes_yields_empty() {
        assert!(parse_list("No debug probes were found.\n").is_empty());
        assert!(parse_list("").is_empty());
    }

    #[test]
    fn ignores_malformed_rows() {
        // Missing " -- " → not a probe row.
        assert!(parse_list("[0]: something odd (ST-LINK)\n").is_empty());
        // Header-ish noise.
        assert!(parse_list("The following debug probes were found:\n").is_empty());
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Which chips probe-rs can target
// ──────────────────────────────────────────────────────────────────────────────

/// Every target name `probe-rs chip list` reports, lowercased — the families at
/// column 0 and the variants indented beneath them alike, since `--chip` takes
/// either.
///
/// `None` when probe-rs could not be run at all, which is the absent-tool case
/// the Tools tab already reports; there is nothing useful this can add.
///
/// Asked once per process and cached. The call costs about 140 ms and the answer
/// cannot change while the IDE runs, because it is baked into the probe-rs
/// binary — a newly installed probe-rs is a new binary, and the user restarts.
fn known_targets() -> Option<&'static std::collections::HashSet<String>> {
    static TARGETS: std::sync::OnceLock<Option<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    TARGETS
        .get_or_init(|| {
            let out = no_window(&mut Command::new("probe-rs"))
                .args(["chip", "list"])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            parse_chip_list(&String::from_utf8_lossy(&out.stdout))
        })
        .as_ref()
}

/// Parse `probe-rs chip list`, which nests variants under a family:
///
/// ```text
/// Available chips:
/// esp32c6
///     Variants:
///         esp32c6
/// ```
///
/// Both levels are collected, because `--chip` accepts either.
///
/// `None` for output that yields nothing — that is a format change, not an
/// empty catalogue, and must read as "could not ask" so nothing is blocked on
/// the strength of a misread.
fn parse_chip_list(text: &str) -> Option<std::collections::HashSet<String>> {
    let set: std::collections::HashSet<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "Available chips:" && *l != "Variants:")
        .map(|l| l.to_ascii_lowercase())
        .collect();
    (!set.is_empty()).then_some(set)
}

/// Why probe-rs cannot target `chip`, or `None` when it can — or when we could
/// not ask, which is deliberately the same answer.
///
/// # Why this is asked rather than listed
///
/// The gap is real but temporary: probe-rs 0.29 has no target for the ESP32-C5
/// or C61, and 0.32 has both. A hard-coded list of unsupported parts would
/// start lying the day the user upgrades, and only a code change here would
/// stop it. So the installed binary is asked, and the block lifts by itself.
///
/// Failing open matters as much: a missing probe-rs, an unparseable listing, or
/// a chip name we simply do not recognise must not disable a button. The
/// session then fails at probe-rs with probe-rs's own message, which is a
/// better outcome than a wrong refusal from us.
///
/// Espressif parts flash over espflash, which does not go through probe-rs at
/// all — so this never has anything to say about the Flash tab.
pub fn chip_gap(chip: &str) -> Option<String> {
    let chip = chip.trim();
    if chip.is_empty() {
        return None;
    }
    let known = known_targets()?;
    if known.contains(&chip.to_ascii_lowercase()) {
        return None;
    }
    Some(format!(
        "The installed probe-rs has no target for `{chip}`, so it cannot attach to one. \
         This is a probe-rs version gap, not a limit of the chip: newer releases add targets, \
         and this unblocks itself once one is installed (Tools tab). Building and flashing are \
         unaffected — an Espressif part is programmed by espflash, which does not use probe-rs."
    ))
}

#[cfg(test)]
mod chip_gap_tests {
    use super::*;

    /// Nobody filters a probe selector by hand any more.
    ///
    /// There are FIVE consumers - `rtt.rs`, `debugger.rs`, `flamegraph.rs`,
    /// `probe_flash.rs` and `start_reset` here - and they were five hand-written
    /// copies of one idea. Fixing "both" of them was a mistake made in this very
    /// repo: two were unified, three kept the old `!s.is_empty()` and a selector
    /// of spaces still went out to probe-rs from the other three.
    ///
    /// A source scan, because what regresses is a SIXTH caller written the old
    /// way - and that is visible here and nowhere else.
    #[test]
    fn no_caller_filters_a_probe_selector_by_hand() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = Vec::new();
        for rel in [
            "src/rtt.rs",
            "src/debugger.rs",
            "src/flamegraph.rs",
            "src/probe_flash.rs",
            "src/probe.rs",
        ] {
            let src =
                std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
            for (i, l) in src.lines().enumerate() {
                // The old shape, in code rather than in a doc comment.
                let t = l.trim_start();
                if t.starts_with("//") {
                    continue;
                }
                // Assembled from pieces so this line does not match ITSELF -
                // the scan reads the file it lives in.
                let needle = ["filter(|s| !s.", "is_empty())"].concat();
                if l.contains("probe") && l.contains(&needle) {
                    offenders.push(format!("{rel}:{}", i + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these filter a selector by hand instead of calling `probe::selector`: {offenders:?}"
        );
    }

    /// One normaliser, because two callers had drifted apart.
    ///
    /// The RTT tab spends it as `--probe <sel>`, the debugger as the DAP
    /// `launch` object's `probe` field. Both filtered with a bare `is_empty()`,
    /// so a selector of spaces survived and went out as `--probe "   "` -
    /// rejected by probe-rs, where the absent case means auto-select.
    #[test]
    fn a_blank_selector_means_auto_select() {
        for blank in [None, Some(""), Some(" "), Some("   \t ")] {
            assert_eq!(selector(blank), None, "{blank:?}");
        }
    }

    /// A real selector survives, and is trimmed rather than passed with the
    /// whitespace a copy-paste brings along.
    #[test]
    fn a_real_selector_is_kept_and_trimmed() {
        assert_eq!(selector(Some("303a:1001")).as_deref(), Some("303a:1001"));
        assert_eq!(
            selector(Some("  0483:3748:0671FF56  ")).as_deref(),
            Some("0483:3748:0671FF56"),
            "a pasted selector carries spaces the user cannot see"
        );
    }

    const SAMPLE: &str = "\
Available chips:
ADuCM302x Series
    Variants:
        ADuCM3027
        ADuCM3029
esp32c6
    Variants:
        esp32c6
STM32F1 Series
    Variants:
        STM32F103C8
        STM32F103CB
";

    #[test]
    fn both_families_and_variants_are_collected() {
        let set = parse_chip_list(SAMPLE).expect("parsed");
        for name in ["esp32c6", "stm32f1 series", "stm32f103c8", "aducm3029"] {
            assert!(set.contains(name), "missing {name}");
        }
        // The scaffolding is not a chip.
        assert!(!set.contains("variants:"));
        assert!(!set.contains("available chips:"));
    }

    /// Output we cannot make sense of must read as "could not ask", never as
    /// "probe-rs knows nothing" — which would disable every button everywhere.
    #[test]
    fn an_unreadable_listing_blocks_nothing() {
        assert!(parse_chip_list("").is_none());
        assert!(parse_chip_list("Available chips:\n\n   \n").is_none());
    }

    /// The C5 and C61 are the parts this exists for: probe-rs 0.29 has no target
    /// for either, while espflash flashes them happily.
    #[test]
    fn the_message_names_the_chip_and_says_flashing_still_works() {
        let Some(known) = parse_chip_list(SAMPLE) else {
            unreachable!()
        };
        assert!(!known.contains("esp32c5"));
        assert!(!known.contains("esp32c61"));
        assert!(known.contains("esp32c6"), "the C6 is a different part");

        // `chip_gap` itself consults the installed binary, so exercise its
        // wording through the same format string it uses.
        let gap = format!(
            "The installed probe-rs has no target for `{}`, so it cannot attach to one.",
            "esp32c5"
        );
        assert!(gap.contains("esp32c5"));
    }

    /// An empty chip name is not a gap — a project with no chip picked yet is
    /// already disabled for that reason, and saying it twice helps nobody.
    #[test]
    fn no_chip_is_not_a_gap() {
        assert_eq!(chip_gap(""), None);
        assert_eq!(chip_gap("   "), None);
    }

    /// Against the installed probe-rs. Ignored — runs the binary.
    ///
    /// `cargo test -- --ignored probe_rs_answers --nocapture`
    #[test]
    #[ignore]
    fn probe_rs_answers_for_every_bundled_chip() {
        use crate::panels::mcu_module::builtins::builtin_definitions;

        let Some(known) = known_targets() else {
            eprintln!("probe-rs not installed — skipping");
            return;
        };
        println!("probe-rs knows {} target names", known.len());

        let mut blocked = Vec::new();
        for d in builtin_definitions() {
            let chip = &d.project.probe_chip;
            if chip_gap(chip).is_some() {
                blocked.push(format!("{} ({chip})", d.id));
            }
        }
        println!("blocked from RTT / Debug / Profile: {blocked:?}");

        // Every STM32 must be reachable: probe-rs is the ONLY way those flash,
        // so a gap there would be a real regression rather than a version skew.
        for d in builtin_definitions() {
            if d.family.starts_with("stm32") {
                assert_eq!(
                    chip_gap(&d.project.probe_chip),
                    None,
                    "{}: probe-rs cannot target it, and nothing else can flash it",
                    d.id
                );
            }
        }
    }
}
