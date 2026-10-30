//! The scrollback viewer: all a terminal pane keeps of its output, read
//! from herdr as it opens and shown over the pane, to select more than a
//! screen of it and copy that. The terminal can't: it shows only herdr's
//! screen, which herdr draws again as it scrolls. Here the list scrolls on
//! its own, so a selection goes on while it does: dragged past an edge,
//! or with the wheel turned while the button is held.
//!
//! Lines are wrapped at the viewer's width (herdr joins those it wrapped),
//! and a place in them is a character of a line (see `Spot`).
//!
//! `/` or Ctrl+F searches it: every place found is tinted, the one gone
//! to more, Enter goes to the next (Shift+Enter, the one before), round
//! past the ends, and ticks on the scrollbar say where they all are. `*`
//! (`#` back) searches for the selection, or the word under the pointer
//! as a whole word, as vim's do.

use std::{ops::Range, time::Instant};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    chrome::{self, ScrollbackHit, SearchBar, SearchPart},
    find,
    scroll::{Bounds, Motion, Scroll},
    text_field::{Edit, TextField},
};

/// Rows a notch of the wheel scrolls.
const NOTCH_ROWS: f64 = 3.0;
/// How fast a drag past the list's top or bottom scrolls it, rows a
/// second: as fast at the edge, and faster for each logical pixel further.
const EDGE_SPEED: f64 = 12.0;
const EDGE_GAIN: f64 = 0.8;
/// The most a drag past an edge scrolls, rows a second.
const EDGE_MAX: f64 = 600.0;
/// Characters a double click takes as part of a word, besides letters and
/// digits: those of paths, flags and `file:line` places.
const WORD_CHARS: &str = "_-./~:@+=%#$&?*!\\";

/// How the list fits where it is shown, from the chrome's layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Characters a row holds.
    pub cols: usize,
    /// Rows that show whole.
    pub page: usize,
    /// A row's height, and the list's, in logical pixels.
    pub row_h: f64,
    pub view_h: f64,
}

impl Fit {
    fn bounds(&self, rows: usize) -> Bounds {
        Bounds {
            max: rows.saturating_sub(self.page) as f64,
            notch: NOTCH_ROWS,
            unit: self.row_h,
            next: self.row_h,
            prev: self.row_h,
            view: self.view_h,
        }
    }
}

/// A place in the text: before character `ch` of line `line` (its length
/// is after the last).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub line: usize,
    pub ch: usize,
}

/// A row as shown: characters `start` to `end` of line `line`, which are
/// bytes `from` to `to` of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    line: usize,
    start: usize,
    end: usize,
    from: usize,
    to: usize,
}

/// What a press and its drag select by: characters, or the words or
/// lines they reach into (a double or triple click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grain {
    Char,
    Word,
    Line,
}

/// The selection: from the place pressed (`anchor`) to the place the
/// pointer is now (`cursor`), each the span its grain takes there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: (Spot, Spot),
    cursor: (Spot, Spot),
    grain: Grain,
}

impl Selection {
    /// Where it starts and ends, in order.
    fn range(&self) -> (Spot, Spot) {
        (
            self.anchor.0.min(self.cursor.0),
            self.anchor.1.max(self.cursor.1),
        )
    }
}

/// The search, while open.
#[derive(Debug, Default)]
struct Find {
    field: TextField,
    /// It has the keyboard (else the list has, the places still tinted).
    focused: bool,
    /// It finds what is typed only as a whole word (`*`), until it is
    /// typed into.
    whole: bool,
    /// Every place found, in order: a line, and characters of it.
    hits: Vec<(usize, Range<usize>)>,
    /// The place gone to, by index into `hits`.
    current: Option<usize>,
    /// The first and the last line in view as the search opened: typing
    /// goes to the first place found there, else the nearest above.
    shown: (usize, usize),
}

/// What a key or a click asks of gerdr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    /// Copy `text`, `lines` long, and close the viewer when `close`.
    Copy {
        text: String,
        lines: usize,
        close: bool,
    },
    /// Copying, with nothing selected.
    NothingSelected,
}

/// The viewer as it stands.
pub struct Scrollback {
    /// The gerdr pane it is over, and its herdr pane.
    pub pane: u32,
    pub pane_id: String,
    /// What the pane's bar calls it.
    pub label: String,
    lines: Vec<String>,
    /// The lines wrapped at `cols`.
    rows: Vec<Row>,
    cols: usize,
    scroll: Scroll,
    /// How many rows up from the bottom the pane's own view stood, until
    /// the list is first fitted: it stands as far up.
    from_bottom: Option<usize>,
    selection: Option<Selection>,
    find: Option<Find>,
    /// The left button is held from a press on the text.
    dragging: bool,
    /// When the last frame moved a drag past an edge on.
    edge_at: Option<Instant>,
    /// What of the viewer is under the pointer.
    pub hover: Option<ScrollbackHit>,
    /// The last press on the text: when, where, and how many came in a
    /// row, for double and triple clicks.
    pub clicks: Option<(Instant, Point<f64, Logical>, u8)>,
}

