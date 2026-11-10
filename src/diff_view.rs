//! The diff view: every change of one or more checkouts one after another,
//! as one diff (per checkout a header, per file a header over its hunks,
//! the words changed within a line marked), with a line cursor and caret,
//! review comments written on its lines (see `review`), a search (see
//! `search`) and vim's deletes, yanks and puts (see `vim`). The
//! repositories panel shows it as its Diff tab, and the worktrees popup
//! for a worktree against what its branch merges into: whatever it learns
//! does both.
//!
//! It knows its checkouts as `Section`s, lent by its host with every call
//! (`Ctx`), and the diffs read for them (`set_diffs`). What it can't do
//! itself it asks for (`Outcome`); what its host follows (the file the
//! cursor went to, the keyboard taken, why a key did nothing) it leaves
//! in `Signals` for the host to take.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    chrome::{
        Check, Density, DetailLine, DetailRow, DetailTab, DetailView, DiffFile, DiffRepo, DiffTone,
        FindingHead, Hint, ReposButton, ReposHit, RowMark, Tone,
    },
    diff::{FILE_LINES, FileDiff, LineKind},
    diff_edit::{FileEdit, Put, Register},
    feature::review_import::{Decision, FindingState},
    feature::review_items::{self, ItemOf},
    find,
    git::Change,
    review::{self, Comment, Quoted, Review},
    scroll::{Bounds, Scroll},
    text_field::{Edit, TextField},
};

mod restore;
mod search;
mod vim;

use search::DiffSearch;

pub const NO_REVIEW: &str = "No review comments: in the diff, press c on a line";
/// Why an edit, restore or comment does nothing in a read-only diff.
pub const READ_ONLY: &str = "A commit's diff can't be edited";
/// Why a copy did nothing with no cursor.
pub(crate) const NOWHERE: &str = "Move the cursor to a line first";
/// Why a comment that went to the Build agent can't be changed.
pub const SENT_COMMENT: &str = "That comment went to the Build agent to fix";
/// Why `f` does nothing on a comment of the user's own.
pub const YOUR_COMMENT: &str = "This is your comment: c edits it, Del deletes it";
/// Why a dismissal did nothing.
pub const NO_REASON: &str = "Not dismissed: say why first";
/// Why a finding can't be commented on or reworded now.
const NOTES_CLOSED: &str = "A finding changes only while AI review decides";
/// The same of a suggestion shown in the plan.
const SUGGESTION_NOTES_CLOSED: &str = "A suggestion changes only while Plan review decides";
/// Why `f` does nothing on a suggestion of the user's own.
pub const YOUR_SUGGESTION: &str = "This is your suggestion: c edits it, Del deletes it";
/// Why a rejection did nothing.
pub const NO_REJECT_REASON: &str = "Not rejected: say why first";
/// Why `f` does nothing on a question with no answer yet.
pub const UNANSWERED: &str = "Answer the question first: c writes the answer, then f accepts";
/// Why `]` and `[` did nothing.
pub const NO_COMMENTS: &str = "No findings or comments here";

/// How an AI review finding's card is coloured: by severity, muted once
/// dismissed.
/// How many characters less a row of the user's comment on a finding holds,
/// its card being inside the finding's.
const NOTE_COLS: usize = 4;

/// Whether `comment` is the user's comment on an AI review finding.
fn is_note(comment: &Comment) -> bool {
    comment.flag.as_ref().is_some_and(|flag| flag.note)
}

/// The tone of the finding whose card `comment` is inside, if it is.
fn note_tone(comment: &Comment) -> Option<Tone> {
    (comment.flag.as_ref())
        .filter(|flag| flag.note)
        .map(finding_tone)
}

fn finding_tone(flag: &review::Flag) -> Tone {
    use crate::feature::review_import::Severity;
    match (flag.state, flag.severity) {
        (FindingState::Dismissed | FindingState::Fixed, _) => Tone::Muted,
        (_, Severity::High) => Tone::Error,
        (_, Severity::Medium) => Tone::Warning,
        (_, Severity::Low) => Tone::Normal,
    }
}

/// What a finding's card says of where it stands; none while undecided.
/// A suggestion's words are its own: accepted, rejected.
fn finding_state_word(flag: &review::Flag) -> Option<&'static str> {
    let words = review_items::words_in(flag.of, flag.pr);
    match (flag.of, flag.state) {
        (_, FindingState::Open) => None,
        (_, FindingState::Fix) => Some(words.wanted),
        (_, FindingState::Dismissed) => Some(words.refused),
        (ItemOf::Suggestion, FindingState::Sent | FindingState::Fixed) => None,
        (ItemOf::Finding, FindingState::Sent) if flag.made => Some("fix made"),
        (ItemOf::Finding, FindingState::Sent) if flag.sent => Some("sent to Build"),
        (ItemOf::Finding, FindingState::Sent) => Some("sent to fix"),
        (ItemOf::Finding, FindingState::Fixed) => Some("fixed"),
        (_, FindingState::Posted) => Some("on GitHub"),
    }
}

/// Why `f` does nothing on an item `of` of the user's own.
fn yours_refused(of: ItemOf) -> &'static str {
    match of {
        ItemOf::Finding => YOUR_COMMENT,
        ItemOf::Suggestion => YOUR_SUGGESTION,
    }
}

/// Why a refusal of an item `of` with no reason did nothing.
fn no_reason(of: ItemOf) -> &'static str {
    match of {
        ItemOf::Finding => NO_REASON,
        ItemOf::Suggestion => NO_REJECT_REASON,
    }
}

/// What a flag's decide button says on an item `of`: its kind's words, as
/// its card on the page says them (on a review that posts to GitHub, `pr`,
/// those of a comment). `button` is one of `FixFinding`, `DismissFinding`
/// or `ReopenFinding`; `state` where the item stands.
fn decide_label(of: ItemOf, pr: bool, button: ReposButton, state: FindingState) -> String {
    let words = review_items::words_in(of, pr);
    match (of, button, state) {
        (_, ReposButton::FixFinding, _) => words.want.into(),
        (_, ReposButton::DismissFinding, _) => words.refuse.into(),
        (_, ReposButton::ReopenFinding, FindingState::Fix) => words.unwant.into(),
        (_, ReposButton::ReopenFinding, _) => words.reopen.into(),
        _ => button.label().into(),
    }
}

/// A checkout the diff shows, as its host lends it.
#[derive(Clone, Copy, Debug)]
pub struct Section<'a> {
    /// Its top folder: who it is, as the diff is read again.
    pub root: &'a Path,
    /// What its header calls it.
    pub label: &'a str,
    pub branch: Option<&'a str>,
    /// The commit checked out, for the review copied to say what it was
    /// written against.
    pub head: Option<&'a str>,
    /// Its changed files: none, and it isn't shown.
    pub changes: &'a [Change],
}

/// Where the keyboard is, as the host has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keys {
    /// The diff has it.
    #[default]
    Diff,
    /// Another part of the host has it (the repositories panel's tree).
    Other,
    /// A field of the host has it: typing goes there.
    Field,
}

/// The checkbox of the file at `path` of the checkout at `root`.
pub type CheckOf<'a> = &'a dyn Fn(&Path, &str) -> Check;

/// What a file's header says of it, muted before its counts, by the
/// repository's top folder and the file's path: `changed since viewed`.
pub type NoteOf<'a> = &'a dyn Fn(&Path, &str) -> Option<String>;

/// What the diff's keys, clicks and views need from its host.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    /// In the order the diff shows them.
    pub sections: &'a [Section<'a>],
    /// How many rows fit.
    pub page: usize,
    /// How many characters a review comment's row holds.
    pub cols: usize,
    pub density: Density,
    pub keys: Keys,
    /// Over the diff, before its counts: `All changes`.
    pub lead: &'a str,
    /// What it says with no changes at all.
    pub empty: &'a str,
    /// The title counts the checkouts with changes too.
    pub count_sections: bool,
    /// Each file's checkbox on its header: none without.
    pub check: Option<CheckOf<'a>>,
    /// What each file's header says of it: none without.
    pub note: Option<NoteOf<'a>>,
    /// The file the host has selected, its header marked.
    pub selected: Option<(&'a Path, &'a str)>,
    /// What it shows isn't the files as they are (a commit's diff): no
    /// edits, restores or review comments from it.
    pub read_only: bool,
    /// Comments work but the keys changing the files don't, or are refused
    /// by the host (a feature's code review, a locked file): the title
    /// doesn't offer them. `read_only` says as much.
    pub no_edits: bool,
}

impl std::fmt::Debug for Ctx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("sections", &self.sections)
            .field("page", &self.page)
            .field("keys", &self.keys)
            .finish_non_exhaustive()
    }
}

/// What the diff asked its host for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Put the review, as Markdown, on the clipboard: `count` comments.
    Copy {
        text: String,
        count: usize,
    },
    /// Open these files of the checkout at `root` in its Zed, the first at
    /// `at` (else where Zed puts it).
    OpenInZed {
        root: PathBuf,
        paths: Vec<String>,
        at: Option<Position>,
    },
    /// Check the file at `path` of the checkout at `root`, or uncheck it.
    Toggle {
        root: PathBuf,
        path: String,
    },
    /// Say this briefly.
    Notice(&'static str),
    /// Cut this out of a file, as a delete in the diff asks.
    Cut(FileEdit),
    /// Keep this, to paste, and copy it.
    Yank(Register),
    /// Put what was yanked or deleted last in a file.
    Paste(Put),
    /// Undo the last cut in the file at the caret (by root and path;
    /// with none, the last anywhere); do the last undone there again.
    UndoCut(Option<(PathBuf, String)>),
    RedoCut(Option<(PathBuf, String)>),
    /// Ask whether to delete every review comment (`clear_sheet`), then
    /// `clear_review`.
    Clear,
    /// Ask whether to delete those on lines no longer in the diff, then
    /// `drop_outdated`.
    ClearOutdated,
}

impl Outcome {
    /// Whether it changes a file: that doesn't fit a diff that isn't the
    /// files as they are. A yank takes what the diff shows: it fits any.
    pub(crate) fn edits(&self) -> bool {
        matches!(
            self,
            Outcome::Cut(_) | Outcome::Paste(_) | Outcome::UndoCut(_) | Outcome::RedoCut(_)
        )
    }
}

/// Where in a file Zed opens it: a line (from 1) and, on one in the file
/// as it is now, the caret's char of it as the diff shows it (tabs as
/// four, as `diff_edit::Spot` has it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub col: Option<usize>,
}

/// Where the diff's cursor went: a checkout's header, or a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Moved {
    Section(PathBuf),
    File(PathBuf, String),
}

/// What the diff's host follows, left for it to take after a call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// The cursor went there.
    pub moved: Option<Moved>,
    /// The diff took the keyboard.
    pub focus: bool,
    /// Why the last key did nothing, or `Some(None)` to clear what was
    /// said.
    pub error: Option<Option<String>>,
    /// What the user decided of an item shown as a comment, by its kind
    /// and id (see `review::Flag`): an AI review finding, or a suggestion
    /// in the plan. The decision is in the findings' words: Fix is accept,
    /// Dismiss reject, Reopen clear.
    pub decided: Option<(ItemOf, String, Decision)>,
    /// The user's own item to delete, by its kind and id: their comment
    /// kept as a finding, or their own suggestion (`review::Flag::yours`).
    pub delete_own: Option<(ItemOf, String)>,
    /// The item to edit, by its kind and id: its fix or change, or what the
    /// user's own item says, is asked for in a dialog.
    pub edit_finding: Option<(ItemOf, String)>,
    /// What the user's comment on an item now says, by the item's kind and
    /// id: empty, it is deleted.
    pub noted: Option<(ItemOf, String, String)>,
}

/// What the diff scrolls to: a checkout's header, a file's, or the first
/// file in a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place<'a> {
    Section(&'a Path),
    File(&'a Path, &'a str),
    Dir(&'a Path, &'a str),
}

/// The diff's state, kept by who things are as it is read again.
#[derive(Debug, Default)]
pub struct DiffView {
    /// Where it stands, in rows.
    pub(crate) scroll: Scroll,
    /// Each checkout's diff read, by root: the read of its changes it was
    /// for (`reads`), and its files.
    pub(crate) diffs: HashMap<PathBuf, (u64, Vec<FileDiff>)>,
    /// Counts the reads of the changes: a diff read for an earlier one is
    /// read again, still shown meanwhile.
    pub(crate) reads: u64,
    /// Files folded, by (root, path).
    pub(crate) folded: HashSet<(PathBuf, String)>,
    /// Long files shown whole, past `diff::FILE_LINES`, by (root, path).
    pub(crate) whole: HashSet<(PathBuf, String)>,
    /// Rows it can be scrolled past its end: a file folded near it keeps
    /// its header where it was. Scrolling back takes them away.
    pub(crate) slack: usize,
    /// Its rows, worked out as what they show changes, not with every
    /// frame; and the roots of the checkouts they index, in
    /// `Ctx::sections`' order as it was.
    pub(crate) rows: Vec<DiffRow>,
    roots: Vec<PathBuf>,
    /// The row shown (by index into those in view) and the char of its
    /// text under the pointer as it was pressed: the caret goes there.
    pub(crate) pointer_char: Option<(usize, usize)>,
    /// The review comments, to copy for an agent; saved by the host when
    /// `review_dirty`.
    pub(crate) review: Review,
    pub(crate) review_dirty: bool,
    /// The line cursor, once moved or clicked.
    pub(crate) cursor: Option<Cursor>,
    /// The comment being written.
    pub(crate) editing: Option<Editing>,
    /// The user's comments on AI review's findings can change: the feature
    /// is at AI review, its findings read in.
    pub(crate) notes_open: bool,
    /// The user's own comments are refused, and this says why: only the
    /// flagged items take comments (Plan review's suggestions in the plan).
    pub(crate) flags_only: Option<&'static str>,
    /// Each comment's rows (by index into `review.comments`) and the
    /// editor's, as byte ranges into their text, wrapped at `cols`.
    comment_wraps: Vec<Vec<Range<usize>>>,
    editor_wrap: Vec<Range<usize>>,
    /// Every comment placed in the diff, in the order the diff has them
    /// (what `]` and `[` go through), each with its first row; one in a
    /// folded file, or past the lines a long file shows, with the row it
    /// would come at.
    comment_order: Vec<(usize, usize)>,
    cols: usize,
    /// The comments whose lines are gone, as the rows were last worked
    /// out; and the lines comments are on, by `DiffRow::Line`'s indices.
    outdated: HashSet<usize>,
    noted: HashSet<(usize, usize, usize, usize)>,
    /// It shows a file as a page, not as a change: no header rows, every
    /// line, none tinted, long lines going on over more rows (`line_parts`).
    pub(crate) plain: bool,
    /// The lines going on over more rows, by `DiffRow::Line`'s indices: the
    /// chars of each row, in order.
    line_parts: HashMap<(usize, usize, usize, usize), Vec<Range<usize>>>,
    /// The search, while open.
    pub(crate) search: Option<DiffSearch>,
    /// Visual mode, while on.
    pub(crate) visual: Option<vim::Visual>,
    /// What is typed of a vim command so far.
    pending: vim::Pending,
    /// The comment (by index into `review.comments`) a first Del armed:
    /// the next deletes it.
    delete_armed: Option<usize>,
    /// Where the caret goes once the diff is read again after a cut, and
    /// the keys typed meanwhile, held till then.
    caret_after: Option<vim::CaretAfter>,
    queued: Vec<vim::Key>,
    /// What the host takes after a call.
    signals: Signals,
}

/// The diff's line cursor: the row it is on, and the row its range reaches
/// back to (itself for none). On a line, its caret is on char `col` of it
/// (the last, on a shorter one), kept as it goes up and down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub(crate) row: usize,
    pub(crate) from: usize,
    col: usize,
    /// Who they are, to find them again as the rows change.
    keys: (Option<RowKey>, Option<RowKey>),
}

/// A comment being written: on these lines of the file at `path` of the
/// repository at `root`; in place of comment `at` (by index into
/// `review.comments`) when it is one written before.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub(crate) at: Option<usize>,
    root: PathBuf,
    path: String,
    lines: Vec<Quoted>,
    field: TextField,
    /// The reason AI review finding `at` (its id here) is dismissed for,
    /// written in place of a comment.
    dismiss: Option<String>,
    /// The user's comment on this AI review finding: under the finding `at`
    /// when it is new, in place of the comment `at` when it is written.
    note_on: Option<String>,
}

/// A diff row by who it is, to find it again as the rows change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowKey {
    Repo(PathBuf),
    File(PathBuf, String),
    Line(PathBuf, String, Quoted),
    /// A comment's first row, by index into `review.comments`.
    Comment(usize),
    /// Any other row, by index.
    At(usize),
}

/// A row of the diff, by index: repository `k` of `DiffView::roots`,
/// file `f` of its diff, hunk `h` of the file, line `l` of the hunk,
/// note `n` of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffRow {
    Repo(usize),
    /// Its diff isn't read yet.
    Reading(usize),
    File(usize, usize),
    Hunk(usize, usize, usize),
    Line(usize, usize, usize, usize),
    /// Part `part` (from 1, the `Line` before it being the first) of a
    /// line too long for one row, on a page (`DiffView::plain`).
    Wrap(usize, usize, usize, usize, usize),
    Note(usize, usize, usize),
    /// After the first `diff::FILE_LINES` lines of a long file: how many
    /// more there are, Enter or a click showing them.
    More(usize, usize),
    /// Comment `c`'s heading (by index into `review.comments`): who wrote
    /// it, over its rows.
    /// The heading of the comments on no line or file, above the diff.
    General,
    CommentHead {
        at: Option<(usize, usize)>,
        c: usize,
    },
    /// Row `i` of comment `c` (by index into `review.comments`), in file
    /// `at` (as `file`), or with the comments whose lines are gone.
    Comment {
        at: Option<(usize, usize)>,
        c: usize,
        i: usize,
    },
    /// Row `i` of the comment being written, and the line under it saying
    /// what the keys do.
    Editor {
        at: Option<(usize, usize)>,
        i: usize,
    },
    EditorHint {
        at: Option<(usize, usize)>,
    },
    /// The heading over the comments whose lines are gone; and over each
    /// of them, by index into `review.comments`, where it was.
    Outdated(Option<usize>),
    /// Room before a repository's header.
    Gap,
}

