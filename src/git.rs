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
    /// `git remote set-url origin <url>` — repoint an EXISTING origin. `add`
    /// errors out when origin already exists, so changing the repository needs
    /// its own op. Followed by a `branch --unset-upstream`: the old upstream
    /// ref points into the previous repo, and leaving it makes the next Push
    /// fail with a confusing "no such ref" — the first Push after a change
    /// re-creates it with `-u`.
    ChangeRemote,
    /// `git checkout -b <name>` — create a new branch from the CURRENT state
    /// (current commit + any uncommitted changes come along) and switch to it,
    /// on the repository the tab's `repo:` picker targets. The name comes from
    /// the tab's `branch_draft` field.
    NewBranch,
    /// `git switch <name>` — check out an EXISTING local branch (from the header
    /// picker). Rewrites the working tree, so on success it bumps `op_gen` (new
    /// HEAD content → gutter baseline refetch) and sets `reload_project` (the
    /// in-memory buffers are reloaded from disk). The name is the tab's
    /// `switch_target`.
    SwitchBranch,
    /// `git branch -D <name>` — delete a LOCAL branch (from the header picker's
    /// trash affordance). Force (`-D`) so an unmerged branch still deletes; the
    /// caller confirms first. Does not touch HEAD/worktree, so no `op_gen` bump
    /// or reload — the status refresh just drops it from the list. Name is the
    /// tab's `switch_target`.
    DeleteBranch,
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
            GitOp::ChangeRemote => "change remote",
            GitOp::NewBranch => "new branch",
            GitOp::SwitchBranch => "switch branch",
            GitOp::DeleteBranch => "delete branch",
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

/// Turn a library's commit message into the project's mirrored one by naming
/// the library after the conventional-commit type:
/// `"feat: bla bla"` + `mw_radar` → `"feat: mw_radar bla bla"`.
///
/// A message with no recognised type just gets the name in front. Already
/// mentioning the library at the start is left alone, so re-running never
/// stutters (`feat: mw_radar mw_radar …`).
pub fn mirror_message(msg: &str, lib: &str) -> String {
    let lib = lib.trim().trim_end_matches('/');
    let msg = msg.trim();
    if lib.is_empty() {
        return msg.to_string();
    }
    // Split off a leading `feat:` / `fix:` / … if there is one.
    let (kind, rest) = COMMIT_TYPES
        .iter()
        .find_map(|(p, _)| msg.strip_prefix(p).map(|r| (Some(*p), r.trim_start())))
        .unwrap_or((None, msg));

    if rest == lib || rest.starts_with(&format!("{lib} ")) {
        return msg.to_string(); // already scoped to this library
    }
    match kind {
        Some(k) if rest.is_empty() => format!("{k} {lib}"),
        Some(k) => format!("{k} {lib} {rest}"),
        None if rest.is_empty() => lib.to_string(),
        None => format!("{lib} {rest}"),
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
    /// Local branch names (`refs/heads`), for the header branch picker. Filled
    /// by the status refresh, not by porcelain — a separate `for-each-ref`.
    pub branches: Vec<String>,
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
    /// Set by a whole-tree restore: the IDE must reload from disk, or its
    /// in-memory buffers would overwrite the files that were just restored.
    pub reload_project: bool,
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
    /// Result of a "Clone from git" library clone: `Some(Ok(lib))` when a clone
    /// finished and the app must wire it into the workspace, `Some(Err(msg))` on
    /// failure. Consumed once by the app (`diag_embed`).
    pub clone_result: Option<Result<ClonedLibrary, String>>,
}

/// A library successfully cloned into the project, awaiting workspace wiring.
#[derive(Clone, Debug, PartialEq)]
pub struct ClonedLibrary {
    /// Project-root-relative folder it was cloned into (`vendor/foo`).
    pub dir: String,
    /// Its `[package] name`, for the path dependency in the root manifest.
    pub name: String,
    /// `true` when added via `git submodule add` (model 1: the project TRACKS it
    /// via `.gitmodules` + a pinned commit, so it must NOT be gitignored).
    /// `false` for a plain `git clone` (model 2: independent, gitignored).
    pub is_submodule: bool,
}

impl GitState {
    pub(crate) fn push(&mut self, kind: GitLine, line: impl Into<String>) {
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

/// Which repository the Git tab operates on.
///
/// A library can have its OWN git repo rooted at `<project>/<lib>`. Both repos
/// share one working tree — the parent keeps tracking the files as normal files
/// (verified: `git add -A` in the parent keeps mode 100644, and a clone of the
/// parent still gets the real contents), so this is double tracking, not a
/// submodule. The consequence to remember is that a library edit shows up as a
/// change in BOTH repos and has to be committed in each.
///
/// Every git operation already takes its directory as a parameter, so pointing
/// this at a library makes the whole tab — status, commit, push, history, diff,
/// restore — work against that repo with no per-operation special cases.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum RepoTarget {
    #[default]
    Project,
    /// Workspace-member crate directory, relative to the project root.
    Library(String),
}

impl RepoTarget {
    /// Resolve to an absolute directory under `project_dir`.
    pub fn dir(&self, project_dir: &Path) -> PathBuf {
        match self {
            RepoTarget::Project => project_dir.to_path_buf(),
            RepoTarget::Library(lib) => project_dir.join(lib),
        }
    }

    /// The path prefix this repo's files carry in PROJECT-relative form —
    /// `""` for the project, `"mw_radar/"` for a library.
    ///
    /// The IDE stores file paths relative to the PROJECT root, but a library
    /// repo's paths are relative to the LIBRARY root. Without stripping this,
    /// the unsaved-changes comparison looks for `mw_radar/mw_radar/src/lib.rs`,
    /// every read fails, and `unsaved_changes` counts a failed read as
    /// "differs" — leaving the amber "unsaved changes" banner permanently lit.
    pub fn prefix(&self) -> String {
        match self {
            RepoTarget::Project => String::new(),
            RepoTarget::Library(lib) => format!("{}/", lib.trim_end_matches('/')),
        }
    }

    pub fn label(&self) -> String {
        match self {
            RepoTarget::Project => "Project".to_string(),
            RepoTarget::Library(lib) => lib.clone(),
        }
    }
}

/// Re-key a project-relative snapshot for `target`: keep only the entries that
/// belong to that repo, with the prefix removed. Pure — tested below.
pub fn snapshot_for_target(
    snapshot: Vec<(String, String)>,
    target: &RepoTarget,
) -> Vec<(String, String)> {
    let prefix = target.prefix();
    if prefix.is_empty() {
        // The project repo also contains the library's files, so nothing is
        // filtered out here — the parent legitimately tracks them.
        return snapshot;
    }
    snapshot
        .into_iter()
        .filter_map(|(p, c)| p.strip_prefix(&prefix).map(|r| (r.to_string(), c)))
        .collect()
}

pub struct GitConsole {
    pub state: Arc<Mutex<GitState>>,
    /// Changes vs History.
    pub view: GitView,
    /// Selected commit in the History list, by full sha.
    pub selected_commit: Option<String>,
    pub commit_msg: String,
    /// Draft for the "Set remote origin" field (shown while no remote exists),
    /// reused by the Change-repository editor.
    pub remote_url_draft: String,
    /// Draft for the "New branch" name field (a `git checkout -b` off the
    /// current state, on the `repo:`-selected repository).
    pub branch_draft: String,
    /// The branch the header picker asked to switch to, consumed by the next
    /// `GitOp::SwitchBranch`. Kept separate from `branch_draft` so picking a
    /// branch doesn't overwrite what's typed in the "New branch" field.
    pub switch_target: Option<String>,
    /// The Change-repository editor is open.
    pub changing_remote: bool,
    /// Validation message for that editor (git errors go to the scrollback).
    pub remote_note: Option<String>,
    /// Changed-file paths UNchecked in the tab — excluded from the next
    /// commit. Stored inverted so freshly appearing changes default to
    /// checked (commit-everything stays the no-touch default).
    pub excluded: std::collections::HashSet<String>,
    /// Which repository the whole tab operates on — the project, or a
    /// library's own repo. Session-only: reopening starts on the project.
    pub target: RepoTarget,
    /// While committing a LIBRARY, also commit the same change to the project
    /// repo (which tracks the same files). Off by default — it writes a second
    /// commit, so it stays an explicit choice.
    pub mirror_to_project: bool,
}

impl Default for GitConsole {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(GitState::default())),
            view: GitView::default(),
            selected_commit: None,
            commit_msg: String::new(),
            remote_url_draft: String::new(),
            branch_draft: String::new(),
            switch_target: None,
            changing_remote: false,
            remote_note: None,
            excluded: std::collections::HashSet::new(),
            target: RepoTarget::default(),
            mirror_to_project: false,
        }
    }
}

