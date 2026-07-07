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

/// Parsed `git status --porcelain=v2 --branch`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitStatus {
    /// Current branch (`None` = detached HEAD or unborn).
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub changes: Vec<GitChange>,
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

/// UI-side handle: shared state + the commit-message draft.
pub struct GitConsole {
    pub state: Arc<Mutex<GitState>>,
    pub commit_msg: String,
}

impl Default for GitConsole {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(GitState::default())),
            commit_msg: String::new(),
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
        if let Some(rest) = line.strip_prefix("# branch.head ") {
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
/// refresh). `msg` is the commit message.
fn commands_for(op: GitOp, msg: &str) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match op {
        GitOp::Refresh => vec![],
        GitOp::Init => vec![s(&["init"])],
        GitOp::Commit => vec![s(&["add", "-A"]), vec!["commit".into(), "-m".into(), msg.to_owned()]],
        GitOp::CommitPush => vec![
            s(&["add", "-A"]),
            vec!["commit".into(), "-m".into(), msg.to_owned()],
            s(&["push"]),
        ],
        GitOp::Push => vec![s(&["push"])],
        GitOp::Pull => vec![s(&["pull"])],
        GitOp::Fetch => vec![s(&["fetch"])],
        GitOp::Log => vec![s(&["log", "--oneline", "--decorate", "-20"])],
    }
}

/// Run one git command in `dir`, collect its output. `Err` = couldn't launch.
fn run_git(dir: &Path, args: &[String]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("git");
    crate::build::no_window(&mut cmd)
        .args(args)
        .current_dir(dir)
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
    }

    std::thread::spawn(move || {
        let mut rec = crate::activity::Recorder::new(format!("Git ({})", op.label()));
        let mut sequence_ok = true;

        for args in commands_for(op, &msg) {
            let shown = format!("> git {}", args.join(" "));
            state.lock().unwrap().push(GitLine::Cmd, shown.clone());
            let t = std::time::Instant::now();
            match run_git(&project_dir, &args) {
                Ok(out) => {
                    let mut st = state.lock().unwrap();
                    push_output(&mut st, &out);
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
        let t = std::time::Instant::now();
        match run_git(
            &project_dir,
            &["status".into(), "--porcelain=v2".into(), "--branch".into()],
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
        let unsaved = unsaved_changes(&project_dir, &snapshot);
        {
            let mut st = state.lock().unwrap();
            st.unsaved = unsaved;
            st.loaded = true;
            st.busy = None;
        }
        let _ = sequence_ok;
        activity.lock().unwrap().push(rec.finish());
        ctx.request_repaint();
    });
}

/// Project-relative paths whose in-memory `snapshot` content differs from the
/// file on disk (or the file is missing) — i.e. edits a commit would MISS.
fn unsaved_changes(project_dir: &Path, snapshot: &[(String, String)]) -> Vec<String> {
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
        let cmds = commands_for(GitOp::Commit, "msg with spaces");
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], vec!["add", "-A"]);
        assert_eq!(cmds[1], vec!["commit", "-m", "msg with spaces"]);
        // Refresh runs no commands — only the always-on status refresh.
        assert!(commands_for(GitOp::Refresh, "").is_empty());
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
