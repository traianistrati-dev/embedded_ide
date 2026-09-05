//! Source text → flowchart, via `syn` (pure logic, tested).
//!
//! `syn` rather than a hand-written scanner because the whole value of this
//! view is that the BRANCHES are right: a diamond in the wrong place is worse
//! than no diamond. A brace scanner can find `if`, but `else if` chains, match
//! arms, `if let`, closures and macro bodies are exactly where hand-rolled
//! parsers rot.
//!
//! Two consequences of that choice, both handled here rather than papered over:
//!
//! * **`syn` fails on half-typed code.** [`charts_of`] returns the error WITH
//!   its line, so the tab can keep showing the last good chart and say why it
//!   is stale, instead of blanking while the user types.
//! * **`syn` drops comments** — they are not tokens. The generated-init markers
//!   are therefore found in the raw TEXT ([`generated_ranges`]) and matched
//!   against statement line numbers.
//!
//! Labels are sliced out of the ORIGINAL source by span, so a box reads exactly
//! what the user wrote (`dist < THRESHOLD && is_night()`), not a reconstruction.
//! That needs `proc-macro2/span-locations`; without it `Span::start()` does not
//! exist at all.

use proc_macro2::Span;
use std::collections::HashMap;
use syn::spanned::Spanned;
use syn::visit::Visit;

// ── Model ────────────────────────────────────────────────────────────────────

/// Classic flowchart shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Stadium — `Start` / `END`.
    Terminal,
    /// Rectangle — a run of plain statements, collapsed into one box.
    Process,
    /// Parallelogram — a statement that talks to the outside world.
    Io,
    /// Diamond — `if` / `match` / a loop's test.
    Decision,
    /// Rectangle with side bars ("predefined process") — a call to another
    /// function of this project. Clicking it jumps to the definition.
    Subroutine,
    /// The generated init block, collapsed to one dimmed box. Not the user's
    /// algorithm, so it must not spend forty rectangles of the reader's screen.
    Generated,
}

/// Where a non-structured edge goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Leave `n` loop levels (1 = the innermost). A labelled `break 'outer`
    /// resolves to its real depth — treating every break as depth 1 would draw
    /// an arrow to the wrong loop, which is a lie, not an approximation.
    Break(usize),
    /// Back to the head of the loop `n` levels out.
    Continue(usize),
    /// Out of the function.
    Return,
}

/// One box.
#[derive(Clone, Debug)]
pub struct FlowNode {
    pub text: String,
    /// Further statements folded into the same box (a run of plain code).
    pub detail: Vec<String>,
    /// Statements past the detail cap — rendered as "+N more".
    pub hidden: usize,
    pub shape: Shape,
    /// 1-based source line — what a click on the box jumps to.
    pub line: usize,
    /// Contains an `.await`: a point where the cooperative executor may switch
    /// to another task. On an async project this is the single most useful
    /// thing the chart can mark.
    pub awaits: bool,
    /// Contains a `?`: an error path leaves here for the function's end.
    pub try_exit: bool,
    /// For [`Shape::Subroutine`] — the callee's definition line in this file.
    pub goto_line: Option<usize>,
}

impl FlowNode {
    fn new(text: String, shape: Shape, line: usize) -> Self {
        Self {
            text,
            detail: Vec::new(),
            hidden: 0,
            shape,
            line,
            awaits: false,
            try_exit: false,
            goto_line: None,
        }
    }

    /// Lines of text the box shows (the first one plus any folded statements).
    pub fn lines(&self) -> usize {
        1 + self.detail.len() + usize::from(self.hidden > 0)
    }
}

/// One labelled way out of a [`Flow::Branch`].
#[derive(Clone, Debug)]
pub struct Arm {
    pub label: String,
    pub body: Flow,
}

/// A loop's entry test, or the absence of one.
///
/// `While` and `For` are kept apart even though both draw a diamond: their
/// EDGE LABELS differ ("YES / NO" against "each / done"), and deriving that
/// from the label text would tie the layout to a string the parser happens to
/// build.
#[derive(Clone, Debug)]
pub enum LoopHead {
    /// `loop { … }` — nothing to test, the back edge is the only way round.
    Infinite,
    /// `while c { … }`
    While(FlowNode),
    /// `for x in it { … }`
    For(FlowNode),
}

impl LoopHead {
    /// The diamond above the body, if this loop has one.
    pub fn node(&self) -> Option<&FlowNode> {
        match self {
            Self::Infinite => None,
            Self::While(n) | Self::For(n) => Some(n),
        }
    }

    /// Labels for the two edges out of the test: (into the body, out of the loop).
    pub fn labels(&self) -> (&'static str, &'static str) {
        match self {
            Self::Infinite => ("", ""),
            Self::While(_) => ("YES", "NO"),
            Self::For(_) => ("each", "done"),
        }
    }
}

/// The structured body of a function.
#[derive(Clone, Debug)]
pub enum Flow {
    Node(FlowNode),
    /// Top-to-bottom on one spine. An EMPTY sequence is meaningful: it is the
    /// missing `else` of an `if`, drawn as a plain line down to the join.
    Seq(Vec<Flow>),
    Branch {
        cond: FlowNode,
        arms: Vec<Arm>,
    },
    Loop {
        head: LoopHead,
        body: Box<Flow>,
        /// 1-based line of the `loop` / `while` / `for` keyword.
        line: usize,
    },
    Jump {
        node: FlowNode,
        target: Target,
    },
}

