//! "All — whole file" in Implementation: every element of a file on ONE canvas
//! (pure logic, tested).
//!
//! The elements stack top to bottom in source order, the way the file reads:
//!
//! * a function is its own flowchart - [`layout`] of its chart, moved into
//!   place, so the whole-file canvas and the single-chart view can never draw
//!   the same function two different ways;
//! * a struct, enum, `const`, `use` run, macro call... is a declaration CARD;
//! * an `impl`, `trait`, inline `mod` or `extern` block is a FRAME around its
//!   members, which are indented inside it;
//! * a `#[cfg(test)]` module is one card listing what it holds. Tests are often
//!   half a file, and drawn in full they would push the code the reader opened
//!   the file for off the bottom of the canvas. The outline still lists them.
//!
//! Nothing here is iterated: members are measured by placing them, and a
//! container's frame is sized from where its last member ended.

use super::layout::{self, Edge, FlowLayout, MARGIN, Placed};
use super::parse::{DeclTag, Element, ElementKind, FileModel, FlowNode};

/// Room between two stacked elements.
pub const GAP: f32 = 18.0;
/// A frame's padding around its members.
pub const FRAME_PAD: f32 = 14.0;
/// Height of a frame's title bar.
pub const FRAME_TITLE_H: f32 = 26.0;
/// Rows a card lists before the rest become "+N more".
const CARD_ROWS: usize = 12;
/// Longest row a card keeps; the tooltip has the full text.
const CARD_TEXT: usize = 72;

/// A container drawn around its members.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub element: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `impl Debug for Foo<T>`, `mod app`.
    pub title: String,
    pub kind: ElementKind,
    /// What a click on the title jumps to.
    pub line: usize,
    pub generated: bool,
}

/// Where one element landed, and which boxes and edges are its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Extent {
    pub element: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// For a function: its chart's boxes and edges. For a container: every
    /// member's. For a card: the card.
    pub boxes: std::ops::Range<usize>,
    pub edges: std::ops::Range<usize>,
}

/// The whole-file canvas.
#[derive(Clone, Debug, Default)]
pub struct Composed {
    /// Every box and edge, already in canvas coordinates - drawn and hit-tested
    /// exactly like a single chart's.
    pub layout: FlowLayout,
    /// Outer containers before inner ones, so painting in order puts an inner
    /// frame on top of its parent.
    pub frames: Vec<Frame>,
    /// One per placed element (members of a collapsed test module have none).
    pub extents: Vec<Extent>,
}

impl Composed {
    /// Where a chart's section starts, by chart key - what "go to the called
    /// function" scrolls to. `None` for a function inside a collapsed test
    /// module, which the caller then opens as a chart of its own instead.
    pub fn anchor(&self, model: &FileModel, key: &str) -> Option<(f32, f32)> {
        self.extents
            .iter()
            .find(|x| model.elements[x.element].key == key)
            .map(|x| (x.x, x.y))
    }
}

/// Lay the whole of `model` out on one canvas.
pub fn compose(model: &FileModel) -> Composed {
    compose_scope(model, None)
}

/// Lay out `root` alone - a container with its members, a type's card, a
/// function's chart - or the whole file when `root` is `None`.
///
/// A `#[cfg(test)]` module picked as the root opens in full: the reader asked
/// for exactly the thing the whole-file view folds away.
pub fn compose_scope(model: &FileModel, root: Option<usize>) -> Composed {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); model.elements.len()];
    let mut top = Vec::new();
    for (i, e) in model.elements.iter().enumerate() {
        match e.parent {
            Some(p) => children[p].push(i),
            None => top.push(i),
        }
    }
    let items = match root {
        Some(r) if r < model.elements.len() => vec![r],
        Some(_) => Vec::new(),
        None => top,
    };
    let mut c = Composer {
        model,
        children,
        root,
        out: Composed::default(),
    };
    let (w, bottom) = c.stack(&items, MARGIN, MARGIN);
    c.out.layout.width = w + 2.0 * MARGIN;
    c.out.layout.height = bottom + MARGIN;
    c.out
}

struct Composer<'m> {
    model: &'m FileModel,
    children: Vec<Vec<usize>>,
    /// The element the canvas was asked for, if not the whole file.
    root: Option<usize>,
    out: Composed,
}