impl Scrollback {
    /// The viewer over gerdr pane `pane` (herdr's `pane_id`), showing
    /// `text`, the pane's view `from_bottom` rows up from its bottom.
    pub fn new(pane: u32, pane_id: String, label: String, text: &str, from_bottom: u64) -> Self {
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|line| line.trim_end_matches('\r').to_owned())
            .collect();
        while lines.len() > 1 && lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        Self {
            pane,
            pane_id,
            label,
            lines,
            rows: Vec::new(),
            cols: 0,
            scroll: Scroll::default(),
            from_bottom: Some(from_bottom as usize),
            selection: None,
            find: None,
            dragging: false,
            edge_at: None,
            hover: None,
            clicks: None,
        }
    }

    /// Lay the list out for `fit`: wrapped again for another width, the
    /// row at the top kept there.
    pub fn fit(&mut self, fit: Fit) {
        let cols = fit.cols.max(1);
        if cols != self.cols || self.rows.is_empty() {
            let top = self.rows.get(self.scroll.first()).map(|r| Spot {
                line: r.line,
                ch: r.start,
            });
            self.cols = cols;
            self.rows = wrap(&self.lines, cols);
            if let Some(top) = top {
                self.scroll.jump(self.row_of(top) as f64);
            }
        }
        let max = fit.bounds(self.rows.len()).max;
        if let Some(up) = self.from_bottom.take() {
            self.scroll.jump((max - up as f64).max(0.0));
        } else if self.scroll.pos() > max {
            self.scroll.jump(max);
        }
    }

    /// How many lines, and rows, it shows.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Where the list stands, in rows (see `Scroll`).
    pub fn first(&self) -> usize {
        self.scroll.first()
    }

    pub fn part(&self) -> f64 {
        self.scroll.part()
    }

    pub fn overscroll(&self) -> f64 {
        self.scroll.overscroll()
    }

    /// The text of row `i`.
    pub fn row_text(&self, i: usize) -> &str {
        self.rows
            .get(i)
            .map_or("", |r| &self.lines[r.line][r.from..r.to])
    }

    /// What of row `i` is selected: from and to which of its characters,
    /// and whether the selection goes on past its line's end there.
    pub fn row_selection(&self, i: usize) -> Option<(usize, usize, bool)> {
        let row = self.rows.get(i)?;
        let (start, end) = self.selection?.range();
        if row.line < start.line || row.line > end.line {
            return None;
        }
        let from = if row.line == start.line { start.ch } else { 0 };
        let to = if row.line == end.line {
            end.ch
        } else {
            usize::MAX
        };
        let last = self
            .rows
            .get(i + 1)
            .is_none_or(|next| next.line != row.line);
        let on = last && row.line < end.line && from <= row.end;
        let a = from.clamp(row.start, row.end);
        let b = to.min(row.end).max(a);
        (b > a || on).then(|| (a - row.start, b - row.start, on))
    }

    /// How many lines the selection reaches into, if there is one.
    pub fn selected_lines(&self) -> Option<usize> {
        let (start, end) = self.selection?.range();
        (start != end).then(|| end.line - start.line + 1)
    }

    /// The selected text, its lines joined by newlines.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection?.range();
        if start == end {
            return None;
        }
        let mut out = Vec::new();
        for (i, line) in self.lines[start.line..=end.line].iter().enumerate() {
            let n = start.line + i;
            let from = if n == start.line { start.ch } else { 0 };
            let to = if n == end.line { end.ch } else { usize::MAX };
            out.push(chars(line, from, to));
        }
        Some(out.join("\n"))
    }

    /// The place at character `col` of row `row`, or the nearest row's
    /// when there is none.
    fn spot(&self, row: i64, col: usize) -> Spot {
        let last = self.rows.len().saturating_sub(1);
        let Some(r) = self.rows.get(row.clamp(0, last as i64) as usize) else {
            return Spot::default();
        };
        let ch = if row < 0 {
            r.start
        } else if row as usize > last {
            r.end
        } else {
            r.start + col.min(r.end - r.start)
        };
        Spot { line: r.line, ch }
    }

    /// The row a place shows on.
    fn row_of(&self, spot: Spot) -> usize {
        let after = self
            .rows
            .partition_point(|r| (r.line, r.start) <= (spot.line, spot.ch));
        after.saturating_sub(1)
    }

    /// What `grain` takes at `spot`: the place itself, the word or the
    /// gap there, or its whole line.
    fn span(&self, spot: Spot, grain: Grain) -> (Spot, Spot) {
        let line = &self.lines[spot.line];
        let at = |ch| Spot {
            line: spot.line,
            ch,
        };
        match grain {
            Grain::Char => (spot, spot),
            Grain::Line => (at(0), at(line.chars().count())),
            Grain::Word => {
                let cs: Vec<char> = line.chars().collect();
                let Some(&c) = cs.get(spot.ch).or(cs.last()) else {
                    return (spot, spot);
                };
                let i = spot.ch.min(cs.len() - 1);
                let same =
                    |d: char| is_word(d) == is_word(c) && d.is_whitespace() == c.is_whitespace();
                let start = (0..i).rev().find(|&j| !same(cs[j])).map_or(0, |j| j + 1);
                let end = (i..cs.len()).find(|&j| !same(cs[j])).unwrap_or(cs.len());
                (at(start), at(end))
            }
        }
    }

    /// A left press at character `col` of row `row` (rows past either end
    /// stand for the first or the last), the `count`th in a row: it
    /// selects from there by characters, words or lines; with `extend`,
    /// the selection reaches there instead.
    pub fn press(&mut self, row: i64, col: usize, extend: bool, count: u8) {
        let spot = self.spot(row, col);
        self.dragging = true;
        if extend && let Some(grain) = self.selection.map(|s| s.grain) {
            let span = self.span(spot, grain);
            if let Some(sel) = self.selection.as_mut() {
                sel.cursor = span;
            }
            return;
        }
        let grain = match count {
            1 => Grain::Char,
            2 => Grain::Word,
            _ => Grain::Line,
        };
        let span = self.span(spot, grain);
        self.selection = Some(Selection {
            anchor: span,
            cursor: span,
            grain,
        });
    }

    /// The pointer moved, holding the press, to character `col` of row
    /// `row`: the selection reaches there.
    pub fn drag_to(&mut self, row: i64, col: usize) {
        if !self.dragging {
            return;
        }
        let spot = self.spot(row, col);
        if let Some(grain) = self.selection.map(|s| s.grain) {
            let span = self.span(spot, grain);
            if let Some(sel) = self.selection.as_mut() {
                sel.cursor = span;
            }
        }
    }

    /// Whether the left button is held from a press on the text.
    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// The left button went up.
    pub fn release(&mut self) {
        self.dragging = false;
        self.edge_at = None;
    }

    /// Select all of it.
    pub fn select_all(&mut self) {
        let last = self.lines.len() - 1;
        let end = Spot {
            line: last,
            ch: self.lines[last].chars().count(),
        };
        self.selection = Some(Selection {
            anchor: (Spot::default(), Spot::default()),
            cursor: (end, end),
            grain: Grain::Char,
        });
    }

    /// Take a key, `typed` with Shift and the layout applied, with the
    /// list as `fit` lays it out. The search takes it first.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        now: Instant,
        fit: Fit,
    ) -> Option<Outcome> {
        if self.find_key(mods, sym, typed, fit) {
            return None;
        }
        let copy = |close| match (self.selected_text(), self.selected_lines()) {
            (Some(text), Some(lines)) => Outcome::Copy { text, lines, close },
            _ if close => Outcome::Close,
            _ => Outcome::NothingSelected,
        };
        let page = fit.page.max(1) as f64;
        let by = match sym {
            Keysym::Escape | Keysym::q if !mods.ctrl => return Some(Outcome::Close),
            Keysym::Return | Keysym::KP_Enter => return Some(copy(true)),
            Keysym::c if mods.ctrl => return Some(copy(false)),
            Keysym::y if !mods.ctrl => return Some(copy(false)),
            Keysym::a if mods.ctrl => {
                self.select_all();
                return None;
            }
            Keysym::Up | Keysym::k => -1.0,
            Keysym::Down | Keysym::j => 1.0,
            Keysym::Page_Up | Keysym::b => -page,
            Keysym::Page_Down | Keysym::space => page,
            Keysym::Home => f64::MIN,
            Keysym::g if !mods.shift => f64::MIN,
            Keysym::End => f64::MAX,
            Keysym::g => f64::MAX,
            _ => return None,
        };
        let b = fit.bounds(self.rows.len());
        let to = (self.scroll.pos().round() + by).clamp(0.0, b.max);
        self.scroll.glide_to(to, now, b);
        None
    }

    /// Whether the search took a key: `/` or Ctrl+F open it (or give it
    /// the keyboard back), Enter or F3 go to the next place (with Shift
    /// the one before; `n` and `N` while the list has the keyboard), Esc
    /// closes it, Tab gives the keyboard to the list. While it has the
    /// keyboard its field takes every other key but those that scroll
    /// the list and, when it has nothing for them, those with Ctrl.
    fn find_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym, fit: Fit) -> bool {
        let others = mods.ctrl || mods.alt || mods.logo;
        let plain = !(others || mods.shift);
        let only_shift = mods.shift && !others;
        let focused = self.find_focused();
        let opens = match sym {
            Keysym::f | Keysym::F => mods.ctrl && !(mods.alt || mods.logo),
            // Typed into the field, it is the field's.
            Keysym::slash => plain && !focused,
            _ => false,
        };
        if opens {
            self.open_find(fit);
            return true;
        }
        let Some(find) = self.find.as_mut() else {
            return false;
        };
        let next = match sym {
            Keysym::Return | Keysym::KP_Enter => focused,
            Keysym::n | Keysym::N => !focused,
            Keysym::F3 => true,
            _ => false,
        };
        if next && (plain || only_shift) {
            self.step_find(if mods.shift { -1 } else { 1 }, fit);
            return true;
        }
        match sym {
            Keysym::Escape if plain => {
                self.close_find();
                return true;
            }
            _ if !focused => return false,
            Keysym::Tab if plain => {
                find.focused = false;
                return true;
            }
            _ => {}
        }
        match find.field.key(mods, sym, typed) {
            Edit::Changed => {
                find.whole = false;
                self.find_hits();
                self.find_near(fit);
                true
            }
            Edit::Moved => true,
            Edit::Ignored => {
                let scrolls = matches!(
                    sym,
                    Keysym::Up
                        | Keysym::Down
                        | Keysym::KP_Up
                        | Keysym::KP_Down
                        | Keysym::Page_Up
                        | Keysym::Page_Down
                );
                !(scrolls || mods.ctrl)
            }
        }
    }

    /// Whether the search has the keyboard.
    pub fn find_focused(&self) -> bool {
        self.find.as_ref().is_some_and(|f| f.focused)
    }

    /// The search's field, while it has the keyboard.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        self.find
            .as_mut()
            .filter(|f| f.focused)
            .map(|f| &mut f.field)
    }

    /// The first and the last line in view, the list as `fit` lays it
    /// out.
    fn shown_lines(&self, fit: Fit) -> (usize, usize) {
        let first = self.scroll.first();
        let line = |row: usize| self.rows.get(row).map_or(0, |r| r.line);
        let last = (first + fit.page.max(1))
            .min(self.rows.len())
            .saturating_sub(1);
        (line(first), line(last))
    }

    /// Open the search, or give it the keyboard back with what it has
    /// typed selected: it looks around what is in view, the list as
    /// `fit` lays it out.
    pub fn open_find(&mut self, fit: Fit) {
        let shown = self.shown_lines(fit);
        let find = self.find.get_or_insert_with(Find::default);
        find.focused = true;
        find.shown = shown;
        find.field.select_all();
    }

    /// Search for the selection when it is on one line, else for the word
    /// at character `col` of row `row` (or the next after it on its line)
    /// as a whole word, as vim's `*` does: to the next place after it, or
    /// with `back` the one before (`#`), round past the ends. The list
    /// keeps the keyboard, for `n` and `N`. Whether there was something to
    /// search for.
    pub fn star(&mut self, at: Option<(i64, usize)>, back: bool, fit: Fit) -> bool {
        let selected = self
            .selection
            .map(|s| s.range())
            .filter(|(a, b)| a.line == b.line && a.ch < b.ch);
        let (query, whole, from) = match (selected, at) {
            (Some((a, b)), _) => (chars(&self.lines[a.line], a.ch, b.ch), false, a),
            (None, Some((row, col))) if row >= 0 && (row as usize) < self.rows.len() => {
                let spot = self.spot(row, col);
                let Some(word) = find::word_at(&self.lines[spot.line], spot.ch) else {
                    return false;
                };
                let text = chars(&self.lines[spot.line], word.start, word.end);
                let from = Spot {
                    line: spot.line,
                    ch: word.start,
                };
                (text, true, from)
            }
            _ => return false,
        };
        let shown = self.shown_lines(fit);
        let find = self.find.get_or_insert_with(Find::default);
        find.field.set(&query);
        find.focused = false;
        find.whole = whole;
        find.shown = shown;
        self.find_hits();
        let Some(find) = self.find.as_ref() else {
            return true;
        };
        let place = |(line, chars): &(usize, Range<usize>)| (*line, chars.start);
        let from = (from.line, from.ch);
        let at = if back {
            let before = find.hits.iter().rposition(|h| place(h) < from);
            before.or(find.hits.len().checked_sub(1))
        } else {
            let after = find.hits.iter().position(|h| place(h) > from);
            after.or((!find.hits.is_empty()).then_some(0))
        };
        if let Some(at) = at {
            self.go_to_found(at, fit);
        }
        true
    }

    /// Close the search: nothing is tinted any more.
    pub fn close_find(&mut self) {
        self.find = None;
    }

    /// Give the keyboard to the list, the places found still tinted.
    pub fn blur_find(&mut self) {
        if let Some(find) = self.find.as_mut() {
            find.focused = false;
        }
    }

    /// A click on a part of the search.
    pub fn find_click(&mut self, part: SearchPart, fit: Fit) {
        match part {
            SearchPart::Open => self.open_find(fit),
            SearchPart::Field => {
                if let Some(find) = self.find.as_mut() {
                    find.focused = true;
                }
            }
            SearchPart::Prev => self.step_find(-1, fit),
            SearchPart::Next => self.step_find(1, fit),
            SearchPart::Close => self.close_find(),
        }
    }

    /// Find what is typed, none gone to yet.
    fn find_hits(&mut self) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let (query, whole) = (find.field.line().text, find.whole);
        find.hits = self
            .lines
            .iter()
            .enumerate()
            .flat_map(|(n, line)| {
                find::find(line, &query, whole)
                    .into_iter()
                    .map(move |r| (n, r))
            })
            .collect();
        find.current = None;
    }

    /// Go to the place found nearest what was in view as the search
    /// opened: the first in it, else the last above it (output read up
    /// from the newest), else the first below.
    fn find_near(&mut self, fit: Fit) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let (top, bottom) = find.shown;
        let above = find.hits.partition_point(|(line, _)| *line < top);
        let at = match find.hits.get(above) {
            Some((line, _)) if *line <= bottom => Some(above),
            _ if above > 0 => Some(above - 1),
            _ => find.hits.first().map(|_| 0),
        };
        if let Some(at) = at {
            self.go_to_found(at, fit);
        }
    }

    /// Go `by` places on from the one gone to (back, for negative), round
    /// past the ends, or to the nearest with none gone to yet.
    fn step_find(&mut self, by: isize, fit: Fit) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let n = find.hits.len() as isize;
        match find.current {
            _ if n == 0 => {}
            Some(i) => self.go_to_found((i as isize + by).rem_euclid(n) as usize, fit),
            None => self.find_near(fit),
        }
    }

    /// Go to place `at` found: out of view, it is scrolled to a third down
    /// the list.
    fn go_to_found(&mut self, at: usize, fit: Fit) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.current = Some(at);
        let (line, ch) = (find.hits[at].0, find.hits[at].1.start);
        let row = self.row_of(Spot { line, ch });
        let (first, page) = (self.scroll.first(), fit.page.max(1));
        if row < first || row >= first + page {
            let max = fit.bounds(self.rows.len()).max;
            self.scroll
                .jump((row.saturating_sub(page / 3) as f64).min(max));
        }
    }

    /// The search as its bar shows it, while open.
    pub fn find_bar(&self) -> Option<SearchBar> {
        let find = self.find.as_ref()?;
        let query = find.field.line();
        let n = find.hits.len();
        Some(SearchBar {
            count: chrome::search_count(!query.text.is_empty(), n, find.current),
            query,
            placeholder: "Find in the scrollback".into(),
            found: n > 0,
            focused: find.focused,
            whole: find.whole,
        })
    }

    /// The places found on row `i`, by character of it, each with whether
    /// it is the one gone to.
    pub fn row_found(&self, i: usize) -> Vec<(Range<usize>, bool)> {
        let (Some(row), Some(find)) = (self.rows.get(i), &self.find) else {
            return Vec::new();
        };
        let from = find.hits.partition_point(|(line, _)| *line < row.line);
        find.hits[from..]
            .iter()
            .take_while(|(line, _)| *line == row.line)
            .enumerate()
            .filter_map(|(j, (_, r))| {
                let (a, b) = (r.start.max(row.start), r.end.min(row.end));
                (a < b).then(|| (a - row.start..b - row.start, find.current == Some(from + j)))
            })
            .collect()
    }

    /// Where the places found are on the scrollbar: shares of all the
    /// rows from the top, one per row, each with whether it has the one
    /// gone to.
    pub fn find_ticks(&self) -> Vec<(f64, bool)> {
        let Some(find) = &self.find else {
            return Vec::new();
        };
        let total = self.rows.len().max(1) as f64;
        let row = |(line, chars): &(usize, Range<usize>)| {
            self.row_of(Spot {
                line: *line,
                ch: chars.start,
            })
        };
        let current = find.current.and_then(|i| find.hits.get(i)).map(row);
        let mut ticks: Vec<(f64, bool)> = Vec::new();
        let mut last = None;
        for hit in &find.hits {
            let at = row(hit);
            if last != Some(at) {
                ticks.push((at as f64 / total, current == Some(at)));
                last = Some(at);
            }
        }
        ticks
    }

    /// The wheel or the touchpad moved the list.
    pub fn scroll(&mut self, motion: Motion, now: Instant, fit: Fit) {
        self.scroll.apply(motion, now, fit.bounds(self.rows.len()));
    }

    /// Stand the list at row `pos`, as its scrollbar's thumb held there
    /// puts it.
    pub fn scroll_to(&mut self, pos: f64, fit: Fit) {
        let max = fit.bounds(self.rows.len()).max;
        self.scroll.jump(pos.clamp(0.0, max));
    }

    /// Stop the list where it stands if flung; whether it was.
    pub fn stop_fling(&mut self) -> bool {
        self.scroll.stop_fling()
    }

    /// Move the list on to `now`: on its own, and by `edge` rows a second
    /// while a drag is past its top (negative) or bottom (positive), that
    /// many logical pixels past. Whether it still moves.
    pub fn tick(&mut self, now: Instant, fit: Fit, edge: f64) -> bool {
        let b = fit.bounds(self.rows.len());
        let mut moving = self.scroll.tick(now, b);
        if self.dragging && edge != 0.0 {
            let speed = (EDGE_SPEED + edge.abs() * EDGE_GAIN).min(EDGE_MAX) * edge.signum();
            if let Some(at) = self.edge_at {
                let by = speed * now.duration_since(at).as_secs_f64();
                self.scroll.jump((self.scroll.pos() + by).clamp(0.0, b.max));
            }
            self.edge_at = Some(now);
            moving = true;
        } else {
            self.edge_at = None;
        }
        moving
    }

    /// Whether the list moves, so what is under the pointer is left be.
    pub fn moving(&self, now: Instant) -> bool {
        self.scroll.moving(now)
    }
}

