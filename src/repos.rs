//! The repositories panel (prefix, g; or a workspace's "Repositories…"):
//! the project's repositories as one. Check repositories, or single
//! changed files under them, then commit the checked files with one
//! message, push, fetch, pull, merge or rebase onto the default branch;
//! each repository's result shows on its line, and the ones that need a
//! hand keep what fixes them: conflicts open in Zed, then are marked
//! resolved and continued, or aborted; a login runs in a terminal; the
//! whole batch can be undone while nothing moved since.
//! The header's branch picker switches them all to a branch they all have,
//! names that are one branch (`main`, `master`) taken as one; typing in
//! its menu finds one, or offers to create a branch by that name. Before
//! creating it they fetch, and if some have a branch by that name already
//! a sheet asks whether to switch to it, create it where it is missing,
//! or edit the name. The same menu picks a branch to merge in, or to
//! rebase onto. Its branches come default branch first, then the config's
//! `top_branches`, then the newest.
//!
//! The detail shows every change of every repository one after another,
//! as one diff: per repository a header, per file a header (its checkbox
//! the same as its tree line's) over its hunks, the words changed within
//! a line marked. Selecting a line in the tree scrolls it there; the
//! header of the file in view stays pinned at its top. Review comments
//! written on its lines show under them, to copy all at once as Markdown
//! for an AI agent (see `review`); kept with the project. `/` in the diff
//! (Ctrl+Shift+F anywhere) searches it, folded files too (see `search`).
//!
//! The tree lists a repository's files flat, by their whole paths, or
//! under their folders (a folder holding only a folder one line with it),
//! as the buttons over it or Alt+F pick, for every repository; the diff
//! goes in the same order. Two more buttons there (Alt+Left / Alt+Right)
//! fold every repository and folder, or unfold them all. A folder checks
//! and folds as a whole.
//!
//! Keys: Tab moves from the tree to the detail to the field. In the tree
//! Up/Down select, Space checks, Left/Right fold, Enter opens a file in
//! Zed or folds a repository or folder, and typing goes to the field; Shift with
//! Up/Down or the pages scrolls the detail. In the diff the arrows and the
//! pages move a cursor over its rows (with Shift, reaching a range back),
//! Alt+Up/Down go to the file before or after, and for the cursor's file
//! Space checks it, Left/Right fold it (on a line, move the caret) and
//! Enter opens it in Zed at the cursor's line (`g` Space and Ctrl+click
//! too, at the caret's char; on a file's header, the file). C writes a comment on the
//! range's lines (or edits the one the cursor is on): Shift+Enter breaks
//! its line, Enter keeps it, Esc drops it; Delete deletes it, Ctrl+Shift+C copies the review, and
//! "Clear review" deletes it all, asked first. Ctrl+N opens the branch
//! picker to create a branch (in it, Ctrl+Enter creates the one typed),
//! Ctrl+Enter
//! commits (Ctrl+Shift+Enter then pushes), Ctrl+P pushes (Ctrl+Shift+P force
//! pushes), Ctrl+F fetches, Ctrl+L pulls, Ctrl+M merges the default
//! branch in (Ctrl+Shift+M rebases on it), Alt+M merges a branch picked
//! in (Alt+Shift+M rebases on it), Ctrl+E opens the conflicts in Zed,
//! Ctrl+R marks a file resolved, Ctrl+G continues (Ctrl+Shift+Delete
//! aborts), Alt+Z undoes the last batch, Alt+R retries what failed, Alt+S
//! stashes (Alt+Shift+S pops), Ctrl+D and Ctrl+T show the diff and the
//! log, Ctrl+S opens the branch picker, F1 lists the keys; Esc closes.
//! What can't be taken back (a force push, an abort, an undo, a branch or
//! stash deleted) asks first, on a sheet. On a repository's line, a click
//! on the arrow up pushes it, on the arrow down pulls it, and on its stash
//! count lists its stashes; a right click offers the actions.

use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
};

use crate::{
    chrome::{
        BRANCH_MENU_ROWS, BranchCreate, BranchMenuView, BranchPicker, Check, Chip, CleanLine,
        Density, DetailLine, DetailRow, DetailTab, DetailView, DiffTone, DirLine, FileLine, Hint,
        LineState, RepoLine, RepoMark, ReposButton, ReposHit, ReposView, ResultBar, SheetView,
        Tone, TreeLine, TreeTool,
    },
    diff::FileDiff,
    diff_edit::{FileEdit, Put},
    diff_view::{
        self, DiffView, Keys, Moved, NO_REVIEW, Place, Section, bounds, detail_line, in_dir,
        move_to, showing,
    },
    git::{Change, GitStatus, InProgress},
    git_ops::{Failure, Head, Integrate, OpResult, Pull, Ran, Verdict},
    scroll::{Bounds, Motion, Scroll},
    text_field::{self, Edit, TextField},
    worktree_diff::WorktreeDiff,
};

/// A repository changing this many files or more starts folded.
pub const FOLD_FILES: usize = 200;

/// How many of the branch menu's longest branches it is measured by.
const WIDEST: usize = 3;

/// The panel, while open, and the batch running or last run.
#[derive(Default)]
pub struct Repos {
    pub(crate) panel: Option<Panel>,
    pub(crate) batch: Option<Batch>,
    /// Hands the batches' and checks' results to the event loop.
    pub(crate) done: Option<channel::Sender<RepoEvent>>,
    /// The id the next batch, check or detail read gets.
    pub(crate) next_id: u64,
    /// What the panel lists, as of the last Git status read.
    pub(crate) items: Vec<RepoItem>,
    /// The panel as closed, with the project it was for: opened again for
    /// the same project, it is as it was left.
    pub(crate) kept: Option<(Option<String>, Panel)>,
    /// The log or stashes being read.
    pub(crate) reading: Option<DetailKey>,
    /// The diffs being read, by root, with the Git status read each is
    /// for.
    pub(crate) reading_diffs: HashMap<PathBuf, u64>,
    /// The check under way, by id: an older one's answer is stale.
    pub(crate) checking: Option<u64>,
    /// How many of the panel's own fetches run (on opening it, before a
    /// check): its summary says it is fetching meanwhile.
    pub(crate) fetches: usize,
    /// Groups of branch names that are one branch (`main`, `master`), from
    /// the config.
    pub(crate) same_branches: Vec<Vec<String>>,
    /// Branches the branch lists show first, after the default branch.
    pub(crate) top_branches: Vec<String>,
    /// The tree's share of the panel's width, dragged on the line between
    /// the tree and the detail; `None` for the default.
    pub(crate) tree_share: Option<f64>,
    /// The cuts made from the diff, the last last, to undo; and those
    /// undone, to do again.
    pub(crate) cuts: Vec<crate::diff_edit::Done>,
    pub(crate) undone: Vec<crate::diff_edit::Done>,
    /// What was yanked or deleted from the diff last, to paste.
    pub(crate) register: crate::diff_edit::Register,
    /// A worktree's diff, shown in the panel in its place, while open.
    pub(crate) worktree: Option<WorktreeDiff>,
    /// The review comments written on worktrees' diffs, by worktree, as
    /// saved: read once they are wanted.
    pub(crate) worktree_reviews: Option<crate::review::Worktrees>,
}

impl Repos {
    /// The panel, or a worktree's diff in its place, is open.
    pub fn is_open(&self) -> bool {
        self.panel.is_some() || self.worktree.is_some()
    }

    /// Close it for good, as when leaving the project.
    pub fn close(&mut self) {
        self.panel = None;
        self.kept = None;
        self.close_worktree();
    }

    /// Close the worktree's diff: its review is saved with every change.
    pub fn close_worktree(&mut self) {
        self.worktree = None;
    }

    /// The diff shown: the worktree's, else the panel's.
    pub fn diff_mut(&mut self) -> Option<&mut DiffView> {
        match (&mut self.worktree, &mut self.panel) {
            (Some(wd), _) => Some(&mut wd.diff),
            (None, Some(panel)) => Some(&mut panel.diff),
            (None, None) => None,
        }
    }

    /// Stop a list flung; whether one was.
    pub fn stop_fling(&mut self) -> bool {
        let panel = (self.panel.as_mut()).is_some_and(|panel| panel.stop_fling());
        let worktree = (self.worktree.as_mut()).is_some_and(|wd| wd.diff.scroll.stop_fling());
        panel || worktree
    }

    /// The field, while the panel is open: a worktree diff's, its search's.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        if let Some(wd) = self.worktree.as_mut() {
            return wd.diff.search_field_mut();
        }
        let panel = self.panel.as_mut()?;
        if panel.search_has_keys() && panel.branch_menu.is_none() {
            return panel.diff.search_field_mut();
        }
        Some(match &mut panel.branch_menu {
            Some(menu) => &mut menu.query,
            None => &mut panel.field,
        })
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
    /// How many tree lines and detail lines fit (`chrome::repos_page`).
    pub page: (usize, usize),
    /// The time now, for how long ago the repositories fetched.
    pub now: SystemTime,
    /// A fetch of the panel's own runs: how long ago it last fetched says
    /// nothing then.
    pub fetching: bool,
    /// Groups of branch names that are one branch (`main`, `master`).
    pub same: &'a [Vec<String>],
    /// Branches the branch lists show first, in this order, after the
    /// default branch.
    pub top: &'a [String],
    /// How much room the chrome takes.
    pub density: Density,
    /// The tree's share of the panel's width (`chrome::REPOS_TREE_SHARE`
    /// unless dragged).
    pub tree_share: f64,
    /// How many characters a review comment's row of the diff holds
    /// (`chrome::repos_detail_cols`).
    pub cols: usize,
}

/// How the tree lists a repository's changed files: a line each with its
/// whole path, or under a line per folder. One for every repository, kept
/// in `state.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    #[default]
    List,
    Folders,
}

impl Grouping {
    pub fn is_list(&self) -> bool {
        *self == Grouping::List
    }

    /// The other one.
    fn other(self) -> Grouping {
        match self {
            Grouping::List => Grouping::Folders,
            Grouping::Folders => Grouping::List,
        }
    }
}

/// Where the keyboard goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Tree,
    /// The detail pane: the keys scroll it, and in the diff act on the
    /// file at its top.
    Detail,
    Field,
}

/// A tree line, by who it is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Selected {
    Repo(PathBuf),
    /// A folder, by its repository's root and its path.
    Dir(PathBuf, String),
    /// A file, by its repository's root and its path.
    File(PathBuf, String),
    /// The heading over the repositories with nothing to do.
    Clean,
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
    /// How the files are listed.
    pub(crate) grouping: Grouping,
    /// Folders folded, by (root, path).
    pub(crate) folded_dirs: HashSet<(PathBuf, String)>,
    /// The repositories that had something to do since the panel opened:
    /// they stay with the others, under no `Clean` heading, so that a
    /// line doesn't jump there as a batch commits and pushes it.
    pub(crate) stirred: HashSet<PathBuf>,
    /// The batch chips clicked away, by (batch id, root): the next batch
    /// shows its own.
    pub(crate) seen_chips: HashSet<(u64, PathBuf)>,
    /// The repositories with nothing to do are hidden under their
    /// heading.
    pub(crate) clean_folded: bool,
    /// Where the tree stands, in lines.
    pub(crate) tree_scroll: Scroll,
    /// Where the log or the stashes stand, in rows.
    pub(crate) detail_scroll: Scroll,
    /// The tab asked for with Ctrl+D / Ctrl+T, else the diff.
    pub(crate) tab: Option<DetailTab>,
    /// The log or stashes read for, and what they say.
    pub(crate) detail: Option<(DetailKey, String)>,
    /// The diff tab: every change of every repository, one diff.
    pub(crate) diff: DiffView,
    pub(crate) hover: Option<ReposHit>,
    /// Why the last action wasn't done, under the field.
    pub(crate) error: Option<String>,
    pub(crate) sheet: Option<Sheet>,
    /// Where the selected line was among the tree's lines: what is
    /// selected once it is gone.
    pub(crate) selected_at: usize,
    /// The detail was read before the last Git status read: it is read
    /// again, still shown meanwhile.
    pub(crate) detail_stale: bool,
    /// The branch picker's menu, while open.
    pub(crate) branch_menu: Option<BranchMenu>,
}

/// The branch picker's menu: where its lines stand, and the branch Enter
/// picks, by index into `Panel::shown_branches`; what is typed to find
/// one, which has the keyboard.
#[derive(Clone, Debug, Default)]
pub struct BranchMenu {
    /// In lines (headings and branches).
    pub scroll: Scroll,
    pub picked: usize,
    pub query: TextField,
    pub action: MenuAction,
    /// Opened to create a branch (Ctrl+N): Enter creates the one typed,
    /// not the first branch found.
    pub create_first: bool,
    /// Enter creates the branch typed rather than picking `picked`.
    pub on_create: bool,
}

/// Where a repository has the branch about to be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// It is the branch checked out.
    On,
    Local,
    /// Only on the remote of this name.
    Remote(String),
}

/// What picking a branch in the branch menu does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuAction {
    /// Switch to it.
    #[default]
    Switch,
    /// Merge it into the branch checked out, or rebase that onto it.
    Integrate(Integrate),
}

/// A branch the picker offers: its names (more than one for a group of
/// `Ctx::same`) and what it is called.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Branch {
    names: Vec<String>,
    /// The name each repository has it by, if they all have the same;
    /// else the names joined: `main / master`.
    label: String,
    /// Whether a repository acted on has it as a local branch; else it is
    /// only on their remotes.
    local: bool,
}

/// A line of the branch menu's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuLine {
    /// The heading over a group: the local branches, or those only on a
    /// remote.
    Heading { local: bool },
    /// Branch `k` of `Panel::shown_branches`.
    Branch(usize),
}

/// The lines of the menu listing `branches`, which come local first: each
/// group under its heading.
fn menu_lines(branches: &[Branch]) -> Vec<MenuLine> {
    let mut lines = Vec::with_capacity(branches.len() + 2);
    for (k, b) in branches.iter().enumerate() {
        if k == 0 || branches[k - 1].local != b.local {
            lines.push(MenuLine::Heading { local: b.local });
        }
        lines.push(MenuLine::Branch(k));
    }
    lines
}

/// Where the menu standing at `pos` (in lines) stands moved the least to
/// show branch `k` of `lines` whole in a page of `page`, with its group's
/// heading when it is the group's first branch.
fn scroll_to(lines: &[MenuLine], k: usize, pos: f64, page: usize) -> f64 {
    let at = lines
        .iter()
        .position(|l| *l == MenuLine::Branch(k))
        .unwrap_or(0);
    let headed = at > 0 && matches!(lines[at - 1], MenuLine::Heading { .. });
    let top = if headed && page > 1 { at - 1 } else { at };
    showing(pos, (top, at), page)
}

/// Where a list of `len` rows with `page` of them in view stands by
/// `scroll`, as far as it goes: its first row in view and the part of
/// that row scrolled past.
fn standing(scroll: &Scroll, len: usize, page: usize) -> (usize, f64) {
    let last = len.saturating_sub(page.max(1));
    let pos = scroll.pos().clamp(0.0, last as f64);
    (pos.floor() as usize, pos.fract())
}

/// The names of the branch `name` is, by `same`: its group, else itself.
fn names_of(name: &str, same: &[Vec<String>]) -> Vec<String> {
    same.iter()
        .find(|group| group.iter().any(|n| n == name))
        .cloned()
        .unwrap_or_else(|| vec![name.to_owned()])
}

/// Who the branch `name` is across repositories: its names joined, as
/// `main / master`. Git allows no spaces in a name, so no branch is called
/// that.
pub fn branch_key(name: &str, same: &[Vec<String>]) -> String {
    names_of(name, same).join(" / ")
}

/// The one branch `names` are, one per repository: the name, or the names
/// joined when they differ but are one branch (`main / master`); none when
/// a repository has none, or they are different branches.
fn one_name<'a>(
    names: impl IntoIterator<Item = Option<&'a str>>,
    same: &[Vec<String>],
) -> Option<String> {
    let mut names = names.into_iter().collect::<Option<Vec<_>>>()?;
    names.sort_unstable();
    names.dedup();
    let (first, rest) = names.split_first()?;
    let key = branch_key(first, same);
    rest.iter()
        .all(|n| branch_key(n, same) == key)
        .then(|| names.join(" / "))
}

/// The first of `names` that `status` has as a local branch, else the
/// first its remote has: what a switch to them goes to.
fn name_in<'a>(status: &GitStatus, names: &'a [String]) -> Option<&'a str> {
    let has = |list: &[String], name: &String| list.contains(name);
    names
        .iter()
        .find(|n| has(&status.local_branches, n))
        .or_else(|| names.iter().find(|n| has(&status.remote_branches, n)))
        .map(String::as_str)
}

/// A question over the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    /// Before `kind` over `jobs`, which can't be taken back: what it does
    /// to each repository, and the button Enter presses: 0 does it, 1
    /// cancels.
    Confirm {
        kind: BatchKind,
        jobs: Vec<Job>,
        lines: Vec<(String, String, Tone)>,
        choice: usize,
    },
    /// The panel's keys.
    Help { choice: usize },
    /// Before dropping the review's comments.
    ClearReview { choice: usize },
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
    /// Branch `name`, about to be created in `jobs`, is in some of them
    /// already (`have`): switch to it, create it where it is missing, or
    /// edit the name.
    Exists {
        name: String,
        jobs: Vec<Job>,
        have: Vec<(PathBuf, Found)>,
        choice: usize,
    },
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
    /// Switch to the first of these names (one branch, as `main` and
    /// `master`) the repository has as a branch, else the first its remote
    /// has.
    Switch(Vec<String>),
    /// Delete those of these names (one branch) the repository has as
    /// local branches; unless `force`, only those losing no commit.
    DeleteBranch {
        names: Vec<String>,
        force: bool,
    },
    Commit {
        msg: String,
        push: bool,
    },
    Push {
        force: bool,
    },
    Fetch,
    Pull(Pull),
    /// Bring the branch `from` in (one branch, as `main` and `master`),
    /// else the default branch.
    Integrate {
        how: Integrate,
        from: Option<Vec<String>>,
    },
    MarkResolved,
    /// Carry on with the operation stopped on conflicts.
    Continue,
    Abort,
    /// Undo the last batch.
    Undo,
    /// Stash the uncommitted changes, untracked files too.
    Stash,
    /// Bring the newest stash back, and drop it.
    StashPop,
    /// Drop the newest stash.
    StashDrop,
}

impl BatchKind {
    /// What it is called in the result bar and toasts: `Commit and push`.
    pub fn label(&self) -> String {
        match self {
            BatchKind::Branch(_) => "Branch",
            BatchKind::Switch(_) => "Switch branch",
            BatchKind::DeleteBranch { .. } => "Delete branch",
            BatchKind::Commit { push: false, .. } => "Commit",
            BatchKind::Commit { push: true, .. } => "Commit and push",
            BatchKind::Push { force: false } => "Push",
            BatchKind::Push { force: true } => "Force push",
            BatchKind::Fetch => "Fetch",
            BatchKind::Pull(Pull::Rebase) => "Pull (rebase)",
            BatchKind::Pull(_) => "Pull",
            BatchKind::Integrate { how, from } => {
                let from = from.as_ref().map(|names| names.join(" / "));
                return match (how, from) {
                    (Integrate::Merge, None) => "Merge base".into(),
                    (Integrate::Rebase, None) => "Rebase on base".into(),
                    (Integrate::Merge, Some(from)) => format!("Merge {from}"),
                    (Integrate::Rebase, Some(from)) => format!("Rebase onto {from}"),
                };
            }
            BatchKind::MarkResolved => "Mark resolved",
            BatchKind::Continue => "Continue",
            BatchKind::Abort => "Abort",
            BatchKind::Undo => "Undo",
            BatchKind::Stash => "Stash",
            BatchKind::StashPop => "Pop stash",
            BatchKind::StashDrop => "Drop stash",
        }
        .into()
    }

    /// What its chip says where it went through: `pushed`; the label says
    /// the rest in the tip.
    pub fn done(&self) -> &'static str {
        match self {
            BatchKind::Branch(_) | BatchKind::Switch(_) => "switched",
            BatchKind::DeleteBranch { .. } => "deleted",
            BatchKind::Commit { push: false, .. } => "committed",
            BatchKind::Commit { push: true, .. } | BatchKind::Push { force: false } => "pushed",
            BatchKind::Push { force: true } => "force pushed",
            BatchKind::Fetch => "fetched",
            BatchKind::Pull(Pull::Rebase) => "pulled (rebase)",
            BatchKind::Pull(_) => "pulled",
            BatchKind::Integrate {
                how: Integrate::Merge,
                ..
            } => "merged",
            BatchKind::Integrate {
                how: Integrate::Rebase,
                ..
            } => "rebased",
            BatchKind::MarkResolved => "resolved",
            BatchKind::Continue => "continued",
            BatchKind::Abort => "aborted",
            BatchKind::Undo => "undone",
            BatchKind::Stash => "stashed",
            BatchKind::StashPop => "popped",
            BatchKind::StashDrop => "dropped",
        }
    }

    /// Whether it can't be taken back, so is asked about first: a force
    /// push, an abort, an undo, a branch or a stash dropped.
    pub fn confirmed_first(&self) -> bool {
        matches!(
            self,
            BatchKind::Push { force: true }
                | BatchKind::Abort
                | BatchKind::Undo
                | BatchKind::DeleteBranch { .. }
                | BatchKind::StashDrop
        )
    }

    /// Whether it is checked first (merge, rebase, pull; a branch switch
    /// with uncommitted changes), the result asked about on a sheet.
    ///
    /// The kind alone can't tell whether a switch carries changes, so the
    /// switches all say yes: only their dirty repositories are checked. A
    /// fast-forward pull isn't: it fails cleanly, and its diverged
    /// repositories are asked about instead.
    pub fn checked_first(&self) -> bool {
        matches!(
            self,
            BatchKind::Branch(_)
                | BatchKind::Switch(_)
                | BatchKind::DeleteBranch { .. }
                | BatchKind::Pull(Pull::Rebase | Pull::Merge)
                | BatchKind::Integrate { .. }
        )
    }

    /// Whether it can be undone (it moved only local branches).
    pub fn undoable(&self) -> bool {
        matches!(
            self,
            BatchKind::Branch(_)
                | BatchKind::Switch(_)
                | BatchKind::Commit { push: false, .. }
                | BatchKind::Pull(_)
                | BatchKind::Integrate { .. }
        )
    }
}

/// How many of the branch menu's rows are in view: no more than the tree's,
/// less one for its search, and one for its row to create a branch when
/// `create` shows it.
fn branch_page(ctx: &Ctx, create: bool) -> usize {
    let above = 1 + usize::from(create);
    BRANCH_MENU_ROWS
        .min(ctx.page.0.saturating_sub(above))
        .max(1)
}

/// `n` and `one`, or its plural with an `s`: `1 conflict`, `2 conflicts`.
fn count(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
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
        self.runs.iter().all(|r| r.state.over())
    }

    /// The result bar's line: `Commit and push: 2 done · 1 conflict ·
    /// 1 failed`, with why when only one failed (`Push: 1 failed: cannot
    /// lock ref …`), and its colour.
    pub fn summary(&self) -> (String, Tone) {
        let (mut done, mut conflicts, mut failed, mut skipped, mut going) = (0, 0, 0, 0, 0);
        for run in &self.runs {
            match &run.state {
                RunState::Queued | RunState::Running(_) => going += 1,
                RunState::Done => done += 1,
                RunState::Failed {
                    failure: Failure::Conflict(_),
                    ..
                } => conflicts += 1,
                RunState::Failed { .. } => failed += 1,
                RunState::Skipped(_) => skipped += 1,
            }
        }
        let label = self.kind.label();
        if self.runs.is_empty() {
            return (format!("{label}: nothing to do"), Tone::Muted);
        }
        let mut parts = Vec::new();
        if done > 0 {
            parts.push(format!("{done} done"));
        }
        if conflicts > 0 {
            parts.push(count(conflicts, "conflict"));
        }
        // One failure says why, here and in the toast when the panel is
        // closed.
        let whys: Vec<&str> = self.runs.iter().filter_map(RepoRun::failed_why).collect();
        match whys[..] {
            [why] if going == 0 && failed == 1 => parts.push(format!("1 failed: {why}")),
            _ if failed > 0 => parts.push(format!("{failed} failed")),
            _ => {}
        }
        if skipped > 0 {
            parts.push(format!("{skipped} skipped"));
        }
        if going > 0 {
            parts.push(format!("{going} of {} running", self.runs.len()));
        }
        let tone = if going > 0 {
            Tone::Normal
        } else if failed > 0 {
            Tone::Error
        } else if conflicts > 0 || skipped > 0 {
            Tone::Warning
        } else {
            Tone::Success
        };
        (format!("{label}: {}", parts.join(" · ")), tone)
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

impl RepoRun {
    /// Why it failed, other than on conflicts, when git said.
    fn failed_why(&self) -> Option<&str> {
        match &self.state {
            RunState::Failed {
                failure: Failure::Conflict(_),
                ..
            } => None,
            RunState::Failed { why, .. } => Some(why.trim()).filter(|w| !w.is_empty()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    Queued,
    /// Doing this: `pushing…`.
    Running(String),
    Done,
    /// Stopped: `Failure::Conflict` leaves it to resolve.
    Failed {
        failure: Failure,
        why: String,
    },
    /// Not tried: why (`nothing checked`, `blocked: detached HEAD`).
    Skipped(String),
}

impl RunState {
    /// It is done with, one way or another.
    fn over(&self) -> bool {
        !matches!(self, RunState::Queued | RunState::Running(_))
    }

    /// It failed with `failure`.
    fn failure(&self) -> Option<&Failure> {
        match self {
            RunState::Failed { failure, .. } => Some(failure),
            _ => None,
        }
    }
}

/// What a detail read is for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DetailKey {
    pub root: PathBuf,
    /// The file whose diff it is; `None` for the repository's log or
    /// stashes.
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
        before: Option<Head>,
        after: Option<Head>,
    },
    /// The fetch on opening the panel is done.
    Fetched,
    /// The check before batch `kind` over `jobs`.
    Checked {
        id: u64,
        kind: BatchKind,
        jobs: Vec<Job>,
        verdicts: Vec<(PathBuf, Verdict)>,
    },
    /// The look, after a fetch, for branch `name` before creating it in
    /// `jobs`: the repositories that have it.
    Found {
        id: u64,
        name: String,
        jobs: Vec<Job>,
        have: Vec<(PathBuf, Found)>,
    },
    Detail {
        key: DetailKey,
        text: String,
    },
    /// A repository's diff, read for Git status read `generation`.
    Diffs {
        root: PathBuf,
        generation: u64,
        files: Vec<FileDiff>,
    },
    /// A worktree's changes since it forked, and their diff, read for its
    /// diff (`WorktreeDiff`); or why they couldn't be.
    WorktreeDiff {
        root: PathBuf,
        read: Result<(crate::git_ops::Since, Vec<FileDiff>), String>,
    },
}

/// What the panel asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    /// Run `kind` over `jobs`, checking first if `kind.checked_first()`.
    Batch {
        kind: BatchKind,
        jobs: Vec<Job>,
    },
    /// Run `kind` over `jobs`, the check done (a sheet's answer).
    Confirmed {
        kind: BatchKind,
        jobs: Vec<Job>,
    },
    /// Fetch, then create branch `name` in `jobs` unless some have it
    /// already: then ask what to do.
    CreateBranch {
        name: String,
        jobs: Vec<Job>,
    },
    /// Run the last batch again over the repositories it failed in.
    RetryFailed,
    /// Put the review, as Markdown, on the clipboard: `count` comments.
    Copy {
        text: String,
        count: usize,
    },
    /// Open these files of the repository at `root` in its workspace's
    /// Zed, the first at `at` (else where Zed puts it).
    OpenInZed {
        root: PathBuf,
        paths: Vec<String>,
        at: Option<diff_view::Position>,
    },
    /// Run the command that failed in the repository at `root` in a new
    /// tab, for the user to answer its prompts.
    RunInTerminal(PathBuf),
    /// Show the workspace of the worktree at this path (where a branch is
    /// checked out already).
    OpenWorkspace(PathBuf),
    /// The files are listed this way now: remember it.
    Grouping(Grouping),
    /// Say this briefly.
    Notice(&'static str),
    /// Cut this out of a file, as a delete in the diff asks.
    Cut(FileEdit),
    /// Keep what this would cut, to paste, and copy it.
    Yank(FileEdit),
    /// Put what was yanked or deleted last in a file.
    Paste(Put),
    /// Undo the last cut; do the last undone again.
    UndoCut,
    RedoCut,
}

/// A tree line, by index: a repository of `Ctx::items`, a folder of its
/// changes, or one of its changes, by index into its `GitStatus::changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Repo(usize),
    /// A folder of repository `k`, `depth` folders in: the first `to`
    /// parts of the path of change `c` (the first file under it), shown
    /// as its parts `from..to`, as a folder holding nothing but one folder
    /// is one line with it.
    Dir {
        k: usize,
        c: usize,
        from: usize,
        to: usize,
        depth: usize,
    },
    /// Change `c` of repository `k`, `depth` folders in.
    File(usize, usize, usize),
    /// The heading over the repositories with nothing to do.
    Clean,
}

impl Row {
    /// The repository it is, or is under; none for the heading.
    fn item(self) -> Option<usize> {
        match self {
            Row::Repo(k) | Row::Dir { k, .. } | Row::File(k, ..) => Some(k),
            Row::Clean => None,
        }
    }

    /// How many folders in it is; a repository is none.
    fn depth(self) -> usize {
        match self {
            Row::Repo(_) | Row::Clean => 0,
            Row::Dir { depth, .. } | Row::File(.., depth) => depth,
        }
    }
}

/// Parts `from..to` of `path`, split at `/`.
fn path_parts(path: &str, from: usize, to: usize) -> &str {
    // Where part `n` starts.
    let cut = |n: usize| match n {
        0 => Some(0),
        n => path.match_indices('/').nth(n - 1).map(|(i, _)| i + 1),
    };
    let start = cut(from).unwrap_or(path.len());
    let end = cut(to).map_or(path.len(), |i| i - 1);
    &path[start..end.max(start)]
}

/// A folder of changed files: the folders in it, its files by name with
/// their index into the changes, and the same for the folders git lists
/// whole, as one change (untracked ones, `new/`).
#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    files: Vec<(&'a str, usize)>,
    whole: Vec<(&'a str, usize)>,
}

/// Names in the order the tree lists them: letter case aside first.
fn by_name(a: &str, b: &str) -> Ordering {
    let lower = |s: &str| s.to_lowercase();
    lower(a).cmp(&lower(b)).then_with(|| a.cmp(b))
}

/// Repository `k`'s file lines, folded or not: as `file_order` has them;
/// or by folder, each folder's folders before its files, by name.
fn file_rows(k: usize, changes: &[Change], grouping: Grouping) -> Vec<Row> {
    if grouping == Grouping::List {
        let order = file_order(changes).into_iter();
        return order.map(|c| Row::File(k, c, 0)).collect();
    }
    let mut top = Folder::default();
    for (c, change) in changes.iter().enumerate() {
        let path = change.path.strip_suffix('/');
        let whole = path.is_some();
        let (dirs, name) = match path.unwrap_or(&change.path).rsplit_once('/') {
            Some((dirs, name)) => (Some(dirs), name),
            None => (None, path.unwrap_or(&change.path)),
        };
        let mut folder = &mut top;
        for part in dirs.into_iter().flat_map(|d| d.split('/')) {
            folder = folder.folders.entry(part).or_default();
        }
        match whole {
            true => folder.whole.push((name, c)),
            false => folder.files.push((name, c)),
        }
    }
    let mut rows = Vec::new();
    folder_rows(&top, k, (0, 0), &mut rows);
    rows
}

/// The order of `changes` in the tree, by index, folded or not.
fn tree_order(changes: &[Change], grouping: Grouping) -> Vec<usize> {
    let rows = file_rows(0, changes, grouping).into_iter();
    rows.filter_map(|r| match r {
        Row::File(_, c, _) => Some(c),
        _ => None,
    })
    .collect()
}

/// The lines of `folder` of repository `k`, `at` parts down its paths
/// and `depth` folders in, onto `rows`.
fn folder_rows(folder: &Folder, k: usize, (at, depth): (usize, usize), rows: &mut Vec<Row>) {
    // The folders git lists whole go by name among the others, a line
    // each.
    let mut folders: Vec<(&str, Option<&Folder>, usize)> = folder
        .folders
        .iter()
        .map(|(name, sub)| (*name, Some(sub), 0))
        .chain(folder.whole.iter().map(|&(name, c)| (name, None, c)))
        .collect();
    folders.sort_by(|a, b| by_name(a.0, b.0));
    for (_, sub, c) in folders {
        let Some(mut sub) = sub else {
            rows.push(Row::File(k, c, depth));
            continue;
        };
        let mut to = at + 1;
        while sub.files.is_empty()
            && sub.whole.is_empty()
            && sub.folders.len() == 1
            && let Some(only) = sub.folders.values().next()
        {
            sub = only;
            to += 1;
        }
        let line = rows.len();
        rows.push(Row::Dir {
            k,
            c: 0,
            from: at,
            to,
            depth,
        });
        folder_rows(sub, k, (to, depth + 1), rows);
        let first = rows[line..].iter().find_map(|r| match r {
            Row::File(_, c, _) => Some(*c),
            _ => None,
        });
        if let (Row::Dir { c, .. }, Some(first)) = (&mut rows[line], first) {
            *c = first;
        }
    }
    let mut files = folder.files.clone();
    files.sort_by(|a, b| by_name(a.0, b.0));
    rows.extend(files.into_iter().map(|(_, c)| Row::File(k, c, depth)));
}

/// The diff's `Ctx` for panel `$panel` over the panel's `$ctx`, as
/// `$dctx`: its repositories with changes as sections, each file's
/// checkbox its tree line's.
macro_rules! diff_ctx {
    ($panel:ident, $ctx:expr, $dctx:ident) => {
        let sections = sections($ctx.items);
        let checked = &$panel.checked_files;
        let check = move |root: &Path, path: &str| file_check(checked, root, path);
        let $dctx = diff_view::Ctx {
            sections: &sections,
            page: $ctx.page.1,
            cols: $ctx.cols,
            density: $ctx.density,
            keys: diff_keys($panel.focus),
            lead: "All changes",
            empty: "No changes in any repository",
            count_sections: true,
            check: Some(&check),
            selected: selected_file(&$panel.selected),
        };
    };
}

/// The diff's sections for `items`: one per repository.
fn sections(items: &[RepoItem]) -> Vec<Section<'_>> {
    items
        .iter()
        .map(|i| Section {
            root: &i.root,
            label: &i.label,
            branch: i.status.branch.as_deref(),
            changes: &i.status.changes,
        })
        .collect()
}

/// The checkbox of the file at `path` of the repository at `root`, by
/// those `checked`.
fn file_check(checked: &HashSet<(PathBuf, String)>, root: &Path, path: &str) -> Check {
    // Asked per file per frame: no copies while nothing is checked.
    if checked.is_empty() {
        return Check::Off;
    }
    match checked.contains(&(root.to_path_buf(), path.to_owned())) {
        true => Check::On,
        false => Check::Off,
    }
}

/// Where the diff has the keyboard, by the panel's focus.
fn diff_keys(focus: Focus) -> Keys {
    match focus {
        Focus::Detail => Keys::Diff,
        Focus::Tree => Keys::Other,
        Focus::Field => Keys::Field,
    }
}

/// The selected tree line, while a file.
fn selected_file(selected: &Option<Selected>) -> Option<(&Path, &str)> {
    match selected {
        Some(Selected::File(root, path)) => Some((root, path)),
        _ => None,
    }
}

/// What the diff asked for, as the panel asks it; a file's check is the
/// panel's own (`Panel::from_diff`).
pub fn from_diff(outcome: diff_view::Outcome) -> Outcome {
    use diff_view::Outcome as D;
    match outcome {
        D::Copy { text, count } => Outcome::Copy { text, count },
        D::OpenInZed { root, paths, at } => Outcome::OpenInZed { root, paths, at },
        D::Notice(notice) => Outcome::Notice(notice),
        D::Cut(edit) => Outcome::Cut(edit),
        D::Yank(edit) => Outcome::Yank(edit),
        D::Paste(put) => Outcome::Paste(put),
        D::UndoCut => Outcome::UndoCut,
        D::RedoCut => Outcome::RedoCut,
        D::Toggle { .. } | D::Clear => Outcome::Notice("Done in the panel"),
    }
}

/// Why an action wasn't done.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Why {
    /// What is typed is the trouble (nothing, a bad name): the field gets
    /// the keyboard, to fix it.
    Field(String),
    Other(String),
}

impl Why {
    /// Not done for a reason other than the field.
    fn other(why: impl Into<String>) -> Why {
        Why::Other(why.into())
    }
}

