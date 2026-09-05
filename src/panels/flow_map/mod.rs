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

pub mod gui;
pub mod layout;
pub mod parse;

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
                    Err(e) => panic!("{what} main.rs does not parse at line {}: {}", e.line, e.message),
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
        assert!(charted > 10, "only {charted} functions charted — the sweep found almost nothing");
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

#[cfg(test)]
mod dump_probe {
    use super::{layout, parse};

    const SMART_LIGHT: &str = r#"
#[embassy_executor::task]
async fn radar_task(mut rx: UartRx<'static, Async>, ctl: &'static Control) {
    let mut buf = [0u8; 64];
    let mut parser = Parser::new();
    loop {
        let n = rx.read(&mut buf).await.unwrap();
        for b in buf[..n].iter() {
            parser.feed(*b);
        }
        let Some(frame) = parser.take() else { continue };
        info!("dist {}", frame.distance_mm);
        if frame.distance_mm > MAX_RANGE {
            continue;
        }
        match ctl.mode() {
            Mode::Night => { ctl.fade_to(NIGHT_DUTY).await; }
            Mode::Normal => { ctl.fade_to(FULL_DUTY).await; }
            Mode::Smart0 => {
                let target = smart_duty(frame.distance_mm);
                ctl.fade_to(target).await;
            }
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

fn smart_duty(mm: u16) -> u16 {
    if mm < 800 { FULL_DUTY } else if mm < 3000 { NIGHT_DUTY } else { 0 }
}
"#;

    #[test]
    #[ignore]
    fn dump() {
        for c in parse::charts_of(SMART_LIGHT).unwrap() {
            println!("-- {} [{}] diverges={}", c.name, c.kind.word(), c.diverges);
            let l = layout::layout(&c);
            println!("   canvas {}x{}", l.width, l.height);
            for b in &l.boxes {
                println!(
                    "   [{:?}] y={:>6.1} x={:>6.1} w={:>5.1} {:?} await={}",
                    b.node.shape, b.y, b.x, b.w, b.node.text, b.node.awaits
                );
            }
            for e in &l.edges {
                println!("   edge {:?} label={:?} {:?}", e.kind, e.label, e.pts);
            }
        }
    }
}
