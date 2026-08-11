//! The virtual serial pair behind the Serial tab's **Bridge (MITM)** mode.
//!
//! # The problem
//!
//! A serial port is OS-exclusive: while a vendor config tool holds `COM4`, the
//! IDE cannot also open it to watch the traffic. The way around that is to stop
//! sharing the port and start relaying it — the app talks to one end of a
//! *virtual* pair, the IDE holds the other end and forwards every byte to the
//! real device, logging both directions:
//!
//! ```text
//!   config app  ⇄  [ virtual pair: app-side ⇄ ide-side ]  ⇄  IDE  ⇄  device
//! ```
//!
//! # The two providers are NOT equivalent
//!
//! - **Unix (`socat`)**: the IDE creates the pair itself, on demand, and the two
//!   ends are plain paths it chooses. Nothing to install beyond `socat`, nothing
//!   to configure.
//! - **Windows (`com0com`)**: pairs are a driver-level, persistent resource the
//!   user creates once in com0com's own setup tool. The IDE can only ask which
//!   existing port to use.
//!
//! That asymmetry is inherent, so it is modelled rather than hidden: the UI asks
//! for different things on each platform.

use std::path::PathBuf;
use std::process::Child;
// Both platforms spawn something — `socat` on unix, `reg` on Windows — but only
// the unix path redirects its pipes.
#[cfg(any(unix, windows))]
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;

/// How a virtual pair is obtained on this host.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PairProvider {
    /// Windows: a persistent pair created by the user in com0com's setup.
    Com0com,
    /// Unix: a pair of PTYs this process spawns and owns.
    Socat,
}

pub fn provider() -> PairProvider {
    if cfg!(windows) {
        PairProvider::Com0com
    } else {
        PairProvider::Socat
    }
}

/// Name of the external tool the provider needs, for the Tools-tab entry and
/// error messages.
pub fn provider_tool() -> &'static str {
    match provider() {
        PairProvider::Com0com => "com0com",
        PairProvider::Socat => "socat",
    }
}

/// The two ends of a live virtual pair.
///
/// Dropping it tears the pair down on Unix (the `socat` child is killed). On
/// Windows there is nothing to drop: the pair outlives the IDE by design.
pub struct VirtualPair {
    /// What the OTHER application must open.
    pub app_side: String,
    /// What the IDE opens.
    pub ide_side: String,
    /// The `socat` process, when we created the pair.
    child: Option<Child>,
}

impl VirtualPair {
    /// A pair the user already created (com0com): the IDE opens `ide_side`, and
    /// the app is expected on its mate, which only the user knows.
    pub fn existing(ide_side: String, app_side: String) -> Self {
        Self {
            app_side,
            ide_side,
            child: None,
        }
    }

    /// Is this pair still alive? A `socat` that died takes the bridge with it.
    pub fn is_alive(&mut self) -> bool {
        match self.child.as_mut() {
            None => true, // com0com: a driver pair, not a process
            Some(c) => matches!(c.try_wait(), Ok(None)),
        }
    }
}

impl Drop for VirtualPair {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            // The symlinks are socat's; it removes them on exit, but a killed
            // process may not get the chance.
            let _ = std::fs::remove_file(&self.app_side);
            let _ = std::fs::remove_file(&self.ide_side);
        }
    }
}

/// Where the PTY symlinks are placed. Temp, not the config dir: they are
/// transient device nodes, meaningless after the process that made them exits.
#[cfg_attr(not(unix), allow(dead_code))]
fn link_paths() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir();
    // Slot-suffixed: two IDE windows each running a bridge would otherwise
    // fight over the same two device nodes.
    let sfx = crate::workspace::suffix();
    (
        base.join(format!("embedded_ide_bridge_app{sfx}")),
        base.join(format!("embedded_ide_bridge_ide{sfx}")),
    )
}

/// The `socat` invocation for a raw PTY pair. Split out so the argument shape is
/// unit-testable without spawning anything.
///
/// `raw` and `echo=0` are both required: a cooked PTY would line-buffer, mangle
/// control bytes and echo everything straight back into the stream — which on a
/// binary protocol looks like the device answering itself.
pub fn socat_args(app_link: &str, ide_link: &str) -> Vec<String> {
    vec![
        // Two -d's make socat report the PTY names it allocated; harmless here
        // (we use the symlinks) but it makes its stderr worth reading on failure.
        "-d".to_string(),
        "-d".to_string(),
        format!("pty,raw,echo=0,link={app_link}"),
        format!("pty,raw,echo=0,link={ide_link}"),
    ]
}

