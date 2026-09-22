//! The automatic arrangement inside a chip frame, and a spot for a frame that
//! has none yet.
//!
//! The frame is the only thing the user moves; everything inside it is placed
//! here, from the chip alone:
//!
//! ```text
//! ┌ STM32F103C8 ───────────────────────────┐
//! │ stm32_main · Blocking                  │
//! │                          ( USART1 )    │
//! │ ( OLED 128x32 )─┐        ( I2C1   )    │
//! │ ( BME280      )─┴────────(        )    │
//! │ ( Status LED  )          ( SPI1   )    │
//! └────────────────────────────────────────┘
//! ```
//!
//! Modules in the right-hand column, one row each. Devices in the left-hand
//! column, each level with the module(s) it uses and pushed down only as far as
//! it takes not to overlap the device above; a device on bare pads has no
//! module to sit beside and goes last. All in the frame's LOCAL coordinates:
//! the canvas adds the frame's position.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::snapshot::ChipView;

/// Inner margin of a frame.
pub const PAD: f32 = 14.0;
/// Title and subtitle, above the first row.
pub const HEADER_H: f32 = 48.0;
/// One module or device.
pub const PILL_H: f32 = 24.0;
/// Space between two rows in a column.
pub const ROW_GAP: f32 = 8.0;
/// Device column width.
pub const DEVICE_W: f32 = 128.0;
/// Module column width.
pub const MODULE_W: f32 = 104.0;
/// Between the columns: room for the device-to-module wires.
pub const COLUMN_GAP: f32 = 36.0;
/// Every frame has the same width, so the columns line up across chips.
pub const FRAME_W: f32 = PAD + DEVICE_W + COLUMN_GAP + MODULE_W + PAD;
/// The body of a frame with nothing in it: one line of text.
pub const NOTE_H: f32 = 28.0;
/// Between frames the canvas places side by side.
pub const FRAME_GAP: f32 = 80.0;
/// Where the first frame of a new system goes.
pub const ORIGIN: Pos2 = pos2(40.0, 40.0);

/// Where everything in one frame goes.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameLayout {
    pub size: Vec2,
    /// One rect per [`ChipView::modules`], same order.
    pub modules: Vec<Rect>,
    /// One rect per [`ChipView::devices`], same order.
    pub devices: Vec<Rect>,
    /// A device-to-module wire: the module's index, then device end to module
    /// end with the two corners between.
    pub wires: Vec<(usize, [Pos2; 4])>,
    /// Where the "nothing here" or problem text goes, when there is one.
    pub note: Option<Rect>,
}