impl GitConsole {
    /// Point the tab at another repository.
    ///
    /// Everything cached belongs to the PREVIOUS repo — status, log, diff, the
    /// commit-message draft, the exclusion set, the remote URL — so it is all
    /// dropped. `loaded = false` makes the tab re-run its automatic status
    /// refresh for the new target on the next frame.
    pub fn switch_target(&mut self, target: RepoTarget) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.selected_commit = None;
        self.commit_msg.clear();
        self.excluded.clear();
        self.changing_remote = false;
        self.remote_note = None;
        self.remote_url_draft.clear();
        let mut st = self.state.lock().unwrap();
        st.lines.clear();
        st.status = GitStatus::default();
        st.log.clear();
        st.commit_files.clear();
        st.commit_files_sha.clear();
        st.diff = None;
        st.unsaved.clear();
        st.remote_url = None;
        st.is_repo = false;
        st.loaded = false;
    }

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
    branch: &str,
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
        // Branch off the CURRENT state (commit + working tree) and switch to it.
        GitOp::NewBranch => vec![vec!["checkout".into(), "-b".into(), branch.to_owned()]],
        // Check out an existing local branch.
        GitOp::SwitchBranch => vec![vec!["switch".into(), branch.to_owned()]],
        // Force-delete a local branch (you can't delete the one you're on).
        GitOp::DeleteBranch => vec![vec!["branch".into(), "-D".into(), branch.to_owned()]],
        GitOp::SetRemote => vec![vec!["remote".into(), "add".into(), "origin".into(), remote.to_owned()]],
        // Repoint origin, then drop the stale upstream — it names a branch in
        // the OLD repository, and leaving it makes the next Push fail. Only
        // when one actually exists: `--unset-upstream` errors out otherwise,
        // and a failing command aborts the sequence with a scary
        // "[exit 1] — sequence stopped" for what is a no-op.
        GitOp::ChangeRemote => {
            let mut v = vec![vec![
                "remote".into(),
                "set-url".into(),
                "origin".into(),
                remote.to_owned(),
            ]];
            if has_upstream {
                v.push(vec!["branch".into(), "--unset-upstream".into()]);
            }
            v
        }
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

// ── Remote URL display (name, browsable link, credential masking) ───────────

/// A git remote URL split into the parts that are safe and useful to SHOW.
///
/// A remote URL can legitimately carry credentials — `https://user:ghp_xxx@…`
/// is accepted by git and stored verbatim in `.git/config`. Rendering one
/// as-is would leak a token into any screenshot of the tab, so the raw string
/// never reaches the UI: everything here is masked at construction.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RemoteInfo {
    /// `owner/repo` — what a human recognises. Falls back to the last path
    /// segment, then to the host.
    pub name: String,
    /// `github.com`, `gitlab.company.com`, … Empty for a local path.
    pub host: String,
    /// Browsable `https://` URL, or `None` when there is nothing a browser can
    /// open (a local path). **Never** carries credentials.
    pub web_url: Option<String>,
    /// The full URL with credentials masked — for the tooltip.
    pub safe_url: String,
}

