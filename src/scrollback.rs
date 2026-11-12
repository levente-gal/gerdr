//! The scrollback viewer: all a terminal pane keeps of its output, read
//! from herdr as it opens and shown over the pane, to select more than a
//! screen of it and copy that. The terminal can't: it shows only herdr's
//! screen, which herdr draws again as it scrolls. Here the list scrolls on
//! its own, so a selection goes on while it does: dragged past an edge,
//! or with the wheel turned while the button is held.
//!
//! It is a snapshot: what the pane had as it opened, until Ctrl+R reads
//! it again. F1 lists every key it takes in place of the text.
//!
//! Lines are wrapped at the viewer's width (herdr joins those it wrapped),
//! and a place in them is a character of a line (see `Spot`). They are
//! drawn in the colours, bold and dim the terminal drew them in, as far
//! as herdr gives those: for the end of the scrollback only (see
//! `set_styles`).
//!
//! `/` or Ctrl+F searches it: every place found is tinted, the one gone
//! to more, Enter goes to the next (Shift+Enter, the one before), round
//! past the ends, and ticks on the scrollbar say where they all are. `*`
//! (`#` back) searches for the selection, or the word at the caret as a
//! whole word, as vim's do.
//!
//! The caret is on a character, as vim's is: the keys move it as vim's
//! do (the list following it), a click puts it there, and the places
//! found take it. Shift with the arrows, or `v` (`V` by lines), select
//! from it as it goes.

use std::{ops::Range, path::PathBuf, time::Instant};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    chrome::{self, ScrollbackHit, SearchBar, SearchPart},
    find,
    key_table::{self, Binding},
    scroll::{Bounds, Motion, Scroll},
    text_field::{Edit, TextField},
};

/// What a key of the viewer does (see `KEYS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    OpenFind,
    /// The next place found, or with `back` the one before.
    Find {
        back: bool,
    },
    /// The keyboard to the search, or back to the text.
    ToggleFind,
    ClearFind,
    CloseFind,
    ClearSelection,
    Close,
    /// Copy the selection; and close (Enter), or leave `v` (Y).
    Copy,
    CopyClose,
    Yank,
    Reload,
    SelectAll,
    Visual(Grain),
    Keys,
}

/// What the viewer's keys read of it now (see `Scrollback::key_ctx`).
pub(crate) struct KeyCtx {
    /// Whether the rows past the search's own are offered: not while its
    /// field, with the keyboard, is still to have the key.
    pub view: bool,
    /// Whether the search has the keyboard, while it is open.
    pub find: Option<bool>,
    pub find_empty: bool,
    pub selected: bool,
    pub visual: bool,
    pub pane: bool,
}

impl KeyCtx {
    /// Neither the search open nor anything selected.
    fn idle(&self) -> bool {
        self.find.is_none() && !self.selected && !self.visual
    }
}

/// The viewer's keys, the search's first: the handler, the footer and F1
/// read them. The caret's and the pages' keys stay `Scrollback::motion`'s.
pub(crate) const KEYS: &[Binding<KeyCtx, Key>] = &[
    Binding {
        chord: "Enter",
        does: |c| (c.find == Some(true)).then_some(Key::Find { back: false }),
        footer: true,
        f1: "Go to the next place found, or the one before",
    },
    Binding {
        chord: "Shift+Enter",
        does: |c| (c.find == Some(true)).then_some(Key::Find { back: true }),
        footer: true,
        f1: "Go to the next place found, or the one before",
    },
    Binding {
        chord: "N",
        does: |c| (c.find == Some(false)).then_some(Key::Find { back: false }),
        footer: true,
        f1: "Go to the next place or the one before, from the text",
    },
    Binding {
        chord: "Shift+N",
        does: |c| (c.find == Some(false)).then_some(Key::Find { back: true }),
        footer: true,
        f1: "Go to the next place or the one before, from the text",
    },
    Binding {
        chord: "F3",
        does: |c| c.find.map(|_| Key::Find { back: false }),
        footer: false,
        f1: "Go to the next place or the one before, from either",
    },
    Binding {
        chord: "Shift+F3",
        does: |c| c.find.map(|_| Key::Find { back: true }),
        footer: false,
        f1: "Go to the next place or the one before, from either",
    },
    // Hinted while selecting; it copies whatever is selected.
    Binding {
        chord: "Y",
        does: |c| (c.view && c.visual && c.find.is_none()).then_some(Key::Yank),
        footer: true,
        f1: "Copy the selection",
    },
    Binding {
        chord: "Y",
        does: |c| c.view.then_some(Key::Yank),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+C",
        does: |c| {
            let hinted = c.find == Some(false) || (c.find.is_none() && c.selected && !c.visual);
            (c.view && hinted).then_some(Key::Copy)
        },
        footer: true,
        f1: "Copy the selection",
    },
    Binding {
        chord: "Ctrl+C",
        does: |c| c.view.then_some(Key::Copy),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+Shift+C",
        does: |c| c.view.then_some(Key::Copy),
        footer: false,
        f1: "Copy the selection",
    },
    Binding {
        chord: "Enter",
        does: |c| {
            let hinted = c.find.is_none() && (c.visual || c.selected);
            (c.view && hinted).then_some(Key::CopyClose)
        },
        footer: true,
        f1: "Copy the selection and close",
    },
    Binding {
        chord: "Enter",
        does: |c| c.view.then_some(Key::CopyClose),
        footer: false,
        f1: "",
    },
    // Hinted beside the selection's keys; idle, beside Esc.
    Binding {
        chord: "Q",
        does: |c| (c.view && c.find.is_none() && c.selected && !c.visual).then_some(Key::Close),
        footer: true,
        f1: "",
    },
    Binding {
        chord: "V",
        does: |c| (c.view && c.idle()).then_some(Key::Visual(Grain::Char)),
        footer: true,
        f1: "Select from the caret, by characters or by lines",
    },
    Binding {
        chord: "V",
        does: |c| c.view.then_some(Key::Visual(Grain::Char)),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Shift+V",
        does: |c| c.view.then_some(Key::Visual(Grain::Line)),
        footer: false,
        f1: "Select from the caret, by characters or by lines",
    },
    Binding {
        chord: "Ctrl+A",
        does: |c| (c.view && c.idle() && !c.pane).then_some(Key::SelectAll),
        footer: true,
        f1: "Select all",
    },
    Binding {
        chord: "Ctrl+A",
        does: |c| c.view.then_some(Key::SelectAll),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "/",
        does: |c| c.idle().then_some(Key::OpenFind),
        footer: true,
        f1: "Find",
    },
    // Typed into the search's field, it is the field's.
    Binding {
        chord: "/",
        does: |c| (c.find != Some(true)).then_some(Key::OpenFind),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+F",
        does: |_| Some(Key::OpenFind),
        footer: false,
        f1: "Find",
    },
    Binding {
        chord: "Ctrl+R",
        does: |c| (c.view && c.idle() && c.pane).then_some(Key::Reload),
        footer: true,
        f1: "Read the output again",
    },
    Binding {
        chord: "Ctrl+R",
        does: |c| c.view.then_some(Key::Reload),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Tab",
        does: |c| c.find.map(|_| Key::ToggleFind),
        footer: true,
        f1: "Move between the search and the text",
    },
    Binding {
        chord: "Shift+Tab",
        does: |c| c.find.map(|_| Key::ToggleFind),
        footer: false,
        f1: "Move between the search and the text",
    },
    Binding {
        chord: "F1",
        does: |_| Some(Key::Keys),
        footer: true,
        f1: "List these keys",
    },
    // What is typed in the search, the search, the selection (a search
    // not focused leaves that first), then the viewer.
    Binding {
        chord: "Esc",
        does: |c| (c.find == Some(true) && !c.find_empty).then_some(Key::ClearFind),
        footer: true,
        f1: "Clear the selection or the search, then close",
    },
    Binding {
        chord: "Esc",
        does: |c| {
            let selection_first = c.find == Some(false) && c.selected;
            (c.find.is_some() && !selection_first).then_some(Key::CloseFind)
        },
        footer: true,
        f1: "",
    },
    Binding {
        chord: "Esc",
        does: |c| (c.view && (c.visual || c.selected)).then_some(Key::ClearSelection),
        footer: true,
        f1: "",
    },
    Binding {
        chord: "Esc",
        does: |c| c.view.then_some(Key::Close),
        footer: true,
        f1: "",
    },
    Binding {
        chord: "Q",
        does: |c| (c.view && c.idle()).then_some(Key::Close),
        footer: true,
        f1: "Close",
    },
    Binding {
        chord: "Q",
        does: |c| c.view.then_some(Key::Close),
        footer: false,
        f1: "",
    },
];

