//! The repositories panel (prefix, g; or a workspace's "Repositories…"):
//! the project's repositories as one. Check repositories, or single
//! changed files under them, then branch them all with one name (or start
//! a feature from each one's default branch), commit the checked files
//! with one message, push, fetch, pull, merge or rebase onto the default
//! branch, park and resume the work; each repository's result shows on its
//! line, and the ones that need a hand keep what fixes them: conflicts open
//! in Zed, then are marked resolved and continued, or aborted; a login runs
//! in a terminal; the whole batch can be undone while nothing moved since.
//!
//! The project remembers its feature, the branch its repositories should
//! be on; those on another get a badge, and Join brings them over.
//!
//! Keys: Tab moves between the tree and the field. In the tree Up/Down
//! select, Space checks, Left/Right fold, and typing goes to the field.
//! Ctrl+B branches (Ctrl+Shift+B starts the feature), Ctrl+J joins it,
//! Ctrl+Enter commits (Ctrl+Shift+Enter then pushes), Ctrl+P pushes
//! (Ctrl+Shift+P force pushes, twice), Ctrl+F fetches, Ctrl+L pulls, Ctrl+M
//! merges the default branch in (Ctrl+Shift+M rebases on it), Ctrl+K parks
//! (Ctrl+Shift+K resumes), Ctrl+E opens the conflicts in Zed, Ctrl+R marks
//! a file resolved, Ctrl+G continues (Ctrl+Shift+Delete aborts, twice),
//! Alt+Z undoes the last batch (twice), Alt+R retries what failed, Ctrl+D
//! and Ctrl+T show the diff and the log; Esc closes.
#![allow(dead_code)]

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
};

use crate::{
    chrome::{DetailTab, ReposHit, ReposView, Tone},
    git::GitStatus,
    git_ops::{Failure, Head, Integrate, OpResult, Pull, Ran, Verdict},
    text_field::TextField,
};

/// A repository changing this many files or more starts folded.
pub const FOLD_FILES: usize = 200;

/// The panel, while open, and the batch running or last run.
#[derive(Default)]
pub struct Repos {
    pub(crate) panel: Option<Panel>,
    pub(crate) batch: Option<Batch>,
    /// Hands the batches' and checks' results to the event loop.
    pub(crate) done: Option<channel::Sender<RepoEvent>>,
    /// The id the next batch, check or detail read gets.
    pub(crate) next_id: u64,
}

impl Repos {
    pub fn is_open(&self) -> bool {
        self.panel.is_some()
    }

    pub fn close(&mut self) {
        self.panel = None;
    }

    /// The field, while the panel is open.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        self.panel.as_mut().map(|p| &mut p.field)
    }
}

/// A repository as the panel lists it: one per checkout among the
/// project's workspaces.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoItem {
    /// The checkout's top folder: who it is, as the list changes.
    pub root: PathBuf,
    /// Its workspace's name.
    pub label: String,
    /// The workspace, for its Zed and its tabs.
    pub workspace: usize,
    pub status: GitStatus,
}

/// What the panel's checks, keys and views need from outside it.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    /// In the order the panel lists them.
    pub items: &'a [RepoItem],
    pub batch: Option<&'a Batch>,
    /// The project's feature branch.
    pub feature: Option<&'a str>,
    /// How many tree lines and detail lines fit (`chrome::repos_page`).
    pub page: (usize, usize),
}

/// Where the keyboard goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Tree,
    Field,
}

/// A tree line, by who it is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Selected {
    Repo(PathBuf),
    /// A file, by its repository's root and its path.
    File(PathBuf, String),
}

