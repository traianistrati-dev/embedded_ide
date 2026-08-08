//! One place that knows what a tagged build/tool failure MEANS, and one card
//! that renders it.
//!
//! Background jobs (`build`, `profile`, …) prefix a fatal message with a
//! `[TAG]` when the cause is a known, actionable environment problem rather
//! than the user's code:
//!
//! ```text
//! [MSVC_LIBS] The MSVC toolchain can't find its C-runtime libraries …
//! ```
//!
//! Before this module every tab stripped its own prefix by hand and rendered
//! its own box — so a new tag (e.g. `[MSVC_LIBS]`) showed up as raw text with
//! the marker still in it, and nothing offered a way out. Now the mapping tag →
//! (headline, responsible tool, guidance) lives here, and [`show_card`] renders
//! the same banner everywhere, with an **Open Tools** button when a catalog
//! entry can fix it (see [`crate::required_tools`]).

use eframe::egui;
use egui_phosphor::regular as ph;

/// What a `[TAG]` means and who can fix it.
pub struct Hint {
    /// Marker at the very start of the message, e.g. `[MSVC_LIBS]`.
    pub tag: &'static str,
    /// Short headline for the card.
    pub title: &'static str,
    /// Matching [`crate::required_tools`] entry name → offer "Open Tools".
    /// `None` for problems no tool install can fix (e.g. a full disk).
    pub tool: Option<&'static str>,
}

pub const HINTS: &[Hint] = &[
    Hint {
        tag: "[MSVC_LIBS]",
        title: "MSVC build tools missing or incomplete",
        tool: Some("MSVC build tools"),
    },
    Hint {
        tag: "[CLIPPY_MISSING]",
        title: "Clippy isn't installed for this toolchain",
        tool: None, // installed with `rustup component add clippy`, not a catalog row
    },
    Hint {
        tag: "[BLOAT_MISSING]",
        title: "cargo-bloat isn't installed",
        tool: Some("cargo-bloat"),
    },
    Hint {
        tag: "[DISK_FULL]",
        title: "Disk full",
        tool: None, // freeing space is the caller's own action
    },
    Hint {
        tag: "[FLASH_FULL]",
        title: "Firmware doesn't fit in the chip's memory",
        tool: None, // nothing to install — the binary has to get smaller
    },
    Hint {
        tag: "[PROBE_RS_PANIC]",
        title: "probe-rs crashed (upstream bug)",
        tool: Some("probe-rs"), // the catalog row reinstalls / updates it
    },
];

/// The `panicked at <where>` line of a probe-rs crash, plus the message under
/// it — `None` when the output holds no panic.
///
/// probe-rs runs as a subprocess (dap-server, `run`, `list`), so a panic inside
/// it reaches us only as text: the process dies and the socket/pipe closes,
/// which on its own looks like an ordinary end of session.
pub fn probe_rs_panic(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let i = lines.iter().position(|l| l.contains("panicked at"))?;
    // `panicked at <path>:<line>:<col>:` then the message on the NEXT line.
    let where_ = lines[i]
        .split_once("panicked at ")
        .map_or(lines[i], |(_, rest)| rest);
    // Keep the crate@version + file, drop the local registry path noise.
    let where_ = where_
        .rsplit_once("index.crates.io-")
        .map_or(where_, |(_, rest)| {
            rest.split_once('\\').map_or(rest, |(_, r)| r)
        });
    let what = lines.get(i + 1).copied().unwrap_or_default();
    Some(if what.is_empty() || what.starts_with("stack backtrace") {
        where_.to_owned()
    } else {
        format!("{where_}\n  {what}")
    })
}

/// The tagged message for a probe-rs crash. The point is to say plainly that
/// this is probe-rs failing, not the user's firmware or this IDE — the console
/// alone shows a backtrace of `<unknown>` frames that reads like a local bug.
pub fn probe_rs_panic_message(detail: &str) -> String {
    format!(
        "[PROBE_RS_PANIC] probe-rs itself panicked and its process died — the session \
         ended with it:\n  {detail}\n\n\
         Nothing is wrong with your firmware or your probe wiring: this is a bug inside \
         probe-rs, usually hit while it enumerates USB devices looking for probes.\n\n\
         -> Update it:  cargo install probe-rs-tools --locked  (Open Tools runs this)\n\
         -> If the newest release still crashes, pin an older one:\n   \
         cargo install probe-rs-tools --locked --version 0.29.0\n\
         -> Meanwhile, unplug other dev boards / USB adapters and retry — enumeration \
         touches every candidate device.\n\n\
         Scan in the probe selector runs the same enumeration, so it is the quickest \
         way to tell whether a version change helped."
    )
}

