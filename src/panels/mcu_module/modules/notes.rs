//! Notes on a Virtual Module: free text, a documentation link and an image of
//! the device wired to it - a GPS receiver's datasheet and a photo of its board,
//! attached to the USART it sits on.
//!
//! # Why this is not a field on `VirtualModule`
//!
//! A module is not a durable thing. `reconcile_modules` runs every frame and
//! drops a derived module the moment its peripheral has no pins, then rebuilds
//! it from `default_config` - new id, position (0, 0) - when the pins come back.
//! The id is not stable even when nothing is dropped: `free_module_id` hands a
//! freed id to the very next module created, so anything keyed on it lands on a
//! stranger. Notes stored on the module would vanish on an unwire and re-wire,
//! on Reset pins, or on clearing a bus to move it.
//!
//! What DOES survive all of that is `(ModuleKind, instance)` - "USART1" - which
//! is exactly what `reconcile_modules` matches a module on. So notes live in a
//! map on `Mcu` keyed by that pair ([`NotesKey`]). `reconcile_modules`,
//! `remove_module` and undo never see the map, so they cannot lose it. A key with
//! no live module is an ORPHAN: kept, listed, and re-attached by itself when that
//! peripheral is wired again.
//!
//! None of this is codegen input - see `calculate_mcu_state_hash`.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

use super::ModuleKind;

/// What notes are stored under. See the module docs for why it is not the
/// module id.
pub type NotesKey = (ModuleKind, u8);

/// What the user wrote about the device on one peripheral instance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleNotes {
    /// Free text, any number of lines.
    #[serde(default)]
    pub text: String,
    /// A documentation link. Only `http(s)` is ever opened - see
    /// [`link_is_openable`].
    #[serde(default)]
    pub link: String,
    /// Project-relative, `/`-separated path of a COPY of the image - see
    /// [`store_image`]. Never absolute: `mcu.config` is committed, so it may
    /// come from someone else's machine.
    #[serde(default)]
    pub image: String,
}

impl ModuleNotes {
    /// Nothing worth keeping. An entry like this is neither written to
    /// `mcu.config` nor listed as an orphan.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.link.trim().is_empty() && self.image.trim().is_empty()
    }

    /// A short text for a hover: the first lines of the notes, then the link and
    /// the image. `None` when there is nothing to show.
    pub fn preview(&self) -> Option<String> {
        const MAX_LINES: usize = 6;
        const MAX_CHARS: usize = 300;

        let mut parts: Vec<String> = Vec::new();
        let text = self.text.trim();
        if !text.is_empty() {
            let lines: Vec<&str> = text.lines().collect();
            let mut body = lines[..lines.len().min(MAX_LINES)].join("\n");
            let mut cut = lines.len() > MAX_LINES;
            // By CHARACTER: a byte slice would panic inside a multi-byte one.
            if body.chars().count() > MAX_CHARS {
                body = body.chars().take(MAX_CHARS).collect();
                cut = true;
            }
            let mut body = body.trim_end().to_owned();
            if cut {
                body.push_str(" \u{2026}");
            }
            parts.push(body);
        }
        let link = self.link.trim();
        if !link.is_empty() {
            parts.push(link.to_owned());
        }
        let image = self.image.trim();
        if !image.is_empty() {
            parts.push(format!("Image: {image}"));
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }
}

/// The link as the browser will get it: PARSED, `http` or `https`, with a host
/// - or `None`, and then it is shown but never opened.
///
/// A prefix check is not enough, and was the first version. The click goes
/// through egui to `webbrowser::open`, which parses the string with the same
/// `url` crate and, when THAT fails, does not refuse: it treats the text as a
/// file path relative to the IDE's working directory and opens a `file://`
/// URL. So `https://[/../../x.html` - an unterminated IPv6 host - passed a
/// prefix check and opened a local file, and a plain typo like `https://st .com`
/// opened a bogus local page. Parsing here, with the same parser, and handing
/// on the SERIALISED result means what reaches `webbrowser` always parses, so
/// its file fallback cannot fire.
///
/// `mcu.config` is committed, so a link can come from someone else.
pub fn openable_link(link: &str) -> Option<String> {
    let url = url::Url::parse(link.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some(url.into())
}

/// Whether the link would be opened - see [`openable_link`].
pub fn link_is_openable(link: &str) -> bool {
    openable_link(link).is_some()
}

/// Where attached images are copied, relative to the project root.
///
/// A ROOT folder with no `Cargo.toml`, on purpose. The project tree reads every
/// file it scans as TEXT, and a PNG is not valid UTF-8, so it comes back as `""`
/// - and Save writes that back, emptying the image. The tree never scans a root
/// folder without a manifest, so an image here is safe. [`store_image`] refuses
/// the case where someone has turned `docs/` into a crate.
pub const NOTES_DIR: &str = "docs/modules";

/// Largest image accepted. Every image is committed with the project; a phone
/// photo is a few MB, and past 20 MB it is a mistake rather than a photo.
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// The formats the decoder is built with - the `image` features in
/// `Cargo.toml`. HEIC, the iPhone default, is not one of them.
pub const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "bmp"];