/// Split a git remote URL into displayable parts. Pure — tested below.
///
/// Handles the four forms git accepts: `https://…`, `ssh://…`, `git://…`, the
/// scp-style `git@host:owner/repo.git`, and local paths.
pub fn parse_remote_url(raw: &str) -> RemoteInfo {
    let url = raw.trim();
    if url.is_empty() {
        return RemoteInfo::default();
    }

    // A Windows drive letter (`C:\repos\x`) must not be read as a scheme
    // separator, and neither must a bare local path.
    let is_local = url.starts_with('/')
        || url.starts_with("file://")
        || (url.len() > 2 && url.as_bytes()[1] == b':' && !url.contains("//"));
    if is_local {
        let path = url.trim_start_matches("file://");
        let name = path
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(path)
            .trim_end_matches(".git")
            .to_string();
        return RemoteInfo {
            name,
            host: String::new(),
            web_url: None, // a local path is not browsable
            safe_url: path.to_string(),
        };
    }

    // scheme://[userinfo@]host[:port]/path   or   [user@]host:path (scp-style)
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("ssh".to_string(), url), // scp-style implies ssh
    };
    let scp_style = !url.contains("://");

    let (userinfo, hostpath) = match rest.split_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };

    // scp-style separates host from path with `:`; URL forms use `/`.
    let (hostport, path) = if scp_style {
        match hostpath.split_once(':') {
            Some((h, p)) => (h, p),
            None => (hostpath, ""),
        }
    } else {
        match hostpath.split_once('/') {
            Some((h, p)) => (h, p),
            None => (hostpath, ""),
        }
    };

    // An SSH port is meaningless to a browser; an HTTPS one is not.
    let https_scheme = scheme == "https" || scheme == "http";
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    };

    let path = path.trim_matches('/').trim_end_matches(".git");
    let name = if path.is_empty() {
        host.to_string()
    } else {
        // Keep the last two segments: `owner/repo` identifies a fork, a bare
        // `repo` does not. Deeper GitLab groups collapse to the tail.
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        match segs.len() {
            0 => host.to_string(),
            1 => segs[0].to_string(),
            n => format!("{}/{}", segs[n - 2], segs[n - 1]),
        }
    };

    // Credentials: for http(s) ANY userinfo is a secret (a bare token is a
    // valid username). For ssh a plain `git@` is the conventional user and
    // harmless — mask only the `user:password` form.
    let masked_user = match userinfo {
        None => None,
        Some(u) if https_scheme || u.contains(':') => Some("***".to_string()),
        Some(u) => Some(u.to_string()),
    };
    let safe_url = {
        let cred = masked_user
            .as_ref()
            .map(|u| format!("{u}@"))
            .unwrap_or_default();
        let sep = if scp_style { ":" } else { "/" };
        let tail = if path.is_empty() {
            String::new()
        } else {
            format!("{sep}{path}")
        };
        if scp_style {
            format!("{cred}{hostport}{tail}")
        } else {
            format!("{scheme}://{cred}{hostport}{tail}")
        }
    };

    // The browsable form is always https and always credential-free.
    let web_url = if host.is_empty() || path.is_empty() {
        None
    } else {
        let port_part = match port {
            Some(p) if https_scheme => format!(":{p}"),
            _ => String::new(), // drop SSH ports
        };
        Some(format!("https://{host}{port_part}/{path}"))
    };

    RemoteInfo {
        name,
        host: host.to_string(),
        web_url,
        safe_url,
    }
}

/// Normalise a directory path for comparison: forward slashes, no trailing
/// separator, no Windows verbatim prefix. Pure — tested below.
pub fn normalize_dir(p: &str) -> String {
    let s = p.trim().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s);
    s.trim_end_matches('/').to_string()
}

/// True when `dir` is the ROOT of a git repository, given the output of
/// `git rev-parse --show-toplevel` run inside it.
///
/// This distinction is what makes per-library repositories work at all: git
/// commands run in a subdirectory operate on the ENCLOSING repository. Run
/// inside `mw_radar/` before it has its own `.git`, `git status` succeeds and
/// `git remote get-url origin` returns the PARENT's URL — so the library would
/// look like it already had a repository (and the same remote as the project),
/// and the Init button that creates its own would never appear.
///
/// Compared case-insensitively: Windows paths differ only in case here, and a
/// false "not a root" would hide a real repository.
pub fn is_repo_root(toplevel_stdout: &str, dir: &Path) -> bool {
    let top = normalize_dir(toplevel_stdout);
    if top.is_empty() {
        return false;
    }
    // Canonicalise both when possible so `..`, symlinks and short names match.
    let canon = |s: &str| {
        std::fs::canonicalize(s)
            .map(|p| normalize_dir(&p.to_string_lossy()))
            .unwrap_or_else(|_| normalize_dir(s))
    };
    let a = canon(&top);
    let b = canon(&dir.to_string_lossy());
    a.eq_ignore_ascii_case(&b)
}

/// Reject a remote URL we would only fail on later, with a reason the user can
/// act on. Deliberately permissive about the FORM (git takes https, ssh,
/// scp-style and local paths) — it only rules out what is certainly wrong.
pub fn validate_remote_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("Enter the repository URL first.".into());
    }
    if u.split_whitespace().count() > 1 {
        return Err("The URL must not contain spaces.".into());
    }
    let looks_remote = u.starts_with("https://")
        || u.starts_with("http://")
        || u.starts_with("ssh://")
        || u.starts_with("git://")
        || u.starts_with("file://")
        || u.contains('@') && u.contains(':') // scp-style git@host:owner/repo.git
        // A local repo path. `Path::is_absolute` is FALSE on Windows for a
        // POSIX-style "/srv/git/repo.git" (it wants a drive prefix), yet git
        // takes it — so accept a leading slash explicitly.
        || u.starts_with('/')
        || Path::new(u).is_absolute();
    if !looks_remote {
        return Err(format!(
            "\"{u}\" doesn't look like a git URL — expected https://…, git@host:owner/repo.git, \
             or an absolute path."
        ));
    }
    Ok(())
}

