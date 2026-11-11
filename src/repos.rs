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
//! Keys: Tab moves from the tree to the detail to the field, Shift+Tab
//! back; Esc steps back to the tree (the message typed kept), then
//! closes. In the tree Up/Down select, Space checks, Left/Right fold,
//! Enter opens a file in Zed or folds a repository or folder, and typing
//! goes to the field; Shift with Up/Down or the pages scrolls the detail.
//! In the diff the arrows and the pages move a cursor over its rows (with
//! Shift, reaching a range back), Alt+Up/Down go to the file before or
//! after, and for the cursor's file Space checks it, Left/Right fold it
//! (on a line, move the caret) and Enter opens it in Zed at the cursor's
//! line (`g` Space and Ctrl+click too, at the caret's char; on a file's
//! header, the file). `c` writes a comment on the range's lines (or edits
//! the one the cursor is on): Shift+Enter breaks its line, Enter or Esc
//! keeps it; Delete deletes it, Ctrl+Shift+C copies the review, and
//! "Clear review" deletes it all, asked first. Alt+1, 2 and 3 show the
//! diff, the log and the stashes. The log lists the commits to pull and
//! to push: the arrows pick one, and Enter or a click shows its diff
//! across the panel, read-only, Esc going back. Ctrl+S opens the branch picker, Ctrl+N
//! it to create a branch (in it, Ctrl+Enter creates the one typed), Alt+Del
//! it to delete one. The actions' chords (`ACTIONS`, which F1 lists with
//! the rest) work with either focus: Ctrl+Enter commits, Ctrl+P pushes,
//! Ctrl+F fetches (in the diff, finds), Ctrl+M merges the default branch
//! in, Alt+D marks a file resolved, Ctrl+R reads them all again (in the
//! diff, redoes), and so on.
//! What can't be taken back (a force push, an abort, an undo, a branch or
//! stash deleted) asks first, on a sheet. On a repository's line, a click
//! on the arrow up pushes it, on the arrow down pulls it (their tooltips
//! list the first commits), and on its stash count lists its stashes; a
//! right click offers the actions.

use std::{
    cmp::Reverse,
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
        LineState, RepoLine, RepoMark, ReposButton, ReposHit, ReposView, ResultBar, RowMark,
        SheetView, Tone, TreeLine, TreeTool,
    },
    diff::FileDiff,
    diff_edit::{FileEdit, Put, Register},
    diff_view::{
        self, DiffView, Keys, Moved, NO_REVIEW, Place, Section, bounds, detail_line, in_dir,
        move_to, showing,
    },
    file_tree::{self, Folds, Row, change_text, file_name_text, file_rows, path_parts, tree_order},
    git::{Change, GitStatus, InProgress},
    git_ops::{Failure, Head, Integrate, OpResult, Pull, Ran, Verdict},
    scroll::{Bounds, Motion, Scroll},
    text_field::{self, Edit, TextField},
    worktree_diff::WorktreeDiff,
};

/// How long the line saying why an action wasn't done stays; one saying
/// what is under way (ending in `…`) stays while it is.
const WHY_LIVES: std::time::Duration = std::time::Duration::from_secs(8);

/// A repository changing this many files or more starts folded.
pub const FOLD_FILES: usize = 200;

/// How many of the branch menu's longest branches it is measured by.
const WIDEST: usize = 3;
/// Why the review can't be sent to an agent.
const NO_AGENT: &str = "No agent runs in this workspace: start one, or copy the review";

/// The panel, while open, and the batch running or last run.
#[derive(Default)]
pub struct Repos {
    pub(crate) panel: Option<Panel>,
    pub(crate) batch: Option<Batch>,
    /// Batches still running that a later one over other repositories
    /// took the panel's result bar from: told in a toast once done.
    pub(crate) background: Vec<Batch>,
    /// The flags that stop each batch running, by id.
    pub(crate) stops: HashMap<u64, std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
    /// The same of a feature's code review, whose diff wants more of the
    /// room; `None` for its default.
    pub(crate) review_tree_share: Option<f64>,
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
    /// The first commits to push (`true`) or pull of each repository, for
    /// the tooltips on its marks, by root: read when first hovered, read
    /// again once its counts or HEAD moved.
    pub(crate) mark_commits: HashMap<(PathBuf, bool), MarkCommits>,
    /// The name and email dialog, while open (`crate::git_identity`).
    pub(crate) identity: Option<crate::git_identity::IdentityDialog>,
    /// What was typed in it and where to keep it (every repository or
    /// not) when last closed unsaved, for the next opening.
    pub(crate) identity_draft: Option<(crate::git_ops::Identity, bool)>,
}

/// A repository's first commits to push or pull (`Repos::mark_commits`),
/// as read at its HEAD and counts `at`; none while read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkCommits {
    pub at: MarkAt,
    pub commits: Option<Vec<(String, String)>>,
}

/// What a repository's commits to push or pull are read at: its HEAD and
/// its counts to push and to pull.
pub type MarkAt = (Option<String>, u32, u32);

impl Repos {
    /// The panel, or a worktree's diff in its place, is open.
    pub fn is_open(&self) -> bool {
        self.panel.is_some() || self.worktree.is_some()
    }