impl DiffRow {
    /// The file it is, or is in.
    fn file(self) -> Option<(usize, usize)> {
        match self {
            DiffRow::File(k, f)
            | DiffRow::Hunk(k, f, _)
            | DiffRow::Line(k, f, _, _)
            | DiffRow::Wrap(k, f, _, _, _)
            | DiffRow::Note(k, f, _)
            | DiffRow::More(k, f) => Some((k, f)),
            DiffRow::CommentHead { at, .. }
            | DiffRow::Comment { at, .. }
            | DiffRow::Editor { at, .. }
            | DiffRow::EditorHint { at } => at,
            DiffRow::Repo(_)
            | DiffRow::Reading(_)
            | DiffRow::Outdated(_)
            | DiffRow::General
            | DiffRow::Gap => None,
        }
    }

    /// A repository's header or a file's.
    fn is_header(self) -> bool {
        matches!(self, DiffRow::Repo(_) | DiffRow::File(..))
    }
}

/// Where the diff is scrolled to, by who it is: a repository's header or
/// a file's, and how many rows past it. Kept as the rows change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    root: PathBuf,
    path: Option<String>,
    offset: usize,
}
/// Which of the rows `wrap` broke a text into the caret at byte `caret`
/// is on: where a row ends and the next starts, the next.
fn caret_row(wrap: &[Range<usize>], caret: usize) -> usize {
    wrap.iter().rposition(|r| r.start <= caret).unwrap_or(0)
}

/// `n` comments, in words.
fn comments(n: usize) -> String {
    match n {
        1 => "1 comment".into(),
        n => format!("{n} comments"),
    }
}

/// `n` checkouts, in words.
fn repos(n: usize) -> String {
    if n == 1 {
        "1 repository".into()
    } else {
        format!("{n} repositories")
    }
}

/// A detail line in `tone`.
pub fn detail_line(text: &str, tone: DiffTone) -> DetailLine {
    DetailLine {
        text: text.into(),
        tone,
    }
}

/// Where a list with `page` rows in view standing at `pos` (in rows)
/// stands to show its rows `top` to `bottom` whole, moved the least.
pub fn showing(pos: f64, (top, bottom): (usize, usize), page: usize) -> f64 {
    let page = page.max(1);
    if (top as f64) < pos {
        top as f64
    } else if (bottom + 1) as f64 > pos + page as f64 {
        (bottom + 1).saturating_sub(page) as f64
    } else {
        pos
    }
}

/// A list of `len` rows `unit` logical pixels high with `page` of them in
/// view, a notch of the wheel scrolling one.
pub fn bounds(len: usize, page: usize, unit: i32) -> Bounds {
    let page = page.max(1);
    Bounds {
        max: len.saturating_sub(page) as f64,
        notch: 1.0,
        unit: unit as f64,
        next: unit as f64,
        prev: unit as f64,
        view: (page as i32 * unit) as f64,
    }
}

/// Stand `scroll` at `pos` unless it stands there already.
pub fn move_to(scroll: &mut Scroll, pos: f64) {
    if scroll.pos() != pos {
        scroll.jump(pos);
    }
}

/// The key a diff button is pressed with, as its chip says: the panel's
/// one table of keys has it (`repos::key_name`).
fn key_name(button: ReposButton) -> &'static str {
    crate::repos::key_name(button)
}

/// What the checkout at `root` is called: its section's label, else its
/// folder's name.
fn label_of(root: &Path, ctx: &Ctx) -> String {
    ctx.sections.iter().find(|i| i.root == root).map_or_else(
        || {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        },
        |i| i.label.to_owned(),
    )
}

/// How many chars past the caret show before a long line's row ends.
const CARET_ROOM: usize = 6;

/// `line` shown from char `skip` on: its text, its words' and colours'
/// ranges moved by as many, those before it gone.
fn skip_chars(line: &mut crate::diff::Line, skip: usize) {
    let byte = line
        .text
        .char_indices()
        .nth(skip)
        .map_or(line.text.len(), |(b, _)| b);
    line.text.drain(..byte);
    let shift = |r: &Range<usize>| r.start.saturating_sub(skip)..r.end.saturating_sub(skip);
    line.words = line
        .words
        .iter()
        .map(shift)
        .filter(|r| !r.is_empty())
        .collect();
    line.syntax = (line.syntax.iter())
        .map(|(r, kind)| (shift(r), *kind))
        .filter(|(r, _)| !r.is_empty())
        .collect();
}

/// `line`'s chars `part` as a row of their own: `first` keeps its numbers.
fn clip_line(line: &crate::diff::Line, part: &Range<usize>, first: bool) -> crate::diff::Line {
    let mut line = line.clone();
    skip_chars(&mut line, part.start);
    let len = part.len();
    let end = (line.text.char_indices())
        .nth(len)
        .map_or(line.text.len(), |(b, _)| b);
    line.text.truncate(end);
    let clip = |r: &Range<usize>| r.start.min(len)..r.end.min(len);
    line.words = (line.words.iter())
        .map(clip)
        .filter(|r| !r.is_empty())
        .collect();
    line.syntax = (line.syntax.iter())
        .map(|(r, kind)| (clip(r), *kind))
        .filter(|(r, _)| !r.is_empty())
        .collect();
    if !first {
        (line.number, line.old) = (0, 0);
    }
    line
}

/// `text` broken at word ends into rows of `width` chars, as `review::wrap`
/// does: each row's chars.
fn char_parts(text: &str, width: usize) -> Vec<Range<usize>> {
    let mut chars = 0;
    (review::wrap(text, width).into_iter())
        .map(|bytes| {
            let start = chars;
            chars += text[bytes].chars().count();
            start..chars
        })
        .collect()
}

/// The chars of `range` on a row showing chars `part`, by char index into
/// that row: none when it has none of them.
fn in_part(range: &Range<usize>, part: &Range<usize>) -> Option<Range<usize>> {
    let (start, end) = (range.start.max(part.start), range.end.min(part.end));
    (start < end).then(|| start - part.start..end - part.start)
}

/// Whether `sym` with `mods` is Ctrl+Shift+C, copying the review.
fn copy_key(mods: &ModifiersState, sym: Keysym) -> bool {
    mods.ctrl && mods.shift && !(mods.alt || mods.logo) && matches!(sym, Keysym::c | Keysym::C)
}

/// Whether `path` is in folder `dir`.
pub fn in_dir(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
}

impl DiffView {
    /// Over the diff: `All changes · 2 repositories · 5 files · +12 −3`.
    fn title(&self, ctx: &Ctx) -> String {
        if let Some(mode) = self.mode_title(!(ctx.read_only || ctx.no_edits)) {
            return mode;
        }
        let dirty: Vec<&Section> = ctx
            .sections
            .iter()
            .filter(|i| !i.changes.is_empty())
            .collect();
        if dirty.is_empty() {
            return ctx.lead.into();
        }
        let files: usize = dirty.iter().map(|i| i.changes.len()).sum();
        let (added, removed) = dirty
            .iter()
            .filter_map(|i| self.diffs.get(i.root))
            .flat_map(|(_, files)| files)
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let files = if files == 1 {
            "1 file".to_string()
        } else {
            format!("{files} files")
        };
        let lead = match ctx.count_sections {
            true => format!("{} · {}", ctx.lead, repos(dirty.len())),
            false => ctx.lead.to_owned(),
        };
        let findings = (self.review.comments.iter())
            .filter_map(|c| c.flag.as_ref())
            .filter(|flag| !flag.yours);
        let (found, open) = findings.fold((0, 0), |(n, open), f| {
            (n + 1, open + usize::from(f.state == FindingState::Open))
        });
        let found = match (found, open) {
            (0, _) => String::new(),
            (1, 0) => " · 1 finding".into(),
            (n, 0) => format!(" · {n} findings"),
            (1, _) => " · 1 finding, open".into(),
            (n, open) => format!(" · {n} findings, {open} open"),
        };
        format!("{lead} · {files} · +{added} −{removed}{found}")
    }

