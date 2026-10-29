//! The repositories panel: the project's repositories as one, nearly as big
//! as the output. On the left a tree, a row per repository (a checkbox,
//! its branch, where it stands, what is being done to it) with its changed
//! files under it, each with a checkbox; on the right every change of
//! every repository as one diff (repository and file headers banded,
//! the file in view's pinned at the top, added and removed lines tinted
//! with their changed words tinted more), or the selected repository's
//! log; under both a field (a commit message), the last batch's result
//! and the actions.
//! A sheet over it asks before a batch that could conflict, or that can't
//! be taken back, and lists the keys. At the left of the header, a
//! branch picker: the branch the repositories acted on are on, and a menu
//! of the ones they all have to switch them to, with a row creating the
//! branch typed in its search. The line between the tree
//! and the detail can be dragged; both scroll, with a bar saying where.
//! Actions the footer has no room for go behind a "more" button.

use super::{
    BUTTON_RADIUS, CAP_PADDING, CAP_RADIUS, CHECKBOX_SIZE, Canvas, DETAIL_FONT_SIZE, Density,
    FIELD_GAP, FONT_SIZE, FieldSpot, GIT_MARK_GAP, GIT_MARK_WIDTH, HELP_MARGIN, HELP_PADDING,
    HELP_RADIUS, HINT_KEY_GAP, KEY_FONT_SIZE, LABEL_FONT_SIZE, LineState, MENU_RADIUS, PADDING,
    Painter, Palette, QUIT_BUTTON_GAP, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, arrow, band, check,
    chevron_down, coverage, field_text, git_branch, icon_sdf, measure, measure_mono, mix, phys,
    physical, plus, rect_px, rounded_rect, trash,
};
use std::ops::Range;

use crate::diff::LineKind;
use crate::text_field::Line;
use crate::wheel;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The panel: at most this wide and high, a margin in from the output's
/// edges.
const REPOS_MAX_WIDTH: i32 = 1600;
const REPOS_MAX_HEIGHT: i32 = 1200;
/// A button's height and the error line's height.
const ROW_BUTTON: i32 = 28;
const ERROR_HEIGHT: i32 = 22;
/// The header row with the title and the summary, and the gap between
/// them.
const REPOS_HEADER: i32 = 44;
const HEADER_GAP: i32 = 12;
/// The tree's share of the panel's width unless dragged, the least and
/// most it can be dragged to, the narrowest it gets (unless that is more
/// than half), and how far either side of the line between the tree and
/// the detail a press takes hold of it.
pub const REPOS_TREE_SHARE: f64 = 0.42;
pub const REPOS_TREE_SHARE_MIN: f64 = 0.2;
pub const REPOS_TREE_SHARE_MAX: f64 = 0.8;
const REPOS_TREE_MIN: i32 = 300;
pub const SPLITTER_GRAB: i32 = 5;
/// The room above and below the tree's lines and the detail's, and beside
/// the tree's.
const REPOS_PAD: i32 = 6;
/// A tree line at each density, the room before its checkbox, and how
/// far a file's line is indented under its repository's.
pub fn repos_row(density: Density) -> i32 {
    match density {
        Density::Comfortable => 30,
        Density::Compact => 26,
    }
}
const REPOS_LINE_INSET: i32 = 4;
const REPOS_INDENT: i32 = 24;
/// The scrollbar at the right of the tree and of the detail: its width,
/// and the shortest its thumb gets.
const SCROLLBAR: i32 = 6;
const SCROLLBAR_MIN: i32 = 24;
/// A checkbox's clickable room, a repository's expander's, and a file's
/// status letter column with the gap before it.
const REPOS_CHECK: i32 = 28;
const REPOS_EXPANDER: i32 = 20;
const REPOS_CODE: i32 = 16;
const REPOS_CODE_GAP: i32 = 2;
/// A repository's chip: its height, the room either side of its text and
/// its corners.
const CHIP_HEIGHT: i32 = 20;
const CHIP_PAD: i32 = 6;
const CHIP_RADIUS: f32 = 4.0;
/// The detail pane: its title row, a line of a diff or log at each
/// density, and the size of the monospace font they are in.
const REPOS_DETAIL_TITLE: i32 = 40;
pub fn repos_detail_line(density: Density) -> i32 {
    match density {
        Density::Comfortable => 22,
        Density::Compact => 19,
    }
}
const REPOS_MONO_SIZE: f32 = 14.0;
/// A file's header in the diff: its fold chevron's room, its checkbox's,
/// and its status letter's pill; how far in a diff line's text starts,
/// past the bar marking it added or removed, and that bar's width.
const DIFF_FOLD: i32 = 18;
const DIFF_CHECK: i32 = 26;
const DIFF_BADGE: i32 = 18;
const DIFF_INSET: i32 = 8;
const DIFF_BAR: i32 = 3;
/// A review comment's text, in from its card's edge.
const COMMENT_INDENT: i32 = 12;
/// A button on a diff row: its padding, the gap before its key, the gap
/// between two, and how far in from the row's ends and edges it sits.
const ROW_CHIP_PAD: i32 = 8;
const ROW_CHIP_KEY_GAP: i32 = 6;
const ROW_CHIP_GAP: i32 = 4;
const ROW_CHIP_INSET: i32 = 2;
/// How strongly an added or removed line is tinted, and its changed
/// words.
const DIFF_TINT: f32 = 0.10;
const DIFF_WORD_TINT: f32 = 0.28;
/// The Diff and Log tabs: each this wide, this far apart.
const REPOS_TAB_WIDTH: i32 = 64;
const REPOS_TAB_GAP: i32 = 4;
/// The field's row, and the field in it.
const REPOS_FIELD_ROW: i32 = 44;
const REPOS_FIELD: i32 = 32;
/// The result bar, and the gap between its buttons.
const REPOS_RESULT: i32 = 36;
const REPOS_BUTTON_GAP: i32 = 6;
/// The footer: the room above and below its rows of actions, a row, the
/// most rows (the rest go behind the "more" button), the gap between
/// actions and the room inside either end of one.
const REPOS_FOOTER_PAD: i32 = 4;
const REPOS_HINT_ROW: i32 = 32;
const REPOS_FOOTER_ROWS: usize = 2;
const HINT_GAP: i32 = 4;
const HINT_PAD: i32 = 6;
/// Without a font to measure with, how wide a character is guessed to be
/// (logical pixels): of a label in the small font, of a key on a keycap,
/// and of a sheet button's label; and what is added for a whole text.
const SMALL_CHAR: f64 = 5.4;
const KEY_CHAR: f64 = 4.9;
const BUTTON_CHAR: f64 = 6.6;
const GUESS_SLACK: f64 = 4.0;
/// The sheet: at most this wide; the room inside it, its title row, a
/// line, the gap after the title and before the buttons, the buttons and
/// the room either side of their labels, and the share of a line its
/// repository's name takes.
const SHEET_WIDTH: i32 = 640;
const SHEET_PAD: i32 = 18;
const SHEET_TITLE: i32 = 28;
const SHEET_LINE: i32 = 28;
const SHEET_GAP: i32 = 10;
const SHEET_BUTTON: i32 = 34;
const SHEET_BUTTON_GAP: i32 = QUIT_BUTTON_GAP;
const SHEET_BUTTON_PAD: i32 = 16;
const SHEET_BUTTON_MIN: i32 = 84;
const SHEET_NAME_SHARE: f32 = 0.32;
/// How dark the panel is under a sheet, as a share of the palette's
/// shadow.
const SCRIM: f32 = 0.5;
/// The branch picker's button: the room inside either end, its icon's and
/// chevron's room, the gap after the icon and before the chevron, and the
/// narrowest it gets.
const PICKER_PAD: i32 = 10;
const PICKER_ICON: i32 = 18;
const PICKER_CHEVRON: i32 = 14;
const PICKER_GAP: i32 = 6;
const PICKER_MIN: i32 = 140;
/// The branch menu: at most this many rows in view, the gap between it
/// and the button, the room above and below its rows, the room inside
/// either end of a row and a row's check mark's room, and the widest it
/// gets.
pub const BRANCH_MENU_ROWS: usize = 10;
const BRANCH_MENU_GAP: i32 = 4;
const BRANCH_MENU_PAD: i32 = 4;
const BRANCH_ROW_PAD: i32 = 10;
const BRANCH_CHECK: i32 = 22;
const BRANCH_MENU_MAX: i32 = 420;

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
    /// The part of the first line scrolled past, 0 to 1; with some,
    /// `lines` has one more under the tree, shown in part.
    pub part: f64,
    /// How far the lines are moved down (up when negative), logical
    /// pixels, while the tree is pulled past an end.
    pub over: f64,
    /// Where `lines` start among all the tree's lines, and how many there
    /// are: where the tree's scrollbar stands.
    pub first: usize,
    pub total: usize,
    /// The selected line, by index into `lines`.
    pub selected: Option<usize>,
    pub hover: Option<ReposHit>,
    /// The field: the commit message being typed.
    pub field: Line,
    /// The field has the keyboard (else the tree has).
    pub field_focused: bool,
    /// What the field says while empty.
    pub placeholder: String,
    /// Under the field, in the error colour: why the last action wasn't
    /// done (an empty message, a bad branch name typed in the menu).
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
    /// The header's branch picker.
    pub branch: BranchPicker,
    /// How much room the chrome takes.
    pub density: Density,
    /// The tree's share of the panel's width.
    pub tree_share: f64,
}

/// The header's branch picker: the branch the repositories acted on are
/// on, and its menu while open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchPicker {
    /// The branch they are all on, else `mixed branches`.
    pub label: String,
    /// The label says they differ (or that there are none): it is muted.
    pub mixed: bool,
    /// There are repositories to switch, and no batch running.
    pub enabled: bool,
    pub menu: Option<BranchMenuView>,
}

/// The branch picker's menu: the branches the repositories acted on all
/// have, in view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchMenuView {
    pub rows: Vec<String>,
    /// Per row: whether it has a local branch to delete, with a button.
    pub deletable: Vec<bool>,
    /// The row of the branch they are all on, by index into `rows`.
    pub current: Option<usize>,
    /// The row Enter picks, by index into `rows`.
    pub picked: Option<usize>,
    /// What it says without rows.
    pub empty: String,
    /// What is typed to find a branch, and what it says while empty.
    pub query: Line,
    pub placeholder: String,
    /// The row creating the branch typed, under the search.
    pub create: Option<BranchCreate>,
    /// The groups' headings (`Local`, `Remote`), each over the row of its
    /// index; `rows.len()` puts one under them all.
    pub headings: Vec<(usize, String)>,
    /// The part of the first line (a heading or a row) scrolled past, 0
    /// to 1; with some, the lines have one more under the list, shown in
    /// part.
    pub part: f64,
}

/// The branch menu's row creating the branch typed in its search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchCreate {
    /// `Create feat/x from main`, or why it can't be.
    pub label: String,
    /// Why the name typed can't be a branch's: the row is muted.
    pub problem: Option<String>,
    /// Enter creates it.
    pub picked: bool,
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
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Its branch has no upstream yet.
    pub no_upstream: bool,
    pub stashes: u32,
    /// What it is in the middle of, or what went wrong: `MERGING · 2`,
    /// `REBASING 2/5`, `login needed`, `detached`.
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
    /// What it means, over it while the pointer is on it.
    pub tip: String,
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
    /// `Undo`, `RetryFailed`, with their keys.
    pub buttons: Vec<Hint>,
}

/// An action in the footer: a button with its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub button: ReposButton,
    /// What it says: the button's label, a merge's with its branch
    /// (`Merge main`).
    pub label: String,
    /// As shown: `Ctrl+N`.
    pub key: String,
    /// `None` when it can be done now, else why not (in a tooltip-like
    /// muted line when hovered).
    pub disabled: Option<String>,
}

/// The right side: every change of every repository one after another,
/// or the selected repository's log or stashes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailView {
    pub tab: DetailTab,
    /// Above it: `All changes · 2 repositories · 5 files`, or the
    /// repository's name.
    pub title: String,
    /// The rows in view, scrolled; with `sticky`, the first is the header
    /// of the file the rest start in, pinned at the top.
    pub rows: Vec<DetailRow>,
    pub sticky: bool,
    /// How each of `rows` is marked: the diff's cursor, the range it
    /// reaches back to, a comment on it. Empty for none.
    pub marks: Vec<RowMark>,
    /// Actions at the right of the title row (copying the review), and
    /// on rows of `rows` by index (commenting on the lines picked, the
    /// comment being written kept or dropped), right-aligned on them.
    pub title_buttons: Vec<Hint>,
    pub row_buttons: Vec<(usize, Hint)>,
    /// The part of the first row scrolled (under the pinned header, if
    /// any) past, 0 to 1; with some, `rows` has one more under the pane,
    /// shown in part.
    pub part: f64,
    /// Where the rows start among them all, and how many there are: where
    /// the scrollbar stands.
    pub first: usize,
    pub total: usize,
    /// The detail has the keyboard.
    pub focused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DetailTab {
    #[default]
    Diff,
    Log,
    /// The repository's stashes.
    Stashes,
}

/// How a row of the diff is marked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowMark {
    /// The cursor is on it, or it is between the cursor and where its
    /// range reaches back to: the rows lit and outlined as one.
    pub selected: bool,
    /// The first of those rows and the last: the outline closes there.
    pub top: bool,
    pub bottom: bool,
    /// A comment is on it.
    pub noted: bool,
}

/// A row of the detail pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailRow {
    /// A line of a log, or what the pane says (`Reading…`).
    Text(DetailLine),
    /// A repository's header over its files.
    Repo(DiffRepo),
    /// A file's header over its hunks.
    File(DiffFile),
    /// Between hunks: what git says they are in (a function's first line),
    /// and the line the hunk starts at.
    Hunk {
        context: String,
        number: usize,
    },
    Line(crate::diff::Line),
    /// What isn't a hunk: `Binary file`, an untracked folder's files.
    Note(String),
    /// A row of a review comment: the first and last of its rows round
    /// its card.
    Comment {
        text: String,
        first: bool,
        last: bool,
    },
    /// A row of the comment being written: with the caret on it, its byte
    /// in `text`; the selection on it, by bytes in `text`.
    Editor {
        text: String,
        caret: Option<usize>,
        selection: Range<usize>,
        first: bool,
        last: bool,
    },
}

/// A repository's header in the diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffRepo {
    pub label: String,
    pub branch: Option<String>,
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

/// A file's header in the diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffFile {
    /// Its repository's name, on the header pinned at the top.
    pub repo: Option<String>,
    /// From the repository's top folder; a rename as `old → new`.
    pub path: String,
    /// `git::Change::code`.
    pub code: char,
    pub check: Check,
    pub folded: bool,
    pub added: usize,
    pub removed: usize,
    /// It is the tree's selected line.
    pub selected: bool,
}

/// A line of a log, in the monospace font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailLine {
    pub text: String,
    pub tone: DiffTone,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffTone {
    #[default]
    Plain,
    /// A command run.
    Header,
    /// What a failed command printed.
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
    Commit,
    CommitPush,
    Push,
    ForcePush,
    Fetch,
    Pull,
    Merge,
    Rebase,
    /// Merge a branch picked in the branch menu, or rebase onto it.
    MergeFrom,
    RebaseFrom,
    OpenInZed,
    MarkResolved,
    Continue,
    Abort,
    Undo,
    RetryFailed,
    RunInTerminal,
    OpenWorkspace,
    /// The selected repository's stashes.
    Stash,
    StashPop,
    StashDrop,
    /// The panel's keys, on a sheet.
    Help,
    /// On a confirm sheet, for a branch picked in the menu.
    DeleteBranch,
    /// On the preflight sheet.
    All,
    CleanOnly,
    /// On the divergence sheet.
    PullRebase,
    PullMerge,
    SkipDiverged,
    Cancel,
    /// On the keys sheet.
    Close,
    /// On the sheet saying the branch to create exists already: switch
    /// to it, create it where it is missing, or edit its name.
    SwitchExisting,
    CreateMissing,
    EditName,
    /// The diff's review comments, copied for an agent; and dropped
    /// (asked first, on a sheet).
    CopyReview,
    ClearReview,
    /// A comment on the diff lines picked; the comment being written
    /// kept, dropped, or (one written before) deleted.
    Comment,
    SaveComment,
    DiscardComment,
    DeleteComment,
}

impl ReposButton {
    /// What it says.
    pub fn label(self) -> &'static str {
        match self {
            ReposButton::Commit => "Commit",
            ReposButton::CommitPush => "Commit and push",
            ReposButton::Push => "Push",
            ReposButton::ForcePush => "Force push",
            ReposButton::Fetch => "Fetch",
            ReposButton::Pull => "Pull",
            ReposButton::Merge => "Merge base",
            ReposButton::Rebase => "Rebase on base",
            ReposButton::MergeFrom => "Merge from…",
            ReposButton::RebaseFrom => "Rebase onto…",
            ReposButton::OpenInZed => "Open in Zed",
            ReposButton::MarkResolved => "Resolved",
            ReposButton::Continue => "Continue",
            ReposButton::Abort => "Abort",
            ReposButton::Undo => "Undo",
            ReposButton::RetryFailed => "Retry failed",
            ReposButton::RunInTerminal => "Run in terminal",
            ReposButton::OpenWorkspace => "Open workspace",
            ReposButton::Stash => "Stash",
            ReposButton::StashPop => "Pop stash",
            ReposButton::StashDrop => "Drop stash",
            ReposButton::Help => "Keys",
            ReposButton::DeleteBranch => "Delete",
            ReposButton::All => "All",
            ReposButton::CleanOnly => "Clean only",
            ReposButton::PullRebase => "Rebase",
            ReposButton::PullMerge => "Merge",
            ReposButton::SkipDiverged => "Skip those",
            ReposButton::Cancel => "Cancel",
            ReposButton::Close => "Close",
            ReposButton::SwitchExisting => "Switch to it",
            ReposButton::CreateMissing => "Create where missing",
            ReposButton::EditName => "Edit name",
            ReposButton::CopyReview => "Copy review",
            ReposButton::ClearReview => "Clear review",
            ReposButton::Comment => "Comment",
            ReposButton::SaveComment => "Save",
            ReposButton::DiscardComment => "Discard",
            ReposButton::DeleteComment => "Delete",
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
    /// A repository line's chip: it says what it means.
    Chip(usize),
    Field,
    /// A footer action, a result bar button or a sheet button.
    Button(ReposButton),
    DetailTab(DetailTab),
    /// The detail pane (for the wheel).
    Detail,
    /// A row of the detail, by index into `DetailView::rows`.
    DiffRow(usize),
    /// A file header's checkbox, and its fold chevron.
    DiffCheck(usize),
    DiffFold(usize),
    /// The branch picker's button.
    BranchButton,
    /// A row of the branch menu, by index into `BranchMenuView::rows`.
    BranchRow(usize),
    /// A branch menu row's delete button, by the same index.
    BranchDelete(usize),
    /// The branch menu's row creating the branch typed.
    BranchCreate,
    /// The branch menu, on no row (for the wheel).
    BranchMenu,
    /// A mark on a repository's line, by index into `ReposView::lines`:
    /// it says what it means.
    Mark(usize, RepoMark),
    /// The footer's "more" button, for the actions it has no room for.
    More,
    /// The line between the tree and the detail, to drag.
    Splitter,
    /// Inside the panel, on nothing.
    Panel,
}

/// A mark after a repository's branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoMark {
    /// A dot: changes not committed.
    Dirty,
    /// An arrow up and a count: commits to push.
    Ahead,
    /// An arrow down and a count: commits to pull.
    Behind,
    /// `2 stashed`, after the marks.
    Stashes,
}

impl RepoMark {
    /// What the mark says of `repo`, over it while the pointer is on it.
    pub fn tip(self, repo: &RepoLine) -> String {
        let commits = |n: u32| match n {
            1 => "1 commit".to_owned(),
            n => format!("{n} commits"),
        };
        match self {
            RepoMark::Dirty => "Changes not committed".into(),
            RepoMark::Ahead => format!("{} to push: click to push", commits(repo.ahead)),
            RepoMark::Behind => format!("{} to pull: click to pull", commits(repo.behind)),
            RepoMark::Stashes => "Click to list the stashes".into(),
        }
    }
}

/// Where a repository line's parts go, between `x` and `right` at `s`
/// pixels a logical pixel, its text measured by `width(text, size)`:
/// the painter and the hit test work it out alike.
struct LineSpans {
    /// The chip's left edge and width.
    chip: Option<(f32, f32)>,
    /// Where the label, branch and marks end at the latest.
    end: f32,
    /// Where the branch starts, when there is room for it.
    branch: Option<f32>,
    marks: Vec<MarkSpan>,
    /// Where `no upstream` starts, when there is room for it.
    no_upstream: Option<f32>,
    /// Where the notes start.
    pen: f32,
}

/// What a repository line says of its stashes.
fn stash_note(repo: &RepoLine) -> String {
    format!("{} stashed", repo.stashes)
}

/// A mark that fits: its shape from `x`, `w` wide, then its count up to
/// `end`.
struct MarkSpan {
    mark: RepoMark,
    x: f32,
    w: f32,
    end: f32,
}

