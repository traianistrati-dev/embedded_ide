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

/// How far beyond the outermost end a shared lane is placed.
///
/// The lane a wire shares with the package edge has to be visibly OFF the pins,
/// not tucked against them. `RING_MID` puts it 9 px past the stub tips, which on
/// screen reads as running along the pin row rather than clear of it.
pub const LANE_OUT: f32 = 34.0;

/// Whether `q` lies ahead of `p` along the axis unit `d`.
fn ahead(p: egui::Pos2, d: egui::Vec2, q: egui::Pos2) -> bool {
    (q - p).dot(d) > 0.5
}

/// The coordinate of `p` ALONG `d`'s axis.
fn on(p: egui::Pos2, d: egui::Vec2) -> f32 {
    if d.x.abs() > d.y.abs() { p.x } else { p.y }
}

/// The coordinate of `p` ACROSS `d`'s axis.
///
/// The distinction matters wherever the two rays are parallel: `on(p, tdir)`
/// then answers about the SAME axis as `on(p, adir)` and a lane built from it
/// collapses onto itself.
fn across(p: egui::Pos2, d: egui::Vec2) -> f32 {
    if d.x.abs() > d.y.abs() { p.y } else { p.x }
}

/// A point built from an along-`d` coordinate and an across-`d` one.
fn at(d: egui::Vec2, along: f32, across: f32) -> egui::Pos2 {
    if d.x.abs() > d.y.abs() {
        egui::pos2(along, across)
    } else {
        egui::pos2(across, along)
    }
}

