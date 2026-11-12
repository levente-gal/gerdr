//! The diff's deletes, as vim's are: `x` (`X` back) a char, `D` on to
//! the line's end, `d` with a motion: `dd`, `dj` and `dk` lines, `dw`,
//! `de`, `db`, `dh`, `dl`, `d0`, `d^` and `d$` along the line; a count
//! first does more. `v` selects by chars and `V` by lines as the caret
//! goes, as Shift with the arrows does; `d` or `x` deletes what is
//! selected. Each cuts the file in the working tree, if it still holds
//! what the diff showed of it, and the caret stays where the cut was as
//! the diff is read again. `y` yanks as `d` deletes (`yy`, `yw`, `Y`, a
//! selection) and copies it too; `p` puts what was yanked or deleted
//! last after the caret (whole lines under its line), `P` before. `u`
//! undoes the last cut, paste or restore in the file at the caret (see
//! `restore`), `U` or Ctrl+R does it again. `gg` and `G` go to the top and the end, `{` and `}` to
//! the hunk before and after, Ctrl+U and Ctrl+D half a page up and down.

use std::{
    ops::Range,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use super::{Ctx, DiffKey, DiffRow, DiffView, NOWHERE, Outcome, RowKey};
use crate::{
    chrome::ReposButton,
    diff::LineKind,
    diff_edit::{self, Cut, FileEdit, Put, Register, Spot},
    key_table::{self, Binding},
    repos::key,
};

/// Vim's keys with a row: those with a button and Ctrl's. The rest are
/// what is typed (vim's symbols, K8), counts and keys that take more than
/// one (`dw`, `gg`), matched in `vim_key`; F1 lists those as text
/// (`super::VIM_F1`).
pub(super) const VIM_KEYS: &[Binding<(), DiffKey>] = &[
    key!(
        "]",
        DiffKey::Button(ReposButton::NextComment),
        "in the diff, go to the next or the previous finding or comment"
    ),
    key!(
        "[",
        DiffKey::Button(ReposButton::PrevComment),
        "in the diff, go to the next or the previous finding or comment"
    ),
    key!(
        "Ctrl+C",
        DiffKey::Button(ReposButton::CopyLines),
        "in the diff, copy what the cursor is on: the selection, a line, a file's path, a hunk"
    ),
    key!(
        "Shift+R",
        DiffKey::Button(ReposButton::Restore),
        "in the diff, restore the change at the caret or the lines selected"
    ),
    key!(
        "Ctrl+R",
        DiffKey::Redo,
        "in the diff, do the edit undone again"
    ),
];

/// The row of `VIM_KEYS` the key is. Without Ctrl, it is what it types,
/// as vim's keys are: `]` wherever the layout has it, a capital as Shift
/// and its letter.
fn vim_row(mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Option<DiffKey> {
    if mods.ctrl {
        return key_table::press(VIM_KEYS, &(), mods, sym);
    }
    let shift = typed.key_char().is_some_and(|c| c.is_ascii_uppercase());
    let mods = ModifiersState { shift, ..*mods };
    key_table::press(VIM_KEYS, &(), &mods, typed)
}

/// The most a count goes to.
const MOST: usize = 9999;
/// How long keys typed after a cut wait for the diff to be read again.
const WAIT: Duration = Duration::from_secs(2);

/// The diff's visual mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Visual {
    /// By chars, from char `col` of the line `key` is: row `row`, as the
    /// rows were last worked out.
    Chars { key: RowKey, row: usize, col: usize },
    /// By lines: the cursor's range.
    Lines,
}

/// What is typed of a command so far: a count, and `d` or `y`; or `g`,
/// Space after it opening the code at the caret in Zed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Pending {
    count: usize,
    op: Option<Op>,
    go: bool,
}

/// What a motion after it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Delete,
    Yank,
}

/// How far a `d` or `y` goes: lines around the caret's (by how many, up
/// and down), or chars along it.
enum Reach {
    Lines(i64, i64),
    Chars(Range<usize>),
}

