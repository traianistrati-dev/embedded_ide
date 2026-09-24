//! "Flow" tab — the project's code drawn as an ALGORITHMIC flowchart
//! (terminal / process / in-out / decision / subroutine), one chart per
//! function.
//!
//! Three parts, the same split the Structure tab uses: `parse` turns source
//! text into a structured `Flow` tree, `layout` gives every box a position and
//! routes the edges, `gui` draws it. Only `gui` touches egui, so the two hard
//! parts are plain testable logic.
//!
//! **Why a TREE and not a node/edge graph.** Rust has no `goto`: every function
//! body is a nesting of sequences, branches and loops. Keeping that nesting is
//! what makes the layout a recursive measure/place — deterministic, nothing
//! iterated to convergence — and it is why the result looks like a textbook
//! flowchart instead of a spider web. The only edges that escape the nesting
//! are `break`, `continue`, `return` and `?`, and those travel in lanes the
//! measure pass reserves for them.

pub mod compose;
pub mod gui;
pub mod layout;
pub mod parse;

/// What to show, by key: a function, a container or a type
/// ([`parse::Element::openable`]).
///
/// `current` while it still names one; else the one saved for this file
/// (`persisted`) - matched by key, or by the bare NAME a project saved before
/// keys existed (`init` for RTIC's `app::init`); else the first ENTRY POINT,
/// because `main` is what the reader wants first, not whichever helper happens
/// to be at the top of the file; else the first chart.
pub fn choose_selection(
    model: &parse::FileModel,
    current: &str,
    persisted: Option<&str>,
) -> String {
    let open = |k: &str| {
        model
            .elements
            .iter()
            .find(|e| e.key == k && e.openable())
            .map(|e| e.key.clone())
    };
    let charts = &model.charts;
    open(current)
        .or_else(|| persisted.and_then(open))
        .or_else(|| {
            persisted
                .and_then(|p| charts.iter().find(|c| c.name == p))
                .map(|c| c.key.clone())
        })
        .or_else(|| {
            charts
                .iter()
                .find(|c| c.kind.is_entry())
                .map(|c| c.key.clone())
        })
        .or_else(|| charts.first().map(|c| c.key.clone()))
        .unwrap_or_default()
}

/// The toolbar's short note - a syntax error, an empty file - or `""` when all
/// is well.
///
/// `all`: the view shows ELEMENTS (the whole file, a container, a type)
/// rather than one function's chart.
pub fn status_line(
    model: &parse::FileModel,
    error: Option<&parse::SyntaxError>,
    all: bool,
) -> String {
    let nothing = if all {
        model.elements.is_empty()
    } else {
        model.charts.is_empty()
    };
    match error {
        Some(e) if nothing => format!("cannot parse this file — line {}: {}", e.line, e.message),
        Some(e) if all => format!(
            "showing the last good version of this file — line {} does not parse",
            e.line
        ),
        Some(e) => format!(
            "showing the last good chart — line {} does not parse",
            e.line
        ),
        None if !all && model.charts.is_empty() && !model.elements.is_empty() => format!(
            "no functions in this file — pick \"{}\" to see its {} elements",
            gui::ALL_LABEL,
            model.elements.len()
        ),
        None if nothing => "this file has no items".to_string(),
        None => String::new(),
    }
}

/// The element that source line `line` (1-based) belongs to: the INNERMOST
/// one whose lines hold it - a method rather than its `impl`, a function from
/// its doc comment down to its closing brace. `None` between items.
///
/// Two items written on one line are told apart by column, which a line does
/// not have: the first one wins.
pub fn element_at_line(model: &parse::FileModel, line: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, e) in model.elements.iter().enumerate() {
        if (e.start_line..=e.end_line).contains(&line)
            && best.is_none_or(|b| e.depth > model.elements[b].depth)
        {
            best = Some(i);
        }
    }
    best
}

/// The 1-based line that char index `idx` of `text` is on - an index past the
/// end is on the last line.
pub fn line_of_char(text: &str, idx: usize) -> usize {
    text.chars().take(idx).filter(|&c| c == '\n').count() + 1
}