/// The action a key asks for, with either focus: before the field, as
/// Ctrl+Shift+Delete would otherwise delete a word there.
fn action_key(mods: &ModifiersState, sym: Keysym) -> Option<ReposButton> {
    use ReposButton as B;
    if mods.logo {
        return None;
    }
    let shift = mods.shift;
    if sym == Keysym::F1 && !mods.ctrl && !mods.alt {
        return Some(B::Help);
    }
    if mods.alt && !mods.ctrl {
        return match sym {
            Keysym::z | Keysym::Z => Some(B::Undo),
            Keysym::r | Keysym::R => Some(B::RetryFailed),
            Keysym::m | Keysym::M if shift => Some(B::RebaseFrom),
            Keysym::m | Keysym::M => Some(B::MergeFrom),
            Keysym::s | Keysym::S if shift => Some(B::StashPop),
            Keysym::s | Keysym::S => Some(B::Stash),
            _ => None,
        };
    }
    if !mods.ctrl || mods.alt {
        return None;
    }
    // Shift changes the symbol: match both cases.
    let button = match sym {
        Keysym::Return | Keysym::KP_Enter if shift => B::CommitPush,
        Keysym::Return | Keysym::KP_Enter => B::Commit,
        Keysym::c | Keysym::C if shift => B::CopyReview,
        Keysym::p | Keysym::P if shift => B::ForcePush,
        Keysym::p | Keysym::P => B::Push,
        Keysym::f | Keysym::F if !shift => B::Fetch,
        Keysym::l | Keysym::L => B::Pull,
        Keysym::m | Keysym::M if shift => B::Rebase,
        Keysym::m | Keysym::M => B::Merge,
        Keysym::e | Keysym::E => B::OpenInZed,
        Keysym::r | Keysym::R => B::MarkResolved,
        Keysym::g | Keysym::G => B::Continue,
        Keysym::Delete | Keysym::KP_Delete if shift => B::Abort,
        _ => return None,
    };
    Some(button)
}

/// Whether `item` is stopped on conflicts: an operation half-way, or
/// conflicted files.
fn in_conflict(item: &RepoItem) -> bool {
    item.status.op.is_some() || item.status.changes.iter().any(Change::conflicted)
}

/// `button`'s key, as the footer shows it; empty for those without one.
fn key_name(button: ReposButton) -> &'static str {
    use ReposButton as B;
    match button {
        B::Commit => "Ctrl+Enter",
        B::CommitPush => "Ctrl+Shift+Enter",
        B::Push => "Ctrl+P",
        B::ForcePush => "Ctrl+Shift+P",
        B::Fetch => "Ctrl+F",
        B::Pull => "Ctrl+L",
        B::Merge => "Ctrl+M",
        B::Rebase => "Ctrl+Shift+M",
        B::MergeFrom => "Alt+M",
        B::RebaseFrom => "Alt+Shift+M",
        B::OpenInZed => "Ctrl+E",
        B::MarkResolved => "Ctrl+R",
        B::Continue => "Ctrl+G",
        B::Abort => "Ctrl+Shift+Del",
        B::Undo => "Alt+Z",
        B::RetryFailed => "Alt+R",
        B::Stash => "Alt+S",
        B::StashPop => "Alt+Shift+S",
        B::Help => "F1",
        B::CopyReview => "Ctrl+Shift+C",
        B::Comment => "C",
        B::Restore => "R",
        B::SaveComment => "Enter",
        B::DiscardComment => "Esc",
        B::All | B::CleanOnly | B::PullRebase | B::PullMerge => "Enter",
        B::SkipDiverged | B::Cancel | B::Close | B::EditName => "Esc",
        B::RunInTerminal
        | B::OpenWorkspace
        | B::StashDrop
        | B::DeleteBranch
        | B::SwitchExisting
        | B::CreateMissing
        | B::ClearReview
        | B::DeleteComment => "",
    }
}

