//! Serial frames view — every protocol frame as its own row.
//!
//! The other views cut the stream where the PROTOCOL doesn't: the text/hex views
//! not at all, and the timed blocks at an idle gap, which a device that streams
//! back to back never produces. So a whole conversation lands on one line. This
//! one cuts where the frame actually ends, and lists them.
//!
//! Two ways to find that end, because protocols pick one or the other:
//!
//! * [`FrameMode::Markers`] — a start pattern and an end pattern (the Find start
//!   / Find end fields). Reuses [`crate::serial::frame_ranges`], the same
//!   framing the Matrix view and the "Between" counter already use.
//! * [`FrameMode::Length`] — a header, then a length field somewhere inside it.
//!   The only option for the many protocols with no unique tail, and the only
//!   one that can tell a TRUNCATED frame from a short one: the frame declares
//!   how long it is, so a mismatch is detectable rather than silently swallowing
//!   the next frame.
//!
//! Everything the framing does NOT claim is kept as an [`FrameKind::Unframed`]
//! row rather than dropped — bytes that vanish from a decoder are how a wrong
//! marker looks like a quiet link.

use crate::serial::{Dir, LogChunk};
use eframe::egui;
use std::time::Instant;

/// How to find a frame's boundaries.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FrameMode {
    /// Start pattern … end pattern (both from the Find fields).
    #[default]
    Markers,
    /// Header pattern + a length field at a fixed offset inside the frame.
    Length,
}

/// The framing rules, as the user set them in the toolbar.
#[derive(Clone, Debug)]
pub struct FrameSpec {
    pub mode: FrameMode,
    /// Frame start (`Markers` and `Length`) — the header pattern.
    pub start: Vec<u8>,
    /// Frame end (`Markers` only).
    pub end: Vec<u8>,
    /// Byte offset of the length field, counted from the FIRST byte of the
    /// header — the way protocol datasheets number it.
    pub len_offset: usize,
    /// Width of the length field in bytes (1, 2 or 4).
    pub len_width: usize,
    /// Little-endian length field (the common case on these MCUs).
    pub len_le: bool,
    /// How many bytes the frame carries AFTER the counted length — a tail /
    /// checksum the length field doesn't include.
    pub tail_len: usize,
    /// Does the length field count itself and the header, or only what follows?
    /// Datasheets disagree, and getting it wrong shifts every frame.
    pub len_covers_header: bool,
}

impl Default for FrameSpec {
    fn default() -> Self {
        Self {
            mode: FrameMode::Markers,
            start: Vec::new(),
            end: Vec::new(),
            len_offset: 4,
            len_width: 2,
            len_le: true,
            tail_len: 4,
            len_covers_header: false,
        }
    }
}

/// What a row IS — the reason it looks the way it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameKind {
    /// A complete frame: header through end/declared length.
    Complete,
    /// Bytes the framing never claimed (before the first header, after the last
    /// tail, or a frame still arriving). Shown, never dropped.
    Unframed,
    /// A header whose declared length doesn't fit what arrived, or whose end
    /// marker never came before the next header. The stream resyncs at that
    /// next header.
    Bad,
}

/// One row of the view: a byte range of the direction's stream.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameSlice {
    pub start: usize,
    pub end: usize,
    pub kind: FrameKind,
}