/// Where the editor's caret went, when the tab should follow it: the caret's
/// char index. `caret` is the editor's caret with the file it is in,
/// `shown` the file the tab shows, `last` the caret followed last (updated
/// here).
///
/// Only a caret that MOVED is followed - every frame would undo each scroll
/// the reader makes in the tab - and only one in the file the tab shows: the
/// editor draws before the project tree, so for the frame of a click in the
/// tree its caret is still the previous file's.
pub fn caret_to_follow<F: Copy + PartialEq>(
    last: &mut Option<(F, usize)>,
    caret: Option<(F, usize)>,
    shown: F,
) -> Option<usize> {
    let (file, idx) = caret?;
    if file != shown || *last == caret {
        return None;
    }
    *last = caret;
    Some(idx)
}

#[cfg(test)]
mod selection {
    use super::{choose_selection, parse, status_line};

    const SRC: &str = "#[rtic::app(device = pac)]\nmod app {\n    #[init]\n    fn init(cx: init::Context) {}\n    fn helper() {}\n}\nfn free() {}\nstruct Frame { a: u8 }\nconst LIMIT: u32 = 3;\n";

    fn model() -> parse::FileModel {
        parse::parse_file(SRC).unwrap()
    }

    /// The current key wins while it still exists.
    #[test]
    fn the_current_chart_stays() {
        assert_eq!(
            choose_selection(&model(), "app::helper", Some("free")),
            "app::helper"
        );
    }

    /// A container or a type is a selection of its own; a `const` is not -
    /// it is read in the whole-file view, so the choice falls back.
    #[test]
    fn containers_and_types_can_be_selected_consts_cannot() {
        let m = model();
        assert_eq!(choose_selection(&m, "mod app", None), "mod app");
        assert_eq!(choose_selection(&m, "struct Frame", None), "struct Frame");
        assert_eq!(choose_selection(&m, "const LIMIT", None), "app::init");
    }

    /// A project saved before keys existed stored the bare NAME (`init` for
    /// RTIC's `app::init`); it still reopens on that function.
    #[test]
    fn an_old_saved_name_still_finds_its_chart() {
        assert_eq!(choose_selection(&model(), "", Some("init")), "app::init");
        // Not the entry point, so the fallback cannot land on it by accident.
        assert_eq!(
            choose_selection(&model(), "", Some("helper")),
            "app::helper"
        );
        assert_eq!(
            choose_selection(&model(), "", Some("app::helper")),
            "app::helper"
        );
    }

    /// Nothing saved: the entry point, not the first helper in the file.
    #[test]
    fn with_nothing_saved_the_entry_point_opens() {
        let src = "fn helper() {}\n#[entry]\nfn main() -> ! { loop {} }\n";
        let m = parse::parse_file(src).unwrap();
        assert_eq!(choose_selection(&m, "gone", None), "main");
        assert_eq!(
            choose_selection(&parse::FileModel::default(), "gone", None),
            ""
        );
    }

    /// The note under the toolbar, per mode.
    #[test]
    fn the_status_line_speaks_for_the_mode_in_view() {
        let empty = parse::FileModel::default();
        let decls = parse::parse_file("mod a;\nmod b;\n").unwrap();
        let full = parse::parse_file(SRC).unwrap();
        let err = parse::SyntaxError {
            line: 7,
            message: "expected `}`".to_string(),
        };
        assert_eq!(status_line(&full, None, false), "");
        assert_eq!(status_line(&full, None, true), "");
        // `pins/mod.rs`: nothing to chart, but a list to show.
        assert_eq!(
            status_line(&decls, None, false),
            "no functions in this file — pick \"All — whole file\" to see its 2 elements"
        );
        assert_eq!(status_line(&decls, None, true), "");
        assert_eq!(status_line(&empty, None, true), "this file has no items");
        assert!(
            status_line(&full, Some(&err), true)
                .starts_with("showing the last good version of this file")
        );
        assert!(status_line(&full, Some(&err), false).starts_with("showing the last good chart"));
        assert!(
            status_line(&empty, Some(&err), true).starts_with("cannot parse this file — line 7")
        );
    }
}