/// What kind of thing starts this chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// `#[entry]`, `#[embassy_executor::main]`, `#[esp_rtos::main]`, RTIC `#[init]`.
    Main,
    /// `#[embassy_executor::task]`, RTIC `#[idle]` / software `#[task]`.
    Task,
    /// `#[interrupt]`, RTIC `#[task(binds = …)]` — started by hardware, not by
    /// a caller.
    Interrupt,
    /// An ordinary function: reachable only because something calls it.
    Function,
}

impl EntryKind {
    pub fn is_entry(self) -> bool {
        !matches!(self, Self::Function)
    }

    /// Short word shown beside the chart's name.
    pub fn word(self) -> &'static str {
        match self {
            Self::Main => "entry",
            Self::Task => "task",
            Self::Interrupt => "irq",
            Self::Function => "fn",
        }
    }
}

/// One function, drawn as one chart.
#[derive(Clone, Debug)]
pub struct Chart {
    /// `"main"`, `"radar_task"`, `"Parser::feed"`.
    pub name: String,
    pub kind: EntryKind,
    /// 1-based line of the function's name.
    pub line: usize,
    pub is_async: bool,
    pub body: Flow,
    /// No path returns — so the chart gets NO `END` terminal.
    ///
    /// Firmware's `fn main() -> !` ends in `loop {}`; drawing an `END` under it
    /// would state something false about the program. The textbook example has
    /// an END because its loop has an exit.
    pub diverges: bool,
}

/// A `syn` parse failure, with the line it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: usize,
    pub message: String,
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// Every function of `src` as a chart, in source order.
///
/// Walks into `impl` blocks (methods are named `Type::method`) and into inline
/// `mod` blocks — which is what makes RTIC readable, since `#[rtic::app] mod
/// app { … }` puts the whole application inside one module item.
pub fn charts_of(src: &str) -> Result<Vec<Chart>, SyntaxError> {
    let file = syn::parse_file(src).map_err(|e| SyntaxError {
        line: e.span().start().line.max(1),
        message: e.to_string(),
    })?;
    let mut b = Builder {
        lines: src.lines().collect(),
        generated: generated_ranges(src),
        locals: HashMap::new(),
        loops: Vec::new(),
    };
    collect_locals(&file.items, &mut b.locals);
    let mut out = Vec::new();
    b.items(&file.items, &mut out);
    Ok(out)
}

/// 1-based inclusive line ranges covered by a GENERATED marker pair.
///
/// Matched on the marker PREFIX rather than the exact constants, because the
/// generator writes three spellings of it — `GEN_BEGIN` / `GEN_END` in
/// `codegen::common` for main.rs, plus the bare `// <<< GENERATED>>>` the
/// peripheral config files open with. An unclosed opener runs to end of file,
/// which is what a half-written file looks like.
pub fn generated_ranges(src: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    let total = src.lines().count();
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if !t.starts_with("// <<< GENERATED") {
            continue;
        }
        if t.contains("END") {
            if let Some(start) = open.take() {
                out.push((start, i + 1));
            }
        } else if open.is_none() {
            open = Some(i + 1);
        }
    }
    if let Some(start) = open {
        out.push((start, total.max(start)));
    }
    out
}

// ── Builder ──────────────────────────────────────────────────────────────────

/// Plain statements folded into one box before the rest become "+N more".
const RUN_DETAIL_MAX: usize = 5;
/// Longest label kept on a box; the rest is elided.
const LABEL_MAX: usize = 56;

struct Builder<'a> {
    lines: Vec<&'a str>,
    generated: Vec<(usize, usize)>,
    /// Every function defined in this file → its line, so a call to one becomes
    /// a clickable subroutine box.
    locals: HashMap<String, usize>,
    /// Innermost-last stack of loop labels, for resolving `break 'outer`.
    loops: Vec<Option<String>>,
}

/// Names of every function in `items` (top level, in `impl` blocks, in inline
/// `mod`s) → the line of its name.
fn collect_locals(items: &[syn::Item], out: &mut HashMap<String, usize>) {
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                out.insert(f.sig.ident.to_string(), f.sig.ident.span().start().line);
            }
            syn::Item::Impl(im) => {
                for it in &im.items {
                    if let syn::ImplItem::Fn(f) = it {
                        out.entry(f.sig.ident.to_string())
                            .or_insert_with(|| f.sig.ident.span().start().line);
                    }
                }
            }
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect_locals(inner, out);
                }
            }
            _ => {}
        }
    }
}