impl Composer<'_> {
    /// Place `items` one under the other from `(x, y)`; returns (widest, bottom).
    fn stack(&mut self, items: &[usize], x: f32, y: f32) -> (f32, f32) {
        let mut cursor = y;
        let mut widest: f32 = 0.0;
        for (k, &i) in items.iter().enumerate() {
            if k > 0 {
                cursor += GAP;
            }
            let (w, h) = self.place(i, x, cursor);
            widest = widest.max(w);
            cursor += h;
        }
        (widest, cursor)
    }

    /// Place one element with its top-left at `(x, y)`; returns its size.
    fn place(&mut self, i: usize, x: f32, y: f32) -> (f32, f32) {
        let e = &self.model.elements[i];
        let (b0, e0) = (self.out.layout.boxes.len(), self.out.layout.edges.len());
        let (w, h) = if let Some(c) = e.chart {
            let l = layout::layout(&self.model.charts[c]);
            self.paste(l, x, y)
        } else if e.kind.is_container()
            && (!is_collapsed_tests(self.model, e) || self.root == Some(i))
        {
            self.frame(i, x, y)
        } else {
            let node = card(self.model, i);
            let (w, h) = layout::box_size(&node);
            self.out.layout.boxes.push(Placed { node, x, y, w, h });
            (w, h)
        };
        self.out.extents.push(Extent {
            element: i,
            x,
            y,
            w,
            h,
            boxes: b0..self.out.layout.boxes.len(),
            edges: e0..self.out.layout.edges.len(),
        });
        (w, h)
    }

    /// Move a laid-out chart to `(x, y)` - every box AND every edge point.
    fn paste(&mut self, l: FlowLayout, x: f32, y: f32) -> (f32, f32) {
        let out = &mut self.out.layout;
        out.boxes.extend(l.boxes.into_iter().map(|mut b| {
            b.x += x;
            b.y += y;
            b
        }));
        out.edges.extend(l.edges.into_iter().map(|e| Edge {
            pts: e.pts.iter().map(|&(px, py)| (px + x, py + y)).collect(),
            ..e
        }));
        (l.width, l.height)
    }

    /// A container: title bar, then its members indented inside it.
    fn frame(&mut self, i: usize, x: f32, y: f32) -> (f32, f32) {
        let e = &self.model.elements[i];
        let title = frame_title(e);
        // Pushed BEFORE the members, so an outer frame paints first.
        let slot = self.out.frames.len();
        self.out.frames.push(Frame {
            element: i,
            x,
            y,
            w: 0.0,
            h: 0.0,
            title: title.clone(),
            kind: e.kind,
            line: e.ident_line,
            generated: e.generated,
        });
        let members = self.children[i].clone();
        let inner_y = y + FRAME_TITLE_H + FRAME_PAD;
        let (members_w, bottom) = if members.is_empty() {
            (0.0, inner_y)
        } else {
            self.stack(&members, x + FRAME_PAD, inner_y)
        };
        let title_w = title.chars().count() as f32 * 6.8 + 2.0 * FRAME_PAD;
        let w = (members_w + 2.0 * FRAME_PAD).max(title_w).max(160.0);
        let h = bottom - y + FRAME_PAD;
        let f = &mut self.out.frames[slot];
        f.w = w;
        f.h = h;
        (w, h)
    }
}

/// The OUTERMOST `#[cfg(test)]` module - drawn as one card, not a frame.
///
/// `test_code` is inherited by everything inside a test module, so a module
/// nested in one has it too; folding that as well would leave a test module
/// picked on its own half drawn, with a card where a frame should be.
fn is_collapsed_tests(model: &FileModel, e: &Element) -> bool {
    e.kind == ElementKind::Mod
        && e.test_code
        && e.parent.is_none_or(|p| !model.elements[p].test_code)
}

fn frame_title(e: &Element) -> String {
    match e.kind {
        ElementKind::Impl => format!("impl {}", e.name),
        ElementKind::ForeignMod => e.name.clone(),
        _ => format!("{} {}", e.kind.word(), e.name),
    }
}

/// `text`, cut to what a card row holds.
fn row(text: &str) -> String {
    let n = text.chars().count();
    if n <= CARD_TEXT {
        return text.to_string();
    }
    let mut s: String = text.chars().take(CARD_TEXT - 1).collect();
    s.push('…');
    s
}

