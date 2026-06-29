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
    /// Keep the RX view pinned to the newest data.
    pub autoscroll: bool,
    /// Append `\r\n` to each sent line.
    pub append_crlf: bool,
    /// The TX input line.
    pub tx_input: String,
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
            autoscroll: true,
            append_crlf: true,
            tx_input: String::new(),
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

/// Render the tail of `rx` for display: decoded text, or space-separated hex
/// (16 bytes per line). Only the last chunk is formatted to keep layout cheap.
pub fn render_rx(rx: &[u8], hex: bool) -> String {
    if hex {
        let start = rx.len().saturating_sub(8 * 1024);
        let mut out = String::with_capacity((rx.len() - start) * 3);
        for (i, b) in rx[start..].iter().enumerate() {
            out.push_str(&format!("{b:02X} "));
            if (i + 1) % 16 == 0 {
                out.push('\n');
            }
        }
        out
    } else {
        let start = rx.len().saturating_sub(16 * 1024);
        String::from_utf8_lossy(&rx[start..]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::render_rx;

    #[test]
    fn text_mode_is_lossy_utf8() {
        assert_eq!(render_rx(b"hi\n", false), "hi\n");
        // An invalid byte becomes the replacement char, not a panic.
        assert!(render_rx(&[0xff, b'a'], false).ends_with('a'));
    }

    #[test]
    fn hex_mode_formats_bytes() {
        assert_eq!(render_rx(&[0x00, 0x1F, 0xAB], true), "00 1F AB ");
    }
}
