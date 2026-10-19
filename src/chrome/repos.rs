//! The repositories panel: the project's repositories as one, nearly as big
//! as the output. On the left a tree, a row per repository (a checkbox,
//! its branch, where it stands, what is being done to it) with its changed
//! files under it, each with a checkbox; on the right the selected file's
//! diff or the selected repository's log; under both a field (a branch
//! name or a commit message), the last batch's result and the actions.
//! A sheet over it asks before a batch that could conflict.
#![allow(dead_code)]

use super::{Canvas, FieldSpot, LineState, Painter, Palette, Rect};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Size};

/// What the repositories panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ReposView {
    /// The project's name.
    pub title: String,
    /// After the title: `feat/x · 3 of 4 on it · 2 dirty · 1 conflict ·
    /// fetched 2m ago`.
    pub summary: String,
    /// The tree's lines in view, scrolled.
    pub lines: Vec<TreeLine>,
    /// The selected line, by index into `lines`.
    pub selected: Option<usize>,
    pub hover: Option<ReposHit>,
    /// The field: the branch name or commit message being typed.
    pub field: Line,
    /// The field has the keyboard (else the tree has).
    pub field_focused: bool,
    /// What the field says while empty.
    pub placeholder: String,
    /// Under the field, in the error colour: why the last action wasn't
    /// done (an empty message, a bad branch name).
    pub error: Option<String>,
    /// The last batch's result.
    pub result: Option<ResultBar>,
    /// The actions for what is selected, in the footer.
    pub hints: Vec<Hint>,
    pub detail: DetailView,
    /// A question over the panel, which then has the keyboard.
    pub sheet: Option<SheetView>,
    /// What the tree says when the project has no repository.
    pub empty: String,
}

/// A line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeLine {
    Repo(RepoLine),
    File(FileLine),
}

/// A repository's line.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoLine {
    pub check: Check,
    /// `Some(open)` when it has files to show or hide.
    pub expanded: Option<bool>,
    /// Its workspace's name.
    pub label: String,
    /// Its branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// The project has a feature and this isn't on it: a `≠` badge.
    pub off_feature: bool,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Its branch has no upstream yet.
    pub no_upstream: bool,
    pub stashes: u32,
    /// What it is in the middle of, or what went wrong: `MERGING · 2`,
    /// `REBASING 2/5`, `STASH CONFLICT`, `login needed`, `detached`.
    pub chip: Option<Chip>,
    /// What is being done to it, `pushing…`, or a second press's warning.
    pub state: LineState,
}

/// A changed file's line, under its repository.
#[derive(Clone, Debug, PartialEq)]
pub struct FileLine {
    pub check: Check,
    /// `git::Change::code`.
    pub code: char,
    /// From the repository's top folder; a rename as `old → new`.
    pub path: String,
    pub conflicted: bool,
}

/// A checkbox: a repository's is `Partial` when only some of its files are
/// checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Check {
    #[default]
    Off,
    On,
    Partial,
}

/// A small tag on a repository's line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub text: String,
    pub tone: Tone,
}

/// What colour something is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    Muted,
    #[default]
    Normal,
    Success,
    Warning,
    Error,
}

/// The last batch's result, between the field and the footer: `Commit and
/// push: 2 done · 1 conflict`, and what can be done about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultBar {
    pub text: String,
    pub tone: Tone,
    /// `Undo`, `RetryFailed`.
    pub buttons: Vec<ReposButton>,
}

/// An action in the footer: a button with its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub button: ReposButton,
    /// As shown: `Ctrl+B`.
    pub key: String,
    /// `None` when it can be done now, else why not (in a tooltip-like
    /// muted line when hovered).
    pub disabled: Option<String>,
}

/// The right side: the selected file's diff, or the selected repository's
/// log.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailView {
    pub tab: DetailTab,
    /// Above it: the file's path, or the repository's name.
    pub title: String,
    /// The lines in view, scrolled.
    pub lines: Vec<DetailLine>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DetailTab {
    #[default]
    Diff,
    Log,
}

/// A line of a diff or a log, in the monospace font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailLine {
    pub text: String,
    pub tone: DiffTone,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffTone {
    #[default]
    Plain,
    Added,
    Removed,
    /// `@@ … @@`.
    Hunk,
    /// `diff --git`, `index`, `---`, `+++`; in a log, a command run.
    Header,
    /// In a log, what a failed command printed.
    Error,
}