/// `text` broken at spaces into rows of about [`CARD_TEXT`] characters, kept
/// whole: [`card`] cuts a longer one, [`card_tip`] shows it.
fn wrap(text: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > CARD_TEXT {
            rows.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

/// A card's header and rows, IN FULL - [`card`] cuts them to fit the box,
/// [`card_tip`] shows them whole.
fn card_text(model: &FileModel, i: usize) -> (String, Vec<String>) {
    let e = &model.elements[i];
    match e.kind {
        ElementKind::Struct | ElementKind::Enum | ElementKind::Union => (
            e.detail.first().cloned().unwrap_or_else(|| e.name.clone()),
            e.detail.iter().skip(1).cloned().collect(),
        ),
        ElementKind::Use => (
            if e.detail.len() > 1 {
                format!("use · {} imports", e.detail.len())
            } else {
                "use".to_string()
            },
            e.detail.clone(),
        ),
        ElementKind::CrateAttrs => ("crate attributes".to_string(), e.detail.clone()),
        ElementKind::MacroCall => (
            e.name.clone(),
            e.detail.first().map(|t| wrap(t)).unwrap_or_default(),
        ),
        // A collapsed test module lists what it holds, one member a row.
        ElementKind::Mod => (
            format!("mod {} · test code", e.name),
            model
                .elements
                .iter()
                .filter(|m| m.parent == Some(i))
                .map(member_row)
                .collect(),
        ),
        _ => (
            e.detail
                .first()
                .cloned()
                .unwrap_or_else(|| e.signature.clone()),
            e.detail.iter().skip(1).cloned().collect(),
        ),
    }
}

/// How a member reads on its test module's card: `fn a`, `use super::*`,
/// `struct Fixture` - never a kind word repeated as a name (`use use`).
fn member_row(m: &Element) -> String {
    match m.kind {
        ElementKind::Use => format!("use {}", m.detail.join(", ")),
        ElementKind::MacroCall | ElementKind::Other => m.signature.clone(),
        ElementKind::ForeignMod => m.name.clone(),
        _ => format!("{} {}", m.kind.word(), m.name),
    }
}

/// The card for a non-function, non-container element (or a collapsed test
/// module, which lists its members).
pub fn card(model: &FileModel, i: usize) -> FlowNode {
    let e = &model.elements[i];
    let (header, rows) = card_text(model, i);
    let hidden = rows.len().saturating_sub(CARD_ROWS);
    let rows: Vec<String> = rows.iter().take(CARD_ROWS).map(|r| row(r)).collect();
    FlowNode::card(
        row(&header),
        rows,
        hidden,
        e.ident_line,
        DeclTag {
            kind: e.kind,
            generated: e.generated,
            element: i,
        },
    )
}

/// A card's tooltip: its header and every row uncut, and where it is.
pub fn card_tip(model: &FileModel, i: usize) -> String {
    let e = &model.elements[i];
    let (header, rows) = card_text(model, i);
    let mut lines = vec![header];
    lines.extend(rows.iter().map(|r| format!("  {r}")));
    lines.push(String::new());
    lines.push(if e.start_line == e.end_line {
        format!("line {}", e.start_line)
    } else {
        format!("lines {}–{}", e.start_line, e.end_line)
    });
    if e.generated {
        lines.push("generated by the IDE".to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::flow_map::parse::{Shape, parse_file};

    const FILE: &str = r#"#![no_std]
use core::fmt;
use core::cell::RefCell;

/// Frames on the wire.
struct Frame { a: u8, b: u16 }
enum Mode { Idle, Busy(u8) }
const LIMIT: u32 = 10;

trait Feed {
    fn feed(&mut self, b: u8);
    fn reset(&mut self) { self.feed(0); }
}

impl Feed for Frame {
    fn feed(&mut self, b: u8) {
        if b > 3 { self.a = b; } else { self.b = 0; }
    }
}

mod inner {
    pub mod deeper {
        pub fn init() { loop { tick(); } }
        fn tick() {}
    }
}

bind_interrupts!(struct Irqs { USART1 => BufferedInterruptHandler<peripherals::USART1>; USART2 => BufferedInterruptHandler<peripherals::USART2>; });

#[entry]
fn main() -> ! {
    helper();
    loop {}
}

fn helper() {}

#[cfg(test)]
mod tests {
    use super::*;
    fn a() {}
    fn b() { assert!(true); }
}
"#;

    fn composed() -> (FileModel, Composed) {
        let m = parse_file(FILE).unwrap();
        let c = compose(&m);
        (m, c)
    }

    /// Top-level elements follow each other down the page, in source order,
    /// without overlapping - the canvas reads like the file.
    #[test]
    fn elements_stack_in_source_order_without_overlapping() {
        let (m, c) = composed();
        let top: Vec<&Extent> = c
            .extents
            .iter()
            .filter(|x| m.elements[x.element].parent.is_none())
            .collect();
        let order: Vec<usize> = top.iter().map(|x| x.element).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "source order");
        for w in top.windows(2) {
            assert!(
                w[1].y >= w[0].y + w[0].h + GAP - 0.01,
                "{} overlaps {}",
                m.elements[w[0].element].key,
                m.elements[w[1].element].key
            );
        }
    }

    /// A function on the whole-file canvas is EXACTLY its single chart, moved:
    /// every box and every edge point shifted by the same offset. Forgetting
    /// to move the edges would leave every arrow of the file at the top.
    #[test]
    fn a_function_is_its_own_chart_moved_into_place() {
        let (m, c) = composed();
        let mut checked = 0;
        for x in &c.extents {
            let Some(ci) = m.elements[x.element].chart else {
                continue;
            };
            let alone = layout::layout(&m.charts[ci]);
            let boxes = &c.layout.boxes[x.boxes.clone()];
            let edges = &c.layout.edges[x.edges.clone()];
            assert_eq!(boxes.len(), alone.boxes.len());
            assert_eq!(edges.len(), alone.edges.len());
            for (a, b) in boxes.iter().zip(&alone.boxes) {
                assert!((a.x - b.x - x.x).abs() < 1e-3 && (a.y - b.y - x.y).abs() < 1e-3);
                assert_eq!(a.node.text, b.node.text);
            }
            for (a, b) in edges.iter().zip(&alone.edges) {
                for (p, q) in a.pts.iter().zip(&b.pts) {
                    assert!((p.0 - q.0 - x.x).abs() < 1e-3 && (p.1 - q.1 - x.y).abs() < 1e-3);
                }
            }
            assert_eq!((x.w, x.h), (alone.width, alone.height));
            checked += 1;
        }
        assert!(checked >= 5, "only {checked} functions checked");
    }

    /// A container's frame holds its members - nested frames included - and
    /// the title bar stays clear of them.
    #[test]
    fn frames_hold_their_members() {
        let (m, c) = composed();
        let titles: Vec<&str> = c.frames.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "trait Feed",
                "impl Feed for Frame",
                "mod inner",
                "mod deeper"
            ]
        );
        for x in &c.extents {
            let Some(p) = m.elements[x.element].parent else {
                continue;
            };
            let f = c
                .frames
                .iter()
                .find(|f| f.element == p)
                .expect("a member's parent is framed");
            assert!(
                x.x >= f.x + FRAME_PAD - 0.01
                    && x.y >= f.y + FRAME_TITLE_H - 0.01
                    && x.x + x.w <= f.x + f.w + 0.01
                    && x.y + x.h <= f.y + f.h + 0.01,
                "{} spills out of {}",
                m.elements[x.element].key,
                f.title
            );
        }
    }

    /// The test module is one card listing what it holds; its functions are
    /// not drawn, and nothing else is missing.
    #[test]
    fn a_test_module_is_one_card_and_nothing_else_is_missing() {
        let (m, c) = composed();
        let tests = m
            .elements
            .iter()
            .position(|e| e.key == "mod tests")
            .unwrap();
        let card = c.extents.iter().find(|x| x.element == tests).unwrap();
        let b = &c.layout.boxes[card.boxes.start];
        assert_eq!(b.node.shape, Shape::Decl);
        assert_eq!(b.node.text, "mod tests · test code");
        // `use super::*` reads as itself, not as "use use".
        assert_eq!(b.node.detail, ["use super::*", "fn a", "fn b"]);
        for (i, e) in m.elements.iter().enumerate() {
            let drawn = c.extents.iter().filter(|x| x.element == i).count();
            let hidden = e.parent == Some(tests);
            assert_eq!(drawn, usize::from(!hidden), "{}", e.key);
        }
    }

    /// Cards read like the declaration: a header, then one row per field,
    /// variant, import - and a long macro call wrapped, not cut to one line.
    #[test]
    fn cards_list_what_they_declare() {
        let (m, c) = composed();
        let card = |key: &str| {
            let i = m.elements.iter().position(|e| e.key == key).unwrap();
            let x = c.extents.iter().find(|x| x.element == i).unwrap();
            c.layout.boxes[x.boxes.start].node.clone()
        };
        let s = card("struct Frame");
        assert_eq!(
            (s.text.as_str(), s.detail.clone()),
            (
                "struct Frame",
                vec!["a: u8".to_string(), "b: u16".to_string()]
            )
        );
        let u = card("use core::fmt");
        assert_eq!(u.text, "use · 2 imports");
        assert_eq!(u.detail, ["core::fmt", "core::cell::RefCell"]);
        assert_eq!(card("const LIMIT").text, "const LIMIT: u32 = 10;");
        let mac = card("macro bind_interrupts");
        assert!(mac.detail.len() > 1, "wrapped: {:?}", mac.detail);
        assert!(mac.detail.iter().all(|r| r.chars().count() <= CARD_TEXT));
        // A required trait method is a card too; the default one is a chart.
        assert_eq!(card("Feed::feed").shape, Shape::Decl);
        assert!(m.charts.iter().any(|ch| ch.key == "Feed::reset"));
    }

    /// The canvas holds everything, and no arrow of one function runs through
    /// a box of another - the sections really are apart.
    #[test]
    fn nothing_escapes_the_canvas_and_no_edge_crosses_a_box() {
        let (_, c) = composed();
        assert_sound(&c);
    }

    fn assert_sound(c: &Composed) {
        let l = &c.layout;
        for b in &l.boxes {
            assert!(
                b.x >= 0.0
                    && b.y >= 0.0
                    && b.x + b.w <= l.width + 0.01
                    && b.y + b.h <= l.height + 0.01,
                "box {:?} escapes the {}x{} canvas",
                b.node.text,
                l.width,
                l.height
            );
        }
        for f in &c.frames {
            assert!(
                f.x + f.w <= l.width + 0.01 && f.y + f.h <= l.height + 0.01,
                "{}",
                f.title
            );
        }
        const IN: f32 = 4.0;
        for e in &l.edges {
            for w in e.pts.windows(2) {
                for b in &l.boxes {
                    let hit = crate::panels::structure_map::layout::seg_hits_rect(
                        w[0],
                        w[1],
                        b.x + IN,
                        b.y + IN,
                        (b.w - 2.0 * IN).max(0.0),
                        (b.h - 2.0 * IN).max(0.0),
                    );
                    assert!(!hit, "a {:?} edge runs through {:?}", e.kind, b.node.text);
                }
            }
        }
    }

    /// Every file the IDE generates composes soundly, for every bundled chip.
    #[test]
    fn every_generated_file_composes() {
        let mut files = 0;
        for def in crate::panels::mcu_module::builtins::builtin_definitions() {
            let mcu = def.build_mcu();
            let mut sources = vec![("main.rs".to_string(), mcu.fresh_main_rs())];
            sources.extend(mcu.config_files());
            for (path, src) in sources {
                if !path.ends_with(".rs") {
                    continue;
                }
                let m = parse_file(&src).unwrap();
                let c = compose(&m);
                assert_sound(&c);
                assert_eq!(
                    c.extents.len(),
                    m.elements.len(),
                    "{} {path}: every element placed",
                    def.id
                );
                files += 1;
            }
        }
        assert!(files > 15);
    }

    /// Scrolling to a called function lands on its section.
    #[test]
    fn the_anchor_is_the_section_top() {
        let (m, c) = composed();
        let helper = m.elements.iter().position(|e| e.key == "helper").unwrap();
        let x = c.extents.iter().find(|x| x.element == helper).unwrap();
        assert_eq!(c.anchor(&m, "helper"), Some((x.x, x.y)));
        assert_eq!(c.anchor(&m, "tests::a"), None, "hidden in the test card");
    }
}