/// Where the caret goes once a file is read again after a cut: its line
/// in file `path` of the repository at `root`, the diff read for a Git
/// status read after `generation`. Cut `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CaretAfter {
    root: PathBuf,
    path: String,
    spot: Spot,
    generation: u64,
    at: Instant,
}

/// A line of the diff in the file as it is now: the repository's root,
/// the file's path, the line's number and the lines the diff shows of it.
type LineOf = (PathBuf, String, usize, Vec<(usize, String)>);

/// A key as `DiffView::key` takes it.
pub type Key = (ModifiersState, Keysym, Keysym);

impl DiffView {
    /// A key for the diff's deletes, visual mode, counts, undo and `g`
    /// Space, while it has the keyboard: what it does, `None` for none of
    /// those.
    pub(super) fn vim_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Option<Outcome>> {
        // Ctrl+R does again, as vim's does; Ctrl+D and Ctrl+U go half a
        // page down and up.
        let only_ctrl = mods.ctrl && !(mods.shift || mods.alt || mods.logo);
        if only_ctrl {
            if let Some(key) = vim_row(mods, sym, typed) {
                self.pending = Pending::default();
                return Some(self.vim_do(key, 1, ctx));
            }
            // Vim's half pages: F1 lists them with `gg`.
            let half = (ctx.page.max(2) / 2) as i64;
            let by = match sym {
                Keysym::d | Keysym::D => half,
                Keysym::u | Keysym::U => -half,
                _ => 0,
            };
            if by != 0 {
                self.pending = Pending::default();
                let lines = self.visual == Some(Visual::Lines);
                self.move_cursor(by, lines, ctx);
                return Some(None);
            }
        }
        if mods.ctrl || mods.alt || mods.logo {
            return None;
        }
        let pending = std::mem::take(&mut self.pending);
        if sym == Keysym::Escape {
            let any = pending != Pending::default() || self.visual.is_some();
            self.leave_visual();
            return any.then_some(None);
        }
        let digit = (typed.raw() >= Keysym::_0.raw() && typed.raw() <= Keysym::_9.raw())
            .then(|| (typed.raw() - Keysym::_0.raw()) as usize);
        if let Some(d) = digit.filter(|&d| d > 0 || pending.count > 0) {
            self.pending = Pending {
                count: (pending.count * 10 + d).min(MOST),
                ..pending
            };
            return Some(None);
        }
        // `g` Space: as Zed's, the code at the caret in Zed; `gg` the
        // top; `g` and anything else says what `g` takes.
        if pending.go {
            return Some(match (sym, typed) {
                (Keysym::space, _) if !mods.shift => {
                    let r = self.open_target(ctx.page.max(1));
                    r.and_then(|r| self.open_diff_row(r))
                }
                (_, Keysym::g) => {
                    self.cursor_to(0, ctx);
                    None
                }
                _ => Some(Outcome::Notice(
                    "After g: Space opens in Zed, g goes to the top",
                )),
            });
        }
        let count = pending.count.max(1);
        if let Some(op) = pending.op {
            return Some(match op {
                Op::Delete => self.delete_motion(sym, typed, count),
                Op::Yank => self.yank_motion(sym, typed, count),
            });
        }
        if let Some(key) = vim_row(mods, sym, typed) {
            return Some(self.vim_do(key, count, ctx));
        }
        let selecting = self.selecting();
        let outcome = match typed {
            Keysym::v => {
                self.toggle_visual(false, ctx);
                None
            }
            Keysym::V => {
                self.toggle_visual(true, ctx);
                None
            }
            Keysym::d | Keysym::x | Keysym::X | Keysym::D if selecting => self.delete_selection(),
            Keysym::y | Keysym::Y if selecting => self.yank_selection(),
            Keysym::d | Keysym::y => {
                let op = if typed == Keysym::d {
                    Op::Delete
                } else {
                    Op::Yank
                };
                self.pending = Pending {
                    op: Some(op),
                    ..pending
                };
                None
            }
            Keysym::g => {
                self.pending = Pending {
                    go: true,
                    ..Pending::default()
                };
                None
            }
            Keysym::G => {
                self.cursor_to(self.rows.len().saturating_sub(1), ctx);
                None
            }
            Keysym::braceright | Keysym::braceleft => {
                let down = typed == Keysym::braceright;
                for _ in 0..count {
                    if let Some(to) = self.next_hunk(down) {
                        self.cursor_to(to, ctx);
                    }
                }
                None
            }
            Keysym::Y => self.yank_motion(Keysym::y, Keysym::y, count),
            Keysym::p | Keysym::P => {
                self.leave_visual();
                self.paste(typed == Keysym::P, count)
            }
            Keysym::x => self.cut_along(|col, _| col..col + count),
            Keysym::X => self.cut_along(|col, _| col.saturating_sub(count)..col),
            Keysym::D => self.cut_along(|col, len| col..len),
            Keysym::u => Some(Outcome::UndoCut(self.cursor_file())),
            Keysym::U => Some(Outcome::RedoCut(self.cursor_file())),
            // A count before a motion: the motion as many times.
            _ if count > 1 && motion(mods, sym) => {
                for _ in 0..count {
                    self.detail_press(mods, sym, typed, ctx);
                }
                None
            }
            _ => return None,
        };
        Some(outcome)
    }

