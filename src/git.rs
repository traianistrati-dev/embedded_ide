//! Git integration (bottom-panel "Git" tab + project-tree context menu).
//!
//! Runs the system `git` CLI in the **project directory** (`project_dir`,
//! where Save writes) — never in the `%TEMP%` check workspace. Commits are
//! **strictly what's on disk** (user decision 2026-07-06): the tab shows an
//! amber warning when the in-memory editors differ from the saved files, but
//! never saves on its own.
//!
//! Each operation is a short command sequence run on a worker thread with
//! collected output (`.output()`, no streaming/Stop in v1 — git ops are brief;
//! long pushes just show the spinner). Every run ends with a `git status
//! --porcelain=v2 --branch` refresh plus the unsaved-changes comparison, and
//! logs its phases to the Activity tab. Credentials are the system git's
//! problem (Windows Credential Manager / ssh-agent), not ours.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Max output lines kept in the tab scrollback.
const MAX_LINES: usize = 2_000;

/// One git action, triggered from the tab's buttons or the tree's context
/// menu. Carried as a SIGNAL (like the Clippy tab's `clippy_run`) so the
/// worker spawn happens in `AppIde::run_git_op`, which owns the state.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GitOp {
    Refresh,
    Init,
    Commit,
    CommitPush,
    Push,
    Pull,
    Fetch,
    Log,
    /// `git remote add origin <url>` — the URL comes from the tab's draft
    /// field (like the commit message).
    SetRemote,
}

impl GitOp {
    /// Short label for the status bar / Activity entry.
    pub fn label(self) -> &'static str {
        match self {
            GitOp::Refresh => "status",
            GitOp::Init => "init",
            GitOp::Commit => "commit",
            GitOp::CommitPush => "commit + push",
            GitOp::Push => "push",
            GitOp::Pull => "pull",
            GitOp::Fetch => "fetch",
            GitOp::Log => "log",
            GitOp::SetRemote => "set remote",
        }
    }
}

/// Where an output line came from — tints it in the tab.
#[derive(Clone, Copy, PartialEq)]
pub enum GitLine {
    /// The command being run (echoed with a `> ` prompt).
    Cmd,
    Out,
    Err,
    /// IDE-generated notice (exit code, guidance).
    Notice,
}

/// One changed path from `git status`: the two-letter XY code + path.
#[derive(Clone, Debug, PartialEq)]
pub struct GitChange {
    /// Porcelain XY (e.g. `M.` staged-modified, `.M` unstaged, `??` untracked,
    /// `UU` conflict).
    pub code: String,
    pub path: String,
}

/// Conventional-commit message prefixes offered by the tab's dropdown, each
/// with a one-line tooltip explaining when to use it (best-practice guidance).
pub const COMMIT_TYPES: &[(&str, &str)] = &[
    ("feat:", "A new feature for the user."),
    ("fix:", "A bug fix for the user."),
    ("refactor:", "A code change that neither fixes a bug nor adds a feature."),
    ("perf:", "A change that improves performance."),
    ("docs:", "Documentation-only changes."),
    ("style:", "Formatting / whitespace only — no code-behaviour change."),
    ("test:", "Adding or correcting tests."),
    ("build:", "Changes to the build system or dependencies."),
    ("ci:", "Changes to CI configuration or scripts."),
    ("chore:", "Routine maintenance — no production code change."),
    ("revert:", "Reverts a previous commit."),
];

/// Prepend a conventional-commit `prefix` (e.g. `"feat:"`) to `msg`, replacing
/// any conventional-commit prefix `msg` already starts with (so picking a
/// different type swaps cleanly) and leaving exactly one trailing space.
pub fn apply_commit_prefix(msg: &str, prefix: &str) -> String {
    let mut rest = msg.trim_start();
    for (p, _) in COMMIT_TYPES {
        if let Some(after) = rest.strip_prefix(p) {
            rest = after.trim_start();
            break;
        }
    }
    if rest.is_empty() {
        format!("{prefix} ")
    } else {
        format!("{prefix} {rest}")
    }
}

/// Parsed `git status --porcelain=v2 --branch`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitStatus {
    /// Current branch (`None` = detached HEAD or unborn).
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    /// `false` while HEAD is unborn (`# branch.oid (initial)`) — pushing then
    /// fails with "src refspec HEAD does not match any", so the Push button
    /// stays disabled until the first commit exists.
    pub has_commits: bool,
    pub changes: Vec<GitChange>,
}

/// One row of a parsed unified diff, ready for rendering.
#[derive(Clone, Debug, PartialEq)]
pub enum DiffRow {
    /// Hunk header (`@@ -a,b +c,d @@ …`) — rendered as a separator.
    Hunk(String),
    /// Context line: (old line no, new line no, text).
    Ctx(u32, u32, String),
    /// Removed line: (old line no, text).
    Del(u32, String),
    /// Added line: (new line no, text).
    Add(u32, String),
}

/// One entry of the commit log (History view).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Commit {
    /// Full hash — what every follow-up command is addressed by.
    pub sha: String,
    pub short: String,
    pub author: String,
    /// `--date=short`, i.e. `YYYY-MM-DD`.
    pub date: String,
    pub subject: String,
    /// Decorations (`HEAD -> main`, `tag: v1`), already stripped of brackets.
    pub refs: String,
}

/// One file touched by a commit.
#[derive(Clone, Debug, PartialEq)]
pub struct CommitFile {
    /// git's `--name-status` letter: `A`dded, `M`odified, `D`eleted, `R`enamed…
    pub status: char,
    pub path: String,
}

/// Field separator for the log format — `\x1f` (unit separator) cannot appear
/// in a commit subject or an author name, unlike any printable delimiter.
const LOG_SEP: char = '\u{1f}';

/// The `--pretty=format:` string matching [`parse_log`].
pub const LOG_FORMAT: &str = "%H\u{1f}%h\u{1f}%an\u{1f}%ad\u{1f}%s\u{1f}%D";