/// `label` with its first letter in lower case, as the footer's actions
/// are written (`open in Zed`).
fn lower_first(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// How `button` brings in a branch picked in the branch menu, if it opens
/// that menu.
fn menu_integrate(button: ReposButton) -> Option<Integrate> {
    match button {
        ReposButton::MergeFrom => Some(Integrate::Merge),
        ReposButton::RebaseFrom => Some(Integrate::Rebase),
        _ => None,
    }
}

/// The button that does `kind` on its confirm sheet.
fn confirm_button(kind: &BatchKind) -> ReposButton {
    match kind {
        BatchKind::Push { force: true } => ReposButton::ForcePush,
        BatchKind::Abort => ReposButton::Abort,
        BatchKind::Undo => ReposButton::Undo,
        BatchKind::DeleteBranch { .. } => ReposButton::DeleteBranch,
        BatchKind::StashDrop => ReposButton::StashDrop,
        _ => ReposButton::Cancel,
    }
}

/// The buttons `sheet` has, in order.
fn sheet_buttons(sheet: &Sheet) -> Vec<ReposButton> {
    match sheet {
        Sheet::Preflight { .. } => vec![
            ReposButton::All,
            ReposButton::CleanOnly,
            ReposButton::Cancel,
        ],
        Sheet::Diverged { .. } => vec![
            ReposButton::PullRebase,
            ReposButton::PullMerge,
            ReposButton::SkipDiverged,
        ],
        Sheet::Confirm { kind, .. } => vec![confirm_button(kind), ReposButton::Cancel],
        Sheet::Help { .. } => vec![ReposButton::Close],
        Sheet::ClearReview { .. } => vec![ReposButton::ClearReview, ReposButton::Cancel],
        // Created where missing only when some are.
        Sheet::Exists { jobs, have, .. } => {
            let mut buttons = vec![ReposButton::SwitchExisting];
            if !missing(jobs, have).is_empty() {
                buttons.push(ReposButton::CreateMissing);
            }
            buttons.push(ReposButton::EditName);
            buttons
        }
    }
}

/// A key on a sheet with `buttons`, `choice` the one Enter presses: the
/// arrows and Tab move it, Enter presses it, Esc presses Cancel. The
/// button pressed, if any.
pub fn sheet_step(
    choice: &mut usize,
    buttons: &[ReposButton],
    mods: &ModifiersState,
    sym: Keysym,
) -> Option<ReposButton> {
    let n = buttons.len();
    if n == 0 {
        return None;
    }
    match sym {
        Keysym::Left | Keysym::KP_Left | Keysym::ISO_Left_Tab => *choice = (*choice + n - 1) % n,
        Keysym::Tab if mods.shift => *choice = (*choice + n - 1) % n,
        Keysym::Right | Keysym::KP_Right | Keysym::Tab => *choice = (*choice + 1) % n,
        Keysym::Return | Keysym::KP_Enter => return Some(buttons[(*choice).min(n - 1)]),
        Keysym::Escape => return Some(ReposButton::Cancel),
        _ => {}
    }
    None
}

/// Those of `jobs` without the branch `have` lists.
fn missing(jobs: &[Job], have: &[(PathBuf, Found)]) -> Vec<Job> {
    jobs.iter()
        .filter(|j| !have.iter().any(|(root, _)| *root == j.root))
        .cloned()
        .collect()
}

/// The panel's keys, as the help sheet lists them: the key, what it does.
const HELP_KEYS: &[(&str, &str)] = &[
    ("Tab", "the tree, the diff, the field"),
    ("Up / Down", "select a line; in the diff, move the cursor"),
    ("PgUp / PgDn", "a page of lines"),
    (
        "Shift+Up / Down",
        "scroll the diff or log; in the diff, a range",
    ),
    ("Alt+Up / Down", "in the diff, the file before or after"),
    (
        "C / Del",
        "in the diff, comment on the lines; delete a comment",
    ),
    ("Shift+Enter", "in a comment, a new line; Enter keeps it"),
    ("Ctrl+Shift+C", "copy the review comments for an AI agent"),
    ("Space", "check the line, or the diff's file"),
    ("Ctrl+A", "check everything, or nothing"),
    (
        "Left / Right",
        "fold a repository, a folder or a diff file header; move the caret",
    ),
    ("Enter", "open the file in Zed; fold a repository or folder"),
    (
        "g Space / Ctrl+click",
        "in the diff, open the code in Zed at the caret; on a file's name, the file",
    ),
    ("Alt+F", "list the files by folder, or flat"),
    ("Alt+Left / Alt+Right", "fold everything, unfold everything"),
    ("Ctrl+D / Ctrl+T", "the diff, the log"),
    (
        "/ / Ctrl+Shift+F",
        "find in the diff: its lines, paths, hunks",
    ),
    (
        "Enter / Shift+Enter",
        "in the search, the next place, the one before",
    ),
    (
        "n / N, F3 / Shift+F3",
        "the next place found, the one before",
    ),
    (
        "h j k l w b e 0 ^ $",
        "in the diff, move the caret, as in vim",
    ),
    ("* / #", "find the word at the caret, back for #"),
    (
        "x X D dd dw de d$ 3dd",
        "in the diff, delete from the file, as in vim",
    ),
    (
        "v / V, then d",
        "select by chars, by lines; delete it from the file",
    ),
    (
        "yy yw y$ Y, p / P",
        "in the diff, yank (and copy); paste after, before",
    ),
    (
        "R",
        "in the diff, restore the change at the caret or the lines selected",
    ),
    (
        "u / U",
        "undo the last delete, paste or restore, do it again",
    ),
    ("Ctrl+S", "switch them all to a branch"),
    ("Ctrl+N", "create a branch: type its name"),
    ("Ctrl+Enter", "in the branch menu, create the branch typed"),
    ("Del", "delete the branch picked in the menu"),
    ("Alt+Z / Alt+R", "undo the last batch, retry what failed"),
    ("Right click", "the actions, on a line or anywhere"),
    ("Esc", "close"),
];

/// `n` repositories, in words.
fn repos(n: usize) -> String {
    if n == 1 {
        "1 repository".into()
    } else {
        format!("{n} repositories")
    }
}

/// The order `changes` are listed in, by index: conflicted ones first,
/// then the rest as git lists them.
fn file_order(changes: &[Change]) -> Vec<usize> {
    let (mut order, mut rest): (Vec<usize>, Vec<usize>) =
        (0..changes.len()).partition(|&c| changes[c].conflicted());
    order.append(&mut rest);
    order
}

/// The conflicted files of `item`.
fn conflicted_files(item: &RepoItem) -> Vec<String> {
    let changes = &item.status.changes;
    changes
        .iter()
        .filter(|c| c.conflicted())
        .map(|c| c.path.clone())
        .collect()
}

/// How a change is shown: its path, a rename as `old → new`.
fn change_text(change: &Change) -> String {
    match &change.orig {
        Some(orig) => format!("{orig} → {}", change.path),
        None => change.path.clone(),
    }
}

/// How a change is shown under its folder: its name, a rename as
/// `old → new`, the old one by its name too when in the same folder.
fn file_name_text(change: &Change) -> String {
    let split = |path: &str| -> (String, String) {
        match path.rsplit_once('/') {
            Some((dir, name)) => (dir.to_owned(), name.to_owned()),
            None => (String::new(), path.to_owned()),
        }
    };
    // A folder git lists whole keeps its `/`.
    let (dir, mut name) = split(change.path.trim_end_matches('/'));
    if change.path.ends_with('/') {
        name.push('/');
    }
    match &change.orig {
        Some(orig) if split(orig).0 == dir => format!("{} → {name}", split(orig).1),
        Some(orig) => format!("{orig} → {name}"),
        None => name,
    }
}

/// How long before `now` `then` was, in its largest unit: `2m`.
fn ago(then: SystemTime, now: SystemTime) -> String {
    let s = now.duration_since(then).map_or(0, |d| d.as_secs());
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// Where `head` is, in words: `main at 1a2b3c4`.
fn head_text(head: &Head) -> String {
    let oid = head.oid.as_deref().map(|o| &o[..o.len().min(7)]);
    match (&head.branch, oid) {
        (Some(branch), Some(oid)) => format!("{branch} at {oid}"),
        (Some(branch), None) => branch.clone(),
        (None, Some(oid)) => oid.into(),
        (None, None) => "nothing".into(),
    }
}

/// The repositories `batch` can be undone in, as its jobs: those it was
/// done in, where it was before and after is known. Else why not.
fn undo_jobs(batch: Option<&Batch>) -> Result<Vec<Job>, String> {
    let Some(batch) = batch else {
        return Err("Nothing to undo".into());
    };
    if !batch.finished() {
        return Err("Wait for the batch to finish".into());
    }
    if !batch.kind.undoable() {
        return Err(format!("{} can't be undone", batch.kind.label()));
    }
    let jobs: Vec<Job> = batch
        .runs
        .iter()
        .filter(|r| r.state == RunState::Done && r.before.is_some() && r.after.is_some())
        .map(|r| Job {
            root: r.root.clone(),
            files: Vec::new(),
        })
        .collect();
    if jobs.is_empty() {
        return Err("Nothing to undo".into());
    }
    Ok(jobs)
}

/// Whether `batch` is over with something failed that trying again could
/// fix: conflicts are resolved instead.
fn retryable(batch: Option<&Batch>) -> bool {
    // An undo again would undo from where the failed one left it.
    batch.is_some_and(|b| {
        b.kind != BatchKind::Undo
            && b.finished()
            && b.runs.iter().any(|r| {
                r.state
                    .failure()
                    .is_some_and(|f| !matches!(f, Failure::Conflict(_)))
            })
    })
}

impl Panel {
    /// Stop any of the lists flung; whether it was.
    pub fn stop_fling(&mut self) -> bool {
        let menu = (self.branch_menu.as_mut()).is_some_and(|menu| menu.scroll.stop_fling());
        // Every one of them, not just up to the first flung.
        [
            self.tree_scroll.stop_fling(),
            self.detail_scroll.stop_fling(),
            self.diff.scroll.stop_fling(),
            menu,
        ]
        .contains(&true)
    }

    /// A panel over `items`, nothing checked: repositories with changes
    /// unfolded unless they have `FOLD_FILES` or more, the first selected.
    pub fn new(items: &[RepoItem]) -> Panel {
        let mut panel = Panel::default();
        panel.stir(items);
        let first = panel.rows(items).first().and_then(|r| r.item());
        panel.selected = first.map(|k| Selected::Repo(items[k].root.clone()));
        panel.diff.rebuild(&sections(items));
        panel
    }

    /// The diff's search has the keyboard.
    fn search_has_keys(&self) -> bool {
        self.focus == Focus::Detail && self.tab() == DetailTab::Diff && self.diff.search_focused()
    }

    /// Scroll the diff to the selected line's repository or file.
    fn show_selected(&mut self) {
        if self.tab() != DetailTab::Diff {
            return;
        }
        let place = match &self.selected {
            Some(Selected::Repo(root)) => Place::Section(root),
            Some(Selected::File(root, path)) => Place::File(root, path),
            Some(Selected::Dir(root, dir)) => Place::Dir(root, dir),
            Some(Selected::Clean) | None => return,
        };
        self.diff.show(place);
    }

    /// Whether `item`'s files are shown.
    fn open(&self, item: &RepoItem) -> bool {
        let n = item.status.changes.len();
        n > 0
            && (self.unfolded.contains(&item.root)
                || (!self.folded.contains(&item.root) && n < FOLD_FILES))
    }

    /// Show or hide `item`'s files.
    fn set_open(&mut self, item: &RepoItem, open: bool) {
        if item.status.changes.is_empty() {
            return;
        }
        let root = item.root.clone();
        if open {
            self.folded.remove(&root);
            self.unfolded.insert(root);
        } else {
            self.unfolded.remove(&root);
            self.folded.insert(root);
        }
    }

    /// The tree's lines, in `items`' order: each repository, then its
    /// files when unfolded, as `file_rows` has them, but those in folded
    /// folders; the repositories with nothing to do last. Never sorted by
    /// state otherwise, so that lines don't jump as a batch goes.
    fn rows(&self, items: &[RepoItem]) -> Vec<Row> {
        let mut rows = Vec::new();
        // Those with nothing to do go last, under a heading, when there
        // are others to go after.
        let (clean, busy): (Vec<usize>, Vec<usize>) =
            (0..items.len()).partition(|&k| self.is_clean(&items[k]));
        let grouped = !clean.is_empty() && !busy.is_empty();
        let order = match grouped {
            true => busy,
            false => (0..items.len()).collect(),
        };
        for k in order {
            let item = &items[k];
            rows.push(Row::Repo(k));
            if !self.open(item) {
                continue;
            }
            // The depth of the folded folder whose lines are skipped.
            let mut hiding = None;
            for row in file_rows(k, &item.status.changes, self.grouping) {
                if hiding.is_some_and(|d| row.depth() > d) {
                    continue;
                }
                hiding = None;
                if !self.dir_open(row, items) {
                    hiding = Some(row.depth());
                }
                rows.push(row);
            }
        }
        if grouped {
            rows.push(Row::Clean);
            if !self.clean_folded {
                rows.extend(clean.into_iter().map(Row::Repo));
            }
        }
        rows
    }

    /// Whether `item` has nothing to do: no changes, nothing to push or
    /// pull, no merge or rebase stopped, on a branch.
    fn quiet(item: &RepoItem) -> bool {
        let status = &item.status;
        status.changes.is_empty()
            && status.ahead == 0
            && status.behind == 0
            && status.op.is_none()
            && status.branch.is_some()
    }

    /// Whether `item` goes under the `Clean` heading: it has had nothing
    /// to do since the panel opened.
    fn is_clean(&self, item: &RepoItem) -> bool {
        Panel::quiet(item) && !self.stirred.contains(&item.root)
    }

    /// Remember the repositories of `items` with something to do.
    fn stir(&mut self, items: &[RepoItem]) {
        let busy = items.iter().filter(|i| !Panel::quiet(i));
        self.stirred.extend(busy.map(|i| i.root.clone()));
    }

    /// The path of folder line `row`.
    fn dir_path(row: Row, items: &[RepoItem]) -> Option<&str> {
        let Row::Dir { k, c, to, .. } = row else {
            return None;
        };
        Some(path_parts(&items[k].status.changes[c].path, 0, to))
    }

    /// Whether `row`, a folder's line, shows its files; true of any other.
    fn dir_open(&self, row: Row, items: &[RepoItem]) -> bool {
        if self.folded_dirs.is_empty() {
            return true;
        }
        match (Panel::dir_path(row, items), row.item()) {
            (Some(path), Some(k)) => {
                let key = (items[k].root.clone(), path.to_owned());
                !self.folded_dirs.contains(&key)
            }
            _ => true,
        }
    }

    /// Show or hide the files of folder line `row`.
    fn set_dir_open(&mut self, row: Row, items: &[RepoItem], open: bool) {
        let (Some(path), Some(k)) = (Panel::dir_path(row, items), row.item()) else {
            return;
        };
        let key = (items[k].root.clone(), path.to_owned());
        if open {
            self.folded_dirs.remove(&key);
        } else {
            self.folded_dirs.insert(key);
        }
    }

    /// Unfold the folders the file at `path` of the repository at `root`
    /// is in.
    fn reveal(&mut self, root: &Path, path: &str) {
        self.folded_dirs
            .retain(|(r, dir)| !(r == root && in_dir(path, dir)));
    }

    /// The line over line `at` of `rows`: its folder's, or its
    /// repository's.
    fn parent(rows: &[Row], at: usize) -> Option<usize> {
        let depth = rows[at].depth();
        rows[..at].iter().rposition(|r| match r {
            Row::Repo(_) | Row::Clean => true,
            Row::Dir { depth: d, .. } => *d < depth,
            Row::File(..) => false,
        })
    }

    /// Whether `row` is the selected line.
    fn is_selected(&self, row: Row, items: &[RepoItem]) -> bool {
        match (&self.selected, row) {
            (Some(Selected::Repo(root)), Row::Repo(k)) => items[k].root == *root,
            (Some(Selected::Dir(root, path)), Row::Dir { k, .. }) => {
                items[k].root == *root && Panel::dir_path(row, items) == Some(path)
            }
            (Some(Selected::File(root, path)), Row::File(k, c, _)) => {
                items[k].root == *root && items[k].status.changes[c].path == *path
            }
            (Some(Selected::Clean), Row::Clean) => true,
            _ => false,
        }
    }

    /// The selected line, by index into `rows`.
    fn selected_row(&self, rows: &[Row], items: &[RepoItem]) -> Option<usize> {
        rows.iter().position(|&r| self.is_selected(r, items))
    }

    /// The selected repository, by index into `items`, and the selected
    /// file, by index into its changes.
    fn selection(&self, items: &[RepoItem]) -> Option<(usize, Option<usize>)> {
        let (root, path) = match self.selected.as_ref()? {
            Selected::Repo(root) | Selected::Dir(root, _) => (root, None),
            Selected::File(root, path) => (root, Some(path)),
            Selected::Clean => return None,
        };
        let k = items.iter().position(|i| i.root == *root)?;
        match path {
            None => Some((k, None)),
            Some(path) => {
                let changes = &items[k].status.changes;
                let c = changes.iter().position(|c| c.path == *path)?;
                Some((k, Some(c)))
            }
        }
    }

    /// The selected repository.
    fn selected_item<'a>(&self, items: &'a [RepoItem]) -> Option<&'a RepoItem> {
        self.selection(items).map(|(k, _)| &items[k])
    }

    /// Where the selected line is on disk: a file's or folder's path, else
    /// its repository's root.
    pub fn selected_path(&self) -> Option<PathBuf> {
        match self.selected.as_ref()? {
            Selected::Repo(root) => Some(root.clone()),
            Selected::Dir(root, path) | Selected::File(root, path) => Some(root.join(path)),
            Selected::Clean => None,
        }
    }

    /// Select line `k` of `rows`, scrolling it into a view of `page`.
    fn select(&mut self, rows: &[Row], k: usize, items: &[RepoItem], page: usize) {
        let Some(&row) = rows.get(k) else { return };
        let root = || items[row.item().unwrap_or_default()].root.clone();
        let id = match row {
            Row::Repo(_) => Selected::Repo(root()),
            Row::Dir { .. } => {
                let path = Panel::dir_path(row, items).unwrap_or_default();
                Selected::Dir(root(), path.to_owned())
            }
            Row::File(k, c, _) => Selected::File(root(), items[k].status.changes[c].path.clone()),
            Row::Clean => Selected::Clean,
        };
        let moved = self.selected.as_ref() != Some(&id);
        self.selected = Some(id);
        if moved && self.tab() == DetailTab::Diff {
            self.show_selected();
        } else if moved {
            self.detail_scroll.jump(0.0);
        }
        self.selected_at = k;
        let to = showing(self.tree_scroll.pos(), (k, k), page);
        move_to(&mut self.tree_scroll, to);
    }

    /// The lines changed (a fold, a Git status read): the selection kept
    /// on who it was, else near where it was, and the view within them.
    fn settle(&mut self, items: &[RepoItem], page: usize) {
        let rows = self.rows(items);
        match self.selected_row(&rows, items) {
            Some(k) => self.selected_at = k,
            None if !rows.is_empty() => {
                let mut k = self.selected_at.min(rows.len() - 1);
                // A line, rather than the heading, takes its place.
                if rows[k] == Row::Clean {
                    k = if k + 1 < rows.len() {
                        k + 1
                    } else {
                        k.saturating_sub(1)
                    };
                }
                self.select(&rows, k, items, page);
            }
            None => self.selected = None,
        }
        let last = rows.len().saturating_sub(page.max(1)) as f64;
        if self.tree_scroll.pos() > last {
            self.tree_scroll.jump(last);
        }
    }

    /// Whether `change` of `item` is checked.
    fn file_checked(&self, item: &RepoItem, change: &Change) -> bool {
        // Asked per file per frame: no copies while nothing is checked.
        if self.checked_files.is_empty() {
            return false;
        }
        let key = (item.root.clone(), change.path.clone());
        self.checked_files.contains(&key)
    }

    /// `item`'s checkbox: on when checked with all its files, partly when
    /// some are (or it is, but a file came since), else off.
    fn check(&self, item: &RepoItem) -> Check {
        let changes = &item.status.changes;
        let n = changes
            .iter()
            .filter(|c| self.file_checked(item, c))
            .count();
        let whole = self.checked_repos.contains(&item.root);
        if whole && n == changes.len() {
            Check::On
        } else if whole || n > 0 {
            Check::Partial
        } else {
            Check::Off
        }
    }

    /// Check `item` and all its files, or none of them.
    fn set_checked(&mut self, item: &RepoItem, on: bool) {
        for change in &item.status.changes {
            let key = (item.root.clone(), change.path.clone());
            if on {
                self.checked_files.insert(key);
            } else {
                self.checked_files.remove(&key);
            }
        }
        if on {
            self.checked_repos.insert(item.root.clone());
        } else {
            self.checked_repos.remove(&item.root);
        }
    }

    /// The checkbox of folder `dir` of `item`: on when all its files are
    /// checked, partly when some are, else off.
    fn dir_check(&self, item: &RepoItem, dir: &str) -> Check {
        let (mut all, mut some) = (true, false);
        for change in item.status.changes.iter().filter(|c| in_dir(&c.path, dir)) {
            let on = self.file_checked(item, change);
            all &= on;
            some |= on;
        }
        match (all, some) {
            (true, true) => Check::On,
            (false, true) => Check::Partial,
            _ => Check::Off,
        }
    }

    /// Check line `row`, or uncheck it: a repository or folder partly
    /// checked is unchecked.
    fn toggle(&mut self, row: Row, items: &[RepoItem]) {
        let Some(k) = row.item() else { return };
        let item = &items[k];
        match row {
            Row::Clean => return,
            Row::Repo(_) => {
                let on = self.check(item) == Check::Off;
                self.set_checked(item, on);
                return;
            }
            Row::Dir { .. } => {
                let dir = Panel::dir_path(row, items).unwrap_or_default();
                let on = self.dir_check(item, dir) == Check::Off;
                for change in item.status.changes.iter().filter(|c| in_dir(&c.path, dir)) {
                    let key = (item.root.clone(), change.path.clone());
                    if on {
                        self.checked_files.insert(key);
                    } else {
                        self.checked_files.remove(&key);
                    }
                }
            }
            Row::File(_, c, _) => {
                let key = (item.root.clone(), item.status.changes[c].path.clone());
                if !self.checked_files.remove(&key) {
                    self.checked_files.insert(key);
                }
            }
        }
        // The repository is whole while all its files are.
        let changes = &item.status.changes;
        if changes.iter().all(|c| self.file_checked(item, c)) {
            self.checked_repos.insert(item.root.clone());
        } else {
            self.checked_repos.remove(&item.root);
        }
    }

    /// List the files as `grouping` says, for every repository: the diff
    /// in the tree's order, the selection kept (a folder's line, gone from
    /// a list, gives way to its repository's) and in view.
    fn set_grouping(&mut self, grouping: Grouping, ctx: &Ctx) -> Option<Outcome> {
        if self.grouping == grouping {
            return None;
        }
        let items = ctx.items;
        self.grouping = grouping;
        if let Some(Selected::Dir(root, _)) = &self.selected {
            self.selected = Some(Selected::Repo(root.clone()));
        }
        self.order_diffs(items);
        self.diff.rebuild(&sections(items));
        self.relist(ctx);
        Some(Outcome::Grouping(grouping))
    }

    /// Unfold every repository and folder, or fold them all (the folders
    /// while listed by folder): the selection, hidden, on its
    /// repository's line.
    fn fold_all(&mut self, open: bool, ctx: &Ctx) {
        let items = ctx.items;
        for item in items {
            self.set_open(item, open);
        }
        self.clean_folded = !open;
        if open {
            self.folded_dirs.clear();
        } else {
            if self.grouping == Grouping::Folders {
                for (k, item) in items.iter().enumerate() {
                    for row in file_rows(k, &item.status.changes, self.grouping) {
                        if let Some(path) = Panel::dir_path(row, items) {
                            self.folded_dirs
                                .insert((item.root.clone(), path.to_owned()));
                        }
                    }
                }
            }
            if let Some(Selected::Dir(root, _) | Selected::File(root, _)) = &self.selected {
                self.selected = Some(Selected::Repo(root.clone()));
            }
        }
        self.relist(ctx);
    }

    /// The tree's lines changed as a whole: the selection kept, and in
    /// view.
    fn relist(&mut self, ctx: &Ctx) {
        let (items, page) = (ctx.items, ctx.page.0.max(1));
        self.settle(items, page);
        let rows = self.rows(items);
        if let Some(k) = self.selected_row(&rows, items) {
            let to = showing(self.tree_scroll.pos(), (k, k), page);
            move_to(&mut self.tree_scroll, to);
        }
    }

    /// The diffs read put in the tree's order.
    fn order_diffs(&mut self, items: &[RepoItem]) {
        for item in items {
            let rank = self.rank(item);
            self.diff.order_files(&item.root, |path| {
                rank.get(path).copied().unwrap_or(usize::MAX)
            });
        }
    }

    /// Where each of `item`'s files goes in the tree's order, by path.
    fn rank<'a>(&self, item: &'a RepoItem) -> HashMap<&'a str, usize> {
        let changes = &item.status.changes;
        tree_order(changes, self.grouping)
            .into_iter()
            .enumerate()
            .map(|(n, c)| (changes[c].path.as_str(), n))
            .collect()
    }

    /// Check everything, or nothing when everything is.
    fn toggle_all(&mut self, items: &[RepoItem]) {
        let on = !items.iter().all(|i| self.check(i) == Check::On);
        for item in items {
            self.set_checked(item, on);
        }
    }

    /// The repositories checked, or with a file checked, by index.
    fn picked(&self, items: &[RepoItem]) -> Vec<usize> {
        (0..items.len())
            .filter(|&k| self.check(&items[k]) != Check::Off)
            .collect()
    }

    /// What an action runs on: the picked repositories, else the selected
    /// one.
    fn targets(&self, items: &[RepoItem]) -> Vec<usize> {
        let picked = self.picked(items);
        if !picked.is_empty() {
            return picked;
        }
        self.selection(items)
            .map(|(k, _)| vec![k])
            .unwrap_or_default()
    }

    /// What `button` says in the footer: a merge or rebase names the
    /// branch it brings in, the default one, and the branch it changes,
    /// the one checked out (`Merge main into feat/x`, `Rebase feat/x onto
    /// main`); see `one_name` for several repositories.
    fn button_label(&self, button: ReposButton, ctx: &Ctx) -> String {
        use ReposButton as B;
        let items = ctx.items;
        // How many repositories it runs on, after its name: `push 3`.
        let over = |what: &str, n: usize| match n {
            0 => what.to_owned(),
            n => format!("{what} {n}"),
        };
        let targets = self.targets(items).len();
        let committing = items
            .iter()
            .filter(|i| i.status.changes.iter().any(|c| self.file_checked(i, c)))
            .count();
        match button {
            B::Push => over("push", targets),
            B::ForcePush => over("force push", targets),
            B::Pull => over("pull", targets),
            B::Commit => over("commit", committing),
            B::CommitPush => over("commit and push", committing.max(self.picked(items).len())),
            B::Fetch => match self.picked(items).len() {
                0 => "fetch all".into(),
                n => format!("fetch {n}"),
            },
            B::Merge | B::Rebase | B::MergeFrom | B::RebaseFrom => {
                let base = self.one_name(ctx, |s| s.default_branch.as_deref());
                let base = base.unwrap_or_else(|| "base".into());
                let current = self.current_name(ctx);
                match button {
                    B::Merge => format!("merge {base} into {current}"),
                    B::Rebase => format!("rebase {current} onto {base}"),
                    B::MergeFrom => format!("merge into {current}…"),
                    _ => format!("rebase {current} onto…"),
                }
            }
            _ => lower_first(button.label()),
        }
    }

    /// The one branch `of` names in each repository acted on: see
    /// `one_name`.
    fn one_name(&self, ctx: &Ctx, of: fn(&GitStatus) -> Option<&str>) -> Option<String> {
        let targets = self.targets(ctx.items);
        let names = targets.iter().map(|&k| of(&ctx.items[k].status));
        one_name(names, ctx.same)
    }

    /// The branch checked out in the repositories acted on, as a merge or
    /// rebase names it: `feat/x`, else `current branch`.
    fn current_name(&self, ctx: &Ctx) -> String {
        let current = self.one_name(ctx, |s| s.branch.as_deref());
        current.unwrap_or_else(|| "current branch".into())
    }

    /// The branches every repository acted on has, local or on its remote:
    /// those one has as a local branch first; in each group their default
    /// branch first, then those of `Ctx::top` in its order, then the last
    /// committed to first, then by name.
    fn branches(&self, ctx: &Ctx) -> Vec<Branch> {
        let items = ctx.items;
        let targets = self.targets(items);
        let of = |k: usize| -> BTreeMap<String, Vec<String>> {
            let status = &items[k].status;
            let all = status.local_branches.iter().chain(&status.remote_branches);
            all.map(|b| (branch_key(b, ctx.same), names_of(b, ctx.same)))
                .collect()
        };
        let Some((&first, rest)) = targets.split_first() else {
            return Vec::new();
        };
        let mut common = of(first);
        for &k in rest {
            let theirs = of(k);
            common.retain(|key, _| theirs.contains_key(key));
        }
        let defaults: HashSet<String> = targets
            .iter()
            .filter_map(|&k| items[k].status.default_branch.as_deref())
            .map(|b| branch_key(b, ctx.same))
            .collect();
        /// Where a branch goes: remote only after local, its rank
        /// (default, top, the rest), then the newest first.
        type Order = (bool, usize, Reverse<Option<i64>>);
        let mut branches: Vec<(Order, Branch)> = common
            .into_iter()
            .map(|(key, names)| {
                let used: HashSet<&str> = targets
                    .iter()
                    .filter_map(|&k| name_in(&items[k].status, &names))
                    .collect();
                let label = match used.into_iter().collect::<Vec<_>>()[..] {
                    [one] => one.to_owned(),
                    _ => key.clone(),
                };
                let rank = if defaults.contains(&key) {
                    0
                } else {
                    let top = ctx.top.iter().position(|t| names.contains(t));
                    top.map_or(ctx.top.len() + 1, |k| k + 1)
                };
                let time = targets
                    .iter()
                    .flat_map(|&k| {
                        let times = &items[k].status.branch_times;
                        names.iter().filter_map(|n| times.get(n).copied())
                    })
                    .max();
                let local = targets.iter().any(|&k| {
                    let local = &items[k].status.local_branches;
                    names.iter().any(|n| local.contains(n))
                });
                let order = (!local, rank, Reverse(time));
                (
                    order,
                    Branch {
                        names,
                        label,
                        local,
                    },
                )
            })
            .collect();
        // Stable: those alike stay by name.
        branches.sort_by_key(|(order, _)| *order);
        branches.into_iter().map(|(_, b)| b).collect()
    }

    /// The branches the menu shows: those with every word typed in its
    /// search in their name, whatever the case; to merge or rebase, not
    /// the one they are all on.
    fn shown_branches(&self, ctx: &Ctx) -> Vec<Branch> {
        let mut branches = self.branches(ctx);
        if self.menu_action() != MenuAction::Switch
            && let Some(current) = self.current_branch(ctx)
        {
            branches.retain(|b| b.names.join(" / ") != current);
        }
        let query = self.branch_menu.as_ref().map(|m| m.query.to_lowercase());
        let words: Vec<&str> = query.as_deref().unwrap_or("").split_whitespace().collect();
        branches.retain(|b| {
            let named = |w: &str| {
                let has = |name: &str| name.to_lowercase().contains(w);
                has(&b.label) || b.names.iter().any(|n| has(n))
            };
            words.iter().all(|w| named(w))
        });
        branches
    }

    /// The branch every repository acted on is on, by `branch_key`; `None`
    /// when they are on different ones, or there are none.
    fn current_branch(&self, ctx: &Ctx) -> Option<String> {
        let items = ctx.items;
        let mut keys = self.targets(items).into_iter().map(|k| {
            items[k]
                .status
                .branch
                .as_deref()
                .map(|b| branch_key(b, ctx.same))
        });
        let first = keys.next()??;
        keys.all(|k| k.as_ref() == Some(&first)).then_some(first)
    }

    /// The picker's button: the branch the repositories acted on are on
    /// (as they name it, if all the same), else that they differ.
    fn branch_picker(&self, ctx: &Ctx) -> BranchPicker {
        let items = ctx.items;
        let targets = self.targets(items);
        let running = ctx.batch.is_some_and(|b| !b.finished());
        let branch = |k: usize| items[k].status.branch.as_deref();
        let (label, mixed) = if targets.is_empty() {
            ("No repository".to_owned(), true)
        } else if targets.iter().all(|&k| branch(k).is_none()) {
            ("detached".to_owned(), false)
        } else if let Some(key) = self.current_branch(ctx) {
            let names: HashSet<&str> = targets.iter().filter_map(|&k| branch(k)).collect();
            match names.into_iter().collect::<Vec<_>>()[..] {
                [one] => (one.to_owned(), false),
                _ => (key, false),
            }
        } else {
            ("mixed branches".to_owned(), true)
        };
        let menu = self.branch_menu.as_ref().map(|menu| {
            let switch = menu.action == MenuAction::Switch;
            let branches = self.shown_branches(ctx);
            let page = self.menu_page(ctx);
            let (lines, part) = self.menu_window(&branches, page);
            let mut headings = Vec::new();
            let mut in_window = Vec::new();
            for line in lines {
                match line {
                    MenuLine::Heading { local } => {
                        let label = if local { "Local" } else { "Remote" };
                        headings.push((in_window.len(), label.to_owned()));
                    }
                    MenuLine::Branch(k) => in_window.push(k),
                }
            }
            let shown: Vec<&Branch> = in_window.iter().map(|&k| &branches[k]).collect();
            let current = self.current_branch(ctx);
            let in_view = |k: usize| in_window.iter().position(|&w| w == k);
            // The longest of them all, which the menu is as wide as, so
            // that it keeps its width as they scroll by.
            let mut widest: Vec<&Branch> = branches.iter().collect();
            widest.sort_by_key(|b| Reverse(b.label.chars().count()));
            widest.truncate(WIDEST);
            BranchMenuView {
                part,
                over: menu.scroll.overscroll(),
                widest: widest.into_iter().map(|b| b.label.clone()).collect(),
                headings,
                rows: shown.iter().map(|b| b.label.clone()).collect(),
                deletable: shown
                    .iter()
                    .map(|&b| switch && self.delete_jobs(b, ctx).is_ok())
                    .collect(),
                current: current
                    .and_then(|key| branches.iter().position(|b| b.names.join(" / ") == key))
                    .and_then(in_view),
                picked: (!menu.on_create)
                    .then(|| in_view(menu.picked.min(branches.len().saturating_sub(1))))
                    .flatten(),
                create: self.create_row(ctx).map(|(name, problem)| BranchCreate {
                    label: match problem {
                        Some(why) => format!("Can't create {name}: {why}"),
                        None => {
                            let from = self.one_name(ctx, |s| s.branch.as_deref());
                            let from = from.unwrap_or_else(|| "HEAD".into());
                            format!("Create {name} from {from}")
                        }
                    },
                    problem: problem.map(str::to_owned),
                    picked: menu.on_create,
                }),
                empty: if !menu.query.trim().is_empty() {
                    "No branch matches".into()
                } else if switch {
                    "No branch they all have".into()
                } else {
                    "No other branch they all have".into()
                },
                query: menu.query.line(),
                placeholder: match menu.action {
                    MenuAction::Switch if menu.create_first => "Name the new branch".into(),
                    MenuAction::Switch => "Find or create a branch".into(),
                    MenuAction::Integrate(Integrate::Merge) => {
                        format!("Find a branch to merge into {}", self.current_name(ctx))
                    }
                    MenuAction::Integrate(Integrate::Rebase) => {
                        format!("Find a branch to rebase {} onto", self.current_name(ctx))
                    }
                },
            }
        });
        BranchPicker {
            label,
            mixed,
            enabled: !targets.is_empty() && !running,
            menu,
        }
    }

    /// Why the branch menu can't open, if it can't.
    fn menu_blocked(&self, ctx: &Ctx) -> Option<&'static str> {
        if ctx.batch.is_some_and(|b| !b.finished()) {
            return Some("Wait for the batch to finish");
        }
        if self.targets(ctx.items).is_empty() {
            return Some("Select a repository first");
        }
        None
    }

    /// What picking a branch in the menu does, while it is open.
    fn menu_action(&self) -> MenuAction {
        self.branch_menu
            .as_ref()
            .map(|m| m.action)
            .unwrap_or_default()
    }

    /// Open the branch menu for `action`, if it can: to switch, on the
    /// branch they are on; else on the first.
    fn open_branch_menu(&mut self, action: MenuAction, ctx: &Ctx) {
        if let Some(why) = self.menu_blocked(ctx) {
            self.error = Some(why.into());
            return;
        }
        let page = self.menu_page(ctx);
        let (first, picked) = match action {
            MenuAction::Switch => {
                let branches = self.branches(ctx);
                let current = self.current_branch(ctx);
                let picked = current
                    .and_then(|key| branches.iter().position(|b| b.names.join(" / ") == key))
                    .unwrap_or(0);
                (scroll_to(&menu_lines(&branches), picked, 0.0, page), picked)
            }
            MenuAction::Integrate(_) => (0.0, 0),
        };
        self.error = None;
        self.branch_menu = Some(BranchMenu {
            scroll: Scroll::at(first),
            picked,
            query: TextField::default(),
            action,
            create_first: false,
            on_create: false,
        });
    }

    /// Open the branch menu to create a branch: Enter creates the one
    /// typed, Up/Down go to those found instead.
    fn open_create_menu(&mut self, ctx: &Ctx) {
        self.open_branch_menu(MenuAction::Switch, ctx);
        if let Some(menu) = self.branch_menu.as_mut() {
            menu.create_first = true;
        }
    }

    /// The branch typed in the menu's search, when the menu offers to
    /// create it: in the menu to switch, while no branch shown is called
    /// that. Why it can't be a branch's name, if it can't.
    fn create_row(&self, ctx: &Ctx) -> Option<(String, Option<&'static str>)> {
        let menu = self.branch_menu.as_ref()?;
        let name = menu.query.trim();
        if menu.action != MenuAction::Switch || name.is_empty() {
            return None;
        }
        let named = |b: &Branch| b.label == name || b.names.iter().any(|n| n == name);
        if self.shown_branches(ctx).iter().any(named) {
            return None;
        }
        Some((name.to_owned(), branch_name_problem(name)))
    }

    /// How many of the menu's branches are in view.
    fn menu_page(&self, ctx: &Ctx) -> usize {
        branch_page(ctx, self.create_row(ctx).is_some())
    }

    /// The menu's lines in view, listing `branches` (those it shows) in a
    /// page of `page`, one more under them when scrolled part of a line;
    /// and that part.
    fn menu_window(&self, branches: &[Branch], page: usize) -> (Vec<MenuLine>, f64) {
        let lines = menu_lines(branches);
        let (first, part) = match &self.branch_menu {
            Some(menu) => standing(&menu.scroll, lines.len(), page),
            None => (0, 0.0),
        };
        let end = (first + page + usize::from(part > 0.0)).min(lines.len());
        (lines[first..end].to_vec(), part)
    }

    /// A key while the branch menu is open, which takes them all:
    /// Up/Down, the pages and Ctrl+Home/End choose (Up from the first
    /// branch goes to the row creating the branch typed), Enter picks,
    /// Ctrl+Enter creates the branch typed, Ctrl+N makes Enter create it,
    /// Delete asks to delete (in the menu to switch), Esc closes; the rest
    /// edit its search, which starts over at the top: on the row creating
    /// the branch typed when opened to create one, or when none is found.
    fn branch_menu_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Outcome> {
        let lines = menu_lines(&self.shown_branches(ctx));
        let len = lines.len()
            - lines
                .iter()
                .filter(|l| matches!(l, MenuLine::Heading { .. }))
                .count();
        let page = self.menu_page(ctx);
        let create = self.create_row(ctx).is_some();
        let plain = !(mods.ctrl || mods.shift || mods.alt || mods.logo);
        let only_ctrl = mods.ctrl && !(mods.shift || mods.alt || mods.logo);
        let switch = self.menu_action() == MenuAction::Switch;
        let on_create = self.branch_menu.as_ref()?.on_create;
        if switch && plain && !on_create && matches!(sym, Keysym::Delete | Keysym::KP_Delete) {
            let picked = self.branch_menu.as_ref()?.picked;
            return self.delete_branch(picked.min(len.saturating_sub(1)), ctx);
        }
        if only_ctrl && matches!(sym, Keysym::Return | Keysym::KP_Enter) {
            return self.create_branch(ctx);
        }
        if only_ctrl && matches!(sym, Keysym::n | Keysym::N) {
            let menu = self.branch_menu.as_mut()?;
            menu.action = MenuAction::Switch;
            menu.create_first = true;
            let create = self.create_row(ctx).is_some();
            self.branch_menu.as_mut()?.on_create = create;
            return None;
        }
        let menu = self.branch_menu.as_mut()?;
        let at = menu.picked.min(len.saturating_sub(1)) as isize;
        let to = match sym {
            // Down from the row creating the branch: the first branch.
            Keysym::Down | Keysym::KP_Down if menu.on_create => {
                menu.on_create = len == 0;
                menu.scroll.jump(0.0);
                menu.picked = 0;
                return None;
            }
            Keysym::Up | Keysym::KP_Up if create && at == 0 => {
                menu.on_create = true;
                return None;
            }
            Keysym::Down | Keysym::KP_Down => at + 1,
            Keysym::Up | Keysym::KP_Up => at - 1,
            Keysym::Page_Down | Keysym::KP_Page_Down => at + page as isize,
            Keysym::Page_Up | Keysym::KP_Page_Up => at - page as isize,
            Keysym::Home | Keysym::KP_Home if mods.ctrl => 0,
            Keysym::End | Keysym::KP_End if mods.ctrl => len as isize,
            Keysym::Return | Keysym::KP_Enter if menu.on_create => {
                return self.create_branch(ctx);
            }
            Keysym::Return | Keysym::KP_Enter => {
                let picked = menu.picked;
                return self.pick_branch(picked, ctx);
            }
            Keysym::Escape => {
                self.branch_menu = None;
                return None;
            }
            _ => {
                if menu.query.key(mods, sym, typed).changed() {
                    // Other branches, from the top.
                    menu.scroll.jump(0.0);
                    menu.picked = 0;
                    let create_first = menu.create_first;
                    let found = !self.shown_branches(ctx).is_empty();
                    let create = self.create_row(ctx).is_some();
                    if let Some(menu) = self.branch_menu.as_mut() {
                        menu.on_create = create && (create_first || !found);
                    }
                }
                return None;
            }
        };
        let to = to.clamp(0, len.saturating_sub(1) as isize) as usize;
        menu.on_create = false;
        menu.picked = to;
        let last = lines.len().saturating_sub(page) as f64;
        let pos = scroll_to(&lines, to, menu.scroll.pos().min(last), page);
        move_to(&mut menu.scroll, pos);
        None
    }

    /// Create the branch typed in the menu's search in the repositories
    /// acted on, from where each is: asked of the event loop, which
    /// fetches and looks for it first. The menu closes, unless the name
    /// can't be a branch's.
    fn create_branch(&mut self, ctx: &Ctx) -> Option<Outcome> {
        let menu = self.branch_menu.as_ref()?;
        let name = menu.query.trim().to_owned();
        if name.is_empty() {
            self.error = Some("Type the new branch's name first".into());
            return None;
        }
        if let Some(why) = branch_name_problem(&name) {
            self.error = Some(format!("Not a branch name: {why}"));
            return None;
        }
        if ctx.batch.is_some_and(|b| !b.finished()) {
            self.error = Some("Wait for the batch to finish".into());
            return None;
        }
        let items = ctx.items;
        let jobs: Vec<Job> = self
            .targets(items)
            .into_iter()
            .map(|k| Job {
                root: items[k].root.clone(),
                files: Vec::new(),
            })
            .collect();
        if jobs.is_empty() {
            self.error = Some("Select a repository first".into());
            return None;
        }
        self.branch_menu = None;
        self.error = None;
        Some(Outcome::CreateBranch { name, jobs })
    }

    /// Back to the branch menu, `name` in its search, to edit it and
    /// create it again.
    fn edit_branch_name(&mut self, name: &str) {
        self.branch_menu = Some(BranchMenu {
            query: TextField::from(name),
            create_first: true,
            on_create: true,
            ..BranchMenu::default()
        });
    }

    /// Switch the repositories acted on to branch `k` of `shown_branches`,
    /// or merge it into them or rebase them onto it, but those already on
    /// it; the menu closes.
    fn pick_branch(&mut self, k: usize, ctx: &Ctx) -> Option<Outcome> {
        // None found: the menu stays for another search.
        let branch = self.shown_branches(ctx).into_iter().nth(k)?;
        let action = self.menu_action();
        self.branch_menu = None;
        if ctx.batch.is_some_and(|b| !b.finished()) {
            self.error = Some("Wait for the batch to finish".into());
            return None;
        }
        let key = branch.names.join(" / ");
        let items = ctx.items;
        let jobs: Vec<Job> = self
            .targets(items)
            .into_iter()
            .filter(|&k| {
                let on = items[k].status.branch.as_deref();
                on.map(|b| branch_key(b, ctx.same)).as_ref() != Some(&key)
            })
            .map(|k| Job {
                root: items[k].root.clone(),
                files: Vec::new(),
            })
            .collect();
        if jobs.is_empty() {
            self.error = Some(format!("Already on {}", branch.label));
            return None;
        }
        self.error = None;
        let kind = match action {
            MenuAction::Switch => BatchKind::Switch(branch.names),
            MenuAction::Integrate(how) => BatchKind::Integrate {
                how,
                from: Some(branch.names),
            },
        };
        Some(Outcome::Batch { kind, jobs })
    }

    /// The jobs deleting local branch `branch` in the repositories acted on
    /// that have it, else why it can't be.
    fn delete_jobs(&self, branch: &Branch, ctx: &Ctx) -> Result<Vec<Job>, String> {
        let items = ctx.items;
        let local = |k: &usize| {
            let status = &items[*k].status;
            branch
                .names
                .iter()
                .any(|n| status.local_branches.contains(n))
        };
        let has: Vec<usize> = self.targets(items).into_iter().filter(local).collect();
        if has.is_empty() {
            return Err(format!("{} is only on the remote", branch.label));
        }
        let on = |k: &usize| {
            let on = items[*k].status.branch.as_ref();
            on.is_some_and(|b| branch.names.contains(b))
        };
        if has.iter().all(on) {
            return Err(format!(
                "On {}: switch to another branch first",
                branch.label
            ));
        }
        // Those on it stay in: the check says why they're left out.
        Ok(has
            .into_iter()
            .map(|k| Job {
                root: items[k].root.clone(),
                files: Vec::new(),
            })
            .collect())
    }

    /// Ask to delete local branch `k` of `shown_branches` where the
    /// repositories acted on have it, on a sheet over the menu.
    fn delete_branch(&mut self, k: usize, ctx: &Ctx) -> Option<Outcome> {
        let branch = self.shown_branches(ctx).into_iter().nth(k)?;
        if ctx.batch.is_some_and(|b| !b.finished()) {
            self.error = Some("Wait for the batch to finish".into());
            return None;
        }
        let jobs = match self.delete_jobs(&branch, ctx) {
            Ok(jobs) => jobs,
            Err(why) => {
                self.error = Some(why);
                return None;
            }
        };
        let menu = self.branch_menu.as_mut()?;
        menu.picked = k;
        self.error = None;
        let kind = BatchKind::DeleteBranch {
            names: branch.names,
            force: false,
        };
        self.confirm(kind, jobs, ctx);
        None
    }

    /// The branch menu moved by `motion` at `now`.
    pub fn scroll_branches(&mut self, motion: Motion, now: Instant, ctx: &Ctx) {
        let b = self.menu_bounds(ctx);
        if let Some(menu) = self.branch_menu.as_mut() {
            menu.scroll.apply(motion, now, b);
        }
    }

    /// The branch menu's lines as a list scrolled.
    fn menu_bounds(&self, ctx: &Ctx) -> Bounds {
        let len = menu_lines(&self.shown_branches(ctx)).len();
        let row = crate::chrome::repos_row(ctx.density);
        bounds(len, self.menu_page(ctx), row)
    }

    /// The list changed (a Git status read): drop what is gone (checked
    /// files committed, repositories closed), keep the selection on who it
    /// was, or near where it was.
    pub fn refresh(&mut self, ctx: &Ctx) {
        let items = ctx.items;
        let roots: HashSet<&Path> = items.iter().map(|i| i.root.as_path()).collect();
        self.checked_repos.retain(|r| roots.contains(r.as_path()));
        self.unfolded.retain(|r| roots.contains(r.as_path()));
        self.folded.retain(|r| roots.contains(r.as_path()));
        self.stir(items);
        self.checked_files.retain(|(root, path)| {
            items
                .iter()
                .find(|i| i.root == *root)
                .is_some_and(|i| i.status.changes.iter().any(|c| c.path == *path))
        });
        // The diff stays where it was, whatever the tree selects.
        let keep = self.diff.scroll.clone();
        self.settle(items, ctx.page.0);
        self.diff.scroll = keep;
        // The diff or log may have changed with it.
        self.detail_stale = true;
        diff_ctx!(self, ctx, dctx);
        self.diff.refresh(&dctx);
    }

    /// Whether `button` can be done now and what it asks for, else why
    /// not. It changes nothing: the footer asks it too.
    fn plan(&self, button: ReposButton, ctx: &Ctx) -> Result<Outcome, Why> {
        use ReposButton as B;
        let items = ctx.items;
        let local = matches!(
            button,
            B::OpenInZed
                | B::RunInTerminal
                | B::OpenWorkspace
                | B::CopyReview
                | B::ClearReview
                | B::Comment
                | B::SaveComment
                | B::DiscardComment
                | B::DeleteComment
        );
        if !local && ctx.batch.is_some_and(|b| !b.finished()) {
            return Err(Why::other("Wait for the batch to finish"));
        }
        if items.is_empty() {
            return Err(Why::other("No git repositories in this project"));
        }
        let job = |k: usize| Job {
            root: items[k].root.clone(),
            files: Vec::new(),
        };
        let run = |kind: BatchKind, ks: Vec<usize>| {
            if ks.is_empty() {
                return Err(Why::other("Select a repository first"));
            }
            let jobs = ks.into_iter().map(job).collect();
            Ok(Outcome::Batch { kind, jobs })
        };
        let targets = self.targets(items);
        let selected = self.selected_item(items);
        let run_of = |item: &RepoItem| ctx.batch.and_then(|b| b.run(&item.root));
        let failure = selected
            .and_then(run_of)
            .and_then(|r| r.state.failure().cloned());
        // Fixing conflicts is for the selected repository alone.
        let stopped = || {
            let item = selected.ok_or_else(|| Why::other("Select a repository first"))?;
            if !in_conflict(item) {
                return Err(Why::other(format!(
                    "{} isn't stopped on conflicts",
                    item.label
                )));
            }
            Ok(item)
        };
        match button {
            B::Commit | B::CommitPush => {
                let mut jobs = Vec::new();
                for item in items {
                    let mut files = Vec::new();
                    for change in &item.status.changes {
                        if self.file_checked(item, change) {
                            files.push(change.path.clone());
                            files.extend(change.orig.clone());
                        }
                    }
                    if !files.is_empty() {
                        let root = item.root.clone();
                        jobs.push(Job { root, files });
                    }
                }
                if jobs.is_empty() {
                    return Err(Why::other("Check the files to commit"));
                }
                let msg = self.field.trim();
                if msg.is_empty() {
                    return Err(Why::Field("Type a commit message first".into()));
                }
                let push = button == B::CommitPush;
                // The other checked repositories have nothing to commit,
                // but their branches go up with the rest.
                if push {
                    for k in self.picked(items) {
                        let root = &items[k].root;
                        if !jobs.iter().any(|j| &j.root == root) {
                            let root = root.clone();
                            jobs.push(Job {
                                root,
                                files: Vec::new(),
                            });
                        }
                    }
                }
                let msg = msg.to_owned();
                Ok(Outcome::Batch {
                    kind: BatchKind::Commit { msg, push },
                    jobs,
                })
            }
            B::Push => run(BatchKind::Push { force: false }, targets),
            B::ForcePush => run(BatchKind::Push { force: true }, targets),
            B::Fetch => {
                let picked = self.picked(items);
                let all = if picked.is_empty() {
                    (0..items.len()).collect()
                } else {
                    picked
                };
                run(BatchKind::Fetch, all)
            }
            B::Pull => run(BatchKind::Pull(Pull::FastForward), targets),
            B::Merge | B::Rebase => {
                let how = match button {
                    B::Merge => Integrate::Merge,
                    _ => Integrate::Rebase,
                };
                run(BatchKind::Integrate { how, from: None }, targets)
            }
            // They open the branch menu (see `act`), when it can.
            B::MergeFrom | B::RebaseFrom => Err(Why::other(
                self.menu_blocked(ctx)
                    .unwrap_or("Pick the branch in the menu"),
            )),
            B::OpenInZed => {
                let (k, c) = self
                    .selection(items)
                    .ok_or_else(|| Why::other("Select a repository first"))?;
                let item = &items[k];
                // A file opens whatever it is in; else the conflicts.
                let paths = match c {
                    Some(c) => vec![item.status.changes[c].path.clone()],
                    None => conflicted_files(stopped()?),
                };
                let root = item.root.clone();
                Ok(Outcome::OpenInZed {
                    root,
                    paths,
                    at: None,
                })
            }
            B::MarkResolved => {
                let item = stopped()?;
                let file = self
                    .selection(items)
                    .and_then(|(_, c)| c)
                    .map(|c| &item.status.changes[c])
                    .filter(|c| c.conflicted());
                let files = match file {
                    Some(change) => vec![change.path.clone()],
                    None => conflicted_files(item),
                };
                if files.is_empty() {
                    return Err(Why::other("No conflicted files here"));
                }
                let root = item.root.clone();
                Ok(Outcome::Batch {
                    kind: BatchKind::MarkResolved,
                    jobs: vec![Job { root, files }],
                })
            }
            B::Continue => {
                let item = stopped()?;
                let left = conflicted_files(item).len();
                if left > 0 {
                    let files = if left == 1 { "file" } else { "files" };
                    return Err(Why::other(format!(
                        "Resolve {left} conflicted {files} first (Ctrl+R)"
                    )));
                }
                let k = items.iter().position(|i| i.root == item.root);
                run(BatchKind::Continue, k.into_iter().collect())
            }
            B::Abort => {
                let item = stopped()?;
                let k = items.iter().position(|i| i.root == item.root);
                run(BatchKind::Abort, k.into_iter().collect())
            }
            B::Undo => {
                let jobs = undo_jobs(ctx.batch).map_err(Why::Other)?;
                Ok(Outcome::Batch {
                    kind: BatchKind::Undo,
                    jobs,
                })
            }
            B::RetryFailed => match retryable(ctx.batch) {
                true => Ok(Outcome::RetryFailed),
                false => Err(Why::other("Nothing failed to retry")),
            },
            B::Stash => {
                let item = selected.ok_or_else(|| Why::other("Select a repository first"))?;
                if !item.status.dirty {
                    return Err(Why::other(format!("Nothing to stash in {}", item.label)));
                }
                if item.status.op.is_some() {
                    return Err(Why::other("Finish or abort what is half-way first"));
                }
                let k = items.iter().position(|i| i.root == item.root);
                run(BatchKind::Stash, k.into_iter().collect())
            }
            B::StashPop | B::StashDrop => {
                let item = selected.ok_or_else(|| Why::other("Select a repository first"))?;
                if item.status.stashes == 0 {
                    return Err(Why::other(format!("No stash in {}", item.label)));
                }
                let kind = match button {
                    B::StashPop => BatchKind::StashPop,
                    _ => BatchKind::StashDrop,
                };
                let k = items.iter().position(|i| i.root == item.root);
                run(kind, k.into_iter().collect())
            }
            B::RunInTerminal => match (selected, failure) {
                (Some(item), Some(Failure::Auth | Failure::Timeout | Failure::Other)) => {
                    Ok(Outcome::RunInTerminal(item.root.clone()))
                }
                _ => Err(Why::other("Nothing to run in a terminal")),
            },
            B::OpenWorkspace => match failure {
                Some(Failure::CheckedOut(path)) => Ok(Outcome::OpenWorkspace(path)),
                _ => Err(Why::other("Not checked out elsewhere")),
            },
            B::Help => Err(Why::other("Press F1 for the keys")),
            B::CopyReview => {
                diff_ctx!(self, ctx, dctx);
                (self.diff.copy_review(&dctx))
                    .map(from_diff)
                    .map_err(Why::other)
            }
            B::ClearReview if self.diff.review.is_empty() => Err(Why::other(NO_REVIEW)),
            B::ClearReview => Err(Why::other("Asked on a sheet")),
            B::Comment | B::SaveComment | B::DiscardComment | B::DeleteComment | B::Restore => {
                Err(Why::other("Done on the diff"))
            }
            B::All
            | B::CleanOnly
            | B::Cancel
            | B::PullRebase
            | B::PullMerge
            | B::SkipDiverged
            | B::Close
            | B::DeleteBranch
            | B::SwitchExisting
            | B::CreateMissing
            | B::EditName => Err(Why::other("Nothing to answer")),
        }
    }

    /// What `kind` over `jobs` does to each repository, for the sheet
    /// asking before it.
    fn confirm_lines(
        &self,
        kind: &BatchKind,
        jobs: &[Job],
        ctx: &Ctx,
    ) -> Vec<(String, String, Tone)> {
        jobs.iter()
            .map(|job| {
                let item = ctx.items.iter().find(|i| i.root == job.root);
                let label = item.map_or_else(
                    || {
                        job.root
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into()
                    },
                    |i| i.label.clone(),
                );
                let status = item.map(|i| &i.status);
                let what = match kind {
                    BatchKind::Push { .. } => {
                        let branch = status.and_then(|s| s.branch.as_deref()).unwrap_or("HEAD");
                        let to = status
                            .and_then(|s| s.upstream.as_deref())
                            .map_or("its remote branch".to_owned(), str::to_owned);
                        format!("overwrite {to} with {branch}, with lease")
                    }
                    BatchKind::Abort => match status.and_then(|s| s.op) {
                        Some(InProgress::Merge) => "abort the merge: back to before it".into(),
                        Some(InProgress::Rebase { .. }) => {
                            "abort the rebase: back to before it".into()
                        }
                        Some(InProgress::CherryPick) => "abort the cherry-pick".into(),
                        Some(InProgress::Revert) => "abort the revert".into(),
                        None => "give up on it".into(),
                    },
                    BatchKind::Undo => {
                        let before = ctx
                            .batch
                            .and_then(|b| b.run(&job.root))
                            .and_then(|r| r.before.as_ref());
                        match before {
                            Some(before) => format!("back to {}", head_text(before)),
                            None => "back to where it was".into(),
                        }
                    }
                    BatchKind::DeleteBranch { names, .. } => {
                        let name = status
                            .and_then(|s| name_in(s, names))
                            .unwrap_or(names.first().map_or("", String::as_str));
                        format!("delete the local branch {name}")
                    }
                    BatchKind::StashDrop => "drop its newest stash for good".into(),
                    _ => kind.label().to_lowercase(),
                };
                (label, what, Tone::Warning)
            })
            .collect()
    }

    /// Ask on a sheet before `kind` over `jobs`, which can't be taken
    /// back; Enter does it.
    fn confirm(&mut self, kind: BatchKind, jobs: Vec<Job>, ctx: &Ctx) {
        let lines = self.confirm_lines(&kind, &jobs, ctx);
        self.sheet = Some(Sheet::Confirm {
            kind,
            jobs,
            lines,
            choice: 0,
        });
    }

    /// Do `button`, as the footer, a menu or a key asks: what to do with
    /// it, if anything.
    pub fn press(&mut self, button: ReposButton, ctx: &Ctx) -> Option<Outcome> {
        self.act(button, ctx)
    }

    /// Do `button`: with a sheet open, answer it; what can't be taken
    /// back (a force push, an abort, an undo, a stash dropped) is asked
    /// about on a sheet first.
    fn act(&mut self, button: ReposButton, ctx: &Ctx) -> Option<Outcome> {
        if self.sheet.is_some() {
            return self.sheet_press(button, ctx);
        }
        if button == ReposButton::ClearReview {
            if self.diff.review.is_empty() {
                self.error = Some(NO_REVIEW.into());
            } else {
                self.error = None;
                self.branch_menu = None;
                self.sheet = Some(Sheet::ClearReview { choice: 0 });
            }
            return None;
        }
        if button == ReposButton::Help {
            self.branch_menu = None;
            self.sheet = Some(Sheet::Help { choice: 0 });
            return None;
        }
        {
            diff_ctx!(self, ctx, dctx);
            if self.diff.comment_button(button, &dctx) {
                self.follow_diff(ctx);
                return None;
            }
        }
        if let Some(how) = menu_integrate(button) {
            self.open_branch_menu(MenuAction::Integrate(how), ctx);
            return None;
        }
        match self.plan(button, ctx) {
            Err(Why::Field(why)) => {
                self.error = Some(why);
                self.focus = Focus::Field;
                None
            }
            Err(Why::Other(why)) => {
                self.error = Some(why);
                None
            }
            Ok(Outcome::Batch { kind, jobs }) if kind.confirmed_first() => {
                self.error = None;
                self.confirm(kind, jobs, ctx);
                None
            }
            Ok(outcome) => {
                self.error = None;
                Some(outcome)
            }
        }
    }

    /// Answer the sheet with `button`; it closes, unless `button` isn't
    /// one of its own.
    fn sheet_press(&mut self, button: ReposButton, ctx: &Ctx) -> Option<Outcome> {
        use ReposButton as B;
        let sheet = self.sheet.take()?;
        match (sheet, button) {
            (
                Sheet::Preflight {
                    mut kind,
                    jobs,
                    verdicts,
                    ..
                },
                B::All | B::CleanOnly,
            ) => {
                let verdict =
                    |root: &Path| verdicts.iter().find(|(r, _)| r == root).map(|(_, v)| v);
                // A repository not checked has nothing against it.
                let keep = |job: &Job| match verdict(&job.root) {
                    None | Some(Verdict::Clean) => true,
                    Some(Verdict::Conflicts(_) | Verdict::Unmerged(_)) => button == B::All,
                    Some(Verdict::Blocked(_)) => false,
                };
                let jobs: Vec<Job> = jobs.into_iter().filter(keep).collect();
                // All: the branches with commits nowhere else go too.
                if let BatchKind::DeleteBranch { force, .. } = &mut kind {
                    *force = button == B::All;
                }
                (!jobs.is_empty()).then_some(Outcome::Confirmed { kind, jobs })
            }
            (Sheet::Confirm { kind, jobs, .. }, button) if confirm_button(&kind) == button => {
                Some(Outcome::Batch { kind, jobs })
            }
            (Sheet::Help { .. }, _) => None,
            (Sheet::ClearReview { .. }, B::ClearReview) => {
                self.diff.clear_review(&sections(ctx.items));
                None
            }
            (Sheet::Diverged { roots, .. }, B::PullRebase | B::PullMerge) => {
                let how = if button == B::PullRebase {
                    Pull::Rebase
                } else {
                    Pull::Merge
                };
                let jobs: Vec<Job> = roots
                    .into_iter()
                    .map(|root| Job {
                        root,
                        files: Vec::new(),
                    })
                    .collect();
                (!jobs.is_empty()).then_some(Outcome::Confirmed {
                    kind: BatchKind::Pull(how),
                    jobs,
                })
            }
            // Switched where they have it (but where it is out already),
            // created where they don't.
            (
                Sheet::Exists {
                    name, jobs, have, ..
                },
                B::SwitchExisting,
            ) => {
                let on = |job: &Job| have.iter().any(|(r, f)| *r == job.root && *f == Found::On);
                let jobs: Vec<Job> = jobs.into_iter().filter(|j| !on(j)).collect();
                if jobs.is_empty() {
                    self.error = Some(format!("Already on {name}"));
                    return None;
                }
                Some(Outcome::Batch {
                    kind: BatchKind::Branch(name),
                    jobs,
                })
            }
            (
                Sheet::Exists {
                    name, jobs, have, ..
                },
                B::CreateMissing,
            ) => {
                let jobs = missing(&jobs, &have);
                (!jobs.is_empty()).then_some(Outcome::Batch {
                    kind: BatchKind::Branch(name),
                    jobs,
                })
            }
            (Sheet::Exists { name, .. }, B::EditName | B::Cancel) => {
                self.edit_branch_name(&name);
                None
            }
            (_, B::Cancel | B::SkipDiverged) => None,
            (sheet, _) => {
                self.sheet = Some(sheet);
                None
            }
        }
    }

    /// A key for the sheet, which takes them all: Left/Right/Tab choose a
    /// button, Enter presses it, Esc cancels.
    fn sheet_key(&mut self, mods: &ModifiersState, sym: Keysym, ctx: &Ctx) -> Option<Outcome> {
        let sheet = self.sheet.as_mut()?;
        let buttons = sheet_buttons(sheet);
        let (Sheet::Preflight { choice, .. }
        | Sheet::Diverged { choice, .. }
        | Sheet::Confirm { choice, .. }
        | Sheet::Help { choice, .. }
        | Sheet::ClearReview { choice, .. }
        | Sheet::Exists { choice, .. }) = sheet;
        let button = sheet_step(choice, &buttons, mods, sym)?;
        self.sheet_press(button, ctx)
    }

    /// Show the detail's `tab`; the diff where the selection is.
    fn set_tab(&mut self, tab: DetailTab) {
        if self.tab() != tab {
            self.tab = Some(tab);
            match tab {
                DetailTab::Diff => self.diff.scroll.jump(0.0),
                _ => self.detail_scroll.jump(0.0),
            }
            self.show_selected();
        }
    }

    /// Follow what the diff did: the keyboard it took, why it did nothing,
    /// and the file its cursor went to, selected in the tree (its
    /// repository unfolded for it) with the diff staying where it is.
    fn follow_diff(&mut self, ctx: &Ctx) {
        let signals = self.diff.take_signals();
        if signals.focus {
            self.set_tab(DetailTab::Diff);
            self.focus = Focus::Detail;
        }
        if let Some(error) = signals.error {
            self.error = error;
        }
        let Some(moved) = signals.moved else {
            return;
        };
        let items = ctx.items;
        let id = match moved {
            Moved::Section(root) => Selected::Repo(root),
            Moved::File(root, path) => Selected::File(root, path),
        };
        if let Selected::File(root, path) = &id
            && let Some(item) = items.iter().find(|i| i.root == *root)
        {
            self.set_open(item, true);
            self.reveal(root, path);
        }
        let rows = self.rows(items);
        let old = self.selected.replace(id);
        let at = self.selected_row(&rows, items);
        self.selected = old;
        if let Some(t) = at {
            let keep = self.diff.scroll.clone();
            self.select(&rows, t, items, ctx.page.0);
            self.diff.scroll = keep;
        }
    }

    /// What the diff asked for, its signals followed: a file checked or
    /// unchecked as its tree line, the rest as the panel asks it.
    fn diff_done(&mut self, outcome: Option<diff_view::Outcome>, ctx: &Ctx) -> Option<Outcome> {
        self.follow_diff(ctx);
        match outcome? {
            diff_view::Outcome::Toggle { root, path } => {
                let items = ctx.items;
                let k = items.iter().position(|i| i.root == root)?;
                let c = items[k]
                    .status
                    .changes
                    .iter()
                    .position(|c| c.path == path)?;
                self.toggle(Row::File(k, c, 0), items);
                None
            }
            diff_view::Outcome::Clear => self.act(ReposButton::ClearReview, ctx),
            outcome => Some(from_diff(outcome)),
        }
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
        if sym.is_modifier_key() {
            return None;
        }
        if self.sheet.is_some() {
            return self.sheet_key(mods, sym, ctx);
        }
        if self.branch_menu.is_some() {
            return self.branch_menu_key(mods, sym, typed, ctx);
        }
        // Ctrl+Shift+F opens the diff's search from anywhere.
        if mods.ctrl
            && mods.shift
            && !(mods.alt || mods.logo)
            && matches!(sym, Keysym::f | Keysym::F)
        {
            self.set_tab(DetailTab::Diff);
            self.focus = Focus::Detail;
        }
        // The diff's keys, while it shows: its search's, its comment
        // being written's, vim's, its own with its focus.
        if self.tab() == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            if let Some(outcome) = self.diff.key(mods, sym, typed, &dctx) {
                return self.diff_done(outcome, ctx);
            }
        }
        let only_ctrl = mods.ctrl && !(mods.shift || mods.alt || mods.logo);
        if only_ctrl && matches!(sym, Keysym::s | Keysym::S) {
            self.open_branch_menu(MenuAction::Switch, ctx);
            return None;
        }
        if only_ctrl && matches!(sym, Keysym::n | Keysym::N) {
            self.open_create_menu(ctx);
            return None;
        }
        let only_alt = mods.alt && !(mods.ctrl || mods.shift || mods.logo);
        if only_alt && matches!(sym, Keysym::f | Keysym::F) {
            return self.set_grouping(self.grouping.other(), ctx);
        }
        if only_alt && matches!(sym, Keysym::Left | Keysym::KP_Left) {
            self.fold_all(false, ctx);
            return None;
        }
        if only_alt && matches!(sym, Keysym::Right | Keysym::KP_Right) {
            self.fold_all(true, ctx);
            return None;
        }
        let in_diff = self.focus == Focus::Detail && self.tab() == DetailTab::Diff;
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        if sym == Keysym::Escape && plain {
            return Some(Outcome::Close);
        }
        if let Some(button) = action_key(mods, sym) {
            return self.act(button, ctx);
        }
        // Shift with the arrows or the pages scrolls the detail, with
        // either focus; in the diff, with its own, they reach the cursor's
        // range further.
        if plain && mods.shift && !in_diff {
            let page = ctx.page.1.max(1) as i64;
            let by = match sym {
                Keysym::Down | Keysym::KP_Down => 1,
                Keysym::Up | Keysym::KP_Up => -1,
                Keysym::Page_Down | Keysym::KP_Page_Down => page,
                Keysym::Page_Up | Keysym::KP_Page_Up => -page,
                _ => 0,
            };
            if by != 0 {
                self.scroll_by(by, true, ctx);
                return None;
            }
        }
        if mods.ctrl && !mods.alt && !mods.logo {
            match sym {
                Keysym::d | Keysym::D => {
                    self.set_tab(DetailTab::Diff);
                    return None;
                }
                Keysym::t | Keysym::T => {
                    self.set_tab(DetailTab::Log);
                    return None;
                }
                _ => {}
            }
        }
        if plain && matches!(sym, Keysym::Tab | Keysym::ISO_Left_Tab) {
            self.focus = match self.focus {
                Focus::Tree => Focus::Detail,
                Focus::Detail => Focus::Field,
                Focus::Field => Focus::Tree,
            };
            return None;
        }
        // The arrows, the pages, Home and End scroll the log.
        if self.focus == Focus::Detail && !in_diff && plain && !mods.shift {
            let page = ctx.page.1.max(1) as i64;
            let by = match sym {
                Keysym::Down | Keysym::KP_Down => 1,
                Keysym::Up | Keysym::KP_Up => -1,
                Keysym::Page_Down | Keysym::KP_Page_Down => page,
                Keysym::Page_Up | Keysym::KP_Page_Up => -page,
                Keysym::Home | Keysym::KP_Home => i64::MIN,
                Keysym::End | Keysym::KP_End => i64::MAX,
                _ => 0,
            };
            if by != 0 {
                self.scroll_by(by, true, ctx);
                return None;
            }
        }
        if self.focus == Focus::Field {
            match self.field.key(mods, sym, typed) {
                Edit::Ignored => {}
                Edit::Changed => {
                    self.error = None;
                    return None;
                }
                Edit::Moved => return None,
            }
        }
        self.tree_key(mods, sym, typed, ctx)
    }

    /// A key for the tree: Up/Down and the pages move with either focus,
    /// the rest only with the tree's; typing goes to the field. Enter on
    /// a file opens it in Zed.
    fn tree_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Outcome> {
        let items = ctx.items;
        let page = ctx.page.0.max(1);
        let rows = self.rows(items);
        let at = self.selected_row(&rows, items);
        let n = rows.len() as isize;
        let step = |by: isize| match at {
            _ if n == 0 => None,
            Some(k) => Some((k as isize + by).clamp(0, n - 1) as usize),
            None => Some(0),
        };
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let tree = self.focus == Focus::Tree;
        let row = at.map(|k| rows[k]);
        let moved = match sym {
            Keysym::Down | Keysym::KP_Down => step(1),
            Keysym::Up | Keysym::KP_Up => step(-1),
            Keysym::Page_Down | Keysym::KP_Page_Down => step(page as isize),
            Keysym::Page_Up | Keysym::KP_Page_Up => step(-(page as isize)),
            Keysym::Home | Keysym::KP_Home if tree => step(-n),
            Keysym::End | Keysym::KP_End if tree => step(n),
            _ => None,
        };
        if let Some(k) = moved {
            self.select(&rows, k, items, page);
            return None;
        }
        if !tree {
            return None;
        }
        match sym {
            Keysym::space if plain => {
                if let Some(row) = row {
                    self.toggle(row, items);
                }
            }
            Keysym::a | Keysym::A if mods.ctrl && !mods.alt && !mods.logo => {
                self.toggle_all(items);
            }
            Keysym::Right | Keysym::KP_Right if plain => match row {
                Some(Row::Repo(k)) => self.set_open(&items[k], true),
                Some(row @ Row::Dir { .. }) => self.set_dir_open(row, items, true),
                Some(Row::Clean) => self.clean_folded = false,
                _ => {}
            },
            // A line folds, else the one over it is selected.
            Keysym::Left | Keysym::KP_Left if plain => match (row, at) {
                (Some(Row::Repo(k)), _) => {
                    self.set_open(&items[k], false);
                    self.settle(items, page);
                }
                (Some(row @ Row::Dir { .. }), _) if self.dir_open(row, items) => {
                    self.set_dir_open(row, items, false);
                    self.settle(items, page);
                }
                (Some(Row::Clean), _) => {
                    self.clean_folded = true;
                    self.settle(items, page);
                }
                (Some(_), Some(at)) => {
                    if let Some(up) = Panel::parent(&rows, at) {
                        self.select(&rows, up, items, page);
                    }
                }
                _ => {}
            },
            Keysym::Return | Keysym::KP_Enter if plain => {
                if let Some(row) = row {
                    return self.fold_line(row, ctx);
                }
            }
            _ => {
                let printable = typed.key_char().is_some_and(|c| !c.is_control());
                if sym == text_field::PASTE || (plain && printable) {
                    self.focus = Focus::Field;
                    if self.field.key(mods, sym, typed).changed() {
                        self.error = None;
                    }
                }
            }
        }
        None
    }

    /// A click on `hit`.
    pub fn click(&mut self, hit: ReposHit, ctx: &Ctx) -> Option<Outcome> {
        if self.sheet.is_some() {
            return match hit {
                ReposHit::Button(button) => self.sheet_press(button, ctx),
                _ => None,
            };
        }
        // The open menu takes the click: elsewhere, it closes.
        if self.branch_menu.is_some() {
            let page = self.menu_page(ctx);
            let (lines, _) = self.menu_window(&self.shown_branches(ctx), page);
            // Row `i` in view: the branch of the `i`th line not a heading.
            let branch = |i: usize| {
                let mut ks = lines.iter().filter_map(|l| match l {
                    MenuLine::Branch(k) => Some(*k),
                    MenuLine::Heading { .. } => None,
                });
                ks.nth(i)
            };
            return match hit {
                ReposHit::BranchCreate => self.create_branch(ctx),
                ReposHit::BranchRow(i) => self.pick_branch(branch(i)?, ctx),
                ReposHit::BranchDelete(i) if self.menu_action() == MenuAction::Switch => {
                    self.delete_branch(branch(i)?, ctx)
                }
                ReposHit::BranchMenu => None,
                _ => {
                    self.branch_menu = None;
                    None
                }
            };
        }
        // The diff's parts act on it; a click elsewhere takes the
        // keyboard from its search and keeps the comment being written.
        {
            diff_ctx!(self, ctx, dctx);
            if let Some(outcome) = self.diff.click(hit, &dctx) {
                return self.diff_done(outcome, ctx);
            }
        }
        self.follow_diff(ctx);
        let items = ctx.items;
        let page = ctx.page.0.max(1);
        let rows = self.rows(items);
        let (first, part) = standing(&self.tree_scroll, rows.len(), page);
        // The line shown in part under the tree too.
        let shown = page + usize::from(part > 0.0);
        let at = |i: usize| (i < shown && first + i < rows.len()).then_some(first + i);
        match hit {
            ReposHit::Line(i) | ReposHit::Mark(i, RepoMark::Dirty) => {
                let k = at(i)?;
                self.select(&rows, k, items, page);
                self.focus = Focus::Tree;
            }
            ReposHit::Chip(i) => {
                let k = at(i)?;
                self.select(&rows, k, items, page);
                self.focus = Focus::Tree;
                // What the batch did there is seen: hide it.
                let root = &items[rows[k].item()?].root;
                if let Some(b) = ctx.batch.filter(|b| b.run(root).is_some()) {
                    self.seen_chips.retain(|(id, _)| *id == b.id);
                    self.seen_chips.insert((b.id, root.clone()));
                }
            }
            ReposHit::Mark(i, mark) => {
                let k = at(i)?;
                self.select(&rows, k, items, page);
                self.focus = Focus::Tree;
                return self.mark_press(mark, rows[k].item()?, ctx);
            }
            ReposHit::Check(i) => {
                let k = at(i)?;
                self.select(&rows, k, items, page);
                self.toggle(rows[k], items);
            }
            ReposHit::Expander(i) => {
                let k = at(i)?;
                self.select(&rows, k, items, page);
                if !matches!(rows[k], Row::File(..)) {
                    return self.fold_line(rows[k], ctx);
                }
            }
            ReposHit::Tool(tool) => match tool {
                TreeTool::Expand => self.fold_all(true, ctx),
                TreeTool::Collapse => self.fold_all(false, ctx),
                TreeTool::List => return self.set_grouping(Grouping::List, ctx),
                TreeTool::Folders => return self.set_grouping(Grouping::Folders, ctx),
            },
            ReposHit::Field => self.focus = Focus::Field,
            ReposHit::Button(button) => return self.act(button, ctx),
            ReposHit::DetailTab(tab) => self.set_tab(tab),
            ReposHit::DiffRow(_)
            | ReposHit::DiffCheck(_)
            | ReposHit::DiffFold(_)
            | ReposHit::DiffRestore(_) => {}
            ReposHit::BranchButton => self.open_branch_menu(MenuAction::Switch, ctx),
            ReposHit::Detail
            | ReposHit::Panel
            | ReposHit::More
            | ReposHit::Search(_)
            | ReposHit::Splitter
            | ReposHit::BranchRow(_)
            | ReposHit::BranchDelete(_)
            | ReposHit::BranchCreate
            | ReposHit::BranchMenu => {}
        }
        None
    }

    /// Enter on tree line `row`: a repository or a folder folds, or
    /// unfolds; a file opens in Zed.
    fn fold_line(&mut self, row: Row, ctx: &Ctx) -> Option<Outcome> {
        let (items, page) = (ctx.items, ctx.page.0.max(1));
        match row {
            Row::Repo(k) => {
                let open = !self.open(&items[k]);
                self.set_open(&items[k], open);
            }
            Row::Dir { .. } => {
                let open = !self.dir_open(row, items);
                self.set_dir_open(row, items, open);
            }
            Row::File(..) => return self.act(ReposButton::OpenInZed, ctx),
            Row::Clean => self.clean_folded = !self.clean_folded,
        }
        self.settle(items, page);
        None
    }

    /// Whether a press on the diff, held, drags the cursor's range on.
    pub fn drags_rows(&self, ctx: &Ctx) -> bool {
        if self.tab() != DetailTab::Diff {
            return false;
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.drags_rows(&dctx)
    }

    /// The pointer dragged past the diff's rows, `by` (up negative).
    pub fn drag_past_rows(&mut self, by: i64, ctx: &Ctx) {
        if self.tab() != DetailTab::Diff {
            return;
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.drag_past_rows(by, &dctx);
        self.follow_diff(ctx);
    }

    /// Shift+click on row `i` of the diff: the cursor goes there, its
    /// range reaching back to where it was.
    pub fn extend_detail_row(&mut self, i: usize, ctx: &Ctx) {
        if self.tab() != DetailTab::Diff {
            return;
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.extend_detail_row(i, &dctx);
        self.follow_diff(ctx);
    }

    /// Open the file shown as row `i` of the diff in Zed, at that line.
    pub fn open_detail_row(&mut self, i: usize, ctx: &Ctx) -> Option<Outcome> {
        if self.tab() != DetailTab::Diff {
            return None;
        }
        diff_ctx!(self, ctx, dctx);
        let outcome = self.diff.open_detail_row(i, &dctx);
        self.diff_done(outcome, ctx)
    }

    /// Ctrl+click on row `i` of the diff: the cursor there, and its code
    /// opened in Zed (`DiffView::goto_detail_row`).
    pub fn goto_detail_row(&mut self, i: usize, ctx: &Ctx) -> Option<Outcome> {
        if self.tab() != DetailTab::Diff {
            return None;
        }
        diff_ctx!(self, ctx, dctx);
        let outcome = self.diff.goto_detail_row(i, &dctx);
        self.diff_done(outcome, ctx)
    }

    /// The repositories whose diff wants reading, with their changes in
    /// the tree's order and the Git status read it is for, while the diff
    /// shows.
    pub fn diffs_wanted(&self, ctx: &Ctx) -> Vec<(PathBuf, Vec<Change>, u64)> {
        if self.tab() != DetailTab::Diff {
            return Vec::new();
        }
        diff_ctx!(self, ctx, dctx);
        let wanted = self.diff.diffs_wanted(&dctx);
        wanted
            .into_iter()
            .filter_map(|(root, generation)| {
                let item = ctx.items.iter().find(|i| i.root == root)?;
                let changes = &item.status.changes;
                let order = tree_order(changes, self.grouping);
                let changes = order.into_iter().map(|c| changes[c].clone()).collect();
                Some((root, changes, generation))
            })
            .collect()
    }

    /// A repository's diff came back, read for Git status read
    /// `generation`: put in the tree's order (listed another way since,
    /// maybe) and shown.
    pub fn set_diffs(
        &mut self,
        root: PathBuf,
        generation: u64,
        mut files: Vec<FileDiff>,
        ctx: &Ctx,
    ) {
        if let Some(item) = ctx.items.iter().find(|i| i.root == root) {
            let rank = self.rank(item);
            files.sort_by_key(|f| rank.get(f.path.as_str()).copied().unwrap_or(usize::MAX));
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.set_diffs(root, generation, files, &dctx);
        self.follow_diff(ctx);
    }

    /// Whether tree line `i` in view is a file: a double click on one
    /// opens it in Zed.
    pub fn is_file_line(&self, i: usize, ctx: &Ctx) -> bool {
        let rows = self.rows(ctx.items);
        let (first, _) = standing(&self.tree_scroll, rows.len(), ctx.page.0);
        matches!(rows.get(first + i), Some(Row::File(..)))
    }

    /// A click on a mark of repository `k`'s line: the arrows push or pull
    /// it alone, whatever is checked; the stash count lists its stashes.
    fn mark_press(&mut self, mark: RepoMark, k: usize, ctx: &Ctx) -> Option<Outcome> {
        let kind = match mark {
            RepoMark::Dirty => return None,
            RepoMark::Stashes => {
                self.set_tab(DetailTab::Stashes);
                return None;
            }
            RepoMark::Ahead => BatchKind::Push { force: false },
            RepoMark::Behind => BatchKind::Pull(Pull::FastForward),
        };
        if ctx.batch.is_some_and(|b| !b.finished()) {
            self.error = Some("Wait for the batch to finish".into());
            return None;
        }
        self.error = None;
        let jobs = vec![Job {
            root: ctx.items[k].root.clone(),
            files: Vec::new(),
        }];
        Some(Outcome::Batch { kind, jobs })
    }

    /// Stand the detail pane, or else the tree, at line `pos`, to the
    /// fraction, as its scrollbar's thumb held there puts it.
    pub fn stand_at(&mut self, pos: f64, detail: bool, ctx: &Ctx) {
        if detail && self.tab() == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            return self.diff.stand_at(pos, &dctx);
        }
        let b = self.scroll_bounds(detail, ctx);
        let pos = pos.clamp(0.0, b.max.max(0.0));
        if detail {
            self.detail_scroll.jump(pos);
        } else {
            self.tree_scroll.jump(pos);
        }
    }

    /// The detail pane, or else the tree, moved by `motion` at `now`.
    pub fn scroll(&mut self, motion: Motion, now: Instant, over_detail: bool, ctx: &Ctx) {
        if over_detail && self.tab() == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            return self.diff.scroll(motion, now, &dctx);
        }
        let b = self.scroll_bounds(over_detail, ctx);
        match over_detail {
            true => self.detail_scroll.apply(motion, now, b),
            false => self.tree_scroll.apply(motion, now, b),
        }
    }

    /// Move the lists moving on their own on to `now`; whether one still
    /// moves (see `Scroll::moving`).
    pub fn scroll_tick(&mut self, now: Instant, ctx: &Ctx) -> bool {
        let mut moving = false;
        if self.tree_scroll.moving(now) {
            let b = self.scroll_bounds(false, ctx);
            moving |= self.tree_scroll.tick(now, b);
        }
        if self.tab() == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            moving |= self.diff.scroll_tick(now, &dctx);
        } else if self.detail_scroll.moving(now) {
            let b = self.scroll_bounds(true, ctx);
            moving |= self.detail_scroll.tick(now, b);
        }
        if self
            .branch_menu
            .as_ref()
            .is_some_and(|m| m.scroll.moving(now))
        {
            let b = self.menu_bounds(ctx);
            if let Some(menu) = self.branch_menu.as_mut() {
                moving |= menu.scroll.tick(now, b);
            }
        }
        moving
    }

    /// Whether a list moves on its own or under the fingers at `now`.
    pub fn scrolling(&self, now: Instant) -> bool {
        self.tree_scroll.moving(now)
            || self.detail_scroll().moving(now)
            || self
                .branch_menu
                .as_ref()
                .is_some_and(|m| m.scroll.moving(now))
    }

    /// Where the detail pane stands: the diff's, else the log's.
    fn detail_scroll(&self) -> &Scroll {
        match self.tab() {
            DetailTab::Diff => &self.diff.scroll,
            _ => &self.detail_scroll,
        }
    }

    /// The detail's rows, or else the tree's lines, as a list scrolled.
    fn scroll_bounds(&self, over_detail: bool, ctx: &Ctx) -> Bounds {
        if over_detail && self.tab() == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            return self.diff.scroll_bounds(&dctx);
        }
        let (_, last) = self.scrolled(over_detail, ctx);
        let (page, unit) = match over_detail {
            true => (ctx.page.1, crate::chrome::repos_detail_line(ctx.density)),
            false => (ctx.page.0, crate::chrome::repos_row(ctx.density)),
        };
        bounds(last + page.max(1), page, unit)
    }

    /// Scroll the detail, or else the tree, by `whole` lines (down
    /// positive), as far as there are lines.
    fn scroll_by(&mut self, whole: i64, over_detail: bool, ctx: &Ctx) {
        if over_detail && self.tab() == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            return self.diff.scroll_by(whole, &dctx);
        }
        let (first, last) = self.scrolled(over_detail, ctx);
        let to = (first as i64).saturating_add(whole).clamp(0, last as i64) as usize;
        if over_detail {
            self.detail_scroll.jump(to as f64);
        } else {
            self.tree_scroll.jump(to as f64);
        }
    }

    /// The first line in view of the log, or else of the tree, and the
    /// last it can be scrolled to.
    fn scrolled(&self, over_detail: bool, ctx: &Ctx) -> (usize, usize) {
        let last = if !over_detail {
            let len = self.rows(ctx.items).len();
            len.saturating_sub(ctx.page.0.max(1))
        } else {
            let len = self.detail_lines(ctx).len();
            len.saturating_sub(ctx.page.1.max(1))
        };
        let first = if over_detail {
            self.detail_scroll.first()
        } else {
            self.tree_scroll.first()
        };
        (first.min(last), last)
    }

    /// `item`'s state chip: the operation it is stopped in, else what the
    /// last batch did to it (unless clicked away), else a detached HEAD.
    fn chip(&self, item: &RepoItem, batch: Option<&Batch>) -> Option<Chip> {
        let chip = |text: &str, tone: Tone, tip: &str| {
            Some(Chip {
                text: text.into(),
                tone,
                tip: tip.into(),
            })
        };
        if let Some(op) = item.status.op {
            let (text, name) = match op {
                InProgress::Merge => ("MERGING".to_owned(), "A merge"),
                InProgress::Rebase { step, total } => {
                    (format!("REBASING {step}/{total}"), "A rebase")
                }
                InProgress::CherryPick => ("CHERRY-PICKING".into(), "A cherry-pick"),
                InProgress::Revert => ("REVERTING".into(), "A revert"),
            };
            let conflicts = conflicted_files(item).len();
            let (text, tip) = match conflicts {
                0 => (
                    text,
                    format!("{name} is half-way with no conflict left: continue or abort it"),
                ),
                n => (
                    format!("{text} · {n}"),
                    format!(
                        "{name} stopped on {}: open them in Zed, mark them resolved, then continue",
                        count(n, "conflicted file")
                    ),
                ),
            };
            return chip(&text, Tone::Error, &tip);
        }
        let seen = |b: &Batch| self.seen_chips.contains(&(b.id, item.root.clone()));
        if let Some(run) = batch.filter(|b| !seen(b)).and_then(|b| b.run(&item.root)) {
            let ctx_kind = batch.map(|b| &b.kind);
            let done = batch.map(|b| b.kind.done()).unwrap_or_default();
            let batch = batch.map(|b| b.kind.label()).unwrap_or_default();
            match &run.state {
                RunState::Done => {
                    return chip(done, Tone::Success, &format!("{batch} went through here"));
                }
                RunState::Skipped(why) => {
                    return chip(
                        why,
                        Tone::Muted,
                        &format!("{batch} was skipped here: {why}"),
                    );
                }
                RunState::Failed { failure, why } => {
                    let (text, tone, tip) = match failure {
                        Failure::Conflict(_) => (
                            "conflict",
                            Tone::Error,
                            format!(
                                "{batch} stopped on conflicts: open them in Zed, mark them resolved, then continue"
                            ),
                        ),
                        Failure::Auth => (
                            "login needed",
                            Tone::Error,
                            "The remote wants a login: run it in a terminal to give it".into(),
                        ),
                        Failure::Rejected => (
                            "rejected",
                            Tone::Error,
                            match ctx_kind {
                                Some(BatchKind::Push { force: true }) => {
                                    "The remote branch has commits not fetched or not seen here: fetch and look at them first".into()
                                }
                                _ => "The remote refused the push: pull first, or force push".into(),
                            },
                        ),
                        Failure::Diverged => (
                            "diverged",
                            Tone::Warning,
                            "Its branch and its upstream both moved on: pull to rebase or merge"
                                .into(),
                        ),
                        Failure::CheckedOut(path) => (
                            "checked out elsewhere",
                            Tone::Warning,
                            format!("The branch is checked out in {}", path.display()),
                        ),
                        Failure::Locked => (
                            "locked",
                            Tone::Warning,
                            "Another git command holds its lock: try again once it is done, or delete the lock if none runs".into(),
                        ),
                        Failure::Timeout => (
                            "timed out",
                            Tone::Error,
                            "The remote took too long: run it in a terminal".into(),
                        ),
                        Failure::Blocked(_) => return chip("blocked", Tone::Warning, why),
                        Failure::Other => (
                            "failed",
                            Tone::Error,
                            format!("{batch} failed: see the log (Ctrl+T)"),
                        ),
                    };
                    // What git said first: the hint alone hides it.
                    let tip = match why.trim() {
                        "" => tip,
                        why => format!("{why} · {tip}"),
                    };
                    return chip(text, tone, &tip);
                }
                RunState::Queued | RunState::Running(_) => {}
            }
        }
        if item.status.branch.is_none() && item.status.head.is_some() {
            return chip(
                "detached",
                Tone::Warning,
                "Not on a branch: switch to one before committing or pushing",
            );
        }
        None
    }

    /// Line `row` of the tree.
    fn tree_line(&self, row: Row, ctx: &Ctx) -> TreeLine {
        let Some(k) = row.item() else {
            return TreeLine::Clean(CleanLine {
                open: !self.clean_folded,
                count: ctx.items.iter().filter(|i| self.is_clean(i)).count(),
            });
        };
        let item = &ctx.items[k];
        let status = &item.status;
        let files = self.diff.diffs.get(&item.root).map(|(_, f)| f.as_slice());
        let files = files.unwrap_or_default();
        if let Row::Dir {
            from, to, depth, ..
        } = row
        {
            let path = Panel::dir_path(row, ctx.items).unwrap_or_default();
            return TreeLine::Dir(DirLine {
                check: self.dir_check(item, path),
                open: self.dir_open(row, ctx.items),
                name: path_parts(path, from, to).to_owned(),
                depth,
            });
        }
        let Row::File(_, c, depth) = row else {
            let chip = self.chip(item, ctx.batch);
            let run = ctx.batch.and_then(|b| b.run(&item.root));
            let state = match run.map(|r| &r.state) {
                Some(RunState::Running(what)) => LineState::Busy(what.clone()),
                Some(RunState::Queued) => LineState::Busy("waiting…".into()),
                _ if chip
                    .as_ref()
                    .is_some_and(|c| matches!(c.tone, Tone::Error | Tone::Warning)) =>
                {
                    LineState::Attention
                }
                _ => LineState::Settled,
            };
            return TreeLine::Repo(RepoLine {
                check: self.check(item),
                expanded: (!status.changes.is_empty()).then(|| self.open(item)),
                label: item.label.clone(),
                branch: status.branch.clone(),
                dirty: status.dirty,
                ahead: status.ahead,
                behind: status.behind,
                no_upstream: status.branch.is_some() && status.upstream.is_none(),
                stashes: status.stashes,
                files: status.changes.len(),
                added: files.iter().map(|f| f.added).sum(),
                removed: files.iter().map(|f| f.removed).sum(),
                chip,
                state,
            });
        };
        let change = &status.changes[c];
        let diff = files.iter().find(|f| f.path == change.path);
        TreeLine::File(FileLine {
            check: if self.file_checked(item, change) {
                Check::On
            } else {
                Check::Off
            },
            code: change.code,
            path: match self.grouping {
                Grouping::List => change_text(change),
                Grouping::Folders => file_name_text(change),
            },
            conflicted: change.conflicted(),
            depth,
            added: diff.map_or(0, |f| f.added),
            removed: diff.map_or(0, |f| f.removed),
        })
    }

    /// The header's line after the title: how many are dirty and
    /// conflicted, and the latest fetch.
    fn summary(&self, ctx: &Ctx) -> String {
        let items = ctx.items;
        if items.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        let dirty = items.iter().filter(|i| i.status.dirty).count();
        if dirty > 0 {
            parts.push(format!("{dirty} dirty"));
        }
        let conflicts = items.iter().filter(|i| in_conflict(i)).count();
        if conflicts > 0 {
            parts.push(count(conflicts, "conflict"));
        }
        // A fetch's own FETCH_HEAD, the first repository's done, would
        // say "fetched 0s ago" while the others still fetch.
        let fetch_batch = ctx.batch.is_some_and(|b| {
            !b.finished() && matches!(b.kind, BatchKind::Fetch | BatchKind::Pull(_))
        });
        parts.push(if ctx.fetching || fetch_batch {
            "fetching…".into()
        } else {
            match items.iter().filter_map(|i| i.status.fetched).max() {
                Some(at) => format!("fetched {} ago", ago(at, ctx.now)),
                None => "never fetched".into(),
            }
        });
        parts.join(" · ")
    }

    /// The footer's actions for what is selected: what fixes it first,
    /// then the usual ones, each with why it can't be done now (but not
    /// for want of text: pressed, it asks for it).
    fn hints(&self, ctx: &Ctx) -> Vec<Hint> {
        use ReposButton as B;
        let items = ctx.items;
        if items.is_empty() {
            return Vec::new();
        }
        let selected = self.selected_item(items);
        let mut buttons = Vec::new();
        if let Some(item) = selected {
            let run = ctx.batch.and_then(|b| b.run(&item.root));
            match run.and_then(|r| r.state.failure()) {
                Some(Failure::Auth | Failure::Timeout | Failure::Other) => {
                    buttons.push(B::RunInTerminal);
                }
                Some(Failure::CheckedOut(_)) => buttons.push(B::OpenWorkspace),
                _ => {}
            }
        }
        if selected.is_some_and(in_conflict) {
            buttons.extend([B::OpenInZed, B::MarkResolved, B::Continue, B::Abort]);
        } else {
            buttons.extend([B::Commit, B::CommitPush, B::Push]);
            // After a rebase the branch and its upstream both moved on.
            if selected.is_some_and(|i| i.status.ahead > 0 && i.status.behind > 0) {
                buttons.push(B::ForcePush);
            }
            buttons.extend([
                B::Fetch,
                B::Pull,
                B::Merge,
                B::Rebase,
                B::MergeFrom,
                B::RebaseFrom,
            ]);
            // The stashes of the selected repository: made while it has
            // changes, brought back or dropped while it has some.
            if let Some(item) = selected {
                if item.status.dirty {
                    buttons.push(B::Stash);
                }
                if item.status.stashes > 0 {
                    buttons.extend([B::StashPop, B::StashDrop]);
                }
            }
        }
        if !self.diff.review.is_empty() {
            buttons.extend([B::CopyReview, B::ClearReview]);
        }
        buttons.push(B::Help);
        buttons
            .into_iter()
            .map(|button| Hint {
                button,
                label: self.button_label(button, ctx),
                key: key_name(button).into(),
                disabled: match self.plan(button, ctx) {
                    _ if button == ReposButton::Help => None,
                    _ if button == ReposButton::ClearReview => {
                        self.diff.review.is_empty().then(|| NO_REVIEW.into())
                    }
                    _ if menu_integrate(button).is_some() => {
                        self.menu_blocked(ctx).map(String::from)
                    }
                    Err(Why::Other(why)) => Some(why),
                    _ => None,
                },
            })
            .collect()
    }

    /// The detail tab shown: the one asked for, else the diff.
    fn tab(&self) -> DetailTab {
        self.tab.unwrap_or_default()
    }

    /// The log or stashes the selection shows, read or not; none for the
    /// diff, read per repository.
    fn detail_key(&self) -> Option<DetailKey> {
        let root = match self.selected.as_ref()? {
            Selected::Repo(root) | Selected::Dir(root, _) | Selected::File(root, _) => root,
            Selected::Clean => return None,
        };
        let tab = self.tab();
        (tab != DetailTab::Diff).then(|| DetailKey {
            root: root.clone(),
            file: None,
            tab,
        })
    }

    /// Every line of the log or stashes: as read, the log with every
    /// command the last batch ran there and what it printed.
    fn detail_lines(&self, ctx: &Ctx) -> Vec<DetailLine> {
        let Some((k, _)) = self.selection(ctx.items) else {
            return Vec::new();
        };
        let key = self.detail_key();
        let text = self
            .detail
            .as_ref()
            .filter(|(k, _)| Some(k) == key.as_ref())
            .map(|(_, text)| text.as_str());
        match self.tab() {
            DetailTab::Diff => Vec::new(),
            DetailTab::Stashes => text
                .into_iter()
                .flat_map(str::lines)
                .map(|l| detail_line(l, DiffTone::Plain))
                .collect(),
            DetailTab::Log => {
                let mut lines: Vec<DetailLine> = text
                    .into_iter()
                    .flat_map(str::lines)
                    .map(|l| detail_line(l, DiffTone::Plain))
                    .collect();
                let root = &ctx.items[k].root;
                let Some(run) = ctx.batch.and_then(|b| b.run(root)) else {
                    return lines;
                };
                let why = match &run.state {
                    RunState::Failed { why, .. } => Some((why, DiffTone::Error)),
                    RunState::Skipped(why) => Some((why, DiffTone::Plain)),
                    _ => None,
                };
                if (!run.log.is_empty() || why.is_some()) && !lines.is_empty() {
                    lines.push(detail_line("", DiffTone::Plain));
                }
                for ran in &run.log {
                    lines.push(detail_line(&format!("$ {}", ran.command), DiffTone::Header));
                    let tone = if ran.ok {
                        DiffTone::Plain
                    } else {
                        DiffTone::Error
                    };
                    lines.extend(ran.output.lines().map(|l| detail_line(l, tone)));
                }
                // Nothing ran: what kept it from it.
                if let Some((why, tone)) = why.filter(|_| run.log.is_empty()) {
                    lines.push(detail_line(why, tone));
                }
                lines
            }
        }
    }

    /// The detail pane, scrolled: of a long diff only the page is copied.
    fn detail_view(&self, ctx: &Ctx) -> DetailView {
        let page = ctx.page.1.max(1);
        let tab = self.tab();
        let focused = self.focus == Focus::Detail;
        if tab == DetailTab::Diff {
            diff_ctx!(self, ctx, dctx);
            return self.diff.view(&dctx);
        }
        let Some((k, _)) = self.selection(ctx.items) else {
            return DetailView {
                tab,
                focused,
                ..DetailView::default()
            };
        };
        let lines = self.detail_lines(ctx);
        let total = lines.len();
        // Scrolled part of a line, the pane shows one more under it.
        let (first, part) = standing(&self.detail_scroll, total, page);
        DetailView {
            tab,
            title: ctx.items[k].label.clone(),
            rows: lines
                .into_iter()
                .skip(first)
                .take(page + usize::from(part > 0.0))
                .map(DetailRow::Text)
                .collect(),
            sticky: false,
            marks: Vec::new(),
            title_buttons: Vec::new(),
            row_buttons: Vec::new(),
            part,
            over: self.detail_scroll.overscroll(),
            pin: 0.0,
            stuck: false,
            first,
            total,
            focused,
            ..DetailView::default()
        }
    }

    /// The sheet, as shown.
    fn sheet_view(&self, ctx: &Ctx) -> Option<SheetView> {
        let sheet = self.sheet.as_ref()?;
        let label = |root: &Path| {
            ctx.items.iter().find(|i| i.root == root).map_or_else(
                || {
                    root.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into()
                },
                |i| i.label.clone(),
            )
        };
        let (title, lines, choice) = match sheet {
            Sheet::Preflight {
                kind,
                jobs,
                verdicts,
                choice,
            } => {
                let rebase = matches!(
                    kind,
                    BatchKind::Integrate {
                        how: Integrate::Rebase,
                        ..
                    } | BatchKind::Pull(Pull::Rebase)
                );
                let conflict = if rebase { "may conflict" } else { "conflicts" };
                let (mut clean, mut conflicts, mut unmerged, mut blocked) = (0, 0, 0, 0);
                let lines: Vec<(String, String, Tone)> = jobs
                    .iter()
                    .map(|job| {
                        let verdict = verdicts.iter().find(|(r, _)| *r == job.root);
                        let (text, tone) = match verdict.map(|(_, v)| v) {
                            None | Some(Verdict::Clean) => {
                                clean += 1;
                                ("clean".to_owned(), Tone::Success)
                            }
                            Some(Verdict::Conflicts(files)) => {
                                conflicts += 1;
                                (format!("{conflict}: {}", files.join(", ")), Tone::Warning)
                            }
                            Some(Verdict::Unmerged(n)) => {
                                unmerged += 1;
                                let lost = count(*n, "commit");
                                (format!("unmerged: {lost} only on it"), Tone::Warning)
                            }
                            Some(Verdict::Blocked(why)) => {
                                blocked += 1;
                                (format!("blocked: {why}"), Tone::Error)
                            }
                        };
                        (label(&job.root), text, tone)
                    })
                    .collect();
                let mut found = Vec::new();
                if clean > 0 {
                    found.push(format!("{clean} clean"));
                }
                if conflicts > 0 {
                    let would = if rebase {
                        "may conflict"
                    } else {
                        "would conflict"
                    };
                    found.push(format!("{conflicts} {would}"));
                }
                if unmerged > 0 {
                    found.push(format!("{unmerged} unmerged"));
                }
                if blocked > 0 {
                    found.push(format!("{blocked} blocked"));
                }
                let title = format!("{}: {}", kind.label(), found.join(" · "));
                (title, lines, *choice)
            }
            Sheet::Diverged { roots, choice } => {
                let lines = roots
                    .iter()
                    .map(|root| {
                        let item = ctx.items.iter().find(|i| i.root == *root);
                        let text = item.map_or_else(
                            || "diverged".into(),
                            |i| format!("diverged: ↑{} ↓{}", i.status.ahead, i.status.behind),
                        );
                        (label(root), text, Tone::Warning)
                    })
                    .collect();
                let title = format!(
                    "Pull: {} diverged from upstream: rebase or merge?",
                    repos(roots.len())
                );
                (title, lines, *choice)
            }
            Sheet::Confirm {
                kind,
                jobs,
                lines,
                choice,
            } => {
                let what = match kind {
                    BatchKind::DeleteBranch { names, .. } => {
                        format!("Delete the branch {}", names.join(" / "))
                    }
                    BatchKind::StashDrop => "Drop the newest stash".into(),
                    BatchKind::Undo => format!(
                        "Undo {}",
                        ctx.batch
                            .map(|b| b.kind.label().to_lowercase())
                            .unwrap_or_default()
                    ),
                    kind => kind.label(),
                };
                let title = format!("{what} in {}?", repos(jobs.len()));
                (title, lines.clone(), *choice)
            }
            Sheet::Exists {
                name,
                jobs,
                have,
                choice,
            } => {
                let lines = jobs
                    .iter()
                    .map(|job| {
                        let found = have.iter().find(|(r, _)| *r == job.root);
                        let (text, tone) = match found.map(|(_, f)| f) {
                            Some(Found::On) => ("on it already".to_owned(), Tone::Success),
                            Some(Found::Local) => ("has it".to_owned(), Tone::Warning),
                            Some(Found::Remote(remote)) => {
                                (format!("has it on {remote}"), Tone::Warning)
                            }
                            None => ("doesn't have it".to_owned(), Tone::Normal),
                        };
                        (label(&job.root), text, tone)
                    })
                    .collect();
                let title = if have.len() == jobs.len() {
                    format!("{name} exists in all of them: switch to it?")
                } else {
                    format!("{name} exists in {} of {}", have.len(), repos(jobs.len()))
                };
                (title, lines, *choice)
            }
            Sheet::ClearReview { choice } => {
                diff_ctx!(self, ctx, dctx);
                let (title, lines) = self.diff.clear_sheet(&dctx);
                (title, lines, *choice)
            }
            Sheet::Help { choice } => {
                let lines = HELP_KEYS
                    .iter()
                    .map(|(key, what)| (key.to_string(), what.to_string(), Tone::Normal))
                    .collect();
                ("Keys in the repositories panel".to_owned(), lines, *choice)
            }
        };
        let buttons = sheet_buttons(sheet);
        Some(SheetView {
            title,
            lines,
            default: choice.min(buttons.len().saturating_sub(1)),
            buttons,
        })
    }

    /// What the panel shows. `title` is the project's name.
    pub fn view(&self, title: &str, ctx: &Ctx) -> ReposView {
        let items = ctx.items;
        let page = ctx.page.0.max(1);
        let rows = self.rows(items);
        let (first, part) = standing(&self.tree_scroll, rows.len(), page);
        // Scrolled part of a line, the tree shows one more under it.
        let end = (first + page + usize::from(part > 0.0)).min(rows.len());
        let shown = &rows[first..end];
        let result = ctx.batch.map(|batch| {
            let (text, tone) = batch.summary();
            let mut buttons = Vec::new();
            if undo_jobs(Some(batch)).is_ok() {
                buttons.push(ReposButton::Undo);
            }
            if retryable(Some(batch)) {
                buttons.push(ReposButton::RetryFailed);
            }
            ResultBar {
                text,
                tone,
                buttons: buttons
                    .into_iter()
                    .map(|button| Hint {
                        button,
                        label: lower_first(button.label()),
                        key: key_name(button).into(),
                        disabled: None,
                    })
                    .collect(),
            }
        });
        ReposView {
            title: title.into(),
            summary: self.summary(ctx),
            branch: self.branch_picker(ctx),
            lines: shown.iter().map(|&row| self.tree_line(row, ctx)).collect(),
            part,
            over: self.tree_scroll.overscroll(),
            first,
            total: rows.len(),
            selected: self
                .selected_row(&rows, items)
                .filter(|k| (first..first + shown.len()).contains(k))
                .map(|k| k - first),
            hover: self.hover,
            field: self.field.line(),
            field_focused: self.focus == Focus::Field && self.branch_menu.is_none(),
            placeholder: if self.checked_files.is_empty() {
                "Commit message (check files to commit them)".into()
            } else {
                "Commit message".into()
            },
            error: self.error.clone(),
            result,
            hints: self.hints(ctx),
            detail: self.detail_view(ctx),
            sheet: self.sheet_view(ctx),
            empty: "No git repositories in this project: add a workspace in a git checkout".into(),
            density: ctx.density,
            tree_share: ctx.tree_share,
            grouping: self.grouping,
            diff_only: false,
        }
    }

    /// The log or stashes the selection wants read, when not read yet (or
    /// read before the last Git status read).
    pub fn detail_wanted(&self, ctx: &Ctx) -> Option<DetailKey> {
        self.selection(ctx.items)?;
        let key = self.detail_key()?;
        let read = self.detail.as_ref().is_some_and(|(k, _)| *k == key);
        (!read || self.detail_stale).then_some(key)
    }

    /// A detail read came back; kept if it is still the one wanted.
    pub fn set_detail(&mut self, key: DetailKey, text: String) {
        if self.detail_key().as_ref() != Some(&key) {
            return;
        }
        if self.detail.as_ref().is_none_or(|(k, _)| *k != key) {
            self.detail_scroll.jump(0.0);
        }
        self.detail = Some((key, text));
        self.detail_stale = false;
    }

    /// Ask about what the check before `kind` found: Enter runs only the
    /// clean ones when some would conflict, else all.
    pub fn ask_preflight(
        &mut self,
        kind: BatchKind,
        jobs: Vec<Job>,
        verdicts: Vec<(PathBuf, Verdict)>,
    ) {
        let conflicts = verdicts
            .iter()
            .any(|(_, v)| matches!(v, Verdict::Conflicts(_) | Verdict::Unmerged(_)));
        self.branch_menu = None;
        self.sheet = Some(Sheet::Preflight {
            kind,
            jobs,
            verdicts,
            choice: usize::from(conflicts),
        });
    }

    /// Ask what to do with branch `name`, about to be created in `jobs`,
    /// which those in `have` have already: Enter edits the name.
    pub fn ask_exists(&mut self, name: String, jobs: Vec<Job>, have: Vec<(PathBuf, Found)>) {
        self.branch_menu = None;
        let mut sheet = Sheet::Exists {
            name,
            jobs,
            have,
            choice: 0,
        };
        let edit = sheet_buttons(&sheet).len() - 1;
        if let Sheet::Exists { choice, .. } = &mut sheet {
            *choice = edit;
        }
        self.sheet = Some(sheet);
    }

    /// Ask which way to pull the repositories at `roots`, which diverged.
    pub fn ask_diverged(&mut self, roots: Vec<PathBuf>) {
        self.branch_menu = None;
        self.sheet = Some(Sheet::Diverged { roots, choice: 0 });
    }

    /// A batch using the field's text (a commit) went through: empty it
    /// for what comes next.
    pub fn clear_field(&mut self) {
        self.field.clear();
    }
}