    /// Work the diff's rows out again for `items`, scrolled back to
    /// `anchor`: per repository with changes its header, then each file's
    /// header and, unless folded, its hunks (each after a line saying what
    /// it is in, but the first when git says nothing) and notes; under a
    /// line, the review comments on it, and the comment being written.
    /// Last, the comments whose lines are gone. The cursor stays on who it
    /// was on. A page (`plain`) has no headers, the comments on the whole
    /// file first, and every line, over as many rows as it takes.
    pub(crate) fn build_rows(&mut self, items: &[Section], anchor: Option<Anchor>) {
        let cols = if self.cols == 0 { 80 } else { self.cols };
        let comments = &self.review.comments;
        let nested = cols.saturating_sub(NOTE_COLS).max(1);
        self.comment_wraps = comments
            .iter()
            .map(|c| review::wrap(&c.text, if is_note(c) { nested } else { cols }))
            .collect();
        self.editor_wrap = self
            .editing
            .as_ref()
            .map(|e| match e.note_on {
                Some(_) => review::wrap(&e.field, nested),
                None => review::wrap(&e.field, cols),
            })
            .unwrap_or_default();
        // Shown under their lines, or waiting for their diffs.
        let mut placed: HashSet<usize> = HashSet::new();
        let mut noted = HashSet::new();
        let mut rows = Vec::new();
        let mut order: Vec<(usize, usize)> = Vec::new();
        let mut line_parts = HashMap::new();
        // Those on the change as a whole, above it: no line or file to go by.
        let general: Vec<usize> = (comments.iter().enumerate())
            .filter(|(_, c)| !self.plain && review::is_general(c))
            .map(|(c, _)| c)
            .collect();
        if !general.is_empty() {
            rows.push(DiffRow::General);
            for &c in &general {
                placed.insert(c);
                order.push((c, rows.len()));
                self.push_comment(&mut rows, None, c);
            }
        }
        // One less than fits: the caret past a line's end has room.
        let width = crate::chrome::diff_line_room(cols, self.plain_digits(), true)
            .saturating_sub(1)
            .max(8);
        for (k, item) in items.iter().enumerate() {
            if item.changes.is_empty() {
                continue;
            }
            if !self.plain {
                if !rows.is_empty() {
                    rows.push(DiffRow::Gap);
                }
                rows.push(DiffRow::Repo(k));
            }
            let Some((_, files)) = self.diffs.get(item.root) else {
                rows.push(DiffRow::Reading(k));
                let here = comments
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.root == item.root);
                placed.extend(here.map(|(c, _)| c));
                continue;
            };
            for (f, file) in files.iter().enumerate() {
                if !self.plain {
                    rows.push(DiffRow::File(k, f));
                }
                // The comments on the whole file, under its header, folded
                // or not; and one being written on it.
                let on_file = |root: &Path, path: &str| root == item.root && path == file.path;
                for (c, comment) in comments.iter().enumerate() {
                    if comment.lines.is_empty() && on_file(&comment.root, &comment.path) {
                        placed.insert(c);
                        order.push((c, rows.len()));
                        self.push_comment(&mut rows, Some((k, f)), c);
                    }
                }
                let new_on_file = self.editing.as_ref().is_some_and(|e| {
                    e.at.is_none() && e.lines.is_empty() && on_file(&e.root, &e.path)
                });
                if new_on_file {
                    self.push_editor(&mut rows, Some((k, f)));
                }
                // The file's comments, by the line they go under.
                let mut under: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
                for (c, comment) in comments.iter().enumerate() {
                    if comment.root != item.root || comment.path != file.path {
                        continue;
                    }
                    if let Some(at) = review::locate(file, comment) {
                        under.entry(at).or_default().push(c);
                        placed.insert(c);
                        let lines = comment.lines.iter().filter_map(|q| review::find(file, q));
                        noted.extend(lines.map(|(h, l)| (k, f, h, l)));
                    }
                }
                let new_at = self
                    .editing
                    .as_ref()
                    .filter(|e| e.at.is_none() && e.root == item.root && e.path == file.path)
                    .and_then(|e| review::find(file, e.lines.last()?));
                let key = (item.root.to_path_buf(), file.path.clone());
                if self.folded.contains(&key) {
                    order.extend(under.values().flatten().map(|&c| (c, rows.len())));
                    continue;
                }
                let at = Some((k, f));
                // A long file's first lines, till asked for them all.
                let whole = self.plain || self.whole.contains(&key);
                let total: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
                let mut shown = 0;
                'hunks: for (h, hunk) in file.hunks.iter().enumerate() {
                    if !whole && shown >= FILE_LINES {
                        break;
                    }
                    if !self.plain {
                        rows.push(DiffRow::Hunk(k, f, h));
                    }
                    for l in 0..hunk.lines.len() {
                        if !whole && shown >= FILE_LINES {
                            break 'hunks;
                        }
                        shown += 1;
                        rows.push(DiffRow::Line(k, f, h, l));
                        if self.plain {
                            let parts = char_parts(&hunk.lines[l].text, width);
                            if parts.len() > 1 {
                                let more = 1..parts.len();
                                rows.extend(more.map(|part| DiffRow::Wrap(k, f, h, l, part)));
                                line_parts.insert((k, f, h, l), parts);
                            }
                        }
                        for &c in under.get(&(h, l)).into_iter().flatten() {
                            order.push((c, rows.len()));
                            self.push_comment(&mut rows, at, c);
                        }
                        if new_at == Some((h, l)) {
                            self.push_editor(&mut rows, at);
                        }
                    }
                }
                if !whole && total > FILE_LINES {
                    let past = (under.values().flatten())
                        .filter(|c| !order.iter().any(|(x, _)| x == *c))
                        .map(|&c| (c, rows.len()))
                        .collect::<Vec<_>>();
                    order.extend(past);
                    rows.push(DiffRow::More(k, f));
                }
                rows.extend((0..file.notes.len()).map(|n| DiffRow::Note(k, f, n)));
            }
        }
        // The comments whose lines are gone, and one being written on a
        // line gone meanwhile.
        let gone: Vec<usize> = (0..comments.len())
            .filter(|c| !placed.contains(c))
            .collect();
        let lost = self.editing.as_ref().is_some_and(|e| e.at.is_none())
            && !rows.iter().any(|r| matches!(r, DiffRow::Editor { .. }));
        if !gone.is_empty() || lost {
            if !rows.is_empty() {
                rows.push(DiffRow::Gap);
            }
            rows.push(DiffRow::Outdated(None));
            for &c in &gone {
                order.push((c, rows.len()));
                // A comment on a finding goes under the finding's place.
                if !comments.get(c).is_some_and(is_note) {
                    rows.push(DiffRow::Outdated(Some(c)));
                }
                self.push_comment(&mut rows, None, c);
            }
            if lost {
                self.push_editor(&mut rows, None);
            }
        }
        // Findings not on a line aren't the user's to drop.
        self.outdated = (gone.into_iter())
            .filter(|&c| comments.get(c).is_some_and(|c| c.flag.is_none()))
            .collect();
        self.noted = noted;
        self.line_parts = line_parts;
        self.rows = rows;
        // A comment on a finding is part of the finding: no stop of its own.
        order.retain(|&(c, _)| !comments.get(c).is_some_and(is_note));
        self.comment_order = order;
        self.roots = items.iter().map(|i| i.root.to_path_buf()).collect();
        self.find_hits(false);
        if let Some(anchor) = anchor {
            self.scroll_to(&anchor);
        }
        if let Some(cursor) = self.cursor.take() {
            let row = cursor.keys.0.as_ref().and_then(|k| self.find_row(k));
            let from = cursor.keys.1.as_ref().and_then(|k| self.find_row(k));
            if let Some(row) = row {
                self.set_cursor(row, Some(from.unwrap_or(row)));
                if let Some(c) = &mut self.cursor {
                    c.col = cursor.col;
                }
            }
        }
        self.find_visual();
    }

    /// Comment `c`'s rows, in file `at`; the editor's in its place while
    /// it is edited.
    fn push_comment(&self, rows: &mut Vec<DiffRow>, at: Option<(usize, usize)>, c: usize) {
        let edited = self.editing.as_ref().filter(|e| e.at == Some(c));
        // Under it: a finding's dismissal, or a first comment on a finding.
        let on_finding = (self.review.comments.get(c)).is_some_and(|c| !is_note(c));
        let under =
            edited.is_some_and(|e| e.dismiss.is_some() || (e.note_on.is_some() && on_finding));
        if edited.is_some() && !under {
            return self.push_editor(rows, at);
        }
        let n = self.comment_wraps.get(c).map_or(1, Vec::len);
        rows.push(DiffRow::CommentHead { at, c });
        rows.extend((0..n).map(|i| DiffRow::Comment { at, c, i }));
        if under {
            self.push_editor(rows, at);
        }
    }

    /// The editor's rows, in file `at`, and the line saying its keys.
    fn push_editor(&self, rows: &mut Vec<DiffRow>, at: Option<(usize, usize)>) {
        rows.extend((0..self.editor_wrap.len().max(1)).map(|i| DiffRow::Editor { at, i }));
        rows.push(DiffRow::EditorHint { at });
    }

    /// The rows worked out again when the comments' rows hold another
    /// number of characters now. A page's lines wrap too: its top line
    /// stays at the top.
    pub(crate) fn rewrap(&mut self, ctx: &Ctx) {
        if ctx.cols == self.cols {
            return;
        }
        self.cols = ctx.cols;
        if self.plain {
            let top = self.top_line();
            self.build_rows(ctx.sections, None);
            if let Some(line) = top {
                self.stand_at_line(line, ctx);
            }
            return;
        }
        if !self.review.is_empty() || self.editing.is_some() {
            self.build_rows(ctx.sections, self.diff_anchor());
        }
    }

    /// Diff row `r` by who it is.
    fn row_key(&self, r: usize) -> Option<RowKey> {
        Some(match *self.rows.get(r)? {
            DiffRow::Repo(k) => RowKey::Repo(self.roots.get(k)?.clone()),
            DiffRow::File(..) => {
                let (root, path) = self.diff_path(r)?;
                RowKey::File(root, path)
            }
            DiffRow::Line(k, f, h, l) | DiffRow::Wrap(k, f, h, l, _) => {
                let (root, file) = self.diff_file(k, f)?;
                let line = file.hunks.get(h)?.lines.get(l)?;
                RowKey::Line(root.to_path_buf(), file.path.clone(), Quoted::of(line))
            }
            DiffRow::CommentHead { c, .. }
            | DiffRow::Comment { c, .. }
            | DiffRow::Outdated(Some(c)) => RowKey::Comment(c),
            _ => RowKey::At(r),
        })
    }

    /// The row `key` is, or the nearest that is still there: a line's
    /// file, a file's repository.
    fn find_row(&self, key: &RowKey) -> Option<usize> {
        let last = self.rows.len().checked_sub(1)?;
        match key {
            RowKey::Repo(root) => self.header_row(root, None),
            RowKey::File(root, path) => self
                .header_row(root, Some(path))
                .or_else(|| self.header_row(root, None)),
            RowKey::Line(root, path, quoted) => {
                let head = self.header_row(root, Some(path));
                let line = head.and_then(|h| {
                    let (k, f) = self.rows[h].file()?;
                    let (hunk, l) = review::find(self.diff_file(k, f)?.1, quoted)?;
                    let n = self.rows[h..]
                        .iter()
                        .position(|r| *r == DiffRow::Line(k, f, hunk, l))?;
                    Some(h + n)
                });
                line.or(head).or_else(|| self.header_row(root, None))
            }
            RowKey::Comment(c) => self.rows.iter().position(|r| {
                matches!(*r, DiffRow::CommentHead { c: x, .. } | DiffRow::Outdated(Some(x)) if x == *c)
            }),
            RowKey::At(r) => Some((*r).min(last)),
        }
    }

    /// Put the diff's cursor on row `row`, its range reaching back to
    /// `from` (none without).
    pub(crate) fn set_cursor(&mut self, row: usize, from: Option<usize>) {
        let row = self.owner(row);
        let from = from.map_or(row, |from| self.owner(from));
        let keys = (self.row_key(row), self.row_key(from));
        let col = self.cursor.as_ref().map_or(0, |c| c.col);
        self.cursor = Some(Cursor {
            row,
            from,
            col,
            keys,
        });
    }

    /// The row of the line whose later part diff row `r` is, else `r`.
    pub(crate) fn owner(&self, r: usize) -> usize {
        match self.rows.get(r) {
            Some(&DiffRow::Wrap(.., part)) => r - part,
            _ => r,
        }
    }

    /// The chars of each row of the line diff row `r` is part of, when it
    /// takes more than one.
    fn parts(&self, r: usize) -> Option<&Vec<Range<usize>>> {
        let (DiffRow::Line(k, f, h, l) | DiffRow::Wrap(k, f, h, l, _)) = *self.rows.get(r)? else {
            return None;
        };
        self.line_parts.get(&(k, f, h, l))
    }

    /// The chars of its line diff row `r` shows, when not all of them.
    fn part_of(&self, r: usize) -> Option<Range<usize>> {
        let parts = self.parts(r)?;
        match self.rows[r] {
            DiffRow::Wrap(.., part) => parts.get(part).cloned(),
            _ => parts.first().cloned(),
        }
    }

    /// How many rows the line diff row `r` is part of takes; 1 for any
    /// other row.
    fn part_count(&self, r: usize) -> usize {
        self.parts(r).map_or(1, Vec::len)
    }

    /// The row showing char `col` of the line diff row `r` is part of.
    pub(crate) fn part_row(&self, r: usize, col: usize) -> usize {
        let row = self.owner(r);
        let part = (self.parts(r)).and_then(|parts| parts.iter().rposition(|p| p.start <= col));
        row + part.unwrap_or(0)
    }

    /// The char of its line that char `col` of what diff row `r` shows is:
    /// past the row's end, its last.
    fn line_col(&self, r: usize, col: usize) -> usize {
        match self.part_of(r) {
            Some(part) => part.start + col.min(part.len().saturating_sub(1)),
            None => col,
        }
    }

    /// The digits of a page's line numbers: as its last line's, wherever
    /// it is scrolled, as its lines wrap by them.
    fn plain_digits(&self) -> usize {
        let last = (self.diffs.values())
            .flat_map(|(_, files)| files)
            .flat_map(|file| &file.hunks)
            .filter_map(|hunk| hunk.lines.last())
            .map(|line| line.number)
            .max()
            .unwrap_or(0);
        last.to_string().len().max(2)
    }

    /// The text of diff row `r`, while it is a line.
    pub(crate) fn line_text(&self, r: usize) -> Option<&str> {
        Some(&self.line_at(r)?.text)
    }

    /// The line at diff row `r`, while it is one.
    pub(super) fn line_at(&self, r: usize) -> Option<&crate::diff::Line> {
        let DiffRow::Line(k, f, h, l) = *self.rows.get(r)? else {
            return None;
        };
        let (_, file) = self.diff_file(k, f)?;
        file.hunks.get(h)?.lines.get(l)
    }

    /// The root and path of the file the cursor is in, if any: what `u`
    /// undoes the edits of.
    pub(crate) fn cursor_file(&self) -> Option<(PathBuf, String)> {
        self.diff_path(self.cursor.as_ref()?.row)
    }

    /// The diff row the caret is on and the char of its line: on the
    /// cursor, while that is on a line.
    pub(crate) fn caret(&self) -> Option<(usize, usize)> {
        let cursor = self.cursor.as_ref()?;
        let len = self.line_text(cursor.row)?.chars().count();
        Some((cursor.row, cursor.col.min(len.saturating_sub(1))))
    }

    /// The number of the line the cursor is on (`diff::Line::number`),
    /// while it is on a line that isn't removed.
    pub(crate) fn cursor_line(&self) -> Option<usize> {
        let DiffRow::Line(k, f, h, l) = *self.rows.get(self.cursor.as_ref()?.row)? else {
            return None;
        };
        let (_, file) = self.diff_file(k, f)?;
        let line = file.hunks.get(h)?.lines.get(l)?;
        (line.kind != LineKind::Removed).then_some(line.number)
    }

    /// The diff row of line `number` (`diff::Line::number`) that isn't
    /// removed: the first, of the first file that has one.
    pub(crate) fn row_of_line(&self, number: usize) -> Option<usize> {
        (self.rows.iter()).position(|&row| self.kept_line(row) == Some(number))
    }

    /// The diff row of the first line from `number` on that isn't
    /// removed, and that line's number.
    fn row_from_line(&self, number: usize) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize)> = None;
        for (r, &row) in self.rows.iter().enumerate() {
            let Some(n) = self.kept_line(row) else {
                continue;
            };
            if n == number {
                return Some((r, n));
            }
            if n > number && best.is_none_or(|(_, b)| n < b) {
                best = Some((r, n));
            }
        }
        best
    }

    /// The number of the line at `row` if it is one that isn't removed.
    fn kept_line(&self, row: DiffRow) -> Option<usize> {
        let DiffRow::Line(k, f, h, l) = row else {
            return None;
        };
        let (_, file) = self.diff_file(k, f)?;
        let line = file.hunks.get(h)?.lines.get(l)?;
        (line.kind != LineKind::Removed).then_some(line.number)
    }

    /// The first row that is a line that isn't removed, and its number.
    fn first_kept(&self) -> Option<(usize, usize)> {
        (self.rows.iter().enumerate()).find_map(|(r, &row)| Some((r, self.kept_line(row)?)))
    }

    /// The line of the file at the top of the diff as it stands, to the
    /// fraction: the number of the first line shown that isn't removed,
    /// less the rows above it, plus the part of the first row scrolled
    /// past. The rows above the first line stand for the line before it,
    /// from its start (`n - 1` with the diff at its top) to its end. None
    /// without such lines.
    pub(crate) fn top_line(&self) -> Option<f64> {
        let pos = self.scroll.pos().max(0.0);
        let (r1, n1) = self.first_kept()?;
        // (A page has no rows above its first line: at its very top, its
        // preview is at its own.)
        if pos <= 0.0 {
            return Some(n1 as f64 - 1.0);
        }
        if pos < r1 as f64 {
            return Some(n1 as f64 - 1.0 + pos / r1 as f64);
        }
        let first = self.scroll.first();
        // Inside a line on many rows, the rows divide it.
        let line = self.owner(first);
        let parts = self.part_count(line);
        if parts > 1
            && let Some(n) = self.rows.get(line).and_then(|&row| self.kept_line(row))
        {
            return Some(n as f64 + ((first - line) as f64 + self.scroll.part()) / parts as f64);
        }
        let (r, n) = (self.rows.iter().enumerate().skip(first))
            .find_map(|(r, &row)| Some((r, self.kept_line(row)?)))?;
        Some(n as f64 - (r - first) as f64 + self.scroll.part())
    }

    /// Stand with line `line` of the file at the top, to the fraction (see
    /// `top_line`).
    pub(crate) fn stand_at_line(&mut self, line: f64, ctx: &Ctx) {
        let Some((r1, n1)) = self.first_kept() else {
            return;
        };
        if line < n1 as f64 {
            let into = (line - (n1 as f64 - 1.0)).max(0.0);
            return self.stand_at(into * r1 as f64, ctx);
        }
        let whole = line.floor() as usize;
        let Some((row, number)) = self.row_from_line(whole) else {
            return;
        };
        let past = if number == whole {
            line - whole as f64
        } else {
            0.0
        };
        self.stand_at(row as f64 + past * self.part_count(row) as f64, ctx);
    }

    /// Each comment's text by the number of its last line (none for one
    /// on the whole file), the user's only.
    pub(crate) fn comment_lines(&self) -> Vec<(usize, String)> {
        let mut lines: Vec<(usize, String)> = self
            .review
            .comments
            .iter()
            .filter(|c| c.flag.is_none())
            .filter_map(|c| Some((c.lines.last()?.number, c.text.clone())))
            .collect();
        lines.sort();
        lines
    }

    /// Put the caret on char `col` of diff row `row`, the diff scrolled
    /// to show it and its file selected in the tree.
    pub(crate) fn caret_to(&mut self, row: usize, col: usize, ctx: &Ctx) {
        // Selecting by lines, the range reaches on.
        let from = match self.visual {
            Some(vim::Visual::Lines) => self.cursor.as_ref().map(|c| c.from),
            _ => None,
        };
        self.set_cursor(row, from);
        if let Some(cursor) = &mut self.cursor {
            cursor.col = col;
        }
        self.follow(self.part_row(row, col), ctx.page.max(1));
        self.select_diff_row(row, ctx);
    }

    /// A key that moves the caret along its line, as vim's do: `h` and
    /// `l` by a char, `w`, `b` and `e` by words (on to the lines after or
    /// before), `0`, `^` and `$` to the line's start, first char and end.
    /// The first puts the cursor on the first row in view. Left and Right
    /// go as `h` and `l` while the cursor is on a line, else they fold.
    /// Whether `sym` was one.
    fn caret_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym, ctx: &Ctx) -> bool {
        if mods.ctrl || mods.alt || mods.logo {
            return false;
        }
        let sym = match sym {
            Keysym::Left | Keysym::KP_Left if !mods.shift && self.caret().is_some() => Keysym::h,
            Keysym::Right | Keysym::KP_Right if !mods.shift && self.caret().is_some() => Keysym::l,
            sym => sym,
        };
        let letter = !mods.shift
            && matches!(
                sym,
                Keysym::h | Keysym::l | Keysym::w | Keysym::b | Keysym::e
            );
        let symbol = matches!(typed, Keysym::_0 | Keysym::asciicircum | Keysym::dollar);
        if !(letter || symbol) {
            return false;
        }
        if self.cursor.is_none() {
            self.move_cursor(1, false, ctx);
            return true;
        }
        let Some((row, col)) = self.caret() else {
            return true;
        };
        let (row, col) = self.caret_target((row, col), sym, typed);
        self.caret_to(row, col, ctx);
        true
    }

    /// Where the caret goes from char `col` of diff row `row` with a
    /// key `caret_key` takes.
    fn caret_target(
        &self,
        (row, col): (usize, usize),
        sym: Keysym,
        typed: Keysym,
    ) -> (usize, usize) {
        let text = self.line_text(row).unwrap_or_default().to_owned();
        let last = text.chars().count().saturating_sub(1);
        // The next line on (`by` 1) or before (-1), and its text.
        let line = |by: i64| {
            let mut r = row as i64 + by;
            while r >= 0 && (r as usize) < self.rows.len() {
                if let Some(text) = self.line_text(r as usize) {
                    return Some((r as usize, text.to_owned()));
                }
                r += by;
            }
            None
        };
        match (sym, typed) {
            (Keysym::h, _) => (row, col.saturating_sub(1)),
            (Keysym::l, _) => (row, (col + 1).min(last)),
            (_, Keysym::_0) => (row, 0),
            (_, Keysym::asciicircum) => (row, find::first_char(&text)),
            (_, Keysym::dollar) => (row, last),
            (Keysym::w, _) => match find::next_word(&text, col) {
                Some(col) => (row, col),
                None => match line(1) {
                    Some((r, text)) => (r, find::first_char(&text)),
                    None => (row, col),
                },
            },
            (Keysym::b, _) => match find::prev_word(&text, col) {
                Some(col) => (row, col),
                None => match line(-1) {
                    Some((r, text)) => (r, find::prev_word(&text, usize::MAX).unwrap_or(0)),
                    None => (row, col),
                },
            },
            _ => match find::word_end(&text, col + 1) {
                Some(col) => (row, col),
                None => match line(1).and_then(|(r, text)| Some((r, find::word_end(&text, 0)?))) {
                    Some(at) => at,
                    None => (row, col),
                },
            },
        }
    }

    /// The comment diff row `r` is of.
    #[cfg(test)]
    pub(crate) fn comment_of_row(&self, r: usize) -> Option<usize> {
        self.comment_at(r)
    }

    /// The comment diff row `r` is of.
    fn comment_at(&self, r: usize) -> Option<usize> {
        match *self.rows.get(r)? {
            DiffRow::CommentHead { c, .. }
            | DiffRow::Comment { c, .. }
            | DiffRow::Outdated(Some(c)) => Some(c),
            _ => None,
        }
    }

    /// Move the diff's cursor `by` rows (down positive), keeping its range
    /// when `extend`, the diff scrolled to show it and its file selected
    /// in the tree. The first move puts it on the first row in view.
    fn move_cursor(&mut self, by: i64, extend: bool, ctx: &Ctx) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let page = ctx.page.max(1);
        let (row, from) = match &self.cursor {
            Some(c) => (c.row.min(n - 1), c.from.min(n - 1)),
            None => {
                let (sticky, range) = self.diff_window(page);
                let at = if range.is_empty() {
                    sticky.unwrap_or(0)
                } else {
                    range.start
                };
                let at = self.cursor_stop(at, 1);
                if by.abs() == 1 {
                    self.set_cursor(at, None);
                    self.follow(at, page);
                    self.select_diff_row(at, ctx);
                    return;
                }
                (at, at)
            }
        };
        let to = (row as i64).saturating_add(by).clamp(0, n as i64 - 1) as usize;
        let to = self.cursor_stop(to, by.signum());
        self.set_cursor(to, extend.then_some(from));
        self.follow(to, page);
        self.select_diff_row(to, ctx);
    }

    /// The row the cursor stops on going `dir` (1 down, -1 up) to row
    /// `r`: not room between repositories, the editor's rows, or a line's
    /// later parts, the cursor being on its first.
    fn cursor_stop(&self, r: usize, dir: i64) -> usize {
        let stops = |r: usize| {
            !matches!(
                self.rows[r],
                DiffRow::Gap
                    | DiffRow::Editor { .. }
                    | DiffRow::EditorHint { .. }
                    | DiffRow::Wrap(..)
            )
        };
        let n = self.rows.len();
        let ahead = |r: usize| {
            if dir < 0 {
                (0..=r).rev().find(|&r| stops(r))
            } else {
                (r..n).find(|&r| stops(r))
            }
        };
        let back = |r: usize| {
            if dir < 0 {
                (r..n).find(|&r| stops(r))
            } else {
                (0..=r).rev().find(|&r| stops(r))
            }
        };
        ahead(r).or_else(|| back(r)).unwrap_or(r)
    }

    /// Scroll the diff the least to show row `r` whole on a page of
    /// `page`: in a file, under its header pinned over the row above it; a
    /// line on many rows with all of them, when they fit.
    fn follow(&mut self, r: usize, page: usize) {
        let page = page.max(1);
        let line = self.owner(r);
        let parts = self.part_count(line);
        let (top, bottom) = match self.file_head(r) {
            _ if parts > 1 && parts <= page => (line, line + parts - 1),
            _ if parts > 1 => (r, r),
            Some(h) if h < r && page > 1 => (r - 1, r),
            _ => (r, r),
        };
        let last = self.diff_last(page) as f64;
        let to = showing(self.scroll.pos().min(last), (top, bottom), page);
        move_to(&mut self.scroll, to.min(last));
    }

    /// The root and path of the file the cursor is in, and the lines of it
    /// its range takes in.
    fn cursor_lines(&self) -> Option<(PathBuf, String, Vec<Quoted>)> {
        let cursor = self.cursor.as_ref()?;
        let (k, f) = self.rows.get(cursor.row)?.file()?;
        let (root, file) = self.diff_file(k, f)?;
        let (a, b) = (cursor.row.min(cursor.from), cursor.row.max(cursor.from));
        let lines = self.rows[a..=b.min(self.rows.len() - 1)]
            .iter()
            .filter_map(|row| match *row {
                DiffRow::Line(kk, ff, h, l) if (kk, ff) == (k, f) => {
                    file.hunks.get(h)?.lines.get(l).map(Quoted::of)
                }
                _ => None,
            })
            .collect();
        Some((root.to_path_buf(), file.path.clone(), lines))
    }

    /// Write a comment on the lines of the cursor's range (C), or on the
    /// whole file from its header (a page with no cursor: it has none),
    /// or edit the comment the cursor is on.
    fn comment_here(&mut self, ctx: &Ctx) {
        if let Some(c) = self
            .cursor
            .as_ref()
            .and_then(|cur| self.comment_at(cur.row))
        {
            return self.edit_comment(c, ctx);
        }
        if let Some(why) = self.flags_only {
            self.signals.error = Some(Some(why.into()));
            return;
        }
        if self.plain && self.cursor.is_none() {
            return self.comment_file(ctx);
        }
        // On a file's header alone, a comment on the whole file.
        let on_header = self.cursor.as_ref().is_some_and(|cur| {
            cur.row == cur.from && matches!(self.rows.get(cur.row), Some(DiffRow::File(..)))
        });
        let Some((root, path, lines)) = self
            .cursor_lines()
            .filter(|(_, _, l)| !l.is_empty() || on_header)
        else {
            let why = match self.plain {
                true => "Move the cursor to a line to comment on it",
                false => "Move the diff's cursor to a line or a file's header to comment on it",
            };
            self.signals.error = Some(Some(why.into()));
            return;
        };
        self.signals.error = Some(None);
        self.editing = Some(Editing {
            at: None,
            root,
            path,
            lines,
            field: TextField::multiline(""),
            dismiss: None,
            note_on: None,
        });
        self.build_rows(ctx.sections, self.diff_anchor());
        self.follow_editor(ctx);
    }

    /// Write a comment on the whole of the file a page shows, over its
    /// first line.
    fn comment_file(&mut self, ctx: &Ctx) {
        if let Some(why) = self.flags_only {
            self.signals.error = Some(Some(why.into()));
            return;
        }
        let file = self.roots.iter().find_map(|root| {
            let file = self.diffs.get(root)?.1.first()?;
            Some((root.clone(), file.path.clone()))
        });
        let Some((root, path)) = file else {
            return;
        };
        self.signals.error = Some(None);
        self.editing = Some(Editing {
            at: None,
            root,
            path,
            lines: Vec::new(),
            field: TextField::multiline(""),
            dismiss: None,
            note_on: None,
        });
        self.build_rows(ctx.sections, self.diff_anchor());
        self.follow_editor(ctx);
    }

    /// Edit comment `c`, in its place.
    fn edit_comment(&mut self, c: usize, ctx: &Ctx) {
        let Some(comment) = self.review.comments.get(c) else {
            return;
        };
        if let Some(flag) = &comment.flag {
            if let Some(why) = flag.frozen {
                self.signals.error = Some(Some(why.into()));
                return;
            }
            if flag.sent {
                self.signals.error = Some(Some(SENT_COMMENT.into()));
                return;
            }
            // The user's own item is reworded where it was written.
            if flag.yours && !flag.note {
                self.signals.edit_finding = Some((flag.of, flag.id.clone()));
            } else {
                self.write_note(c, ctx);
            }
            return;
        }
        self.signals.error = Some(None);
        self.signals.focus = true;
        self.editing = Some(Editing {
            at: Some(c),
            root: comment.root.clone(),
            path: comment.path.clone(),
            lines: comment.lines.clone(),
            field: TextField::multiline(&comment.text),
            dismiss: None,
            note_on: None,
        });
        self.build_rows(ctx.sections, self.diff_anchor());
        self.follow_editor(ctx);
    }

    /// Why the user's comment on item `flag` can't change now, if so: a
    /// frozen one says why first.
    fn note_refused(&self, flag: &review::Flag) -> Option<String> {
        if let Some(why) = flag.frozen {
            return Some(why.into());
        }
        if !self.notes_open {
            return Some(
                match flag.of {
                    ItemOf::Finding => NOTES_CLOSED,
                    ItemOf::Suggestion => SUGGESTION_NOTES_CLOSED,
                }
                .into(),
            );
        }
        review_items::note_editable(flag.of, &flag.id, flag.state).err()
    }

    /// Open the editor on the user's comment on the finding comment `c` is,
    /// or is the comment on: under the finding when there is none yet.
    fn write_note(&mut self, c: usize, ctx: &Ctx) {
        let comments = &self.review.comments;
        let Some(flag) = comments.get(c).and_then(|comment| comment.flag.clone()) else {
            return;
        };
        if let Some(why) = self.note_refused(&flag) {
            self.signals.error = Some(Some(why));
            return;
        }
        let at = match flag.note || !self.has_note(c) {
            true => c,
            false => c + 1,
        };
        let comment = &comments[at];
        let text = if is_note(comment) {
            comment.text.as_str()
        } else {
            ""
        };
        self.signals.error = Some(None);
        self.signals.focus = true;
        self.editing = Some(Editing {
            at: Some(at),
            root: comment.root.clone(),
            path: comment.path.clone(),
            lines: comment.lines.clone(),
            field: TextField::multiline(text),
            dismiss: None,
            note_on: Some(flag.id),
        });
        self.build_rows(ctx.sections, self.diff_anchor());
        self.follow_editor(ctx);
    }

    /// Whether the finding comment `c` is has the user's comment under it.
    fn has_note(&self, c: usize) -> bool {
        (self.review.comments.get(c + 1)).is_some_and(is_note)
    }

    /// Shift+E on a finding: ask for its fix. Whether the cursor is on one.
    fn edit_fix(&mut self) -> bool {
        let Some((_, flag)) = self.finding_here() else {
            return false;
        };
        let id = (flag.of, flag.id.clone());
        match self.note_refused(flag) {
            Some(why) => self.signals.error = Some(Some(why)),
            None => self.signals.edit_finding = Some(id),
        }
        true
    }

    /// Which kind of item the comment being written is on: a finding when
    /// it is on none.
    fn editing_of(&self) -> ItemOf {
        (self.editing.as_ref())
            .and_then(|e| self.review.comments.get(e.at?)?.flag.as_ref())
            .map_or(ItemOf::Finding, |f| f.of)
    }

    /// Whether a finding's dismissal is being written with no reason yet.
    fn reason_missing(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| e.dismiss.is_some() && e.field.trim().is_empty())
    }

    /// Enter, Save or Dismiss in the editor: keep what is written, as
    /// `save_comment` does; a dismissal with no reason stays open to be
    /// written, or what is typed next would go to the diff as keys.
    fn submit_comment(&mut self, ctx: &Ctx) {
        if self.reason_missing() {
            let of = self.editing_of();
            self.signals.error = Some(Some(no_reason(of).into()));
            return;
        }
        self.save_comment(ctx);
    }

    /// Keep the comment being written: a comment written before, emptied,
    /// is deleted.
    fn save_comment(&mut self, ctx: &Ctx) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let text = editing.field.trim().to_owned();
        let comment = editing.at.and_then(|c| self.review.comments.get(c));
        let of = (comment.and_then(|c| c.flag.as_ref())).map_or(ItemOf::Finding, |f| f.of);
        if let Some(id) = editing.note_on {
            // The record has it, and shows it again from there. Nothing
            // written where there was nothing: nothing to do.
            if comment.is_some_and(is_note) || !text.is_empty() {
                self.signals.noted = Some((of, id, text));
            }
            return self.build_rows(ctx.sections, self.diff_anchor());
        }
        if let Some(id) = editing.dismiss {
            // No reason, no dismissal.
            self.signals.error = Some(match text.is_empty() {
                true => Some(no_reason(of).into()),
                false => None,
            });
            if !text.is_empty() {
                self.signals.decided = Some((of, id, Decision::Dismiss(text)));
            }
            return self.build_rows(ctx.sections, self.diff_anchor());
        }
        match (editing.at, text.is_empty()) {
            (Some(c), true) if c < self.review.comments.len() => {
                self.review.comments.remove(c);
            }
            (Some(c), false) if c < self.review.comments.len() => {
                self.review.comments[c].text = text;
            }
            (None, false) => {
                // Before the findings: those are the host's, after the
                // user's.
                let comments = &self.review.comments;
                let at = (comments.iter().position(|c| c.flag.is_some())).unwrap_or(comments.len());
                self.shift_comment_keys(at, 1);
                let comment = Comment {
                    root: editing.root,
                    path: editing.path,
                    lines: editing.lines,
                    text,
                    flag: None,
                };
                self.review.comments.insert(at, comment);
            }
            _ => {}
        }
        self.review_dirty = true;
        self.build_rows(ctx.sections, self.diff_anchor());
    }

    /// Delete the comment the cursor is on, the cursor going to the row
    /// over it: the first press arms it (and says so), the second, with
    /// the cursor still on it, deletes it.
    fn delete_comment(&mut self, ctx: &Ctx) {
        let Some(cursor) = &self.cursor else {
            return;
        };
        let Some(c) = self.comment_at(cursor.row) else {
            return;
        };
        if let Some(flag) = self.note_here().map(|(_, flag)| flag)
            && let Some(why) = self.note_refused(flag)
        {
            self.signals.error = Some(Some(why));
            return;
        }
        if self.delete_armed != Some(c) {
            self.delete_armed = Some(c);
            let why = "Press Del again to delete the comment";
            self.signals.error = Some(Some(why.into()));
            return;
        }
        self.delete_armed = None;
        self.signals.error = Some(None);
        if self.delete_note() {
            return;
        }
        let Some(cursor) = &self.cursor else {
            return;
        };
        let first = self.find_row(&RowKey::Comment(c)).unwrap_or(cursor.row);
        let above = self.cursor_stop(first.saturating_sub(1), -1);
        self.set_cursor(above, None);
        // A comment after it is one up now.
        if let Some(Cursor { keys, .. }) = &mut self.cursor {
            for key in [&mut keys.0, &mut keys.1].into_iter().flatten() {
                if let RowKey::Comment(x) = key
                    && *x > c
                {
                    *x -= 1;
                }
            }
        }
        self.review.comments.remove(c);
        self.review_dirty = true;
        self.build_rows(ctx.sections, self.diff_anchor());
    }

    /// Delete the user's comment on a finding the cursor is on, the cursor
    /// going to the finding. Whether it was on one.
    fn delete_note(&mut self) -> bool {
        let Some((c, flag)) = self.note_here() else {
            return false;
        };
        let (of, id) = (flag.of, flag.id.clone());
        if let Some(first) = self.find_row(&RowKey::Comment(c - 1)) {
            self.set_cursor(first, None);
        }
        self.signals.noted = Some((of, id, String::new()));
        true
    }

    /// The cursor's keys on comments from `from` on moved `by` (one was
    /// put in or taken out before them).
    fn shift_comment_keys(&mut self, from: usize, by: isize) {
        if let Some(Cursor { keys, .. }) = &mut self.cursor {
            for key in [&mut keys.0, &mut keys.1].into_iter().flatten() {
                if let RowKey::Comment(x) = key
                    && *x >= from
                {
                    *x = x.saturating_add_signed(by);
                }
            }
        }
    }

    /// Whether the cursor is on an AI review finding.
    pub fn on_finding(&self) -> bool {
        self.finding_here().is_some()
    }

    /// The AI review finding the cursor is on, by index into
    /// `review.comments`, and where it stands.
    fn finding_here(&self) -> Option<(usize, &review::Flag)> {
        let c = self.comment_at(self.cursor.as_ref()?.row)?;
        let flag = self.review.comments.get(c)?.flag.as_ref()?;
        if flag.note {
            // On the comment on a finding, what is decided is the finding.
            let finding = c.checked_sub(1)?;
            return Some((finding, self.review.comments.get(finding)?.flag.as_ref()?));
        }
        Some((c, flag))
    }

    /// The user's comment on a finding the cursor is on, by index into
    /// `review.comments`, and where the finding stands.
    fn note_here(&self) -> Option<(usize, &review::Flag)> {
        let c = self.comment_at(self.cursor.as_ref()?.row)?;
        let flag = self.review.comments.get(c)?.flag.as_ref()?;
        flag.note.then_some((c, flag))
    }

    /// Whether the cursor is on a comment of the user's kept as a finding.
    pub fn on_yours(&self) -> bool {
        self.finding_here().is_some_and(|(_, flag)| flag.yours && !flag.sent)
    }

    /// The next undecided finding after `id` in the diff's order, coming
    /// round past the last; `id` itself doesn't count.
    pub fn next_undecided(&self, id: &str) -> Option<String> {
        let flag = |c: usize| self.review.comments.get(c)?.flag.as_ref();
        let order = &self.comment_order;
        let from = order
            .iter()
            .position(|&(c, _)| flag(c).is_some_and(|f| f.id == id))
            .map_or(0, |at| at + 1);
        let rest = order[from..].iter().chain(&order[..from]);
        rest.filter_map(|&(c, _)| flag(c))
            .find(|f| f.state == FindingState::Open && f.id != id)
            .map(|f| f.id.clone())
    }

    /// `f` on a finding: to fix (from undecided or dismissed), or back to
    /// undecided (from to fix). Off one, it says where to go when there
    /// are findings; else it isn't the diff's key.
    fn fix_finding(&mut self) -> bool {
        let Some((_, flag)) = self.finding_here() else {
            let any = self.review.comments.iter().find_map(|c| c.flag.as_ref());
            if let Some(flag) = any {
                let why = review_items::words(flag.of).none_here;
                self.signals.error = Some(Some(why.into()));
            }
            return any.is_some();
        };
        if let Some(why) = flag.frozen {
            self.signals.error = Some(Some(why.into()));
            return true;
        }
        if flag.sent {
            self.signals.error = Some(Some(SENT_COMMENT.into()));
            return true;
        }
        if flag.yours {
            self.signals.error = Some(Some(yours_refused(flag.of).into()));
            return true;
        }
        // A question is accepted by answering it: f waits for the answer.
        if flag.unanswered && flag.state != FindingState::Fix {
            self.signals.error = Some(Some(UNANSWERED.into()));
            return true;
        }
        match review_items::want_key(flag.of, flag.state) {
            Ok(decision) => self.signals.decided = Some((flag.of, flag.id.clone(), decision)),
            Err(why) => self.signals.error = Some(Some(why.into())),
        }
        true
    }

    /// Del on a finding: dismissed, its reason asked first (from
    /// undecided or to fix), or back to undecided (from dismissed).
    /// Whether the cursor is on one.
    fn dismiss_finding(&mut self, ctx: &Ctx) -> bool {
        // Del on the user's comment on it deletes that.
        if self.note_here().is_some() {
            return false;
        }
        let Some((c, flag)) = self.finding_here() else {
            return false;
        };
        if let Some(why) = flag.frozen {
            self.signals.error = Some(Some(why.into()));
            return true;
        }
        if flag.sent {
            self.signals.error = Some(Some(SENT_COMMENT.into()));
            return true;
        }
        // The user's own: nothing to tell themselves, so it just goes.
        if flag.yours {
            self.signals.delete_own = Some((flag.of, flag.id.clone()));
            return true;
        }
        match review_items::refuse_key(flag.of, flag.state) {
            Ok(None) => {
                let (id, comment) = (flag.id.clone(), &self.review.comments[c]);
                self.signals.error = Some(None);
                self.signals.focus = true;
                self.editing = Some(Editing {
                    at: Some(c),
                    root: comment.root.clone(),
                    path: comment.path.clone(),
                    lines: comment.lines.clone(),
                    field: TextField::multiline(""),
                    dismiss: Some(id),
                    note_on: None,
                });
                self.build_rows(ctx.sections, self.diff_anchor());
                self.follow_editor(ctx);
            }
            Ok(Some(decision)) => {
                self.signals.decided = Some((flag.of, flag.id.clone(), decision));
            }
            Err(why) => self.signals.error = Some(Some(why.into())),
        }
        true
    }

    /// Go to the next file (`by` 1) or the one before (-1) whose
    /// checkbox isn't checked, round past the end: its header at the top,
    /// the cursor on it. Whether there was one.
    pub fn jump_unchecked(&mut self, by: i64, ctx: &Ctx) -> bool {
        let Some(check) = ctx.check else {
            return false;
        };
        let n = self.rows.len();
        let from = match &self.cursor {
            Some(c) => c.row,
            None => self.diff_window(ctx.page).1.start,
        };
        let unchecked = |r: usize| {
            matches!(self.rows[r], DiffRow::File(..))
                && self
                    .diff_path(r)
                    .is_some_and(|(root, path)| check(&root, &path) != Check::On)
        };
        let order: Vec<usize> = match by > 0 {
            true => (from + 1..n)
                .chain(0..=from.min(n.saturating_sub(1)))
                .collect(),
            false => (0..from).rev().chain((from..n).rev()).collect(),
        };
        let Some(to) = order.into_iter().find(|&r| r < n && unchecked(r)) else {
            return false;
        };
        let last = self.diff_last(ctx.page);
        self.scroll.jump(to.min(last) as f64);
        self.set_cursor(to, None);
        self.select_diff_row(to, ctx);
        true
    }

    /// Check the file at the cursor (else at the top) or uncheck it, as
    /// Space does.
    pub fn toggle_current(&mut self, ctx: &Ctx) -> Option<Outcome> {
        let head = self.current_file(ctx.page)?;
        self.toggle_diff_file(head, ctx)
    }

    /// Put the cursor on comment `c` (by index into `review.comments`),
    /// the diff scrolled to show it, a third down the page when it was out
    /// of view: its file unfolded, and all of a long one. Whether it is
    /// shown.
    pub fn show_comment(&mut self, c: usize, ctx: &Ctx) -> bool {
        if self.find_row(&RowKey::Comment(c)).is_none()
            && let Some(comment) = self.review.comments.get(c)
        {
            let key = (comment.root.clone(), comment.path.clone());
            if self.folded.remove(&key) | self.whole.insert(key) {
                self.build_rows(ctx.sections, self.diff_anchor());
            }
        }
        let Some(r) = self.find_row(&RowKey::Comment(c)) else {
            return false;
        };
        self.set_cursor(r, None);
        let page = ctx.page.max(1);
        // Its whole card in view, or nothing moves.
        let end = (r..self.rows.len())
            .take_while(|&x| self.comment_at(x) == Some(c))
            .last()
            .unwrap_or(r);
        let room = (page / 4).min(3) as f64;
        let shown = |at: Option<f64>| at.is_some_and(|at| at >= 1.0 && at < page as f64 - room);
        if !(shown(self.shown_at(r, page)) && shown(self.shown_at(end, page))) {
            let last = self.diff_last(page) as f64;
            let to = (r as f64 - (page / 3) as f64).clamp(0.0, last);
            move_to(&mut self.scroll, to);
        }
        self.select_diff_row(r, ctx);
        true
    }

    /// Go to the next comment or finding (`by` 1), or the one before (-1),
    /// from the cursor, else from the top of the view; past the last, the
    /// first. Whether there was one.
    pub fn jump_comment(&mut self, by: i64, ctx: &Ctx) -> bool {
        let n = self.comment_order.len() as i64;
        if n == 0 {
            return false;
        }
        let cursor = self.cursor.as_ref().map(|c| c.row);
        let here = cursor.and_then(|r| self.comment_at(r)).and_then(|c| {
            let order = &self.comment_order;
            order.iter().position(|&(x, _)| x == c)
        });
        let order = &self.comment_order;
        let top = self.diff_window(ctx.page.max(1)).1.start;
        let to = match (here, cursor) {
            (Some(at), _) => at as i64 + by,
            // Those after a row aren't the ones it is on.
            (None, Some(row)) if by > 0 => order.partition_point(|&(_, r)| r <= row) as i64,
            (None, Some(row)) => order.partition_point(|&(_, r)| r < row) as i64 - 1,
            (None, None) if by > 0 => order.partition_point(|&(_, r)| r < top) as i64,
            (None, None) => order.partition_point(|&(_, r)| r < top) as i64 - 1,
        };
        let c = order[to.rem_euclid(n) as usize].0;
        self.show_comment(c, ctx)
    }

    /// Where comment `c` is among those of its kind, findings or the
    /// user's own, in the diff's order: 1-based, and how many there are.
    fn comment_place(&self, c: usize) -> (usize, usize) {
        let finding = |c: usize| {
            (self.review.comments.get(c))
                .and_then(|c| c.flag.as_ref())
                .is_some_and(|flag| !flag.yours)
        };
        let kind = finding(c);
        let like = self.comment_order.iter().map(|&(x, _)| x);
        let like: Vec<usize> = like.filter(|&x| finding(x) == kind).collect();
        let at = like.iter().position(|&x| x == c).map_or(0, |at| at + 1);
        (at, like.len())
    }

    /// Whether the card of comment `c`, a finding, goes on under its last
    /// row: with the user's comment on it, or one being written.
    fn card_goes_on(&self, c: usize) -> bool {
        let finding = self.review.comments.get(c).is_some_and(|c| !is_note(c));
        let writing = self
            .editing
            .as_ref()
            .is_some_and(|e| e.at == Some(c) && e.note_on.is_some());
        finding && (self.has_note(c) || writing)
    }

    /// What `]` and `[` go through: findings (or suggestions) when all
    /// are, else comments.
    fn comment_noun(&self) -> &'static str {
        let of = (self.review.comments.iter()).find_map(|c| c.flag.as_ref().map(|f| f.of));
        match self.review.own() == 0 {
            true => review_items::words(of.unwrap_or_default()).item,
            false => "comment",
        }
    }

    /// Whether there is a comment or finding to go to, and what they are.
    pub fn comments_to_visit(&self) -> Option<&'static str> {
        (!self.comment_order.is_empty()).then(|| self.comment_noun())
    }

    /// Where the comments are on the diff's scrollbar, as `search_ticks`
    /// has it, the one the cursor is on marked.
    fn comment_ticks(&self) -> Vec<(f64, bool)> {
        let total = self.rows.len().max(1) as f64;
        let here = (self.cursor.as_ref()).and_then(|cursor| self.comment_at(cursor.row));
        (self.comment_order.iter())
            .map(|&(c, r)| (r as f64 / total, here == Some(c)))
            .collect()
    }

    /// Scroll the diff to show the editor's caret, and the line under it
    /// if it fits.
    fn follow_editor(&mut self, ctx: &Ctx) {
        let Some(editing) = &self.editing else {
            return;
        };
        let i = caret_row(&self.editor_wrap, editing.field.line().caret);
        let page = ctx.page;
        let hint = self
            .rows
            .iter()
            .position(|r| matches!(r, DiffRow::EditorHint { .. }));
        let caret = self
            .rows
            .iter()
            .position(|r| matches!(*r, DiffRow::Editor { i: j, .. } if j == i));
        if let Some(hint) = hint {
            self.follow(hint, page);
        }
        if let Some(caret) = caret {
            self.follow(caret, page);
        }
    }

    /// A key while a comment is being written, which takes them all: Enter
    /// (or Ctrl+Enter) keeps it, and so does Esc (a draft is never thrown
    /// away by it: the Discard button does that), Shift+Enter breaks the
    /// line; Ctrl+Shift+C copies the review still.
    fn editor_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Outcome> {
        let others = mods.alt || mods.logo;
        let plain = !(mods.ctrl || mods.shift || others);
        let only_ctrl = mods.ctrl && !(mods.shift || others);
        let only_shift = mods.shift && !(mods.ctrl || others);
        if copy_key(mods, sym) {
            return self.copy(ctx);
        }
        let enter = matches!(sym, Keysym::Return | Keysym::KP_Enter);
        if enter && (plain || only_ctrl) {
            self.submit_comment(ctx);
            return None;
        }
        if sym == Keysym::Escape && plain {
            // Esc with no reason keeps the finding, as the editor says:
            // nothing to warn about.
            if self.reason_missing() {
                self.editing = None;
                self.build_rows(ctx.sections, self.diff_anchor());
            } else {
                self.save_comment(ctx);
            }
            return None;
        }
        let field = &mut self.editing.as_mut()?.field;
        let edit = if enter && only_shift {
            field.newline()
        } else {
            field.key(mods, sym, typed)
        };
        match edit {
            Edit::Changed => {
                self.build_rows(ctx.sections, self.diff_anchor());
                self.follow_editor(ctx);
            }
            Edit::Moved => self.follow_editor(ctx),
            Edit::Ignored => {}
        }
        None
    }

    /// Do `button` if it is one of the diff's comment buttons: write a
    /// comment on the cursor's lines, keep or drop the one being written,
    /// or delete it. Whether it was.
    pub fn comment_button(&mut self, button: ReposButton, ctx: &Ctx) -> bool {
        use ReposButton as B;
        match button {
            B::Comment => {
                self.signals.focus = true;
                self.comment_here(ctx);
            }
            B::CommentFile => {
                self.signals.focus = true;
                if self.editing.is_some() {
                    self.save_comment(ctx);
                }
                self.comment_file(ctx);
            }
            B::SaveComment => self.submit_comment(ctx),
            B::DiscardComment => {
                self.editing = None;
                self.build_rows(ctx.sections, self.diff_anchor());
            }
            B::DeleteComment => {
                if let Some(editing) = &mut self.editing {
                    editing.field = TextField::multiline("");
                    self.save_comment(ctx);
                } else if self.delete_note() {
                    self.signals.error = Some(None);
                }
            }
            B::EditFix => {
                self.edit_fix();
            }
            _ => return false,
        }
        true
    }

    /// A press on detail row `i`, if it is a row of the comment being
    /// written: its caret goes to the char pressed (`pointer_char`), the
    /// selection reaching there when `extend`; a second press in a row
    /// selects the word there, a third all of it. Whether it was on one.
    pub fn press_editor(&mut self, i: usize, extend: bool, count: u32, ctx: &Ctx) -> bool {
        self.rewrap(ctx);
        if self.editor_wrap_at(i, ctx).is_none() {
            return false;
        }
        self.signals.focus = true;
        let at = (self.pointer_char)
            .filter(|&(j, _)| j == i)
            .and_then(|(_, col)| self.editor_byte(i, col, ctx));
        if let (Some(at), Some(editing)) = (at, self.editing.as_mut()) {
            editing.field.click(at, extend, count);
        }
        true
    }

    /// A press on the comment being written dragged to char `col` of
    /// detail row `i`, one of its rows: the selection reaches there.
    pub fn drag_editor(&mut self, i: usize, col: usize, ctx: &Ctx) {
        self.rewrap(ctx);
        if let (Some(at), Some(editing)) = (self.editor_byte(i, col, ctx), self.editing.as_mut()) {
            editing.field.drag_to(at);
        }
    }

    /// A press on the comment being written held past the diff's rows,
    /// above them (`by` negative) or below: the selection reaches char
    /// `col` of the comment's `by`th row out of view that way (as far as
    /// there are), the diff scrolled to show it; with all of them in view
    /// that way, its first or last.
    pub fn drag_editor_past(&mut self, by: i64, col: usize, ctx: &Ctx) {
        self.rewrap(ctx);
        if self.editing.is_none() {
            return;
        }
        let rows = (self.rows.iter().enumerate()).filter_map(|(r, row)| match row {
            DiffRow::Editor { i: w, .. } => Some((r, *w)),
            _ => None,
        });
        let rows: Vec<(usize, usize)> = rows.collect();
        // The rows wholly in view: not one scrolled partly past the top or
        // the bottom, nor under the pinned file header.
        let page = ctx.page.max(1);
        let (sticky, shown, part) = self.diff_shown(page);
        let top = shown.start + usize::from(part > 0.0);
        let bottom = shown.start + page - usize::from(sticky.is_some());
        let n = (by.unsigned_abs() as usize).max(1);
        let to = match by < 0 {
            true => {
                let above = rows.iter().filter(|&&(r, _)| r < top).count();
                rows.get(above.saturating_sub(n)).filter(|_| above > 0)
            }
            false => {
                let below = rows.iter().position(|&(r, _)| r >= bottom);
                below.and_then(|k| rows.get((k + n - 1).min(rows.len() - 1)))
            }
        };
        let edge = match by < 0 {
            true => rows.first(),
            false => rows.last(),
        };
        let Some(&(r, w)) = to.or(edge) else {
            return;
        };
        self.follow(r, ctx.page);
        let at = self.wrap_byte(w, col);
        if let (Some(at), Some(editing)) = (at, self.editing.as_mut()) {
            editing.field.drag_to(at);
        }
    }

    /// The row of the comment being written that detail row `i` shows, by
    /// index into `editor_wrap`.
    fn editor_wrap_at(&self, i: usize, ctx: &Ctx) -> Option<usize> {
        self.editing.as_ref()?;
        match self.rows.get(self.detail_row_at(i, ctx)?)? {
            DiffRow::Editor { i: w, .. } => Some(*w),
            _ => None,
        }
    }

    /// The byte of the comment being written that char `col` of detail row
    /// `i`, one of its rows, starts at: past its end, the end. A row
    /// wrapped onto the next ends before its last char, since its end is
    /// the next row's start, where the caret shows.
    fn editor_byte(&self, i: usize, col: usize, ctx: &Ctx) -> Option<usize> {
        self.wrap_byte(self.editor_wrap_at(i, ctx)?, col)
    }

    /// The byte of the comment being written that char `col` of its row
    /// `w` (by index into `editor_wrap`) starts at: see `editor_byte`.
    fn wrap_byte(&self, w: usize, col: usize) -> Option<usize> {
        let line = self.editing.as_ref()?.field.line();
        let range = self.editor_wrap.get(w).cloned().unwrap_or(0..0);
        let row = &line.text[range.clone()];
        let mut at = (row.char_indices().nth(col)).map_or(range.end, |(b, _)| range.start + b);
        if (self.editor_wrap.get(w + 1)).is_some_and(|next| next.start == range.end) {
            let last = row.char_indices().last().map_or(0, |(b, _)| b);
            at = at.min(range.start + last);
        }
        Some(at)
    }

    /// Whether a press on the diff, held, drags the cursor's range on:
    /// the cursor is on a row and no comment is being written.
    pub fn drags_rows(&self, ctx: &Ctx) -> bool {
        ctx.keys == Keys::Diff && self.cursor.is_some() && self.editing.is_none()
    }

    /// The press held past the diff's rows: the cursor goes `by` rows on
    /// (up negative), its range kept, the diff scrolled along.
    pub fn drag_past_rows(&mut self, by: i64, ctx: &Ctx) {
        self.rewrap(ctx);
        if self.drags_rows(ctx) {
            self.move_cursor(by, true, ctx);
        }
    }

    /// Shift+click on row `i` of the detail pane: the cursor goes there,
    /// its range reaching back to where it was.
    pub fn extend_detail_row(&mut self, i: usize, ctx: &Ctx) {
        self.rewrap(ctx);
        if self.editing.is_some() {
            return;
        }
        let Some(r) = self.detail_row_at(i, ctx) else {
            return;
        };
        let from = self.cursor.as_ref().map_or(r, |c| c.from);
        self.signals.focus = true;
        self.set_cursor(r, Some(from));
    }

    /// The review as Markdown for an agent, to copy.
    pub fn copy(&mut self, ctx: &Ctx) -> Option<Outcome> {
        match self.copy_review(ctx) {
            Ok(outcome) => {
                self.signals.error = Some(None);
                Some(outcome)
            }
            Err(why) => {
                self.signals.error = Some(Some(why.into()));
                None
            }
        }
    }

    /// The review as Markdown for an agent, to copy, else why not.
    pub fn copy_review(&self, ctx: &Ctx) -> Result<Outcome, &'static str> {
        self.review_markdown(ctx, false)
    }

    /// The review as Markdown for a feature's Build agent, each comment with
    /// its id, else why not.
    pub fn send_review(&self, ctx: &Ctx) -> Result<Outcome, &'static str> {
        self.review_markdown(ctx, true)
    }

    fn review_markdown(&self, ctx: &Ctx, numbered: bool) -> Result<Outcome, &'static str> {
        if self.review.own() == 0 {
            return Err(match self.plain {
                true => "No review comments: press c on a line",
                false => NO_REVIEW,
            });
        }
        let order: Vec<PathBuf> = ctx.sections.iter().map(|i| i.root.to_path_buf()).collect();
        // Each repository with the branch and commit the review is of.
        let label = |root: &Path| {
            let label = label_of(root, ctx);
            let section = ctx.sections.iter().find(|s| s.root == root);
            let oid = section.and_then(|s| s.head).map(|o| &o[..o.len().min(7)]);
            match (section.and_then(|s| s.branch), oid) {
                (Some(branch), Some(oid)) => format!("{label} · {branch} at {oid}"),
                (Some(branch), None) => format!("{label} · {branch}"),
                (None, Some(oid)) => format!("{label} · detached at {oid}"),
                (None, None) => label,
            }
        };
        let comments = &self.review.comments;
        let outdated = |c: &Comment| {
            let at = comments.iter().position(|x| std::ptr::eq(x, c));
            at.is_some_and(|at| self.outdated.contains(&at))
        };
        let text = match numbered {
            true => review::numbered_markdown(&self.review, &order, label, outdated),
            false => review::markdown(&self.review, &order, label, outdated),
        };
        Ok(Outcome::Copy {
            text,
            count: self.review.own(),
        })
    }

    /// File `f` of the diff of repository `k` of `diff_roots`, with its
    /// root.
    fn diff_file(&self, k: usize, f: usize) -> Option<(&Path, &FileDiff)> {
        let root = self.roots.get(k)?;
        let file = self.diffs.get(root)?.1.get(f)?;
        Some((root, file))
    }

    /// The root and path of the file diff row `r` is, or is in.
    fn diff_path(&self, r: usize) -> Option<(PathBuf, String)> {
        let (k, f) = self.rows.get(r)?.file()?;
        let (root, file) = self.diff_file(k, f)?;
        Some((root.to_path_buf(), file.path.clone()))
    }

    /// Where the diff is scrolled to, by who it is.
    fn diff_anchor(&self) -> Option<Anchor> {
        let first = (self.scroll.first()).min(self.rows.len().checked_sub(1)?);
        // A page has no header: from its file's first row.
        if self.plain {
            let head = self.rows.iter().position(|r| r.file().is_some())?;
            let (root, path) = self.diff_path(head)?;
            return Some(Anchor {
                root,
                path: Some(path),
                offset: first.saturating_sub(head),
            });
        }
        let head = (0..=first).rev().find(|&r| self.rows[r].is_header())?;
        let (root, path) = match self.rows[head] {
            DiffRow::Repo(k) => (self.roots.get(k)?.clone(), None),
            row => {
                let (root, path) = self.diff_path(head).filter(|_| row.file().is_some())?;
                (root, Some(path))
            }
        };
        Some(Anchor {
            root,
            path,
            offset: first - head,
        })
    }

    /// The row of `root`'s header, or of its file `path`'s.
    pub(crate) fn header_row(&self, root: &Path, path: Option<&str>) -> Option<usize> {
        // A page has no headers: its file's first row stands for them.
        if self.plain {
            return self.rows.iter().position(|row| {
                (row.file())
                    .and_then(|(k, f)| self.diff_file(k, f))
                    .is_some_and(|(r, file)| r == root && path.is_none_or(|p| file.path == p))
            });
        }
        self.rows.iter().position(|&row| match (row, path) {
            (DiffRow::Repo(k), None) => self.roots.get(k).is_some_and(|r| r == root),
            (DiffRow::File(k, f), Some(path)) => self
                .diff_file(k, f)
                .is_some_and(|(r, file)| r == root && file.path == path),
            _ => false,
        })
    }

    /// Scroll the diff back to `anchor`, no further than its file's rows;
    /// to its repository's header when the file is gone. Standing at its
    /// row already, it keeps the part of it scrolled past and goes on
    /// moving (the rows read again while it glides, say).
    fn scroll_to(&mut self, anchor: &Anchor) {
        let (head, offset) = match self.header_row(&anchor.root, anchor.path.as_deref()) {
            Some(head) => (head, anchor.offset),
            None => match self.header_row(&anchor.root, None) {
                Some(head) => (head, 0),
                None => return,
            },
        };
        let end = self.rows[head + 1..]
            .iter()
            .position(|r| r.is_header() || *r == DiffRow::Gap)
            .map_or(self.rows.len(), |n| head + 1 + n);
        self.scroll.jump_row((head + offset).min(end - 1));
    }

    /// Scroll the diff to `place`.
    pub fn show(&mut self, place: Place) {
        if let Some(head) = self.head_of(place) {
            self.scroll.jump(head as f64);
        }
    }

    /// Go to `place` as `show` does, the cursor on its header: what the next
    /// key acts on is what shows.
    pub fn show_with_cursor(&mut self, place: Place) {
        if let Some(head) = self.head_of(place) {
            self.scroll.jump(head as f64);
            self.set_cursor(head, None);
        }
    }

    /// The row `place`'s header is on.
    fn head_of(&self, place: Place) -> Option<usize> {
        match place {
            Place::Section(root) => self.header_row(root, None),
            Place::File(root, path) => self
                .header_row(root, Some(path))
                .or_else(|| self.header_row(root, None)),
            // Its first file.
            Place::Dir(root, dir) => self
                .rows
                .iter()
                .position(|&row| match row {
                    DiffRow::File(k, f) => self
                        .diff_file(k, f)
                        .is_some_and(|(r, file)| r == root && in_dir(&file.path, dir)),
                    _ => false,
                })
                .or_else(|| self.header_row(root, None)),
        }
    }

    /// The furthest row the diff can be scrolled to on a page of `page`:
    /// its last row shows at the bottom, or as many rows further as there
    /// is slack.
    fn diff_last(&self, page: usize) -> usize {
        let last = self.diff_end(page) + self.slack;
        last.min(self.rows.len().saturating_sub(1))
    }

    /// The furthest row the diff can be scrolled to without slack.
    fn diff_end(&self, page: usize) -> usize {
        self.rows.len().saturating_sub(page.max(1))
    }

    /// Where the diff stands on a page of `page`, as far as it goes (its
    /// slack too): its first row in view and the part of it scrolled past.
    fn diff_standing(&self, page: usize) -> (usize, f64) {
        let last = self.diff_last(page) as f64;
        let pos = self.scroll.pos().clamp(0.0, last);
        (pos.floor() as usize, pos.fract())
    }

    /// The diff scrolled: the slack it is scrolled back from is gone.
    fn take_slack(&mut self, page: usize) {
        let past = self.scroll.pos() - self.diff_end(page) as f64;
        self.slack = self.slack.min(past.max(0.0).ceil() as usize);
    }

    /// Where in the pane diff row `r` shows on a page of `page`, in rows
    /// down from its top (to the fraction): 0 for the header pinned there.
    fn shown_at(&self, r: usize, page: usize) -> Option<f64> {
        let (sticky, range, part) = self.diff_shown(page);
        if sticky == Some(r) {
            return Some(0.0);
        }
        let at = r as f64 - (range.start as f64 + part);
        (at >= 0.0 && at < page.max(1) as f64).then_some(at)
    }

    /// Scroll the diff to show row `r` `at` rows down from the pane's
    /// top, past the end if need be.
    fn show_at(&mut self, r: usize, at: f64, page: usize) {
        let pos = (r as f64 - at).max(0.0);
        let past = pos - self.diff_end(page) as f64;
        self.slack = past.max(0.0).ceil() as usize;
        self.scroll.jump(pos);
    }

    /// The diff in view on a page of `page`: the header of the file its
    /// first row is in, pinned at the top over that row, and the rows
    /// shown whole under it.
    fn diff_window(&self, page: usize) -> (Option<usize>, Range<usize>) {
        let page = page.max(1);
        let (sticky, rows, _) = self.diff_shown(page);
        let first = rows.start;
        let start = match sticky {
            Some(h) if h < first => first + 1,
            _ => first,
        };
        let end = (first + page).min(self.rows.len());
        (sticky, start..end.max(start))
    }

    /// The diff's rows in view on a page of `page`, from the first, with
    /// one more under them when scrolled part of a row; the header of the
    /// file the first is in, pinned at the top over them (unless the page
    /// is a row); and the part of the first row scrolled past.
    fn diff_shown(&self, page: usize) -> (Option<usize>, Range<usize>, f64) {
        let page = page.max(1);
        let total = self.rows.len();
        let (first, part) = self.diff_standing(page);
        let end = (first + page + usize::from(part > 0.0)).min(total);
        let sticky = self.file_head(first).filter(|_| page > 1);
        (sticky, first..end, part)
    }

    /// Where the header `head` pinned at the top of the diff on a page of
    /// `page` is, logical pixels down from the top of its rows `line_h`
    /// high (as CSS's sticky has it): at the top, pushed up by the next header as it comes, down
    /// with the rows pulled past the start; and whether it is scrolled
    /// past its own place.
    fn diff_pin(&self, head: usize, page: usize, line_h: i32) -> (f64, bool) {
        let file = self.rows[head].file();
        let total = self.rows.len();
        let next = (head + 1..total)
            .find(|&r| self.rows[r].file() != file)
            .unwrap_or(total);
        let (first, part) = self.diff_standing(page);
        let pos = first as f64 + part;
        let over = self.scroll.overscroll();
        let line = line_h as f64;
        let place = (head as f64 - pos) * line + over;
        let pushed = (next as f64 - pos) * line + over - line;
        (place.max(0.0).min(pushed), place < 0.0)
    }

    /// The header row of the file diff row `r` is in.
    fn file_head(&self, r: usize) -> Option<usize> {
        let (k, f) = self.rows.get(r)?.file()?;
        (0..=r).rev().find(|&h| self.rows[h] == DiffRow::File(k, f))
    }

    /// The header of the file the diff's keys act on: the cursor's, else
    /// the one at its top, else the first in view.
    fn current_file(&self, page: usize) -> Option<usize> {
        if self.plain {
            return self.rows.iter().position(|r| r.file().is_some());
        }
        if let Some(head) = self.cursor.as_ref().and_then(|c| self.file_head(c.row)) {
            return Some(head);
        }
        let (_, range) = self.diff_window(page);
        let first = range.start;
        self.file_head(first).or_else(|| {
            range
                .clone()
                .find(|&r| matches!(self.rows[r], DiffRow::File(..)))
        })
    }

    /// Diff row `r` as the detail pane shows it; a file's header `pinned`
    /// at the top says its repository too.
    fn diff_row_view(&self, r: usize, pinned: bool, ctx: &Ctx) -> DetailRow {
        let blank = || DetailRow::Text(detail_line("", DiffTone::Plain));
        let item = |k: usize| {
            ctx.sections
                .get(k)
                .filter(|i| self.roots.get(k).is_some_and(|r| r == i.root))
        };
        let row = self.rows[r];
        let file = row
            .file()
            .and_then(|(k, f)| Some((item(k)?, self.diff_file(k, f)?.1)));
        match (row, file) {
            (DiffRow::Repo(k), _) => {
                let Some(item) = item(k) else {
                    return blank();
                };
                let files = self.diffs.get(item.root).map(|(_, f)| f.as_slice());
                let files = files.unwrap_or_default();
                DetailRow::Repo(DiffRepo {
                    label: item.label.to_owned(),
                    branch: item.branch.map(str::to_owned),
                    files: item.changes.len(),
                    added: files.iter().map(|f| f.added).sum(),
                    removed: files.iter().map(|f| f.removed).sum(),
                })
            }
            (DiffRow::Reading(_), _) => DetailRow::Note("Reading…".into()),
            (DiffRow::File(..), Some((item, file))) => {
                let check = ctx.check.map(|check| check(item.root, &file.path));
                let note = ctx.note.and_then(|note| note(item.root, &file.path));
                let selected = ctx.selected == Some((item.root, file.path.as_str()));
                let key = (item.root.to_path_buf(), file.path.clone());
                DetailRow::File(DiffFile {
                    repo: pinned.then(|| item.label.to_owned()),
                    path: search::shown_path(file),
                    code: file.code,
                    check,
                    folded: self.folded.contains(&key),
                    added: file.added,
                    removed: file.removed,
                    selected,
                    note,
                })
            }
            (DiffRow::Hunk(_, _, h), Some((_, file))) => {
                let hunk = &file.hunks[h];
                DetailRow::Hunk {
                    context: hunk.context.clone(),
                    number: hunk.lines.first().map_or(0, |l| l.number),
                }
            }
            (DiffRow::Line(_, _, h, l), Some((_, file))) => {
                let line = &file.hunks[h].lines[l];
                DetailRow::Line(match self.part_of(r) {
                    Some(part) => clip_line(line, &part, true),
                    None => line.clone(),
                })
            }
            (DiffRow::Wrap(_, _, h, l, _), Some((_, file))) => match self.part_of(r) {
                Some(part) => DetailRow::Line(clip_line(&file.hunks[h].lines[l], &part, false)),
                None => blank(),
            },
            (DiffRow::Note(_, _, n), Some((_, file))) => DetailRow::Note(file.notes[n].clone()),
            (DiffRow::More(..), Some((_, file))) => {
                let total: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
                let more = total.saturating_sub(FILE_LINES);
                DetailRow::Note(format!(
                    "… {more} more lines · Enter or a click shows them all"
                ))
            }
            (DiffRow::CommentHead { c, .. }, _) => {
                let Some(comment) = self.review.comments.get(c) else {
                    return blank();
                };
                let within = note_tone(comment);
                let (at, of) = self.comment_place(c);
                let reviewer = (comment.flag.as_ref()).filter(|flag| !flag.yours);
                let noun = match comment.flag.as_ref().map(|flag| (flag.of, flag.yours)) {
                    Some((ItemOf::Suggestion, true)) => "your suggestion",
                    Some((of, false)) => review_items::words(of).item,
                    _ => "comment",
                };
                let who = match comment.flag.as_ref() {
                    Some(flag) if !flag.yours => review_items::words(flag.of).by,
                    Some(flag) if !flag.note => review_items::words(flag.of).own_head,
                    // As the item's card on the page calls it.
                    Some(flag) if flag.question => review_items::words(flag.of).answer,
                    _ => "Your comment",
                };
                DetailRow::CommentHead {
                    who,
                    finding: reviewer.map(|flag| FindingHead {
                        severity: flag.severity.label().into(),
                        state: finding_state_word(flag).map(Into::into),
                        tone: finding_tone(flag),
                    }),
                    // Part of its finding: it has no place of its own.
                    // One that went to Build says so, by the id the agent has.
                    place: match (within, comment.flag.as_ref().filter(|flag| flag.sent)) {
                        (Some(_), _) => String::new(),
                        (None, Some(flag)) => format!(
                            "{} \u{b7} {} \u{b7} {noun} {at} of {of}",
                            flag.id,
                            finding_state_word(flag).unwrap_or_default()
                        ),
                        (None, None) => format!("{noun} {at} of {of}"),
                    },
                    within,
                }
            }
            (DiffRow::Comment { c, i, .. }, _) => {
                let (Some(comment), Some(wrap)) =
                    (self.review.comments.get(c), self.comment_wraps.get(c))
                else {
                    return blank();
                };
                let range = wrap.get(i).cloned().unwrap_or(0..0);
                DetailRow::Comment {
                    text: comment.text[range].to_owned(),
                    // Its heading rounds the card's top.
                    first: false,
                    // The finding's card goes on round the comment on it.
                    last: i + 1 >= wrap.len() && !self.card_goes_on(c),
                    tone: (comment.flag.as_ref())
                        .filter(|flag| !flag.yours)
                        .map(finding_tone),
                    within: note_tone(comment),
                }
            }
            (DiffRow::Editor { i, .. }, _) => {
                let Some(editing) = &self.editing else {
                    return blank();
                };
                let line = editing.field.line();
                let range = self.editor_wrap.get(i).cloned().unwrap_or(0..0);
                let sel = line.selection();
                let clip = |b: usize| b.clamp(range.start, range.end) - range.start;
                let on = caret_row(&self.editor_wrap, line.caret) == i;
                DetailRow::Editor {
                    text: line.text[range.clone()].to_owned(),
                    caret: on.then(|| clip(line.caret)),
                    selection: clip(sel.start)..clip(sel.end),
                    first: i == 0,
                    last: i + 1 >= self.editor_wrap.len(),
                    within: (editing.note_on.as_ref())
                        .and(editing.at)
                        .and_then(|c| self.review.comments.get(c)?.flag.as_ref())
                        .map(finding_tone),
                }
            }
            (DiffRow::EditorHint { .. }, _) => DetailRow::Note(
                match self.editing.as_ref().is_some_and(|e| e.dismiss.is_some()) {
                    true if self.editing_of() == ItemOf::Suggestion => {
                        "Why reject it? Enter rejects it · Esc with no reason keeps it"
                    }
                    true => "Why dismiss it? Enter dismisses it · Esc with no reason keeps it",
                    false => "Shift+Enter breaks the line · Enter or Esc keeps it",
                }
                .into(),
            ),
            (DiffRow::General, _) => DetailRow::Hunk {
                context: "General comments".into(),
                number: 0,
            },
            (DiffRow::Outdated(None), _) => DetailRow::Hunk {
                context: match (self.outdated.is_empty(), self.plain) {
                    (true, _) => "Findings not on a line of the diff",
                    (false, true) => "Comments on lines no longer in the file",
                    (false, false) => "Comments on lines no longer in the diff",
                }
                .into(),
                number: 0,
            },
            (DiffRow::Outdated(Some(c)), _) => {
                let Some(comment) = self.review.comments.get(c) else {
                    return blank();
                };
                let repo = label_of(&comment.root, ctx);
                DetailRow::Note(format!("{repo} › {}", review::place(comment)))
            }
            _ => blank(),
        }
    }

    /// How diff row `r` is marked: the cursor on it or in its range, the
    /// range's first or last row (of those lit, not a header), a comment
    /// on it.
    fn row_mark(&self, r: usize) -> RowMark {
        let noted = match self.rows.get(r) {
            Some(&(DiffRow::Line(k, f, h, l) | DiffRow::Wrap(k, f, h, l, _))) => {
                self.noted.contains(&(k, f, h, l))
            }
            _ => false,
        };
        let Some(c) = &self.cursor else {
            return RowMark {
                noted,
                ..RowMark::default()
            };
        };
        // Down to the last row of the last line, on as many as it takes.
        let end = c.row.max(c.from);
        let range = c.row.min(c.from)..=end + self.part_count(end) - 1;
        let lit = |r: &usize| {
            !matches!(
                self.rows.get(*r),
                Some(DiffRow::Repo(_) | DiffRow::File(..)) | None
            )
        };
        RowMark {
            selected: range.contains(&r),
            top: range.clone().find(lit) == Some(r),
            bottom: range.rev().find(lit) == Some(r),
            noted,
        }
    }

    /// The buttons on the diff rows in view, `shown` (by index among
    /// them): writing a comment on the cursor's lines, at the cursor; the
    /// comment being written kept, dropped or deleted, under it.
    fn row_buttons(&self, shown: &[usize], read_only: bool) -> Vec<(usize, Hint)> {
        use ReposButton as B;
        let at = |r: usize| shown.iter().position(|&x| x == r);
        let hint = |button: B| Hint {
            button,
            label: button.label().into(),
            key: key_name(button).into(),
            disabled: None,
        };
        // Copying is all a read-only diff offers, on the cursor's line.
        if read_only {
            let Some(cursor) = self
                .cursor
                .as_ref()
                .filter(|c| self.line_at(c.row).is_some())
            else {
                return Vec::new();
            };
            let last = cursor.row + self.part_count(cursor.row) - 1;
            return (at(last).or_else(|| at(cursor.row)).into_iter())
                .map(|i| (i, hint(B::CopyLines)))
                .collect();
        }
        if let Some(editing) = &self.editing {
            let Some(i) = self
                .rows
                .iter()
                .position(|r| matches!(r, DiffRow::EditorHint { .. }))
                .and_then(at)
            else {
                return Vec::new();
            };
            if editing.dismiss.is_some() {
                let dismiss = Hint {
                    key: "Enter".into(),
                    label: review_items::words(self.editing_of()).refuse_verb.into(),
                    ..hint(B::DismissFinding)
                };
                return vec![(i, dismiss), (i, hint(B::DiscardComment))];
            }
            let mut buttons = vec![(i, hint(B::SaveComment)), (i, hint(B::DiscardComment))];
            // A first comment on a finding has nothing to delete.
            let on = editing.at.and_then(|c| self.review.comments.get(c));
            let new_note = editing.note_on.is_some() && !on.is_some_and(is_note);
            if editing.at.is_some() && !new_note {
                buttons.push((i, hint(B::DeleteComment)));
            }
            return buttons;
        }
        // Every hunk's header in view puts the hunk back: no key does
        // that, R putting back the change at the caret.
        let hunk = Hint {
            key: String::new(),
            ..hint(B::Restore)
        };
        let mut buttons: Vec<(usize, Hint)> = shown
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(self.rows.get(**r), Some(DiffRow::Hunk(..))))
            .map(|(i, _)| (i, hunk.clone()))
            .collect();
        let Some(cursor) = &self.cursor else {
            return buttons;
        };
        // On the user's comment on a finding, what can be done to it.
        if let Some((c, flag)) = self.note_here() {
            let first = self.find_row(&RowKey::Comment(c)).and_then(at);
            if let (Some(i), None) = (first, self.note_refused(flag)) {
                let edit = Hint {
                    label: "Edit".into(),
                    ..hint(B::Comment)
                };
                let delete = Hint {
                    key: "Del".into(),
                    ..hint(B::DeleteComment)
                };
                buttons.push((i, edit));
                buttons.push((i, delete));
            }
            return buttons;
        }
        // On a finding, what can be decided of it, on its first row.
        if let Some((c, flag)) = self.finding_here() {
            let first = self.find_row(&RowKey::Comment(c)).and_then(at);
            let keyed = |button: B, key: &str| Hint {
                key: key.into(),
                label: decide_label(flag.of, flag.pr, button, flag.state),
                ..hint(button)
            };
            let words = review_items::words_in(flag.of, flag.pr);
            let choices = match flag.state {
                // Frozen, it is only read: nothing on it would be taken.
                _ if flag.frozen.is_some() => Vec::new(),
                FindingState::Fix if flag.yours => vec![
                    Hint {
                        label: words.edit_own.into(),
                        ..keyed(B::Comment, "c")
                    },
                    Hint {
                        label: "Delete".into(),
                        ..keyed(B::DismissFinding, "Del")
                    },
                ],
                _ if flag.yours => Vec::new(),
                // A question is accepted by answering it.
                FindingState::Open if flag.unanswered => vec![keyed(B::DismissFinding, "Del")],
                FindingState::Open => {
                    vec![keyed(B::FixFinding, "f"), keyed(B::DismissFinding, "Del")]
                }
                FindingState::Fix => {
                    vec![
                        keyed(B::ReopenFinding, "f"),
                        keyed(B::DismissFinding, "Del"),
                    ]
                }
                FindingState::Dismissed => {
                    vec![keyed(B::FixFinding, "f"), keyed(B::ReopenFinding, "Del")]
                }
                FindingState::Sent | FindingState::Fixed | FindingState::Posted => Vec::new(),
            };
            // What the user can say of a reviewer's finding.
            let say = (!flag.yours && self.note_refused(flag).is_none()).then(|| {
                let comment = (!self.has_note(c)).then(|| Hint {
                    label: match flag.unanswered {
                        true => "Answer".into(),
                        false => B::Comment.label().into(),
                    },
                    ..keyed(B::Comment, "c")
                });
                // A dismissed finding's fix waits for it to be reopened.
                let fix = review_items::editable(flag.of, &flag.id, flag.state)
                    .is_ok()
                    .then(|| Hint {
                        label: words.edit.into(),
                        ..keyed(B::EditFix, "Shift+E")
                    });
                comment.into_iter().chain(fix)
            });
            if let Some(i) = first {
                buttons.extend(choices.into_iter().map(|h| (i, h)));
                buttons.extend(say.into_iter().flatten().map(|h| (i, h)));
            }
            return buttons;
        }
        let lines = self.cursor_lines().is_some_and(|(_, _, l)| !l.is_empty());
        // Under the line's last row: it covers text less there, and rows
        // of a line on many are what the chips would hide.
        let last = cursor.row + self.part_count(cursor.row) - 1;
        if let Some(i) = at(last).or_else(|| at(cursor.row))
            && lines
            && self.comment_at(cursor.row).is_none()
        {
            if self.flags_only.is_none() {
                buttons.push((i, hint(B::Comment)));
            }
            buttons.push((i, hint(B::CopyLines)));
            if self.changed_row(cursor.row) {
                buttons.push((i, hint(B::Restore)));
            }
        }
        buttons
    }

    /// Whether diff row `r` is an added or removed line.
    fn changed_row(&self, r: usize) -> bool {
        let Some(&DiffRow::Line(k, f, h, l)) = self.rows.get(r) else {
            return false;
        };
        self.diff_file(k, f)
            .is_some_and(|(_, file)| file.hunks[h].lines[l].kind != LineKind::Context)
    }

    /// The title row's button copying the review, while there is one.
    fn title_buttons(&self) -> Vec<Hint> {
        let hint = |button: ReposButton, label: String| Hint {
            button,
            label,
            key: String::new(),
            disabled: None,
        };
        let mut buttons = Vec::new();
        if let Some(noun) = self.comments_to_visit() {
            buttons.push(hint(ReposButton::PrevComment, format!("Previous {noun}")));
            buttons.push(hint(ReposButton::NextComment, format!("Next {noun}")));
        }
        let label = match self.review.own() {
            0 => return buttons,
            1 => "Copy 1 comment".to_owned(),
            n => format!("Copy {n} comments"),
        };
        buttons.push(hint(ReposButton::CopyReview, label));
        // Those on lines gone (after a commit, say) dropped on their own.
        if !self.outdated.is_empty() {
            let label = format!("Drop {} outdated", self.outdated.len());
            buttons.push(hint(ReposButton::DropOutdated, label));
        }
        buttons.push(hint(
            ReposButton::ClearReview,
            ReposButton::ClearReview.label().into(),
        ));
        buttons
    }

    /// What the sheet asking whether to delete the review (only those on
    /// lines gone, when `outdated`) says: its title, and a line per file
    /// with comments.
    pub fn clear_sheet(&self, ctx: &Ctx, outdated: bool) -> (String, Vec<(String, String, Tone)>) {
        let mut files: BTreeMap<(String, &str), usize> = BTreeMap::new();
        let taken = (self.review.comments.iter().enumerate())
            .filter(|(_, c)| c.flag.is_none())
            .filter(|(c, _)| !outdated || self.outdated.contains(c));
        for (_, c) in taken {
            *files
                .entry((label_of(&c.root, ctx), c.path.as_str()))
                .or_default() += 1;
        }
        let lines = files
            .into_iter()
            .map(|((repo, path), n)| (repo, format!("{path}: {}", comments(n)), Tone::Warning))
            .collect();
        let title = match outdated {
            true => format!(
                "Delete the {} on lines no longer in the {}?",
                comments(self.outdated.len()),
                if self.plain { "file" } else { "diff" }
            ),
            false => {
                let n = self.review.own();
                format!("Delete the review's {} for good?", comments(n))
            }
        };
        (title, lines)
    }

    /// How many review comments are on lines no longer in the diff.
    pub fn outdated_count(&self) -> usize {
        self.outdated.len()
    }

    /// Delete the review comments on lines no longer in the diff (and the
    /// edit of one of them).
    pub fn drop_outdated(&mut self, sections: &[Section]) {
        let gone = std::mem::take(&mut self.outdated);
        if self
            .editing
            .as_ref()
            .is_some_and(|e| e.at.is_some_and(|c| gone.contains(&c)))
        {
            self.editing = None;
        }
        let mut c = 0;
        self.review.comments.retain(|_| {
            c += 1;
            !gone.contains(&(c - 1))
        });
        // The comment being edited is one up for each dropped before it.
        if let Some(Editing { at: Some(at), .. }) = &mut self.editing {
            *at -= gone.iter().filter(|&&g| g < *at).count();
        }
        self.review_dirty = true;
        self.cursor = None;
        self.rebuild(sections);
    }

    /// The diff row shown as row `i` of the detail pane.
    pub(crate) fn detail_row_at(&self, i: usize, ctx: &Ctx) -> Option<usize> {
        let (sticky, range, _) = self.diff_shown(ctx.page);
        let r = match sticky {
            Some(head) if i == 0 => head,
            Some(_) => range.start + i - 1,
            None => range.start + i,
        };
        (r < range.end || Some(r) == sticky).then_some(r)
    }

    /// Tell the host the cursor went to the checkout or file diff row
    /// `r` is in, for it to select it too, the diff staying where it is.
    pub(crate) fn select_diff_row(&mut self, r: usize, _ctx: &Ctx) {
        let moved = match self.rows.get(r) {
            Some(DiffRow::Repo(k) | DiffRow::Reading(k)) => match self.roots.get(*k) {
                Some(root) => Moved::Section(root.clone()),
                None => return,
            },
            _ => match self.diff_path(r) {
                Some((root, path)) => Moved::File(root, path),
                None => return,
            },
        };
        self.signals.moved = Some(moved);
    }

    /// Check the file diff row `r` is in, or uncheck it, as the host
    /// does.
    fn toggle_diff_file(&mut self, r: usize, ctx: &Ctx) -> Option<Outcome> {
        ctx.check?;
        let (root, path) = self.diff_path(r)?;
        Some(Outcome::Toggle { root, path })
    }

    /// Fold the file diff row `r` is in, or unfold it, its header staying
    /// where it shows (at the top when out of view): what is under the
    /// pointer then is still that header.
    fn fold_diff_file(&mut self, r: usize, fold: bool, ctx: &Ctx) {
        let Some((root, path)) = self.diff_path(r) else {
            return;
        };
        let page = ctx.page.max(1);
        let at = self
            .file_head(r)
            .and_then(|h| self.shown_at(h, page))
            .unwrap_or(0.0);
        let key = (root.clone(), path.clone());
        if fold {
            self.folded.insert(key);
        } else {
            self.folded.remove(&key);
        }
        self.build_rows(ctx.sections, None);
        if let Some(head) = self.header_row(&root, Some(&path)) {
            self.show_at(head, at, page);
        }
    }

    /// Show the long file whose last row shown is diff row `r` whole;
    /// whether it was that row.
    fn show_whole(&mut self, r: usize, ctx: &Ctx) -> bool {
        if !matches!(self.rows.get(r), Some(DiffRow::More(..))) {
            return false;
        }
        let Some(key) = self.diff_path(r) else {
            return false;
        };
        self.whole.insert(key);
        self.build_rows(ctx.sections, self.diff_anchor());
        true
    }

    /// Scroll the diff to the next file's header (`by` 1) or the one
    /// before (-1), selecting it in the tree.
    fn jump_file(&mut self, by: i64, ctx: &Ctx) {
        let (_, range) = self.diff_window(ctx.page);
        let first = range.start;
        let is_file = |r: &DiffRow| matches!(r, DiffRow::File(..));
        let to = if by > 0 {
            (first + 1..self.rows.len()).find(|&r| is_file(&self.rows[r]))
        } else {
            (0..first).rev().find(|&r| is_file(&self.rows[r]))
        };
        if let Some(to) = to {
            let last = self.diff_last(ctx.page);
            self.scroll.jump(to.min(last) as f64);
            self.set_cursor(to, None);
            self.select_diff_row(to, ctx);
        }
    }

    /// Open the file shown as row `i` of the detail pane in Zed, there
    /// (`open_diff_row`).
    pub fn open_detail_row(&mut self, i: usize, ctx: &Ctx) -> Option<Outcome> {
        let r = self.detail_row_at(i, ctx)?;
        self.open_diff_row(r)
    }

    /// Open the file diff row `r` is in in Zed: at a line's own (at the
    /// caret's char, the cursor on it), at the first of a hunk, at a
    /// comment's last; at none on its header, Zed putting it where it was.
    fn open_diff_row(&self, r: usize) -> Option<Outcome> {
        let r = self.owner(r);
        let row = *self.rows.get(r)?;
        let (k, f) = row.file()?;
        let (root, file) = self.diff_file(k, f)?;
        let at = match row {
            DiffRow::Line(_, _, h, l) => {
                let line = &file.hunks[h].lines[l];
                let col = (line.kind != LineKind::Removed)
                    .then(|| self.caret().filter(|&(row, _)| row == r))
                    .flatten()
                    .map(|(_, col)| col);
                Some(Position {
                    line: line.number,
                    col,
                })
            }
            DiffRow::CommentHead { c, .. } | DiffRow::Comment { c, .. } => {
                let comment = self.review.comments.get(c);
                let line = comment.and_then(|c| c.lines.last()).map(|q| q.number);
                line.map(|line| Position { line, col: None })
            }
            DiffRow::Hunk(_, _, h) => file.hunks[h].lines.first().map(|l| Position {
                line: l.number,
                col: None,
            }),
            _ => None,
        };
        Some(Outcome::OpenInZed {
            root: root.to_path_buf(),
            paths: vec![file.path.clone()],
            at: at.filter(|_| file.code != 'D'),
        })
    }

    /// The row Enter (and `g` Space) opens in Zed: the cursor's, else the
    /// first line in view of the file at the top, else its header.
    fn open_target(&self, page: usize) -> Option<usize> {
        if let Some(cursor) = &self.cursor {
            return Some(cursor.row);
        }
        let head = self.current_file(page)?;
        let (_, range) = self.diff_window(page);
        let file = self.rows[head].file();
        let line = range
            .clone()
            .find(|&r| matches!(self.rows[r], DiffRow::Line(..)) && self.rows[r].file() == file);
        Some(line.unwrap_or(head))
    }

    /// Ctrl+click on row `i` of the detail pane: the cursor there (the
    /// caret on the char pressed), and its code opened in Zed there; a
    /// file's header opens the file. A comment isn't edited.
    pub fn goto_detail_row(&mut self, i: usize, ctx: &Ctx) -> Option<Outcome> {
        self.rewrap(ctx);
        let shown = self.detail_row_at(i, ctx);
        let col = (self.pointer_char)
            .filter(|&(j, _)| j == i)
            .map(|(_, col)| shown.map_or(col, |r| self.line_col(r, col)));
        if self.editing.is_some() {
            let key = shown.and_then(|r| self.row_key(r));
            self.save_comment(ctx);
            let r = key.and_then(|k| self.find_row(&k))?;
            return self.goto_row(r, col, ctx);
        }
        self.goto_row(shown?, col, ctx)
    }

    /// The cursor on diff row `r`, the caret on char `col` of its line
    /// when pressed there; and its code opened in Zed.
    fn goto_row(&mut self, r: usize, col: Option<usize>, ctx: &Ctx) -> Option<Outcome> {
        self.signals.focus = true;
        self.leave_visual();
        self.set_cursor(r, None);
        if let (Some(col), Some(c)) = (col, &mut self.cursor) {
            c.col = col;
        }
        self.select_diff_row(r, ctx);
        self.open_diff_row(r)
    }

    /// A key while the diff has the keyboard; `None` when it isn't one of
    /// its own. The arrows, the pages, Home and End move its cursor (with
    /// Shift reaching its range further), Alt+Up/Down go to the file
    /// before or after, and for the
    /// cursor's file (else the one at the top) Space checks it, Left and
    /// Right fold and unfold it (off a line, where they move the caret),
    /// and Enter opens it in Zed at the cursor's
    /// line and caret (else the first line in view; `open_target`). C comments on the cursor's lines, or
    /// edits the comment it is on; Delete deletes that. `j` and `k` move
    /// the cursor as Down and Up do, and some letters its caret
    /// (`caret_key`).
    fn detail_press(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Option<Outcome>> {
        let page = ctx.page.max(1);
        let plain = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        let alt = mods.alt && !(mods.ctrl || mods.logo || mods.shift);
        let shift = mods.shift && !(mods.ctrl || mods.alt || mods.logo);
        let only_ctrl = mods.ctrl && !(mods.alt || mods.logo || mods.shift);
        let by = match sym {
            // Ctrl+J and Ctrl+K move as Down and Up do, in every list.
            Keysym::j | Keysym::J if only_ctrl => 1,
            Keysym::k | Keysym::K if only_ctrl => -1,
            _ if !(plain || shift) => 0,
            Keysym::Down | Keysym::KP_Down => 1,
            Keysym::Up | Keysym::KP_Up => -1,
            Keysym::j if plain => 1,
            Keysym::k if plain => -1,
            Keysym::Page_Down | Keysym::KP_Page_Down => page as i64,
            Keysym::Page_Up | Keysym::KP_Page_Up => -(page as i64),
            Keysym::Home | Keysym::KP_Home => i64::MIN,
            Keysym::End | Keysym::KP_End => i64::MAX,
            _ => 0,
        };
        if by != 0 {
            let lines = self.visual == Some(vim::Visual::Lines);
            self.move_cursor(by, shift || lines, ctx);
            return Some(None);
        }
        if self.caret_key(mods, sym, typed, ctx) {
            return Some(None);
        }
        match sym {
            Keysym::Down | Keysym::KP_Down if alt => self.jump_file(1, ctx),
            Keysym::Up | Keysym::KP_Up if alt => self.jump_file(-1, ctx),
            Keysym::space if plain => {
                let head = self.current_file(page);
                return Some(head.and_then(|head| self.toggle_diff_file(head, ctx)));
            }
            Keysym::Left | Keysym::KP_Left | Keysym::Right | Keysym::KP_Right if plain => {
                let fold = matches!(sym, Keysym::Left | Keysym::KP_Left);
                // A page has no header to find it again by.
                if !self.plain
                    && let Some(head) = self.current_file(page)
                {
                    self.fold_diff_file(head, fold, ctx);
                }
            }
            Keysym::Return | Keysym::KP_Enter if plain => {
                let r = self.open_target(page)?;
                if self.show_whole(r, ctx) {
                    return Some(None);
                }
                return Some(self.open_diff_row(r));
            }
            Keysym::c if plain && ctx.read_only => return Some(Some(Outcome::Notice(READ_ONLY))),
            Keysym::c if plain => self.comment_here(ctx),
            // Shift+letter comes as the letter's own sym: `typed` has the capital.
            Keysym::c | Keysym::C
                if self.plain && mods.shift && !(mods.ctrl || mods.alt || mods.logo) =>
            {
                if ctx.read_only {
                    return Some(Some(Outcome::Notice(READ_ONLY)));
                }
                self.comment_file(ctx);
            }
            Keysym::Delete | Keysym::KP_Delete if plain => {
                if !self.dismiss_finding(ctx) {
                    self.delete_comment(ctx);
                }
                return Some(None);
            }
            Keysym::f if plain => {
                if !self.fix_finding() {
                    return None;
                }
            }
            Keysym::e | Keysym::E if mods.shift && !(mods.ctrl || mods.alt || mods.logo) => {
                if !self.edit_fix() {
                    return None;
                }
            }
            _ => return None,
        }
        Some(None)
    }

    /// The checkouts whose diff wants reading, with the read of their
    /// changes it is for: those not read since it.
    pub fn diffs_wanted(&self, ctx: &Ctx) -> Vec<(PathBuf, u64)> {
        ctx.sections
            .iter()
            .filter(|i| !i.changes.is_empty())
            .filter(|i| {
                self.diffs
                    .get(i.root)
                    .is_none_or(|(generation, _)| *generation < self.reads)
            })
            .map(|i| (i.root.to_path_buf(), self.reads))
            .collect()
    }

    /// A file of the checkout at `root` was edited from the diff: its diff
    /// wants reading again now, the other checkouts' not, without waiting
    /// for Git status to be read again.
    pub fn reread(&mut self, root: &Path) {
        let read = self.reads;
        self.reads += 1;
        // Those read for the last read of the changes still are up to date
        // (none of them is being read: that would be for a later read).
        for (other, (generation, _)) in &mut self.diffs {
            if other != root && *generation == read {
                *generation = self.reads;
            }
        }
    }

    /// A checkout's diff came back, its files in the order to show them,
    /// read for read `generation` of its changes: kept unless one read
    /// later is, or the checkout is gone or clean; the diff stays on what
    /// it showed.
    pub fn set_diffs(&mut self, root: PathBuf, generation: u64, files: Vec<FileDiff>, ctx: &Ctx) {
        let items = ctx.sections;
        let wanted = items
            .iter()
            .any(|i| i.root == root && !i.changes.is_empty());
        let newer = self.diffs.get(&root).is_some_and(|(g, _)| *g > generation);
        if !wanted || newer {
            return;
        }
        let anchor = self.diff_anchor();
        self.diffs.insert(root.clone(), (generation, files));
        self.cols = ctx.cols;
        self.build_rows(items, anchor);
        self.caret_placed(&root, generation, ctx);
    }

    /// The checkouts' changes were read again: diffs of those gone or
    /// clean are dropped, the others read again (still shown meanwhile),
    /// and the rows worked out again where they stood.
    pub fn refresh(&mut self, ctx: &Ctx) {
        self.cols = ctx.cols;
        let items = ctx.sections;
        let anchor = self.diff_anchor();
        let changed = |root: &Path, path: &str| {
            items
                .iter()
                .find(|i| i.root == root)
                .is_some_and(|i| i.changes.iter().any(|c| c.path == path))
        };
        let dirty = |root: &Path| {
            items
                .iter()
                .any(|i| i.root == root && !i.changes.is_empty())
        };
        self.diffs.retain(|root, _| dirty(root));
        self.folded.retain(|(root, path)| changed(root, path));
        self.whole.retain(|(root, path)| changed(root, path));
        self.forget_caret(|root| !dirty(root));
        self.reads += 1;
        self.build_rows(items, anchor);
    }

    /// Work the rows out again where they stand, as what they show
    /// changed (the files read in another order, say).
    pub fn rebuild(&mut self, sections: &[Section]) {
        self.build_rows(sections, self.diff_anchor());
    }

    /// Put the files of the diff read of the checkout at `root` in the
    /// order `rank` gives them.
    pub fn order_files(&mut self, root: &Path, rank: impl Fn(&str) -> usize) {
        if let Some((_, files)) = self.diffs.get_mut(root) {
            files.sort_by_key(|f| rank(&f.path));
        }
    }

    /// Delete every review comment, and the one being written.
    pub fn clear_review(&mut self, sections: &[Section]) {
        // The findings aren't the user's: they stay.
        self.review.comments.retain(|c| c.flag.is_some());
        self.review_dirty = true;
        self.editing = None;
        self.cursor = None;
        self.rebuild(sections);
    }

    /// Where the cursor went since the host last asked, and whether the
    /// diff took the keyboard: for a host following only those, the rest of
    /// the signals left for `take_signals`.
    pub fn take_followed(&mut self) -> (Option<Moved>, bool) {
        let focus = std::mem::take(&mut self.signals.focus);
        (self.signals.moved.take(), focus)
    }

    /// What the host follows since it last took them.
    pub fn take_signals(&mut self) -> Signals {
        std::mem::take(&mut self.signals)
    }

    /// The diff, scrolled: of a long diff only the page is copied.
    pub fn view(&self, ctx: &Ctx) -> DetailView {
        let page = ctx.page.max(1);
        let (sticky, range, part) = self.diff_shown(page);
        let mut rows: Vec<DetailRow> = sticky
            .iter()
            .map(|&h| self.diff_row_view(h, true, ctx))
            .collect();
        rows.extend(range.clone().map(|r| self.diff_row_view(r, false, ctx)));
        let shown: Vec<usize> = sticky.into_iter().chain(range.clone()).collect();
        let mut marks: Vec<RowMark> = shown.iter().map(|&r| self.row_mark(r)).collect();
        // By the line's chars: what falls on each of its rows.
        let found_on = |r: usize| match self.part_of(r) {
            Some(part) => (self.found_on(self.owner(r)).into_iter())
                .filter_map(|(hit, gone_to)| Some((in_part(&hit, &part)?, gone_to)))
                .collect(),
            None => self.found_on(r),
        };
        let mut found: Vec<_> = shown.iter().map(|&r| found_on(r)).collect();
        let mut row_buttons = self.row_buttons(&shown, ctx.read_only);
        let line_h = crate::chrome::repos_detail_line(ctx.density);
        let pin = |h| self.diff_pin(h, page, line_h);
        let (pin, stuck) = sticky.map_or((0.0, false), pin);
        if self.rows.is_empty() && !ctx.sections.is_empty() {
            rows = vec![DetailRow::Text(detail_line(ctx.empty, DiffTone::Plain))];
            marks.clear();
            found.clear();
            row_buttons.clear();
        }
        let caret = self.caret().and_then(|(r, col)| {
            let row = self.part_row(r, col);
            let i = shown.iter().position(|&s| s == row)?;
            let col = col - self.part_of(row).map_or(0, |part| part.start);
            matches!(rows.get(i)?, DetailRow::Line(_)).then_some((i, col))
        });
        let mut word = caret.and_then(|(i, col)| {
            if let Some(part) = shown.get(i).and_then(|&r| self.part_of(r)) {
                let text = self.line_text(self.owner(shown[i]))?;
                let word = find::word_at(text, part.start + col)?;
                return Some((i, in_part(&word, &part)?));
            }
            match rows.get(i)? {
                DetailRow::Line(line) => Some((i, find::word_at(&line.text, col)?)),
                _ => None,
            }
        });
        // The line numbers, as wide as the widest in view.
        let widest = rows
            .iter()
            .filter_map(|row| match row {
                DetailRow::Line(line) => Some(line.number.max(line.old)),
                _ => None,
            })
            .max();
        let gutter = widest.map_or(0, |n| match self.plain {
            true => self.plain_digits(),
            false => n.to_string().len().max(2),
        });
        // The caret's line too long for the row: shown from a char on that
        // keeps the caret in view, a few chars from the edge.
        // (A page wraps its lines instead.)
        let owners: Vec<usize> = shown.iter().map(|&r| self.owner(r)).collect();
        let mut chosen: Vec<_> = (self.chosen(&owners).into_iter())
            .filter_map(|(i, chars)| match self.part_of(shown[i]) {
                Some(part) => Some((i, in_part(&chars, &part)?)),
                None => Some((i, chars)),
            })
            .collect();
        let mut caret = caret;
        let room = crate::chrome::diff_line_room(ctx.cols, gutter, self.plain);
        let skipped = caret.filter(|_| !self.plain).and_then(|(i, col)| {
            let skip = (col + CARET_ROOM).checked_sub(room).filter(|&n| n > 0)?;
            let DetailRow::Line(line) = &mut rows[i] else {
                return None;
            };
            skip_chars(line, skip);
            let shift = |r: &mut Range<usize>| {
                *r = r.start.saturating_sub(skip)..r.end.saturating_sub(skip);
            };
            found[i].iter_mut().for_each(|(r, _)| shift(r));
            chosen
                .iter_mut()
                .filter(|(j, _)| *j == i)
                .for_each(|(_, r)| shift(r));
            word.iter_mut()
                .filter(|(j, _)| *j == i)
                .for_each(|(_, r)| shift(r));
            caret = Some((i, col - skip));
            Some((i, skip))
        });
        DetailView {
            tab: DetailTab::Diff,
            title: self.title(ctx),
            search: self.search_bar(ctx),
            found,
            chosen,
            caret,
            word,
            gutter,
            plain: self.plain,
            skipped,
            ticks: match self.search.is_some() {
                true => self.search_ticks(),
                false => self.comment_ticks(),
            },
            rows,
            sticky: sticky.is_some(),
            marks,
            title_buttons: self.title_buttons(),
            row_buttons,
            part,
            over: self.scroll.overscroll(),
            pin,
            stuck,
            first: range.start,
            total: self.rows.len(),
            focused: ctx.keys == Keys::Diff,
        }
    }

    /// A key for the diff, in the order they are tried: the search's while
    /// it has the keyboard, Ctrl+Shift+F opening it from anywhere; with
    /// the diff's, the comment being written's, then (held while a cut
    /// waits for the diff to be read again) vim's; `n` and `N` going round
    /// what the search found; with the diff's, `/` opening the search;
    /// `*` and `#` searching the word at the caret; then the diff's own
    /// (`detail_press`). `None` when it isn't one of them: the host's.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Option<Outcome>> {
        let outcome = self.any_key(mods, sym, typed, ctx);
        match outcome {
            Some(Some(o)) if ctx.read_only && o.edits() => Some(Some(Outcome::Notice(READ_ONLY))),
            outcome => outcome,
        }
    }

    /// `key`, read-only or not.
    fn any_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Option<Outcome>> {
        if sym.is_modifier_key() {
            return None;
        }
        self.rewrap(ctx);
        // Any key but Del lets an armed comment be.
        if !matches!(sym, Keysym::Delete | Keysym::KP_Delete) {
            self.delete_armed = None;
        }
        if self.search_has_keys(ctx) {
            return Some(self.search_key(mods, sym, typed, ctx));
        }
        // Ctrl+Shift+F from anywhere; Ctrl+F with the diff's keys.
        let in_diff = ctx.keys == Keys::Diff;
        if mods.ctrl
            && (mods.shift || in_diff)
            && !(mods.alt || mods.logo)
            && matches!(sym, Keysym::f | Keysym::F)
        {
            self.open_search(ctx);
            return Some(None);
        }
        if self.editing.is_some() && in_diff {
            return Some(self.editor_key(mods, sym, typed, ctx));
        }
        if copy_key(mods, sym) {
            return Some(self.copy(ctx));
        }
        if in_diff && self.hold_key((*mods, sym, typed)) {
            return Some(None);
        }
        if in_diff && let Some(outcome) = self.vim_key(mods, sym, typed, ctx) {
            return Some(outcome);
        }
        if self.search_open_key(mods, sym, ctx) {
            return Some(None);
        }
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        if in_diff && plain && typed == Keysym::slash {
            self.open_search(ctx);
            return Some(None);
        }
        // `*` (`#` back): the word at the caret, as a whole word, with
        // either focus but a field's.
        let star = match typed {
            Keysym::asterisk | Keysym::KP_Multiply => Some(false),
            Keysym::numbersign => Some(true),
            _ => None,
        };
        if ctx.keys != Keys::Field
            && plain
            && let Some(back) = star
        {
            let at = self.caret();
            let found = at.is_some_and(|(row, col)| self.star_search(row, col, back, ctx));
            return Some((!found).then_some(Outcome::Notice("No word at the caret")));
        }
        if in_diff {
            return self.detail_press(mods, sym, typed, ctx);
        }
        None
    }

    /// A click somewhere in the host: on the diff's search, rows or
    /// buttons it acts on them (`None` back for none of those); elsewhere
    /// the search gives the keyboard back and the comment being written
    /// is kept.
    pub fn click(&mut self, hit: ReposHit, ctx: &Ctx) -> Option<Option<Outcome>> {
        self.rewrap(ctx);
        // The search's parts act on it; a click elsewhere gives the
        // keyboard back from it.
        if let ReposHit::Search(part) = hit {
            self.search_click(part, ctx);
            return Some(None);
        }
        if let Some(search) = &mut self.search {
            search.focused = false;
        }
        if hit == ReposHit::Button(ReposButton::CopyReview) {
            return Some(self.copy(ctx));
        }
        if hit == ReposHit::Button(ReposButton::CopyLines) {
            return Some(Some(self.copy_here()));
        }
        if hit == ReposHit::Button(ReposButton::ClearReview) {
            return Some(Some(Outcome::Clear));
        }
        if hit == ReposHit::Button(ReposButton::DropOutdated) {
            return Some(Some(Outcome::ClearOutdated));
        }
        // A hunk's header puts the hunk back; the cursor's row, the change
        // there or the lines selected.
        if let ReposHit::DiffRestore(i) = hit {
            if ctx.read_only {
                return Some(Some(Outcome::Notice(READ_ONLY)));
            }
            let r = self.detail_row_at(i, ctx);
            return Some(match r.map(|r| self.rows[r]) {
                Some(DiffRow::Hunk(..)) => r.and_then(|r| self.restore_hunk(r)),
                _ => self.restore_here(),
            });
        }
        // A click elsewhere than on the comment being written saves it;
        // the diff row clicked found again among the rows without it.
        let diff_row = match hit {
            ReposHit::DiffRow(i) | ReposHit::DiffCheck(i) | ReposHit::DiffFold(i) => {
                self.detail_row_at(i, ctx)
            }
            _ => None,
        };
        let on_editor = diff_row.is_some_and(|r| {
            matches!(
                self.rows[r],
                DiffRow::Editor { .. } | DiffRow::EditorHint { .. }
            )
        });
        if on_editor {
            self.signals.focus = true;
            return Some(None);
        }
        // Going to a comment keeps the one being written, as any click off
        // it does.
        if let ReposHit::Button(button @ (ReposButton::NextComment | ReposButton::PrevComment)) =
            hit
        {
            if self.editing.is_some() {
                self.save_comment(ctx);
            }
            self.signals.focus = true;
            self.leave_visual();
            let by = if button == ReposButton::NextComment {
                1
            } else {
                -1
            };
            self.jump_comment(by, ctx);
            return Some(None);
        }
        // A finding's buttons decide the one the cursor is on; the
        // dismissal's reason is kept by its Dismiss.
        if let ReposHit::Button(button @ (ReposButton::FixFinding | ReposButton::ReopenFinding)) =
            hit
        {
            let dismissed =
                self.finding_here().map(|(_, f)| f.state) == Some(FindingState::Dismissed);
            match (button, dismissed) {
                (ReposButton::ReopenFinding, true) => {
                    self.dismiss_finding(ctx);
                }
                _ => {
                    self.fix_finding();
                }
            }
            return Some(None);
        }
        if hit == ReposHit::Button(ReposButton::DismissFinding) {
            self.signals.focus = true;
            match self.editing.as_ref().is_some_and(|e| e.dismiss.is_some()) {
                true => self.submit_comment(ctx),
                false => {
                    self.dismiss_finding(ctx);
                }
            }
            return Some(None);
        }
        // The editor's own buttons act on what it holds.
        if let ReposHit::Button(
            button @ (ReposButton::Comment
            | ReposButton::CommentFile
            | ReposButton::SaveComment
            | ReposButton::DiscardComment
            | ReposButton::DeleteComment
            | ReposButton::EditFix),
        ) = hit
        {
            self.signals.focus = true;
            self.comment_button(button, ctx);
            return Some(None);
        }
        // The char pressed, of the line: its row is gone once rows change.
        let pressed = (self.pointer_char)
            .map(|(j, col)| (j, diff_row.map_or(col, |r| self.line_col(r, col))));
        let key = diff_row.and_then(|r| self.row_key(r));
        if self.editing.is_some() {
            self.save_comment(ctx);
        }
        let diff_row = key.and_then(|k| self.find_row(&k));
        match hit {
            ReposHit::DiffRow(i) => {
                self.signals.focus = true;
                if let Some(r) = diff_row {
                    self.set_cursor(r, None);
                    // On a line, the caret goes to the char pressed.
                    if let (Some((j, col)), Some(c)) = (pressed, &mut self.cursor)
                        && j == i
                    {
                        c.col = col;
                    }
                    self.select_diff_row(r, ctx);
                    // On a comment, it is edited; on a long file's last
                    // row shown, the rest show.
                    if let Some(c) = self.comment_at(r)
                        && (self.review.comments.get(c)).is_some_and(|comment| {
                            match comment.flag.as_ref() {
                                None => true,
                                Some(flag) => flag.note && self.note_refused(flag).is_none(),
                            }
                        })
                    {
                        self.edit_comment(c, ctx);
                    }
                    self.show_whole(r, ctx);
                }
                Some(None)
            }
            ReposHit::DiffCheck(_) => {
                let r = diff_row?;
                self.select_diff_row(r, ctx);
                Some(self.toggle_diff_file(r, ctx))
            }
            ReposHit::DiffFold(_) => {
                let r = diff_row?;
                let (root, path) = self.diff_path(r)?;
                let folded = self.folded.contains(&(root, path));
                self.fold_diff_file(r, !folded, ctx);
                Some(None)
            }
            _ => None,
        }
    }

    /// Scroll by `whole` rows (down positive), as far as there are rows.
    pub fn scroll_by(&mut self, whole: i64, ctx: &Ctx) {
        let last = self.diff_last(ctx.page);
        let first = self.scroll.first().min(last);
        let to = (first as i64).saturating_add(whole).clamp(0, last as i64) as usize;
        self.scroll.jump(to as f64);
        self.take_slack(ctx.page);
    }

    /// The rows as a list scrolled.
    pub fn scroll_bounds(&self, ctx: &Ctx) -> Bounds {
        let unit = crate::chrome::repos_detail_line(ctx.density);
        bounds(self.diff_last(ctx.page) + ctx.page.max(1), ctx.page, unit)
    }

    /// Moved by `motion` at `now`.
    pub fn scroll(&mut self, motion: crate::scroll::Motion, now: std::time::Instant, ctx: &Ctx) {
        let b = self.scroll_bounds(ctx);
        self.scroll.apply(motion, now, b);
        self.take_slack(ctx.page);
    }

    /// Moved on to `now` while it moves on its own; whether it still does.
    pub fn scroll_tick(&mut self, now: std::time::Instant, ctx: &Ctx) -> bool {
        if !self.scroll.moving(now) {
            return false;
        }
        let b = self.scroll_bounds(ctx);
        let moving = self.scroll.tick(now, b);
        self.take_slack(ctx.page);
        moving
    }

    /// Stand at row `pos`, to the fraction, as the scrollbar's thumb held
    /// there puts it.
    pub fn stand_at(&mut self, pos: f64, ctx: &Ctx) {
        let b = self.scroll_bounds(ctx);
        self.scroll.jump(pos.clamp(0.0, b.max.max(0.0)));
        self.take_slack(ctx.page);
    }
}