fn line_spans(
    repo: &RepoLine,
    (x, right): (f32, f32),
    gap: f32,
    s: f32,
    width: &dyn Fn(&str, f32) -> f32,
) -> LineSpans {
    let mut end = right;
    let chip = repo.chip.as_ref().map(|chip| {
        let pad = (CHIP_PAD as f32 * s).round();
        let cw = (width(&chip.text, DETAIL_FONT_SIZE) + 2.0 * pad).round();
        let cx = (right - cw).max(x);
        end = cx - gap;
        (cx, cw)
    });
    let mut pen = (x + width(&repo.label, FONT_SIZE)).min(end).max(x);
    let branch = (pen + gap < end).then(|| {
        let bx = pen + gap;
        let text = repo.branch.as_deref().unwrap_or("detached");
        pen = (bx + width(text, SMALL_FONT_SIZE)).min(end);
        bx
    });
    let (w, mark_gap) = (
        (GIT_MARK_WIDTH as f32 * s).round(),
        (GIT_MARK_GAP as f32 * s).round(),
    );
    let wanted = [
        (RepoMark::Dirty, repo.dirty, w, 0),
        (RepoMark::Ahead, repo.ahead > 0, w, repo.ahead),
        (RepoMark::Behind, repo.behind > 0, w, repo.behind),
    ];
    let mut marks = Vec::new();
    for (mark, on, w, count) in wanted {
        if !on {
            continue;
        }
        let mx = pen + mark_gap;
        if mx + w > end {
            break;
        }
        let mut mark_end = mx + w;
        if count > 0 {
            mark_end = (mark_end + s + width(&count.to_string(), DETAIL_FONT_SIZE)).min(end);
        }
        marks.push(MarkSpan {
            mark,
            x: mx,
            w,
            end: mark_end,
        });
        pen = mark_end;
    }
    // The notes, as far as they fit: the stash count is a mark too.
    let no_upstream = (repo.no_upstream && pen + gap < end).then(|| {
        let nx = pen + gap;
        pen = (nx + width("no upstream", SMALL_FONT_SIZE)).min(end);
        nx
    });
    if repo.stashes > 0 && pen + gap < end {
        let nx = pen + gap;
        let note_end = (nx + width(&stash_note(repo), SMALL_FONT_SIZE)).min(end);
        marks.push(MarkSpan {
            mark: RepoMark::Stashes,
            x: nx,
            w: note_end - nx,
            end: note_end,
        });
        pen = note_end;
    }
    LineSpans {
        chip,
        end,
        branch,
        marks,
        no_upstream,
        pen,
    }
}

/// Where the repositories panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReposLayout {
    pub panel: Rect,
    pub header: Rect,
    /// The tree's area.
    pub tree: Rect,
    /// Per tree line: its rect, its checkbox's, and a repository line's
    /// expander's. A scrolled tree's first and last lines reach out of
    /// `tree_list`.
    pub lines: Vec<(Rect, Rect, Option<Rect>)>,
    /// Where the tree's lines show, across the tree.
    pub tree_list: Rect,
    /// The marks on repository lines, by index into `lines`.
    pub marks: Vec<(usize, RepoMark, Rect)>,
    /// The chips on repository lines, by index into `lines`.
    pub chips: Vec<(usize, Rect)>,
    /// The line between the tree and the detail, as wide as a press takes
    /// hold of it.
    pub splitter: Rect,
    pub detail: Rect,
    pub detail_tabs: Vec<(DetailTab, Rect)>,
    /// Per detail row in view, its rect; a file header's checkbox and
    /// fold chevron, by index into them. A scrolled pane's first and last
    /// rows reach out of `detail_list`.
    pub detail_rows: Vec<Rect>,
    /// Where the detail's scrolled rows show, across the pane: under the
    /// header pinned at the top, if any.
    pub detail_list: Rect,
    pub detail_checks: Vec<(usize, Rect)>,
    pub detail_folds: Vec<(usize, Rect)>,
    /// The buttons of the detail's title row, and of its rows by index
    /// into `detail_rows`.
    pub detail_title_buttons: Vec<(ReposButton, Rect)>,
    pub detail_row_buttons: Vec<(usize, ReposButton, Rect)>,
    pub field: Rect,
    pub result: Option<Rect>,
    /// The result bar's buttons, the footer's, and the sheet's.
    pub buttons: Vec<(ReposButton, Rect)>,
    pub footer: Rect,
    /// The footer's "more" button, when it has no room for every action,
    /// and the actions behind it, by index into `ReposView::hints`.
    pub more: Option<Rect>,
    pub overflow: Vec<usize>,
    pub sheet: Option<Rect>,
    /// The branch picker's button, and its menu with a rect per row while
    /// open.
    pub branch: Rect,
    pub branch_menu: Option<(Rect, Vec<Rect>)>,
    /// The branch menu's search, at its top, and the row under it creating
    /// the branch typed.
    pub branch_search: Option<Rect>,
    pub branch_create: Option<Rect>,
    /// The delete buttons of the menu's rows that have one, by row.
    pub branch_deletes: Vec<(usize, Rect)>,
    /// The menu's group headings in view, by index into
    /// `BranchMenuView::headings`.
    pub branch_headings: Vec<(usize, Rect)>,
    /// Where the menu's headings and rows show; a scrolled menu's first
    /// and last lines reach out of it.
    pub branch_list: Option<Rect>,
}

/// Inside the panel's parts: what the painter needs besides `ReposLayout`.
struct Parts {
    layout: ReposLayout,
    /// The line under the field saying what went wrong.
    error: Option<Rect>,
    /// Where the detail pane's lines go, under its title row.
    body: Rect,
    /// The result bar's buttons, by index into `ResultBar::buttons`.
    result_buttons: Vec<(usize, Rect)>,
    /// The footer's buttons, by index into `ReposView::hints`.
    hints: Vec<(usize, Rect)>,
    /// The sheet's title, a rect per line shown, and its buttons by index
    /// into `SheetView::buttons`.
    sheet_title: Option<Rect>,
    sheet_lines: Vec<Rect>,
    sheet_buttons: Vec<(usize, Rect)>,
}

/// The panel on an output of `size`: centred, a margin in from its edges,
/// at most `REPOS_MAX_WIDTH` by `REPOS_MAX_HEIGHT`.
fn panel_rect(size: Size<i32, Logical>) -> Rect {
    let w = REPOS_MAX_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let h = REPOS_MAX_HEIGHT.min(size.h - 2 * HELP_MARGIN).max(1);
    Rectangle::new(
        (((size.w - w) / 2).max(0), ((size.h - h) / 2).max(0)).into(),
        (w, h).into(),
    )
}

/// How wide `text` is at `size` in logical pixels, measured with the UI
/// font; without one, a guess of `per_char` a character on the wide side
/// (the painter draws inside whatever it is given).
fn text_extent(text: &str, size: f32, per_char: f64) -> i32 {
    match measure(text, size) {
        Some(w) => w.ceil() as i32,
        None => (text.chars().count() as f64 * per_char + GUESS_SLACK).ceil() as i32,
    }
}

/// How wide a footer action is: its key on a keycap, then its label; one
/// without a key, its label on a button.
fn hint_width(hint: &Hint) -> i32 {
    if hint.key.is_empty() {
        let label = text_extent(&hint.label, SMALL_FONT_SIZE, SMALL_CHAR);
        return 2 * HINT_PAD + 2 * CAP_PADDING + label;
    }
    2 * HINT_PAD
        + 2 * CAP_PADDING
        + text_extent(&hint.key, KEY_FONT_SIZE, KEY_CHAR)
        + HINT_KEY_GAP
        + text_extent(&hint.label, SMALL_FONT_SIZE, SMALL_CHAR)
}

/// The detail's title row buttons and its rows', as laid out.
type DetailButtons<'a> = (&'a [(ReposButton, Rect)], &'a [(usize, ReposButton, Rect)]);

/// How wide a button on a diff row is: its label, then its key muted.
fn row_chip_width(hint: &Hint) -> i32 {
    let key = match hint.key.is_empty() {
        true => 0,
        false => ROW_CHIP_KEY_GAP + text_extent(&hint.key, KEY_FONT_SIZE, KEY_CHAR),
    };
    2 * ROW_CHIP_PAD + text_extent(&hint.label, SMALL_FONT_SIZE, SMALL_CHAR) + key
}

/// What the footer's "more" button says with `n` actions behind it.
fn more_label(n: usize) -> String {
    format!("{n} more…")
}

/// How wide the "more" button is with `n` actions behind it.
fn more_width(n: usize) -> i32 {
    2 * HINT_PAD + 2 * CAP_PADDING + text_extent(&more_label(n), SMALL_FONT_SIZE, SMALL_CHAR)
}

/// How wide a sheet's button is.
fn sheet_button_width(button: ReposButton) -> i32 {
    (text_extent(button.label(), FONT_SIZE, BUTTON_CHAR) + 2 * SHEET_BUTTON_PAD)
        .max(SHEET_BUTTON_MIN)
}

/// The footer's actions split into rows of at most `w` logical pixels, as
/// many rows as `REPOS_FOOTER_ROWS`: per row, (index into `hints`, width);
/// then those that don't fit, which go behind the "more" button, for
/// which the last row keeps room.
fn hint_rows(hints: &[Hint], w: i32) -> (Vec<Vec<(usize, i32)>>, Vec<usize>) {
    let fit = |reserve: i32| {
        let mut rows: Vec<Vec<(usize, i32)>> = vec![Vec::new()];
        let mut used = 0;
        let mut rest = Vec::new();
        for (i, hint) in hints.iter().enumerate() {
            if !rest.is_empty() {
                rest.push(i);
                continue;
            }
            let width = hint_width(hint).min(w);
            let last = rows.len() == REPOS_FOOTER_ROWS;
            let room = if last { w - reserve } else { w };
            let row = rows.last_mut().expect("a row");
            let more = if row.is_empty() {
                width
            } else {
                HINT_GAP + width
            };
            if used + more <= room {
                row.push((i, width));
                used += more;
            } else if !last {
                rows.push(vec![(i, width)]);
                used = width;
            } else {
                rest.push(i);
            }
        }
        (rows, rest)
    };
    let (rows, rest) = fit(0);
    if rest.is_empty() {
        return (rows, rest);
    }
    // With a "more" button on the last row, a few less may fit, and the
    // button then says so.
    let (_, rest) = fit(HINT_GAP + more_width(rest.len()));
    fit(HINT_GAP + more_width(rest.len()))
}

/// How high the footer is with `rows` rows of actions.
fn footer_h(rows: usize) -> i32 {
    2 * REPOS_FOOTER_PAD + rows.max(1) as i32 * REPOS_HINT_ROW
}

/// The tree's width in a panel `w` wide, with `share` of it.
fn tree_width(w: i32, share: f64) -> i32 {
    let share = share.clamp(REPOS_TREE_SHARE_MIN, REPOS_TREE_SHARE_MAX);
    ((w as f64 * share).round() as i32)
        .max(REPOS_TREE_MIN.min(w / 2))
        .min(w)
}

/// The tree's share of the panel for a press dragging the line between
/// the tree and the detail to `x` (output coordinates), on an output of
/// `size`.
pub fn repos_share_at(x: f64, size: Size<i32, Logical>) -> f64 {
    let panel = panel_rect(size);
    let share = (x - panel.loc.x as f64) / panel.size.w.max(1) as f64;
    share.clamp(REPOS_TREE_SHARE_MIN, REPOS_TREE_SHARE_MAX)
}

/// How wide the branch picker's button is for `label`.
fn picker_width(label: &str) -> i32 {
    let text = text_extent(label, FONT_SIZE, BUTTON_CHAR);
    (2 * PICKER_PAD + PICKER_ICON + 2 * PICKER_GAP + text + PICKER_CHEVRON).max(PICKER_MIN)
}

/// How many tree lines and detail lines the panel shows at once on an
/// output of `size`: with the error line, the result bar and the footer's
/// rows all there, so that the page doesn't change as they come and go.
pub fn repos_page(size: Size<i32, Logical>, density: Density) -> (usize, usize) {
    let panel = panel_rect(size);
    let bottom = REPOS_FIELD_ROW + ERROR_HEIGHT + REPOS_RESULT + footer_h(REPOS_FOOTER_ROWS);
    let middle = panel.size.h - REPOS_HEADER - bottom;
    let tree = (middle - 2 * REPOS_PAD) / repos_row(density);
    let detail = (middle - REPOS_DETAIL_TITLE - 2 * REPOS_PAD) / repos_detail_line(density);
    (tree.max(1) as usize, detail.max(1) as usize)
}

/// How many characters of the monospace font a review comment's row of
/// the diff holds, on an output of `size` with the tree's share `share`.
pub fn repos_detail_cols(size: Size<i32, Logical>, share: f64) -> usize {
    let w = panel_rect(size).size.w;
    let detail = (w - tree_width(w, share)).max(0);
    let text = detail - 2 * HELP_PADDING - SCROLLBAR - 2 * DIFF_INSET - COMMENT_INDENT;
    let advance = measure_mono("0", REPOS_MONO_SIZE)
        .filter(|a| *a > 0.0)
        .unwrap_or(REPOS_MONO_SIZE * 0.6);
    ((text.max(0) as f32 / advance).floor() as usize).max(16)
}

/// How far up a scrolled list's rows `height` high are moved, logical
/// pixels: by the `part` of its first row scrolled past, less how far it
/// is pulled down past an end (`over`).
fn scrolled_by(part: f64, over: f64, height: i32) -> f64 {
    part * height as f64 - over
}

/// A tree line in `rect`, its checkbox's rect and a repository line's
/// expander's.
fn line_rects(line: &TreeLine, rect: Rect) -> (Rect, Rect, Option<Rect>) {
    let (x, y, h) = (rect.loc.x, rect.loc.y, rect.size.h);
    let indent = match line {
        TreeLine::Repo(_) => 0,
        TreeLine::File(_) => REPOS_INDENT,
    };
    let check_x = x + REPOS_LINE_INSET + indent;
    let check = Rectangle::new((check_x, y).into(), (REPOS_CHECK, h).into());
    let expander = match line {
        TreeLine::Repo(repo) if repo.expanded.is_some() => Some(Rectangle::new(
            (check_x + REPOS_CHECK, y).into(),
            (REPOS_EXPANDER, h).into(),
        )),
        _ => None,
    };
    (rect, check, expander)
}

/// Where a buffer holding the panel `panel` and its shadow has its top
/// left, in output coordinates negated: what moves a rect onto it.
fn buffer_origin(panel: Rect) -> Point<i32, Logical> {
    Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y))
}

/// `rect` moved by `by`.
fn moved(rect: Rect, by: Point<i32, Logical>) -> Rect {
    Rectangle::new(rect.loc + by, rect.size)
}

/// The tree's scrollbar in `layout`, and how many of `view`'s lines it
/// shows whole.
fn tree_bar(view: &ReposView, layout: &ReposLayout) -> (Rect, usize) {
    let tree = layout.tree;
    let track = Rectangle::new(
        (
            tree.loc.x + tree.size.w - REPOS_PAD - SCROLLBAR,
            tree.loc.y + REPOS_PAD,
        )
            .into(),
        (SCROLLBAR, (tree.size.h - 2 * REPOS_PAD).max(0)).into(),
    );
    let whole = view
        .lines
        .len()
        .saturating_sub(usize::from(view.part > 0.0));
    (track, layout.lines.len().min(whole))
}

/// Where the thumb of a scrollbar in `track` goes for a list of `total`
/// rows showing `shown` of them from row `pos` (to the fraction): its top,
/// in logical pixels to the fraction, and its height. None when they all
/// fit.
fn thumb_place(track: Rect, pos: f64, shown: usize, total: usize) -> Option<(f64, i32)> {
    if total <= shown || shown == 0 {
        return None;
    }
    let h = track.size.h as f64;
    let thumb = (h * shown as f64 / total as f64)
        .max((SCROLLBAR_MIN as f64).min(h))
        .min(h)
        .round();
    let pos = pos.clamp(0.0, (total - shown) as f64);
    let top = track.loc.y as f64 + (h - thumb) * pos / (total - shown) as f64;
    Some((top, thumb as i32))
}

/// A tree line as drawn: what its pixels come from, but its place.
#[derive(Clone, Debug, PartialEq)]
struct TreeRow {
    line: TreeLine,
    selected: bool,
    /// Selected while the field has the keyboard: lit less.
    dim: bool,
    /// What the pointer is over on it, as on the first line.
    hover: Option<ReposHit>,
}

/// Tree line `i` of `view`, as drawn.
fn tree_row(view: &ReposView, i: usize) -> TreeRow {
    use ReposHit::*;
    let hover = match view.hover {
        Some(Line(j)) if j == i => Some(Line(0)),
        Some(Check(j)) if j == i => Some(Check(0)),
        Some(Expander(j)) if j == i => Some(Expander(0)),
        Some(Chip(j)) if j == i => Some(Chip(0)),
        Some(Mark(j, mark)) if j == i => Some(Mark(0, mark)),
        _ => None,
    };
    let selected = view.selected == Some(i);
    TreeRow {
        line: view.lines[i].clone(),
        selected,
        dim: selected && view.field_focused,
        hover,
    }
}

/// Where the parts of the panel showing `view` go on an output of `size`.
pub fn repos_layout(view: &ReposView, size: Size<i32, Logical>) -> ReposLayout {
    parts(view, size).layout
}