/// ONE corner: out along `adir`, then straight in along `tdir`.
///
/// The shape the eye reads fastest, and the one the two rays admit whenever they
/// are perpendicular and their crossing lies ahead of both. Its corner is as far
/// from the pins as the box is, which is the other half of what makes it read.
fn l_route(
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Option<Vec<egui::Pos2>> {
    if adir.dot(tdir).abs() > 0.5 {
        return None; // not perpendicular
    }
    let corner = at(adir, on(term, adir), across(anchor, adir));
    (ahead(anchor, adir, corner) && ahead(term, tdir, corner)).then(|| vec![anchor, corner, term])
}

/// TWO corners over a shared lane, for two rays pointing the SAME way.
///
/// The lane is placed beyond whichever end is already furthest out, plus
/// [`LANE_OUT`] — so it clears the pin row, the stubs and both endpoints by
/// construction, and both corners land well away from the pins.
fn z_parallel(
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Option<Vec<egui::Pos2>> {
    if adir.dot(tdir) < 0.5 {
        return None; // not the same direction
    }
    let sign = on(egui::Pos2::ZERO + adir, adir);
    let outer = if sign > 0.0 {
        on(anchor, adir).max(on(term, adir))
    } else {
        on(anchor, adir).min(on(term, adir))
    };
    let lane = outer + sign * LANE_OUT;
    Some(vec![
        anchor,
        at(adir, lane, across(anchor, adir)),
        at(adir, lane, across(term, adir)),
        term,
    ])
}

/// TWO corners in the channel, for two rays pointing AT each other — a box
/// across from its pad but not lined up with it.
///
/// The lane goes down the middle of what is between them, which is all the room
/// there is: the box is 18 px away and there is nowhere further to put it.
fn z_facing(
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Option<Vec<egui::Pos2>> {
    if adir.dot(tdir) > -0.5 || !ahead(anchor, adir, term) {
        return None;
    }
    let lane = (on(anchor, adir) + on(term, adir)) / 2.0;
    Some(vec![
        anchor,
        at(adir, lane, across(anchor, adir)),
        at(adir, lane, across(term, adir)),
        term,
    ])
}

/// What a wire may not cross.
#[derive(Clone, Copy)]
pub struct Blocked<'a> {
    /// The package.
    pub body: egui::Rect,
    /// Every module box on the canvas.
    pub boxes: &'a [egui::Rect],
    /// Where in `boxes` this wire's OWN box sits.
    ///
    /// Exempt on the FINAL segment only — the one that reaches the terminal.
    /// That segment ends on the box by definition, and for a chamfered box the
    /// terminal is snapped onto the silhouette and can sit inside the
    /// axis-aligned rect while being visually outside the shape.
    ///
    /// Every OTHER segment is held to the same rule as any foreign box. A
    /// blanket exemption let a wire run the length of its own box's interior on
    /// the way to its terminal, which looks exactly as wrong as crossing
    /// somebody else's.
    pub own: usize,
}

fn hits(a: egui::Pos2, b: egui::Pos2, r: egui::Rect) -> bool {
    crate::panels::structure_map::layout::seg_hits_rect(
        (a.x, a.y),
        (b.x, b.y),
        r.left(),
        r.top(),
        r.width(),
        r.height(),
    )
}

/// Whether every segment of `pts` stays out of the package and out of every
/// module box but its own.
///
/// A wire drawn across a box reads as though it connects to it. That is the one
/// thing a schematic line may never suggest wrongly, so a route that would do it
/// is refused and a longer one is taken instead.
fn clears(pts: &[egui::Pos2], b: Blocked<'_>) -> bool {
    // Routes are built anchor-first, so the LAST window is the one that lands on
    // the terminal — the only place the wire is entitled to touch its own box.
    let last = pts.len().saturating_sub(2);
    !pts.windows(2).enumerate().any(|(k, w)| {
        hits(w[0], w[1], b.body)
            || b.boxes
                .iter()
                .enumerate()
                .any(|(i, r)| !(i == b.own && k == last) && hits(w[0], w[1], *r))
    })
}

/// The orthogonal route from a box terminal to a pad anchor, or `None` when the
/// geometry cannot carry one and the caller should try another box edge or fall
/// back to a straight segment.
///
/// Candidates in order of how few corners they cost, first one that clears the
/// package wins: straight, then the one-corner L, then a two-corner lane. The
/// package-wrapping ring is the last resort, for a pad on the far side where
/// nothing shorter can avoid crossing the die.
///
/// * `anchor` / `adir` — the pad's stub tip and its outward unit vector;
/// * `term` / `tdir` — the box's terminal and the outward normal of the edge it
///   leaves by;
/// * `body` — the package, which no segment may cross.
pub fn route(
    ring: egui::Rect,
    blocked: Blocked<'_>,
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Option<Vec<egui::Pos2>> {
    let body = blocked.body;
    // Three refusals, and each of them is a real shape this cannot draw.
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
    if body.contains(term) {
        return None;
    }
    let cheap = [
        l_route(anchor, adir, term, tdir),
        z_facing(anchor, adir, term, tdir),
        z_parallel(anchor, adir, term, tdir),
    ];
    for c in cheap.into_iter().flatten() {
        let pts = dedup_collinear(c);
        if pts.len() >= 2 && clears(&pts, blocked) {
            return Some(pts);
        }
    }
    // Nothing short works: something is in the way - the package, or a module
    // box - so the wire goes around.
    wrap(ring, anchor, adir, term, tdir)
}

/// The last resort: onto the corridor around the package, round it, and off.
///
/// Only reached when no one- or two-corner route is clear — in practice a box
/// wired to a pad on the opposite face.
///
/// It does NOT re-check what it produces. The corridor is clear of the die by
/// construction, so there is nothing there to catch; and a box dragged onto the
/// corridor is accepted rather than refused, because refusing here hands the
/// caller its straight-segment fallback, and one diagonal across the die and
/// three boxes is worse than a wire that clips one.
fn wrap(
    ring: egui::Rect,
    anchor: egui::Pos2,
    adir: egui::Vec2,
    term: egui::Pos2,
    tdir: egui::Vec2,
) -> Option<Vec<egui::Pos2>> {
    if ring.contains(term) {
        // A box dragged onto the corridor itself — the route would start on the
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

/// The cheaper of several ways out of a box, as the drawn path and the terminal
/// it leaves from.
///
/// FEWEST CORNERS wins, and a tie goes to the earlier candidate — so the
/// everyday wire, whose first candidate is already a straight segment, is never
/// talked out of it.
pub fn best_route(
    ring: egui::Rect,
    blocked: Blocked<'_>,
    anchor: egui::Pos2,
    adir: egui::Vec2,
    cands: &[(egui::Pos2, egui::Vec2)],
) -> Option<(Vec<egui::Pos2>, egui::Pos2)> {
    cands
        .iter()
        .enumerate()
        .filter_map(|(i, (t, d))| {
            route(ring, blocked, anchor, adir, *t, *d).map(|p| (p.len(), i, p, *t))
        })
        .min_by_key(|(n, i, ..)| (*n, *i))
        .map(|(_, _, p, t)| (p.into_iter().rev().collect(), t))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canvas with nothing on it but the package — what most of these tests
    /// are about, since the box obstacles have their own.
    fn free(c: egui::Rect) -> Blocked<'static> {
        Blocked {
            body: c,
            boxes: &[],
            own: usize::MAX,
        }
    }

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
        assert!(
            (r.right() - tip - RING_MID).abs() < 0.01,
            "clear of the stubs"
        );
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
        let term = egui::pos2(
            c.right() + PIN_HEIGHT + super::super::modules::PIN_GAP,
            20.0,
        );
        let pts = route(
            r,
            free(c),
            anchor,
            egui::vec2(1.0, 0.0),
            term,
            egui::vec2(-1.0, 0.0),
        )
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
        let pts = route(
            r,
            free(c),
            anchor,
            egui::vec2(0.0, -1.0),
            term,
            egui::vec2(1.0, 0.0),
        )
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
        let pts = route(
            r,
            free(c),
            anchor,
            egui::vec2(-1.0, 0.0),
            term,
            egui::vec2(-1.0, 0.0),
        )
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
                free(c),
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
                free(c),
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
                free(c),
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
        let term = egui::pos2(
            c.left() - PIN_HEIGHT - super::super::modules::PIN_GAP,
            -20.0,
        );
        let (p, _) = best_route(
            r,
            free(c),
            anchor,
            egui::vec2(0.0, -1.0),
            &[(term, egui::vec2(1.0, 0.0))],
        )
        .expect("a route");
        assert_eq!(p[0], term);
        assert_eq!(*p.last().expect("an end"), anchor);
        assert!(p.len() > 2, "and it is the routed one: {p:?}");
    }

    /// A geometry the router refuses yields NOTHING, so the caller falls back to
    /// the straight segment the canvas drew before this module existed.
    #[test]
    fn a_refused_geometry_yields_nothing_to_draw() {
        let c = chip();
        let r = ring(c);
        let ball = egui::pos2(0.0, 0.0);
        let term = egui::pos2(200.0, 0.0);
        assert!(
            best_route(
                r,
                free(c),
                ball,
                egui::vec2(1.0, 0.0),
                &[(term, egui::vec2(-1.0, 0.0))]
            )
            .is_none()
        );
    }

    /// The shape the user drew for PWM0: a pad on the bottom edge, its box below
    /// and to the right. ONE corner, and it sits down at the box's own level
    /// rather than up against the pin row.
    #[test]
    fn a_pad_below_its_box_turns_once_far_from_the_pins() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.left() + 30.0, c.bottom() + PIN_HEIGHT);
        let term = egui::pos2(c.right() + 68.0, c.bottom() + 200.0);
        let pts = route(
            r,
            free(c),
            anchor,
            egui::vec2(0.0, 1.0),
            term,
            egui::vec2(-1.0, 0.0),
        )
        .expect("a route");
        assert_eq!(pts.len(), 3, "one corner: {pts:?}");
        assert_eq!(pts[1], egui::pos2(anchor.x, term.y), "and it is the L's");
        assert!(
            pts[1].y - anchor.y > 100.0,
            "the corner is well clear of the pin row: {pts:?}"
        );
    }

    /// The shape the user drew for I2C0: pads on the top edge, the box away to
    /// the left. Leaving by the box's TOP edge makes the two rays parallel, and
    /// the lane then goes ABOVE both of them instead of crawling along the pins.
    #[test]
    fn a_box_beside_the_pads_row_runs_its_lane_clear_of_them() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.right() - 30.0, c.top() - PIN_HEIGHT);
        // Out of the box's own top edge, pointing the same way the pad does —
        // and FURTHER out than the pad, which is what says the lane is placed
        // beyond whichever end is already outermost rather than beyond the pad.
        let term = egui::pos2(c.left() - 120.0, c.top() - PIN_HEIGHT - 60.0);
        let pts = route(
            r,
            free(c),
            anchor,
            egui::vec2(0.0, -1.0),
            term,
            egui::vec2(0.0, -1.0),
        )
        .expect("a route");
        assert_eq!(pts.len(), 4, "two corners: {pts:?}");
        let lane = pts[1].y;
        assert_eq!(pts[2].y, lane, "and one shared lane");
        // The corners sit directly out from each end — a lane whose ends do not
        // line up with the endpoints is not a lane, it is two diagonals.
        assert_eq!(pts[1].x, anchor.x, "{pts:?}");
        assert_eq!(pts[2].x, term.x, "{pts:?}");
        assert!(
            term.y - lane >= LANE_OUT,
            "the lane clears the OUTERMOST end by {}: {pts:?}",
            term.y - lane
        );
        assert!(anchor.y - lane >= LANE_OUT, "and the pad tips too: {pts:?}");
    }

    /// The L is only an L when its corner lies AHEAD of both ends. A pad on the
    /// top edge and a box away to the left, leaving by the edge that faces the
    /// chip, cross behind the pad — taking that corner would run the wire back
    /// down through the pin row it just left.
    #[test]
    fn an_l_whose_corner_lies_behind_an_end_is_refused() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.right() - 30.0, c.top() - PIN_HEIGHT);
        // The terminal sits BELOW the pad tip but still above the die, so the L
        // this would take is clear of the package - only the ahead test can
        // refuse it, which is the point of the test.
        let term = egui::pos2(c.left() - 68.0, c.top() - 20.0);
        let corner = egui::pos2(anchor.x, term.y);
        assert!(
            !c.contains(corner) && corner.y > anchor.y,
            "the corner is behind the pad and outside the die: {corner:?}"
        );
        let pts = route(
            r,
            free(c),
            anchor,
            egui::vec2(0.0, -1.0),
            term,
            egui::vec2(1.0, 0.0),
        )
        .expect("a route");
        assert!(
            !pts.contains(&corner),
            "it took the corner behind the pad: {pts:?}"
        );
        assert!(pts.len() > 3, "so it costs more than an L: {pts:?}");
    }

    /// …and the choice between the two ways out of the box is the one that costs
    /// fewer corners. Through the box's facing edge this same wire has to wrap
    /// around the package.
    #[test]
    fn the_cheaper_way_out_of_the_box_is_the_one_taken() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.right() - 30.0, c.top() - PIN_HEIGHT);
        let facing = (
            egui::pos2(c.left() - 68.0, c.top() + 40.0),
            egui::vec2(1.0, 0.0),
        );
        let top = (
            egui::pos2(c.left() - 120.0, c.top() + 40.0),
            egui::vec2(0.0, -1.0),
        );
        let (pts, term) =
            best_route(r, free(c), anchor, egui::vec2(0.0, -1.0), &[facing, top]).expect("a route");
        assert_eq!(term, top.0, "the top edge won: {pts:?}");
        assert_eq!(pts.len(), 4);
        assert_eq!(pts[0], top.0, "terminal first");
    }

    /// A tie goes to the FIRST candidate, so the everyday straight wire is never
    /// talked out of the edge facing the chip.
    #[test]
    fn a_tie_keeps_the_edge_facing_the_chip() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.right() + PIN_HEIGHT, 20.0);
        let facing = (
            egui::pos2(
                c.right() + PIN_HEIGHT + super::super::modules::PIN_GAP,
                20.0,
            ),
            egui::vec2(-1.0, 0.0),
        );
        let other = (egui::pos2(c.right() + 200.0, 20.0), egui::vec2(-1.0, 0.0));
        let (pts, term) = best_route(r, free(c), anchor, egui::vec2(1.0, 0.0), &[facing, other])
            .expect("a route");
        assert_eq!(pts.len(), 2, "still one straight segment");
        assert_eq!(term, facing.0);
    }

    /// A wire drawn across a module box reads as though it connects to it. A box
    /// standing in the way of the short route is an obstacle like the package,
    /// and the wire takes the longer way rather than the lie.
    #[test]
    fn a_wire_goes_around_a_box_in_its_way_rather_than_through_it() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.left() + 30.0, c.bottom() + PIN_HEIGHT);
        let term = egui::pos2(c.right() + 68.0, c.bottom() + 200.0);
        let adir = egui::vec2(0.0, 1.0);
        let tdir = egui::vec2(-1.0, 0.0);
        // With nothing in the way it is the one-corner L.
        let plain = route(r, free(c), anchor, adir, term, tdir).expect("a route");
        assert_eq!(plain.len(), 3);

        // A foreign box sitting exactly on that L's long leg.
        let other = egui::Rect::from_min_max(
            egui::pos2(anchor.x - 40.0, c.bottom() + 100.0),
            egui::pos2(anchor.x + 40.0, c.bottom() + 150.0),
        );
        let boxes = [other];
        let blocked = Blocked {
            body: c,
            boxes: &boxes,
            own: usize::MAX,
        };
        let pts = route(r, blocked, anchor, adir, term, tdir).expect("a route");
        assert!(
            !pts.windows(2).any(|w| super::hits(w[0], w[1], other)),
            "it still crosses the box: {pts:?}"
        );
        assert_ne!(pts, plain, "so it is not the same route");
    }

    /// Its OWN box is not an obstacle to a wire, and the exemption is per-INDEX
    /// rather than a blanket "ignore boxes".
    ///
    /// Every route arrives at its terminal along the inward normal of the edge it
    /// leaves by, so it reaches the box from outside and stops on the boundary.
    /// The exemption is for the boundary itself — and for a chamfered box, whose
    /// terminal is snapped onto the silhouette and can sit inside the
    /// axis-aligned rect while being outside the drawn shape.
    #[test]
    fn a_wire_is_not_blocked_by_the_box_it_belongs_to() {
        let c = chip();
        let mine = egui::Rect::from_min_max(
            egui::pos2(c.right() + 68.0, -40.0),
            egui::pos2(c.right() + 238.0, 60.0),
        );
        // A terminal snapped onto a chamfer sits INSIDE the axis-aligned rect.
        let term = egui::pos2(mine.left() + 6.0, 20.0);
        assert!(mine.contains(term));
        let seg = [egui::pos2(c.right() + PIN_HEIGHT, 20.0), term];

        let one = [mine];
        assert!(
            clears(
                &seg,
                Blocked {
                    body: c,
                    boxes: &one,
                    own: 0
                }
            ),
            "its own box does not block it"
        );
        assert!(
            !clears(
                &seg,
                Blocked {
                    body: c,
                    boxes: &one,
                    own: usize::MAX
                }
            ),
            "and it would, without the exemption"
        );
        // Per-index: a SECOND box in the same place is foreign and still blocks.
        let two = [mine, mine];
        assert!(
            !clears(
                &seg,
                Blocked {
                    body: c,
                    boxes: &two,
                    own: 0
                }
            ),
            "the exemption is not a blanket switch"
        );
    }

    /// The corridor route collapses back to a straight segment when getting on
    /// and off the lane happens to be collinear — which is what stops the last
    /// resort from emitting four points to draw one line.
    #[test]
    fn a_degenerate_corridor_route_collapses_to_one_segment() {
        let c = chip();
        let r = ring(c);
        let anchor = egui::pos2(c.right() + PIN_HEIGHT, 20.0);
        let term = egui::pos2(c.right() + 68.0, 20.0);
        let pts =
            wrap(r, anchor, egui::vec2(1.0, 0.0), term, egui::vec2(-1.0, 0.0)).expect("a route");
        assert_eq!(pts.len(), 2, "{pts:?}");
    }

    /// The own-box exemption is for the segment that ENDS on the box, and for no
    /// other. Running the length of its own box's interior on the way to its
    /// terminal looks exactly as wrong as crossing somebody else's.
    #[test]
    fn a_wire_may_touch_its_own_box_only_where_it_ends() {
        let c = chip();
        let bx = egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(200.0, 200.0));
        let boxes = [bx];
        let mine = Blocked {
            body: egui::Rect::NOTHING,
            boxes: &boxes,
            own: 0,
        };
        let _ = c;

        // Anchor-first, so the LAST window is the one that lands on the box. Here
        // it ends just inside the rect, as a terminal snapped onto a chamfer
        // does — and that is allowed.
        let ends_on_it = [
            egui::pos2(250.0, 50.0),
            egui::pos2(250.0, 150.0),
            egui::pos2(190.0, 150.0),
        ];
        assert!(clears(&ends_on_it, mine), "the final segment may reach it");

        // The same box, crossed by the FIRST segment on the way somewhere else.
        let runs_through_it = [
            egui::pos2(50.0, 150.0),
            egui::pos2(250.0, 150.0),
            egui::pos2(250.0, 50.0),
        ];
        assert!(
            !clears(&runs_through_it, mine),
            "but a segment that only passes through is refused"
        );
    }

    /// The walk is the SHORT way round, and it does not depend on which wire
    /// asked first.
    #[test]
    fn the_walk_takes_the_short_way_round() {
        let r = ring(chip());
        // From the top edge to the right edge: one corner, the top-right one.
        let w = ring_walk(
            r,
            egui::pos2(0.0, r.top()),
            0,
            egui::pos2(r.right(), 0.0),
            1,
        );
        assert_eq!(w, vec![r.right_top()]);
        // The other way is three corners, so it loses.
        let w = ring_walk(
            r,
            egui::pos2(r.right(), 0.0),
            1,
            egui::pos2(0.0, r.top()),
            0,
        );
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
