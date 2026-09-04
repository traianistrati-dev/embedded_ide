//! Orthogonal routing for the wire between a module box and a pad.
//!
//! A straight diagonal from a box to its pad reads fine when the pad is directly
//! across the channel, and badly the moment it is not: a wire to a pad on the
//! next side of the package cuts the corner, and one to a pad on the far side
//! crosses the die itself. A schematic does not do that, and a reader following
//! one wire out of eight has nothing to follow.
//!
//! ## There is no search here
//!
//! No obstacle set, no cost model, no A*. The package already leaves a corridor
//! and the whole router is "get onto it, walk it, get off it":
//!
//! * `PinGeom::anchor` puts every edge pad's tip at `chip ± PIN_HEIGHT` (50);
//! * `packed_rect` puts every auto-packed box's facing edge at
//!   `chip ± (PIN_HEIGHT + PIN_GAP)` (68).
//!
//! So the ring taken down the middle of that 18 px gap — `chip ± 59` — is 9 px
//! clear of every stub tip and 9 px clear of every packed box, with nothing at
//! all sitting on it. Clearance is a property of WHERE the ring is, not
//! something a search has to discover, which is why this module is a hundred
//! lines of arithmetic instead of a graph.
//!
//! ## It always yields
//!
//! [`route`] returns `None` rather than a bad path: on a 45° diamond (where a
//! pad's outward vector is not axis-aligned), for a ball pad (whose anchor is
//! inside the body), and for a box dragged into the corridor itself. The caller
//! draws today's straight segment in every one of those cases — which is also
//! why there is no per-rotation branch anywhere below. The test is on the
//! VECTOR, and a diamond simply fails it.

use crate::panels::mcu_module::mcu::model::PIN_HEIGHT;
use eframe::egui;

/// Half the channel between the stub tips and the packed boxes: the ring's
/// distance out from the pad tips.
pub const RING_MID: f32 = super::modules::PIN_GAP / 2.0;

/// How far a wire steps straight off its pad, and off its box, before it turns.
/// Equal to [`RING_MID`] by construction — that step IS the one onto the ring.
pub const LEAD: f32 = RING_MID;

/// The fillet on a wire's corners.
pub const WIRE_R: f32 = 8.0;

/// The free corridor around the package, taken down its middle.
///
/// See the module docs for why this rect is clear by construction.
pub fn ring(display_chip: egui::Rect) -> egui::Rect {
    display_chip.expand(PIN_HEIGHT + RING_MID)
}

/// Whether `v` points along an axis — the guard that makes a rotated diamond
/// fall back to a straight wire without this module knowing what a diamond is.
pub fn is_axis(v: egui::Vec2) -> bool {
    (v.x.abs() < 1e-3 && v.y.abs() > 1e-3) || (v.y.abs() < 1e-3 && v.x.abs() > 1e-3)
}

/// Ring edge ids: 0 top, 1 right, 2 bottom, 3 left.
fn edge_line(r: egui::Rect, e: usize) -> f32 {
    match e {
        0 => r.top(),
        1 => r.right(),
        2 => r.bottom(),
        _ => r.left(),
    }
}

/// The ring's four corners, `c[e]` being where edge `e` starts when the edges
/// are walked clockwise from the top-left.
fn corner(r: egui::Rect, e: usize) -> egui::Pos2 {
    match e % 4 {
        0 => r.left_top(),
        1 => r.right_top(),
        2 => r.right_bottom(),
        _ => r.left_bottom(),
    }
}

/// The ring edge an outward direction leads to.
fn edge_of(d: egui::Vec2) -> usize {
    if d.y < 0.0 {
        0
    } else if d.x > 0.0 {
        1
    } else if d.y > 0.0 {
        2
    } else {
        3
    }
}