    /// Close it for good, as when leaving the project: the edits made
    /// from the diff can't be undone after.
    pub fn close(&mut self) {
        self.panel = None;
        self.kept = None;
        self.cuts.clear();
        self.undone.clear();
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
    /// How many lines a sheet shows at once (`chrome::repos_sheet_rows`).
    pub sheet_rows: usize,
    /// Batches still running in the background: started before `batch`,
    /// over other repositories.
    pub background: &'a [Batch],
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
    pub(crate) fn other(self) -> Grouping {
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
    pub(crate) folded_dirs: Folds,
    /// The repositories that had something to do while a batch ran:
    /// they stay with the others, under no `Clean` heading, so that a
    /// line doesn't jump there as a batch commits and pushes it. Those
    /// with nothing left to do go there once no batch runs.
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
    /// The tab asked for with Alt+1 / 2 / 3, else the diff.
    pub(crate) tab: Option<DetailTab>,
    /// The log or stashes read for, and what they say.
    pub(crate) detail: Option<(DetailKey, String)>,
    /// The diff tab: every change of every repository, one diff.
    pub(crate) diff: DiffView,
    pub(crate) hover: Option<ReposHit>,
    /// Why the last action wasn't done, under the field.
    pub(crate) error: Option<String>,
    /// That line as first shown, and when: it goes after `WHY_LIVES`.
    pub(crate) error_shown: std::cell::RefCell<Option<(String, SystemTime)>>,
    pub(crate) sheet: Option<Sheet>,
    /// Where the selected line was among the tree's lines: what is
    /// selected once it is gone.
    pub(crate) selected_at: usize,
    /// The detail was read before the last Git status read: it is read
    /// again, still shown meanwhile.
    pub(crate) detail_stale: bool,
    /// The branch picker's menu, while open.
    pub(crate) branch_menu: Option<BranchMenu>,
    /// The files edited from the diff since the panel opened, by (root,
    /// path): marked in the tree, while `u` can undo it.
    pub(crate) edited: HashSet<(PathBuf, String)>,
    /// The stash picked in the Stashes tab, by line (0 the newest).
    pub(crate) stash_pick: usize,
    /// The commit each line of the log or stashes shows, by line; none for the
    /// lines saying what a list is.
    pub(crate) log_oids: Vec<Option<String>>,
    /// The commit picked in the Log tab, by line.
    pub(crate) log_pick: usize,
    /// What the agent the review would be sent to is called; `None` while
    /// none runs in its workspaces. Kept up by the host.
    pub(crate) agent: Option<String>,
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
    /// Delete it, where it is a local branch (asked first).
    Delete,
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
pub(crate) fn standing(scroll: &Scroll, len: usize, page: usize) -> (usize, f64) {
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

/// A question over the panel. Each keeps the button Enter presses, by
/// index into `sheet_buttons`.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    /// Before `kind` over `jobs`, which can't be taken back: what it does
    /// to each repository, and the button Enter presses: 0 cancels, 1
    /// does it.
    Confirm {
        kind: BatchKind,
        jobs: Vec<Job>,
        lines: Vec<(String, String, Tone)>,
        choice: usize,
    },
    /// The panel's keys, from line `top` on.
    Help { choice: usize, top: usize },
    /// Before dropping the review's comments; only those on lines no
    /// longer in the diff when `outdated`.
    ClearReview { choice: usize, outdated: bool },
    /// What the check before `kind` found, per repository in `jobs`.
    Preflight {
        kind: BatchKind,
        jobs: Vec<Job>,
        verdicts: Vec<(PathBuf, Verdict)>,
        /// The button Enter presses: 0 Cancel, 1 Clean only, 2 All.
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
    /// Amend the last commit with the job's files (none to only reword
    /// it), its message replaced by `msg` if there is one; `pushed` when
    /// the commit is on an upstream already, so it is asked first.
    Amend {
        msg: Option<String>,
        pushed: bool,
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
    /// Bring stash `n` (0 the newest) back, and drop it.
    StashPop(usize),
    /// Drop stash `n` (0 the newest).
    StashDrop(usize, crate::git_ops::StashSeen),
    /// Throw away the uncommitted changes to the job's files.
    Discard,
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
            BatchKind::Amend { .. } => "Amend last commit",
            BatchKind::Push { force: false } => "Push",
            BatchKind::Push { force: true } => "Force push",
            BatchKind::Fetch => "Fetch",
            BatchKind::Pull(Pull::Rebase) => "Pull (rebase)",
            BatchKind::Pull(_) => "Pull",
            BatchKind::Integrate { how, from } => {
                let from = from.as_ref().map(|names| names.join(" / "));
                return match (how, from) {
                    (Integrate::Merge, None) => "Merge default branch in".into(),
                    (Integrate::Rebase, None) => "Rebase onto default branch".into(),
                    (Integrate::Merge, Some(from)) => format!("Merge {from}"),
                    (Integrate::Rebase, Some(from)) => format!("Rebase onto {from}"),
                };
            }
            BatchKind::MarkResolved => "Mark resolved",
            BatchKind::Continue => "Continue",
            BatchKind::Abort => "Abort",
            BatchKind::Undo => "Undo",
            BatchKind::Stash => "Stash",
            BatchKind::StashPop(_) => "Pop stash",
            BatchKind::StashDrop(..) => "Drop stash",
            BatchKind::Discard => "Discard changes",
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
            BatchKind::Amend { .. } => "amended",
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
            BatchKind::StashPop(_) => "popped",
            BatchKind::StashDrop(..) => "dropped",
            BatchKind::Discard => "discarded",
        }
    }

    /// Whether it can't be taken back, so is asked about first: a force
    /// push, an abort, an undo, a branch or a stash dropped, a pushed
    /// commit amended.
    pub fn confirmed_first(&self) -> bool {
        matches!(
            self,
            BatchKind::Push { force: true }
                | BatchKind::Amend { pushed: true, .. }
                | BatchKind::Abort
                | BatchKind::Undo
                | BatchKind::DeleteBranch { .. }
                | BatchKind::StashDrop(..)
                | BatchKind::Discard
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
                | BatchKind::Amend { .. }
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
    /// When it started: how long it has run shows while it does.
    pub started: SystemTime,
    /// The user asked it to stop: its commands are killed, the rest not
    /// run.
    pub stopping: bool,
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
        if going > 0 && self.stopping {
            parts.push("stopping…".into());
        } else if going > 0 {
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

    /// Whether it still runs in the repository at `root`.
    pub fn runs_in(&self, root: &Path) -> bool {
        self.run(root).is_some_and(|r| !r.state.over())
    }
}

/// How long since `then`, at `now`, in words: `12s`, `2m 5s`.
pub fn took(then: SystemTime, now: SystemTime) -> String {
    let s = now.duration_since(then).map_or(0, |d| d.as_secs());
    match s {
        0..60 => format!("{s}s"),
        _ => format!("{}m {}s", s / 60, s % 60),
    }
}

/// Why a batch over the repositories at `roots` must wait, if one of them
/// is in a batch still running: which, and where.
pub fn busy<'a>(roots: impl IntoIterator<Item = &'a Path>, ctx: &Ctx) -> Option<String> {
    let running = ctx.batch.into_iter().chain(ctx.background);
    let batches: Vec<&Batch> = running.filter(|b| !b.finished()).collect();
    roots.into_iter().find_map(|root| {
        let batch = batches.iter().find(|b| b.runs_in(root))?;
        let label = (ctx.items.iter())
            .find(|i| i.root == root)
            .map_or_else(|| root.display().to_string(), |i| i.label.clone());
        let what = batch.kind.label().to_lowercase();
        Some(format!(
            "Wait for the {what} running in {label} (Alt+C stops it)"
        ))
    })
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
    /// When it started running there.
    pub since: Option<SystemTime>,
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
    /// The first commits to push (`ahead`) or pull of the repository at
    /// `root`, read at `at`.
    MarkCommits {
        root: PathBuf,
        ahead: bool,
        at: MarkAt,
        commits: Vec<(String, String)>,
    },
    Detail {
        key: DetailKey,
        text: String,
        /// The Log or Stashes tab's commit of each line (`Panel::log_oids`).
        oids: Vec<Option<String>>,
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
    /// A feature's touched repositories read for its code review
    /// (`feature_diff`).
    FeatureDiff(Box<crate::feature_diff::Read>),
    /// The name and email guessed for opening `serial` of the identity
    /// dialog.
    IdentityGuess {
        serial: u64,
        guess: crate::git_ops::Identity,
    },
    /// The identity dialog's name and email written, or why not.
    IdentitySaved {
        serial: u64,
        result: Result<(), String>,
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
    /// Ask for the name and email git wants for commits, where the last
    /// batch failed for want of them; then run it again there.
    SetIdentity,
    /// Put the review, as Markdown, on the clipboard: `count` comments.
    Copy {
        text: String,
        count: usize,
    },
    /// Send the review, as Markdown, to the agent of its workspace:
    /// `count` comments.
    SendReview {
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
    Yank(Register),
    /// Put what was yanked or deleted last in a file.
    Paste(Put),
    /// Undo the last cut in the file (by root and path; with none, the
    /// last anywhere); do the last undone there again.
    UndoCut(Option<(PathBuf, String)>),
    RedoCut(Option<(PathBuf, String)>),
    /// Show what commit `oid` (its log line `line`) of the repository at
    /// `root` changed.
    OpenCommit {
        root: PathBuf,
        oid: String,
        line: String,
    },
    /// Read the repositories' status and diffs again.
    Refresh,
    /// Stop the batches running: their commands killed, the rest not run.
    Stop,
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
            note: None,
            selected: selected_file(&$panel.selected),
            read_only: false,
            no_edits: false,
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
            head: i.status.head.as_deref(),
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

/// The detail tab Alt and a digit shows: 1 the diff, 2 the log, 3 the
/// stashes.
fn tab_key(sym: Keysym) -> Option<DetailTab> {
    match sym {
        Keysym::_1 | Keysym::KP_1 => Some(DetailTab::Diff),
        Keysym::_2 | Keysym::KP_2 => Some(DetailTab::Log),
        Keysym::_3 | Keysym::KP_3 => Some(DetailTab::Stashes),
        _ => None,
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
        D::UndoCut(file) => Outcome::UndoCut(file),
        D::RedoCut(file) => Outcome::RedoCut(file),
        D::Toggle { .. } | D::Clear | D::ClearOutdated => Outcome::Notice("Done in the panel"),
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

/// A chord: the modifiers held, exactly, and the symbols it is pressed
/// with (both cases of a letter: Shift changes the symbol).
#[derive(Clone, Copy, Debug)]
struct Chord {
    ctrl: bool,
    shift: bool,
    alt: bool,
    syms: &'static [Keysym],
}

impl Chord {
    /// Whether `sym` with `mods` is it.
    fn matches(&self, mods: &ModifiersState, sym: Keysym) -> bool {
        !mods.logo
            && (mods.ctrl, mods.shift, mods.alt) == (self.ctrl, self.shift, self.alt)
            && self.syms.contains(&sym)
    }
}

const fn ctrl(syms: &'static [Keysym]) -> Chord {
    Chord {
        ctrl: true,
        shift: false,
        alt: false,
        syms,
    }
}

const fn ctrl_shift(syms: &'static [Keysym]) -> Chord {
    Chord {
        shift: true,
        ..ctrl(syms)
    }
}

const fn alt(syms: &'static [Keysym]) -> Chord {
    Chord {
        ctrl: false,
        shift: false,
        alt: true,
        syms,
    }
}

const fn alt_shift(syms: &'static [Keysym]) -> Chord {
    Chord {
        shift: true,
        ..alt(syms)
    }
}

const ENTER: &[Keysym] = &[Keysym::Return, Keysym::KP_Enter];
const DELETE: &[Keysym] = &[Keysym::Delete, Keysym::KP_Delete];

/// An action of the panel with a chord of its own, pressed with either
/// focus (before the field, as Ctrl+Shift+Delete would otherwise delete a
/// word there): the button it presses, its chord, the key as the footer
/// shows it, and what it does as the keys sheet (F1) says. `action_key`,
/// the footer's keys and the sheet all read it.
struct Action {
    button: ReposButton,
    chord: Chord,
    key: &'static str,
    what: &'static str,
}

/// The panel's actions with a chord, as F1 lists them.
const ACTIONS: &[Action] = {
    use ReposButton as B;
    &[
        Action {
            button: B::Commit,
            chord: ctrl(ENTER),
            key: "Ctrl+Enter",
            what: "commit the checked files with the message",
        },
        Action {
            button: B::CommitPush,
            chord: ctrl_shift(ENTER),
            key: "Ctrl+Shift+Enter",
            what: "commit them, then push",
        },
        Action {
            button: B::Amend,
            chord: alt(&[Keysym::a, Keysym::A]),
            key: "Alt+A",
            what: "amend the last commit with the checked files; a message typed replaces its own",
        },
        Action {
            button: B::Push,
            chord: ctrl(&[Keysym::p, Keysym::P]),
            key: "Ctrl+P",
            what: "push the checked repositories, else the selected",
        },
        Action {
            button: B::ForcePush,
            chord: ctrl_shift(&[Keysym::p, Keysym::P]),
            key: "Ctrl+Shift+P",
            what: "force push them, with lease (asked first)",
        },
        Action {
            button: B::Fetch,
            chord: ctrl(&[Keysym::f, Keysym::F]),
            key: "Ctrl+F",
            what: "fetch the checked, else all; in the diff, find",
        },
        Action {
            button: B::Pull,
            chord: ctrl(&[Keysym::l, Keysym::L]),
            key: "Ctrl+L",
            what: "pull the checked, else the selected",
        },
        Action {
            button: B::Merge,
            chord: ctrl(&[Keysym::m, Keysym::M]),
            key: "Ctrl+M",
            what: "merge the default branch into the branch checked out",
        },
        Action {
            button: B::Rebase,
            chord: ctrl_shift(&[Keysym::m, Keysym::M]),
            key: "Ctrl+Shift+M",
            what: "rebase the branch checked out onto the default branch",
        },
        Action {
            button: B::MergeFrom,
            chord: alt(&[Keysym::m, Keysym::M]),
            key: "Alt+M",
            what: "merge a branch picked in the menu into the one checked out",
        },
        Action {
            button: B::RebaseFrom,
            chord: alt_shift(&[Keysym::m, Keysym::M]),
            key: "Alt+Shift+M",
            what: "rebase the branch checked out onto one picked",
        },
        Action {
            button: B::OpenInZed,
            chord: ctrl(&[Keysym::e, Keysym::E]),
            key: "Ctrl+E",
            what: "open the file, or the conflicts, in Zed",
        },
        Action {
            button: B::MarkResolved,
            chord: alt(&[Keysym::d, Keysym::D]),
            key: "Alt+D",
            what: "mark the conflicted file resolved (on a repository, all)",
        },
        Action {
            button: B::Continue,
            chord: ctrl(&[Keysym::g, Keysym::G]),
            key: "Ctrl+G",
            what: "continue the merge or rebase stopped on conflicts",
        },
        Action {
            button: B::Abort,
            chord: ctrl_shift(DELETE),
            key: "Ctrl+Shift+Del",
            what: "abort it, back to before it (asked first)",
        },
        Action {
            button: B::Undo,
            chord: alt(&[Keysym::z, Keysym::Z]),
            key: "Alt+Z",
            what: "undo the last batch (asked first)",
        },
        Action {
            button: B::RetryFailed,
            chord: alt(&[Keysym::r, Keysym::R]),
            key: "Alt+R",
            what: "retry what failed in the last batch",
        },
        Action {
            button: B::Stash,
            chord: alt(&[Keysym::s, Keysym::S]),
            key: "Alt+S",
            what: "stash the selected repository's changes",
        },
        Action {
            button: B::StashPop,
            chord: alt_shift(&[Keysym::s, Keysym::S]),
            key: "Alt+Shift+S",
            what: "pop the selected repository's newest stash",
        },
        Action {
            button: B::DeleteBranchMenu,
            chord: alt(DELETE),
            key: "Alt+Del",
            what: "delete a local branch picked in the menu (asked first)",
        },
        Action {
            button: B::Stop,
            chord: alt(&[Keysym::c, Keysym::C]),
            key: "Alt+C",
            what: "stop the batches running: their git commands are killed",
        },
        Action {
            button: B::CopyReview,
            chord: ctrl_shift(&[Keysym::c, Keysym::C]),
            key: "Ctrl+Shift+C",
            what: "copy the review comments for an AI agent",
        },
        Action {
            button: B::Refresh,
            chord: ctrl(&[Keysym::r, Keysym::R]),
            key: "Ctrl+R",
            what: "read the repositories again; in the diff, redo as vim's",
        },
        Action {
            button: B::Help,
            chord: Chord {
                ctrl: false,
                shift: false,
                alt: false,
                syms: &[Keysym::F1],
            },
            key: "F1",
            what: "list the keys",
        },
    ]
};

/// The panel's other keys, taken where the focus is, as F1 lists them
/// before the actions: the key, what it does.
const FOCUS_KEYS: &[(&str, &str)] = &[
    (
        "Tab / Shift+Tab",
        "the tree, the diff, the field; the other way",
    ),
    (
        "Esc",
        "back a step: the field or the diff to the tree, then close",
    ),
    (
        "Up / Down, Ctrl+K / J",
        "select a line; in the diff, move the cursor",
    ),
    ("PgUp / PgDn", "a page of lines"),
    (
        "Shift+Up / Down",
        "scroll the diff or log; in the diff, reach a range",
    ),
    ("Space", "check the line, or the diff's file"),
    ("Ctrl+A", "check everything, or nothing"),
    (
        "Left / Right",
        "in the tree, fold or unfold the line; in the diff, move the caret (on a header, fold it)",
    ),
    (
        "Enter",
        "on a file, open it in Zed; on a repository or folder, fold it",
    ),
    ("Typing", "in the tree, starts the commit message"),
    (
        "Del",
        "in the tree, discard a file's or folder's changes (asked first)",
    ),
    (
        "Up / Down, Enter, Del",
        "in the Stashes tab, pick a stash, pop it, drop it (asked first)",
    ),
    (
        "Up / Down, Enter",
        "in the Log tab, pick a commit to pull or push, show its diff (Esc back)",
    ),
    ("Alt+F", "list the files by folder, or flat"),
    ("Alt+Left / Alt+Right", "fold everything, unfold everything"),
    (
        "Alt+1 / 2 / 3, Ctrl+D",
        "the diff, the log, the stashes; Ctrl+D the diff (in it, half a page)",
    ),
    ("Ctrl+S", "switch them all to a branch picked in the menu"),
    ("Ctrl+N", "create a branch: type its name"),
    ("Ctrl+Enter", "in the branch menu, create the branch typed"),
    ("Del", "in the branch menu, delete the branch picked"),
    ("Alt+Up / Down", "in the diff, the file before or after"),
    (
        "] / [",
        "in the diff, go to the next or the previous finding or comment",
    ),
    (
        "c / Del Del",
        "in the diff, comment on the lines (on a file's header, the file) or edit the comment; delete it",
    ),
    (
        "Enter / Shift+Enter",
        "in a comment, keep it; a new line (Esc keeps it too)",
    ),
    (
        "g Space / Ctrl+click",
        "in the diff, open the code in Zed at the caret; on a file's name, the file",
    ),
    (
        "/ / Ctrl+F",
        "in the diff, find its lines, paths, hunks; capitals match case (Ctrl+Shift+F from anywhere)",
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
    (
        "gg G { } Ctrl+U Ctrl+D",
        "in the diff, the top, the end, the hunk before or after, half a page",
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
        "Ctrl+C",
        "in the diff, copy what the cursor is on: the selection, a line, a file's path, a hunk",
    ),
    (
        "Shift+R",
        "in the diff, restore the change at the caret or the lines selected",
    ),
    (
        "u / U or Ctrl+R",
        "undo the last edit to this file, do it again",
    ),
    ("Right click", "the actions, on a line or anywhere"),
];

/// The keys of a worktree's diff (`worktree_diff`), as its keys sheet
/// lists them: the diff's own, and Esc.
pub fn diff_help_lines() -> Vec<(String, String, Tone)> {
    let diff = FOCUS_KEYS
        .iter()
        .filter(|(_, what)| what.starts_with("in the diff") || what.starts_with("in a comment"));
    let shared = FOCUS_KEYS
        .iter()
        .filter(|(key, _)| matches!(*key, "PgUp / PgDn" | "* / #" | "Right click"));
    [
        ("Esc", "close the diff, back to the worktrees"),
        ("F1", "list the keys"),
    ]
    .iter()
    .chain(diff)
    .chain(shared)
    .chain([("Ctrl+Shift+C", "copy the review comments for an AI agent")].iter())
    .map(|(key, what)| (key.to_string(), what.to_string(), Tone::Normal))
    .collect()
}

/// The action a key asks for, with either focus.
fn action_key(mods: &ModifiersState, sym: Keysym) -> Option<ReposButton> {
    ACTIONS
        .iter()
        .find(|a| a.chord.matches(mods, sym))
        .map(|a| a.button)
}

/// Whether `item` is stopped on conflicts: an operation half-way, or
/// conflicted files.
fn in_conflict(item: &RepoItem) -> bool {
    item.status.op.is_some() || item.status.changes.iter().any(Change::conflicted)
}

/// `button`'s key, as the footer and the diff's chips show it: its
/// chord's, else the key that presses it where it is; empty for those
/// without one.
pub(crate) fn key_name(button: ReposButton) -> &'static str {
    use ReposButton as B;
    if let Some(action) = ACTIONS.iter().find(|a| a.button == button) {
        return action.key;
    }
    match button {
        B::Comment => "c",
        B::EditFix => "Shift+E",
        B::CopyLines => "Ctrl+C",
        B::CommentFile => "Shift+C",
        B::NextComment => "]",
        B::PrevComment => "[",
        B::Restore => "Shift+R",
        B::SaveComment => "Enter",
        B::DiscardFile => "Del",
        B::ShowCommit => "Enter",
        B::All | B::CleanOnly | B::PullRebase | B::PullMerge => "Enter",
        B::SkipDiverged | B::Cancel | B::Close | B::EditName => "Esc",
        _ => "",
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
        BatchKind::Amend { .. } => ReposButton::Amend,
        BatchKind::Abort => ReposButton::Abort,
        BatchKind::Undo => ReposButton::Undo,
        BatchKind::DeleteBranch { .. } => ReposButton::DeleteBranch,
        BatchKind::StashDrop(..) => ReposButton::StashDrop,
        BatchKind::Discard => ReposButton::DiscardFile,
        _ => ReposButton::Cancel,
    }
}

/// The buttons `sheet` has, in order: what backs out on the left, what
/// goes ahead on the right.
fn sheet_buttons(sheet: &Sheet) -> Vec<ReposButton> {
    match sheet {
        Sheet::Preflight { .. } => vec![
            ReposButton::Cancel,
            ReposButton::CleanOnly,
            ReposButton::All,
        ],
        Sheet::Diverged { .. } => vec![
            ReposButton::SkipDiverged,
            ReposButton::PullMerge,
            ReposButton::PullRebase,
        ],
        Sheet::Confirm { kind, .. } => vec![ReposButton::Cancel, confirm_button(kind)],
        Sheet::Help { .. } => vec![ReposButton::Close],
        Sheet::ClearReview {
            outdated: false, ..
        } => {
            vec![ReposButton::Cancel, ReposButton::ClearReview]
        }
        Sheet::ClearReview { outdated: true, .. } => {
            vec![ReposButton::Cancel, ReposButton::DropOutdated]
        }
        // Created where missing only when some are.
        Sheet::Exists { jobs, have, .. } => {
            let mut buttons = vec![ReposButton::EditName];
            if !missing(jobs, have).is_empty() {
                buttons.push(ReposButton::CreateMissing);
            }
            buttons.push(ReposButton::SwitchExisting);
            buttons
        }
    }
}

/// Whether the button on the right of `sheet` loses what can't be got
/// back (a force push, a branch or stash dropped, the review deleted):
/// it is painted in the error colour, and Enter means Cancel until the
/// user moves to it.
fn sheet_danger(sheet: &Sheet) -> bool {
    match sheet {
        Sheet::Confirm { .. } | Sheet::ClearReview { .. } => true,
        Sheet::Preflight { kind, .. } => matches!(kind, BatchKind::DeleteBranch { .. }),
        Sheet::Help { .. } | Sheet::Diverged { .. } | Sheet::Exists { .. } => false,
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

/// Scroll a list of `len` lines with `rows` in view, standing at line
/// `top`, by `sym`: the arrows a line, the pages a page, Home and End to
/// the ends. Whether `sym` was one of those.
pub fn scroll_lines(top: &mut usize, len: usize, rows: usize, sym: Keysym) -> bool {
    let last = len.saturating_sub(rows.max(1));
    let page = rows.max(2) - 1;
    *top = match sym {
        Keysym::Down | Keysym::KP_Down => *top + 1,
        Keysym::Up | Keysym::KP_Up => top.saturating_sub(1),
        Keysym::Page_Down | Keysym::KP_Page_Down => *top + page,
        Keysym::Page_Up | Keysym::KP_Page_Up => top.saturating_sub(page),
        Keysym::Home | Keysym::KP_Home => 0,
        Keysym::End | Keysym::KP_End => last,
        _ => return false,
    }
    .min(last);
    true
}

/// The keys sheet's title, `title`, saying where it stands when its
/// `len` lines are scrolled to line `top`.
pub fn help_title(title: &str, top: usize, len: usize) -> String {
    match top {
        0 => format!("{title} · Up/Down for more"),
        _ => format!("{title} · from {} of {len}", top + 1),
    }
}

/// Every key of the panel, as the keys sheet (F1) lists them: those
/// taken where the focus is, then the actions'.
fn help_lines() -> Vec<(String, String, Tone)> {
    let focus = FOCUS_KEYS.iter().map(|&(key, what)| (key, what));
    let actions = ACTIONS.iter().map(|a| (a.key, a.what));
    focus
        .chain(actions)
        .map(|(key, what)| (key.to_owned(), what.to_owned(), Tone::Normal))
        .collect()
}

/// `n` repositories, in words.
fn repos(n: usize) -> String {
    if n == 1 {
        "1 repository".into()
    } else {
        format!("{n} repositories")
    }
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
        panel.stir(items, false);
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

    /// Whether `item` goes under the `Clean` heading: it has nothing to
    /// do, and had nothing while the batch running now ran.
    fn is_clean(&self, item: &RepoItem) -> bool {
        Panel::quiet(item) && !self.stirred.contains(&item.root)
    }

    /// Remember the repositories of `items` with something to do; with no
    /// batch `running`, forget those with nothing left to do, so that
    /// they go under the `Clean` heading.
    fn stir(&mut self, items: &[RepoItem], running: bool) {
        if !running {
            self.stirred.clear();
        }
        let busy = items.iter().filter(|i| !Panel::quiet(i));
        self.stirred.extend(busy.map(|i| i.root.clone()));
    }

    /// The path of folder line `row`.
    fn dir_path(row: Row, items: &[RepoItem]) -> Option<&str> {
        let Row::Dir { k, .. } = row else {
            return None;
        };
        file_tree::dir_path(row, &items[k].status.changes)
    }

    /// Whether `row`, a folder's line, shows its files; true of any other.
    fn dir_open(&self, row: Row, items: &[RepoItem]) -> bool {
        match (Panel::dir_path(row, items), row.item()) {
            (Some(path), Some(k)) => self.folded_dirs.open(&items[k].root, path),
            _ => true,
        }
    }

    /// Show or hide the files of folder line `row`.
    fn set_dir_open(&mut self, row: Row, items: &[RepoItem], open: bool) {
        let (Some(path), Some(k)) = (Panel::dir_path(row, items), row.item()) else {
            return;
        };
        self.folded_dirs.set_open(&items[k].root, path, open);
    }

    /// Unfold the folders the file at `path` of the repository at `root`
    /// is in.
    fn reveal(&mut self, root: &Path, path: &str) {
        self.folded_dirs.reveal(root, path);
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

    /// Select the line of the repository at `root`, in view and with the
    /// keys on the tree: the clean ones' heading unfolded when it is under
    /// it, the diff on it even when it was selected already.
    pub fn select_repo(&mut self, root: &Path, ctx: &Ctx) {
        let items = ctx.items;
        let Some(k) = items.iter().position(|item| item.root == root) else {
            return;
        };
        if self.is_clean(&items[k]) {
            self.clean_folded = false;
        }
        let rows = self.rows(items);
        let Some(r) = rows.iter().position(|row| *row == Row::Repo(k)) else {
            return;
        };
        self.focus = Focus::Tree;
        self.selected = None;
        self.select(&rows, r, items, ctx.page.0.max(1));
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
                            self.folded_dirs.set_open(&item.root, path, false);
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

    /// The stash of `item` a pop or drop takes: the one picked in the
    /// Stashes tab while it shows, else the newest.
    fn stash_at(&self, item: &RepoItem) -> usize {
        match self.tab() {
            DetailTab::Stashes => self
                .stash_pick
                .min(item.status.stashes.saturating_sub(1) as usize),
            _ => 0,
        }
    }

    /// What a drop of `item`'s stash `n` checks before it goes: the
    /// stash's commit as its Stashes list was read, else how many stashes
    /// the last status read counted (no git runs here, on the UI thread).
    fn stash_seen(&self, item: &RepoItem, n: usize) -> crate::git_ops::StashSeen {
        let listed = self.detail.as_ref().filter(|(key, _)| {
            key.tab == DetailTab::Stashes && key.root == item.root
        });
        match listed.and_then(|_| self.log_oids.get(n).cloned().flatten()) {
            Some(oid) => crate::git_ops::StashSeen::Oid(oid),
            None => crate::git_ops::StashSeen::Count(item.status.stashes),
        }
    }

    /// The repository and the files a discard throws the changes of away:
    /// the selected file (both paths of a rename), or every file under
    /// the selected folder; else why none.
    fn discard_files(&self, items: &[RepoItem]) -> Result<(usize, Vec<String>), Why> {
        let pick = "Select a file or folder to discard its changes";
        let k = (self.selected_item(items))
            .and_then(|item| items.iter().position(|i| i.root == item.root))
            .ok_or_else(|| Why::other(pick))?;
        let changes = &items[k].status.changes;
        let taken: Vec<&Change> = match &self.selected {
            Some(Selected::File(_, path)) => changes.iter().filter(|c| c.path == *path).collect(),
            Some(Selected::Dir(_, dir)) => {
                changes.iter().filter(|c| in_dir(&c.path, dir)).collect()
            }
            _ => return Err(Why::other(pick)),
        };
        let files: Vec<String> = taken
            .into_iter()
            .flat_map(|c| [Some(c.path.clone()), c.orig.clone()])
            .flatten()
            .collect();
        if files.is_empty() {
            return Err(Why::other(pick));
        }
        Ok((k, files))
    }

    /// What `button` says in the footer: a merge or rebase names the
    /// branch it brings in, the default one, and the branch it changes,
    /// the one checked out (`Merge main into feat/x`, `Rebase feat/x onto
    /// main`); see `one_name` for several repositories.
    fn button_label(&self, button: ReposButton, ctx: &Ctx) -> String {
        use ReposButton as B;
        let items = ctx.items;
        // How many repositories it runs on, after its name: `commit 2`.
        let over = |what: &str, n: usize| match n {
            0 => what.to_owned(),
            n => format!("{what} {n}"),
        };
        let committing = items
            .iter()
            .filter(|i| i.status.changes.iter().any(|c| self.file_checked(i, c)))
            .count();
        // One repository's push or pull says its commits, as its row's ↑ / ↓
        // marks do; several's, how many repositories.
        let moves = |what: &str, arrow: char, of: fn(&GitStatus) -> u32| {
            let picked = self.targets(items);
            match picked[..] {
                [k] if of(&items[k].status) > 0 => {
                    format!("{what} {arrow}{}", of(&items[k].status))
                }
                [_, _, ..] => format!("{what} {} repos", picked.len()),
                _ => what.to_owned(),
            }
        };
        match button {
            B::Push => moves("push", '\u{2191}', |s| s.ahead),
            B::ForcePush => moves("force push", '\u{2191}', |s| s.ahead),
            B::Pull => moves("pull", '\u{2193}', |s| s.behind),
            B::Commit => over("commit", committing),
            B::CommitPush => over("commit and push", committing.max(self.picked(items).len())),
            B::StashPop | B::StashDrop if self.tab() == DetailTab::Stashes => {
                let verb = if button == B::StashPop { "pop" } else { "drop" };
                let n = self.selected_item(items).map_or(0, |i| self.stash_at(i));
                format!("{verb} stash@{{{n}}}")
            }
            B::DiscardFile => "discard changes…".into(),
            B::SendReview => match &self.agent {
                Some(agent) => format!("send to {agent}"),
                None => "send to agent".into(),
            },
            B::Fetch => match self.picked(items).len() {
                0 => "fetch all".into(),
                n => format!("fetch {n}"),
            },
            B::Merge | B::Rebase | B::MergeFrom | B::RebaseFrom => {
                let base = self.one_name(ctx, |s| s.default_branch.as_deref());
                let base = base.unwrap_or_else(|| "base".into());
                let current = self.current_name(ctx);
                // On the default branch itself, what comes in is its remote's.
                let remote = self.one_name(ctx, |s| s.remote.as_deref());
                let base = match remote {
                    Some(remote) if base == current => (base.split(" / "))
                        .map(|b| format!("{remote}/{b}"))
                        .collect::<Vec<_>>()
                        .join(" / "),
                    _ => base,
                };
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
    /// the one they are all on; to delete, those that can be.
    fn shown_branches(&self, ctx: &Ctx) -> Vec<Branch> {
        let mut branches = self.branches(ctx);
        if self.menu_action() != MenuAction::Switch
            && let Some(current) = self.current_branch(ctx)
        {
            branches.retain(|b| b.names.join(" / ") != current);
        }
        if self.menu_action() == MenuAction::Delete {
            branches.retain(|b| self.delete_jobs(b, ctx).is_ok());
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
        let roots = targets.iter().map(|&k| items[k].root.as_path());
        let running = busy(roots, ctx).is_some();
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
                    .map(|&b| {
                        let deletes = switch || menu.action == MenuAction::Delete;
                        deletes && self.delete_jobs(b, ctx).is_ok()
                    })
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
                } else if menu.action == MenuAction::Delete {
                    "No local branch to delete: not the one checked out".into()
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
                    MenuAction::Delete => "Find a local branch to delete".into(),
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
    fn menu_blocked(&self, ctx: &Ctx) -> Option<String> {
        let targets = self.targets(ctx.items);
        if targets.is_empty() {
            return Some("Select a repository first".into());
        }
        busy(targets.iter().map(|&k| ctx.items[k].root.as_path()), ctx)
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
            self.error = Some(why);
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
            MenuAction::Integrate(_) | MenuAction::Delete => (0.0, 0),
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
        let deletes = switch || self.menu_action() == MenuAction::Delete;
        if deletes && plain && !on_create && matches!(sym, Keysym::Delete | Keysym::KP_Delete) {
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
                if menu.action == MenuAction::Delete {
                    return self.delete_branch(picked.min(len.saturating_sub(1)), ctx);
                }
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
        if let Some(why) = busy(jobs.iter().map(|j| j.root.as_path()), ctx) {
            self.error = Some(why);
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
        // To delete, asked on a sheet over the menu.
        if action == MenuAction::Delete {
            return self.delete_branch(k, ctx);
        }
        self.branch_menu = None;
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
        if let Some(why) = busy(jobs.iter().map(|j| j.root.as_path()), ctx) {
            self.error = Some(why);
            return None;
        }
        self.error = None;
        let kind = match action {
            MenuAction::Switch => BatchKind::Switch(branch.names),
            MenuAction::Integrate(how) => BatchKind::Integrate {
                how,
                from: Some(branch.names),
            },
            MenuAction::Delete => return None,
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
        let jobs = match self.delete_jobs(&branch, ctx) {
            Ok(jobs) => jobs,
            Err(why) => {
                self.error = Some(why);
                return None;
            }
        };
        if let Some(why) = busy(jobs.iter().map(|j| j.root.as_path()), ctx) {
            self.error = Some(why);
            return None;
        }
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
        self.stir(items, ctx.batch.is_some_and(|b| !b.finished()));
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
    /// not: a batch waits for those of its repositories in one still
    /// running. It changes nothing: the footer asks it too.
    fn plan(&self, button: ReposButton, ctx: &Ctx) -> Result<Outcome, Why> {
        let outcome = self.plan_free(button, ctx)?;
        if let Outcome::Batch { jobs, .. } = &outcome
            && let Some(why) = busy(jobs.iter().map(|j| j.root.as_path()), ctx)
        {
            return Err(Why::Other(why));
        }
        Ok(outcome)
    }

    /// What `plan` says, but for the batches running.
    fn plan_free(&self, button: ReposButton, ctx: &Ctx) -> Result<Outcome, Why> {
        use ReposButton as B;
        let items = ctx.items;
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
            B::Amend => {
                let ks = self.targets(items);
                if ks.is_empty() {
                    return Err(Why::other("Select a repository first"));
                }
                let mut jobs = Vec::new();
                let mut pushed = false;
                for k in ks {
                    let item = &items[k];
                    if item.status.head.is_none() {
                        return Err(Why::other(format!("{} has no commit to amend", item.label)));
                    }
                    // On its upstream already: nothing ahead of it.
                    pushed |= item.status.upstream.is_some() && item.status.ahead == 0;
                    let mut files = Vec::new();
                    for change in &item.status.changes {
                        if self.file_checked(item, change) {
                            files.push(change.path.clone());
                            files.extend(change.orig.clone());
                        }
                    }
                    let root = item.root.clone();
                    jobs.push(Job { root, files });
                }
                let msg = Some(self.field.trim().to_owned()).filter(|m| !m.is_empty());
                if msg.is_none() && jobs.iter().all(|j| j.files.is_empty()) {
                    return Err(Why::Field(
                        "Check the files to add, or type the new message first".into(),
                    ));
                }
                Ok(Outcome::Batch {
                    kind: BatchKind::Amend { msg, pushed },
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
                    .unwrap_or_else(|| "Pick the branch in the menu".into()),
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
                        "Resolve {left} conflicted {files} first (Alt+D)"
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
                // The one picked in the Stashes tab, else the newest.
                let n = self.stash_at(item);
                let kind = match button {
                    B::StashPop => BatchKind::StashPop(n),
                    _ => BatchKind::StashDrop(n, self.stash_seen(item, n)),
                };
                let k = items.iter().position(|i| i.root == item.root);
                run(kind, k.into_iter().collect())
            }
            B::DiscardFile => {
                let (k, files) = self.discard_files(items)?;
                if in_conflict(&items[k]) {
                    return Err(Why::other("Resolve the conflicts, or abort, first"));
                }
                Ok(Outcome::Batch {
                    kind: BatchKind::Discard,
                    jobs: vec![Job {
                        root: items[k].root.clone(),
                        files,
                    }],
                })
            }
            B::RunInTerminal => match (selected, failure) {
                (Some(item), Some(Failure::Auth | Failure::Timeout | Failure::Other)) => {
                    Ok(Outcome::RunInTerminal(item.root.clone()))
                }
                _ => Err(Why::other("Nothing to run in a terminal")),
            },
            B::SetIdentity => match failure {
                Some(Failure::Identity) => Ok(Outcome::SetIdentity),
                _ => Err(Why::other("git knows who commits there")),
            },
            B::OpenWorkspace => match failure {
                Some(Failure::CheckedOut(path)) => Ok(Outcome::OpenWorkspace(path)),
                _ => Err(Why::other("Not checked out elsewhere")),
            },
            B::Help => Err(Why::other("Press F1 for the keys")),
            // Shown by `act`, when one is picked.
            B::ShowCommit => Err(Why::other("Pick a commit in the Log tab first")),
            B::Refresh => Ok(Outcome::Refresh),
            B::Stop => {
                let running = ctx.batch.into_iter().chain(ctx.background);
                match running.filter(|b| !b.finished()).count() {
                    0 => Err(Why::other("No batch is running")),
                    _ => Ok(Outcome::Stop),
                }
            }
            // It opens the branch menu (see `act`), when it can.
            B::DeleteBranchMenu => Err(Why::other(
                self.menu_blocked(ctx)
                    .unwrap_or_else(|| "Pick the branch in the menu".into()),
            )),
            B::CopyReview => {
                diff_ctx!(self, ctx, dctx);
                (self.diff.copy_review(&dctx))
                    .map(from_diff)
                    .map_err(Why::other)
            }
            B::SendReview if self.agent.is_none() && !self.diff.review.is_empty() => {
                Err(Why::other(NO_AGENT))
            }
            B::SendReview => {
                diff_ctx!(self, ctx, dctx);
                match self.diff.copy_review(&dctx).map(from_diff) {
                    Ok(Outcome::Copy { text, count }) => Ok(Outcome::SendReview { text, count }),
                    Ok(other) => Ok(other),
                    Err(why) => Err(Why::other(why)),
                }
            }
            B::ClearReview if self.diff.review.is_empty() => Err(Why::other(NO_REVIEW)),
            B::DropOutdated if self.diff.outdated_count() == 0 => {
                Err(Why::other("No comment is on lines no longer in the diff"))
            }
            B::ClearReview | B::DropOutdated => Err(Why::other("Asked on a sheet")),
            B::Comment
            | B::CopyLines
            | B::SaveComment
            | B::DiscardComment
            | B::DeleteComment
            | B::Restore
            | B::NextComment
            | B::PrevComment => Err(Why::other("Done on the diff")),
            // Only a feature stage's file under review has them.
            B::CommentFile
            | B::Approve
            | B::SendBack
            | B::Reopen
            | B::GoOn
            | B::FixFinding
            | B::DismissFinding
            | B::ReopenFinding
            | B::EditFix
            | B::MarkViewed
            | B::NextUnviewed
            | B::AddComment
            | B::FixAllFindings
            | B::PlanFixes
            | B::MakeFixes
            | B::ReviewFixPlan => Err(Why::other("Nothing to review here")),
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
                    BatchKind::StashDrop(n, _) => format!("drop stash@{{{n}}} for good"),
                    BatchKind::Amend { .. } => match status {
                        Some(s) if s.upstream.is_some() && s.ahead == 0 => {
                            "rewrite its last commit, which is pushed: \
                             putting it up again takes a force push"
                                .into()
                        }
                        _ => "rewrite its last commit".into(),
                    },
                    BatchKind::Discard => {
                        let n = job.files.len();
                        let files = if n == 1 { "1 file" } else { "files" };
                        let shown = job.files.iter().take(3).cloned().collect::<Vec<_>>();
                        let more = n.saturating_sub(3);
                        let rest = if more > 0 {
                            format!(" and {more} more")
                        } else {
                            String::new()
                        };
                        format!(
                            "throw away the changes to {files}: {}{rest}",
                            shown.join(", ")
                        )
                    }
                    _ => kind.label().to_lowercase(),
                };
                (label, what, Tone::Warning)
            })
            .collect()
    }

    /// Ask on a sheet before `kind` over `jobs`, which can't be taken
    /// back; Enter cancels until the user moves to the button doing it.
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
        // Said again, the line why shows anew.
        self.error_shown.replace(None);
        if self.sheet.is_some() {
            return self.sheet_press(button, ctx);
        }
        if button == ReposButton::ClearReview {
            if self.diff.review.is_empty() {
                self.error = Some(NO_REVIEW.into());
            } else {
                self.error = None;
                self.branch_menu = None;
                self.sheet = Some(Sheet::ClearReview {
                    choice: 0,
                    outdated: false,
                });
            }
            return None;
        }
        if button == ReposButton::DropOutdated {
            if self.diff.outdated_count() == 0 {
                self.error = Some("No comment is on lines no longer in the diff".into());
            } else {
                self.error = None;
                self.branch_menu = None;
                self.sheet = Some(Sheet::ClearReview {
                    choice: 0,
                    outdated: true,
                });
            }
            return None;
        }
        if button == ReposButton::Help {
            self.branch_menu = None;
            self.sheet = Some(Sheet::Help { choice: 0, top: 0 });
            return None;
        }
        if button == ReposButton::ShowCommit {
            return self.show_commit(ctx);
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
        if button == ReposButton::DeleteBranchMenu {
            self.open_branch_menu(MenuAction::Delete, ctx);
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
            (Sheet::ClearReview { .. }, B::DropOutdated) => {
                self.diff.drop_outdated(&sections(ctx.items));
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
        if let Sheet::Help { top, .. } = sheet
            && scroll_lines(top, help_lines().len(), ctx.sheet_rows, sym)
        {
            return None;
        }
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

    /// Pick stash `n` in the Stashes tab, scrolled to show it on a page
    /// of `page`.
    fn pick_stash(&mut self, n: usize, page: usize) {
        self.stash_pick = n;
        let pos = showing(self.detail_scroll.pos(), (n, n), page);
        move_to(&mut self.detail_scroll, pos);
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
            self.error_shown.replace(None);
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
            diff_view::Outcome::ClearOutdated => self.act(ReposButton::DropOutdated, ctx),
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
        if only_alt && let Some(tab) = tab_key(sym) {
            self.set_tab(tab);
            return None;
        }
        // Ctrl+D shows the diff, as it does in the other popups; with the
        // diff's keys it goes half a page down (vim's), taken above.
        if only_ctrl && matches!(sym, Keysym::d | Keysym::D) {
            self.set_tab(DetailTab::Diff);
            return None;
        }
        let in_diff = self.focus == Focus::Detail && self.tab() == DetailTab::Diff;
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        // Esc steps back: the field (its message kept) or the detail give
        // the keyboard to the tree; from there it closes.
        if sym == Keysym::Escape && plain {
            if self.focus == Focus::Tree {
                return Some(Outcome::Close);
            }
            self.focus = Focus::Tree;
            return None;
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
        // Tab goes round the tree, the detail and the field; Shift+Tab the
        // other way.
        if plain && matches!(sym, Keysym::Tab | Keysym::ISO_Left_Tab) {
            let back = mods.shift || sym == Keysym::ISO_Left_Tab;
            self.focus = match (self.focus, back) {
                (Focus::Tree, false) | (Focus::Field, true) => Focus::Detail,
                (Focus::Detail, false) | (Focus::Tree, true) => Focus::Field,
                (Focus::Field, false) | (Focus::Detail, true) => Focus::Tree,
            };
            return None;
        }
        // In the log, the arrows (and Ctrl+J/K) pick a commit: Enter shows
        // its diff.
        let only_ctrl = mods.ctrl && !(mods.alt || mods.logo || mods.shift);
        let commits = self.log_oids.iter().any(Option::is_some);
        if self.focus == Focus::Detail
            && self.tab() == DetailTab::Log
            && commits
            && (plain || only_ctrl)
        {
            let step = match sym {
                Keysym::Down | Keysym::KP_Down if plain => Some(1),
                Keysym::Up | Keysym::KP_Up if plain => Some(-1),
                Keysym::j | Keysym::J if only_ctrl => Some(1),
                Keysym::k | Keysym::K if only_ctrl => Some(-1),
                _ => None,
            };
            if let Some(step) = step {
                let from = self.log_pick as isize + step;
                let next = (from >= 0)
                    .then(|| self.commit_row(from as usize, step))
                    .flatten();
                match next {
                    Some(r) => self.pick_commit(r, ctx.page.1),
                    // Past the first, the list's own line shows again.
                    None if step < 0 => self.detail_scroll.jump(0.0),
                    None => self.scroll_by(1, true, ctx),
                }
                return None;
            }
            if plain && matches!(sym, Keysym::Return | Keysym::KP_Enter) {
                return self.show_commit(ctx);
            }
        }
        // In the stashes, the arrows pick one: Enter pops it, Del drops
        // it (asked first).
        if self.focus == Focus::Detail && self.tab() == DetailTab::Stashes && plain && !mods.shift {
            let count = self
                .selected_item(ctx.items)
                .map_or(0, |i| i.status.stashes as usize);
            let last = count.saturating_sub(1);
            match sym {
                Keysym::Down | Keysym::KP_Down => {
                    self.pick_stash((self.stash_pick + 1).min(last), ctx.page.1)
                }
                Keysym::Up | Keysym::KP_Up => {
                    self.pick_stash(self.stash_pick.saturating_sub(1), ctx.page.1)
                }
                Keysym::Return | Keysym::KP_Enter => return self.act(ReposButton::StashPop, ctx),
                Keysym::Delete | Keysym::KP_Delete => {
                    return self.act(ReposButton::StashDrop, ctx);
                }
                _ => {}
            }
            if matches!(
                sym,
                Keysym::Down | Keysym::KP_Down | Keysym::Up | Keysym::KP_Up
            ) {
                return None;
            }
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
        let only_ctrl = mods.ctrl && !(mods.shift || mods.alt || mods.logo);
        let moved = match sym {
            Keysym::Down | Keysym::KP_Down => step(1),
            Keysym::Up | Keysym::KP_Up => step(-1),
            // Ctrl+J and Ctrl+K move as Down and Up do, in every list.
            Keysym::j | Keysym::J if only_ctrl => step(1),
            Keysym::k | Keysym::K if only_ctrl => step(-1),
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
            // A file's or folder's changes thrown away, asked first.
            Keysym::Delete | Keysym::KP_Delete if plain && !mods.shift => {
                return self.act(ReposButton::DiscardFile, ctx);
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
                ReposHit::BranchDelete(i)
                    if matches!(self.menu_action(), MenuAction::Switch | MenuAction::Delete) =>
                {
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
        // The log's or stashes' rows are no diff's.
        let other_rows = self.tab() != DetailTab::Diff && matches!(hit, ReposHit::DiffRow(_));
        if !other_rows {
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
                // Laid out only over a feature's code review.
                TreeTool::PrevUnviewed | TreeTool::NextUnviewed => {}
            },
            ReposHit::Field => self.focus = Focus::Field,
            ReposHit::Button(button) => return self.act(button, ctx),
            ReposHit::DetailTab(tab) => self.set_tab(tab),
            ReposHit::DiffRow(i) if self.tab() == DetailTab::Stashes => {
                self.focus = Focus::Detail;
                let (first, _) = standing(&self.detail_scroll, usize::MAX, ctx.page.1);
                let count = self
                    .selected_item(items)
                    .map_or(0, |i| i.status.stashes as usize);
                if first + i < count {
                    self.stash_pick = first + i;
                }
            }
            ReposHit::DiffRow(i) if self.tab() == DetailTab::Log => {
                self.focus = Focus::Detail;
                let (first, _) = standing(&self.detail_scroll, usize::MAX, ctx.page.1);
                if self.log_oids.get(first + i).is_some_and(Option::is_some) {
                    self.log_pick = first + i;
                    return self.show_commit(ctx);
                }
            }
            ReposHit::DiffRow(_)
            | ReposHit::DiffCheck(_)
            | ReposHit::DiffFold(_)
            | ReposHit::DiffRestore(_) => {}
            ReposHit::BranchButton => self.open_branch_menu(MenuAction::Switch, ctx),
            ReposHit::Detail
            | ReposHit::Panel
            | ReposHit::More
            | ReposHit::Mode(_)
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

    /// The press held past the diff's rows, `by` rows on (up negative).
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

    /// A press on row `i` of the diff, if it is on the comment being
    /// written: see `DiffView::press_editor`.
    pub fn press_editor(&mut self, i: usize, extend: bool, count: u32, ctx: &Ctx) -> bool {
        if self.tab() != DetailTab::Diff {
            return false;
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.press_editor(i, extend, count, &dctx)
    }

    /// A press on the comment being written dragged to char `col` of row
    /// `i` of the diff: see `DiffView::drag_editor`.
    pub fn drag_editor(&mut self, i: usize, col: usize, ctx: &Ctx) {
        if self.tab() != DetailTab::Diff {
            return;
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.drag_editor(i, col, &dctx);
    }

    /// A press on the comment being written dragged past the diff's rows:
    /// see `DiffView::drag_editor_past`.
    pub fn drag_editor_past(&mut self, by: i64, col: usize, ctx: &Ctx) {
        if self.tab() != DetailTab::Diff {
            return;
        }
        diff_ctx!(self, ctx, dctx);
        self.diff.drag_editor_past(by, col, &dctx);
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
        if let Some(why) = busy([ctx.items[k].root.as_path()], ctx) {
            self.error = Some(why);
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
                        Failure::Identity => (
                            "author unknown",
                            Tone::Error,
                            "git needs your name and email to commit: set them, then it goes on".into(),
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
                            format!("{batch} failed: see the log (Alt+2)"),
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
            // Running in the background, or in the batch shown.
            let running = ctx.background.iter().find(|b| b.runs_in(&item.root));
            let run = running.or(ctx.batch).and_then(|b| b.run(&item.root));
            let state = match run.map(|r| (&r.state, r.since)) {
                Some((RunState::Running(what), Some(since))) => {
                    LineState::Busy(format!("{what} {}", took(since, ctx.now)))
                }
                Some((RunState::Running(what), None)) => LineState::Busy(what.clone()),
                Some((RunState::Queued, _)) => LineState::Busy("waiting…".into()),
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
        let here = |root: &Path, path: &str| *root == item.root && path == change.path;
        TreeLine::File(FileLine {
            viewed: false,
            edited: self.edited.iter().any(|(root, path)| here(root, path)),
            comments: (self.diff.review.comments.iter())
                .filter(|c| here(&c.root, &c.path))
                .count(),
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
        let behind = ctx.background.iter().filter(|b| !b.finished()).count();
        if behind > 0 {
            parts.push(match behind {
                1 => "1 more batch running".into(),
                n => format!("{n} more batches running"),
            });
        }
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
                Some(Failure::Identity) => buttons.push(B::SetIdentity),
                _ => {}
            }
        }
        if selected.is_some_and(in_conflict) {
            buttons.extend([B::OpenInZed, B::MarkResolved, B::Continue, B::Abort]);
        } else {
            buttons.extend([B::Commit, B::CommitPush, B::Push, B::Amend]);
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
            // A file opened in Zed; its or a folder's changes thrown away,
            // asked first.
            if matches!(self.selected, Some(Selected::File(..))) {
                buttons.push(B::OpenInZed);
            }
            if matches!(self.selected, Some(Selected::File(..) | Selected::Dir(..))) {
                buttons.push(B::DiscardFile);
            }
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
            if self.tab() == DetailTab::Log && self.picked_commit().is_some() {
                buttons.push(B::ShowCommit);
            }
        }
        if !self.diff.review.is_empty() {
            buttons.extend([B::CopyReview, B::SendReview, B::ClearReview]);
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
                    _ if menu_integrate(button).is_some() => self.menu_blocked(ctx),
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
                // A list's own line stands out over its commits.
                let tone = |r: usize, l: &str| match self.log_oids.get(r) {
                    Some(None) if !l.is_empty() => DiffTone::Header,
                    _ => DiffTone::Plain,
                };
                let mut lines: Vec<DetailLine> = text
                    .into_iter()
                    .flat_map(str::lines)
                    .enumerate()
                    .map(|(r, l)| detail_line(l, tone(r, l)))
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
        let stashes = ctx.items[k].status.stashes;
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
            // The stash picked, lit as the diff's cursor is.
            marks: match tab {
                DetailTab::Stashes => (first..first + page + usize::from(part > 0.0))
                    .map(|r| RowMark {
                        selected: r == self.stash_pick && focused && stashes > 0,
                        top: true,
                        bottom: true,
                        noted: false,
                    })
                    .collect(),
                // The commit picked, lit the same way.
                DetailTab::Log => (first..first + page + usize::from(part > 0.0))
                    .map(|r| RowMark {
                        selected: r == self.log_pick && focused && self.picked_commit().is_some(),
                        top: true,
                        bottom: true,
                        noted: false,
                    })
                    .collect(),
                _ => Vec::new(),
            },
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
                    BatchKind::StashDrop(0, _) => "Drop the newest stash".into(),
                    BatchKind::StashDrop(n, _) => format!("Drop stash@{{{n}}}"),
                    BatchKind::Discard => {
                        let n: usize = jobs.iter().map(|j| j.files.len()).sum();
                        let files = if n == 1 { "1 file" } else { "files" };
                        format!("Discard the changes to {files}")
                    }
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
            Sheet::ClearReview { choice, outdated } => {
                diff_ctx!(self, ctx, dctx);
                let (title, lines) = self.diff.clear_sheet(&dctx, *outdated);
                (title, lines, *choice)
            }
            Sheet::Help { choice, top } => {
                let lines = help_lines();
                let top = (*top).min(lines.len().saturating_sub(1));
                let title = help_title("Keys in the repositories panel", top, lines.len());
                (title, lines[top..].to_vec(), *choice)
            }
        };
        let buttons = sheet_buttons(sheet);
        Some(SheetView {
            title,
            lines,
            default: choice.min(buttons.len().saturating_sub(1)),
            buttons,
            danger: sheet_danger(sheet),
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
            let (mut text, tone) = batch.summary();
            let mut buttons = Vec::new();
            // How long it has run, and a button to stop it.
            if !batch.finished() {
                text = format!("{text} · {}", took(batch.started, ctx.now));
                if !batch.stopping {
                    buttons.push(ReposButton::Stop);
                }
            }
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
                        label: button.label().into(),
                        key: key_name(button).into(),
                        disabled: None,
                    })
                    .collect(),
            }
        });
        ReposView {
            tree_note: None,
            title: title.into(),
            summary: self.summary(ctx),
            summary_tone: Tone::Muted,
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
            error: self.why(ctx.now),
            result,
            hints: self.hints(ctx),
            detail: self.detail_view(ctx),
            sheet: self.sheet_view(ctx),
            empty: "No git repositories in this project: add a workspace in a git checkout".into(),
            density: ctx.density,
            tree_share: ctx.tree_share,
            grouping: self.grouping,
            diff_only: false,
            diff_tree: false,
            unchecked: false,
            tree_nav: None,
            flat: false,
            detail_share: 1.0,
            modes: None,
        }
    }

    /// The line saying why the last action wasn't done, at `now`: gone
    /// `WHY_LIVES` after it was first shown, but one saying what is under
    /// way.
    fn why(&self, now: SystemTime) -> Option<String> {
        let mut shown = self.error_shown.borrow_mut();
        let Some(error) = self.error.as_ref() else {
            *shown = None;
            return None;
        };
        let since = match &*shown {
            Some((text, at)) if text == error => *at,
            _ => {
                *shown = Some((error.clone(), now));
                now
            }
        };
        let old = now.duration_since(since).is_ok_and(|d| d >= WHY_LIVES);
        (!old || error.ends_with('…')).then(|| error.clone())
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
    pub fn set_detail(&mut self, key: DetailKey, text: String, oids: Vec<Option<String>>) {
        if self.detail_key().as_ref() != Some(&key) {
            return;
        }
        let picked = self.picked_commit().map(str::to_owned);
        if self.detail.as_ref().is_none_or(|(k, _)| *k != key) {
            self.detail_scroll.jump(0.0);
            self.stash_pick = 0;
            self.log_pick = 0;
        }
        self.detail = Some((key, text));
        self.log_oids = oids;
        self.detail_stale = false;
        // The commit picked stays picked where it went; else the first.
        let at = |oid: &str| (self.log_oids.iter()).position(|o| o.as_deref() == Some(oid));
        self.log_pick = match picked.as_deref().and_then(at) {
            Some(r) => r,
            None => self.commit_row(self.log_pick, 1).unwrap_or(0),
        };
    }

    /// The repository of tree line `i` among those shown, by index into
    /// `items`, `page` lines fitting.
    pub fn line_item(&self, i: usize, items: &[RepoItem], page: usize) -> Option<usize> {
        let rows = self.rows(items);
        let (first, part) = standing(&self.tree_scroll, rows.len(), page.max(1));
        let shown = page.max(1) + usize::from(part > 0.0);
        (i < shown).then(|| rows.get(first + i)?.item()).flatten()
    }

    /// The commit picked in the Log tab, while it shows one.
    fn picked_commit(&self) -> Option<&str> {
        self.detail
            .as_ref()
            .filter(|(k, _)| k.tab == DetailTab::Log)?;
        self.log_oids.get(self.log_pick)?.as_deref()
    }

    /// The first line of the log from `from` on showing a commit, going
    /// `step` lines at a time (1 down, -1 up).
    fn commit_row(&self, from: usize, step: isize) -> Option<usize> {
        let mut r = from as isize;
        while r >= 0 && (r as usize) < self.log_oids.len() {
            if self.log_oids[r as usize].is_some() {
                return Some(r as usize);
            }
            r += step;
        }
        None
    }

    /// Pick the commit on log line `r`, scrolled into view.
    fn pick_commit(&mut self, r: usize, page: usize) {
        self.log_pick = r;
        let pos = showing(self.detail_scroll.pos(), (r, r), page);
        move_to(&mut self.detail_scroll, pos);
    }

    /// Show the picked commit's diff.
    fn show_commit(&mut self, ctx: &Ctx) -> Option<Outcome> {
        let item = self.selected_item(ctx.items)?;
        let Some(oid) = self.picked_commit() else {
            self.error = Some("Pick a commit in the Log tab first".into());
            return None;
        };
        let line = self.detail.as_ref()?.1.lines().nth(self.log_pick)?;
        Some(Outcome::OpenCommit {
            root: item.root.clone(),
            oid: oid.to_owned(),
            line: line.to_owned(),
        })
    }

    /// Ask about what the check before `kind` found: Enter runs only the
    /// clean ones when some would conflict (or lose commits), else all.
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
            choice: if conflicts { 1 } else { 2 },
        });
    }

    /// Ask what to do with branch `name`, about to be created in `jobs`,
    /// which those in `have` have already: Enter edits the name.
    pub fn ask_exists(&mut self, name: String, jobs: Vec<Job>, have: Vec<(PathBuf, Found)>) {
        self.branch_menu = None;
        self.sheet = Some(Sheet::Exists {
            name,
            jobs,
            have,
            choice: 0,
        });
    }

    /// Ask which way to pull the repositories at `roots`, which diverged:
    /// Enter rebases.
    pub fn ask_diverged(&mut self, roots: Vec<PathBuf>) {
        self.branch_menu = None;
        self.sheet = Some(Sheet::Diverged { roots, choice: 2 });
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
            sheet_rows: 12,
            background: &[],
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
            // Its heading says who wrote it, not what.
            .filter(|row| !matches!(row, DetailRow::CommentHead { .. }))
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
                DetailRow::CommentHead { .. } => unreachable!("filtered out above"),
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
            since: None,
        }
    }

    fn failed(failure: Failure) -> RunState {
        RunState::Failed {
            failure,
            why: "it went wrong".into(),
        }
    }

    fn batch(kind: BatchKind, runs: Vec<RepoRun>) -> Batch {
        Batch {
            id: 1,
            kind,
            runs,
            started: UNIX_EPOCH + Duration::from_secs(NOW),
            stopping: false,
        }
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
            "Rebase onto default branch"
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
    fn a_repository_asked_for_is_selected_even_under_the_clean_heading() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        panel.clean_folded = true;
        panel.focus = Focus::Detail;
        // herdr, clean, sits under the folded heading: it unfolds.
        panel.select_repo(&root("herdr"), &ctx);
        assert_eq!(panel.selected, Some(Selected::Repo(root("herdr"))));
        assert!(!panel.clean_folded);
        assert_eq!(panel.focus, Focus::Tree);
        // One with changes selects as it is; one not listed changes nothing.
        panel.select_repo(&root("zed"), &ctx);
        assert_eq!(panel.selected, Some(Selected::Repo(root("zed"))));
        panel.select_repo(&root("nowhere"), &ctx);
        assert_eq!(panel.selected, Some(Selected::Repo(root("zed"))));
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
    fn amend_takes_the_checked_files_and_asks_first_when_pushed() {
        let mut items = items();
        // gerdr's last commit isn't up yet.
        items[0].status.ahead = 1;
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select(&mut panel, "gerdr");
        // Nothing to add and no message: it asks for one.
        assert_eq!(press(&mut panel, alt(), Keysym::a, &ctx), None);
        assert_eq!(
            panel.error.as_deref(),
            Some("Check the files to add, or type the new message first")
        );
        let mut panel = Panel::new(&items);
        select_file(&mut panel, "gerdr", "src/git.rs");
        press(&mut panel, none(), Keysym::space, &ctx);
        assert_eq!(
            press(&mut panel, alt(), Keysym::a, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Amend {
                    msg: None,
                    pushed: false,
                },
                jobs: vec![job("gerdr", &["src/git.rs"])],
            })
        );
        // zed's is on its upstream: rewording it is asked about first.
        let mut panel = Panel::new(&items);
        select(&mut panel, "zed");
        panel.field.set("Reworded");
        let kind = BatchKind::Amend {
            msg: Some("Reworded".into()),
            pushed: true,
        };
        assert_eq!(press(&mut panel, alt(), Keysym::a, &ctx), None);
        assert!(matches!(&panel.sheet, Some(Sheet::Confirm { kind: k, .. }) if *k == kind));
        assert_eq!(
            panel.press(ReposButton::Amend, &ctx),
            Some(Outcome::Batch {
                kind,
                jobs: vec![job("zed", &[])],
            })
        );
        assert!(
            BatchKind::Amend {
                msg: None,
                pushed: false
            }
            .undoable()
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
        // Where it runs, a batch waits; elsewhere, it goes ahead.
        let waits = "Wait for the fetch running in herdr (Alt+C stops it)";
        select(&mut panel, "herdr");
        for (m, sym) in [
            (ctrl(), Keysym::p),
            (ctrl(), Keysym::f),
            (ctrl_shift(), Keysym::M),
        ] {
            assert_eq!(press(&mut panel, m, sym, &ctx), None);
            assert_eq!(panel.error.as_deref(), Some(waits));
        }
        assert!(
            panel
                .view("p", &ctx)
                .hints
                .iter()
                .find(|h| h.button == ReposButton::Push)
                .is_some_and(|h| h.disabled.as_deref() == Some(waits))
        );
        select(&mut panel, "gerdr");
        assert_eq!(
            press(&mut panel, ctrl(), Keysym::p, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Push { force: false },
                jobs: jobs(&["gerdr"])
            })
        );
        // Alt+C stops it.
        assert_eq!(
            press(&mut panel, alt(), Keysym::c, &ctx),
            Some(Outcome::Stop)
        );
        let view = panel.view("p", &ctx);
        let lines = repo_lines(&view);
        // herdr, with nothing to do, is under the heading, last.
        assert_eq!(lines[2].label, "herdr");
        assert_eq!(lines[2].state, LineState::Busy("fetching…".into()));
        assert_eq!(lines[0].chip.as_ref().map(|c| c.tone), Some(Tone::Success));
        // How long it has run, and a button to stop it.
        let result = view.result.unwrap();
        assert_eq!(result.text, "Fetch: 1 done · 1 of 2 running · 0s");
        let buttons: Vec<ReposButton> = result.buttons.iter().map(|h| h.button).collect();
        assert_eq!(buttons, [ReposButton::Stop]);
        // A line running a while says for how long.
        let mut long = b.clone();
        long.runs[1].since = Some(UNIX_EPOCH + Duration::from_secs(NOW - 75));
        let ctx = super::Ctx {
            batch: Some(&long),
            ..ctx
        };
        let view = panel.view("p", &ctx);
        let busy = LineState::Busy("fetching… 1m 15s".into());
        assert_eq!(repo_lines(&view)[2].state, busy);
        // Stopping, it says so, and no button stops it again.
        let mut stopping = long.clone();
        stopping.stopping = true;
        let ctx = super::Ctx {
            batch: Some(&stopping),
            ..ctx
        };
        let result = panel.view("p", &ctx).result.unwrap();
        assert_eq!(result.text, "Fetch: 1 done · stopping… · 0s");
        assert!(result.buttons.is_empty());
        // In the background, its line is busy still and the header says
        // so, the result bar another batch's.
        let other = batch(
            BatchKind::Push { force: false },
            vec![run("gerdr", RunState::Done)],
        );
        let ctx = super::Ctx {
            batch: Some(&other),
            background: std::slice::from_ref(&b),
            ..ctx
        };
        let view = panel.view("p", &ctx);
        assert!(view.summary.contains("1 more batch running"));
        assert_eq!(
            repo_lines(&view)[2].state,
            LineState::Busy("fetching…".into())
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
        // Cancel on the left, the push in the error colour on the right;
        // Enter cancels until the push is moved to.
        assert_eq!(sheet.buttons, [ReposButton::Cancel, ReposButton::ForcePush]);
        assert_eq!(sheet.default, 0);
        assert!(sheet.danger);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.sheet.is_none());
        press(&mut panel, ctrl_shift(), Keysym::P, &ctx);
        // The panel's keys wait; Right, then Enter does it.
        assert_eq!(press(&mut panel, ctrl(), Keysym::p, &ctx), None);
        assert_eq!(press(&mut panel, none(), Keysym::Right, &ctx), None);
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
        press(&mut panel, none(), Keysym::Right, &ctx);
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
        assert_eq!(buttons[0].label, "Undo");
        press(&mut panel, none(), Keysym::Tab, &ctx);
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
        assert_eq!(
            sheet.title,
            "Keys in the repositories panel · Up/Down for more"
        );
        assert_eq!(sheet.lines.len(), FOCUS_KEYS.len() + ACTIONS.len());
        assert_eq!(sheet.lines[0].0, "Tab / Shift+Tab");
        assert_eq!(sheet.buttons, [ReposButton::Close]);
        assert_eq!(help.disabled, None);
        // Every action's key is listed, from the table the keys come from.
        for action in ACTIONS {
            assert!(sheet.lines.iter().any(|(key, ..)| key == action.key));
            assert_eq!(key_name(action.button), action.key);
        }
        // It scrolls, no further than its last page.
        press(&mut panel, none(), Keysym::Down, &ctx);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.lines[0].0, FOCUS_KEYS[1].0);
        assert!(
            sheet
                .title
                .ends_with(&format!("from 2 of {}", help_lines().len()))
        );
        press(&mut panel, none(), Keysym::End, &ctx);
        let rest = panel.view("p", &ctx).sheet.expect("a sheet").lines.len();
        assert_eq!(rest, ctx.sheet_rows);
        press(&mut panel, none(), Keysym::Home, &ctx);
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
                kind: BatchKind::StashPop(0),
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
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::StashDrop(0, crate::git_ops::StashSeen::Count(2)),
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
    fn del_discards_a_files_changes_after_a_sheet_asks() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        // On a repository: nothing to pick.
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        assert!(panel.sheet.is_none());
        assert_eq!(
            panel.error.as_deref(),
            Some("Select a file or folder to discard its changes")
        );
        select_file(&mut panel, "gerdr", "src/git.rs");
        let hints: Vec<ReposButton> = panel
            .view("p", &ctx)
            .hints
            .iter()
            .map(|h| h.button)
            .collect();
        assert!(
            hints.contains(&ReposButton::OpenInZed) && hints.contains(&ReposButton::DiscardFile)
        );
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(
            sheet.title,
            "Discard the changes to 1 file in 1 repository?"
        );
        assert!(sheet.danger && sheet.default == 0);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(panel.sheet.is_none());
        press(&mut panel, none(), Keysym::Delete, &ctx);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Discard,
                jobs: vec![job("gerdr", &["src/git.rs"])],
            })
        );
        // A rename takes both its paths.
        select_file(&mut panel, "zed", "new.rs");
        press(&mut panel, none(), Keysym::Delete, &ctx);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::Discard,
                jobs: vec![job("zed", &["new.rs", "old.rs"])],
            })
        );
    }

    #[test]
    fn a_stash_is_picked_in_its_tab_to_pop_or_drop() {
        let mut items = items();
        items[0].status.stashes = 3;
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        press(&mut panel, alt(), Keysym::_3, &ctx);
        let key = panel.detail_wanted(&ctx).unwrap();
        panel.set_detail(
            key,
            "stash@{0} · a\nstash@{1} · b\nstash@{2} · c".into(),
            Vec::new(),
        );
        panel.focus = Focus::Detail;
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        press(&mut panel, none(), Keysym::Down, &ctx);
        let view = panel.view("p", &ctx);
        assert!(view.detail.marks[2].selected);
        let pop = view
            .hints
            .iter()
            .find(|h| h.button == ReposButton::StashPop)
            .unwrap();
        assert_eq!(pop.label, "pop stash@{2}");
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Batch {
                kind: BatchKind::StashPop(2),
                jobs: jobs(&["gerdr"])
            })
        );
        press(&mut panel, none(), Keysym::Up, &ctx);
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(sheet.title, "Drop stash@{1} in 1 repository?");
    }

    #[test]
    fn why_an_action_wasnt_done_goes_after_a_while() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select(&mut panel, "herdr");
        press(&mut panel, alt(), Keysym::s, &ctx);
        let said = Some("Nothing to stash in herdr".to_owned());
        assert_eq!(panel.view("p", &ctx).error, said);
        let later = |s: u64| super::Ctx {
            now: ctx.now + Duration::from_secs(s),
            ..ctx
        };
        assert_eq!(panel.view("p", &later(7)).error, said);
        assert_eq!(panel.view("p", &later(9)).error, None);
        // Asked again, it says so again.
        press(&mut panel, alt(), Keysym::s, &later(9));
        assert_eq!(panel.view("p", &later(10)).error, said);
        // What is under way stays.
        panel.error = Some("Checking merge…".into());
        assert!(panel.view("p", &later(100)).error.is_some());
    }

    #[test]
    fn labels_say_how_many_they_run_on() {
        let mut items = items();
        for item in &mut items {
            (item.status.ahead, item.status.behind) = (3, 8);
        }
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let label = |panel: &Panel, button| panel.button_label(button, &ctx);
        // The selected one alone, nothing checked: its commits.
        assert_eq!(label(&panel, ReposButton::Push), "push \u{2191}3");
        assert_eq!(label(&panel, ReposButton::Pull), "pull \u{2193}8");
        assert_eq!(label(&panel, ReposButton::Fetch), "fetch all");
        assert_eq!(label(&panel, ReposButton::Commit), "commit");
        assert_eq!(label(&panel, ReposButton::OpenInZed), "open in Zed");
        check_repo(&mut panel, "gerdr", &ctx);
        check_repo(&mut panel, "herdr", &ctx);
        assert_eq!(label(&panel, ReposButton::Push), "push 2 repos");
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
            "Merge default branch in: 1 clean · 1 would conflict · 1 blocked"
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
        // All, on the right, drops only the blocked.
        ask(&mut panel);
        press(&mut panel, none(), Keysym::Right, &ctx);
        assert_eq!(
            press(&mut panel, none(), Keysym::Return, &ctx),
            Some(Outcome::Confirmed {
                kind: kind.clone(),
                jobs: jobs(&["gerdr", "herdr"])
            })
        );
        // Cancel, on the left, or Esc.
        ask(&mut panel);
        press(&mut panel, none(), Keysym::Left, &ctx);
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
                ReposButton::SkipDiverged,
                ReposButton::PullMerge,
                ReposButton::PullRebase
            ]
        );
        assert_eq!(sheet.default, 2);
        assert!(!sheet.danger);
        press(&mut panel, none(), Keysym::Left, &ctx);
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
        // Committed and pushed while the batch runs on, zed stays where
        // it was.
        let mut done = items.clone();
        done[2].status.changes.clear();
        let running = batch(
            BatchKind::Commit {
                msg: "m".into(),
                push: true,
            },
            vec![run("zed", RunState::Done), run("gerdr", RunState::Queued)],
        );
        let ctx = super::Ctx {
            items: &done,
            batch: Some(&running),
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
        // The batch over, zed goes under the heading.
        let ctx = super::Ctx { batch: None, ..ctx };
        panel.refresh(&ctx);
        assert_eq!(
            tree(&panel, &ctx),
            [
                "gerdr",
                "src/git.rs",
                "notes.md",
                "Clean · 2",
                "herdr",
                "zed"
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
        // Tab goes round: the tree, the detail, the field; Shift+Tab
        // the other way.
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Tree);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Detail);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        assert_eq!(panel.focus, Focus::Field);
        press(
            &mut panel,
            mods(false, true, false),
            Keysym::ISO_Left_Tab,
            &ctx,
        );
        assert_eq!(panel.focus, Focus::Detail);
        press(
            &mut panel,
            mods(false, true, false),
            Keysym::ISO_Left_Tab,
            &ctx,
        );
        assert_eq!(panel.focus, Focus::Tree);
        press(
            &mut panel,
            mods(false, true, false),
            Keysym::ISO_Left_Tab,
            &ctx,
        );
        assert_eq!(panel.focus, Focus::Field);
        // Esc steps back: from the field to the tree, the message kept;
        // then it closes.
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert_eq!(panel.focus, Focus::Tree);
        assert_eq!(&*panel.field, "fix it");
        assert_eq!(
            press(&mut panel, none(), Keysym::Escape, &ctx),
            Some(Outcome::Close)
        );
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
    fn nobody_to_commit_as_offers_the_name_and_email_dialog() {
        let items = vec![repo("app", "main", vec![change("a.rs", 'M')])];
        let kind = BatchKind::Commit {
            msg: "fix".into(),
            push: true,
        };
        let b = batch(kind, vec![run("app", failed(Failure::Identity))]);
        let ctx = ctx(&items, Some(&b));
        let mut panel = Panel::new(&items);
        select(&mut panel, "app");
        let view = panel.view("p", &ctx);
        let chip = repo_lines(&view)[0].chip.clone().unwrap();
        assert_eq!(
            (chip.text.as_str(), chip.tone),
            ("author unknown", Tone::Error)
        );
        assert!(
            view.hints
                .iter()
                .any(|h| h.button == ReposButton::SetIdentity)
        );
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::SetIdentity), &ctx),
            Some(Outcome::SetIdentity)
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
            runs: runs.to_vec(),
            ..batch(BatchKind::Fetch, Vec::new())
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
        panel.set_detail(
            key,
            "stash@{0} · 1 day ago · On main: wip\n".into(),
            Vec::new(),
        );
        let detail = panel.view("p", &ctx).detail;
        assert_eq!(detail.tab, DetailTab::Stashes);
        assert_eq!(
            detail.rows[0],
            DetailRow::Text(detail_line(
                "stash@{0} · 1 day ago · On main: wip",
                DiffTone::Plain
            ))
        );
        // Not while a batch runs there.
        let b = batch(
            BatchKind::Fetch,
            vec![run("herdr", RunState::Running("fetching…".into()))],
        );
        let ctx = super::Ctx {
            batch: Some(&b),
            ..ctx
        };
        assert_eq!(panel.click(ReposHit::Mark(6, RepoMark::Ahead), &ctx), None);
        assert_eq!(
            panel.error.as_deref(),
            Some("Wait for the fetch running in herdr (Alt+C stops it)")
        );
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
        // A file edited from the diff has its repository's read again, not
        // the others', and a read from before the edit doesn't do.
        read_diffs(&mut panel, &ctx);
        let before = panel.diff.reads;
        panel.diff.reread(&root("gerdr"));
        let wanted = panel.diffs_wanted(&ctx);
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].0, root("gerdr"));
        panel.set_diffs(root("gerdr"), before, vec![], &ctx);
        assert_eq!(panel.diffs_wanted(&ctx).len(), 1);
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
        press(&mut panel, alt(), Keysym::_2, &ctx);
        let log = DetailKey {
            root: root("gerdr"),
            file: None,
            tab: DetailTab::Log,
        };
        assert_eq!(panel.detail_wanted(&ctx), Some(log.clone()));
        assert!(panel.diffs_wanted(&ctx).is_empty());
        panel.set_detail(log.clone(), "abc1234 Read the branch".into(), Vec::new());
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
        // Alt+1, the diff again, where the selection is.
        press(&mut panel, alt(), Keysym::_1, &ctx);
        assert_eq!(panel.detail_wanted(&ctx), None);
        assert_eq!(panel.diff.scroll.pos(), 1.0);
    }

    #[test]
    fn the_log_picks_a_commit_and_shows_it() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        select(&mut panel, "gerdr");
        press(&mut panel, alt(), Keysym::_2, &ctx);
        let log = DetailKey {
            root: root("gerdr"),
            file: None,
            tab: DetailTab::Log,
        };
        let text = "To pull from origin/main (1):\nb2 theirs\n\nTo push to origin/main (2):\nc3 two\nc1 one";
        let oid = |o: &str| Some(o.to_owned());
        let oids = vec![None, oid("b2"), None, None, oid("c3"), oid("c1")];
        panel.set_detail(log.clone(), text.into(), oids.clone());
        // The first commit is picked; the lists' own lines stand out.
        assert_eq!(panel.log_pick, 1);
        let detail = panel.view("p", &ctx).detail;
        let tones: Vec<DiffTone> = (detail.rows.iter())
            .map(|row| match row {
                DetailRow::Text(l) => l.tone,
                other => panic!("{other:?}"),
            })
            .collect();
        use DiffTone::*;
        assert_eq!(&tones[..6], [Header, Plain, Plain, Header, Plain, Plain]);
        // The arrows go from commit to commit, over the others.
        panel.focus = Focus::Detail;
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.log_pick, 4);
        press(&mut panel, ctrl(), Keysym::j, &ctx);
        assert_eq!(panel.log_pick, 5);
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(panel.log_pick, 5);
        let marks = panel.view("p", &ctx).detail.marks;
        assert!(marks[5].selected && !marks[4].selected);
        // Enter shows it, its footer hint too.
        let shown = Some(Outcome::OpenCommit {
            root: root("gerdr"),
            oid: "c1".into(),
            line: "c1 one".into(),
        });
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), shown);
        let hints = panel.view("p", &ctx).hints;
        let hint = hints.iter().find(|h| h.button == ReposButton::ShowCommit);
        assert_eq!(hint.map(|h| h.key.as_str()), Some("Enter"));
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::ShowCommit), &ctx),
            shown
        );
        // Read again, the commit picked stays picked where it went.
        let text = format!("New (1):\nd4 new\n{text}");
        let mut again = vec![None, oid("d4")];
        again.extend(oids);
        panel.set_detail(log, text, again);
        assert_eq!(panel.log_pick, 7);
        // A click on a commit shows it; on a list's line, nothing.
        assert_eq!(panel.click(ReposHit::DiffRow(0), &ctx), None);
        let second = Some(Outcome::OpenCommit {
            root: root("gerdr"),
            oid: "d4".into(),
            line: "d4 new".into(),
        });
        assert_eq!(panel.click(ReposHit::DiffRow(1), &ctx), second);
        assert_eq!(panel.log_pick, 1);
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
        press(&mut panel, alt(), Keysym::_2, &ctx);
        let log = DetailKey {
            root: root("gerdr"),
            file: None,
            tab: DetailTab::Log,
        };
        panel.set_detail(log, "a\nb\nc\nd\ne\nf".into(), Vec::new());
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
                (4, ReposButton::CopyLines),
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
            [
                "| why",
                "| and b",
                "~ Shift+Enter breaks the line · Enter or Esc keeps it",
                "+c"
            ]
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
        // Sent to an agent only while one runs in the workspace.
        let send = |panel: &Panel| {
            let hints = panel.view("p", &ctx).hints;
            let hint = hints.iter().find(|h| h.button == ReposButton::SendReview);
            hint.map(|h| (h.label.clone(), h.disabled.clone()))
        };
        assert_eq!(
            send(&panel),
            Some(("send to agent".into(), Some(NO_AGENT.into())))
        );
        assert_eq!(panel.press(ReposButton::SendReview, &ctx), None);
        panel.agent = Some("claude".into());
        assert_eq!(send(&panel), Some(("send to claude".into(), None)));
        assert_eq!(
            panel.press(ReposButton::SendReview, &ctx),
            Some(Outcome::SendReview {
                text: text.clone(),
                count: 1
            })
        );

        // A line added above, it stays under b.
        let g = panel.diff.reads + 1;
        panel.diff.reads = g;
        let moved = vec![added("src/git.rs", 'M', &["x", "a", "b", "c"])];
        panel.set_diffs(root("gerdr"), g, moved, &ctx);
        let rows = detail_rows(&panel.view("p", &ctx));
        assert_eq!(rows[3..9], ["+x", "+a", "+b", "> why", "> and b", "+c"]);

        // c on it edits it; Esc keeps the edit, and doesn't close the
        // panel.
        panel.diff.set_cursor(6, None);
        press(&mut panel, none(), Keysym::c, &ctx);
        assert_eq!(panel.diff.editing.as_ref().map(|e| e.at), Some(Some(0)));
        type_text(&mut panel, "!", &ctx);
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.diff.editing.is_none());
        assert_eq!(panel.diff.review.comments[0].text, "why\nand b!");
        panel.diff.review.comments[0].text = "why\nand b".into();
        panel.diff.rebuild(&sections(&items));

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
        // It says the branch it was written on.
        assert!(text.contains("## gerdr · main"), "{text}");
        // Those on lines gone can be dropped on their own, asked first.
        let title: Vec<String> = (panel.view("p", &ctx).detail.title_buttons.iter())
            .map(|h| h.label.clone())
            .collect();
        assert_eq!(
            title,
            [
                "Previous comment",
                "Next comment",
                "Copy 1 comment",
                "Drop 1 outdated",
                "Clear review"
            ]
        );
        assert_eq!(
            panel.click(ReposHit::Button(ReposButton::DropOutdated), &ctx),
            None
        );
        let sheet = panel.view("p", &ctx).sheet.expect("a sheet");
        assert_eq!(
            sheet.title,
            "Delete the 1 comment on lines no longer in the diff?"
        );
        assert_eq!(
            sheet.buttons,
            [ReposButton::Cancel, ReposButton::DropOutdated]
        );
        press(&mut panel, none(), Keysym::Escape, &ctx);
        assert_eq!(panel.diff.review.comments.len(), 1);

        // Delete on it arms it, saying so; again deletes it.
        panel.diff.review_dirty = false;
        panel.diff.set_cursor(n - 2, None);
        press(&mut panel, none(), Keysym::Delete, &ctx);
        assert_eq!(
            panel.error.as_deref(),
            Some("Press Del again to delete the comment")
        );
        assert!(!panel.diff.review.is_empty());
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
    fn a_comment_on_a_file_header_is_on_the_whole_file() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 30);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        press(&mut panel, none(), Keysym::Tab, &ctx);
        let header = panel
            .diff
            .rows
            .iter()
            .position(|r| matches!(r, DiffRow::File(..)))
            .expect("a file's header");
        panel.diff.set_cursor(header, None);
        press(&mut panel, none(), Keysym::c, &ctx);
        assert!(panel.diff.editing.is_some(), "{:?}", panel.error);
        type_text(&mut panel, "split it", &ctx);
        press(&mut panel, none(), Keysym::Return, &ctx);
        let comment = &panel.diff.review.comments[0];
        assert!(comment.lines.is_empty());
        assert_eq!(comment.text, "split it");
        // Shown under the header, before the file's hunks.
        let rows = detail_rows(&panel.view("p", &ctx));
        assert_eq!(rows[header + 1], "> split it");
        assert_eq!(panel.diff.outdated_count(), 0);
        let Some(Outcome::Copy { text, .. }) = press(&mut panel, ctrl_shift(), Keysym::C, &ctx)
        else {
            panic!("not copied");
        };
        assert!(
            text.contains("### `src/git.rs` (the whole file)\n\nsplit it\n"),
            "{text}"
        );
        // The diff's search finds what comments say too.
        panel.focus = Focus::Detail;
        panel.key(&none(), Keysym::slash, Keysym::slash, &ctx);
        type_text(&mut panel, "split", &ctx);
        assert_eq!(search_count(&panel, &ctx), "1 of 1");
        // Under its heading.
        assert_eq!(cursor_row(&panel), Some(header + 2));
        assert_eq!(panel.diff.found_on(header + 2), vec![(0..5, true)]);
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
            [
                hunks[0],
                hunks[1],
                (4, B::Comment),
                (4, B::CopyLines),
                (4, B::Restore)
            ]
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
                (B::PrevComment, "Previous comment"),
                (B::NextComment, "Next comment"),
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
        // Held far past, many rows a frame at once, the range kept.
        panel.click(ReposHit::DiffRow(3), &ctx);
        panel.drag_past_rows(2, &ctx);
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
        // Enter cancels first; on Clear review, it deletes them.
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert_eq!(panel.diff.review.comments.len(), 1);
        panel.press(ReposButton::ClearReview, &ctx);
        press(&mut panel, none(), Keysym::Right, &ctx);
        press(&mut panel, none(), Keysym::Return, &ctx);
        assert!(panel.sheet.is_none() && panel.diff.review.is_empty());
    }

    #[test]
    fn a_long_file_shows_its_first_lines_till_asked_for_all() {
        use crate::diff::FILE_LINES;
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 6);
        let mut panel = Panel::new(&items);
        let many: Vec<String> = (0..FILE_LINES + 5).map(|n| n.to_string()).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let g = panel.diff.reads;
        let files = vec![added("src/git.rs", 'M', &many)];
        panel.set_diffs(root("gerdr"), g, files, &ctx);
        let more = |panel: &Panel| {
            (0..panel.diff.rows.len())
                .find(|&r| matches!(panel.diff.rows[r], crate::diff_view::DiffRow::More(..)))
        };
        let at = more(&panel).expect("a row saying there are more");
        // Right after the lines shown, saying how many more.
        assert_eq!(
            panel.diff.line_text(at - 1),
            Some(&*(FILE_LINES - 1).to_string())
        );
        panel.diff.scroll.jump(at as f64);
        let view = panel.view("p", &ctx);
        let i = view
            .detail
            .rows
            .iter()
            .position(|r| matches!(r, DetailRow::Note(n) if n.starts_with("… 5 more lines")));
        assert!(i.is_some(), "{:?}", detail_rows(&view));
        // Enter on it shows them all.
        panel.focus = Focus::Detail;
        panel.diff.set_cursor(at, None);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert_eq!(more(&panel), None);
        assert_eq!(panel.diff.line_text(at), Some(&*FILE_LINES.to_string()));
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
            Some("Resolve 1 conflicted file first (Alt+D)")
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
            Some(Outcome::Refresh)
        );
        assert_eq!(
            press(&mut panel, alt(), Keysym::d, &ctx),
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
        // On the default branch, its remote's comes in.
        let mut on_main = items.clone();
        on_main[2].status.branch = Some("main".into());
        on_main[2].status.remote = Some("origin".into());
        assert_eq!(label(&on_main, &[], merge), "merge origin/main into main");
        assert_eq!(
            label(&on_main, &[], ReposButton::Rebase),
            "rebase main onto origin/main"
        );
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
            [ReposButton::Cancel, ReposButton::DeleteBranch]
        );
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.sheet.is_none() && panel.branch_menu.is_some());
        // Those on it go too: the check tells why they are left out.
        assert_eq!(press(&mut panel, none(), Keysym::Delete, &ctx), None);
        press(&mut panel, none(), Keysym::Right, &ctx);
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
        // Alt+Del opens the menu to delete one: only those that can be,
        // Enter asking.
        panel.branch_menu = None;
        panel.sheet = None;
        press(&mut panel, alt(), Keysym::Delete, &ctx);
        assert_eq!(menu_rows(&panel, &ctx), ["main / master", "feat/x"]);
        let menu = panel.view("p", &ctx).branch.menu.unwrap();
        assert_eq!(menu.placeholder, "Find a local branch to delete");
        press(&mut panel, none(), Keysym::Down, &ctx);
        assert_eq!(press(&mut panel, none(), Keysym::Return, &ctx), None);
        assert!(matches!(
            panel.sheet,
            Some(Sheet::Confirm {
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
        assert_eq!(
            panel.error.as_deref(),
            Some("Wait for the fetch running in gerdr (Alt+C stops it)")
        );
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
                ReposButton::EditName,
                ReposButton::CreateMissing,
                ReposButton::SwitchExisting
            ]
        );
        // Enter edits the name: back in the menu, to create it again.
        assert_eq!(sheet.default, 0);
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
            [ReposButton::EditName, ReposButton::SwitchExisting]
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
        panel.folded_dirs.set_open(&root("app"), "src", false);
        let r = panel
            .diff
            .header_row(&root("app"), Some("src/main.rs"))
            .unwrap();
        panel.select_diff_row(r, &ctx);
        assert_eq!(
            panel.selected,
            Some(Selected::File(root("app"), "src/main.rs".into()))
        );
        assert_eq!(panel.folded_dirs, Folds::default());
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

    /// Comment `text` on the first line of the file the diff row `r` is in.
    fn comment_on(panel: &mut Panel, r: usize, text: &str, ctx: &Ctx) {
        panel.focus = Focus::Detail;
        panel.diff.set_cursor(r, None);
        press(panel, none(), Keysym::c, ctx);
        type_text(panel, text, ctx);
        press(panel, none(), Keysym::Return, ctx);
    }

    /// The first line of each file of the diff, by row.
    fn first_lines(panel: &Panel) -> Vec<usize> {
        use crate::diff_view::DiffRow;
        let rows = &panel.diff.rows;
        (0..rows.len())
            .filter(|&r| {
                matches!(rows[r], DiffRow::Line(_, _, _, 0))
                    && !matches!(rows[r - 1], DiffRow::Line(..))
            })
            .collect()
    }

    #[test]
    fn the_brackets_go_through_the_comments_and_wrap() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 6);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        panel.focus = Focus::Detail;
        // None yet: it says so.
        assert_eq!(
            press(&mut panel, none(), Keysym::bracketright, &ctx),
            Some(Outcome::Notice(diff_view::NO_COMMENTS))
        );
        let lines = first_lines(&panel);
        assert!(lines.len() >= 2, "{lines:?}");
        comment_on(&mut panel, lines[1], "the later one", &ctx);
        comment_on(&mut panel, lines[0], "the first one", &ctx);
        let cursor_text = |panel: &Panel| {
            let r = cursor_row(panel).unwrap();
            let on = panel.diff.comment_of_row(r).expect("on a comment");
            panel.diff.review.comments[on].text.clone()
        };
        // From above them, the first; on, and past the last back round.
        panel.diff.set_cursor(0, None);
        press(&mut panel, none(), Keysym::bracketright, &ctx);
        assert_eq!(cursor_text(&panel), "the first one");
        press(&mut panel, none(), Keysym::bracketright, &ctx);
        assert_eq!(cursor_text(&panel), "the later one");
        press(&mut panel, none(), Keysym::bracketright, &ctx);
        assert_eq!(cursor_text(&panel), "the first one");
        // Back, past the first to the last.
        press(&mut panel, none(), Keysym::bracketleft, &ctx);
        assert_eq!(cursor_text(&panel), "the later one");
        // From a line between them: the ones either side.
        let between = first_lines(&panel)[1];
        panel.diff.set_cursor(between, None);
        press(&mut panel, none(), Keysym::bracketleft, &ctx);
        assert_eq!(cursor_text(&panel), "the first one");
        // A count goes as many.
        panel.diff.set_cursor(0, None);
        press(&mut panel, none(), Keysym::_2, &ctx);
        press(&mut panel, none(), Keysym::bracketright, &ctx);
        assert_eq!(cursor_text(&panel), "the later one");
        // So do the title's buttons.
        panel.click(ReposHit::Button(ReposButton::NextComment), &ctx);
        assert_eq!(cursor_text(&panel), "the first one");
        panel.click(ReposHit::Button(ReposButton::PrevComment), &ctx);
        assert_eq!(cursor_text(&panel), "the later one");
    }

    #[test]
    fn the_brackets_find_a_comment_in_a_folded_file() {
        use crate::diff_view::DiffRow;
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 6);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        let lines = first_lines(&panel);
        comment_on(&mut panel, lines[0], "folded away", &ctx);
        let head = (0..panel.diff.rows.len())
            .find(|&r| matches!(panel.diff.rows[r], DiffRow::File(..)))
            .unwrap();
        panel.diff.set_cursor(head, None);
        press(&mut panel, none(), Keysym::Left, &ctx);
        assert!(
            !panel
                .diff
                .rows
                .iter()
                .any(|r| matches!(r, DiffRow::CommentHead { .. })),
            "folded with its comment"
        );
        press(&mut panel, none(), Keysym::bracketright, &ctx);
        let r = cursor_row(&panel).unwrap();
        assert!(
            matches!(panel.diff.rows[r], DiffRow::CommentHead { .. }),
            "{:?}",
            panel.diff.rows[r]
        );
    }

    #[test]
    fn a_comment_says_whose_it_is_and_where_on_its_heading() {
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 40);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        let lines = first_lines(&panel);
        comment_on(&mut panel, lines[1], "b", &ctx);
        comment_on(&mut panel, lines[0], "a", &ctx);
        let view = panel.view("p", &ctx);
        let places: Vec<(bool, String)> = (view.detail.rows.iter())
            .filter_map(|row| match row {
                DetailRow::CommentHead { finding, place, .. } => {
                    Some((finding.is_some(), place.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            places,
            [
                (false, "comment 1 of 2".to_owned()),
                (false, "comment 2 of 2".to_owned())
            ]
        );
        assert_eq!(view.detail.ticks.len(), 2);
    }

    #[test]
    fn gg_g_the_braces_and_half_pages_move_the_diffs_cursor() {
        use crate::diff_view::DiffRow;
        let items = items();
        let mut ctx = ctx(&items, None);
        ctx.page = (20, 6);
        let mut panel = Panel::new(&items);
        read_diffs(&mut panel, &ctx);
        panel.focus = Focus::Detail;
        let rows = panel.diff.rows.clone();
        let shift = mods(false, true, false);
        // G the end, gg the top.
        press(&mut panel, shift, Keysym::G, &ctx);
        assert_eq!(cursor_row(&panel), Some(rows.len() - 1));
        press(&mut panel, none(), Keysym::g, &ctx);
        press(&mut panel, none(), Keysym::g, &ctx);
        assert_eq!(cursor_row(&panel), Some(0));
        // } and { the hunk (or file) after and before.
        let heads: Vec<usize> = (0..rows.len())
            .filter(|&r| matches!(rows[r], DiffRow::Hunk(..) | DiffRow::File(..)))
            .collect();
        press(&mut panel, shift, Keysym::braceright, &ctx);
        assert_eq!(cursor_row(&panel), Some(heads[0]));
        press(&mut panel, shift, Keysym::braceright, &ctx);
        assert_eq!(cursor_row(&panel), Some(heads[1]));
        press(&mut panel, shift, Keysym::braceleft, &ctx);
        assert_eq!(cursor_row(&panel), Some(heads[0]));
        // Ctrl+D and Ctrl+U half a page.
        let at = cursor_row(&panel).unwrap();
        press(&mut panel, ctrl(), Keysym::d, &ctx);
        assert_eq!(cursor_row(&panel), Some(at + 3));
        press(&mut panel, ctrl(), Keysym::u, &ctx);
        assert_eq!(cursor_row(&panel), Some(at));
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
        // No key puts the hunk back, so its button has none.
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
        assert_eq!(key(line.unwrap()).as_deref(), Some("Shift+R"));
        assert_eq!(
            restore(panel.click(ReposHit::DiffRestore(hunk), &ctx)),
            (vec![2, 3, 4, 6], put(6, "\told"))
        );
    }

    #[test]
    fn lines_have_numbers_and_a_long_ones_caret_stays_in_view() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let long = "x".repeat(300);
        let g = panel.diff.reads;
        let files = vec![added("src/git.rs", 'M', &["a", &long])];
        panel.set_diffs(root("gerdr"), g, files, &ctx);
        let view = panel.view("p", &ctx);
        assert_eq!(view.detail.gutter, 2);
        assert_eq!(view.detail.skipped, None);
        // The caret far along the long line: it is shown from a char on,
        // the caret moved back by as many.
        let row = (0..panel.diff.rows.len())
            .find(|&r| panel.diff.line_text(r) == Some(&long))
            .unwrap();
        panel.diff.set_cursor(row, None);
        panel.focus = Focus::Detail;
        press(&mut panel, none(), Keysym::dollar, &ctx);
        let view = panel.view("p", &ctx);
        let (i, skip) = view.detail.skipped.expect("shown from a char on");
        let room = crate::chrome::diff_line_room(ctx.cols, 2, false);
        assert_eq!(view.detail.caret, Some((i, 299 - skip)));
        assert!(299 - skip < room);
        let DetailRow::Line(line) = &view.detail.rows[i] else {
            panic!();
        };
        assert_eq!(line.text.chars().count(), 300 - skip);
    }

    #[test]
    fn the_tree_marks_files_edited_from_the_diff_and_commented_on() {
        let items = items();
        let ctx = ctx(&items, None);
        let mut panel = Panel::new(&items);
        let file = |panel: &Panel| {
            let view = panel.view("p", &ctx);
            view.lines
                .into_iter()
                .find_map(|l| match l {
                    TreeLine::File(f) if f.path == "src/git.rs" => Some(f),
                    _ => None,
                })
                .unwrap()
        };
        assert!(!file(&panel).edited && file(&panel).comments == 0);
        panel.edited.insert((root("gerdr"), "src/git.rs".into()));
        panel.diff.review.comments.push(crate::review::Comment {
            root: root("gerdr"),
            path: "src/git.rs".into(),
            lines: vec![crate::review::Quoted::default()],
            text: "why".into(),
            flag: None,
        });
        let f = file(&panel);
        assert!(f.edited);
        assert_eq!(f.comments, 1);
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
            panel.diff.mode_title(true).as_deref(),
            Some("-- VISUAL LINE -- d delete · y yank · Shift+R restore · Esc stop")
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
        // `y` yanks as `d` deletes, what the diff shows; `p` and `P` paste
        // at the caret.
        let yank = |o: Option<Outcome>| match o {
            Some(Outcome::Yank(taken)) => (taken.text, taken.lines),
            other => panic!("not a yank: {other:?}"),
        };
        let whole = |text: &str| (text.to_owned(), true);
        let chars = |text: &str| (text.to_owned(), false);
        panel.caret_to(id, 4, &ctx);
        assert_eq!(yank(keys(&mut panel, "yy")), whole("let id = 1;\n"));
        assert_eq!(
            yank(keys(&mut panel, "2Y")),
            whole("let id = 1;\n\tfoo(bar)\n")
        );
        assert_eq!(yank(keys(&mut panel, "yw")), chars("id "));
        assert_eq!(panel.diff.mode_title(true), None);
        keys(&mut panel, "3y");
        assert_eq!(panel.diff.mode_title(true).as_deref(), Some("3y"));
        // Only what the diff shows: this hunk ends there.
        assert_eq!(
            yank(keys(&mut panel, "j")),
            whole("let id = 1;\n\tfoo(bar)\n")
        );
        keys(&mut panel, "vl");
        assert_eq!(yank(keys(&mut panel, "y")), chars("id"));
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
        // `u` and `U` undo and do again, in the file at the caret.
        let here = Some((root("gerdr"), "src/a.rs".to_owned()));
        assert_eq!(keys(&mut panel, "u"), Some(Outcome::UndoCut(here.clone())));
        assert_eq!(keys(&mut panel, "U"), Some(Outcome::RedoCut(here)));
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
        // Esc closes the search first, then gives the tree the keys,
        // then closes the panel.
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert!(panel.diff.search.is_none());
        assert!(panel.view("p", &ctx).detail.found.iter().all(Vec::is_empty));
        assert_eq!(press(&mut panel, none(), Keysym::Escape, &ctx), None);
        assert_eq!(panel.focus, Focus::Tree);
        assert_eq!(
            press(&mut panel, none(), Keysym::Escape, &ctx),
            Some(Outcome::Close)
        );
    }

    #[test]
    fn ctrl_f_finds_in_the_diff_and_fetches_elsewhere() {
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
        // With the diff's keys, Ctrl+F finds too.
        assert_eq!(press(&mut panel, ctrl(), Keysym::f, &ctx), None);
        assert!(panel.search_has_keys());
        press(&mut panel, none(), Keysym::Escape, &ctx);
        // With the tree's, it fetches.
        press(&mut panel, none(), Keysym::Escape, &ctx);
        assert_eq!(panel.focus, Focus::Tree);
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