/// `rust-lld`'s linker-script overflow, as one compact line — `None` when the
/// output holds none.
///
/// The raw form is one line per section, all overflowing by roughly the same
/// amount, e.g.:
/// ```text
/// rust-lld: error: section '.text' will not fit in region 'FLASH': overflowed by 11472 bytes
/// ```
/// Only the FIRST is kept: the others are the same shortfall counted again, and
/// a four-line dump buries the number that matters.
pub fn flash_overflow(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| l.contains("will not fit in region"))
        .map(|l| {
            // Drop the tool prefix ("rust-lld: error: ") — the card says it.
            l.rsplit_once("error: ").map_or(l, |(_, rest)| rest).to_owned()
        })
}

/// The tagged message for a link that overflowed the chip's memory. One place
/// composes it so the Cargo tab and the RTT/Debug builds say the same thing.
pub fn flash_full_message(detail: &str) -> String {
    format!(
        "[FLASH_FULL] The linker couldn't fit the firmware into the memory declared in \
         memory.x:\n  {detail}\n\n\
         The build itself is fine — the binary is simply too big for this part.\n\n\
         -> If \"Debug-friendly build\" is ON in the Debug tab, turn it OFF: it relaxes \
         [profile.release] to opt-level = 1, which costs several KB. That is the usual \
         cause when a project that used to link suddenly doesn't.\n\
         -> Otherwise: drop features or dependencies, keep lto = true and \
         opt-level = \"s\"/\"z\", or move to a part with more Flash.\n\n\
         The Size button (Cargo / Flash tab) shows what is actually using the space."
    )
}

/// The hint for `msg` plus the message with its marker removed. Pure.
pub fn parse(msg: &str) -> Option<(&'static Hint, &str)> {
    HINTS.iter().find_map(|h| {
        msg.strip_prefix(h.tag)
            .map(|rest| (h, rest.trim_start_matches(' ')))
    })
}

/// `msg` without a leading `[TAG] ` (unchanged when it carries none) — for the
/// one-line status badges that have no room for a card.
pub fn strip(msg: &str) -> &str {
    parse(msg).map(|(_, rest)| rest).unwrap_or(msg)
}

/// egui id used to ask the app to switch to the Tools tab. A temp-data flag
/// avoids threading an out-param through `show_diag_panel` and every tab;
/// [`take_open_tools_request`] consumes it once per frame.
fn open_tools_id() -> egui::Id {
    egui::Id::new("failure_hint_open_tools")
}

/// True once after a card's "Open Tools" was clicked (clears the request).
pub fn take_open_tools_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(open_tools_id()).unwrap_or(false))
}