impl<'a> Builder<'a> {
    /// Walk items, appending one chart per function found.
    fn items(&mut self, items: &[syn::Item], out: &mut Vec<Chart>) {
        for item in items {
            match item {
                syn::Item::Fn(f) => {
                    let kind = entry_kind(&f.attrs);
                    let name = f.sig.ident.to_string();
                    let chart = self.function(&name, kind, &f.sig, &f.block.stmts);
                    out.push(chart);
                }
                syn::Item::Impl(im) => {
                    let ty = self.snippet(im.self_ty.span());
                    for it in &im.items {
                        if let syn::ImplItem::Fn(f) = it {
                            let kind = entry_kind(&f.attrs);
                            let name = format!("{ty}::{}", f.sig.ident);
                            let chart = self.function(&name, kind, &f.sig, &f.block.stmts);
                            out.push(chart);
                        }
                    }
                }
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        // RTIC puts the whole application in one `mod app`; a
                        // path prefix would make every name unreadable, so only
                        // the function name is kept.
                        self.items(inner, out);
                    }
                }
                _ => {}
            }
        }
    }

    fn function(
        &mut self,
        name: &str,
        kind: EntryKind,
        sig: &syn::Signature,
        stmts: &[syn::Stmt],
    ) -> Chart {
        self.loops.clear();
        let body = self.block(stmts);
        let never = matches!(
            &sig.output,
            syn::ReturnType::Type(_, t) if matches!(**t, syn::Type::Never(_))
        );
        Chart {
            name: name.to_string(),
            kind,
            line: sig.ident.span().start().line,
            is_async: sig.asyncness.is_some(),
            diverges: never || !falls_through(&body),
            body,
        }
    }

    /// A block's statements as a sequence, with runs of plain statements folded
    /// into single boxes.
    fn block(&mut self, stmts: &[syn::Stmt]) -> Flow {
        let mut out: Vec<Flow> = Vec::new();
        let mut run: Option<FlowNode> = None;
        for st in stmts {
            self.stmt(st, &mut out, &mut run);
        }
        flush(&mut run, &mut out);
        Flow::Seq(out)
    }

    fn stmt(&mut self, st: &syn::Stmt, out: &mut Vec<Flow>, run: &mut Option<FlowNode>) {
        let line = st.span().start().line;
        if self.in_generated(line) {
            let mut n = FlowNode::new("generated setup".to_string(), Shape::Generated, line);
            n.detail.push(self.label(st.span()));
            self.push_plain(n, out, run);
            return;
        }
        match st {
            // A nested `fn` / `struct` definition is not part of the flow.
            syn::Stmt::Item(_) => {}
            syn::Stmt::Local(l) => {
                let mut n = self.plain_node(l.span(), line);
                // `let x = if c { a } else { b };` is a VALUE, not a branch in
                // the function's spine — it stays one box.
                if let Some(init) = &l.init {
                    self.mark(&mut n, &init.expr);
                }
                self.push_plain(n, out, run);
            }
            syn::Stmt::Macro(m) => {
                let mut n = self.plain_node(m.span(), line);
                if is_io_macro(&m.mac.path) {
                    n.shape = Shape::Io;
                }
                self.push_plain(n, out, run);
            }
            syn::Stmt::Expr(e, _) => self.expr_stmt(e, line, out, run),
        }
    }

    fn expr_stmt(
        &mut self,
        e: &syn::Expr,
        line: usize,
        out: &mut Vec<Flow>,
        run: &mut Option<FlowNode>,
    ) {
        match e {
            syn::Expr::If(i) => {
                flush(run, out);
                let f = self.if_expr(i);
                out.push(f);
            }
            syn::Expr::Match(m) => {
                flush(run, out);
                let f = self.match_expr(m);
                out.push(f);
            }
            syn::Expr::Loop(l) => {
                flush(run, out);
                let label = l.label.as_ref().map(|x| x.name.ident.to_string());
                let mut f = self.loop_body(LoopHead::Infinite, label, &l.body.stmts, line);
                // `loop {}` — a spin with no statements. Left empty it draws a
                // back edge from nothing to nothing, which reads as a broken
                // chart rather than as an idle loop; the `panic_handler` every
                // generated main.rs carries is exactly this shape.
                if l.body.stmts.is_empty()
                    && let Flow::Loop { body, .. } = &mut f
                {
                    **body = Flow::Seq(vec![Flow::Node(FlowNode::new(
                        self.label(e.span()),
                        Shape::Process,
                        line,
                    ))]);
                }
                out.push(f);
            }
            syn::Expr::While(w) => {
                flush(run, out);
                let text = format!("while {}", self.snippet(w.cond.span()));
                let head = LoopHead::While(self.decision(text, line));
                let label = w.label.as_ref().map(|x| x.name.ident.to_string());
                let f = self.loop_body(head, label, &w.body.stmts, line);
                out.push(f);
            }
            syn::Expr::ForLoop(fl) => {
                flush(run, out);
                let text = format!(
                    "for {} in {}",
                    self.snippet(fl.pat.span()),
                    self.snippet(fl.expr.span())
                );
                let head = LoopHead::For(self.decision(text, line));
                let label = fl.label.as_ref().map(|x| x.name.ident.to_string());
                let f = self.loop_body(head, label, &fl.body.stmts, line);
                out.push(f);
            }
            // A bare block (or `unsafe { … }`) adds nesting but no control flow.
            syn::Expr::Block(b) => {
                flush(run, out);
                let f = self.block(&b.block.stmts);
                out.push(f);
            }
            syn::Expr::Unsafe(u) => {
                flush(run, out);
                let f = self.block(&u.block.stmts);
                out.push(f);
            }
            syn::Expr::Break(b) => {
                flush(run, out);
                let depth = self.depth_of(b.label.as_ref());
                let node = self.plain_node(b.span(), line);
                out.push(Flow::Jump {
                    node,
                    target: Target::Break(depth),
                });
            }
            syn::Expr::Continue(c) => {
                flush(run, out);
                let depth = self.depth_of(c.label.as_ref());
                let node = self.plain_node(c.span(), line);
                out.push(Flow::Jump {
                    node,
                    target: Target::Continue(depth),
                });
            }
            syn::Expr::Return(r) => {
                flush(run, out);
                let node = self.plain_node(r.span(), line);
                out.push(Flow::Jump {
                    node,
                    target: Target::Return,
                });
            }
            other => {
                let mut n = self.plain_node(other.span(), line);
                self.mark(&mut n, other);
                self.push_plain(n, out, run);
            }
        }
    }

    fn if_expr(&mut self, i: &syn::ExprIf) -> Flow {
        let line = i.if_token.span.start().line;
        let text = self.snippet(i.cond.span());
        let cond = self.decision(text, line);
        let then = self.block(&i.then_branch.stmts);
        let mut arms = vec![Arm {
            label: "YES".to_string(),
            body: then,
        }];
        let no = match i.else_branch.as_ref().map(|(_, e)| &**e) {
            // `else if` becomes a nested diamond in the NO arm — which is what
            // an else-if chain actually is.
            Some(syn::Expr::If(inner)) => self.if_expr(inner),
            Some(syn::Expr::Block(b)) => self.block(&b.block.stmts),
            Some(other) => {
                let l = other.span().start().line;
                let mut seq = Vec::new();
                let mut run = None;
                self.expr_stmt(other, l, &mut seq, &mut run);
                flush(&mut run, &mut seq);
                Flow::Seq(seq)
            }
            // No `else`: an empty arm, drawn as a plain line down to the join.
            None => Flow::Seq(Vec::new()),
        };
        arms.push(Arm {
            label: "NO".to_string(),
            body: no,
        });
        Flow::Branch { cond, arms }
    }

    fn match_expr(&mut self, m: &syn::ExprMatch) -> Flow {
        let line = m.match_token.span.start().line;
        let text = format!("match {}", self.snippet(m.expr.span()));
        let cond = self.decision(text, line);
        let mut arms = Vec::new();
        for a in &m.arms {
            let mut label = self.snippet(a.pat.span());
            if let Some((_, guard)) = &a.guard {
                label = format!("{label} if {}", self.snippet(guard.span()));
            }
            let body = match &*a.body {
                syn::Expr::Block(b) => self.block(&b.block.stmts),
                other => {
                    let l = other.span().start().line;
                    let mut seq = Vec::new();
                    let mut run = None;
                    self.expr_stmt(other, l, &mut seq, &mut run);
                    flush(&mut run, &mut seq);
                    Flow::Seq(seq)
                }
            };
            arms.push(Arm {
                label: truncate(&label, 28),
                body,
            });
        }
        Flow::Branch { cond, arms }
    }

    fn loop_body(
        &mut self,
        head: LoopHead,
        label: Option<String>,
        stmts: &[syn::Stmt],
        line: usize,
    ) -> Flow {
        self.loops.push(label);
        let body = self.block(stmts);
        self.loops.pop();
        Flow::Loop {
            head,
            body: Box::new(body),
            line,
        }
    }

    /// How many loop levels a `break` / `continue` leaves: 1 for the innermost,
    /// more when it names an enclosing label.
    fn depth_of(&self, label: Option<&syn::Lifetime>) -> usize {
        let Some(lt) = label else { return 1 };
        let want = lt.ident.to_string();
        self.loops
            .iter()
            .rev()
            .position(|l| l.as_deref() == Some(want.as_str()))
            .map(|i| i + 1)
            .unwrap_or(1)
    }

    fn decision(&self, text: String, line: usize) -> FlowNode {
        FlowNode::new(truncate(&text, LABEL_MAX), Shape::Decision, line)
    }

    /// A plain statement box (classification happens in [`Self::mark`]).
    fn plain_node(&self, span: Span, line: usize) -> FlowNode {
        FlowNode::new(self.label(span), Shape::Process, line)
    }

    /// Text of `span`, squeezed and truncated for a box label.
    fn label(&self, span: Span) -> String {
        truncate(&self.snippet(span), LABEL_MAX)
    }

    /// Classify `expr` and stamp the node with what the scan found.
    fn mark(&self, n: &mut FlowNode, expr: &syn::Expr) {
        let mut scan = Scan::default();
        scan.visit_expr(expr);
        n.awaits = scan.awaits;
        n.try_exit = scan.try_exit;
        // A call into this project's own code wins over the I-O heuristic: the
        // reader can OPEN a subroutine box, so saying so is worth more than
        // saying the statement touches a peripheral.
        if let Some(line) = scan.calls.iter().find_map(|c| self.locals.get(c)) {
            n.shape = Shape::Subroutine;
            n.goto_line = Some(*line);
        } else if scan.io {
            n.shape = Shape::Io;
        }
    }

    /// Verbatim source of `span`, whitespace squeezed to single spaces.
    fn snippet(&self, span: Span) -> String {
        let (s, e) = (span.start(), span.end());
        if s.line == 0 || s.line > self.lines.len() {
            return String::new();
        }
        let cut = |line: usize, from: usize, to: Option<usize>| -> String {
            let cs: Vec<char> = self.lines[line - 1].chars().collect();
            let a = from.min(cs.len());
            let b = to.map(|t| t.min(cs.len())).unwrap_or(cs.len()).max(a);
            cs[a..b].iter().collect()
        };
        let raw = if s.line == e.line {
            cut(s.line, s.column, Some(e.column))
        } else {
            let mut parts = vec![cut(s.line, s.column, None)];
            for i in (s.line + 1)..e.line.min(self.lines.len() + 1) {
                parts.push(self.lines[i - 1].trim().to_string());
            }
            if e.line <= self.lines.len() {
                parts.push(cut(e.line, 0, Some(e.column)).trim().to_string());
            }
            parts.join(" ")
        };
        squeeze(&raw)
    }

    fn in_generated(&self, line: usize) -> bool {
        self.generated.iter().any(|&(a, b)| line >= a && line <= b)
    }

    /// Append a plain box, folding it into the previous one when both are the
    /// same foldable shape.
    ///
    /// Only `Process` and `Generated` fold. An I-O parallelogram or a
    /// subroutine box is the very thing the reader is looking for — folding one
    /// into a run of `let`s would hide it.
    fn push_plain(&self, n: FlowNode, out: &mut Vec<Flow>, run: &mut Option<FlowNode>) {
        let foldable = matches!(n.shape, Shape::Process | Shape::Generated);
        match run {
            Some(prev) if foldable && prev.shape == n.shape => {
                prev.awaits |= n.awaits;
                prev.try_exit |= n.try_exit;
                // Fold what the incoming box would SHOW. A generated box
                // carries its statement in `detail` and a fixed title in
                // `text`, so folding the title instead repeated the words
                // "generated setup" once per statement.
                let mut rows = n.detail;
                if rows.is_empty() {
                    rows.push(n.text);
                }
                for row in rows {
                    if prev.detail.len() < RUN_DETAIL_MAX {
                        prev.detail.push(row);
                    } else {
                        prev.hidden += 1;
                    }
                }
            }
            _ => {
                flush(run, out);
                if foldable {
                    *run = Some(n);
                } else {
                    out.push(Flow::Node(n));
                }
            }
        }
    }
}