/// Resolve a stored image path against the project root, refusing anything that
/// could point outside it: an absolute path, a drive or UNC prefix, `..`.
///
/// `mcu.config` travels with the project, so the path in it is untrusted input.
/// A leading separator and a `:` are refused by TEXT as well as by component,
/// because on Linux `C:/x.png` is a perfectly relative path to a folder `C:`.
pub fn resolve_image(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("no image".to_owned());
    }
    let refuse = || Err(format!("not a path inside the project: {rel}"));
    if rel.starts_with(['/', '\\']) || rel.contains(':') {
        return refuse();
    }
    let path = Path::new(rel);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return refuse();
    }
    Ok(root.join(path))
}

/// Read an image the user picked: its bytes, and a file name that is safe to
/// store under.
///
/// Refused before anything is copied: a file that is not one of
/// [`IMAGE_EXTENSIONS`], and one over [`MAX_IMAGE_BYTES`]. The size is checked
/// from the metadata first, so a 4 GB file is never read into memory.
pub fn read_image_file(src: &Path) -> Result<(String, Vec<u8>), String> {
    read_image_file_capped(src, MAX_IMAGE_BYTES)
}

fn read_image_file_capped(src: &Path, cap: u64) -> Result<(String, Vec<u8>), String> {
    let name = safe_file_name(src)?;
    let meta = std::fs::metadata(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", src.display()));
    }
    if meta.len() > cap {
        return Err(format!(
            "{name} is {:.1} MB; the limit is {} MB, because every image is committed with the project",
            meta.len() as f64 / (1024.0 * 1024.0),
            cap / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    Ok((name, bytes))
}

/// The picked file's name, reduced to characters that are safe in a path on
/// every OS and in [`resolve_image`] (which refuses `:`).
fn safe_file_name(src: &Path) -> Result<String, String> {
    let raw = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "the file has no usable name".to_owned())?;
    let ext = Path::new(raw)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "{raw}: only PNG, JPEG and BMP images are supported"
        ));
    }
    Ok(raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect())
}