/// Lay out one chip's frame.
pub fn frame_layout(chip: &ChipView) -> FrameLayout {
    let module_x = PAD + DEVICE_W + COLUMN_GAP;
    let row = |y: f32, x: f32, w: f32| Rect::from_min_size(pos2(x, y), vec2(w, PILL_H));
    let step = PILL_H + ROW_GAP;

    let modules: Vec<Rect> = chip
        .modules
        .iter()
        .enumerate()
        .map(|(i, _)| row(HEADER_H + i as f32 * step, module_x, MODULE_W))
        .collect();

    // Attached devices first, ordered by the middle of their modules, so a
    // device lands as close to level with what it uses as the ones above allow.
    let anchor = |i: usize| -> Option<f32> {
        let ms = &chip.devices[i].modules;
        (!ms.is_empty())
            .then(|| ms.iter().map(|&m| modules[m].center().y).sum::<f32>() / ms.len() as f32)
    };
    let mut order: Vec<usize> = (0..chip.devices.len()).collect();
    order.sort_by(|&a, &b| match (anchor(a), anchor(b)) {
        (Some(x), Some(y)) => x.total_cmp(&y).then(a.cmp(&b)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(&b),
    });
    let mut devices = vec![Rect::NOTHING; chip.devices.len()];
    let mut cursor = HEADER_H;
    for i in order {
        let want = anchor(i).map_or(cursor, |c| c - PILL_H / 2.0);
        let y = want.max(cursor);
        devices[i] = row(y, PAD, DEVICE_W);
        cursor = y + step;
    }

    let mid_x = PAD + DEVICE_W + COLUMN_GAP / 2.0;
    let mut wires = Vec::new();
    for (d, dev) in chip.devices.iter().enumerate() {
        let from = devices[d].right_center();
        for &m in &dev.modules {
            let to = modules[m].left_center();
            wires.push((m, [from, pos2(mid_x, from.y), pos2(mid_x, to.y), to]));
        }
    }

    let bottom = modules
        .iter()
        .chain(devices.iter())
        .map(|r| r.bottom())
        .fold(f32::NEG_INFINITY, f32::max);
    let empty = chip.modules.is_empty() && chip.devices.is_empty();
    let (note, body_bottom) = if empty || chip.problem.is_some() {
        let top = if empty { HEADER_H } else { bottom + ROW_GAP };
        let r = Rect::from_min_size(pos2(PAD, top), vec2(FRAME_W - 2.0 * PAD, NOTE_H));
        (Some(r), r.bottom())
    } else {
        (None, bottom)
    };
    FrameLayout {
        size: vec2(FRAME_W, body_bottom + PAD),
        modules,
        devices,
        wires,
        note,
    }
}

/// Top-left for every frame: its saved spot, or - for one that has none yet -
/// the next spot to the right of everything already placed, in list order.
///
/// Computed, not stored, until the user drags the frame: a chip added by hand
/// to `system.config` without a position still gets a sensible place.
pub fn frame_positions(frames: &[(Option<(f32, f32)>, Vec2)]) -> Vec<Pos2> {
    let placed_right = frames
        .iter()
        .filter_map(|(p, size)| p.map(|(x, _)| x + size.x))
        .fold(f32::NEG_INFINITY, f32::max);
    let mut next_x = if placed_right.is_finite() {
        placed_right + FRAME_GAP
    } else {
        ORIGIN.x
    };
    frames
        .iter()
        .map(|(p, size)| match p {
            Some((x, y)) => pos2(*x, *y),
            None => {
                let at = pos2(next_x, ORIGIN.y);
                next_x += size.x + FRAME_GAP;
                at
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::board::snapshot::{DeviceItem, ModuleItem};
    use crate::panels::mcu_module::modules::ModuleKind;

    fn chip(modules: &[&str], devices: &[(&str, &[usize])]) -> ChipView {
        ChipView {
            dir: "c".into(),
            chip: "C".into(),
            runtime: None,
            modules: modules
                .iter()
                .map(|n| ModuleItem {
                    name: (*n).into(),
                    kind: ModuleKind::GenericInterfaceUsart,
                    instance: 1,
                    pins: Default::default(),
                })
                .collect(),
            devices: devices
                .iter()
                .map(|(n, m)| DeviceItem {
                    name: (*n).into(),
                    modules: m.to_vec(),
                })
                .collect(),
            problem: None,
        }
    }

    fn overlaps(rs: &[Rect]) -> bool {
        rs.iter()
            .enumerate()
            .any(|(i, a)| rs[i + 1..].iter().any(|b| a.intersects(*b)))
    }

    /// A device sits level with its module when nothing is in the way, and
    /// the wire runs from one to the other.
    #[test]
    fn a_device_sits_level_with_its_module() {
        let c = chip(&["USART1", "I2C1", "SPI1"], &[("OLED", &[1])]);
        let l = frame_layout(&c);
        assert_eq!(l.devices[0].center().y, l.modules[1].center().y);
        assert_eq!(l.wires.len(), 1);
        let (m, pts) = l.wires[0];
        assert_eq!(m, 1);
        assert_eq!(pts[0], l.devices[0].right_center());
        assert_eq!(pts[3], l.modules[1].left_center());
    }

    /// Two devices on one module stack instead of overlapping; a device on
    /// bare pads goes after them; nothing leaves the frame or crosses the
    /// module column.
    #[test]
    fn devices_never_overlap_and_stay_in_their_column() {
        let c = chip(
            &["USART1", "I2C1"],
            &[
                ("Status LED", &[]),
                ("OLED", &[1]),
                ("BME280", &[1]),
                ("Bridge", &[0, 1]),
            ],
        );
        let l = frame_layout(&c);
        assert!(!overlaps(&l.devices));
        assert!(!overlaps(&l.modules));
        let frame = Rect::from_min_size(Pos2::ZERO, l.size);
        let module_x = l.modules[0].left();
        for r in l.devices.iter().chain(&l.modules) {
            assert!(frame.contains_rect(*r), "{r:?} outside {frame:?}");
            assert!(r.top() >= HEADER_H);
        }
        for d in &l.devices {
            assert!(d.right() < module_x);
        }
        let led = l.devices[0];
        assert!(
            l.devices[1..].iter().all(|d| d.top() < led.top()),
            "bare-pad device last"
        );
        assert_eq!(l.wires.len(), 4, "Bridge draws one wire per module");
    }

    /// An empty chip, and a chip that could not be read, still get a body with
    /// room for one line saying so.
    #[test]
    fn an_empty_or_broken_chip_keeps_a_note_row() {
        let empty = frame_layout(&chip(&[], &[]));
        let note = empty.note.unwrap();
        assert!(note.top() >= HEADER_H);
        assert_eq!(empty.size.y, note.bottom() + PAD);
        let broken = frame_layout(&ChipView::broken("x", "Folder not found"));
        assert!(broken.note.is_some());
        let mut half = chip(&["USART1"], &[]);
        half.problem = Some("Unknown chip".into());
        let l = frame_layout(&half);
        assert!(l.note.unwrap().top() > l.modules[0].bottom());
    }

    /// Unplaced frames line up to the right of the placed ones, in order, never
    /// on top of each other.
    #[test]
    fn frames_without_a_spot_go_right_of_the_placed_ones() {
        let size = vec2(FRAME_W, 200.0);
        let pos = frame_positions(&[(None, size), (Some((100.0, 300.0)), size), (None, size)]);
        assert_eq!(pos[1], pos2(100.0, 300.0));
        let first_free = 100.0 + FRAME_W + FRAME_GAP;
        assert_eq!(pos[0], pos2(first_free, ORIGIN.y));
        assert_eq!(pos[2], pos2(first_free + FRAME_W + FRAME_GAP, ORIGIN.y));
        assert_eq!(frame_positions(&[(None, size)])[0], ORIGIN);
    }
}