/// Parse `git log --pretty=format:LOG_FORMAT --date=short` output.
///
/// Malformed lines are skipped rather than failing the whole log: one odd
/// commit message must not blank the entire History view.
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let f: Vec<&str> = line.split(LOG_SEP).collect();
            if f.len() < 6 {
                return None;
            }
            Some(Commit {
                sha: f[0].to_owned(),
                short: f[1].to_owned(),
                author: f[2].to_owned(),
                date: f[3].to_owned(),
                subject: f[4].to_owned(),
                refs: f[5].to_owned(),
            })
        })
        .collect()
}

/// Parse `git diff-tree --name-status -r` output (`M\tsrc/main.rs`).
///
/// Renames arrive as `R100\told\tnew`; the NEW path is what the user can open,
/// so that is the one kept.
pub fn parse_name_status(text: &str) -> Vec<CommitFile> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?.chars().next()?;
            let first = parts.next()?;
            let path = parts.next().unwrap_or(first);
            Some(CommitFile {
                status,
                path: path.to_owned(),
            })
        })
        .collect()
}

/// A parsed per-file diff (disk vs HEAD) shown in the Git tab's right pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub rows: Vec<DiffRow>,
    pub added: usize,
    pub removed: usize,
}

/// Shared tab state, written by the worker thread.
#[derive(Default)]
pub struct GitState {
    pub lines: Vec<(GitLine, String)>,
    /// `Some(op label)` while a worker is running (disables the buttons and
    /// shows in the status bar).
    pub busy: Option<&'static str>,
    /// A status refresh has completed at least once (gates the auto-refresh
    /// the tab fires on first open).
    pub loaded: bool,
    /// `false` when the project dir isn't a git repository (shows Init).
    pub is_repo: bool,
    /// `git` wasn't found on PATH at all.
    pub git_missing: bool,
    pub status: GitStatus,
    /// Project-relative paths whose IN-MEMORY content differs from disk —
    /// the "unsaved changes" warning (commit uses only what's on disk).
    pub unsaved: Vec<String>,
    /// Set when a commit succeeds; the tab consumes it to clear the message.
    pub commit_succeeded: bool,
    /// The diff currently open in the tab's right pane (`None` → the output
    /// scrollback shows instead). Cleared when any operation runs — a commit
    /// or pull makes the open diff stale.
    pub diff: Option<FileDiff>,
    /// Commit log for the History view, newest first.
    pub log: Vec<Commit>,
    /// Files touched by `commit_files_sha`.
    pub commit_files: Vec<CommitFile>,
    /// Which commit `commit_files` belongs to — guards against showing one
    /// commit's file list next to another's diff while a load is in flight.
    pub commit_files_sha: String,
    /// Bumped after every operation that can move HEAD or rewrite the worktree
    /// (commit / pull / init) — the editor's gutter-diff baseline cache keys on
    /// it, so marks refresh right after a commit.
    pub op_gen: u64,
    /// `origin`'s URL (`git remote get-url origin`), refreshed with the
    /// status. `None` → the tab offers the "Set remote" field instead of Push.
    pub remote_url: Option<String>,
}

impl GitState {
    fn push(&mut self, kind: GitLine, line: impl Into<String>) {
        self.lines.push((kind, line.into()));
        if self.lines.len() > MAX_LINES {
            let excess = self.lines.len() - MAX_LINES;
            self.lines.drain(..excess);
        }
    }
}

/// UI-side handle: shared state + the commit-message and remote-URL drafts.
/// Which half of the Git tab is showing.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum GitView {
    /// Working-tree changes: commit box, file checkboxes, hunk revert.
    #[default]
    Changes,
    /// Commit log — strictly READ-ONLY (log / diff-tree / show).
    History,
}

pub struct GitConsole {
    pub state: Arc<Mutex<GitState>>,
    /// Changes vs History.
    pub view: GitView,
    /// Selected commit in the History list, by full sha.
    pub selected_commit: Option<String>,
    pub commit_msg: String,
    /// Draft for the "Set remote origin" field (shown while no remote exists).
    pub remote_url_draft: String,
    /// Changed-file paths UNchecked in the tab — excluded from the next
    /// commit. Stored inverted so freshly appearing changes default to
    /// checked (commit-everything stays the no-touch default).
    pub excluded: std::collections::HashSet<String>,
}

impl Default for GitConsole {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(GitState::default())),
            view: GitView::default(),
            selected_commit: None,
            commit_msg: String::new(),
            remote_url_draft: String::new(),
            excluded: std::collections::HashSet::new(),
        }
    }
}

impl GitConsole {
    pub fn is_busy(&self) -> bool {
        self.state.lock().unwrap().busy.is_some()
    }
}

