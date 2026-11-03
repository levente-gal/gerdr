//! The file under review beside its preview: while a feature stage's file
//! is under review (`WorktreeDiff::doc`) and a tab of the review's
//! workspace shows its preview (see `md_preview`), the review panel takes
//! the output's left part and the preview's window shows in the right
//! part, though its tab isn't the one shown. The output must be wide
//! enough (`DOCK_MIN_WIDTH`); on a narrower one, the preview is a tab of
//! its own, as before.
//!
//! The keys stay with the review. The pointer over the preview is the
//! browser's: it scrolls the page and follows its links, and a click on
//! a block of the page puts the review's caret on its first line. The
//! two scroll together: each tells the other the line of the file at its
//! top, to the fraction (`DiffView::top_line`). The
//! page marks the block the caret is in, and the lines with comments.
//! The gap between the two drags their split, kept in `state.json`.
//!
//! Ctrl+Shift+V puts the preview beside the review, opening its tab
//! behind the one shown if there is none, and takes it away again. Esc
//! closes the review; the preview's tab stays, for the next review.

use std::path::Path;

use smithay::{
    desktop::Window,
    utils::{Logical, Point, Size},
};

use crate::{
    Drag, Gerdr, Tab,
    chrome::{self, DOCK_GAP, DOCK_MIN_WIDTH, DOCK_SHARE_MAX, DOCK_SHARE_MIN, Rect},
    md_preview::{self, NO_EXTENSION},
};

/// How far, in lines, the review may move before its preview follows.
const SCROLL_STILL: f64 = 0.01;

/// The line of a file under review its caret is on, and its comments by
/// the line they end on.
pub type ReviewMarks = (Option<usize>, Vec<(usize, String)>);

/// The review beside its preview, as laid out now.
#[derive(Clone, Debug)]
pub struct Dock {
    /// The preview's pane, and its window.
    pub pane: u32,
    pub window: Window,
    /// What the review panel is centred in: the output's left part.
    pub panel: Size<i32, Logical>,
    /// Where the preview's window shows.
    pub preview: Rect,
    /// The gap between the panel and the preview, which drags their split.
    pub gap: Rect,
}

/// Where the review and its preview go on an output of `size`, its app
/// area `area`, the review taking `share` of its width: the review
/// panel's part, the preview's place and the gap between them.
pub fn dock_rects(
    size: Size<i32, Logical>,
    area: Rect,
    share: f64,
) -> (Size<i32, Logical>, Rect, Rect) {
    let share = share.clamp(DOCK_SHARE_MIN, DOCK_SHARE_MAX);
    let split = (size.w as f64 * share).round() as i32;
    let panel_part: Size<i32, Logical> = (split.max(1), size.h).into();
    let panel = chrome::repos_panel_rect(panel_part);
    let panel_right = panel.loc.x + panel.size.w;
    let left = split.max(area.loc.x);
    let right = area.loc.x + area.size.w - DOCK_GAP;
    let top = area.loc.y + DOCK_GAP / 2;
    let bottom = area.loc.y + area.size.h - DOCK_GAP / 2;
    let preview = Rect::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    );
    let gap = Rect::new(
        (panel_right, top).into(),
        ((left - panel_right).max(1), (bottom - top).max(1)).into(),
    );
    (panel_part, preview, gap)
}

/// The tab of `ws`'s tabs, and its pane, showing the preview at `url`,
/// its window come or not; `pane` is a preview of the file (see
/// `md_preview::Previews`).
fn preview_tab(
    tabs: &[Tab],
    url: &str,
    shows: impl Fn(u32) -> bool,
) -> Option<(usize, u32, Option<Window>)> {
    tabs.iter().enumerate().find_map(|(t, tab)| {
        let shown = tab
            .panes
            .iter()
            .find(|p| shows(p.id) || p.app_command.as_deref().map(str::trim) == Some(url));
        if let Some(pane) = shown {
            return Some((t, pane.id, Some(pane.window.clone())));
        }
        let coming = tab
            .expected
            .iter()
            .find(|e| e.app_command.as_deref().map(str::trim) == Some(url))?;
        Some((t, coming.pane, None))
    })
}