/// Emit the pending run, if any.
fn flush(run: &mut Option<FlowNode>, out: &mut Vec<Flow>) {
    if let Some(n) = run.take() {
        out.push(Flow::Node(n));
    }
}

// ── Classification ───────────────────────────────────────────────────────────

/// Method names that unmistakably touch the outside world.
///
/// A deliberately conservative list: a false parallelogram is a lie about the
/// program, while a missed one only costs a rectangle. Phase 3 replaces this
/// with the IDE's OWN knowledge — it generated `usart1`, so it knows what that
/// handle is — and the heuristic then only has to cover hand-written code.
const IO_METHODS: &[&str] = &[
    "read",
    "read_exact",
    "read_data",
    "read_byte",
    "read_bytes",
    "read_raw",
    "write",
    "write_all",
    "write_str",
    "write_fmt",
    "write_byte",
    "write_bytes",
    "flush",
    "send",
    "recv",
    "receive",
    "transfer",
    "transfer_in_place",
    "transaction",
    "set_high",
    "set_low",
    "toggle",
    "set_duty",
    "set_duty_cycle",
    "is_high",
    "is_low",
    "get_level",
    "set_level",
    "blocking_read",
    "blocking_write",
    "blocking_flush",
    "wait_for_high",
    "wait_for_low",
    "wait_for_rising_edge",
    "wait_for_falling_edge",
    "wait_for_any_edge",
];