/// Parse `git status --porcelain=v2 --branch` output. Pure — tested below.
pub fn parse_porcelain_v2(text: &str) -> GitStatus {
    let mut st = GitStatus::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            st.has_commits = rest.trim() != "(initial)";
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            let name = rest.trim();
            st.branch = (name != "(detached)").then(|| name.to_owned());
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            st.upstream = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // "+A -B"
            for part in rest.split_whitespace() {
                if let Some(a) = part.strip_prefix('+') {
                    st.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = part.strip_prefix('-') {
                    st.behind = b.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("1 ").or_else(|| line.strip_prefix("2 ")) {
            // `1 XY sub mH mI mW hH hI path` (ordinary) or
            // `2 XY sub mH mI mW hH hI Xscore path\torig` (rename/copy).
            let mut it = rest.splitn(8, ' ');
            let xy = it.next().unwrap_or("??").to_owned();
            let tail = it.nth(6).unwrap_or(""); // 8th field onward = path…
            // Rename entries carry `Xscore path\torig` — drop score + orig.
            let path = if line.starts_with("2 ") {
                let after_score = tail.splitn(2, ' ').nth(1).unwrap_or(tail);
                after_score.split('\t').next().unwrap_or(after_score)
            } else {
                tail
            };
            if !path.is_empty() {
                st.changes.push(GitChange { code: xy, path: path.to_owned() });
            }
        } else if let Some(rest) = line.strip_prefix("u ") {
            // Unmerged (conflict): `u XY sub m1 m2 m3 mW h1 h2 h3 path`.
            let xy = rest.split(' ').next().unwrap_or("UU").to_owned();
            if let Some(path) = rest.splitn(11, ' ').nth(9) {
                st.changes.push(GitChange { code: xy, path: path.to_owned() });
            }
        } else if let Some(path) = line.strip_prefix("? ") {
            st.changes.push(GitChange { code: "??".into(), path: path.to_owned() });
        }
        // `! ignored` and `# branch.oid` are skipped.
    }
    st
}

/// The command sequence one [`GitOp`] runs (before the always-appended status
/// refresh). `msg` is the commit message, `remote` the Set-remote URL draft.
/// `has_upstream = false` turns pushes into `push -u origin HEAD` — the first
/// push of a branch sets its upstream, so later ones are a plain `push`.
/// `add_paths`: `None` = stage everything (`add -A`); `Some(paths)` = stage
/// only the CHECKED files (`add -A -- <paths>` — `-A` also stages deletions
/// matching those paths).
fn commands_for(
    op: GitOp,
    msg: &str,
    remote: &str,
    has_upstream: bool,
    add_paths: &Option<Vec<String>>,
) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let push = if has_upstream {
        s(&["push"])
    } else {
        s(&["push", "-u", "origin", "HEAD"])
    };
    let add = match add_paths {
        None => s(&["add", "-A"]),
        Some(paths) => {
            let mut v = s(&["add", "-A", "--"]);
            v.extend(paths.iter().cloned());
            v
        }
    };
    match op {
        GitOp::Refresh => vec![],
        GitOp::Init => vec![s(&["init"])],
        GitOp::Commit => vec![add, vec!["commit".into(), "-m".into(), msg.to_owned()]],
        GitOp::CommitPush => vec![
            add,
            vec!["commit".into(), "-m".into(), msg.to_owned()],
            push,
        ],
        GitOp::Push => vec![push],
        GitOp::Pull => vec![s(&["pull"])],
        GitOp::Fetch => vec![s(&["fetch"])],
        // Structured for the History view — parsed by `parse_log`, not dumped
        // into the console. `--date=short` keeps the column narrow.
        GitOp::Log => vec![s(&[
            "log",
            "--no-color",
            &format!("--pretty=format:{LOG_FORMAT}"),
            "--date=short",
            "-n",
            "200",
        ])],
        GitOp::SetRemote => vec![vec!["remote".into(), "add".into(), "origin".into(), remote.to_owned()]],
    }
}

/// Run one git command in `dir`, collect its output. `Err` = couldn't launch.
fn run_git(dir: &Path, args: &[String]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("git");
    crate::build::no_window(&mut cmd)
        .args(args)
        .current_dir(dir)
        // We run with stdin closed and no console — a credential/hostkey
        // TERMINAL prompt would hang forever. Fail fast instead; Git
        // Credential Manager still works (it spawns its own UI window).
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    cmd.output()
}

/// Append a command's output lines to the state.
fn push_output(st: &mut GitState, out: &std::process::Output) {
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        st.push(GitLine::Out, line);
    }
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        st.push(GitLine::Err, line);
    }
}

/// Spawn the worker for `op`. `snapshot` is the in-memory project content
/// (project-relative path → content) used for the unsaved-changes comparison
/// against disk — computed here on the worker, never on the UI thread.
pub fn run_op(
    op: GitOp,
    msg: String,
    remote: String,
    add_paths: Option<Vec<String>>,
    project_dir: PathBuf,
    snapshot: Vec<(String, String)>,
    state: Arc<Mutex<GitState>>,
    activity: Arc<Mutex<crate::activity::ActivityLog>>,
    ctx: egui::Context,
) {
    {
        let mut st = state.lock().unwrap();
        if st.busy.is_some() {
            return; // one op at a time
        }
        st.busy = Some(op.label());
        // Any operation invalidates an open diff (commit/pull change HEAD or
        // the worktree; even a refresh means the user moved on).
        st.diff = None;
    }

    std::thread::spawn(move || {
        let mut rec = crate::activity::Recorder::new(format!("Git ({})", op.label()));
        let mut sequence_ok = true;

        // First push of a branch needs `-u origin HEAD` to create its
        // upstream; once one exists, a plain `push` suffices.
        let has_upstream = state.lock().unwrap().status.upstream.is_some();
        for args in commands_for(op, &msg, &remote, has_upstream, &add_paths) {
            let shown = format!("> git {}", args.join(" "));
            state.lock().unwrap().push(GitLine::Cmd, shown.clone());
            let t = std::time::Instant::now();
            match run_git(&project_dir, &args) {
                Ok(out) => {
                    let mut st = state.lock().unwrap();
                    // The log is DATA, not console output: parsed into the
                    // History view instead of flooding the scrollback with 200
                    // separator-delimited lines.
                    if op == GitOp::Log && out.status.success() {
                        st.log = parse_log(&String::from_utf8_lossy(&out.stdout));
                        st.commit_files.clear();
                        st.commit_files_sha.clear();
                        let n = st.log.len();
                        st.push(GitLine::Notice, format!("[info] loaded {n} commit(s)"));
                    } else {
                        push_output(&mut st, &out);
                    }
                    let code = out.status.code();
                    rec.cmd_phase(
                        format!("git {}", args.first().map(String::as_str).unwrap_or("")),
                        shown.trim_start_matches("> ").to_owned(),
                        t.elapsed(),
                        code,
                    );
                    if !out.status.success() {
                        st.push(
                            GitLine::Notice,
                            format!("[exit {}] — sequence stopped", code.unwrap_or(-1)),
                        );
                        sequence_ok = false;
                        break;
                    }
                    if args.first().map(String::as_str) == Some("commit") {
                        st.commit_succeeded = true;
                    }
                }
                Err(e) => {
                    let missing = e.kind() == std::io::ErrorKind::NotFound;
                    let note = if missing {
                        "[error] `git` not found on PATH — install from https://git-scm.com".to_owned()
                    } else {
                        format!("[error] couldn't launch git: {e}")
                    };
                    let mut st = state.lock().unwrap();
                    st.git_missing = missing;
                    st.push(GitLine::Notice, note);
                    sequence_ok = false;
                    break;
                }
            }
            ctx.request_repaint();
        }

        // ── Always refresh the status + the unsaved comparison ────────────
        // `--untracked-files=all`: by default git COLLAPSES an untracked
        // directory into one `? dir/` entry — a fresh (never-committed)
        // project showed a single "src/" row instead of its files (reported
        // as "nu se văd fișierele din src").
        let t = std::time::Instant::now();
        match run_git(
            &project_dir,
            &[
                "status".into(),
                "--porcelain=v2".into(),
                "--branch".into(),
                "--untracked-files=all".into(),
            ],
        ) {
            Ok(out) => {
                let parsed = out
                    .status
                    .success()
                    .then(|| parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout)));
                let mut st = state.lock().unwrap();
                st.is_repo = out.status.success();
                if let Some(p) = parsed {
                    st.status = p;
                } else {
                    st.status = GitStatus::default();
                    if op != GitOp::Init {
                        st.push(
                            GitLine::Notice,
                            "not a git repository — use Init to create one",
                        );
                    }
                }
                rec.add("status refresh", t.elapsed());
            }
            Err(e) => {
                let mut st = state.lock().unwrap();
                st.git_missing = e.kind() == std::io::ErrorKind::NotFound;
                st.is_repo = false;
            }
        }
        // Which remote is configured (drives the tab's "Set remote" field).
        let remote_url = run_git(&project_dir, &["remote".into(), "get-url".into(), "origin".into()])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|u| !u.is_empty());

        let unsaved = unsaved_changes(&project_dir, &snapshot);
        {
            let mut st = state.lock().unwrap();
            st.remote_url = remote_url;
            st.unsaved = unsaved;
            st.loaded = true;
            st.busy = None;
            // Commit / pull / init move HEAD or rewrite the worktree — the
            // editor's gutter-diff baseline must refetch.
            if matches!(op, GitOp::Commit | GitOp::CommitPush | GitOp::Pull | GitOp::Init) {
                st.op_gen += 1;
            }
        }
        let _ = sequence_ok;
        activity.lock().unwrap().push(rec.finish());
        ctx.request_repaint();
    });
}