/// Spawn the worker for `op`. `snapshot` is the in-memory project content
/// (project-relative path → content) used for the unsaved-changes comparison
/// against disk — computed here on the worker, never on the UI thread.
pub fn run_op(
    op: GitOp,
    msg: String,
    remote: String,
    branch: String,
    add_paths: Option<Vec<String>>,
    project_dir: PathBuf,
    snapshot: Vec<(String, String)>,
    // `(library dir name, project root)` to also commit this change into the
    // project repository — set only when committing a library that the project
    // tracks as well. `None` for an ordinary commit.
    mirror: Option<(String, PathBuf)>,
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
        for args in commands_for(op, &msg, &remote, &branch, has_upstream, &add_paths) {
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

        // ── Mirror the commit into the project repository ─────────────────
        // With a library tracked by BOTH repos, a commit made here leaves the
        // same change uncommitted in the project — this closes that gap in one
        // step instead of making the user repeat it by hand.
        //
        // `add -A -- <lib>` is REQUIRED before the commit: a pathspec-limited
        // `git commit -- <lib>` silently skips files git does not track yet, so
        // a newly added library file would never reach the project repo.
        // The pathspec on the commit then keeps it scoped — verified that work
        // the user had staged elsewhere is left untouched.
        if sequence_ok && matches!(op, GitOp::Commit | GitOp::CommitPush) {
            if let Some((lib, proj_dir)) = &mirror {
                let mirrored = mirror_message(&msg, lib);
                let steps = [
                    vec!["add".to_string(), "-A".to_string(), "--".to_string(), lib.clone()],
                    vec![
                        "commit".to_string(),
                        "-m".to_string(),
                        mirrored.clone(),
                        "--".to_string(),
                        lib.clone(),
                    ],
                ];
                for args in steps {
                    state
                        .lock()
                        .unwrap()
                        .push(GitLine::Cmd, format!("> git {}", args.join(" ")));
                    match run_git(proj_dir, &args) {
                        Ok(out) => {
                            let text = format!(
                                "{}{}",
                                String::from_utf8_lossy(&out.stdout),
                                String::from_utf8_lossy(&out.stderr)
                            );
                            // "nothing to commit" is a NO-OP, not a failure: the
                            // project may already hold this change. Reporting it
                            // as an error would flag every such commit.
                            if !out.status.success()
                                && (text.contains("nothing to commit")
                                    || text.contains("no changes added to commit"))
                            {
                                state.lock().unwrap().push(
                                    GitLine::Notice,
                                    format!("[info] project already up to date for {lib}/"),
                                );
                                break;
                            }
                            let mut st = state.lock().unwrap();
                            push_output(&mut st, &out);
                            if !out.status.success() {
                                st.push(
                                    GitLine::Err,
                                    format!("[error] mirroring the commit into the project failed"),
                                );
                                break;
                            }
                            if args.first().map(String::as_str) == Some("commit") {
                                st.push(
                                    GitLine::Notice,
                                    format!("[OK] also committed to the project: {mirrored}"),
                                );
                            }
                        }
                        Err(e) => {
                            state
                                .lock()
                                .unwrap()
                                .push(GitLine::Err, format!("[error] could not run git: {e}"));
                            break;
                        }
                    }
                }
                ctx.request_repaint();
            }
        }

        // ── Always refresh the status + the unsaved comparison ────────────
        // `--untracked-files=all`: by default git COLLAPSES an untracked
        // directory into one `? dir/` entry — a fresh (never-committed)
        // project showed a single "src/" row instead of its files (reported
        // as "nu se văd fișierele din src").
        // `--ignore-submodules=dirty`: don't recurse into submodule working
        // trees. A BROKEN or stale submodule entry (a `.gitmodules` path whose
        // directory is gone) otherwise makes `git status` exit 128 — which the
        // detection below reads as "not a git repository", so a repo with ONE
        // bad submodule appeared uninitialised (`git init`). Submodule POINTER
        // changes still show; only their inner dirty state is skipped.
        //
        // First, refresh the index STAT cache. When the IDE rewrites a file with
        // BYTE-IDENTICAL content it still bumps the mtime, and `git status` then
        // reports it modified (stat-dirty) though `git diff` sees nothing — a
        // phantom ".M" that `reset --hard` cannot clear (no content to reset), so
        // "Discard all" looked like it did nothing. `--refresh` re-stats such
        // files and marks the content-clean ones unmodified. Best-effort: it
        // exits non-zero when a file GENUINELY differs, which is expected here.
        let _ = run_git(
            &project_dir,
            &["update-index".into(), "-q".into(), "--refresh".into()],
        );
        let t = std::time::Instant::now();
        match run_git(
            &project_dir,
            &[
                "status".into(),
                "--porcelain=v2".into(),
                "--branch".into(),
                "--untracked-files=all".into(),
                "--ignore-submodules=dirty".into(),
            ],
        ) {
            Ok(out) => {
                // `git status` SUCCEEDS in any subdirectory of a repository, so
                // success alone would report a library as "already a repo" —
                // the enclosing project's. Only the repo ROOT counts here.
                let own_repo = out.status.success()
                    && run_git(&project_dir, &["rev-parse".into(), "--show-toplevel".into()])
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| {
                            is_repo_root(&String::from_utf8_lossy(&o.stdout), &project_dir)
                        })
                        .unwrap_or(false);
                let parsed =
                    own_repo.then(|| parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout)));
                let mut st = state.lock().unwrap();
                st.is_repo = own_repo;
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
        // Only when this directory owns the repository — otherwise git answers
        // with the PARENT's origin and the library would appear to share the
        // project's URL.
        let owns = state.lock().unwrap().is_repo;
        let remote_url = owns
            .then(|| {
                run_git(&project_dir, &["remote".into(), "get-url".into(), "origin".into()])
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    .filter(|u| !u.is_empty())
            })
            .flatten();

        // Local branch names for the header picker (only when this dir owns the
        // repo, so a library never lists the parent's branches).
        let branches: Vec<String> = owns
            .then(|| {
                run_git(
                    &project_dir,
                    &["for-each-ref".into(), "--format=%(refname:short)".into(), "refs/heads".into()],
                )
                .ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .map(|l| l.trim().to_owned())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default()
            })
            .unwrap_or_default();

        let unsaved = unsaved_changes(&project_dir, &snapshot);
        {
            let mut st = state.lock().unwrap();
            st.remote_url = remote_url;
            st.unsaved = unsaved;
            st.status.branches = branches;
            st.loaded = true;
            st.busy = None;
            // Commit / pull / init move HEAD or rewrite the worktree — the
            // editor's gutter-diff baseline must refetch.
            if matches!(op, GitOp::Commit | GitOp::CommitPush | GitOp::Pull | GitOp::Init) {
                st.op_gen += 1;
            }
            // Switching branches rewrote the worktree (new HEAD content): refetch
            // the gutter baseline AND reload the in-memory buffers from disk.
            // Only when the switch actually happened (a dirty-tree conflict makes
            // `git switch` fail and abort the sequence).
            if op == GitOp::SwitchBranch && sequence_ok {
                st.op_gen += 1;
                st.reload_project = true;
            }
        }
        let _ = sequence_ok;
        activity.lock().unwrap().push(rec.finish());
        ctx.request_repaint();
    });
}

