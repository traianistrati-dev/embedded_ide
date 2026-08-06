//! Locate a **usable** MSVC host toolchain on Windows and expose its `LIB` /
//! `INCLUDE` environment.
//!
//! Why this exists: Rust needs the MSVC linker (and, for crates with C code, the
//! MSVC compiler) to build **host** artifacts — every build-script and
//! proc-macro. `rustc`/`cc-rs` pick an install by asking little more than "does
//! `cl.exe` exist here?", so a **partially-installed** Visual Studio (compiler
//! present, `lib\x64\` + `include\` missing — e.g. a workload that was removed
//! or an interrupted update) **shadows** a complete one and every build dies with
//!
//! ```text
//! LINK : fatal error LNK1104: cannot open file 'msvcrt.lib'
//! fatal error C1083: Cannot open include file: 'vcruntime.h'
//! ```
//!
//! So we verify installs **by file** (`lib\x64\msvcrt.lib` + `include\vcruntime.h`),
//! not by binary presence, and inject the good one's `LIB`/`INCLUDE` into every
//! command we spawn (see [`crate::build::no_window`]). The values come from the
//! install's own `vcvars64.bat`, so they match exactly what a "Developer Command
//! Prompt" would set — no hand-assembled SDK paths to drift.

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// One Visual Studio / Build Tools installation and whether its **x64 host C++
/// toolchain is complete** (usable for linking Rust build-scripts).
#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct MsvcInstall {
    /// Install root, e.g. `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`.
    pub path: PathBuf,
    /// Newest toolset found, e.g. `14.44.35207` (empty when none).
    pub toolset: String,
    /// `lib\x64\msvcrt.lib` present (the linker's C runtime).
    pub has_libs: bool,
    /// `include\vcruntime.h` present (the compiler's headers).
    pub has_headers: bool,
}

#[cfg(windows)]
impl MsvcInstall {
    /// Complete enough to build Rust host artifacts (link + compile C).
    pub fn is_complete(&self) -> bool {
        self.has_libs && self.has_headers
    }
    /// Short label for the Tools tab, e.g. `BuildTools 14.44.35207`.
    pub fn label(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.toolset.is_empty() {
            name
        } else {
            format!("{name} {}", self.toolset)
        }
    }
}

// ── Enumeration ───────────────────────────────────────────────────────────────

/// Every Visual Studio install known to `vswhere` (the official locator), each
/// probed for a complete x64 toolchain. Falls back to scanning the standard
/// install roots when `vswhere` is absent.
#[cfg(windows)]
pub fn installs() -> Vec<MsvcInstall> {
    let mut roots = vswhere_paths();
    if roots.is_empty() {
        roots = fallback_scan();
    }
    roots.into_iter().map(probe_install).collect()
}

/// The first install with a COMPLETE x64 toolchain, if any.
#[cfg(windows)]
pub fn usable() -> Option<MsvcInstall> {
    installs().into_iter().find(MsvcInstall::is_complete)
}

/// Ask `vswhere` for every install path. NOTE: `vswhere` exits **0 with empty
/// output** when nothing matches, so the exit code alone proves nothing — we key
/// off the printed paths.
#[cfg(windows)]
fn vswhere_paths() -> Vec<PathBuf> {
    let Some(exe) = vswhere_exe() else {
        return Vec::new();
    };
    let out = crate::build::no_window_raw(&mut std::process::Command::new(exe))
        .args([
            "-all",
            "-products",
            "*",
            "-property",
            "installationPath",
            "-nologo",
        ])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(windows)]
fn vswhere_exe() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramFiles(x86)")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .map(PathBuf::from)?;
    let p = base
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    p.is_file().then_some(p)
}

/// Standard install roots (`<ProgramFiles[ (x86)]>\Microsoft Visual Studio\<year>\<edition>`),
/// used only when `vswhere` is missing.
#[cfg(windows)]
fn fallback_scan() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Some(base) = std::env::var_os(var).map(PathBuf::from) else {
            continue;
        };
        let vs = base.join("Microsoft Visual Studio");
        let Ok(years) = std::fs::read_dir(&vs) else {
            continue;
        };
        for y in years.flatten() {
            let Ok(editions) = std::fs::read_dir(y.path()) else {
                continue;
            };
            for e in editions.flatten() {
                if e.path().join("VC").join("Tools").join("MSVC").is_dir() {
                    out.push(e.path());
                }
            }
        }
    }
    out
}

/// Check one install root: newest toolset + whether its x64 libs/headers exist.
#[cfg(windows)]
fn probe_install(path: PathBuf) -> MsvcInstall {
    let tools = path.join("VC").join("Tools").join("MSVC");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&tools)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    versions.sort(); // lexicographic is fine for 14.xx.yyyyy
    let newest = versions.last().cloned();
    let (toolset, has_libs, has_headers) = match newest {
        Some(v) => (
            v.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            v.join("lib").join("x64").join("msvcrt.lib").is_file(),
            v.join("include").join("vcruntime.h").is_file(),
        ),
        None => (String::new(), false, false),
    };
    MsvcInstall {
        path,
        toolset,
        has_libs,
        has_headers,
    }
}

// ── Environment injection ─────────────────────────────────────────────────────

