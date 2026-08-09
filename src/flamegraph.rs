//! On-target statistical profiler → flamegraph (the Profile tab's "Runtime"
//! mode).
//!
//! There is no `perf` on an MCU and probe-rs ships no profiler command, so we
//! **halt-sample the call stack**: with the firmware already flashed + running,
//! open a `probe-rs dap-server`, attach, then repeatedly `pause → stackTrace →
//! continue`. Each stackTrace (function names, resolved by probe-rs from the
//! ELF's DWARF) is one sample; folding them builds a flame tree.
//!
//! CAVEAT: halt-sampling is INTRUSIVE — every pause stops the CPU for ~ms, so
//! this is a STATISTICAL view (where time is spent), not cycle-accurate, and it
//! perturbs timing. A true non-intrusive profiler would need SWO/ITM or DWT
//! PC-sampling (chip- + probe-specific). Not hardware-tested in this repo.

use crate::build::no_window;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── Flame tree (pure, testable) ───────────────────────────────────────────────

/// One node of the flamegraph: a function frame + how many samples passed
/// through it, and the callees below it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlameNode {
    pub name: String,
    pub count: usize,
    pub children: Vec<FlameNode>,
}

impl FlameNode {
    fn child_mut(&mut self, name: &str) -> &mut FlameNode {
        if let Some(i) = self.children.iter().position(|c| c.name == name) {
            &mut self.children[i]
        } else {
            self.children.push(FlameNode {
                name: name.to_owned(),
                count: 0,
                children: Vec::new(),
            });
            self.children.last_mut().unwrap()
        }
    }
}

/// Fold samples into a flame tree. Each sample is a call stack **outermost
/// first** (`["main", "loop", "read"]`), so the root's direct children are the
/// entry points. The synthetic root is named "all"; its count = sample total.
/// Children are sorted by descending count so the hot path is leftmost.
pub fn build_tree(samples: &[Vec<String>]) -> FlameNode {
    let mut root = FlameNode {
        name: "all".to_owned(),
        count: 0,
        children: Vec::new(),
    };
    for stack in samples {
        root.count += 1;
        let mut node = &mut root;
        for frame in stack {
            node = node.child_mut(frame);
            node.count += 1;
        }
    }
    sort_desc(&mut root);
    root
}

fn sort_desc(node: &mut FlameNode) {
    node.children
        .sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    for c in &mut node.children {
        sort_desc(c);
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FlameResult {
    pub root: FlameNode,
    pub samples: usize,
}

#[derive(Clone, Debug, Default)]
pub enum FlameState {
    #[default]
    Idle,
    /// `cargo build --release` (to get the ELF + symbols).
    Building,
    /// Sampling in progress: `(collected, target)`.
    Sampling(usize, usize),
    Done(FlameResult),
    Failed(String),
}

impl FlameState {
    pub fn is_busy(&self) -> bool {
        matches!(self, FlameState::Building | FlameState::Sampling(..))
    }
}

// ── Runner ────────────────────────────────────────────────────────────────────

/// Build `--release`, attach via `probe-rs dap-server`, and collect `n_samples`
/// stack samples on a background thread. The firmware must already be running on
/// the target (attach, no flash). Result / progress land in `state`.
pub fn start_flame(
    project_dir: PathBuf,
    target: String,
    chip: String,
    probe: Option<String>,
    n_samples: usize,
    state: Arc<Mutex<FlameState>>,
    ctx: eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = FlameState::Building;
    ctx.request_repaint();
    thread::spawn(move || {
        let next = run(
            &project_dir,
            &target,
            &chip,
            probe.as_deref(),
            n_samples,
            &state,
            &ctx,
        );
        if let Err(e) = &next {
            *state.lock().unwrap() = FlameState::Failed(e.clone());
        } else if let Ok(res) = next {
            *state.lock().unwrap() = FlameState::Done(res);
        }
        ctx.request_repaint();
    });
}

fn run(
    project_dir: &std::path::Path,
    target: &str,
    chip: &str,
    probe: Option<&str>,
    n_samples: usize,
    state: &Arc<Mutex<FlameState>>,
    ctx: &eframe::egui::Context,
) -> Result<FlameResult, String> {
    let elf = build_elf(project_dir, target)?;

    // Spawn the DAP server.
    let port = free_port();
    let mut server = no_window(&mut Command::new("probe-rs"))
        .current_dir(project_dir)
        .args(["dap-server", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Capture stderr: probe-rs logs the REAL reason an attach fails here
        // (no probe, probe busy, unknown chip, target not responding …).
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "probe-rs not found in PATH (cargo install probe-rs-tools)".to_string()
            } else {
                format!("could not launch probe-rs dap-server: {e}")
            }
        })?;
    // Drain the dap-server's stderr on a background thread (capped) so a chatty
    // server can never fill the pipe buffer and block the sampling loop; the tail
    // is surfaced if the run fails.
    let log = Arc::new(Mutex::new(String::new()));
    let drain = server.stderr.take().map(|mut se| {
        let log = log.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = se.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut l = log.lock().unwrap();
                l.push_str(&String::from_utf8_lossy(&buf[..n]));
                if l.len() > 8192 {
                    let cut = l.len() - 8192;
                    *l = l.split_off(cut);
                }
            }
        })
    });
    // Kill the server whatever happens next.
    let result = sample_over_dap(port, &elf, chip, probe, n_samples, state, ctx);
    let _ = server.kill();
    let _ = server.wait();
    if let Some(h) = drain {
        let _ = h.join(); // kill → stderr EOF → drain exits; ensures full capture
    }
    // On failure, enrich the terse DAP error with the dap-server's own log tail
    // (probe-rs logs the real reason there).
    result.map_err(|e| {
        let log = log.lock().unwrap();
        let tail: Vec<&str> = log
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .rev()
            .take(4)
            .collect();
        if tail.is_empty() {
            e
        } else {
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            format!("{e}\n\nprobe-rs dap-server:\n{tail}")
        }
    })
}