/// Parse `git diff --no-color` unified output into renderable rows. Content
/// lines are only interpreted INSIDE a hunk (after the first `@@`), so the
/// `---`/`+++` file headers can't masquerade as removals/additions. The
/// `\ No newline at end of file` marker is dropped. Pure — tested below.
pub fn parse_unified_diff(text: &str) -> (Vec<DiffRow>, usize, usize) {
    let mut rows = Vec::new();
    let (mut added, mut removed) = (0usize, 0usize);
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_hunk = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            // `@@ -a[,b] +c[,d] @@ …` — read the two start numbers.
            in_hunk = true;
            for part in rest.split_whitespace() {
                if let Some(a) = part.strip_prefix('-') {
                    old_no = a.split(',').next().and_then(|n| n.parse().ok()).unwrap_or(1);
                } else if let Some(c) = part.strip_prefix('+') {
                    new_no = c.split(',').next().and_then(|n| n.parse().ok()).unwrap_or(1);
                }
            }
            rows.push(DiffRow::Hunk(line.to_owned()));
            continue;
        }
        if !in_hunk {
            continue; // diff --git / index / --- / +++ headers
        }
        if let Some(t) = line.strip_prefix('+') {
            rows.push(DiffRow::Add(new_no, t.to_owned()));
            new_no += 1;
            added += 1;
        } else if let Some(t) = line.strip_prefix('-') {
            rows.push(DiffRow::Del(old_no, t.to_owned()));
            old_no += 1;
            removed += 1;
        } else if let Some(t) = line.strip_prefix(' ') {
            rows.push(DiffRow::Ctx(old_no, new_no, t.to_owned()));
            old_no += 1;
            new_no += 1;
        }
        // `\ No newline at end of file` (and anything else) — skipped.
    }
    (rows, added, removed)
}

/// Reconstruct a single-hunk unified-diff patch (revertible with `git apply
/// --reverse`) from a [`FileDiff`]'s rows. `hunk_row` is the index of the
/// [`DiffRow::Hunk`] that starts the hunk; the body runs to the next hunk (or
/// the end). `None` if `hunk_row` isn't a hunk header. Pure — tested below.
pub fn hunk_patch(path: &str, rows: &[DiffRow], hunk_row: usize) -> Option<String> {
    let DiffRow::Hunk(header) = rows.get(hunk_row)? else {
        return None;
    };
    let mut body = String::new();
    for row in &rows[hunk_row + 1..] {
        match row {
            DiffRow::Hunk(_) => break, // next hunk starts here
            DiffRow::Ctx(_, _, t) => {
                body.push(' ');
                body.push_str(t);
                body.push('\n');
            }
            DiffRow::Del(_, t) => {
                body.push('-');
                body.push_str(t);
                body.push('\n');
            }
            DiffRow::Add(_, t) => {
                body.push('+');
                body.push_str(t);
                body.push('\n');
            }
        }
    }
    Some(format!("--- a/{path}\n+++ b/{path}\n{header}\n{body}"))
}