/// The panel's parts: the header across the top; the tree on the left and
/// the detail pane on the right under it; then across the bottom the
/// field, the error line, the result bar and the footer's actions.
fn parts(view: &ReposView, size: Size<i32, Logical>) -> Parts {
    let panel = panel_rect(size);
    let (x, top, w) = (panel.loc.x, panel.loc.y, panel.size.w);
    let bottom_edge = top + panel.size.h;
    let inner_x = x + HELP_PADDING;
    let inner_w = (w - 2 * HELP_PADDING).max(0);
    let header = Rectangle::new((x, top).into(), (w, REPOS_HEADER).into());
    // The branch picker at the header's left, no more than a third of it.
    let pw = picker_width(&view.branch.label).min(inner_w / 3).max(1);
    let branch = Rectangle::new(
        (inner_x, top + (REPOS_HEADER - ROW_BUTTON) / 2).into(),
        (pw, ROW_BUTTON).into(),
    );

    let row_h = repos_row(view.density);
    let line_h = repos_detail_line(view.density);

    // From the bottom up: the footer, the result bar, the error, the field.
    let (rows, overflow) = hint_rows(&view.hints, inner_w);
    let footer_height = footer_h(rows.len());
    let footer_y = bottom_edge - footer_height;
    let footer = Rectangle::new((x, footer_y).into(), (w, footer_height).into());
    let mut y = footer_y;
    let result = view.result.as_ref().map(|_| {
        y -= REPOS_RESULT;
        Rectangle::new((x, y).into(), (w, REPOS_RESULT).into())
    });
    let error = view.error.as_ref().map(|_| {
        y -= ERROR_HEIGHT;
        Rectangle::new((inner_x, y).into(), (inner_w, ERROR_HEIGHT).into())
    });
    y -= REPOS_FIELD_ROW;
    let field = Rectangle::new(
        (inner_x, y + (REPOS_FIELD_ROW - REPOS_FIELD) / 2).into(),
        (inner_w, REPOS_FIELD).into(),
    );
    let middle_y = top + REPOS_HEADER;
    let middle_h = (y - middle_y).max(0);

    // The tree and its lines, short of its scrollbar.
    let tree_w = tree_width(w, view.tree_share);
    let tree = Rectangle::new((x, middle_y).into(), (tree_w, middle_h).into());
    let (line_x, line_w) = (x + REPOS_PAD, (tree_w - 2 * REPOS_PAD - SCROLLBAR).max(0));
    let tree_end = middle_y + middle_h - REPOS_PAD;
    let lines_y = middle_y + REPOS_PAD;
    // Scrolled part of the first line, the lines start up by as much and
    // the last is shown in part, cut at where the others end; pulled past
    // an end, they move with the pull.
    let tree_shift = scrolled_by(view.part, view.over, row_h).round() as i32;
    let whole_lines = view
        .lines
        .len()
        .saturating_sub(usize::from(view.part > 0.0));
    let (mut lines, mut marks, mut chips) = (Vec::new(), Vec::new(), Vec::new());
    let mut tree_shown = 0;
    for (i, line) in view.lines.iter().enumerate() {
        let at = lines_y + i as i32 * row_h;
        if i < whole_lines && at + row_h > tree_end {
            break;
        }
        tree_shown += usize::from(i < whole_lines);
        let ly = at - tree_shift;
        let rect = Rectangle::new((line_x, ly).into(), (line_w, row_h).into());
        let (_, check, expander) = line_rects(line, rect);
        if let TreeLine::Repo(repo) = line {
            // Measured as the painter measures, at one pixel a pixel.
            let width = |text: &str, size: f32| measure(text, size).unwrap_or(0.0);
            let x = (check.loc.x + REPOS_CHECK + REPOS_EXPANDER) as f32;
            let right = (line_x + line_w) as f32 - PADDING;
            let spans = line_spans(repo, (x, right), FIELD_GAP as f32, 1.0, &width);
            for m in spans.marks {
                let mx = m.x.floor() as i32;
                let mw = (m.end.ceil() as i32 - mx).max(1);
                let rect = Rectangle::new((mx, ly).into(), (mw, row_h).into());
                marks.push((i, m.mark, rect));
            }
            if let Some((cx, cw)) = spans.chip {
                let cx = cx.floor() as i32;
                let cw = (cw.ceil() as i32).max(1);
                chips.push((i, Rectangle::new((cx, ly).into(), (cw, row_h).into())));
            }
        }
        lines.push((rect, check, expander));
    }
    let tree_list = Rectangle::new(
        (x, lines_y).into(),
        (tree_w, tree_shown as i32 * row_h).into(),
    );

    // The detail pane, its tabs at the right of its title row; the line
    // between it and the tree can be dragged.
    let detail = Rectangle::new(
        (x + tree_w, middle_y).into(),
        ((w - tree_w).max(0), middle_h).into(),
    );
    let splitter = Rectangle::new(
        (x + tree_w - SPLITTER_GRAB, middle_y).into(),
        (2 * SPLITTER_GRAB, middle_h).into(),
    );
    let tab_y = middle_y + (REPOS_DETAIL_TITLE - ROW_BUTTON) / 2;
    let tabs = [DetailTab::Diff, DetailTab::Log, DetailTab::Stashes];
    let tabs_w = tabs.len() as i32 * (REPOS_TAB_WIDTH + REPOS_TAB_GAP) - REPOS_TAB_GAP;
    let tabs_x = detail.loc.x + detail.size.w - HELP_PADDING - tabs_w;
    let detail_tabs = tabs
        .into_iter()
        .enumerate()
        .map(|(k, tab)| {
            let tx = tabs_x + k as i32 * (REPOS_TAB_WIDTH + REPOS_TAB_GAP);
            let size = (REPOS_TAB_WIDTH, ROW_BUTTON);
            (tab, Rectangle::new((tx, tab_y).into(), size.into()))
        })
        .collect();
    let body = Rectangle::new(
        (
            detail.loc.x + HELP_PADDING,
            middle_y + REPOS_DETAIL_TITLE + REPOS_PAD,
        )
            .into(),
        (
            (detail.size.w - 2 * HELP_PADDING - SCROLLBAR).max(0),
            (middle_h - REPOS_DETAIL_TITLE - 2 * REPOS_PAD).max(0),
        )
            .into(),
    );

    // The detail's rows; a file header's chevron, then its checkbox.
    let (mut detail_rows, mut detail_checks, mut detail_folds) =
        (Vec::new(), Vec::new(), Vec::new());
    // Scrolled part of the first row under the pinned header, the rows
    // start up by as much (under the header) and the last is shown in
    // part, cut at where the others end.
    let pinned = usize::from(view.detail.sticky);
    let detail_shift = wheel::shift(view.detail.part, line_h);
    let under = usize::from(view.detail.part > 0.0);
    let whole_rows = view.detail.rows.len().saturating_sub(under);
    let mut detail_shown = 0;
    for (i, row) in view.detail.rows.iter().enumerate() {
        let at = body.loc.y + i as i32 * line_h;
        if i < whole_rows && at + line_h > body.loc.y + body.size.h {
            break;
        }
        detail_shown += usize::from(i < whole_rows);
        let ry = if i < pinned { at } else { at - detail_shift };
        if let DetailRow::File(_) = row {
            let fold = Rectangle::new((body.loc.x, ry).into(), (DIFF_FOLD, line_h).into());
            let check = Rectangle::new(
                (body.loc.x + DIFF_FOLD, ry).into(),
                (DIFF_CHECK, line_h).into(),
            );
            detail_folds.push((i, fold));
            detail_checks.push((i, check));
        }
        detail_rows.push(Rectangle::new(
            (body.loc.x, ry).into(),
            (body.size.w, line_h).into(),
        ));
    }
    let detail_list = {
        let top = body.loc.y + pinned.min(detail_shown) as i32 * line_h;
        let bottom = body.loc.y + detail_shown as i32 * line_h;
        Rectangle::new(
            (detail.loc.x, top).into(),
            (detail.size.w, bottom - top).into(),
        )
    };
    // The title row's buttons left of the tabs, the rows' at their right,
    // each right-aligned in order; none on a title too narrow for them.
    let mut detail_title_buttons = Vec::new();
    let mut right = tabs_x - REPOS_TAB_GAP * 2;
    for hint in view.detail.title_buttons.iter().rev() {
        let bw = hint_width(hint);
        right -= bw;
        if right < detail.loc.x + detail.size.w / 3 {
            break;
        }
        let rect = Rectangle::new((right, tab_y).into(), (bw, ROW_BUTTON).into());
        detail_title_buttons.push((hint.button, rect));
        right -= REPOS_BUTTON_GAP;
    }
    detail_title_buttons.reverse();
    let mut detail_row_buttons = Vec::new();
    let mut ends: Vec<(usize, i32)> = Vec::new();
    for (i, hint) in view.detail.row_buttons.iter().rev() {
        let Some(row) = detail_rows.get(*i) else {
            continue;
        };
        let end = match ends.iter_mut().find(|(j, _)| j == i) {
            Some((_, end)) => end,
            None => {
                ends.push((*i, row.loc.x + row.size.w - ROW_CHIP_INSET));
                &mut ends.last_mut().expect("just pushed").1
            }
        };
        let bw = row_chip_width(hint);
        *end -= bw;
        let rect = Rectangle::new(
            (*end, row.loc.y + ROW_CHIP_INSET).into(),
            (bw, (row.size.h - 2 * ROW_CHIP_INSET).max(1)).into(),
        );
        detail_row_buttons.push((*i, hint.button, rect));
        *end -= ROW_CHIP_GAP;
    }
    detail_row_buttons.reverse();

    // The result bar's buttons, right-aligned.
    let mut result_buttons = Vec::new();
    if let (Some(bar), Some(rect)) = (&view.result, result) {
        let mut right = inner_x + inner_w;
        let by = rect.loc.y + (REPOS_RESULT - ROW_BUTTON) / 2;
        for (i, hint) in bar.buttons.iter().enumerate().rev() {
            let bw = hint_width(hint);
            right -= bw;
            if right < inner_x + inner_w / 2 {
                break;
            }
            result_buttons.push((
                i,
                Rectangle::new((right, by).into(), (bw, ROW_BUTTON).into()),
            ));
            right -= REPOS_BUTTON_GAP;
        }
        result_buttons.reverse();
    }

    // The footer's actions, row by row from the left; the "more" button
    // after the last row's.
    let mut hints = Vec::new();
    let mut more = None;
    for (r, row) in rows.iter().enumerate() {
        let ry = footer_y + REPOS_FOOTER_PAD + r as i32 * REPOS_HINT_ROW;
        let by = ry + (REPOS_HINT_ROW - ROW_BUTTON) / 2;
        let mut hx = inner_x;
        for &(i, hw) in row {
            hints.push((i, Rectangle::new((hx, by).into(), (hw, ROW_BUTTON).into())));
            hx += hw + HINT_GAP;
        }
        if r + 1 == rows.len() && !overflow.is_empty() {
            let mw = more_width(overflow.len()).min((inner_x + inner_w - hx).max(1));
            more = Some(Rectangle::new((hx, by).into(), (mw, ROW_BUTTON).into()));
        }
    }

    // The sheet, a card centred over the tree and detail.
    let (mut sheet, mut sheet_title, mut sheet_lines, mut sheet_buttons) =
        (None, None, Vec::new(), Vec::new());
    if let Some(s) = &view.sheet {
        let fixed = 2 * SHEET_PAD + SHEET_TITLE + 2 * SHEET_GAP + SHEET_BUTTON;
        let room = (middle_h - 2 * REPOS_PAD - fixed).max(0) / SHEET_LINE;
        let shown = s.lines.len().min(room as usize);
        let sw = SHEET_WIDTH.min(inner_w).max(1);
        let sh = (fixed + shown as i32 * SHEET_LINE).min(middle_h).max(1);
        let sx = x + (w - sw) / 2;
        let sy = middle_y + (middle_h - sh) / 2;
        let card = Rectangle::new((sx, sy).into(), (sw, sh).into());
        let (cx, cw) = (sx + SHEET_PAD, (sw - 2 * SHEET_PAD).max(0));
        sheet_title = Some(Rectangle::new(
            (cx, sy + SHEET_PAD).into(),
            (cw, SHEET_TITLE).into(),
        ));
        let lines_y = sy + SHEET_PAD + SHEET_TITLE + SHEET_GAP;
        sheet_lines = (0..shown)
            .map(|i| {
                Rectangle::new(
                    (cx, lines_y + i as i32 * SHEET_LINE).into(),
                    (cw, SHEET_LINE).into(),
                )
            })
            .collect();
        let by = sy + sh - SHEET_PAD - SHEET_BUTTON;
        let mut right = cx + cw;
        for (i, &button) in s.buttons.iter().enumerate().rev() {
            let bw = sheet_button_width(button);
            right -= bw;
            if right < cx {
                break;
            }
            sheet_buttons.push((
                i,
                Rectangle::new((right, by).into(), (bw, SHEET_BUTTON).into()),
            ));
            right -= SHEET_BUTTON_GAP;
        }
        sheet_buttons.reverse();
        sheet = Some(card);
    }

    // The branch menu under the button, its left edge on the button's:
    // its search, the row creating the branch typed, then as many lines as
    // fit above the field, each a group's heading or a branch (one saying
    // none is found, unless it creates).
    let mut branch_search = None;
    let mut branch_create = None;
    let mut branch_headings = Vec::new();
    let mut branch_list = None;
    let branch_menu = view.branch.menu.as_ref().map(|menu| {
        let create = menu.create.as_ref().map(|c| &c.label);
        let widest = menu
            .rows
            .iter()
            .chain([&menu.empty, &menu.placeholder, &menu.query.text])
            .chain(create)
            .map(|r| text_extent(r, FONT_SIZE, BUTTON_CHAR))
            .max()
            .unwrap_or(0);
        let mw = (widest + 2 * BRANCH_ROW_PAD + 2 * BRANCH_CHECK)
            .clamp(pw, BRANCH_MENU_MAX.max(pw))
            .min(inner_w);
        let my = branch.loc.y + branch.size.h + BRANCH_MENU_GAP;
        let mx = branch.loc.x;
        let inner = (mw - 2 * BRANCH_MENU_PAD).max(0);
        let mut rows_y = my + 2 * BRANCH_MENU_PAD + row_h;
        if create.is_some() {
            branch_create = Some(Rectangle::new(
                (mx + BRANCH_MENU_PAD, rows_y).into(),
                (inner, row_h).into(),
            ));
            rows_y += row_h;
        }
        let room = (field.loc.y - REPOS_PAD - rows_y - BRANCH_MENU_PAD).max(0) / row_h;
        // Scrolled part of a line, the menu has one more under its list,
        // shown in part: it is as tall as without it.
        let under = usize::from(menu.part > 0.0);
        let lines = (menu.rows.len() + menu.headings.len()).saturating_sub(under);
        let shown = match (lines, create) {
            (0, Some(_)) => 0,
            (n, _) => n.max(1).min(room.max(1) as usize),
        };
        let mh = rows_y - my + BRANCH_MENU_PAD + shown as i32 * row_h;
        branch_list = Some(Rectangle::new(
            (mx + BRANCH_MENU_PAD, rows_y).into(),
            (inner, shown as i32 * row_h).into(),
        ));
        let shift = wheel::shift(menu.part, row_h);
        let card = Rectangle::new((mx, my).into(), (mw, mh).into());
        branch_search = Some(Rectangle::new(
            (mx + BRANCH_MENU_PAD, my + BRANCH_MENU_PAD).into(),
            (inner, row_h).into(),
        ));
        let line = |j: usize| {
            Rectangle::new(
                (mx + BRANCH_MENU_PAD, rows_y + j as i32 * row_h - shift).into(),
                (inner, row_h).into(),
            )
        };
        let mut rows: Vec<Rect> = Vec::new();
        let laid = if shown == lines { shown + under } else { shown };
        for j in 0..laid {
            let next = branch_headings.len();
            if menu
                .headings
                .get(next)
                .is_some_and(|&(at, _)| at == rows.len())
            {
                branch_headings.push((next, line(j)));
            } else if rows.len() < menu.rows.len() {
                rows.push(line(j));
            }
        }
        (card, rows)
    });
    let branch_deletes = match (&view.branch.menu, &branch_menu) {
        (Some(menu), Some((_, rows))) => rows
            .iter()
            .enumerate()
            .filter(|&(i, _)| menu.deletable.get(i) == Some(&true))
            .map(|(i, row)| (i, branch_delete(*row)))
            .collect(),
        _ => Vec::new(),
    };

    let mut buttons: Vec<(ReposButton, Rect)> = Vec::new();
    if let Some(bar) = &view.result {
        buttons.extend(
            result_buttons
                .iter()
                .map(|&(i, r)| (bar.buttons[i].button, r)),
        );
    }
    buttons.extend(hints.iter().map(|&(i, r)| (view.hints[i].button, r)));
    if let Some(s) = &view.sheet {
        buttons.extend(sheet_buttons.iter().map(|&(i, r)| (s.buttons[i], r)));
    }
    let _ = line_h;
    Parts {
        layout: ReposLayout {
            panel,
            header,
            tree,
            lines,
            tree_list,
            marks,
            chips,
            splitter,
            detail,
            detail_tabs,
            detail_rows,
            detail_list,
            detail_checks,
            detail_folds,
            detail_title_buttons,
            detail_row_buttons,
            field,
            result,
            buttons,
            footer,
            more,
            overflow,
            sheet,
            branch,
            branch_menu,
            branch_search,
            branch_create,
            branch_deletes,
            branch_headings,
            branch_list,
        },
        error,
        body,
        result_buttons,
        hints,
        sheet_title,
        sheet_lines,
        sheet_buttons,
    }
}

/// A card over the panel in `rect`, with corners of `radius`: its drop
/// shadow, its border and its face.
fn card_over(c: &mut Canvas, rect: Rect, radius: f32, scale: f64, p: &Palette) {
    let s = scale as f32;
    let radius = radius * s;
    let (x, y, w, h) = rect_px(rect, scale);
    let blur = (SHADOW as f32 - 2.0) * s;
    let drop = 3.0 * s;
    let grow = blur;
    c.paint_in(
        (x - grow) as i32,
        (y - grow) as i32,
        (w + 2.0 * grow) as i32 + 1,
        (h + 2.0 * grow + drop) as i32 + 1,
        [0, 0, 0],
        |px, py| {
            let d = rounded_rect(px, py, x, y + drop, w, h, radius);
            let t = 1.0 - (d / blur).clamp(0.0, 1.0);
            if d < 0.0 { 0.0 } else { p.shadow * t * t }
        },
    );
    let b = ((scale.round() as i32).max(1)) as f32;
    c.rounded(x, y, w, h, radius, p.border);
    c.rounded(
        x + b,
        y + b,
        w - 2.0 * b,
        h - 2.0 * b,
        radius - b,
        p.elevated,
    );
}

/// `inner` lies wholly in `outer`.
/// Where a branch menu row's check mark goes, at its right end.
fn branch_check(row: Rect) -> Rect {
    Rectangle::new(
        (
            row.loc.x + row.size.w - BRANCH_ROW_PAD / 2 - BRANCH_CHECK,
            row.loc.y,
        )
            .into(),
        (BRANCH_CHECK, row.size.h).into(),
    )
}

/// Where a branch menu row's delete button goes, left of its check mark.
fn branch_delete(row: Rect) -> Rect {
    let check = branch_check(row);
    Rectangle::new(
        (check.loc.x - BRANCH_CHECK, row.loc.y).into(),
        (BRANCH_CHECK, row.size.h).into(),
    )
}

fn within(outer: Rect, inner: Rect) -> bool {
    inner.loc.x >= outer.loc.x
        && inner.loc.y >= outer.loc.y
        && inner.loc.x + inner.size.w <= outer.loc.x + outer.size.w
        && inner.loc.y + inner.size.h <= outer.loc.y + outer.size.h
}

/// What `pos` (output coordinates) is over; `None` outside the panel.
/// With a sheet open only its buttons, else `Panel`.
pub fn repos_hit(layout: &ReposLayout, pos: Point<f64, Logical>) -> Option<ReposHit> {
    let at = |r: &Rect| r.to_f64().contains(pos);
    if !at(&layout.panel) {
        return None;
    }
    if let Some(sheet) = layout.sheet {
        let button = layout
            .buttons
            .iter()
            .find(|(_, r)| within(sheet, *r) && at(r));
        return Some(button.map_or(ReposHit::Panel, |(b, _)| ReposHit::Button(*b)));
    }
    // The open menu is over the rest, which it closes on a click.
    if let Some((menu, rows)) = &layout.branch_menu {
        if layout.branch_create.as_ref().is_some_and(at) {
            return Some(ReposHit::BranchCreate);
        }
        // A scrolled menu's rows count only where they show.
        let in_list = layout.branch_list.as_ref().is_some_and(at);
        let delete = layout.branch_deletes.iter().find(|(_, r)| at(r));
        if let (true, Some((i, _))) = (in_list, delete) {
            return Some(ReposHit::BranchDelete(*i));
        }
        if let (true, Some(i)) = (in_list, rows.iter().position(at)) {
            return Some(ReposHit::BranchRow(i));
        }
        if at(menu) {
            return Some(ReposHit::BranchMenu);
        }
    }
    if at(&layout.branch) {
        return Some(ReposHit::BranchButton);
    }
    if layout.branch_menu.is_some() {
        return Some(ReposHit::Panel);
    }
    // A scrolled tree's lines count only where they show.
    let lines = if at(&layout.tree_list) {
        &layout.lines[..]
    } else {
        &[]
    };
    for (i, (line, check, expander)) in lines.iter().enumerate() {
        if at(check) {
            return Some(ReposHit::Check(i));
        }
        if expander.as_ref().is_some_and(at) {
            return Some(ReposHit::Expander(i));
        }
        let mark = layout.marks.iter().find(|(j, _, r)| *j == i && at(r));
        if let Some(&(_, mark, _)) = mark {
            return Some(ReposHit::Mark(i, mark));
        }
        if layout.chips.iter().any(|(j, r)| *j == i && at(r)) {
            return Some(ReposHit::Chip(i));
        }
        if at(line) {
            return Some(ReposHit::Line(i));
        }
    }
    if at(&layout.splitter) {
        return Some(ReposHit::Splitter);
    }
    if let Some((tab, _)) = layout.detail_tabs.iter().find(|(_, r)| at(r)) {
        return Some(ReposHit::DetailTab(*tab));
    }
    if let Some((button, _)) = layout.detail_title_buttons.iter().find(|(_, r)| at(r)) {
        return Some(ReposHit::Button(*button));
    }
    // A scrolled pane's rows count only where they show, but for the
    // header pinned above them.
    let list = layout.detail_list;
    let shown = |i: usize| {
        at(&list)
            || layout
                .detail_rows
                .get(i)
                .is_some_and(|r| r.loc.y + r.size.h <= list.loc.y)
    };
    let row_button = layout
        .detail_row_buttons
        .iter()
        .find(|(i, _, r)| at(r) && shown(*i));
    if let Some((_, button, _)) = row_button {
        return Some(ReposHit::Button(*button));
    }
    if let Some((i, _)) = layout.detail_folds.iter().find(|(i, r)| at(r) && shown(*i)) {
        return Some(ReposHit::DiffFold(*i));
    }
    if let Some((i, _)) = layout
        .detail_checks
        .iter()
        .find(|(i, r)| at(r) && shown(*i))
    {
        return Some(ReposHit::DiffCheck(*i));
    }
    let row = (0..layout.detail_rows.len()).find(|&i| at(&layout.detail_rows[i]) && shown(i));
    if let Some(i) = row {
        return Some(ReposHit::DiffRow(i));
    }
    if at(&layout.detail) {
        return Some(ReposHit::Detail);
    }
    if at(&layout.field) {
        return Some(ReposHit::Field);
    }
    if let Some((button, _)) = layout.buttons.iter().find(|(_, r)| at(r)) {
        return Some(ReposHit::Button(*button));
    }
    if layout.more.as_ref().is_some_and(at) {
        return Some(ReposHit::More);
    }
    Some(ReposHit::Panel)
}

/// The panel's field, for the caret and selection, on an output of `size`.
pub fn repos_field(view: &ReposView, size: Size<i32, Logical>, scale: f64) -> FieldSpot {
    let layout = repos_layout(view, size);
    // The open branch menu's search has the keyboard.
    let (area, line) = match (&view.branch.menu, layout.branch_search) {
        (Some(menu), Some(search)) => (search, &menu.query),
        _ => (layout.field, &view.field),
    };
    FieldSpot {
        size: FONT_SIZE,
        area,
        text: field_text(area, scale),
        line: line.clone(),
    }
}

/// Signed distance to a small chevron centred on the origin pointing right
/// (a folded repository) or down (an open one).
fn expander_mark(open: bool, x: f32, y: f32, s: f32) -> f32 {
    if open {
        chevron_down(x, y, s)
    } else {
        chevron_down(y, x, s)
    }
}