/// The keys while the viewer's keys are listed: they close the list.
pub(crate) const SHEET_KEYS: &[Binding<(), ()>] = &[
    Binding {
        chord: "Esc",
        does: |_| Some(()),
        footer: true,
        f1: "Close the keys",
    },
    Binding {
        chord: "F1",
        does: |_| Some(()),
        footer: false,
        f1: "Close the keys",
    },
    Binding {
        chord: "Q",
        does: |_| Some(()),
        footer: false,
        f1: "Close the keys",
    },
    Binding {
        chord: "Enter",
        does: |_| Some(()),
        footer: false,
        f1: "Close the keys",
    },
];

/// Rows a notch of the wheel scrolls.
const NOTCH_ROWS: f64 = 3.0;
/// How fast a drag past the list's top or bottom scrolls it, rows a
/// second: as fast at the edge, and faster for each logical pixel further.
const EDGE_SPEED: f64 = 12.0;
const EDGE_GAIN: f64 = 0.8;
/// The most a drag past an edge scrolls, rows a second.
const EDGE_MAX: f64 = 600.0;
/// How fast a drag held `edge` logical pixels past a list's top (negative)
/// or bottom scrolls it, rows a second (down positive).
pub(crate) fn edge_speed(edge: f64) -> f64 {
    (EDGE_SPEED + edge.abs() * EDGE_GAIN).min(EDGE_MAX) * edge.signum()
}

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
pub(crate) enum Grain {
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

/// How the terminal drew some of the text: in a colour of its own (else
/// the terminal's), bold, dim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Option<Ink>,
    pub bold: bool,
    pub dim: bool,
}

/// A colour as the terminal names it: one of its 256 (the first 16 its
/// theme's), or red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ink {
    Index(u8),
    Rgb([u8; 3]),
}

/// The runs of a line drawn other than plainly, by character of it.
type Styled = Vec<(Range<usize>, Style)>;

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
    /// Read the pane's output again.
    Reload,
}

/// What the viewer shows the text of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A terminal: the gerdr pane it is over, and its herdr pane.
    Pane { pane: u32, pane_id: String },
    /// The setup log of a feature, followed as it grows.
    SetupLog {
        project: String,
        slug: String,
        path: PathBuf,
        /// How long the file was when last read.
        read: u64,
    },
    /// A file read once, `what` it is: an earlier fix plan.
    File { what: String },
}

/// The viewer as it stands.
pub struct Scrollback {
    pub source: Source,
    /// What the pane's bar calls it, or the log's feature's title.
    pub label: String,
    lines: Vec<String>,
    /// How each line was drawn, by index into `lines`: none, drawn
    /// plainly, past those herdr gave colours for.
    styles: Vec<Styled>,
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
    /// The character the caret is on: the word there is underlined.
    caret: Spot,
    /// How far into its row the caret stood as it began to go up or
    /// down, to stand as far into the rows it goes by.
    want: Option<usize>,
    /// `v` (by characters) or `V` (by lines) is on: the selection goes
    /// with the caret.
    visual: Option<Grain>,
    /// The keys it takes are listed in place of the text (F1).
    keys: bool,
    /// The last press on the text: when, where, and how many came in a
    /// row, for double and triple clicks.
    pub clicks: Option<(Instant, Point<f64, Logical>, u8)>,
}

impl Scrollback {
    /// The viewer over gerdr pane `pane` (herdr's `pane_id`), showing
    /// `text`, the pane's view `from_bottom` rows up from its bottom.
    pub fn new(pane: u32, pane_id: String, label: String, text: &str, from_bottom: u64) -> Self {
        Self::of(Source::Pane { pane, pane_id }, label, text, from_bottom)
    }

    /// The viewer over `source`, showing `text`, standing `from_bottom`
    /// rows up from its bottom.
    pub fn of(source: Source, label: String, text: &str, from_bottom: u64) -> Self {
        Self {
            source,
            label,
            lines: split(text),
            styles: Vec::new(),
            rows: Vec::new(),
            cols: 0,
            scroll: Scroll::default(),
            from_bottom: Some(from_bottom as usize),
            selection: None,
            find: None,
            dragging: false,
            edge_at: None,
            hover: None,
            caret: Spot::default(),
            want: None,
            visual: None,
            keys: false,
            clicks: None,
        }
    }

