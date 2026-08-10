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
// Only the unix pair-creation path spawns anything.
#[cfg(unix)]
use std::process::{Command, Stdio};

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
    (
        base.join("embedded_ide_bridge_app"),
        base.join("embedded_ide_bridge_ide"),
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

    let (app_s, ide_s) = (app.to_string_lossy().to_string(), ide.to_string_lossy().to_string());
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
    Err("socat pairs are a Unix feature; on Windows create a com0com pair \
         and pick its ports."
        .to_string())
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
            "Press “Create pair” — socat will make two linked PTYs; the IDE takes \
             one and your other application opens the other."
                .to_string()
        }
        (PairProvider::Com0com, _) => {
            "Create a pair in com0com's setup (e.g. COM10 ⇄ COM11), pick ONE of \
             them here, and point your other application at its mate."
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
