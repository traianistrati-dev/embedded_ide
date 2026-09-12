//! Diagram rotation for the Pins canvas (view-only — never touches the model's
//! pin/side vecs or codegen).
//!
//! Two SHAPES, and three kinds of package feeding them
//! ([`RotMode::for_package`]):
//! * **Quarter** — a 2-sided (DIP) chip turns 90° clockwise (vertical ⇄
//!   horizontal). Everything stays axis-aligned, so the pins are re-drawn on
//!   their rotated screen side reusing the normal per-side pin renderers.
//! * **Diamond** — a 4-sided (QFP) chip, or a BALL GRID (BGA, WLCSP), becomes a
//!   45° diamond. That is a real 2-D rotation: geometry is rotated for drawing
//!   via [`Rot`], and the pointer is inverse-rotated for hit-testing.
//!
//! A grid rides the same 45° rotation as a quad because a ball is a POINT, and
//! a point transform is all the diamond is. What the grid still cannot do is
//! TRANSPOSE (row ⇄ column) — which is what "turn the ballout" means on a
//! datasheet, and is a different, so far unimplemented, operation. This module
//! used to refuse a grid outright on the strength of that distinction, which
//! left the toggle lit over a chip that never moved.
//!
//! Both compute pin geometry in the chip's LOCAL (un-rotated) frame — identical
//! to the default layout — then apply [`Rot`]. Angles are clockwise-positive to
//! match egui's y-down screen space.

use super::super::model::Mcu;
use eframe::egui;

/// The rotation the current chip + toggle produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RotMode {
    /// Default orientation (no rotation).
    None,
    /// 2-sided (DIP) chip rotated 90° clockwise.
    Quarter,
    /// 4-sided (QFP) chip or ball grid rotated 45° clockwise (diamond).
    Diamond,
}

/// Hover text for the toggle, one per shape. Whole literals, not assembled from
/// continued ones: a `\`-continuation renders as a run of spaces the moment
/// rustfmt reflows it, and this file has no reason to risk that.
const DIAMOND_HINT: &str =
    "Rotate the chip 45° into a diamond — helps line up pins & modules. Toggle off to reset.";
const QUARTER_HINT: &str =
    "Rotate the chip 90° (vertical / horizontal) — helps line up pins & modules.";

impl RotMode {
    /// The shape this PACKAGE turns into — what the toggle *would* do, asked
    /// without reference to whether it is currently on.
    ///
    /// Split out from [`Self::of`] because the hover text has to name the
    /// rotation BEFORE it happens, and while it ran its own package test the
    /// two could disagree: [`Mcu::is_quad_package`] counts non-empty side vecs
    /// and needs three, so a ball grid — whose four sides are all empty, every
    /// pin being a cell — scored zero and was promised a 90° turn.
    pub fn for_package(mcu: &Mcu) -> Self {
        if mcu.is_quad_package() || mcu.has_inner_pins() {
            RotMode::Diamond
        } else {
            RotMode::Quarter
        }
    }

    /// Pick the mode from the chip's package and its `rotated` toggle.
    pub fn of(mcu: &Mcu) -> Self {
        if mcu.rotated {
            Self::for_package(mcu)
        } else {
            RotMode::None
        }
    }

    /// What the Rotate toggle promises, for its hover text.
    pub fn hint(self) -> &'static str {
        match self {
            RotMode::Diamond => DIAMOND_HINT,
            _ => QUARTER_HINT,
        }
    }

    /// Rotation angle in radians (clockwise, y-down).
    pub fn angle(self) -> f32 {
        match self {
            RotMode::None => 0.0,
            RotMode::Quarter => std::f32::consts::FRAC_PI_2,
            RotMode::Diamond => std::f32::consts::FRAC_PI_4,
        }
    }
}

/// A rotation about `center` by `angle` radians (clockwise in egui's y-down
/// space). `angle == 0` is the identity, so callers can always route geometry
/// through it regardless of the mode.
#[derive(Clone, Copy)]
pub struct Rot {
    pub center: egui::Pos2,
    pub angle: f32,
}

impl Rot {
    pub fn new(center: egui::Pos2, angle: f32) -> Self {
        Self { center, angle }
    }

    /// Rotate a point about `center` by `+angle`.
    pub fn apply(&self, p: egui::Pos2) -> egui::Pos2 {
        let (s, c) = self.angle.sin_cos();
        let d = p - self.center;
        egui::pos2(
            self.center.x + d.x * c - d.y * s,
            self.center.y + d.x * s + d.y * c,
        )
    }

    /// Inverse rotation (screen → local frame) — used to hit-test the pointer.
    pub fn inverse(&self, p: egui::Pos2) -> egui::Pos2 {
        Rot {
            center: self.center,
            angle: -self.angle,
        }
        .apply(p)
    }

