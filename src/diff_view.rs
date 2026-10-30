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
        Hint, ReposButton, ReposHit, RowMark, Tone,
    },
    diff::{FileDiff, LineKind},
    diff_edit::{FileEdit, Put},
    find,
    git::Change,
    review::{self, Comment, Quoted, Review},
    scroll::{Bounds, Scroll},
    text_field::{Edit, TextField},
};

mod search;
mod vim;

use search::DiffSearch;

pub const NO_REVIEW: &str = "No review comments: in the diff, press C on a line";

/// A checkout the diff shows, as its host lends it.
#[derive(Clone, Copy, Debug)]
pub struct Section<'a> {
    /// Its top folder: who it is, as the diff is read again.
    pub root: &'a Path,
    /// What its header calls it.
    pub label: &'a str,
    pub branch: Option<&'a str>,
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
    /// The file the host has selected, its header marked.
    pub selected: Option<(&'a Path, &'a str)>,
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
    /// Keep what this would cut, to paste, and copy it.
    Yank(FileEdit),
    /// Put what was yanked or deleted last in a file.
    Paste(Put),
    /// Undo the last cut; do the last undone again.
    UndoCut,
    RedoCut,
    /// Ask whether to delete every review comment (`clear_sheet`), then
    /// `clear_review`.
    Clear,
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
    /// Each comment's rows (by index into `review.comments`) and the
    /// editor's, as byte ranges into their text, wrapped at `cols`.
    comment_wraps: Vec<Vec<Range<usize>>>,
    editor_wrap: Vec<Range<usize>>,
    cols: usize,
    /// The comments whose lines are gone, as the rows were last worked
    /// out; and the lines comments are on, by `DiffRow::Line`'s indices.
    outdated: HashSet<usize>,
    noted: HashSet<(usize, usize, usize, usize)>,
    /// The search, while open.
    pub(crate) search: Option<DiffSearch>,
    /// Visual mode, while on.
    pub(crate) visual: Option<vim::Visual>,
    /// What is typed of a vim command so far.
    pending: vim::Pending,
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
    Note(usize, usize, usize),
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
            | DiffRow::Note(k, f, _) => Some((k, f)),
            DiffRow::Comment { at, .. }
            | DiffRow::Editor { at, .. }
            | DiffRow::EditorHint { at } => at,
            DiffRow::Repo(_) | DiffRow::Reading(_) | DiffRow::Outdated(_) | DiffRow::Gap => None,
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

