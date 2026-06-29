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
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Cap on the retained RX byte buffer (older bytes are dropped).
const RX_CAP: usize = 64_000;

/// Shared between the background reader thread and the UI.
#[derive(Default)]
pub struct SerialState {
    /// Raw received bytes (capped to the last [`RX_CAP`]).
    pub rx: Vec<u8>,
    /// Set by the UI to ask the reader thread to exit.
    pub stop: bool,
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
    /// Selected port name (e.g. `COM4` / `/dev/ttyUSB0`).
    pub port: String,
    /// Selected baud rate.
    pub baud: u32,
    /// Display mode: hex bytes vs. decoded text.
    pub hex: bool,
    /// Number of bytes per repeating sequence to colour / list (hex mode).
    pub seq_len: usize,
    /// Width (px) of the unique-sequences legend (draggable divider).
    pub legend_w: f32,
    /// Keep the RX view pinned to the newest data.
    pub autoscroll: bool,
    /// Append `\r\n` to each sent line.
    pub append_crlf: bool,
    /// The TX input line.
    pub tx_input: String,
    /// Height (px) of the resizable send area (dragged via its handle).
    pub tx_height: f32,
    /// Cached list of available ports (refreshed on demand).
    pub ports: Vec<String>,
}

impl Default for SerialMonitor {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(SerialState::default())),
            writer: None,
            port: String::new(),
            baud: 115_200,
            hex: false,
            seq_len: 1,
            legend_w: 160.0,
            autoscroll: true,
            append_crlf: true,
            tx_input: String::new(),
            tx_height: 30.0,
            ports: Vec::new(),
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
        {
            let mut s = self.state.lock().unwrap();
            s.stop = false;
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
                    spawn_reader(reader, Arc::clone(&self.state), ctx.clone());
                }
                Err(e) => self.state.lock().unwrap().error = Some(e.to_string()),
            },
            Err(e) => self.state.lock().unwrap().error = Some(e.to_string()),
        }
    }

    /// Stop the reader thread and release the port.
    pub fn disconnect(&mut self) {
        let mut s = self.state.lock().unwrap();
        s.stop = true;
        s.connected = false;
        drop(s);
        self.writer = None;
    }

    /// Send raw bytes on the open port (no-op when disconnected).
    pub fn send(&mut self, bytes: &[u8]) {
        if let Some(w) = self.writer.as_mut() {
            if let Err(e) = w.write_all(bytes) {
                self.state.lock().unwrap().error = Some(e.to_string());
            }
        }
    }

    pub fn clear_rx(&mut self) {
        self.state.lock().unwrap().rx.clear();
    }
}

/// Background loop: read bytes into the shared buffer until asked to stop or the
/// port errors. The short read timeout lets it notice `stop` promptly.
fn spawn_reader(
    mut port: Box<dyn serialport::SerialPort>,
    state: Arc<Mutex<SerialState>>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            if state.lock().unwrap().stop {
                break;
            }
            match port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    let mut s = state.lock().unwrap();
                    s.rx.extend_from_slice(&buf[..n]);
                    if s.rx.len() > RX_CAP {
                        let excess = s.rx.len() - RX_CAP;
                        s.rx.drain(..excess);
                    }
                    drop(s);
                    ctx.request_repaint();
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
pub fn hex_layout_job(bytes: &[u8], fontsize: f32, seq_len: usize) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let seq_len = seq_len.max(1);
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
        if (i + 1) % 16 == 0 {
            job.append("\n", 0.0, TextFormat::simple(font.clone(), egui::Color32::GRAY));
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::{byte_color, render_rx_text, seq_color, seq_counts};

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