/// Apply `patch` in REVERSE to the working tree in `dir` — undo exactly that
/// hunk (`git apply --reverse --recount`). `--recount` lets git recompute the
/// `@@` line counts, so a faithfully-reconstructed body applies even if the
/// header counts drift. The patch is written to a temp file (git apply reads a
/// path — avoids stdin plumbing). Synchronous; a fast local operation.
pub fn apply_reverse_patch(dir: &Path, patch: &str) -> Result<(), String> {
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("eide_revert_{}_{salt}.patch", std::process::id()));
    std::fs::write(&tmp, patch).map_err(|e| format!("temp patch write failed: {e}"))?;
    let out = run_git(
        dir,
        &[
            "apply".into(),
            "--reverse".into(),
            "--recount".into(),
            tmp.to_string_lossy().into_owned(),
        ],
    );
    let _ = std::fs::remove_file(&tmp);
    let out = out.map_err(|e| format!("couldn't run git apply: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Restore one TRACKED file to its HEAD version on disk — discard its
/// working-tree changes (Phase A). Returns that content so the caller can
/// refresh the file's in-memory buffer to match. `git show HEAD:<path>`
/// (LF-normalised, like [`fetch_baseline`]) → write. Synchronous; fast/local.
pub fn restore_file_to_head(dir: &Path, path: &str) -> Result<String, String> {
    let out = run_git(dir, &["show".into(), format!("HEAD:{path}")])
        .map_err(|e| format!("couldn't run git show: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let content = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    std::fs::write(dir.join(path), &content).map_err(|e| format!("write failed: {e}"))?;
    Ok(content)
}

/// Discard ALL working-tree changes back to HEAD (Phase C): `git reset --hard
/// HEAD` (tracked, staged + unstaged) then `git clean -f -d` (untracked files +
/// dirs; `.gitignore`'d paths like `target/` are kept — no `-x`). Synchronous.
/// Only sensible with a committed HEAD; callers gate on `has_commits`.
pub fn discard_all_to_head(dir: &Path) -> Result<(), String> {
    for args in [
        vec!["reset".to_string(), "--hard".into(), "HEAD".into()],
        vec!["clean".into(), "-f".into(), "-d".into()],
    ] {
        let out = run_git(dir, &args).map_err(|e| format!("couldn't run git: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    }
    Ok(())
}

/// An all-added [`FileDiff`] for an UNTRACKED file (`git diff HEAD` doesn't
/// show those) from its on-disk content.
fn synthesized_added(path: &str, content: &str) -> FileDiff {
    let rows: Vec<DiffRow> = content
        .lines()
        .enumerate()
        .map(|(i, l)| DiffRow::Add(i as u32 + 1, l.to_owned()))
        .collect();
    let added = rows.len();
    FileDiff { path: path.to_owned(), rows, added, removed: 0 }
}

/// Open the diff (disk vs HEAD — exactly what a commit would record) for one
/// changed file in the tab's right pane. `untracked` files get a synthesized
/// all-added view. Runs on a worker; called DIRECTLY by the tab (unlike
/// [`GitOp`]s it needs no snapshot from the app).
pub fn run_diff(
    path: String,
    untracked: bool,
    project_dir: PathBuf,
    state: Arc<Mutex<GitState>>,
    ctx: egui::Context,
) {
    {
        let mut st = state.lock().unwrap();
        if st.busy.is_some() {
            return;
        }
        st.busy = Some("diff");
    }
    std::thread::spawn(move || {
        let diff = if untracked {
            std::fs::read_to_string(project_dir.join(&path))
                .ok()
                .map(|content| synthesized_added(&path, &content))
        } else {
            run_git(
                &project_dir,
                &[
                    "diff".into(),
                    "HEAD".into(),
                    "--no-color".into(),
                    "--".into(),
                    path.clone(),
                ],
            )
            .ok()
            .filter(|out| out.status.success())
            .map(|out| {
                let (rows, added, removed) =
                    parse_unified_diff(&String::from_utf8_lossy(&out.stdout));
                FileDiff { path: path.clone(), rows, added, removed }
            })
        };
        let mut st = state.lock().unwrap();
        match diff {
            Some(d) if d.rows.is_empty() => {
                st.push(GitLine::Notice, format!("no differences vs HEAD for {path}"));
                st.diff = None;
            }
            Some(d) => st.diff = Some(d),
            None => st.push(GitLine::Notice, format!("[error] couldn't diff {path}")),
        }
        st.busy = None;
        drop(st);
        ctx.request_repaint();
    });
}

// ── Editor gutter diff (in-memory text vs HEAD) ──────────────────────────────
// Unlike the tab's diff viewer (disk vs HEAD via `git diff`), the gutter marks
// compare the LIVE editor text — including unsaved edits — so the baseline
// comes from `git show HEAD:<path>` and the line diff runs in-process.

/// One contiguous change between the HEAD baseline and the current editor
/// text, in LINE coordinates. `new_len == 0` → pure deletion (marker between
/// lines); `old_len == 0` → pure addition; both > 0 → modified lines.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
}

/// Load the file list of `sha` into `state.commit_files` (History view).
///
/// `--root` matters: without it the INITIAL commit reports no files at all,
/// because it has no parent to diff against.
pub fn run_commit_files(
    sha: String,
    project_dir: PathBuf,
    state: Arc<Mutex<GitState>>,
    ctx: egui::Context,
) {
    {
        let mut st = state.lock().unwrap();
        if st.busy.is_some() {
            return;
        }
        st.busy = Some("show");
        st.diff = None;
    }
    std::thread::spawn(move || {
        let out = run_git(
            &project_dir,
            &[
                "diff-tree".into(),
                "--no-commit-id".into(),
                "--name-status".into(),
                "-r".into(),
                "--root".into(),
                "--first-parent".into(),
                sha.clone(),
            ],
        );
        let mut st = state.lock().unwrap();
        match out {
            Ok(o) if o.status.success() => {
                st.commit_files = parse_name_status(&String::from_utf8_lossy(&o.stdout));
                st.commit_files_sha = sha;
                if st.commit_files.is_empty() {
                    // A merge shows nothing against its first parent.
                    st.push(
                        GitLine::Notice,
                        "[info] no file changes against the first parent (merge commit?)".to_owned(),
                    );
                }
            }
            _ => st.push(GitLine::Notice, format!("[error] couldn't read commit {sha}")),
        }
        st.busy = None;
        drop(st);
        ctx.request_repaint();
    });
}

/// Load one file's diff AS OF `sha` into `state.diff` (History view).
///
/// Read-only by construction: `git show` never touches the worktree. The tab
/// hides the hunk-revert buttons for this diff — they reverse-patch the CURRENT
/// files, which is not what "undo part of an old commit" would mean.
pub fn run_commit_file_diff(
    sha: String,
    path: String,
    project_dir: PathBuf,
    state: Arc<Mutex<GitState>>,
    ctx: egui::Context,
) {
    {
        let mut st = state.lock().unwrap();
        if st.busy.is_some() {
            return;
        }
        st.busy = Some("show");
    }
    std::thread::spawn(move || {
        let out = run_git(
            &project_dir,
            &[
                "show".into(),
                "--no-color".into(),
                "--first-parent".into(),
                "--format=".into(), // suppress the commit header — diff only
                sha.clone(),
                "--".into(),
                path.clone(),
            ],
        );
        let mut st = state.lock().unwrap();
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                let (rows, added, removed) = parse_unified_diff(&text);
                if rows.is_empty() {
                    st.push(
                        GitLine::Notice,
                        format!("[info] no textual diff for {path} (binary file?)"),
                    );
                    st.diff = None;
                } else {
                    st.diff = Some(FileDiff { path, rows, added, removed });
                }
            }
            _ => st.push(GitLine::Notice, format!("[error] couldn't diff {path} at {sha}")),
        }
        st.busy = None;
        drop(st);
        ctx.request_repaint();
    });
}

/// Line-diff `old` → `new` into gutter hunks. Pure — tested below.
pub fn compute_hunks(old: &str, new: &str) -> Vec<DiffHunk> {
    use similar::DiffOp;
    similar::TextDiff::from_lines(old, new)
        .ops()
        .iter()
        .filter_map(|op| match *op {
            DiffOp::Equal { .. } => None,
            DiffOp::Delete { old_index, old_len, new_index } => Some(DiffHunk {
                old_start: old_index,
                old_len,
                new_start: new_index,
                new_len: 0,
            }),
            DiffOp::Insert { old_index, new_index, new_len } => Some(DiffHunk {
                old_start: old_index,
                old_len: 0,
                new_start: new_index,
                new_len,
            }),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => Some(DiffHunk {
                old_start: old_index,
                old_len,
                new_start: new_index,
                new_len,
            }),
        })
        .collect()
}

/// Replace hunk `h`'s lines in `current` with the corresponding `baseline`
/// lines — the "Revert hunk" action. Reverting a deletion (`new_len == 0`)
/// re-inserts the removed lines. Pure — tested below.
pub fn revert_hunk(current: &str, baseline: &str, h: &DiffHunk) -> String {
    let cur: Vec<&str> = current.split_inclusive('\n').collect();
    let old: Vec<&str> = baseline.split_inclusive('\n').collect();
    let mut out = String::with_capacity(current.len());
    for l in cur.iter().take(h.new_start) {
        out.push_str(l);
    }
    // Re-inserting after a final line that lacks its `\n` would glue the
    // baseline lines onto it — restore the separator first.
    if h.old_len > 0 && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for l in old.iter().skip(h.old_start).take(h.old_len) {
        out.push_str(l);
    }
    for l in cur.iter().skip(h.new_start + h.new_len) {
        out.push_str(l);
    }
    out
}

/// Shared slot for the gutter's HEAD baseline, filled by [`fetch_baseline`].
#[derive(Default)]
pub struct BaselineFetch {
    /// `(git path, GitState::op_gen)` this slot holds / is loading.
    pub key: (String, u64),
    /// The fetch finished (content may still be `None` — untracked file, no
    /// repo, or no HEAD yet → no gutter marks).
    pub done: bool,
    pub content: Option<String>,
    /// Hash of `content` (0 when `None`) — spares per-frame re-hashing.
    pub content_hash: u64,
}

/// Fetch `git show HEAD:<path>` on a worker into `slot`. CRLF is normalised so
/// a repo checked out/committed with CRLF doesn't flag every line as modified
/// against the editor's LF text.
pub fn fetch_baseline(
    key: (String, u64),
    project_dir: PathBuf,
    slot: Arc<Mutex<BaselineFetch>>,
    ctx: egui::Context,
) {
    {
        let mut s = slot.lock().unwrap();
        s.key = key.clone();
        s.done = false;
        s.content = None;
        s.content_hash = 0;
    }
    std::thread::spawn(move || {
        let content = run_git(&project_dir, &["show".into(), format!("HEAD:{}", key.0)])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n"));
        let mut s = slot.lock().unwrap();
        if s.key == key {
            s.content_hash = content
                .as_ref()
                .map(|c| {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    c.hash(&mut h);
                    h.finish().max(1)
                })
                .unwrap_or(0);
            s.content = content;
            s.done = true;
        }
        drop(s);
        ctx.request_repaint();
    });
}

/// Project-relative paths whose in-memory `snapshot` content differs from the
/// file on disk (or the file is missing) — i.e. edits a commit would MISS.
/// `pub(crate)` so the exit prompt can ask the same question.
pub(crate) fn unsaved_changes(project_dir: &Path, snapshot: &[(String, String)]) -> Vec<String> {
    snapshot
        .iter()
        .filter(|(rel, content)| {
            std::fs::read_to_string(project_dir.join(rel))
                .map(|disk| disk != *content)
                .unwrap_or(true)
        })
        .map(|(rel, _)| rel.clone())
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_branch_header_parses() {
        let out = "\
# branch.oid 7c46695deadbeef
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
";
        let st = parse_porcelain_v2(out);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!(st.ahead, 2);
        assert_eq!(st.behind, 1);
        assert!(st.changes.is_empty());
    }

    #[test]
    fn porcelain_changes_parse() {
        let out = "\
# branch.head main
1 .M N... 100644 100644 100644 abc def src/main.rs
1 M. N... 100644 100644 100644 abc def Cargo.toml
? src/new_file.rs
";
        let st = parse_porcelain_v2(out);
        assert_eq!(
            st.changes,
            vec![
                GitChange { code: ".M".into(), path: "src/main.rs".into() },
                GitChange { code: "M.".into(), path: "Cargo.toml".into() },
                GitChange { code: "??".into(), path: "src/new_file.rs".into() },
            ]
        );
    }

    #[test]
    fn porcelain_rename_keeps_new_path_only() {
        let out = "2 R. N... 100644 100644 100644 abc def R100 src/new.rs\tsrc/old.rs\n";
        let st = parse_porcelain_v2(out);
        assert_eq!(st.changes.len(), 1);
        assert_eq!(st.changes[0].path, "src/new.rs");
        assert_eq!(st.changes[0].code, "R.");
    }

    #[test]
    fn porcelain_detached_head_has_no_branch() {
        let st = parse_porcelain_v2("# branch.head (detached)\n");
        assert_eq!(st.branch, None);
    }

    #[test]
    fn commit_sequence_is_add_commit() {
        let cmds = commands_for(GitOp::Commit, "msg with spaces", "", true, &None);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], vec!["add", "-A"]);
        assert_eq!(cmds[1], vec!["commit", "-m", "msg with spaces"]);
        // Refresh runs no commands — only the always-on status refresh.
        assert!(commands_for(GitOp::Refresh, "", "", true, &None).is_empty());
    }

    #[test]
    fn checked_files_stage_selectively() {
        // Checkbox selection → `add -A -- <paths>` stages only those (incl.
        // deletions matching them); everything-checked keeps plain `add -A`.
        let picked = Some(vec!["src/main.rs".to_owned(), "Cargo.toml".to_owned()]);
        let cmds = commands_for(GitOp::Commit, "m", "", true, &picked);
        assert_eq!(cmds[0], vec!["add", "-A", "--", "src/main.rs", "Cargo.toml"]);
        assert_eq!(cmds[1], vec!["commit", "-m", "m"]);
    }

    #[test]
    fn first_push_sets_upstream_later_pushes_are_plain() {
        // No upstream yet → the push must create it (`-u origin HEAD`).
        assert_eq!(
            commands_for(GitOp::Push, "", "", false, &None),
            vec![vec!["push", "-u", "origin", "HEAD"]]
        );
        assert_eq!(commands_for(GitOp::Push, "", "", true, &None), vec![vec!["push"]]);
        // Commit & Push uses the same upstream-aware push as its last step.
        let cmds = commands_for(GitOp::CommitPush, "m", "", false, &None);
        assert_eq!(cmds[2], vec!["push", "-u", "origin", "HEAD"]);
    }

    #[test]
    fn set_remote_adds_origin_with_the_draft_url() {
        assert_eq!(
            commands_for(GitOp::SetRemote, "", "https://github.com/u/r.git", true, &None),
            vec![vec!["remote", "add", "origin", "https://github.com/u/r.git"]]
        );
    }

    #[test]
    fn commit_prefix_prepends_and_swaps() {
        // Empty message → just the prefix + space.
        assert_eq!(apply_commit_prefix("", "feat:"), "feat: ");
        // Plain text → prefix prepended.
        assert_eq!(apply_commit_prefix("add blinker", "feat:"), "feat: add blinker");
        // Existing conventional prefix is REPLACED, not stacked.
        assert_eq!(apply_commit_prefix("feat: add x", "fix:"), "fix: add x");
        // Leading whitespace + existing prefix are stripped; the rest is kept
        // verbatim (only the front is trimmed).
        assert_eq!(apply_commit_prefix("  refactor:  tidy ", "chore:"), "chore: tidy ");
        // A non-prefix colon word is left alone.
        assert_eq!(apply_commit_prefix("note: hi", "feat:"), "feat: note: hi");
    }

    #[test]
    fn unborn_head_has_no_commits() {
        // The user's exact failure: Push on a fresh repo (no commits) died
        // with "src refspec HEAD does not match any" — the UI now disables
        // Push until `has_commits`.
        let st = parse_porcelain_v2("# branch.oid (initial)\n# branch.head master\n");
        assert!(!st.has_commits);
        let st = parse_porcelain_v2("# branch.oid 7c46695deadbeef\n# branch.head main\n");
        assert!(st.has_commits);
    }

    /// The History view parses the log instead of showing it raw. The fields are
    /// `\x1f`-separated precisely so a subject containing spaces, tabs or pipes
    /// cannot break the columns.
    #[test]
    fn log_lines_parse_into_commits() {
        let text = "abc123\u{1f}abc\u{1f}Ana\u{1f}2026-07-20\u{1f}feat: add radar\u{1f}HEAD -> main\n\
                    def456\u{1f}def\u{1f}Bogdan\u{1f}2026-07-19\u{1f}fix: off by one\u{1f}\n";
        let log = parse_log(text);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].sha, "abc123");
        assert_eq!(log[0].subject, "feat: add radar");
        assert_eq!(log[0].refs, "HEAD -> main");
        assert_eq!(log[1].author, "Bogdan");
        assert_eq!(log[1].refs, "", "an undecorated commit has no refs");
    }

    /// A subject with the delimiters people actually type must survive intact.
    #[test]
    fn a_subject_with_tabs_and_pipes_survives() {
        let text = "a\u{1f}a\u{1f}A\u{1f}2026-01-01\u{1f}fix: a\tb | c\u{1f}\n";
        assert_eq!(parse_log(text)[0].subject, "fix: a\tb | c");
    }

    /// One malformed line must not blank the whole History view.
    #[test]
    fn a_malformed_log_line_is_skipped_not_fatal() {
        let text = "garbage\nabc\u{1f}abc\u{1f}A\u{1f}2026-01-01\u{1f}s\u{1f}\n";
        assert_eq!(parse_log(text).len(), 1);
    }

    #[test]
    fn name_status_keeps_the_new_path_of_a_rename() {
        let text = "M\tsrc/main.rs\nA\tsrc/new.rs\nD\tsrc/old.rs\nR100\tsrc/a.rs\tsrc/b.rs\n";
        let files = parse_name_status(text);
        assert_eq!(files.len(), 4);
        assert_eq!(
            files[0],
            CommitFile {
                status: 'M',
                path: "src/main.rs".into()
            }
        );
        assert_eq!(files[2].status, 'D');
        assert_eq!(
            files[3],
            CommitFile {
                status: 'R',
                path: "src/b.rs".into()
            },
            "a rename is listed under the path you can still open"
        );
    }

    #[test]
    fn unified_diff_parses_rows_and_line_numbers() {
        let text = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,4 @@ fn main() {
 ctx line
-old line
+new line
+extra line
\\ No newline at end of file
";
        let (rows, added, removed) = parse_unified_diff(text);
        assert_eq!(added, 2);
        assert_eq!(removed, 1);
        assert_eq!(
            rows,
            vec![
                DiffRow::Hunk("@@ -10,3 +10,4 @@ fn main() {".into()),
                DiffRow::Ctx(10, 10, "ctx line".into()),
                DiffRow::Del(11, "old line".into()),
                DiffRow::Add(11, "new line".into()),
                DiffRow::Add(12, "extra line".into()),
            ]
        );
    }

    #[test]
    fn unified_diff_headers_never_count_as_changes() {
        // `---`/`+++` appear BEFORE any hunk — they must not parse as Del/Add.
        let text = "--- a/x\n+++ b/x\n";
        let (rows, added, removed) = parse_unified_diff(text);
        assert!(rows.is_empty());
        assert_eq!((added, removed), (0, 0));
        // Empty input → empty diff.
        assert!(parse_unified_diff("").0.is_empty());
    }

    #[test]
    fn unified_diff_multiple_hunks_reset_numbers() {
        let text = "\
@@ -1,1 +1,1 @@
-a
+A
@@ -50,1 +50,1 @@
 same
";
        let (rows, ..) = parse_unified_diff(text);
        assert_eq!(rows[1], DiffRow::Del(1, "a".into()));
        assert_eq!(rows[2], DiffRow::Add(1, "A".into()));
        assert_eq!(rows[4], DiffRow::Ctx(50, 50, "same".into()));
    }

    #[test]
    fn hunk_patch_reconstructs_a_single_hunk() {
        let rows = vec![
            DiffRow::Hunk("@@ -10,3 +10,4 @@ fn main() {".into()),
            DiffRow::Ctx(10, 10, "ctx line".into()),
            DiffRow::Del(11, "old line".into()),
            DiffRow::Add(11, "new line".into()),
            DiffRow::Add(12, "extra line".into()),
            DiffRow::Hunk("@@ -50,1 +51,1 @@".into()),
            DiffRow::Del(50, "x".into()),
            DiffRow::Add(51, "y".into()),
        ];
        let p = hunk_patch("src/main.rs", &rows, 0).unwrap();
        assert!(p.starts_with(
            "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -10,3 +10,4 @@ fn main() {\n"
        ));
        assert!(p.contains(" ctx line\n"));
        assert!(p.contains("-old line\n"));
        assert!(p.contains("+new line\n"));
        assert!(p.contains("+extra line\n"));
        // Stops at the next hunk header — the second hunk isn't bundled in.
        assert!(!p.contains("-x\n"), "second hunk leaked in:\n{p}");
        // The second hunk reconstructs on its own.
        let p2 = hunk_patch("src/main.rs", &rows, 5).unwrap();
        assert!(p2.ends_with("@@ -50,1 +51,1 @@\n-x\n+y\n"), "{p2}");
        // A non-hunk index is rejected.
        assert!(hunk_patch("x", &rows, 1).is_none());
    }

    #[test]
    fn untracked_file_synthesizes_all_added() {
        let d = synthesized_added("src/new.rs", "line1\nline2\n");
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 0);
        assert_eq!(
            d.rows,
            vec![
                DiffRow::Add(1, "line1".into()),
                DiffRow::Add(2, "line2".into()),
            ]
        );
    }

    #[test]
    fn hunks_classify_add_delete_modify() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nd\ne_extra\n"; // b→B modified, one line appended
        let hunks = compute_hunks(old, new);
        assert_eq!(
            hunks,
            vec![
                // b → B: replace at old line 1 / new line 1
                DiffHunk { old_start: 1, old_len: 1, new_start: 1, new_len: 1 },
                // appended line after d
                DiffHunk { old_start: 4, old_len: 0, new_start: 4, new_len: 1 },
            ]
        );
        // Pure deletion → new_len == 0 marker at the boundary.
        let hunks = compute_hunks("a\nb\nc\n", "a\nc\n");
        assert_eq!(hunks, vec![DiffHunk { old_start: 1, old_len: 1, new_start: 1, new_len: 0 }]);
        // Identical → no hunks.
        assert!(compute_hunks("x\n", "x\n").is_empty());
    }

    #[test]
    fn revert_hunk_restores_baseline_lines() {
        let baseline = "a\nb\nc\n";
        // Modified line: reverting the single hunk restores the baseline.
        let current = "a\nB\nc\n";
        let h = &compute_hunks(baseline, current)[0];
        assert_eq!(revert_hunk(current, baseline, h), baseline);
        // Deletion: revert re-inserts the removed line.
        let current = "a\nc\n";
        let h = &compute_hunks(baseline, current)[0];
        assert_eq!(revert_hunk(current, baseline, h), baseline);
        // Addition: revert removes the new line.
        let current = "a\nb\nNEW\nc\n";
        let h = &compute_hunks(baseline, current)[0];
        assert_eq!(revert_hunk(current, baseline, h), baseline);
    }

    #[test]
    fn revert_after_final_line_without_newline_keeps_separator() {
        // Deletion at EOF while the current last line lacks `\n`: the re-added
        // line must not glue onto it.
        let baseline = "a\nb\n";
        let current = "a"; // no trailing newline, `b` deleted
        let hunks = compute_hunks(baseline, current);
        let restored = revert_hunk(current, baseline, hunks.last().unwrap());
        assert!(restored.ends_with("b\n"));
        assert!(restored.contains("a\n"), "separator restored, not glued: {restored:?}");
    }

    #[test]
    fn unsaved_changes_flags_differing_and_missing() {
        let dir = std::env::temp_dir().join(format!("eide_git_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "same").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "old").unwrap();

        let snapshot = vec![
            ("src/main.rs".to_owned(), "same".to_owned()),      // identical
            ("Cargo.toml".to_owned(), "new".to_owned()),        // differs
            ("src/missing.rs".to_owned(), "anything".to_owned()), // not on disk
        ];
        let unsaved = unsaved_changes(&dir, &snapshot);
        assert_eq!(unsaved, vec!["Cargo.toml".to_owned(), "src/missing.rs".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