/// Copy image bytes into [`NOTES_DIR`] and return the project-relative path to
/// store in [`ModuleNotes::image`].
///
/// A name taken by DIFFERENT bytes gets the tree's own `_1`, `_2` suffix
/// ([`crate::project_tree::clipboard::free_name`]); the same bytes under the
/// same name are reused rather than copied twice, so one photo attached to two
/// modules is one file.
///
/// The IDE never deletes an image. Removing it from a module clears only the
/// reference, so nothing the user put in the project disappears behind them.
pub fn store_image(root: &Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    for d in ["docs", NOTES_DIR] {
        if root.join(d).join("Cargo.toml").exists() {
            return Err(format!(
                "{d}/ holds a Cargo.toml, so the project tree scans it and Save would empty any image in it"
            ));
        }
    }
    let dir = root.join(NOTES_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {NOTES_DIR}: {e}"))?;
    let pick = crate::project_tree::clipboard::free_name(name, |cand| {
        let p = dir.join(cand);
        // Free when absent, or when it already holds exactly these bytes.
        p.exists() && std::fs::read(&p).map_or(true, |existing| existing != bytes)
    });
    let dest = dir.join(&pick);
    if !dest.exists() {
        std::fs::write(&dest, bytes)
            .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    }
    Ok(format!("{NOTES_DIR}/{pick}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eide_notes_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_http_and_https_links_open() {
        for ok in [
            "https://www.st.com/x.pdf",
            "http://a.b",
            "  HTTPS://CAPS.example  ",
        ] {
            assert!(link_is_openable(ok), "{ok}");
        }
        for bad in [
            "",
            "https://",
            "javascript:alert(1)",
            "file:///C:/x.pdf",
            "C:\\x.pdf",
            "www.st.com",
            "ftp://x",
        ] {
            assert!(!link_is_openable(bad), "{bad}");
        }
    }

    /// The case a prefix check let through: text that starts with `https://`
    /// but does not parse. `webbrowser` would have opened each as a local
    /// `file://` path instead of refusing it.
    #[test]
    fn a_link_that_does_not_parse_is_not_opened() {
        for bad in [
            "https://[/../../../../Users/Public/x.html",
            "https://[x",
            "https://st .com/x.pdf",
            "http://",
        ] {
            assert_eq!(openable_link(bad), None, "{bad:?}");
        }
        // What IS opened is the parsed form, which always parses again.
        let got = openable_link("  HTTPS://Example.COM/a b.pdf ").unwrap();
        assert_eq!(got, "https://example.com/a%20b.pdf");
        assert!(url::Url::parse(&got).is_ok());
    }

    #[test]
    fn whitespace_is_empty_and_has_no_preview() {
        let n = ModuleNotes {
            text: "  \n ".into(),
            link: " ".into(),
            image: String::new(),
        };
        assert!(n.is_empty());
        assert_eq!(n.preview(), None);
    }

    #[test]
    fn the_preview_caps_long_text_and_keeps_the_link() {
        let n = ModuleNotes {
            text: (1..=10)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            link: "https://x.example/gps.pdf".into(),
            image: "docs/modules/gps.jpg".into(),
        };
        let p = n.preview().unwrap();
        assert!(p.contains("line 6") && !p.contains("line 7"), "{p}");
        assert!(p.contains('\u{2026}'), "a cut preview says so: {p}");
        assert!(p.contains("https://x.example/gps.pdf"), "{p}");
        assert!(p.contains("docs/modules/gps.jpg"), "{p}");

        // Multi-byte text is cut by character, never inside one.
        let wide = ModuleNotes {
            text: "\u{00fc}".repeat(400),
            ..Default::default()
        };
        assert!(wide.preview().unwrap().starts_with('\u{00fc}'));
    }

    #[test]
    fn resolve_refuses_paths_outside_the_project() {
        let root = Path::new("proj");
        for bad in [
            "../x.png",
            "docs/../../x.png",
            "./x.png",
            "C:/x.png",
            "/etc/x",
            "\\\\server\\share\\x.png",
            "",
        ] {
            assert!(resolve_image(root, bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            resolve_image(root, "docs/modules/x.png").unwrap(),
            root.join("docs/modules/x.png")
        );
    }

    #[test]
    fn an_image_is_copied_byte_for_byte_under_a_free_name() {
        let root = scratch("copy");
        let a = store_image(&root, "x.png", b"first").unwrap();
        assert_eq!(a, "docs/modules/x.png");
        let b = store_image(&root, "x.png", b"second").unwrap();
        assert_eq!(b, "docs/modules/x_1.png", "different bytes take a new name");
        assert_eq!(std::fs::read(root.join(&a)).unwrap(), b"first");
        assert_eq!(std::fs::read(root.join(&b)).unwrap(), b"second");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_same_file_twice_is_not_copied_twice() {
        let root = scratch("same");
        let a = store_image(&root, "gps.jpg", b"photo").unwrap();
        let b = store_image(&root, "gps.jpg", b"photo").unwrap();
        assert_eq!(a, b);
        let n = std::fs::read_dir(root.join(NOTES_DIR)).unwrap().count();
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_refuses_when_docs_is_a_crate() {
        let root = scratch("crate");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/Cargo.toml"), "[package]").unwrap();
        let err = store_image(&root, "x.png", b"x").unwrap_err();
        assert!(err.contains("Cargo.toml"), "{err}");
        assert!(!root.join(NOTES_DIR).exists(), "nothing written");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_picked_file_is_checked_before_it_is_read() {
        let root = scratch("pick");
        let txt = root.join("notes.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(read_image_file(&txt).unwrap_err().contains("PNG"));

        let big = root.join("big photo (1).JPG");
        std::fs::write(&big, vec![0u8; 2048]).unwrap();
        let err = read_image_file_capped(&big, 1024).unwrap_err();
        assert!(err.contains("limit"), "{err}");

        let (name, bytes) = read_image_file_capped(&big, 4096).unwrap();
        assert_eq!(name, "big_photo__1_.JPG", "only path-safe characters");
        assert_eq!(bytes.len(), 2048);
        let _ = std::fs::remove_dir_all(&root);
    }
}