/// A scrollbar in `track` (buffer coordinates) for a list of `total`
/// lines showing `shown` from `first`: its thumb where they are, nothing
/// when they all fit.
pub(super) fn scrollbar(
    c: &mut Canvas,
    track: Rect,
    (first, shown, total): (usize, usize, usize),
    scale: f64,
    p: &Palette,
) {
    if total <= shown || shown == 0 {
        return;
    }
    let s = scale as f32;
    let (x, y, w, h) = rect_px(track, scale);
    let min = (SCROLLBAR_MIN as f32 * s).min(h);
    let thumb_h = (h * shown as f32 / total as f32).max(min).min(h);
    let first = first.min(total - shown) as f32;
    let thumb_y = y + ((h - thumb_h) * first / (total - shown) as f32).round();
    c.rounded(
        x,
        thumb_y,
        w,
        thumb_h,
        w / 2.0,
        mix(p.elevated, p.muted, 0.45),
    );
}

/// The colour of a change's status letter (`git::Change::code`) in `p`.
fn code_color(code: char, p: &Palette) -> Rgb {
    match code {
        'A' => p.success,
        'D' | 'U' => p.error,
        'R' | 'C' => p.accent,
        '?' => p.muted,
        _ => p.warning,
    }
}

/// The colour of `tone` in `p`.
fn tone_color(tone: Tone, p: &Palette) -> Rgb {
    match tone {
        Tone::Muted => p.muted,
        Tone::Normal => p.text,
        Tone::Success => p.success,
        Tone::Warning => p.warning,
        Tone::Error => p.error,
    }
}

