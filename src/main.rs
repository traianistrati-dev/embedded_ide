// A GUI app in EVERY profile: Windows never allocates a console for it, so no
// black window pops up at startup — debug included. Debug used to keep the
// console just to see `println!`; `build::attach_parent_console()` (called first
// thing in `main`) buys that back without the window, by adopting the console we
// were launched from when there is one.
//
// The other half: every child process we spawn is given CREATE_NO_WINDOW (see
// `build::no_window`) so none of them flash a console either. Both halves are
// needed — the subsystem alone makes children flash WORSE, since they no longer
// have a parent console to inherit.
#![windows_subsystem = "windows"]

use eframe::egui;
pub mod activity;
pub mod app;
use app::AppIde;

pub mod build;
pub mod debugger;
pub mod dfu;
pub mod editor;
pub mod espflash;
pub mod failure_hint;
pub mod flamegraph;
pub mod git;
pub mod lsp;
pub mod msvc;
pub mod openocd;
pub mod panels;
pub mod probe;
pub mod probe_flash;
pub mod profile;
pub mod project_tree;
pub mod required_tools;
pub mod reveal;
pub mod rtt;
pub mod serial;
pub mod serial_bridge;
pub mod serial_frames;
pub mod serial_matrix;
pub mod serial_plot;
pub mod size;
pub mod terminal;
pub mod udev;
pub mod workspace;

/// Guard against the tofu-square bug coming back.
///
/// The bundled font has no arrow glyphs, so a raw `→` / `⇄` in a string that
/// reaches egui renders as an empty box. It has now been introduced and fixed
/// three times, always the same way: someone reaches for the prettier character
/// while writing a label. A grep-shaped test is the only thing that catches it
/// before the user does — the alternative is noticing it in a screenshot.
///
/// Comments are free to use arrows; only string LITERALS are checked. Use ASCII
/// `->` and `<->`, or a phosphor `ph::` icon.
#[cfg(test)]
mod glyph_guard {
    /// Arrow-class characters absent from the bundled font. Em dash, ellipsis
    /// and `×` are deliberately NOT here — those do render.
    const BANNED: [char; 7] = ['\u{2192}', '\u{2190}', '\u{2194}', '\u{21c4}', '\u{21c6}', '\u{21d2}', '\u{21bb}'];

    /// Does this line carry a banned glyph INSIDE a double-quoted literal?
    ///
    /// Walks the line tracking string state, so `// see "x" -> y` (a comment
    /// that merely contains quotes) is not flagged and `"a -> b" // note` is.
    /// A `//` reached outside a string ends the line.
    fn in_literal(line: &str) -> bool {
        let mut chars = line.chars().peekable();
        let mut in_str = false;
        while let Some(c) = chars.next() {
            match c {
                '\\' if in_str => {
                    chars.next(); // escaped char, whatever it is
                }
                '"' => in_str = !in_str,
                '/' if !in_str && chars.peek() == Some(&'/') => return false,
                c if in_str && BANNED.contains(&c) => return true,
                _ => {}
            }
        }
        false
    }

    fn scan(dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                scan(&p, out);
            } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    if in_literal(line) {
                        out.push(format!("{}:{}  {}", p.display(), i + 1, line.trim()));
                    }
                }
            }
        }
    }

    #[test]
    fn the_scanner_tells_comments_from_literals() {
        let a = '\u{2192}';
        assert!(in_literal(&format!("ui.label(\"a {a} b\");")));
        assert!(in_literal(&format!("x(\"a {a} b\"); // fine")));
        // Comments are allowed to use arrows, even alongside quotes.
        assert!(!in_literal(&format!("// \"Restore\" {a} confirm")));
        assert!(!in_literal(&format!("/// maps `\"x\"` {a} text")));
        assert!(!in_literal(&format!("let x = 1; // re-open {a} promoted")));
        // A `//` inside a string is not a comment.
        assert!(in_literal(&format!("u(\"http://x {a} y\");")));
        assert!(!in_literal("nothing here at all"));
    }

    #[test]
    fn no_arrow_glyphs_in_string_literals() {
        let mut bad = Vec::new();
        scan(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut bad,
        );
        assert!(
            bad.is_empty(),
            "the bundled font has no arrow glyphs - these render as empty boxes.\n\
             Use ASCII (-> , <->) or a phosphor ph:: icon:\n{}",
            bad.join("\n")
        );
    }
}

fn main() -> eframe::Result<()> {
    // FIRST, before anything can print: adopt the console we were launched from
    // (if any). A GUI-subsystem binary has no standard handles until this runs.
    build::attach_parent_console();
    // Then make a crash survivable to diagnose — double-clicked, there is no
    // stderr for the panic message to reach, so it also goes to a file.
    build::install_panic_logger();

    // Claim this instance's scratch workspace BEFORE anything can look it up —
    // `msvc::warm_up` below already spawns a thread, and every later reader
    // (build, LSP, watcher) must see the same answer. A second IDE window gets
    // its own slot instead of fighting over one directory (see `workspace`).
    workspace::init();

    // Resolve the MSVC toolchain env off-thread so the first build doesn't pay
    // for the one-off `vcvars64.bat` capture (see `msvc`).
    msvc::warm_up();

    // Everything eframe keys off the app NAME — including where it persists the
    // app state. Sharing that file between windows meant the last one to exit
    // decided which project both would reopen, so instances past the first get
    // their own. Slot 1 keeps the original name, and with it the state every
    // existing install already has.
    let app_name = format!("Embedded IDE{}", workspace::suffix());
    // The title is set explicitly so the storage name above doesn't leak into
    // it verbatim — a second window says "#2", which is what you want on a
    // taskbar, not "Embedded IDE_2".
    let title = match workspace::slot() {
        1 => "Embedded IDE".to_owned(),
        s => format!("Embedded IDE #{s}"),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_maximized(true)
            .with_title(&title)
            // Window + taskbar icon while the app runs. The PNG is baked into
            // the exe at compile time — replace assets/icon.png (any size,
            // 256×256 recommended) and rebuild to change it.
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("assets/icon.png must be a valid PNG"),
            ),
        ..Default::default()
    };

    eframe::run_native(
        &app_name,
        options,
        Box::new(|cc| Ok(Box::new(AppIde::new(cc)))),
    )
}
