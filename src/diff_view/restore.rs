//! Changes put back as the diff's base had them: `R` the change at the
//! caret (its run of removed and added lines) or the lines selected, the
//! Restore button on a hunk's header the hunk. A restore cuts the file in
//! the working tree as vim's deletes do (see `vim`): only while it holds
//! what the diff showed, and `u` undoes it.

use super::{DiffRow, DiffView, Outcome, vim::Visual};
use crate::{
    diff::LineKind,
    diff_edit::{self, FileEdit, Spot},
};

impl DiffView {
    /// `R`: put back the lines selected, else the change at the cursor.
    pub(super) fn restore_here(&mut self) -> Option<Outcome> {
        let cursor = self.cursor.as_ref()?;
        let (row, from) = (cursor.row, cursor.from);
        let rows = match (self.visual.take(), self.caret()) {
            (Some(Visual::Chars { row: start, .. }), Some((at, _))) => {
                start.min(at)..=start.max(at)
            }
            _ => from.min(row)..=from.max(row),
        };
        if let Some(cursor) = &mut self.cursor {
            cursor.from = cursor.row;
            cursor.keys.1 = cursor.keys.0.clone();
        }
        if rows.start() != rows.end() {
            return self.restore_rows(rows);
        }
        let Some(&DiffRow::Line(k, f, h, l)) = self.rows.get(row) else {
            return Some(Outcome::Notice("No change there"));
        };
        let (_, file) = self.diff_file(k, f)?;
        let lines = &file.hunks[h].lines;
        if lines[l].kind == LineKind::Context {
            return Some(Outcome::Notice("No change there"));
        }
        let changed = |i: &usize| lines[*i].kind != LineKind::Context;
        let first = (0..l).rev().take_while(changed).last().unwrap_or(l);
        let last = (l + 1..lines.len()).take_while(changed).last().unwrap_or(l);
        self.restore_in(k, f, |hh, ll| hh == h && (first..=last).contains(&ll))
    }

    /// The Restore button on the header of the hunk at diff row `r`: the
    /// whole hunk put back.
    pub(super) fn restore_hunk(&self, r: usize) -> Option<Outcome> {
        let &DiffRow::Hunk(k, f, h) = self.rows.get(r)? else {
            return None;
        };
        self.restore_in(k, f, |hh, _| hh == h)
    }

    /// Put back the changed lines among diff rows `rows`: all in one file.
    fn restore_rows(&self, rows: std::ops::RangeInclusive<usize>) -> Option<Outcome> {
        let mut file = None;
        let mut picked = Vec::new();
        for r in rows {
            let Some(&DiffRow::Line(k, f, h, l)) = self.rows.get(r) else {
                continue;
            };
            if file.get_or_insert((k, f)) != &(k, f) {
                return Some(Outcome::Notice("Select lines of one file"));
            }
            picked.push((h, l));
        }
        let Some((k, f)) = file else {
            return Some(Outcome::Notice("No change there"));
        };
        self.restore_in(k, f, |h, l| picked.contains(&(h, l)))
    }

    /// Put back the changed lines of file `f` of repository `k` that
    /// `picked` takes (by hunk and line): its added lines go, its removed
    /// lines come back where they were.
    fn restore_in(
        &self,
        k: usize,
        f: usize,
        picked: impl Fn(usize, usize) -> bool,
    ) -> Option<Outcome> {
        let (root, file) = self.diff_file(k, f)?;
        let why = match file.code {
            'U' => Some("Resolve the conflict in Zed first"),
            'D' => Some("The file is deleted: nothing to put lines back in"),
            _ => None,
        };
        if let Some(why) = why {
            return Some(Outcome::Notice(why));
        }
        let picked_last = |h: usize, l: usize| {
            let last = h + 1 == file.hunks.len() && l + 1 == file.hunks[h].lines.len();
            last && picked(h, l)
        };
        // Its diff was cut short after its last line: more may be gone.
        let cut_short = file.notes.first().is_some_and(|n| n.starts_with('…'));
        let h = file.hunks.len().saturating_sub(1);
        let l = file
            .hunks
            .last()
            .map_or(0, |h| h.lines.len().saturating_sub(1));
        if cut_short && picked_last(h, l) {
            return Some(Outcome::Notice("Its diff is cut short: restore it in Zed"));
        }
        let Some((cut, line)) = diff_edit::restore_cut(file, picked) else {
            return Some(Outcome::Notice("No change there"));
        };
        Some(Outcome::Cut(FileEdit {
            root: root.to_path_buf(),
            path: file.path.clone(),
            cut,
            shown: diff_edit::shown(file),
            caret: Spot { line, col: 0 },
        }))
    }
}