impl FrameSlice {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

/// Split `bytes` into frames + the gaps between them, per `spec`.
///
/// Pure and total: the ranges tile the input exactly once, in order, with no
/// holes — the sum of all `len()` is `bytes.len()`. That invariant is what makes
/// "nothing disappeared" checkable instead of hoped for.
pub fn split_frames(bytes: &[u8], spec: &FrameSpec) -> Vec<FrameSlice> {
    let mut out: Vec<FrameSlice> = Vec::new();
    let push = |start: usize, end: usize, kind: FrameKind, out: &mut Vec<FrameSlice>| {
        if end > start {
            out.push(FrameSlice { start, end, kind });
        }
    };
    if spec.start.is_empty() {
        push(0, bytes.len(), FrameKind::Unframed, &mut out);
        return out;
    }

    let mut cursor = 0usize; // first byte not yet emitted
    let mut i = 0usize; // scan position
    while i + spec.start.len() <= bytes.len() {
        if &bytes[i..i + spec.start.len()] != spec.start.as_slice() {
            i += 1;
            continue;
        }
        // A header. Everything skipped before it belongs to nobody.
        push(cursor, i, FrameKind::Unframed, &mut out);

        let frame_end = match spec.mode {
            FrameMode::Markers => marker_end(bytes, i, spec),
            FrameMode::Length => length_end(bytes, i, spec),
        };
        match frame_end {
            Some(end) => {
                push(i, end, FrameKind::Complete, &mut out);
                cursor = end;
                i = end;
            }
            None => {
                // Either still arriving, or broken. Tell them apart by whether
                // ANOTHER header follows: a header after a header means the
                // first frame never completed.
                match next_header(bytes, i + spec.start.len(), &spec.start) {
                    Some(next) => {
                        push(i, next, FrameKind::Bad, &mut out);
                        cursor = next;
                        i = next;
                    }
                    // Nothing else yet — the tail of the buffer is an incoming
                    // frame, not a failure.
                    None => {
                        push(i, bytes.len(), FrameKind::Unframed, &mut out);
                        return out;
                    }
                }
            }
        }
    }
    push(cursor, bytes.len(), FrameKind::Unframed, &mut out);
    out
}

/// End (exclusive) of the frame opened at `at`, per the end marker — `None`
/// when it hasn't arrived. A second header before it means the frame is broken,
/// which the caller handles.
fn marker_end(bytes: &[u8], at: usize, spec: &FrameSpec) -> Option<usize> {
    if spec.end.is_empty() {
        return None;
    }
    let mut j = at + spec.start.len();
    while j < bytes.len() {
        if bytes[j..].starts_with(&spec.end) {
            return Some(j + spec.end.len());
        }
        if bytes[j..].starts_with(&spec.start) {
            return None; // restarted — the frame at `at` never closed
        }
        j += 1;
    }
    None
}

/// End (exclusive) of the frame opened at `at`, per its length field — `None`
/// while the declared bytes haven't all arrived (or the field itself hasn't).
///
/// The length is the AUTHORITY here: a header pattern occurring inside a
/// satisfied length is payload, not a new frame. Binary payloads do contain
/// bytes that look like headers, and splitting a valid frame on that
/// coincidence is a worse failure than trusting a field the protocol defines.
fn length_end(bytes: &[u8], at: usize, spec: &FrameSpec) -> Option<usize> {
    let w = spec.len_width.clamp(1, 4);
    let lo = at + spec.len_offset;
    if lo + w > bytes.len() {
        return None; // the length field itself is still incoming
    }
    let mut v: u64 = 0;
    for k in 0..w {
        let b = bytes[lo + k] as u64;
        if spec.len_le {
            v |= b << (8 * k);
        } else {
            v = (v << 8) | b;
        }
    }
    // Where the counted region starts: after the length field, unless the field
    // counts the header too.
    let counted_from = if spec.len_covers_header {
        at
    } else {
        lo + w
    };
    let end = counted_from.checked_add(v as usize)?.checked_add(spec.tail_len)?;
    (end <= bytes.len() && end > at).then_some(end)
}

fn next_header(bytes: &[u8], from: usize, header: &[u8]) -> Option<usize> {
    if header.is_empty() {
        return None;
    }
    (from..bytes.len().saturating_sub(header.len() - 1))
        .find(|&k| &bytes[k..k + header.len()] == header)
}

/// A frame with the wall-clock of the burst its FIRST byte arrived in, and the
/// bytes themselves — what the view renders.
#[derive(Clone, Debug)]
pub struct TimedFrame {
    pub dir: Dir,
    pub at: Instant,
    pub kind: FrameKind,
    pub bytes: Vec<u8>,
}

/// Frame the logged traffic, one direction at a time, keeping arrival times.
///
/// Framing runs over each direction's CONCATENATED stream, not per block: a
/// frame split across two reads (which is the normal case at speed) must stay
/// one frame. The timestamp then comes from the block that contained the
/// frame's first byte — the closest thing to "when it started arriving" that
/// the host can honestly claim.
pub fn frames_from_log(log: &[LogChunk], spec: &FrameSpec) -> Vec<TimedFrame> {
    let mut out: Vec<TimedFrame> = Vec::new();
    for dir in [Dir::AppToSensor, Dir::SensorToApp] {
        let mut stream: Vec<u8> = Vec::new();
        // (offset where this chunk starts in `stream`, its arrival time)
        let mut marks: Vec<(usize, Instant)> = Vec::new();
        for c in log.iter().filter(|c| c.dir == dir) {
            marks.push((stream.len(), c.at));
            stream.extend_from_slice(&c.bytes);
        }
        if stream.is_empty() {
            continue;
        }
        for s in split_frames(&stream, spec) {
            // The last chunk that started at or before this frame's first byte.
            let at = marks
                .iter()
                .rev()
                .find(|(off, _)| *off <= s.start)
                .map(|(_, t)| *t)
                .unwrap_or_else(|| marks[0].1);
            out.push(TimedFrame {
                dir,
                at,
                kind: s.kind,
                bytes: stream[s.start..s.end].to_vec(),
            });
        }
    }
    // Both directions interleaved back into arrival order.
    out.sort_by_key(|f| f.at);
    out
}

/// Colour of an unframed row — bytes the framing never claimed.
const UNFRAMED: egui::Color32 = egui::Color32::from_rgb(120, 125, 135);
/// Colour of a broken frame's row.
const BAD: egui::Color32 = egui::Color32::from_rgb(225, 100, 90);
/// Colour of the `#n len=… [clock] (+Δ)` prefix.
const META: egui::Color32 = egui::Color32::from_rgb(130, 140, 160);

/// One row per frame: `#n >> len=8 [14:32:07.436] (+120 ms)  FD FC …`.
///
/// The delta is against the previous row, which for a reply right after a
/// command is the turnaround time — the number this whole view exists to show.
#[allow(clippy::too_many_arguments)]
pub fn frames_log_job(
    frames: &[TimedFrame],
    hex: bool,
    font_size: f32,
    find_a: &[u8],
    find_b: &[u8],
    epoch: Option<(Instant, std::time::SystemTime)>,
) -> egui::text::LayoutJob {
    use crate::serial::{DIR_APP, DIR_SENSOR, SEARCH_HIT, SEARCH_HIT2, match_positions};
    const MAX_ROWS: usize = 400;
    let font = egui::FontId::monospace(font_size);
    let mut job = egui::text::LayoutJob::default();
    let start = frames.len().saturating_sub(MAX_ROWS);
    let mut prev: Option<Instant> = None;

    for (n, f) in frames.iter().enumerate().skip(start) {
        let (arrow, dir_color) = match f.dir {
            Dir::AppToSensor => (">>", DIR_APP),
            Dir::SensorToApp => ("<<", DIR_SENSOR),
        };
        let body_color = match f.kind {
            FrameKind::Complete => dir_color,
            FrameKind::Unframed => UNFRAMED,
            FrameKind::Bad => BAD,
        };
        let tag = match f.kind {
            FrameKind::Complete => "     ",
            FrameKind::Unframed => " raw ",
            FrameKind::Bad => " BAD ",
        };
        let clock = match epoch {
            Some((i0, t0)) => {
                crate::activity::fmt_clock(t0 + f.at.saturating_duration_since(i0))
            }
            None => "--:--:--.---".to_string(),
        };
        let delta = match prev {
            Some(p) => format!("(+{} ms)", f.at.saturating_duration_since(p).as_millis()),
            None => String::new(),
        };
        prev = Some(f.at);
        job.append(
            &format!(
                "#{n:<4}{tag}len={:<5} [{clock}] {delta:<11}",
                f.bytes.len()
            ),
            0.0,
            egui::TextFormat::simple(font.clone(), META),
        );
        job.append(
            &format!("{arrow} "),
            0.0,
            egui::TextFormat::simple(font.clone(), dir_color),
        );

        // Per-byte colours (markers highlighted), emitted as runs.
        let n_bytes = f.bytes.len();
        let mut colors = vec![body_color; n_bytes];
        for (pat, hit) in [(find_a, SEARCH_HIT), (find_b, SEARCH_HIT2)] {
            for i in match_positions(&f.bytes, pat) {
                for c in colors.iter_mut().skip(i).take(pat.len()) {
                    *c = hit;
                }
            }
        }
        let mut i = 0;
        while i < n_bytes {
            let mut j = i + 1;
            while j < n_bytes && colors[j] == colors[i] {
                j += 1;
            }
            let run = &f.bytes[i..j];
            let text = if hex {
                run.iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
                    + " "
            } else {
                String::from_utf8_lossy(run)
                    .trim_end_matches(['\r', '\n'])
                    .to_string()
            };
            job.append(&text, 0.0, egui::TextFormat::simple(font.clone(), colors[i]));
            i = j;
        }
        job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), body_color));
    }
    job
}

