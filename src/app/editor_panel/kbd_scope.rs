//! Which of the two editors holds the keyboard — the arbitration both views
//! run every frame, main pass first, over one completion popup.
//!
//! Kept pure so the rules can be pinned by tests: every input that depends on
//! the frame (focus, which pass is running, whether the Reference view drew)
//! is passed in.

use crate::app::EditorSlot;

/// Is the Reference view still drawing?
///
/// `drawn` is the frame it last ran `show_code_view` in. The main pass runs
/// BEFORE it within a frame, so "live" there means it drew last frame; the
/// Reference pass itself sets `drawn` to this frame first.
///
/// Its focus flag is refreshed only by that pass. Collapsing the MCU zone, or
/// the reference file disappearing, stops the pass without clearing the flag —
/// and a stale `true` sent every main-editor keystroke to a view that no longer
/// runs.
pub(super) fn reference_view_live(drawn: Option<u64>, frame: u64) -> bool {
    view_live(drawn, frame)
}

/// The same rule for any view drawn AFTER the main pass — the Definition tab
/// too, whose keyboard flag goes stale exactly the same way once it stops
/// drawing (another tab picked, the MCU zone collapsed).
pub(super) fn view_live(drawn: Option<u64>, frame: u64) -> bool {
    drawn.is_some_and(|d| d.saturating_add(1) >= frame)
}

/// Does the Definition tab keep the keyboard a click in it gave it?
///
/// It lets go when it stops drawing (another tab picked, the MCU zone
/// collapsed) — a return to it later must not find the flag still on — and
/// when a text field takes the focus: this editor, the find bar, a commit box.
/// A press elsewhere with nothing focusable under it is handled where the tab
/// draws, since only there is its rectangle known.
pub(super) fn definition_keeps_kbd(owns: bool, live: bool, no_text_focus: bool) -> bool {
    owns && live && no_text_focus
}

/// Should the completion popup owned by `owner` close in this pass?
///
/// `editor_kbd_active` is THIS pass's keyboard scope, which is exactly why it
/// cannot be used for the other view's popup:
///
/// - **Owner `Main`** closes only in the main pass. The Reference pass runs
///   after it in the same frame, and there `!editor_kbd_active` means "the
///   Reference editor is not focused" — true whenever the user is typing in the
///   main editor. It closed the main popup one frame after every Ctrl+Space
///   while a file was open beside the editor: the spinner flashed, and nothing.
///   The main pass is always drawn, so nothing needs the Reference pass to close
///   it: once the Reference editor takes focus, the next main pass sees that.
/// - **Owner `Reference`** closes in either pass, on facts that mean the same in
///   both: the Reference view stopped drawing, or it lost focus to a text field.
///   Losing focus to nothing — a context-menu action, a click in the popup's own
///   scrollbar — is not leaving the editor, and must not close the list it just
///   opened.
pub(super) fn owner_lost_keyboard(
    owner: EditorSlot,
    is_main_pass: bool,
    editor_kbd_active: bool,
    reference_owns_kbd: bool,
    reference_live: bool,
    no_text_focus: bool,
) -> bool {
    match owner {
        EditorSlot::Main => is_main_pass && !editor_kbd_active,
        EditorSlot::Reference => !(reference_live && (reference_owns_kbd || no_text_focus)),
    }
}

#[cfg(test)]
mod tests {
    use super::{definition_keeps_kbd, owner_lost_keyboard, reference_view_live, view_live};
    use crate::app::EditorSlot::{Main, Reference};

    /// The report: Ctrl+Space in the main editor while a file is open beside it.
    /// The Reference pass sees its own editor unfocused (`editor_kbd_active`
    /// false) — that must not close the main editor's popup.
    #[test]
    fn the_reference_pass_never_closes_the_main_popup() {
        for kbd in [false, true] {
            for owns in [false, true] {
                for no_text in [false, true] {
                    assert!(!owner_lost_keyboard(Main, false, kbd, owns, true, no_text));
                }
            }
        }
    }

    #[test]
    fn the_main_popup_closes_in_its_own_pass_when_the_keys_move() {
        assert!(owner_lost_keyboard(Main, true, false, true, true, false));
        assert!(!owner_lost_keyboard(Main, true, true, false, true, false));
    }

    #[test]
    fn the_reference_popup_survives_while_its_editor_is_focused() {
        for main_pass in [false, true] {
            assert!(!owner_lost_keyboard(
                Reference, main_pass, !main_pass, true, true, false
            ));
        }
    }

    /// A context-menu "Code suggestions" leaves nothing focused; the list it
    /// opened must stay up.
    #[test]
    fn a_reference_popup_opened_from_a_menu_is_kept() {
        for main_pass in [false, true] {
            assert!(!owner_lost_keyboard(
                Reference, main_pass, main_pass, false, true, true
            ));
        }
    }

    #[test]
    fn a_reference_popup_closes_when_another_text_field_takes_the_keys() {
        for main_pass in [false, true] {
            assert!(owner_lost_keyboard(
                Reference, main_pass, main_pass, false, true, false
            ));
        }
    }

    /// Collapsed MCU zone: the Reference view is gone, so its popup is too,
    /// focus flag or not.
    #[test]
    fn a_reference_popup_closes_once_its_view_stops_drawing() {
        assert!(owner_lost_keyboard(
            Reference, true, true, true, false, true
        ));
    }

    /// The report behind the flag: a click in the Definition tab focuses
    /// nothing, and that must leave the keyboard with the tab.
    #[test]
    fn the_definition_tab_keeps_the_keys_while_nothing_else_is_focused() {
        assert!(definition_keeps_kbd(true, true, true));
    }

    #[test]
    fn the_definition_tab_lets_go_to_a_text_field_or_when_hidden() {
        assert!(
            !definition_keeps_kbd(true, true, false),
            "a text field took focus"
        );
        assert!(
            !definition_keeps_kbd(true, false, true),
            "the tab stopped drawing"
        );
        assert!(!definition_keeps_kbd(false, true, true), "never had it");
    }

    #[test]
    fn any_view_is_live_only_while_it_keeps_drawing() {
        assert!(view_live(Some(4), 5));
        assert!(!view_live(Some(3), 5));
        assert!(!view_live(None, 5));
    }

    #[test]
    fn the_reference_view_is_live_only_while_it_keeps_drawing() {
        assert!(!reference_view_live(None, 10));
        assert!(reference_view_live(Some(10), 10), "drew this frame");
        assert!(
            reference_view_live(Some(9), 10),
            "drew last frame (main pass)"
        );
        assert!(!reference_view_live(Some(8), 10), "skipped a frame: stale");
        assert!(reference_view_live(Some(u64::MAX), 3), "no overflow panic");
    }
}
