//! The repositories panel: the project's repositories as one, nearly as big
//! as the output. On the left a tree, a row per repository (a checkbox,
//! its branch, where it stands, what is being done to it) with its changed
//! files under it, each with a checkbox; on the right the selected file's
//! diff or the selected repository's log; under both a field (a branch
//! name or a commit message), the last batch's result and the actions.
//! A sheet over it asks before a batch that could conflict. At the right
//! of the header, a branch picker: the branch the repositories acted on
//! are on, and a menu of the ones they all have to switch them to.

use super::{
    BUTTON_RADIUS, CAP_PADDING, CAP_RADIUS, CHECKBOX_SIZE, Canvas, FIELD_GAP, FieldSpot,
    GIT_MARK_GAP, GIT_MARK_WIDTH, HELP_MARGIN, HELP_PADDING, HELP_RADIUS, HINT_KEY_GAP, LineState,
    MENU_RADIUS, PADDING, Painter, Palette, QUIT_BUTTON_GAP, Rect, Rgb, SHADOW, arrow, band, check,
    chevron_down, coverage, field_text, git_branch, icon_sdf, measure, mix, phys, physical,
    rect_px, rounded_rect, segment,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The panel: at most this wide and high, a margin in from the output's
/// edges.
const REPOS_MAX_WIDTH: i32 = 1600;
const REPOS_MAX_HEIGHT: i32 = 1200;
/// The panel's text, a size up from the rest of the chrome's to be read
/// at a glance: its labels, the header's title, paths and actions, chips,
/// and keys on keycaps.
const FONT_SIZE: f32 = 18.0;
const LABEL_FONT_SIZE: f32 = 21.0;
const SMALL_FONT_SIZE: f32 = 16.0;
const DETAIL_FONT_SIZE: f32 = 15.0;
const KEY_FONT_SIZE: f32 = 15.0;
/// A button's height and the error line's height, to go with the bigger
/// text.
const ROW_BUTTON: i32 = 32;
const ERROR_HEIGHT: i32 = 24;
/// The header row with the title and the summary, and the gap between
/// them.
const REPOS_HEADER: i32 = 48;
const HEADER_GAP: i32 = 14;
/// The tree's share of the panel's width, and the narrowest it gets
/// (unless that is more than half).
const REPOS_TREE_SHARE: f64 = 0.42;
const REPOS_TREE_MIN: i32 = 300;
/// The room above and below the tree's lines and the detail's, and beside
/// the tree's.
const REPOS_PAD: i32 = 6;
/// A tree line, the room before its checkbox, and how far a file's line
/// is indented under its repository's.
pub const REPOS_ROW: i32 = 34;
const REPOS_LINE_INSET: i32 = 4;
const REPOS_INDENT: i32 = 24;
/// A checkbox's clickable room, a repository's expander's, and a file's
/// status letter column with the gap before it.
const REPOS_CHECK: i32 = 28;
const REPOS_EXPANDER: i32 = 20;
const REPOS_CODE: i32 = 16;
const REPOS_CODE_GAP: i32 = 2;
/// A repository's chip: its height, the room either side of its text and
/// its corners.
const CHIP_HEIGHT: i32 = 24;
const CHIP_PAD: i32 = 7;
const CHIP_RADIUS: f32 = 4.0;
/// The detail pane: its title row, a line of a diff or log, and the size
/// of the monospace font they are in.
const REPOS_DETAIL_TITLE: i32 = 44;
pub const REPOS_DETAIL_LINE: i32 = 24;
const REPOS_MONO_SIZE: f32 = 16.0;
/// The Diff and Log tabs: each this wide, this far apart.
const REPOS_TAB_WIDTH: i32 = 64;
const REPOS_TAB_GAP: i32 = 4;
/// The field's row, and the field in it.
const REPOS_FIELD_ROW: i32 = 48;
const REPOS_FIELD: i32 = 40;
/// The result bar, and the gap between its buttons and the room either
/// side of their labels.
const REPOS_RESULT: i32 = 40;
const REPOS_BUTTON_GAP: i32 = 6;
const PILL_PAD: i32 = 12;
/// The footer: the room above and below its rows of actions, a row, the
/// most rows, the gap between actions and the room inside either end of
/// one.
const REPOS_FOOTER_PAD: i32 = 4;
const REPOS_HINT_ROW: i32 = 36;
const REPOS_FOOTER_ROWS: usize = 3;
const HINT_GAP: i32 = 4;
const HINT_PAD: i32 = 6;
/// Without a font to measure with, how wide a character is guessed to be
/// (logical pixels): of a label in the small font, of a key on a keycap,
/// and of a sheet button's label; and what is added for a whole text.
const SMALL_CHAR: f64 = 6.6;
const KEY_CHAR: f64 = 5.6;
const BUTTON_CHAR: f64 = 7.7;
const GUESS_SLACK: f64 = 4.0;
/// The card saying why an action can't be done: its height and its gap to
/// the action.
const TIP_HEIGHT: i32 = 30;
const TIP_GAP: i32 = 4;
/// The sheet: at most this wide; the room inside it, its title row, a
/// line, the gap after the title and before the buttons, the buttons and
/// the room either side of their labels, and the share of a line its
/// repository's name takes.
const SHEET_WIDTH: i32 = 640;
const SHEET_PAD: i32 = 18;
const SHEET_TITLE: i32 = 32;
const SHEET_LINE: i32 = 32;
const SHEET_GAP: i32 = 10;
const SHEET_BUTTON: i32 = 38;
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
    /// The header's branch picker.
    pub branch: BranchPicker,
}

