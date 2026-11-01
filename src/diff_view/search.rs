//! The diff's search (`/` in the diff, Ctrl+Shift+F anywhere in the
//! panel): a field in the detail's title row finding what is typed in the
//! diff's lines, its files' paths and what its hunks are in, folded files
//! too, and the review comments shown. Every place found is tinted, the one gone to more; Enter goes to
//! the next (Shift+Enter, the one before), round past the ends, the
//! cursor on its row and a folded file unfolded for it. Ticks on the
//! diff's scrollbar say where they all are. Esc closes it. `*` (`#` back)
//! searches for the word under the pointer as a whole word, as vim's do.

use std::{collections::HashMap, ops::Range, path::PathBuf};

use smithay::input::keyboard::{Keysym, ModifiersState};

use super::{Ctx, DiffRow, DiffView, Keys, Outcome, copy_key, move_to};
use crate::{
    chrome::{self, SearchBar, SearchPart},
    find,
    text_field::{Edit, TextField},
};

/// The search, while open.
#[derive(Debug, Default)]
pub(crate) struct DiffSearch {
    field: TextField,
    /// It has the keyboard (else the diff has, the places still tinted).
    pub(super) focused: bool,
    /// It finds what is typed only as a whole word (`*`), until it is
    /// typed into.
    whole: bool,
    /// Every place found, in the diff's order.
    hits: Vec<Hit>,
    /// The place gone to, by index into `hits`.
    current: Option<usize>,
    /// The diff row the search started from: typing goes to the first
    /// place found from there.
    from: usize,
}

/// A place found, by who its file is, so that it is found again as the
/// diff is read again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hit {
    root: PathBuf,
    path: String,
    within: Within,
    /// By char index into what is searched.
    chars: Range<usize>,
    /// The diff row it shows on: none in a folded file.
    row: Option<usize>,
    /// The row its tick on the scrollbar stands for: its own, else its
    /// file's header.
    tick: usize,
}

/// What a place is found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Within {
    /// The file's path, as its header shows it (`path_text`).
    Path,
    /// What hunk `h` is in, on the line over it.
    Context(usize),
    /// Line `l` of hunk `h`.
    Line(usize, usize),
    /// Row `i` of review comment `c` (by index into `review.comments`),
    /// as it wraps.
    Comment(usize, usize),
}

/// A file header's path: after what a rename was called.
pub fn shown_path(file: &crate::diff::FileDiff) -> String {
    match &file.orig {
        Some(orig) => format!("{orig} → {}", file.path),
        None => file.path.clone(),
    }
}

/// What the search looks in on a file's header, as it shows it: its
/// name, then on a line of its own (which nothing typed matches across)
/// its folder and where it was renamed from.
fn path_text(file: &crate::diff::FileDiff) -> String {
    let shown = shown_path(file);
    let (name, rest) = chrome::name_first(&shown);
    format!("{name}\n{rest}")
}

impl Hit {
    fn same(&self, other: &Hit) -> bool {
        self.root == other.root && self.path == other.path && self.within == other.within
    }
}

impl DiffView {
    /// The search has the keyboard, the diff having it.
    pub(super) fn search_has_keys(&self, ctx: &Ctx) -> bool {
        ctx.keys == Keys::Diff && self.search_focused()
    }