/// Render the explanation card for a tagged failure. Returns `false` (drawing
/// nothing) when `msg` carries no known tag, so callers can fall back to their
/// plain error view. `extra` adds tab-specific actions next to the standard
/// buttons (e.g. Cargo's "Clean target/").
pub fn show_card(ui: &mut egui::Ui, msg: &str, extra: impl FnOnce(&mut egui::Ui)) -> bool {
    let Some((hint, body)) = parse(msg) else {
        return false;
    };
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(60, 45, 10))
        .inner_margin(egui::Margin::same(8))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} {}", ph::WARNING, hint.title))
                        .size(12.0)
                        .strong()
                        .color(egui::Color32::from_rgb(250, 190, 60)),
                );
            });
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(body)
                        .size(10.5)
                        .color(egui::Color32::from_rgb(215, 200, 165)),
                )
                .wrap(),
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if hint.tool.is_some()
                    && ui
                        .button(
                            egui::RichText::new(format!("{} Open Tools", ph::WRENCH))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(255, 210, 80)),
                        )
                        .on_hover_text("Check / install this dependency")
                        .clicked()
                {
                    ui.ctx().data_mut(|d| d.insert_temp(open_tools_id(), true));
                }
                extra(ui);
            });
        });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_tags_and_strips_marker() {
        let (h, rest) = parse("[MSVC_LIBS] libs are gone").unwrap();
        assert_eq!(h.title, "MSVC build tools missing or incomplete");
        assert_eq!(rest, "libs are gone");
        assert_eq!(strip("[DISK_FULL] no space"), "no space");
    }

    #[test]
    fn untagged_message_is_untouched() {
        assert!(parse("error[E0425]: not found").is_none());
        assert_eq!(strip("error[E0425]: not found"), "error[E0425]: not found");
        // A tag must be at the START to count.
        assert!(parse("see [DISK_FULL] above").is_none());
    }

    /// Every tag a background job can emit must be in the table — otherwise it
    /// reaches the user as raw text with the marker still in it (the bug this
    /// module fixes for `[MSVC_LIBS]`).
    #[test]
    fn table_covers_every_emitted_tag() {
        for tag in [
            "[MSVC_LIBS]",
            "[CLIPPY_MISSING]",
            "[BLOAT_MISSING]",
            "[DISK_FULL]",
            "[FLASH_FULL]",
            "[PROBE_RS_PANIC]",
        ] {
            assert!(
                HINTS.iter().any(|h| h.tag == tag),
                "{tag} has no hint entry"
            );
        }
    }

    /// The linker dump repeats the same shortfall once per section — the card
    /// gets the first line, without the tool prefix, and the composed message
    /// stays parseable as a tagged hint.
    #[test]
    fn flash_overflow_is_summarised_to_one_line() {
        let dump = "  = note: rust-lld: error: section '.text' will not fit in region \
                    'FLASH': overflowed by 11472 bytes\n\
                    rust-lld: error: section '.rodata' will not fit in region 'FLASH': \
                    overflowed by 26620 bytes\n";
        let detail = flash_overflow(dump).expect("detected");
        assert_eq!(
            detail,
            "section '.text' will not fit in region 'FLASH': overflowed by 11472 bytes"
        );
        // The composed message is a real tagged hint, and keeps the numbers.
        let msg = flash_full_message(&detail);
        let (hint, body) = parse(&msg).expect("tagged");
        assert_eq!(hint.tag, "[FLASH_FULL]");
        assert!(body.contains("11472 bytes"), "{body}");
        assert!(body.contains("Debug-friendly build"), "{body}");
        // An ordinary compile error is not mistaken for one.
        assert!(flash_overflow("error[E0425]: cannot find value `x`").is_none());
    }

    /// A probe-rs crash reaches us as console text; the card needs the crate +
    /// file and the message, without the user's cargo-registry path.
    #[test]
    fn probe_rs_panic_keeps_crate_file_and_message() {
        let out = "probe-rs-debug: Starting debug session from: 127.0.0.1:54531\n\
                   thread 'main' (20992) panicked at C:\\Users\\me\\.cargo\\registry\\src\\\
                   index.crates.io-1949cf8c6b5b557f\\probe-rs-0.31.0\\src\\probe\\glasgow\\\
                   mux.rs:97:13:\n\
                   internal error: entered unreachable code\n\
                   stack backtrace:\n   0:     0x7ff699f13a82 - <unknown>\n";
        let detail = probe_rs_panic(out).expect("detected");
        assert!(detail.starts_with("probe-rs-0.31.0"), "{detail}");
        assert!(detail.contains("mux.rs:97:13"), "{detail}");
        assert!(detail.contains("entered unreachable code"), "{detail}");
        assert!(!detail.contains("index.crates.io"), "registry noise:\n{detail}");

        let msg = probe_rs_panic_message(&detail);
        let (hint, body) = parse(&msg).expect("tagged");
        assert_eq!(hint.tool, Some("probe-rs"), "the card offers Open Tools");
        assert!(body.contains("probe-rs-tools --locked"), "{body}");
        // Ordinary probe-rs chatter is not a crash.
        assert!(probe_rs_panic("probe-rs-debug: Listening on port 54529").is_none());
    }

    /// A hint that names a tool must name one that actually exists in the
    /// catalog, or "Open Tools" would send the user to a row that isn't there.
    #[test]
    fn referenced_tools_exist_in_the_catalog() {
        let state = crate::required_tools::make_tools_state();
        let state = state.lock().unwrap();
        for h in HINTS.iter().filter_map(|h| h.tool.map(|t| (h.tag, t))) {
            let (tag, tool) = h;
            assert!(
                state.tools.iter().any(|t| t.name == tool),
                "{tag} points at unknown tool {tool:?}"
            );
        }
    }
}
