//! Diagram rotation for the Pins canvas (view-only — never touches the model's
//! pin/side vecs or codegen).
//!
//! Two modes, chosen by the package ([`RotMode::of`]):
//! * **Quarter** — a 2-sided (DIP) chip turns 90° clockwise (vertical ⇄
//!   horizontal). Everything stays axis-aligned, so the pins are re-drawn on
//!   their rotated screen side reusing the normal per-side pin renderers.
//! * **Diamond** — a 4-sided (QFP) chip becomes a 45° diamond. That is a real
//!   2-D rotation: geometry is rotated for drawing via [`Rot`], and the pointer
//!   is inverse-rotated for hit-testing.
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
    /// 4-sided (QFP) chip rotated 45° clockwise (diamond).
    Diamond,
}

impl RotMode {
    /// Pick the mode from the chip's package and its `rotated` toggle.
    pub fn of(mcu: &Mcu) -> Self {
        // A ball grid has no edges to rotate onto: turning it means transposing
        // (row, column), which is a different operation from either mode here.
        // Until that exists, a grid package simply doesn't rotate.
        if mcu.grid.is_some() {
            RotMode::None
        } else if !mcu.rotated {
            RotMode::None
        } else if mcu.is_quad_package() {
            RotMode::Diamond
        } else {
            RotMode::Quarter
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