#[cfg(test)]
mod caret {
    use super::{caret_to_follow, element_at_line, line_of_char, parse};

    const SRC: &str = "use a::b;\nuse c::d;\n\n/// Docs.\n#[inline]\nfn free() {\n    x();\n}\n\nimpl Uart {\n    fn send(&self) {\n        y();\n    }\n\n    fn recv(&self) {}\n}\nstruct A; struct B;\n";

    fn at(line: usize) -> Option<String> {
        let m = parse::parse_file(SRC).unwrap();
        element_at_line(&m, line).map(|i| m.elements[i].key.clone())
    }

    /// A function owns its doc comment and attributes, and the lines down to
    /// its closing brace.
    #[test]
    fn a_function_owns_its_docs_and_its_body() {
        for line in 4..=8 {
            assert_eq!(at(line).as_deref(), Some("free"), "line {line}");
        }
    }

    /// Between two items, nothing.
    #[test]
    fn a_blank_line_between_items_is_nobody_s() {
        assert_eq!(at(3), None);
        assert_eq!(at(9), None);
        assert_eq!(at(99), None);
    }

    /// Inside an `impl`, the method - the innermost - and the `impl` only
    /// where no method is.
    #[test]
    fn a_method_line_is_the_method_not_its_impl() {
        assert_eq!(at(12).as_deref(), Some("Uart::send"));
        assert_eq!(at(15).as_deref(), Some("Uart::recv"));
        let imp = at(10).unwrap();
        assert!(imp.starts_with("impl"), "{imp}");
        assert_eq!(at(14), Some(imp.clone()), "the blank line between methods");
        assert_eq!(at(16), Some(imp), "its closing brace");
    }

    /// A run of `use`s is one element; two items on one line are the first.
    #[test]
    fn a_use_run_is_one_element_and_one_line_goes_to_its_first_item() {
        assert_eq!(at(1), at(2));
        assert!(at(1).is_some());
        assert_eq!(at(17).as_deref(), Some("struct A"));
    }

    #[test]
    fn a_char_index_maps_to_its_line() {
        assert_eq!(line_of_char("ab\ncd\n", 0), 1);
        assert_eq!(line_of_char("ab\ncd\n", 2), 1, "on the newline itself");
        assert_eq!(line_of_char("ab\ncd\n", 3), 2);
        assert_eq!(line_of_char("ab\ncd\n", 6), 3);
        assert_eq!(line_of_char("ab\ncd\n", 600), 3, "past the end");
        assert_eq!(line_of_char("é\nx", 2), 2, "chars, not bytes");
    }

    /// Followed once per move, in the tab's own file only.
    #[test]
    fn only_a_caret_that_moved_in_the_shown_file_is_followed() {
        let mut last = None;
        assert_eq!(caret_to_follow(&mut last, Some((1, 40)), 1), Some(40));
        // The same caret the next frame: the reader may be scrolling.
        assert_eq!(caret_to_follow(&mut last, Some((1, 40)), 1), None);
        assert_eq!(caret_to_follow(&mut last, Some((1, 41)), 1), Some(41));
        // No caret at all.
        assert_eq!(caret_to_follow(&mut last, None, 1), None);
        assert_eq!(last, Some((1, 41)));
    }

    /// The frame of a click in the project tree: the tab already shows file
    /// 2, the editor's caret is still file 1's. Not followed, and not taken
    /// as seen either - so file 2's caret, the next frame, is.
    #[test]
    fn a_caret_in_another_file_is_not_followed() {
        let mut last = Some((1, 41));
        assert_eq!(caret_to_follow(&mut last, Some((1, 90)), 2), None);
        assert_eq!(last, Some((1, 41)));
        assert_eq!(caret_to_follow(&mut last, Some((2, 90)), 2), Some(90));
        // The same index in another file is a move.
        assert_eq!(caret_to_follow(&mut last, Some((1, 90)), 1), Some(90));
    }
}

