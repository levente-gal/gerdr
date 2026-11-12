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
    BUTTON_RADIUS, ButtonStyle, CAP_PADDING, CAP_RADIUS, CHECKBOX_SIZE, Canvas, DETAIL_FONT_SIZE,
    DOCK_GAP, Density, FIELD_GAP, FONT_SIZE, FieldSpot, GIT_MARK_GAP, GIT_MARK_WIDTH, HELP_MARGIN,
    HELP_PADDING, HELP_RADIUS, HINT_KEY_GAP, KEY_FONT_SIZE, LABEL_FONT_SIZE, LineState,
    MENU_MARK_WIDTH, MENU_RADIUS, PADDING, Painter, Palette, QUIT_BUTTON_GAP, Rect, Rgb, SHADOW,
    SMALL_FONT_SIZE, SearchBar, SearchPart, arrow, band, check, chevron_down, coverage, field_text,
    folder, found_split, found_within, git_branch, icon_sdf, magnifier, measure, measure_bold,
    measure_mono, mix, phys, physical, plus, rect_px, rounded_rect, search_field_text,
    search_spots, segment, trash,
};
use std::ops::Range;

use crate::diff::LineKind;
use crate::repos::Grouping;
use crate::text_field::Line;
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
pub const REPOS_HEADER: i32 = 44;
const HEADER_GAP: i32 = 12;
/// The tree's share of the panel's width unless dragged, the least and
/// most it can be dragged to, the narrowest it gets (unless that is more
/// than half), and how far either side of the line between the tree and
/// the detail a press takes hold of it.
pub const REPOS_TREE_SHARE: f64 = 0.42;
pub const REPOS_TREE_SHARE_MIN: f64 = 0.2;
pub const REPOS_TREE_SHARE_MAX: f64 = 0.8;
/// The file list's share of a feature's code review, whose diff wants the
/// room more than the repositories panel's does.
pub const REVIEW_TREE_SHARE: f64 = 0.25;
/// How wide a feature's code review must be to have its file list beside
/// the diff, logical pixels.
pub const REVIEW_TREE_MIN_PANEL: i32 = 640;
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
/// How much further a line is indented per folder it is in, and the room
/// of a folder's icon.
const REPOS_LEVEL: i32 = 20;
const REPOS_FOLDER: i32 = 20;
/// The branch icon on a repository's header, in the tree and the diff.
const BRANCH_ICON: i32 = 14;
/// The room between the tree's toolbar's groups of buttons.
const REPOS_TOOL_GAP: i32 = 12;
/// The scrollbar at the right of the tree and of the detail: its width
/// (its thumb's least height is every scrollbar's, see `Thumb::of`).
const SCROLLBAR: i32 = 6;
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
/// How strongly a finding's heading band takes its severity's colour.
const COMMENT_HEAD_TINT: f32 = 0.24;
/// Between the parts of a comment's heading.
const COMMENT_HEAD_GAP: i32 = 8;
/// A finding's "AI review" tag's padding.
const COMMENT_TAG_PAD: i32 = 6;
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
/// How far the line and shadow under the pinned header reach down.
const PIN_SHADOW: i32 = 5;
/// A button on a diff row: its padding, the gap between two, and how far
/// in from the row's ends and edges it sits.
const ROW_CHIP_PAD: i32 = 8;
const ROW_CHIP_GAP: i32 = 4;
const ROW_CHIP_INSET: i32 = 2;
/// How strongly an added or removed line is tinted, and its changed
/// words.
const DIFF_TINT: f32 = 0.10;
/// How strongly a repository's header is tinted, in the tree and the
/// diff: it heads what is under it.
const HEADER_TINT: f32 = 0.07;
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
    /// The colour of `summary`: muted, unless it says something settled.
    pub summary_tone: Tone,
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
    /// What the tree's title says in place of its name, and in what tone: how
    /// many of a code review's files are left.
    pub tree_note: Option<(String, Tone)>,
    /// How the tree lists the files: the toolbar over it says.
    pub grouping: Grouping,
    /// The diff alone across the panel, `title` its heading: no tree,
    /// branch picker, tabs, field or footer (a worktree's diff).
    pub diff_only: bool,
    /// With `diff_only`: the tree beside the diff all the same, its lines
    /// and toolbar (a feature's code review).
    pub diff_tree: bool,
    /// With `diff_tree`: its files have no checkboxes (a fix round's diff,
    /// only to read).
    pub unchecked: bool,
    /// The tree's toolbar going to the previous and the next file not
    /// viewed (a feature's code review): whether one is left to go to.
    pub tree_nav: Option<bool>,
    /// Laid flat into a tab rather than floating over the app: no shadow,
    /// border or rounded corners.
    pub flat: bool,
    /// No header row of its own: what it is laid into names it (the page's
    /// bar over Code review on the feature tab).
    pub no_header: bool,
    /// The share of the panel's width under the header the detail takes,
    /// a file's page showing in the rest (see `doc_tab`): 1 for none of
    /// the page, 0 for the page alone.
    pub detail_share: f64,
    /// The switch between what a file's tab shows, at the header's right.
    pub modes: Option<ModeSwitch>,
}

/// A switch of a few choices at the panel's header's right.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModeSwitch {
    pub labels: Vec<String>,
    /// The choice shown, by index into `labels`.
    pub on: usize,
    /// The choices that can't be had now: a tooltip says why.
    pub off: Vec<usize>,
}

impl ReposView {
    /// The panel with a diff alone across it, `title` its heading.
    pub fn diff_only(
        title: String,
        summary: String,
        hover: Option<ReposHit>,
        hints: Vec<Hint>,
        detail: DetailView,
        sheet: Option<SheetView>,
        density: Density,
    ) -> Self {
        ReposView {
            title,
            summary,
            summary_tone: Tone::Muted,
            lines: Vec::new(),
            part: 0.0,
            over: 0.0,
            first: 0,
            total: 0,
            selected: None,
            hover,
            field: Default::default(),
            field_focused: false,
            placeholder: String::new(),
            error: None,
            result: None,
            hints,
            detail,
            sheet,
            empty: String::new(),
            branch: Default::default(),
            density,
            tree_share: 0.0,
            tree_note: None,
            grouping: Default::default(),
            diff_only: true,
            diff_tree: false,
            unchecked: false,
            tree_nav: None,
            flat: false,
            no_header: false,
            detail_share: 1.0,
            modes: None,
        }
    }
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
    /// How far the lines are moved down (up when negative), logical
    /// pixels, while the menu is pulled past an end.
    pub over: f64,
    /// The longest of all its branches, in view or not: the menu is wide
    /// enough for them.
    pub widest: Vec<String>,
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

/// A button of the tree's toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeTool {
    /// Unfold every repository and folder.
    Expand,
    /// Fold every repository and folder.
    Collapse,
    /// List the files flat, or by folder.
    List,
    Folders,
    /// Go to the file not viewed before the one shown, or after it (a
    /// feature's code review).
    PrevUnviewed,
    NextUnviewed,
}

/// A line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeLine {
    Repo(RepoLine),
    Dir(DirLine),
    File(FileLine),
    /// The heading over the repositories with nothing to do, after the
    /// others.
    Clean(CleanLine),
}

/// The heading over the repositories with nothing to do: `Clean · 3`.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanLine {
    /// Its repositories show.
    pub open: bool,
    pub count: usize,
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
    /// How many files it has changed, and the lines added and removed in
    /// them as its diff has it (none while it is read).
    pub files: usize,
    pub added: usize,
    pub removed: usize,
    /// What it is in the middle of, or what went wrong: `MERGING · 2`,
    /// `REBASING 2/5`, `login needed`, `detached`.
    pub chip: Option<Chip>,
    /// What is being done to it, `pushing…`, or a second press's warning.
    pub state: LineState,
}

/// A folder's line, with changed files under it.
#[derive(Clone, Debug, PartialEq)]
pub struct DirLine {
    /// On when all its files are checked, partly when some are.
    pub check: Check,
    /// Its files show.
    pub open: bool,
    /// Its path from its folder's line, `src/chrome` for one holding only
    /// a folder.
    pub name: String,
    /// How many folders it is in.
    pub depth: usize,
}

/// A changed file's line, under its repository.
#[derive(Clone, Debug, PartialEq)]
pub struct FileLine {
    pub check: Check,
    /// `git::Change::code`.
    pub code: char,
    /// From the repository's top folder, or its name under its folder's
    /// line; a rename as `old → new`.
    pub path: String,
    pub conflicted: bool,
    /// How many folders' lines it is under.
    pub depth: usize,
    /// The lines added and removed in it, as its diff has it (none while
    /// it is read).
    pub added: usize,
    pub removed: usize,
    /// It was edited from the diff since the panel opened (`u` undoes
    /// it there).
    pub edited: bool,
    /// How many review comments are on it.
    pub comments: usize,
    /// It is read and done with: it steps back, what is left standing out.
    pub viewed: bool,
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
    /// A medium-severity finding's own colour, apart from `Warning`.
    Medium,
}

/// The last batch's result, between the field and the footer: `Commit and
/// push: 2 done · 1 conflict`, and what can be done about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultBar {
    pub text: String,
    pub tone: Tone,
    /// `Undo`, `RetryFailed`: their keys in their tips, as `without_key`.
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
    /// How it looks as a button of its own (a title button): `Primary` for
    /// the one to press next, else `Plain`. Footer hints ignore it.
    pub style: ButtonStyle,
}

/// Marks the title button to press next `Primary`, the rest `Plain`: what
/// a feature's diff waits for (Make fixes, else Plan fixes once it can be
/// pressed); else the rightmost of what goes on that can be pressed (a
/// file's Approve or Send back, the fix plan's Make fixes, the plan's Write
/// final plan). One filled button, at the right.
pub fn mark_primary(title: &mut [Hint]) {
    let enabled = |b: ReposButton| title.iter().any(|h| h.button == b && h.disabled.is_none());
    let main = if title.iter().any(|h| h.button == ReposButton::MakeFixes) {
        Some(ReposButton::MakeFixes)
    } else if enabled(ReposButton::PlanFixes) {
        Some(ReposButton::PlanFixes)
    } else {
        (title.iter().rev())
            .find(|h| {
                h.disabled.is_none()
                    && matches!(
                        h.button,
                        ReposButton::Approve | ReposButton::SendBack | ReposButton::GoOn
                    )
            })
            .map(|h| h.button)
    };
    for hint in title {
        hint.style = match Some(hint.button) == main && hint.disabled.is_none() {
            true => ButtonStyle::Primary,
            false => ButtonStyle::Plain,
        };
    }
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
    /// of the file the rest start in, pinned at the top over them.
    pub rows: Vec<DetailRow>,
    pub sticky: bool,
    /// Where the pinned header is, logical pixels down from the top of the
    /// rows: 0 pinned there, less while the next file's header pushes it
    /// up, more while the rows are pulled down past the start.
    pub pin: f64,
    /// The pinned header is scrolled past its own place: it casts a
    /// shadow on the rows under it.
    pub stuck: bool,
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
    /// How far the rows are moved down (up when negative), logical
    /// pixels, while the pane is pulled past an end.
    pub over: f64,
    /// Where the rows start among them all, and how many there are: where
    /// the scrollbar stands.
    pub first: usize,
    pub total: usize,
    /// The detail has the keyboard.
    pub focused: bool,
    /// The diff's search, in the title row in place of the title while
    /// open.
    pub search: Option<SearchBar>,
    /// Per row of `rows`, the places the search found on it, by char
    /// index into what it shows, each with whether it is the one gone to.
    /// Empty for none.
    pub found: Vec<Vec<(Range<usize>, bool)>>,
    /// The chars selected in visual mode, per row of `rows` with some.
    pub chosen: Vec<(usize, Range<usize>)>,
    /// The row of `rows` the caret is on and the char of its line.
    pub caret: Option<(usize, usize)>,
    /// The row of `rows` the caret is on and the word at it there, by
    /// char index: what `*` searches for, underlined.
    pub word: Option<(usize, Range<usize>)>,
    /// Where the places found are on the scrollbar: shares of all the
    /// rows from the top, each with whether it is the one gone to.
    pub ticks: Vec<(f64, bool)>,
    /// It shows a file as a page, not as a change: one column of line
    /// numbers, no tint.
    pub plain: bool,
    /// How many digits the line numbers before the diff's lines take: as
    /// many as the widest in view; 0 for no numbers.
    pub gutter: usize,
    /// A row of `rows` (the caret's) whose line is shown from a char on,
    /// to keep the caret in view: the row and how many chars are left
    /// off its start (its ranges already moved by as many).
    pub skipped: Option<(usize, usize)>,
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
    /// A review comment's heading: who wrote it and where it is among
    /// those of its kind (`place`: `finding 2 of 5`). The first of its card.
    CommentHead {
        /// Who wrote it, in a word: "AI review" on a reviewer's (a tag),
        /// "Your comment" or "Your suggestion" on the user's.
        who: &'static str,
        /// What the AI reviewer found; none for the user's own.
        finding: Option<FindingHead>,
        place: String,
        /// It is the user's comment on a finding: inside that card, of this
        /// tone.
        within: Option<Tone>,
    },
    /// A row of a review comment: the last of its rows rounds its card. An
    /// AI review finding's card takes its `tone` (by severity, muted once
    /// dismissed); the user's own have none.
    Comment {
        text: String,
        first: bool,
        last: bool,
        tone: Option<Tone>,
        /// It is of the user's comment on a finding: inside that card, of
        /// this tone. `last` is of them both.
        within: Option<Tone>,
    },
    /// A row of the comment being written: with the caret on it, its byte
    /// in `text`; the selection on it, by bytes in `text`.
    Editor {
        text: String,
        caret: Option<usize>,
        selection: Range<usize>,
        first: bool,
        last: bool,
        /// It writes the user's comment on a finding: inside that card, of
        /// this tone.
        within: Option<Tone>,
    },
}

/// What an AI review finding's heading says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingHead {
    /// `high`.
    pub severity: String,
    /// `to fix`; none while undecided.
    pub state: Option<String>,
    pub tone: Tone,
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
    /// Its checkbox: none where files aren't checked.
    pub check: Option<Check>,
    pub folded: bool,
    pub added: usize,
    pub removed: usize,
    /// It is the tree's selected line.
    pub selected: bool,
    /// Said muted before its counts: `changed since viewed`.
    pub note: Option<String>,
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
    /// Cancel (or what backs out) on the left, what goes ahead on the
    /// right.
    pub buttons: Vec<ReposButton>,
    /// The button Enter presses, by index into `buttons`: ringed.
    pub default: usize,
    /// The button on the right loses what can't be got back: it is in
    /// the error colour, not the accent.
    pub danger: bool,
}

/// Everything the panel can be asked to do with a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReposButton {
    Commit,
    CommitPush,
    /// The last commit amended with the checked files, and the message if
    /// one is typed.
    Amend,
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
    /// Ask for the name and email git wants for commits.
    SetIdentity,
    OpenWorkspace,
    /// The selected repository's stashes.
    Stash,
    StashPop,
    StashDrop,
    /// The commit picked in the Log tab, its diff shown.
    ShowCommit,
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
    /// The cursor's line, or what is selected, to the clipboard.
    CopyLines,
    ClearReview,
    /// The review, sent to the agent running in its workspace.
    SendReview,
    /// A comment on the diff lines picked; the comment being written
    /// kept, dropped, or (one written before) deleted.
    Comment,
    /// Write a comment on the whole of the file under review.
    CommentFile,
    /// Ask for the fix of the AI review finding the cursor is on.
    EditFix,
    SaveComment,
    DiscardComment,
    DeleteComment,
    /// The change at the diff's cursor, or a hunk, put back as it was.
    Restore,
    /// Read the repositories' status and diffs again.
    Refresh,
    /// The branch menu, to pick a local branch to delete.
    DeleteBranchMenu,
    /// Stop the batches running.
    Stop,
    /// Throw away the selected file's or folder's changes (asked first).
    DiscardFile,
    /// Delete the review comments on lines no longer in the diff (asked
    /// first).
    DropOutdated,
    /// A feature stage's file under review (`worktree_diff::DocOf`):
    /// approved as it is, or sent back to its agent with the comments.
    Approve,
    SendBack,
    /// That file approved: reopened, or on to the stage the feature is at.
    Reopen,
    GoOn,
    /// An AI review finding in a feature's code review
    /// (`feature_diff`): to fix, dismissed (a reason asked first), or
    /// back to undecided.
    FixFinding,
    DismissFinding,
    ReopenFinding,
    /// A feature's code review: mark the file at the cursor viewed, or not.
    MarkViewed,
    /// A feature's code review: go to the next file not viewed.
    NextUnviewed,
    /// A feature's diff (`feature_diff::FeatureOf::buttons`): a comment on no
    /// line or file, every undecided finding marked to fix, a fix round
    /// started on what is to fix, and the plan it wrote read.
    AddComment,
    FixAllFindings,
    PlanFixes,
    MakeFixes,
    ReviewFixPlan,
    /// Go to the next review comment or finding in the diff, or the one
    /// before.
    NextComment,
    PrevComment,
    /// A feature's code review after fixes: all the changes since its base,
    /// or back to only those since the last review.
    AllChanges,
    SinceReview,
}