/// The first ring LINE crossed travelling from `p` along the axis unit `d`, as
/// the point on that line and the edge's id.
///
/// The point may lie past the ring's corner. That is not a bug: it is the
/// extension the far-side lane runs on, and [`ring_walk`] brings it back round.
pub fn ring_hit(r: egui::Rect, p: egui::Pos2, d: egui::Vec2) -> Option<(egui::Pos2, usize)> {
    let (e_pos, e_neg) = if d.x.abs() > d.y.abs() {
        (1_usize, 3_usize)
    } else {
        (2, 0)
    };
    let along = |e: usize| {
        let line = edge_line(r, e);
        let (from, dir) = if d.x.abs() > d.y.abs() {
            (p.x, d.x)
        } else {
            (p.y, d.y)
        };
        let t = (line - from) / dir;
        (t > 0.0).then(|| (p + d * t, e))
    };
    // Whichever of the two parallel lines is AHEAD, nearest first.
    let mut hits: Vec<(egui::Pos2, usize)> = [e_pos, e_neg].into_iter().filter_map(along).collect();
    hits.sort_by(|a, b| (a.0 - p).length().total_cmp(&(b.0 - p).length()));
    hits.into_iter().next()
}

/// The ring corners strictly between edge `ea` and edge `eb`, the SHORT way
/// round.
///
/// Both directions are built and measured; the shorter wins and clockwise breaks
/// a tie, so the answer never depends on which wire asked first.
pub fn ring_walk(
    r: egui::Rect,
    a: egui::Pos2,
    ea: usize,
    b: egui::Pos2,
    eb: usize,
) -> Vec<egui::Pos2> {
    if ea == eb {
        return Vec::new();
    }
    let build = |cw: bool| {
        let mut out = Vec::new();
        let mut e = ea;
        while e != eb {
            let next = if cw { (e + 1) % 4 } else { (e + 3) % 4 };
            // Walking clockwise off edge e crosses corner c[e+1]; walking
            // anticlockwise off edge e crosses corner c[e].
            out.push(corner(r, if cw { e + 1 } else { e }));
            e = next;
        }
        out
    };
    let len = |pts: &[egui::Pos2]| {
        let mut all = vec![a];
        all.extend_from_slice(pts);
        all.push(b);
        all.windows(2).map(|w| (w[1] - w[0]).length()).sum::<f32>()
    };
    let (cw, ccw) = (build(true), build(false));
    if len(&ccw) < len(&cw) { ccw } else { cw }
}

/// Drop any point that sits on the segment through its neighbours, so a route
/// that needed no corner comes back as the one segment it really is.
pub fn dedup_collinear(pts: Vec<egui::Pos2>) -> Vec<egui::Pos2> {
    let mut out: Vec<egui::Pos2> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last().is_some_and(|l| (p - *l).length() < 0.01) {
            continue;
        }
        if out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            let v = p - a;
            let n = v.length();
            if n > 0.01 {
                let t = ((b - a).dot(v)) / (n * n);
                if (b - (a + v * t)).length() < 0.5 && (0.0..=1.0).contains(&t) {
                    out.pop();
                }
            }
        }
        out.push(p);
    }
    out
}