/// Whether `name` can be a branch's name, by `git check-ref-format
/// --branch`'s rules, without running git: else why not.
pub fn branch_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("it is empty");
    }
    if name == "@" {
        return Some("it can't be @ alone");
    }
    if name == "HEAD" {
        return Some("it can't be HEAD");
    }
    if name.contains(char::is_whitespace) {
        return Some("it can't have spaces");
    }
    if name.contains(char::is_control) {
        return Some("it can't have control characters");
    }
    if name.contains(['~', '^', ':', '?', '*', '[', '\\']) {
        return Some(r"it can't have ~ ^ : ? * [ or \");
    }
    if name.contains("..") {
        return Some("it can't have ..");
    }
    if name.contains("@{") {
        return Some("it can't have @{");
    }
    if name.starts_with('-') {
        return Some("it can't start with -");
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Some("it can't start or end with /, or have //");
    }
    if name.ends_with('.') {
        return Some("it can't end with .");
    }
    for part in name.split('/') {
        if part.starts_with('.') {
            return Some("no part of it can start with .");
        }
        if part.ends_with(".lock") {
            return Some("no part of it can end with .lock");
        }
    }
    None
}

/// The diff's calls the tests make through the panel, with its context.
#[cfg(test)]
impl Panel {
    fn caret_to(&mut self, row: usize, col: usize, ctx: &Ctx) {
        diff_ctx!(self, ctx, dctx);
        self.diff.caret_to(row, col, &dctx);
        self.follow_diff(ctx);
    }