/// Create a fresh pair with `socat` and wait for both ends to appear.
///
/// The wait is not optional: socat allocates the PTYs and creates the symlinks
/// asynchronously after `spawn` returns, so opening them immediately races and
/// fails with "No such file or directory" perhaps one time in three.
#[cfg(unix)]
pub fn create_socat_pair() -> Result<VirtualPair, String> {
    let (app, ide) = link_paths();
    // A leftover link from a killed run would make socat fail with EADDRINUSE.
    let _ = std::fs::remove_file(&app);
    let _ = std::fs::remove_file(&ide);

    let (app_s, ide_s) = (
        app.to_string_lossy().to_string(),
        ide.to_string_lossy().to_string(),
    );
    let mut child = Command::new("socat")
        .args(socat_args(&app_s, &ide_s))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start socat: {e}. Install it and try again."))?;

    // Poll for the links; ~2 s is far longer than socat needs and still short
    // enough that a genuine failure is reported promptly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if app.exists() && ide.exists() {
            return Ok(VirtualPair {
                app_side: app_s,
                ide_side: ide_s,
                child: Some(child),
            });
        }
        // socat exiting early means it failed — read why instead of waiting out
        // the whole deadline for links that will never appear.
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            return Err(format!(
                "socat exited immediately ({status}): {}",
                err.trim()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let _ = child.kill();
    Err("socat did not create the PTY pair within 2 s".to_string())
}

#[cfg(not(unix))]
pub fn create_socat_pair() -> Result<VirtualPair, String> {
    Err(
        "socat pairs are a Unix feature; on Windows create a com0com pair \
         and pick its ports."
            .to_string(),
    )
}

// ── com0com pair discovery (Windows) ─────────────────────────────────────────
// A com0com pair is `CNCA<n>` ⇄ `CNCB<n>`, configured in the driver's registry.
// Asking the user to remember which two COM numbers are mates is exactly the
// kind of thing the IDE can just look up.
//
// The catch found on a real install: `PortName` in the driver's Parameters key
// is often the literal **`COM#`**, com0com's "assign the next free number"
// placeholder — the concrete name only exists once Windows has enumerated the
// device, and it lives under `Enum\com0com\…\Device Parameters\PortName`. So a
// pair in Parameters is a pair that was CONFIGURED, not one that is USABLE, and
// both keys have to be read and then cross-checked against the ports the OS
// actually offers.

/// One side of a pair: `CNCA0` / `CNCB1` …
fn side_id(key_line: &str) -> Option<(char, u32)> {
    let up = key_line.to_ascii_uppercase();
    let pos = up.rfind("CNC")?;
    let rest = &up[pos + 3..];
    let ab = rest.chars().next()?;
    if ab != 'A' && ab != 'B' {
        return None;
    }
    let digits: String = rest[1..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok().map(|n| (ab, n))
}

/// Map every `CNCA<n>` / `CNCB<n>` mentioned in `reg query … /s` output to its
/// `PortName`. Pure, and shape-compatible with BOTH registry locations, so the
/// Parameters and Enum outputs can be parsed by the same code and merged.
///
/// Returns `(side, index) -> port name`.
pub fn parse_port_names(reg_output: &str) -> std::collections::HashMap<(char, u32), String> {
    let mut out = std::collections::HashMap::new();
    let mut current: Option<(char, u32)> = None;
    for line in reg_output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Key lines start at column 0; value lines are indented.
        if !line.starts_with([' ', '\t']) {
            current = side_id(t);
            continue;
        }
        let Some(id) = current else { continue };
        // `    PortName    REG_SZ    COM10`
        let mut parts = t.split_whitespace();
        if parts.next() != Some("PortName") {
            continue;
        }
        let _ty = parts.next();
        if let Some(name) = parts.next() {
            out.insert(id, name.to_string());
        }
    }
    out
}

/// Pair up resolved side names into `(A, B)` per index, keeping only pairs where
/// BOTH sides are in `live` — a name that no longer answers is worse than none,
/// because the user would pick it and get "port not found" on Connect.
pub fn usable_pairs(
    names: &std::collections::HashMap<(char, u32), String>,
    live: &[String],
) -> Vec<(String, String)> {
    let is_live = |n: &String| live.iter().any(|l| l.eq_ignore_ascii_case(n));
    let mut idx: Vec<u32> = names.keys().map(|(_, n)| *n).collect();
    idx.sort_unstable();
    idx.dedup();
    idx.into_iter()
        .filter_map(|n| {
            let a = names.get(&('A', n))?;
            let b = names.get(&('B', n))?;
            (is_live(a) && is_live(b)).then(|| (a.clone(), b.clone()))
        })
        .collect()
}

/// Every com0com pair whose BOTH ends are live ports, as `(A, B)`.
///
/// Enum wins over Parameters: it holds the name Windows actually assigned, which
/// is the only truth when Parameters says `COM#`.
#[cfg(windows)]
pub fn com0com_pairs(live: &[String]) -> Vec<(String, String)> {
    let query = |key: &str| -> String {
        crate::build::no_window(&mut Command::new("reg"))
            .args(["query", key, "/s"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let mut names = parse_port_names(&query(
        r"HKLM\SYSTEM\CurrentControlSet\Services\com0com\Parameters",
    ));
    // `COM#` is a placeholder, not a port — drop it so Enum can fill the gap.
    names.retain(|_, v| !v.contains('#'));
    for (k, v) in parse_port_names(&query(r"HKLM\SYSTEM\CurrentControlSet\Enum\com0com")) {
        names.insert(k, v);
    }
    usable_pairs(&names, live)
}

#[cfg(not(windows))]
pub fn com0com_pairs(_live: &[String]) -> Vec<(String, String)> {
    Vec::new()
}

/// One-line instruction for the user, per provider. Shown next to the Bridge
/// controls, because "what do I point my other app at?" is the only genuinely
/// confusing part of setting this up.
pub fn setup_hint(pair: Option<&VirtualPair>) -> String {
    match (provider(), pair) {
        (PairProvider::Socat, Some(p)) => format!(
            "Point your other application at  {}  (the IDE holds {}).",
            p.app_side, p.ide_side
        ),
        (PairProvider::Socat, None) => {
            "Press 'Create pair' - socat will make two linked PTYs; the IDE takes \
             one and your other application opens the other."
                .to_string()
        }
        // With the pair detected there is nothing left to explain — just name
        // the port the other application needs.
        (PairProvider::Com0com, Some(p)) => format!(
            "Point your other application at  {}  (the IDE holds {}).",
            p.app_side, p.ide_side
        ),
        (PairProvider::Com0com, None) => {
            "No com0com pair with two live ports was found. Create one in \
             com0com's setup (e.g. COM10 <-> COM11) - a pair configured with the \
             `COM#` placeholder does not count until Windows assigns it a number."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_matches_the_host() {
        if cfg!(windows) {
            assert_eq!(provider(), PairProvider::Com0com);
            assert_eq!(provider_tool(), "com0com");
        } else {
            assert_eq!(provider(), PairProvider::Socat);
            assert_eq!(provider_tool(), "socat");
        }
    }

    /// `raw` and `echo=0` on BOTH ends are what make the pair a transparent
    /// pipe. Losing either turns the bridge into a line-buffered echo chamber,
    /// which on a binary protocol reads as the device replying to itself.
    #[test]
    fn socat_args_request_raw_non_echoing_ptys() {
        let a = socat_args("/tmp/app", "/tmp/ide");
        let ends: Vec<&String> = a.iter().filter(|s| s.starts_with("pty,")).collect();
        assert_eq!(ends.len(), 2, "expected exactly two pty endpoints: {a:?}");
        for e in ends {
            assert!(e.contains("raw"), "{e}");
            assert!(e.contains("echo=0"), "{e}");
        }
        assert!(a.iter().any(|s| s == "pty,raw,echo=0,link=/tmp/app"));
        assert!(a.iter().any(|s| s == "pty,raw,echo=0,link=/tmp/ide"));
    }

    #[test]
    fn the_two_link_paths_differ() {
        let (app, ide) = link_paths();
        assert_ne!(app, ide, "both ends would collide on one node");
    }

    /// A com0com pair is not a child process, so it must never be reported dead.
    #[test]
    fn an_existing_pair_is_always_alive() {
        let mut p = VirtualPair::existing("COM11".into(), "COM10".into());
        assert!(p.is_alive());
        assert_eq!(p.ide_side, "COM11");
        assert_eq!(p.app_side, "COM10");
    }

    /// VERBATIM output of `reg query …\Services\com0com\Parameters /s` on this
    /// machine — a real com0com install whose pair uses the `COM#` placeholder.
    const REAL_PARAMETERS: &str = "\
HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Services\\com0com\\Parameters\\CNCA1
    PortName    REG_SZ    COM#

HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Services\\com0com\\Parameters\\CNCB1
    PortName    REG_SZ    COM#
";

    #[test]
    fn parses_the_real_registry_output() {
        let n = parse_port_names(REAL_PARAMETERS);
        assert_eq!(n.get(&('A', 1)).map(String::as_str), Some("COM#"));
        assert_eq!(n.get(&('B', 1)).map(String::as_str), Some("COM#"));
        assert_eq!(n.len(), 2);
    }

    /// The finding that shaped this code: a pair configured with `COM#` is not a
    /// usable pair. Reporting it would send the user to a port that does not
    /// exist — worse than reporting nothing.
    #[test]
    fn a_com_hash_placeholder_is_not_a_usable_pair() {
        let mut names = parse_port_names(REAL_PARAMETERS);
        names.retain(|_, v| !v.contains('#'));
        assert!(usable_pairs(&names, &["COM1".into(), "COM17".into()]).is_empty());
    }

    #[test]
    fn a_fully_assigned_pair_is_found_when_both_ends_are_live() {
        let reg = "\
HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Enum\\com0com\\port\\CNCA0\\Device Parameters
    PortName    REG_SZ    COM10

HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Enum\\com0com\\port\\CNCB0\\Device Parameters
    PortName    REG_SZ    COM11
";
        let names = parse_port_names(reg);
        let live = vec!["COM10".to_string(), "COM11".to_string(), "COM3".to_string()];
        assert_eq!(
            usable_pairs(&names, &live),
            vec![("COM10".to_string(), "COM11".to_string())]
        );
        // One end unplugged → the pair is unusable, not half-usable.
        assert!(usable_pairs(&names, &["COM10".to_string()]).is_empty());
        // Matching is case-insensitive: Windows is inconsistent about "com10".
        assert_eq!(
            usable_pairs(&names, &["com10".into(), "com11".into()]).len(),
            1
        );
    }

    #[test]
    fn a_half_configured_pair_is_ignored() {
        // Only the A side exists — nothing to bridge to.
        let names = parse_port_names("HKLM\\...\\CNCA2\n    PortName    REG_SZ    COM20\n");
        assert!(usable_pairs(&names, &["COM20".to_string()]).is_empty());
    }

    #[test]
    fn side_ids_are_read_off_the_key_line() {
        assert_eq!(side_id("HKLM\\x\\CNCA0"), Some(('A', 0)));
        assert_eq!(side_id("HKLM\\x\\CNCB12"), Some(('B', 12)));
        // MUST still resolve with a trailing subkey: the Enum location — the one
        // holding the actually-assigned port name — is
        // `…\Enum\com0com\port\CNCA0\Device Parameters`.
        assert_eq!(side_id("HKLM\\x\\CNCA0\\Device Parameters"), Some(('A', 0)));
        assert_eq!(side_id("HKLM\\Services\\com0com"), None);
        assert_eq!(side_id("HKLM\\x\\CNCC0"), None);
    }

    /// Opt-in diagnostic against the REAL machine, not an assertion:
    /// `cargo test -- --ignored --nocapture com0com_diagnostic`.
    ///
    /// Ignored by default because it depends on hardware/driver state, which a
    /// test suite must never do. It exists because every other test here runs on
    /// captured output — this is the one that proves the two `reg query` calls,
    /// the Enum/Parameters merge and the live-port filter agree on a pair that
    /// actually exists.
    #[test]
    #[ignore = "requires a real com0com pair; run with --ignored"]
    fn com0com_diagnostic() {
        let live: Vec<String> = serialport::available_ports()
            .map(|ps| ps.into_iter().map(|p| p.port_name).collect())
            .unwrap_or_default();
        println!("live ports      : {live:?}");
        let pairs = com0com_pairs(&live);
        println!("detected pairs  : {pairs:?}");
        for (a, b) in &pairs {
            let p = VirtualPair::existing(b.clone(), a.clone());
            println!("hint            : {}", setup_hint(Some(&p)));
        }
        assert!(
            !pairs.is_empty(),
            "no usable com0com pair found — create one with:\n  \
             setupc.exe install PortName=COM20 PortName=COM21"
        );
    }

    #[test]
    fn the_hint_names_the_port_the_other_app_needs() {
        let p = VirtualPair::existing("/tmp/ide".into(), "/tmp/app".into());
        if cfg!(unix) {
            let h = setup_hint(Some(&p));
            assert!(h.contains("/tmp/app"), "{h}");
        }
        // With no pair yet there is still something actionable to say.
        assert!(!setup_hint(None).trim().is_empty());
    }
}