/// Bring an external library repo into the project as a workspace member. Two
/// models, chosen by `submodule`:
/// - `false`: `git clone <url> <dir>` — INDEPENDENT repo (its own `.git`/remote;
///   the project gitignores it; a fresh project clone won't include it).
/// - `true`: `git submodule add <url> <dir>` — the project TRACKS it via
///   `.gitmodules` + a pinned commit, so a fresh clone (+ `submodule update`)
///   is self-contained. Requires the project to already BE a git repo.
///
/// On success validates it is a Cargo crate and reads its package name; the app
/// (`diag_embed`) then registers it as a workspace member and rescans the tree.
/// Runs on a worker; sets `clone_result` when done.
pub fn run_clone_library(
    url: String,
    dir: String,
    submodule: bool,
    project_dir: PathBuf,
    state: Arc<Mutex<GitState>>,
    ctx: egui::Context,
) {
    {
        let mut st = state.lock().unwrap();
        if st.busy.is_some() {
            return; // one op at a time
        }
        st.busy = Some("clone");
        st.clone_result = None;
    }
    std::thread::spawn(move || {
        let finish = |res: Result<ClonedLibrary, String>| {
            let mut st = state.lock().unwrap();
            match &res {
                Ok(l) => st.push(
                    GitLine::Notice,
                    format!(
                        "[ok] {} into {}",
                        if l.is_submodule { "added submodule" } else { "cloned" },
                        l.dir
                    ),
                ),
                Err(e) => st.push(GitLine::Err, format!("[error] {e}")),
            }
            st.clone_result = Some(res);
            st.busy = None;
            drop(st);
            ctx.request_repaint();
        };

        // Refuse to clone onto an existing folder — git would fail anyway, but a
        // clear message beats "destination path already exists".
        if project_dir.join(&dir).exists() {
            return finish(Err(format!(
                "`{dir}` already exists in the project — pick another folder name"
            )));
        }
        // A submodule can only be added inside a git repo.
        if submodule && !project_dir.join(".git").exists() {
            return finish(Err(
                "the project isn't a git repository yet — Init it (Git tab) before adding a \
                 submodule, or clone as an independent repo instead"
                    .into(),
            ));
        }

        let args = if submodule {
            vec!["submodule".into(), "add".into(), url.clone(), dir.clone()]
        } else {
            vec!["clone".into(), url.clone(), dir.clone()]
        };
        state.lock().unwrap().push(GitLine::Cmd, format!("> git {}", args.join(" ")));
        let out = match run_git(&project_dir, &args) {
            Ok(o) => o,
            Err(e) => return finish(Err(format!("could not run git: {e}"))),
        };
        // git streams clone progress to stderr; surface it in the console.
        for line in String::from_utf8_lossy(&out.stderr).lines() {
            if !line.trim().is_empty() {
                state.lock().unwrap().push(GitLine::Out, line.to_string());
            }
        }
        if !out.status.success() {
            let what = if submodule { "git submodule add" } else { "git clone" };
            return finish(Err(format!("{what} failed — see the output above")));
        }

        // It must be a Cargo crate to join the workspace.
        let manifest = project_dir.join(&dir).join("Cargo.toml");
        let toml = match std::fs::read_to_string(&manifest) {
            Ok(t) => t,
            Err(_) => {
                return finish(Err(format!(
                    "the cloned repo has no Cargo.toml — `{dir}` is not a Rust crate"
                )))
            }
        };
        match parse_package_name(&toml) {
            Some(name) => finish(Ok(ClonedLibrary { dir, name, is_submodule: submodule })),
            None => finish(Err(format!(
                "`{dir}/Cargo.toml` has no [package] name — not a library crate"
            ))),
        }
    });
}

