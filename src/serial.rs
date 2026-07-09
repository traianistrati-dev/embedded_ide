//! Serial monitor — a built-in USART/UART console.
//!
//! Opens a host serial port (the USB-UART bridge / on-board VCP the firmware's
//! USART is wired to) and shows the bytes live, with a send line — so the data a
//! Virtual USART module exchanges can be read/written from inside the IDE
//! without an external terminal. Phase 1: a raw console (text / hex).
//!
//! A background thread owns a read handle and appends incoming bytes to the
//! shared [`SerialState`]; the UI keeps a `try_clone`d write handle for sending.

use eframe::egui;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cap on the retained RX byte buffer (older bytes are dropped).
const RX_CAP: usize = 64_000;

/// Shared between the background reader thread and the UI.
#[derive(Default)]
pub struct SerialState {
    /// Raw received bytes (capped to the last [`RX_CAP`]).
    pub rx: Vec<u8>,
    /// `true` while a reader thread is alive and the port is open.
    pub connected: bool,
    /// Last open / read / write error, shown in the UI.
    pub error: Option<String>,
}

/// Serial monitor state owned by `AppIde`: the shared RX buffer, the write
/// handle (while connected), and the UI selections.
pub struct SerialMonitor {
    pub state: Arc<Mutex<SerialState>>,
    /// Write half of the open port (`try_clone`d from the reader's handle).
    writer: Option<Box<dyn serialport::SerialPort>>,
    /// Per-connection stop flag for the reader thread. A fresh one is made each
    /// `connect` so a quick disconnect→reconnect can't leave the old thread
    /// running (the old flag stays `true`).
    stop: Option<Arc<AtomicBool>>,
    /// Selected port name (e.g. `COM4` / `/dev/ttyUSB0`).
    pub port: String,
    /// Selected baud rate.
    pub baud: u32,
    /// Display mode: hex bytes vs. decoded text.
    pub hex: bool,
    /// Number of bytes per repeating sequence to colour / list (hex mode).
    pub seq_len: usize,
    /// Bytes shown per line in the hex view.
    pub row_bytes: usize,
    /// Width (px) of the unique-sequences legend (draggable divider).
    pub legend_w: f32,
    /// Hex byte sequences to search/highlight in the RX view (yellow / blue).
    pub search: String,
    pub search2: String,
    /// Keep the RX view pinned to the newest data.
    pub autoscroll: bool,
    /// Append `\r\n` to each sent line.
    pub append_crlf: bool,
    /// The TX input line.
    pub tx_input: String,
    /// Height (px) of the resizable send area (dragged via its handle).
    pub tx_height: f32,
    /// Pause (ms) inserted between each line when sending a multi-line block —
    /// for command sequences where the device needs time before the next one.
    pub line_delay_ms: u64,
    /// Lines still waiting to be sent (front = next), each already fully encoded
    /// (hex parsed + optional CR+LF). Drained one-per-`line_delay_ms` by
    /// [`pump_tx_queue`], so the UI never blocks while pacing a sequence.
    tx_queue: std::collections::VecDeque<Vec<u8>>,
    /// When the next queued line is due to go out.
    tx_next_at: Option<Instant>,
    /// Cached list of available ports (refreshed on demand).
    pub ports: Vec<String>,
    /// One-shot: `false` until the baud has been seeded from the first
    /// GI_USART virtual module (done when the Serial tab first opens while
    /// idle — replaces the old toolbar Serial button's seeding).
    pub baud_seeded: bool,
}

impl Default for SerialMonitor {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(SerialState::default())),
            writer: None,
            stop: None,
            port: String::new(),
            baud: 115_200,
            hex: false,
            seq_len: 1,
            row_bytes: 16,
            legend_w: 160.0,
            search: String::new(),
            search2: String::new(),
            autoscroll: true,
            append_crlf: false,
            tx_input: String::new(),
            tx_height: 30.0,
            line_delay_ms: 100,
            tx_queue: std::collections::VecDeque::new(),
            tx_next_at: None,
            ports: Vec::new(),
            baud_seeded: false,
        }
    }
}