    /// The pane it is over; none over a log.
    pub fn pane(&self) -> Option<u32> {
        match self.source {
            Source::Pane { pane, .. } => Some(pane),
            Source::SetupLog { .. } | Source::File { .. } => None,
        }
    }

    /// Whether the keys it takes are listed (F1).
    pub fn keys_open(&self) -> bool {
        self.keys
    }

    /// List the keys it takes, or stop.
    pub fn show_keys(&mut self, on: bool) {
        self.keys = on;
    }

    /// Show `text`, the pane's output read again, in place of what it
    /// showed, the list as `fit` lays it out: at the bottom it stays
    /// there, for what came since, else the line at the top stays there.
    /// The caret stays on its line as far as there is one, the selection
    /// goes, and the search finds again.
    pub fn reload(&mut self, text: &str, fit: Fit) {
        let max = fit.bounds(self.rows.len()).max;
        let bottom = self.scroll.pos() >= max - 0.5;
        let top = self.rows.get(self.scroll.first()).map_or(0, |r| r.line);
        self.lines = split(text);
        self.styles = Vec::new();
        self.rows = wrap(&self.lines, self.cols.max(1));
        let last = self.lines.len() - 1;
        if bottom {
            self.from_bottom = Some(0);
        } else {
            let row = self.row_of(Spot {
                line: top.min(last),
                ch: 0,
            });
            self.scroll.jump(row as f64);
        }
        let line = self.caret.line.min(last);
        self.caret = Spot {
            line,
            ch: self.caret.ch.min(self.last_ch(line)),
        };
        self.want = None;
        self.selection = None;
        self.visual = None;
        self.dragging = false;
        self.edge_at = None;
        self.find_hits();
        self.fit(fit);
    }