/// The `[package] name = "…"` value from a Cargo.toml, if present. Hand-parsed
/// (no toml dep, like `workspace_members`): track the current `[section]`, then
/// read the first `name = "…"` inside `[package]`.
pub fn parse_package_name(cargo_toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = t.strip_prefix("name") {
                if let Some(v) = rest.trim_start().strip_prefix('=') {
                    let name = v.trim().trim_matches('"').trim();
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

/// The `path = …` values declared in a `.gitmodules` file. Pure (tested below).
/// `.gitmodules` uses git-config syntax: sections `[submodule "name"]` with
/// `path`/`url` entries — the PATH is what a submodule lives at in the tree.
fn gitmodules_paths(gitmodules: &str) -> Vec<String> {
    gitmodules
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("path")?.trim_start().strip_prefix('=')?;
            Some(rest.trim().trim_matches('"').to_string())
        })
        .collect()
}

/// Whether `dir` (project-root-relative, forward slashes) is registered as a git
/// submodule of the repo at `project_dir` — i.e. `.gitmodules` has a matching
/// `path = <dir>` entry.
pub fn is_submodule(project_dir: &Path, dir: &str) -> bool {
    let Ok(gm) = std::fs::read_to_string(project_dir.join(".gitmodules")) else {
        return false;
    };
    gitmodules_paths(&gm).iter().any(|p| p == dir)
}

/// Fully remove the submodule checked out at `dir` from the repo at
/// `project_dir`: deinit it, drop the gitlink + its `.gitmodules` entry
/// (`git rm`), and delete the backing repo under `.git/modules`. Best-effort —
/// every step tolerates a partial/broken submodule.
///
/// This is exactly what a plain `remove_dir_all` MISSES: deleting only the
/// working tree leaves a stale `.gitmodules` entry (and gitlink) pointing at a
/// gone path, which later makes `git status` recurse into it and exit non-zero —
/// the IDE then reads the repo as "not a repo" and offers `git init`.
pub fn remove_submodule(project_dir: &Path, dir: &str) {
    // Unregister: empties the working tree and drops the `.git/config` entry.
    let _ = run_git(
        project_dir,
        &["submodule".into(), "deinit".into(), "-f".into(), "--".into(), dir.to_owned()],
    );
    // Drop the gitlink from the index AND the `.gitmodules` entry (staged), and
    // remove the working tree. Modern git (>=1.8.5) rewrites `.gitmodules` here.
    let _ = run_git(
        project_dir,
        &["rm".into(), "-f".into(), "--".into(), dir.to_owned()],
    );
    // The backing repo (submodule NAME == path in the common case). Harmless if
    // already gone or if a differently-named module dir is left behind.
    let _ = std::fs::remove_dir_all(project_dir.join(".git").join("modules").join(dir));
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
    restore_file_at(dir, "HEAD", path)
}

/// Write `path` as it was at `rev` (a sha, `HEAD`, a tag…) into the worktree
/// and return the content, so the caller can refresh its in-memory buffer.
///
/// Only this one file is touched — nothing about HEAD or any other file moves,
/// which is what makes restoring from history reversible: the result shows up
/// as an ordinary uncommitted change.
pub fn restore_file_at(dir: &Path, rev: &str, path: &str) -> Result<String, String> {
    let out = run_git(dir, &["show".into(), format!("{rev}:{path}")])
        .map_err(|e| format!("couldn't run git show: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let content = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    let dest = dir.join(path);
    // Skip the write when the working file already holds this exact content — a
    // needless rewrite only bumps the mtime, which `git status` then flags as a
    // phantom "modified" (stat-dirty) even though nothing changed. That is
    // exactly what made a "Restore to HEAD" leave the file still showing as
    // changed. (The status refresh also `update-index --refresh`es to be safe.)
    if std::fs::read(&dest).is_ok_and(|b| b == content.as_bytes()) {
        return Ok(content);
    }
    // The file may have been DELETED since `rev` — then its folder is gone too.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    std::fs::write(&dest, &content).map_err(|e| format!("write failed: {e}"))?;
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

/// Make the TRACKED worktree match `rev`, without moving HEAD.
///
/// Two steps, because `checkout` alone leaves a mix:
///   1. `git checkout <rev> -- .` restores every file that exists in `rev`;
///   2. files tracked at HEAD but ABSENT from `rev` (added afterwards) are
///      removed — without this you get old code plus orphan files.
///
/// Safety, by construction:
/// * **HEAD does not move.** Everything this does is one uncommitted change,
///   so "Discard all" (`reset --hard` + `clean`) undoes the whole operation.
/// * **Untracked files are never touched.** They exist in no commit, so
///   deleting them could not be undone; leaving them is the only safe choice.
///
/// Sets `reload_project` on success: the IDE holds these files in memory and
/// would otherwise overwrite the restored ones at the next save.
pub fn run_restore_tree(
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
        st.busy = Some("restore");
        st.diff = None;
    }
    std::thread::spawn(move || {
        let short = sha[..sha.len().min(7)].to_owned();
        let mut ok = true;

        let checkout = run_git(
            &project_dir,
            &["checkout".into(), sha.clone(), "--".into(), ".".into()],
        );
        match checkout {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                ok = false;
                let mut st = state.lock().unwrap();
                push_output(&mut st, &o);
                st.lines.push((
                    GitLine::Notice,
                    format!("[error] checkout {short} failed — nothing was changed"),
                ));
            }
            Err(e) => {
                ok = false;
                state
                    .lock()
                    .unwrap()
                    .lines
                    .push((GitLine::Notice, format!("[error] couldn't launch git: {e}")));
            }
        }

        // Files added between `sha` and HEAD — they are tracked, so removing
        // them is recoverable from HEAD.
        if ok {
            let added = run_git(
                &project_dir,
                &[
                    "diff".into(),
                    "--name-only".into(),
                    "--diff-filter=A".into(),
                    sha.clone(),
                    "HEAD".into(),
                ],
            );
            if let Ok(o) = added {
                let paths: Vec<String> = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                if !paths.is_empty() {
                    let mut args = vec!["rm".into(), "-f".into(), "--quiet".into(), "--".into()];
                    args.extend(paths.iter().cloned());
                    let _ = run_git(&project_dir, &args);
                    state.lock().unwrap().lines.push((
                        GitLine::Notice,
                        format!("[info] removed {} file(s) added after {short}", paths.len()),
                    ));
                }
            }
        }

        let mut st = state.lock().unwrap();
        if ok {
            st.lines.push((
                GitLine::Notice,
                format!("[ok] worktree restored to {short} — review it in Changes, or Discard all to undo"),
            ));
            st.op_gen += 1;
            st.reload_project = true;
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
    // Compare EOL-AGNOSTICALLY. The in-memory snapshot is LF-normalized, but a
    // project checked out on Windows (`core.autocrlf=true`) has CRLF files on
    // disk, and `write_if_changed` deliberately keeps that CRLF. A raw `disk !=
    // content` therefore flagged EVERY file as unsaved (CRLF vs LF) even right
    // after a save — while git itself reported the tree clean. Stripping `\r`
    // (the canonical LF form `lf_to_crlf` assumes) makes "unsaved" mean a REAL
    // content change — exactly what `write_if_changed` would actually rewrite.
    let norm = |s: &str| s.replace('\r', "");
    snapshot
        .iter()
        .filter(|(rel, content)| {
            std::fs::read_to_string(project_dir.join(rel))
                .map(|disk| norm(&disk) != norm(content))
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
    fn parse_package_name_reads_only_the_package_section() {
        let toml = "[workspace]\nname = \"not-this\"\n\n[package]\nname = \"my_lib\"\nversion = \"0.1.0\"\n";
        assert_eq!(parse_package_name(toml).as_deref(), Some("my_lib"));
        // No [package] → None (e.g. a bare workspace root).
        assert_eq!(parse_package_name("[workspace]\nmembers = []\n"), None);
    }

    #[test]
    fn gitmodules_paths_reads_every_submodule_path() {
        let gm = "\
[submodule \"my-lib\"]
\tpath = my-lib
\turl = https://example.com/my-lib.git
[submodule \"vendor/other\"]
    path = vendor/other
    url = https://example.com/other.git
";
        assert_eq!(gitmodules_paths(gm), vec!["my-lib", "vendor/other"]);
        // A file with no submodule sections yields nothing.
        assert!(gitmodules_paths("").is_empty());
    }

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
        let cmds = commands_for(GitOp::Commit, "msg with spaces", "", "", true, &None);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], vec!["add", "-A"]);
        assert_eq!(cmds[1], vec!["commit", "-m", "msg with spaces"]);
        // Refresh runs no commands — only the always-on status refresh.
        assert!(commands_for(GitOp::Refresh, "", "", "", true, &None).is_empty());
    }

    #[test]
    fn checked_files_stage_selectively() {
        // Checkbox selection → `add -A -- <paths>` stages only those (incl.
        // deletions matching them); everything-checked keeps plain `add -A`.
        let picked = Some(vec!["src/main.rs".to_owned(), "Cargo.toml".to_owned()]);
        let cmds = commands_for(GitOp::Commit, "m", "", "", true, &picked);
        assert_eq!(cmds[0], vec!["add", "-A", "--", "src/main.rs", "Cargo.toml"]);
        assert_eq!(cmds[1], vec!["commit", "-m", "m"]);
    }

    #[test]
    fn first_push_sets_upstream_later_pushes_are_plain() {
        // No upstream yet → the push must create it (`-u origin HEAD`).
        assert_eq!(
            commands_for(GitOp::Push, "", "", "", false, &None),
            vec![vec!["push", "-u", "origin", "HEAD"]]
        );
        assert_eq!(commands_for(GitOp::Push, "", "", "", true, &None), vec![vec!["push"]]);
        // Commit & Push uses the same upstream-aware push as its last step.
        let cmds = commands_for(GitOp::CommitPush, "m", "", "", false, &None);
        assert_eq!(cmds[2], vec!["push", "-u", "origin", "HEAD"]);
    }

    #[test]
    fn set_remote_adds_origin_with_the_draft_url() {
        assert_eq!(
            commands_for(GitOp::SetRemote, "", "https://github.com/u/r.git", "", true, &None),
            vec![vec!["remote", "add", "origin", "https://github.com/u/r.git"]]
        );
    }

    #[test]
    fn new_branch_checks_out_a_branch_from_the_draft_name() {
        // Branch off the current state (checkout -b keeps working-tree changes).
        assert_eq!(
            commands_for(GitOp::NewBranch, "", "", "feature/foo", true, &None),
            vec![vec!["checkout", "-b", "feature/foo"]]
        );
    }

    #[test]
    fn switch_branch_checks_out_the_target() {
        assert_eq!(
            commands_for(GitOp::SwitchBranch, "", "", "develop", true, &None),
            vec![vec!["switch", "develop"]]
        );
    }

    #[test]
    fn delete_branch_force_deletes_the_target() {
        assert_eq!(
            commands_for(GitOp::DeleteBranch, "", "", "stale-lib", true, &None),
            vec![vec!["branch", "-D", "stale-lib"]]
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
    fn change_remote_repoints_origin_and_clears_a_stale_upstream() {
        // With an upstream: it names a branch in the OLD repo, so it must go
        // or the next Push fails.
        let with = commands_for(
            GitOp::ChangeRemote,
            "",
            "https://github.com/u/new.git",
            "",
            true,
            &None,
        );
        assert_eq!(with.len(), 2);
        assert_eq!(with[0][0], "remote");
        assert_eq!(with[0][1], "set-url"); // NOT `add` — origin already exists
        assert_eq!(with[1], vec!["branch", "--unset-upstream"]);

        // Without one, `--unset-upstream` would fail and abort the sequence
        // with a misleading error for what is a no-op.
        let without = commands_for(
            GitOp::ChangeRemote,
            "",
            "https://github.com/u/new.git",
            "",
            false,
            &None,
        );
        assert_eq!(without.len(), 1);
    }

    #[test]
    fn mirror_message_names_the_library_after_the_type() {
        // The shape the user asked for.
        assert_eq!(
            mirror_message("feat: bla bla", "mw_radar"),
            "feat: mw_radar bla bla"
        );
        assert_eq!(
            mirror_message("fix: parser off by one", "mw_radar"),
            "fix: mw_radar parser off by one"
        );
    }

    #[test]
    fn mirror_message_without_a_known_type_just_prefixes() {
        assert_eq!(mirror_message("bla bla", "mw_radar"), "mw_radar bla bla");
    }

    #[test]
    fn mirror_message_does_not_stutter_when_already_scoped() {
        // Re-running (or a message the user already scoped) must not produce
        // "feat: mw_radar mw_radar …".
        assert_eq!(
            mirror_message("feat: mw_radar bla", "mw_radar"),
            "feat: mw_radar bla"
        );
        assert_eq!(mirror_message("mw_radar bla", "mw_radar"), "mw_radar bla");
        // A DIFFERENT name that merely starts the same must still be scoped.
        assert_eq!(
            mirror_message("feat: mw_radar_extra bla", "mw_radar"),
            "feat: mw_radar mw_radar_extra bla"
        );
    }

    #[test]
    fn mirror_message_handles_empty_and_type_only() {
        assert_eq!(mirror_message("feat:", "mw_radar"), "feat: mw_radar");
        assert_eq!(mirror_message("  ", "mw_radar"), "mw_radar");
        // No library → unchanged.
        assert_eq!(mirror_message("feat: bla", ""), "feat: bla");
        // A trailing slash on the member name must not leak into the message.
        assert_eq!(
            mirror_message("feat: bla", "mw_radar/"),
            "feat: mw_radar bla"
        );
    }

    #[test]
    fn normalize_dir_squares_separators_and_prefixes() {
        assert_eq!(normalize_dir("C:\\a\\b\\"), "C:/a/b");
        assert_eq!(normalize_dir("//?/C:/a/b"), "C:/a/b");
        assert_eq!(normalize_dir("  /srv/git/  "), "/srv/git");
        assert_eq!(normalize_dir(""), "");
    }

    #[test]
    fn repo_root_check_rejects_a_subdirectory_of_a_repo() {
        // THE bug this exists for: `git status` and `git remote get-url` both
        // succeed inside `mw_radar/` before it has its own `.git`, answering
        // for the PARENT — so the library looked like it already had a repo,
        // sharing the project's URL, and Init never appeared.
        let top = "C:/proj\n";
        assert!(is_repo_root(top, Path::new("C:/proj")));
        assert!(!is_repo_root(top, Path::new("C:/proj/mw_radar")));
        // Separator style and trailing slashes must not decide it.
        assert!(is_repo_root("C:/proj\n", Path::new("C:\\proj\\")));
        // Empty output (not a repo at all) is never a root.
        assert!(!is_repo_root("", Path::new("C:/proj")));
    }

    #[test]
    fn repo_target_resolves_dir_and_prefix() {
        let root = Path::new("C:/proj");
        assert_eq!(RepoTarget::Project.dir(root), root.to_path_buf());
        assert_eq!(
            RepoTarget::Library("mw_radar".into()).dir(root),
            root.join("mw_radar")
        );
        assert_eq!(RepoTarget::Project.prefix(), "");
        assert_eq!(RepoTarget::Library("mw_radar".into()).prefix(), "mw_radar/");
        // A trailing slash in the member name must not double up.
        assert_eq!(RepoTarget::Library("mw_radar/".into()).prefix(), "mw_radar/");
    }

    #[test]
    fn snapshot_is_rekeyed_to_the_library_root() {
        // The IDE keys files from the PROJECT root; a library repo's paths are
        // relative to the LIBRARY root. Without stripping, every read fails and
        // `unsaved_changes` treats a failed read as "differs" — the amber
        // banner would be permanently lit.
        let snap = vec![
            ("mw_radar/src/lib.rs".to_owned(), "a".to_owned()),
            ("mw_radar/Cargo.toml".to_owned(), "b".to_owned()),
            ("src/main.rs".to_owned(), "c".to_owned()),
            // A sibling that merely starts with the same letters must not match.
            ("mw_radar_extra/src/lib.rs".to_owned(), "d".to_owned()),
        ];
        let lib = snapshot_for_target(snap.clone(), &RepoTarget::Library("mw_radar".into()));
        assert_eq!(
            lib,
            vec![
                ("src/lib.rs".to_owned(), "a".to_owned()),
                ("Cargo.toml".to_owned(), "b".to_owned()),
            ]
        );
        // The project repo legitimately tracks the library's files too, so
        // nothing is filtered out there.
        assert_eq!(snapshot_for_target(snap.clone(), &RepoTarget::Project), snap);
    }

    #[test]
    fn remote_https_yields_name_host_and_browsable_url() {
        let r = parse_remote_url("https://github.com/traian/mw_radar.git");
        assert_eq!(r.name, "traian/mw_radar");
        assert_eq!(r.host, "github.com");
        assert_eq!(
            r.web_url.as_deref(),
            Some("https://github.com/traian/mw_radar")
        );
        assert_eq!(r.safe_url, "https://github.com/traian/mw_radar");
    }

    #[test]
    fn remote_scp_style_ssh_becomes_a_browsable_https_url() {
        // `git@github.com:owner/repo.git` has no scheme and uses `:` — the
        // form most likely to be mis-parsed.
        let r = parse_remote_url("git@github.com:traian/mw_radar.git");
        assert_eq!(r.name, "traian/mw_radar");
        assert_eq!(r.host, "github.com");
        assert_eq!(
            r.web_url.as_deref(),
            Some("https://github.com/traian/mw_radar")
        );
        // A bare `git@` is the conventional ssh user, not a secret.
        assert_eq!(r.safe_url, "git@github.com:traian/mw_radar");
    }

    #[test]
    fn embedded_token_never_reaches_the_display_or_the_link() {
        // The security case: git accepts and STORES this verbatim, so a
        // screenshot of the tab would leak the token.
        let r = parse_remote_url("https://traian:ghp_SECRET123@github.com/traian/mw_radar.git");
        assert!(!r.safe_url.contains("ghp_SECRET123"), "{}", r.safe_url);
        assert!(!r.safe_url.contains("traian:"), "{}", r.safe_url);
        assert_eq!(r.safe_url, "https://***@github.com/traian/mw_radar");
        // The browsable link must be credential-free too.
        let web = r.web_url.unwrap();
        assert_eq!(web, "https://github.com/traian/mw_radar");
        assert!(!web.contains('@'));
        assert_eq!(r.name, "traian/mw_radar");
    }

    #[test]
    fn bare_token_as_username_is_masked_too() {
        // No colon, but on https a lone userinfo IS the token.
        let r = parse_remote_url("https://ghp_SECRET123@github.com/o/r.git");
        assert!(!r.safe_url.contains("ghp_SECRET123"), "{}", r.safe_url);
        assert_eq!(r.safe_url, "https://***@github.com/o/r");
    }

    #[test]
    fn ssh_port_is_dropped_from_the_web_url_but_https_port_is_kept() {
        // 2222 is an SSH port — meaningless to a browser.
        let ssh = parse_remote_url("ssh://git@gitlab.corp.com:2222/team/lib.git");
        assert_eq!(
            ssh.web_url.as_deref(),
            Some("https://gitlab.corp.com/team/lib")
        );
        // 8443 is the actual https port — keep it or the link 404s.
        let https = parse_remote_url("https://gitlab.corp.com:8443/team/lib.git");
        assert_eq!(
            https.web_url.as_deref(),
            Some("https://gitlab.corp.com:8443/team/lib")
        );
    }

    #[test]
    fn nested_group_paths_collapse_to_the_last_two_segments() {
        let r = parse_remote_url("https://gitlab.com/corp/team/sub/mw_radar.git");
        assert_eq!(r.name, "sub/mw_radar");
        // The LINK must still target the full path, not the shortened name.
        assert_eq!(
            r.web_url.as_deref(),
            Some("https://gitlab.com/corp/team/sub/mw_radar")
        );
    }

    #[test]
    fn local_paths_have_a_name_but_are_not_browsable() {
        for p in ["C:\\repos\\mw_radar", "/srv/git/mw_radar.git"] {
            let r = parse_remote_url(p);
            assert_eq!(r.name, "mw_radar", "{p}");
            // Nothing for a browser to open — the UI must not offer a link.
            assert!(r.web_url.is_none(), "{p}");
        }
    }

    #[test]
    fn empty_remote_is_inert() {
        let r = parse_remote_url("   ");
        assert_eq!(r, RemoteInfo::default());
        assert!(r.web_url.is_none());
    }

    #[test]
    fn remote_url_validation_accepts_every_form_git_takes() {
        for ok in [
            "https://github.com/user/mw_radar.git",
            "http://host/repo.git",
            "ssh://git@host/repo.git",
            "git://host/repo.git",
            "git@github.com:user/mw_radar.git",
            "C:\\repos\\mw_radar",
            "/srv/git/mw_radar.git",
        ] {
            assert!(validate_remote_url(ok).is_ok(), "rejected {ok}");
        }
    }

    #[test]
    fn remote_url_validation_rejects_the_certainly_wrong() {
        assert!(validate_remote_url("").is_err());
        assert!(validate_remote_url("   ").is_err());
        // A bare name is the likely typo (pasting a repo name, not a URL).
        assert!(validate_remote_url("mw_radar").is_err());
        // Spaces would split into extra git arguments.
        assert!(validate_remote_url("https://host/a repo.git").is_err());
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

    /// A file that is CRLF on disk (Windows checkout) but LF in the in-memory
    /// snapshot must NOT count as unsaved — content is identical modulo EOL. This
    /// is the false-positive the exit dialog + Git-tab warning used to show.
    #[test]
    fn unsaved_changes_ignores_crlf_vs_lf() {
        let dir = std::env::temp_dir().join(format!("eide_git_eol_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        // On disk: CRLF (as `core.autocrlf=true` checks out on Windows).
        std::fs::write(dir.join("src/main.rs"), "line1\r\nline2\r\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "a\r\nB\r\n").unwrap();

        let snapshot = vec![
            // In-memory: LF-normalized, SAME content → not unsaved.
            ("src/main.rs".to_owned(), "line1\nline2\n".to_owned()),
            // A REAL edit (line 2 changed) → still flagged despite the EOL diff.
            ("Cargo.toml".to_owned(), "a\nb\n".to_owned()),
        ];
        let unsaved = unsaved_changes(&dir, &snapshot);
        assert_eq!(unsaved, vec!["Cargo.toml".to_owned()], "only the real edit is unsaved");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