    /// The search is open and takes the diff's keys.
    pub fn search_focused(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.focused)
    }

    /// The search's field, while it takes the diff's keys.
    pub fn search_field_mut(&mut self) -> Option<&mut TextField> {
        self.search
            .as_mut()
            .filter(|s| s.focused)
            .map(|s| &mut s.field)
    }

    /// Open the search on the diff, or give it the keyboard back with
    /// what it has typed selected.
    pub fn open_search(&mut self, ctx: &Ctx) {
        self.signals.focus = true;
        let page = ctx.page.max(1);
        let from = match &self.cursor {
            Some(c) => c.row,
            None => self.diff_window(page).1.start,
        };
        let search = self.search.get_or_insert_with(DiffSearch::default);
        search.focused = true;
        search.from = from;
        search.field.select_all();
    }

    /// Search the diff for the word at char `col` of diff row `row` (or
    /// the next after it on its line), a changed or unchanged line, as a
    /// whole word, as vim's `*` does: to the next place after it, or with
    /// `back` the one before (`#`), round past the ends. The diff keeps
    /// the keyboard, for `n` and `N`. Whether there was a word to search
    /// for.
    pub(super) fn star_search(&mut self, row: usize, col: usize, back: bool, ctx: &Ctx) -> bool {
        let Some(text) = self.line_text(row).map(str::to_owned) else {
            return false;
        };
        let Some(word) = find::word_at(&text, col) else {
            return false;
        };
        let query: String = text.chars().skip(word.start).take(word.len()).collect();
        self.signals.focus = true;
        let search = self.search.get_or_insert_with(DiffSearch::default);
        search.field.set(&query);
        search.focused = false;
        search.whole = true;
        search.from = row;
        self.find_hits(true);
        let Some(search) = &mut self.search else {
            return true;
        };
        let n = search.hits.len() as i64;
        let here = search
            .hits
            .iter()
            .position(|h| h.row == Some(row) && h.chars.start == word.start);
        search.current = match here {
            _ if n == 0 => None,
            Some(i) => Some((i as i64 + if back { -1 } else { 1 }).rem_euclid(n) as usize),
            None => Some(search.hits.iter().position(|h| h.tick >= row).unwrap_or(0)),
        };
        self.show_hit(ctx);
        true
    }

    /// Close the search: nothing is tinted any more.
    pub(super) fn close_search(&mut self) {
        self.search = None;
    }

    /// A key while the search has the keyboard, which takes them all:
    /// Enter (or F3) goes to the next place, with Shift the one before,
    /// Esc closes it, Down or Tab give the keyboard back to the diff;
    /// Ctrl+Shift+C copies the review still.
    pub(super) fn search_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        ctx: &Ctx,
    ) -> Option<Outcome> {
        let others = mods.ctrl || mods.alt || mods.logo;
        let plain = !(others || mods.shift);
        let only_shift = mods.shift && !others;
        if copy_key(mods, sym) {
            return self.copy(ctx);
        }
        let next = matches!(sym, Keysym::Return | Keysym::KP_Enter | Keysym::F3);
        match sym {
            Keysym::Escape if plain => self.close_search(),
            _ if next && plain => self.step_search(1, ctx),
            _ if next && only_shift => self.step_search(-1, ctx),
            Keysym::Down | Keysym::KP_Down | Keysym::Tab if plain => {
                if let Some(search) = &mut self.search {
                    search.focused = false;
                }
            }
            Keysym::f | Keysym::F if mods.ctrl && mods.shift && !(mods.alt || mods.logo) => {
                self.open_search(ctx);
            }
            _ => {
                let search = self.search.as_mut()?;
                if search.field.key(mods, sym, typed) == Edit::Changed {
                    search.whole = false;
                    self.find_hits(true);
                    self.search_from_start(ctx);
                }
            }
        }
        None
    }

    /// The keys for the search while open, with either focus: F3 goes on
    /// (Shift+F3 back), as `n` (`N` back) do but while the field below
    /// the tree has the keyboard, and Esc closes it. Whether `sym` was
    /// one.
    pub(super) fn search_open_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        ctx: &Ctx,
    ) -> bool {
        let others = mods.ctrl || mods.alt || mods.logo;
        if self.search.is_none() || others {
            return false;
        }
        match sym {
            Keysym::Escape if !mods.shift => self.close_search(),
            Keysym::F3 => self.step_search(if mods.shift { -1 } else { 1 }, ctx),
            Keysym::n | Keysym::N if ctx.keys != Keys::Field => {
                self.step_search(if mods.shift { -1 } else { 1 }, ctx);
            }
            _ => return false,
        }
        true
    }

    /// A click on the search's part `part`.
    pub(super) fn search_click(&mut self, part: SearchPart, ctx: &Ctx) {
        match part {
            SearchPart::Open | SearchPart::Field => match &mut self.search {
                Some(search) => {
                    search.focused = true;
                    self.signals.focus = true;
                }
                None => self.open_search(ctx),
            },
            SearchPart::Prev => self.step_search(-1, ctx),
            SearchPart::Next => self.step_search(1, ctx),
            SearchPart::Close => self.close_search(),
        }
    }

    /// Go to the place found `by` (1 or -1) from the one gone to, round
    /// past the ends; from none, to the first from where the diff is.
    fn step_search(&mut self, by: i64, ctx: &Ctx) {
        let page = ctx.page.max(1);
        let top = self.diff_window(page).1.start;
        let Some(search) = &mut self.search else {
            return;
        };
        let n = search.hits.len();
        if n == 0 {
            return;
        }
        search.current = Some(match search.current {
            Some(i) => (i as i64 + by).rem_euclid(n as i64) as usize,
            None if by > 0 => search.hits.iter().position(|h| h.tick >= top).unwrap_or(0),
            None => search
                .hits
                .iter()
                .rposition(|h| h.tick < top)
                .unwrap_or(n - 1),
        });
        self.show_hit(ctx);
    }

    /// What is typed changed: to the first place found from where the
    /// search started, round past the end.
    fn search_from_start(&mut self, ctx: &Ctx) {
        let Some(search) = &mut self.search else {
            return;
        };
        let from = search.from;
        search.current = if search.hits.is_empty() {
            None
        } else {
            Some(search.hits.iter().position(|h| h.tick >= from).unwrap_or(0))
        };
        if search.current.is_some() {
            self.show_hit(ctx);
        }
    }

    /// Show the place gone to: its file unfolded (unless the place is its
    /// path), the cursor on its row, the diff scrolled to show it with
    /// room around it, and its file selected in the tree.
    fn show_hit(&mut self, ctx: &Ctx) {
        let Some(hit) = self
            .search
            .as_ref()
            .and_then(|s| s.hits.get(s.current?))
            .cloned()
        else {
            return;
        };
        // In a folded file, or past the lines a long one shows: shown.
        let key = (hit.root.clone(), hit.path.clone());
        let in_file = !matches!(hit.within, Within::Path | Within::Comment(..));
        let hidden =
            in_file && (self.folded.remove(&key) | (hit.row.is_none() && self.whole.insert(key)));
        if hidden {
            self.build_rows(ctx.sections, self.diff_anchor());
        }
        let row = self
            .search
            .as_ref()
            .and_then(|s| s.hits.get(s.current?))
            .and_then(|h| h.row);
        let Some(row) = row else {
            return;
        };
        self.set_cursor(row, None);
        if let Some(cursor) = &mut self.cursor {
            cursor.col = hit.chars.start;
        }
        let page = ctx.page.max(1);
        // Out of view or near an edge (under the pinned header), it goes
        // a third down the page.
        let room = (page / 4).min(3) as f64;
        let near = |at: f64| at < room.max(1.0) || at > page as f64 - 1.0 - room;
        if self.shown_at(row, page).is_none_or(near) {
            let last = self.diff_last(page) as f64;
            let to = (row as f64 - (page / 3) as f64).clamp(0.0, last);
            move_to(&mut self.scroll, to);
        }
        self.select_diff_row(row, ctx);
    }

    /// Find what the search has typed again in the diffs read, as the
    /// rows have them now; the place gone to stays the one it was, or the
    /// nearest after it. `typed`: the query changed, so the place gone to
    /// is found again from the start.
    pub(super) fn find_hits(&mut self, typed: bool) {
        let Some(search) = &self.search else {
            return;
        };
        let (query, whole) = (search.field.line().text, search.whole);
        let was = search.current.and_then(|i| search.hits.get(i)).cloned();
        // The rows each place could show on, by who it is.
        let mut rows: HashMap<(usize, usize, Within), usize> = HashMap::new();
        let mut heads: HashMap<(usize, usize), usize> = HashMap::new();
        for (r, row) in self.rows.iter().enumerate() {
            match *row {
                DiffRow::File(k, f) => {
                    heads.insert((k, f), r);
                    rows.insert((k, f, Within::Path), r);
                }
                DiffRow::Hunk(k, f, h) => {
                    rows.insert((k, f, Within::Context(h)), r);
                }
                DiffRow::Line(k, f, h, l) => {
                    rows.insert((k, f, Within::Line(h, l)), r);
                }
                _ => {}
            }
        }
        // The comments' rows, each with its text, by the row they are
        // under (a line, a file's header), so that they are found in the
        // rows' order; those whose lines are gone on their own.
        let mut under: HashMap<usize, Vec<(usize, Within, &str)>> = HashMap::new();
        let mut gone = Vec::new();
        let mut parent = None;
        for (r, row) in self.rows.iter().enumerate() {
            let DiffRow::Comment { at, c, i } = *row else {
                if !matches!(row, DiffRow::Editor { .. } | DiffRow::EditorHint { .. }) {
                    parent = Some(r);
                }
                continue;
            };
            let (Some(comment), Some(wrap)) =
                (self.review.comments.get(c), self.comment_wraps.get(c))
            else {
                continue;
            };
            let text = wrap.get(i).map_or("", |range| &comment.text[range.clone()]);
            let found = (r, Within::Comment(c, i), text);
            match (at, parent) {
                (Some(_), Some(p)) => under.entry(p).or_default().push(found),
                _ => gone.push((comment, found)),
            }
        }
        let mut hits = Vec::new();
        for (k, root) in self.roots.iter().enumerate() {
            let Some((_, files)) = self.diffs.get(root) else {
                continue;
            };
            for (f, file) in files.iter().enumerate() {
                let Some(&head) = heads.get(&(k, f)) else {
                    continue;
                };
                let mut add = |within: Within, text: &str, at: Option<usize>| {
                    let row = at.or_else(|| rows.get(&(k, f, within)).copied());
                    hits.extend(
                        find::find(text, &query, whole)
                            .into_iter()
                            .map(|chars| Hit {
                                root: root.clone(),
                                path: file.path.clone(),
                                within,
                                chars,
                                row,
                                tick: row.unwrap_or(head),
                            }),
                    );
                };
                add(Within::Path, &path_text(file), None);
                for &(r, within, text) in under.get(&head).into_iter().flatten() {
                    add(within, text, Some(r));
                }
                for (h, hunk) in file.hunks.iter().enumerate() {
                    if !hunk.context.is_empty() {
                        add(Within::Context(h), &hunk.context, None);
                    }
                    for (l, line) in hunk.lines.iter().enumerate() {
                        add(Within::Line(h, l), &line.text, None);
                        let row = rows.get(&(k, f, Within::Line(h, l)));
                        for &(r, within, text) in
                            row.and_then(|r| under.get(r)).into_iter().flatten()
                        {
                            add(within, text, Some(r));
                        }
                    }
                }
            }
        }
        for (comment, (r, within, text)) in gone {
            hits.extend(
                find::find(text, &query, whole)
                    .into_iter()
                    .map(|chars| Hit {
                        root: comment.root.clone(),
                        path: comment.path.clone(),
                        within,
                        chars,
                        row: Some(r),
                        tick: r,
                    }),
            );
        }
        let current = match (&was, typed) {
            (Some(was), false) => hits
                .iter()
                .position(|h| h.same(was) && h.chars == was.chars)
                .or_else(|| hits.iter().position(|h| h.same(was)))
                .or_else(|| hits.iter().position(|h| h.tick >= was.tick))
                .or((!hits.is_empty()).then_some(0)),
            _ => None,
        };
        let search = self.search.as_mut().expect("open");
        search.hits = hits;
        search.current = current;
    }

    /// The places found on diff row `r`, by char index into what it
    /// shows, each with whether it is the one gone to.
    pub(crate) fn found_on(&self, r: usize) -> Vec<(Range<usize>, bool)> {
        let Some(search) = &self.search else {
            return Vec::new();
        };
        // The places are in the rows' order: those on `r` are together.
        let hits = &search.hits;
        let start = hits.partition_point(|h| h.tick < r);
        hits[start..]
            .iter()
            .enumerate()
            .take_while(|(_, h)| h.tick == r)
            .filter(|(_, h)| h.row == Some(r))
            .map(|(i, h)| (h.chars.clone(), search.current == Some(start + i)))
            .collect()
    }

    /// The search as the detail's title row shows it.
    pub(super) fn search_bar(&self, ctx: &Ctx) -> Option<SearchBar> {
        let search = self.search.as_ref()?;
        let query = search.field.line();
        let n = search.hits.len();
        let count = chrome::search_count(!query.text.is_empty(), n, search.current);
        Some(SearchBar {
            query,
            placeholder: "Find in the diff".into(),
            count,
            found: n > 0,
            focused: self.search_has_keys(ctx),
            whole: search.whole,
        })
    }

    /// Where the places found are on the diff's scrollbar, as shares of
    /// its rows from the top, each with whether it is the one gone to;
    /// one per row.
    pub(crate) fn search_ticks(&self) -> Vec<(f64, bool)> {
        let Some(search) = &self.search else {
            return Vec::new();
        };
        let total = self.rows.len().max(1) as f64;
        let current = search
            .current
            .and_then(|i| search.hits.get(i))
            .map(|h| h.tick);
        let mut ticks: Vec<(f64, bool)> = Vec::new();
        let mut last = None;
        for hit in &search.hits {
            if last != Some(hit.tick) {
                ticks.push((hit.tick as f64 / total, current == Some(hit.tick)));
                last = Some(hit.tick);
            }
        }
        ticks
    }
}