/// The key a diff button is pressed with, as its chip says.
fn key_name(button: ReposButton) -> &'static str {
    use ReposButton as B;
    match button {
        B::CopyReview => "Ctrl+Shift+C",
        B::Comment => "C",
        B::SaveComment => "Enter",
        B::DiscardComment => "Esc",
        _ => "",
    }
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
        if let Some(mode) = self.mode_title() {
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
        format!("{lead} · {files} · +{added} −{removed}")
    }

    /// Work the diff's rows out again for `items`, scrolled back to
    /// `anchor`: per repository with changes its header, then each file's
    /// header and, unless folded, its hunks (each after a line saying what
    /// it is in, but the first when git says nothing) and notes; under a
    /// line, the review comments on it, and the comment being written.
    /// Last, the comments whose lines are gone. The cursor stays on who it
    /// was on.
    pub(crate) fn build_rows(&mut self, items: &[Section], anchor: Option<Anchor>) {
        let cols = if self.cols == 0 { 80 } else { self.cols };
        let comments = &self.review.comments;
        self.comment_wraps = comments
            .iter()
            .map(|c| review::wrap(&c.text, cols))
            .collect();
        self.editor_wrap = self
            .editing
            .as_ref()
            .map(|e| review::wrap(&e.field, cols))
            .unwrap_or_default();
        // Shown under their lines, or waiting for their diffs.
        let mut placed: HashSet<usize> = HashSet::new();
        let mut noted = HashSet::new();
        let mut rows = Vec::new();
        for (k, item) in items.iter().enumerate() {
            if item.changes.is_empty() {
                continue;
            }
            if !rows.is_empty() {
                rows.push(DiffRow::Gap);
            }
            rows.push(DiffRow::Repo(k));
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
                rows.push(DiffRow::File(k, f));
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
                    continue;
                }
                let at = Some((k, f));
                for (h, hunk) in file.hunks.iter().enumerate() {
                    if h > 0 || !hunk.context.is_empty() {
                        rows.push(DiffRow::Hunk(k, f, h));
                    }
                    for l in 0..hunk.lines.len() {
                        rows.push(DiffRow::Line(k, f, h, l));
                        for &c in under.get(&(h, l)).into_iter().flatten() {
                            self.push_comment(&mut rows, at, c);
                        }
                        if new_at == Some((h, l)) {
                            self.push_editor(&mut rows, at);
                        }
                    }
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
                rows.push(DiffRow::Outdated(Some(c)));
                self.push_comment(&mut rows, None, c);
            }
            if lost {
                self.push_editor(&mut rows, None);
            }
        }
        self.outdated = gone.into_iter().collect();
        self.noted = noted;
        self.rows = rows;
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
        if self.editing.as_ref().is_some_and(|e| e.at == Some(c)) {
            return self.push_editor(rows, at);
        }
        let n = self.comment_wraps.get(c).map_or(1, Vec::len);
        rows.extend((0..n).map(|i| DiffRow::Comment { at, c, i }));
    }

    /// The editor's rows, in file `at`, and the line saying its keys.
    fn push_editor(&self, rows: &mut Vec<DiffRow>, at: Option<(usize, usize)>) {
        rows.extend((0..self.editor_wrap.len().max(1)).map(|i| DiffRow::Editor { at, i }));
        rows.push(DiffRow::EditorHint { at });
    }

    /// The rows worked out again when the comments' rows hold another
    /// number of characters now.
    fn rewrap(&mut self, ctx: &Ctx) {
        if ctx.cols == self.cols {
            return;
        }
        self.cols = ctx.cols;
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
            DiffRow::Line(k, f, h, l) => {
                let (root, file) = self.diff_file(k, f)?;
                let line = file.hunks.get(h)?.lines.get(l)?;
                RowKey::Line(root.to_path_buf(), file.path.clone(), Quoted::of(line))
            }
            DiffRow::Comment { c, .. } | DiffRow::Outdated(Some(c)) => RowKey::Comment(c),
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
                    let DiffRow::File(k, f) = self.rows[h] else {
                        return None;
                    };
                    let (hunk, l) = review::find(self.diff_file(k, f)?.1, quoted)?;
                    let n = self.rows[h..]
                        .iter()
                        .position(|r| *r == DiffRow::Line(k, f, hunk, l))?;
                    Some(h + n)
                });
                line.or(head).or_else(|| self.header_row(root, None))
            }
            RowKey::Comment(c) => self.rows.iter().position(|r| {
                matches!(*r, DiffRow::Comment { c: x, i: 0, .. } | DiffRow::Outdated(Some(x)) if x == *c)
            }),
            RowKey::At(r) => Some((*r).min(last)),
        }
    }

    /// Put the diff's cursor on row `row`, its range reaching back to
    /// `from` (none without).
    pub(crate) fn set_cursor(&mut self, row: usize, from: Option<usize>) {
        let from = from.unwrap_or(row);
        let keys = (self.row_key(row), self.row_key(from));
        let col = self.cursor.as_ref().map_or(0, |c| c.col);
        self.cursor = Some(Cursor {
            row,
            from,
            col,
            keys,
        });
    }

    /// The text of diff row `r`, while it is a line.
    pub(crate) fn line_text(&self, r: usize) -> Option<&str> {
        let DiffRow::Line(k, f, h, l) = *self.rows.get(r)? else {
            return None;
        };
        let (_, file) = self.diff_file(k, f)?;
        Some(&file.hunks.get(h)?.lines.get(l)?.text)
    }

    /// The diff row the caret is on and the char of its line: on the
    /// cursor, while that is on a line.
    pub(crate) fn caret(&self) -> Option<(usize, usize)> {
        let cursor = self.cursor.as_ref()?;
        let len = self.line_text(cursor.row)?.chars().count();
        Some((cursor.row, cursor.col.min(len.saturating_sub(1))))
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
        self.follow(row, ctx.page.max(1));
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
    fn comment_at(&self, r: usize) -> Option<usize> {
        match *self.rows.get(r)? {
            DiffRow::Comment { c, .. } | DiffRow::Outdated(Some(c)) => Some(c),
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
    /// `r`: not room between repositories, nor the editor's rows.
    fn cursor_stop(&self, r: usize, dir: i64) -> usize {
        let stops = |r: usize| {
            !matches!(
                self.rows[r],
                DiffRow::Gap | DiffRow::Editor { .. } | DiffRow::EditorHint { .. }
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
    /// `page`: in a file, under its header pinned over the row above it.
    fn follow(&mut self, r: usize, page: usize) {
        let page = page.max(1);
        let top = match self.file_head(r) {
            Some(h) if h < r && page > 1 => r - 1,
            _ => r,
        };
        let last = self.diff_last(page) as f64;
        let to = showing(self.scroll.pos().min(last), (top, r), page);
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

    /// Write a comment on the lines of the cursor's range (C), or edit the
    /// comment the cursor is on.
    fn comment_here(&mut self, ctx: &Ctx) {
        if let Some(c) = self
            .cursor
            .as_ref()
            .and_then(|cur| self.comment_at(cur.row))
        {
            return self.edit_comment(c, ctx);
        }
        let Some((root, path, lines)) = self.cursor_lines().filter(|(_, _, l)| !l.is_empty())
        else {
            let why = "Move the diff's cursor to a line to comment on it";
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
        });
        self.build_rows(ctx.sections, self.diff_anchor());
        self.follow_editor(ctx);
    }

    /// Edit comment `c`, in its place.
    fn edit_comment(&mut self, c: usize, ctx: &Ctx) {
        let Some(comment) = self.review.comments.get(c) else {
            return;
        };
        self.signals.error = Some(None);
        self.signals.focus = true;
        self.editing = Some(Editing {
            at: Some(c),
            root: comment.root.clone(),
            path: comment.path.clone(),
            lines: comment.lines.clone(),
            field: TextField::multiline(&comment.text),
        });
        self.build_rows(ctx.sections, self.diff_anchor());
        self.follow_editor(ctx);
    }

    /// Keep the comment being written: a comment written before, emptied,
    /// is deleted.
    fn save_comment(&mut self, ctx: &Ctx) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let text = editing.field.trim().to_owned();
        match (editing.at, text.is_empty()) {
            (Some(c), true) if c < self.review.comments.len() => {
                self.review.comments.remove(c);
            }
            (Some(c), false) if c < self.review.comments.len() => {
                self.review.comments[c].text = text;
            }
            (None, false) => self.review.comments.push(Comment {
                root: editing.root,
                path: editing.path,
                lines: editing.lines,
                text,
            }),
            _ => {}
        }
        self.review_dirty = true;
        self.build_rows(ctx.sections, self.diff_anchor());
    }

    /// Delete the comment the cursor is on, the cursor going to the row
    /// over it.
    fn delete_comment(&mut self, ctx: &Ctx) {
        let Some(cursor) = &self.cursor else {
            return;
        };
        let Some(c) = self.comment_at(cursor.row) else {
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
    /// (or Ctrl+Enter) keeps it, Shift+Enter breaks the line, Esc drops
    /// it; Ctrl+Shift+C copies the review still.
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
        if sym == Keysym::Escape && plain {
            self.editing = None;
            self.build_rows(ctx.sections, self.diff_anchor());
            return None;
        }
        if enter && (plain || only_ctrl) {
            self.save_comment(ctx);
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
            B::SaveComment => self.save_comment(ctx),
            B::DiscardComment => {
                self.editing = None;
                self.build_rows(ctx.sections, self.diff_anchor());
            }
            B::DeleteComment => {
                if let Some(editing) = &mut self.editing {
                    editing.field = TextField::multiline("");
                }
                self.save_comment(ctx);
            }
            _ => return false,
        }
        true
    }

    /// Whether a press on the diff, held, drags the cursor's range on:
    /// the cursor is on a row and no comment is being written.
    pub fn drags_rows(&self, ctx: &Ctx) -> bool {
        ctx.keys == Keys::Diff && self.cursor.is_some() && self.editing.is_none()
    }

    /// The pointer dragged past the diff's rows: the cursor goes on a
    /// row `by` (up negative), its range kept, the diff scrolled along.
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
        if self.review.is_empty() {
            return Err(NO_REVIEW);
        }
        let order: Vec<PathBuf> = ctx.sections.iter().map(|i| i.root.to_path_buf()).collect();
        let label = |root: &Path| label_of(root, ctx);
        let comments = &self.review.comments;
        let outdated = |c: &Comment| {
            let at = comments.iter().position(|x| std::ptr::eq(x, c));
            at.is_some_and(|at| self.outdated.contains(&at))
        };
        Ok(Outcome::Copy {
            text: review::markdown(&self.review, &order, label, outdated),
            count: comments.len(),
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
        let head = match place {
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
        };
        if let Some(head) = head {
            self.scroll.jump(head as f64);
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
                DetailRow::Line(file.hunks[h].lines[l].clone())
            }
            (DiffRow::Note(_, _, n), Some((_, file))) => DetailRow::Note(file.notes[n].clone()),
            (DiffRow::Comment { c, i, .. }, _) => {
                let (Some(comment), Some(wrap)) =
                    (self.review.comments.get(c), self.comment_wraps.get(c))
                else {
                    return blank();
                };
                let range = wrap.get(i).cloned().unwrap_or(0..0);
                DetailRow::Comment {
                    text: comment.text[range].to_owned(),
                    first: i == 0,
                    last: i + 1 >= wrap.len(),
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
                }
            }
            (DiffRow::EditorHint { .. }, _) => {
                DetailRow::Note("Shift+Enter breaks the line".into())
            }
            (DiffRow::Outdated(None), _) => DetailRow::Hunk {
                context: "Comments on lines no longer in the diff".into(),
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
            Some(&DiffRow::Line(k, f, h, l)) => self.noted.contains(&(k, f, h, l)),
            _ => false,
        };
        let Some(c) = &self.cursor else {
            return RowMark {
                noted,
                ..RowMark::default()
            };
        };
        let range = c.row.min(c.from)..=c.row.max(c.from);
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
    fn row_buttons(&self, shown: &[usize]) -> Vec<(usize, Hint)> {
        use ReposButton as B;
        let at = |r: usize| shown.iter().position(|&x| x == r);
        let hint = |button: B| Hint {
            button,
            label: button.label().into(),
            key: key_name(button).into(),
            disabled: None,
        };
        if let Some(editing) = &self.editing {
            let Some(i) = self
                .rows
                .iter()
                .position(|r| matches!(r, DiffRow::EditorHint { .. }))
                .and_then(at)
            else {
                return Vec::new();
            };
            let mut buttons = vec![(i, hint(B::SaveComment)), (i, hint(B::DiscardComment))];
            if editing.at.is_some() {
                buttons.push((i, hint(B::DeleteComment)));
            }
            return buttons;
        }
        let Some(cursor) = &self.cursor else {
            return Vec::new();
        };
        let lines = self.cursor_lines().is_some_and(|(_, _, l)| !l.is_empty());
        match at(cursor.row) {
            Some(i) if lines && self.comment_at(cursor.row).is_none() => {
                vec![(i, hint(B::Comment))]
            }
            _ => Vec::new(),
        }
    }

    /// The title row's button copying the review, while there is one.
    fn title_buttons(&self) -> Vec<Hint> {
        let label = match self.review.comments.len() {
            0 => return Vec::new(),
            1 => "Copy 1 comment".to_owned(),
            n => format!("Copy {n} comments"),
        };
        let hint = |button: ReposButton, label: String| Hint {
            button,
            label,
            key: String::new(),
            disabled: None,
        };
        vec![
            hint(ReposButton::CopyReview, label),
            hint(
                ReposButton::ClearReview,
                ReposButton::ClearReview.label().into(),
            ),
        ]
    }

    /// What the sheet asking whether to delete the review says: its title,
    /// and a line per file with comments.
    pub fn clear_sheet(&self, ctx: &Ctx) -> (String, Vec<(String, String, Tone)>) {
        let mut files: BTreeMap<(String, &str), usize> = BTreeMap::new();
        for c in &self.review.comments {
            *files
                .entry((label_of(&c.root, ctx), c.path.as_str()))
                .or_default() += 1;
        }
        let lines = files
            .into_iter()
            .map(|((repo, path), n)| (repo, format!("{path}: {}", comments(n)), Tone::Warning))
            .collect();
        let n = self.review.comments.len();
        let title = format!("Delete the review's {} for good?", comments(n));
        (title, lines)
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
            DiffRow::Comment { c, .. } => {
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
        if self.editing.is_some() {
            let key = self.detail_row_at(i, ctx).and_then(|r| self.row_key(r));
            self.save_comment(ctx);
            let r = key.and_then(|k| self.find_row(&k))?;
            return self.goto_row(r, i, ctx);
        }
        let r = self.detail_row_at(i, ctx)?;
        self.goto_row(r, i, ctx)
    }

    /// The cursor on diff row `r`, shown as row `i`, the caret on the char
    /// pressed there; and its code opened in Zed.
    fn goto_row(&mut self, r: usize, i: usize, ctx: &Ctx) -> Option<Outcome> {
        self.signals.focus = true;
        self.leave_visual();
        self.set_cursor(r, None);
        if let (Some((j, col)), Some(c)) = (self.pointer_char, &mut self.cursor)
            && j == i
        {
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
        let by = match sym {
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
                if let Some(head) = self.current_file(page) {
                    self.fold_diff_file(head, fold, ctx);
                }
            }
            Keysym::Return | Keysym::KP_Enter if plain => {
                let r = self.open_target(page)?;
                return Some(self.open_diff_row(r));
            }
            Keysym::c if plain => self.comment_here(ctx),
            Keysym::Delete | Keysym::KP_Delete if plain => self.delete_comment(ctx),
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
        self.review.comments.clear();
        self.review_dirty = true;
        self.editing = None;
        self.rebuild(sections);
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
        let mut found: Vec<_> = shown.iter().map(|&r| self.found_on(r)).collect();
        let mut row_buttons = self.row_buttons(&shown);
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
            let i = shown.iter().position(|&s| s == r)?;
            matches!(rows.get(i)?, DetailRow::Line(_)).then_some((i, col))
        });
        let word = caret.and_then(|(i, col)| match rows.get(i)? {
            DetailRow::Line(line) => Some((i, find::word_at(&line.text, col)?)),
            _ => None,
        });
        DetailView {
            tab: DetailTab::Diff,
            title: self.title(ctx),
            search: self.search_bar(ctx),
            found,
            chosen: self.chosen(&shown),
            caret,
            word,
            ticks: self.search_ticks(),
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
        if sym.is_modifier_key() {
            return None;
        }
        self.rewrap(ctx);
        if self.search_has_keys(ctx) {
            return Some(self.search_key(mods, sym, typed, ctx));
        }
        if mods.ctrl
            && mods.shift
            && !(mods.alt || mods.logo)
            && matches!(sym, Keysym::f | Keysym::F)
        {
            self.open_search(ctx);
            return Some(None);
        }
        let in_diff = ctx.keys == Keys::Diff;
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
        if hit == ReposHit::Button(ReposButton::ClearReview) {
            return Some(Some(Outcome::Clear));
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
        // The editor's own buttons act on what it holds.
        if let ReposHit::Button(
            button @ (ReposButton::Comment
            | ReposButton::SaveComment
            | ReposButton::DiscardComment
            | ReposButton::DeleteComment),
        ) = hit
        {
            self.signals.focus = true;
            self.comment_button(button, ctx);
            return Some(None);
        }
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
                    if let (Some((j, col)), Some(c)) = (self.pointer_char, &mut self.cursor)
                        && j == i
                    {
                        c.col = col;
                    }
                    self.select_diff_row(r, ctx);
                    // On a comment, it is edited.
                    if let Some(c) = self.comment_at(r) {
                        self.edit_comment(c, ctx);
                    }
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
