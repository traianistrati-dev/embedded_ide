//! Debug-probe enumeration via `probe-rs list`, shared by the RTT and Debug
//! tabs so the user can target a specific probe when several are connected.
//!
//! Both tabs drive `probe-rs`, which — given only `--chip` — auto-selects the
//! sole attached probe and turns ambiguous the moment a second one is plugged
//! in. Passing `--probe <VID:PID[:Serial]>` (RTT) or the DAP `probe` launch
//! field (Debug) pins the session to one probe. The selector strings come
//! straight from `probe-rs list`, so they are always in the exact form
//! probe-rs expects — no reconstruction from our own USB scan.

use crate::build::no_window;
use std::process::Command;

/// One probe as reported by `probe-rs list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeInfo {
    /// Human name, e.g. "STLink V2".
    pub name: String,
    /// probe-rs family tag, e.g. "ST-LINK", "EspJtag".
    pub kind: String,
    /// The exact `--probe` selector (`VID:PID` or `VID:PID:Serial`).
    pub selector: String,
}

impl ProbeInfo {
    /// One-line label for the ComboBox.
    pub fn combo_label(&self) -> String {
        format!("[{}] {}  ·  {}", self.kind, self.name, self.selector)
    }
}

/// Run `probe-rs list` and parse the connected probes. `Ok(vec![])` when none
/// are attached; `Err` only when the binary itself cannot be run.
pub fn list_probes() -> Result<Vec<ProbeInfo>, String> {
    let out = no_window(&mut Command::new("probe-rs"))
        .arg("list")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "probe-rs not found in PATH (cargo install probe-rs-tools)".to_string()
            } else {
                format!("could not run `probe-rs list`: {e}")
            }
        })?;
    // The probe rows go to stdout; scan stderr too in case a build ever changes
    // that — parsing only picks up the `[n]: … -- … (Kind)` shape either way.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(parse_list(&text))
}

/// Parse `probe-rs list` output. Recognised line shape (probe-rs 0.2x–0.31):
/// `[<idx>]: <name> -- <VID:PID[:Serial]> (<Kind>)`. The serial itself can hold
/// colons (ESP JTAG → `303a:1001:50:78:7D:62:33:A4`), so the selector is taken
/// verbatim between ` -- ` and the trailing ` (Kind)` rather than split on `:`.
pub fn parse_list(stdout: &str) -> Vec<ProbeInfo> {
    let mut probes = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        // Only the numbered "[0]: …" rows are probes (skip the header line).
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some(idx_end) = rest.find("]:") else {
            continue;
        };
        let body = rest[idx_end + 2..].trim(); // "STLink V2 -- 0483:3748: (ST-LINK)"

        // name -- tail, split on the first " -- ".
        let Some(dash) = body.find(" -- ") else {
            continue;
        };
        let name = body[..dash].trim().to_string();
        let tail = body[dash + 4..].trim(); // "0483:3748: (ST-LINK)"

        // Kind is the last parenthesised token; the selector is everything
        // before it (rfind so a probe name never confuses the split).
        let (selector, kind) = match tail.rfind('(') {
            Some(paren) => {
                let sel = tail[..paren].trim();
                let kind = tail[paren + 1..].trim_end().trim_end_matches(')').trim();
                (sel, kind.to_string())
            }
            None => (tail, String::new()),
        };

        // Drop a trailing empty-serial colon ("0483:3748:" → "0483:3748") while
        // leaving the colon-bearing ESP serials intact.
        let selector = selector.trim().trim_end_matches(':').to_string();
        if selector.is_empty() {
            continue;
        }
        probes.push(ProbeInfo {
            name,
            kind,
            selector,
        });
    }
    probes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stlink_and_esp_jtag() {
        let out = "The following debug probes were found:\n\
                   [0]: STLink V2 -- 0483:3748: (ST-LINK)\n\
                   [1]: ESP JTAG -- 303a:1001:50:78:7D:62:33:A4 (EspJtag)\n";
        let p = parse_list(out);
        assert_eq!(p.len(), 2);

        assert_eq!(p[0].name, "STLink V2");
        assert_eq!(p[0].kind, "ST-LINK");
        // Empty serial → trailing colon dropped.
        assert_eq!(p[0].selector, "0483:3748");

        assert_eq!(p[1].name, "ESP JTAG");
        assert_eq!(p[1].kind, "EspJtag");
        // Colon-bearing serial kept verbatim.
        assert_eq!(p[1].selector, "303a:1001:50:78:7D:62:33:A4");
    }

    #[test]
    fn parses_serial_with_hex_string() {
        let out = "[0]: J-Link -- 1366:0101:000059012345 (JLink)\n";
        let p = parse_list(out);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].selector, "1366:0101:000059012345");
        assert_eq!(p[0].kind, "JLink");
    }

    #[test]
    fn no_probes_yields_empty() {
        assert!(parse_list("No debug probes were found.\n").is_empty());
        assert!(parse_list("").is_empty());
    }

    #[test]
    fn ignores_malformed_rows() {
        // Missing " -- " → not a probe row.
        assert!(parse_list("[0]: something odd (ST-LINK)\n").is_empty());
        // Header-ish noise.
        assert!(parse_list("The following debug probes were found:\n").is_empty());
    }
}