/// The header's branch picker: the branch the repositories acted on are
/// on, and its menu while open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchMenuView {
    pub rows: Vec<String>,
    /// The row of the branch they are all on, by index into `rows`.
    pub current: Option<usize>,
    /// The row Enter picks, by index into `rows`.
    pub picked: Option<usize>,
    /// What it says without rows.
    pub empty: String,
    /// What is typed to find a branch, and what it says while empty.
    pub query: Line,
    pub placeholder: String,
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
    /// The project's feature, when this isn't on it: a `≠` badge.
    pub off_feature: Option<String>,
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
    /// As shown: `Ctrl+N`.
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
    /// The branch picker's button.
    BranchButton,
    /// A row of the branch menu, by index into `BranchMenuView::rows`.
    BranchRow(usize),
    /// The branch menu, on no row (for the wheel).
    BranchMenu,
    /// A mark on a repository's line, by index into `ReposView::lines`:
    /// it says what it means.
    Mark(usize, RepoMark),
    /// Inside the panel, on nothing.
    Panel,
}

/// A mark after a repository's branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoMark {
    /// `≠`: not on the project's feature.
    OffFeature,
    /// A dot: changes not committed.
    Dirty,
    /// An arrow up and a count: commits to push.
    Ahead,
    /// An arrow down and a count: commits to pull.
    Behind,
}