impl ReposButton {
    /// What it says.
    pub fn label(self) -> &'static str {
        match self {
            ReposButton::Commit => "Commit",
            ReposButton::CommitPush => "Commit and push",
            ReposButton::Amend => "Amend last commit",
            ReposButton::Push => "Push",
            ReposButton::ForcePush => "Force push",
            ReposButton::Fetch => "Fetch",
            ReposButton::Pull => "Pull",
            ReposButton::Merge => "Merge default branch in",
            ReposButton::Rebase => "Rebase onto default branch",
            ReposButton::MergeFrom => "Merge from…",
            ReposButton::RebaseFrom => "Rebase onto…",
            ReposButton::OpenInZed => "Open in Zed",
            ReposButton::MarkResolved => "Resolved",
            ReposButton::Continue => "Continue",
            ReposButton::Abort => "Abort",
            ReposButton::Undo => "Undo",
            ReposButton::RetryFailed => "Retry failed",
            ReposButton::RunInTerminal => "Run in terminal",
            ReposButton::SetIdentity => "Set name and email\u{2026}",
            ReposButton::OpenWorkspace => "Open workspace",
            ReposButton::Stash => "Stash",
            ReposButton::StashPop => "Pop stash",
            ReposButton::StashDrop => "Drop stash",
            ReposButton::ShowCommit => "Show commit",
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
            ReposButton::CopyLines => "Copy",
            ReposButton::SendReview => "Send to agent",
            ReposButton::ClearReview => "Clear review",
            ReposButton::Comment => "Comment",
            ReposButton::CommentFile => "Comment on file",
            ReposButton::SaveComment => "Save",
            ReposButton::DiscardComment => "Discard",
            ReposButton::DeleteComment => "Delete",
            ReposButton::EditFix => "Edit fix\u{2026}",
            ReposButton::Restore => "Restore",
            ReposButton::Refresh => "Refresh",
            ReposButton::DeleteBranchMenu => "Delete a branch…",
            ReposButton::Stop => "Stop",
            ReposButton::DiscardFile => "Discard changes",
            ReposButton::DropOutdated => "Drop outdated",
            ReposButton::Approve => "Approve",
            ReposButton::SendBack => "Send back",
            ReposButton::Reopen => "Reopen",
            ReposButton::GoOn => "Go to current stage",
            ReposButton::FixFinding => "Fix",
            ReposButton::DismissFinding => "Dismiss…",
            ReposButton::ReopenFinding => "Reopen",
            ReposButton::MarkViewed => "Viewed",
            ReposButton::NextUnviewed => "Next unviewed",
            ReposButton::AddComment => "Add comment\u{2026}",
            ReposButton::FixAllFindings => "Fix all open",
            ReposButton::PlanFixes => "Plan fixes",
            ReposButton::MakeFixes => "Make fixes",
            ReposButton::ReviewFixPlan => "Review fix plan",
            ReposButton::NextComment => "Next comment",
            ReposButton::PrevComment => "Previous comment",
            ReposButton::AllChanges => "All changes",
            ReposButton::SinceReview => "Since last review",
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
    /// A repository's or a folder's line's expander.
    Expander(usize),
    /// A repository line's chip: it says what it means.
    Chip(usize),
    Field,
    /// A footer action, a result bar button or a sheet button.
    Button(ReposButton),
    /// A choice of the switch at the header's right.
    Mode(usize),
    DetailTab(DetailTab),
    /// The detail pane (for the wheel).
    Detail,
    /// A row of the detail, by index into `DetailView::rows`.
    DiffRow(usize),
    /// A file header's checkbox, and its fold chevron.
    DiffCheck(usize),
    DiffFold(usize),
    /// A row's Restore button: at the cursor, or on a hunk's header.
    DiffRestore(usize),
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
    /// A button of the tree's toolbar.
    Tool(TreeTool),
    /// A part of the diff's search.
    Search(SearchPart),
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
    /// Where its counts start, at the right.
    counts: Option<f32>,
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

/// What a diff's counts say, in their tones: `lead` (`3 files`), the
/// lines added and those removed, each left out when there is none.
fn count_parts(lead: Option<String>, (added, removed): (usize, usize)) -> Vec<(String, Tone)> {
    let mut parts = Vec::new();
    parts.extend(lead.map(|lead| (lead, Tone::Muted)));
    if added > 0 {
        parts.push((format!("+{added}"), Tone::Success));
    }
    if removed > 0 {
        parts.push((format!("−{removed}"), Tone::Error));
    }
    parts
}

/// The gap between two of a diff's counts, physical pixels.
fn count_gap(s: f32) -> f32 {
    (6.0 * s).round()
}

/// A repository line's counts: its files while folded (they show else),
/// and its lines added and removed.
fn repo_counts(repo: &RepoLine) -> Vec<(String, Tone)> {
    let files = (repo.expanded == Some(false)).then(|| files_text(repo.files));
    count_parts(files, (repo.added, repo.removed))
}

/// `1 file`, `3 files`.
fn files_text(n: usize) -> String {
    match n {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    }
}

/// How wide a text is at a size, physical pixels: in the UI font, or the
/// bold one.
type Width<'a> = &'a dyn Fn(&str, f32) -> f32;

/// The room of the branch icon before a repository's branch, with the gap
/// after it, physical pixels.
fn branch_icon(gap: f32, s: f32) -> f32 {
    (BRANCH_ICON as f32 * s).round() + (gap / 2.0).round()
}

fn line_spans(
    repo: &RepoLine,
    (x, right): (f32, f32),
    gap: f32,
    s: f32,
    (width, bold): (Width, Width),
) -> LineSpans {
    let mut end = right;
    let parts = repo_counts(repo);
    let counts = (!parts.is_empty()).then(|| {
        let w: f32 = parts.iter().map(|(t, _)| width(t, SMALL_FONT_SIZE)).sum();
        let w = w + count_gap(s) * (parts.len() - 1) as f32;
        let cx = (right - w).floor().max(x);
        end = cx - gap;
        cx
    });
    let chip = repo.chip.as_ref().map(|chip| {
        let pad = (CHIP_PAD as f32 * s).round();
        let cw = (width(&chip.text, DETAIL_FONT_SIZE) + 2.0 * pad).round();
        let cx = (end - cw).max(x);
        end = cx - gap;
        (cx, cw)
    });
    let mut pen = (x + bold(&repo.label, FONT_SIZE)).min(end).max(x);
    let branch = (pen + gap + branch_icon(gap, s) < end).then(|| {
        let bx = pen + gap;
        let text = repo.branch.as_deref().unwrap_or("detached");
        pen = (bx + branch_icon(gap, s) + width(text, SMALL_FONT_SIZE)).min(end);
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
        counts,
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
    /// The buttons of the tree's toolbar, over its lines.
    pub tree_tools: Vec<(TreeTool, Rect)>,
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
    /// The diff's search in the title row: the button opening it, or its
    /// field and buttons; and where it says how many it found.
    pub detail_search: Vec<(SearchPart, Rect)>,
    pub detail_search_count: Option<Rect>,
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
    /// The switch at the header's right: its track, and a rect per choice.
    pub modes: Option<(Rect, Vec<Rect>)>,
    /// The part left open for a file's page, which shows through.
    pub page: Option<Rect>,
    /// The gap between the detail and that page, which drags their split.
    pub page_gap: Option<Rect>,
}

/// Inside the panel's parts: what the painter needs besides `ReposLayout`.
struct Parts {
    layout: ReposLayout,
    /// The line under the field saying what went wrong.
    error: Option<Rect>,
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

/// The panel as a popup on an output of `size`: centred, a margin in from
/// its edges, at most `REPOS_MAX_WIDTH` by `REPOS_MAX_HEIGHT`.
pub fn repos_panel_rect(size: Size<i32, Logical>) -> Rect {
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

/// `hint` as a button outside the footer draws it: its label alone. Only
/// the footer shows keys; the others' are in their tips.
fn without_key(hint: &Hint) -> Hint {
    Hint {
        key: String::new(),
        ..hint.clone()
    }
}

/// How wide a button on a diff row is: its label with room either side.
fn row_chip_width(hint: &Hint) -> i32 {
    2 * ROW_CHIP_PAD + text_extent(&hint.label, SMALL_FONT_SIZE, SMALL_CHAR)
}

/// Where the buttons `widths` wide go on a diff row in `row`: in order,
/// right-aligned on it.
fn chip_rects(row: Rect, widths: impl DoubleEndedIterator<Item = i32>) -> Vec<Rect> {
    let mut end = row.loc.x + row.size.w - ROW_CHIP_INSET;
    let mut rects: Vec<Rect> = widths
        .rev()
        .map(|w| {
            end -= w;
            let rect = Rectangle::new(
                (end, row.loc.y + ROW_CHIP_INSET).into(),
                (w, (row.size.h - 2 * ROW_CHIP_INSET).max(1)).into(),
            );
            end -= ROW_CHIP_GAP;
            rect
        })
        .collect();
    rects.reverse();
    rects
}

/// `chip_rects` of diff row `row`, laid in `rect`: on a line of the diff
/// (numbers of `gutter`'s digits and `plain` before it), right after its
/// text when they fit there, so a line's actions sit beside what they act
/// on, not across a wide pane.
fn line_chip_rects(
    rect: Rect,
    row: Option<&DetailRow>,
    (gutter, plain): (usize, bool),
    widths: impl DoubleEndedIterator<Item = i32>,
) -> Vec<Rect> {
    let mut rects = chip_rects(rect, widths);
    if let (Some(DetailRow::Line(line)), Some(first)) = (row, rects.first()) {
        let advance = measure_mono("0", REPOS_MONO_SIZE)
            .filter(|a| *a > 0.0)
            .unwrap_or(REPOS_MONO_SIZE * 0.6);
        let chars: usize = line.text.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
        let text_end = rect.loc.x
            + DIFF_INSET
            + gutter_width(gutter, plain)
            + (chars as f32 * advance).ceil() as i32
            + 3 * ROW_CHIP_GAP;
        let by = text_end - first.loc.x;
        if by < 0 {
            for r in &mut rects {
                r.loc.x += by;
            }
        }
    }
    rects
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
/// the tree and the detail to `x` (output coordinates), in `panel`.
pub fn repos_share_at(x: f64, panel: Rect) -> f64 {
    let share = (x - panel.loc.x as f64) / panel.size.w.max(1) as f64;
    share.clamp(REPOS_TREE_SHARE_MIN, REPOS_TREE_SHARE_MAX)
}

/// How wide the branch picker's button is for `label`.
fn picker_width(label: &str) -> i32 {
    let text = text_extent(label, FONT_SIZE, BUTTON_CHAR);
    (2 * PICKER_PAD + PICKER_ICON + 2 * PICKER_GAP + text + PICKER_CHEVRON).max(PICKER_MIN)
}

/// How many tree lines and detail lines `panel` shows at once: with the error line, the result bar and the footer's
/// rows all there, so that the page doesn't change as they come and go.
pub fn repos_page(panel: Rect, density: Density) -> (usize, usize) {
    let bottom = REPOS_FIELD_ROW + ERROR_HEIGHT + REPOS_RESULT + footer_h(REPOS_FOOTER_ROWS);
    let middle = panel.size.h - REPOS_HEADER - bottom;
    let tree = (middle - REPOS_DETAIL_TITLE - 2 * REPOS_PAD) / repos_row(density);
    let detail = (middle - REPOS_DETAIL_TITLE - 2 * REPOS_PAD) / repos_detail_line(density);
    (tree.max(1) as usize, detail.max(1) as usize)
}

/// How many lines a sheet over `panel` shows at once, with the error line, the result bar and the footer all there
/// (as `repos_page`); with the diff alone across the panel when
/// `diff_only`.
pub fn repos_sheet_rows(panel: Rect, diff_only: bool) -> usize {
    let bottom = match diff_only {
        true => footer_h(REPOS_FOOTER_ROWS),
        false => REPOS_FIELD_ROW + ERROR_HEIGHT + REPOS_RESULT + footer_h(REPOS_FOOTER_ROWS),
    };
    let middle = panel.size.h - REPOS_HEADER - bottom;
    let fixed = 2 * SHEET_PAD + SHEET_TITLE + 2 * SHEET_GAP + SHEET_BUTTON;
    ((middle - 2 * REPOS_PAD - fixed) / SHEET_LINE).max(1) as usize
}

/// How many characters of the monospace font a review comment's row of
/// the diff holds, in `panel` with the tree's share `share`.
pub fn repos_detail_cols(panel: Rect, share: f64) -> usize {
    let w = panel.size.w;
    detail_cols((w - tree_width(w, share)).max(0))
}

/// `area` shared across: `share` of its width on the left, then a gap, then
/// the rest. All of it on the left from 1 up, all on the right from 0 down,
/// without a gap: the other two are then empty, at the edge.
pub fn split_rects(area: Rect, share: f64) -> (Rect, Rect, Rect) {
    let (x, y) = (area.loc.x, area.loc.y);
    let (w, h) = (area.size.w, area.size.h);
    let empty = |at: i32| Rectangle::new((at, y).into(), (0, h).into());
    if share >= 1.0 {
        return (area, empty(x + w), empty(x + w));
    }
    if share <= 0.0 {
        return (empty(x), empty(x), area);
    }
    let left = (w as f64 * share).round() as i32;
    let right = (w - left - DOCK_GAP).max(0);
    (
        Rectangle::new((x, y).into(), (left, h).into()),
        Rectangle::new((x + left, y).into(), (DOCK_GAP, h).into()),
        Rectangle::new((x + w - right, y).into(), (right, h).into()),
    )
}

/// What of `outer` is around `hole`, inside it: up to four rects.
fn around(outer: Rect, hole: Rect) -> Vec<Rect> {
    let (x, y, w, h) = (outer.loc.x, outer.loc.y, outer.size.w, outer.size.h);
    let (hx, hy) = (hole.loc.x, hole.loc.y);
    let (hend, vend) = (hx + hole.size.w, hy + hole.size.h);
    let rect = |x: i32, y: i32, w: i32, h: i32| Rectangle::new((x, y).into(), (w, h).into());
    [
        rect(x, y, w, hy - y),
        rect(x, vend, w, y + h - vend),
        rect(x, hy, hx - x, hole.size.h),
        rect(hend, hy, x + w - hend, hole.size.h),
    ]
    .into_iter()
    .filter(|r| r.size.w > 0 && r.size.h > 0)
    .collect()
}

/// How many diff rows the panel shows at once with the diff alone across
/// it (`ReposView::diff_only`), in `panel`: with the footer's rows all
/// there, as `repos_page`.
pub fn diff_only_page(panel: Rect, density: Density) -> usize {
    let middle = panel.size.h - REPOS_HEADER - footer_h(REPOS_FOOTER_ROWS);
    let detail = (middle - REPOS_DETAIL_TITLE - 2 * REPOS_PAD) / repos_detail_line(density);
    detail.max(1) as usize
}

/// How many characters a review comment's row holds with the diff alone
/// across `panel`.
pub fn diff_only_cols(panel: Rect) -> usize {
    detail_cols(panel.size.w)
}

/// How many characters a review comment's row holds with a diff and its
/// file list across `panel`, the list taking `share`; none (0): the diff
/// alone.
pub fn diff_tree_cols(panel: Rect, share: f64) -> usize {
    match share > 0.0 {
        true => repos_detail_cols(panel, share),
        false => diff_only_cols(panel),
    }
}

/// How many lines the file list beside a diff shows at once in `panel`:
/// with the footer's rows all there, as `diff_only_page`.
pub fn diff_tree_page(panel: Rect, density: Density) -> usize {
    let middle = panel.size.h - REPOS_HEADER - footer_h(REPOS_FOOTER_ROWS);
    let tree = (middle - REPOS_DETAIL_TITLE - 2 * REPOS_PAD) / repos_row(density);
    tree.max(1) as usize
}

/// The room between the line numbers before a diff line, and between
/// them and its text.
const GUTTER_GAP: i32 = 6;

/// How wide the line numbers before the diff's lines are, logical pixels,
/// with `digits` digits each (old, then new; on a `plain` page, the one):
/// none for none.
fn gutter_width(digits: usize, plain: bool) -> i32 {
    if digits == 0 {
        return 0;
    }
    let advance = measure_mono("0", SMALL_FONT_SIZE)
        .filter(|a| *a > 0.0)
        .unwrap_or(SMALL_FONT_SIZE * 0.6);
    let columns = if plain { 1.0 } else { 2.0 };
    (columns * digits as f32 * advance).ceil() as i32 + 2 * GUTTER_GAP
}

/// How many chars of a diff line fit on its row past numbers of `digits`
/// digits (`plain`: of the one column), where a comment's row holds `cols`
/// (`repos_detail_cols`).
pub fn diff_line_room(cols: usize, digits: usize, plain: bool) -> usize {
    let advance = measure_mono("0", REPOS_MONO_SIZE)
        .filter(|a| *a > 0.0)
        .unwrap_or(REPOS_MONO_SIZE * 0.6);
    let px = cols as f32 * advance + COMMENT_INDENT as f32 - gutter_width(digits, plain) as f32;
    ((px / advance).floor() as usize).max(8)
}

/// How many characters a review comment's row holds in a detail pane
/// `detail` wide.
fn detail_cols(detail: i32) -> usize {
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
    let level = |depth: usize| REPOS_INDENT + depth as i32 * REPOS_LEVEL;
    let indent = match line {
        TreeLine::Repo(_) | TreeLine::Clean(_) => 0,
        TreeLine::Dir(dir) => level(dir.depth),
        TreeLine::File(file) => level(file.depth),
    };
    let check_x = x + REPOS_LINE_INSET + indent;
    // The heading has no checkbox: its chevron is where a repository's is.
    let check_w = match line {
        TreeLine::Clean(_) => 0,
        _ => REPOS_CHECK,
    };
    let check = Rectangle::new((check_x, y).into(), (check_w, h).into());
    let expands = match line {
        TreeLine::Repo(repo) => repo.expanded.is_some(),
        TreeLine::Dir(_) | TreeLine::Clean(_) => true,
        TreeLine::File(_) => false,
    };
    let expander = expands.then(|| {
        Rectangle::new(
            (check_x + REPOS_CHECK, y).into(),
            (REPOS_EXPANDER, h).into(),
        )
    });
    (rect, check, expander)
}

/// The tree's toolbar over its lines, in `tree`: its buttons at its
/// right, with `nav` first those going to the previous and the next file
/// not viewed, then apart from them those unfolding and folding
/// everything, then apart again one per way of listing the files.
fn tree_tools(tree: Rect, nav: bool) -> Vec<(TreeTool, Rect)> {
    use TreeTool::*;
    let y = tree.loc.y + (REPOS_DETAIL_TITLE - ROW_BUTTON) / 2;
    let mut x = tree.loc.x + tree.size.w - HELP_PADDING;
    let mut tools = Vec::new();
    let all = [Folders, List, Collapse, Expand, NextUnviewed, PrevUnviewed];
    for tool in &all[..if nav { all.len() } else { 4 }] {
        x -= ROW_BUTTON;
        tools.push((
            *tool,
            Rectangle::new((x, y).into(), (ROW_BUTTON, ROW_BUTTON).into()),
        ));
        x -= match tool {
            List | Expand => REPOS_TOOL_GAP,
            _ => REPOS_TAB_GAP,
        };
    }
    tools.reverse();
    tools
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
            tree.loc.y + REPOS_DETAIL_TITLE + REPOS_PAD,
        )
            .into(),
        (
            SCROLLBAR,
            (tree.size.h - REPOS_DETAIL_TITLE - 2 * REPOS_PAD).max(0),
        )
            .into(),
    );
    let whole = view
        .lines
        .len()
        .saturating_sub(usize::from(view.part > 0.0));
    (track, layout.lines.len().min(whole))
}

/// The scrollbar in `track` of a list of `total` rows showing `shown` of
/// them from row `pos` (to the fraction), as every scrollbar is placed
/// (`Thumb::of`); none when they all fit.
fn rows_thumb(track: Rect, pos: f64, shown: usize, total: usize) -> Option<super::Thumb> {
    let bar = super::ListBar {
        down: pos,
        total: total as f64,
    };
    super::Thumb::of(track, shown as f64, bar)
}

/// The scrollbars of the tree and of the detail in `layout`, each where
/// its list does not all show.
pub fn repos_thumbs(view: &ReposView, layout: &ReposLayout) -> [Option<super::Thumb>; 2] {
    let (tree, shown) = tree_bar(view, layout);
    let tree = rows_thumb(tree, view.first as f64 + view.part, shown, view.total);
    let detail = &view.detail;
    let (track, shown) = detail_bar(view, layout);
    let pos = detail.first as f64 + detail.part;
    [tree, rows_thumb(track, pos, shown, detail.total)]
}

/// How wide the detail's rows are in a pane `w` wide: short of its
/// padding and its scrollbar.
fn body_width(w: i32) -> i32 {
    (w - 2 * HELP_PADDING - SCROLLBAR).max(0)
}

/// The detail's scrollbar in `layout`, and how many of `view`'s rows it
/// shows: a page.
fn detail_bar(view: &ReposView, layout: &ReposLayout) -> (Rect, usize) {
    let pane = layout.detail;
    let line_h = repos_detail_line(view.density);
    let track = Rectangle::new(
        (
            pane.loc.x + pane.size.w - HELP_PADDING / 2 - SCROLLBAR,
            pane.loc.y + REPOS_DETAIL_TITLE + REPOS_PAD,
        )
            .into(),
        (
            SCROLLBAR,
            (pane.size.h - REPOS_DETAIL_TITLE - 2 * REPOS_PAD).max(0),
        )
            .into(),
    );
    let shown = layout.detail_list.size.h / line_h.max(1);
    (track, shown.max(1) as usize)
}

/// A row of the detail as drawn: what its pixels come from, but its
/// place.
#[derive(Clone, Debug, PartialEq)]
struct DetailLook {
    row: DetailRow,
    mark: RowMark,
    /// Its buttons, in order.
    buttons: Vec<Hint>,
    /// What the pointer is over on it, as on the first row.
    hover: Option<ReposHit>,
    /// The detail has the keyboard.
    focused: bool,
    /// The places the search found on it (`DetailView::found`).
    found: Vec<(Range<usize>, bool)>,
    /// The chars selected on it (`DetailView::chosen`).
    chosen: Option<Range<usize>>,
    /// The char the caret is on on it (`DetailView::caret`).
    caret: Option<usize>,
    /// The word at the caret on it (`DetailView::word`).
    word: Option<Range<usize>>,
    /// The digits of the line numbers before a line (`DetailView::gutter`).
    gutter: usize,
    /// One column of them, no tint (`DetailView::plain`).
    plain: bool,
    /// Its line is shown from a char on (`DetailView::skipped`).
    skipped: bool,
}

/// Row `i` of `view`'s detail, as drawn.
fn detail_look(view: &ReposView, i: usize) -> DetailLook {
    use ReposHit::*;
    let buttons: Vec<Hint> = view
        .detail
        .row_buttons
        .iter()
        .filter(|(j, _)| *j == i)
        .map(|(_, hint)| hint.clone())
        .collect();
    let hover = match view.hover {
        Some(DiffRow(j)) if j == i => Some(DiffRow(0)),
        Some(DiffCheck(j)) if j == i => Some(DiffCheck(0)),
        Some(DiffFold(j)) if j == i => Some(DiffFold(0)),
        Some(DiffRestore(j)) if j == i => Some(Button(ReposButton::Restore)),
        Some(Button(b)) if buttons.iter().any(|h| h.button == b) => Some(Button(b)),
        _ => None,
    };
    DetailLook {
        row: view.detail.rows[i].clone(),
        mark: view.detail.marks.get(i).copied().unwrap_or_default(),
        buttons,
        hover,
        focused: view.detail.focused,
        found: view.detail.found.get(i).cloned().unwrap_or_default(),
        chosen: (view.detail.chosen.iter())
            .find(|(j, _)| *j == i)
            .map(|(_, chars)| chars.clone()),
        caret: (view.detail.caret)
            .filter(|(j, _)| *j == i)
            .map(|(_, col)| col),
        word: (view.detail.word.as_ref())
            .filter(|(j, _)| *j == i)
            .map(|(_, word)| word.clone()),
        gutter: view.detail.gutter,
        plain: view.detail.plain,
        skipped: view.detail.skipped.is_some_and(|(j, _)| j == i),
    }
}

/// A line of the branch menu's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuItem {
    /// By index into `BranchMenuView::headings`.
    Heading(usize),
    /// By index into `BranchMenuView::rows`.
    Row(usize),
}

/// The first `laid` lines of `menu`'s list, top down: each group's
/// heading over its rows.
fn menu_items(menu: &BranchMenuView, laid: usize) -> Vec<MenuItem> {
    let (mut headings, mut rows) = (0, 0);
    let mut items = Vec::with_capacity(laid);
    for _ in 0..laid {
        if menu
            .headings
            .get(headings)
            .is_some_and(|&(at, _)| at == rows)
        {
            items.push(MenuItem::Heading(headings));
            headings += 1;
        } else if rows < menu.rows.len() {
            items.push(MenuItem::Row(rows));
            rows += 1;
        }
    }
    items
}

/// A line of the branch menu as drawn: what its pixels come from, but
/// its place.
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuLook {
    Heading(String),
    Branch {
        label: String,
        /// Enter picks it.
        picked: bool,
        /// They are all on it.
        current: bool,
        /// It has a local branch to delete.
        deletable: bool,
        /// What the pointer is over on it, as on the first row.
        hover: Option<ReposHit>,
    },
}

/// Line `item` of `view`'s branch menu `menu`, as drawn.
fn menu_look(view: &ReposView, menu: &BranchMenuView, item: MenuItem) -> MenuLook {
    match item {
        MenuItem::Heading(i) => MenuLook::Heading(
            menu.headings
                .get(i)
                .map_or(String::new(), |(_, l)| l.clone()),
        ),
        MenuItem::Row(i) => MenuLook::Branch {
            label: menu.rows[i].clone(),
            picked: menu.picked == Some(i),
            current: menu.current == Some(i),
            deletable: menu.deletable.get(i) == Some(&true),
            hover: match view.hover {
                Some(ReposHit::BranchRow(j)) if j == i => Some(ReposHit::BranchRow(0)),
                Some(ReposHit::BranchDelete(j)) if j == i => Some(ReposHit::BranchDelete(0)),
                _ => None,
            },
        },
    }
}

/// The lines of `menu` as `layout` has them, top down, each with its
/// rect; `rows` the rows'.
fn menu_rects(menu: &BranchMenuView, layout: &ReposLayout, rows: &[Rect]) -> Vec<(MenuItem, Rect)> {
    let laid = layout.branch_headings.len() + rows.len();
    menu_items(menu, laid)
        .into_iter()
        .filter_map(|item| {
            let rect = match item {
                MenuItem::Heading(i) => layout.branch_headings.iter().find(|(j, _)| *j == i)?.1,
                MenuItem::Row(i) => *rows.get(i)?,
            };
            Some((item, rect))
        })
        .collect()
}

/// What of the branch menu `menu` its card is drawn from: all but its
/// lines and where they stand. `empty` is what it says, empty while it
/// has rows.
fn menu_card_view(menu: &BranchMenuView) -> BranchMenuView {
    BranchMenuView {
        empty: match menu.rows.is_empty() {
            true => menu.empty.clone(),
            false => String::new(),
        },
        query: menu.query.clone(),
        placeholder: menu.placeholder.clone(),
        create: menu.create.clone(),
        ..BranchMenuView::default()
    }
}

/// A tree line as drawn: what its pixels come from, but its place.
#[derive(Clone, Debug, PartialEq)]
struct TreeRow {
    line: TreeLine,
    /// It is the tree's first line: a repository's has no rule over it.
    top: bool,
    selected: bool,
    /// Selected while the field has the keyboard: lit less.
    dim: bool,
    /// What the pointer is over on it, as on the first line.
    hover: Option<ReposHit>,
    /// It has no checkbox (`ReposView::unchecked`).
    unchecked: bool,
    /// It is the file list of a feature's code review (`ReposView::diff_tree`):
    /// a file's half box there means changed since viewed, said by a tag.
    review: bool,
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
        top: view.first + i == 0,
        selected,
        dim: selected && view.field_focused,
        hover,
        unchecked: view.unchecked,
        review: view.diff_tree,
    }
}

/// Where the parts of the panel showing `view` go in `panel`.
pub fn repos_layout(view: &ReposView, panel: Rect) -> ReposLayout {
    parts(view, panel).layout
}

/// The panel's parts: the header across the top; the tree on the left and
/// the detail pane on the right under it; then across the bottom the
/// field, the error line, the result bar and the footer's actions.
fn parts(view: &ReposView, panel: Rect) -> Parts {
    let (x, top, w) = (panel.loc.x, panel.loc.y, panel.size.w);
    let bottom_edge = top + panel.size.h;
    let inner_x = x + HELP_PADDING;
    let inner_w = (w - 2 * HELP_PADDING).max(0);
    let header_h = if view.no_header { 0 } else { REPOS_HEADER };
    let header = Rectangle::new((x, top).into(), (w, header_h).into());
    // The diff alone: none of the rest, all of the panel under the header.
    let only = view.diff_only;
    let no_tree = only && !view.diff_tree;
    // The branch picker at the header's left, no more than a third of it.
    let pw = picker_width(&view.branch.label).min(inner_w / 3).max(1);
    let branch = Rectangle::new(
        (inner_x, top + (REPOS_HEADER - ROW_BUTTON) / 2).into(),
        (if only { 0 } else { pw }, ROW_BUTTON).into(),
    );

    let row_h = repos_row(view.density);
    let line_h = repos_detail_line(view.density);

    // From the bottom up: the footer, the result bar, the error, the field;
    // the diff alone has a footer only, while it has actions.
    let (rows, overflow) = hint_rows(&view.hints, inner_w);
    let footer_height = if only && view.hints.is_empty() {
        0
    } else {
        footer_h(rows.len())
    };
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
    let field_row = if only { 0 } else { REPOS_FIELD_ROW };
    y -= field_row;
    let field = Rectangle::new(
        (inner_x, y + (field_row - REPOS_FIELD).max(0) / 2).into(),
        (inner_w, if only { 0 } else { REPOS_FIELD }).into(),
    );
    let middle_y = top + header_h;
    let middle_h = (y - middle_y).max(0);

    // The tree and its lines, short of its scrollbar.
    let tree_w = if no_tree {
        0
    } else {
        tree_width(w, view.tree_share)
    };
    let tree = Rectangle::new((x, middle_y).into(), (tree_w, middle_h).into());
    let (line_x, line_w) = (x + REPOS_PAD, (tree_w - 2 * REPOS_PAD - SCROLLBAR).max(0));
    let tree_end = middle_y + middle_h - REPOS_PAD;
    let lines_y = middle_y + REPOS_DETAIL_TITLE + REPOS_PAD;
    // Scrolled part of the first line, the lines start up by as much and
    // the last is shown in part, cut at where the others end; pulled past
    // an end, they move with the pull.
    let tree_shift = scrolled_by(view.part, view.over, row_h);
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
        let ly = (at as f64 - tree_shift).round() as i32;
        let rect = Rectangle::new((line_x, ly).into(), (line_w, row_h).into());
        let (_, check, expander) = line_rects(line, rect);
        // Nothing to check: the box takes no clicks, its room stays.
        let check = match view.unchecked {
            true => Rectangle::new(check.loc, (0, 0).into()),
            false => check,
        };
        if let TreeLine::Repo(repo) = line {
            // Measured as the painter measures, at one pixel a pixel.
            let width = |text: &str, size: f32| measure(text, size).unwrap_or(0.0);
            let bold = |text: &str, size: f32| measure_bold(text, size).unwrap_or(0.0);
            let x = (check.loc.x + REPOS_CHECK + REPOS_EXPANDER) as f32;
            let right = (line_x + line_w) as f32 - PADDING;
            let widths: (Width, Width) = (&width, &bold);
            let spans = line_spans(repo, (x, right), FIELD_GAP as f32, 1.0, widths);
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
    // A file's page takes what the view leaves of it.
    let (detail, page_gap, page) = split_rects(
        Rectangle::new(
            (x + tree_w, middle_y).into(),
            ((w - tree_w).max(0), middle_h).into(),
        ),
        view.detail_share,
    );
    let (page, page_gap) = match view.detail_share < 1.0 {
        true => (Some(page), Some(page_gap).filter(|g| g.size.w > 0)),
        false => (None, None),
    };
    let modes = view.modes.as_ref().map(|switch| {
        let n = switch.labels.len() as i32;
        let (b, track_w) = (1, n * REPOS_TAB_WIDTH + 2);
        let track = Rectangle::new(
            (
                x + w - HELP_PADDING - track_w,
                top + (REPOS_HEADER - ROW_BUTTON) / 2,
            )
                .into(),
            (track_w, ROW_BUTTON).into(),
        );
        let segments = (0..n)
            .map(|i| {
                Rectangle::new(
                    (track.loc.x + b + i * REPOS_TAB_WIDTH, track.loc.y + b).into(),
                    (REPOS_TAB_WIDTH, ROW_BUTTON - 2 * b).into(),
                )
            })
            .collect();
        (track, segments)
    });
    let splitter = Rectangle::new(
        (x + tree_w - SPLITTER_GRAB, middle_y).into(),
        (if no_tree { 0 } else { 2 * SPLITTER_GRAB }, middle_h).into(),
    );
    let tab_y = middle_y + (REPOS_DETAIL_TITLE - ROW_BUTTON) / 2;
    let tabs: &[DetailTab] = match only {
        true => &[],
        false => &[DetailTab::Diff, DetailTab::Log, DetailTab::Stashes],
    };
    let tabs_w = (tabs.len() as i32 * (REPOS_TAB_WIDTH + REPOS_TAB_GAP) - REPOS_TAB_GAP).max(0);
    let tabs_x = detail.loc.x + detail.size.w - HELP_PADDING - tabs_w;
    let detail_tabs = tabs
        .iter()
        .copied()
        .enumerate()
        .map(|(k, tab)| {
            let tx = tabs_x + k as i32 * (REPOS_TAB_WIDTH + REPOS_TAB_GAP);
            let size = (REPOS_TAB_WIDTH, ROW_BUTTON);
            (tab, Rectangle::new((tx, tab_y).into(), size.into()))
        })
        .collect();
    let body: Rect = Rectangle::new(
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
    // Scrolled part of the first row, the rows start up by as much and the
    // last is shown in part, cut at where the others end; pulled past an
    // end, they move with the pull. The header pinned at the top goes over
    // them, where its pin has it.
    let pinned = usize::from(view.detail.sticky);
    let detail_shift = scrolled_by(view.detail.part, view.detail.over, line_h);
    let under = usize::from(view.detail.part > 0.0);
    let whole_rows = view.detail.rows.len().saturating_sub(pinned + under);
    let mut detail_shown = 0;
    for (i, row) in view.detail.rows.iter().enumerate() {
        let ry = match i.checked_sub(pinned) {
            None => (body.loc.y as f64 + view.detail.pin).round() as i32,
            Some(j) => {
                let at = body.loc.y + j as i32 * line_h;
                if j < whole_rows && at + line_h > body.loc.y + body.size.h {
                    break;
                }
                detail_shown += usize::from(j < whole_rows);
                (at as f64 - detail_shift).round() as i32
            }
        };
        if let DetailRow::File(file) = row {
            let fold = Rectangle::new((body.loc.x, ry).into(), (DIFF_FOLD, line_h).into());
            detail_folds.push((i, fold));
            if file.check.is_some() {
                let check = Rectangle::new(
                    (body.loc.x + DIFF_FOLD, ry).into(),
                    (DIFF_CHECK, line_h).into(),
                );
                detail_checks.push((i, check));
            }
        }
        detail_rows.push(Rectangle::new(
            (body.loc.x, ry).into(),
            (body.size.w, line_h).into(),
        ));
    }
    let detail_list = Rectangle::new(
        (detail.loc.x, body.loc.y).into(),
        (detail.size.w, detail_shown as i32 * line_h).into(),
    );
    // The title row's buttons left of the tabs, the rows' at their right,
    // each right-aligned in order; none on a title too narrow for them.
    let mut detail_title_buttons = Vec::new();
    let mut right = tabs_x - REPOS_TAB_GAP * 2;
    for hint in view.detail.title_buttons.iter().rev() {
        let bw = hint_width(&without_key(hint));
        right -= bw;
        if right < detail.loc.x + detail.size.w / 3 {
            break;
        }
        let rect = Rectangle::new((right, tab_y).into(), (bw, ROW_BUTTON).into());
        detail_title_buttons.push((hint.button, rect));
        right -= REPOS_BUTTON_GAP;
    }
    detail_title_buttons.reverse();
    // The diff's search left of them: its button while closed; open, its
    // field across the rest of the row, then how many it found and its
    // buttons.
    let (mut detail_search, mut detail_search_count) = (Vec::new(), None);
    let right = detail_title_buttons
        .first()
        .map_or(tabs_x - REPOS_TAB_GAP * 2, |(_, r)| {
            r.loc.x - REPOS_BUTTON_GAP
        });
    let square = |x: i32| Rectangle::new((x, tab_y).into(), (ROW_BUTTON, ROW_BUTTON).into());
    let left = detail.loc.x + HELP_PADDING;
    if view.detail.tab == DetailTab::Diff {
        match &view.detail.search {
            None if right - ROW_BUTTON >= detail.loc.x + detail.size.w / 3 => {
                detail_search.push((SearchPart::Open, square(right - ROW_BUTTON)));
            }
            None => {}
            Some(_) => {
                let count;
                (detail_search, count) = search_spots(left, right, tab_y, ROW_BUTTON);
                detail_search_count = Some(count);
            }
        }
    }
    let mut detail_row_buttons = Vec::new();
    for (i, row) in detail_rows.iter().enumerate() {
        let hints: Vec<&Hint> = view
            .detail
            .row_buttons
            .iter()
            .filter(|(j, _)| *j == i)
            .map(|(_, hint)| hint)
            .collect();
        let rects = line_chip_rects(
            *row,
            view.detail.rows.get(i),
            (view.detail.gutter, view.detail.plain),
            hints.iter().map(|h| row_chip_width(h)),
        );
        for (hint, rect) in hints.iter().zip(rects) {
            detail_row_buttons.push((i, hint.button, rect));
        }
    }

    // The result bar's buttons, right-aligned.
    let mut result_buttons = Vec::new();
    if let (Some(bar), Some(rect)) = (&view.result, result) {
        let mut right = inner_x + inner_w;
        let by = rect.loc.y + (REPOS_RESULT - ROW_BUTTON) / 2;
        for (i, hint) in bar.buttons.iter().enumerate().rev() {
            let bw = hint_width(&without_key(hint));
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

    // The footer's actions, row by row, centred as every footer's are; the
    // "more" button after the last row's.
    let mut hints = Vec::new();
    let mut more = None;
    for (r, row) in rows.iter().enumerate() {
        let ry = footer_y + REPOS_FOOTER_PAD + r as i32 * REPOS_HINT_ROW;
        let by = ry + (REPOS_HINT_ROW - ROW_BUTTON) / 2;
        let mut used: i32 = row.iter().map(|(_, hw)| hw + HINT_GAP).sum::<i32>() - HINT_GAP;
        if r + 1 == rows.len() && !overflow.is_empty() {
            used += HINT_GAP + more_width(overflow.len());
        }
        let mut hx = inner_x + (inner_w - used).max(0) / 2;
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
            .chain(&menu.widest)
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
        // Scrolled part of a line, the lines start up by as much; pulled
        // past an end, they move with the pull.
        let shift = scrolled_by(menu.part, menu.over, row_h);
        let card = Rectangle::new((mx, my).into(), (mw, mh).into());
        branch_search = Some(Rectangle::new(
            (mx + BRANCH_MENU_PAD, my + BRANCH_MENU_PAD).into(),
            (inner, row_h).into(),
        ));
        let line = |j: usize| {
            let y = (rows_y as f64 + (j as i32 * row_h) as f64 - shift).round() as i32;
            Rectangle::new((mx + BRANCH_MENU_PAD, y).into(), (inner, row_h).into())
        };
        let mut rows: Vec<Rect> = Vec::new();
        let laid = if shown == lines { shown + under } else { shown };
        for (j, item) in menu_items(menu, laid).into_iter().enumerate() {
            match item {
                MenuItem::Heading(i) => branch_headings.push((i, line(j))),
                MenuItem::Row(_) => rows.push(line(j)),
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
            tree_tools: if no_tree {
                Vec::new()
            } else {
                tree_tools(tree, view.tree_nav.is_some())
            },
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
            detail_search,
            detail_search_count,
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
            modes,
            page,
            page_gap,
        },
        error,
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
    if let Some((_, segments)) = &layout.modes
        && let Some(i) = segments.iter().position(at)
    {
        return Some(ReposHit::Mode(i));
    }
    if at(&layout.branch) {
        return Some(ReposHit::BranchButton);
    }
    if layout.branch_menu.is_some() {
        return Some(ReposHit::Panel);
    }
    if let Some(&(tool, _)) = layout.tree_tools.iter().find(|(_, r)| at(r)) {
        return Some(ReposHit::Tool(tool));
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
    if let Some((part, _)) = layout.detail_search.iter().find(|(_, r)| at(r)) {
        return Some(ReposHit::Search(*part));
    }
    // A scrolled pane's rows count only where they show; the header
    // pinned at the top, the first, over those under it.
    let list = layout.detail_list;
    let shown = |_: usize| at(&list);
    let row_button = layout
        .detail_row_buttons
        .iter()
        .find(|(i, _, r)| at(r) && shown(*i));
    match row_button {
        Some((i, ReposButton::Restore, _)) => return Some(ReposHit::DiffRestore(*i)),
        Some((_, button, _)) => return Some(ReposHit::Button(*button)),
        None => {}
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

/// The diff line shown (by index into `DetailView::rows`) and the char of
/// its text at `pos`, in `panel`, if `pos` is on one.
pub fn repos_diff_char(
    view: &ReposView,
    panel: Rect,
    pos: Point<f64, Logical>,
) -> Option<(usize, usize)> {
    let layout = repos_layout(view, panel);
    let Some(ReposHit::DiffRow(i)) = repos_hit(&layout, pos) else {
        return None;
    };
    let x0 = layout.detail_rows.get(i)?.loc.x + DIFF_INSET;
    let advance = measure_mono("0", REPOS_MONO_SIZE).filter(|a| *a > 0.0)? as f64;
    let start = match view.detail.rows.get(i)? {
        DetailRow::Line(_) => x0 + gutter_width(view.detail.gutter, view.detail.plain),
        DetailRow::Editor { .. } => return editor_char(view, &layout, i, pos.x),
        _ => return None,
    };
    let x = pos.x - start as f64;
    // A row shown from a char on: chars of the line, not of the row.
    let skip = match view.detail.skipped {
        Some((j, skip)) if j == i => skip,
        _ => 0,
    };
    (x >= 0.0).then(|| (i, (x / advance).floor() as usize + skip))
}

/// The row of the comment being written (by index into `DetailView::rows`)
/// nearest `pos`, in `panel`, and the caret's place at `pos.x` on it: for
/// a press there dragged on, above its rows the first, below the last.
/// With none of its rows in view, no row, and the place as on one of them
/// outside a finding's card.
pub fn repos_editor_char(
    view: &ReposView,
    panel: Rect,
    pos: Point<f64, Logical>,
) -> Option<(Option<usize>, usize)> {
    let layout = repos_layout(view, panel);
    let rows = (layout.detail_rows.iter().enumerate())
        .filter(|&(i, _)| matches!(view.detail.rows.get(i), Some(DetailRow::Editor { .. })));
    let mut nearest = None;
    for (i, rect) in rows {
        nearest = Some(i);
        if pos.y < (rect.loc.y + rect.size.h) as f64 {
            break;
        }
    }
    let within = match nearest.and_then(|i| view.detail.rows.get(i)) {
        Some(DetailRow::Editor { within, .. }) => within.is_some(),
        _ => false,
    };
    let rect = layout.detail_rows.get(nearest.unwrap_or(0))?;
    let advance = measure_mono("0", REPOS_MONO_SIZE).filter(|a| *a > 0.0)? as f64;
    let card = rect.loc.x + DIFF_INSET + if within { COMMENT_INDENT } else { 0 };
    let x = pos.x - (card + 1 + COMMENT_INDENT) as f64;
    Some((nearest, (x / advance).round().max(0.0) as usize))
}

/// On row `i`, of the comment being written, the caret's place at output x
/// `x`: the char boundary nearest, as `detail_row` lays its text out
/// (inside a finding's card, a step further in).
fn editor_char(view: &ReposView, layout: &ReposLayout, i: usize, x: f64) -> Option<(usize, usize)> {
    let DetailRow::Editor { within, .. } = view.detail.rows.get(i)? else {
        return None;
    };
    let advance = measure_mono("0", REPOS_MONO_SIZE).filter(|a| *a > 0.0)? as f64;
    let card = layout.detail_rows.get(i)?.loc.x
        + DIFF_INSET
        + if within.is_some() { COMMENT_INDENT } else { 0 };
    let x = x - (card + 1 + COMMENT_INDENT) as f64;
    Some((i, (x / advance).round().max(0.0) as usize))
}

/// The panel's field, for the caret and selection, in `panel`.
pub fn repos_field(view: &ReposView, panel: Rect, scale: f64) -> FieldSpot {
    let layout = repos_layout(view, panel);
    // The open branch menu's search has the keyboard, else the diff's
    // while it has.
    let diff_search = layout
        .detail_search
        .iter()
        .find(|(part, _)| *part == SearchPart::Field);
    // Both searches start with a magnifier; the commit message doesn't.
    let (area, line, text) = match (&view.branch.menu, layout.branch_search) {
        (Some(menu), Some(search)) => (search, &menu.query, search_field_text(search, scale)),
        _ => match (&view.detail.search, diff_search) {
            (Some(bar), Some(&(_, field))) if bar.focused => {
                (field, &bar.query, search_field_text(field, scale))
            }
            _ => (layout.field, &view.field, field_text(layout.field, scale)),
        },
    };
    FieldSpot {
        size: FONT_SIZE,
        area,
        text,
        line: line.clone(),
    }
}

/// Signed distance to a square with a "+" in it (unfold everything) or a
/// "−" (fold everything), centred on the origin.
fn fold_all_mark(open: bool, x: f32, y: f32, s: f32) -> f32 {
    let frame = rounded_rect(x, y, -6.5 * s, -6.5 * s, 13.0 * s, 13.0 * s, 2.5 * s).abs() - 0.8 * s;
    let across = segment(x, y, -3.0 * s, 0.0, 3.0 * s, 0.0) - 0.8 * s;
    let down = segment(x, y, 0.0, -3.0 * s, 0.0, 3.0 * s) - 0.8 * s;
    match open {
        true => frame.min(across).min(down),
        false => frame.min(across),
    }
}

/// Signed distance to a list centred on the origin: three rows, each a
/// dot and a line.
fn list_mark(x: f32, y: f32, s: f32) -> f32 {
    [-4.5, 0.0, 4.5]
        .into_iter()
        .map(|row: f32| {
            let dot = (x + 5.5 * s).hypot(y - row * s) - 1.1 * s;
            let line = segment(x, y, -2.5 * s, row * s, 6.5 * s, row * s) - 0.8 * s;
            dot.min(line)
        })
        .fold(f32::MAX, f32::min)
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
/// when they all fit. Only the single-buffer drawings (for tests) draw it
/// in place; the panels place theirs as parts of their own.
#[cfg(test)]
pub(super) fn scrollbar(
    c: &mut Canvas,
    track: Rect,
    (first, shown, total): (usize, usize, usize),
    scale: f64,
    p: &Palette,
) {
    let Some(t) = rows_thumb(track, first as f64, shown, total) else {
        return;
    };
    let (x, _, w, _) = rect_px(track, scale);
    let (top, h) = ((t.top * scale).round() as f32, (t.h * scale).round() as f32);
    c.rounded(x, top, w, h, w / 2.0, mix(p.elevated, p.muted, 0.45));
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

/// A file's path as shown name first: its name, then its folder and,
/// renamed, where it was (`src/chrome  ← src/old.rs`). A rename comes as
/// `old → new`.
pub fn name_first(path: &str) -> (&str, String) {
    let (from, to) = match path.split_once(" → ") {
        Some((from, to)) => (Some(from), to),
        None => (None, path),
    };
    // A folder git lists whole keeps its `/`.
    let cut = to.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
    let mut rest = to[..cut].trim_end_matches('/').to_owned();
    if let Some(from) = from {
        if !rest.is_empty() {
            rest.push_str("  ");
        }
        rest.push_str("← ");
        rest.push_str(from);
    }
    (&to[cut..], rest)
}

/// The colour of the words on a solid `tone` tag ("AI review").
fn comment_tag_ink(tone: Tone, p: &Palette) -> Rgb {
    match tone {
        Tone::Normal | Tone::Muted => p.elevated,
        _ => p.on_accent,
    }
}

/// The colour of a finding's heading words on its band, dim as its card's
/// once dismissed.
fn comment_head_word(tone: Tone, p: &Palette) -> Rgb {
    match tone {
        Tone::Muted => p.muted,
        _ => p.text,
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
        Tone::Medium => p.medium,
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

    /// A file's `path` name first between `x` and `max_x`: its name in
    /// the first of `colors`, then its folder (and where it was renamed
    /// from) in the second, cut off at the end when it doesn't fit.
    /// Returns where it ends.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn name_first(
        &self,
        c: &mut Canvas,
        path: &str,
        size: f32,
        (x, max_x): (f32, f32),
        band: (i32, i32),
        scale: f64,
        (name_color, rest_color): (Rgb, Rgb),
    ) -> f32 {
        let (name, rest) = name_first(path);
        let pen = self.mixed_text(c, name, size, x, max_x, band, scale, name_color);
        let gap = (FIELD_GAP as f32 * scale as f32).round();
        if rest.is_empty() || pen + gap >= max_x {
            return pen;
        }
        self.mixed_text(c, &rest, size, pen + gap, max_x, band, scale, rest_color)
    }

    /// The repositories panel laid out in `panel`, inside a margin of
    /// `SHADOW` holding its drop shadow, in one buffer: its frame with the
    /// lists over it as they show at rest, then the branch menu and the
    /// sheet over all.
    #[cfg(test)]
    pub(super) fn repos(
        &self,
        view: &ReposView,
        panel: Rect,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let parts = parts(view, panel);
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
        self.detail_rows(&mut c, view, layout, by, scale, p);
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
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let mut c = match view.flat {
            // Laid into a tab: no shadow, border or corners. The buffer
            // keeps the popup's margin, so that everything goes where it
            // goes in one.
            // What is left open shows the file's page under it.
            true => {
                let mut c = Canvas::new(physical(super::shadowed(panel).1, scale));
                let hole = (layout.page).unwrap_or(Rectangle::new(panel.loc, (0, 0).into()));
                for r in around(panel, hole) {
                    let r = moved(r, buffer_origin(panel));
                    let (x, y) = (px(r.loc.x), px(r.loc.y));
                    let (w, h) = (px(r.loc.x + r.size.w) - x, px(r.loc.y + r.size.h) - y);
                    c.fill(x, y, w, h, p.elevated);
                }
                c
            }
            false => self.popup(panel.size, HELP_RADIUS, scale, p),
        };
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
        let only = view.diff_only;
        let hx = match only {
            true => px(header.loc.x + HELP_PADDING) as f32,
            false => px(picker.loc.x + picker.size.w + HEADER_GAP) as f32,
        };
        let hend = match &layout.modes {
            Some((track, _)) => px(shift(*track).loc.x - HEADER_GAP) as f32,
            None => hend - px(HELP_PADDING) as f32,
        };
        let heading = if only {
            view.title.clone()
        } else if view.title.is_empty() {
            "Repositories".to_owned()
        } else {
            format!("Repositories · {}", view.title)
        };
        if !view.no_header {
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
                tone_color(view.summary_tone, p),
            );
            if !only {
                self.branch_button(&mut c, view, picker, scale, p);
            }
            if let (Some(switch), Some((track, rects))) = (&view.modes, &layout.modes) {
                let segments: Vec<(&str, Rect)> = (switch.labels.iter().map(String::as_str))
                    .zip(rects.iter().map(|&r| shift(r)))
                    .collect();
                let hover = match view.hover {
                    Some(ReposHit::Mode(i)) => Some(i),
                    _ => None,
                };
                let state = (switch.on, hover, &switch.off[..]);
                self.segmented_off(&mut c, shift(*track), &segments, state, scale, p);
            }
            across(&mut c, px(header.loc.y + header.size.h) - border);
        }
        // The line between the detail and a file's page beside it.
        if let Some(gap) = layout.page_gap.map(shift) {
            c.fill(
                px(gap.loc.x + gap.size.w / 2),
                px(gap.loc.y),
                border,
                px(gap.loc.y + gap.size.h) - px(gap.loc.y),
                p.border_variant,
            );
        }

        // The tree, then the line between it and the detail pane.
        let tree = shift(layout.tree);
        let tools: Vec<_> = layout
            .tree_tools
            .iter()
            .map(|&(g, r)| (g, shift(r)))
            .collect();
        let no_tree = only && !view.diff_tree;
        if !no_tree {
            self.tree_title(&mut c, view, tree, &tools, scale, p);
        }
        if !no_tree && view.lines.is_empty() && !view.empty.is_empty() {
            let first = Rectangle::new(
                (
                    tree.loc.x + REPOS_PAD + REPOS_LINE_INSET,
                    tree.loc.y + REPOS_DETAIL_TITLE + REPOS_PAD,
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
        if !no_tree {
            c.fill(
                px(detail.loc.x),
                px(detail.loc.y),
                border,
                px(detail.loc.y + detail.size.h) - px(detail.loc.y),
                p.border_variant,
            );
        }
        let tabs: Vec<_> = layout
            .detail_tabs
            .iter()
            .map(|(t, r)| (*t, shift(*r)))
            .collect();
        let title_buttons: Vec<_> = layout
            .detail_title_buttons
            .iter()
            .map(|&(b, r)| (b, shift(r)))
            .collect();
        let search: Vec<_> = layout
            .detail_search
            .iter()
            .map(|&(part, r)| (part, shift(r)))
            .collect();
        let buttons = (&title_buttons[..], &search[..]);
        self.detail_title(&mut c, view, detail, &tabs, buttons, scale, p);
        let count = layout.detail_search_count.map(shift);
        let hover = match view.hover {
            Some(ReposHit::Search(part)) => Some(part),
            _ => None,
        };
        let bar = view.detail.search.as_ref();
        self.search_bar(&mut c, bar, &search, count, hover, scale, p);

        // The field, what went wrong, the result bar and the footer; the
        // diff alone has the footer only, while it has actions.
        if only && view.hints.is_empty() {
            return c;
        }
        if !only {
            let field = shift(layout.field);
            across(
                &mut c,
                px(field.loc.y - (REPOS_FIELD_ROW - REPOS_FIELD) / 2) - border,
            );
            self.repos_field_box(&mut c, view, field, scale, p);
        }
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
                style: Default::default(),
            };
            self.hint_button(&mut c, view, &more, ReposHit::More, shift(rect), scale, p);
        }
        c
    }

    /// The branch menu of `view`, while open, as `layout` has it, moved by
    /// `by` onto the buffer: its card, then its lines cut to where they
    /// show.
    #[cfg(test)]
    fn repos_menu(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        layout: &ReposLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let (Some(menu), Some((card, rows))) = (&view.branch.menu, &layout.branch_menu) else {
            return;
        };
        self.repos_menu_card(c, view, layout, by, scale, p);
        if menu.rows.is_empty() {
            return;
        }
        let list = moved(layout.branch_list.unwrap_or(*card), by);
        let (lx, ly, lw, lh) = rect_px(list, scale);
        c.clipped(lx as i32, ly as i32, lw as i32, lh as i32, |c| {
            for (item, rect) in menu_rects(menu, layout, rows) {
                let look = menu_look(view, menu, item);
                self.branch_line(c, &look, moved(rect, by), scale, p);
            }
        });
    }

    /// The branch menu's card of `view`, while open, as `layout` has it,
    /// moved by `by` onto the buffer.
    fn repos_menu_card(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        layout: &ReposLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let (Some(menu), Some((card, _)), Some(search)) =
            (&view.branch.menu, &layout.branch_menu, layout.branch_search)
        else {
            return;
        };
        let at = (
            moved(*card, by),
            moved(search, by),
            layout.branch_create.map(|r| moved(r, by)),
        );
        let card_view = menu_card_view(menu);
        let hover = view.hover.filter(|h| *h == ReposHit::BranchCreate);
        self.branch_menu_card(c, &card_view, hover, at, view.density, scale, p);
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
    /// A small tag in `color` on a tint of it, its right edge at `right`
    /// in `line_band`: where it starts, to go on left of it.
    #[allow(clippy::too_many_arguments)]
    fn tree_tag(
        &self,
        c: &mut Canvas,
        text: &str,
        right: f32,
        line_band: (i32, i32),
        color: Rgb,
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let s = scale as f32;
        let pad = (CHIP_PAD as f32 * s / 2.0).round();
        let w = self.mixed_width(text, SMALL_FONT_SIZE, scale) + 2.0 * pad;
        let x = (right - w).round();
        let (y, h) = line_band;
        let th = (CHIP_HEIGHT as f32 * s * 0.8).round();
        let ty = (y as f32 + (h as f32 - th) / 2.0).round();
        c.rounded(x, ty, w, th, CHIP_RADIUS * s, mix(p.elevated, color, 0.18));
        let ink = mix(color, p.text, 0.25);
        self.mixed_text(
            c,
            text,
            SMALL_FONT_SIZE,
            x + pad,
            right,
            line_band,
            scale,
            ink,
        );
        x
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
            TreeLine::Dir(_) | TreeLine::File(_) | TreeLine::Clean(_) => None,
        };
        let border = px(1).max(1);
        if let TreeLine::Repo(_) | TreeLine::Clean(_) = line {
            // It heads the lines under it: a band across, a rule over it
            // but on the first line.
            let (bx, by) = (x as i32, y as i32);
            c.fill(
                bx,
                by,
                w as i32,
                h as i32,
                mix(p.elevated, p.text, HEADER_TINT),
            );
            if !row.top {
                c.fill(bx, by, w as i32, border, p.border_variant);
            }
        }
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
        // A rule down from each line it is under, through its checkbox's
        // middle: its repository's, then its folders'.
        let depth = match line {
            TreeLine::Dir(dir) => Some(dir.depth),
            TreeLine::File(file) => Some(file.depth),
            TreeLine::Repo(_) | TreeLine::Clean(_) => None,
        };
        for level in 0..depth.map_or(0, |d| d + 1) {
            let indent = match level {
                0 => 0,
                n => REPOS_INDENT + (n as i32 - 1) * REPOS_LEVEL,
            };
            let gx = px(rect.loc.x + REPOS_LINE_INSET + indent + REPOS_CHECK / 2);
            c.fill(gx, y as i32, border, h as i32, p.border);
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
        let expander_color = if row.hover == Some(ReposHit::Expander(0)) {
            p.text
        } else {
            p.muted
        };
        match line {
            TreeLine::Repo(repo) => {
                let check_state = repo.check;
                self.checkbox(c, box_rect, check_state, check_hovered, scale, p);
                if let (Some(open), Some(rect)) = (repo.expanded, expander) {
                    icon_sdf(c, rect, scale, expander_color, |x, y| {
                        expander_mark(open, x, y, s)
                    });
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
            TreeLine::Clean(clean) => {
                if let Some(rect) = expander {
                    icon_sdf(c, rect, scale, expander_color, |x, y| {
                        expander_mark(clean.open, x, y, s)
                    });
                }
                let label_x = px(check.loc.x + REPOS_CHECK + REPOS_EXPANDER) as f32;
                let label = format!("Clean · {}", clean.count);
                self.mixed_text(
                    c,
                    &label,
                    SMALL_FONT_SIZE,
                    label_x,
                    right,
                    line_band,
                    scale,
                    p.muted,
                );
            }
            TreeLine::Dir(dir) => {
                if !row.unchecked {
                    self.checkbox(c, box_rect, dir.check, check_hovered, scale, p);
                }
                if let Some(rect) = expander {
                    icon_sdf(c, rect, scale, expander_color, |x, y| {
                        expander_mark(dir.open, x, y, s)
                    });
                }
                let icon_x = check.loc.x + REPOS_CHECK + REPOS_EXPANDER;
                let icon = Rectangle::new(
                    (icon_x, check.loc.y).into(),
                    (REPOS_FOLDER, check.size.h).into(),
                );
                icon_sdf(c, icon, scale, p.muted, |x, y| folder(x, y, 0.75 * s));
                self.mixed_text(
                    c,
                    &dir.name,
                    SMALL_FONT_SIZE,
                    px(icon_x + REPOS_FOLDER) as f32 + gap / 2.0,
                    right,
                    line_band,
                    scale,
                    p.text,
                );
            }
            TreeLine::File(file) => {
                if !row.unchecked {
                    self.checkbox(c, box_rect, file.check, check_hovered, scale, p);
                }
                // At the right its status letter, its counts before it.
                let code_w = px(REPOS_CODE) as f32;
                let code_x = right - code_w;
                let letter = file.code.to_string();
                let tone = match file.conflicted {
                    true => p.error,
                    false => code_color(file.code, p),
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
                    tone,
                );
                let counts = (file.added, file.removed);
                let end =
                    self.diff_counts(c, None, counts, code_x - gap / 2.0, line_band, scale, p);
                // Before the counts, what was done to it here: comments
                // written on it, edits made from the diff.
                let mut end = end;
                if file.comments > 0 {
                    let text = match file.comments {
                        1 => "1 comment".to_owned(),
                        n => format!("{n} comments"),
                    };
                    end = self.tree_tag(c, &text, end - gap, line_band, p.accent, scale, p);
                }
                if file.edited {
                    end = self.tree_tag(c, "edited", end - gap, line_band, p.warning, scale, p);
                }
                // Viewed once, changed since: the box alone is easy to miss.
                if row.review && file.check == Check::Partial {
                    end = self.tree_tag(c, "changed", end - gap, line_band, p.warning, scale, p);
                }
                let name = match file.code {
                    _ if file.conflicted => p.error,
                    _ if file.viewed => p.muted,
                    'D' => p.muted,
                    _ => p.text,
                };
                let x = px(check.loc.x + check.size.w + REPOS_CODE_GAP) as f32;
                self.name_first(
                    c,
                    &file.path,
                    SMALL_FONT_SIZE,
                    (x, end - gap),
                    line_band,
                    scale,
                    (name, p.muted),
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
        let bold = |text: &str, size: f32| self.bold_width(text, size, scale);
        let spans = line_spans(repo, (x, right), gap, s, (&width, &bold));
        let end = spans.end;
        if let Some(cx) = spans.counts {
            let mut pen = cx;
            for (text, tone) in repo_counts(repo) {
                let color = tone_color(tone, p);
                pen = self.mixed_text(
                    c,
                    &text,
                    SMALL_FONT_SIZE,
                    pen,
                    f32::MAX,
                    line_band,
                    scale,
                    color,
                );
                pen += count_gap(s);
            }
        }
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
        self.bold_text(
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
            let (top, height) = line_band;
            let icon_w = (BRANCH_ICON as f32 * s).round();
            c.paint_in(
                bx as i32,
                top,
                icon_w as i32 + 1,
                height,
                branch_color,
                |px, py| {
                    let (cx, cy) = (bx + icon_w / 2.0, top as f32 + height as f32 / 2.0);
                    coverage(git_branch(px - cx, py - cy, s))
                },
            );
            self.mixed_text(
                c,
                branch,
                SMALL_FONT_SIZE,
                bx + branch_icon(gap, s),
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

    /// The detail pane's title row in `rect`, the pane's: its title, its
    /// buttons in `title_buttons` and the search's in `search`, and its
    /// tabs in `tabs`, a line under it.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn detail_title(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        rect: Rect,
        tabs: &[(DetailTab, Rect)],
        (title_buttons, search): (&[(ReposButton, Rect)], &[(SearchPart, Rect)]),
        scale: f64,
        p: &Palette,
    ) {
        // Preview gives the detail no width: an ellipsis would be all it fits.
        if rect.size.w <= 0 {
            return;
        }
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
        // The search open in its place, or its button left of the others.
        if let Some((_, first)) = search.first() {
            right = right.min(px(first.loc.x) as f32 - (FIELD_GAP as f32 * s).round());
        }
        let title_text = match search.iter().any(|(part, _)| *part == SearchPart::Field) {
            true => "",
            false => &view.detail.title,
        };
        self.mixed_text(
            c,
            title_text,
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
                self.hint_button(c, view, &without_key(hint), hit, rect, scale, p);
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
    }

    /// The detail's rows of `view` as `layout` has them, moved by `by`
    /// onto the buffer: those scrolled cut to where they show, the header
    /// pinned at the top over them with its shadow, then the scrollbar.
    #[cfg(test)]
    fn detail_rows(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        layout: &ReposLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let pane = moved(layout.detail, by);
        let border = px(1).max(1);
        // Tints reach across the pane, short of its edges.
        let tint = (
            px(pane.loc.x) + border,
            px(pane.loc.x + pane.size.w) - border,
        );
        let pinned = usize::from(view.detail.sticky);
        let rows: Vec<(usize, Rect)> = layout
            .detail_rows
            .iter()
            .map(|r| moved(*r, by))
            .enumerate()
            .collect();
        let (lx, ly, lw, lh) = rect_px(moved(layout.detail_list, by), scale);
        c.clipped(lx as i32, ly as i32, lw as i32, lh as i32, |c| {
            for &(i, rect) in rows[pinned.min(rows.len())..].iter().chain(&rows[..pinned]) {
                self.detail_line(c, &detail_look(view, i), rect, tint, scale, p);
            }
            if let (true, Some(&(_, head))) = (view.detail.stuck, rows.first()) {
                let (y, h) = band(head, scale);
                self.pin_shadow(c, tint, y + h - border, scale, p);
            }
        });
        let (track, shown) = detail_bar(view, layout);
        let at = (view.detail.first, shown, view.detail.total);
        scrollbar(c, moved(track, by), at, scale, p);
        self.search_ticks(
            c,
            &view.detail.ticks,
            moved(track, by),
            p.elevated,
            scale,
            p,
        );
    }

    /// The detail's row `look` in `rect`, its tint across the pane from
    /// `tint.0` to `tint.1` (physical pixels), with its buttons.
    fn detail_line(
        &self,
        c: &mut Canvas,
        look: &DetailLook,
        rect: Rect,
        tint: (i32, i32),
        scale: f64,
        p: &Palette,
    ) {
        self.detail_row(c, look, rect, tint, scale, p);
        let rects = line_chip_rects(
            rect,
            Some(&look.row),
            (look.gutter, look.plain),
            look.buttons.iter().map(row_chip_width),
        );
        self.row_chips(c, look, &rects, scale, p);
    }

    /// A detail row across a pane `size` big, in a buffer of its own.
    fn detail_row_buffer(
        &self,
        look: &DetailLook,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let border = ((scale.round() as i32).max(1)).min(c.w);
        let rect = Rectangle::new(
            (HELP_PADDING, 0).into(),
            (body_width(size.w), size.h).into(),
        );
        let tint = (border, c.w - border);
        self.detail_line(&mut c, look, rect, tint, scale, p);
        c.into_buffer(false)
    }

    /// Under the header pinned at the top of the diff, from `x.0` to `x.1`
    /// and from `y` down (physical pixels): a line, then the shadow it
    /// casts on what scrolls under it.
    fn pin_shadow(&self, c: &mut Canvas, (x0, x1): (i32, i32), y: i32, scale: f64, p: &Palette) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let border = px(1).max(1);
        c.fill(x0, y, x1 - x0, border, p.border);
        let depth = px(PIN_SHADOW - 1).max(1);
        for dy in 0..depth {
            let fade = 1.0 - dy as f32 / depth as f32;
            for x in x0..x1 {
                c.blend(x, y + border + dy, [0, 0, 0], p.shadow * 0.35 * fade * fade);
            }
        }
    }

    /// The pinned header's line and shadow across a pane `w` wide, in a
    /// buffer of its own.
    fn pin_shadow_buffer(&self, w: i32, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical((w, PIN_SHADOW).into(), scale));
        let border = ((scale.round() as i32).max(1)).min(c.w);
        let x1 = c.w - border;
        self.pin_shadow(&mut c, (border, x1), 0, scale, p);
        c.into_buffer(false)
    }

    /// `text` from `x`, cut at `max_x`, in `color` but for the places
    /// the search `found` in it (by char index): tinted, in the text's own
    /// colour. Returns where it ends.
    #[allow(clippy::too_many_arguments)]
    fn found_text(
        &self,
        c: &mut Canvas,
        text: &str,
        found: &[(Range<usize>, bool)],
        size: f32,
        (x, max_x): (f32, f32),
        band: (i32, i32),
        scale: f64,
        color: Rgb,
        p: &Palette,
    ) -> f32 {
        let width = |t: &str| self.mixed_width(t, size, scale);
        self.found_tints(
            c,
            text,
            found,
            (x, max_x),
            band,
            width,
            p.elevated,
            scale,
            p,
        );
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(b, _)| b);
        let mut pen = x;
        for (part, on) in found_split(0..text.chars().count(), found) {
            let piece = &text[byte(part.start)..byte(part.end)];
            let color = if on { p.text } else { color };
            pen = self.mixed_text(c, piece, size, pen, max_x, band, scale, color);
        }
        pen
    }

    /// A row of the detail in `rect`; its tint across the pane from
    /// `tint.0` to `tint.1` (physical pixels). A repository's header and a
    /// file's are banded, a hunk is a rule with what it is in, an added or
    /// removed line is tinted with a bar at its left and its changed words
    /// tinted more; the places the search found tinted too.
    fn detail_row(
        &self,
        c: &mut Canvas,
        look: &DetailLook,
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
        let (row, mark) = (&look.row, look.mark);
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
                across(c, mix(p.elevated, p.text, HEADER_TINT));
                c.fill(tint_x, y, tint_end - tint_x, border, p.border_variant);
                c.fill(tint_x, y + h - border, tint_end - tint_x, border, p.border);
                let files = files_text(repo.files);
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
                let mut pen = self.bold_text(
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
                        (BRANCH_ICON, rect.size.h).into(),
                    );
                    if pen + branch_icon(gap, s) < right {
                        icon_sdf(c, icon, scale, p.muted, |x, y| git_branch(x, y, s));
                        pen += branch_icon(gap, s);
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
                let hovered = look.hover.is_some();
                let fill = if file.selected {
                    mix(p.elevated, p.selected, 0.75)
                } else if hovered {
                    mix(p.elevated, p.selected, 0.45)
                } else {
                    mix(p.elevated, p.text, 0.035)
                };
                across(c, fill);
                c.fill(tint_x, y, tint_end - tint_x, border, p.border_variant);
                if file.selected && look.focused {
                    c.fill(tint_x, y, px(2), h, p.accent);
                }
                let fold = Rectangle::new(rect.loc, (DIFF_FOLD, rect.size.h).into());
                let color = if look.hover == Some(ReposHit::DiffFold(0)) {
                    p.text
                } else {
                    p.muted
                };
                icon_sdf(c, fold, scale, color, |x, y| {
                    expander_mark(!file.folded, x, y, s)
                });
                // Its checkbox, when files are checked here.
                if let Some(check) = file.check {
                    let side = CHECKBOX_SIZE;
                    let box_rect = Rectangle::new(
                        (rect.loc.x + DIFF_FOLD + (DIFF_CHECK - side) / 2, rect.loc.y).into(),
                        (side, rect.size.h).into(),
                    );
                    let check_hovered = look.hover == Some(ReposHit::DiffCheck(0));
                    self.checkbox(c, box_rect, check, check_hovered, scale, p);
                }
                // The status letter on a pill of its colour.
                let tone = code_color(file.code, p);
                let (bw, bh) = (px(DIFF_BADGE) as f32, px(DIFF_BADGE - 2) as f32);
                let check_w = if file.check.is_some() { DIFF_CHECK } else { 0 };
                let badge_x = px(rect.loc.x + DIFF_FOLD + check_w) as f32;
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
                    file.note.as_deref(),
                    (file.added, file.removed),
                    bend,
                    row_band,
                    scale,
                    p,
                );
                let right = counts - gap;
                let mut pen = badge_x + bw + gap;
                if let Some(repo) = &file.repo {
                    pen = self.bold_text(
                        c,
                        repo,
                        SMALL_FONT_SIZE,
                        pen,
                        right,
                        row_band,
                        scale,
                        p.text,
                    );
                    pen = self.mixed_text(
                        c,
                        " ›",
                        SMALL_FONT_SIZE,
                        pen,
                        right,
                        row_band,
                        scale,
                        p.muted,
                    );
                    pen += (gap / 2.0).round();
                }
                // Name first, then its folder; what the search found in
                // either tinted (`search::path_text` joins them so).
                let (name, rest) = name_first(&file.path);
                let n = name.chars().count();
                let in_name = found_within(&look.found, 0..n);
                let pen = self.found_text(
                    c,
                    name,
                    &in_name,
                    FONT_SIZE,
                    (pen, right),
                    row_band,
                    scale,
                    p.text,
                    p,
                );
                if !rest.is_empty() && pen + gap < right {
                    let in_rest = found_within(&look.found, n + 1..usize::MAX);
                    self.found_text(
                        c,
                        &rest,
                        &in_rest,
                        FONT_SIZE,
                        (pen + gap, right),
                        row_band,
                        scale,
                        p.muted,
                        p,
                    );
                }
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
                let width = |t: &str| self.mono_width(t, SMALL_FONT_SIZE, scale);
                self.found_tints(
                    c,
                    &label,
                    &look.found,
                    (tx, bend),
                    row_band,
                    width,
                    p.elevated,
                    scale,
                    p,
                );
                // What the search found in it in the text's own colour.
                let mut end = tx;
                let chars = label.chars().count();
                let byte = |n: usize| label.char_indices().nth(n).map_or(label.len(), |(b, _)| b);
                for (part, found) in found_split(0..chars, &look.found) {
                    let text = &label[byte(part.start)..byte(part.end)];
                    let color = if found { p.text } else { p.muted };
                    end =
                        self.mono_text(c, text, SMALL_FONT_SIZE, end, bend, row_band, scale, color);
                }
                let from = (end + (gap / 2.0).round()) as i32;
                if (from as f32) < bend {
                    c.fill(from, mid, bend as i32 - from, border, p.border_variant);
                }
            }
            DetailRow::Line(line) => {
                let tone = match line.kind {
                    _ if look.plain => None,
                    LineKind::Added => Some(p.success),
                    LineKind::Removed => Some(p.error),
                    LineKind::Context => None,
                };
                // After the line numbers (where it was, where it is).
                let gutter = px(gutter_width(look.gutter, look.plain)) as f32;
                let tx = bx + inset + gutter;
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
                let width = |t: &str| self.mono_width(t, REPOS_MONO_SIZE, scale);
                let text = &line.text;
                if let Some(chars) = &look.chosen {
                    self.chosen_tint(c, text, chars, (tx, bend), row_band, width, scale, p);
                }
                self.found_tints(
                    c,
                    text,
                    &look.found,
                    (tx, bend),
                    row_band,
                    width,
                    p.elevated,
                    scale,
                    p,
                );
                if let Some(ch) = look.caret {
                    let ground = p.elevated;
                    self.caret_block(c, text, ch, (tx, bend), row_band, width, ground, scale, p);
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
                // In the places the search found, in the text's own colour
                // to read on their tint.
                let ends = bytes.len() - 1;
                let mut run = |from: usize, to: usize, color: Rgb| {
                    for (part, found) in found_split(from..to.min(ends), &look.found) {
                        run(part.start, part.end, if found { p.text } else { color });
                    }
                };
                let mut at = 0;
                for (range, kind) in &line.syntax {
                    run(at, range.start, p.text);
                    run(range.start, range.end, p.code(*kind));
                    at = range.end;
                }
                run(at, usize::MAX, p.text);
                if let Some(word) = &look.word {
                    let text = &line.text;
                    self.word_underline(c, text, word, (tx, bend), row_band, width, scale, p);
                }
                // The numbers: where it was (none for an added line), where
                // it is (none for a removed one), right-aligned in theirs.
                if look.gutter > 0 {
                    let columns = if look.plain { 1.0 } else { 2.0 };
                    let col = (gutter - 2.0 * px(GUTTER_GAP) as f32) / columns;
                    let x0 = bx + inset;
                    // A page's lines have the one number.
                    let numbers = match look.plain {
                        true => [Some(line.number), None],
                        false => [
                            (line.kind != LineKind::Added).then_some(line.old),
                            (line.kind != LineKind::Removed).then_some(line.number),
                        ],
                    };
                    for (k, n) in numbers.into_iter().enumerate() {
                        let Some(n) = n.filter(|&n| n > 0) else {
                            continue;
                        };
                        let text = n.to_string();
                        let w = self.mono_width(&text, SMALL_FONT_SIZE, scale);
                        let right = x0 + (k as f32 + 1.0) * col + k as f32 * px(GUTTER_GAP) as f32;
                        let color = p.placeholder;
                        let x = (right - w).round();
                        self.mono_text(
                            c,
                            &text,
                            SMALL_FONT_SIZE,
                            x,
                            right + 1.0,
                            row_band,
                            scale,
                            color,
                        );
                    }
                }
                // Cut at the row's end, or shown from a char on: marked.
                let mark_w = (self.mono_width("›", REPOS_MONO_SIZE, scale)).ceil();
                let ground = match tone {
                    Some(tone) => lighten(mix(p.elevated, tone, DIFF_TINT)),
                    None if lit.is_some() => lighten(p.elevated),
                    None => p.elevated,
                };
                if tx + width(&line.text) > bend {
                    let x = bend - mark_w;
                    c.fill(x as i32, y, mark_w as i32 + 1, h, ground);
                    self.mono_text(
                        c,
                        "›",
                        REPOS_MONO_SIZE,
                        x,
                        f32::MAX,
                        row_band,
                        scale,
                        p.muted,
                    );
                }
                if look.skipped {
                    let x = tx - mark_w;
                    c.fill(x as i32, y, mark_w as i32, h, ground);
                    self.mono_text(c, "‹", REPOS_MONO_SIZE, x, tx, row_band, scale, p.muted);
                }
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
            DetailRow::CommentHead {
                who,
                finding,
                place,
                within,
            } => {
                let room = self.finding_around(
                    c,
                    *within,
                    (bx + inset, bend),
                    row_band,
                    false,
                    lit.is_some(),
                    scale,
                    p,
                );
                self.comment_head(
                    c,
                    room,
                    row_band,
                    (who, finding.as_ref(), place),
                    lit.is_some(),
                    scale,
                    p,
                );
            }
            DetailRow::Comment {
                text,
                first,
                last,
                tone,
                within,
            } => {
                let (x0, x1) = self.finding_around(
                    c,
                    *within,
                    (bx + inset, bend),
                    row_band,
                    *last,
                    lit.is_some(),
                    scale,
                    p,
                );
                // A finding's card in its severity's colour.
                let accent = tone.map_or(p.accent, |t| tone_color(t, p));
                let fill = mix(p.elevated, accent, if lit.is_some() { 0.2 } else { 0.1 });
                let tx = self.comment_card(c, (x0, x1), row_band, (*first, *last), fill, scale);
                c.fill(x0 as i32, y, px(2).max(1), h, mix(p.elevated, accent, 0.7));
                // What the search found in it, tinted on the card.
                let width = |t: &str| self.mono_width(t, REPOS_MONO_SIZE, scale);
                let found = &look.found;
                self.found_tints(c, text, found, (tx, bend), row_band, width, fill, scale, p);
                // A dismissed finding's words dim.
                let ink = match tone {
                    Some(Tone::Muted) => p.muted,
                    _ => p.text,
                };
                self.mono_text(c, text, REPOS_MONO_SIZE, tx, bend, row_band, scale, ink);
            }
            DetailRow::Editor {
                text,
                caret,
                selection,
                first,
                last,
                within,
            } => {
                let (x0, x1) = self.finding_around(
                    c,
                    *within,
                    (bx + inset, bend),
                    row_band,
                    *last,
                    lit.is_some(),
                    scale,
                    p,
                );
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
                if let Some(caret) = caret.filter(|_| look.focused) {
                    let cx = at(caret).min(x1 - b) as i32;
                    let pad = px(3);
                    c.fill(cx, y + pad, px(1).max(1), h - 2 * pad, p.accent);
                }
            }
        }
        // The rows picked outlined as one, a bar down their left.
        if picked {
            let color = if look.focused { p.accent } else { p.muted };
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

    /// The buttons of the detail's row `look`, each in its rect of
    /// `rects`: the first of them (keeping the comment written, writing
    /// one) in the accent, the others plain; lighter under the pointer.
    fn row_chips(
        &self,
        c: &mut Canvas,
        look: &DetailLook,
        rects: &[Rect],
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        for (k, (hint, &rect)) in look.buttons.iter().zip(rects).enumerate() {
            let (x, y, w, h) = rect_px(rect, scale);
            let r = (h / 2.0).min(BUTTON_RADIUS * s);
            let b = s.round().max(1.0);
            let hovered = look.hover == Some(ReposHit::Button(hint.button));
            let (fill, label) = if k == 0 {
                let fill = if hovered {
                    mix(p.accent, p.on_accent, 0.15)
                } else {
                    p.accent
                };
                (fill, p.on_accent)
            } else {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                (fill, p.text)
            };
            if k == 0 {
                c.rounded(x, y, w, h, r, fill);
            } else {
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
            }
            let chip_band = band(rect, scale);
            let tx = x + (ROW_CHIP_PAD as f32 * s).round();
            self.mixed_text(
                c,
                &hint.label,
                SMALL_FONT_SIZE,
                tx,
                x + w,
                chip_band,
                scale,
                label,
            );
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

    /// With `within`, the row of a finding's card from `x.0` to `x.1` that
    /// holds the user's comment on it, in its tone, the card's end at `last`.
    /// Returns where the comment's own card goes.
    #[allow(clippy::too_many_arguments)]
    fn finding_around(
        &self,
        c: &mut Canvas,
        within: Option<Tone>,
        (x0, x1): (f32, f32),
        row_band: (i32, i32),
        last: bool,
        lit: bool,
        scale: f64,
        p: &Palette,
    ) -> (f32, f32) {
        let Some(tone) = within else {
            return (x0, x1);
        };
        let s = scale as f32;
        let accent = tone_color(tone, p);
        let fill = mix(p.elevated, accent, if lit { 0.2 } else { 0.1 });
        self.comment_card(c, (x0, x1), row_band, (false, last), fill, scale);
        c.fill(
            x0 as i32,
            row_band.0,
            (2.0 * s).round().max(1.0) as i32,
            row_band.1,
            mix(p.elevated, accent, 0.7),
        );
        let pad = (COMMENT_INDENT as f32 * s).round();
        (x0 + pad, x1 - pad)
    }

    /// A review comment's heading, the first row of its card, says whose
    /// it is at a glance (`who`): on a finding, a band in its severity's
    /// colour, a solid "AI review" tag, the severity and where it stands; on
    /// the user's own, the plain card and "Your comment". Then its `place`.
    #[allow(clippy::too_many_arguments)]
    fn comment_head(
        &self,
        c: &mut Canvas,
        (x0, x1): (f32, f32),
        row_band: (i32, i32),
        (who, finding, place): (&str, Option<&FindingHead>, &str),
        lit: bool,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (y, h) = row_band;
        let accent = finding.map_or(p.accent, |f| tone_color(f.tone, p));
        // Grey takes less of a band to show, and the words stay readable.
        let band_tint = match finding.map(|f| f.tone) {
            Some(Tone::Normal | Tone::Muted) => COMMENT_HEAD_TINT - 0.08,
            Some(_) => COMMENT_HEAD_TINT,
            None => 0.1,
        };
        let strength = band_tint + if lit { 0.08 } else { 0.0 };
        let fill = mix(p.elevated, accent, strength);
        let mut pen = self.comment_card(c, (x0, x1), row_band, (true, false), fill, scale);
        c.fill(
            x0 as i32,
            y,
            (2.0 * s).round().max(1.0) as i32,
            h,
            mix(p.elevated, accent, 0.7),
        );
        let word = who;
        let Some(finding) = finding else {
            self.bold_text(c, word, SMALL_FONT_SIZE, pen, x1, row_band, scale, p.text);
            pen += self.bold_width(word, SMALL_FONT_SIZE, scale) + COMMENT_HEAD_GAP as f32 * s;
            self.mixed_text(c, place, SMALL_FONT_SIZE, pen, x1, row_band, scale, p.muted);
            return;
        };
        let pad = (COMMENT_TAG_PAD as f32 * s).round();
        let w = self.bold_width(word, SMALL_FONT_SIZE, scale) + 2.0 * pad;
        let tag_h = (h as f32 - 2.0 * s.round()).max(1.0);
        let tag_y = (y as f32 + (h as f32 - tag_h) / 2.0).round();
        c.rounded(pen, tag_y, w, tag_h, CHIP_RADIUS * s, accent);
        let ink = comment_tag_ink(finding.tone, p);
        let tag_band = (tag_y as i32, tag_h as i32);
        self.bold_text(
            c,
            word,
            SMALL_FONT_SIZE,
            pen + pad,
            x1,
            tag_band,
            scale,
            ink,
        );
        pen += w + COMMENT_HEAD_GAP as f32 * s;
        let words = comment_head_word(finding.tone, p);
        self.bold_text(
            c,
            &finding.severity,
            SMALL_FONT_SIZE,
            pen,
            x1,
            row_band,
            scale,
            words,
        );
        pen += self.bold_width(&finding.severity, SMALL_FONT_SIZE, scale);
        let rest = match &finding.state {
            Some(state) => format!(" \u{b7} {state} \u{b7} {place}"),
            None => format!(" \u{b7} {place}"),
        };
        self.mixed_text(c, &rest, SMALL_FONT_SIZE, pen, x1, row_band, scale, words);
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
        let gap = count_gap(scale as f32);
        let parts = count_parts(lead.map(str::to_owned), (added, removed));
        let widths: Vec<f32> = parts
            .iter()
            .map(|(t, _)| self.mixed_width(t, size, scale))
            .collect();
        let total = widths.iter().sum::<f32>() + gap * parts.len().saturating_sub(1) as f32;
        let left = (right - total).round();
        let mut x = left;
        for ((text, tone), w) in parts.iter().zip(widths) {
            let color = tone_color(*tone, p);
            self.mixed_text(c, text, size, x, f32::MAX, band, scale, color);
            x += w + gap;
        }
        left
    }

    /// The detail pane's `tab` segment in `rect`: an outlined pill, filled
    /// when it is the tab shown, lighter under the pointer.
    fn tree_title(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        tree: Rect,
        tools: &[(TreeTool, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let title = Rectangle::new(tree.loc, (tree.size.w, REPOS_DETAIL_TITLE).into());
        let (x, mut right) = phys(title, scale);
        if let Some((_, first)) = tools.first() {
            right = px(first.loc.x) as f32 - (FIELD_GAP as f32 * s).round();
        }
        let x = x + px(HELP_PADDING) as f32;
        let (text, color) = match &view.tree_note {
            Some((text, Tone::Success)) => (text.as_str(), p.success),
            Some((text, Tone::Warning)) => (text.as_str(), p.warning),
            Some((text, _)) => (text.as_str(), p.text),
            None => ("Changes", p.text),
        };
        self.mixed_text(c, text, FONT_SIZE, x, right, band(title, scale), scale, color);
        for &(tool, rect) in tools {
            self.tool_button(c, view, tool, rect, scale, p);
        }
        let border = px(1).max(1);
        c.fill(
            px(tree.loc.x) + border,
            px(tree.loc.y + REPOS_DETAIL_TITLE) - border,
            px(tree.loc.x + tree.size.w) - px(tree.loc.x) - border,
            border,
            p.border_variant,
        );
    }

    /// A button of the tree's toolbar in `rect`: a way of listing the
    /// files lit while they are listed so.
    fn tool_button(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        tool: TreeTool,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let on = match tool {
            TreeTool::List => view.grouping == Grouping::List,
            TreeTool::Folders => view.grouping == Grouping::Folders,
            _ => false,
        };
        // With every file viewed there is nowhere to go.
        let off = matches!(tool, TreeTool::PrevUnviewed | TreeTool::NextUnviewed)
            && view.tree_nav == Some(false);
        let fill = if on {
            p.selected
        } else if !off && view.hover == Some(ReposHit::Tool(tool)) {
            mix(p.element, p.selected, 0.5)
        } else {
            p.element
        };
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let color = match (on, off) {
            (true, _) => p.text,
            (_, true) => p.placeholder,
            _ => p.muted,
        };
        match tool {
            TreeTool::PrevUnviewed => icon_sdf(c, rect, scale, color, |x, y| arrow(true, x, y, s)),
            TreeTool::NextUnviewed => icon_sdf(c, rect, scale, color, |x, y| arrow(false, x, y, s)),
            TreeTool::Expand => {
                icon_sdf(c, rect, scale, color, |x, y| fold_all_mark(true, x, y, s))
            }
            TreeTool::Collapse => {
                icon_sdf(c, rect, scale, color, |x, y| fold_all_mark(false, x, y, s))
            }
            TreeTool::List => icon_sdf(c, rect, scale, color, |x, y| list_mark(x, y, s)),
            TreeTool::Folders => icon_sdf(c, rect, scale, color, |x, y| folder(x, y, 0.8 * s)),
        }
    }

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
            self.hint_button(c, view, &without_key(hint), hit, rect, scale, p);
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
            // The one to press next, filled (`mark_primary`).
            let primary = hint.style == ButtonStyle::Primary && !disabled;
            let (edge, inner, label_color) = match (primary, view.hover == Some(hit)) {
                (true, true) => {
                    let fill = mix(p.accent, p.text, 0.12);
                    (fill, fill, p.on_accent)
                }
                (true, false) => (p.accent, p.accent, p.on_accent),
                (false, _) if disabled => (edge, p.elevated, label_color),
                (false, _) => (edge, p.element, label_color),
            };
            c.rounded(bx, cap_y, bw, cap_h, cr, edge);
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

    /// The branch menu's card in `card` with its search in `search` and
    /// the row creating the branch typed in `create`, or what it says
    /// without rows; not its lines (see `menu_card_view`). `hover` is what
    /// the pointer is over.
    #[allow(clippy::too_many_arguments)]
    fn branch_menu_card(
        &self,
        c: &mut Canvas,
        menu: &BranchMenuView,
        hover: Option<ReposHit>,
        (card, search, create): (Rect, Rect, Option<Rect>),
        density: Density,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        card_over(c, card, MENU_RADIUS, scale, p);
        let (x, y, w, h) = rect_px(search, scale);
        let (b, r) = ((scale.round() as i32).max(1) as f32, BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.accent);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        // The magnifier every query field starts with, where the diff's
        // search has it.
        let mark = Rectangle::new(
            (search.loc.x + PADDING as i32 / 2, search.loc.y).into(),
            (MENU_MARK_WIDTH, search.size.h).into(),
        );
        icon_sdf(c, mark, scale, p.placeholder, |x, y| {
            magnifier(x, y, 0.9 * s)
        });
        self.edit_line(
            c,
            &menu.query,
            &menu.placeholder,
            search_field_text(search, scale),
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
            } else if hover == Some(ReposHit::BranchCreate) {
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
        if menu.rows.is_empty() && !menu.empty.is_empty() {
            let row = Rectangle::new(
                (
                    card.loc.x + BRANCH_MENU_PAD,
                    search.loc.y + search.size.h + BRANCH_MENU_PAD,
                )
                    .into(),
                (
                    (card.size.w - 2 * BRANCH_MENU_PAD).max(0),
                    repos_row(density),
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
        }
    }

    /// A line of the branch menu in `rect`: a group's heading, muted; or a
    /// branch, lit when Enter picks it, checked when they are all on it.
    fn branch_line(&self, c: &mut Canvas, look: &MenuLook, rect: Rect, scale: f64, p: &Palette) {
        let s = scale as f32;
        let pad = BRANCH_ROW_PAD as f32 * s;
        let row_band = band(rect, scale);
        let (label, picked, current, deletable, hover) = match look {
            MenuLook::Heading(label) => {
                let (x, end) = phys(rect, scale);
                let color = p.placeholder;
                self.text(
                    c,
                    label,
                    FONT_SIZE,
                    x + pad,
                    end - pad,
                    row_band,
                    scale,
                    color,
                );
                return;
            }
            MenuLook::Branch {
                label,
                picked,
                current,
                deletable,
                hover,
            } => (label, *picked, *current, *deletable, *hover),
        };
        let (x, y, w, h) = rect_px(rect, scale);
        let r = (MENU_RADIUS - 2.0) * s;
        let on_delete = hover == Some(ReposHit::BranchDelete(0));
        let hovered = hover.is_some();
        if picked {
            c.rounded(x, y, w, h, r, p.selected);
        } else if hovered {
            c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
        }
        let mark = branch_check(rect);
        if current {
            icon_sdf(c, mark, scale, p.accent, |x, y| check(x, y, s));
        }
        // The delete button shows on the row pointed at or picked.
        let delete = branch_delete(rect);
        if deletable && (hovered || picked) {
            let color = if on_delete { p.error } else { p.placeholder };
            icon_sdf(c, delete, scale, color, |x, y| trash(x, y, s));
        }
        let end = (delete.loc.x as f64 * scale) as f32;
        self.mixed_text(c, label, FONT_SIZE, x + pad, end, row_band, scale, p.text);
    }

    /// A line of the branch menu `size` big, in a buffer of its own.
    fn branch_line_buffer(
        &self,
        look: &MenuLook,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        self.branch_line(&mut c, look, Rectangle::from_size(size), scale, p);
        c.into_buffer(false)
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
        // The button going ahead, on the right, filled; the one Enter
        // presses ringed.
        let last = sheet.buttons.len().saturating_sub(1);
        for &(i, rect) in &parts.sheet_buttons {
            let button = sheet.buttons[i];
            let hovered = view.hover == Some(ReposHit::Button(button));
            let rect = shift(rect);
            let focused = i == sheet.default;
            let style = match i == last {
                true if sheet.danger => ButtonStyle::Destructive,
                true => ButtonStyle::Primary,
                false => ButtonStyle::Plain,
            };
            self.button(c, button.label(), rect, (hovered, focused), style, scale, p);
        }
    }
}

/// What of `view` the panel's frame is drawn from: all but the tree's
/// lines and the detail's rows and where they stand, the branch menu (but
/// that it is open), the sheet, and the pointer on what those show.
/// `empty` is what the tree says, empty while it has lines.
fn frame_view(view: &ReposView) -> ReposView {
    use ReposHit::*;
    let on_row = |b: ReposButton| view.detail.row_buttons.iter().any(|(_, h)| h.button == b);
    let hover = view.hover.filter(|hit| match *hit {
        Button(b) => !on_row(b),
        hit => !matches!(
            hit,
            Line(_)
                | Check(_)
                | Expander(_)
                | Chip(_)
                | Mark(..)
                | DiffRow(_)
                | DiffCheck(_)
                | DiffFold(_)
                | DiffRestore(_)
                | BranchRow(_)
                | BranchDelete(_)
                | BranchCreate
                | BranchMenu
        ),
    });
    let detail = &view.detail;
    let menu = view.branch.menu.as_ref().map(|_| BranchMenuView::default());
    ReposView {
        title: view.title.clone(),
        summary: view.summary.clone(),
        summary_tone: view.summary_tone,
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
        detail: DetailView {
            tab: detail.tab,
            title: detail.title.clone(),
            title_buttons: detail.title_buttons.clone(),
            focused: detail.focused,
            search: detail.search.clone(),
            ..DetailView::default()
        },
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
        tree_note: view.tree_note.clone(),
        grouping: view.grouping,
        diff_only: view.diff_only,
        diff_tree: view.diff_tree,
        unchecked: view.unchecked,
        tree_nav: view.tree_nav,
        flat: view.flat,
        no_header: view.no_header,
        detail_share: view.detail_share,
        modes: view.modes.clone(),
    }
}

/// The frame's inputs: the view as `frame_view` has it, the panel, the
/// theme and the scale.
type FrameKey = (ReposView, Rect, super::Theme, f64);
/// A tree line's: the line as drawn, its size, the theme and the scale.
type TreeKey = (TreeRow, Size<i32, Logical>, super::Theme, f64);
/// A detail row's: the row as drawn, its size, the theme and the scale.
type DetailKey = (DetailLook, Size<i32, Logical>, super::Theme, f64);
/// A scrollbar thumb's, and the pinned header's shadow's: its size, the
/// theme and the scale.
type ThumbKey = (Size<i32, Logical>, super::Theme, f64);
/// The search's ticks on the detail's scrollbar: the ticks, the track's
/// size, the theme and the scale.
type TicksKey = (Vec<(f64, bool)>, Size<i32, Logical>, super::Theme, f64);
/// The branch menu's card's: the menu as `menu_card_view` has it, what
/// the pointer is over on it, the card, the density, the theme and the
/// scale.
type MenuKey = (
    BranchMenuView,
    Option<ReposHit>,
    Rect,
    Density,
    super::Theme,
    f64,
);
/// A branch menu line's: the line as drawn, its size, the theme and the
/// scale.
type MenuLineKey = (MenuLook, Size<i32, Logical>, super::Theme, f64);
/// The sheet's: the sheet, what the pointer is over, its card, the panel,
/// the theme and the scale.
type SheetKey = (
    SheetView,
    Option<ReposHit>,
    Option<Rect>,
    Rect,
    super::Theme,
    f64,
);

/// The repositories panel as the chrome shows it, fading in and out (see
/// `Overlay`): its frame; over it each of the tree's lines and of the
/// detail's rows in a buffer of its own, the detail's pinned header with
/// its shadow and the thumbs of their scrollbars, so that scrolling them
/// only moves those; then the branch menu's card and over it its lines,
/// each its own buffer too; the sheet over all.
#[derive(Default)]
pub(super) struct ReposLayers {
    overlay: super::Overlay<FrameKey>,
    tree: super::RowCache<TreeKey>,
    tree_thumb: super::Cached<ThumbKey>,
    detail: super::RowCache<DetailKey>,
    detail_thumb: super::Cached<ThumbKey>,
    detail_ticks: super::Cached<TicksKey>,
    pin_shadow: super::Cached<ThumbKey>,
    menu: super::Cached<MenuKey>,
    menu_lines: super::RowCache<MenuLineKey>,
    sheet: super::Cached<SheetKey>,
    /// How many times the frame and the branch menu's card were drawn,
    /// for tests.
    #[cfg(test)]
    frames: usize,
    #[cfg(test)]
    cards: usize,
}

impl ReposLayers {
    /// The parts showing the repositories panel at `now`, front to back.
    pub(super) fn parts(
        &mut self,
        view: Option<ReposView>,
        panel: Rect,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
    ) -> Vec<super::Part> {
        let placed = view.as_ref().map(|view| {
            let (loc, size) = super::shadowed(panel);
            ((frame_view(view), panel, theme, scale), loc, size)
        });
        let Self {
            overlay,
            tree,
            tree_thumb,
            detail,
            detail_thumb,
            detail_ticks,
            pin_shadow,
            menu,
            menu_lines,
            sheet,
            #[cfg(test)]
            frames,
            #[cfg(test)]
            cards,
        } = self;
        let p = view.as_ref().map_or(theme.palette(), |v| look(v, theme));
        let draw = |(view, panel, ..): &FrameKey| {
            #[cfg(test)]
            {
                *frames += 1;
            }
            let laid = parts(view, *panel);
            painter
                .repos_frame(view, &laid, scale, p)
                .into_buffer(false)
        };
        let over = |_: &FrameKey| {
            let Some(view) = &view else {
                return Vec::new();
            };
            let laid = parts(view, panel);
            let layout = &laid.layout;
            let mut over = Vec::new();
            if let Some(s) = &view.sheet {
                let (loc, size) = super::shadowed(layout.panel);
                let key = (s.clone(), view.hover, layout.sheet, panel, theme, scale);
                let buffer = sheet.get(key, |_| {
                    let mut c = Canvas::new(physical(size, scale));
                    let by = buffer_origin(layout.panel);
                    painter.sheet(&mut c, view, s, &laid, |r| moved(r, by), scale, p);
                    c.into_buffer(false)
                });
                over.push(super::Part::new(buffer, loc, size, 1.0));
            }
            if let (Some(m), Some((card, rows))) = (&view.branch.menu, &layout.branch_menu) {
                let list = layout.branch_list.unwrap_or(*card);
                let row_h = repos_row(view.density);
                let shift = scrolled_by(m.part, m.over, row_h);
                for (j, (item, rect)) in menu_rects(m, layout, rows).into_iter().enumerate() {
                    let key = (menu_look(view, m, item), rect.size, theme, scale);
                    let buffer = menu_lines.get(key, rect.size, scale, |(look, size, ..)| {
                        painter.branch_line_buffer(look, *size, scale, p)
                    });
                    let y = (list.loc.y + j as i32 * row_h) as f64 - shift;
                    let at = Point::from((rect.loc.x as f64, y));
                    let part = super::Part::new(&buffer, rect.loc, rect.size, 1.0).at(at);
                    over.push(part.clipped(list));
                }
                let (loc, size) = super::shadowed(*card);
                let hover = view.hover.filter(|h| *h == ReposHit::BranchCreate);
                let key = (menu_card_view(m), hover, *card, view.density, theme, scale);
                let buffer = menu.get(key, |_| {
                    #[cfg(test)]
                    {
                        *cards += 1;
                    }
                    let mut c = Canvas::new(physical(size, scale));
                    let by = Point::from((SHADOW - card.loc.x, SHADOW - card.loc.y));
                    painter.repos_menu_card(&mut c, view, layout, by, scale, p);
                    c.into_buffer(false)
                });
                over.push(super::Part::new(buffer, loc, size, 1.0));
            }
            over.extend(detail_parts(
                view,
                layout,
                (detail, detail_thumb, detail_ticks, pin_shadow),
                (theme, scale),
                painter,
            ));
            let (track, shown) = tree_bar(view, layout);
            let pos = view.first as f64 + view.part;
            if let Some(t) = rows_thumb(track, pos, shown, view.total) {
                let size = Size::from((SCROLLBAR, t.h as i32));
                let buffer =
                    tree_thumb.get((size, theme, scale), |_| painter.bar_thumb(size, scale, p));
                let at = Point::from((track.loc.x as f64, t.top));
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
        // Laid flat into a tab, it comes and goes with the tab, at once.
        let flat = view.as_ref().map_or(overlay.instant, |view| view.flat);
        overlay.instant = flat;
        let parts = overlay.parts(placed, now, !flat, draw, over);
        tree.sweep();
        detail.sweep();
        menu_lines.sweep();
        if !overlay.lingers() {
            tree.clear();
            detail.clear();
            menu_lines.clear();
        }
        parts
    }
}

/// The colours `view` is drawn in: laid flat into a tab it is a page.
fn look(view: &ReposView, theme: super::Theme) -> &'static Palette {
    if view.flat {
        theme.page_palette()
    } else {
        theme.palette()
    }
}

/// The detail's parts over the frame, front to back: the search's ticks
/// on its scrollbar, its thumb, the pinned header's shadow and the
/// header, and its rows, each of its own buffer from `caches` (the
/// rows', the thumb's, the ticks' and the shadow's).
fn detail_parts(
    view: &ReposView,
    layout: &ReposLayout,
    (rows, thumb, ticks, shadow): (
        &mut super::RowCache<DetailKey>,
        &mut super::Cached<ThumbKey>,
        &mut super::Cached<TicksKey>,
        &mut super::Cached<ThumbKey>,
    ),
    (theme, scale): (super::Theme, f64),
    painter: &Painter,
) -> Vec<super::Part> {
    use super::Part;
    let p = look(view, theme);
    let mut parts = Vec::new();
    let detail = &view.detail;
    let (track, shown) = detail_bar(view, layout);
    if detail.tab == DetailTab::Diff && !detail.ticks.is_empty() {
        let key = (detail.ticks.clone(), track.size, theme, scale);
        let buffer = ticks.get(key, |(t, size, ..)| {
            painter.search_ticks_buffer(t, *size, p.elevated, scale, p)
        });
        parts.push(Part::new(buffer, track.loc, track.size, 1.0));
    }
    let pos = detail.first as f64 + detail.part;
    if let Some(t) = rows_thumb(track, pos, shown, detail.total) {
        let size = Size::from((SCROLLBAR, t.h as i32));
        let buffer = thumb.get((size, theme, scale), |_| painter.bar_thumb(size, scale, p));
        let at = Point::from((track.loc.x as f64, t.top));
        parts.push(Part::new(buffer, track.loc, size, 1.0).at(at));
    }
    let list = layout.detail_list;
    let line_h = repos_detail_line(view.density);
    let size = Size::from((layout.detail.size.w, line_h));
    let x = layout.detail.loc.x;
    let pinned = usize::from(detail.sticky);
    let mut row = |i: usize, y: f64| {
        let key = (detail_look(view, i), size, theme, scale);
        let buffer = rows.get(key, size, scale, |(look, size, ..)| {
            painter.detail_row_buffer(look, *size, scale, p)
        });
        let at = Point::from((x as f64, y));
        Part::new(&buffer, list.loc, size, 1.0).at(at).clipped(list)
    };
    if pinned == 1 && !layout.detail_rows.is_empty() {
        let y = list.loc.y as f64 + detail.pin;
        if detail.stuck {
            let shadow_size = Size::from((size.w, PIN_SHADOW));
            let buffer = shadow.get((shadow_size, theme, scale), |_| {
                painter.pin_shadow_buffer(size.w, scale, p)
            });
            let at = Point::from((x as f64, y + (line_h - 1) as f64));
            let part = Part::new(buffer, list.loc, shadow_size, 1.0).at(at);
            parts.push(part.clipped(list));
        }
        parts.push(row(0, y));
    }
    let shift = scrolled_by(detail.part, detail.over, line_h);
    for i in pinned..layout.detail_rows.len() {
        let y = (list.loc.y + (i - pinned) as i32 * line_h) as f64 - shift;
        parts.push(row(i, y));
    }
    parts
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
            files: 0,
            added: 0,
            removed: 0,
            chip: None,
            state: LineState::Settled,
        }
    }

    fn file(code: char, path: &str, check: Check) -> TreeLine {
        TreeLine::File(FileLine {
            check,
            code,
            viewed: false,
            path: path.into(),
            conflicted: code == 'U',
            depth: 0,
            added: 0,
            removed: 0,
            edited: false,
            comments: 0,
        })
    }

    /// File line `line` with `(added, removed)` lines.
    fn counted(line: TreeLine, (added, removed): (usize, usize)) -> TreeLine {
        match line {
            TreeLine::File(f) => TreeLine::File(FileLine {
                added,
                removed,
                ..f
            }),
            line => line,
        }
    }

    fn hint(button: ReposButton, key: &str, disabled: Option<&str>) -> Hint {
        Hint {
            button,
            label: button.label().to_lowercase(),
            key: key.into(),
            disabled: disabled.map(Into::into),
            style: Default::default(),
        }
    }

    const DENSITY: Density = Density::Comfortable;

    fn diff_line(kind: LineKind, text: &str, words: &[Range<usize>]) -> DetailRow {
        DetailRow::Line(crate::diff::Line {
            kind,
            text: text.into(),
            raw: None,
            number: 1,
            old: 1,
            words: words.to_vec(),
            syntax: crate::syntax::lang("x.rs")
                .map(|lang| crate::syntax::highlight(lang, text, &mut Default::default()))
                .unwrap_or_default(),
        })
    }

    #[test]
    fn one_title_button_is_primary_the_one_to_press_next() {
        use ReposButton::*;
        let primary = |title: &mut Vec<Hint>| {
            mark_primary(title);
            (title.iter().filter(|h| h.style == ButtonStyle::Primary))
                .map(|h| h.button)
                .collect::<Vec<_>>()
        };
        // The fix plan: Make fixes, or Send back once a comment is written.
        let mut plan = vec![
            hint(CommentFile, "", None),
            hint(SendBack, "", None),
            hint(Approve, "", None),
        ];
        assert_eq!(primary(&mut plan), [Approve]);
        plan.swap(1, 2);
        assert_eq!(primary(&mut plan), [SendBack]);
        // One that can't be pressed isn't it.
        let mut waits = vec![
            hint(SendBack, "", None),
            hint(Approve, "", Some("Decide first")),
        ];
        assert_eq!(primary(&mut waits), [SendBack]);
        let mut fixes = vec![hint(AddComment, "", None), hint(MakeFixes, "", None)];
        assert_eq!(primary(&mut fixes), [MakeFixes]);
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
                files: 3,
                added: 15,
                removed: 1,
                ..repo("gerdr", Some("feat/x"))
            }),
            counted(file('M', "src/git.rs", Check::On), (2, 1)),
            counted(file('?', "notes.md", Check::Off), (13, 0)),
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
                files: 4,
                added: 20,
                removed: 7,
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
            TreeLine::Clean(CleanLine {
                open: true,
                count: 1,
            }),
            TreeLine::Repo(repo("docs", Some("main"))),
        ];
        let total = lines.len() + 12;
        ReposView {
            detail_share: 1.0,
            modes: None,
            grouping: Grouping::List,
            title: "gerdr".into(),
            summary: "2 dirty · 1 conflict · fetched 2m ago".into(),
            summary_tone: Tone::Muted,
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
                hint(MarkResolved, "Alt+D", None),
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
                        check: Some(Check::On),
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
                    DetailRow::CommentHead {
                        who: "Your comment",
                        finding: None,
                        place: "comment 1 of 1".into(),
                        within: None,
                    },
                    DetailRow::Comment {
                        text: "Why -z here? Paths with newlines".into(),
                        first: false,
                        last: false,
                        tone: None,
                        within: None,
                    },
                    DetailRow::Comment {
                        text: "need it, but check the parser.".into(),
                        first: false,
                        last: true,
                        tone: None,
                        within: None,
                    },
                    DetailRow::Editor {
                        text: "And the caller".into(),
                        caret: Some(7),
                        selection: 4..7,
                        first: true,
                        last: true,
                        within: None,
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
                over: 0.0,
                pin: 0.0,
                stuck: true,
                first: 3,
                total: 40,
                focused: true,
                gutter: 3,
                ..DetailView::default()
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
            tree_note: None,
            diff_only: false,
            diff_tree: false,
            unchecked: false,
            tree_nav: None,
            flat: false,
            no_header: false,
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
                    over: 0.0,
                    widest: vec!["main / master".into(), "production".into()],
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
                buttons: vec![Cancel, CleanOnly, All],
                default: 1,
                danger: false,
            }),
            hover: Some(ReposHit::Button(CleanOnly)),
            ..full()
        }
    }

    /// The views the layout is checked with: bare, full, and with a sheet.
    /// `full` with the files listed by folder.
    fn by_folder() -> ReposView {
        let dir = |name: &str, depth: usize, open: bool, check: Check| {
            TreeLine::Dir(DirLine {
                check,
                open,
                name: name.into(),
                depth,
            })
        };
        let at = |depth: usize, line: TreeLine| match line {
            TreeLine::File(f) => TreeLine::File(FileLine { depth, ..f }),
            line => line,
        };
        let mut view = full();
        let repo = view.lines[0].clone();
        let rest = view.lines[3..].to_vec();
        view.lines = [
            repo,
            dir("src", 0, true, Check::Partial),
            dir("chrome", 1, true, Check::On),
            at(2, file('M', "repos.rs", Check::On)),
            at(2, file('A', "mod.rs", Check::On)),
            at(1, file('M', "git.rs", Check::Off)),
            dir("docs/deep", 0, false, Check::Off),
            file('?', "notes.md", Check::Off),
        ]
        .into_iter()
        .chain(rest)
        .collect();
        view.grouping = Grouping::Folders;
        view.selected = Some(2);
        view.hover = Some(ReposHit::Tool(TreeTool::Collapse));
        view
    }

    /// `full()` with the diff's search open on "git", which has the keys:
    /// found in the pinned header's path, and in two lines, the second
    /// gone to; and in what the hunk is in.
    fn with_search() -> ReposView {
        let mut view = full();
        view.field_focused = false;
        view.detail.search = Some(SearchBar {
            query: Line {
                text: "git".into(),
                caret: 3,
                anchor: 3,
                scroll: 0,
            },
            placeholder: "Find in the diff".into(),
            count: "2 of 3".into(),
            found: true,
            focused: true,
            whole: false,
        });
        let mut found = vec![Vec::new(); view.detail.rows.len()];
        // "git.rs", then "src".
        found[0] = vec![(0..3, false)];
        found[1] = vec![(7..13, false)];
        found[3] = vec![(14..17, true)];
        found[4] = vec![(14..17, false), (60..90, false)];
        view.detail.found = found;
        view.detail.ticks = vec![(0.1, false), (0.12, true), (0.8, false)];
        view
    }

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
            by_folder(),
            with_sheet(),
            with_menu(),
            with_empty_menu(),
            with_create_menu(),
            with_bad_name_menu(),
            with_search(),
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
    fn the_trees_toolbar_is_over_its_lines_and_its_buttons_hit() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            let mut view = full();
            view.lines.insert(
                1,
                TreeLine::Dir(DirLine {
                    check: Check::Partial,
                    open: true,
                    name: "src/chrome".into(),
                    depth: 1,
                }),
            );
            let l = repos_layout(&view, repos_panel_rect(size));
            use TreeTool::*;
            let tools: Vec<_> = l.tree_tools.iter().map(|(t, _)| *t).collect();
            assert_eq!(tools, [Expand, Collapse, List, Folders]);
            // In a row left to right, the two groups further apart.
            let rects: Vec<Rect> = l.tree_tools.iter().map(|(_, r)| *r).collect();
            let gap = |k: usize| rects[k + 1].loc.x - (rects[k].loc.x + rects[k].size.w);
            assert_eq!(
                (gap(0), gap(1), gap(2)),
                (REPOS_TAB_GAP, REPOS_TOOL_GAP, REPOS_TAB_GAP)
            );
            let right = rects[3].loc.x + rects[3].size.w;
            assert_eq!(right, l.tree.loc.x + l.tree.size.w - HELP_PADDING);
            for (tool, rect) in &l.tree_tools {
                assert!(inside(l.tree, *rect), "{size:?}");
                assert!(rect.loc.y + rect.size.h <= l.tree_list.loc.y);
                let at = rect.loc.to_f64() + Point::from((4.0, 4.0));
                assert_eq!(repos_hit(&l, at), Some(ReposHit::Tool(*tool)));
            }
            // A folder's line is indented by its depth, with an expander.
            let (_, check, expander) = l.lines[1];
            let (_, repo_check, _) = l.lines[0];
            let indent = REPOS_INDENT + REPOS_LEVEL;
            assert_eq!(check.loc.x - repo_check.loc.x, indent);
            assert!(expander.is_some());
            // A feature's code review leads with going to the files not
            // viewed, a group of its own.
            view.tree_nav = Some(true);
            let l = repos_layout(&view, repos_panel_rect(size));
            let tools: Vec<_> = l.tree_tools.iter().map(|(t, _)| *t).collect();
            assert_eq!(
                tools,
                [PrevUnviewed, NextUnviewed, Expand, Collapse, List, Folders]
            );
            let rects: Vec<Rect> = l.tree_tools.iter().map(|(_, r)| *r).collect();
            let gap = |k: usize| rects[k + 1].loc.x - (rects[k].loc.x + rects[k].size.w);
            assert_eq!((gap(0), gap(1)), (REPOS_TAB_GAP, REPOS_TOOL_GAP));
            for (tool, rect) in &l.tree_tools {
                assert!(inside(l.tree, *rect), "{size:?}");
                let at = rect.loc.to_f64() + Point::from((4.0, 4.0));
                assert_eq!(repos_hit(&l, at), Some(ReposHit::Tool(*tool)));
            }
        }
    }

    #[test]
    fn every_part_is_inside_the_panel() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            for view in views() {
                let l = repos_layout(&view, repos_panel_rect(size));
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
                let page = view
                    .lines
                    .len()
                    .min(repos_page(repos_panel_rect(size), DENSITY).0);
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
        let l = repos_layout(&view, repos_panel_rect((2560, 1440).into()));
        let hints = l
            .buttons
            .iter()
            .filter(|(b, _)| view.hints.iter().any(|h| h.button == *b))
            .count();
        assert_eq!(hints, view.hints.len());
        assert_eq!(l.more, None);
        assert!(l.overflow.is_empty());
        let narrow = repos_layout(&view, repos_panel_rect((800, 600).into()));
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
        let panel = repos_panel_rect(size);
        let at = |share: f64| panel.loc.x as f64 + panel.size.w as f64 * share;
        assert!((repos_share_at(at(0.5), repos_panel_rect(size)) - 0.5).abs() < 0.01);
        assert_eq!(
            repos_share_at(at(0.05), repos_panel_rect(size)),
            REPOS_TREE_SHARE_MIN
        );
        assert_eq!(
            repos_share_at(at(0.95), repos_panel_rect(size)),
            REPOS_TREE_SHARE_MAX
        );
        let wide = ReposView {
            tree_share: 0.7,
            ..full()
        };
        let l = repos_layout(&wide, repos_panel_rect(size));
        assert!(l.tree.size.w > repos_layout(&full(), repos_panel_rect(size)).tree.size.w);
        assert_eq!(repos_hit(&l, centre(l.splitter)), Some(ReposHit::Splitter));
    }

    /// `full`'s diff alone across the panel, as a worktree's, its files
    /// with no checkbox.
    fn diff_only() -> ReposView {
        let mut view = full();
        for row in &mut view.detail.rows {
            if let DetailRow::File(file) = row {
                file.check = None;
            }
        }
        let hints = vec![
            hint(ReposButton::Comment, "c", None),
            hint(ReposButton::Help, "F1", None),
            hint(ReposButton::Close, "Esc", None),
        ];
        let title = "Worktree · feat/x".into();
        let summary = "2 commits on main".into();
        ReposView {
            field: view.field,
            field_focused: view.field_focused,
            placeholder: view.placeholder,
            empty: view.empty,
            branch: view.branch,
            tree_share: view.tree_share,
        tree_note: view.tree_note.clone(),
            grouping: view.grouping,
            part: view.part,
            over: view.over,
            first: view.first,
            total: view.total,
            selected: view.selected,
            ..ReposView::diff_only(
                title,
                summary,
                view.hover,
                hints,
                view.detail,
                view.sheet,
                view.density,
            )
        }
    }

    /// A feature's brief under review: Send back and Approve on the diff's
    /// title row.
    fn doc_review() -> ReposView {
        let mut view = diff_only();
        view.title = "Brief \u{b7} Log in with a password".into();
        view.summary = "ready for review".into();
        view.detail.title = "brief.md \u{b7} 12 lines".into();
        view.detail.title_buttons = [
            (ReposButton::SendBack, "Ctrl+Shift+Enter"),
            (ReposButton::Approve, "Ctrl+Enter"),
        ]
        .into_iter()
        .map(|(button, key)| Hint {
            button,
            label: button.label().into(),
            key: key.into(),
            disabled: None,
            style: Default::default(),
        })
        .collect();
        view
    }

    /// The Log tab: the commits to pull and to push, one picked.
    fn with_log() -> ReposView {
        let view = full();
        let lines = [
            ("To pull from origin/main (2):", DiffTone::Header),
            (
                "4f1c2ab Fold ignored errors · lgal, 3 hours ago",
                DiffTone::Plain,
            ),
            (
                "9e0d7a1 Show a project on its tab · lgal, 5 hours ago",
                DiffTone::Plain,
            ),
            ("", DiffTone::Plain),
            ("To push to origin/main (1):", DiffTone::Header),
            (
                "c0ffee1 Read the log both ways · lgal, 2 minutes ago",
                DiffTone::Plain,
            ),
        ];
        let rows: Vec<DetailRow> = (lines.iter())
            .map(|(text, tone)| DetailRow::Text(crate::diff_view::detail_line(text, *tone)))
            .collect();
        let marks = (0..rows.len())
            .map(|r| RowMark {
                selected: r == 2,
                top: true,
                bottom: true,
                noted: false,
            })
            .collect();
        let mut hints = view.hints.clone();
        hints.insert(0, hint(ReposButton::ShowCommit, "Enter", None));
        ReposView {
            detail: DetailView {
                tab: DetailTab::Log,
                title: "gerdr".into(),
                total: rows.len(),
                rows,
                marks,
                focused: true,
                ..DetailView::default()
            },
            hints,
            ..view
        }
    }

    /// A commit's diff across the panel.
    fn commit_only() -> ReposView {
        ReposView {
            title: "Commit · gerdr".into(),
            summary: "c0ffee1 Read the log both ways · lgal, 2 minutes ago".into(),
            hints: vec![
                hint(ReposButton::Help, "F1", None),
                Hint {
                    label: "back".into(),
                    ..hint(ReposButton::Close, "Esc", None)
                },
            ],
            ..diff_only()
        }
    }

    #[test]
    fn the_diff_alone_takes_the_whole_panel() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            let view = diff_only();
            let l = repos_layout(&view, repos_panel_rect(size));
            let hit = |r: Rect| repos_hit(&l, centre(r));
            // No tree, picker, tabs or field: the diff across it, over
            // the footer's actions.
            assert_eq!(l.detail.loc.x, l.panel.loc.x);
            assert_eq!(l.detail.size.w, l.panel.size.w);
            assert_eq!(l.detail.loc.y + l.detail.size.h, l.footer.loc.y);
            assert_eq!(
                l.footer.loc.y + l.footer.size.h,
                l.panel.loc.y + l.panel.size.h
            );
            assert!(l.tree_tools.is_empty() && l.detail_tabs.is_empty());
            assert_eq!(l.buttons.len(), 3);
            for (b, r) in &l.buttons {
                assert_eq!(hit(*r), Some(ReposHit::Button(*b)));
            }
            for r in [l.branch, l.field, l.splitter, l.tree] {
                assert!(r.size.w == 0 || r.size.h == 0, "{r:?}");
            }
            assert!(l.detail_checks.is_empty());
            assert!(!l.detail_folds.is_empty());
            for &(i, r) in &l.detail_folds {
                assert_eq!(hit(r), Some(ReposHit::DiffFold(i)));
            }
            for &(part, r) in &l.detail_search {
                assert_eq!(hit(r), Some(ReposHit::Search(part)));
            }
            for (b, r) in &l.detail_title_buttons {
                assert_eq!(hit(*r), Some(ReposHit::Button(*b)));
            }
            // As many rows as fit, which the page says.
            let page = diff_only_page(repos_panel_rect(size), view.density);
            let line_h = repos_detail_line(view.density);
            assert_eq!(
                l.detail_list.size.h / line_h,
                page.min(view.detail.rows.len() - 1) as i32
            );
            assert!(
                diff_only_cols(repos_panel_rect(size))
                    > repos_detail_cols(repos_panel_rect(size), REPOS_TREE_SHARE)
            );
        }
    }

    #[test]
    fn hits_land_on_what_is_under_them() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            let plain = |v: &ReposView| v.sheet.is_none() && v.branch.menu.is_none();
            for view in views().into_iter().filter(plain) {
                let l = repos_layout(&view, repos_panel_rect(size));
                let hit = |r: Rect| repos_hit(&l, centre(r));
                assert_eq!(hit(l.branch), Some(ReposHit::BranchButton));
                for (i, (line, check, expander)) in l.lines.iter().enumerate() {
                    // The heading has no checkbox.
                    if check.size.w > 0 {
                        assert_eq!(hit(*check), Some(ReposHit::Check(i)));
                    }
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
                for &(part, r) in &l.detail_search {
                    assert_eq!(hit(r), Some(ReposHit::Search(part)));
                }
                assert_eq!(hit(l.field), Some(ReposHit::Field));
                // The pinned header is over the row it covers.
                let pinned = view.detail.sticky as usize;
                let covered = |i: usize| i >= pinned && l.detail_rows[i].overlaps(l.detail_rows[0]);
                for (i, r) in l.detail_rows.iter().enumerate() {
                    if pinned == 1 && covered(i) {
                        assert_eq!(hit(*r), Some(ReposHit::DiffRow(0)));
                        continue;
                    }
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
        let l = repos_layout(&view, repos_panel_rect((1600, 1000).into()));
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
            let spans = line_spans(repo, (0.0, 1000.0), 8.0, 1.0, (&width, &width));
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
            let l = repos_layout(&view, repos_panel_rect(size));
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
            assert_eq!(repos_field(&view, repos_panel_rect(size), 1.0).area, search);
            // Its text starts after the magnifier, as painted.
            let text = repos_field(&view, repos_panel_rect(size), 1.0).text;
            assert_eq!(text, search_field_text(search, 1.0));
            let closed = ReposView {
                branch: full().branch,
                ..view.clone()
            };
            assert_eq!(
                repos_field(&closed, repos_panel_rect(size), 1.0).area,
                l.field
            );
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
            let l = repos_layout(&with_create_menu(), repos_panel_rect(size));
            let create = l.branch_create.expect("a row to create");
            assert_eq!(repos_hit(&l, centre(create)), Some(ReposHit::BranchCreate));
        }
    }

    #[test]
    fn scrolled_lists_move_up_and_show_their_last_line_in_part() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let (row_h, line_h) = (repos_row(DENSITY), repos_detail_line(DENSITY));
        let fits = repos_layout(&full(), repos_panel_rect(size));
        // As much as fits, then scrolled half a line with one more.
        let mut whole = full();
        whole.lines.truncate(fits.lines.len());
        whole.detail.rows.truncate(fits.detail_rows.len());
        let mut scrolled = whole.clone();
        scrolled.part = 0.5;
        scrolled.lines.push(whole.lines[1].clone());
        scrolled.detail.part = 0.5;
        scrolled.detail.rows.push(whole.detail.rows[1].clone());
        let (a, b) = (
            repos_layout(&whole, repos_panel_rect(size)),
            repos_layout(&scrolled, repos_panel_rect(size)),
        );
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

        // The detail: its pinned header stays at the top, the rows under
        // it move up half the first; the last is shown in part.
        assert!(scrolled.detail.sticky);
        assert_eq!(a.detail_list, b.detail_list);
        assert_eq!(a.detail_rows[0], b.detail_rows[0]);
        assert_eq!(b.detail_list.loc.y, b.detail_rows[0].loc.y);
        assert_eq!(b.detail_rows[1].loc.y, b.detail_list.loc.y - line_h / 2);
        let last = *b.detail_rows.last().unwrap();
        assert!(last.loc.y < bottom(b.detail_list) && bottom(last) > bottom(b.detail_list));
        // On the pinned header, the row under it is not hit.
        let x = b.detail_list.loc.x + b.detail_list.size.w / 2;
        let under = bottom(b.detail_rows[0]);
        assert_eq!(hit((x, under - 2)), Some(ReposHit::DiffRow(0)));
        assert_eq!(hit((x, under + 2)), Some(ReposHit::DiffRow(2)));
        assert_ne!(
            hit((x, b.detail_list.loc.y - 2)),
            Some(ReposHit::DiffRow(0))
        );
        // Pushed up by the next file's header, only what shows of it is.
        let mut pushed = scrolled.clone();
        pushed.detail.pin = -(line_h / 4) as f64;
        let c = repos_layout(&pushed, repos_panel_rect(size));
        let top = c.detail_list.loc.y;
        assert_eq!(c.detail_rows[0].loc.y, top - line_h / 4);
        assert_eq!(c.detail_rows[1], b.detail_rows[1]);
        let hit_c = |p: (i32, i32)| repos_hit(&c, (p.0 as f64, p.1 as f64).into());
        assert_ne!(hit_c((x, top - 2)), Some(ReposHit::DiffRow(0)));
        assert_eq!(hit_c((x, top + 2)), Some(ReposHit::DiffRow(0)));
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
        let (a, b) = (
            repos_layout(&whole, repos_panel_rect(size)),
            repos_layout(&scrolled, repos_panel_rect(size)),
        );
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
            let l = repos_layout(&view, repos_panel_rect(size));
            let (menu, rows) = l.branch_menu.expect("a menu");
            assert!(menu.loc.y + menu.size.h <= l.field.loc.y, "{size:?}");
            // As many as the tree's page less the search, whatever the size.
            let page = (repos_page(repos_panel_rect(size), DENSITY).0 - 1).min(BRANCH_MENU_ROWS);
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
            let l = repos_layout(&view, repos_panel_rect(size));
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
            let (tree_rows, detail_rows) = repos_page(repos_panel_rect(size), DENSITY);
            assert!(tree_rows >= 1 && detail_rows >= 1);
            if h >= 600 {
                assert!(tree_rows >= 8, "{size:?}: {tree_rows}");
                assert!(detail_rows >= 8, "{size:?}: {detail_rows}");
            }
            assert_eq!(
                repos_page(repos_panel_rect(size), DENSITY),
                (tree_rows, detail_rows)
            );
            // Compact rows are shorter: more of them fit.
            let compact = repos_page(repos_panel_rect(size), Density::Compact);
            assert!(compact.0 >= tree_rows && compact.1 >= detail_rows);
            // A page of lines fits with or without the error and result.
            let filled = |v: ReposView| ReposView {
                lines: (0..tree_rows)
                    .map(|i| file('M', &format!("src/{i}.rs"), Check::Off))
                    .collect(),
                ..v
            };
            for view in views().into_iter().map(filled) {
                let parts = parts(&view, repos_panel_rect(size));
                if h >= 600 {
                    assert_eq!(parts.layout.lines.len(), tree_rows, "{size:?}");
                    let (track, _) = detail_bar(&view, &parts.layout);
                    assert!(track.size.h >= detail_rows as i32 * repos_detail_line(DENSITY));
                }
            }
        }
    }

    #[test]
    fn the_field_spot_is_the_field() {
        let view = full();
        let size = (1280, 800).into();
        let spot = repos_field(&view, repos_panel_rect(size), 2.0);
        assert_eq!(spot.area, repos_layout(&view, repos_panel_rect(size)).field);
        assert_eq!(spot.line, view.field);
        assert!(spot.text.0 < spot.text.1);
    }

    #[test]
    fn the_pointer_on_a_diff_line_is_a_char_of_its_text() {
        let view = full();
        let size: Size<i32, Logical> = (1280, 800).into();
        let layout = repos_layout(&view, repos_panel_rect(size));
        let i = view
            .detail
            .rows
            .iter()
            .position(|r| matches!(r, DetailRow::Line(_)))
            .expect("a line");
        let rect = layout.detail_rows[i];
        let advance = measure_mono("0", REPOS_MONO_SIZE).unwrap() as f64;
        // Its text starts after the line numbers.
        let x0 = (rect.loc.x + DIFF_INSET + gutter_width(view.detail.gutter, false)) as f64;
        let y = rect.loc.y as f64 + rect.size.h as f64 / 2.0;
        let at = |x: f64| repos_diff_char(&view, repos_panel_rect(size), (x, y).into());
        assert_eq!(at(x0 + 0.5 * advance), Some((i, 0)));
        assert_eq!(at(x0 + 3.5 * advance), Some((i, 3)));
        // Left of its text, or on a row that is no line: none.
        assert_eq!(at(x0 - 2.0), None);
        // On a row shown from a char on, the char of its line.
        let skipped = ReposView {
            detail: DetailView {
                skipped: Some((i, 10)),
                ..view.detail.clone()
            },
            ..view.clone()
        };
        let pos = (x0 + 0.5 * advance, y);
        assert_eq!(
            repos_diff_char(&skipped, repos_panel_rect(size), pos.into()),
            Some((i, 10))
        );
        let head = view
            .detail
            .rows
            .iter()
            .position(|r| !matches!(r, DetailRow::Line(_)));
        if let Some(h) = head {
            let r = layout.detail_rows[h];
            let pos = (x0 + advance, r.loc.y as f64 + r.size.h as f64 / 2.0);
            assert_eq!(
                repos_diff_char(&view, repos_panel_rect(size), pos.into()),
                None
            );
        }
    }

    #[test]
    fn the_pointer_on_the_comment_being_written_is_a_place_of_its_caret() {
        // Only rows 2 and 3 of its own.
        let mut view = full();
        let blank = || DetailRow::Text(detail_line("", DiffTone::Plain));
        for row in &mut view.detail.rows {
            if matches!(row, DetailRow::Editor { .. }) {
                *row = blank();
            }
        }
        let none = view.clone();
        let editor = |first: bool| DetailRow::Editor {
            text: "hello ".into(),
            caret: None,
            selection: 0..0,
            first,
            last: !first,
            within: None,
        };
        view.detail.rows[2] = editor(true);
        view.detail.rows[3] = editor(false);
        let size: Size<i32, Logical> = (1280, 800).into();
        let panel = repos_panel_rect(size);
        let layout = repos_layout(&view, panel);
        let advance = measure_mono("0", REPOS_MONO_SIZE).unwrap() as f64;
        let (top, bottom) = (layout.detail_rows[2], layout.detail_rows[3]);
        let x0 = (top.loc.x + DIFF_INSET + 1 + COMMENT_INDENT) as f64;
        let mid = |r: Rect| r.loc.y as f64 + r.size.h as f64 / 2.0;
        // A press: the char boundary nearest, none left of the text.
        let at = |x: f64, y: f64| repos_diff_char(&view, panel, (x, y).into());
        assert_eq!(at(x0 + 2.4 * advance, mid(top)), Some((2, 2)));
        assert_eq!(at(x0 + 2.6 * advance, mid(top)), Some((2, 3)));
        assert_eq!(at(x0 - 20.0, mid(bottom)), Some((3, 0)));
        // Dragged on: its row there, above its rows the first, below the
        // last.
        let drag = |x: f64, y: f64| repos_editor_char(&view, panel, (x, y).into());
        assert_eq!(drag(x0 + advance, mid(bottom)), Some((Some(3), 1)));
        assert_eq!(drag(x0, top.loc.y as f64 - 100.0), Some((Some(2), 0)));
        assert_eq!(drag(x0, mid(bottom) + 300.0), Some((Some(3), 0)));
        // None of its rows in view: the place all the same.
        let off = repos_editor_char(&none, panel, (x0 + 2.0 * advance, mid(top)).into());
        assert_eq!(off, Some((None, 2)));
    }

    #[test]
    fn the_search_sits_in_the_title_row_and_takes_the_field_spot() {
        for (w, h) in SIZES {
            let size: Size<i32, Logical> = (w, h).into();
            // Closed, its button is left of the title's, over the diff
            // only.
            let closed = repos_layout(&full(), repos_panel_rect(size));
            let parts: Vec<SearchPart> = closed.detail_search.iter().map(|p| p.0).collect();
            assert_eq!(parts, [SearchPart::Open]);
            let (_, open) = closed.detail_search[0];
            for (_, b) in &closed.detail_title_buttons {
                assert!(open.loc.x + open.size.w <= b.loc.x);
            }
            let log = ReposView {
                detail: DetailView {
                    tab: DetailTab::Log,
                    ..full().detail
                },
                ..full()
            };
            assert!(
                repos_layout(&log, repos_panel_rect(size))
                    .detail_search
                    .is_empty()
            );
            // Open: the field, then the count and the buttons, in a row
            // left of the title's buttons and the tabs.
            let view = with_search();
            let l = repos_layout(&view, repos_panel_rect(size));
            let parts: Vec<SearchPart> = l.detail_search.iter().map(|p| p.0).collect();
            let order = [
                SearchPart::Field,
                SearchPart::Prev,
                SearchPart::Next,
                SearchPart::Close,
            ];
            assert_eq!(parts, order);
            let count = l.detail_search_count.expect("a count");
            let field = l.detail_search[0].1;
            assert!(field.size.w > 0 && field.loc.x + field.size.w <= count.loc.x);
            for pair in l.detail_search.windows(2) {
                assert!(pair[0].1.loc.x + pair[0].1.size.w <= pair[1].1.loc.x);
            }
            let tabs = l.detail_tabs[0].1;
            let (_, close) = l.detail_search[3];
            assert!(close.loc.x + close.size.w <= tabs.loc.x);
            assert!(within(l.detail, field));
            let spot = repos_field(&view, repos_panel_rect(size), 1.0);
            assert_eq!(spot.area, field);
            assert_eq!(spot.line.text, "git");
            // Without the keys, the commit field has the spot.
            let mut away = with_search();
            away.detail.search.as_mut().unwrap().focused = false;
            assert_eq!(
                repos_field(&away, repos_panel_rect(size), 1.0).area,
                l.field
            );
        }
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
        let fits = repos_page(repos_panel_rect(size), DENSITY).0;
        view.lines.truncate(fits + 1);
        let mut layers = ReposLayers::default();
        layers.parts(
            Some(view.clone()),
            repos_panel_rect(size),
            Theme::Dark,
            1.0,
            opened,
            &painter,
        );
        let a = layers.parts(
            Some(view.clone()),
            repos_panel_rect(size),
            Theme::Dark,
            1.0,
            now,
            &painter,
        );
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
            let layout = repos_layout(&moved, repos_panel_rect(size));
            let b = layers.parts(
                Some(moved.clone()),
                repos_panel_rect(size),
                Theme::Dark,
                1.0,
                now,
                &painter,
            );
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
    fn scrolling_the_diff_by_part_of_a_row_draws_nothing_again() {
        let painter = Painter::new();
        let size: Size<i32, Logical> = (1280, 800).into();
        let line_h = repos_detail_line(DENSITY) as f64;
        let opened = std::time::Instant::now();
        let now = opened + std::time::Duration::from_secs(1);
        // More rows than fit under the pinned header, scrolled part of a
        // row: one more shown in part under those that fit.
        let mut view = ReposView {
            hover: None,
            ..full()
        };
        let fits = repos_page(repos_panel_rect(size), DENSITY).1;
        let detail = &mut view.detail;
        detail.rows = detail.rows.iter().cycle().take(60).cloned().collect();
        detail.rows.truncate(1 + fits + 1);
        detail.part = 0.2;
        let mut layers = ReposLayers::default();
        layers.parts(
            Some(view.clone()),
            repos_panel_rect(size),
            Theme::Dark,
            1.0,
            opened,
            &painter,
        );
        let a = layers.parts(
            Some(view.clone()),
            repos_panel_rect(size),
            Theme::Dark,
            1.0,
            now,
            &painter,
        );
        let (frames, rows) = (layers.frames, layers.detail.drawn());
        assert_eq!(frames, 1);
        assert!(rows > 1);
        let list = repos_layout(&view, repos_panel_rect(size)).detail_list;
        let in_list = |parts: &[crate::chrome::Part]| -> Vec<f64> {
            let ys = parts.iter().filter(|p| p.clip == Some(list));
            ys.map(|p| p.loc.y).collect()
        };
        // The shadow under the pinned header, the header, the rows.
        let ys_a = in_list(&a);
        assert_eq!(ys_a.len(), 2 + fits + 1);
        // Further by part of a row; the header pushed up by the next;
        // pulled down past the start with the header in its place.
        let cases = [
            (0.7, 0.0, 0.0, true),
            (0.7, 0.0, -7.5, true),
            (0.0, 9.0, 9.0, false),
        ];
        for (part, over, pin, stuck) in cases {
            let mut moved = view.clone();
            moved.detail.part = part;
            moved.detail.over = over;
            moved.detail.pin = pin;
            moved.detail.stuck = stuck;
            let layout = repos_layout(&moved, repos_panel_rect(size));
            let b = layers.parts(
                Some(moved.clone()),
                repos_panel_rect(size),
                Theme::Dark,
                1.0,
                now,
                &painter,
            );
            assert_eq!((layers.frames, layers.detail.drawn()), (frames, rows));
            let ys_b = in_list(&b);
            let shadow = usize::from(stuck);
            let header = ys_b[shadow];
            assert_eq!(header, list.loc.y as f64 + pin);
            assert_eq!(layout.detail_rows[0].loc.y, header.round() as i32);
            let by = (part - 0.2) * line_h - over;
            for (k, y) in ys_b[shadow + 1..].iter().enumerate() {
                assert!((ys_a[2 + k] - y - by).abs() < 1e-9, "{k}: {y}");
                assert_eq!(layout.detail_rows[k + 1].loc.y, y.round() as i32);
            }
        }
    }

    #[test]
    fn scrolling_the_branch_menu_by_part_of_a_line_draws_nothing_again() {
        let painter = Painter::new();
        let size: Size<i32, Logical> = (1280, 800).into();
        let row_h = repos_row(DENSITY) as f64;
        let opened = std::time::Instant::now();
        let now = opened + std::time::Duration::from_secs(1);
        // Scrolled part of a line, one more shown in part under the list.
        let mut view = ReposView {
            hover: None,
            ..with_menu()
        };
        if let Some(menu) = &mut view.branch.menu {
            menu.part = 0.2;
            menu.rows.push("release/2.2".into());
            menu.deletable.push(false);
        }
        let mut layers = ReposLayers::default();
        layers.parts(
            Some(view.clone()),
            repos_panel_rect(size),
            Theme::Dark,
            1.0,
            opened,
            &painter,
        );
        let a = layers.parts(
            Some(view.clone()),
            repos_panel_rect(size),
            Theme::Dark,
            1.0,
            now,
            &painter,
        );
        let drawn = (layers.frames, layers.cards, layers.menu_lines.drawn());
        assert_eq!(drawn, (1, 1, 7));
        let list = repos_layout(&view, repos_panel_rect(size))
            .branch_list
            .unwrap();
        let in_list = |parts: &[crate::chrome::Part]| -> Vec<f64> {
            let ys = parts.iter().filter(|p| p.clip == Some(list));
            ys.map(|p| p.loc.y).collect()
        };
        let ys_a = in_list(&a);
        assert_eq!(ys_a.len(), 7);
        // Further by part of a line; pulled down past the start, the line
        // under the list gone.
        for (part, over) in [(0.7, 0.0), (0.0, 6.5)] {
            let mut moved = view.clone();
            if let Some(menu) = &mut moved.branch.menu {
                (menu.part, menu.over) = (part, over);
                if part == 0.0 {
                    menu.rows.pop();
                    menu.deletable.pop();
                }
            }
            let layout = repos_layout(&moved, repos_panel_rect(size));
            assert_eq!(layout.branch_list, Some(list));
            let b = layers.parts(
                Some(moved.clone()),
                repos_panel_rect(size),
                Theme::Dark,
                1.0,
                now,
                &painter,
            );
            let again = (layers.frames, layers.cards, layers.menu_lines.drawn());
            assert_eq!(again, drawn);
            let ys_b = in_list(&b);
            let by = (part - 0.2) * row_h - over;
            for (y_a, y_b) in ys_a.iter().zip(&ys_b) {
                assert!((y_a - y_b - by).abs() < 1e-9, "{y_a} {y_b}");
            }
            // Hit where they are drawn, to the logical pixel.
            let (_, rows) = layout.branch_menu.clone().unwrap();
            let lines = menu_rects(moved.branch.menu.as_ref().unwrap(), &layout, &rows);
            for ((_, rect), y) in lines.iter().zip(&ys_b) {
                assert_eq!(rect.loc.y, y.round() as i32);
            }
        }
    }

    #[test]
    fn a_split_leaves_the_rest_to_the_page() {
        let area = Rectangle::new((10, 20).into(), (1000, 500).into());
        // All the review: the page has nothing, at the right.
        let (detail, gap, page) = split_rects(area, 1.0);
        assert_eq!(detail, area);
        assert_eq!((gap.size.w, page.size.w, page.loc.x), (0, 0, 1010));
        // All the page: the review has nothing, at the left.
        let (detail, gap, page) = split_rects(area, 0.0);
        assert_eq!(page, area);
        assert_eq!((detail.size.w, gap.size.w, detail.loc.x), (0, 0, 10));
        // Between: they meet at the gap and fill the width, at the height.
        let (detail, gap, page) = split_rects(area, 0.4);
        assert_eq!((detail.loc.x, detail.size.w), (10, 400));
        assert_eq!((gap.loc.x, gap.size.w), (410, DOCK_GAP));
        assert_eq!(page.loc.x, gap.loc.x + DOCK_GAP);
        assert_eq!(page.loc.x + page.size.w, 1010);
        assert!([detail, gap, page].iter().all(|r| r.size.h == 500));
    }

    #[test]
    fn it_paints() {
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
            by_folder(),
            with_sheet(),
            disabled_hover,
            compact,
            long_sheet,
            with_menu(),
            with_empty_menu(),
            with_create_menu(),
            with_bad_name_menu(),
            with_search(),
            ReposView {
                hover: Some(ReposHit::Search(SearchPart::Next)),
                ..with_search()
            },
            diff_only(),
        ];
        let jobs: Vec<_> = views
            .iter()
            .flat_map(|view| SIZES.map(|size| (view, size)))
            .flat_map(|(view, size)| {
                [(Theme::Dark, 1.0), (Theme::Light, 2.0)].map(|case| (view, size, case))
            })
            .collect();
        crate::chrome::golden::fan_out(jobs.len(), |painter, job| {
            let (view, (w, h), (theme, scale)) = jobs[job];
            let _ = painter.repos(
                view,
                repos_panel_rect((w, h).into()),
                scale,
                theme.palette(),
            );
        });
    }

    /// The full panel with Restore buttons on the hunk's header and at the
    /// cursor, the pointer on the header's.
    fn with_restore() -> ReposView {
        use ReposButton::*;
        let mut view = full();
        view.detail.row_buttons = vec![
            (2, hint(Restore, "", None)),
            (4, hint(Comment, "C", None)),
            (4, hint(Restore, "R", None)),
        ];
        view.hover = Some(ReposHit::DiffRestore(2));
        view
    }

    /// The full panel with a keyed button on the title row too.
    fn with_keyed_buttons() -> ReposView {
        let mut view = with_restore();
        view.detail.title_buttons = vec![hint(ReposButton::Approve, "Ctrl+Enter", None)];
        view
    }

    #[test]
    fn only_the_footers_buttons_take_room_for_their_keys() {
        use ReposButton::*;
        let view = with_keyed_buttons();
        let layout = repos_layout(&view, repos_panel_rect((1600, 900).into()));
        let wide = |button| layout.buttons.iter().find(|(b, _)| *b == button).unwrap().1;
        assert_eq!(
            layout.detail_title_buttons[0].1.size.w,
            hint_width(&hint(Approve, "", None))
        );
        assert_eq!(wide(Undo).size.w, hint_width(&hint(Undo, "", None)));
        let commit = hint(Commit, "Ctrl+Enter", None);
        assert_eq!(wide(Commit).size.w, hint_width(&commit));
        assert!(hint_width(&commit) > hint_width(&hint(Commit, "", None)));
        assert_eq!(
            row_chip_width(&hint(Comment, "Ctrl+Shift+Del", None)),
            row_chip_width(&hint(Comment, "", None))
        );
    }

    #[test]
    fn a_button_without_its_key_says_it_in_its_tip() {
        use ReposButton::*;
        let view = with_keyed_buttons();
        let layout = repos_layout(&view, repos_panel_rect((1600, 900).into()));
        let tip = |rect: Rect| {
            let pos = (
                rect.loc.x as f64 + rect.size.w as f64 / 2.0,
                rect.loc.y as f64 + rect.size.h as f64 / 2.0,
            );
            crate::repo_batches::button_tip(&view, &layout, pos.into())
                .map(|t| (t.text, t.key.unwrap_or_default()))
        };
        let said = |text: &str, key: &str| Some((text.to_owned(), key.to_owned()));
        let on_row = |row: usize, button: ReposButton| {
            let found = (layout.detail_row_buttons.iter())
                .find(|(i, b, _)| *i == row && *b == button)
                .unwrap();
            found.2
        };
        let wide = |button| layout.buttons.iter().find(|(b, _)| *b == button).unwrap().1;

        assert_eq!(
            tip(layout.detail_title_buttons[0].1),
            said("Approve", "Ctrl+Enter")
        );
        assert_eq!(tip(wide(Undo)), said("Undo", "Alt+Z"));
        assert_eq!(tip(on_row(4, Comment)), said("Comment", "C"));
        assert_eq!(tip(on_row(4, Restore)), said("Restore", "R"));
        // No key, nothing to say; the footer's shows its own.
        assert_eq!(tip(on_row(2, Restore)), None);
        assert_eq!(tip(wide(Commit)), None);
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
            ("folders", by_folder()),
            ("sheet", with_sheet()),
            ("empty", empty()),
            ("hover", disabled_hover),
            ("menu", with_menu()),
            ("menu_empty", with_empty_menu()),
            ("menu_create", with_create_menu()),
            ("menu_bad_name", with_bad_name_menu()),
            ("search", with_search()),
            ("diff_only", diff_only()),
            ("doc_review", doc_review()),
            ("log", with_log()),
            ("commit", commit_only()),
            (
                "diff_only_sheet",
                ReposView {
                    sheet: Some(SheetView {
                        title: "Delete the review's 2 comments for good?".into(),
                        lines: vec![(
                            "gerdr".into(),
                            "src/git.rs: 2 comments".into(),
                            Tone::Warning,
                        )],
                        buttons: vec![ReposButton::Cancel, ReposButton::ClearReview],
                        default: 0,
                        danger: true,
                    }),
                    ..diff_only()
                },
            ),
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
            ("restore", with_restore()),
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
                        buttons: vec![ReposButton::Cancel, ReposButton::ForcePush],
                        default: 0,
                        danger: true,
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
                        danger: false,
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
                let panel = repos_layout(view, repos_panel_rect(size)).panel;
                let w = ((panel.size.w + 2 * SHADOW) as f64 * scale).round() as usize;
                let mut buffer =
                    painter.repos(view, repos_panel_rect(size), scale, theme.palette());
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