/// The panel's state, kept by who things are as the list changes under it
/// with every Git status read.
#[derive(Debug, Default)]
pub struct Panel {
    pub(crate) focus: Focus,
    pub(crate) field: TextField,
    pub(crate) selected: Option<Selected>,
    /// Repositories checked as a whole (a clean one, or one with every
    /// file checked).
    pub(crate) checked_repos: HashSet<PathBuf>,
    /// Files checked, by (root, path).
    pub(crate) checked_files: HashSet<(PathBuf, String)>,
    /// Repositories unfolded, and folded, against their default.
    pub(crate) unfolded: HashSet<PathBuf>,
    pub(crate) folded: HashSet<PathBuf>,
    /// The first tree line in view.
    pub(crate) first: usize,
    /// The first detail line in view.
    pub(crate) detail_first: usize,
    /// The tab asked for with Ctrl+D / Ctrl+T, else the selection's own
    /// (a file's diff, a repository's log).
    pub(crate) tab: Option<DetailTab>,
    /// The detail read for, and what it says.
    pub(crate) detail: Option<(DetailKey, String)>,
    pub(crate) hover: Option<ReposHit>,
    /// What a second press does.
    pub(crate) armed: Option<Armed>,
    /// Why the last action wasn't done, under the field.
    pub(crate) error: Option<String>,
    pub(crate) sheet: Option<Sheet>,
    /// Wheel rows not scrolled yet, over the tree and over the detail.
    pub(crate) wheel: (f64, f64),
}

/// What a second press confirms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Armed {
    ForcePush,
    Abort(PathBuf),
    Undo,
}

/// A question over the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    /// What the check before `kind` found, per repository in `jobs`.
    Preflight {
        kind: BatchKind,
        jobs: Vec<Job>,
        verdicts: Vec<(PathBuf, Verdict)>,
        /// The button Enter presses: 0 All, 1 Clean only, 2 Cancel.
        choice: usize,
    },
    /// These repositories' branches and upstreams both moved: rebase,
    /// merge or skip them.
    Diverged { roots: Vec<PathBuf>, choice: usize },
}

/// A repository in a batch, and what it does there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub root: PathBuf,
    /// For a commit: the checked files (both paths of a rename); for
    /// marking resolved: the file.
    pub files: Vec<String>,
}

/// What a batch does to each of its repositories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchKind {
    /// Switch to the branch, or make it from HEAD.
    Branch(String),
    /// Make the branch from the freshly fetched default branch.
    StartFeature(String),
    /// Bring the repositories off the feature onto it.
    Join(String),
    Commit {
        msg: String,
        push: bool,
    },
    Push {
        force: bool,
    },
    Fetch,
    Pull(Pull),
    /// Bring the default branch in.
    Integrate(Integrate),
    /// Stash the feature's work and go to the default branch.
    Park(String),
    /// Back to the feature, its stash applied.
    Resume(String),
    MarkResolved,
    /// Carry on with the operation (or the resume) stopped on conflicts.
    Continue,
    Abort,
    /// Undo the last batch.
    Undo,
}

impl BatchKind {
    /// What it is called in the result bar and toasts: `Commit and push`.
    pub fn label(&self) -> String {
        todo!()
    }

    /// Whether it is checked first (merge, rebase, pull; a branch switch
    /// with uncommitted changes), the result asked about on a sheet.
    pub fn checked_first(&self) -> bool {
        todo!()
    }

    /// Whether it can be undone (it moved only local branches).
    pub fn undoable(&self) -> bool {
        todo!()
    }
}

/// A batch: one kind of operation over some repositories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub id: u64,
    pub kind: BatchKind,
    pub runs: Vec<RepoRun>,
}

impl Batch {
    /// Every repository is done with, one way or another.
    pub fn finished(&self) -> bool {
        todo!()
    }

    /// The result bar's line: `Commit and push: 2 done · 1 conflict ·
    /// 1 failed`, and its colour.
    pub fn summary(&self) -> (String, Tone) {
        todo!()
    }

    pub fn run(&self, root: &Path) -> Option<&RepoRun> {
        self.runs.iter().find(|r| r.root == root)
    }
}

/// A repository in a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRun {
    pub job: Job,
    pub root: PathBuf,
    /// Where it stood before, and after: what undo goes back to, while it
    /// stays at `after`.
    pub before: Option<Head>,
    pub after: Option<Head>,
    pub state: RunState,
    /// Every command run in it, for the log.
    pub log: Vec<Ran>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    Queued,
    /// Doing this: `pushing…`.
    Running(String),
    Done,
    /// Stopped: `Failure::Conflict` leaves it to resolve.
    Failed { failure: Failure, why: String },
    /// Not tried: why (`nothing checked`, `blocked: detached HEAD`).
    Skipped(String),
}

/// What a detail read is for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DetailKey {
    pub root: PathBuf,
    /// The file whose diff it is; `None` for the repository's log.
    pub file: Option<String>,
    pub tab: DetailTab,
}