impl SerialMonitor {
    /// Re-enumerate the available serial ports; pick the first one if none chosen.
    pub fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports()
            .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
            .unwrap_or_default();
        if self.port.is_empty() {
            if let Some(first) = self.ports.first() {
                self.port = first.clone();
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }

    /// Open `self.port` at `self.baud` and start the background reader.
    pub fn connect(&mut self, ctx: &egui::Context) {
        if self.port.is_empty() || self.is_connected() {
            return;
        }
        // Make sure any previous reader is signalled to stop (defensive).
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        {
            let mut s = self.state.lock().unwrap();
            s.error = None;
            s.connected = false;
        }
        match serialport::new(&self.port, self.baud)
            .timeout(Duration::from_millis(100))
            .open()
        {
            Ok(port) => match port.try_clone() {
                Ok(reader) => {
                    self.writer = Some(port);
                    self.state.lock().unwrap().connected = true;
                    let stop = Arc::new(AtomicBool::new(false));
                    self.stop = Some(Arc::clone(&stop));
                    spawn_reader(reader, Arc::clone(&self.state), stop, ctx.clone());
                }
                Err(e) => self.state.lock().unwrap().error = Some(e.to_string()),
            },
            Err(e) => self.state.lock().unwrap().error = Some(e.to_string()),
        }
    }

    /// Stop the reader thread and release the port.
    pub fn disconnect(&mut self) {
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        self.state.lock().unwrap().connected = false;
        self.writer = None;
        // Drop any not-yet-sent queued lines (the port is gone).
        self.tx_queue.clear();
        self.tx_next_at = None;
    }

    /// Send raw bytes on the open port (no-op when disconnected).
    pub fn send(&mut self, bytes: &[u8]) {
        if let Some(w) = self.writer.as_mut() {
            if let Err(e) = w.write_all(bytes) {
                self.state.lock().unwrap().error = Some(e.to_string());
            }
        }
    }

    /// Queue `lines` to be sent one at a time, `line_delay_ms` apart (the first
    /// goes out on the next [`pump_tx_queue`]). Replaces any pending queue.
    pub fn queue_lines(&mut self, lines: Vec<Vec<u8>>) {
        self.tx_queue = lines.into();
        self.tx_next_at = if self.tx_queue.is_empty() {
            None
        } else {
            Some(Instant::now()) // first line is due immediately
        };
    }

    /// Send every queued line whose delay has elapsed (all of them at once when
    /// `line_delay_ms == 0`, so a zero gap is truly back-to-back). Returns the
    /// time until the next not-yet-due line (so the caller can
    /// `request_repaint_after` it), or `None` when the queue is empty. Call once
    /// per frame.
    pub fn pump_tx_queue(&mut self) -> Option<Duration> {
        while self.tx_next_at.is_some_and(|at| Instant::now() >= at) {
            if let Some(bytes) = self.tx_queue.pop_front() {
                self.send(&bytes);
            }
            self.tx_next_at = if self.tx_queue.is_empty() {
                None
            } else {
                Some(Instant::now() + Duration::from_millis(self.line_delay_ms))
            };
        }
        self.tx_next_at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    pub fn clear_rx(&mut self) {
        self.state.lock().unwrap().rx.clear();
    }
}

/// Background loop: read bytes into the shared buffer until asked to stop or the
/// port errors. The short read timeout lets it notice `stop` promptly. Repaints
/// are throttled (~30 fps) so a continuously-streaming device doesn't pin the UI
/// at max frame-rate and starve the rest of the app.
fn spawn_reader(
    mut port: Box<dyn serialport::SerialPort>,
    state: Arc<Mutex<SerialState>>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    const REPAINT_EVERY: Duration = Duration::from_millis(33);
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let mut last_repaint = Instant::now() - REPAINT_EVERY;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match port.read(&mut buf) {
                Ok(0) => {
                    // No data but not a timeout — avoid a busy-spin.
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(n) => {
                    let mut s = state.lock().unwrap();
                    s.rx.extend_from_slice(&buf[..n]);
                    if s.rx.len() > RX_CAP {
                        let excess = s.rx.len() - RX_CAP;
                        s.rx.drain(..excess);
                    }
                    drop(s);
                    // Coalesce repaints: at most one ~every 33 ms while streaming.
                    if last_repaint.elapsed() >= REPAINT_EVERY {
                        ctx.request_repaint();
                        last_repaint = Instant::now();
                    } else {
                        ctx.request_repaint_after(REPAINT_EVERY);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    let mut s = state.lock().unwrap();
                    s.error = Some(e.to_string());
                    s.connected = false;
                    drop(s);
                    ctx.request_repaint();
                    break;
                }
            }
        }
    });
}

/// Decoded-text tail of `rx` (lossy UTF-8). Only the last chunk is returned so
/// layout stays cheap. (Hex mode uses [`hex_layout_job`] for coloured output.)
pub fn render_rx_text(rx: &[u8]) -> String {
    let start = rx.len().saturating_sub(16 * 1024);
    String::from_utf8_lossy(&rx[start..]).into_owned()
}

/// A deterministic, vivid colour for a single byte — same byte → same colour,
/// nearby values clearly different (golden-ratio hue spread).
pub fn byte_color(b: u8) -> egui::Color32 {
    let hue = (b as f32 * 0.618_034).fract();
    egui::Color32::from(egui::ecolor::Hsva::new(hue, 0.6, 0.95, 1.0))
}

/// A deterministic colour for a byte *sequence* — same sequence → same colour.
/// (Single byte falls back to [`byte_color`]; longer sequences use an FNV-1a
/// hash so repeated multi-byte frames are visually linked.)
pub fn seq_color(seq: &[u8]) -> egui::Color32 {
    if let [b] = seq {
        return byte_color(*b);
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in seq {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hue = (h % 9973) as f32 / 9973.0;
    egui::Color32::from(egui::ecolor::Hsva::new(hue, 0.6, 0.95, 1.0))
}

/// Distinct `seq_len`-byte sequences in `bytes` with their counts, most-frequent
/// first — the "repeating sequences as a set of unique values" legend. Sequences
/// are aligned from offset 0; the trailing partial chunk is ignored.
pub fn seq_counts(bytes: &[u8], seq_len: usize) -> Vec<(Vec<u8>, u32)> {
    let seq_len = seq_len.max(1);
    let mut map: HashMap<Vec<u8>, u32> = HashMap::new();
    for chunk in bytes.chunks_exact(seq_len) {
        *map.entry(chunk.to_vec()).or_insert(0) += 1;
    }
    let mut out: Vec<(Vec<u8>, u32)> = map.into_iter().collect();
    // Most-repeated first; ties by sequence bytes.
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Coloured hex `LayoutJob` for the RX view: bytes are grouped into aligned
/// `seq_len`-byte sequences (from offset 0) and every byte of a group is
/// coloured by [`seq_color`] of that group, so repeated sequences share a
/// colour. Only the tail is built so layout stays cheap.
pub fn hex_layout_job(
    bytes: &[u8],
    fontsize: f32,
    seq_len: usize,
    row_bytes: usize,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let seq_len = seq_len.max(1);
    let row_bytes = row_bytes.max(1);
    let font = egui::FontId::monospace(fontsize);
    let mut job = LayoutJob::default();
    let n = bytes.len();
    let start = n.saturating_sub(4 * 1024);
    let mut cur_cs = usize::MAX;
    let mut cur_color = egui::Color32::GRAY;
    for i in start..n {
        // Aligned chunk (from offset 0) containing byte i.
        let cs = (i / seq_len) * seq_len;
        if cs != cur_cs {
            let ce = (cs + seq_len).min(n);
            cur_color = seq_color(&bytes[cs..ce]);
            cur_cs = cs;
        }
        job.append(
            &format!("{:02X} ", bytes[i]),
            0.0,
            TextFormat::simple(font.clone(), cur_color),
        );
        if (i + 1) % row_bytes == 0 {
            job.append(
                "\n",
                0.0,
                TextFormat::simple(font.clone(), egui::Color32::GRAY),
            );
        }
    }
    job
}

/// Colours of the two searched sequences (yellow / blue) and the rest (grey).
pub const SEARCH_HIT: egui::Color32 = egui::Color32::from_rgb(255, 230, 0);
pub const SEARCH_HIT2: egui::Color32 = egui::Color32::from_rgb(0, 200, 250);
pub const SEARCH_MISS: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

/// Parse a user-typed hex search string into bytes: hex digits are kept (spaces
/// / other chars ignored) and grouped into pairs; a trailing lone digit is
/// dropped. `"0D 0A"` / `"0d0a"` → `[0x0D, 0x0A]`.
pub fn parse_hex_search(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    digits
        .chunks_exact(2)
        .filter_map(|p| {
            let hi = (p[0] as char).to_digit(16)?;
            let lo = (p[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

/// Hex `LayoutJob` in *search* mode: bytes belonging to an occurrence of any
/// `(pattern, colour)` are coloured with that colour, everything else grey
/// ([`SEARCH_MISS`]). Later patterns win on overlap. Tail only; overlapping
/// matches of a pattern are all highlighted.
pub fn hex_search_job(
    bytes: &[u8],
    fontsize: f32,
    patterns: &[(&[u8], egui::Color32)],
    row_bytes: usize,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let row_bytes = row_bytes.max(1);
    let font = egui::FontId::monospace(fontsize);
    let mut job = LayoutJob::default();
    let n = bytes.len();
    let start = n.saturating_sub(4 * 1024);
    let tail = &bytes[start..];
    let m = tail.len();

    let mut colors = vec![SEARCH_MISS; m];
    for (pattern, col) in patterns {
        let plen = pattern.len();
        if plen == 0 || plen > m {
            continue;
        }
        let mut i = 0;
        while i + plen <= m {
            if &tail[i..i + plen] == *pattern {
                for c in colors.iter_mut().skip(i).take(plen) {
                    *c = *col;
                }
            }
            i += 1;
        }
    }

    for (i, &b) in tail.iter().enumerate() {
        job.append(
            &format!("{b:02X} "),
            0.0,
            TextFormat::simple(font.clone(), colors[i]),
        );
        if (start + i + 1) % row_bytes == 0 {
            job.append("\n", 0.0, TextFormat::simple(font.clone(), SEARCH_MISS));
        }
    }
    job
}

/// Text-view `LayoutJob`: whole lines that START (case-insensitively, after
/// leading whitespace) with `needle` are painted [`SEARCH_HIT`] yellow; every
/// other line keeps `default_color`. Used by Find-1 in the plain-text serial
/// view. `needle` is assumed non-empty (the caller renders plain text when it
/// is empty).
pub fn text_search_job(
    text: &str,
    needle: &str,
    fontsize: f32,
    default_color: egui::Color32,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::FontId::monospace(fontsize);
    let needle_lc = needle.to_lowercase();
    let mut job = LayoutJob::default();
    // `split_inclusive` keeps each line's trailing '\n', so the rendered text
    // reproduces the input exactly.
    for line in text.split_inclusive('\n') {
        let hit = line.trim_start().to_lowercase().starts_with(&needle_lc);
        let color = if hit { SEARCH_HIT } else { default_color };
        job.append(line, 0.0, TextFormat::simple(font.clone(), color));
    }
    job
}

#[cfg(test)]
mod tests {
    use super::{byte_color, render_rx_text, seq_color, seq_counts, text_search_job, SEARCH_HIT};
    use eframe::egui;

    /// Collect (line_text, is_yellow) from a text-search job.
    fn rows(text: &str, needle: &str) -> Vec<(String, bool)> {
        let job = text_search_job(text, needle, 12.0, egui::Color32::GRAY);
        job.sections
            .iter()
            .map(|s| {
                (
                    job.text[s.byte_range.clone()].to_string(),
                    s.format.color == SEARCH_HIT,
                )
            })
            .collect()
    }

    #[test]
    fn text_search_highlights_matching_line_prefixes() {
        let out = rows("INFO ready\nERR bad\ninfo again\n", "info");
        // Case-insensitive prefix match on lines 1 and 3; line 2 stays default.
        assert_eq!(out[0], ("INFO ready\n".to_owned(), true));
        assert_eq!(out[1], ("ERR bad\n".to_owned(), false));
        assert_eq!(out[2], ("info again\n".to_owned(), true));
    }

    #[test]
    fn text_search_ignores_leading_whitespace_and_needs_a_prefix() {
        // Leading spaces don't defeat the prefix match…
        assert!(rows("   DATA=5\n", "DATA")[0].1);
        // …but a match in the MIDDLE of the line does not highlight it.
        assert!(!rows("x DATA y\n", "DATA")[0].1);
        // Text reproduced exactly (concatenated sections == input).
        let job = text_search_job("a\nb\n", "a", 12.0, egui::Color32::GRAY);
        assert_eq!(job.text, "a\nb\n");
    }


    #[test]
    fn text_mode_is_lossy_utf8() {
        assert_eq!(render_rx_text(b"hi\n"), "hi\n");
        // An invalid byte becomes the replacement char, not a panic.
        assert!(render_rx_text(&[0xff, b'a']).ends_with('a'));
    }

    #[test]
    fn colors_are_deterministic_and_distinct() {
        // Same byte / sequence → same colour; different → different.
        assert_eq!(byte_color(0x4E), byte_color(0x4E));
        assert_ne!(byte_color(0x4E), byte_color(0x4F));
        assert_eq!(seq_color(b"ON"), seq_color(b"ON"));
        assert_ne!(seq_color(b"ON"), seq_color(b"NO"));
        // Single-byte sequence matches byte_color.
        assert_eq!(seq_color(&[0x4E]), byte_color(0x4E));
    }

    #[test]
    fn parse_hex_search_groups_pairs() {
        use super::parse_hex_search;
        assert_eq!(parse_hex_search("0D 0A"), vec![0x0D, 0x0A]);
        assert_eq!(parse_hex_search("4f4e"), vec![0x4F, 0x4E]);
        assert_eq!(parse_hex_search("  "), Vec::<u8>::new());
        assert_eq!(parse_hex_search("4"), Vec::<u8>::new()); // lone digit dropped
    }

    #[test]
    fn hex_search_colors_two_patterns_rest_grey() {
        use super::{SEARCH_HIT, SEARCH_HIT2, SEARCH_MISS, hex_search_job};
        // 01 0D 0A 02: pattern A = 0D 0A (yellow), pattern B = 01 (blue).
        let job = hex_search_job(
            &[0x01, 0x0D, 0x0A, 0x02],
            12.0,
            &[(&[0x0D, 0x0A], SEARCH_HIT), (&[0x01], SEARCH_HIT2)],
            16,
        );
        let colors: Vec<_> = job
            .sections
            .iter()
            .map(|s| {
                (
                    job.text[s.byte_range.clone()].trim().to_string(),
                    s.format.color,
                )
            })
            .filter(|(t, _)| !t.is_empty())
            .collect();
        assert_eq!(colors[0], ("01".into(), SEARCH_HIT2)); // blue
        assert_eq!(colors[1], ("0D".into(), SEARCH_HIT)); // yellow
        assert_eq!(colors[2], ("0A".into(), SEARCH_HIT)); // yellow
        assert_eq!(colors[3], ("02".into(), SEARCH_MISS)); // grey
    }

    #[test]
    fn seq_counts_groups_by_length_sorted_by_frequency() {
        // Single bytes (len 1): 'A'×3, 'B'×1.
        assert_eq!(
            seq_counts(b"AABA", 1),
            vec![(vec![b'A'], 3), (vec![b'B'], 1)]
        );
        // 2-byte sequences aligned from 0: "ON" "ON" "XY" → "ON"×2 first.
        assert_eq!(
            seq_counts(b"ONONXY", 2),
            vec![(b"ON".to_vec(), 2), (b"XY".to_vec(), 1)]
        );
        // Trailing partial chunk is ignored ("ON" + leftover 'X').
        assert_eq!(seq_counts(b"ONX", 2), vec![(b"ON".to_vec(), 1)]);
    }
}
