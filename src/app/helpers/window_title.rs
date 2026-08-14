//! The window title — which is also the label Windows puts over each taskbar
//! thumbnail, and the text in Alt+Tab.
//!
//! It used to be fixed at startup ("Embedded IDE", "Embedded IDE #2"), chosen
//! before any project was loaded, so two windows peeked from the taskbar were
//! indistinguishable — the thumbnails showed different projects while the
//! labels said nothing. The project name is what the user is actually looking
//! for there, so the title follows it.
//!
//! Pure on purpose: the composition rules (when the instance marker earns its
//! place, where the name is cut) are the whole substance and are tested here
//! rather than by squinting at a taskbar.

/// Longest project name kept whole, in CHARACTERS. Generous: Windows truncates
/// the thumbnail label to the thumbnail's width by itself, so this cap is not
/// what makes the peek readable — it only stops a pathological folder name from
/// filling Alt+Tab and the title bar, where there IS room for the full name.
const MAX_NAME: usize = 40;

/// Cut `name` to `max` characters, marking the cut. Counts characters, not
/// bytes, so a non-ASCII folder name can't be split mid-character.
fn truncate(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_owned();
    }
    // The ellipsis is safe with the bundled font — `glyph_guard` bans the
    // arrows, not this.
    name.chars().take(max).collect::<String>() + "…"
}

/// Build the title from the open project, this instance's marker, and whether
/// another window has the same project open.
///
/// `tag` is the instance marker (`"#2"`), `None` for the first instance. It is
/// shown only where it earns its place:
/// - no project → there is no name to tell the windows apart;
/// - `duplicate` → the same project IS open twice, so the names are identical
///   and the marker is the only thing separating them (this is the state the
///   project-folder claim already tracks).
///
/// Otherwise the project name alone identifies the window, and a permanent
/// "#2" would be the noise this change exists to remove.
pub(crate) fn compose(project: Option<&str>, tag: Option<&str>, duplicate: bool) -> String {
    match (project, tag) {
        (Some(name), Some(tag)) if duplicate => format!("{} {tag}", truncate(name, MAX_NAME)),
        (Some(name), _) => truncate(name, MAX_NAME),
        (None, Some(tag)) => format!("Embedded IDE {tag}"),
        (None, None) => "Embedded IDE".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_NAME, compose, truncate};

    #[test]
    fn the_project_name_is_the_title() {
        assert_eq!(compose(Some("blink_f411"), None, false), "blink_f411");
        // A second instance with its own project needs no marker — the name
        // already tells the two windows apart.
        assert_eq!(compose(Some("blink_f411"), Some("#2"), false), "blink_f411");
    }

    #[test]
    fn the_marker_appears_only_where_it_disambiguates() {
        // Nothing open: the marker is all there is.
        assert_eq!(compose(None, Some("#2"), false), "Embedded IDE #2");
        assert_eq!(compose(None, None, false), "Embedded IDE");
        // Same project in both windows: identical names, so the marker is back.
        assert_eq!(
            compose(Some("radar"), Some("#3"), true),
            "radar #3",
            "the duplicate case is exactly what the folder claim detects"
        );
        // …but a duplicate in the FIRST instance has no marker to show.
        assert_eq!(compose(Some("radar"), None, true), "radar");
    }

    #[test]
    fn long_names_are_cut_with_an_ellipsis() {
        let long = "STM32F103_mw_radar_led_fade_encoder_display_and_more";
        let out = compose(Some(long), None, false);
        assert!(out.starts_with("STM32F103_mw_radar_led"));
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), MAX_NAME + 1);
        // A name that fits is left exactly as it is — no stray ellipsis.
        assert_eq!(truncate("short", MAX_NAME), "short");
    }

    /// Counting characters, not bytes: cutting a multi-byte name mid-character
    /// would panic on a byte slice.
    #[test]
    fn a_non_ascii_name_is_cut_by_characters() {
        let name = "ăăăăă".repeat(20); // 100 chars, 200 bytes
        let out = truncate(&name, 40);
        assert_eq!(out.chars().count(), 41);
    }
}