/// A question over the panel: what the check before a batch found, per
/// repository, or which way to pull diverged ones.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetView {
    pub title: String,
    /// Per repository: its name, what was found (`clean`, `conflicts:
    /// a.rs, b.rs`, `blocked: detached HEAD`) and its colour.
    pub lines: Vec<(String, String, Tone)>,
    pub buttons: Vec<ReposButton>,
    /// The button Enter presses, by index into `buttons`.
    pub default: usize,
}

/// Everything the panel can be asked to do with a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReposButton {
    Branch,
    StartFeature,
    Join,
    Commit,
    CommitPush,
    Push,
    ForcePush,
    Fetch,
    Pull,
    Merge,
    Rebase,
    Park,
    Resume,
    OpenInZed,
    MarkResolved,
    Continue,
    Abort,
    Undo,
    RetryFailed,
    RunInTerminal,
    OpenWorkspace,
    /// On the preflight sheet.
    All,
    CleanOnly,
    /// On the divergence sheet.
    PullRebase,
    PullMerge,
    SkipDiverged,
    Cancel,
}

impl ReposButton {
    /// What it says.
    pub fn label(self) -> &'static str {
        match self {
            ReposButton::Branch => "Branch",
            ReposButton::StartFeature => "Start feature",
            ReposButton::Join => "Join feature",
            ReposButton::Commit => "Commit",
            ReposButton::CommitPush => "Commit and push",
            ReposButton::Push => "Push",
            ReposButton::ForcePush => "Force push",
            ReposButton::Fetch => "Fetch",
            ReposButton::Pull => "Pull",
            ReposButton::Merge => "Merge base",
            ReposButton::Rebase => "Rebase on base",
            ReposButton::Park => "Park",
            ReposButton::Resume => "Resume",
            ReposButton::OpenInZed => "Open in Zed",
            ReposButton::MarkResolved => "Resolved",
            ReposButton::Continue => "Continue",
            ReposButton::Abort => "Abort",
            ReposButton::Undo => "Undo",
            ReposButton::RetryFailed => "Retry failed",
            ReposButton::RunInTerminal => "Run in terminal",
            ReposButton::OpenWorkspace => "Open workspace",
            ReposButton::All => "All",
            ReposButton::CleanOnly => "Clean only",
            ReposButton::PullRebase => "Rebase",
            ReposButton::PullMerge => "Merge",
            ReposButton::SkipDiverged => "Skip those",
            ReposButton::Cancel => "Cancel",
        }
    }
}

/// What the pointer is over in the repositories panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReposHit {
    /// A tree line, by index into `ReposView::lines`.
    Line(usize),
    /// Its checkbox.
    Check(usize),
    /// A repository line's expander.
    Expander(usize),
    Field,
    /// A footer action, a result bar button or a sheet button.
    Button(ReposButton),
    DetailTab(DetailTab),
    /// The detail pane (for the wheel).
    Detail,
    /// Inside the panel, on nothing.
    Panel,
}

/// Where the repositories panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReposLayout {
    pub panel: Rect,
    pub header: Rect,
    /// The tree's area.
    pub tree: Rect,
    /// Per tree line: its rect, its checkbox's, and a repository line's
    /// expander's.
    pub lines: Vec<(Rect, Rect, Option<Rect>)>,
    pub detail: Rect,
    pub detail_tabs: Vec<(DetailTab, Rect)>,
    pub field: Rect,
    pub result: Option<Rect>,
    /// The result bar's buttons, the footer's, and the sheet's.
    pub buttons: Vec<(ReposButton, Rect)>,
    pub footer: Rect,
    pub sheet: Option<Rect>,
}

/// How many tree lines and detail lines the panel shows at once on an
/// output of `size`.
pub fn repos_page(size: Size<i32, Logical>) -> (usize, usize) {
    let _ = size;
    todo!()
}

/// Where the parts of the panel showing `view` go on an output of `size`.
pub fn repos_layout(view: &ReposView, size: Size<i32, Logical>) -> ReposLayout {
    let _ = (view, size);
    todo!()
}

/// What `pos` (output coordinates) is over; `None` outside the panel.
/// With a sheet open only its buttons, else `Panel`.
pub fn repos_hit(layout: &ReposLayout, pos: Point<f64, Logical>) -> Option<ReposHit> {
    let _ = (layout, pos);
    todo!()
}

/// The panel's field, for the caret and selection, on an output of `size`.
pub fn repos_field(view: &ReposView, size: Size<i32, Logical>, scale: f64) -> FieldSpot {
    let _ = (view, size, scale);
    todo!()
}

impl Painter {
    /// The repositories panel for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow.
    pub(super) fn repos(
        &self,
        view: &ReposView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let _ = (view, size, scale, p, Canvas::new);
        todo!()
    }
}