/// The Flow tab against the code this IDE actually writes.
///
/// The unit tests either side of this exercise hand-written snippets, which is
/// the wrong shape to catch the thing that would really break the tab: the
/// generator emitting something `syn` cannot read, or an entry-point attribute
/// nobody added to [`parse::EntryKind`]. Those only show up on the real output,
/// and the same trap has been paid for once already — three passes over the
/// codegen templates missed sixty constants that only the GENERATED text
/// revealed.
#[cfg(test)]
mod against_generated_code {
    use super::{layout, parse};
    use crate::panels::mcu_module::builtins;
    use crate::panels::mcu_module::mcu::Runtime;

    /// Nothing may land outside the canvas the GUI scales to fit — an
    /// off-canvas box is simply invisible.
    fn assert_on_canvas(what: &str, l: &layout::FlowLayout) {
        for b in &l.boxes {
            assert!(
                b.x >= 0.0
                    && b.y >= 0.0
                    && b.x + b.w <= l.width + 0.01
                    && b.y + b.h <= l.height + 0.01,
                "{what}: box {:?} escapes the {}x{} canvas",
                b.node.text,
                l.width,
                l.height
            );
        }
    }

    /// Every built-in chip, on both runtimes: main.rs parses, has an entry
    /// point, and every one of its functions lays out.
    #[test]
    fn every_generated_main_charts() {
        let mut charted = 0;
        for def in builtins::builtin_definitions() {
            for runtime in [Runtime::Blocking, Runtime::Async] {
                let mut mcu = def.build_mcu();
                mcu.runtime = runtime;
                let src = mcu.fresh_main_rs();
                let what = format!("{} {runtime:?}", def.id);
                let charts = match parse::charts_of(&src) {
                    Ok(c) => c,
                    Err(e) => panic!(
                        "{what} main.rs does not parse at line {}: {}",
                        e.line, e.message
                    ),
                };
                assert!(
                    charts.iter().any(|c| c.kind.is_entry()),
                    "{what}: no entry point found — main.rs always has one, so an \
                     attribute shape is missing from `entry_kind`. Got: {:?}",
                    charts.iter().map(|c| (&c.name, c.kind)).collect::<Vec<_>>()
                );
                for c in &charts {
                    assert_on_canvas(&format!("{what} {}", c.name), &layout::layout(c));
                }
                charted += charts.len();
            }
        }
        assert!(
            charted > 10,
            "only {charted} functions charted — the sweep found almost nothing"
        );
    }

    /// A generated `main` runs forever, so its chart must not claim an END.
    /// This is the single most visible way the drawing could lie about the
    /// program.
    #[test]
    fn a_generated_main_never_ends() {
        let mut checked = 0;
        for def in builtins::builtin_definitions() {
            let mcu = def.build_mcu();
            let src = mcu.fresh_main_rs();
            for c in parse::charts_of(&src).expect("parses") {
                if c.name != "main" {
                    continue;
                }
                assert!(
                    c.diverges,
                    "{}: `main` is drawn as if it returns; its endless loop was missed",
                    def.id
                );
                let l = layout::layout(&c);
                assert!(
                    !l.boxes.iter().any(|b| b.node.text == "END"),
                    "{}: an END box under an endless main",
                    def.id
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no generated `main` was found to check");
    }

    /// The generated init block collapses to ONE box. Without this the chart of
    /// a real main.rs opens on forty rectangles of peripheral setup and the
    /// user's own loop is somewhere off the bottom of the screen.
    #[test]
    fn the_generated_init_is_one_box_not_forty() {
        // A chip whose init is long enough for the difference to matter.
        let def = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id.contains("esp32"))
            .expect("an ESP built-in");
        let src = def.build_mcu().fresh_main_rs();
        assert!(
            parse::generated_ranges(&src).len() == 1,
            "main.rs should carry exactly one GENERATED block, found {:?}",
            parse::generated_ranges(&src)
        );
        let main = parse::charts_of(&src)
            .expect("parses")
            .into_iter()
            .find(|c| c.name == "main")
            .expect("a main");
        let l = layout::layout(&main);
        let generated: Vec<&layout::Placed> = l
            .boxes
            .iter()
            .filter(|b| b.node.shape == parse::Shape::Generated)
            .collect();
        assert_eq!(
            generated.len(),
            1,
            "the generated setup must be a single box, got {}",
            generated.len()
        );
    }
}