impl Gerdr {
    /// The review beside its preview, if it is now.
    pub fn dock(&self) -> Option<Dock> {
        if self.previews.undocked || !self.repos.is_open() {
            return None;
        }
        let wd = self.wd()?;
        let doc = wd.doc.as_ref()?;
        let size = self.output_size();
        if wd.workspace != self.active_workspace || size.w < DOCK_MIN_WIDTH {
            return None;
        }
        let tabs = &self.workspaces.get(wd.workspace)?.tabs;
        let url = md_preview::page_url(&doc.path);
        let shows = |pane| self.previews.shows(pane, &doc.path);
        let (_, pane, window) = preview_tab(tabs, &url, shows)?;
        let (panel, preview, gap) = dock_rects(size, self.content_rect(), self.previews.share);
        Some(Dock {
            pane,
            window: window?,
            panel,
            preview,
            gap,
        })
    }

    /// Where the repositories panel goes: centred in the output, or in
    /// its left part beside a preview.
    pub fn repos_rect(&self) -> chrome::Rect {
        let around = self.dock().map_or_else(|| self.output_size(), |d| d.panel);
        chrome::repos_panel_rect(around)
    }

    /// Lay the windows out again if the review went beside its preview,
    /// or away from it, or their split moved; and tell the previews where
    /// the review's caret is and what it commented. Whether the review is
    /// beside its preview. Every frame.
    pub(crate) fn sync_dock(&mut self) -> bool {
        let dock = self.dock();
        let now = dock.as_ref().map(|d| (d.pane, d.preview));
        if now != self.previews.docked {
            self.arrange();
            self.refresh_pointer_focus();
        }
        self.sync_preview_marks();
        if let Some(dock) = &dock {
            self.sync_dock_scroll(dock.pane);
        }
        dock.is_some()
    }

    /// Ctrl+Shift+V in a file's review: its preview beside it, on an
    /// output wide enough, opened behind the tab shown if no tab shows it;
    /// taken away again when it is. On a narrow one, the review closes
    /// for its preview's tab (see `open_doc_preview`).
    pub fn toggle_doc_preview(&mut self) {
        if self.output_size().w < DOCK_MIN_WIDTH {
            return self.open_doc_preview();
        }
        if self.dock().is_some() {
            self.previews.undocked = true;
            return;
        }
        let Some(wd) = self.wd() else {
            return;
        };
        let Some(doc) = wd.doc.as_ref() else {
            return;
        };
        let (w, file) = (wd.workspace, doc.path.clone());
        self.previews.undocked = false;
        let url = md_preview::page_url(&file);
        let Some(ws) = self.workspaces.get(w) else {
            return;
        };
        let shows = |pane| self.previews.shows(pane, &file);
        if preview_tab(&ws.tabs, &url, shows).is_some() {
            return;
        }
        if !self.steers(&url, &ws.dir.clone()) {
            crate::problems::report(md_preview::PROBLEM, NO_EXTENSION);
            return self.show_error(NO_EXTENSION.into());
        }
        crate::problems::clear(md_preview::PROBLEM);
        self.open_preview_behind(w, &url);
    }

    /// Open the preview at `url` in a new tab of workspace `w`, behind the
    /// tab shown.
    fn open_preview_behind(&mut self, w: usize, url: &str) {
        let Some(ws) = self.workspaces.get_mut(w) else {
            return;
        };
        ws.tabs.push(Tab::default());
        let t = ws.tabs.len() - 1;
        if self.launch_into(w, t, None, url).is_none() {
            tracing::warn!("the preview's tab didn't open: {url}");
        }
        self.arrange();
    }

    /// The divider between the review and its preview, if it is at `pos`.
    pub(crate) fn dock_divider_at(&self, pos: Point<f64, Logical>) -> Option<Drag> {
        let dock = self.dock()?;
        let gap = dock.gap.to_f64();
        let near = pos.x >= gap.loc.x - 2.0
            && pos.x <= gap.loc.x + gap.size.w + 2.0
            && pos.y >= gap.loc.y
            && pos.y <= gap.loc.y + gap.size.h;
        near.then_some(Drag::DockSplit)
    }