impl Painter {
    /// `text` split into runs drawn in the UI font (`false`) or, for the
    /// characters it lacks (`→`), in the monospace font.
    fn runs<'a>(&self, text: &'a str) -> Vec<(&'a str, bool)> {
        let mut runs: Vec<(&str, bool)> = Vec::new();
        let mut start = 0;
        let mut mono = None;
        for (i, ch) in text.char_indices() {
            let this = !self.has_glyph(ch) && ch != ' ';
            match mono {
                Some(m) if m != this => {
                    runs.push((&text[start..i], m));
                    start = i;
                }
                _ => {}
            }
            mono = Some(this);
        }
        if let Some(m) = mono {
            runs.push((&text[start..], m));
        }
        runs
    }

    /// Width of `text` as `mixed_text` draws it, in physical pixels.
    fn mixed_width(&self, text: &str, size: f32, scale: f64) -> f32 {
        self.runs(text)
            .into_iter()
            .map(|(run, mono)| {
                if mono {
                    self.mono_width(run, size, scale)
                } else {
                    self.text_width(run, size, scale)
                }
            })
            .sum()
    }

    /// Draw `text` as `text` does, but what the UI font has no glyph for in
    /// the monospace font. Returns where it ends.
    #[allow(clippy::too_many_arguments)]
    fn mixed_text(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        color: Rgb,
    ) -> f32 {
        let runs = self.runs(text);
        if runs.iter().all(|(_, mono)| !mono) {
            return self.text(c, text, size, x, max_x, band, scale, color);
        }
        let mut shown = text;
        let cut = x + self.mixed_width(text, size, scale) > max_x;
        if cut {
            let ellipsis = self.text_width("…", size, scale);
            if x + ellipsis > max_x {
                return x;
            }
            let mut end = 0;
            for (i, ch) in text.char_indices() {
                let next = i + ch.len_utf8();
                if x + self.mixed_width(&text[..next], size, scale) + ellipsis > max_x {
                    break;
                }
                end = next;
            }
            shown = &text[..end];
        }
        let mut pen = x;
        for (run, mono) in self.runs(shown) {
            pen = if mono {
                self.mono_text(c, run, size, pen, f32::MAX, band, scale, color)
            } else {
                self.text(c, run, size, pen, f32::MAX, band, scale, color)
            };
        }
        if cut {
            pen = self.text(c, "…", size, pen, f32::MAX, band, scale, color);
        }
        pen
    }

    /// A file's `path` from `x`, its folders in the first of `colors` and
    /// its name in the second; cut off at the start with "…" when it
    /// doesn't fit before `max_x`, so that the name shows.
    #[allow(clippy::too_many_arguments)]
    fn path_text(
        &self,
        c: &mut Canvas,
        path: &str,
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        (folders, name): (Rgb, Rgb),
    ) {
        let split = path.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
        let width = |text: &str| self.mixed_width(text, size, scale);
        let mut pen = x;
        let mut start = 0;
        if x + width(path) > max_x {
            let ellipsis = self.text_width("…", size, scale);
            let Some(from) = path
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| x + ellipsis + width(&path[i..]) <= max_x)
            else {
                // Not even the end of the name fits: as much of it as does.
                self.mixed_text(c, &path[split..], size, x, max_x, band, scale, name);
                return;
            };
            pen = self.text(c, "…", size, pen, f32::MAX, band, scale, folders);
            start = from;
        }
        if start < split {
            pen = self.mixed_text(
                c,
                &path[start..split],
                size,
                pen,
                f32::MAX,
                band,
                scale,
                folders,
            );
        }
        self.mixed_text(
            c,
            &path[start.max(split)..],
            size,
            pen,
            f32::MAX,
            band,
            scale,
            name,
        );
    }

    /// The repositories panel for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow, in one buffer: its frame with the
    /// lists over it as they show at rest, then the branch menu and the
    /// sheet over all.
    #[cfg(test)]
    pub(super) fn repos(
        &self,
        view: &ReposView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let parts = parts(view, size);
        let layout = &parts.layout;
        let mut c = self.repos_frame(view, &parts, scale, p);
        let by = buffer_origin(layout.panel);
        let shift = |r: Rect| moved(r, by);
        // The lines, cut to where they show (a scrolled tree's first and
        // last in part).
        let (lx, ly, lw, lh) = rect_px(shift(layout.tree_list), scale);
        c.clipped(lx as i32, ly as i32, lw as i32, lh as i32, |c| {
            for (i, (rect, ..)) in layout.lines.iter().enumerate() {
                let row = tree_row(view, i);
                let rects = line_rects(&row.line, shift(*rect));
                self.tree_line(c, &row, rects, scale, p);
            }
        });
        let (track, shown) = tree_bar(view, layout);
        let at = (view.first, shown, view.total);
        scrollbar(&mut c, shift(track), at, scale, p);
        self.repos_menu(&mut c, view, layout, by, scale, p);
        if let Some(sheet) = &view.sheet {
            self.sheet(&mut c, view, sheet, &parts, shift, scale, p);
        }
        c.into_buffer(false)
    }

    /// The panel's frame laid out as `parts`, inside a margin of `SHADOW`
    /// holding its drop shadow: all of it but the tree's lines and its
    /// scrollbar, the branch menu and the sheet.
    fn repos_frame(&self, view: &ReposView, parts: &Parts, scale: f64, p: &Palette) -> Canvas {
        let layout = &parts.layout;
        let panel = layout.panel;
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        let by = buffer_origin(panel);
        let shift = |r: Rect| moved(r, by);
        let across = |c: &mut Canvas, y: i32| {
            c.fill(
                px(SHADOW) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };

        // The header: the branch picker, the title, then where the
        // repositories stand.
        let header = shift(layout.header);
        let picker = shift(layout.branch);
        let (_, hend) = phys(header, scale);
        let hx = px(picker.loc.x + picker.size.w + HEADER_GAP) as f32;
        let hend = hend - px(HELP_PADDING) as f32;
        let heading = if view.title.is_empty() {
            "Repositories".to_owned()
        } else {
            format!("Repositories · {}", view.title)
        };
        let header_band = band(header, scale);
        let after = self.mixed_text(
            &mut c,
            &heading,
            LABEL_FONT_SIZE,
            hx,
            hend,
            header_band,
            scale,
            p.text,
        );
        self.mixed_text(
            &mut c,
            &view.summary,
            FONT_SIZE,
            after + (HEADER_GAP as f32 * s).round(),
            hend,
            header_band,
            scale,
            p.muted,
        );
        self.branch_button(&mut c, view, picker, scale, p);
        across(&mut c, px(header.loc.y + header.size.h) - border);

        // The tree, then the line between it and the detail pane.
        let tree = shift(layout.tree);
        if view.lines.is_empty() && !view.empty.is_empty() {
            let first = Rectangle::new(
                (
                    tree.loc.x + REPOS_PAD + REPOS_LINE_INSET,
                    tree.loc.y + REPOS_PAD,
                )
                    .into(),
                (
                    (tree.size.w - 2 * REPOS_PAD).max(0),
                    repos_row(view.density),
                )
                    .into(),
            );
            let (x, right) = phys(first, scale);
            self.mixed_text(
                &mut c,
                &view.empty,
                FONT_SIZE,
                x,
                right,
                band(first, scale),
                scale,
                p.placeholder,
            );
        }
        let detail = shift(layout.detail);
        c.fill(
            px(detail.loc.x),
            px(detail.loc.y),
            border,
            px(detail.loc.y + detail.size.h) - px(detail.loc.y),
            p.border_variant,
        );
        let tabs: Vec<_> = layout
            .detail_tabs
            .iter()
            .map(|(t, r)| (*t, shift(*r)))
            .collect();
        let rows: Vec<Rect> = layout.detail_rows.iter().map(|r| shift(*r)).collect();
        let body = (shift(parts.body), shift(layout.detail_list), &rows[..]);
        let title_buttons: Vec<_> = layout
            .detail_title_buttons
            .iter()
            .map(|&(b, r)| (b, shift(r)))
            .collect();
        let row_buttons: Vec<_> = layout
            .detail_row_buttons
            .iter()
            .map(|&(i, b, r)| (i, b, shift(r)))
            .collect();
        let buttons = (&title_buttons[..], &row_buttons[..]);
        self.detail_pane(&mut c, view, detail, body, &tabs, buttons, scale, p);

        // The field, what went wrong, the result bar and the footer.
        let field = shift(layout.field);
        across(
            &mut c,
            px(field.loc.y - (REPOS_FIELD_ROW - REPOS_FIELD) / 2) - border,
        );
        self.repos_field_box(&mut c, view, field, scale, p);
        if let (Some(error), Some(rect)) = (&view.error, parts.error) {
            let rect = shift(rect);
            let (x, right) = phys(rect, scale);
            self.mixed_text(
                &mut c,
                error,
                SMALL_FONT_SIZE,
                x + PADDING * s,
                right,
                band(rect, scale),
                scale,
                p.error,
            );
        }
        if let (Some(bar), Some(rect)) = (&view.result, layout.result) {
            let buttons: Vec<_> = parts
                .result_buttons
                .iter()
                .map(|&(i, r)| (i, shift(r)))
                .collect();
            self.result_bar(&mut c, view, bar, shift(rect), &buttons, scale, p);
        }
        let footer = shift(layout.footer);
        across(&mut c, px(footer.loc.y));
        for &(i, rect) in &parts.hints {
            let hint = &view.hints[i];
            let hit = ReposHit::Button(hint.button);
            self.hint_button(&mut c, view, hint, hit, shift(rect), scale, p);
        }
        if let Some(rect) = layout.more {
            let more = Hint {
                button: ReposButton::Help,
                label: more_label(layout.overflow.len()),
                key: String::new(),
                disabled: None,
            };
            self.hint_button(&mut c, view, &more, ReposHit::More, shift(rect), scale, p);
        }
        c
    }

    /// The branch menu of `view`, while open, as `layout` has it, moved by
    /// `by` onto the buffer.
    fn repos_menu(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        layout: &ReposLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let (Some(menu), Some((card, rows)), Some(search)) =
            (&view.branch.menu, &layout.branch_menu, layout.branch_search)
        else {
            return;
        };
        let shift = |r: Rect| moved(r, by);
        let rows: Vec<Rect> = rows.iter().map(|r| shift(*r)).collect();
        let create = layout.branch_create.map(shift);
        let headings: Vec<(usize, Rect)> = layout
            .branch_headings
            .iter()
            .map(|&(i, r)| (i, shift(r)))
            .collect();
        let list = layout.branch_list.map_or(shift(*card), shift);
        let parts = (
            shift(*card),
            shift(search),
            create,
            (list, &rows[..]),
            &headings[..],
        );
        self.branch_menu(c, view, menu, parts, scale, p);
    }

    /// A tree line `size` big in a buffer of its own.
    fn tree_row(
        &self,
        row: &TreeRow,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let rects = line_rects(&row.line, Rectangle::from_size(size));
        self.tree_line(&mut c, row, rects, scale, p);
        c.into_buffer(false)
    }

    /// A scrollbar's thumb `size` big in a buffer of its own.
    fn thumb(&self, size: Size<i32, Logical>, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let (w, h) = (c.w as f32, c.h as f32);
        c.rounded(0.0, 0.0, w, h, w / 2.0, mix(p.elevated, p.muted, 0.45));
        c.into_buffer(false)
    }
    /// A tree line in its `(rect, checkbox, expander)` (buffer
    /// coordinates): its highlight, then a repository's or a file's line.
    fn tree_line(
        &self,
        c: &mut Canvas,
        row: &TreeRow,
        (rect, check, expander): (Rect, Rect, Option<Rect>),
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        // The highlight stops short of the lines above and below.
        let (hy, hh) = (y + px(1) as f32, h - 2.0 * px(1) as f32);
        let r = (MENU_RADIUS - 2.0) * s;
        let line = &row.line;
        let hovered = row.hover.is_some();
        let state = match line {
            TreeLine::Repo(repo) => Some(&repo.state),
            TreeLine::File(_) => None,
        };
        if row.selected {
            let fill = if row.dim {
                mix(p.elevated, p.selected, 0.6)
            } else {
                p.selected
            };
            c.rounded(x, hy, w, hh, r, fill);
        } else if hovered {
            c.rounded(x, hy, w, hh, r, mix(p.elevated, p.selected, 0.5));
        }
        if matches!(state, Some(LineState::Attention)) {
            // A bar at the left edge: it needs a look.
            c.rounded(x, hy + 4.0 * s, 2.0 * s, hh - 8.0 * s, s, p.warning);
        }

        let check_hovered = row.hover == Some(ReposHit::Check(0));
        let side = CHECKBOX_SIZE;
        let box_rect = Rectangle::new(
            (check.loc.x + (check.size.w - side) / 2, check.loc.y).into(),
            (side, check.size.h).into(),
        );
        let line_band = band(rect, scale);
        let right = x + w - PADDING * s;
        let gap = (FIELD_GAP as f32 * s).round();
        match line {
            TreeLine::Repo(repo) => {
                let check_state = repo.check;
                self.checkbox(c, box_rect, check_state, check_hovered, scale, p);
                if let (Some(open), Some(rect)) = (repo.expanded, expander) {
                    let color = if row.hover == Some(ReposHit::Expander(0)) {
                        p.text
                    } else {
                        p.muted
                    };
                    icon_sdf(c, rect, scale, color, |x, y| expander_mark(open, x, y, s));
                }
                let label_x = px(check.loc.x + REPOS_CHECK + REPOS_EXPANDER) as f32;
                let mark = match row.hover {
                    Some(ReposHit::Mark(_, mark)) => Some(mark),
                    _ => None,
                };
                let on_chip = row.hover == Some(ReposHit::Chip(0));
                let at = (label_x, right);
                self.repo_line(c, repo, at, line_band, gap, (mark, on_chip), scale, p);
            }
            TreeLine::File(file) => {
                self.checkbox(c, box_rect, file.check, check_hovered, scale, p);
                let code_x = px(check.loc.x + check.size.w + REPOS_CODE_GAP) as f32;
                let code_w = px(REPOS_CODE) as f32;
                let letter = file.code.to_string();
                let color = match file.code {
                    'U' => p.error,
                    '?' => p.placeholder,
                    _ if file.conflicted => p.error,
                    _ => p.text,
                };
                let lw = self.mono_width(&letter, REPOS_MONO_SIZE, scale);
                self.mono_text(
                    c,
                    &letter,
                    REPOS_MONO_SIZE,
                    (code_x + (code_w - lw) / 2.0).round(),
                    f32::MAX,
                    line_band,
                    scale,
                    color,
                );
                let name = if file.conflicted { p.error } else { p.text };
                self.path_text(
                    c,
                    &file.path,
                    SMALL_FONT_SIZE,
                    code_x + code_w + gap / 2.0,
                    right,
                    line_band,
                    scale,
                    (p.muted, name),
                );
            }
        }
    }

    /// A repository's line from `x` to `right` (physical pixels): its
    /// label, branch, marks and what it is doing, its chip at the right.
    #[allow(clippy::too_many_arguments)]
    fn repo_line(
        &self,
        c: &mut Canvas,
        repo: &RepoLine,
        (x, right): (f32, f32),
        line_band: (i32, i32),
        gap: f32,
        (hovered, on_chip): (Option<RepoMark>, bool),
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let width = |text: &str, size: f32| self.mixed_width(text, size, scale);
        let spans = line_spans(repo, (x, right), gap, s, &width);
        let end = spans.end;
        if let (Some(chip), Some((cx, cw))) = (&repo.chip, spans.chip) {
            let color = tone_color(chip.tone, p);
            let (top, height) = line_band;
            let pad = (CHIP_PAD as f32 * s).round();
            let ch = (CHIP_HEIGHT as f32 * s).round();
            let cy = (top as f32 + (height as f32 - ch) / 2.0).round();
            // Lit under the pointer: it has something to say.
            let strength = if on_chip { 0.3 } else { 0.18 };
            let fill = match chip.tone {
                Tone::Normal | Tone::Muted if on_chip => mix(p.element, p.selected, 0.5),
                Tone::Normal | Tone::Muted => p.element,
                _ => mix(p.elevated, color, strength),
            };
            c.rounded(cx, cy, cw, ch, CHIP_RADIUS * s, fill);
            self.mixed_text(
                c,
                &chip.text,
                DETAIL_FONT_SIZE,
                cx + pad,
                right,
                (cy as i32, ch as i32),
                scale,
                color,
            );
        }
        let label_color = match repo.state {
            LineState::Busy(_) => p.muted,
            _ => p.text,
        };
        self.mixed_text(
            c,
            &repo.label,
            FONT_SIZE,
            x,
            end,
            line_band,
            scale,
            label_color,
        );
        let (branch, branch_color) = match &repo.branch {
            Some(branch) => (branch.as_str(), p.muted),
            None => ("detached", p.placeholder),
        };
        if let Some(bx) = spans.branch {
            self.mixed_text(
                c,
                branch,
                SMALL_FONT_SIZE,
                bx,
                end,
                line_band,
                scale,
                branch_color,
            );
        }
        let (top, height) = line_band;
        let cy = top as f32 + height as f32 / 2.0;
        for m in &spans.marks {
            let cx = m.x + m.w / 2.0;
            // The ones a click acts on light up under the pointer.
            let lit = |color| {
                if hovered == Some(m.mark) {
                    p.text
                } else {
                    color
                }
            };
            let (color, count) = match m.mark {
                RepoMark::Dirty => (p.warning, 0),
                RepoMark::Ahead => (lit(p.muted), repo.ahead),
                RepoMark::Behind => (lit(p.muted), repo.behind),
                RepoMark::Stashes => {
                    let note = stash_note(repo);
                    let color = lit(p.placeholder);
                    self.mixed_text(c, &note, SMALL_FONT_SIZE, m.x, end, line_band, scale, color);
                    continue;
                }
            };
            c.paint_in(m.x as i32, top, m.w as i32 + 1, height, color, |x, y| {
                let (x, y) = (x - cx, y - cy);
                coverage(match m.mark {
                    RepoMark::Dirty => x.hypot(y) - 2.5 * s,
                    RepoMark::Ahead => arrow(true, x, y, s),
                    RepoMark::Behind => arrow(false, x, y, s),
                    RepoMark::Stashes => unreachable!(),
                })
            });
            if count > 0 {
                self.text(
                    c,
                    &count.to_string(),
                    DETAIL_FONT_SIZE,
                    m.x + m.w + s,
                    end,
                    line_band,
                    scale,
                    color,
                );
            }
        }
        if let Some(nx) = spans.no_upstream {
            let color = p.placeholder;
            self.mixed_text(
                c,
                "no upstream",
                SMALL_FONT_SIZE,
                nx,
                end,
                line_band,
                scale,
                color,
            );
        }
        let mut pen = spans.pen;
        let mut notes: Vec<(String, Rgb)> = Vec::new();
        match &repo.state {
            LineState::Busy(what) => notes.push((what.clone(), p.muted)),
            LineState::Armed(what) => notes.push((what.clone(), p.warning)),
            _ => {}
        }
        for (note, color) in notes {
            if pen + gap >= end {
                break;
            }
            pen = self.mixed_text(
                c,
                &note,
                SMALL_FONT_SIZE,
                pen + gap,
                end,
                line_band,
                scale,
                color,
            );
        }
    }

    /// The detail pane in `rect`: its title, its buttons and tabs, then
    /// its lines in `body` in the monospace font, added and removed ones
    /// on a tint: each in its rect of `rows`, the scrolled ones cut to
    /// `list`, with their buttons.
    #[allow(clippy::too_many_arguments)]
    fn detail_pane(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        rect: Rect,
        (body, list, rows): (Rect, Rect, &[Rect]),
        tabs: &[(DetailTab, Rect)],
        (title_buttons, row_buttons): DetailButtons,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let title = Rectangle::new(rect.loc, (rect.size.w, REPOS_DETAIL_TITLE).into());
        let (x, mut right) = phys(title, scale);
        let x = x + px(HELP_PADDING) as f32;
        let first = title_buttons
            .first()
            .map(|(_, r)| r)
            .or(tabs.first().map(|(_, r)| r));
        if let Some(first) = first {
            right = px(first.loc.x) as f32 - (FIELD_GAP as f32 * s).round();
        }
        self.mixed_text(
            c,
            &view.detail.title,
            FONT_SIZE,
            x,
            right,
            band(title, scale),
            scale,
            p.text,
        );
        for &(tab, rect) in tabs {
            self.tab_segment(c, view, tab, rect, scale, p);
        }
        for &(button, rect) in title_buttons {
            if let Some(hint) = view
                .detail
                .title_buttons
                .iter()
                .find(|h| h.button == button)
            {
                let hit = ReposHit::Button(button);
                self.hint_button(c, view, hint, hit, rect, scale, p);
            }
        }
        let border = px(1).max(1);
        c.fill(
            px(rect.loc.x) + border,
            px(rect.loc.y + REPOS_DETAIL_TITLE) - border,
            px(rect.loc.x + rect.size.w) - px(rect.loc.x) - 2 * border,
            border,
            p.border_variant,
        );

        let line_h = repos_detail_line(view.density);
        // Tints reach across the pane, short of its edges.
        let tint = (
            px(rect.loc.x) + border,
            px(rect.loc.x + rect.size.w) - border,
        );
        // The rows scrolled, cut to where they show; then the header
        // pinned at the top, over them.
        let pinned = usize::from(view.detail.sticky);
        let shown = rows.iter().zip(&view.detail.rows).enumerate();
        let (lx, ly, lw, lh) = rect_px(list, scale);
        c.clipped(lx as i32, ly as i32, lw as i32, lh as i32, |c| {
            for (i, (at, row)) in shown.clone().skip(pinned) {
                self.detail_row(c, view, i, row, *at, tint, scale, p);
                self.row_chips(c, view, i, row_buttons, scale, p);
            }
        });
        for (i, (at, row)) in shown.take(pinned) {
            self.detail_row(c, view, i, row, *at, tint, scale, p);
            self.row_chips(c, view, i, row_buttons, scale, p);
        }
        // The header pinned at the top casts a shadow on what scrolls under.
        if view.detail.sticky && !rows.is_empty() {
            let (y, h) = band(
                Rectangle::new(body.loc, (body.size.w, line_h).into()),
                scale,
            );
            c.fill(tint.0, y + h - border, tint.1 - tint.0, border, p.border);
            let depth = px(4).max(1);
            for dy in 0..depth {
                let fade = 1.0 - dy as f32 / depth as f32;
                for x in tint.0..tint.1 {
                    c.blend(x, y + h + dy, [0, 0, 0], p.shadow * 0.35 * fade * fade);
                }
            }
        }
        let track = Rectangle::new(
            (
                rect.loc.x + rect.size.w - HELP_PADDING / 2 - SCROLLBAR,
                body.loc.y,
            )
                .into(),
            (SCROLLBAR, body.size.h).into(),
        );
        let (first, total) = (view.detail.first, view.detail.total);
        let under = usize::from(view.detail.part > 0.0);
        let whole = view.detail.rows.len().saturating_sub(under);
        let shown = rows.len().min(whole).saturating_sub(pinned);
        scrollbar(c, track, (first, shown.max(1), total), scale, p);
    }

    /// Row `i` of the detail in `rect`; its tint across the pane from
    /// `tint.0` to `tint.1` (physical pixels). A repository's header and a
    /// file's are banded, a hunk is a rule with what it is in, an added or
    /// removed line is tinted with a bar at its left and its changed words
    /// tinted more.
    #[allow(clippy::too_many_arguments)]
    fn detail_row(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        i: usize,
        row: &DetailRow,
        rect: Rect,
        (tint_x, tint_end): (i32, i32),
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (bx, bend) = phys(rect, scale);
        let row_band = band(rect, scale);
        let (y, h) = row_band;
        let border = px(1).max(1);
        let gap = (FIELD_GAP as f32 * s).round();
        let inset = px(DIFF_INSET) as f32;
        let across = |c: &mut Canvas, color: Rgb| c.fill(tint_x, y, tint_end - tint_x, h, color);
        // The cursor's row and the range it reaches back to, lit as one;
        // not a file's header, pinned or not, which shows its own.
        let mark = view.detail.marks.get(i).copied().unwrap_or_default();
        let picked = mark.selected && !matches!(row, DetailRow::File(_));
        let lit = picked.then_some(0.5);
        if let Some(amount) = lit
            && matches!(
                row,
                DetailRow::Text(_) | DetailRow::Hunk { .. } | DetailRow::Note(_)
            )
        {
            across(c, mix(p.elevated, p.selected, amount));
        }
        match row {
            DetailRow::Text(line) => {
                let color = match line.tone {
                    DiffTone::Plain => p.text,
                    DiffTone::Header => p.muted,
                    DiffTone::Error => p.error,
                };
                self.mono_text(
                    c,
                    &line.text,
                    REPOS_MONO_SIZE,
                    bx,
                    bend,
                    row_band,
                    scale,
                    color,
                );
            }
            DetailRow::Repo(repo) => {
                across(c, mix(p.elevated, p.text, 0.07));
                c.fill(tint_x, y + h - border, tint_end - tint_x, border, p.border);
                let files = match repo.files {
                    1 => "1 file".to_string(),
                    n => format!("{n} files"),
                };
                let counts = self.diff_counts(
                    c,
                    Some(&files),
                    (repo.added, repo.removed),
                    bend,
                    row_band,
                    scale,
                    p,
                );
                let right = counts - gap;
                let mut pen = self.mixed_text(
                    c,
                    &repo.label,
                    FONT_SIZE,
                    bx,
                    right,
                    row_band,
                    scale,
                    p.text,
                );
                if let Some(branch) = &repo.branch {
                    pen += gap;
                    let icon = Rectangle::new(
                        ((pen / s).round() as i32, rect.loc.y).into(),
                        (14, rect.size.h).into(),
                    );
                    if pen + px(14) as f32 + gap < right {
                        icon_sdf(c, icon, scale, p.muted, |x, y| git_branch(x, y, s));
                        pen += px(14) as f32 + (gap / 2.0).round();
                        self.mixed_text(
                            c,
                            branch,
                            SMALL_FONT_SIZE,
                            pen,
                            right,
                            row_band,
                            scale,
                            p.muted,
                        );
                    }
                }
            }
            DetailRow::File(file) => {
                let hovered = matches!(
                    view.hover,
                    Some(ReposHit::DiffRow(j) | ReposHit::DiffCheck(j) | ReposHit::DiffFold(j)) if j == i
                );
                let fill = if file.selected {
                    mix(p.elevated, p.selected, 0.75)
                } else if hovered {
                    mix(p.elevated, p.selected, 0.45)
                } else {
                    mix(p.elevated, p.text, 0.035)
                };
                across(c, fill);
                c.fill(tint_x, y, tint_end - tint_x, border, p.border_variant);
                if file.selected && view.detail.focused {
                    c.fill(tint_x, y, px(2), h, p.accent);
                }
                let fold = Rectangle::new(rect.loc, (DIFF_FOLD, rect.size.h).into());
                let color = if view.hover == Some(ReposHit::DiffFold(i)) {
                    p.text
                } else {
                    p.muted
                };
                icon_sdf(c, fold, scale, color, |x, y| {
                    expander_mark(!file.folded, x, y, s)
                });
                let side = CHECKBOX_SIZE;
                let box_rect = Rectangle::new(
                    (rect.loc.x + DIFF_FOLD + (DIFF_CHECK - side) / 2, rect.loc.y).into(),
                    (side, rect.size.h).into(),
                );
                let check_hovered = view.hover == Some(ReposHit::DiffCheck(i));
                self.checkbox(c, box_rect, file.check, check_hovered, scale, p);
                // The status letter on a pill of its colour.
                let tone = code_color(file.code, p);
                let (bw, bh) = (px(DIFF_BADGE) as f32, px(DIFF_BADGE - 2) as f32);
                let badge_x = px(rect.loc.x + DIFF_FOLD + DIFF_CHECK) as f32;
                let badge_y = (y as f32 + (h as f32 - bh) / 2.0).round();
                c.rounded(
                    badge_x,
                    badge_y,
                    bw,
                    bh,
                    3.0 * s,
                    mix(p.elevated, tone, 0.2),
                );
                let letter = file.code.to_string();
                let lw = self.mono_width(&letter, SMALL_FONT_SIZE, scale);
                let lx = (badge_x + (bw - lw) / 2.0).round();
                self.mono_text(
                    c,
                    &letter,
                    SMALL_FONT_SIZE,
                    lx,
                    f32::MAX,
                    row_band,
                    scale,
                    tone,
                );
                let counts = self.diff_counts(
                    c,
                    None,
                    (file.added, file.removed),
                    bend,
                    row_band,
                    scale,
                    p,
                );
                let right = counts - gap;
                let mut pen = badge_x + bw + gap;
                if let Some(repo) = &file.repo {
                    let label = format!("{repo} ›");
                    pen = self.mixed_text(
                        c,
                        &label,
                        SMALL_FONT_SIZE,
                        pen,
                        right,
                        row_band,
                        scale,
                        p.muted,
                    );
                    pen += (gap / 2.0).round();
                }
                self.path_text(
                    c,
                    &file.path,
                    FONT_SIZE,
                    pen,
                    right,
                    row_band,
                    scale,
                    (p.muted, p.text),
                );
            }
            DetailRow::Hunk { context, number } => {
                // A short rule, what the hunk is in, then a rule on.
                let mid = y + h / 2;
                let lead = px(12);
                c.fill(bx as i32, mid, lead, border, p.border_variant);
                let label = if context.is_empty() {
                    format!("line {number}")
                } else {
                    context.clone()
                };
                let tx = bx + lead as f32 + (gap / 2.0).round();
                let end = self.mono_text(
                    c,
                    &label,
                    SMALL_FONT_SIZE,
                    tx,
                    bend,
                    row_band,
                    scale,
                    p.muted,
                );
                let from = (end + (gap / 2.0).round()) as i32;
                if (from as f32) < bend {
                    c.fill(from, mid, bend as i32 - from, border, p.border_variant);
                }
            }
            DetailRow::Line(line) => {
                let tone = match line.kind {
                    LineKind::Added => Some(p.success),
                    LineKind::Removed => Some(p.error),
                    LineKind::Context => None,
                };
                let tx = bx + inset;
                let lighten = |color: Rgb| match lit {
                    Some(amount) => mix(color, p.selected, amount),
                    None => color,
                };
                if tone.is_none() && lit.is_some() {
                    across(c, lighten(p.elevated));
                }
                if let Some(tone) = tone {
                    across(c, lighten(mix(p.elevated, tone, DIFF_TINT)));
                    let word = mix(p.elevated, tone, DIFF_WORD_TINT);
                    let upto = |n: usize| {
                        let end = line
                            .text
                            .char_indices()
                            .nth(n)
                            .map_or(line.text.len(), |(b, _)| b);
                        tx + self.mono_width(&line.text[..end], REPOS_MONO_SIZE, scale)
                    };
                    for w in &line.words {
                        let (x0, x1) = (upto(w.start), upto(w.end).min(bend));
                        if x0 >= bend {
                            break;
                        }
                        let (wy, wh) = ((y + border) as f32, (h - 2 * border) as f32);
                        c.rounded(x0, wy, x1 - x0, wh, 2.0 * s, word);
                    }
                    c.fill(tint_x, y, px(DIFF_BAR), h, tone);
                }
                // A dot in the gutter: a comment is on it.
                if mark.noted {
                    let d = (4.0 * s).round();
                    let dx = tint_x as f32 + px(DIFF_BAR) as f32 + ((inset - d) / 2.0).max(1.0);
                    let dy = (y as f32 + (h as f32 - d) / 2.0).round();
                    c.rounded(dx, dy, d, d, d / 2.0, p.accent);
                }
                // The code's coloured runs and the plain text between,
                // each placed by the width of the text before it.
                let bytes: Vec<usize> = line
                    .text
                    .char_indices()
                    .map(|(b, _)| b)
                    .chain([line.text.len()])
                    .collect();
                let byte = |n: usize| bytes[n.min(bytes.len() - 1)];
                let mut run = |from: usize, to: usize, color: Rgb| {
                    let (from, to) = (byte(from), byte(to));
                    if from >= to {
                        return;
                    }
                    let x = tx + self.mono_width(&line.text[..from], REPOS_MONO_SIZE, scale);
                    if x < bend {
                        let text = &line.text[from..to];
                        self.mono_text(c, text, REPOS_MONO_SIZE, x, bend, row_band, scale, color);
                    }
                };
                let mut at = 0;
                for (range, kind) in &line.syntax {
                    run(at, range.start, p.text);
                    run(range.start, range.end, p.code(*kind));
                    at = range.end;
                }
                run(at, usize::MAX, p.text);
            }
            DetailRow::Note(text) => {
                self.mixed_text(
                    c,
                    text,
                    SMALL_FONT_SIZE,
                    bx + inset,
                    bend,
                    row_band,
                    scale,
                    p.placeholder,
                );
            }
            DetailRow::Comment { text, first, last } => {
                let fill = mix(p.elevated, p.accent, if lit.is_some() { 0.2 } else { 0.1 });
                let tx = self.comment_card(
                    c,
                    (bx + inset, bend),
                    row_band,
                    (*first, *last),
                    fill,
                    scale,
                );
                c.fill(
                    (bx + inset) as i32,
                    y,
                    px(2).max(1),
                    h,
                    mix(p.elevated, p.accent, 0.7),
                );
                self.mono_text(c, text, REPOS_MONO_SIZE, tx, bend, row_band, scale, p.text);
            }
            DetailRow::Editor {
                text,
                caret,
                selection,
                first,
                last,
            } => {
                let (x0, x1) = (bx + inset, bend);
                self.comment_card(c, (x0, x1), row_band, (*first, *last), p.accent, scale);
                let b = border as f32;
                let (top, bottom) = (if *first { b } else { 0.0 }, if *last { b } else { 0.0 });
                let inner = (y as f32 + top) as i32..(y as f32 + h as f32 - bottom) as i32;
                let (iy, ih) = (inner.start, inner.end - inner.start);
                let tx = self.comment_card(
                    c,
                    (x0 + b, x1 - b),
                    (iy, ih),
                    (*first, *last),
                    p.element,
                    scale,
                );
                let at = |byte: usize| {
                    tx + self.mono_width(&text[..byte.min(text.len())], REPOS_MONO_SIZE, scale)
                };
                if !selection.is_empty() {
                    let (sx, ex) = (at(selection.start), at(selection.end).min(x1));
                    c.fill(sx as i32, iy, (ex - sx).max(1.0) as i32, ih, p.selected);
                }
                self.mono_text(c, text, REPOS_MONO_SIZE, tx, x1, row_band, scale, p.text);
                if let Some(caret) = caret.filter(|_| view.detail.focused) {
                    let cx = at(caret).min(x1 - b) as i32;
                    let pad = px(3);
                    c.fill(cx, y + pad, px(1).max(1), h - 2 * pad, p.accent);
                }
            }
        }
        // The rows picked outlined as one, a bar down their left.
        if picked {
            let color = if view.detail.focused {
                p.accent
            } else {
                p.muted
            };
            let w = tint_end - tint_x;
            c.fill(tint_x, y, px(DIFF_BAR), h, color);
            c.fill(tint_end - border, y, border, h, color);
            if mark.top {
                c.fill(tint_x, y, w, border, color);
            }
            if mark.bottom {
                c.fill(tint_x, y + h - border, w, border, color);
            }
        }
    }

    /// The buttons on row `i` of the detail, of those in `buttons`: the
    /// first of them (keeping the comment written, writing one) in the
    /// accent, the others plain; lighter under the pointer, with their
    /// key muted after their label.
    fn row_chips(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        i: usize,
        buttons: &[(usize, ReposButton, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let hints = view.detail.row_buttons.iter().filter(|(j, _)| *j == i);
        for (k, (_, hint)) in hints.enumerate() {
            let Some(&(.., rect)) = buttons
                .iter()
                .find(|&&(j, b, _)| j == i && b == hint.button)
            else {
                continue;
            };
            let (x, y, w, h) = rect_px(rect, scale);
            let r = (h / 2.0).min(BUTTON_RADIUS * s);
            let b = s.round().max(1.0);
            let hovered = view.hover == Some(ReposHit::Button(hint.button));
            let (fill, label, key) = if k == 0 {
                let fill = if hovered {
                    mix(p.accent, p.on_accent, 0.15)
                } else {
                    p.accent
                };
                (fill, p.on_accent, mix(p.accent, p.on_accent, 0.7))
            } else {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                (fill, p.text, p.muted)
            };
            if k == 0 {
                c.rounded(x, y, w, h, r, fill);
            } else {
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
            }
            let chip_band = band(rect, scale);
            let tx = x + (ROW_CHIP_PAD as f32 * s).round();
            let end = self.mixed_text(
                c,
                &hint.label,
                SMALL_FONT_SIZE,
                tx,
                x + w,
                chip_band,
                scale,
                label,
            );
            if !hint.key.is_empty() {
                let kx = end + (ROW_CHIP_KEY_GAP as f32 * s).round();
                self.mixed_text(
                    c,
                    &hint.key,
                    KEY_FONT_SIZE,
                    kx,
                    x + w,
                    chip_band,
                    scale,
                    key,
                );
            }
        }
    }

    /// A row of a review comment's card from `x.0` to `x.1` (physical
    /// pixels) over `band`, in `fill`: its corners rounded at its first
    /// row and its last. Returns where its text starts.
    fn comment_card(
        &self,
        c: &mut Canvas,
        (x0, x1): (f32, f32),
        (y, h): (i32, i32),
        (first, last): (bool, bool),
        fill: Rgb,
        scale: f64,
    ) -> f32 {
        let s = scale as f32;
        let r = (4.0 * s).round();
        let top = if first { y as f32 } else { y as f32 - r };
        let bottom = if last {
            (y + h) as f32
        } else {
            (y + h) as f32 + r
        };
        c.clipped(x0 as i32, y, (x1 - x0).ceil() as i32, h, |c| {
            c.rounded(x0, top, x1 - x0, bottom - top, r, fill);
        });
        x0 + (COMMENT_INDENT as f32 * s).round()
    }

    /// At the right of a header, ending at `right` (physical pixels):
    /// `lead` muted, then the lines added and removed in their colours,
    /// each only when some are. Returns where they start.
    #[allow(clippy::too_many_arguments)]
    fn diff_counts(
        &self,
        c: &mut Canvas,
        lead: Option<&str>,
        (added, removed): (usize, usize),
        right: f32,
        band: (i32, i32),
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let size = SMALL_FONT_SIZE;
        let gap = (6.0 * scale as f32).round();
        let mut parts: Vec<(String, Rgb)> = Vec::new();
        if let Some(lead) = lead {
            parts.push((lead.to_string(), p.muted));
        }
        if added > 0 {
            parts.push((format!("+{added}"), p.success));
        }
        if removed > 0 {
            parts.push((format!("−{removed}"), p.error));
        }
        let widths: Vec<f32> = parts
            .iter()
            .map(|(t, _)| self.mixed_width(t, size, scale))
            .collect();
        let total = widths.iter().sum::<f32>() + gap * parts.len().saturating_sub(1) as f32;
        let left = (right - total).round();
        let mut x = left;
        for ((text, color), w) in parts.iter().zip(widths) {
            self.mixed_text(c, text, size, x, f32::MAX, band, scale, *color);
            x += w + gap;
        }
        left
    }

    /// The detail pane's `tab` segment in `rect`: an outlined pill, filled
    /// when it is the tab shown, lighter under the pointer.
    fn tab_segment(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        tab: DetailTab,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let on = view.detail.tab == tab;
        let fill = if on {
            p.selected
        } else if view.hover == Some(ReposHit::DetailTab(tab)) {
            mix(p.element, p.selected, 0.5)
        } else {
            p.element
        };
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let label = match tab {
            DetailTab::Diff => "Diff",
            DetailTab::Log => "Log",
            DetailTab::Stashes => "Stashes",
        };
        let color = if on { p.text } else { p.muted };
        self.centered(c, label, SMALL_FONT_SIZE, rect, scale, color);
    }

    /// The field in `rect`: outlined in the accent while it has the
    /// keyboard, with its caret; else in the border colour.
    fn repos_field_box(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (px(1).max(1) as f32, BUTTON_RADIUS * s);
        let (edge, fill) = if view.field_focused {
            (p.accent, p.element)
        } else if view.hover == Some(ReposHit::Field) {
            (p.border, mix(p.element, p.selected, 0.4))
        } else {
            (p.border, mix(p.elevated, p.element, 0.6))
        };
        c.rounded(x, y, w, h, r, edge);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        self.edit_line(
            c,
            &view.field,
            &view.placeholder,
            field_text(rect, scale),
            band(rect, scale),
            view.field_focused,
            FONT_SIZE,
            scale,
            p,
        );
    }

    /// The result bar in `rect`, on a tint of its colour: what the last
    /// batch did, its `buttons` at the right.
    #[allow(clippy::too_many_arguments)]
    fn result_bar(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        bar: &ResultBar,
        rect: Rect,
        buttons: &[(usize, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let color = tone_color(bar.tone, p);
        let (x, y, w, h) = rect_px(rect, scale);
        let border = px(1).max(1) as f32;
        if !matches!(bar.tone, Tone::Normal | Tone::Muted) {
            c.fill(
                (x + border) as i32,
                y as i32,
                (w - 2.0 * border) as i32,
                h as i32,
                mix(p.elevated, color, 0.1),
            );
        }
        let mut right = x + w - px(HELP_PADDING) as f32;
        if let Some((_, first)) = buttons.first() {
            right = px(first.loc.x) as f32 - (FIELD_GAP as f32 * s).round();
        }
        self.mixed_text(
            c,
            &bar.text,
            FONT_SIZE,
            x + px(HELP_PADDING) as f32 + PADDING * s,
            right,
            band(rect, scale),
            scale,
            color,
        );
        for &(i, rect) in buttons {
            let hint = &bar.buttons[i];
            let hit = ReposHit::Button(hint.button);
            self.hint_button(c, view, hint, hit, rect, scale, p);
        }
    }

    /// An action in `rect` (the footer's, the result bar's, the "more"
    /// button), the pointer on it as `hit`: its key on a keycap, then
    /// what it does; lighter under the pointer, dimmed when it can't be
    /// done now.
    #[allow(clippy::too_many_arguments)]
    fn hint_button(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        hint: &Hint,
        hit: ReposHit,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let r = BUTTON_RADIUS * s;
        let disabled = hint.disabled.is_some();
        if view.hover == Some(hit) {
            let fill = if disabled {
                mix(p.elevated, p.selected, 0.5)
            } else {
                p.selected
            };
            c.rounded(x, y, w, h, r, fill);
        }
        let hint_band = band(rect, scale);
        let (cap_h, b) = ((20.0 * s).round(), s.round().max(1.0));
        let cap_y = (y + (h - cap_h) / 2.0).round();
        let cap_x = (x + HINT_PAD as f32 * s).round();
        let pad = CAP_PADDING as f32 * s;
        let cap_w = (self.mixed_width(&hint.key, KEY_FONT_SIZE, scale) + 2.0 * pad).round();
        let (edge, key_color, label_color) = if disabled {
            (mix(p.elevated, p.border, 0.6), p.placeholder, p.placeholder)
        } else {
            (p.border, p.muted, p.text)
        };
        let right = x + w;
        if hint.key.is_empty() {
            // No key: the label on a button of its own.
            let (bx, bw) = (cap_x, w - 2.0 * (HINT_PAD as f32 * s).round());
            let cr = CAP_RADIUS * s;
            c.rounded(bx, cap_y, bw, cap_h, cr, edge);
            let inner = if disabled { p.elevated } else { p.element };
            c.rounded(
                bx + b,
                cap_y + b,
                bw - 2.0 * b,
                cap_h - 2.0 * b,
                cr - b,
                inner,
            );
            self.mixed_text(
                c,
                &hint.label,
                SMALL_FONT_SIZE,
                bx + pad,
                right,
                hint_band,
                scale,
                label_color,
            );
            return;
        }
        if cap_x + cap_w <= right {
            let cr = CAP_RADIUS * s;
            c.rounded(cap_x, cap_y, cap_w, cap_h, cr, edge);
            let inner = if disabled { p.elevated } else { p.element };
            c.rounded(
                cap_x + b,
                cap_y + b,
                cap_w - 2.0 * b,
                cap_h - 2.0 * b,
                cr - b,
                inner,
            );
        }
        self.mixed_text(
            c,
            &hint.key,
            KEY_FONT_SIZE,
            cap_x + pad,
            right,
            (cap_y as i32, cap_h as i32),
            scale,
            key_color,
        );
        self.mixed_text(
            c,
            &hint.label,
            SMALL_FONT_SIZE,
            cap_x + cap_w + (HINT_KEY_GAP as f32 * s).round(),
            right - (HINT_PAD as f32 * s).round() / 2.0,
            hint_band,
            scale,
            label_color,
        );
    }

    /// The branch picker's button in `rect`: the branch icon, the label
    /// (muted when they differ) and a chevron; lit while its menu is open
    /// or under the pointer, dimmed when there is nothing to switch.
    fn branch_button(&self, c: &mut Canvas, view: &ReposView, rect: Rect, scale: f64, p: &Palette) {
        let picker = &view.branch;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let hovered = picker.enabled && view.hover == Some(ReposHit::BranchButton);
        let fill = if picker.menu.is_some() || hovered {
            p.selected
        } else {
            p.element
        };
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let (icon_color, label_color) = match (picker.enabled, picker.mixed) {
            (false, _) => (p.placeholder, p.placeholder),
            (true, true) => (p.muted, p.muted),
            (true, false) => (p.muted, p.text),
        };
        let icon = Rectangle::new(
            (rect.loc.x + PICKER_PAD, rect.loc.y).into(),
            (PICKER_ICON, rect.size.h).into(),
        );
        icon_sdf(c, icon, scale, icon_color, |x, y| git_branch(x, y, 0.9 * s));
        let chevron = Rectangle::new(
            (
                rect.loc.x + rect.size.w - PICKER_PAD - PICKER_CHEVRON,
                rect.loc.y,
            )
                .into(),
            (PICKER_CHEVRON, rect.size.h).into(),
        );
        icon_sdf(c, chevron, scale, icon_color, |x, y| chevron_down(x, y, s));
        let tx = ((icon.loc.x + icon.size.w + PICKER_GAP) as f64 * scale) as f32;
        let tend = ((chevron.loc.x - PICKER_GAP) as f64 * scale) as f32;
        self.mixed_text(
            c,
            &picker.label,
            FONT_SIZE,
            tx,
            tend,
            band(rect, scale),
            scale,
            label_color,
        );
    }

    /// The branch menu, a card in `card` with its search in `search`, the
    /// row creating the branch typed in `create`, then a row per branch in
    /// `rows`: the one Enter picks lit, the one they are all on checked;
    /// the groups' headings in `headings`, muted. Rows and headings are cut
    /// to `list`, where they show.
    #[allow(clippy::type_complexity)]
    fn branch_menu(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        menu: &BranchMenuView,
        (card, search, create, (list, rows), headings): (
            Rect,
            Rect,
            Option<Rect>,
            (Rect, &[Rect]),
            &[(usize, Rect)],
        ),
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        card_over(c, card, MENU_RADIUS, scale, p);
        let (x, y, w, h) = rect_px(search, scale);
        let (b, r) = ((scale.round() as i32).max(1) as f32, BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.accent);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        self.edit_line(
            c,
            &menu.query,
            &menu.placeholder,
            field_text(search, scale),
            band(search, scale),
            true,
            FONT_SIZE,
            scale,
            p,
        );
        if let (Some(rect), Some(row)) = (create, &menu.create) {
            let (x, y, w, h) = rect_px(rect, scale);
            let r = (MENU_RADIUS - 2.0) * s;
            if row.picked {
                c.rounded(x, y, w, h, r, p.selected);
            } else if view.hover == Some(ReposHit::BranchCreate) {
                c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
            }
            let color = if row.problem.is_some() {
                p.placeholder
            } else {
                p.text
            };
            let mark = branch_check(rect);
            let icon = if row.problem.is_some() {
                p.placeholder
            } else {
                p.accent
            };
            icon_sdf(c, mark, scale, icon, |x, y| {
                plus(x, y, 0.0, 0.0, 5.0 * s, s)
            });
            let pad = BRANCH_ROW_PAD as f32 * s;
            let end = (mark.loc.x as f64 * scale) as f32;
            let row_band = band(rect, scale);
            self.mixed_text(
                c,
                &row.label,
                FONT_SIZE,
                x + pad,
                end,
                row_band,
                scale,
                color,
            );
            if menu.rows.is_empty() {
                return;
            }
        }
        if menu.rows.is_empty() {
            let row = Rectangle::new(
                (
                    card.loc.x + BRANCH_MENU_PAD,
                    search.loc.y + search.size.h + BRANCH_MENU_PAD,
                )
                    .into(),
                (
                    (card.size.w - 2 * BRANCH_MENU_PAD).max(0),
                    repos_row(view.density),
                )
                    .into(),
            );
            let (x, end) = phys(row, scale);
            let pad = BRANCH_ROW_PAD as f32 * s;
            let row_band = band(row, scale);
            self.text(
                c,
                &menu.empty,
                FONT_SIZE,
                x + pad,
                end - pad,
                row_band,
                scale,
                p.placeholder,
            );
            return;
        }
        let (lx, ly, lw, lh) = rect_px(list, scale);
        c.clipped(lx as i32, ly as i32, lw as i32, lh as i32, |c| {
            for &(i, rect) in headings {
                let Some((_, label)) = menu.headings.get(i) else {
                    continue;
                };
                let (x, end) = phys(rect, scale);
                let pad = BRANCH_ROW_PAD as f32 * s;
                let row_band = band(rect, scale);
                self.text(
                    c,
                    label,
                    FONT_SIZE,
                    x + pad,
                    end - pad,
                    row_band,
                    scale,
                    p.placeholder,
                );
            }
            for (i, (rect, label)) in rows.iter().zip(&menu.rows).enumerate() {
                let (x, y, w, h) = rect_px(*rect, scale);
                let r = (MENU_RADIUS - 2.0) * s;
                let on_delete = view.hover == Some(ReposHit::BranchDelete(i));
                let hovered = on_delete || view.hover == Some(ReposHit::BranchRow(i));
                if menu.picked == Some(i) {
                    c.rounded(x, y, w, h, r, p.selected);
                } else if hovered {
                    c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
                }
                let mark = branch_check(*rect);
                if menu.current == Some(i) {
                    icon_sdf(c, mark, scale, p.accent, |x, y| check(x, y, s));
                }
                // The delete button shows on the row pointed at or picked.
                let delete = branch_delete(*rect);
                let deletable = menu.deletable.get(i) == Some(&true);
                if deletable && (hovered || menu.picked == Some(i)) {
                    let color = if on_delete { p.error } else { p.placeholder };
                    icon_sdf(c, delete, scale, color, |x, y| trash(x, y, s));
                }
                let pad = BRANCH_ROW_PAD as f32 * s;
                let end = (delete.loc.x as f64 * scale) as f32;
                let row_band = band(*rect, scale);
                self.mixed_text(c, label, FONT_SIZE, x + pad, end, row_band, scale, p.text);
            }
        });
    }

    /// The sheet over the panel, which is dimmed under it: its title, a
    /// line per repository and its buttons, the default one in the accent.
    #[allow(clippy::too_many_arguments)]
    fn sheet(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        sheet: &SheetView,
        parts: &Parts,
        shift: impl Fn(Rect) -> Rect,
        scale: f64,
        p: &Palette,
    ) {
        let Some(card) = parts.layout.sheet else {
            return;
        };
        let s = scale as f32;
        let (px_, py_, pw, ph) = rect_px(shift(parts.layout.panel), scale);
        let radius = HELP_RADIUS * s;
        c.paint_in(
            px_ as i32,
            py_ as i32,
            pw as i32,
            ph as i32,
            [0, 0, 0],
            |x, y| coverage(rounded_rect(x, y, px_, py_, pw, ph, radius)) * SCRIM * p.shadow,
        );
        card_over(c, shift(card), HELP_RADIUS, scale, p);

        if let Some(title) = parts.sheet_title {
            let title = shift(title);
            let (tx, tend) = phys(title, scale);
            self.mixed_text(
                c,
                &sheet.title,
                LABEL_FONT_SIZE,
                tx,
                tend,
                band(title, scale),
                scale,
                p.text,
            );
        }
        let shown = parts.sheet_lines.len();
        // Without room for them all, the last line says how many more.
        let more = sheet.lines.len().saturating_sub(shown);
        for (n, (rect, (name, verdict, tone))) in
            parts.sheet_lines.iter().zip(&sheet.lines).enumerate()
        {
            let rect = shift(*rect);
            let (lx, lend) = phys(rect, scale);
            let line_band = band(rect, scale);
            if more > 0 && n + 1 == shown {
                let rest = format!("and {} more", more + 1);
                self.text(
                    c,
                    &rest,
                    FONT_SIZE,
                    lx,
                    lend,
                    line_band,
                    scale,
                    p.placeholder,
                );
                break;
            }
            let name_end = lx + (rect.size.w as f32 * SHEET_NAME_SHARE * s).round();
            self.mixed_text(c, name, FONT_SIZE, lx, name_end, line_band, scale, p.text);
            self.mixed_text(
                c,
                verdict,
                FONT_SIZE,
                name_end + (FIELD_GAP as f32 * s).round(),
                lend,
                line_band,
                scale,
                tone_color(*tone, p),
            );
        }
        for &(i, rect) in &parts.sheet_buttons {
            let button = sheet.buttons[i];
            let hovered = view.hover == Some(ReposHit::Button(button));
            self.button(
                c,
                button.label(),
                shift(rect),
                (hovered, false),
                i == sheet.default,
                scale,
                p,
            );
        }
    }
}

/// What of `view` the panel's frame is drawn from: all but the tree's
/// lines and where it stands, the branch menu (but that it is open), the
/// sheet, and the pointer on what those show. `empty` is what the tree
/// says, empty while it has lines.
fn frame_view(view: &ReposView) -> ReposView {
    use ReposHit::*;
    let hover = view.hover.filter(|hit| {
        !matches!(
            hit,
            Line(_)
                | Check(_)
                | Expander(_)
                | Chip(_)
                | Mark(..)
                | BranchRow(_)
                | BranchDelete(_)
                | BranchCreate
                | BranchMenu
        )
    });
    let menu = view.branch.menu.as_ref().map(|_| BranchMenuView::default());
    ReposView {
        title: view.title.clone(),
        summary: view.summary.clone(),
        lines: Vec::new(),
        part: 0.0,
        over: 0.0,
        first: 0,
        total: 0,
        selected: None,
        hover,
        field: view.field.clone(),
        field_focused: view.field_focused,
        placeholder: view.placeholder.clone(),
        error: view.error.clone(),
        result: view.result.clone(),
        hints: view.hints.clone(),
        detail: view.detail.clone(),
        sheet: None,
        empty: match view.lines.is_empty() {
            true => view.empty.clone(),
            false => String::new(),
        },
        branch: BranchPicker {
            label: view.branch.label.clone(),
            mixed: view.branch.mixed,
            enabled: view.branch.enabled,
            menu,
        },
        density: view.density,
        tree_share: view.tree_share,
    }
}

/// The frame's inputs: the view as `frame_view` has it, the output's
/// size, the theme and the scale.
type FrameKey = (ReposView, Size<i32, Logical>, super::Theme, f64);
/// A tree line's: the line as drawn, its size, the theme and the scale.
type TreeKey = (TreeRow, Size<i32, Logical>, super::Theme, f64);
/// A scrollbar thumb's: its size, the theme and the scale.
type ThumbKey = (Size<i32, Logical>, super::Theme, f64);
/// The branch menu's: the menu, what the pointer is over, its card, the
/// density, the theme and the scale.
type MenuKey = (
    BranchMenuView,
    Option<ReposHit>,
    Rect,
    Density,
    super::Theme,
    f64,
);
/// The sheet's: the sheet, what the pointer is over, its card, the
/// output's size, the theme and the scale.
type SheetKey = (
    SheetView,
    Option<ReposHit>,
    Option<Rect>,
    Size<i32, Logical>,
    super::Theme,
    f64,
);

/// The repositories panel as the chrome shows it, fading in and out (see
/// `Overlay`): its frame; over it each of the tree's lines in a buffer of
/// its own and the thumb of its scrollbar, so that scrolling the tree only
/// moves them; then the branch menu and the sheet over all.
#[derive(Default)]
pub(super) struct ReposLayers {
    overlay: super::Overlay<FrameKey>,
    tree: super::RowCache<TreeKey>,
    tree_thumb: super::Cached<ThumbKey>,
    menu: super::Cached<MenuKey>,
    sheet: super::Cached<SheetKey>,
    /// How many times the frame was drawn, for tests.
    #[cfg(test)]
    frames: usize,
}

impl ReposLayers {
    /// The parts showing the repositories panel at `now`, front to back.
    pub(super) fn parts(
        &mut self,
        view: Option<ReposView>,
        output: Size<i32, Logical>,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
    ) -> Vec<super::Part> {
        let placed = view.as_ref().map(|view| {
            let (loc, size) = super::shadowed(panel_rect(output));
            ((frame_view(view), output, theme, scale), loc, size)
        });
        let Self {
            overlay,
            tree,
            tree_thumb,
            menu,
            sheet,
            #[cfg(test)]
            frames,
        } = self;
        let p = theme.palette();
        let draw = |(view, output, ..): &FrameKey| {
            #[cfg(test)]
            {
                *frames += 1;
            }
            let laid = parts(view, *output);
            painter
                .repos_frame(view, &laid, scale, p)
                .into_buffer(false)
        };
        let over = |_: &FrameKey| {
            let Some(view) = &view else {
                return Vec::new();
            };
            let laid = parts(view, output);
            let layout = &laid.layout;
            let mut over = Vec::new();
            if let Some(s) = &view.sheet {
                let (loc, size) = super::shadowed(layout.panel);
                let key = (s.clone(), view.hover, layout.sheet, output, theme, scale);
                let buffer = sheet.get(key, |_| {
                    let mut c = Canvas::new(physical(size, scale));
                    let by = buffer_origin(layout.panel);
                    painter.sheet(&mut c, view, s, &laid, |r| moved(r, by), scale, p);
                    c.into_buffer(false)
                });
                over.push(super::Part::new(buffer, loc, size, 1.0));
            }
            if let (Some(m), Some((card, _))) = (&view.branch.menu, &layout.branch_menu) {
                let (loc, size) = super::shadowed(*card);
                let key = (m.clone(), view.hover, *card, view.density, theme, scale);
                let buffer = menu.get(key, |_| {
                    let mut c = Canvas::new(physical(size, scale));
                    let by = Point::from((SHADOW - card.loc.x, SHADOW - card.loc.y));
                    painter.repos_menu(&mut c, view, layout, by, scale, p);
                    c.into_buffer(false)
                });
                over.push(super::Part::new(buffer, loc, size, 1.0));
            }
            let (track, shown) = tree_bar(view, layout);
            let pos = view.first as f64 + view.part;
            if let Some((top, h)) = thumb_place(track, pos, shown, view.total) {
                let size = Size::from((SCROLLBAR, h));
                let buffer =
                    tree_thumb.get((size, theme, scale), |_| painter.thumb(size, scale, p));
                let at = Point::from((track.loc.x as f64, top));
                over.push(super::Part::new(buffer, track.loc, size, 1.0).at(at));
            }
            let row_h = repos_row(view.density);
            let shift = scrolled_by(view.part, view.over, row_h);
            for (i, (rect, ..)) in layout.lines.iter().enumerate() {
                let key = (tree_row(view, i), rect.size, theme, scale);
                let buffer = tree.get(key, rect.size, scale, |(row, size, ..)| {
                    painter.tree_row(row, *size, scale, p)
                });
                let y = (layout.tree_list.loc.y + i as i32 * row_h) as f64 - shift;
                let at = Point::from((rect.loc.x as f64, y));
                let part = super::Part::new(&buffer, rect.loc, rect.size, 1.0).at(at);
                over.push(part.clipped(layout.tree_list));
            }
            over
        };
        let parts = overlay.parts(placed, now, true, draw, over);
        tree.sweep();
        if !overlay.lingers() {
            tree.clear();
        }
        parts
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::chrome::{Theme, inside};

    const SIZES: [(i32, i32); 3] = [(800, 600), (1280, 800), (2560, 1440)];

    fn repo(label: &str, branch: Option<&str>) -> RepoLine {
        RepoLine {
            check: Check::Off,
            expanded: None,
            label: label.into(),
            branch: branch.map(Into::into),
            dirty: false,
            ahead: 0,
            behind: 0,
            no_upstream: false,
            stashes: 0,
            chip: None,
            state: LineState::Settled,
        }
    }

    fn file(code: char, path: &str, check: Check) -> TreeLine {
        TreeLine::File(FileLine {
            check,
            code,
            path: path.into(),
            conflicted: code == 'U',
        })
    }

    fn hint(button: ReposButton, key: &str, disabled: Option<&str>) -> Hint {
        Hint {
            button,
            label: button.label().to_lowercase(),
            key: key.into(),
            disabled: disabled.map(Into::into),
        }
    }

    const DENSITY: Density = Density::Comfortable;

    fn diff_line(kind: LineKind, text: &str, words: &[Range<usize>]) -> DetailRow {
        DetailRow::Line(crate::diff::Line {
            kind,
            text: text.into(),
            number: 1,
            words: words.to_vec(),
            syntax: crate::syntax::lang("x.rs")
                .map(|lang| crate::syntax::highlight(lang, text, &mut Default::default()))
                .unwrap_or_default(),
        })
    }

    fn detail_line(text: &str, tone: DiffTone) -> DetailLine {
        DetailLine {
            text: text.into(),
            tone,
        }
    }

    /// A panel with something of everything but a sheet.
    fn full() -> ReposView {
        use ReposButton::*;
        let lines = vec![
            TreeLine::Repo(RepoLine {
                check: Check::Partial,
                expanded: Some(true),
                dirty: true,
                ahead: 2,
                ..repo("gerdr", Some("feat/x"))
            }),
            file('M', "src/git.rs", Check::On),
            file('?', "notes.md", Check::Off),
            file(
                'R',
                "src/old/name.rs → src/chrome/a/rather/long/path/to/the/new_name.rs",
                Check::Off,
            ),
            TreeLine::Repo(RepoLine {
                expanded: Some(true),
                chip: Some(Chip {
                    text: "MERGING · 2".into(),
                    tone: Tone::Error,
                    tip: "A merge stopped on 2 conflicted files".into(),
                }),
                state: LineState::Attention,
                ..repo("herdr", Some("feat/x"))
            }),
            file('U', "src/api.rs", Check::Off),
            TreeLine::Repo(RepoLine {
                check: Check::On,
                expanded: Some(false),
                behind: 1,
                ..repo("zed-ext", Some("main"))
            }),
            TreeLine::Repo(RepoLine {
                no_upstream: true,
                stashes: 2,
                ..repo("notes", Some("feat/x"))
            }),
            TreeLine::Repo(RepoLine {
                state: LineState::Busy("pushing…".into()),
                chip: Some(Chip {
                    text: "detached".into(),
                    tone: Tone::Muted,
                    tip: "Not on a branch".into(),
                }),
                ..repo("scratch", None)
            }),
            TreeLine::Repo(RepoLine {
                ahead: 1,
                behind: 3,
                ..repo("infra", Some("feat/x"))
            }),
        ];
        let total = lines.len() + 12;
        ReposView {
            title: "gerdr".into(),
            summary: "2 dirty · 1 conflict · fetched 2m ago".into(),
            lines,
            part: 0.0,
            over: 0.0,
            first: 0,
            total,
            selected: Some(1),
            hover: Some(ReposHit::Line(6)),
            field: Line::from("Add multi-repo panel"),
            field_focused: true,
            placeholder: "Commit message".into(),
            error: Some("Type a commit message first".into()),
            result: Some(ResultBar {
                text: "Commit and push: 2 done · 1 conflict".into(),
                tone: Tone::Warning,
                buttons: vec![hint(Undo, "Alt+Z", None), hint(RetryFailed, "Alt+R", None)],
            }),
            hints: vec![
                hint(Commit, "Ctrl+Enter", None),
                hint(CommitPush, "Ctrl+Shift+Enter", None),
                hint(Push, "Ctrl+P", None),
                hint(ForcePush, "Ctrl+Shift+P", Some("nothing has diverged")),
                hint(Fetch, "Ctrl+F", None),
                hint(Pull, "Ctrl+L", None),
                hint(Merge, "Ctrl+M", None),
                hint(Rebase, "Ctrl+Shift+M", None),
                hint(MergeFrom, "Alt+M", None),
                hint(RebaseFrom, "Alt+Shift+M", None),
                hint(OpenInZed, "Ctrl+E", None),
                hint(MarkResolved, "Ctrl+R", None),
                hint(Continue, "Ctrl+G", None),
                hint(Abort, "Ctrl+Shift+Del", None),
                hint(Stash, "Alt+S", None),
                hint(Help, "F1", None),
            ],
            detail: DetailView {
                tab: DetailTab::Diff,
                title: "All changes · 2 repositories · 3 files · +3 −1".into(),
                rows: vec![
                    DetailRow::File(DiffFile {
                        repo: Some("gerdr".into()),
                        path: "src/git.rs".into(),
                        code: 'M',
                        check: Check::On,
                        added: 2,
                        removed: 1,
                        selected: true,
                        ..DiffFile::default()
                    }),
                    DetailRow::Hunk {
                        context: "pub fn status(dir: &Path)".into(),
                        number: 104,
                    },
                    diff_line(LineKind::Context, "    let root = toplevel(dir)?;", &[]),
                    diff_line(
                        LineKind::Removed,
                        "    let out = git(dir, &[\"status\", \"--porcelain=v2\"])?;",
                        &[],
                    ),
                    diff_line(
                        LineKind::Added,
                        "    let out = git(dir, &[\"status\", \"--porcelain=v2\", \"-z\"])?;",
                        &[51..57],
                    ),
                    diff_line(
                        LineKind::Added,
                        "    let changes = parse_changes(&out);",
                        &[],
                    ),
                    DetailRow::Comment {
                        text: "Why -z here? Paths with newlines".into(),
                        first: true,
                        last: false,
                    },
                    DetailRow::Comment {
                        text: "need it, but check the parser.".into(),
                        first: false,
                        last: true,
                    },
                    DetailRow::Editor {
                        text: "And the caller".into(),
                        caret: Some(7),
                        selection: 4..7,
                        first: true,
                        last: true,
                    },
                    DetailRow::Note("Shift+Enter breaks the line".into()),
                    DetailRow::Note("Binary file".into()),
                    DetailRow::Text(detail_line("", DiffTone::Plain)),
                    DetailRow::Repo(DiffRepo {
                        label: "zed".into(),
                        branch: Some("main".into()),
                        files: 2,
                        added: 1,
                        removed: 0,
                    }),
                    DetailRow::File(DiffFile {
                        path: "old.rs → crates/editor/src/new.rs".into(),
                        code: 'R',
                        ..DiffFile::default()
                    }),
                    DetailRow::File(DiffFile {
                        path: "notes.md".into(),
                        code: '?',
                        folded: true,
                        added: 1,
                        ..DiffFile::default()
                    }),
                    DetailRow::Text(detail_line(
                        "error: hook pre-commit failed",
                        DiffTone::Error,
                    )),
                ],
                sticky: true,
                marks: (0..16)
                    .map(|i| RowMark {
                        selected: (2..=4).contains(&i),
                        top: i == 2,
                        bottom: i == 4,
                        noted: i == 4,
                    })
                    .collect(),
                title_buttons: vec![hint(CopyReview, "", None)],
                row_buttons: vec![
                    (4, hint(Comment, "C", None)),
                    (9, hint(SaveComment, "Enter", None)),
                    (9, hint(DiscardComment, "Esc", None)),
                    (9, hint(DeleteComment, "", None)),
                ],
                part: 0.0,
                first: 3,
                total: 40,
                focused: true,
            },
            sheet: None,
            empty: "No repositories in this project".into(),
            branch: BranchPicker {
                label: "mixed branches".into(),
                mixed: true,
                enabled: true,
                menu: None,
            },
            density: DENSITY,
            tree_share: REPOS_TREE_SHARE,
        }
    }

    /// `full` with the branch menu open, the pointer on its second row.
    fn with_menu() -> ReposView {
        let rows = ["main / master", "feat/x", "production", "release/2.1"];
        ReposView {
            branch: BranchPicker {
                menu: Some(BranchMenuView {
                    rows: rows.map(String::from).to_vec(),
                    deletable: vec![false, true, true, false],
                    current: Some(1),
                    picked: Some(0),
                    empty: "No branch they all have".into(),
                    query: Line::default(),
                    placeholder: "Find or create a branch".into(),
                    create: None,
                    headings: vec![(0, "Local".into()), (3, "Remote".into())],
                    part: 0.0,
                }),
                ..full().branch
            },
            hover: Some(ReposHit::BranchRow(1)),
            ..full()
        }
    }

    /// `with_menu` with no branch they all have, on one branch.
    fn with_empty_menu() -> ReposView {
        let mut view = with_menu();
        view.branch.label = "main / master".into();
        view.branch.mixed = false;
        if let Some(menu) = &mut view.branch.menu {
            *menu = BranchMenuView {
                empty: "No branch matches".into(),
                query: "zzz".into(),
                placeholder: menu.placeholder.clone(),
                ..BranchMenuView::default()
            };
        }
        view
    }

    /// `with_menu` with a name typed that no branch shown has: a row
    /// creating it under the search, picked.
    fn with_create_menu() -> ReposView {
        let mut view = with_menu();
        if let Some(menu) = &mut view.branch.menu {
            menu.query = "feat/new-login-flow".into();
            menu.picked = None;
            menu.create = Some(BranchCreate {
                label: "Create feat/new-login-flow from main".into(),
                problem: None,
                picked: true,
            });
        }
        view.hover = Some(ReposHit::BranchCreate);
        view
    }

    /// A name typed that can't be a branch's, and nothing found.
    fn with_bad_name_menu() -> ReposView {
        let mut view = with_empty_menu();
        if let Some(menu) = &mut view.branch.menu {
            menu.query = "a..b".into();
            menu.create = Some(BranchCreate {
                label: "Can't create a..b: it can't have ..".into(),
                problem: Some("it can't have ..".into()),
                picked: true,
            });
        }
        view
    }

    /// Nothing in it: no repositories, hints, error or result.
    fn empty() -> ReposView {
        ReposView {
            lines: Vec::new(),
            first: 0,
            total: 0,
            selected: None,
            hover: None,
            field: Line::default(),
            field_focused: false,
            error: None,
            result: None,
            hints: Vec::new(),
            detail: DetailView::default(),
            branch: BranchPicker {
                label: "No repository".into(),
                mixed: true,
                ..BranchPicker::default()
            },
            ..full()
        }
    }

    /// `full` with the check before a merge asking.
    fn with_sheet() -> ReposView {
        use ReposButton::*;
        ReposView {
            sheet: Some(SheetView {
                title: "Merge the base branch into 3 repositories?".into(),
                lines: vec![
                    ("gerdr".into(), "clean".into(), Tone::Success),
                    ("herdr".into(), "conflicts: a.rs, b.rs".into(), Tone::Error),
                    (
                        "zed-ext".into(),
                        "blocked: detached HEAD".into(),
                        Tone::Warning,
                    ),
                ],
                buttons: vec![All, CleanOnly, Cancel],
                default: 1,
            }),
            hover: Some(ReposHit::Button(CleanOnly)),
            ..full()
        }
    }

    /// The views the layout is checked with: bare, full, and with a sheet.
    fn views() -> Vec<ReposView> {
        let bare = ReposView {
            error: None,
            result: None,
            ..full()
        };
        vec![
            empty(),
            bare,
            full(),
            with_sheet(),
            with_menu(),
            with_empty_menu(),
            with_create_menu(),
            with_bad_name_menu(),
        ]
    }

    fn centre(r: Rect) -> Point<f64, Logical> {
        (
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        )
            .into()
    }

    #[test]
    fn every_part_is_inside_the_panel() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            for view in views() {
                let l = repos_layout(&view, size);
                let output = Rectangle::from_size(size);
                assert!(inside(output, l.panel), "{size:?}");
                assert_eq!(l.panel.loc.x, w - (l.panel.loc.x + l.panel.size.w));
                let mut parts = vec![l.header, l.tree, l.detail, l.field, l.footer];
                // The branch picker at the header's left, its menu under it
                // from its left edge, above the field.
                assert!(inside(l.header, l.branch), "{size:?}");
                assert_eq!(l.branch.loc.x, l.header.loc.x + HELP_PADDING);
                parts.push(l.branch);
                assert_eq!(l.branch_menu.is_some(), view.branch.menu.is_some());
                if let (Some((menu, rows)), Some(m)) = (&l.branch_menu, &view.branch.menu) {
                    parts.push(*menu);
                    assert!(menu.loc.y >= l.branch.loc.y + l.branch.size.h);
                    assert!(menu.loc.y + menu.size.h <= l.field.loc.y, "{size:?}");
                    assert!(menu.size.w >= l.branch.size.w);
                    assert_eq!(menu.loc.x, l.branch.loc.x);
                    assert!(menu.loc.x + menu.size.w <= l.header.loc.x + l.header.size.w);
                    assert!(!rows.is_empty() || m.rows.is_empty());
                    assert!(rows.len() <= m.rows.len());
                    for row in rows {
                        assert!(inside(*menu, *row));
                    }
                    // The headings: in it, apart from the rows.
                    for (i, heading) in &l.branch_headings {
                        assert!(*i < m.headings.len());
                        assert!(inside(*menu, *heading));
                        assert!(rows.iter().all(|r| !r.overlaps(*heading)));
                    }
                    // The row creating the branch typed: under the search,
                    // over the branches.
                    assert_eq!(l.branch_create.is_some(), m.create.is_some());
                    if let (Some(create), Some(search)) = (l.branch_create, l.branch_search) {
                        assert!(inside(*menu, create));
                        assert!(create.loc.y >= search.loc.y + search.size.h);
                        assert!(rows.iter().all(|r| r.loc.y >= create.loc.y + create.size.h));
                    }
                }
                parts.extend(l.result);
                parts.extend(l.sheet);
                parts.extend(l.detail_tabs.iter().map(|(_, r)| *r));
                parts.extend(l.buttons.iter().map(|(_, r)| *r));
                for (line, check, expander) in &l.lines {
                    parts.extend([*line, *check]);
                    parts.extend(*expander);
                    assert!(inside(*line, *check));
                    assert!(expander.is_none_or(|e| inside(*line, e)));
                    // The tree's lines keep out of the detail pane.
                    assert!(inside(l.tree, *line));
                    assert!(line.loc.x + line.size.w <= l.detail.loc.x);
                }
                for part in parts {
                    assert!(inside(l.panel, part), "{size:?}: {part:?}");
                }
                // Top to bottom: header, tree and detail, field, result,
                // footer.
                assert!(l.tree.loc.y >= l.header.loc.y + l.header.size.h);
                assert_eq!(l.tree.loc.y, l.detail.loc.y);
                assert!(l.field.loc.y >= l.tree.loc.y + l.tree.size.h);
                let below_field = l.result.unwrap_or(l.footer);
                assert!(below_field.loc.y >= l.field.loc.y + l.field.size.h);
                assert!(l.footer.loc.y + l.footer.size.h <= l.panel.loc.y + l.panel.size.h);
                for (_, tab) in &l.detail_tabs {
                    assert!(inside(l.detail, *tab));
                }
                // Every line of a page fits (the panel is given no more).
                let page = view.lines.len().min(repos_page(size, DENSITY).0);
                assert!(l.lines.len() >= page, "{size:?}");
                // The footer's actions stay in the footer and the result
                // bar's in it; the "more" button in the footer too, with
                // the rest of the actions behind it.
                let n_result = view.result.as_ref().map_or(0, |r| r.buttons.len());
                for (i, (_, rect)) in l.buttons.iter().enumerate() {
                    if i < n_result {
                        assert!(inside(l.result.unwrap(), *rect));
                    } else if l.sheet.is_none_or(|s| !inside(s, *rect)) {
                        assert!(inside(l.footer, *rect), "{size:?}: {rect:?}");
                    }
                }
                if let Some(more) = l.more {
                    assert!(inside(l.footer, more), "{size:?}");
                    assert!(!l.overflow.is_empty());
                }
                let n_sheet = view.sheet.as_ref().map_or(0, |s| s.buttons.len());
                let in_footer = l.buttons.len() - n_result - n_sheet;
                assert_eq!(in_footer + l.overflow.len(), view.hints.len(), "{size:?}");
                for &i in &l.overflow {
                    assert!(!l.buttons.iter().any(|(b, _)| *b == view.hints[i].button));
                }
                // The chips are on their lines, and the splitter between
                // the tree and the detail.
                for (i, chip) in &l.chips {
                    assert!(inside(l.lines[*i].0, *chip));
                }
                assert!(l.splitter.loc.x < l.detail.loc.x);
                assert!(l.splitter.loc.x + l.splitter.size.w > l.detail.loc.x);
                // No two buttons overlap.
                for (i, (_, a)) in l.buttons.iter().enumerate() {
                    for (_, b) in &l.buttons[i + 1..] {
                        assert!(!a.overlaps(*b), "{a:?} {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_footer_wraps_and_puts_what_does_not_fit_behind_more() {
        let view = full();
        let l = repos_layout(&view, (2560, 1440).into());
        let hints = l
            .buttons
            .iter()
            .filter(|(b, _)| view.hints.iter().any(|h| h.button == *b))
            .count();
        assert_eq!(hints, view.hints.len());
        assert_eq!(l.more, None);
        assert!(l.overflow.is_empty());
        let narrow = repos_layout(&view, (800, 600).into());
        assert!(narrow.footer.size.h > footer_h(1));
        assert!(narrow.footer.size.h <= footer_h(REPOS_FOOTER_ROWS));
        let more = narrow.more.expect("a more button");
        assert!(!narrow.overflow.is_empty());
        // The last ones go behind it, in order.
        let n = view.hints.len();
        let behind: Vec<usize> = (n - narrow.overflow.len()..n).collect();
        assert_eq!(narrow.overflow, behind);
        assert_eq!(repos_hit(&narrow, centre(more)), Some(ReposHit::More));
        // No button is under it.
        for (_, r) in &narrow.buttons {
            assert!(!r.overlaps(more), "{r:?}");
        }
    }

    #[test]
    fn the_tree_share_is_dragged_within_bounds() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let panel = panel_rect(size);
        let at = |share: f64| panel.loc.x as f64 + panel.size.w as f64 * share;
        assert!((repos_share_at(at(0.5), size) - 0.5).abs() < 0.01);
        assert_eq!(repos_share_at(at(0.05), size), REPOS_TREE_SHARE_MIN);
        assert_eq!(repos_share_at(at(0.95), size), REPOS_TREE_SHARE_MAX);
        let wide = ReposView {
            tree_share: 0.7,
            ..full()
        };
        let l = repos_layout(&wide, size);
        assert!(l.tree.size.w > repos_layout(&full(), size).tree.size.w);
        assert_eq!(repos_hit(&l, centre(l.splitter)), Some(ReposHit::Splitter));
    }

    #[test]
    fn hits_land_on_what_is_under_them() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            let plain = |v: &ReposView| v.sheet.is_none() && v.branch.menu.is_none();
            for view in views().into_iter().filter(plain) {
                let l = repos_layout(&view, size);
                let hit = |r: Rect| repos_hit(&l, centre(r));
                assert_eq!(hit(l.branch), Some(ReposHit::BranchButton));
                for (i, (line, check, expander)) in l.lines.iter().enumerate() {
                    assert_eq!(hit(*check), Some(ReposHit::Check(i)));
                    if let Some(e) = expander {
                        assert_eq!(hit(*e), Some(ReposHit::Expander(i)));
                    }
                    // Its middle may be on a mark on a narrow panel.
                    let on_it = matches!(
                        hit(*line),
                        Some(ReposHit::Line(j) | ReposHit::Mark(j, _)) if j == i
                    );
                    assert!(on_it, "line {i}");
                }
                for &(i, mark, r) in &l.marks {
                    assert!(within(l.lines[i].0, r), "{mark:?} on line {i}");
                    assert_eq!(hit(r), Some(ReposHit::Mark(i, mark)));
                }
                for &(i, r) in &l.chips {
                    assert_eq!(hit(r), Some(ReposHit::Chip(i)));
                }
                for (b, r) in &l.buttons {
                    assert_eq!(hit(*r), Some(ReposHit::Button(*b)));
                }
                for (t, r) in &l.detail_tabs {
                    assert_eq!(hit(*r), Some(ReposHit::DetailTab(*t)));
                }
                assert_eq!(hit(l.field), Some(ReposHit::Field));
                for (i, r) in l.detail_rows.iter().enumerate() {
                    // Past a file header's chevron and checkbox.
                    let end = Rectangle::new(
                        (r.loc.x + r.size.w - 4, r.loc.y).into(),
                        (4, r.size.h).into(),
                    );
                    assert_eq!(hit(end), Some(ReposHit::DiffRow(i)));
                }
                for &(i, r) in &l.detail_folds {
                    assert_eq!(hit(r), Some(ReposHit::DiffFold(i)));
                }
                for &(i, r) in &l.detail_checks {
                    assert_eq!(hit(r), Some(ReposHit::DiffCheck(i)));
                }
                assert!(matches!(
                    hit(l.detail),
                    Some(ReposHit::Detail | ReposHit::DiffRow(_))
                ));
                assert_eq!(hit(l.header), Some(ReposHit::Panel));
                assert_eq!(repos_hit(&l, (1.0, 1.0).into()), None);
                let below = (
                    l.panel.loc.x as f64 + 4.0,
                    (l.panel.loc.y + l.panel.size.h) as f64 + 2.0,
                );
                assert_eq!(repos_hit(&l, below.into()), None);
            }
        }
    }

    #[test]
    fn marks_say_what_they_mean() {
        let view = full();
        let l = repos_layout(&view, (1600, 1000).into());
        let marks: Vec<_> = l.marks.iter().map(|&(i, m, _)| (i, m)).collect();
        assert!(marks.contains(&(6, RepoMark::Behind)), "{marks:?}");
        let TreeLine::Repo(repo) = &view.lines[6] else {
            panic!("a repository");
        };
        assert_eq!(
            RepoMark::Behind.tip(repo),
            "1 commit to pull: click to pull"
        );
        assert_eq!(RepoMark::Dirty.tip(repo), "Changes not committed");
        let more = RepoLine {
            ahead: 3,
            ..repo.clone()
        };
        assert_eq!(
            RepoMark::Ahead.tip(&more),
            "3 commits to push: click to push"
        );
        // The stash count is a mark, after `no upstream`.
        let stashed = view
            .lines
            .iter()
            .position(|line| matches!(line, TreeLine::Repo(repo) if repo.stashes > 0));
        let stashed = stashed.expect("a repository with stashes");
        assert!(marks.contains(&(stashed, RepoMark::Stashes)), "{marks:?}");
        let note = |repo: &RepoLine| {
            let width = |text: &str, _: f32| text.chars().count() as f32 * 6.0;
            let spans = line_spans(repo, (0.0, 1000.0), 8.0, 1.0, &width);
            let mark = spans.marks.iter().find(|m| m.mark == RepoMark::Stashes);
            (spans.no_upstream, mark.map(|m| (m.x, m.end)))
        };
        let lone = RepoLine {
            no_upstream: true,
            stashes: 2,
            ..repo.clone()
        };
        let (upstream, stash) = note(&lone);
        let (ux, (sx, send)) = (upstream.unwrap(), stash.unwrap());
        assert!(ux + 11.0 * 6.0 < sx, "{ux} {sx}");
        assert_eq!(send - sx, 9.0 * 6.0);
        assert_eq!(note(&RepoLine { stashes: 0, ..lone }).1, None);
    }

    #[test]
    fn the_branch_menu_takes_every_click() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            let view = with_menu();
            let l = repos_layout(&view, size);
            let (menu, rows) = l.branch_menu.clone().expect("a menu");
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(repos_hit(&l, centre(*row)), Some(ReposHit::BranchRow(i)));
            }
            // A heading over each group, which picks nothing.
            let first = l.branch_headings.first().expect("a heading");
            assert_eq!(*first, (0, first.1));
            assert!(first.1.loc.y + first.1.size.h <= rows[0].loc.y);
            for (_, r) in &l.branch_headings {
                assert_eq!(repos_hit(&l, centre(*r)), Some(ReposHit::BranchMenu));
            }
            if rows.len() == 4 {
                let (_, remote) = l.branch_headings[1];
                assert!(remote.loc.y >= rows[2].loc.y + rows[2].size.h);
                assert!(remote.loc.y + remote.size.h <= rows[3].loc.y);
            }
            // Rows with a local branch have a delete button, in them.
            let deletes: Vec<usize> = l.branch_deletes.iter().map(|(i, _)| *i).collect();
            let shown: Vec<usize> = [1, 2].into_iter().filter(|&i| i < rows.len()).collect();
            assert_eq!(deletes, shown);
            for &(i, r) in &l.branch_deletes {
                assert!(within(rows[i], r));
                assert_eq!(repos_hit(&l, centre(r)), Some(ReposHit::BranchDelete(i)));
            }
            // Its search at the top has the keyboard.
            let search = l.branch_search.expect("a search");
            assert!(within(menu, search) && search.loc.y + search.size.h <= rows[0].loc.y);
            assert_eq!(repos_hit(&l, centre(search)), Some(ReposHit::BranchMenu));
            assert_eq!(repos_field(&view, size, 1.0).area, search);
            let closed = ReposView {
                branch: full().branch,
                ..view.clone()
            };
            assert_eq!(repos_field(&closed, size, 1.0).area, l.field);
            let pad = (menu.loc.x as f64 + 1.0, menu.loc.y as f64 + 1.0);
            assert_eq!(repos_hit(&l, pad.into()), Some(ReposHit::BranchMenu));
            assert_eq!(
                repos_hit(&l, centre(l.branch)),
                Some(ReposHit::BranchButton)
            );
            // The rest is the panel, which a click closes the menu on.
            let mut others = vec![l.field, l.detail, l.header];
            others.extend(l.lines.iter().map(|(line, _, _)| *line));
            others.extend(l.buttons.iter().map(|(_, r)| *r));
            for r in others {
                if !menu.to_f64().contains(centre(r)) {
                    assert_eq!(repos_hit(&l, centre(r)), Some(ReposHit::Panel), "{r:?}");
                }
            }
            assert_eq!(repos_hit(&l, (1.0, 1.0).into()), None);
            // The row creating the branch typed.
            let l = repos_layout(&with_create_menu(), size);
            let create = l.branch_create.expect("a row to create");
            assert_eq!(repos_hit(&l, centre(create)), Some(ReposHit::BranchCreate));
        }
    }

    #[test]
    fn scrolled_lists_move_up_and_show_their_last_line_in_part() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let (row_h, line_h) = (repos_row(DENSITY), repos_detail_line(DENSITY));
        let fits = repos_layout(&full(), size);
        // As much as fits, then scrolled half a line with one more.
        let mut whole = full();
        whole.lines.truncate(fits.lines.len());
        whole.detail.rows.truncate(fits.detail_rows.len());
        let mut scrolled = whole.clone();
        scrolled.part = 0.5;
        scrolled.lines.push(whole.lines[1].clone());
        scrolled.detail.part = 0.5;
        scrolled.detail.rows.push(whole.detail.rows[1].clone());
        let (a, b) = (repos_layout(&whole, size), repos_layout(&scrolled, size));
        let hit = |p: (i32, i32)| repos_hit(&b, (p.0 as f64, p.1 as f64).into());
        let bottom = |r: Rect| r.loc.y + r.size.h;

        // The tree: as tall as without its last line, moved up half the
        // first, the last shown in part.
        assert_eq!(a.tree_list, b.tree_list);
        assert_eq!(b.lines.len(), a.lines.len() + 1);
        assert_eq!(b.lines[0].0.loc.y, b.tree_list.loc.y - row_h / 2);
        let last = b.lines.last().unwrap().0;
        assert!(last.loc.y < bottom(b.tree_list) && bottom(last) > bottom(b.tree_list));
        // Out of the tree's list, the lines are not hit.
        let line = b.lines[0].0;
        let x = line.loc.x + line.size.w / 2;
        assert_ne!(hit((x, b.tree_list.loc.y - 2)), Some(ReposHit::Line(0)));
        assert_eq!(hit((x, b.tree_list.loc.y + 2)), Some(ReposHit::Line(0)));
        let n = b.lines.len() - 1;
        assert_eq!(hit((x, bottom(b.tree_list) - 2)), Some(ReposHit::Line(n)));
        assert_ne!(hit((x, bottom(b.tree_list) + 2)), Some(ReposHit::Line(n)));

        // The detail: its pinned header stays, the rows under it move up
        // half the first, under the header; the last is shown in part.
        assert!(scrolled.detail.sticky);
        assert_eq!(a.detail_list, b.detail_list);
        assert_eq!(a.detail_rows[0], b.detail_rows[0]);
        assert_eq!(b.detail_list.loc.y, bottom(b.detail_rows[0]));
        assert_eq!(b.detail_rows[1].loc.y, b.detail_list.loc.y - line_h / 2);
        let last = *b.detail_rows.last().unwrap();
        assert!(last.loc.y < bottom(b.detail_list) && bottom(last) > bottom(b.detail_list));
        // Over the pinned header, the row under it is not hit.
        let x = b.detail_list.loc.x + b.detail_list.size.w / 2;
        assert_eq!(
            hit((x, b.detail_list.loc.y - 2)),
            Some(ReposHit::DiffRow(0))
        );
        assert_eq!(
            hit((x, b.detail_list.loc.y + 2)),
            Some(ReposHit::DiffRow(1))
        );
        let n = b.detail_rows.len() - 1;
        assert_eq!(
            hit((x, bottom(b.detail_list) - 2)),
            Some(ReposHit::DiffRow(n))
        );
        assert_eq!(hit((x, bottom(b.detail_list) + 2)), Some(ReposHit::Detail));

        // The branch menu: as tall as without its last line, its lines
        // moved up half the first, the last shown in part.
        let whole = with_menu();
        let mut scrolled = with_menu();
        if let Some(menu) = &mut scrolled.branch.menu {
            menu.part = 0.5;
            menu.rows.push("release/2.2".into());
        }
        let (a, b) = (repos_layout(&whole, size), repos_layout(&scrolled, size));
        let hit = |p: (i32, i32)| repos_hit(&b, (p.0 as f64, p.1 as f64).into());
        let (card_a, _) = a.branch_menu.unwrap();
        let (card_b, rows) = b.branch_menu.clone().unwrap();
        let list = b.branch_list.unwrap();
        assert_eq!((card_a, a.branch_list), (card_b, Some(list)));
        assert_eq!(b.branch_headings[0].1.loc.y, list.loc.y - row_h / 2);
        let last = *rows.last().unwrap();
        assert!(last.loc.y < bottom(list) && bottom(last) > bottom(list));
        let x = list.loc.x + 20;
        let n = rows.len() - 1;
        assert_eq!(hit((x, bottom(list) - 2)), Some(ReposHit::BranchRow(n)));
        assert_eq!(hit((x, bottom(list) + 2)), Some(ReposHit::BranchMenu));
    }

    #[test]
    fn the_branch_menu_shows_what_fits() {
        let mut view = with_menu();
        if let Some(menu) = &mut view.branch.menu {
            menu.rows = (0..BRANCH_MENU_ROWS)
                .map(|i| format!("feature/a-rather-long-branch-name-{i}"))
                .collect();
        }
        for (w, h) in SIZES.into_iter().chain([(640, 480)]) {
            let size: Size<i32, Logical> = (w, h).into();
            let l = repos_layout(&view, size);
            let (menu, rows) = l.branch_menu.expect("a menu");
            assert!(menu.loc.y + menu.size.h <= l.field.loc.y, "{size:?}");
            // As many as the tree's page less the search, whatever the size.
            let page = (repos_page(size, DENSITY).0 - 1).min(BRANCH_MENU_ROWS);
            let lines = rows.len() + l.branch_headings.len();
            assert!(lines >= page, "{size:?}: {lines} < {page}");
            assert!(menu.size.w <= BRANCH_MENU_MAX.max(l.branch.size.w));
        }
    }

    #[test]
    fn a_sheet_takes_every_click() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            let view = with_sheet();
            let l = repos_layout(&view, size);
            let sheet = l.sheet.expect("a sheet");
            // It lies over the tree and the detail, clear of the bottom.
            assert!(sheet.loc.y >= l.tree.loc.y);
            assert!(sheet.loc.y + sheet.size.h <= l.field.loc.y);
            let mine = view.sheet.as_ref().unwrap().buttons.clone();
            let on_sheet: Vec<_> = l
                .buttons
                .iter()
                .filter(|(_, r)| inside(sheet, *r))
                .collect();
            assert_eq!(
                on_sheet.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
                mine,
                "{size:?}"
            );
            for (b, r) in &on_sheet {
                assert_eq!(repos_hit(&l, centre(*r)), Some(ReposHit::Button(*b)));
            }
            let mut others = vec![l.field, l.detail, l.header];
            others.extend(l.lines.iter().map(|(line, _, _)| *line));
            others.extend(l.detail_tabs.iter().map(|(_, r)| *r));
            others.extend(
                l.buttons
                    .iter()
                    .filter(|(_, r)| !inside(sheet, *r))
                    .map(|(_, r)| *r),
            );
            for r in others {
                let on_button = on_sheet.iter().any(|(_, b)| b.to_f64().contains(centre(r)));
                if !on_button {
                    assert_eq!(repos_hit(&l, centre(r)), Some(ReposHit::Panel), "{r:?}");
                }
            }
            assert_eq!(repos_hit(&l, (1.0, 1.0).into()), None);
        }
    }

    #[test]
    fn the_page_fits_whatever_shows_and_stays_put() {
        for (w, h) in SIZES.into_iter().chain([(640, 480), (3840, 2160)]) {
            let size: Size<i32, Logical> = (w, h).into();
            let (tree_rows, detail_rows) = repos_page(size, DENSITY);
            assert!(tree_rows >= 1 && detail_rows >= 1);
            if h >= 600 {
                assert!(tree_rows >= 8, "{size:?}: {tree_rows}");
                assert!(detail_rows >= 8, "{size:?}: {detail_rows}");
            }
            assert_eq!(repos_page(size, DENSITY), (tree_rows, detail_rows));
            // Compact rows are shorter: more of them fit.
            let compact = repos_page(size, Density::Compact);
            assert!(compact.0 >= tree_rows && compact.1 >= detail_rows);
            // A page of lines fits with or without the error and result.
            let filled = |v: ReposView| ReposView {
                lines: (0..tree_rows)
                    .map(|i| file('M', &format!("src/{i}.rs"), Check::Off))
                    .collect(),
                ..v
            };
            for view in views().into_iter().map(filled) {
                let parts = parts(&view, size);
                if h >= 600 {
                    assert_eq!(parts.layout.lines.len(), tree_rows, "{size:?}");
                    assert!(parts.body.size.h >= detail_rows as i32 * repos_detail_line(DENSITY));
                }
            }
        }
    }

    #[test]
    fn the_field_spot_is_the_field() {
        let view = full();
        let size = (1280, 800).into();
        let spot = repos_field(&view, size, 2.0);
        assert_eq!(spot.area, repos_layout(&view, size).field);
        assert_eq!(spot.line, view.field);
        assert!(spot.text.0 < spot.text.1);
    }

    #[test]
    fn what_the_ui_font_lacks_is_drawn_in_the_monospace_one() {
        let painter = Painter::new();
        if painter.font.is_none() || painter.has_glyph('→') {
            return;
        }
        let runs = painter.runs("a.rs → b.rs");
        assert_eq!(runs, vec![("a.rs ", false), ("→", true), (" b.rs", false)]);
        assert_eq!(painter.runs("plain"), vec![("plain", false)]);
    }

    /// The tree's lines among `parts`: those cut to its list.
    fn tree_parts(parts: &[crate::chrome::Part], layout: &ReposLayout) -> Vec<f64> {
        parts
            .iter()
            .filter(|p| p.clip == Some(layout.tree_list))
            .map(|p| p.loc.y)
            .collect()
    }

    #[test]
    fn scrolling_the_tree_by_part_of_a_line_draws_nothing_again() {
        let painter = Painter::new();
        let size: Size<i32, Logical> = (1280, 800).into();
        let row_h = repos_row(DENSITY) as f64;
        // Shown a second after it opened: risen and faded in.
        let opened = std::time::Instant::now();
        let now = opened + std::time::Duration::from_secs(1);
        // More lines than fit, scrolled part of a line: one more shown in
        // part under those that fit.
        let mut view = ReposView {
            hover: None,
            part: 0.2,
            ..full()
        };
        view.lines = view.lines.iter().cycle().take(60).cloned().collect();
        let fits = repos_page(size, DENSITY).0;
        view.lines.truncate(fits + 1);
        let mut layers = ReposLayers::default();
        layers.parts(Some(view.clone()), size, Theme::Dark, 1.0, opened, &painter);
        let a = layers.parts(Some(view.clone()), size, Theme::Dark, 1.0, now, &painter);
        let (frames, rows) = (layers.frames, layers.tree.drawn());
        // Alike lines share a buffer.
        assert_eq!(frames, 1);
        assert!(rows > 1 && rows <= fits + 1);
        // Further by part of a line, and pulled past the start: the same
        // buffers, moved.
        for (part, over) in [(0.7, 0.0), (0.0, 12.5)] {
            let moved = ReposView {
                part,
                over,
                ..view.clone()
            };
            let layout = repos_layout(&moved, size);
            let b = layers.parts(Some(moved.clone()), size, Theme::Dark, 1.0, now, &painter);
            assert_eq!((layers.frames, layers.tree.drawn()), (frames, rows));
            let (ys_a, ys_b) = (tree_parts(&a, &layout), tree_parts(&b, &layout));
            assert!(ys_b.len() >= fits);
            for (y_a, y_b) in ys_a.iter().zip(&ys_b) {
                let by = (part - 0.2) * row_h - over;
                assert!((y_a - y_b - by).abs() < 1e-9, "{y_a} {y_b}");
            }
            // Hit where they are drawn, to the logical pixel.
            for ((rect, ..), y) in layout.lines.iter().zip(&ys_b) {
                assert_eq!(rect.loc.y, y.round() as i32);
            }
        }
        // The lines come with the frame, front to back.
        assert!(a.len() > fits + 1);
        assert_eq!(a.last().map(|p| p.clip), Some(None));
    }

    #[test]
    fn it_paints() {
        let painter = Painter::new();
        let disabled_hover = ReposView {
            hover: Some(ReposHit::Button(ReposButton::ForcePush)),
            field_focused: false,
            ..full()
        };
        let compact = ReposView {
            density: Density::Compact,
            hover: Some(ReposHit::Chip(4)),
            ..full()
        };
        let long_sheet = {
            let mut v = with_sheet();
            if let Some(sheet) = &mut v.sheet {
                sheet.lines = (0..60)
                    .map(|i| (format!("repo-{i}"), "clean".into(), Tone::Success))
                    .collect();
            }
            v
        };
        let views = [
            empty(),
            full(),
            with_sheet(),
            disabled_hover,
            compact,
            long_sheet,
            with_menu(),
            with_empty_menu(),
            with_create_menu(),
            with_bad_name_menu(),
        ];
        for view in views {
            for (w, h) in SIZES {
                for (theme, scale) in [(Theme::Dark, 1.0), (Theme::Light, 2.0)] {
                    let _ = painter.repos(&view, (w, h).into(), scale, theme.palette());
                }
            }
        }
    }

    /// With `GERDR_DUMP_DIR` set, write the panel there as images to look
    /// at (both themes, scale 1 and 2, with and without a sheet).
    #[test]
    fn dump_repos() {
        let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let painter = Painter::new();
        let size: Size<i32, Logical> = (1280, 800).into();
        let disabled_hover = ReposView {
            hover: Some(ReposHit::Button(ReposButton::ForcePush)),
            field_focused: false,
            selected: Some(4),
            ..full()
        };
        let cases = [
            ("full", full()),
            ("sheet", with_sheet()),
            ("empty", empty()),
            ("hover", disabled_hover),
            ("menu", with_menu()),
            ("menu_empty", with_empty_menu()),
            ("menu_create", with_create_menu()),
            ("menu_bad_name", with_bad_name_menu()),
            (
                "menu_delete",
                ReposView {
                    hover: Some(ReposHit::BranchDelete(1)),
                    ..with_menu()
                },
            ),
            (
                "mark",
                ReposView {
                    hover: Some(ReposHit::Mark(6, RepoMark::Behind)),
                    ..full()
                },
            ),
            (
                "chip",
                ReposView {
                    hover: Some(ReposHit::Chip(4)),
                    ..full()
                },
            ),
            (
                "confirm",
                ReposView {
                    sheet: Some(SheetView {
                        title: "Force push in 2 repositories?".into(),
                        lines: vec![
                            (
                                "gerdr".into(),
                                "overwrite origin/feat/x with feat/x, with lease".into(),
                                Tone::Warning,
                            ),
                            (
                                "infra".into(),
                                "overwrite origin/feat/x with feat/x, with lease".into(),
                                Tone::Warning,
                            ),
                        ],
                        buttons: vec![ReposButton::ForcePush, ReposButton::Cancel],
                        default: 0,
                    }),
                    ..full()
                },
            ),
            (
                "help",
                ReposView {
                    sheet: Some(SheetView {
                        title: "Keys in the repositories panel".into(),
                        lines: vec![
                            ("Tab".into(), "the field, or the tree".into(), Tone::Normal),
                            ("Up / Down".into(), "select a line".into(), Tone::Normal),
                            ("Space".into(), "check the line".into(), Tone::Normal),
                            ("Esc".into(), "close".into(), Tone::Normal),
                        ],
                        buttons: vec![ReposButton::Close],
                        default: 0,
                    }),
                    ..full()
                },
            ),
            (
                "compact",
                ReposView {
                    density: Density::Compact,
                    ..full()
                },
            ),
        ];
        for (theme, scale) in [(Theme::Dark, 1.0), (Theme::Light, 1.0), (Theme::Dark, 2.0)] {
            // The narrow one has the footer's "more" button.
            let narrow = (("narrow", full()), Size::from((800, 600)));
            let cases = cases
                .iter()
                .map(|c| (c, size))
                .chain([(&narrow.0, narrow.1)]);
            for ((name, view), size) in cases {
                let panel = repos_layout(view, size).panel;
                let w = ((panel.size.w + 2 * SHADOW) as f64 * scale).round() as usize;
                let mut buffer = painter.repos(view, size, scale, theme.palette());
                let mut out = Vec::new();
                let _ = buffer.render().draw(|mem| {
                    let h = mem.len() / 4 / w;
                    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
                    for px in mem.as_chunks::<4>().0 {
                        let a = px[3] as u32;
                        for c in [px[2], px[1], px[0]] {
                            out.push((c as u32 + 0x80 * (255 - a) / 255) as u8);
                        }
                    }
                    Ok::<_, ()>(Vec::new())
                });
                let file = format!("repos_{name}_{}_{scale}.ppm", theme.name());
                let _ = std::fs::write(dir.join(file), out);
            }
        }
    }
}