impl RepoMark {
    /// What the mark says of `repo`, over it while the pointer is on it.
    pub fn tip(self, repo: &RepoLine) -> String {
        let commits = |n: u32| match n {
            1 => "1 commit".to_owned(),
            n => format!("{n} commits"),
        };
        match self {
            RepoMark::OffFeature => match &repo.off_feature {
                Some(feature) => format!("Not on the feature, {feature}"),
                None => "Not on the feature".into(),
            },
            RepoMark::Dirty => "Changes not committed".into(),
            RepoMark::Ahead => format!("{} to push", commits(repo.ahead)),
            RepoMark::Behind => format!("{} to pull", commits(repo.behind)),
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
    /// Where the notes start.
    pen: f32,
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
        (
            RepoMark::OffFeature,
            repo.off_feature.is_some(),
            w + 2.0 * s,
            0,
        ),
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
    LineSpans {
        chip,
        end,
        branch,
        marks,
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
    /// expander's.
    pub lines: Vec<(Rect, Rect, Option<Rect>)>,
    /// The marks on repository lines, by index into `lines`.
    pub marks: Vec<(usize, RepoMark, Rect)>,
    pub detail: Rect,
    pub detail_tabs: Vec<(DetailTab, Rect)>,
    pub field: Rect,
    pub result: Option<Rect>,
    /// The result bar's buttons, the footer's, and the sheet's.
    pub buttons: Vec<(ReposButton, Rect)>,
    pub footer: Rect,
    pub sheet: Option<Rect>,
    /// The branch picker's button, and its menu with a rect per row while
    /// open.
    pub branch: Rect,
    pub branch_menu: Option<(Rect, Vec<Rect>)>,
    /// The branch menu's search, at its top.
    pub branch_search: Option<Rect>,
}

/// Inside the panel's parts: what the painter needs besides `ReposLayout`.
struct Parts {
    layout: ReposLayout,
    /// The line under the field saying what went wrong.
    error: Option<Rect>,
    /// Where the detail pane's lines go, under its title row.
    body: Rect,
    /// The result bar's buttons.
    result_buttons: Vec<(ReposButton, Rect)>,
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
        let label = text_extent(hint.button.label(), SMALL_FONT_SIZE, SMALL_CHAR);
        return 2 * HINT_PAD + 2 * CAP_PADDING + label;
    }
    2 * HINT_PAD
        + 2 * CAP_PADDING
        + text_extent(&hint.key, KEY_FONT_SIZE, KEY_CHAR)
        + HINT_KEY_GAP
        + text_extent(hint.button.label(), SMALL_FONT_SIZE, SMALL_CHAR)
}

/// How wide a pill button in the result bar is.
fn pill_width(button: ReposButton) -> i32 {
    text_extent(button.label(), SMALL_FONT_SIZE, SMALL_CHAR) + 2 * PILL_PAD
}

/// How wide a sheet's button is.
fn sheet_button_width(button: ReposButton) -> i32 {
    (text_extent(button.label(), FONT_SIZE, BUTTON_CHAR) + 2 * SHEET_BUTTON_PAD)
        .max(SHEET_BUTTON_MIN)
}

/// The footer's actions split into rows of at most `w` logical pixels, as
/// many rows as `REPOS_FOOTER_ROWS`: per row, (index into `hints`, width).
/// What doesn't fit the last row is left out.
fn hint_rows(hints: &[Hint], w: i32) -> Vec<Vec<(usize, i32)>> {
    let mut rows: Vec<Vec<(usize, i32)>> = vec![Vec::new()];
    let mut used = 0;
    for (i, hint) in hints.iter().enumerate() {
        let width = hint_width(hint).min(w);
        let row = rows.last_mut().expect("a row");
        let more = if row.is_empty() {
            width
        } else {
            HINT_GAP + width
        };
        if used + more <= w {
            row.push((i, width));
            used += more;
        } else if rows.len() < REPOS_FOOTER_ROWS {
            rows.push(vec![(i, width)]);
            used = width;
        } else {
            break;
        }
    }
    rows
}

/// How high the footer is with `rows` rows of actions.
fn footer_h(rows: usize) -> i32 {
    2 * REPOS_FOOTER_PAD + rows.max(1) as i32 * REPOS_HINT_ROW
}

/// The tree's width in a panel `w` wide.
fn tree_width(w: i32) -> i32 {
    ((w as f64 * REPOS_TREE_SHARE).round() as i32)
        .max(REPOS_TREE_MIN.min(w / 2))
        .min(w)
}

/// How wide the branch picker's button is for `label`.
fn picker_width(label: &str) -> i32 {
    let text = text_extent(label, FONT_SIZE, BUTTON_CHAR);
    (2 * PICKER_PAD + PICKER_ICON + 2 * PICKER_GAP + text + PICKER_CHEVRON).max(PICKER_MIN)
}

/// How many tree lines and detail lines the panel shows at once on an
/// output of `size`: with the error line, the result bar and the footer's
/// rows all there, so that the page doesn't change as they come and go.
pub fn repos_page(size: Size<i32, Logical>) -> (usize, usize) {
    let panel = panel_rect(size);
    let bottom = REPOS_FIELD_ROW + ERROR_HEIGHT + REPOS_RESULT + footer_h(REPOS_FOOTER_ROWS);
    let middle = panel.size.h - REPOS_HEADER - bottom;
    let tree = (middle - 2 * REPOS_PAD) / REPOS_ROW;
    let detail = (middle - REPOS_DETAIL_TITLE - 2 * REPOS_PAD) / REPOS_DETAIL_LINE;
    (tree.max(1) as usize, detail.max(1) as usize)
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
    // The branch picker at the header's right, no more than a third of it.
    let pw = picker_width(&view.branch.label).min(inner_w / 3).max(1);
    let branch = Rectangle::new(
        (
            inner_x + inner_w - pw,
            top + (REPOS_HEADER - ROW_BUTTON) / 2,
        )
            .into(),
        (pw, ROW_BUTTON).into(),
    );

    // From the bottom up: the footer, the result bar, the error, the field.
    let rows = hint_rows(&view.hints, inner_w);
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

    // The tree and its lines.
    let tree_w = tree_width(w);
    let tree = Rectangle::new((x, middle_y).into(), (tree_w, middle_h).into());
    let (line_x, line_w) = (x + REPOS_PAD, (tree_w - 2 * REPOS_PAD).max(0));
    let tree_end = middle_y + middle_h - REPOS_PAD;
    let (mut lines, mut marks) = (Vec::new(), Vec::new());
    for (i, line) in view.lines.iter().enumerate() {
        let ly = middle_y + REPOS_PAD + i as i32 * REPOS_ROW;
        if ly + REPOS_ROW > tree_end {
            break;
        }
        let rect = Rectangle::new((line_x, ly).into(), (line_w, REPOS_ROW).into());
        let indent = match line {
            TreeLine::Repo(_) => 0,
            TreeLine::File(_) => REPOS_INDENT,
        };
        let check_x = line_x + REPOS_LINE_INSET + indent;
        let check = Rectangle::new((check_x, ly).into(), (REPOS_CHECK, REPOS_ROW).into());
        let expander = match line {
            TreeLine::Repo(repo) if repo.expanded.is_some() => Some(Rectangle::new(
                (check_x + REPOS_CHECK, ly).into(),
                (REPOS_EXPANDER, REPOS_ROW).into(),
            )),
            _ => None,
        };
        if let TreeLine::Repo(repo) = line {
            // Measured as the painter measures, at one pixel a pixel.
            let width = |text: &str, size: f32| measure(text, size).unwrap_or(0.0);
            let x = (check_x + REPOS_CHECK + REPOS_EXPANDER) as f32;
            let right = (line_x + line_w) as f32 - PADDING;
            let spans = line_spans(repo, (x, right), FIELD_GAP as f32, 1.0, &width);
            for m in spans.marks {
                let mx = m.x.floor() as i32;
                let mw = (m.end.ceil() as i32 - mx).max(1);
                let rect = Rectangle::new((mx, ly).into(), (mw, REPOS_ROW).into());
                marks.push((i, m.mark, rect));
            }
        }
        lines.push((rect, check, expander));
    }

    // The detail pane, its tabs at the right of its title row.
    let detail = Rectangle::new(
        (x + tree_w, middle_y).into(),
        ((w - tree_w).max(0), middle_h).into(),
    );
    let tab_y = middle_y + (REPOS_DETAIL_TITLE - ROW_BUTTON) / 2;
    let log_x = detail.loc.x + detail.size.w - HELP_PADDING - REPOS_TAB_WIDTH;
    let diff_x = log_x - REPOS_TAB_GAP - REPOS_TAB_WIDTH;
    let detail_tabs = vec![
        (
            DetailTab::Diff,
            Rectangle::new((diff_x, tab_y).into(), (REPOS_TAB_WIDTH, ROW_BUTTON).into()),
        ),
        (
            DetailTab::Log,
            Rectangle::new((log_x, tab_y).into(), (REPOS_TAB_WIDTH, ROW_BUTTON).into()),
        ),
    ];
    let body = Rectangle::new(
        (
            detail.loc.x + HELP_PADDING,
            middle_y + REPOS_DETAIL_TITLE + REPOS_PAD,
        )
            .into(),
        (
            (detail.size.w - 2 * HELP_PADDING).max(0),
            (middle_h - REPOS_DETAIL_TITLE - 2 * REPOS_PAD).max(0),
        )
            .into(),
    );

    // The result bar's buttons, right-aligned.
    let mut result_buttons = Vec::new();
    if let (Some(bar), Some(rect)) = (&view.result, result) {
        let mut right = inner_x + inner_w;
        let by = rect.loc.y + (REPOS_RESULT - ROW_BUTTON) / 2;
        for &button in bar.buttons.iter().rev() {
            let bw = pill_width(button);
            right -= bw;
            if right < inner_x + inner_w / 2 {
                break;
            }
            result_buttons.push((
                button,
                Rectangle::new((right, by).into(), (bw, ROW_BUTTON).into()),
            ));
            right -= REPOS_BUTTON_GAP;
        }
        result_buttons.reverse();
    }

    // The footer's actions, row by row from the left.
    let mut hints = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let ry = footer_y + REPOS_FOOTER_PAD + r as i32 * REPOS_HINT_ROW;
        let by = ry + (REPOS_HINT_ROW - ROW_BUTTON) / 2;
        let mut hx = inner_x;
        for &(i, hw) in row {
            hints.push((i, Rectangle::new((hx, by).into(), (hw, ROW_BUTTON).into())));
            hx += hw + HINT_GAP;
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

    // The branch menu under the button, its right edge on the button's:
    // its search, then as many rows as fit above the field.
    let mut branch_search = None;
    let branch_menu = view.branch.menu.as_ref().map(|menu| {
        let widest = menu
            .rows
            .iter()
            .chain([&menu.empty, &menu.placeholder, &menu.query.text])
            .map(|r| text_extent(r, FONT_SIZE, BUTTON_CHAR))
            .max()
            .unwrap_or(0);
        let mw = (widest + 2 * BRANCH_ROW_PAD + BRANCH_CHECK)
            .clamp(pw, BRANCH_MENU_MAX.max(pw))
            .min(inner_w);
        let my = branch.loc.y + branch.size.h + BRANCH_MENU_GAP;
        let rows_y = my + 2 * BRANCH_MENU_PAD + REPOS_ROW;
        let room = (field.loc.y - REPOS_PAD - rows_y - BRANCH_MENU_PAD).max(0) / REPOS_ROW;
        let shown = menu.rows.len().max(1).min(room.max(1) as usize);
        let mh = rows_y - my + BRANCH_MENU_PAD + shown as i32 * REPOS_ROW;
        let mx = branch.loc.x + branch.size.w - mw;
        let card = Rectangle::new((mx, my).into(), (mw, mh).into());
        let inner = (mw - 2 * BRANCH_MENU_PAD).max(0);
        branch_search = Some(Rectangle::new(
            (mx + BRANCH_MENU_PAD, my + BRANCH_MENU_PAD).into(),
            (inner, REPOS_ROW).into(),
        ));
        let rows = (0..shown.min(menu.rows.len()))
            .map(|i| {
                Rectangle::new(
                    (mx + BRANCH_MENU_PAD, rows_y + i as i32 * REPOS_ROW).into(),
                    ((mw - 2 * BRANCH_MENU_PAD).max(0), REPOS_ROW).into(),
                )
            })
            .collect();
        (card, rows)
    });

    let mut buttons = result_buttons.clone();
    buttons.extend(hints.iter().map(|&(i, r)| (view.hints[i].button, r)));
    if let Some(s) = &view.sheet {
        buttons.extend(sheet_buttons.iter().map(|&(i, r)| (s.buttons[i], r)));
    }
    Parts {
        layout: ReposLayout {
            panel,
            header,
            tree,
            lines,
            marks,
            detail,
            detail_tabs,
            field,
            result,
            buttons,
            footer,
            sheet,
            branch,
            branch_menu,
            branch_search,
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
        if let Some(i) = rows.iter().position(at) {
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
    for (i, (line, check, expander)) in layout.lines.iter().enumerate() {
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
        if at(line) {
            return Some(ReposHit::Line(i));
        }
    }
    if let Some((tab, _)) = layout.detail_tabs.iter().find(|(_, r)| at(r)) {
        return Some(ReposHit::DetailTab(*tab));
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

/// Signed distance to a "≠" centred on the origin: a repository off the
/// project's feature.
fn not_equal(x: f32, y: f32, s: f32) -> f32 {
    let bars = segment(x, y, -3.0 * s, -1.6 * s, 3.0 * s, -1.6 * s).min(segment(
        x,
        y,
        -3.0 * s,
        1.6 * s,
        3.0 * s,
        1.6 * s,
    ));
    let slash = segment(x, y, 1.6 * s, -4.2 * s, -1.6 * s, 4.2 * s);
    bars.min(slash) - 0.7 * s
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
    /// characters it lacks (`→`, `≠`), in the monospace font.
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
    /// `SHADOW` holding its drop shadow.
    pub(super) fn repos(
        &self,
        view: &ReposView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let parts = parts(view, size);
        let layout = &parts.layout;
        let panel = layout.panel;
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        c.popup(panel.size, HELP_RADIUS, scale, p);
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let across = |c: &mut Canvas, y: i32| {
            c.fill(
                px(SHADOW) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };

        // The header: the title, then where the repositories stand.
        let header = shift(layout.header);
        let picker = shift(layout.branch);
        let (hx, _) = phys(header, scale);
        let hx = hx + px(HELP_PADDING) as f32;
        let hend = px(picker.loc.x - HEADER_GAP) as f32;
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
        if view.lines.is_empty() {
            let first = Rectangle::new(
                (
                    tree.loc.x + REPOS_PAD + REPOS_LINE_INSET,
                    tree.loc.y + REPOS_PAD,
                )
                    .into(),
                ((tree.size.w - 2 * REPOS_PAD).max(0), REPOS_ROW).into(),
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
        for (i, ((rect, check, expander), line)) in layout.lines.iter().zip(&view.lines).enumerate()
        {
            let rects = (shift(*rect), shift(*check), expander.map(shift));
            self.tree_line(&mut c, view, i, line, rects, scale, p);
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
        self.detail_pane(&mut c, view, detail, shift(parts.body), &tabs, scale, p);

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
                .map(|(b, r)| (*b, shift(*r)))
                .collect();
            self.result_bar(&mut c, view, bar, shift(rect), &buttons, scale, p);
        }
        let footer = shift(layout.footer);
        across(&mut c, px(footer.loc.y));
        for &(i, rect) in &parts.hints {
            self.hint_button(&mut c, view, &view.hints[i], shift(rect), scale, p);
        }

        if let (Some(menu), Some((card, rows)), Some(search)) =
            (&view.branch.menu, &layout.branch_menu, layout.branch_search)
        {
            let rows: Vec<Rect> = rows.iter().map(|r| shift(*r)).collect();
            let parts = (shift(*card), shift(search), &rows[..]);
            self.branch_menu(&mut c, view, menu, parts, scale, p);
        }
        match &view.sheet {
            Some(sheet) => self.sheet(&mut c, view, sheet, &parts, shift, scale, p),
            None => {
                // Why a disabled action can't be done, over it.
                let reason = parts.hints.iter().find_map(|&(i, rect)| {
                    let hint = &view.hints[i];
                    let hovered = view.hover == Some(ReposHit::Button(hint.button));
                    hint.disabled
                        .as_ref()
                        .filter(|_| hovered)
                        .map(|why| (why, rect))
                });
                if let Some((why, rect)) = reason {
                    self.reason_tip(&mut c, why, shift(rect), shift(panel), scale, p);
                }
            }
        }
        c.into_buffer(false)
    }

    /// Tree line `i` in its `(rect, checkbox, expander)` (buffer
    /// coordinates): its highlight, then a repository's or a file's line.
    #[allow(clippy::too_many_arguments)]
    fn tree_line(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        i: usize,
        line: &TreeLine,
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
        let hovered = matches!(
            view.hover,
            Some(ReposHit::Line(j) | ReposHit::Check(j) | ReposHit::Expander(j) | ReposHit::Mark(j, _)) if j == i
        );
        let state = match line {
            TreeLine::Repo(repo) => Some(&repo.state),
            TreeLine::File(_) => None,
        };
        if matches!(state, Some(LineState::Armed(_))) {
            c.rounded(x, hy, w, hh, r, mix(p.elevated, p.warning, 0.15));
        } else if view.selected == Some(i) {
            let fill = if view.field_focused {
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

        let check_hovered = view.hover == Some(ReposHit::Check(i));
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
                    let color = if view.hover == Some(ReposHit::Expander(i)) {
                        p.text
                    } else {
                        p.muted
                    };
                    icon_sdf(c, rect, scale, color, |x, y| expander_mark(open, x, y, s));
                }
                let label_x = px(check.loc.x + REPOS_CHECK + REPOS_EXPANDER) as f32;
                self.repo_line(c, repo, (label_x, right), line_band, gap, scale, p);
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
            let fill = match chip.tone {
                Tone::Normal | Tone::Muted => p.element,
                _ => mix(p.elevated, color, 0.18),
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
            let (color, count) = match m.mark {
                RepoMark::OffFeature => (p.warning, 0),
                RepoMark::Dirty => (p.warning, 0),
                RepoMark::Ahead => (p.muted, repo.ahead),
                RepoMark::Behind => (p.muted, repo.behind),
            };
            c.paint_in(m.x as i32, top, m.w as i32 + 1, height, color, |x, y| {
                let (x, y) = (x - cx, y - cy);
                coverage(match m.mark {
                    RepoMark::OffFeature => not_equal(x, y, s),
                    RepoMark::Dirty => x.hypot(y) - 2.5 * s,
                    RepoMark::Ahead => arrow(true, x, y, s),
                    RepoMark::Behind => arrow(false, x, y, s),
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
                    p.muted,
                );
            }
        }
        let mut pen = spans.pen;
        let mut notes: Vec<(String, Rgb)> = Vec::new();
        if repo.no_upstream {
            notes.push(("no upstream".into(), p.placeholder));
        }
        if repo.stashes > 0 {
            notes.push((format!("{} stashed", repo.stashes), p.placeholder));
        }
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

    /// The detail pane in `rect`: its title and tabs, then its lines in
    /// `body` in the monospace font, added and removed ones on a tint.
    #[allow(clippy::too_many_arguments)]
    fn detail_pane(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        rect: Rect,
        body: Rect,
        tabs: &[(DetailTab, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let title = Rectangle::new(rect.loc, (rect.size.w, REPOS_DETAIL_TITLE).into());
        let (x, mut right) = phys(title, scale);
        let x = x + px(HELP_PADDING) as f32;
        if let Some((_, first)) = tabs.first() {
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
        let border = px(1).max(1);
        c.fill(
            px(rect.loc.x) + border,
            px(rect.loc.y + REPOS_DETAIL_TITLE) - border,
            px(rect.loc.x + rect.size.w) - px(rect.loc.x) - 2 * border,
            border,
            p.border_variant,
        );

        let (bx, bend) = phys(body, scale);
        // Tints reach across the pane, short of its edges.
        let (tint_x, tint_end) = (
            px(rect.loc.x) + border,
            px(rect.loc.x + rect.size.w) - border,
        );
        for (i, line) in view.detail.lines.iter().enumerate() {
            let ly = body.loc.y + i as i32 * REPOS_DETAIL_LINE;
            if ly + REPOS_DETAIL_LINE > body.loc.y + body.size.h {
                break;
            }
            let row = Rectangle::new(
                (body.loc.x, ly).into(),
                (body.size.w, REPOS_DETAIL_LINE).into(),
            );
            let row_band = band(row, scale);
            let (color, tint) = match line.tone {
                DiffTone::Plain => (p.text, None),
                DiffTone::Added => (p.success, Some(mix(p.elevated, p.success, 0.12))),
                DiffTone::Removed => (p.error, Some(mix(p.elevated, p.error, 0.12))),
                DiffTone::Hunk => (p.accent, None),
                DiffTone::Header => (p.muted, None),
                DiffTone::Error => (p.error, None),
            };
            if let Some(tint) = tint {
                c.fill(tint_x, row_band.0, tint_end - tint_x, row_band.1, tint);
            }
            let text = line.text.replace('\t', "    ");
            self.mono_text(c, &text, REPOS_MONO_SIZE, bx, bend, row_band, scale, color);
        }
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
        buttons: &[(ReposButton, Rect)],
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
        for &(button, rect) in buttons {
            let (bx, by, bw, bh) = rect_px(rect, scale);
            let b = ((scale.round() as i32).max(1)) as f32;
            let r = BUTTON_RADIUS * s;
            let hot = view.hover == Some(ReposHit::Button(button));
            let fill = if hot { p.selected } else { p.element };
            c.rounded(bx, by, bw, bh, r, p.border);
            c.rounded(bx + b, by + b, bw - 2.0 * b, bh - 2.0 * b, r - b, fill);
            self.centered(c, button.label(), SMALL_FONT_SIZE, rect, scale, p.text);
        }
    }

    /// A footer action in `rect`: its key on a keycap, then what it does;
    /// lighter under the pointer, dimmed when it can't be done now.
    fn hint_button(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        hint: &Hint,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let r = BUTTON_RADIUS * s;
        let disabled = hint.disabled.is_some();
        if view.hover == Some(ReposHit::Button(hint.button)) {
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
                hint.button.label(),
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
            hint.button.label(),
            SMALL_FONT_SIZE,
            cap_x + cap_w + (HINT_KEY_GAP as f32 * s).round(),
            right - (HINT_PAD as f32 * s).round() / 2.0,
            hint_band,
            scale,
            label_color,
        );
    }

    /// `why` an action in `rect` can't be done, on a small card over it,
    /// kept in `panel` (buffer coordinates).
    fn reason_tip(
        &self,
        c: &mut Canvas,
        why: &str,
        rect: Rect,
        panel: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let pad = (PADDING * s).round();
        let w = (self.mixed_width(why, SMALL_FONT_SIZE, scale) + 2.0 * pad).round();
        let (_, panel_y, panel_w, _) = rect_px(panel, scale);
        let (panel_x, edge) = phys(panel, scale);
        let max_w = panel_w - 2.0 * px(HELP_PADDING) as f32;
        let w = w.min(max_w);
        let h = px(TIP_HEIGHT) as f32;
        let x = (px(rect.loc.x) as f32)
            .min(edge - px(HELP_PADDING) as f32 - w)
            .max(panel_x + px(HELP_PADDING) as f32);
        let y = (px(rect.loc.y) as f32 - px(TIP_GAP) as f32 - h).max(panel_y);
        let b = px(1).max(1) as f32;
        let r = BUTTON_RADIUS * s;
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        self.mixed_text(
            c,
            why,
            SMALL_FONT_SIZE,
            x + pad,
            x + w - pad / 2.0,
            (y as i32, h as i32),
            scale,
            p.muted,
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

    /// The branch menu, a card in `card` with its search in `search`, then
    /// a row per branch in `rows`: the one Enter picks lit, the one they
    /// are all on checked.
    fn branch_menu(
        &self,
        c: &mut Canvas,
        view: &ReposView,
        menu: &BranchMenuView,
        (card, search, rows): (Rect, Rect, &[Rect]),
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
        if menu.rows.is_empty() {
            let row = Rectangle::new(
                (
                    card.loc.x + BRANCH_MENU_PAD,
                    search.loc.y + search.size.h + BRANCH_MENU_PAD,
                )
                    .into(),
                ((card.size.w - 2 * BRANCH_MENU_PAD).max(0), REPOS_ROW).into(),
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
        for (i, (rect, label)) in rows.iter().zip(&menu.rows).enumerate() {
            let (x, y, w, h) = rect_px(*rect, scale);
            let r = (MENU_RADIUS - 2.0) * s;
            if menu.picked == Some(i) {
                c.rounded(x, y, w, h, r, p.selected);
            } else if view.hover == Some(ReposHit::BranchRow(i)) {
                c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
            }
            let mark = Rectangle::new(
                (
                    rect.loc.x + rect.size.w - BRANCH_ROW_PAD / 2 - BRANCH_CHECK,
                    rect.loc.y,
                )
                    .into(),
                (BRANCH_CHECK, rect.size.h).into(),
            );
            if menu.current == Some(i) {
                icon_sdf(c, mark, scale, p.accent, |x, y| check(x, y, s));
            }
            let pad = BRANCH_ROW_PAD as f32 * s;
            let end = (mark.loc.x as f64 * scale) as f32;
            let row_band = band(*rect, scale);
            self.mixed_text(c, label, FONT_SIZE, x + pad, end, row_band, scale, p.text);
        }
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

#[cfg(test)]
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
            off_feature: None,
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
            key: key.into(),
            disabled: disabled.map(Into::into),
        }
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
                }),
                state: LineState::Attention,
                ..repo("herdr", Some("feat/x"))
            }),
            file('U', "src/api.rs", Check::Off),
            TreeLine::Repo(RepoLine {
                check: Check::On,
                expanded: Some(false),
                off_feature: Some("feat/x".into()),
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
                }),
                ..repo("scratch", None)
            }),
            TreeLine::Repo(RepoLine {
                state: LineState::Armed("Force push again to overwrite origin".into()),
                ahead: 1,
                behind: 3,
                ..repo("infra", Some("feat/x"))
            }),
        ];
        ReposView {
            title: "gerdr".into(),
            summary: "feat/x · 3 of 4 on it · 2 dirty · 1 conflict · fetched 2m ago".into(),
            lines,
            selected: Some(1),
            hover: Some(ReposHit::Line(6)),
            field: Line::from("Add multi-repo panel"),
            field_focused: true,
            placeholder: "Commit message or branch name".into(),
            error: Some("Type a commit message first".into()),
            result: Some(ResultBar {
                text: "Commit and push: 2 done · 1 conflict".into(),
                tone: Tone::Warning,
                buttons: vec![Undo, RetryFailed],
            }),
            hints: vec![
                hint(Branch, "Ctrl+N", None),
                hint(StartFeature, "Ctrl+Shift+N", None),
                hint(Join, "Ctrl+J", None),
                hint(Commit, "Ctrl+Enter", None),
                hint(CommitPush, "Ctrl+Shift+Enter", None),
                hint(Push, "Ctrl+P", None),
                hint(ForcePush, "Ctrl+Shift+P", Some("nothing has diverged")),
                hint(Fetch, "Ctrl+F", None),
                hint(Pull, "Ctrl+L", None),
                hint(Merge, "Ctrl+M", None),
                hint(Rebase, "Ctrl+Shift+M", None),
                hint(Park, "Ctrl+K", None),
                hint(Resume, "Ctrl+Shift+K", Some("no parked changes")),
                hint(OpenInZed, "Ctrl+E", None),
                hint(MarkResolved, "Ctrl+R", None),
                hint(Continue, "Ctrl+G", None),
                hint(Abort, "Ctrl+Shift+Delete", None),
            ],
            detail: DetailView {
                tab: DetailTab::Diff,
                title: "src/git.rs".into(),
                lines: vec![
                    detail_line("diff --git a/src/git.rs b/src/git.rs", DiffTone::Header),
                    detail_line("index 3f2a1c0..9b8e7d6 100644", DiffTone::Header),
                    detail_line("--- a/src/git.rs", DiffTone::Header),
                    detail_line("+++ b/src/git.rs", DiffTone::Header),
                    detail_line(
                        "@@ -104,6 +104,9 @@ pub fn status(dir: &Path)",
                        DiffTone::Hunk,
                    ),
                    detail_line("     let root = toplevel(dir)?;", DiffTone::Plain),
                    detail_line(
                        "-    let out = git(dir, &[\"status\", \"--porcelain=v2\"])?;",
                        DiffTone::Removed,
                    ),
                    detail_line(
                        "+    let out = git(dir, &[\"status\", \"--porcelain=v2\", \"-z\"])?;",
                        DiffTone::Added,
                    ),
                    detail_line("+\tlet changes = parse_changes(&out);", DiffTone::Added),
                    detail_line(
                        "     Ok(GitStatus { root, ..parse(&out) })",
                        DiffTone::Plain,
                    ),
                    detail_line("error: hook pre-commit failed", DiffTone::Error),
                ],
            },
            sheet: None,
            empty: "No repositories in this project".into(),
            branch: BranchPicker {
                label: "mixed branches".into(),
                mixed: true,
                enabled: true,
                menu: None,
            },
        }
    }

    /// `full` with the branch menu open, the pointer on its second row.
    fn with_menu() -> ReposView {
        let rows = ["main / master", "feat/x", "production", "release/2.1"];
        ReposView {
            branch: BranchPicker {
                menu: Some(BranchMenuView {
                    rows: rows.map(String::from).to_vec(),
                    current: Some(1),
                    picked: Some(0),
                    empty: "No branch they all have".into(),
                    query: Line::default(),
                    placeholder: "Find a branch".into(),
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

    /// Nothing in it: no repositories, hints, error or result.
    fn empty() -> ReposView {
        ReposView {
            lines: Vec::new(),
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
                // The branch picker in the header, its menu above the field.
                assert!(inside(l.header, l.branch), "{size:?}");
                parts.push(l.branch);
                assert_eq!(l.branch_menu.is_some(), view.branch.menu.is_some());
                if let (Some((menu, rows)), Some(m)) = (&l.branch_menu, &view.branch.menu) {
                    parts.push(*menu);
                    assert!(menu.loc.y >= l.branch.loc.y + l.branch.size.h);
                    assert!(menu.loc.y + menu.size.h <= l.field.loc.y, "{size:?}");
                    assert!(menu.size.w >= l.branch.size.w);
                    assert!(!rows.is_empty() || m.rows.is_empty());
                    assert!(rows.len() <= m.rows.len());
                    for row in rows {
                        assert!(inside(*menu, *row));
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
                let page = view.lines.len().min(repos_page(size).0);
                assert!(l.lines.len() >= page, "{size:?}");
                // The footer's actions stay in the footer and the result
                // bar's in it.
                let n_result = view.result.as_ref().map_or(0, |r| r.buttons.len());
                for (i, (_, rect)) in l.buttons.iter().enumerate() {
                    if i < n_result {
                        assert!(inside(l.result.unwrap(), *rect));
                    } else if l.sheet.is_none_or(|s| !inside(s, *rect)) {
                        assert!(inside(l.footer, *rect), "{size:?}: {rect:?}");
                    }
                }
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
    fn the_footer_wraps_and_keeps_every_action_on_a_wide_output() {
        let view = full();
        let l = repos_layout(&view, (2560, 1440).into());
        let hints = l
            .buttons
            .iter()
            .filter(|(b, _)| view.hints.iter().any(|h| h.button == *b))
            .count();
        assert_eq!(hints, view.hints.len());
        let narrow = repos_layout(&view, (800, 600).into());
        assert!(narrow.footer.size.h > footer_h(1));
        assert!(narrow.footer.size.h <= footer_h(REPOS_FOOTER_ROWS));
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
                for (b, r) in &l.buttons {
                    assert_eq!(hit(*r), Some(ReposHit::Button(*b)));
                }
                for (t, r) in &l.detail_tabs {
                    assert_eq!(hit(*r), Some(ReposHit::DetailTab(*t)));
                }
                assert_eq!(hit(l.field), Some(ReposHit::Field));
                assert_eq!(hit(l.detail), Some(ReposHit::Detail));
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
        assert!(marks.contains(&(6, RepoMark::OffFeature)), "{marks:?}");
        assert!(marks.contains(&(6, RepoMark::Behind)), "{marks:?}");
        let TreeLine::Repo(repo) = &view.lines[6] else {
            panic!("a repository");
        };
        assert_eq!(RepoMark::OffFeature.tip(repo), "Not on the feature, feat/x");
        assert_eq!(RepoMark::Behind.tip(repo), "1 commit to pull");
        assert_eq!(RepoMark::Dirty.tip(repo), "Changes not committed");
        let more = RepoLine {
            ahead: 3,
            ..repo.clone()
        };
        assert_eq!(RepoMark::Ahead.tip(&more), "3 commits to push");
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
        }
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
            let page = (repos_page(size).0 - 1).min(BRANCH_MENU_ROWS);
            assert!(rows.len() >= page, "{size:?}: {} < {page}", rows.len());
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
            let (tree_rows, detail_rows) = repos_page(size);
            assert!(tree_rows >= 1 && detail_rows >= 1);
            if h >= 600 {
                assert!(tree_rows >= 8, "{size:?}: {tree_rows}");
                assert!(detail_rows >= 8, "{size:?}: {detail_rows}");
            }
            assert_eq!(repos_page(size), (tree_rows, detail_rows));
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
                    assert!(parts.body.size.h >= detail_rows as i32 * REPOS_DETAIL_LINE);
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

    #[test]
    fn it_paints() {
        let painter = Painter::new();
        let disabled_hover = ReposView {
            hover: Some(ReposHit::Button(ReposButton::ForcePush)),
            field_focused: false,
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
            long_sheet,
            with_menu(),
            with_empty_menu(),
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
            (
                "mark",
                ReposView {
                    hover: Some(ReposHit::Mark(6, RepoMark::OffFeature)),
                    ..full()
                },
            ),
        ];
        for (theme, scale) in [(Theme::Dark, 1.0), (Theme::Light, 1.0), (Theme::Dark, 2.0)] {
            for (name, view) in &cases {
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