    /// The divider between the review and its preview dragged to `pos`.
    pub(crate) fn dock_drag_to(&mut self, pos: Point<f64, Logical>) {
        let w = self.output_size().w.max(1) as f64;
        // The middle of the gap follows the pointer.
        let half = DOCK_GAP as f64 / 2.0;
        self.previews.share = ((pos.x + half) / w).clamp(DOCK_SHARE_MIN, DOCK_SHARE_MAX);
    }

    /// Whether `pos` is on the preview beside the review: the browser's,
    /// not the panel's.
    pub(crate) fn on_dock(&self, pos: Point<f64, Logical>) -> bool {
        self.dock()
            .is_some_and(|d| d.preview.to_f64().contains(pos))
    }

    /// The line of the file under review the caret is on, and its
    /// comments by line, while `file` is under review.
    pub(crate) fn review_marks(&self, file: &Path) -> Option<ReviewMarks> {
        if !self.repos.is_open() {
            return None;
        }
        let wd = self.wd()?;
        let doc = wd.doc.as_ref().filter(|doc| doc.path == file)?;
        let comments = match doc.evidence() {
            true => Vec::new(),
            false => wd.diff.comment_lines(),
        };
        Some((wd.diff.cursor_line(), comments))
    }

    /// Scroll the preview `pane` beside the review to where the review
    /// stands, if it moved since they last stood together.
    fn sync_dock_scroll(&mut self, pane: u32) {
        let Some(top) = self.wd().and_then(|wd| wd.diff.top_line()) else {
            return;
        };
        if (self.previews.top(pane)).is_some_and(|was| (was - top).abs() < SCROLL_STILL) {
            return;
        }
        if self.scroll_to_browser(pane, top) {
            self.previews.set_top(pane, top);
        }
    }

    /// The preview `pane` beside the review scrolled to `line` of its
    /// file at its top: the review stands there too.
    pub(crate) fn preview_scrolled(&mut self, pane: u32, line: f64) {
        if !line.is_finite() || self.dock().is_none_or(|d| d.pane != pane) {
            return;
        }
        let env = self.worktree_env();
        let Some(wd) = self.wd_mut() else {
            return;
        };
        let top = wd.with_diff(env, |diff, ctx| {
            diff.stand_at_line(line, ctx);
            diff.top_line()
        });
        // What the review stands at is what the page shows: not sent back.
        self.previews.set_top(pane, top.unwrap_or(line));
    }

    /// A block clicked in `pane`'s preview, from `line` of its file: the
    /// caret of the file's review goes there.
    pub(crate) fn preview_line(&mut self, pane: u32, line: usize) {
        let Some(file) = self.previews.file(pane) else {
            return;
        };
        if self.review_marks(&file).is_none() {
            return;
        }
        let env = self.worktree_env();
        let Some(wd) = self.wd_mut() else {
            return;
        };
        wd.with_diff(env, |diff, ctx| {
            if let Some(row) = diff.row_of_line(line) {
                diff.caret_to(row, 0, ctx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::DOCK_SHARE;

    #[test]
    fn the_review_and_its_preview_share_the_output() {
        let size: Size<i32, Logical> = (1600, 1000).into();
        let area = Rect::new((280, 40).into(), (1320, 930).into());
        let (panel, preview, gap) = dock_rects(size, area, DOCK_SHARE);
        assert_eq!(panel, (800, 1000).into());
        let review = chrome::repos_panel_rect(panel);
        // Side by side, the gap between them, all on the app area's right.
        assert_eq!(gap.loc.x, review.loc.x + review.size.w);
        assert_eq!(gap.loc.x + gap.size.w, preview.loc.x);
        assert_eq!(preview.loc.x, 800);
        assert_eq!(preview.loc.x + preview.size.w, 1600 - DOCK_GAP);
        assert!(preview.loc.y >= area.loc.y);
        assert!(preview.loc.y + preview.size.h <= area.loc.y + area.size.h);
        // The split stays within its bounds.
        let (panel, ..) = dock_rects(size, area, 0.95);
        assert_eq!(panel.w, (1600.0 * DOCK_SHARE_MAX).round() as i32);
        let (panel, ..) = dock_rects(size, area, 0.0);
        assert_eq!(panel.w, (1600.0 * DOCK_SHARE_MIN).round() as i32);
    }
}