    /// Do `key`, a row of `VIM_KEYS`, `count` times where that counts.
    fn vim_do(&mut self, key: DiffKey, count: usize, ctx: &Ctx) -> Option<Outcome> {
        use ReposButton as B;
        match key {
            // The next comment or finding, or the one before.
            DiffKey::Button(button @ (B::NextComment | B::PrevComment)) => {
                self.leave_visual();
                let by = if button == B::NextComment { 1 } else { -1 };
                let went = (0..count).fold(false, |_, _| self.jump_comment(by, ctx));
                (!went).then_some(Outcome::Notice(super::NO_COMMENTS))
            }
            DiffKey::Button(B::Restore) => self.restore_here(),
            DiffKey::Button(B::CopyLines) => Some(self.copy_here()),
            DiffKey::Redo => Some(Outcome::RedoCut(self.cursor_file())),
            _ => None,
        }
    }

    /// The cursor on diff row `to` (the nearest it stops on), as a move
    /// takes it there: by lines in visual mode, reaching its range.
    fn cursor_to(&mut self, to: usize, ctx: &Ctx) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        // The first move puts the cursor on the first row in view.
        if self.cursor.is_none() {
            self.move_cursor(1, false, ctx);
        }
        let Some(row) = self.cursor.as_ref().map(|c| c.row) else {
            return;
        };
        let by = to.min(n - 1) as i64 - row as i64;
        if by != 0 {
            let lines = self.visual == Some(Visual::Lines);
            self.move_cursor(by, lines, ctx);
        }
    }

    /// The header of the hunk (or file) after the cursor's row, `down`,
    /// else the one before.
    fn next_hunk(&self, down: bool) -> Option<usize> {
        let at = self.cursor.as_ref().map_or(0, |c| c.row);
        let head = |r: &usize| matches!(self.rows[*r], DiffRow::Hunk(..) | DiffRow::File(..));
        match down {
            true => (at + 1..self.rows.len()).find(head),
            false => (0..at).rev().find(head),
        }
    }

    /// Visual mode on (by lines when `lines`, else by chars) from the
    /// caret, or off when on that way already.
    fn toggle_visual(&mut self, lines: bool, ctx: &Ctx) {
        let on = match &self.visual {
            Some(Visual::Lines) => lines,
            Some(Visual::Chars { .. }) => !lines,
            None => false,
        };
        if on {
            return self.leave_visual();
        }
        if self.cursor.is_none() {
            self.move_cursor(1, false, ctx);
        }
        let Some(cursor) = &mut self.cursor else {
            return;
        };
        cursor.from = cursor.row;
        cursor.keys.1 = cursor.keys.0.clone();
        let row = cursor.row;
        self.visual = if lines {
            Some(Visual::Lines)
        } else {
            let col = self.caret().map_or(0, |(_, col)| col);
            self.row_key(row).map(|key| Visual::Chars { key, row, col })
        };
    }

    /// Visual mode off, the cursor's range with it.
    pub(super) fn leave_visual(&mut self) {
        if self.visual.take().is_some()
            && let Some(cursor) = &mut self.cursor
        {
            cursor.from = cursor.row;
            cursor.keys.1 = cursor.keys.0.clone();
        }
    }

    /// The visual mode's first row by chars, as the rows are now.
    pub(super) fn find_visual(&mut self) {
        let Some(Visual::Chars { key, .. }) = &self.visual else {
            return;
        };
        match self.find_row_of(key) {
            Some(r) => {
                if let Some(Visual::Chars { row, .. }) = &mut self.visual {
                    *row = r;
                }
            }
            None => self.visual = None,
        }
    }

    fn find_row_of(&self, key: &RowKey) -> Option<usize> {
        let r = self.find_row(key)?;
        matches!(self.rows.get(r), Some(DiffRow::Line(..))).then_some(r)
    }

    /// The chars selected by chars on the diff rows `shown`, by index
    /// among them.
    pub(crate) fn chosen(&self, shown: &[usize]) -> Vec<(usize, Range<usize>)> {
        let Some((Visual::Chars { row, col, .. }, (to, at))) =
            self.visual.as_ref().zip(self.caret())
        else {
            return Vec::new();
        };
        let ((r0, c0), (r1, c1)) = ordered((*row, *col), (to, at));
        let mut chosen = Vec::new();
        for (i, &r) in shown.iter().enumerate() {
            if r < r0 || r > r1 {
                continue;
            }
            let Some(text) = self.line_text(r) else {
                continue;
            };
            let len = text.chars().count();
            let from = if r == r0 { c0 } else { 0 };
            let to = if r == r1 { c1 + 1 } else { len };
            chosen.push((i, from..to.min(len.max(from))));
        }
        chosen
    }

    /// The file and line of diff row `r`, and its file's lines shown: who
    /// a cut there is in, unless the line isn't in the file as it is now.
    fn line_of(&self, r: usize) -> Result<LineOf, &'static str> {
        let Some(&DiffRow::Line(k, f, h, l)) = self.rows.get(r) else {
            return Err("No line there");
        };
        let (root, file) = self.diff_file(k, f).ok_or("No line there")?;
        let line = &file.hunks[h].lines[l];
        if line.kind == LineKind::Removed {
            return Err("A removed line isn't in the file");
        }
        if file.code == 'U' {
            return Err("Resolve the conflict in Zed first");
        }
        let shown = diff_edit::shown(file);
        Ok((root.to_path_buf(), file.path.clone(), line.number, shown))
    }

    /// The cut `cut` in the file of the caret's line, where the caret is.
    fn cut_here(&self, cut: impl FnOnce(Spot) -> Cut) -> Option<Outcome> {
        let (row, col) = self.caret()?;
        let (root, path, line, shown) = match self.line_of(row) {
            Ok(it) => it,
            Err(why) => return Some(Outcome::Notice(why)),
        };
        let caret = Spot { line, col };
        Some(Outcome::Cut(FileEdit {
            root,
            path,
            cut: cut(caret),
            shown,
            caret,
        }))
    }

    /// Put what was yanked or deleted last in at the caret, `count`
    /// times: after it, or `before`.
    fn paste(&self, before: bool, count: usize) -> Option<Outcome> {
        let (row, col) = self.caret()?;
        let (root, path, line, shown) = match self.line_of(row) {
            Ok(it) => it,
            Err(why) => return Some(Outcome::Notice(why)),
        };
        Some(Outcome::Paste(Put {
            root,
            path,
            shown,
            at: Spot { line, col },
            before,
            count,
        }))
    }

    /// Cut the chars `range` (of the caret's char, and the line's length)
    /// gives of the caret's line.
    fn cut_along(&self, range: impl FnOnce(usize, usize) -> Range<usize>) -> Option<Outcome> {
        let (row, col) = self.caret()?;
        let len = self.line_text(row)?.chars().count();
        let cols = range(col, len);
        if cols.start >= cols.end.min(len) {
            return Some(Outcome::Notice("Nothing there"));
        }
        self.cut_here(|at| {
            let spot = |col| Spot { col, ..at };
            Cut::Chars(spot(cols.start), spot(cols.end))
        })
    }

    /// `d` and then `sym` (`typed`), `count` times: lines, or chars along
    /// the caret's line, never past it.
    fn delete_motion(&mut self, sym: Keysym, typed: Keysym, count: usize) -> Option<Outcome> {
        match self.reach(Keysym::d, sym, typed, count)? {
            Reach::Lines(from, to) => self.cut_here(|at| {
                let n = at.line as i64;
                Cut::Lines(((n + from).max(1)..=n + to).map(|n| n as usize).collect())
            }),
            Reach::Chars(cols) => self.cut_along(|_, _| cols),
        }
    }

    /// `y` and then `sym` (`typed`), `count` times: as `d` would go, among
    /// the lines the diff shows, with a removed line as good as any. What
    /// isn't on a line, what the cursor is on (`yank_row`).
    fn yank_motion(&mut self, sym: Keysym, typed: Keysym, count: usize) -> Option<Outcome> {
        let reach = self.reach(Keysym::y, sym, typed, count)?;
        let Some((row, _)) = self.caret() else {
            return Some(self.yank_row());
        };
        Some(match reach {
            Reach::Lines(from, to) => {
                let shown: Vec<usize> = (0..self.rows.len())
                    .filter(|&r| self.line_at(r).is_some())
                    .collect();
                let at = shown.iter().position(|&r| r == row).unwrap_or(0) as i64;
                let first = (at + from).max(0) as usize;
                let last = (at + to).max(0) as usize;
                self.yank_rows(shown.iter().skip(first).take(last + 1 - first).copied())
            }
            Reach::Chars(cols) => {
                let line = self.line_at(row)?;
                let len = line.text.chars().count();
                match cols.start < cols.end.min(len) {
                    true => yank_of(diff_edit::shown_span(line.raw(), cols).to_owned(), false),
                    false => Outcome::Notice("Nothing there"),
                }
            }
        })
    }

    /// Whether lines or chars are selected.
    fn selecting(&self) -> bool {
        self.visual.is_some() || self.cursor.as_ref().is_some_and(|c| c.from != c.row)
    }

    /// Ctrl+C: what is selected, else the line or other row the cursor is
    /// on.
    pub(super) fn copy_here(&mut self) -> Outcome {
        if self.selecting() {
            return self.yank_selection().unwrap_or(Outcome::Notice(NOWHERE));
        }
        match self.caret() {
            Some((row, _)) => self.yank_rows(row..=row),
            None => self.yank_row(),
        }
    }

    /// Keep what is selected, to paste, and copy it: as the diff shows it,
    /// across hunks and files, with its removed lines.
    fn yank_selection(&mut self) -> Option<Outcome> {
        let visual = self.visual.take();
        let cursor = self.cursor.as_ref()?;
        let rows = cursor.from.min(cursor.row)..=cursor.from.max(cursor.row);
        let outcome = match (visual, self.caret()) {
            (Some(Visual::Chars { row, col, .. }), Some(at)) => self.yank_chars((row, col), at),
            _ => self.yank_rows(rows),
        };
        self.collapse_cursor();
        Some(outcome)
    }

    /// The lines among diff rows `rows`, whole.
    fn yank_rows(&self, rows: impl Iterator<Item = usize>) -> Outcome {
        let mut text = String::new();
        for line in rows.filter_map(|r| self.line_at(r)) {
            text.push_str(line.raw());
            text.push('\n');
        }
        match text.is_empty() {
            true => Outcome::Notice("No lines there"),
            false => yank_of(text, true),
        }
    }

    /// From char `a` of a row to char `b` of another, with both.
    fn yank_chars(&self, a: (usize, usize), b: (usize, usize)) -> Outcome {
        let ((r0, c0), (r1, c1)) = ordered(a, b);
        let parts: Vec<&str> = (r0..=r1)
            .filter_map(|r| {
                let line = self.line_at(r)?;
                let from = if r == r0 { c0 } else { 0 };
                let to = if r == r1 {
                    c1 + 1
                } else {
                    line.text.chars().count()
                };
                Some(diff_edit::shown_span(line.raw(), from..to))
            })
            .collect();
        let text = parts.join("\n");
        match text.is_empty() {
            true => Outcome::Notice("Nothing there"),
            false => yank_of(text, false),
        }
    }

    /// What the cursor's row off any line holds: a comment's or finding's
    /// text, a file's path, a hunk's lines.
    fn yank_row(&self) -> Outcome {
        let Some(row) = self.cursor.as_ref().map(|c| c.row) else {
            return Outcome::Notice(NOWHERE);
        };
        if let Some(comment) = (self.comment_at(row)).and_then(|c| self.review.comments.get(c)) {
            return yank_of(comment.text.clone(), false);
        }
        match self.rows.get(row) {
            Some(DiffRow::File(..)) => match self.diff_path(row) {
                Some((_, path)) => yank_of(path, false),
                None => Outcome::Notice("Nothing to copy there"),
            },
            Some(&DiffRow::Hunk(k, f, h)) => {
                let of_hunk = |&r: &usize| matches!(self.rows[r], DiffRow::Line(a, b, c, _) if (a, b, c) == (k, f, h));
                self.yank_rows((0..self.rows.len()).filter(of_hunk))
            }
            _ => Outcome::Notice("Nothing to copy there"),
        }
    }

    /// How far `twice` (`d` or `y`) and then `sym` (`typed`) goes, `count`
    /// times.
    fn reach(&self, twice: Keysym, sym: Keysym, typed: Keysym, count: usize) -> Option<Reach> {
        let count = count as i64;
        match sym {
            _ if sym == twice => return Some(Reach::Lines(0, count - 1)),
            Keysym::j | Keysym::Down | Keysym::KP_Down => return Some(Reach::Lines(0, count)),
            Keysym::k | Keysym::Up | Keysym::KP_Up => return Some(Reach::Lines(-count, 0)),
            _ => {}
        }
        let (row, col) = self.caret()?;
        let len = self.line_text(row)?.chars().count();
        // Where the motion goes, `count` times; past the line, its end
        // (start going back).
        let mut to = (row, col);
        for _ in 0..count {
            to = self.caret_target(to, sym, typed);
        }
        let along = |back: bool| match to.0 == row {
            true => to.1,
            false if back => 0,
            false => len,
        };
        let cols = match (sym, typed) {
            (Keysym::w, _) => col..along(false).max(col),
            (Keysym::e, _) => col..along(false) + 1,
            (Keysym::l, _) => col..col + count as usize,
            (Keysym::b | Keysym::h, _) | (_, Keysym::_0) => along(true)..col,
            (_, Keysym::asciicircum) => {
                let first = along(false);
                first.min(col)..first.max(col)
            }
            (_, Keysym::dollar) => col..len,
            _ => return None,
        };
        // `dw` on the line's last word takes it to the end.
        let cols = match sym {
            Keysym::w if to.0 != row || to == (row, col) => col..len,
            _ => cols,
        };
        Some(Reach::Chars(cols))
    }

    /// Cut what is selected: by chars, or the lines of the cursor's range.
    fn delete_selection(&mut self) -> Option<Outcome> {
        let visual = self.visual.take();
        let cursor = self.cursor.as_ref()?;
        let outcome = match (visual, self.caret()) {
            (Some(Visual::Chars { row, col, .. }), Some(at)) => self.cut_chars((row, col), at),
            _ => self.cut_lines(cursor.from.min(cursor.row)..=cursor.from.max(cursor.row)),
        };
        self.collapse_cursor();
        outcome
    }

    /// The cursor's range back to its row alone.
    fn collapse_cursor(&mut self) {
        if let Some(cursor) = &mut self.cursor {
            cursor.from = cursor.row;
            cursor.keys.1 = cursor.keys.0.clone();
        }
    }

    /// Cut from char `a` of a row to char `b` of another, with both:
    /// within one hunk, so that no line the diff doesn't show goes.
    fn cut_chars(&self, a: (usize, usize), b: (usize, usize)) -> Option<Outcome> {
        let ((r0, c0), (r1, c1)) = ordered(a, b);
        let hunk = |r: usize| match self.rows.get(r) {
            Some(&DiffRow::Line(k, f, h, _)) => Some((k, f, h)),
            _ => None,
        };
        if hunk(r0) != hunk(r1) {
            return Some(Outcome::Notice(
                "Select within one hunk, or by lines with V",
            ));
        }
        let ends = self
            .line_of(r0)
            .and_then(|first| Ok((first, self.line_of(r1)?)));
        let ((root, path, l0, shown), (_, _, l1, _)) = match ends {
            Ok(ends) => ends,
            Err(why) => return Some(Outcome::Notice(why)),
        };
        let caret = self.caret().map_or(0, |(_, col)| col);
        let caret = Spot {
            line: if self.caret().is_some_and(|(r, _)| r == r0) {
                l0
            } else {
                l1
            },
            col: caret,
        };
        Some(Outcome::Cut(FileEdit {
            root,
            path,
            cut: Cut::Chars(
                Spot { line: l0, col: c0 },
                Spot {
                    line: l1,
                    col: c1 + 1,
                },
            ),
            shown,
            caret,
        }))
    }

    /// Cut the lines in the file among diff rows `rows`: all in one file.
    fn cut_lines(&self, rows: std::ops::RangeInclusive<usize>) -> Option<Outcome> {
        let mut file = None;
        let mut numbers = Vec::new();
        let mut why = "No lines there";
        for r in rows {
            let Some(&DiffRow::Line(k, f, ..)) = self.rows.get(r) else {
                continue;
            };
            if file.get_or_insert((k, f)) != &(k, f) {
                return Some(Outcome::Notice("Select lines of one file"));
            }
            match self.line_of(r) {
                Ok(line) => numbers.push(line),
                Err(not) => why = not,
            }
        }
        let Some((root, path, _, shown)) = numbers.first().cloned() else {
            return Some(Outcome::Notice(why));
        };
        let caret = self.caret().and_then(|(r, col)| {
            let (_, _, line, _) = self.line_of(r).ok()?;
            Some(Spot { line, col })
        });
        let first = numbers[0].2;
        Some(Outcome::Cut(FileEdit {
            root,
            path,
            cut: Cut::Lines(numbers.into_iter().map(|(_, _, n, _)| n).collect()),
            shown,
            caret: caret.unwrap_or(Spot {
                line: first,
                col: 0,
            }),
        }))
    }

    /// Put the caret on line `spot` of file `path` of the repository at
    /// `root` once its diff is read again.
    pub fn place_caret(&mut self, root: PathBuf, path: String, spot: Spot) {
        self.leave_visual();
        self.caret_after = Some(CaretAfter {
            root,
            path,
            spot,
            generation: self.reads,
            at: Instant::now(),
        });
    }

    /// Whether a cut waits for its diff to be read again: what is typed
    /// in the diff meanwhile waits too, so that it goes by the diff as
    /// the cut left it.
    pub(super) fn waiting(&self) -> bool {
        self.caret_after
            .as_ref()
            .is_some_and(|a| a.at.elapsed() < WAIT)
    }

    /// Hold `key` until the diff is read again after a cut, if it waits
    /// for that; whether it does. Esc doesn't, and drops those held; after
    /// too long they are dropped.
    pub(super) fn hold_key(&mut self, key: Key) -> bool {
        let (_, sym, _) = key;
        if sym == Keysym::Escape || !self.waiting() {
            // Held too long, the file not read again: said, not lost
            // unseen.
            if !self.queued.is_empty() && sym != Keysym::Escape {
                let n = self.queued.len();
                let keys = if n == 1 { "key" } else { "keys" };
                let why = format!("{n} {keys} typed while the file was read again were dropped");
                self.signals.error = Some(Some(why));
            }
            self.queued.clear();
            return false;
        }
        self.queued.push(key);
        true
    }

    /// The keys held while the diff was read again after a cut, now that
    /// it is.
    pub fn take_held(&mut self) -> Vec<Key> {
        if self.waiting() {
            return Vec::new();
        }
        std::mem::take(&mut self.queued)
    }

    /// No caret to place in the repository at `root` once it has no
    /// changes left: its diff isn't read again.
    pub(super) fn forget_caret(&mut self, root_clean: impl Fn(&std::path::Path) -> bool) {
        if self
            .caret_after
            .as_ref()
            .is_some_and(|a| root_clean(&a.root))
        {
            self.caret_after = None;
        }
    }

    /// The caret put where it was to go, when this diff read of the
    /// repository at `root` for Git status read `generation` is the one.
    pub(super) fn caret_placed(&mut self, root: &std::path::Path, generation: u64, ctx: &Ctx) {
        let Some(after) = self
            .caret_after
            .take_if(|after| after.root == root && generation > after.generation)
        else {
            return;
        };
        let Some(head) = self.header_row(&after.root, Some(&after.path)) else {
            return;
        };
        let Some((k, f)) = self.rows[head].file() else {
            return;
        };
        let Some((_, file)) = self.diff_file(k, f) else {
            return;
        };
        // The line it was on, else the nearest in the file.
        let rows = (head..self.rows.len()).take_while(|&r| match self.rows[r] {
            DiffRow::File(..) | DiffRow::Repo(_) | DiffRow::Gap => r == head,
            _ => true,
        });
        let near = rows
            .filter_map(|r| match self.rows[r] {
                DiffRow::Line(_, _, h, l) => {
                    let line = &file.hunks[h].lines[l];
                    (line.kind != LineKind::Removed)
                        .then(|| (line.number.abs_diff(after.spot.line), r))
                }
                _ => None,
            })
            .min();
        if let Some((_, r)) = near {
            self.caret_to(r, after.spot.col, ctx);
        }
    }

    /// What the title says while a command is typed or visual mode is on;
    /// `edits` when the keys that change the files work here.
    pub(crate) fn mode_title(&self, edits: bool) -> Option<String> {
        let mode = match (&self.visual, edits) {
            (Some(Visual::Chars { .. }), true) => {
                "-- VISUAL -- d delete · y yank · Shift+R restore · Esc stop"
            }
            (Some(Visual::Lines), true) => {
                "-- VISUAL LINE -- d delete · y yank · Shift+R restore · Esc stop"
            }
            (Some(Visual::Chars { .. }), false) => "-- VISUAL -- y yank · Esc stop",
            (Some(Visual::Lines), false) => "-- VISUAL LINE -- y yank · Esc stop",
            (None, _) => "",
        };
        let Pending { count, op, go } = self.pending;
        let count = if count > 0 {
            count.to_string()
        } else {
            String::new()
        };
        let op = match op {
            Some(Op::Delete) => "d",
            Some(Op::Yank) => "y",
            None => "",
        };
        let go = if go { "g" } else { "" };
        let typed = format!("{count}{op}{go}");
        match (typed.is_empty(), mode.is_empty()) {
            (true, true) => None,
            (true, false) => Some(mode.into()),
            (false, true) => Some(typed),
            (false, false) => Some(format!("{mode} · {typed}")),
        }
    }
}

/// Whether `sym` moves the cursor or the caret in the diff.
fn motion(mods: &ModifiersState, sym: Keysym) -> bool {
    let letter = matches!(
        sym,
        Keysym::j | Keysym::k | Keysym::h | Keysym::l | Keysym::w | Keysym::b | Keysym::e
    );
    let arrow = matches!(
        sym,
        Keysym::Down | Keysym::Up | Keysym::KP_Down | Keysym::KP_Up
    );
    letter && !mods.shift || arrow
}

/// Keep `text` to paste, and copy it.
fn yank_of(text: String, lines: bool) -> Outcome {
    Outcome::Yank(Register { text, lines })
}

/// Two places in order.
fn ordered(a: (usize, usize), b: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    if a <= b { (a, b) } else { (b, a) }
}
