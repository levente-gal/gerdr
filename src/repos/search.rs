//! The diff's search (`/` in the diff, Ctrl+Shift+F anywhere in the
//! panel): a field in the detail's title row finding what is typed in the
//! diff's lines, its files' paths and what its hunks are in, folded files
//! too. Every place found is tinted, the one gone to more; Enter goes to
//! the next (Shift+Enter, the one before), round past the ends, the
//! cursor on its row and a folded file unfolded for it. Ticks on the
//! diff's scrollbar say where they all are. Esc closes it.

use std::{collections::HashMap, ops::Range, path::PathBuf};

use smithay::input::keyboard::{Keysym, ModifiersState};

use super::{Ctx, DiffRow, Focus, Outcome, Panel, action_key, move_to};
use crate::{
    chrome::{DetailTab, ReposButton, SearchBar, SearchPart},
    find,
    text_field::{Edit, TextField},
};

/// The search, while open.
#[derive(Debug, Default)]
pub(super) struct DiffSearch {
    field: TextField,
    /// It has the keyboard (else the diff has, the places still tinted).
    pub(super) focused: bool,
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
    /// The file's path, as its header shows it.
    Path,
    /// What hunk `h` is in, on the line over it.
    Context(usize),
    /// Line `l` of hunk `h`.
    Line(usize, usize),
}

/// A file header's text: its path, after what a rename was called.
pub(super) fn shown_path(file: &crate::diff::FileDiff) -> String {
    match &file.orig {
        Some(orig) => format!("{orig} → {}", file.path),
        None => file.path.clone(),
    }
}

impl Hit {
    fn same(&self, other: &Hit) -> bool {
        self.root == other.root && self.path == other.path && self.within == other.within
    }
}

impl Panel {
    /// The search has the keyboard.
    pub(super) fn search_has_keys(&self) -> bool {
        self.focus == Focus::Detail
            && self.tab() == DetailTab::Diff
            && self.search.as_ref().is_some_and(|s| s.focused)
    }

    /// The search's field, while it has the keyboard.
    pub(super) fn search_field_mut(&mut self) -> Option<&mut TextField> {
        if !self.search_has_keys() {
            return None;
        }
        self.search.as_mut().map(|s| &mut s.field)
    }

    /// Open the search on the diff, or give it the keyboard back with
    /// what it has typed selected.
    pub(super) fn open_search(&mut self, ctx: &Ctx) {
        self.set_tab(DetailTab::Diff);
        self.focus = Focus::Detail;
        let page = ctx.page.1.max(1);
        let from = match &self.cursor {
            Some(c) => c.row,
            None => self.diff_window(page).1.start,
        };
        let search = self.search.get_or_insert_with(DiffSearch::default);
        search.focused = true;
        search.from = from;
        search.field.select_all();
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
        if action_key(mods, sym) == Some(ReposButton::CopyReview) {
            return self.act(ReposButton::CopyReview, ctx);
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
                    self.find_hits(true);
                    self.search_from_start(ctx);
                }
            }
        }
        None
    }

    /// The keys for the search while open, with either focus: F3 goes on
    /// (Shift+F3 back) and Esc closes it. Whether `sym` was one.
    pub(super) fn search_open_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        ctx: &Ctx,
    ) -> bool {
        let others = mods.ctrl || mods.alt || mods.logo;
        if self.search.is_none() || self.tab() != DetailTab::Diff || others {
            return false;
        }
        match sym {
            Keysym::Escape if !mods.shift => self.close_search(),
            Keysym::F3 => self.step_search(if mods.shift { -1 } else { 1 }, ctx),
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
                    self.focus = Focus::Detail;
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
        let page = ctx.page.1.max(1);
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
        if hit.within != Within::Path
            && self
                .diff_folded
                .remove(&(hit.root.clone(), hit.path.clone()))
        {
            self.build_rows(ctx.items, self.diff_anchor());
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
        let page = ctx.page.1.max(1);
        // Out of view or near an edge (under the pinned header), it goes
        // a third down the page.
        let room = (page / 4).min(3) as f64;
        let near = |at: f64| at < room.max(1.0) || at > page as f64 - 1.0 - room;
        if self.shown_at(row, page).is_none_or(near) {
            let last = self.diff_last(page) as f64;
            let to = (row as f64 - (page / 3) as f64).clamp(0.0, last);
            move_to(&mut self.detail_scroll, to);
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
        let query = search.field.line().text;
        let was = search.current.and_then(|i| search.hits.get(i)).cloned();
        // The rows each place could show on, by who it is.
        let mut rows: HashMap<(usize, usize, Within), usize> = HashMap::new();
        let mut heads: HashMap<(usize, usize), usize> = HashMap::new();
        for (r, row) in self.diff_rows.iter().enumerate() {
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
        let mut hits = Vec::new();
        for (k, root) in self.diff_roots.iter().enumerate() {
            let Some((_, files)) = self.diffs.get(root) else {
                continue;
            };
            for (f, file) in files.iter().enumerate() {
                let Some(&head) = heads.get(&(k, f)) else {
                    continue;
                };
                let mut add = |within: Within, text: &str| {
                    let row = rows.get(&(k, f, within)).copied();
                    hits.extend(find::find_all(text, &query).into_iter().map(|chars| Hit {
                        root: root.clone(),
                        path: file.path.clone(),
                        within,
                        chars,
                        row,
                        tick: row.unwrap_or(head),
                    }));
                };
                add(Within::Path, &shown_path(file));
                for (h, hunk) in file.hunks.iter().enumerate() {
                    if !hunk.context.is_empty() {
                        add(Within::Context(h), &hunk.context);
                    }
                    for (l, line) in hunk.lines.iter().enumerate() {
                        add(Within::Line(h, l), &line.text);
                    }
                }
            }
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
    pub(super) fn found_on(&self, r: usize) -> Vec<(Range<usize>, bool)> {
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
    pub(super) fn search_bar(&self) -> Option<SearchBar> {
        let search = self.search.as_ref()?;
        let query = search.field.line();
        let n = search.hits.len();
        let count = match (query.text.is_empty(), search.current) {
            (true, _) => String::new(),
            _ if n == 0 => "No matches".into(),
            (_, Some(i)) => format!("{} of {n}", i + 1),
            (_, None) => format!("{n} found"),
        };
        Some(SearchBar {
            query,
            placeholder: "Find in the diff".into(),
            count,
            found: n > 0,
            focused: self.search_has_keys(),
        })
    }

    /// Where the places found are on the diff's scrollbar, as shares of
    /// its rows from the top, each with whether it is the one gone to;
    /// one per row.
    pub(super) fn search_ticks(&self) -> Vec<(f64, bool)> {
        let Some(search) = &self.search else {
            return Vec::new();
        };
        let total = self.diff_rows.len().max(1) as f64;
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