/// Macros that print or log — the `Print X` parallelogram of the textbook.
const IO_MACROS: &[&str] = &[
    "print", "println", "eprint", "eprintln", "info", "warn", "error", "debug", "trace",
];

fn is_io_macro(path: &syn::Path) -> bool {
    path.segments
        .last()
        .map(|s| IO_MACROS.contains(&s.ident.to_string().as_str()))
        .unwrap_or(false)
}

/// What one statement's expression contains.
///
/// Closures and inner `async` blocks are NOT descended into: a `?` or `.await`
/// inside a closure belongs to the closure, not to the function being drawn,
/// and an arrow leaving this box for the function's end would be wrong.
#[derive(Default)]
struct Scan {
    awaits: bool,
    try_exit: bool,
    io: bool,
    calls: Vec<String>,
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}

    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}

    fn visit_expr_await(&mut self, e: &'ast syn::ExprAwait) {
        self.awaits = true;
        syn::visit::visit_expr_await(self, e);
    }

    fn visit_expr_try(&mut self, e: &'ast syn::ExprTry) {
        self.try_exit = true;
        syn::visit::visit_expr_try(self, e);
    }

    fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
        let name = e.method.to_string();
        if IO_METHODS.contains(&name.as_str()) {
            self.io = true;
        }
        self.calls.push(name);
        syn::visit::visit_expr_method_call(self, e);
    }

    fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*e.func
            && let Some(seg) = p.path.segments.last()
        {
            self.calls.push(seg.ident.to_string());
        }
        syn::visit::visit_expr_call(self, e);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if is_io_macro(&m.path) {
            self.io = true;
        }
    }
}

