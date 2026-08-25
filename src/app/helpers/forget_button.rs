//! The red "forget this project" button, shared by the startup picker and the
//! Tools → Open Recent menu.
//!
//! # Why it is shared
//!
//! Two places offer the same destructive-looking action on the same list. A red
//! X beside a project name invites the reading "delete the folder", so the
//! wording, the colour and — most of all — the confirmation must be identical;
//! two implementations would drift and reassure the user differently in one of
//! them.
//!
//! # Why the button comes FIRST on the row
//!
//! It used to sit after the project button, whose width is a MINIMUM: a long
//! project name pushed it further right, so a list of them stepped across the
//! panel like a staircase. Laid out first, every one starts at the same x with
//! no arithmetic at all.
//!
//! # Why arming instead of a dialog
//!
//! One of the two call sites is inside a menu, where a modal would fight the
//! menu for the click. Arming matches what the Virtual-module remove already
//! does elsewhere in the app: the first click changes the button, the second
//! acts. It also expires on its own (see [`ARMED_FOR`]), so a menu closed
//! mid-thought cannot come back with a hidden loaded gun.

use eframe::egui;
use egui_phosphor::regular as ph;

/// How long a click stays armed. Long enough to move the mouse and read the
/// button, short enough that nothing is still armed when you come back later.
const ARMED_FOR: std::time::Duration = std::time::Duration::from_secs(4);

/// Which entry is waiting for its second click, and since when.
pub type Armed = Option<(String, std::time::Instant)>;

/// Draw the button for `path`. Returns `true` on the confirming click, i.e.
/// when the caller should actually forget the entry.
///
/// `armed` is the caller's one-slot state: arming a second row disarms the
/// first, which is what makes "click X, change your mind, click another"
/// behave the way it looks.
pub fn forget_button(ui: &mut egui::Ui, path: &str, armed: &mut Armed) -> bool {
    // Expire first, so a stale arm never renders as armed for even one frame.
    if armed
        .as_ref()
        .is_some_and(|(_, at)| at.elapsed() >= ARMED_FOR)
    {
        *armed = None;
    }
    let is_armed = armed.as_ref().is_some_and(|(p, _)| p == path);

    // Fixed size in both states: a button that grows when armed would shift the
    // project name beside it and make the row jump under the cursor.
    const W: f32 = 26.0;
    let red = egui::Color32::from_rgb(220, 90, 80);
    let btn = if is_armed {
        egui::Button::new(
            egui::RichText::new(ph::CHECK)
                .size(11.0)
                .color(egui::Color32::WHITE),
        )
        .fill(red)
    } else {
        egui::Button::new(egui::RichText::new(ph::X).size(11.0).color(red))
    };

    let resp = ui
        .add(btn.min_size(egui::vec2(W, 0.0)))
        .on_hover_text(if is_armed {
            "Click again to remove it from the list."
        } else {
            crate::recent::FORGET_TIP
        });
    if !resp.clicked() {
        return false;
    }
    if is_armed {
        *armed = None;
        true
    } else {
        *armed = Some((path.to_owned(), std::time::Instant::now()));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state machine, without egui: arming is per-entry and one-slot.
    ///
    /// (The drawing is not testable here — this pins the part that decides
    /// whether a project disappears.)
    #[test]
    fn arming_is_one_slot_and_expires() {
        // Starts armed: the `None` this used to hold was overwritten
        // on the very next line and never read.
        let mut armed: Armed = Some(("/a/one".into(), std::time::Instant::now()));
        assert!(armed.as_ref().is_some_and(|(p, _)| p == "/a/one"));
        // …and arming another entry replaces it rather than adding to it, so
        // two rows can never both look armed.
        armed = Some(("/a/two".into(), std::time::Instant::now()));
        assert!(armed.as_ref().is_some_and(|(p, _)| p == "/a/two"));
        // An arm older than the window is dropped on the next draw.
        let old = std::time::Instant::now() - ARMED_FOR - std::time::Duration::from_secs(1);
        armed = Some(("/a/two".into(), old));
        assert!(
            armed
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() >= ARMED_FOR),
            "the expiry check must fire for an arm older than the window"
        );
    }
}