/// `42 frames · 3 raw · 1 bad · payload 1280 B` — the line above the list.
pub fn frames_summary(frames: &[TimedFrame]) -> String {
    let complete = frames
        .iter()
        .filter(|f| f.kind == FrameKind::Complete)
        .count();
    let raw = frames
        .iter()
        .filter(|f| f.kind == FrameKind::Unframed)
        .count();
    let bad = frames.iter().filter(|f| f.kind == FrameKind::Bad).count();
    // The most common complete-frame size: a protocol's steady state, and the
    // number that makes an odd frame stand out.
    let mut sizes: Vec<usize> = frames
        .iter()
        .filter(|f| f.kind == FrameKind::Complete)
        .map(|f| f.bytes.len())
        .collect();
    sizes.sort_unstable();
    let typical = sizes.get(sizes.len() / 2).copied().unwrap_or(0);
    format!("{complete} frames · {raw} raw · {bad} bad · typical {typical} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_markers(a: &[u8], b: &[u8]) -> FrameSpec {
        FrameSpec {
            mode: FrameMode::Markers,
            start: a.to_vec(),
            end: b.to_vec(),
            ..Default::default()
        }
    }

    /// The ranges must TILE the input: every byte in exactly one row, in order.
    /// This is what makes "nothing was dropped" a checked property — a decoder
    /// that quietly eats bytes looks exactly like a quiet link.
    fn assert_tiles(bytes: &[u8], slices: &[FrameSlice]) {
        let mut at = 0usize;
        for s in slices {
            assert_eq!(s.start, at, "hole or overlap at {at}: {slices:?}");
            assert!(s.end > s.start, "empty slice: {s:?}");
            at = s.end;
        }
        assert_eq!(at, bytes.len(), "tail not covered: {slices:?}");
    }

    #[test]
    fn markers_split_frames_and_keep_the_leftovers() {
        let spec = spec_markers(&[0xFD, 0xFC], &[0x04, 0x03]);
        let stream = [
            0xEE, 0xEE, // noise before anything
            0xFD, 0xFC, 1, 2, 0x04, 0x03, // frame #1
            0xFD, 0xFC, 9, 0x04, 0x03, // frame #2
            0xAA, // trailing noise
        ];
        let got = split_frames(&stream, &spec);
        assert_tiles(&stream, &got);
        let kinds: Vec<FrameKind> = got.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                FrameKind::Unframed,
                FrameKind::Complete,
                FrameKind::Complete,
                FrameKind::Unframed
            ]
        );
        assert_eq!(got[1].len(), 6); // header + 2 payload + tail
        assert_eq!(got[2].len(), 5);
    }

    /// A header that never closed, followed by another header: the first is
    /// BAD and the stream resyncs — it must not swallow the good frame.
    #[test]
    fn a_truncated_frame_is_flagged_not_merged() {
        let spec = spec_markers(&[0xAA], &[0xBB]);
        let stream = [0xAA, 7, 7, 0xAA, 1, 0xBB];
        let got = split_frames(&stream, &spec);
        assert_tiles(&stream, &got);
        assert_eq!(got[0].kind, FrameKind::Bad);
        assert_eq!(got[0].len(), 3);
        assert_eq!(got[1].kind, FrameKind::Complete);
        assert_eq!(got[1].len(), 3);
    }

    /// A frame still arriving is not an error — it is the tail of the buffer.
    #[test]
    fn an_incoming_frame_stays_unframed() {
        let spec = spec_markers(&[0xAA], &[0xBB]);
        let stream = [0xAA, 1, 2];
        let got = split_frames(&stream, &spec);
        assert_tiles(&stream, &got);
        assert_eq!(got[0].kind, FrameKind::Unframed);
    }

    /// Length framing: `AA 55 | len=3 (LE u16) | 3 payload | 2 tail`.
    #[test]
    fn length_field_ends_the_frame() {
        let spec = FrameSpec {
            mode: FrameMode::Length,
            start: vec![0xAA, 0x55],
            len_offset: 2,
            len_width: 2,
            len_le: true,
            tail_len: 2,
            len_covers_header: false,
            ..Default::default()
        };
        let stream = [
            0xAA, 0x55, 0x03, 0x00, // header + len = 3
            1, 2, 3, // payload
            0xEE, 0xEF, // tail
            0xAA, 0x55, 0x01, 0x00, 9, 0xEE, 0xEF, // a second frame
        ];
        let got = split_frames(&stream, &spec);
        assert_tiles(&stream, &got);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|s| s.kind == FrameKind::Complete));
        assert_eq!(got[0].len(), 9);
        assert_eq!(got[1].len(), 7);
    }

    /// A declared length longer than what arrived: still incoming, so the tail
    /// stays Unframed — and once ANOTHER header has arrived past it, the frame
    /// is Bad and the stream resyncs there.
    ///
    /// Note what this does NOT do: a header appearing INSIDE a satisfied length
    /// is not treated as corruption. The length field is the authority in this
    /// mode — payload bytes may legitimately look like a header, and splitting
    /// valid frames on that coincidence would be the worse error.
    #[test]
    fn a_short_read_waits_then_becomes_bad() {
        let spec = FrameSpec {
            mode: FrameMode::Length,
            start: vec![0xAA],
            len_offset: 1,
            len_width: 1,
            tail_len: 0,
            end: Vec::new(),
            ..Default::default()
        };
        // Declares 4 payload bytes, only 2 arrived → still incoming.
        let waiting = [0xAA, 4, 1, 2];
        let got = split_frames(&waiting, &spec);
        assert_tiles(&waiting, &got);
        assert_eq!(got[0].kind, FrameKind::Unframed);

        // Declares 9, can never be satisfied, and another header has arrived →
        // broken; the second frame is decoded normally.
        let broken = [0xAA, 9, 1, 2, 0xAA, 1, 9];
        let got = split_frames(&broken, &spec);
        assert_tiles(&broken, &got);
        assert_eq!(got[0].kind, FrameKind::Bad);
        assert_eq!(got[0].len(), 4, "resyncs at the next header");
        assert_eq!(got[1].kind, FrameKind::Complete);

        // The length is trusted even when the payload contains the header byte.
        let inner = [0xAA, 4, 1, 2, 0xAA, 1, 9];
        let got = split_frames(&inner, &spec);
        assert_eq!(got[0].kind, FrameKind::Complete);
        assert_eq!(got[0].len(), 6);
    }

    /// `len_covers_header` shifts every boundary — the datasheet convention has
    /// to be selectable, not assumed.
    #[test]
    fn length_can_count_from_the_header() {
        let spec = FrameSpec {
            mode: FrameMode::Length,
            start: vec![0xAA],
            len_offset: 1,
            len_width: 1,
            tail_len: 0,
            len_covers_header: true,
            ..Default::default()
        };
        // len = 4 counted from the header → the frame is exactly 4 bytes.
        let stream = [0xAA, 4, 1, 2];
        let got = split_frames(&stream, &spec);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, FrameKind::Complete);
        assert_eq!(got[0].len(), 4);
    }

    /// No header configured: everything is one unframed row, never an empty
    /// view — the user has to see the bytes to work out what the header IS.
    #[test]
    fn without_a_header_nothing_is_hidden() {
        let got = split_frames(&[1, 2, 3], &FrameSpec::default());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, FrameKind::Unframed);
        assert_eq!(got[0].len(), 3);
        assert!(split_frames(&[], &FrameSpec::default()).is_empty());
    }
}