    /// Rotate a direction/vector (no translation).
    pub fn vec(&self, v: egui::Vec2) -> egui::Vec2 {
        let (s, c) = self.angle.sin_cos();
        egui::vec2(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// The 4 rotated corners of `rect` (in min→max ring order), for a filled
    /// [`egui::Shape::convex_polygon`].
    pub fn quad(&self, rect: egui::Rect) -> Vec<egui::Pos2> {
        vec![
            self.apply(rect.left_top()),
            self.apply(rect.right_top()),
            self.apply(rect.right_bottom()),
            self.apply(rect.left_bottom()),
        ]
    }
}

/// Which screen edge an outward direction points to (nearest axis) — used to
/// pick a per-side pin renderer (Quarter) and to place module boxes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScreenSide {
    Right,
    Left,
    Top,
    Bottom,
}

impl ScreenSide {
    pub fn from_outward(v: egui::Vec2) -> Self {
        if v.x.abs() >= v.y.abs() {
            if v.x >= 0.0 {
                ScreenSide::Right
            } else {
                ScreenSide::Left
            }
        } else if v.y >= 0.0 {
            ScreenSide::Bottom
        } else {
            ScreenSide::Top
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::mock_mcu;

    // ── Which shape a package turns into ────────────────────────────────────
    // `RotMode::of` had no test at all until a ball grid turned out not to
    // rotate — the gate that produced the bug was the one uncovered thing in
    // the file. These cover it in both directions: what turns, and what the
    // button SAYS will turn.

    /// A ball grid becomes a diamond.
    ///
    /// It cannot lean on [`Mcu::is_quad_package`]: a WLCSP has all four side
    /// vecs empty — every pin is a grid cell — so that predicate says no. The
    /// diamond is right anyway, because a ball is a POINT and 45° is a point
    /// transform; only a row/column TRANSPOSE would need edges to land on.
    #[test]
    fn a_ball_grid_rotates_into_a_diamond() {
        let mut mcu = mock_mcu::create_wlcsp12();
        assert!(
            !mcu.is_quad_package(),
            "a WLCSP has no edge pins to count, which is the trap"
        );
        mcu.rotated = true;
        assert_eq!(RotMode::of(&mcu), RotMode::Diamond);
    }

    /// The TOGGLE decides whether anything happens, the PACKAGE only decides
    /// what. The old gate confused the two: it answered `None` for a grid
    /// however the toggle stood, so the button lit up over a chip that never
    /// moved.
    #[test]
    fn an_unrotated_grid_is_still_upright() {
        let mcu = mock_mcu::create_wlcsp12();
        assert!(!mcu.rotated, "the fixture starts upright");
        assert_eq!(RotMode::of(&mcu), RotMode::None);
        assert_eq!(
            RotMode::for_package(&mcu),
            RotMode::Diamond,
            "…but it is a diamond the moment it is switched on"
        );
    }

    /// Every package the app can draw has a rotation.
    ///
    /// A new package kind that fell through to `None` would draw nothing and
    /// explain nothing — the exact failure this file used to have — so it fails
    /// here instead of on someone's screen.
    #[test]
    fn every_package_has_a_rotation() {
        for mcu in [
            mock_mcu::create_stm32f103c8tx(),
            mock_mcu::create_wlcsp12(),
            mock_mcu::create_two_sided(),
        ] {
            assert_ne!(RotMode::for_package(&mcu), RotMode::None, "{}", mcu.name);
        }
    }

    /// The hover text and the rotation read ONE predicate.
    ///
    /// They did not: the toggle ran its own `is_quad_package()` test, so on a
    /// ball grid it promised a 90° turn while the renderer would have delivered
    /// 45°. Two hand-written package tests is how that divergence was possible
    /// at all, and this is what stops a third from appearing.
    #[test]
    fn the_hint_matches_the_mode() {
        for mcu in [
            mock_mcu::create_stm32f103c8tx(),
            mock_mcu::create_wlcsp12(),
            mock_mcu::create_two_sided(),
        ] {
            let mode = RotMode::for_package(&mcu);
            let hint = mode.hint();
            assert_eq!(
                hint.contains("45°"),
                mode == RotMode::Diamond,
                "{} says: {hint}",
                mcu.name
            );
            assert_eq!(
                hint.contains("90°"),
                mode == RotMode::Quarter,
                "{} says: {hint}",
                mcu.name
            );
        }
    }

    #[test]
    fn identity_is_noop() {
        let r = Rot::new(egui::pos2(10.0, 20.0), 0.0);
        let p = egui::pos2(3.0, 4.0);
        assert!((r.apply(p) - p).length() < 1e-4);
    }

    #[test]
    fn inverse_round_trips() {
        let r = Rot::new(egui::pos2(5.0, 5.0), std::f32::consts::FRAC_PI_4);
        let p = egui::pos2(9.0, -2.0);
        let back = r.inverse(r.apply(p));
        assert!((back - p).length() < 1e-3, "{back:?}");
    }

    #[test]
    fn quarter_cw_left_goes_up_right_goes_down() {
        // 90° clockwise (egui y-down): left (9 o'clock) → up (12 o'clock), which
        // is exactly the "rotate right → pin 1 moves to the top" the user asked
        // for; the opposite right edge → down.
        let r = Rot::new(egui::pos2(0.0, 0.0), std::f32::consts::FRAC_PI_2);
        let up = r.vec(egui::vec2(-1.0, 0.0));
        assert!(up.x.abs() < 1e-4 && (up.y + 1.0).abs() < 1e-4, "{up:?}");
        let down = r.vec(egui::vec2(1.0, 0.0));
        assert!(
            down.x.abs() < 1e-4 && (down.y - 1.0).abs() < 1e-4,
            "{down:?}"
        );
    }

    #[test]
    fn screen_side_snaps_to_nearest_axis() {
        assert_eq!(
            ScreenSide::from_outward(egui::vec2(1.0, 0.2)),
            ScreenSide::Right
        );
        assert_eq!(
            ScreenSide::from_outward(egui::vec2(-1.0, 0.2)),
            ScreenSide::Left
        );
        assert_eq!(
            ScreenSide::from_outward(egui::vec2(0.2, 1.0)),
            ScreenSide::Bottom
        );
        assert_eq!(
            ScreenSide::from_outward(egui::vec2(0.2, -1.0)),
            ScreenSide::Top
        );
    }
}