/// Characters `from` up to `to` of `line`.
fn chars(line: &str, from: usize, to: usize) -> String {
    line.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// Whether a double click takes `c` as part of a word.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || WORD_CHARS.contains(c)
}

/// `lines` as rows of at most `cols` characters; an empty line is a row.
fn wrap(lines: &[String], cols: usize) -> Vec<Row> {
    let mut rows = Vec::with_capacity(lines.len());
    for (line, text) in lines.iter().enumerate() {
        let mut row = Row {
            line,
            start: 0,
            end: 0,
            from: 0,
            to: 0,
        };
        for (n, (i, c)) in text.char_indices().enumerate() {
            if n - row.start == cols {
                rows.push(row);
                row = Row {
                    line,
                    start: n,
                    end: n,
                    from: i,
                    to: i,
                };
            }
            row.end = n + 1;
            row.to = i + c.len_utf8();
        }
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(cols: usize, page: usize) -> Fit {
        Fit {
            cols,
            page,
            row_h: 18.0,
            view_h: page as f64 * 18.0,
        }
    }

    fn viewer(text: &str, cols: usize, page: usize) -> Scrollback {
        let mut s = Scrollback::new(1, "w1:p1".into(), "zsh".into(), text, 0);
        s.fit(fit(cols, page));
        s
    }

    fn rows(s: &Scrollback) -> Vec<&str> {
        (0..s.row_count()).map(|i| s.row_text(i)).collect()
    }

    #[test]
    fn long_lines_wrap_at_the_width_and_wrap_again_for_another() {
        let mut s = viewer("abcdefgh\n\nxy\n\n\n", 3, 10);
        // The empty lines at the end go; one inside stays a row.
        assert_eq!(rows(&s), ["abc", "def", "gh", "", "xy"]);
        s.fit(fit(4, 10));
        assert_eq!(rows(&s), ["abcd", "efgh", "", "xy"]);
        let wide = viewer("äöüß€", 2, 10);
        assert_eq!(rows(&wide), ["äö", "üß", "€"]);
    }

    #[test]
    fn it_opens_as_far_up_as_the_pane_stood_and_keeps_its_top_row_through_a_rewrap() {
        let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let mut s = Scrollback::new(1, "p".into(), "zsh".into(), &text, 10);
        s.fit(fit(80, 20));
        // 100 rows, 20 shown: the bottom is at 80, ten up from it 70.
        assert_eq!(s.first(), 70);
        assert_eq!(s.row_text(s.first()), "line 71");
        s.fit(fit(4, 20));
        assert_eq!(s.row_text(s.first()), "line");
        assert_eq!(s.row_text(s.first() + 1), " 71");
    }

    #[test]
    fn a_drag_selects_across_rows_and_lines_and_copies_them_joined() {
        let mut s = viewer(
            "Traceback:\n  File \"a.py\", line 3\nValueError: bad",
            10,
            10,
        );
        assert_eq!(
            rows(&s),
            [
                "Traceback:",
                "  File \"a.",
                "py\", line ",
                "3",
                "ValueError",
                ": bad"
            ]
        );
        s.press(0, 0, false, 1);
        s.drag_to(5, 2);
        s.release();
        assert_eq!(
            s.selected_text().as_deref(),
            Some("Traceback:\n  File \"a.py\", line 3\nValueError: ")
        );
        assert_eq!(s.selected_lines(), Some(3));
        // Each row says what of it is lit; a line's last row goes on past
        // its end when the selection does.
        assert_eq!(s.row_selection(0), Some((0, 10, true)));
        assert_eq!(s.row_selection(1), Some((0, 10, false)));
        assert_eq!(s.row_selection(3), Some((0, 1, true)));
        assert_eq!(s.row_selection(5), Some((0, 2, false)));
        // Dragged back up, it goes from the press the other way.
        s.press(4, 3, false, 1);
        s.drag_to(1, 2);
        assert_eq!(
            s.selected_text().as_deref(),
            Some("File \"a.py\", line 3\nVal")
        );
    }

    #[test]
    fn past_a_rows_end_the_selection_takes_it_to_its_end() {
        let mut s = viewer("ab\ncd", 10, 10);
        s.press(0, 1, false, 1);
        s.drag_to(0, 40);
        assert_eq!(s.selected_text().as_deref(), Some("b"));
        // Past the last row, the end of it; before the first, its start.
        s.drag_to(9, 0);
        assert_eq!(s.selected_text().as_deref(), Some("b\ncd"));
        s.drag_to(-3, 5);
        assert_eq!(s.selected_text().as_deref(), Some("a"));
    }

    #[test]
    fn a_double_click_takes_a_word_with_its_path_and_a_triple_one_the_line() {
        let mut s = viewer("  File \"/srv/app/main.py\", line 12, in run", 200, 10);
        s.press(0, 12, false, 2);
        assert_eq!(s.selected_text().as_deref(), Some("/srv/app/main.py"));
        // Dragged on, by whole words.
        s.drag_to(0, 33);
        assert_eq!(
            s.selected_text().as_deref(),
            Some("/srv/app/main.py\", line 12")
        );
        s.press(0, 12, false, 3);
        assert_eq!(
            s.selected_text().as_deref(),
            Some("  File \"/srv/app/main.py\", line 12, in run")
        );
    }

    #[test]
    fn shift_click_reaches_the_selection_there() {
        let mut s = viewer("one two three", 80, 10);
        s.press(0, 4, false, 1);
        s.release();
        s.press(0, 7, true, 1);
        assert_eq!(s.selected_text().as_deref(), Some("two"));
        s.press(0, 0, true, 1);
        assert_eq!(s.selected_text().as_deref(), Some("one "));
    }

    #[test]
    fn keys_copy_select_all_and_close() {
        let mut s = viewer("a\nb", 80, 10);
        let none = ModifiersState::default();
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        let now = Instant::now();
        let f = fit(80, 10);
        assert_eq!(
            s.key(&ctrl, Keysym::c, Keysym::c, now, f),
            Some(Outcome::NothingSelected)
        );
        assert_eq!(
            s.key(&none, Keysym::Return, Keysym::Return, now, f),
            Some(Outcome::Close)
        );
        assert_eq!(s.key(&ctrl, Keysym::a, Keysym::a, now, f), None);
        let all = Outcome::Copy {
            text: "a\nb".into(),
            lines: 2,
            close: false,
        };
        assert_eq!(s.key(&ctrl, Keysym::c, Keysym::c, now, f), Some(all));
        assert!(matches!(
            s.key(&none, Keysym::Return, Keysym::Return, now, f),
            Some(Outcome::Copy { close: true, .. })
        ));
        assert_eq!(
            s.key(&none, Keysym::Escape, Keysym::Escape, now, f),
            Some(Outcome::Close)
        );
    }

    /// Type `text` into what has the keyboard.
    fn type_in(s: &mut Scrollback, text: &str, f: Fit) -> Option<Outcome> {
        let none = ModifiersState::default();
        let mut out = None;
        for c in text.chars() {
            let k = Keysym::from_char(c);
            out = s.key(&none, k, k, Instant::now(), f);
        }
        out
    }

    fn count(s: &Scrollback) -> String {
        s.find_bar().map(|b| b.count).unwrap_or_default()
    }

    #[test]
    fn slash_finds_what_is_typed_and_enter_goes_round_the_places() {
        let text: String = (0..60)
            .map(|n| match n % 10 {
                3 => format!("error {n}\n"),
                _ => format!("ok {n}\n"),
            })
            .collect();
        let mut s = viewer(&text, 80, 10);
        let f = fit(80, 10);
        let none = ModifiersState::default();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        let now = Instant::now();
        assert_eq!(s.first(), 50);
        type_in(&mut s, "/", f);
        assert!(s.find_focused());
        // Typed, it goes to the first place in view.
        assert_eq!(type_in(&mut s, "error", f), None);
        assert_eq!(count(&s), "6 of 6");
        assert_eq!(s.row_found(53), [(0..5, true)]);
        assert_eq!(s.row_found(43), [(0..5, false)]);
        assert!(s.row_found(52).is_empty());
        // Round past the end, to the top; and back.
        s.key(&none, Keysym::Return, Keysym::Return, now, f);
        assert_eq!(count(&s), "1 of 6");
        assert_eq!(s.first(), 0);
        s.key(&shift, Keysym::Return, Keysym::Return, now, f);
        assert_eq!(count(&s), "6 of 6");
        assert_eq!(s.first(), 50);
        // A capital matches only as typed.
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        s.key(&ctrl, Keysym::a, Keysym::a, now, f);
        type_in(&mut s, "Error", f);
        assert_eq!(count(&s), "No matches");
        s.key(&ctrl, Keysym::a, Keysym::a, now, f);
        type_in(&mut s, "error", f);
        assert_eq!(count(&s), "6 of 6");
        // With none in view (five rows of it), the nearest above.
        let f = fit(80, 5);
        s.key(&none, Keysym::Escape, Keysym::Escape, now, f);
        s.scroll_to(25.0, f);
        type_in(&mut s, "/", f);
        s.key(&ctrl, Keysym::a, Keysym::a, now, f);
        type_in(&mut s, "error", f);
        assert_eq!(count(&s), "3 of 6");
        assert_eq!(s.first(), 22);
        // Tab gives the keys to the list: `n` goes on, `N` back.
        s.key(&none, Keysym::Tab, Keysym::Tab, now, f);
        assert!(!s.find_focused());
        s.key(&none, Keysym::n, Keysym::n, now, f);
        assert_eq!(count(&s), "4 of 6");
        assert_eq!(s.first(), 32);
        s.key(&shift, Keysym::N, Keysym::N, now, f);
        assert_eq!(count(&s), "3 of 6");
        // Esc closes the search first, then the viewer.
        assert_eq!(s.key(&none, Keysym::Escape, Keysym::Escape, now, f), None);
        assert!(s.find_bar().is_none());
        assert!(s.row_found(53).is_empty());
        assert_eq!(
            s.key(&none, Keysym::Escape, Keysym::Escape, now, f),
            Some(Outcome::Close)
        );
    }

    #[test]
    fn star_finds_the_word_under_the_pointer_or_else_the_selection() {
        let mut s = viewer("let id = 1;\nlet idle = id;\nid(id_x)", 80, 10);
        let f = fit(80, 10);
        let none = ModifiersState::default();
        let now = Instant::now();
        // On `id`: the next place it is a word of its own, in the list.
        assert!(s.star(Some((0, 5)), false, f));
        let bar = s.find_bar().unwrap();
        assert!(bar.whole && !bar.focused);
        assert_eq!(bar.query.text, "id");
        assert_eq!(count(&s), "2 of 3");
        assert_eq!(s.row_found(1), [(11..13, true)]);
        assert_eq!(s.row_found(2), [(0..2, false)]);
        s.key(&none, Keysym::n, Keysym::n, now, f);
        assert_eq!(count(&s), "3 of 3");
        // `#` goes back from it: round to the last.
        assert!(s.star(Some((0, 4)), true, f));
        assert_eq!(count(&s), "3 of 3");
        // Past the line's last word there is none.
        assert!(!s.star(Some((0, 40)), false, f));
        assert!(!s.star(None, false, f));
        // A selection is searched for as it is, in words too.
        s.press(1, 4, false, 1);
        s.drag_to(1, 6);
        s.release();
        assert!(s.star(Some((0, 5)), false, f));
        let bar = s.find_bar().unwrap();
        assert!(!bar.whole);
        assert_eq!(count(&s), "3 of 5");
        // Typed into, it finds anywhere again.
        s.selection = None;
        assert!(s.star(Some((0, 5)), false, f));
        type_in(&mut s, "/", f);
        type_in(&mut s, "id", f);
        assert!(!s.find_bar().unwrap().whole);
        assert_eq!(s.find_bar().unwrap().count.split(" of ").nth(1), Some("5"));
    }

    #[test]
    fn a_place_wrapped_over_two_rows_is_tinted_on_both_and_ticked_once() {
        let mut s = viewer("abcdefgh\nxx\ndefdef", 4, 10);
        let f = fit(4, 10);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        let now = Instant::now();
        s.key(&ctrl, Keysym::f, Keysym::f, now, f);
        type_in(&mut s, "def", f);
        // Rows: "abcd", "efgh", "xx", "defd", "ef".
        assert_eq!(count(&s), "1 of 3");
        assert_eq!(s.row_found(0), [(3..4, true)]);
        assert_eq!(s.row_found(1), [(0..2, true)]);
        assert_eq!(s.row_found(3), [(0..3, false), (3..4, false)]);
        assert_eq!(s.row_found(4), [(0..2, false)]);
        assert_eq!(s.find_ticks(), [(0.0, true), (0.6, false)]);
        // Ctrl+C copies the selection with nothing selected in the field.
        s.select_all();
        assert!(matches!(
            s.key(&ctrl, Keysym::c, Keysym::c, now, f),
            Some(Outcome::Copy { lines: 3, .. })
        ));
    }

    #[test]
    fn a_drag_held_past_an_edge_scrolls_and_the_selection_goes_with_it() {
        let text: String = (0..100).map(|n| format!("{n}\n")).collect();
        let mut s = viewer(&text, 80, 10);
        let f = fit(80, 10);
        assert_eq!(s.first(), 90);
        // On the sixth row shown.
        s.press(95, 0, false, 1);
        let start = Instant::now();
        // Held 20 pixels over the top for half a second.
        s.tick(start, f, -20.0);
        assert!(s.tick(start + std::time::Duration::from_millis(500), f, -20.0));
        let first = s.first();
        assert!(first < 90 - 10, "scrolled up to {first}");
        s.drag_to(first as i64, 0);
        assert_eq!(s.selected_lines(), Some(95 - first + 1));
        // Let go, it stops.
        s.release();
        let at = s.first();
        s.tick(start + std::time::Duration::from_millis(900), f, -20.0);
        assert_eq!(s.first(), at);
    }
}