/// What the threads hand back to the event loop.
#[derive(Debug)]
pub enum RepoEvent {
    /// A repository of batch `batch` moved on to `what` (`pushing…`).
    Step {
        batch: u64,
        root: PathBuf,
        what: String,
    },
    /// A repository of batch `batch` is done, and where it is now.
    Finished {
        batch: u64,
        root: PathBuf,
        result: OpResult,
        after: Option<Head>,
    },
    /// The check before batch `kind` over `jobs`.
    Checked {
        id: u64,
        kind: BatchKind,
        jobs: Vec<Job>,
        verdicts: Vec<(PathBuf, Verdict)>,
    },
    Detail {
        key: DetailKey,
        text: String,
    },
}

/// What the panel asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    /// Run `kind` over `jobs`, checking first if `kind.checked_first()`.
    Batch { kind: BatchKind, jobs: Vec<Job> },
    /// Run `kind` over `jobs`, the check done (a sheet's answer).
    Confirmed { kind: BatchKind, jobs: Vec<Job> },
    /// Run the last batch again over the repositories it failed in.
    RetryFailed,
    /// Open these files of the repository at `root` in its workspace's Zed.
    OpenInZed { root: PathBuf, paths: Vec<String> },
    /// Run the command that failed in the repository at `root` in a new
    /// tab, for the user to answer its prompts.
    RunInTerminal(PathBuf),
    /// Show the workspace of the worktree at this path (where a branch is
    /// checked out already).
    OpenWorkspace(PathBuf),
}

impl Panel {
    /// A panel over `items`, nothing checked: repositories with changes
    /// unfolded unless they have `FOLD_FILES` or more, the first selected.
    pub fn new(items: &[RepoItem]) -> Panel {
        let _ = items;
        todo!()
    }

    /// The list changed (a Git status read): drop what is gone (checked
    /// files committed, repositories closed), keep the selection on who it
    /// was, or near where it was.
    pub fn refresh(&mut self, ctx: &Ctx) {
        let _ = ctx;
        todo!()
    }

    /// A key while the panel has the keyboard; what to do, if anything.
    /// `sym` is the key's own symbol, `typed` the one with Shift and the
    /// layout applied. A sheet takes the keys first, then the field (when
    /// focused) the ones that edit it.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Outcome> {
        let _ = (mods, sym, typed, ctx);
        todo!()
    }

    /// A click on `hit`.
    pub fn click(&mut self, hit: ReposHit, ctx: &Ctx) -> Option<Outcome> {
        let _ = (hit, ctx);
        todo!()
    }

    /// The wheel turned `rows` rows (down positive), over the detail pane
    /// or else the tree.
    pub fn scroll(&mut self, rows: f64, over_detail: bool, ctx: &Ctx) {
        let _ = (rows, over_detail, ctx);
        todo!()
    }

    /// What the panel shows. `title` is the project's name.
    pub fn view(&self, title: &str, ctx: &Ctx) -> ReposView {
        let _ = (title, ctx);
        todo!()
    }

    /// The detail the selection wants read, when it isn't read yet.
    pub fn detail_wanted(&self, ctx: &Ctx) -> Option<DetailKey> {
        let _ = ctx;
        todo!()
    }

    /// A detail read came back; kept if it is still the one wanted.
    pub fn set_detail(&mut self, key: DetailKey, text: String) {
        let _ = (key, text);
        todo!()
    }

    /// Ask about what the check before `kind` found.
    pub fn ask_preflight(&mut self, kind: BatchKind, jobs: Vec<Job>, verdicts: Vec<(PathBuf, Verdict)>) {
        let _ = (kind, jobs, verdicts);
        todo!()
    }

    /// Ask which way to pull the repositories at `roots`, which diverged.
    pub fn ask_diverged(&mut self, roots: Vec<PathBuf>) {
        let _ = roots;
        todo!()
    }

    /// A batch using the field's text (a branch, a commit) went through:
    /// empty it for what comes next.
    pub fn clear_field(&mut self) {
        self.field.clear();
    }
}

/// Whether `name` can be a branch's name, by `git check-ref-format
/// --branch`'s rules, without running git: else why not.
pub fn branch_name_problem(name: &str) -> Option<&'static str> {
    let _ = name;
    todo!()
}