    /// Draw the lines as `ansi` says the terminal drew them: the end of
    /// the pane's output with the escape sequences that colour it (see
    /// `parse_ansi`). Its lines are matched to these from the bottom up,
    /// one going to a line with the same text; those it has none for, or
    /// another text for, are drawn plainly.
    pub fn set_styles(&mut self, ansi: &str) {
        let mut styled = parse_ansi(ansi);
        while styled.len() > 1 && styled.last().is_some_and(|(l, _)| l.trim().is_empty()) {
            styled.pop();
        }
        self.styles = vec![Vec::new(); self.lines.len()];
        let lines = &self.lines;
        let same = |i: usize, j: usize| lines[i].trim_end() == styled[j].0.trim_end();
        let (mut i, mut j) = (lines.len(), styled.len());
        let mut pairs = Vec::new();
        while i > 0 && j > 0 {
            if same(i - 1, j - 1) {
                pairs.push((i - 1, j - 1));
                (i, j) = (i - 1, j - 1);
            } else if j > 1 && same(i - 1, j - 2) {
                // A line here it has no match for: past it.
                j -= 1;
            } else if i > 1 && same(i - 2, j - 1) {
                i -= 1;
            } else {
                (i, j) = (i - 1, j - 1);
            }
        }
        for (i, j) in pairs {
            let len = self.lines[i].chars().count();
            self.styles[i] = std::mem::take(&mut styled[j].1)
                .into_iter()
                .filter_map(|(r, style)| {
                    let end = r.end.min(len);
                    (r.start < end).then_some((r.start..end, style))
                })
                .collect();
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
            // The caret starts on the last row in view.
            let last = (self.scroll.first() + fit.page.max(1)).min(self.rows.len());
            if let Some(r) = last.checked_sub(1).and_then(|i| self.rows.get(i)) {
                self.caret = Spot {
                    line: r.line,
                    ch: r.start,
                };
            }
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

    /// How row `i` is drawn: the runs of it drawn other than plainly, by
    /// character of the row.
    pub fn row_styles(&self, i: usize) -> Styled {
        let Some(row) = self.rows.get(i) else {
            return Vec::new();
        };
        let Some(spans) = self.styles.get(row.line) else {
            return Vec::new();
        };
        spans
            .iter()
            .filter_map(|(r, style)| {
                let (a, b) = (r.start.max(row.start), r.end.min(row.end));
                (a < b).then(|| (a - row.start..b - row.start, *style))
            })
            .collect()
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
        self.visual = None;
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

    /// The left button went up: after a drag the caret is on the
    /// selection's last character the pointer went to.
    pub fn release(&mut self) {
        if self.dragging
            && let Some(sel) = self.selection
            && sel.range().0 != sel.range().1
        {
            let (a, b) = sel.range();
            self.caret = if sel.cursor.0 >= sel.anchor.0 {
                Spot {
                    line: b.line,
                    ch: b.ch.saturating_sub(1),
                }
            } else {
                a
            };
            self.caret.ch = self.caret.ch.min(self.last_ch(self.caret.line));
            self.want = None;
        }
        self.dragging = false;
        self.edge_at = None;
    }

    /// Whether `v` (or `V`) is on.
    pub fn selecting(&self) -> bool {
        self.visual.is_some()
    }

    /// Put the caret on character `col` of row `row` (rows past either
    /// end stand for the first or the last), as a click does.
    pub fn caret_to(&mut self, row: i64, col: usize) {
        let spot = self.spot(row, col);
        let ch = spot.ch.min(self.last_ch(spot.line));
        self.caret = Spot { ch, ..spot };
        self.want = None;
    }

    /// The last character of line `line` the caret can be on.
    fn last_ch(&self, line: usize) -> usize {
        self.lines[line].chars().count().saturating_sub(1)
    }

    /// What the caret selects at `spot` going by `grain`: its character,
    /// or its line.
    fn caret_span(&self, spot: Spot, grain: Grain) -> (Spot, Spot) {
        match grain {
            Grain::Line => self.span(spot, Grain::Line),
            _ => {
                let len = self.lines[spot.line].chars().count();
                let end = Spot {
                    ch: (spot.ch + 1).min(len),
                    ..spot
                };
                (spot, end)
            }
        }
    }

    /// Move the caret to `to`: with `extend` or `v`, the selection goes
    /// with it from where it was, else there is none.
    fn move_caret(&mut self, to: Spot, extend: bool) {
        if extend || self.visual.is_some() {
            let grain = self.visual.unwrap_or(Grain::Char);
            let anchor = match self.selection {
                Some(sel) if sel.range().0 != sel.range().1 => sel.anchor,
                _ => self.caret_span(self.caret, grain),
            };
            let cursor = self.caret_span(to, grain);
            self.selection = Some(Selection {
                anchor,
                cursor,
                grain,
            });
        } else {
            self.selection = None;
        }
        self.caret = to;
    }

    /// Where the caret goes `by` rows down (up, for negative), as far
    /// into the row as it stood.
    fn caret_rows(&mut self, by: i64) -> Spot {
        let r = self.row_of(self.caret);
        let Some(row) = self.rows.get(r) else {
            return self.caret;
        };
        let want = *self.want.get_or_insert(self.caret.ch - row.start);
        let last = self.rows.len().saturating_sub(1) as i64;
        let to = self.rows[(r as i64).saturating_add(by).clamp(0, last) as usize];
        let ch = (to.start + want).min(to.end.saturating_sub(1).max(to.start));
        Spot {
            line: to.line,
            ch: ch.min(self.last_ch(to.line)),
        }
    }

    /// Where vim's `w` takes the caret: the next word's start, on a later
    /// line if none is on its own, or an empty line.
    fn word_forward(&self) -> Spot {
        let Spot { line, ch } = self.caret;
        if let Some(ch) = find::next_word(&self.lines[line], ch) {
            return Spot { line, ch };
        }
        for (n, text) in self.lines.iter().enumerate().skip(line + 1) {
            if text.is_empty() {
                return Spot { line: n, ch: 0 };
            }
            if !text.trim().is_empty() {
                let ch = find::first_char(text);
                return Spot { line: n, ch };
            }
        }
        self.caret
    }

    /// Where vim's `b` takes it: the start of the word before, on an
    /// earlier line if none is before it on its own, or an empty line.
    fn word_back(&self) -> Spot {
        let Spot { line, ch } = self.caret;
        if let Some(ch) = find::prev_word(&self.lines[line], ch) {
            return Spot { line, ch };
        }
        for n in (0..line).rev() {
            let text = &self.lines[n];
            if text.is_empty() {
                return Spot { line: n, ch: 0 };
            }
            if let Some(ch) = find::prev_word(text, usize::MAX) {
                return Spot { line: n, ch };
            }
        }
        Spot { line: 0, ch: 0 }
    }

    /// Where vim's `e` takes it: the end of the word, or the next one's,
    /// on a later line if none is on its own.
    fn word_forward_end(&self) -> Spot {
        let Spot { line, ch } = self.caret;
        if let Some(ch) = find::word_end(&self.lines[line], ch + 1) {
            return Spot { line, ch };
        }
        for (n, text) in self.lines.iter().enumerate().skip(line + 1) {
            if let Some(ch) = find::word_end(text, 0) {
                return Spot { line: n, ch };
            }
        }
        self.caret
    }

    /// Scroll the list the least to show the caret's row whole, from
    /// where it stands or else from `from` (where a page goes).
    fn follow(&mut self, now: Instant, fit: Fit, from: Option<f64>) {
        let row = self.row_of(self.caret) as f64;
        let page = fit.page.max(1) as f64;
        let b = fit.bounds(self.rows.len());
        let pos = from.unwrap_or(self.scroll.pos());
        let to = if row < pos {
            row
        } else if row + 1.0 > pos + page {
            row + 1.0 - page
        } else {
            pos
        };
        if from.is_some() || to != pos {
            self.scroll.glide_to(to, now, b);
        }
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

    /// What the keys read of the viewer now; `view`, whether the rows
    /// past the search's own are offered (see `KeyCtx`).
    pub(crate) fn key_ctx(&self, view: bool) -> KeyCtx {
        KeyCtx {
            view,
            find: self.find.as_ref().map(|f| f.focused),
            find_empty: self.find_empty(),
            selected: self.has_selection(),
            visual: self.visual.is_some(),
            pane: self.pane().is_some(),
        }
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
        // The keys listed take only those that close them.
        if self.keys {
            if key_table::press(SHEET_KEYS, &(), mods, sym).is_some() {
                self.keys = false;
            }
            return None;
        }
        // The search's field, with the keyboard, takes what its rows don't
        // before the viewer does.
        let focused = self.find_focused();
        let mut key = key_table::press(KEYS, &self.key_ctx(!focused), mods, sym);
        if key.is_none() && focused {
            if self.field_key(mods, sym, typed, fit) {
                return None;
            }
            key = key_table::press(KEYS, &self.key_ctx(true), mods, sym);
        }
        match key {
            Some(key) => self.do_key(key, fit),
            None => self.motion(mods, sym, typed, now, fit),
        }
    }

    /// Do what `key` does, with the list as `fit` lays it out.
    fn do_key(&mut self, key: Key, fit: Fit) -> Option<Outcome> {
        let copy = |close| match (self.selected_text(), self.selected_lines()) {
            (Some(text), Some(lines)) => Outcome::Copy { text, lines, close },
            _ if close => Outcome::Close,
            _ => Outcome::NothingSelected,
        };
        match key {
            Key::Keys => self.keys = true,
            Key::OpenFind => self.open_find(fit),
            Key::Find { back } => self.step_find(if back { -1 } else { 1 }, fit),
            Key::ToggleFind => {
                if let Some(find) = self.find.as_mut() {
                    find.focused = !find.focused;
                    if find.focused {
                        find.field.select_all();
                    }
                }
            }
            Key::ClearFind => {
                if let Some(find) = self.find.as_mut() {
                    find.field.set("");
                    find.whole = false;
                }
                self.find_hits();
            }
            Key::CloseFind => self.close_find(),
            // Out of `v` first, the selection gone; a selection made with
            // the pointer goes before the viewer does too.
            Key::ClearSelection => {
                self.visual = None;
                self.selection = None;
            }
            Key::Close => return Some(Outcome::Close),
            Key::CopyClose => return Some(copy(true)),
            Key::Reload => return Some(Outcome::Reload),
            Key::Copy => return Some(copy(false)),
            Key::Yank => {
                let copied = copy(false);
                self.visual = None;
                return Some(copied);
            }
            Key::SelectAll => {
                self.visual = None;
                self.select_all();
            }
            Key::Visual(grain) => {
                if self.visual == Some(grain) {
                    self.visual = None;
                    self.selection = None;
                } else {
                    self.visual = Some(grain);
                    let span = self.caret_span(self.caret, grain);
                    let anchor = match self.selection {
                        Some(sel) if sel.range().0 != sel.range().1 => sel.anchor,
                        _ => span,
                    };
                    self.selection = Some(Selection {
                        anchor,
                        cursor: span,
                        grain,
                    });
                }
            }
        }
        None
    }

    /// A key that moves the caret or pages the list, `typed` with Shift
    /// and the layout applied: plain motions, kept out of `KEYS` with
    /// their many forms (F1 lists them from `MOTION_KEYS`).
    fn motion(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        now: Instant,
        fit: Fit,
    ) -> Option<Outcome> {
        let page = fit.page.max(1) as f64;
        let ctrl = mods.ctrl && !(mods.alt || mods.logo);
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        // The pages scroll the list as far as the caret goes.
        let pages = match sym {
            _ if !(plain || ctrl) => None,
            Keysym::Page_Up | Keysym::KP_Page_Up if !mods.ctrl => Some(-page),
            Keysym::Page_Down | Keysym::KP_Page_Down if !mods.ctrl => Some(page),
            Keysym::space if plain && !mods.shift => Some(page),
            Keysym::u if ctrl => Some(-(page / 2.0).ceil()),
            Keysym::d if ctrl => Some((page / 2.0).ceil()),
            _ => None,
        };
        let Spot { line, ch } = self.caret;
        let arrows = matches!(
            sym,
            Keysym::Left
                | Keysym::Right
                | Keysym::Up
                | Keysym::Down
                | Keysym::KP_Left
                | Keysym::KP_Right
                | Keysym::KP_Up
                | Keysym::KP_Down
                | Keysym::Home
                | Keysym::End
                | Keysym::KP_Home
                | Keysym::KP_End
                | Keysym::Page_Up
                | Keysym::Page_Down
        );
        let extend = mods.shift && arrows;
        let last_line = self.lines.len() - 1;
        let at = |line, ch| Spot { line, ch };
        let mut keep_want = false;
        let max = fit.bounds(self.rows.len()).max;
        let paged = pages.map(|by| (self.scroll.pos().round() + by).clamp(0.0, max));
        let to = if let Some(by) = pages {
            keep_want = true;
            self.caret_rows(by as i64)
        } else if !plain {
            return None;
        } else {
            match (sym, typed) {
                (Keysym::Left | Keysym::KP_Left | Keysym::h, _) => at(line, ch.saturating_sub(1)),
                (Keysym::Right | Keysym::KP_Right | Keysym::l, _) => {
                    at(line, (ch + 1).min(self.last_ch(line)))
                }
                (Keysym::Up | Keysym::KP_Up | Keysym::k, _) => {
                    keep_want = true;
                    self.caret_rows(-1)
                }
                (Keysym::Down | Keysym::KP_Down | Keysym::j, _) => {
                    keep_want = true;
                    self.caret_rows(1)
                }
                (Keysym::w | Keysym::W, _) => self.word_forward(),
                (Keysym::b | Keysym::B, _) => self.word_back(),
                (Keysym::e | Keysym::E, _) => self.word_forward_end(),
                (_, Keysym::_0) => at(line, 0),
                (_, Keysym::asciicircum) => at(line, find::first_char(&self.lines[line])),
                (_, Keysym::dollar) => at(line, self.last_ch(line)),
                (Keysym::Home | Keysym::KP_Home, _) => at(0, 0),
                (Keysym::g, _) if !mods.shift => at(0, 0),
                (Keysym::End | Keysym::KP_End | Keysym::g | Keysym::G, _) => {
                    at(last_line, find::first_char(&self.lines[last_line]))
                }
                _ => return None,
            }
        };
        if !keep_want {
            self.want = None;
        }
        self.move_caret(to, extend);
        self.follow(now, fit, paged);
        None
    }

    /// Whether the search's field, with the keyboard, took a key its
    /// rows in `KEYS` don't: every key but those that scroll the list and,
    /// when it has nothing for them, those with Ctrl.
    fn field_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym, fit: Fit) -> bool {
        let Some(find) = self.find.as_mut() else {
            return false;
        };
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

    /// Whether something is selected, with the pointer, the keys or `v`.
    pub fn has_selection(&self) -> bool {
        self.selected_lines().is_some()
    }

    /// Whether what is typed in the search is empty (or it is closed).
    pub fn find_empty(&self) -> bool {
        self.find
            .as_ref()
            .is_none_or(|f| f.field.line().text.is_empty())
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
    /// at the caret (or the next after it on its line) as a whole word,
    /// as vim's `*` does: to the next place after it, or with `back` the
    /// one before (`#`), round past the ends. The list keeps the
    /// keyboard, for `n` and `N`. Whether there was something to search
    /// for.
    pub fn star(&mut self, back: bool, fit: Fit) -> bool {
        let selected = self
            .selection
            .map(|s| s.range())
            .filter(|(a, b)| a.line == b.line && a.ch < b.ch);
        let (query, whole, from) = match selected {
            Some((a, b)) => (chars(&self.lines[a.line], a.ch, b.ch), false, a),
            None => {
                let spot = self.caret;
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
        self.caret = Spot { line, ch };
        self.want = None;
        let row = self.row_of(Spot { line, ch });
        let (first, page) = (self.scroll.first(), fit.page.max(1));
        if row < first || row >= first + page {
            let max = fit.bounds(self.rows.len()).max;
            self.scroll
                .jump((row.saturating_sub(page / 3) as f64).min(max));
        }
    }

    /// Stand at the last line reading `text` (trailing blanks aside), the
    /// caret on it, a third down the view as `fit` lays it out; nowhere
    /// without one.
    pub fn reveal_last(&mut self, text: &str, fit: Fit) {
        let text = text.trim_end();
        let Some(line) = self.lines.iter().rposition(|l| l.trim_end() == text) else {
            return;
        };
        // The pane's own view no longer says where it stands.
        self.from_bottom = None;
        self.caret = Spot { line, ch: 0 };
        self.want = None;
        let row = self.row_of(self.caret);
        let max = fit.bounds(self.rows.len()).max;
        let page = fit.page.max(1);
        self.scroll
            .jump((row.saturating_sub(page / 3) as f64).min(max));
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

    /// Where on row `i` the caret is, by character of the row.
    pub fn row_caret(&self, i: usize) -> Option<usize> {
        let row = self.rows.get(i)?;
        (self.row_of(self.caret) == i).then(|| self.caret.ch - row.start)
    }

    /// The part on row `i` of the word at the caret, by character of the
    /// row: what `*` searches for, but while there is a selection (which
    /// it searches for then) or a drag.
    pub fn row_word(&self, i: usize) -> Option<Range<usize>> {
        let row = self.rows.get(i)?;
        let selected = self.selection.is_some_and(|s| {
            let (a, b) = s.range();
            a != b
        });
        if selected || self.dragging {
            return None;
        }
        let spot = self.caret;
        if spot.line != row.line {
            return None;
        }
        let word = find::word_at(&self.lines[spot.line], spot.ch)?;
        let (a, b) = (word.start.max(row.start), word.end.min(row.end));
        (a < b).then(|| a - row.start..b - row.start)
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
            let speed = edge_speed(edge);
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

/// The lines of `text`, with the escape sequences a terminal draws by,
/// as they show: each line's text and the runs of it drawn other than
/// plainly. Colours, bold and dim are kept, going on past a line's end
/// as a terminal's do; other sequences are dropped, but a move of the
/// cursor forward, which stands for blanks.
fn parse_ansi(text: &str) -> Vec<(String, Styled)> {
    let mut out = Vec::new();
    let (mut line, mut runs, mut n) = (String::new(), Styled::new(), 0);
    let mut style = Style::default();
    let put = |c: char, line: &mut String, runs: &mut Styled, n: &mut usize, style: Style| {
        line.push(c);
        if style != Style::default() {
            match runs.last_mut() {
                Some((r, s)) if *s == style && r.end == *n => r.end += 1,
                _ => runs.push((*n..*n + 1, style)),
            }
        }
        *n += 1;
    };
    let mut chars = text.chars().peekable();
    // A string sequence (OSC, DCS, ...) up to BEL or ESC \.
    let skip_string = |chars: &mut std::iter::Peekable<std::str::Chars>| {
        while let Some(c) = chars.next() {
            if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                break;
            }
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                out.push((std::mem::take(&mut line), std::mem::take(&mut runs)));
                n = 0;
            }
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut end = None;
                    for d in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&d) {
                            end = Some(d);
                            break;
                        }
                        params.push(d);
                    }
                    match end {
                        Some('m') => sgr(&params, &mut style),
                        Some('C') => {
                            let by = params.parse::<usize>().unwrap_or(1).clamp(1, 1000);
                            for _ in 0..by {
                                put(' ', &mut line, &mut runs, &mut n, style);
                            }
                        }
                        _ => {}
                    }
                }
                Some(']' | 'P' | 'X' | '^' | '_') => skip_string(&mut chars),
                // A character set chosen: one more.
                Some('(' | ')' | '*' | '+' | '#' | '%') => {
                    chars.next();
                }
                _ => {}
            },
            c if c.is_control() && c != '\t' => {}
            c => put(c, &mut line, &mut runs, &mut n, style),
        }
    }
    out.push((line, runs));
    out
}

/// Apply SGR parameters `params` (as in ESC [ `params` m) to `style`:
/// colours of the text, bold and dim; the ground's colour and the rest
/// passed over.
fn sgr(params: &str, style: &mut Style) {
    let params: Vec<&str> = params.split(';').collect();
    let num = |i: usize| params.get(i).and_then(|p| p.parse::<u32>().ok());
    let byte = |v: Option<u32>| v.unwrap_or(0).min(255) as u8;
    let mut i = 0;
    while i < params.len() {
        // As 38:5:n or 38:2:r:g:b (a colour space first, perhaps).
        if params[i].contains(':') {
            let sub: Vec<Option<u32>> = params[i].split(':').map(|s| s.parse().ok()).collect();
            if sub[0] == Some(38) {
                style.fg = match (sub.get(1).copied().flatten(), sub.len()) {
                    (Some(5), _) => Some(Ink::Index(byte(sub.get(2).copied().flatten()))),
                    (Some(2), 6..) => Some(Ink::Rgb([byte(sub[3]), byte(sub[4]), byte(sub[5])])),
                    (Some(2), 5) => Some(Ink::Rgb([byte(sub[2]), byte(sub[3]), byte(sub[4])])),
                    _ => style.fg,
                };
            }
            i += 1;
            continue;
        }
        let code = if params[i].is_empty() {
            Some(0)
        } else {
            num(i)
        };
        match code {
            Some(0) => *style = Style::default(),
            Some(1) => style.bold = true,
            Some(2) => style.dim = true,
            Some(22) => (style.bold, style.dim) = (false, false),
            Some(c @ 30..=37) => style.fg = Some(Ink::Index((c - 30) as u8)),
            Some(c @ 90..=97) => style.fg = Some(Ink::Index((c - 90 + 8) as u8)),
            Some(39) => style.fg = None,
            Some(c @ (38 | 48)) => {
                let (ink, by) = match num(i + 1) {
                    Some(5) => (Some(Ink::Index(byte(num(i + 2)))), 2),
                    Some(2) => {
                        let rgb = [byte(num(i + 2)), byte(num(i + 3)), byte(num(i + 4))];
                        (Some(Ink::Rgb(rgb)), 4)
                    }
                    _ => (None, 0),
                };
                if c == 38 && ink.is_some() {
                    style.fg = ink;
                }
                i += by;
            }
            _ => {}
        }
        i += 1;
    }
}

/// The lines of `text` as herdr gives it, without the blank ones at its
/// end (the rows under the prompt) but one.
fn split(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    while lines.len() > 1 && lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
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
    fn the_key_tables_agree_with_themselves() {
        let mut ctxs = Vec::new();
        for find in [None, Some(false), Some(true)] {
            for (selected, visual) in [(false, false), (true, false), (true, true)] {
                for (find_empty, pane) in [(false, false), (true, true)] {
                    for view in [false, true] {
                        ctxs.push(KeyCtx {
                            view,
                            find,
                            find_empty,
                            selected,
                            visual,
                            pane,
                        });
                    }
                }
            }
        }
        key_table::check(KEYS, &ctxs);
        key_table::check(SHEET_KEYS, &[()]);
        // A bare q closes, as the footer's "Q" says (K8).
        let idle = KeyCtx {
            view: true,
            find: None,
            find_empty: true,
            selected: false,
            visual: false,
            pane: true,
        };
        let none = ModifiersState::default();
        assert_eq!(
            key_table::press(KEYS, &idle, &none, Keysym::q),
            Some(Key::Close)
        );
        let shift = ModifiersState {
            shift: true,
            ..none
        };
        let v = key_table::press(KEYS, &idle, &shift, Keysym::v);
        assert_eq!(v, Some(Key::Visual(Grain::Line)));
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
        // Esc lets go of the selection first, then closes; so does `q`
        // at once.
        assert_eq!(s.key(&none, Keysym::Escape, Keysym::Escape, now, f), None);
        assert!(!s.has_selection());
        assert_eq!(
            s.key(&none, Keysym::Escape, Keysym::Escape, now, f),
            Some(Outcome::Close)
        );
        s.key(&ctrl, Keysym::a, Keysym::a, now, f);
        assert_eq!(
            s.key(&none, Keysym::q, Keysym::q, now, f),
            Some(Outcome::Close)
        );
    }

    fn fg(ink: Ink) -> Style {
        Style {
            fg: Some(ink),
            ..Default::default()
        }
    }

    #[test]
    fn escape_sequences_give_colours_bold_and_dim_and_the_rest_go() {
        let red = fg(Ink::Index(1));
        let bold = Style {
            bold: true,
            ..Default::default()
        };
        let lines = parse_ansi(
            "\x1b[31merror\x1b[0m: \x1b[1mbad\x1b[22m\n\
             \x1b]8;;file:///a\x1b\\a.rs\x1b]8;;\x07\x1b[2C\x1b[38;5;208mx\x1b[38;2;1;2;3my\r\n\
             \x1b[38:2::4:5:6mz\x1b[39;48;5;3mw\x1b[1;2;92mv\x1b[mu\x1b(B",
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            ("error: bad".into(), vec![(0..5, red), (7..10, bold)])
        );
        // A link's sequences go; a move forward stands for blanks.
        assert_eq!(lines[1].0, "a.rs  xy");
        assert_eq!(
            lines[1].1,
            [(6..7, fg(Ink::Index(208))), (7..8, fg(Ink::Rgb([1, 2, 3])))]
        );
        // Colons, a colour space among them; the ground's colour passed
        // over; a style going on past the line's end.
        let bright = Style {
            fg: Some(Ink::Index(10)),
            bold: true,
            dim: true,
        };
        assert_eq!(lines[2].0, "zwvu");
        assert_eq!(
            lines[2].1,
            [(0..1, fg(Ink::Rgb([4, 5, 6]))), (2..3, bright)]
        );
        let on = parse_ansi("\x1b[33ma\nb\x1b[0m");
        assert_eq!(on[1].1, [(0..1, fg(Ink::Index(3)))]);
    }

    #[test]
    fn styles_go_to_the_lines_with_their_text_from_the_bottom_up() {
        let mut s = viewer("old\nkept\nsame\nchanged\nlong line here\nlast", 5, 10);
        let red = fg(Ink::Index(1));
        // Only the end; a line it lacks; one that differs.
        s.set_styles(
            "\x1b[31msame\x1b[0m\nchanged \x1b[31mnot\x1b[0m\n\
             \x1b[31mlong line\x1b[0m here\n\x1b[31mlast\x1b[0m\n\n",
        );
        let styled: Vec<bool> = s.styles.iter().map(|l| !l.is_empty()).collect();
        assert_eq!(styled, [false, false, true, false, true, true]);
        // Split by rows as the lines wrap ("long " / "line " / "here").
        let first = s.rows.iter().position(|r| r.line == 4).unwrap();
        assert_eq!(s.row_styles(first), [(0..5, red)]);
        assert_eq!(s.row_styles(first + 1), [(0..4, red)]);
        assert!(s.row_styles(first + 2).is_empty());
        // Read again, plain until styled again.
        s.reload("last", fit(5, 10));
        assert!(s.row_styles(0).is_empty());
        s.set_styles("\x1b[31mlast");
        assert_eq!(s.row_styles(0), [(0..4, red)]);
    }

    #[test]
    fn f1_lists_the_keys_until_esc_f1_q_or_enter_and_takes_no_other() {
        let mut s = viewer("one two\nthree", 80, 10);
        let none = ModifiersState::default();
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        let now = Instant::now();
        let f = fit(80, 10);
        let caret = |s: &Scrollback| (s.caret.line, s.caret.ch);
        let before = caret(&s);
        for close in [Keysym::Escape, Keysym::F1, Keysym::q, Keysym::Return] {
            assert_eq!(s.key(&none, Keysym::F1, Keysym::F1, now, f), None);
            assert!(s.keys_open());
            assert_eq!(s.key(&none, Keysym::k, Keysym::k, now, f), None);
            assert_eq!(s.key(&ctrl, Keysym::a, Keysym::a, now, f), None);
            type_in(&mut s, "/", f);
            assert!(s.keys_open() && caret(&s) == before);
            assert!(!s.has_selection() && s.find_bar().is_none());
            // Closing them leaves the viewer open.
            assert_eq!(s.key(&none, close, close, now, f), None);
            assert!(!s.keys_open());
        }
        // From the search's field too.
        type_in(&mut s, "/", f);
        s.key(&none, Keysym::F1, Keysym::F1, now, f);
        assert!(s.keys_open() && s.find_empty());
    }

    #[test]
    fn ctrl_r_reads_again_at_the_bottom_or_where_it_stood() {
        let text = |n: usize| (0..n).map(|i| format!("line {i}\n")).collect::<String>();
        let f = fit(80, 10);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        let now = Instant::now();
        let mut s = viewer(&text(30), 80, 10);
        assert_eq!(
            s.key(&ctrl, Keysym::r, Keysym::r, now, f),
            Some(Outcome::Reload)
        );
        // At the bottom, it shows what came since.
        assert_eq!(s.first(), 20);
        s.select_all();
        s.reload(&text(40), f);
        assert_eq!((s.line_count(), s.first()), (40, 30));
        assert!(!s.has_selection());
        // Up the list, the line at the top stays there.
        s.scroll_to(5.0, f);
        s.reload(&text(50), f);
        assert_eq!((s.line_count(), s.first()), (50, 5));
        // The search finds in what is read.
        type_in(&mut s, "/", f);
        type_in(&mut s, "line 5", f);
        assert_eq!(count(&s), "1 of 1");
        s.reload(&text(60), f);
        assert_eq!(count(&s), "11 found");
        // Fewer lines than it stood at: it stands at the bottom.
        s.reload(&text(3), f);
        assert_eq!((s.line_count(), s.first()), (3, 0));
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
        // Esc clears what is typed, then closes the search.
        s.key(&none, Keysym::Escape, Keysym::Escape, now, f);
        assert!(s.find_focused() && s.find_empty());
        assert_eq!(count(&s), "");
        assert!(s.row_found(53).is_empty());
        s.key(&none, Keysym::Escape, Keysym::Escape, now, f);
        assert!(s.find_bar().is_none());
        // With none in view (five rows of it), the nearest above.
        let f = fit(80, 5);
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
        // Shift+Tab (or Tab) gives them back to the search, and again
        // to the list.
        let left_tab = Keysym::ISO_Left_Tab;
        s.key(&shift, left_tab, left_tab, now, f);
        assert!(s.find_focused());
        s.key(&none, Keysym::Tab, Keysym::Tab, now, f);
        assert!(!s.find_focused());
        // A selection goes before the search does.
        s.key(&shift, Keysym::Right, Keysym::Right, now, f);
        assert!(s.has_selection());
        assert_eq!(s.key(&none, Keysym::Escape, Keysym::Escape, now, f), None);
        assert!(!s.has_selection() && s.find_bar().is_some());
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
    fn star_finds_the_word_at_the_caret_or_else_the_selection() {
        let mut s = viewer("let id = 1;\nlet idle = id;\nid(id_x)", 80, 10);
        let f = fit(80, 10);
        let none = ModifiersState::default();
        let now = Instant::now();
        // On `id`: the next place it is a word of its own, in the list.
        s.caret_to(0, 5);
        assert!(s.star(false, f));
        let bar = s.find_bar().unwrap();
        assert!(bar.whole && !bar.focused);
        assert_eq!(bar.query.text, "id");
        assert_eq!(count(&s), "2 of 3");
        assert_eq!(s.row_found(1), [(11..13, true)]);
        assert_eq!(s.row_found(2), [(0..2, false)]);
        assert_eq!(s.row_caret(1), Some(11));
        // `n` takes the caret on.
        s.key(&none, Keysym::n, Keysym::n, now, f);
        assert_eq!(count(&s), "3 of 3");
        assert_eq!((s.row_caret(1), s.row_caret(2)), (None, Some(0)));
        // The word at the caret is underlined, on its row only.
        s.caret_to(1, 6);
        assert_eq!(s.row_word(1), Some(4..8));
        assert_eq!(s.row_word(0), None);
        // `#` goes back from it: round to the last.
        s.caret_to(0, 4);
        assert!(s.star(true, f));
        assert_eq!(count(&s), "3 of 3");
        // Past the line's last word there is none.
        s.caret_to(0, 40);
        assert!(!s.star(false, f));
        // A selection is searched for as it is, in words too.
        s.press(1, 4, false, 1);
        s.drag_to(1, 6);
        s.release();
        // Then that is searched for, not the word.
        assert_eq!(s.row_word(1), None);
        assert!(s.star(false, f));
        let bar = s.find_bar().unwrap();
        assert!(!bar.whole);
        assert_eq!(count(&s), "3 of 5");
        // Typed into, it finds anywhere again.
        s.selection = None;
        s.caret_to(0, 5);
        assert!(s.star(false, f));
        type_in(&mut s, "/", f);
        type_in(&mut s, "id", f);
        assert!(!s.find_bar().unwrap().whole);
        assert_eq!(s.find_bar().unwrap().count.split(" of ").nth(1), Some("5"));
    }

    /// Where the caret is: its row and the character of it.
    fn caret(s: &Scrollback) -> (usize, usize) {
        (0..s.row_count())
            .find_map(|i| Some((i, s.row_caret(i)?)))
            .unwrap()
    }

    #[test]
    fn the_caret_goes_as_vims_does_and_the_list_follows_it() {
        let text = "one two\n\n  three-four five\nlast";
        let mut s = viewer(text, 80, 2);
        let f = fit(80, 2);
        let none = ModifiersState::default();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        let mut now = Instant::now();
        let mut key = |s: &mut Scrollback, mods: &ModifiersState, k: Keysym| {
            now += std::time::Duration::from_secs(5);
            let out = s.key(mods, k, k, now, f);
            s.tick(now + std::time::Duration::from_secs(2), f, 0.0);
            out
        };
        // It starts on the last row in view.
        assert_eq!((caret(&s), s.first()), ((3, 0), 2));
        key(&mut s, &none, Keysym::g);
        assert_eq!((caret(&s), s.first()), ((0, 0), 0));
        // By words, stopping on an empty line, and runs of other chars.
        let mut went = Vec::new();
        for k in [Keysym::w, Keysym::w, Keysym::w, Keysym::w, Keysym::w] {
            key(&mut s, &none, k);
            went.push(caret(&s));
        }
        assert_eq!(went, [(0, 4), (1, 0), (2, 2), (2, 7), (2, 8)]);
        // The list follows it down.
        assert_eq!(s.first(), 1);
        key(&mut s, &none, Keysym::e);
        assert_eq!(caret(&s), (2, 11));
        let mut went = Vec::new();
        for _ in 0..5 {
            key(&mut s, &none, Keysym::b);
            went.push(caret(&s));
        }
        assert_eq!(went, [(2, 8), (2, 7), (2, 2), (1, 0), (0, 4)]);
        assert_eq!(s.first(), 0);
        // Along a line, kept on it.
        key(&mut s, &none, Keysym::dollar);
        assert_eq!(caret(&s), (0, 6));
        key(&mut s, &none, Keysym::l);
        assert_eq!(caret(&s), (0, 6));
        key(&mut s, &none, Keysym::h);
        key(&mut s, &none, Keysym::h);
        assert_eq!(caret(&s), (0, 4));
        // Down past an empty line, as far into the next as it stood.
        key(&mut s, &none, Keysym::j);
        assert_eq!(caret(&s), (1, 0));
        key(&mut s, &none, Keysym::j);
        assert_eq!(caret(&s), (2, 4));
        key(&mut s, &none, Keysym::asciicircum);
        assert_eq!(caret(&s), (2, 2));
        key(&mut s, &none, Keysym::_0);
        assert_eq!(caret(&s), (2, 0));
        key(&mut s, &shift, Keysym::G);
        assert_eq!((caret(&s), s.first()), ((3, 0), 2));
        // `v` selects as it goes, `y` copies it.
        key(&mut s, &none, Keysym::v);
        key(&mut s, &none, Keysym::l);
        key(&mut s, &none, Keysym::l);
        let copied = Outcome::Copy {
            text: "las".into(),
            lines: 1,
            close: false,
        };
        assert_eq!(key(&mut s, &none, Keysym::y), Some(copied));
        // Moved without it, there is none.
        key(&mut s, &none, Keysym::h);
        assert_eq!(s.selected_lines(), None);
        // Shift with the arrows selects, back up too; `V` by lines.
        key(&mut s, &shift, Keysym::Up);
        assert_eq!(s.selected_text().as_deref(), Some(" three-four five\nla"));
        key(&mut s, &shift, Keysym::V);
        key(&mut s, &none, Keysym::k);
        assert_eq!(s.selected_lines(), Some(3));
        // Esc leaves `v`, the selection gone; then it closes.
        assert_eq!(key(&mut s, &none, Keysym::Escape), None);
        assert_eq!(s.selected_lines(), None);
        assert_eq!(key(&mut s, &none, Keysym::Escape), Some(Outcome::Close));
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