/// The orthogonal route from a box terminal to a pad anchor, or `None` when the
/// geometry cannot carry one and the caller should draw a straight segment.
///
/// * `anchor` / `adir` — the pad's stub tip and its outward unit vector;
/// * `term` / `tdir` — the box's terminal and the outward normal of the edge it
///   leaves by;
/// * `body` — the package, used only to refuse a ball pad.
pub fn route(
    ring: egui::Rect,
    body: egui::Rect,
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Option<Vec<egui::Pos2>> {
    // Four refusals, and each of them is a real shape this cannot draw.
    if !is_axis(adir) || !is_axis(tdir) {
        // A 45° diamond. A diagonal package reads correctly with diagonal
        // wires, and forcing axis-aligned ones onto it would be a lie about
        // where its pins are.
        return None;
    }
    if body.contains(anchor) {
        // A ball: its anchor is inside the die, so there is no stub to leave.
        return None;
    }
    if ring.contains(term) {
        // A box dragged into the corridor itself — the route would start on the
        // lane it is supposed to join.
        return None;
    }
    let a1 = anchor + adir * RING_MID;
    let ea = edge_of(adir);
    let (t2, eb) = ring_hit(ring, term, tdir)?;
    let mut pts = vec![anchor, a1];
    pts.extend(ring_walk(ring, a1, ea, t2, eb));
    pts.push(t2);
    pts.push(term);
    let pts = dedup_collinear(pts);
    (pts.len() >= 2).then_some(pts)
}

/// The path a wire is actually drawn along: TERMINAL first, pad last.
///
/// That orientation is not cosmetic. A Custom module's arrowhead is aimed along
/// the route's last leg, and "last" has to mean the same end for every wire or
/// half the arrows point into the middle of the diagram.
///
/// Always at least two points: a geometry [`route`] refuses falls back to the
/// straight segment the canvas drew before this module existed.
pub fn wire_path(
    ring: egui::Rect,
    body: egui::Rect,
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Vec<egui::Pos2> {
    match route(ring, body, anchor, adir, term, tdir) {
        Some(p) => p.into_iter().rev().collect(),
        None => vec![term, anchor],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip() -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(-100.0, -100.0), egui::pos2(100.0, 100.0))
    }

    fn axis_aligned(pts: &[egui::Pos2]) -> bool {
        pts.windows(2)
            .all(|w| (w[1].x - w[0].x).abs() < 0.01 || (w[1].y - w[0].y).abs() < 0.01)
    }

    /// The corridor is 9 px clear of the pad tips and 9 px clear of the packed
    /// boxes. Both numbers come from constants this module does not own, so the
    /// derivation is pinned here rather than left to drift into the stubs.
    #[test]
    fn the_corridor_runs_down_the_middle_of_the_free_channel() {
        let c = chip();
        let r = ring(c);
        let tip = c.right() + PIN_HEIGHT;
        let box_edge = c.right() + PIN_HEIGHT + super::super::modules::PIN_GAP;
        assert!((r.right() - tip - RING_MID).abs() < 0.01, "clear of the stubs");
        assert!(
            (box_edge - r.right() - RING_MID).abs() < 0.01,
            "and clear of the boxes"
        );
    }

    /// The everyday wire — a pad directly across the channel from its box — must
    /// come back as the ONE straight segment it is today. Every other case is a
    /// change to how the diagram looks; this one may not be.
    #[test]
    fn a_pad_across_from_its_box_is_still_one_straight_segment() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.right() + PIN_HEIGHT, 20.0);
        let term = egui::pos2(c.right() + PIN_HEIGHT + super::super::modules::PIN_GAP, 20.0);
        let pts = route(r, c, anchor, egui::vec2(1.0, 0.0), term, egui::vec2(-1.0, 0.0))
            .expect("a route");
        assert_eq!(pts.len(), 2, "{pts:?}");
        assert_eq!(pts[0], anchor);
        assert_eq!(pts[1], term);
    }

    /// The user's case: a box on the LEFT reaching a pad on the TOP. Right
    /// angles all the way, and it goes round the corner instead of cutting it.
    #[test]
    fn a_perpendicular_wire_turns_instead_of_cutting_the_corner() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(-30.0, c.top() - PIN_HEIGHT);
        let term = egui::pos2(
            c.left() - PIN_HEIGHT - super::super::modules::PIN_GAP,
            -20.0,
        );
        let pts = route(r, c, anchor, egui::vec2(0.0, -1.0), term, egui::vec2(1.0, 0.0))
            .expect("a route");
        assert!(axis_aligned(&pts), "{pts:?}");
        assert!(pts.len() >= 3, "it really does turn: {pts:?}");
        assert_eq!(*pts.first().expect("start"), anchor);
        assert_eq!(*pts.last().expect("end"), term);
    }

    /// A pad on the FAR side of the package: the wire goes around, never across
    /// the die.
    #[test]
    fn a_far_side_wire_goes_around_the_package() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.left() - PIN_HEIGHT, 10.0);
        let term = egui::pos2(
            c.right() + PIN_HEIGHT + super::super::modules::PIN_GAP,
            -10.0,
        );
        let pts = route(r, c, anchor, egui::vec2(-1.0, 0.0), term, egui::vec2(-1.0, 0.0))
            .expect("a route");
        assert!(axis_aligned(&pts), "{pts:?}");
        assert!(
            !pts.windows(2).any(|w| {
                // No segment may pass through the die.
                let (a, b) = (w[0], w[1]);
                let mid = a.lerp(b, 0.5);
                c.contains(mid) || c.contains(a) || c.contains(b)
            }),
            "a segment crosses the package: {pts:?}"
        );
    }

    /// Every refusal draws today's straight wire instead of a wrong one.
    #[test]
    fn a_geometry_it_cannot_carry_is_refused_rather_than_guessed() {
        let c = chip();
        let r = ring(c);
        let d = std::f32::consts::FRAC_1_SQRT_2;
        // A 45 degree diamond's pad.
        assert!(
            route(
                r,
                c,
                egui::pos2(-140.0, -140.0),
                egui::vec2(-d, -d),
                egui::pos2(-200.0, -200.0),
                egui::vec2(1.0, 1.0)
            )
            .is_none()
        );
        // A ball, whose anchor is inside the die.
        assert!(
            route(
                r,
                c,
                egui::pos2(0.0, 0.0),
                egui::vec2(1.0, 0.0),
                egui::pos2(200.0, 0.0),
                egui::vec2(-1.0, 0.0)
            )
            .is_none()
        );
        // A box dragged into the corridor itself.
        assert!(
            route(
                r,
                c,
                egui::pos2(150.0, 0.0),
                egui::vec2(1.0, 0.0),
                egui::pos2(120.0, 0.0),
                egui::vec2(-1.0, 0.0)
            )
            .is_none()
        );
    }

    /// Terminal first, pad last, every time — a Custom module's arrowhead is
    /// aimed along the last leg, and "last" has to mean the same end for every
    /// wire or half the arrows point into the middle of the diagram.
    #[test]
    fn a_drawn_path_always_runs_from_the_terminal_to_the_pad() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(-30.0, c.top() - PIN_HEIGHT);
        let term = egui::pos2(c.left() - PIN_HEIGHT - super::super::modules::PIN_GAP, -20.0);
        let p = wire_path(r, c, anchor, egui::vec2(0.0, -1.0), term, egui::vec2(1.0, 0.0));
        assert_eq!(p[0], term);
        assert_eq!(*p.last().expect("an end"), anchor);
        assert!(p.len() > 2, "and it is the routed one: {p:?}");
    }

    /// A refused geometry still draws a wire — the straight one the canvas drew
    /// before this module existed.
    #[test]
    fn a_refused_geometry_still_draws_the_old_straight_wire() {
        let c = chip();
        let r = ring(c);
        let ball = egui::pos2(0.0, 0.0);
        let term = egui::pos2(200.0, 0.0);
        let p = wire_path(r, c, ball, egui::vec2(1.0, 0.0), term, egui::vec2(-1.0, 0.0));
        assert_eq!(p, vec![term, ball]);
    }

    /// The walk is the SHORT way round, and it does not depend on which wire
    /// asked first.
    #[test]
    fn the_walk_takes_the_short_way_round() {
        let r = ring(chip());
        // From the top edge to the right edge: one corner, the top-right one.
        let w = ring_walk(r, egui::pos2(0.0, r.top()), 0, egui::pos2(r.right(), 0.0), 1);
        assert_eq!(w, vec![r.right_top()]);
        // The other way is three corners, so it loses.
        let w = ring_walk(r, egui::pos2(r.right(), 0.0), 1, egui::pos2(0.0, r.top()), 0);
        assert_eq!(w.len(), 1, "{w:?}");
    }

    /// A route that needed no corner comes back as the segment it really is —
    /// which is what keeps the everyday wire byte-identical to today's.
    #[test]
    fn a_straight_run_is_collapsed_back_to_two_points() {
        let pts = vec![
            egui::pos2(0.0, 0.0),
            egui::pos2(5.0, 0.0),
            egui::pos2(10.0, 0.0),
            egui::pos2(10.0, 0.0),
        ];
        assert_eq!(
            dedup_collinear(pts),
            vec![egui::pos2(0.0, 0.0), egui::pos2(10.0, 0.0)]
        );
    }
}