/// The entry-point kind an attribute list declares.
fn entry_kind(attrs: &[syn::Attribute]) -> EntryKind {
    for a in attrs {
        let Some(last) = a.path().segments.last() else {
            continue;
        };
        match last.ident.to_string().as_str() {
            // `#[entry]`, `#[embassy_executor::main]`, `#[esp_rtos::main]`.
            "entry" | "main" | "init" => return EntryKind::Main,
            "interrupt" => return EntryKind::Interrupt,
            // RTIC's `#[task(binds = EXTI0)]` IS an interrupt handler; a plain
            // `#[task]` (RTIC software task, embassy task) is not.
            "task" => {
                let binds = match &a.meta {
                    syn::Meta::List(l) => l.tokens.to_string().contains("binds"),
                    _ => false,
                };
                return if binds {
                    EntryKind::Interrupt
                } else {
                    EntryKind::Task
                };
            }
            "idle" => return EntryKind::Task,
            _ => {}
        }
    }
    EntryKind::Function
}

// ── Reachability ─────────────────────────────────────────────────────────────

/// Whether control can reach the END of this piece of flow.
///
/// Drives whether the chart gets an `END` terminal at all. A `Seq` needs EVERY
/// element to fall through: once one does not, whatever follows is unreachable
/// and so is the sequence's own exit.
pub fn falls_through(f: &Flow) -> bool {
    match f {
        Flow::Node(_) => true,
        Flow::Jump { .. } => false,
        Flow::Seq(v) => v.iter().all(falls_through),
        Flow::Branch { arms, .. } => arms.iter().any(|a| falls_through(&a.body)),
        // A tested loop can always fail its test and fall out; an infinite one
        // is only left by a `break` aimed at it.
        Flow::Loop { head, body, .. } => match head {
            LoopHead::Infinite => breaks_out(body, 1),
            _ => true,
        },
    }
}

/// Whether `f` contains a `break` that leaves the loop `k` levels above it.
fn breaks_out(f: &Flow, k: usize) -> bool {
    match f {
        Flow::Node(_) => false,
        Flow::Jump { target, .. } => matches!(target, Target::Break(d) if *d >= k),
        Flow::Seq(v) => v.iter().any(|x| breaks_out(x, k)),
        Flow::Branch { arms, .. } => arms.iter().any(|a| breaks_out(&a.body, k)),
        Flow::Loop { body, .. } => breaks_out(body, k + 1),
    }
}

// ── Text helpers ─────────────────────────────────────────────────────────────