/// `cargo build --release --message-format=json`, return the ELF path.
fn build_elf(project_dir: &std::path::Path, target: &str) -> Result<PathBuf, String> {
    let out = no_window(&mut Command::new("cargo"))
        .current_dir(project_dir)
        .args([
            "build",
            "--release",
            "--target",
            target,
            "--message-format=json",
            "--color=never",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("could not launch cargo: {e}"))?;
    let mut elf: Option<PathBuf> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["reason"] == "compiler-artifact" {
            if let Some(exe) = v["executable"].as_str() {
                elf = Some(PathBuf::from(exe));
            }
        }
    }
    if !out.status.success() {
        return Err(format!(
            "release build failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    elf.ok_or_else(|| "build produced no executable (nothing to profile)".to_string())
}

/// Connect to the DAP server, attach, and run the pause/stackTrace/continue
/// sampling loop.
fn sample_over_dap(
    port: u16,
    elf: &std::path::Path,
    chip: &str,
    probe: Option<&str>,
    n_samples: usize,
    state: &Arc<Mutex<FlameState>>,
    ctx: &eframe::egui::Context,
) -> Result<FlameResult, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = None;
    for _ in 0..50 {
        if let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            stream = Some(s);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let mut stream = stream.ok_or("could not connect to probe-rs dap-server")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(6)))
        .map_err(|e| e.to_string())?;
    let mut seq = 1i64;

    // Handshake: initialize → launch (ATTACH: no flash) → configurationDone.
    send(
        &mut stream,
        &mut seq,
        "initialize",
        json!({"adapterID": "probe-rs"}),
    )?;
    wait_response(&mut stream, "initialize")?;
    let mut launch = json!({
        "chip": chip,
        "connectUnderReset": false,
        "flashingConfig": { "flashingEnabled": false },
        "coreConfigs": [{ "coreIndex": 0, "programBinary": elf.to_string_lossy() }],
    });
    if let Some(p) = probe.filter(|s| !s.is_empty()) {
        launch["probe"] = json!(p);
    }
    send(&mut stream, &mut seq, "attach", launch)?;
    // The `initialized` event tells us to finish configuration.
    wait_event(&mut stream, "initialized")?;
    send(&mut stream, &mut seq, "configurationDone", json!({}))?;

    *state.lock().unwrap() = FlameState::Sampling(0, n_samples);
    ctx.request_repaint();

    let mut samples: Vec<Vec<String>> = Vec::with_capacity(n_samples);
    let mut thread_id = 0i64;
    for i in 0..n_samples {
        // Give the target a moment to run between samples (spreads the samples,
        // reduces "always halted in the same spot" bias from back-to-back halts).
        thread::sleep(Duration::from_millis(8));
        send(
            &mut stream,
            &mut seq,
            "pause",
            json!({"threadId": thread_id.max(1)}),
        )?;
        let stopped = wait_event(&mut stream, "stopped")?;
        if let Some(t) = stopped["body"]["threadId"].as_i64() {
            thread_id = t;
        }
        send(
            &mut stream,
            &mut seq,
            "stackTrace",
            json!({"threadId": thread_id.max(1), "startFrame": 0, "levels": 32}),
        )?;
        let st = wait_response(&mut stream, "stackTrace")?;
        if let Some(stack) = frames_of(&st) {
            if !stack.is_empty() {
                samples.push(stack);
            }
        }
        send(
            &mut stream,
            &mut seq,
            "continue",
            json!({"threadId": thread_id.max(1)}),
        )?;

        if i % 4 == 0 {
            *state.lock().unwrap() = FlameState::Sampling(samples.len(), n_samples);
            ctx.request_repaint();
        }
    }
    let _ = send(&mut stream, &mut seq, "disconnect", json!({}));

    if samples.is_empty() {
        return Err(
            "no stack samples — is the firmware flashed and running, and the ELF matching?"
                .to_string(),
        );
    }
    let taken = samples.len();
    Ok(FlameResult {
        root: build_tree(&samples),
        samples: taken,
    })
}

/// Frame names from a `stackTrace` response, OUTERMOST first (reversed from the
/// DAP order, which is innermost first), skipping unnamed frames.
fn frames_of(resp: &Value) -> Option<Vec<String>> {
    let arr = resp["body"]["stackFrames"].as_array()?;
    let mut names: Vec<String> = arr
        .iter()
        .filter_map(|f| f["name"].as_str())
        .filter(|n| !n.is_empty() && *n != "<unknown>")
        .map(str::to_owned)
        .collect();
    names.reverse();
    Some(names)
}

// ── Minimal blocking DAP client ───────────────────────────────────────────────

fn send(
    stream: &mut TcpStream,
    seq: &mut i64,
    command: &str,
    arguments: Value,
) -> Result<(), String> {
    let msg = json!({"seq": *seq, "type": "request", "command": command, "arguments": arguments});
    *seq += 1;
    let body = msg.to_string();
    // ONE `write_all` for header + body, never `write!` with a format string:
    // that issues a syscall per piece, so the header can reach the server split
    // across TCP segments. probe-rs's dap-server (0.29) parses its two header
    // lines with `read_line` on a NON-blocking socket, and a partial line
    // desyncs that reader for good — it then drops the connection, which
    // arrives here as `dap-server closed waiting for 'initialized'`. Same fix
    // as `debugger.rs`'s `Wire::request`.
    let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
    stream
        .write_all(frame.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

/// Read one `Content-Length`-framed DAP message (`None` on EOF / timeout).
fn read_msg(stream: &mut TcpStream) -> Option<Value> {
    let mut header = Vec::new();
    let mut b = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        match stream.read(&mut b) {
            Ok(1) => header.push(b[0]),
            _ => return None,
        }
        if header.len() > 4096 {
            return None;
        }
    }
    let len: usize = String::from_utf8_lossy(&header)
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))
        .and_then(|v| v.trim().parse().ok())?;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Read messages until a successful RESPONSE to `command` arrives.
fn wait_response(stream: &mut TcpStream, command: &str) -> Result<Value, String> {
    for _ in 0..200 {
        let msg =
            read_msg(stream).ok_or_else(|| format!("dap-server closed waiting for {command}"))?;
        if msg["type"] == "response" && msg["command"] == command {
            if msg["success"].as_bool() == Some(false) {
                let m = msg["message"].as_str().unwrap_or("request failed");
                return Err(format!("{command}: {m}"));
            }
            return Ok(msg);
        }
    }
    Err(format!("no response to {command}"))
}

/// Read messages until EVENT `event` arrives. A FAILED response to a pending
/// request (e.g. `attach` when the probe/target isn't there) is surfaced with
/// its message rather than skipped — otherwise the server just closes and the
/// real reason is lost as a bare "closed waiting for 'initialized'".
fn wait_event(stream: &mut TcpStream, event: &str) -> Result<Value, String> {
    for _ in 0..200 {
        let msg =
            read_msg(stream).ok_or_else(|| format!("dap-server closed waiting for '{event}'"))?;
        if msg["type"] == "event" && msg["event"] == event {
            return Ok(msg);
        }
        if msg["type"] == "response" && msg["success"].as_bool() == Some(false) {
            let cmd = msg["command"].as_str().unwrap_or("request");
            let m = msg["message"]
                .as_str()
                .or_else(|| msg["body"]["error"]["format"].as_str())
                .unwrap_or("request failed");
            return Err(format!("{cmd} failed: {m}"));
        }
    }
    Err(format!("no '{event}' event"))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(4712)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_stacks_into_a_tree_hot_path_first() {
        // main→loop→work (x3), main→loop→idle (x1), main→setup (x1).
        let samples = vec![
            vec!["main".into(), "loop".into(), "work".into()],
            vec!["main".into(), "loop".into(), "work".into()],
            vec!["main".into(), "loop".into(), "work".into()],
            vec!["main".into(), "loop".into(), "idle".into()],
            vec!["main".into(), "setup".into()],
        ];
        let root = build_tree(&samples);
        assert_eq!(root.name, "all");
        assert_eq!(root.count, 5);
        // one entry point: main, with all 5.
        assert_eq!(root.children.len(), 1);
        let main = &root.children[0];
        assert_eq!((main.name.as_str(), main.count), ("main", 5));
        // main's children sorted by count: loop (4) before setup (1).
        assert_eq!(main.children[0].name, "loop");
        assert_eq!(main.children[0].count, 4);
        assert_eq!(main.children[1].name, "setup");
        // loop→work is the hot leaf (3), before idle (1).
        let lp = &main.children[0];
        assert_eq!(
            (lp.children[0].name.as_str(), lp.children[0].count),
            ("work", 3)
        );
        assert_eq!(lp.children[1].name, "idle");
    }

    #[test]
    fn empty_input_is_an_empty_root() {
        let root = build_tree(&[]);
        assert_eq!(root.count, 0);
        assert!(root.children.is_empty());
    }
}
