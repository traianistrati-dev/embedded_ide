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
];

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
        ] {
            assert!(
                HINTS.iter().any(|h| h.tag == tag),
                "{tag} has no hint entry"
            );
        }
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