    fn detail_row_at(&self, i: usize, ctx: &Ctx) -> Option<usize> {
        diff_ctx!(self, ctx, dctx);
        self.diff.detail_row_at(i, &dctx)
    }

    fn select_diff_row(&mut self, r: usize, ctx: &Ctx) {
        diff_ctx!(self, ctx, dctx);
        self.diff.select_diff_row(r, &dctx);
        self.follow_diff(ctx);
    }

    fn build_rows(&mut self, items: &[RepoItem]) {
        self.diff.rebuild(&sections(items));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::LazyLock,
        time::{Duration, UNIX_EPOCH},
    };

    use super::*;
    use crate::diff_view::DiffRow;

    /// The time the tests take as now, in seconds after the epoch.
    const NOW: u64 = 1_000_000;

    /// The branch names that are one branch, as by default.
    static SAME: LazyLock<Vec<Vec<String>>> =
        LazyLock::new(|| vec![vec!["main".into(), "master".into()]]);
    static TOP: LazyLock<Vec<String>> =
        LazyLock::new(|| crate::config::Git::default().top_branches);

    fn change(path: &str, code: char) -> Change {
        Change {
            path: path.into(),
            orig: None,
            code,
        }
    }

    fn root(name: &str) -> PathBuf {
        format!("/src/{name}").into()
    }

    /// A repository at /src/`name` on `branch`, tracking its namesake.
    fn repo(name: &str, branch: &str, changes: Vec<Change>) -> RepoItem {
        RepoItem {
            root: root(name),
            label: name.into(),
            workspace: 0,
            status: GitStatus {
                branch: Some(branch.into()),
                dirty: !changes.is_empty(),
                root: Some(root(name)),
                head: Some("1a2b3c4d5e6f".into()),
                upstream: Some(format!("origin/{branch}")),
                changes,
                ..GitStatus::default()
            },
        }
    }

    /// gerdr with two changes, herdr clean, zed with a rename: the lines
    /// gerdr, src/git.rs, notes.md, herdr, zed, new.rs.
    fn items() -> Vec<RepoItem> {
        let rename = Change {
            path: "new.rs".into(),
            orig: Some("old.rs".into()),
            code: 'R',
        };
        vec![
            repo(
                "gerdr",
                "main",
                vec![change("src/git.rs", 'M'), change("notes.md", '?')],
            ),
            repo("herdr", "main", vec![]),
            repo("zed", "main", vec![rename]),
        ]
    }

    /// herdr stopped merging, src/api.rs conflicted.
    fn merging(mut items: Vec<RepoItem>) -> Vec<RepoItem> {
        let herdr = &mut items[1].status;
        herdr.op = Some(InProgress::Merge);
        herdr.changes = vec![change("README.md", 'M'), change("src/api.rs", 'U')];
        herdr.dirty = true;
        items
    }

    fn ctx<'a>(items: &'a [RepoItem], batch: Option<&'a Batch>) -> Ctx<'a> {
        Ctx {
            items,
            batch,
            page: (20, 20),
            now: UNIX_EPOCH + Duration::from_secs(NOW),
            fetching: false,
            same: &SAME,
            top: &TOP,
            density: Density::Comfortable,
            tree_share: crate::chrome::REPOS_TREE_SHARE,
            cols: 40,
        }
    }