/// `LIB` / `INCLUDE` of a complete install, captured **once** by running its
/// `vcvars64.bat` (so the Windows-SDK paths are exactly what Microsoft's own
/// script computes).
///
/// Deliberately empty — i.e. we touch nothing — unless there is a real hazard:
/// * the process already has `LIB` (a Developer Command Prompt): the user's
///   environment wins, never second-guess it;
/// * **every** install is complete: whatever `rustc` picks works, so staying out
///   of the way avoids pairing one install's libs with another's `cl.exe`;
/// * no install is complete: nothing to point at anyway.
///
/// That leaves exactly the broken case — a complete install *and* an incomplete
/// one, where `rustc`/`cc-rs` may pick the incomplete one and fail.
#[cfg(windows)]
pub fn env_pairs() -> &'static [(std::ffi::OsString, std::ffi::OsString)] {
    use std::sync::OnceLock;
    static ENV: OnceLock<Vec<(std::ffi::OsString, std::ffi::OsString)>> = OnceLock::new();
    ENV.get_or_init(|| {
        // Already inside a Developer prompt → its env wins, change nothing.
        if std::env::var_os("LIB").is_some_and(|v| !v.is_empty()) {
            return Vec::new();
        }
        let all = installs();
        // An install that carries a compiler but no libs/headers is what shadows
        // a good one. With none of those around, leave the toolchain alone.
        if !all.iter().any(|i| !i.is_complete()) {
            return Vec::new();
        }
        all.into_iter()
            .find(MsvcInstall::is_complete)
            .map(|i| vcvars_env(&i.path))
            .unwrap_or_default()
    })
}

/// Run `<install>\VC\Auxiliary\Build\vcvars64.bat` and harvest `LIB`/`INCLUDE`.
#[cfg(windows)]
fn vcvars_env(install: &Path) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let bat = install
        .join("VC")
        .join("Auxiliary")
        .join("Build")
        .join("vcvars64.bat");
    if !bat.is_file() {
        return Vec::new();
    }
    // `cmd.exe` does NOT parse quotes the way Rust's argument escaping writes
    // them, so a normal `.args(["/C", "call \"…\" && set"])` arrives mangled and
    // silently produces no output. Pass the command line VERBATIM with `raw_arg`,
    // using cmd's own convention: `/C "<everything>"` (it strips that outer pair).
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("cmd");
    crate::build::no_window_raw(&mut cmd);
    // `call … >nul` keeps the banner out; `set` then dumps the environment.
    cmd.raw_arg(format!(
        "/C \"\"{}\" >nul 2>&1 && set\"",
        bat.display()
    ));
    let out = cmd.output();
    let Ok(out) = out else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pairs = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if v.trim().is_empty() {
            continue;
        }
        if k.eq_ignore_ascii_case("LIB") || k.eq_ignore_ascii_case("INCLUDE") {
            pairs.push((
                std::ffi::OsString::from(k.to_ascii_uppercase()),
                std::ffi::OsString::from(v),
            ));
        }
    }
    pairs
}

/// Warm the (slow, one-off) `vcvars64.bat` capture on a background thread so the
/// first build doesn't pay for it. Safe to call more than once.
#[cfg(windows)]
pub fn warm_up() {
    std::thread::spawn(|| {
        let _ = env_pairs();
    });
}

#[cfg(not(windows))]
pub fn warm_up() {}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Build a fake install tree: `<root>/VC/Tools/MSVC/<ver>/…`.
    fn fake_install(root: &Path, ver: &str, libs: bool, headers: bool) {
        let v = root.join("VC").join("Tools").join("MSVC").join(ver);
        if libs {
            let d = v.join("lib").join("x64");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("msvcrt.lib"), b"").unwrap();
        }
        if headers {
            let d = v.join("include");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("vcruntime.h"), b"").unwrap();
        }
        if !libs && !headers {
            std::fs::create_dir_all(v.join("bin")).unwrap();
        }
    }

    #[test]
    fn complete_install_is_detected() {
        let tmp = std::env::temp_dir().join("eide_msvc_test_ok");
        let _ = std::fs::remove_dir_all(&tmp);
        fake_install(&tmp, "14.44.35207", true, true);
        let i = probe_install(tmp.clone());
        assert_eq!(i.toolset, "14.44.35207");
        assert!(i.has_libs && i.has_headers, "{i:?}");
        assert!(i.is_complete());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The real-world failure: `cl.exe` is there but libs/headers are not — the
    /// case a "does the binary exist" check would wrongly report as OK.
    #[test]
    fn partial_install_is_not_complete() {
        let tmp = std::env::temp_dir().join("eide_msvc_test_partial");
        let _ = std::fs::remove_dir_all(&tmp);
        fake_install(&tmp, "14.44.35207", false, false);
        let i = probe_install(tmp.clone());
        assert!(!i.is_complete(), "binary-only install must NOT count: {i:?}");
        assert!(!i.has_libs && !i.has_headers);

        // libs but no headers (links, but any C crate fails to compile)
        let tmp2 = std::env::temp_dir().join("eide_msvc_test_libs_only");
        let _ = std::fs::remove_dir_all(&tmp2);
        fake_install(&tmp2, "14.44.35207", true, false);
        assert!(!probe_install(tmp2.clone()).is_complete());

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp2);
    }

    #[test]
    fn newest_toolset_wins_and_label_is_readable() {
        let tmp = std::env::temp_dir().join("eide_msvc_test_multi");
        let _ = std::fs::remove_dir_all(&tmp);
        fake_install(&tmp, "14.30.00000", true, true);
        fake_install(&tmp, "14.44.35207", true, true);
        let i = probe_install(tmp.clone());
        assert_eq!(i.toolset, "14.44.35207", "newest toolset picked");
        assert!(i.label().ends_with("14.44.35207"), "{}", i.label());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_vc_dir_is_incomplete() {
        let tmp = std::env::temp_dir().join("eide_msvc_test_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let i = probe_install(tmp.clone());
        assert!(i.toolset.is_empty() && !i.is_complete());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