/// Collapse every run of whitespace to a single space and trim.
fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// At most `max` characters, with an ellipsis when something was cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(src: &str) -> Chart {
        charts_of(src).expect("parses").pop().expect("one chart")
    }

    fn seq(f: &Flow) -> &[Flow] {
        match f {
            Flow::Seq(v) => v,
            other => panic!("expected a Seq, got {other:?}"),
        }
    }

    /// The readability rule the whole view depends on: a run of ordinary
    /// statements is ONE box. Five `let`s drawn as five rectangles is what makes
    /// a real function unreadable.
    #[test]
    fn a_run_of_plain_statements_collapses_to_one_box() {
        let c = chart("fn f() { let a = 1; let b = 2; let c = a + b; x = c; }");
        let body = seq(&c.body);
        assert_eq!(body.len(), 1, "expected one folded box, got {body:?}");
        let Flow::Node(n) = &body[0] else {
            panic!("not a node")
        };
        assert_eq!(n.shape, Shape::Process);
        assert_eq!(n.text, "let a = 1;");
        assert_eq!(n.detail.len(), 3, "the other three statements fold in");
    }

    /// An I-O parallelogram must never be swallowed by the run beside it — it
    /// is the thing the reader came for.
    #[test]
    fn an_io_statement_breaks_the_run() {
        let c = chart("fn f() { let a = 1; uart.write(&buf); let b = 2; }");
        let body = seq(&c.body);
        assert_eq!(body.len(), 3, "run / io / run, got {body:?}");
        let Flow::Node(io) = &body[1] else {
            panic!("not a node")
        };
        assert_eq!(io.shape, Shape::Io);
    }

    #[test]
    fn a_print_macro_is_io() {
        let c = chart("fn f() { println!(\"{}\", x); }");
        let Flow::Node(n) = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(n.shape, Shape::Io);
    }

    /// A call to a function of this same file is a subroutine box that knows
    /// where to jump.
    #[test]
    fn a_local_call_becomes_a_clickable_subroutine() {
        let src = "fn helper() {}\nfn f() {\n    helper();\n}\n";
        let c = charts_of(src).unwrap().pop().unwrap();
        let Flow::Node(n) = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(n.shape, Shape::Subroutine);
        assert_eq!(n.goto_line, Some(1), "jumps to `helper`'s own line");
    }

    /// A call to something that is not in this file stays a plain box — the
    /// chart must not offer a jump it cannot make.
    #[test]
    fn an_unknown_call_is_not_a_subroutine() {
        let c = chart("fn f() { some_external_thing(); }");
        let Flow::Node(n) = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(n.shape, Shape::Process);
        assert_eq!(n.goto_line, None);
    }

    /// `if` with no `else` still gets a NO arm — an empty one, which is the
    /// line that goes straight down to the join.
    #[test]
    fn an_if_without_else_has_an_empty_no_arm() {
        let c = chart("fn f() { if x > 3 { go(); } }");
        let Flow::Branch { cond, arms } = &seq(&c.body)[0] else {
            panic!("not a branch")
        };
        assert_eq!(cond.text, "x > 3", "the label is the user's own text");
        assert_eq!(cond.shape, Shape::Decision);
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[1].label, "NO");
        assert!(matches!(&arms[1].body, Flow::Seq(v) if v.is_empty()));
    }

    /// `else if` is a diamond INSIDE the NO arm, not a third arm of the first
    /// diamond — that is what the code actually does.
    #[test]
    fn else_if_nests_in_the_no_arm() {
        let c = chart("fn f() { if a { p(); } else if b { q(); } else { r(); } }");
        let Flow::Branch { arms, .. } = &seq(&c.body)[0] else {
            panic!()
        };
        let Flow::Branch { cond, arms: inner } = &arms[1].body else {
            panic!("the NO arm should hold the second diamond")
        };
        assert_eq!(cond.text, "b");
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn a_match_gets_one_labelled_arm_per_pattern() {
        let src =
            "fn f() { match mode { Mode::Night => a(), Mode::Day | Mode::Dusk => b(), _ => {} } }";
        let c = chart(src);
        let Flow::Branch { cond, arms } = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(cond.text, "match mode");
        let labels: Vec<&str> = arms.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Mode::Night", "Mode::Day | Mode::Dusk", "_"]);
    }

    #[test]
    fn a_match_guard_is_part_of_the_arm_label() {
        let c = chart("fn f() { match n { x if x > 3 => a(), _ => b() } }");
        let Flow::Branch { arms, .. } = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(arms[0].label, "x if x > 3");
    }

    /// Firmware's endless main loop: no path returns, so no END terminal.
    #[test]
    fn an_endless_loop_leaves_the_chart_without_an_end() {
        let c = chart("fn main() { init(); loop { tick(); } }");
        assert!(c.diverges, "nothing after an endless loop can be reached");
    }

    /// The same loop with a way out DOES end — the textbook example's shape.
    #[test]
    fn a_loop_with_a_break_can_reach_the_end() {
        let c = chart("fn main() { loop { x += 1; if x > 20 { break; } } }");
        assert!(!c.diverges);
    }

    /// A `break` in a NESTED loop does not open the outer one.
    #[test]
    fn a_break_in_an_inner_loop_does_not_end_the_outer() {
        let c = chart("fn main() { loop { loop { break; } } }");
        assert!(
            c.diverges,
            "the break belongs to the inner loop; the outer one is still endless"
        );
    }

    /// ...unless it names the outer loop's label.
    #[test]
    fn a_labelled_break_reaches_the_loop_it_names() {
        let c = chart("fn main() { 'outer: loop { loop { break 'outer; } } }");
        assert!(!c.diverges, "break 'outer leaves both loops");
    }

    #[test]
    fn a_return_type_of_never_diverges_even_with_a_reachable_tail() {
        let c = chart("fn main() -> ! { setup(); loop { if done() { break; } } }");
        assert!(c.diverges, "`-> !` is the function's own promise");
    }

    #[test]
    fn a_while_loop_can_always_fall_out() {
        let c = chart("fn f() { while go() { step(); } }");
        assert!(!c.diverges);
    }

    /// `.await` is marked, and it survives folding into a run.
    #[test]
    fn await_is_marked_on_the_box() {
        let c = chart("async fn f() { let x = 1; rx.read(&mut b).await; }");
        let nodes = seq(&c.body);
        let awaited = nodes.iter().any(|f| matches!(f, Flow::Node(n) if n.awaits));
        assert!(awaited, "the await point must be visible: {nodes:?}");
        assert!(c.is_async);
    }

    /// A `?` in a closure is the CLOSURE's early exit, not the function's —
    /// drawing an arrow to this function's end would be wrong.
    #[test]
    fn a_question_mark_inside_a_closure_is_not_this_functions_exit() {
        let c = chart("fn f() { run(|| { inner()?; Ok(()) }); }");
        let Flow::Node(n) = &seq(&c.body)[0] else {
            panic!()
        };
        assert!(!n.try_exit);
    }

    #[test]
    fn a_question_mark_in_the_statement_itself_is_marked() {
        let c = chart("fn f() { let v = thing()?; }");
        let Flow::Node(n) = &seq(&c.body)[0] else {
            panic!()
        };
        assert!(n.try_exit);
    }

    /// The generated init block is ONE dimmed box, however many statements it
    /// holds — it is not the user's algorithm.
    #[test]
    fn the_generated_block_collapses_to_a_single_box() {
        let src = "fn main() {\n\
                   // <<< GENERATED BEGIN — do not edit between these markers >>>\n\
                   let p = init();\n\
                   let mut u = Uart::new(p.U1);\n\
                   let mut s = Spi::new(p.S1);\n\
                   // <<< GENERATED END >>>\n\
                   run();\n\
                   }\n";
        let c = charts_of(src).unwrap().pop().unwrap();
        let body = seq(&c.body);
        assert_eq!(body.len(), 2, "one generated box + the user's code");
        let Flow::Node(g) = &body[0] else { panic!() };
        assert_eq!(g.shape, Shape::Generated);
        assert_eq!(g.detail.len(), 3, "all three init statements fold in");
    }

    /// The folded rows must be the STATEMENTS, not the box's own title. Folding
    /// `text` instead printed "generated setup" once per statement — a box that
    /// says its own name five times and hides what it actually does.
    #[test]
    fn the_generated_box_lists_the_statements_not_its_own_title() {
        let src = "fn main() {
                   // <<< GENERATED BEGIN >>>
                   let p = init();
                   let u = Uart::new(p.U1);
                   // <<< GENERATED END >>>
                   }
";
        let c = charts_of(src).unwrap().pop().unwrap();
        let Flow::Node(g) = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(g.text, "generated setup");
        assert_eq!(g.detail, ["let p = init();", "let u = Uart::new(p.U1);"]);
    }

    /// `loop {}` — the shape of every generated `panic_handler`. With an empty
    /// body the chart drew a back edge from nothing to nothing, which reads as
    /// broken rather than as an idle spin.
    #[test]
    fn an_empty_loop_still_has_something_to_loop_around() {
        let c = chart("fn panic() -> ! { loop {} }");
        let Flow::Loop { body, .. } = &seq(&c.body)[0] else {
            panic!("not a loop")
        };
        let inner = seq(body);
        assert_eq!(inner.len(), 1, "the loop needs a box to circle back to");
        let Flow::Node(n) = &inner[0] else { panic!() };
        assert_eq!(n.text, "loop {}");
        assert!(c.diverges, "and it still never ends");
    }

    #[test]
    fn generated_ranges_reads_the_config_file_spelling_too() {
        let src =
            "// <<< GENERATED>>>\npub const B: u32 = 9600;\n// <<< GENERATED END >>>\nuse x;\n";
        assert_eq!(generated_ranges(src), vec![(1, 3)]);
    }

    /// Entry points are what the chart list leads with, so each attribute shape
    /// the generator emits has to be recognised.
    #[test]
    fn every_generated_entry_attribute_is_recognised() {
        let cases: [(&str, EntryKind); 7] = [
            ("#[entry] fn main() {}", EntryKind::Main),
            (
                "#[embassy_executor::main] async fn main(s: Spawner) {}",
                EntryKind::Main,
            ),
            ("#[esp_rtos::main] async fn main() {}", EntryKind::Main),
            (
                "#[embassy_executor::task] async fn radar() {}",
                EntryKind::Task,
            ),
            ("#[interrupt] fn EXTI0() {}", EntryKind::Interrupt),
            (
                "#[task(binds = EXTI0, local = [led])] fn on_pin(c: on_pin::Context) {}",
                EntryKind::Interrupt,
            ),
            (
                "#[task] fn software(c: software::Context) {}",
                EntryKind::Task,
            ),
        ];
        for (src, want) in cases {
            let c = charts_of(src).unwrap().pop().unwrap();
            assert_eq!(c.kind, want, "for {src}");
        }
    }

    /// RTIC puts the whole application inside `#[rtic::app] mod app { … }`; a
    /// walk that stops at items would find nothing at all there.
    #[test]
    fn rtic_functions_are_found_inside_the_app_module() {
        let src = "#[rtic::app(device = pac)]\n\
                   mod app {\n\
                       #[init]\n\
                       fn init(cx: init::Context) -> (Shared, Local) { boot(); }\n\
                       #[task(binds = EXTI0)]\n\
                       fn on_pin(cx: on_pin::Context) { toggle(); }\n\
                   }\n";
        let charts = charts_of(src).unwrap();
        let names: Vec<&str> = charts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["init", "on_pin"]);
        assert_eq!(charts[1].kind, EntryKind::Interrupt);
    }

    #[test]
    fn impl_methods_are_charted_under_their_type() {
        let src = "struct P;\nimpl P {\n    fn feed(&mut self, b: u8) { self.n += 1; }\n}\n";
        let charts = charts_of(src).unwrap();
        assert_eq!(charts[0].name, "P::feed");
    }

    /// A half-typed file must not blank the tab — the caller keeps the last
    /// good chart and needs the line to say where the trouble is.
    #[test]
    fn a_syntax_error_reports_its_line() {
        let err = charts_of("fn a() {\n    if x {\n").unwrap_err();
        assert_eq!(err.line, 2);
    }

    /// Labels come out of the source verbatim, including operators and calls —
    /// a reconstruction would not match what the user reads in the editor.
    #[test]
    fn a_condition_label_is_the_users_own_text() {
        let c =
            chart("fn f() {\n    if dist < THRESHOLD && is_night() {\n        go();\n    }\n}\n");
        let Flow::Branch { cond, .. } = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(cond.text, "dist < THRESHOLD && is_night()");
        assert_eq!(cond.line, 2, "and it knows the line to jump to");
    }

    #[test]
    fn a_for_loop_reads_as_for_each() {
        let c = chart("fn f() { for byte in buf.iter() { feed(*byte); } }");
        let Flow::Loop { head, .. } = &seq(&c.body)[0] else {
            panic!()
        };
        let LoopHead::For(n) = head else { panic!() };
        assert_eq!(n.text, "for byte in buf.iter()");
    }

    #[test]
    fn long_labels_are_elided_not_wrapped() {
        let long = "a".repeat(200);
        let c = chart(&format!("fn f() {{ if {long} {{ go(); }} }}"));
        let Flow::Branch { cond, .. } = &seq(&c.body)[0] else {
            panic!()
        };
        assert_eq!(cond.text.chars().count(), LABEL_MAX);
        assert!(cond.text.ends_with('…'));
    }
}