    fn mods(ctrl: bool, shift: bool, alt: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            shift,
            alt,
            ..ModifiersState::default()
        }
    }

    fn none() -> ModifiersState {
        mods(false, false, false)
    }

    fn ctrl() -> ModifiersState {
        mods(true, false, false)
    }

    fn ctrl_shift() -> ModifiersState {
        mods(true, true, false)
    }

    fn alt() -> ModifiersState {
        mods(false, false, true)
    }

    fn press(panel: &mut Panel, m: ModifiersState, sym: Keysym, ctx: &Ctx) -> Option<Outcome> {
        panel.key(&m, sym, sym, ctx)
    }

    fn type_text(panel: &mut Panel, text: &str, ctx: &Ctx) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            panel.key(&none(), sym, sym, ctx);
        }
    }

    fn select(panel: &mut Panel, name: &str) {
        panel.selected = Some(Selected::Repo(root(name)));
    }

    fn select_file(panel: &mut Panel, name: &str, path: &str) {
        panel.selected = Some(Selected::File(root(name), path.into()));
    }

    /// A diff of `path` adding `lines`.
    fn added(path: &str, code: char, lines: &[&str]) -> FileDiff {
        let mut file = FileDiff::default();
        if !lines.is_empty() {
            let mut text = String::from("diff --git a/f b/f\n@@ -0,0 +1 @@\n");
            for line in lines {
                text.push_str(&format!("+{line}\n"));
            }
            file = crate::diff::parse(&text).pop().unwrap();
        }
        file.path = path.into();
        file.code = code;
        file
    }

    /// The diffs of `items()` read: src/git.rs adding a, b and c,
    /// notes.md adding n, new.rs renamed from old.rs. The diff's rows:
    /// gerdr, src/git.rs, a, b, c, notes.md, n, a gap, zed, new.rs.
    fn read_diffs(panel: &mut Panel, ctx: &Ctx) {
        let g = panel.diff.reads;
        let gerdr = vec![
            added("src/git.rs", 'M', &["a", "b", "c"]),
            added("notes.md", '?', &["n"]),
        ];
        panel.set_diffs(root("gerdr"), g, gerdr, ctx);
        let mut renamed = added("new.rs", 'R', &[]);
        renamed.orig = Some("old.rs".into());
        panel.set_diffs(root("zed"), g, vec![renamed], ctx);
    }

    /// The detail's rows in short: `# repo`, `= file`, `+line`, `@ hunk`,
    /// `~ note`, else the text.
    fn detail_rows(view: &ReposView) -> Vec<String> {
        view.detail
            .rows
            .iter()
            .map(|row| match row {
                DetailRow::Text(line) => line.text.clone(),
                DetailRow::Repo(repo) => format!("# {}", repo.label),
                DetailRow::File(file) => format!("= {}", file.path),
                DetailRow::Hunk { context, .. } => format!("@ {context}"),
                DetailRow::Line(line) => {
                    let sign = match line.kind {
                        crate::diff::LineKind::Added => '+',
                        crate::diff::LineKind::Removed => '-',
                        crate::diff::LineKind::Context => ' ',
                    };
                    format!("{sign}{}", line.text)
                }
                DetailRow::Note(note) => format!("~ {note}"),
                DetailRow::Comment { text, .. } => format!("> {text}"),
                DetailRow::Editor { text, .. } => format!("| {text}"),
            })
            .collect()
    }

    fn job(name: &str, files: &[&str]) -> Job {
        Job {
            root: root(name),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    fn jobs(names: &[&str]) -> Vec<Job> {
        names.iter().map(|n| job(n, &[])).collect()
    }

    fn run(name: &str, state: RunState) -> RepoRun {
        RepoRun {
            job: job(name, &[]),
            root: root(name),
            before: None,
            after: None,
            state,
            log: Vec::new(),
        }
    }

    fn failed(failure: Failure) -> RunState {
        RunState::Failed {
            failure,
            why: "it went wrong".into(),
        }
    }

    fn batch(kind: BatchKind, runs: Vec<RepoRun>) -> Batch {
        Batch { id: 1, kind, runs }
    }

    /// The repository lines of `view`.
    fn repo_lines(view: &ReposView) -> Vec<&RepoLine> {
        view.lines
            .iter()
            .filter_map(|l| match l {
                TreeLine::Repo(r) => Some(r),
                TreeLine::Dir(_) | TreeLine::File(_) | TreeLine::Clean(_) => None,
            })
            .collect()
    }

    fn checks(panel: &Panel, ctx: &Ctx) -> Vec<Check> {
        let view = panel.view("p", ctx);
        repo_lines(&view).iter().map(|r| r.check).collect()
    }

    /// The tree's lines as text: a repository's label, a folder's name
    /// (with `/` after it), a file's path, indented two spaces a folder.
    fn texts(view: &ReposView) -> Vec<String> {
        view.lines
            .iter()
            .map(|l| match l {
                TreeLine::Repo(r) => r.label.clone(),
                TreeLine::Dir(d) => format!("{}{}/", "  ".repeat(d.depth), d.name),
                TreeLine::File(f) => format!("{}{}", "  ".repeat(f.depth), f.path),
                TreeLine::Clean(c) => format!("Clean · {}", c.count),
            })
            .collect()
    }

    /// Long after `now`: whatever moved the lists then is done.
    fn later(now: Instant) -> Instant {
        now + Duration::from_secs(10)
    }

    /// Frames from `now` until the lists stop, three seconds at most;
    /// the time they stopped.
    fn run_out(panel: &mut Panel, mut now: Instant, ctx: &Ctx) -> Instant {
        for _ in 0..360 {
            now += Duration::from_micros(8333);
            if !panel.scroll_tick(now, ctx) {
                break;
            }
        }
        now
    }

    /// Fingers moving the tree by `rows` of its lines, then lifted at
    /// rest: it stands as far as that goes.
    fn swipe_tree(panel: &mut Panel, rows: f64, ctx: &Ctx) {
        let px = rows * crate::chrome::repos_row(ctx.density) as f64;
        swipe(panel, px, false, ctx);
    }

    /// `swipe_tree` over the detail, by its rows.
    fn swipe_detail(panel: &mut Panel, rows: f64, ctx: &Ctx) {
        let px = rows * crate::chrome::repos_detail_line(ctx.density) as f64;
        swipe(panel, px, true, ctx);
    }

    /// Fingers moving the detail, or else the tree, by `px`, then lifted
    /// at rest.
    fn swipe(panel: &mut Panel, px: f64, over_detail: bool, ctx: &Ctx) {
        let now = Instant::now();
        panel.scroll(Motion::Finger { px, time: 0 }, now, over_detail, ctx);
        panel.scroll(Motion::Lift { time: 1000 }, now, over_detail, ctx);
        panel.scroll_tick(later(now), ctx);
    }

    /// `swipe_tree` over the branch menu, by its lines.
    fn swipe_menu(panel: &mut Panel, rows: f64, ctx: &Ctx) {
        let px = rows * crate::chrome::repos_row(ctx.density) as f64;
        let now = Instant::now();
        panel.scroll_branches(Motion::Finger { px, time: 0 }, now, ctx);
        panel.scroll_branches(Motion::Lift { time: 1000 }, now, ctx);
        panel.scroll_tick(later(now), ctx);
    }

    /// The wheel turned `n` notches over the tree, and its glide done.
    fn wheel_tree(panel: &mut Panel, n: f64, ctx: &Ctx) {
        let now = Instant::now();
        panel.scroll(Motion::Notches(n), now, false, ctx);
        panel.scroll_tick(later(now), ctx);
    }

    #[test]
    fn labels_and_what_is_checked_first_or_undone() {
        let commit = |push| BatchKind::Commit {
            msg: "m".into(),
            push,
        };
        assert_eq!(commit(true).label(), "Commit and push");
        assert_eq!(BatchKind::Pull(Pull::Rebase).label(), "Pull (rebase)");
        assert_eq!(BatchKind::Pull(Pull::FastForward).label(), "Pull");
        assert_eq!(
            BatchKind::Integrate {
                how: Integrate::Rebase,
                from: None
            }
            .label(),
            "Rebase on base"
        );
        assert_eq!(BatchKind::Push { force: true }.label(), "Force push");
        assert!(
            BatchKind::Integrate {
                how: Integrate::Merge,
                from: None
            }
            .checked_first()
        );
        assert!(BatchKind::Pull(Pull::Merge).checked_first());
        assert!(BatchKind::Branch("x".into()).checked_first());
        assert!(!BatchKind::Pull(Pull::FastForward).checked_first());
        assert!(!BatchKind::Fetch.checked_first());
        assert!(commit(false).undoable());
        assert!(!commit(true).undoable());
        assert!(
            BatchKind::Integrate {
                how: Integrate::Rebase,
                from: None
            }
            .undoable()
        );
        assert!(!BatchKind::Push { force: false }.undoable());
    }

    #[test]
    fn the_summary_counts_each_end() {
        let kind = BatchKind::Commit {
            msg: "m".into(),
            push: true,
        };
        let mut b = batch(
            kind,
            vec![
                run("a", RunState::Done),
                run("b", RunState::Done),
                run("c", failed(Failure::Conflict(vec!["x".into()]))),
                run("d", failed(Failure::Other)),
                run("e", RunState::Skipped("nothing checked".into())),
            ],
        );
        assert!(b.finished());
        assert_eq!(
            b.summary(),
            (
                "Commit and push: 2 done · 1 conflict · 1 failed: it went wrong · 1 skipped".into(),
                Tone::Error
            )
        );
        // Two failures: only how many.
        b.runs.push(run("h", failed(Failure::Locked)));
        assert_eq!(
            b.summary().0,
            "Commit and push: 2 done · 1 conflict · 2 failed · 1 skipped"
        );
        b.runs.pop();
        b.runs.remove(3);
        assert_eq!(b.summary().1, Tone::Warning);
        b.runs.truncate(2);
        assert_eq!(
            b.summary(),
            ("Commit and push: 2 done".into(), Tone::Success)
        );
        b.runs.push(run("f", RunState::Running("pushing…".into())));
        b.runs.push(run("g", RunState::Queued));
        assert!(!b.finished());
        assert_eq!(
            b.summary(),
            (
                "Commit and push: 2 done · 2 of 4 running".into(),
                Tone::Normal
            )
        );
    }

    #[test]
    fn a_repository_is_checked_whole_partly_or_not() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        // Nothing is checked to start with.
        assert_eq!(checks(&panel, &ctx), [Check::Off; 3]);
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(checks(&panel, &ctx)[0], Check::On);
        assert_eq!(panel.checked_files.len(), 2);
        // A file unchecked leaves its repository partly checked.
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(checks(&panel, &ctx)[0], Check::Partial);
        let view = panel.view("p", &ctx);
        assert!(matches!(&view.lines[1], TreeLine::File(f) if f.check == Check::Off));
        assert!(matches!(&view.lines[2], TreeLine::File(f) if f.check == Check::On));
        // Space on a partly checked repository unchecks it all.
        press(&mut panel, none(), Keysym::Up, &ctx);
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(checks(&panel, &ctx)[0], Check::Off);
        assert!(panel.checked_files.is_empty());
        // Both files checked one by one make it whole.
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::space, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(checks(&panel, &ctx)[0], Check::On);
        // A clean one checks on its own.
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(checks(&panel, &ctx)[1], Check::On);
        // Ctrl+A checks everything, then nothing.
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        assert_eq!(checks(&panel, &ctx), [Check::On; 3]);
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        assert_eq!(checks(&panel, &ctx), [Check::Off; 3]);
        assert!(panel.checked_files.is_empty() && panel.checked_repos.is_empty());
    }

    #[test]
    fn actions_run_on_the_checked_else_the_selected() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select(&mut panel, "herdr");
        let push = |jobs| {
            Some(Outcome::Batch {
                kind: BatchKind::Push { force: false },
                jobs,
            })
        };
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::p, &ctx),
            push(jobs(&["herdr"]))
        );
        // A file checked picks its repository.
        select_file(&mut panel, "zed", "new.rs");
        press(&mut panel, none(), Keysym::space, &ctx);
        select(&mut panel, "gerdr");
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::p, &ctx),
            push(jobs(&["zed"]))
        );
        // Fetch with nothing checked fetches them all.
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::f, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Fetch,
                jobs: jobs(&["gerdr", "herdr", "zed"])
            })
        );
        // Shift changes the symbol, which still counts.
        assert_eq!(
            press(&mut panel, ctrl_shift(), Keysym::M, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Integrate {
                    how: Integrate::Rebase,
                    from: None
                },
                jobs: jobs(&["gerdr"])
            })
        );
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::l, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Pull(Pull::FastForward),
                jobs: jobs(&["gerdr"])
            })
        );
    }

    #[test]
    fn a_commit_takes_the_checked_files_and_both_paths_of_a_rename() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select_file(&mut panel, "gerdr", "src/git.rs");
        press(&mut panel, none(), Keysym::space, &ctx);
        // herdr, clean, has nothing to commit: left out, but pushed.
        select(&mut panel, "herdr");
        press(&mut panel, none(), Keysym::space, &ctx);
        select(&mut panel, "zed");
        press(&mut panel, none(), Keysym::space, &ctx);
        panel.field.set("Fix it");
        let commit = |push| {
            let mut jobs = vec![
                job("gerdr", &["src/git.rs"]),
                job("zed", &["new.rs", "old.rs"]),
            ];
            if push {
                jobs.push(job("herdr", &[]));
            }
            Some(Outcome::Batch {
                kind: BatchKind::Commit {
                    msg: "Fix it".into(),
                    push,
                },
                jobs,
            })
        };
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::Return, &ctx),
            commit(false)
        );
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::KP_Enter, &ctx),
            commit(false)
        );
        assert_eq!(
            press(&mut panel, ctrl_shift(), Keysym::Return, &ctx),
            commit(true)
        );
    }

    #[test]
    fn no_action_takes_the_default_prefix() {
        // Ctrl+B arms the prefix before the panel sees it, and the prefix
        // then eats the next key too.
        assert_eq!(action_key(&ctrl(), Keysym::b), None);
        assert_eq!(action_key(&ctrl_shift(), Keysym::B), None);
    }

    #[test]
    fn actions_say_why_they_are_not_done() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let fails = |panel: &mut Panel, m, sym, ctx: &Ctx, why: &str| {
            assert_eq!(press(panel, m, sym, ctx), None);
            assert_eq!(panel.error.as_deref(), Some(why));
        };
        fails(
            &mut panel,
            ctrl(),
            Keysym::Return,
            &ctx,
            "Check the files to commit",
        );
        assert_eq!(panel.focus, Focus::Tree);
        press(&mut panel, none(), Keysym::space, &ctx);
        fails(
            &mut panel,
            ctrl(),
            Keysym::Return,
            &ctx,
            "Type a commit message first",
        );
        // What is wrong is in the field: it gets the keyboard.
        assert_eq!(panel.focus, Focus::Field);
        type_text(&mut panel, "Fix", &ctx);
        // Typing takes the message away.
        assert_eq!(panel.error, None);
        // Nothing to retry, nothing to undo.
        fails(
            &mut panel,
            alt(),
            Keysym::r,
            &ctx,
            "Nothing failed to retry",
        );
        fails(&mut panel, alt(), Keysym::z, &ctx, "Nothing to undo");
    }

    #[test]
    fn a_running_batch_holds_the_next() {
        let items = items();
        let b = batch(
            BatchKind::Fetch,
            vec![
                run("gerdr", RunState::Done),
                run("herdr", RunState::Running("fetching…".into())),
            ],
        );
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        for (m, sym) in [
            (ctrl(), Keysym::p),
            (ctrl(), Keysym::f),
            (ctrl_shift(), Keysym::M),
        ] {
            assert_eq!(press(&mut panel, m, sym, &ctx), None);
            assert_eq!(panel.error.as_deref(), Some("Wait for the batch to finish"));
        }
        let view = panel.view("p", &ctx);
        let lines = repo_lines(&view);
        // herdr, with nothing to do, is under the heading, last.
        assert_eq!(lines[2].label, "herdr");
        assert_eq!(lines[2].state, LineState::Busy("fetching…".into()));
        assert_eq!(lines[0].chip.as_ref().map(|c| c.tone), Some(Tone::Success));
        let result = view.result.unwrap();
        assert_eq!(result.text, "Fetch: 1 done · 1 of 2 running");
        assert!(result.buttons.is_empty());
        assert!(
            view.hints
                .iter()
                .find(|h| h.button == ReposButton::Push)
                .is_some_and(|h| h.disabled.as_deref() == Some("Wait for the batch to finish"))
        );
    }

    #[test]
    fn branch_names_follow_gits_rules() {
        for good in ["feat/x", "fix-1", "a.b", "user/feat/x_y", "v1@2"] {
            assert_eq!(branch_name_problem(good), None, "{good}");
        }
        for bad in [
            "", "a b", "a\tb", "a\u{7f}b", "a..b", "a@{b", "-a", "/a", "a/", "a//b", "a.",
            "a.lock", "a.lock/b", "a/.b", ".a", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b",
            "@", "HEAD",
        ] {
            assert!(branch_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn force_push_abort_and_undo_are_asked_about_on_a_sheet() {
        let items = merging(items());
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        assert_eq!(press(&mut panel, ctrl_shift(), Keysym::P, &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.title, "Force push in 1 repository?");
        assert_eq!(
            sheet.lines,
            [(
                "gerdr".to_owned(),
                "overwrite origin/main with main, with lease".to_owned(),
                Tone::Warning
            )]
        );
        assert_eq!(sheet.buttons, [ReposButton::ForcePush, ReposButton::Cancel]);
        assert_eq!(sheet.default, 0);
        // The panel's keys wait; Enter does it.
        assert_eq!(press(&mut panel, ctrl(), Keysym::p, &ctx), None);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Push { force: true },
                jobs: jobs(&["gerdr"])
            })
        );
        assert!(panel.sheet.is_none());
        // Esc takes it back, so does Cancel.
        press(&mut panel, ctrl_shift(), Keysym::P, &ctx);
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.sheet.is_none());
        press(&mut panel, ctrl_shift(), Keysym::P, &ctx);
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::Cancel), &ctx),
            None
        );
        assert!(panel.sheet.is_none());
        // A click on its button does it.
        press(&mut panel, ctrl_shift(), Keysym::P, &ctx);
        assert!(matches!(
            panel.click(ReposHit::Button(ReposButton::ForcePush), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Push { force: true },
                ..
            })
        ));
        // Abort, on the repository stopped merging.
        select(&mut panel, "herdr");
        assert_eq!(press(&mut panel, ctrl_shift(), Keysym::Delete, &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.title, "Abort in 1 repository?");
        assert_eq!(
            sheet.lines[0],
            (
                "herdr".into(),
                "abort the merge: back to before it".into(),
                Tone::Warning
            )
        );
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Abort,
                jobs: jobs(&["herdr"])
            })
        );
        // Nothing to abort on a repository going well.
        select(&mut panel, "zed");
        assert_eq!(press(&mut panel, ctrl_shift(), Keysym::Delete, &ctx), None);
        assert!(panel.sheet.is_none());
        assert_eq!(
            panel.error.as_deref(),
            Some("zed isn't stopped on conflicts")
        );
        // Undo, over the batch done.
        let head = |oid: &str| Head {
            branch: Some("main".into()),
            oid: Some(oid.into()),
        };
        let mut done = run("zed", RunState::Done);
        (done.before, done.after) = (Some(head("1111111aaa")), Some(head("2222222bbb")));
        let b = batch(BatchKind::Branch("x".into()), vec![done.clone()]);
        let ctx = super::Ctx {
            batch: Some(&b),
            ..ctx
        };
        assert_eq!(press(&mut panel, alt(), Keysym::z, &ctx), None);
        let view = panel.view("p", &ctx);
        let sheet = view.sheet.expect("a sheet");
        assert_eq!(sheet.title, "Undo branch in 1 repository?");
        assert_eq!(
            sheet.lines[0],
            (
                "zed".into(),
                "back to main at 1111111".into(),
                Tone::Warning
            )
        );
        // The result bar's buttons carry their keys.
        let buttons = view.result.unwrap().buttons;
        assert_eq!(buttons.len(), 1);
        assert_eq!(
            (buttons[0].button, buttons[0].key.as_str()),
            (ReposButton::Undo, "Alt+Z")
        );
        assert_eq!(buttons[0].label, "undo");
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Undo,
                jobs: jobs(&["zed"])
            })
        );
        // A push can't be taken back.
        let pushed = batch(BatchKind::Push { force: false }, vec![done]);
        let ctx = super::Ctx {
            batch: Some(&pushed),
            ..ctx
        };
        assert_eq!(press(&mut panel, alt(), Keysym::z, &ctx), None);
        assert_eq!(panel.error.as_deref(), Some("Push can't be undone"));
        assert!(panel.sheet.is_none());
    }

    #[test]
    fn f1_lists_the_keys_on_a_sheet() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let hints = panel.view("p", &ctx).hints;
        let help = hints.last().expect("a hint");
        assert_eq!((help.button, help.key.as_str()), (ReposButton::Help, "F1"));
        assert_eq!(help.label, "keys");
        assert_eq!(press(&mut panel, none(), Keysym::F1, &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.title, "Keys in the repositories panel");
        assert_eq!(sheet.lines.len(), HELP_KEYS.len());
        assert_eq!(sheet.lines[0].0, "Tab");
        assert_eq!(sheet.buttons, [ReposButton::Close]);
        assert_eq!(help.disabled, None);
        // Enter or Esc closes it, and so does a click on its button.
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.sheet.is_none());
        panel.click(ReposHit::Button(ReposButton::Help), &ctx);
        assert!(panel.sheet.is_some());
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.sheet.is_none());
    }

    #[test]
    fn stashes_are_made_popped_and_dropped() {
        let mut items = items();
        items[0].status.stashes = 2;
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let buttons: Vec<ReposButton> = panel
            .view("p", &ctx)
            .hints
            .iter()
            .map(|h| h.button)
            .collect();
        assert!(buttons.contains(&ReposButton::Stash));
        assert!(buttons.contains(&ReposButton::StashPop));
        assert!(buttons.contains(&ReposButton::StashDrop));
        assert_eq!(
            press(&mut panel, alt(), Keysym::s, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Stash,
                jobs: jobs(&["gerdr"])
            })
        );
        assert_eq!(
            press(&mut panel, mods(false, true, true), Keysym::S, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::StashPop,
                jobs: jobs(&["gerdr"])
            })
        );
        // Dropping is asked about.
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::StashDrop), &ctx),
            None
        );
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.title, "Drop the newest stash in 1 repository?");
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::StashDrop,
                jobs: jobs(&["gerdr"])
            })
        );
        // A clean repository without stashes offers none.
        select(&mut panel, "herdr");
        let buttons: Vec<ReposButton> = panel
            .view("p", &ctx)
            .hints
            .iter()
            .map(|h| h.button)
            .collect();
        assert!(
            !buttons
                .iter()
                .any(|b| matches!(b, ReposButton::Stash | ReposButton::StashPop))
        );
        assert_eq!(press(&mut panel, alt(), Keysym::s, &ctx), None);
        assert_eq!(panel.error.as_deref(), Some("Nothing to stash in herdr"));
    }

    #[test]
    fn labels_say_how_many_they_run_on() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let label = |panel: &Panel, button| panel.button_label(button, &ctx);
        // The selected one alone, nothing checked.
        assert_eq!(label(&panel, ReposButton::Push), "push 1");
        assert_eq!(label(&panel, ReposButton::Fetch), "fetch all");
        assert_eq!(label(&panel, ReposButton::Commit), "commit");
        assert_eq!(label(&panel, ReposButton::OpenInZed), "open in Zed");
        check_repo(&mut panel, "gerdr", &ctx);
        check_repo(&mut panel, "herdr", &ctx);
        assert_eq!(label(&panel, ReposButton::Push), "push 2");
        assert_eq!(label(&panel, ReposButton::Fetch), "fetch 2");
        // Only gerdr has files checked; herdr is pushed with it.
        assert_eq!(label(&panel, ReposButton::Commit), "commit 1");
        assert_eq!(label(&panel, ReposButton::CommitPush), "commit and push 2");
    }

    #[test]
    fn enter_on_a_file_opens_it_and_shift_scrolls_the_detail() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 2);
        let mut panel = Panel::new(&items);
        select_file(&mut panel, "gerdr", "src/git.rs");
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::OpenInZed {
                root: root("gerdr"),
                paths: vec!["src/git.rs".into()],
                at: None,
            })
        );
        // The diff, twelve rows, two in view: Shift+Down scrolls it.
        read_diffs(&mut panel, &ctx);
        panel.diff.scroll.jump(0.0);
        let view = panel.view("p", &ctx);
        assert_eq!((view.detail.first, view.detail.total), (0, 12));
        assert_eq!(
            press(&mut panel, mods(false, true, false), Keysym::Down, &ctx),
            None
        );
        assert_eq!(panel.diff.scroll.pos(), 1.0);
        let page_down = |panel: &mut Panel| {
            press(panel, mods(false, true, false), Keysym::Page_Down, &ctx);
        };
        page_down(&mut panel);
        assert_eq!(panel.diff.scroll.pos(), 3.0);
        page_down(&mut panel);
        assert_eq!(panel.diff.scroll.pos(), 5.0);
        press(&mut panel, mods(false, true, false), Keysym::Up, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 4.0);
        // With the field focused too.
        panel.focus = Focus::Field;
        press(&mut panel, mods(false, true, false), Keysym::Page_Up, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 2.0);
        assert_eq!(panel.view("p", &ctx).detail.first, 2);
        // As far as the last row, at the bottom.
        swipe_detail(&mut panel, 100.0, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 10.0);
        let view = panel.view("p", &ctx);
        assert_eq!(detail_rows(&view), ["# zed", "= old.rs → new.rs"]);
        assert!(!view.detail.sticky);
    }

    #[test]
    fn sheets_take_the_keys_and_answer() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let kind = BatchKind::Integrate {
            how: Integrate::Merge,
            from: None,
        };
        let verdicts = vec![
            (root("gerdr"), Verdict::Clean),
            (root("herdr"), Verdict::Conflicts(vec!["a.rs".into()])),
            (root("zed"), Verdict::Blocked("detached HEAD".into())),
        ];
        let ask = |panel: &mut Panel| {
            let all = jobs(&["gerdr", "herdr", "zed"]);
            panel.ask_preflight(kind.clone(), all, verdicts.clone());
        };
        ask(&mut panel);
        let sheet = panel.view("p", &ctx).sheet.unwrap();
        assert_eq!(
            sheet.title,
            "Merge base: 1 clean · 1 would conflict · 1 blocked"
        );
        assert_eq!(
            sheet.lines[1],
            ("herdr".into(), "conflicts: a.rs".into(), Tone::Warning)
        );
        assert_eq!(
            sheet.lines[2],
            ("zed".into(), "blocked: detached HEAD".into(), Tone::Error)
        );
        // With conflicts found, Enter runs the clean ones.
        assert_eq!(sheet.default, 1);
        // The panel's keys wait.
        assert_eq!(press(&mut panel, ctrl(), Keysym::p, &ctx), None);
        assert!(panel.sheet.is_some());
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Confirmed {
                kind: kind.clone(),
                jobs: jobs(&["gerdr"])
            })
        );
        assert!(panel.sheet.is_none());
        // All drops only the blocked.
        ask(&mut panel);
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Confirmed {
                kind: kind.clone(),
                jobs: jobs(&["gerdr", "herdr"])
            })
        );
        // Cancel, by Tab round to it or by Esc.
        ask(&mut panel);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.sheet.is_none());
        ask(&mut panel);
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.sheet.is_none());
        // Nothing clean to run: it just closes.
        panel.ask_preflight(
            kind.clone(),
            jobs(&["zed"]),
            vec![(root("zed"), Verdict::Blocked("no".into()))],
        );
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.sheet.is_none());
        // Diverged ones: rebase, merge or skip.
        panel.ask_diverged(vec![root("zed")]);
        let sheet = panel.view("p", &ctx).sheet.unwrap();
        assert_eq!(
            sheet.buttons,
            [
                ReposButton::PullRebase,
                ReposButton::PullMerge,
                ReposButton::SkipDiverged
            ]
        );
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Confirmed {
                kind: BatchKind::Pull(Pull::Merge),
                jobs: jobs(&["zed"])
            })
        );
        panel.ask_diverged(vec![root("zed")]);
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::PullRebase), &ctx),
            Some(Outcome::Confirmed {
                kind: BatchKind::Pull(Pull::Rebase),
                jobs: jobs(&["zed"])
            })
        );
        panel.ask_diverged(vec![root("zed")]);
        let skip = ReposHit::Button(ReposButton::SkipDiverged);
        assert_eq!(panel.click(skip, &ctx), None);
        assert!(panel.sheet.is_none());
    }

    #[test]
    fn repositories_with_nothing_to_do_go_last_under_a_heading() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let tree = |panel: &Panel, ctx: &Ctx| texts(&panel.view("p", ctx));
        assert_eq!(
            tree(&panel, &ctx),
            [
                "gerdr",
                "src/git.rs",
                "notes.md",
                "zed",
                "old.rs → new.rs",
                "Clean · 1",
                "herdr"
            ]
        );
        // The heading folds and unfolds like a folder.
        panel.selected = Some(Selected::Clean);
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(tree(&panel, &ctx).last().unwrap(), "Clean · 1");
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(tree(&panel, &ctx).last().unwrap(), "herdr");
        // Committed and pushed, zed stays where it was.
        let mut done = items.clone();
        done[2].status.changes.clear();
        let ctx = super::Ctx {
            items: &done,
            ..ctx
        };
        panel.refresh(&ctx);
        assert_eq!(
            tree(&panel, &ctx),
            [
                "gerdr",
                "src/git.rs",
                "notes.md",
                "zed",
                "Clean · 1",
                "herdr"
            ]
        );
        // With nothing to do anywhere, there is no heading.
        let clean: Vec<RepoItem> = (items.iter().cloned())
            .map(|mut i| {
                i.status.changes.clear();
                i
            })
            .collect();
        let ctx = super::Ctx {
            items: &clean,
            ..ctx
        };
        assert_eq!(tree(&Panel::new(&clean), &ctx), ["gerdr", "herdr", "zed"]);
    }

    #[test]
    fn big_change_lists_start_folded() {
        let many = |n: usize| (0..n).map(|i| change(&format!("f{i}"), 'M')).collect();
        let items = vec![
            repo("big", "main", many(FOLD_FILES)),
            repo("small", "main", many(FOLD_FILES - 1)),
            repo("clean", "main", vec![]),
        ];
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let view = panel.view("p", &ctx);
        let lines = repo_lines(&view);
        assert_eq!(lines[0].expanded, Some(false));
        assert_eq!(lines[1].expanded, Some(true));
        // small's files fill the rest of the page: clean isn't in view.
        assert_eq!(view.lines.len(), 20);
        // Space still checks every file of a folded one.
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(panel.checked_files.len(), FOLD_FILES);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(repo_lines(&panel.view("p", &ctx))[0].expanded, Some(true));
        press(&mut panel, none(), Keysym::Left, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Return, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!(texts(&view), ["big", "small", "Clean · 1", "clean"]);
        assert_eq!(repo_lines(&view)[2].expanded, None);
        // Left on a file selects its repository.
        press(&mut panel, none(), Keysym::Return, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(panel.selected, Some(Selected::Repo(root("small"))));
    }

    #[test]
    fn conflicted_files_come_first() {
        let items = merging(items());
        let ctx = ctx(&items, None);
        let panel = Panel::new(&items);
        let view = panel.view("p", &ctx);
        assert_eq!(
            texts(&view),
            [
                "gerdr",
                "src/git.rs",
                "notes.md",
                "herdr",
                "src/api.rs",
                "README.md",
                "zed",
                "old.rs → new.rs"
            ]
        );
        assert!(matches!(&view.lines[4], TreeLine::File(f) if f.conflicted && f.code == 'U'));
    }

    #[test]
    fn typing_goes_to_the_field_and_esc_steps_back() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        type_text(&mut panel, "fix it", &ctx);
        assert_eq!(panel.focus, Focus::Field);
        assert_eq!(&*panel.field, "fix it");
        // Up and Down still move the selection.
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(
            panel.selected,
            Some(Selected::File(root("gerdr"), "src/git.rs".into()))
        );
        assert_eq!(panel.view("p", &ctx).selected, Some(1));
        // Home is the field's here.
        press(&mut panel, none(), Keysym::Home, &ctx);
        assert_eq!(panel.field.line().caret, 0);
        assert_eq!(panel.view("p", &ctx).selected, Some(1));
        assert!(panel.view("p", &ctx).field_focused);
        // Tab goes round: the tree, the detail, the field.
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Tree);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Detail);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Field);
        // Esc closes, from the field too.
        assert_eq!(
            press(&mut panel, none(), Keysym::Escape, &ctx),
            Some(Outcome::Close)
        );
        assert_eq!(panel.focus, Focus::Field);
        // Ctrl+A in the field selects its text instead of checking.
        panel.focus = Focus::Field;
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        assert_eq!(panel.field.line().selection(), 0..6);
        assert!(panel.checked_repos.is_empty());
    }

    #[test]
    fn a_status_read_drops_the_committed_and_keeps_the_selection() {
        let mut items = items();
        let mut panel = Panel::new(&items);
        {
            let ctx = ctx(&items, None);
            select_file(&mut panel, "gerdr", "src/git.rs");
            press(&mut panel, none(), Keysym::space, &ctx);
            select_file(&mut panel, "gerdr", "notes.md");
            press(&mut panel, none(), Keysym::Down, &ctx);
            press(&mut panel, none(), Keysym::Up, &ctx);
            select(&mut panel, "zed");
            press(&mut panel, none(), Keysym::space, &ctx);
            select_file(&mut panel, "gerdr", "notes.md");
        }
        // src/git.rs is committed, zed's workspace closed.
        items[0].status.changes.remove(0);
        items.pop();
        let ctx = ctx(&items, None);
        panel.refresh(&ctx);
        assert!(panel.checked_files.is_empty());
        assert!(panel.checked_repos.is_empty());
        assert_eq!(
            panel.selected,
            Some(Selected::File(root("gerdr"), "notes.md".into()))
        );
        assert_eq!(panel.view("p", &ctx).selected, Some(1));
        // Gone too, the line that took its place is selected.
        let mut clean = items.clone();
        clean[0].status.changes.clear();
        let ctx = super::Ctx {
            items: &clean,
            ..ctx
        };
        panel.refresh(&ctx);
        assert_eq!(panel.selected, Some(Selected::Repo(root("herdr"))));
    }

    #[test]
    fn the_header_says_where_they_all_stand() {
        let mut items = items();
        items[0].status.branch = Some("feat/x".into());
        items[1].status.branch = Some("feat/x".into());
        let at = |ago: u64| Some(UNIX_EPOCH + Duration::from_secs(NOW - ago));
        items[1].status.fetched = at(3 * 3600);
        items[2].status.fetched = at(125);
        let ctx = ctx(&items, None);
        let panel = Panel::new(&items);
        let view = panel.view("gerdr", &ctx);
        assert_eq!(view.summary, "2 dirty · fetched 2m ago");
        assert_eq!(
            view.placeholder,
            "Commit message (check files to commit them)"
        );
        let mut checked = Panel::new(&items);
        select_file(&mut checked, "gerdr", "src/git.rs");
        press(&mut checked, none(), Keysym::space, &ctx);
        assert_eq!(checked.view("gerdr", &ctx).placeholder, "Commit message");
        let items = merging(vec![items[0].clone(), items[1].clone()]);
        let mut items = items;
        items[1].status.fetched = None;
        let ctx = super::Ctx {
            items: &items,
            ..ctx
        };
        let view = panel.view("gerdr", &ctx);
        assert_eq!(view.summary, "2 dirty · 1 conflict · never fetched");
        // While a fetch runs, how long ago one did says nothing.
        let fetching = super::Ctx {
            fetching: true,
            ..ctx
        };
        assert_eq!(
            panel.view("gerdr", &fetching).summary,
            "2 dirty · 1 conflict · fetching…"
        );
        let b = batch(BatchKind::Fetch, vec![run("a", RunState::Queued)]);
        let ctx = super::Ctx {
            batch: Some(&b),
            ..ctx
        };
        assert_eq!(
            panel.view("gerdr", &ctx).summary,
            "2 dirty · 1 conflict · fetching…"
        );
        let done = batch(BatchKind::Fetch, vec![run("a", RunState::Done)]);
        let ctx = super::Ctx {
            batch: Some(&done),
            ..ctx
        };
        assert_eq!(
            panel.view("gerdr", &ctx).summary,
            "2 dirty · 1 conflict · never fetched"
        );
        assert_eq!(ago(UNIX_EPOCH, UNIX_EPOCH + Duration::from_secs(5)), "5s");
        assert_eq!(
            ago(UNIX_EPOCH, UNIX_EPOCH + Duration::from_secs(2 * 86400)),
            "2d"
        );
        // The project without a repository.
        let ctx = super::Ctx { items: &[], ..ctx };
        let view = Panel::new(&[]).view("gerdr", &ctx);
        assert!(view.lines.is_empty() && view.summary.is_empty());
        assert_eq!(
            view.empty,
            "No git repositories in this project: add a workspace in a git checkout"
        );
    }

    #[test]
    fn chips_say_what_each_is_in_the_middle_of() {
        let mut items = merging(items());
        items[1].status.changes.push(change("b.rs", 'U'));
        items.push(repo("rebasing", "feat", vec![]));
        items[3].status.op = Some(InProgress::Rebase { step: 2, total: 5 });
        for name in ["auth", "skipped", "done", "conflict", "detached"] {
            items.push(repo(name, "main", vec![]));
        }
        items[8].status.branch = None;
        let b = batch(
            BatchKind::Pull(Pull::Merge),
            vec![
                run("auth", failed(Failure::Auth)),
                run("skipped", RunState::Skipped("nothing checked".into())),
                run("done", RunState::Done),
                run("conflict", failed(Failure::Conflict(vec!["a.rs".into()]))),
            ],
        );
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        // Folded, for every repository's line on the page.
        for item in &items {
            panel.set_open(item, false);
        }
        let view = panel.view("p", &ctx);
        let chips: Vec<Option<(String, Tone)>> = repo_lines(&view)
            .iter()
            .map(|l| l.chip.as_ref().map(|c| (c.text.clone(), c.tone)))
            .collect();
        let chip = |text: &str, tone| Some((text.to_owned(), tone));
        assert_eq!(
            chips,
            [
                None,
                chip("MERGING · 2", Tone::Error),
                None,
                chip("REBASING 2/5", Tone::Error),
                // Not on a branch, it has something to do; the rest
                // haven't, and go last.
                chip("detached", Tone::Warning),
                chip("login needed", Tone::Error),
                chip("nothing checked", Tone::Muted),
                chip("pulled", Tone::Success),
                chip("conflict", Tone::Error),
            ]
        );
        let lines = repo_lines(&view);
        assert_eq!(lines[1].state, LineState::Attention);
        assert_eq!(lines[7].state, LineState::Settled);
        assert_eq!(lines[4].branch, None);
        // Login needed: run it in a terminal.
        select(&mut panel, "auth");
        let hints = panel.view("p", &ctx).hints;
        assert_eq!(hints[0].button, ReposButton::RunInTerminal);
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::RunInTerminal), &ctx),
            Some(Outcome::RunInTerminal(root("auth")))
        );
    }

    #[test]
    fn the_tree_pages_and_keeps_the_selection_in_view() {
        let items = items();
        let ctx = super::Ctx {
            page: (3, 2),
            ..ctx(&items, None)
        };
        let mut panel = Panel::new(&items);
        for _ in 0..4 {
            press(&mut panel, none(), Keysym::Down, &ctx);
        }
        let view = panel.view("p", &ctx);
        assert_eq!(texts(&view), ["notes.md", "zed", "old.rs → new.rs"]);
        assert_eq!(view.selected, Some(2));
        press(&mut panel, none(), Keysym::End, &ctx);
        assert_eq!(panel.view("p", &ctx).selected, Some(2));
        assert_eq!(panel.tree_scroll.pos(), 4.0);
        press(&mut panel, none(), Keysym::Page_Up, &ctx);
        assert_eq!(panel.selected_at, 3);
        press(&mut panel, none(), Keysym::Home, &ctx);
        assert_eq!((panel.selected_at, panel.tree_scroll.pos()), (0, 0.0));
        // A notch of the wheel glides a line, as far as there are lines.
        wheel_tree(&mut panel, 1.0, &ctx);
        assert_eq!(panel.tree_scroll.pos(), 1.0);
        wheel_tree(&mut panel, 10.0, &ctx);
        assert_eq!(panel.tree_scroll.pos(), 4.0);
        wheel_tree(&mut panel, -10.0, &ctx);
        assert_eq!(panel.tree_scroll.pos(), 0.0);
        // Out of view, the selection isn't shown.
        wheel_tree(&mut panel, 3.0, &ctx);
        assert_eq!(panel.view("p", &ctx).selected, None);
    }

    #[test]
    fn a_touchpad_scrolls_the_tree_by_parts_of_a_line() {
        let items = items();
        let ctx = super::Ctx {
            page: (3, 2),
            ..ctx(&items, None)
        };
        let mut panel = Panel::new(&items);
        let all = texts(&panel.view(
            "p",
            &super::Ctx {
                page: (20, 2),
                ..ctx
            },
        ));
        swipe_tree(&mut panel, 1.5, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.first, view.part), (1, 0.5));
        // The line shown in part under the tree too, and a click on it
        // scrolls it into view.
        assert_eq!(texts(&view), all[1..5]);
        panel.click(ReposHit::Line(3), &ctx);
        assert_eq!(panel.selected_at, 4);
        let view = panel.view("p", &ctx);
        assert_eq!((view.first, view.part), (2, 0.0));
        // The ends hold the tree whole once let go.
        swipe_tree(&mut panel, 10.5, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.first, view.part, view.lines.len()), (4, 0.0, 3));
        assert_eq!(view.over, 0.0);
        swipe_tree(&mut panel, -0.25, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.first, view.part), (3, 0.75));
        swipe_tree(&mut panel, -10.0, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.first, view.part), (0, 0.0));
        // A key moving the tree drops the part.
        swipe_tree(&mut panel, 0.5, &ctx);
        assert_eq!(panel.view("p", &ctx).part, 0.5);
        press(&mut panel, none(), Keysym::End, &ctx);
        assert_eq!(panel.view("p", &ctx).part, 0.0);
    }

    #[test]
    fn the_tree_stretches_past_an_end_under_the_fingers_and_flings() {
        let items = items();
        let ctx = super::Ctx {
            page: (3, 2),
            ..ctx(&items, None)
        };
        let row = crate::chrome::repos_row(ctx.density) as f64;
        let mut panel = Panel::new(&items);
        let now = Instant::now();
        // Pulled down at the top: the lines move down, less than the pull.
        panel.scroll(Motion::Finger { px: -row, time: 0 }, now, false, &ctx);
        let view = panel.view("p", &ctx);
        assert!(view.over > 0.0 && view.over < row, "{}", view.over);
        assert!(panel.scrolling(now));
        // Let go, it springs back.
        panel.scroll(Motion::Lift { time: 1000 }, now, false, &ctx);
        assert!(panel.scroll_tick(now + Duration::from_millis(16), &ctx));
        assert!(!panel.scroll_tick(later(now), &ctx));
        assert_eq!(panel.view("p", &ctx).over, 0.0);
        // Swiped fast and let go, it goes on and slows to a stop at the
        // end.
        for k in 0..5 {
            let at = now + Duration::from_millis(8 * k as u64);
            let time = 2000 + 8 * k;
            panel.scroll(Motion::Finger { px: 8.0, time }, at, false, &ctx);
        }
        let lift = now + Duration::from_millis(40);
        panel.scroll(Motion::Lift { time: 2040 }, lift, false, &ctx);
        let before = panel.tree_scroll.pos();
        panel.scroll_tick(lift + Duration::from_millis(50), &ctx);
        assert!(panel.tree_scroll.pos() > before);
        let end = run_out(&mut panel, lift, &ctx);
        assert!(!panel.scrolling(end));
        assert_eq!(panel.tree_scroll.pos(), 4.0);
    }

    #[test]
    fn clicks_do_what_the_keys_do() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        panel.focus = Focus::Field;
        assert_eq!(panel.click(ReposHit::Line(6), &ctx), None);
        assert_eq!(panel.selected, Some(Selected::Repo(root("herdr"))));
        assert_eq!(panel.focus, Focus::Tree);
        panel.click(ReposHit::Check(1), &ctx);
        assert_eq!(checks(&panel, &ctx)[0], Check::Partial);
        panel.click(ReposHit::Expander(0), &ctx);
        assert_eq!(
            texts(&panel.view("p", &ctx)),
            ["gerdr", "zed", "old.rs → new.rs", "Clean · 1", "herdr"]
        );
        panel.click(ReposHit::Field, &ctx);
        assert_eq!(panel.focus, Focus::Field);
        panel.click(ReposHit::DetailTab(DetailTab::Log), &ctx);
        assert_eq!(panel.tab, Some(DetailTab::Log));
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::Push), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Push { force: false },
                jobs: jobs(&["gerdr"])
            })
        );
        // Twice for a force push, by click too.
        let force = ReposHit::Button(ReposButton::ForcePush);
        assert_eq!(panel.click(force, &ctx), None);
        assert!(panel.click(force, &ctx).is_some());
        // Checked out elsewhere: open that workspace.
        let b = batch(
            BatchKind::Branch("x".into()),
            vec![run("zed", failed(Failure::CheckedOut("/src/zed2".into())))],
        );
        let ctx = super::Ctx {
            batch: Some(&b),
            ..ctx
        };
        select(&mut panel, "zed");
        let chip = repo_lines(&panel.view("p", &ctx))[1].chip.clone();
        assert_eq!(chip.unwrap().text, "checked out elsewhere");
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::OpenWorkspace), &ctx),
            Some(Outcome::OpenWorkspace("/src/zed2".into()))
        );
    }

    #[test]
    fn a_clicked_chip_goes_until_the_next_batch() {
        let items = merging(items());
        let runs = ["gerdr", "herdr", "zed"].map(|n| run(n, RunState::Done));
        let b = batch(BatchKind::Push { force: false }, runs.to_vec());
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        for item in &items {
            panel.set_open(item, false);
        }
        let chips = |panel: &Panel, ctx: &Ctx| -> Vec<Option<String>> {
            repo_lines(&panel.view("p", ctx))
                .iter()
                .map(|l| l.chip.as_ref().map(|c| c.text.clone()))
                .collect()
        };
        let line = |panel: &Panel, ctx: &Ctx, name: &str| {
            let view = panel.view("p", ctx);
            view.lines
                .iter()
                .position(|l| matches!(l, TreeLine::Repo(r) if r.label == name))
                .unwrap()
        };
        let some = |s: &str| Some(s.to_owned());
        assert_eq!(
            chips(&panel, &ctx),
            [some("pushed"), some("MERGING · 1"), some("pushed")]
        );
        let zed = line(&panel, &ctx, "zed");
        panel.click(ReposHit::Chip(zed), &ctx);
        assert_eq!(panel.selected, Some(Selected::Repo(root("zed"))));
        // A stopped merge is no batch's result: it stays.
        let herdr = line(&panel, &ctx, "herdr");
        panel.click(ReposHit::Chip(herdr), &ctx);
        assert_eq!(
            chips(&panel, &ctx),
            [some("pushed"), some("MERGING · 1"), None]
        );
        // The next batch's chips show.
        let next = Batch {
            id: 2,
            kind: BatchKind::Fetch,
            runs: runs.to_vec(),
        };
        let ctx = super::Ctx {
            batch: Some(&next),
            ..ctx
        };
        assert_eq!(
            chips(&panel, &ctx),
            [some("fetched"), some("MERGING · 1"), some("fetched")]
        );
    }

    #[test]
    fn a_lines_marks_push_pull_and_list_stashes() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        // gerdr is checked, but the arrows act on their own line alone.
        panel.click(ReposHit::Check(0), &ctx);
        assert_eq!(
            panel.click(ReposHit::Mark(6, RepoMark::Ahead), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Push { force: false },
                jobs: jobs(&["herdr"])
            })
        );
        assert_eq!(panel.selected, Some(Selected::Repo(root("herdr"))));
        assert_eq!(
            panel.click(ReposHit::Mark(3, RepoMark::Behind), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Pull(Pull::FastForward),
                jobs: jobs(&["zed"])
            })
        );
        assert_eq!(panel.click(ReposHit::Mark(3, RepoMark::Dirty), &ctx), None);
        assert_eq!(
            panel.click(ReposHit::Mark(6, RepoMark::Stashes), &ctx),
            None
        );
        assert_eq!(panel.tab, Some(DetailTab::Stashes));
        let key = panel.detail_wanted(&ctx).unwrap();
        assert_eq!((key.root.clone(), key.file.clone()), (root("herdr"), None));
        panel.set_detail(key, "stash@{0} · 1 day ago · On main: wip\n".into());
        let detail = panel.view("p", &ctx).detail;
        assert_eq!(detail.tab, DetailTab::Stashes);
        assert_eq!(
            detail.rows[0],
            DetailRow::Text(detail_line(
                "stash@{0} · 1 day ago · On main: wip",
                DiffTone::Plain
            ))
        );
        // Not while a batch runs.
        let b = batch(
            BatchKind::Fetch,
            vec![run("gerdr", RunState::Running("fetching…".into()))],
        );
        let ctx = super::Ctx {
            batch: Some(&b),
            ..ctx
        };
        assert_eq!(panel.click(ReposHit::Mark(6, RepoMark::Ahead), &ctx), None);
        assert_eq!(panel.error.as_deref(), Some("Wait for the batch to finish"));
    }

    #[test]
    fn the_detail_shows_every_diff_and_the_log() {
        let items = items();
        let mut ran = Ran {
            command: "git push -u origin HEAD".into(),
            output: "! [rejected] main -> main (fetch first)\n".into(),
            ok: false,
        };
        let mut pushed = run("gerdr", failed(Failure::Rejected));
        pushed.log.push(ran.clone());
        ran.ok = true;
        ran.command = "git fetch".into();
        ran.output = "From origin\n".into();
        pushed.log.insert(0, ran);
        let b = batch(BatchKind::Push { force: false }, vec![pushed]);
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        // Every repository with changes, read one by one.
        let wanted: Vec<(PathBuf, Vec<String>, u64)> = panel
            .diffs_wanted(&ctx)
            .into_iter()
            .map(|(root, changes, g)| (root, changes.into_iter().map(|c| c.path).collect(), g))
            .collect();
        assert_eq!(
            wanted,
            [
                (
                    root("gerdr"),
                    vec!["src/git.rs".into(), "notes.md".into()],
                    0
                ),
                (root("zed"), vec!["new.rs".into()], 0),
            ]
        );
        assert_eq!(panel.detail_wanted(&ctx), None);
        let view = panel.view("p", &ctx);
        assert_eq!(view.detail.tab, DetailTab::Diff);
        assert_eq!(
            view.detail.title,
            "All changes · 2 repositories · 3 files · +0 −0"
        );
        assert_eq!(
            detail_rows(&view),
            ["# gerdr", "~ Reading…", "", "# zed", "~ Reading…"]
        );
        // A clean repository's is dropped.
        panel.set_diffs(root("herdr"), 0, vec![added("x", 'M', &["x"])], &ctx);
        assert!(!panel.diff.diffs.contains_key(&root("herdr")));
        read_diffs(&mut panel, &ctx);
        assert!(panel.diffs_wanted(&ctx).is_empty());
        let view = panel.view("p", &ctx);
        assert_eq!(
            view.detail.title,
            "All changes · 2 repositories · 3 files · +4 −0"
        );
        assert_eq!(
            detail_rows(&view),
            [
                "# gerdr",
                "= src/git.rs",
                "@ ",
                "+a",
                "+b",
                "+c",
                "= notes.md",
                "@ ",
                "+n",
                "",
                "# zed",
                "= old.rs → new.rs",
            ]
        );
        // Scrolled into a file, its header is pinned at the top over the
        // first row, with its repository's name.
        let paged = super::Ctx {
            page: (20, 4),
            ..ctx
        };
        panel.diff.scroll.jump(4.0);
        let view = panel.view("p", &paged);
        assert!(view.detail.sticky && view.detail.stuck);
        assert_eq!(
            detail_rows(&view),
            ["= src/git.rs", "+b", "+c", "= notes.md", "@ "]
        );
        // The next file's header pushes it up as it comes.
        let line = crate::chrome::repos_detail_line(paged.density) as f64;
        assert_eq!(view.detail.pin, 0.0);
        panel.diff.scroll.jump(5.25);
        let view = panel.view("p", &paged);
        assert_eq!(view.detail.pin, -0.25 * line);
        assert_eq!(detail_rows(&view)[..2], ["= src/git.rs", "+c"]);
        panel.diff.scroll.jump(4.0);
        let DetailRow::File(file) = &view.detail.rows[0] else {
            panic!("{:?}", view.detail.rows[0]);
        };
        assert_eq!(file.repo.as_deref(), Some("gerdr"));
        assert_eq!((file.added, file.code), (3, 'M'));
        // A status read has them read again, shown meanwhile, scrolled
        // where they were.
        panel.refresh(&ctx);
        assert_eq!(panel.diffs_wanted(&ctx).len(), 2);
        assert_eq!(panel.view("p", &ctx).detail.total, 12);
        panel.set_diffs(
            root("gerdr"),
            1,
            vec![added("src/git.rs", 'M', &["a", "c"])],
            &ctx,
        );
        assert_eq!(panel.diff.scroll.pos(), 4.0);
        assert_eq!(detail_rows(&panel.view("p", &paged))[1], "+c");
        // A read for an earlier status read, coming late, is dropped.
        panel.set_diffs(root("gerdr"), 0, vec![], &ctx);
        assert_eq!(panel.diff.diffs[&root("gerdr")].1.len(), 1);
        // Selecting a line in the tree scrolls the diff to it.
        read_diffs(&mut panel, &ctx);
        panel.focus = Focus::Tree;
        select(&mut panel, "gerdr");
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 1.0);
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 6.0);
        let view = panel.view("p", &ctx);
        let DetailRow::File(file) = &view.detail.rows[6] else {
            panic!("{:?}", view.detail.rows[6]);
        };
        assert!(file.selected);
        // The repository's log, with what the batch ran there.
        select(&mut panel, "gerdr");
        press(&mut panel, ctrl(), Keysym::t, &ctx);
        let log = DetailKey {
            root: root("gerdr"),
            file: None,
            tab: DetailTab::Log,
        };
        assert_eq!(panel.detail_wanted(&ctx), Some(log.clone()));
        assert!(panel.diffs_wanted(&ctx).is_empty());
        panel.set_detail(log.clone(), "abc1234 Read the branch".into());
        assert_eq!(panel.detail_scroll.pos(), 0.0);
        let detail = panel.view("p", &ctx).detail;
        assert_eq!(detail.title, "gerdr");
        assert_eq!(detail.tab, DetailTab::Log);
        use DiffTone::*;
        let lines: Vec<(&str, DiffTone)> = detail
            .rows
            .iter()
            .map(|row| match row {
                DetailRow::Text(l) => (l.text.as_str(), l.tone),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            lines,
            [
                ("abc1234 Read the branch", Plain),
                ("", Plain),
                ("$ git fetch", Header),
                ("From origin", Plain),
                ("$ git push -u origin HEAD", Header),
                ("! [rejected] main -> main (fetch first)", Error),
            ]
        );
        // A file's line, its repository's log.
        select_file(&mut panel, "gerdr", "src/git.rs");
        assert_eq!(panel.detail_wanted(&ctx), None);
        // Ctrl+D, the diff again, where the selection is.
        press(&mut panel, ctrl(), Keysym::d, &ctx);
        assert_eq!(panel.detail_wanted(&ctx), None);
        assert_eq!(panel.diff.scroll.pos(), 1.0);
    }

    #[test]
    fn a_touchpad_scrolls_the_detail_by_parts_of_a_row() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 4);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        // Part of the repository's header scrolled past, one more row
        // under the pane.
        swipe_detail(&mut panel, 0.5, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.detail.first, view.detail.part), (0, 0.5));
        assert_eq!(
            detail_rows(&view),
            ["# gerdr", "= src/git.rs", "@ ", "+a", "+b"]
        );
        // A file's header scrolled past its place stays at the top, pinned
        // over itself and the rows moving under it.
        swipe_detail(&mut panel, 1.0, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.detail.first, view.detail.part), (1, 0.5));
        assert!(view.detail.sticky && view.detail.stuck);
        assert_eq!(view.detail.pin, 0.0);
        assert_eq!(
            detail_rows(&view),
            ["= src/git.rs", "= src/git.rs", "@ ", "+a", "+b", "+c"]
        );
        // Under the pinned header, the rows move by the part.
        swipe_detail(&mut panel, 0.75, &ctx);
        let view = panel.view("p", &ctx);
        assert!(view.detail.sticky);
        assert_eq!((view.detail.first, view.detail.part), (2, 0.25));
        assert_eq!(
            detail_rows(&view),
            ["= src/git.rs", "@ ", "+a", "+b", "+c", "= notes.md"]
        );
        // A click on the row shown in part under the pane selects it.
        panel.click(ReposHit::DiffRow(5), &ctx);
        let notes = Selected::File(root("gerdr"), "notes.md".into());
        assert_eq!(panel.selected, Some(notes));
        // The end holds the diff whole, its last row at the bottom.
        swipe_detail(&mut panel, 10.5, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.detail.first, view.detail.part), (8, 0.0));
        assert_eq!(
            detail_rows(&view),
            ["= notes.md", "+n", "", "# zed", "= old.rs → new.rs"]
        );
        // A key moving the cursor drops the part, the row it goes on
        // shown whole.
        swipe_detail(&mut panel, -2.5, &ctx);
        assert_eq!(panel.view("p", &ctx).detail.part, 0.5);
        panel.focus = Focus::Detail;
        press(&mut panel, none(), Keysym::Up, &ctx);
        assert_eq!(panel.view("p", &ctx).detail.part, 0.0);
        // A log too, from its top.
        select(&mut panel, "gerdr");
        press(&mut panel, ctrl(), Keysym::t, &ctx);
        let log = DetailKey {
            root: root("gerdr"),
            file: None,
            tab: DetailTab::Log,
        };
        panel.set_detail(log, "a\nb\nc\nd\ne\nf".into());
        swipe_detail(&mut panel, 0.5, &ctx);
        let detail = panel.view("p", &ctx).detail;
        assert_eq!((detail.first, detail.part, detail.rows.len()), (0, 0.5, 5));
        swipe_detail(&mut panel, 5.0, &ctx);
        let detail = panel.view("p", &ctx).detail;
        assert_eq!((detail.first, detail.part, detail.rows.len()), (2, 0.0, 4));
    }

    #[test]
    fn the_diff_read_again_while_it_moves_moves_on() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 4);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        let now = Instant::now();
        let line = crate::chrome::repos_detail_line(ctx.density) as f64;
        panel.scroll(Motion::Notches(3.0), now, true, &ctx);
        let soon = now + Duration::from_millis(40);
        panel.scroll_tick(soon, &ctx);
        let pos = panel.diff.scroll.pos();
        assert!(pos > 1.0 && pos < 2.0, "{pos}");
        // A status read, then the diffs read again.
        panel.refresh(&ctx);
        read_diffs(&mut panel, &ctx);
        assert_eq!(panel.diff.scroll.pos(), pos);
        assert!(panel.scrolling(soon));
        run_out(&mut panel, soon, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 3.0);
        // Under the fingers, it stretches past the end and comes back.
        panel.scroll(
            Motion::Finger {
                px: 20.0 * line,
                time: 0,
            },
            now,
            true,
            &ctx,
        );
        assert!(panel.view("p", &ctx).detail.over < 0.0);
        panel.scroll(Motion::Lift { time: 1000 }, now, true, &ctx);
        run_out(&mut panel, now, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.detail.first, view.detail.over), (8, 0.0));
    }

    /// The cursor's row, and where its range reaches back to.
    fn cursor(panel: &Panel) -> Option<(usize, usize)> {
        panel.diff.cursor.as_ref().map(|c| (c.row, c.from))
    }

    #[test]
    fn comments_are_written_on_the_cursors_lines_and_copied() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 30);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        // C needs a line.
        press(&mut panel, none(), Keysym::c, &ctx);
        assert!(panel.diff.editing.is_none() && panel.error.is_some());
        // On a and b, Shift+Down reaching the range.
        for _ in 0..4 {
            press(&mut panel, none(), Keysym::Down, &ctx);
        }
        press(&mut panel, mods(false, true, false), Keysym::Down, &ctx);
        assert_eq!(cursor(&panel), Some((4, 3)));
        // Shift+Down in the diff doesn't scroll it.
        assert_eq!(panel.diff.scroll.pos(), 0.0);
        let marks = panel.view("p", &ctx).detail.marks;
        assert!(marks[3].selected && marks[3].top && !marks[3].bottom);
        assert!(marks[4].selected && marks[4].bottom && !marks[5].selected);
        // The buttons writing a comment on them and putting them back, at
        // the cursor; each hunk's header puts it back.
        let buttons = panel.view("p", &ctx).detail.row_buttons;
        assert_eq!(
            buttons
                .iter()
                .map(|(i, h)| (*i, h.button))
                .collect::<Vec<_>>(),
            [
                (2, ReposButton::Restore),
                (7, ReposButton::Restore),
                (4, ReposButton::Comment),
                (4, ReposButton::Restore)
            ]
        );
        press(&mut panel, none(), Keysym::c, &ctx);
        type_text(&mut panel, "why", &ctx);
        press(&mut panel, mods(false, true, false), Keysym::Return, &ctx);
        type_text(&mut panel, "and b", &ctx);
        let rows = detail_rows(&panel.view("p", &ctx));
        assert_eq!(
            rows[5..9],
            ["| why", "| and b", "~ Shift+Enter breaks the line", "+c"]
        );
        // The editor has the keys: Tab and Up don't leave it.
        press(&mut panel, none(), Keysym::Up, &ctx);
        assert_eq!(panel.focus, Focus::Detail);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.diff.editing.is_none() && panel.diff.review_dirty);
        let comment = &panel.diff.review.comments[0];
        assert_eq!(comment.text, "why\nand b");
        assert_eq!(
            comment
                .lines
                .iter()
                .map(|q| (q.number, q.text.as_str()))
                .collect::<Vec<_>>(),
            [(1, "a"), (2, "b")]
        );
        let view = panel.view("p", &ctx);
        assert_eq!(
            detail_rows(&view)[3..8],
            ["+a", "+b", "> why", "> and b", "+c"]
        );
        assert!(view.detail.marks[3].noted && view.detail.marks[4].noted);
        assert!(!view.detail.marks[7].noted);
        // Copied for an agent, with Ctrl+Shift+C.
        let Some(Outcome::Copy { text, count }) = press(&mut panel, ctrl_shift(), Keysym::C, &ctx)
        else {
            panic!("not copied");
        };
        assert_eq!(count, 1);
        assert!(text.contains("### `src/git.rs:1-2`\n\n```diff\n+a\n+b\n```\n\nwhy\nand b\n"));
        assert!(
            panel
                .view("p", &ctx)
                .hints
                .iter()
                .any(|h| h.button == ReposButton::CopyReview)
        );

        // A line added above, it stays under b.
        let g = panel.diff.reads + 1;
        panel.diff.reads = g;
        let moved = vec![added("src/git.rs", 'M', &["x", "a", "b", "c"])];
        panel.set_diffs(root("gerdr"), g, moved, &ctx);
        let rows = detail_rows(&panel.view("p", &ctx));
        assert_eq!(rows[3..9], ["+x", "+a", "+b", "> why", "> and b", "+c"]);

        // C on it edits it; Esc drops the edit, not the panel.
        panel.diff.set_cursor(6, None);
        press(&mut panel, none(), Keysym::c, &ctx);
        assert_eq!(panel.diff.editing.as_ref().map(|e| e.at), Some(Some(0)));
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.diff.editing.is_none());
        assert_eq!(panel.diff.review.comments[0].text, "why\nand b");

        // b gone, it is listed with those whose lines are gone.
        let g = g + 1;
        panel.diff.reads = g;
        let gone = vec![added("src/git.rs", 'M', &["a", "c"])];
        panel.set_diffs(root("gerdr"), g, gone, &ctx);
        let rows = detail_rows(&panel.view("p", &ctx));
        let n = rows.len();
        assert_eq!(
            rows[n - 4..],
            [
                "@ Comments on lines no longer in the diff",
                "~ gerdr › src/git.rs:1-2",
                "> why",
                "> and b"
            ]
        );
        let Some(Outcome::Copy { text, .. }) = panel.press(ReposButton::CopyReview, &ctx) else {
            panic!("not copied");
        };
        assert!(text.contains("`src/git.rs:1-2` (the code changed since)"));

        // Delete on it deletes it.
        panel.diff.review_dirty = false;
        panel.diff.set_cursor(n - 2, None);
        press(&mut panel, none(), Keysym::Delete, &ctx);
        assert!(panel.diff.review.is_empty() && panel.diff.review_dirty);
        assert!(
            !detail_rows(&panel.view("p", &ctx))
                .iter()
                .any(|r| r.starts_with('>'))
        );
        assert_eq!(press(&mut panel, ctrl_shift(), Keysym::C, &ctx), None);
        assert_eq!(panel.error.as_deref(), Some(NO_REVIEW));
    }

    #[test]
    fn comments_are_written_and_copied_with_the_mouse_alone() {
        use ReposButton as B;
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 30);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        let buttons = |panel: &Panel| -> Vec<(usize, B)> {
            let view = panel.view("p", &ctx);
            view.detail
                .row_buttons
                .iter()
                .map(|(i, h)| (*i, h.button))
                .collect()
        };
        // A press on a and a drag to b pick them.
        panel.click(ReposHit::DiffRow(3), &ctx);
        assert!(panel.drags_rows(&ctx));
        panel.extend_detail_row(4, &ctx);
        assert_eq!(cursor(&panel), Some((4, 3)));
        let hunks = [(2, B::Restore), (7, B::Restore)];
        assert_eq!(
            buttons(&panel),
            [hunks[0], hunks[1], (4, B::Comment), (4, B::Restore)]
        );
        assert!(panel.view("p", &ctx).detail.title_buttons.is_empty());
        // Its button writes on them; Discard drops it, Save keeps it.
        assert_eq!(panel.click(ReposHit::Button(B::Comment), &ctx), None);
        assert!(panel.diff.editing.is_some() && !panel.drags_rows(&ctx));
        assert_eq!(
            buttons(&panel),
            [(6, B::SaveComment), (6, B::DiscardComment)]
        );
        type_text(&mut panel, "why", &ctx);
        panel.click(ReposHit::Button(B::DiscardComment), &ctx);
        assert!(panel.diff.editing.is_none() && panel.diff.review.is_empty());
        panel.click(ReposHit::Button(B::Comment), &ctx);
        type_text(&mut panel, "why", &ctx);
        panel.click(ReposHit::Button(B::SaveComment), &ctx);
        assert!(panel.diff.editing.is_none());
        assert_eq!(panel.diff.review.comments[0].text, "why");
        // The title row copies it, or clears it after asking.
        let title = panel.view("p", &ctx).detail.title_buttons;
        let title: Vec<(B, &str)> = title.iter().map(|h| (h.button, h.label.as_str())).collect();
        assert_eq!(
            title,
            [
                (B::CopyReview, "Copy 1 comment"),
                (B::ClearReview, "Clear review")
            ]
        );
        let copied = panel.click(ReposHit::Button(B::CopyReview), &ctx);
        assert!(matches!(copied, Some(Outcome::Copy { count: 1, .. })));
        assert_eq!(panel.click(ReposHit::Button(B::ClearReview), &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.unwrap();
        assert_eq!(sheet.title, "Delete the review's 1 comment for good?");
        panel.click(ReposHit::Button(B::Cancel), &ctx);
        assert!(panel.sheet.is_none() && !panel.diff.review.is_empty());
        // Edited, it can be deleted.
        panel.click(ReposHit::DiffRow(5), &ctx);
        assert_eq!(panel.diff.editing.as_ref().map(|e| e.at), Some(Some(0)));
        assert!(buttons(&panel).contains(&(6, B::DeleteComment)));
        panel.click(ReposHit::Button(B::DeleteComment), &ctx);
        assert!(panel.diff.editing.is_none() && panel.diff.review.is_empty());
        // Dragged below the rows, the range goes on down.
        panel.click(ReposHit::DiffRow(3), &ctx);
        panel.drag_past_rows(1, &ctx);
        panel.drag_past_rows(1, &ctx);
        assert_eq!(cursor(&panel), Some((5, 3)));
    }

    #[test]
    fn a_long_comment_wraps_and_clearing_asks_first() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 30);
        ctx.cols = 10;
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        panel.focus = Focus::Detail;
        panel.diff.set_cursor(3, None);
        press(&mut panel, none(), Keysym::c, &ctx);
        type_text(&mut panel, "one two three", &ctx);
        let rows = detail_rows(&panel.view("p", &ctx));
        assert_eq!(rows[4..6], ["| one two ", "| three"]);
        let DetailRow::Editor { caret, .. } = &panel.view("p", &ctx).detail.rows[5] else {
            panic!();
        };
        assert_eq!(*caret, Some(5));
        press(&mut panel, none(), Keysym::Return, &ctx);
        // Wider, it is worked out again.
        ctx.cols = 40;
        press(&mut panel, none(), Keysym::Up, &ctx);
        assert_eq!(detail_rows(&panel.view("p", &ctx))[4], "> one two three");
        // A click on it edits it.
        panel.click(ReposHit::DiffRow(4), &ctx);
        assert_eq!(panel.diff.editing.as_ref().map(|e| e.at), Some(Some(0)));
        // A click elsewhere keeps the edit.
        type_text(&mut panel, "!", &ctx);
        panel.click(ReposHit::Line(0), &ctx);
        assert!(panel.diff.editing.is_none());
        assert_eq!(panel.diff.review.comments[0].text, "one two three!");
        // Clear review asks on a sheet, then deletes them all.
        panel.press(ReposButton::ClearReview, &ctx);
        assert!(matches!(panel.sheet, Some(Sheet::ClearReview { .. })));
        assert_eq!(panel.diff.review.comments.len(), 1);
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert!(panel.sheet.is_none() && panel.diff.review.is_empty());
    }

    #[test]
    fn a_folded_file_keeps_its_header_under_the_pointer() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 6);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        // Under src/git.rs pinned, notes.md folds and unfolds in place,
        // also scrolled part of a row.
        for pos in [4.0, 4.5] {
            panel.diff.scroll.jump(pos);
            panel.click(ReposHit::DiffFold(3), &ctx);
            let view = panel.view("p", &ctx);
            assert_eq!(panel.diff.scroll.pos(), pos);
            assert_eq!(view.detail.total, 10);
            let rows = detail_rows(&view);
            assert_eq!(rows[..4], ["= src/git.rs", "+b", "+c", "= notes.md"]);
            panel.click(ReposHit::DiffFold(3), &ctx);
            let rows = detail_rows(&panel.view("p", &ctx));
            assert_eq!(rows[3..5], ["= notes.md", "@ "]);
        }
        // At the end, the diff goes past it rather than pull the rows
        // above down under the pointer.
        swipe_detail(&mut panel, 10.0, &ctx);

        assert_eq!(panel.diff.scroll.pos(), 6.0);
        panel.click(ReposHit::DiffFold(1), &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!((view.detail.total, view.detail.first), (10, 6));
        assert_eq!(detail_rows(&view)[1], "= notes.md");
        // Not even a glide past the end pulls it back.
        let now = Instant::now();
        panel.scroll(Motion::Notches(1.0), now, true, &ctx);
        run_out(&mut panel, now, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 6.0);
        panel.click(ReposHit::DiffFold(1), &ctx);
        let rows = detail_rows(&panel.view("p", &ctx));
        assert_eq!(rows[1..3], ["= notes.md", "@ "]);
        // Scrolled back, the rows past the end are gone.
        panel.click(ReposHit::DiffFold(1), &ctx);
        swipe_detail(&mut panel, -2.0, &ctx);
        swipe_detail(&mut panel, 1.0, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 4.0);
    }

    #[test]
    fn the_diff_has_its_own_keys_and_clicks() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 4);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        // Tab: the tree, the detail, the field.
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Detail);
        // Down puts the cursor on the first row in view, then moves it,
        // its file selected in the tree and the diff following it.
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.diff.cursor.as_ref().map(|c| c.row), Some(0));
        assert_eq!(panel.selected, Some(Selected::Repo(root("gerdr"))));
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 0.0);
        let git = Selected::File(root("gerdr"), "src/git.rs".into());
        assert_eq!(panel.selected, Some(git));
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.diff.cursor.as_ref().map(|c| c.row), Some(4));
        assert_eq!(panel.diff.scroll.pos(), 1.0);
        // Its header at the top, pinned over itself.
        let marks = panel.view("p", &ctx).detail.marks;
        assert!(marks[4].selected && marks[4].top && marks[4].bottom && !marks[3].selected);
        // Alt+Down goes to the next file, and Alt+Up back.
        press(&mut panel, mods(false, false, true), Keysym::Down, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 6.0);
        let notes = Selected::File(root("gerdr"), "notes.md".into());
        assert_eq!(panel.selected, Some(notes));
        press(&mut panel, mods(false, false, true), Keysym::Down, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 8.0);
        press(&mut panel, mods(false, false, true), Keysym::Up, &ctx);
        press(&mut panel, mods(false, false, true), Keysym::Up, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 1.0);
        // Space checks the cursor's file, as its tree line.
        assert_eq!(panel.diff.cursor.as_ref().map(|c| c.row), Some(1));
        panel.diff.scroll.jump(3.0);
        press(&mut panel, none(), Keysym::space, &ctx);
        let key = (root("gerdr"), "src/git.rs".to_string());
        assert!(panel.checked_files.contains(&key));
        let DetailRow::File(file) = &panel.view("p", &ctx).detail.rows[0] else {
            panic!();
        };
        assert_eq!(file.check, Some(Check::On));
        // Enter opens it in Zed at the cursor's line, the diff scrolled
        // back to show it.
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 1.0);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::OpenInZed {
                root: root("gerdr"),
                paths: vec!["src/git.rs".into()],
                at: Some(diff_view::Position {
                    line: 1,
                    col: Some(0),
                }),
            })
        );
        // On a line, Left moves the caret, folding nothing.
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(panel.view("p", &ctx).detail.total, 12);
        assert_eq!(panel.diff.cursor.as_ref().map(|c| c.row), Some(3));
        // On its header, Left folds it, its header at the top; Right
        // unfolds it.
        press(&mut panel, none(), Keysym::Up, &ctx);
        press(&mut panel, none(), Keysym::Up, &ctx);
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(panel.diff.scroll.pos(), 1.0);
        assert_eq!(panel.view("p", &ctx).detail.total, 8);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(panel.view("p", &ctx).detail.total, 12);
        // Typing doesn't act on the tree.
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Field);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Tree);

        // Clicks: on a file's checkbox, its chevron, a row.
        panel.diff.scroll.jump(0.0);
        panel.click(ReposHit::DiffCheck(1), &ctx);
        assert!(!panel.checked_files.contains(&key));
        panel.click(ReposHit::DiffFold(1), &ctx);
        assert_eq!(panel.view("p", &ctx).detail.total, 8);
        // Folded, its header stayed where it was.
        assert_eq!(panel.diff.scroll.pos(), 0.0);
        panel.click(ReposHit::DiffFold(1), &ctx);
        assert_eq!(panel.view("p", &ctx).detail.total, 12);
        panel.diff.scroll.jump(0.0);
        panel.click(ReposHit::DiffRow(3), &ctx);
        assert_eq!(panel.focus, Focus::Detail);
        let git = Selected::File(root("gerdr"), "src/git.rs".into());
        assert_eq!(panel.selected, Some(git));
        assert_eq!(panel.diff.scroll.pos(), 0.0);
        assert_eq!(
            panel.open_detail_row(3, &ctx),
            Some(Outcome::OpenInZed {
                root: root("gerdr"),
                paths: vec!["src/git.rs".into()],
                at: Some(diff_view::Position {
                    line: 1,
                    col: Some(0),
                }),
            })
        );
        // Under the pinned header, rows are one down.
        panel.diff.scroll.jump(3.0);
        assert_eq!(panel.detail_row_at(0, &ctx), Some(1));
        assert_eq!(panel.detail_row_at(1, &ctx), Some(3));
        assert_eq!(panel.detail_row_at(4, &ctx), Some(6));
        assert_eq!(panel.detail_row_at(5, &ctx), None);
    }

    #[test]
    fn hints_follow_the_selection() {
        let items = merging(items());
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let buttons = |panel: &Panel| -> Vec<ReposButton> {
            panel
                .view("p", &ctx)
                .hints
                .iter()
                .map(|h| h.button)
                .collect()
        };
        let usual = buttons(&panel);
        assert_eq!(
            usual[..3],
            [
                ReposButton::Commit,
                ReposButton::CommitPush,
                ReposButton::Push
            ]
        );
        assert!(usual.contains(&ReposButton::Fetch));
        let hints = panel.view("p", &ctx).hints;
        assert_eq!(hints[0].key, "Ctrl+Enter");
        assert_eq!(
            hints[0].disabled.as_deref(),
            Some("Check the files to commit")
        );
        select(&mut panel, "herdr");
        assert_eq!(
            buttons(&panel),
            [
                ReposButton::OpenInZed,
                ReposButton::MarkResolved,
                ReposButton::Continue,
                ReposButton::Abort,
                ReposButton::Help
            ]
        );
        let hints = panel.view("p", &ctx).hints;
        assert_eq!(
            hints[2].disabled.as_deref(),
            Some("Resolve 1 conflicted file first (Ctrl+R)")
        );
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::e, &ctx),
            Some(Outcome::OpenInZed {
                root: root("herdr"),
                paths: vec!["src/api.rs".into()],
                at: None,
            })
        );
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::r, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::MarkResolved,
                jobs: vec![job("herdr", &["src/api.rs"])]
            })
        );
        // On a file, that file.
        select_file(&mut panel, "herdr", "README.md");
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::e, &ctx),
            Some(Outcome::OpenInZed {
                root: root("herdr"),
                paths: vec!["README.md".into()],
                at: None,
            })
        );
    }

    #[test]
    fn retry_failed_is_offered_once_over() {
        let items = items();
        let b = batch(
            BatchKind::Push { force: false },
            vec![
                run("gerdr", failed(Failure::Locked)),
                run("herdr", failed(Failure::Conflict(Vec::new()))),
            ],
        );
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        let view = panel.view("p", &ctx);
        let buttons = view.result.unwrap().buttons;
        assert_eq!(buttons.len(), 1);
        assert_eq!(
            (buttons[0].button, buttons[0].key.as_str()),
            (ReposButton::RetryFailed, "Alt+R")
        );
        assert_eq!(
            press(&mut panel, alt(), Keysym::r, &ctx),
            Some(Outcome::RetryFailed)
        );
        // Conflicts aren't retried: they are resolved.
        let b = batch(
            BatchKind::Push { force: false },
            vec![run("herdr", failed(Failure::Conflict(Vec::new())))],
        );
        let ctx = super::Ctx {
            batch: Some(&b),
            ..ctx
        };
        assert_eq!(press(&mut panel, alt(), Keysym::r, &ctx), None);
    }

    /// `items` with branches: gerdr on main, herdr on master (the same
    /// branch), zed on feat/x; all have feat/x, gerdr production, herdr's
    /// remote release.
    fn branched() -> Vec<RepoItem> {
        let mut items = items();
        let set = |item: &mut RepoItem, on: &str, local: &[&str], remote: &[&str]| {
            let names = |l: &[&str]| l.iter().map(|b| b.to_string()).collect();
            item.status.branch = Some(on.into());
            item.status.local_branches = names(local);
            item.status.remote_branches = names(remote);
            item.status.default_branch =
                Some(if on == "master" { "master" } else { "main" }.into());
        };
        set(
            &mut items[0],
            "main",
            &["feat/x", "main", "production"],
            &["feat/x", "main"],
        );
        set(
            &mut items[1],
            "master",
            &["feat/x", "master"],
            &["master", "release"],
        );
        set(&mut items[2], "feat/x", &["feat/x", "main"], &[]);
        items
    }

    /// The branch menu's rows as `panel` shows them.
    fn menu_rows(panel: &Panel, ctx: &Ctx) -> Vec<String> {
        let view = panel.view("p", ctx);
        view.branch.menu.map(|m| m.rows).unwrap_or_default()
    }

    fn check_repo(panel: &mut Panel, name: &str, ctx: &Ctx) {
        select(panel, name);
        press(panel, none(), Keysym::space, ctx);
    }

    #[test]
    fn merge_and_rebase_name_both_branches() {
        let label = |items: &[RepoItem], checked: &[&str], button| {
            let ctx = ctx(items, None);
            let mut panel = Panel::new(items);
            select(&mut panel, "zed");
            for name in checked {
                check_repo(&mut panel, name, &ctx);
            }
            panel.button_label(button, &ctx)
        };
        let mut items = branched();
        let merge = ReposButton::Merge;
        assert_eq!(label(&items, &[], merge), "merge main into feat/x");
        assert_eq!(
            label(&items, &[], ReposButton::Rebase),
            "rebase feat/x onto main"
        );
        assert_eq!(label(&items, &[], ReposButton::Fetch), "fetch all");
        // main and master are one branch.
        let both = ["gerdr", "herdr"];
        assert_eq!(
            label(&items, &both, merge),
            "merge main / master into main / master"
        );
        assert_eq!(
            label(&items, &["gerdr", "zed"], merge),
            "merge main into current branch"
        );
        items[1].status.default_branch = Some("trunk".into());
        assert_eq!(label(&items, &both, merge), "merge base into main / master");
        items[2].status.default_branch = None;
        items[2].status.branch = None;
        assert_eq!(label(&items, &[], merge), "merge base into current branch");
    }

    #[test]
    fn branches_go_default_then_top_then_newest() {
        let mut items = branched();
        let zed = &mut items[2].status;
        zed.local_branches = ["a-old", "b-new", "dev", "feat/x", "main", "zz"]
            .map(String::from)
            .into();
        let times = [
            ("a-old", 100),
            ("b-new", 300),
            ("zz", 300),
            ("feat/x", 200),
            ("dev", 50),
            ("main", 10),
        ];
        zed.branch_times = times.map(|(n, t)| (n.to_owned(), t)).into();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select(&mut panel, "zed");
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        assert_eq!(
            menu_rows(&panel, &ctx),
            ["main", "dev", "b-new", "zz", "feat/x", "a-old"]
        );
    }

    #[test]
    fn merge_or_rebase_a_branch_picked_in_the_menu() {
        let mut items = branched();
        items[2].status.local_branches.push("dev".into());
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select(&mut panel, "zed");
        assert_eq!(
            panel.button_label(ReposButton::MergeFrom, &ctx),
            "merge into feat/x…"
        );
        assert_eq!(
            panel.button_label(ReposButton::RebaseFrom, &ctx),
            "rebase feat/x onto…"
        );
        press(&mut panel, alt(), Keysym::m, &ctx);
        // The branch they are on isn't offered, nor deleted from here.
        assert_eq!(menu_rows(&panel, &ctx), ["main", "dev"]);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.deletable, [false, false]);
        assert_eq!(menu.placeholder, "Find a branch to merge into feat/x");
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        assert!(panel.sheet.is_none());
        let merge = BatchKind::Integrate {
            how: Integrate::Merge,
            from: Some(vec!["main".into(), "master".into()]),
        };
        assert_eq!(merge.label(), "Merge main / master");
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: merge,
                jobs: vec![job("zed", &[])]
            })
        );
        assert!(panel.branch_menu.is_none());

        press(&mut panel, mods(false, true, true), Keysym::M, &ctx);
        type_text(&mut panel, "dev", &ctx);
        let rebase = BatchKind::Integrate {
            how: Integrate::Rebase,
            from: Some(vec!["dev".into()]),
        };
        assert_eq!(rebase.label(), "Rebase onto dev");
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: rebase,
                jobs: vec![job("zed", &[])]
            })
        );

        // Those already on the branch picked are left out.
        check_repo(&mut panel, "gerdr", &ctx);
        check_repo(&mut panel, "zed", &ctx);
        press(&mut panel, alt(), Keysym::m, &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["main", "feat/x"]);
        let Some(Outcome::Batch { jobs, .. }) = press(&mut panel, none(), Keysym::Return, &ctx)
        else {
            panic!("no batch");
        };
        assert_eq!(jobs, [job("zed", &[])]);
    }

    #[test]
    fn names_in_a_group_are_one_branch() {
        let same = &*SAME;
        assert_eq!(branch_key("master", same), "main / master");
        assert_eq!(branch_key("main", same), "main / master");
        assert_eq!(branch_key("feat/x", same), "feat/x");
        assert_eq!(branch_key("master", &[]), "master");
        let prod = vec![vec!["production".to_owned(), "prod".to_owned()]];
        assert_eq!(branch_key("prod", &prod), "production / prod");
    }

    #[test]
    fn the_picker_offers_the_branches_they_all_have() {
        let items = branched();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        // The selected one alone, its default branch first.
        let picker = panel.view("p", &ctx).branch;
        assert_eq!((picker.label.as_str(), picker.mixed), ("main", false));
        assert!(picker.enabled);
        assert_eq!(press(&mut panel, ctrl(), Keysym::s, &ctx), None);
        assert_eq!(menu_rows(&panel, &ctx), ["main", "feat/x", "production"]);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!((menu.current, menu.picked), (Some(0), Some(0)));
        press(&mut panel, none(), Keysym::Escape, &ctx);
        assert!(panel.branch_menu.is_none());
        // main and master are one branch, called by both names.
        check_repo(&mut panel, "gerdr", &ctx);
        check_repo(&mut panel, "herdr", &ctx);
        let picker = panel.view("p", &ctx).branch;
        assert_eq!(
            (picker.label.as_str(), picker.mixed),
            ("main / master", false)
        );
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["main / master", "feat/x"]);
        // On different branches.
        press(&mut panel, none(), Keysym::Escape, &ctx);
        check_repo(&mut panel, "zed", &ctx);
        let picker = panel.view("p", &ctx).branch;
        assert_eq!(
            (picker.label.as_str(), picker.mixed),
            ("mixed branches", true)
        );
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["main / master", "feat/x"]);
        assert_eq!(panel.view("p", &ctx).branch.menu.unwrap().current, None);
    }

    #[test]
    fn picking_a_branch_switches_those_not_on_it() {
        let items = branched();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        for name in ["gerdr", "herdr", "zed"] {
            check_repo(&mut panel, name, &ctx);
        }
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        // Keys go to the menu alone.
        assert_eq!(press(&mut panel, ctrl(), Keysym::p, &ctx), None);
        assert!(panel.branch_menu.is_some());
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Switch(vec!["main".into(), "master".into()]),
                jobs: jobs(&["zed"]),
            })
        );
        assert!(panel.branch_menu.is_none());
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Switch(vec!["feat/x".into()]),
                jobs: jobs(&["gerdr", "herdr"]),
            })
        );
        // A click on a row picks it; one beside the menu only closes it.
        panel.click(ReposHit::BranchButton, &ctx);
        assert!(panel.branch_menu.is_some());
        assert_eq!(panel.click(ReposHit::Line(0), &ctx), None);
        assert!(panel.branch_menu.is_none());
        panel.click(ReposHit::BranchButton, &ctx);
        assert_eq!(
            panel.click(ReposHit::BranchRow(1), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Switch(vec!["feat/x".into()]),
                jobs: jobs(&["gerdr", "herdr"]),
            })
        );
        // All on it already: nothing to do.
        let mut one = Panel::new(&items);
        press(&mut one, ctrl(), Keysym::s, &ctx);
        assert_eq!(press(&mut one, none(), Keysym::Return, &ctx), None);
        assert_eq!(one.error.as_deref(), Some("Already on main"));
    }

    #[test]
    fn a_local_branch_is_deleted_after_a_sheet_asks() {
        let items = branched();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        for name in ["gerdr", "herdr", "zed"] {
            check_repo(&mut panel, name, &ctx);
        }
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.rows, ["main / master", "feat/x"]);
        assert_eq!(menu.deletable, [true, true]);
        // Delete asks on a sheet over the menu; Esc takes it back.
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.title, "Delete the branch feat/x in 3 repositories?");
        assert_eq!(
            sheet.lines[0],
            (
                "gerdr".into(),
                "delete the local branch feat/x".into(),
                Tone::Warning
            )
        );
        assert_eq!(
            sheet.buttons,
            [ReposButton::DeleteBranch, ReposButton::Cancel]
        );
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.sheet.is_none() && panel.branch_menu.is_some());
        // Those on it go too: the check tells why they are left out.
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::DeleteBranch {
                    names: vec!["feat/x".into()],
                    force: false,
                },
                jobs: jobs(&["gerdr", "herdr", "zed"]),
            })
        );
        // The menu stays, for the next.
        assert!(panel.branch_menu.is_some());
        // A click on a row's button asks the same.
        assert_eq!(panel.click(ReposHit::BranchDelete(1), &ctx), None);
        assert!(panel.sheet.is_some());
        assert!(matches!(
            panel.click(ReposHit::Button(ReposButton::DeleteBranch), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::DeleteBranch { .. },
                ..
            })
        ));
        // Not the branch they are all on, nor one only on the remote.
        let mut one = Panel::new(&items);
        press(&mut one, ctrl(), Keysym::s, &ctx);
        assert_eq!(menu_rows(&one, &ctx), ["main", "feat/x", "production"]);
        let menu = one.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.deletable, [false, true, true]);
        assert_eq!(press(&mut one, none(), Keysym::Delete, &ctx), None);
        assert!(one.sheet.is_none());
        assert_eq!(
            one.error.as_deref(),
            Some("On main: switch to another branch first")
        );
        press(&mut one, none(), Keysym::Escape, &ctx);
        select(&mut one, "herdr");
        press(&mut one, ctrl(), Keysym::s, &ctx);
        assert_eq!(menu_rows(&one, &ctx), ["master", "feat/x", "release"]);
        press(&mut one, ctrl(), Keysym::End, &ctx);
        assert_eq!(press(&mut one, none(), Keysym::Delete, &ctx), None);
        assert_eq!(one.error.as_deref(), Some("release is only on the remote"));
    }

    #[test]
    fn deleting_unmerged_branches_is_asked_about() {
        let items = branched();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let kind = BatchKind::DeleteBranch {
            names: vec!["feat/x".into()],
            force: false,
        };
        let ask = |panel: &mut Panel| {
            panel.ask_preflight(
                kind.clone(),
                jobs(&["gerdr", "herdr", "zed"]),
                vec![
                    (root("gerdr"), Verdict::Clean),
                    (root("herdr"), Verdict::Unmerged(2)),
                    (root("zed"), Verdict::Blocked("checked out here".into())),
                ],
            )
        };
        ask(&mut panel);
        let sheet = panel.view("p", &ctx).sheet.unwrap();
        assert_eq!(
            sheet.title,
            "Delete branch: 1 clean · 1 unmerged · 1 blocked"
        );
        assert_eq!(sheet.lines[1].1, "unmerged: 2 commits only on it");
        // Clean only, by default.
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Confirmed {
                kind: kind.clone(),
                jobs: jobs(&["gerdr"]),
            })
        );
        ask(&mut panel);
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::All), &ctx),
            Some(Outcome::Confirmed {
                kind: BatchKind::DeleteBranch {
                    names: vec!["feat/x".into()],
                    force: true,
                },
                jobs: jobs(&["gerdr", "herdr"]),
            })
        );
    }

    #[test]
    fn the_picker_waits_for_the_batch() {
        let items = branched();
        let b = batch(BatchKind::Fetch, vec![run("gerdr", RunState::Queued)]);
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        assert!(!panel.view("p", &ctx).branch.enabled);
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        assert!(panel.branch_menu.is_none());
        assert_eq!(panel.error.as_deref(), Some("Wait for the batch to finish"));
        // With nothing selected, there is nothing to switch.
        let mut none_selected = Panel::new(&items);
        none_selected.selected = None;
        let free = super::Ctx { batch: None, ..ctx };
        let picker = none_selected.view("p", &free).branch;
        assert!(!picker.enabled && picker.mixed);
    }

    #[test]
    fn a_long_branch_menu_scrolls_with_the_pick() {
        let mut items = branched();
        items[0].status.local_branches = (0..30).map(|i| format!("b{i:02}")).collect();
        items[0].status.local_branches.push("main".into());
        // Five rows: the tree's six, less the search's.
        let ctx = super::Ctx {
            page: (6, 6),
            ..ctx(&items, None)
        };
        let mut panel = Panel::new(&items);
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        // On main, which is first, of 31 and main/feat/x on the remote:
        // under the heading of the local ones, which takes a row.
        assert_eq!(menu_rows(&panel, &ctx).len(), 4);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.headings, [(0, "Local".to_owned())]);
        for _ in 0..7 {
            press(&mut panel, none(), Keysym::Down, &ctx);
        }
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.picked, Some(4));
        assert_eq!(menu.rows[4], "b06");
        press(&mut panel, ctrl(), Keysym::End, &ctx);
        // The one only on the remote last, under its heading.
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.rows.last().unwrap(), "feat/x");
        let last = menu.rows.len() - 1;
        assert_eq!(menu.headings, [(last, "Remote".to_owned())]);
        assert_eq!(menu.picked, Some(last));
        // A click on it picks it, not the heading's line.
        assert_eq!(
            panel.click(ReposHit::BranchRow(last), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Switch(vec!["feat/x".into()]),
                jobs: vec![Job {
                    root: items[0].root.clone(),
                    files: Vec::new(),
                }],
            })
        );
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        press(&mut panel, ctrl(), Keysym::End, &ctx);
        swipe_menu(&mut panel, -100.0, &ctx);
        assert_eq!(menu_rows(&panel, &ctx)[0], "main");
    }

    #[test]
    fn a_touchpad_scrolls_the_branch_menu_by_parts_of_a_line() {
        let mut items = branched();
        items[0].status.local_branches = (0..30).map(|i| format!("b{i:02}")).collect();
        items[0].status.local_branches.push("main".into());
        let ctx = super::Ctx {
            page: (6, 6),
            ..ctx(&items, None)
        };
        let mut panel = Panel::new(&items);
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        // Part of the heading scrolled past, one more row under the list.
        swipe_menu(&mut panel, 0.5, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.part, 0.5);
        assert_eq!(menu.headings, [(0, "Local".to_owned())]);
        assert_eq!(menu.rows, ["main", "b00", "b01", "b02", "b03"]);
        // A click on the row shown in part picks its branch.
        assert!(matches!(
            panel.click(ReposHit::BranchRow(4), &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Switch(names),
                ..
            }) if names == ["b03"]
        ));
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        swipe_menu(&mut panel, 1.25, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!((menu.part, menu.rows.len()), (0.25, 6));
        // The end holds the menu whole, and typing drops the part.
        swipe_menu(&mut panel, 100.5, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!((menu.part, menu.rows.len() + menu.headings.len()), (0.0, 5));
        swipe_menu(&mut panel, -1.5, &ctx);
        assert_eq!(panel.view("p", &ctx).branch.menu.unwrap().part, 0.5);
        press(&mut panel, none(), Keysym::b, &ctx);
        assert_eq!(panel.view("p", &ctx).branch.menu.unwrap().part, 0.0);
    }

    #[test]
    fn typing_finds_a_branch() {
        let mut items = branched();
        items[0].status.local_branches = (0..30).map(|i| format!("b{i:02}")).collect();
        items[0].status.local_branches.push("Feature/Login".into());
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        let view = panel.view("p", &ctx);
        assert!(!view.field_focused);
        assert_eq!(
            view.branch.menu.unwrap().placeholder,
            "Find or create a branch"
        );
        // Whatever the case, each word somewhere in the name.
        type_text(&mut panel, "FEAT", &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["Feature/Login", "feat/x"]);
        type_text(&mut panel, " log", &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["Feature/Login"]);
        assert_eq!(&*panel.field, "");
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!((menu.query.as_str(), menu.picked), ("FEAT log", Some(0)));
        // A name in a group finds the group.
        for _ in 0.."FEAT log".len() {
            press(&mut panel, none(), Keysym::BackSpace, &ctx);
        }
        type_text(&mut panel, "mast", &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["main"]);
        // Nothing found: Enter leaves the menu for another try, unless
        // the name could be a branch's, which it creates.
        type_text(&mut panel, "z z", &ctx);
        assert!(menu_rows(&panel, &ctx).is_empty());
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.empty, "No branch matches");
        let create = menu.create.unwrap();
        assert_eq!(create.label, "Can't create mastz z: it can't have spaces");
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.branch_menu.is_some());
        assert_eq!(
            panel.error.as_deref(),
            Some("Not a branch name: it can't have spaces")
        );
        // Enter picks what was found; Down moves among it.
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        type_text(&mut panel, "b2", &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        let picked = press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(
            picked,
            Some(Outcome::Batch {
                kind: BatchKind::Switch(vec!["b21".into()]),
                jobs: jobs(&["gerdr"]),
            })
        );
        // It opens empty again.
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        assert_eq!(
            panel.view("p", &ctx).branch.menu.unwrap().query.as_str(),
            ""
        );
    }

    #[test]
    fn ctrl_n_opens_the_menu_to_create_a_branch() {
        let items = branched();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        press(&mut panel, ctrl(), Keysym::n, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.placeholder, "Name the new branch");
        assert_eq!(menu.create, None);
        // Enter creates the branch typed, though some are found.
        type_text(&mut panel, "feat", &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.rows, ["feat/x"]);
        assert_eq!(menu.picked, None);
        let create = menu.create.unwrap();
        assert_eq!(create.label, "Create feat from main");
        assert!(create.picked);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::CreateBranch {
                name: "feat".into(),
                jobs: jobs(&["gerdr"]),
            })
        );
        assert!(panel.branch_menu.is_none());
        // The field is for commit messages alone.
        assert_eq!(&*panel.field, "");
    }

    #[test]
    fn the_switch_menu_offers_to_create_what_it_does_not_find() {
        let items = branched();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        type_text(&mut panel, "feat", &ctx);
        // Enter picks what was found; Up from it goes to creating.
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.picked, Some(0));
        assert!(!menu.create.unwrap().picked);
        press(&mut panel, none(), Keysym::Up, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.picked, None);
        assert!(menu.create.unwrap().picked);
        press(&mut panel, none(), Keysym::Down, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.picked, Some(0));
        // Ctrl+Enter creates, wherever the pick is.
        let create = Some(Outcome::CreateBranch {
            name: "feat".into(),
            jobs: jobs(&["gerdr"]),
        });
        assert_eq!(press(&mut panel, ctrl(), Keysym::Return, &ctx), create);
        // A branch shown by that name is picked, not created.
        press(&mut panel, ctrl(), Keysym::s, &ctx);
        type_text(&mut panel, "feat/x", &ctx);
        assert_eq!(panel.view("p", &ctx).branch.menu.unwrap().create, None);
        // Nothing found: Enter creates it.
        press(&mut panel, ctrl(), Keysym::a, &ctx);
        type_text(&mut panel, "fix/y", &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::CreateBranch {
                name: "fix/y".into(),
                jobs: jobs(&["gerdr"]),
            })
        );
        // The menu to merge from creates nothing.
        press(&mut panel, alt(), Keysym::m, &ctx);
        type_text(&mut panel, "fix/y", &ctx);
        assert_eq!(panel.view("p", &ctx).branch.menu.unwrap().create, None);
        // Ctrl+N there turns it into the menu to create.
        press(&mut panel, ctrl(), Keysym::n, &ctx);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert!(menu.create.unwrap().picked);
    }

    #[test]
    fn creating_a_branch_some_have_asks_first() {
        let items = items();
        let ctx = ctx(&items, None);
        let all = || jobs(&["gerdr", "herdr", "zed"]);
        let have = vec![
            (root("gerdr"), Found::On),
            (root("herdr"), Found::Remote("origin".into())),
        ];
        let mut panel = Panel::new(&items);
        panel.ask_exists("feat/x".into(), all(), have.clone());
        let sheet = panel.view("p", &ctx).sheet.unwrap();
        assert_eq!(sheet.title, "feat/x exists in 2 of 3 repositories");
        let lines: Vec<&str> = sheet.lines.iter().map(|(_, t, _)| t.as_str()).collect();
        assert_eq!(
            lines,
            ["on it already", "has it on origin", "doesn't have it"]
        );
        assert_eq!(
            sheet.buttons,
            [
                ReposButton::SwitchExisting,
                ReposButton::CreateMissing,
                ReposButton::EditName
            ]
        );
        // Enter edits the name: back in the menu, to create it again.
        assert_eq!(sheet.default, 2);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.sheet.is_none());
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.query.as_str(), "feat/x");
        assert_eq!(menu.placeholder, "Name the new branch");
        // Esc does too.
        panel.ask_exists("feat/x".into(), all(), have.clone());
        press(&mut panel, none(), Keysym::Escape, &ctx);
        assert!(panel.sheet.is_none() && panel.branch_menu.is_some());
        // Switched where they have it but are not on it, made elsewhere.
        panel.ask_exists("feat/x".into(), all(), have.clone());
        let switch = ReposHit::Button(ReposButton::SwitchExisting);
        assert_eq!(
            panel.click(switch, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Branch("feat/x".into()),
                jobs: jobs(&["herdr", "zed"]),
            })
        );
        // Made only where it is missing.
        panel.ask_exists("feat/x".into(), all(), have.clone());
        let create = ReposHit::Button(ReposButton::CreateMissing);
        assert_eq!(
            panel.click(create, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Branch("feat/x".into()),
                jobs: jobs(&["zed"]),
            })
        );
        // All have it: nowhere to create it.
        let have = vec![
            (root("gerdr"), Found::Local),
            (root("herdr"), Found::Local),
            (root("zed"), Found::Local),
        ];
        panel.ask_exists("feat/x".into(), all(), have);
        let sheet = panel.view("p", &ctx).sheet.unwrap();
        assert_eq!(sheet.title, "feat/x exists in all of them: switch to it?");
        assert_eq!(
            sheet.buttons,
            [ReposButton::SwitchExisting, ReposButton::EditName]
        );
    }

    /// A repository with files in folders, nested and not.
    fn foldered() -> Vec<RepoItem> {
        let paths = [
            "src/chrome/repos.rs",
            "src/chrome/mod.rs",
            "src/main.rs",
            "README.md",
            "docs/deep/only/a.md",
            "Cargo.toml",
            "assets/Logo.png",
            "assets/icon.svg",
        ];
        let changes = paths.iter().map(|p| change(p, 'M')).collect();
        vec![repo("app", "main", changes), repo("lib", "main", vec![])]
    }

    /// The tree of `foldered()` by folder.
    const FOLDERS: [&str; 15] = [
        "app",
        "assets/",
        "  icon.svg",
        "  Logo.png",
        "docs/deep/only/",
        "  a.md",
        "src/",
        "  chrome/",
        "    mod.rs",
        "    repos.rs",
        "  main.rs",
        "Cargo.toml",
        "README.md",
        "Clean · 1",
        "lib",
    ];

    fn by_folder(items: &[RepoItem]) -> Panel {
        let mut panel = Panel::new(items);
        panel.grouping = Grouping::Folders;
        panel
    }

    #[test]
    fn path_parts_cut_at_slashes() {
        assert_eq!(path_parts("a/b/c.rs", 0, 2), "a/b");
        assert_eq!(path_parts("a/b/c.rs", 1, 2), "b");
        assert_eq!(path_parts("a/b/c.rs", 2, 3), "c.rs");
        assert_eq!(path_parts("a/b/c.rs", 0, 3), "a/b/c.rs");
        assert!(in_dir("a/b/c.rs", "a/b"));
        assert!(!in_dir("a/bc.rs", "a/b"));
        assert!(!in_dir("a/b", "a/b"));
    }

    #[test]
    fn by_folder_folders_come_first_by_name_and_lone_folders_join() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let panel = by_folder(&items);
        assert_eq!(texts(&panel.view("p", &ctx)), FOLDERS);
        // Flat, every path whole, as git lists them.
        let list = Panel::new(&items);
        let view = list.view("p", &ctx);
        assert_eq!(
            texts(&view)[1..4],
            ["src/chrome/repos.rs", "src/chrome/mod.rs", "src/main.rs"]
        );
        assert_eq!(view.grouping, Grouping::List);
    }

    #[test]
    fn a_folder_folds_and_left_climbs_to_the_line_over() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let mut panel = by_folder(&items);
        panel.selected = Some(Selected::File(root("app"), "src/chrome/mod.rs".into()));
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(
            panel.selected,
            Some(Selected::Dir(root("app"), "src/chrome".into()))
        );
        press(&mut panel, none(), Keysym::Left, &ctx);
        let texts_now = texts(&panel.view("p", &ctx));
        assert_eq!(texts_now[7..9], ["  chrome/", "  main.rs"]);
        // Folded, Left goes up again; Right unfolds it.
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(
            panel.selected,
            Some(Selected::Dir(root("app"), "src".into()))
        );
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(texts(&panel.view("p", &ctx))[6..8], ["src/", "Cargo.toml"]);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(
            texts(&panel.view("p", &ctx))[6..9],
            ["src/", "  chrome/", "  main.rs"]
        );
    }

    #[test]
    fn a_folder_checks_its_files() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let mut panel = by_folder(&items);
        panel.selected = Some(Selected::Dir(root("app"), "src".into()));
        press(&mut panel, none(), Keysym::space, &ctx);
        let checked = |panel: &Panel, path: &str| {
            let key = (root("app"), path.to_owned());
            panel.checked_files.contains(&key)
        };
        assert!(checked(&panel, "src/main.rs") && checked(&panel, "src/chrome/mod.rs"));
        assert!(!checked(&panel, "README.md"));
        let view = panel.view("p", &ctx);
        let dir = |i: usize| match &view.lines[i] {
            TreeLine::Dir(d) => d.check,
            _ => panic!("not a folder"),
        };
        assert_eq!((dir(6), dir(7), dir(1)), (Check::On, Check::On, Check::Off));
        assert_eq!(checks(&panel, &ctx)[0], Check::Partial);
        panel.selected = Some(Selected::File(root("app"), "src/main.rs".into()));
        press(&mut panel, none(), Keysym::space, &ctx);
        let view = panel.view("p", &ctx);
        assert!(matches!(&view.lines[6], TreeLine::Dir(d) if d.check == Check::Partial));
        // Partly checked, it is unchecked.
        panel.selected = Some(Selected::Dir(root("app"), "src".into()));
        press(&mut panel, none(), Keysym::space, &ctx);
        assert!(panel.checked_files.is_empty());
    }

    #[test]
    fn alt_f_lists_by_folder_or_flat_and_says_so() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let outcome = press(&mut panel, alt(), Keysym::f, &ctx);
        assert_eq!(outcome, Some(Outcome::Grouping(Grouping::Folders)));
        assert_eq!(texts(&panel.view("p", &ctx)), FOLDERS);
        // A folder's line is gone from the list: its repository's is
        // selected.
        panel.selected = Some(Selected::Dir(root("app"), "src".into()));
        let outcome = panel.click(ReposHit::Tool(TreeTool::List), &ctx);
        assert_eq!(outcome, Some(Outcome::Grouping(Grouping::List)));
        assert_eq!(panel.selected, Some(Selected::Repo(root("app"))));
        assert_eq!(panel.click(ReposHit::Tool(TreeTool::List), &ctx), None);
    }

    #[test]
    fn the_diff_goes_in_the_trees_order() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let wanted = panel.diffs_wanted(&ctx);
        let paths: Vec<_> = wanted[0].1.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths[0], "src/chrome/repos.rs");
        let files = wanted[0]
            .1
            .iter()
            .map(|c| added(&c.path, 'M', &["x"]))
            .collect();
        panel.set_diffs(root("app"), wanted[0].2, files, &ctx);
        panel.set_grouping(Grouping::Folders, &ctx);
        let order: Vec<_> = panel.diff.diffs[&root("app")]
            .1
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        let tree: Vec<_> = tree_order(&items[0].status.changes, Grouping::Folders)
            .into_iter()
            .map(|c| items[0].status.changes[c].path.as_str())
            .collect();
        assert_eq!(order, tree);
        assert_eq!(order[..2], ["assets/icon.svg", "assets/Logo.png"]);
        // Selecting a folder scrolls the diff to its first file.
        panel.selected = Some(Selected::Dir(root("app"), "src".into()));
        panel.show_selected();
        let head = panel
            .diff
            .header_row(&root("app"), Some("src/chrome/mod.rs"));
        assert_eq!(Some(panel.diff.scroll.pos() as usize), head);
    }

    #[test]
    fn a_diff_row_selects_its_file_in_folded_folders() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let mut panel = by_folder(&items);
        let wanted = panel.diffs_wanted(&ctx);
        let files = wanted[0]
            .1
            .iter()
            .map(|c| added(&c.path, 'M', &["x"]))
            .collect();
        panel.set_diffs(root("app"), wanted[0].2, files, &ctx);
        panel.folded_dirs.insert((root("app"), "src".into()));
        let r = panel
            .diff
            .header_row(&root("app"), Some("src/main.rs"))
            .unwrap();
        panel.select_diff_row(r, &ctx);
        assert_eq!(
            panel.selected,
            Some(Selected::File(root("app"), "src/main.rs".into()))
        );
        assert!(panel.folded_dirs.is_empty());
    }

    #[test]
    fn a_folder_git_lists_whole_is_one_line_among_the_folders() {
        let changes = vec![
            change("src/main.rs", 'M'),
            change("assets/", '?'),
            change("src/new/", '?'),
            change("zed.toml", '?'),
            change("only/", '?'),
        ];
        let items = vec![repo("app", "main", changes)];
        let ctx = ctx(&items, None);
        let panel = by_folder(&items);
        assert_eq!(
            texts(&panel.view("p", &ctx)),
            [
                "app",
                "assets/",
                "only/",
                "src/",
                "  new/",
                "  main.rs",
                "zed.toml"
            ]
        );
    }

    #[test]
    fn under_its_folder_a_file_shows_by_name() {
        let rename = |orig: &str| Change {
            path: "src/new.rs".into(),
            orig: Some(orig.into()),
            code: 'R',
        };
        assert_eq!(file_name_text(&change("src/a/b.rs", 'M')), "b.rs");
        assert_eq!(file_name_text(&rename("src/old.rs")), "old.rs → new.rs");
        assert_eq!(file_name_text(&rename("lib/old.rs")), "lib/old.rs → new.rs");
    }

    #[test]
    fn everything_folds_and_unfolds_at_once() {
        let items = foldered();
        let ctx = ctx(&items, None);
        let mut panel = by_folder(&items);
        panel.selected = Some(Selected::File(root("app"), "src/main.rs".into()));
        press(&mut panel, alt(), Keysym::Left, &ctx);
        assert_eq!(texts(&panel.view("p", &ctx)), ["app", "Clean · 1"]);
        // The file hidden, its repository's line is selected.
        assert_eq!(panel.selected, Some(Selected::Repo(root("app"))));
        // Its repository unfolded, its folders are folded too.
        press(&mut panel, none(), Keysym::Right, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!(
            texts(&view),
            [
                "app",
                "assets/",
                "docs/deep/only/",
                "src/",
                "Cargo.toml",
                "README.md",
                "Clean · 1"
            ]
        );
        panel.click(ReposHit::Tool(TreeTool::Expand), &ctx);
        assert_eq!(texts(&panel.view("p", &ctx)), FOLDERS);
        panel.click(ReposHit::Tool(TreeTool::Collapse), &ctx);
        assert_eq!(texts(&panel.view("p", &ctx)), ["app", "Clean · 1"]);
        press(&mut panel, alt(), Keysym::Right, &ctx);
        assert_eq!(texts(&panel.view("p", &ctx)), FOLDERS);
    }

    /// The diff row the cursor is on.
    fn cursor_row(panel: &Panel) -> Option<usize> {
        panel.diff.cursor.as_ref().map(|c| c.row)
    }

    /// What the search says it found.
    fn search_count(panel: &Panel, ctx: &Ctx) -> String {
        panel.view("p", ctx).detail.search.expect("open").count
    }

    #[test]
    fn star_finds_the_word_at_the_caret_as_a_whole_word() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let lines = ["let id = 1;", "let idle = id;", "id(id_x)"];
        let g = panel.diff.reads;
        panel.set_diffs(root("gerdr"), g, vec![added("src/a.rs", 'M', &lines)], &ctx);
        let line_row = |panel: &Panel, n: usize| {
            let at = |r: &DiffRow| matches!(r, DiffRow::Line(_, _, _, l) if *l == n);
            panel.diff.rows.iter().position(at).unwrap()
        };
        let (shift, star, hash) = (
            mods(false, true, false),
            Keysym::asterisk,
            Keysym::numbersign,
        );
        // Typed into the field, it is the field's.
        panel.caret_to(line_row(&panel, 0), 5, &ctx);
        panel.focus = Focus::Field;
        panel.key(&shift, star, star, &ctx);
        assert!(panel.diff.search.is_none());
        panel.focus = Focus::Tree;
        // On `id` of the first line: the next whole `id`, on the second,
        // the diff taking the keys, the caret on it.
        assert_eq!(panel.key(&shift, star, star, &ctx), None);
        let bar = panel.view("p", &ctx).detail.search.expect("open");
        assert!(bar.whole && !bar.focused);
        assert_eq!(bar.query.text, "id");
        assert_eq!(bar.count, "2 of 3");
        assert_eq!(panel.diff.caret(), Some((line_row(&panel, 1), 11)));
        assert_eq!(
            panel.diff.found_on(line_row(&panel, 2)),
            vec![(0..2, false)]
        );
        // `#`: back from it, round to the last.
        panel.caret_to(line_row(&panel, 0), 4, &ctx);
        panel.key(&shift, hash, hash, &ctx);
        assert_eq!(search_count(&panel, &ctx), "3 of 3");
        assert_eq!(cursor_row(&panel), Some(line_row(&panel, 2)));
        // `n` goes on, round past the end, `N` back.
        press(&mut panel, none(), Keysym::n, &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 3");
        panel.key(&shift, Keysym::N, Keysym::N, &ctx);
        assert_eq!(search_count(&panel, &ctx), "3 of 3");
        // The word at the caret is underlined, off a word the next.
        panel.caret_to(line_row(&panel, 1), 6, &ctx);
        let view = panel.view("p", &ctx);
        let second = |r: &DetailRow| matches!(r, DetailRow::Line(l) if l.text == "let idle = id;");
        let i = view.detail.rows.iter().position(second).unwrap();
        assert_eq!(view.detail.caret, Some((i, 6)));
        assert_eq!(view.detail.word, Some((i, 4..8)));
        panel.caret_to(line_row(&panel, 1), 3, &ctx);
        assert_eq!(panel.view("p", &ctx).detail.word, Some((i, 4..8)));
        // Typed into, it finds anywhere again.
        panel.key(&none(), Keysym::slash, Keysym::slash, &ctx);
        type_text(&mut panel, "id", &ctx);
        let bar = panel.view("p", &ctx).detail.search.expect("open");
        assert!(!bar.whole);
        assert!(bar.count.ends_with("of 5"));
        // The cursor off a line: it says so.
        press(&mut panel, none(), Keysym::Down, &ctx);
        let head = panel
            .diff
            .rows
            .iter()
            .position(|r| matches!(r, DiffRow::File(..)));
        panel.diff.set_cursor(head.unwrap(), None);
        assert_eq!(
            panel.key(&shift, star, star, &ctx),
            Some(Outcome::Notice("No word at the caret"))
        );
    }

    #[test]
    fn the_caret_goes_along_the_diffs_lines_as_vims_does() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let lines = ["let id = 1;", "  foo(bar)"];
        let g = panel.diff.reads;
        panel.set_diffs(root("gerdr"), g, vec![added("src/a.rs", 'M', &lines)], &ctx);
        let (r0, r1) = {
            let at = |n: usize| {
                let on = |r: &DiffRow| matches!(r, DiffRow::Line(_, _, _, l) if *l == n);
                panel.diff.rows.iter().position(on).unwrap()
            };
            (at(0), at(1))
        };
        panel.focus = Focus::Detail;
        // The first puts the cursor in view.
        press(&mut panel, none(), Keysym::w, &ctx);
        assert!(panel.diff.cursor.is_some());
        panel.caret_to(r0, 0, &ctx);
        let mut went = Vec::new();
        for k in [Keysym::w, Keysym::w, Keysym::w, Keysym::w, Keysym::w] {
            press(&mut panel, none(), k, &ctx);
            went.push(panel.diff.caret().unwrap());
        }
        assert_eq!(went, [(r0, 4), (r0, 7), (r0, 9), (r0, 10), (r1, 2)]);
        press(&mut panel, none(), Keysym::e, &ctx);
        assert_eq!(panel.diff.caret(), Some((r1, 4)));
        press(&mut panel, none(), Keysym::e, &ctx);
        assert_eq!(panel.diff.caret(), Some((r1, 5)));
        press(&mut panel, none(), Keysym::b, &ctx);
        press(&mut panel, none(), Keysym::b, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 10)));
        press(&mut panel, none(), Keysym::_0, &ctx);
        press(&mut panel, none(), Keysym::l, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 1)));
        press(&mut panel, none(), Keysym::h, &ctx);
        press(&mut panel, none(), Keysym::h, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 0)));
        // Right and Left go as l and h.
        press(&mut panel, none(), Keysym::Right, &ctx);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 2)));
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 1)));
        press(&mut panel, none(), Keysym::Left, &ctx);
        // Up and down keep how far in it stands, on the line's last char
        // for a shorter one.
        let shift = mods(false, true, false);
        panel.key(&shift, Keysym::dollar, Keysym::dollar, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 10)));
        press(&mut panel, none(), Keysym::j, &ctx);
        assert_eq!(panel.diff.caret(), Some((r1, 9)));
        press(&mut panel, none(), Keysym::k, &ctx);
        assert_eq!(panel.diff.caret(), Some((r0, 10)));
        press(&mut panel, none(), Keysym::Down, &ctx);
        panel.key(&shift, Keysym::asciicircum, Keysym::asciicircum, &ctx);
        assert_eq!(panel.diff.caret(), Some((r1, 2)));
    }

    #[test]
    fn r_and_the_restore_buttons_put_changes_back() {
        use crate::diff_edit::Cut;
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let text = "diff --git a/f b/f\n@@ -1,4 +1,7 @@ fn main() {\n f();\n+if x == nil {\n+\treturn\n+}\n g();\n-\told\n+new\n h();\n";
        let mut file = crate::diff::parse(text).pop().unwrap();
        file.path = "src/a.rs".into();
        file.code = 'M';
        let g = panel.diff.reads;
        panel.set_diffs(root("gerdr"), g, vec![file], &ctx);
        let at = |panel: &Panel, n: usize| {
            let on = |r: &DiffRow| matches!(r, DiffRow::Line(_, _, _, l) if *l == n);
            panel.diff.rows.iter().position(on).unwrap()
        };
        panel.focus = Focus::Detail;
        let restore = |o: Option<Outcome>| match o {
            Some(Outcome::Cut(edit)) => match edit.cut {
                Cut::Restore { drop, put } => (drop, put),
                other => panic!("not a restore: {other:?}"),
            },
            other => panic!("not a cut: {other:?}"),
        };
        let put = |n: usize, l: &str| vec![(n, vec![l.to_owned()])];
        let keys = |panel: &mut Panel, keys: &str| {
            let mut last = None;
            for c in keys.chars() {
                let sym = Keysym::from_char(c);
                let shift = mods(false, c.is_uppercase(), false);
                last = panel.key(&shift, Keysym::from_char(c.to_ascii_lowercase()), sym, &ctx);
            }
            last
        };
        // The change at the caret: the added check, whole.
        panel.caret_to(at(&panel, 2), 0, &ctx);
        assert_eq!(restore(keys(&mut panel, "R")), (vec![2, 3, 4], vec![]));
        // A changed line: the removed one back, tab and all.
        panel.caret_to(at(&panel, 6), 0, &ctx);
        assert_eq!(restore(keys(&mut panel, "R")), (vec![6], put(6, "\told")));
        // Not on a change.
        panel.caret_to(at(&panel, 4), 0, &ctx);
        assert_eq!(
            keys(&mut panel, "R"),
            Some(Outcome::Notice("No change there"))
        );
        // `V` on one line: that line alone.
        panel.caret_to(at(&panel, 6), 0, &ctx);
        assert_eq!(keys(&mut panel, "V"), None);
        assert_eq!(restore(keys(&mut panel, "R")), (vec![6], vec![]));
        // The lines selected only: `}` and the removed line.
        panel.caret_to(at(&panel, 3), 0, &ctx);
        assert_eq!(keys(&mut panel, "Vjj"), None);
        assert_eq!(restore(keys(&mut panel, "R")), (vec![4], put(6, "\told")));
        // The hunk's header has a button putting the whole hunk back; the
        // cursor's changed line one for its change.
        panel.caret_to(at(&panel, 6), 0, &ctx);
        let view = panel.view("p", &ctx);
        let hunk = view
            .detail
            .rows
            .iter()
            .position(|r| matches!(r, DetailRow::Hunk { .. }));
        let hunk = hunk.unwrap();
        let on = |i: usize| {
            view.detail
                .row_buttons
                .iter()
                .any(|(j, h)| *j == i && h.button == ReposButton::Restore)
        };
        // No key puts the hunk back, so its button shows none.
        let key = |i: usize| {
            let restore = |(j, h): &&(usize, Hint)| *j == i && h.button == ReposButton::Restore;
            view.detail
                .row_buttons
                .iter()
                .find(restore)
                .map(|(_, h)| h.key.clone())
        };
        assert!(on(hunk));
        let line = view
            .detail
            .rows
            .iter()
            .position(|r| matches!(r, DetailRow::Line(l) if l.text == "new"));
        assert!(on(line.unwrap()));
        assert_eq!(key(hunk).as_deref(), Some(""));
        assert_eq!(key(line.unwrap()).as_deref(), Some("R"));
        assert_eq!(
            restore(panel.click(ReposHit::DiffRestore(hunk), &ctx)),
            (vec![2, 3, 4, 6], put(6, "\told"))
        );
    }

    #[test]
    fn vims_deletes_cut_the_file_at_the_caret() {
        use crate::diff_edit::{Cut, Spot};
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let text =
            "diff --git a/f b/f\n@@ -1,3 +1,3 @@\n ctx(a, b);\n-old\n+let id = 1;\n+\tfoo(bar)\n";
        let mut file = crate::diff::parse(text).pop().unwrap();
        file.path = "src/a.rs".into();
        file.code = 'M';
        let g = panel.diff.reads;
        panel.set_diffs(root("gerdr"), g, vec![file], &ctx);
        let at = |panel: &Panel, n: usize| {
            let on = |r: &DiffRow| matches!(r, DiffRow::Line(_, _, _, l) if *l == n);
            panel.diff.rows.iter().position(on).unwrap()
        };
        let (ctx_row, old, id, foo) = (at(&panel, 0), at(&panel, 1), at(&panel, 2), at(&panel, 3));
        panel.focus = Focus::Detail;
        let spot = |line, col| Spot { line, col };
        let cut = |o: Option<Outcome>| match o {
            Some(Outcome::Cut(edit)) => {
                assert_eq!((edit.root, edit.path.as_str()), (root("gerdr"), "src/a.rs"));
                assert_eq!(edit.shown.len(), 3);
                (edit.cut, edit.caret)
            }
            other => panic!("not a cut: {other:?}"),
        };
        let keys = |panel: &mut Panel, keys: &str| {
            let mut last = None;
            for c in keys.chars() {
                let sym = Keysym::from_char(c);
                let shift = mods(false, c.is_uppercase() || "$^".contains(c), false);
                last = panel.key(&shift, Keysym::from_char(c.to_ascii_lowercase()), sym, &ctx);
            }
            last
        };
        // Lines, by their numbers in the file: `dd`, a count, `dj`, `dk`.
        panel.caret_to(id, 4, &ctx);
        assert_eq!(
            cut(keys(&mut panel, "dd")),
            (Cut::Lines(vec![2]), spot(2, 4))
        );
        assert_eq!(cut(keys(&mut panel, "3dd")).0, Cut::Lines(vec![2, 3, 4]));
        assert_eq!(cut(keys(&mut panel, "dj")).0, Cut::Lines(vec![2, 3]));
        assert_eq!(cut(keys(&mut panel, "d2k")).0, Cut::Lines(vec![1, 2]));
        // Chars along the line, never past it.
        assert_eq!(
            cut(keys(&mut panel, "x")).0,
            Cut::Chars(spot(2, 4), spot(2, 5))
        );
        assert_eq!(
            cut(keys(&mut panel, "2x")).0,
            Cut::Chars(spot(2, 4), spot(2, 6))
        );
        assert_eq!(
            cut(keys(&mut panel, "X")).0,
            Cut::Chars(spot(2, 3), spot(2, 4))
        );
        assert_eq!(
            cut(keys(&mut panel, "D")).0,
            Cut::Chars(spot(2, 4), spot(2, 11))
        );
        assert_eq!(
            cut(keys(&mut panel, "d$")).0,
            Cut::Chars(spot(2, 4), spot(2, 11))
        );
        assert_eq!(
            cut(keys(&mut panel, "dw")).0,
            Cut::Chars(spot(2, 4), spot(2, 7))
        );
        assert_eq!(
            cut(keys(&mut panel, "de")).0,
            Cut::Chars(spot(2, 4), spot(2, 6))
        );
        assert_eq!(
            cut(keys(&mut panel, "db")).0,
            Cut::Chars(spot(2, 0), spot(2, 4))
        );
        assert_eq!(
            cut(keys(&mut panel, "d0")).0,
            Cut::Chars(spot(2, 0), spot(2, 4))
        );
        // On the line's last word, `dw` takes it to the end.
        panel.caret_to(id, 10, &ctx);
        assert_eq!(
            cut(keys(&mut panel, "dw")).0,
            Cut::Chars(spot(2, 10), spot(2, 11))
        );
        // Esc forgets the `d`; a removed line isn't in the file.
        assert_eq!(keys(&mut panel, "d"), None);
        assert_eq!(
            panel.key(&none(), Keysym::Escape, Keysym::Escape, &ctx),
            None
        );
        assert_eq!(keys(&mut panel, "j"), None);
        assert_eq!(panel.diff.caret().map(|(r, _)| r), Some(foo));
        panel.caret_to(old, 0, &ctx);
        assert_eq!(
            keys(&mut panel, "dd"),
            Some(Outcome::Notice("A removed line isn't in the file"))
        );
        // A count moves on as many times.
        panel.caret_to(ctx_row, 0, &ctx);
        keys(&mut panel, "3j");
        assert_eq!(panel.diff.caret().map(|(r, _)| r), Some(foo));
        // By lines with V, over the removed line; Shift with the arrows too.
        panel.caret_to(ctx_row, 0, &ctx);
        assert_eq!(keys(&mut panel, "Vjj"), None);
        assert_eq!(
            panel.diff.mode_title().as_deref(),
            Some("-- VISUAL LINE -- d delete · y yank · R restore · Esc stop")
        );
        assert_eq!(cut(keys(&mut panel, "d")).0, Cut::Lines(vec![1, 2]));
        assert!(panel.diff.visual.is_none());
        panel.caret_to(id, 0, &ctx);
        panel.key(&mods(false, true, false), Keysym::Down, Keysym::Down, &ctx);
        assert_eq!(cut(keys(&mut panel, "x")).0, Cut::Lines(vec![2, 3]));
        // By chars with v, on to the next line; shown as chosen.
        panel.caret_to(id, 4, &ctx);
        keys(&mut panel, "vj");
        let shown: Vec<usize> = (0..panel.diff.rows.len()).collect();
        assert_eq!(panel.diff.chosen(&shown), [(id, 4..11), (foo, 0..5)]);
        assert_eq!(
            cut(keys(&mut panel, "d")).0,
            Cut::Chars(spot(2, 4), spot(3, 5))
        );
        assert!(panel.diff.chosen(&shown).is_empty());
        // `y` yanks as `d` deletes; `p` and `P` paste at the caret.
        let yank = |o: Option<Outcome>| match o {
            Some(Outcome::Yank(edit)) => edit.cut,
            other => panic!("not a yank: {other:?}"),
        };
        panel.caret_to(id, 4, &ctx);
        assert_eq!(yank(keys(&mut panel, "yy")), Cut::Lines(vec![2]));
        assert_eq!(yank(keys(&mut panel, "2Y")), Cut::Lines(vec![2, 3]));
        assert_eq!(
            yank(keys(&mut panel, "yw")),
            Cut::Chars(spot(2, 4), spot(2, 7))
        );
        assert_eq!(panel.diff.mode_title(), None);
        keys(&mut panel, "3y");
        assert_eq!(panel.diff.mode_title().as_deref(), Some("3y"));
        assert_eq!(yank(keys(&mut panel, "j")), Cut::Lines(vec![2, 3, 4, 5]));
        keys(&mut panel, "vl");
        assert_eq!(
            yank(keys(&mut panel, "y")),
            Cut::Chars(spot(2, 4), spot(2, 6))
        );
        assert!(panel.diff.visual.is_none());
        match keys(&mut panel, "2P") {
            Some(Outcome::Paste(put)) => {
                assert_eq!((put.at, put.before, put.count), (spot(2, 5), true, 2));
                assert_eq!(put.path, "src/a.rs");
            }
            other => panic!("not a paste: {other:?}"),
        }
        assert!(matches!(keys(&mut panel, "p"), Some(Outcome::Paste(put)) if !put.before));
        panel.caret_to(old, 0, &ctx);
        assert_eq!(
            keys(&mut panel, "p"),
            Some(Outcome::Notice("A removed line isn't in the file"))
        );
        // `u` and `U` undo and do again.
        assert_eq!(keys(&mut panel, "u"), Some(Outcome::UndoCut));
        assert_eq!(keys(&mut panel, "U"), Some(Outcome::RedoCut));
    }

    #[test]
    fn after_a_cut_the_caret_goes_back_to_its_line() {
        use crate::diff_edit::Spot;
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let g = panel.diff.reads;
        let lines = ["one", "two", "three"];
        panel.set_diffs(root("gerdr"), g, vec![added("src/a.rs", 'M', &lines)], &ctx);
        panel.focus = Focus::Detail;
        panel
            .diff
            .place_caret(root("gerdr"), "src/a.rs".into(), Spot { line: 3, col: 2 });
        // A read for the Git status read before stays where it was.
        panel.set_diffs(root("gerdr"), g, vec![added("src/a.rs", 'M', &lines)], &ctx);
        assert!(panel.diff.caret().is_none());
        let lines = ["one", "three"];
        panel.set_diffs(
            root("gerdr"),
            g + 1,
            vec![added("src/a.rs", 'M', &lines)],
            &ctx,
        );
        let (row, col) = panel.diff.caret().unwrap();
        assert_eq!((panel.diff.line_text(row), col), (Some("three"), 2));
        // Keys typed while it is read again wait for it.
        panel
            .diff
            .place_caret(root("gerdr"), "src/a.rs".into(), Spot { line: 1, col: 0 });
        assert_eq!(press(&mut panel, none(), Keysym::x, &ctx), None);
        assert!(panel.diff.take_held().is_empty());
        panel.set_diffs(
            root("gerdr"),
            g + 2,
            vec![added("src/a.rs", 'M', &lines)],
            &ctx,
        );
        assert_eq!(panel.diff.take_held(), [(none(), Keysym::x, Keysym::x)]);
    }

    #[test]
    fn slash_searches_the_diff_and_enter_goes_round_what_it_found() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        // Typed in the tree, a slash goes to the field.
        panel.key(&none(), Keysym::slash, Keysym::slash, &ctx);
        assert!(panel.diff.search.is_none());
        assert_eq!(panel.field.line().text, "/");
        panel.focus = Focus::Detail;
        panel.key(&none(), Keysym::slash, Keysym::slash, &ctx);
        let bar = panel.view("p", &ctx).detail.search.expect("open");
        assert!(bar.focused && bar.count.is_empty());
        // "n": notes.md's path (row 6), its line (8), new.rs's path (11).
        type_text(&mut panel, "n", &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 3");
        assert_eq!(cursor_row(&panel), Some(6));
        let notes = Selected::File(root("gerdr"), "notes.md".into());
        assert_eq!(panel.selected, Some(notes));
        assert_eq!(panel.diff.found_on(6), vec![(0..1, true)]);
        assert_eq!(panel.diff.found_on(8), vec![(0..1, false)]);
        // On new.rs's header, its name first.
        assert_eq!(panel.diff.found_on(11), vec![(0..1, false)]);
        assert!(panel.diff.found_on(3).is_empty());
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(cursor_row(&panel), Some(8));
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(search_count(&panel, &ctx), "3 of 3");
        assert_eq!(cursor_row(&panel), Some(11));
        // Round past the end, and back.
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(cursor_row(&panel), Some(6));
        press(&mut panel, mods(false, true, false), Keysym::Return, &ctx);
        assert_eq!(cursor_row(&panel), Some(11));
        // The ticks: a share of the twelve rows each, the one gone to
        // marked.
        let ticks = panel.view("p", &ctx).detail.ticks;
        let share = |r: f64| r / 12.0;
        assert_eq!(
            ticks,
            vec![
                (share(6.0), false),
                (share(8.0), false),
                (share(11.0), true)
            ]
        );
        // A capital finds only its case.
        type_text(&mut panel, "N", &ctx);
        assert_eq!(search_count(&panel, &ctx), "No matches");
        assert!(panel.view("p", &ctx).detail.ticks.is_empty());
        press(&mut panel, none(), Keysym::BackSpace, &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 3");
        // Down gives the diff the keys back, the places still found; F3
        // goes on from there.
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert!(!panel.search_has_keys());
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(cursor_row(&panel), Some(7));
        press(&mut panel, none(), Keysym::F3, &ctx);
        assert_eq!(cursor_row(&panel), Some(8));
        assert_eq!(search_count(&panel, &ctx), "2 of 3");
        // Esc closes the search first, then the panel.
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.diff.search.is_none());
        assert!(panel.view("p", &ctx).detail.found.iter().all(Vec::is_empty));
        assert_eq!(
            press(&mut panel, none(), Keysym::Escape, &ctx),
            Some(Outcome::Close)
        );
    }

    #[test]
    fn ctrl_shift_f_searches_and_ctrl_f_still_fetches() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        panel.tab = Some(DetailTab::Log);
        assert_eq!(press(&mut panel, ctrl_shift(), Keysym::F, &ctx), None);
        assert_eq!(panel.tab(), DetailTab::Diff);
        assert_eq!(panel.focus, Focus::Detail);
        assert!(panel.search_has_keys());
        // Its own keys type; Esc closes it, the panel open.
        type_text(&mut panel, "git", &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 1");
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.diff.search.is_none());
        let fetch = press(&mut panel, ctrl(), Keysym::f, &ctx);
        assert!(matches!(
            fetch,
            Some(Outcome::Batch {
                kind: BatchKind::Fetch,
                ..
            })
        ));
        assert!(panel.diff.search.is_none());
    }

    #[test]
    fn a_place_in_a_folded_file_unfolds_it_but_its_path() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        let key = (root("gerdr"), "notes.md".to_string());
        panel.diff.folded.insert(key.clone());
        panel.build_rows(&items);
        // Folded: gerdr, src/git.rs, its hunk, a, b, c, notes.md, a gap,
        // zed, new.rs.
        press(&mut panel, ctrl_shift(), Keysym::F, &ctx);
        type_text(&mut panel, "n", &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 3");
        // Its path shows on its header: it stays folded.
        assert_eq!(cursor_row(&panel), Some(6));
        assert!(panel.diff.folded.contains(&key));
        // Its line's tick is on its header.
        let ticks: Vec<f64> = panel.diff.search_ticks().iter().map(|t| t.0).collect();
        assert_eq!(ticks, vec![6.0 / 10.0, 9.0 / 10.0]);
        assert_eq!(panel.diff.found_on(6), vec![(0..1, true)]);
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert!(!panel.diff.folded.contains(&key));
        assert_eq!(cursor_row(&panel), Some(8));
        assert_eq!(search_count(&panel, &ctx), "2 of 3");
        assert_eq!(panel.diff.found_on(8), vec![(0..1, true)]);
    }

    #[test]
    fn the_search_finds_what_hunks_are_in_and_keeps_its_place_on_a_read() {
        use crate::chrome::SearchPart;
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let text = "diff --git a/f b/f\n\
            @@ -1,2 +1,2 @@ fn alpha()\n x\n-y\n+z\n\
            @@ -9,1 +9,1 @@ fn beta()\n-alpha\n+gamma\n";
        let mut file = crate::diff::parse(text).pop().unwrap();
        file.path = "src/git.rs".into();
        file.code = 'M';
        let g = panel.diff.reads;
        panel.set_diffs(root("gerdr"), g, vec![file.clone()], &ctx);
        // gerdr, src/git.rs, fn alpha(), x, y, z, fn beta(), alpha, gamma.
        press(&mut panel, ctrl_shift(), Keysym::F, &ctx);
        type_text(&mut panel, "alpha", &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 2");
        assert_eq!(cursor_row(&panel), Some(2));
        assert_eq!(panel.diff.found_on(2), vec![(3..8, true)]);
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(cursor_row(&panel), Some(7));
        // Read again, it stays on the same place.
        panel.set_diffs(root("gerdr"), g, vec![file], &ctx);
        assert_eq!(search_count(&panel, &ctx), "2 of 2");
        assert_eq!(panel.diff.found_on(7), vec![(0..5, true)]);
        // The buttons go back and on; a click on the diff takes the keys
        // from the search, its own field gives them back.
        panel.click(ReposHit::Search(SearchPart::Prev), &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 2");
        panel.click(ReposHit::DiffRow(0), &ctx);
        assert!(!panel.search_has_keys() && panel.diff.search.is_some());
        panel.click(ReposHit::Search(SearchPart::Field), &ctx);
        assert!(panel.search_has_keys());
        panel.click(ReposHit::Search(SearchPart::Close), &ctx);
        assert!(panel.diff.search.is_none());
        panel.click(ReposHit::Search(SearchPart::Open), &ctx);
        assert!(panel.search_has_keys());
    }
}
