//! A feature stage's file in a tab of its own: flat across the tab, not a
//! popup. The tab (`TabKind::Review`) shows its review, or that and the file
//! rendered side by side; Verify's evidence, only read, the file rendered
//! too on its own (`DocMode::choices`). A stage's own page shows its file to
//! read, so its tab is for working on it. The switch in the header and
//! Ctrl+Shift+V move between them. It is the same review as ever
//! (`WorktreeDiff`, painted by the repositories panel's own `ReposLayers`),
//! and the same page (`md_preview`) in the tab's one pane, a web window
//! laid out where the flat panel leaves a hole (`ReposLayout::page`).
//!
//! The review owns the keys but in Preview, where the page has them and
//! only what acts on the file is taken (`doc_preview_key`). The pointer
//! over the page is the browser's: a click on a block puts the review's
//! caret on its first line, and the two scroll together, each telling the
//! other the line of the file at its top (`DiffView::top_line`). The gap
//! between them drags their split, kept in `state.json`.
//!
//! The page opens when first wanted, on the frame its tab shows, so a
//! restored tab starts no browser and takes no focus until it is looked
//! at. Without gerdr's extension the review still works:
//! the switch's other choices are off, with a tooltip saying why. The tab
//! is saved and comes back on its file and mode; one whose feature is gone
//! doesn't. Esc goes back to the feature's tab; the file's tab stays, for
//! the next look.

use std::path::Path;

use serde::{Deserialize, Serialize};
use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Drag, Gerdr, Tab, TabKind,
    chrome::{
        self, DOCK_MIN_WIDTH, DOCK_SHARE_MAX, DOCK_SHARE_MIN, DetailView, ModeSwitch, Rect,
        ReposButton, ReposView,
    },
    doc_review::DocFile,
    md_preview::{self, NO_EXTENSION},
    review::{self, Review},
    worktree_diff::{DRAFT_OPEN, DocOf, Env, WorktreeDiff},
};

/// How far, in lines, the review may move before its page follows.
const SCROLL_STILL: f64 = 0.01;

/// The line of a file under review its caret is on, and its comments by
/// the line they end on.
pub type ReviewMarks = (Option<usize>, Vec<(usize, String)>);

/// What a file's tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocMode {
    /// The file rendered, with the page holding the keys.
    #[default]
    Preview,
    /// Its review across the tab.
    Review,
    /// The two side by side, the review holding the keys.
    Split,
}

impl DocMode {
    /// All of them, in the order the switch shows them.
    pub const ALL: [DocMode; 3] = [DocMode::Preview, DocMode::Review, DocMode::Split];

    /// What the switch calls it.
    pub fn label(self) -> &'static str {
        match self {
            DocMode::Preview => "Preview",
            DocMode::Review => "Review",
            DocMode::Split => "Split",
        }
    }

    /// What `file`'s tab can show, in the order the switch shows them. A
    /// stage's file is read on its page, so its tab has no Preview; the
    /// evidence has no page but this.
    pub fn choices(file: DocFile) -> &'static [DocMode] {
        match file {
            DocFile::Stage(_) => &[DocMode::Review, DocMode::Split],
            DocFile::Evidence => &DocMode::ALL,
        }
    }

    /// The next one round of `file`'s, on an output `wide` enough for Split
    /// or not.
    pub fn next(self, file: DocFile, wide: bool) -> DocMode {
        let round: Vec<DocMode> = (Self::choices(file).iter().copied())
            .filter(|m| wide || *m != DocMode::Split)
            .collect();
        let at = round.iter().position(|m| *m == self);
        at.map_or(round[0], |at| round[(at + 1) % round.len()])
    }

    /// What a file's tab opens on when nothing asked for more: a stage's
    /// file is reviewed, the evidence is read.
    pub fn default_for(file: DocFile) -> DocMode {
        match file {
            DocFile::Stage(_) => DocMode::Review,
            DocFile::Evidence => DocMode::Preview,
        }
    }

    /// What a new tab of `file` opens on, on an output `wide` enough for
    /// Split or not: a stage's file beside its page, so it is read as it is
    /// reviewed. Too narrow, no page starts unseen.
    pub fn opens_on(file: DocFile, wide: bool) -> DocMode {
        match file {
            DocFile::Stage(_) if wide => DocMode::Split,
            _ => Self::default_for(file),
        }
    }

    /// This one as `file`'s tab can show it: a tab saved on a mode it no
    /// longer has comes back on what it opens on.
    pub fn of(self, file: DocFile) -> DocMode {
        match Self::choices(file).contains(&self) {
            true => self,
            false => Self::default_for(file),
        }
    }
}

/// A file's tab, saved with the tabs (see `TabRecord`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRecord {
    pub file: DocFile,
    pub mode: DocMode,
}

/// What a file's tab holds beyond any tab's own.
pub struct ReviewTab {
    pub mode: DocMode,
    pub wd: WorktreeDiff,
    /// Where the page's window goes, as the view was laid out last: none
    /// in Review, where it hides.
    pub page: Option<Rect>,
    /// The page can't open: no gerdr extension to steer it.
    pub no_browser: bool,
    /// Its page was opened, or tried: one gone since doesn't come back by
    /// itself.
    pub page_asked: bool,
    /// Closed by the user: it goes with its panes (see `Gerdr::prune`).
    pub closing: bool,
}

impl ReviewTab {
    pub fn new(wd: WorktreeDiff, mode: DocMode) -> Self {
        ReviewTab {
            mode,
            wd,
            page: None,
            no_browser: false,
            page_asked: false,
            closing: false,
        }
    }
}

// A diff is too much to print with a tab.
impl std::fmt::Debug for ReviewTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewTab")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

/// Whether Preview takes this key from the page: only what acts on the
/// file, the page keeping the rest.
pub fn doc_preview_key(mods: &ModifiersState, sym: Keysym) -> bool {
    let shift = mods.shift;
    let plain = !mods.ctrl && !mods.alt && !mods.logo;
    let ctrl = mods.ctrl && !mods.alt && !mods.logo;
    match sym {
        Keysym::Escape | Keysym::F1 => plain && !shift,
        Keysym::v | Keysym::V => ctrl && shift,
        Keysym::Return | Keysym::KP_Enter => ctrl,
        _ => false,
    }
}

/// The review's share of the tab's width in `mode`: none in Preview, all
/// of it in Review or on an output too narrow for Split, `split` in Split.
pub fn detail_share(mode: DocMode, wide: bool, split: f64) -> f64 {
    match mode {
        DocMode::Preview => 0.0,
        DocMode::Review => 1.0,
        DocMode::Split if wide => split.clamp(DOCK_SHARE_MIN, DOCK_SHARE_MAX),
        DocMode::Split => 1.0,
    }
}

/// The switch of `file`'s tab: Split only on an output `wide` enough, and
/// what needs the page off `without_page`. None with nothing to go to.
pub fn mode_switch(
    file: DocFile,
    mode: DocMode,
    wide: bool,
    without_page: bool,
) -> Option<ModeSwitch> {
    let shown = (DocMode::choices(file).iter().copied())
        .filter(|m| wide || *m != DocMode::Split || mode == DocMode::Split);
    let shown: Vec<DocMode> = shown.collect();
    (shown.len() > 1).then(|| ModeSwitch {
        labels: shown.iter().map(|m| m.label().to_owned()).collect(),
        on: shown.iter().position(|m| *m == mode).unwrap_or(0),
        off: match without_page {
            true => (shown.iter().enumerate())
                .filter(|(_, m)| **m != DocMode::Review)
                .map(|(i, _)| i)
                .collect(),
            false => Vec::new(),
        },
    })
}

impl Gerdr {
    /// The file's tab showing, if the shown tab is one.
    pub(crate) fn review_tab(&self) -> Option<&ReviewTab> {
        self.tab()?.review()
    }

    /// `review_tab`, to change.
    pub(crate) fn review_tab_mut(&mut self) -> Option<&mut ReviewTab> {
        let ws = self.workspaces.get_mut(self.active_workspace)?;
        ws.tabs.get_mut(ws.active_tab)?.review_mut()
    }

    /// What the mode switch's click means at index `i` of what it shows.
    pub(crate) fn doc_mode_at(&self, i: usize) -> Option<DocMode> {
        let label = self.doc_tab_view()?.modes?.labels.get(i)?.clone();
        DocMode::ALL.into_iter().find(|m| m.label() == label)
    }

    /// Whether the output has room for the review beside its page.
    fn doc_wide(&self) -> bool {
        self.output_size().w >= DOCK_MIN_WIDTH
    }

    /// What the review's views and keys need of the chrome in `mode`. Its
    /// columns are those it shows in, so going to Preview and back doesn't
    /// wrap its comments anew.
    pub(crate) fn doc_env(&self, mode: DocMode) -> Env {
        let area = self.content_rect();
        let share = match mode {
            DocMode::Preview => 1.0,
            _ => detail_share(mode, self.doc_wide(), self.previews.share),
        };
        let (left, ..) = chrome::split_rects(area, share);
        Env {
            page: chrome::diff_only_page(area, self.density),
            cols: chrome::diff_only_cols(left),
            density: self.density,
            sheet_rows: chrome::repos_sheet_rows(area, true),
        }
    }

    /// What the shown file's tab paints, whatever is over it.
    pub(crate) fn doc_tab_view(&self) -> Option<ReposView> {
        let tab = self.review_tab()?;
        let file = tab.wd.doc.as_ref()?.file;
        let mut view = tab.wd.repos_view(self.doc_env(tab.mode));
        let wide = self.doc_wide();
        view.flat = true;
        view.detail_share = detail_share(tab.mode, wide, self.previews.share);
        view.modes = mode_switch(file, tab.mode, wide, tab.no_browser);
        if tab.mode == DocMode::Preview {
            view.detail = DetailView::default();
            (view.hints)
                .retain(|h| !matches!(h.button, ReposButton::Comment | ReposButton::CopyLines));
        }
        Some(view)
    }

    /// Bring the shown file's tab up to date, every frame: read when it
    /// comes to show; its page opened if wanted and its window laid out
    /// where the view leaves room for it; what the page marks and where it
    /// stands sent on.
    pub(crate) fn sync_doc_tab(&mut self) -> Option<ReposView> {
        let shown = self.tab().filter(|t| t.review().is_some()).map(|t| t.id);
        if shown != self.doc_tab_shown {
            self.doc_tab_shown = shown;
            self.doc_review_read();
            // What it says it is locked by is the latest there is.
            if let Some((project, slug)) = (self.review_tab())
                .and_then(|r| r.wd.doc.as_ref())
                .map(|d| (d.project.clone(), d.slug.clone()))
            {
                self.probe_feature(&project, &slug);
            }
        }
        self.sync_preview_marks();
        shown?;
        self.doc_stand();
        let (w, t) = (self.active_workspace, self.workspace()?.active_tab);
        let workspace = self.active_workspace;
        let env = self.doc_env(self.review_tab()?.mode);
        let review = self.review_tab_mut()?;
        review.wd.workspace = workspace;
        // Its lines wrap at the width it shows in, as the gap is dragged too.
        review.wd.with_diff(env, |diff, ctx| diff.rewrap(ctx));
        self.open_wanted_page(w, t);
        let view = self.doc_tab_view()?;
        let page = chrome::repos_layout(&view, self.content_rect()).page;
        let review = self.review_tab_mut()?;
        if review.page != page {
            review.page = page;
            self.arrange();
            self.refresh_pointer_focus();
        }
        self.sync_doc_scroll();
        Some(view)
    }

    /// Open the page of the file's tab `t` of workspace `w` when its mode
    /// wants it and there is none. One asked for and gone (its window
    /// closed, the browser quit) isn't opened again: back to the review.
    fn open_wanted_page(&mut self, w: usize, t: usize) {
        let Some(tab) = self.workspaces.get_mut(w).and_then(|ws| ws.tabs.get_mut(t)) else {
            return;
        };
        let empty = tab.layout.is_none() && tab.expected.is_empty();
        let Some(review) = tab.review_mut() else {
            return;
        };
        if review.mode == DocMode::Review || !empty {
            return;
        }
        let asked = std::mem::replace(&mut review.page_asked, true);
        if (asked || !self.ensure_doc_page(w, t))
            && let Some(review) = self.review_tab_mut()
        {
            review.mode = DocMode::Review;
        }
    }

    /// The page of the file's tab `t` of workspace `w`, opened if it has
    /// none. Without gerdr's extension it can't: an error says so, and the
    /// switch's choices needing it go off.
    fn ensure_doc_page(&mut self, w: usize, t: usize) -> bool {
        let Some(ws) = self.workspaces.get(w) else {
            return false;
        };
        let Some(tab) = ws.tabs.get(t) else {
            return false;
        };
        if tab.layout.is_some() || !tab.expected.is_empty() {
            return true;
        }
        let Some(doc) = tab.review().and_then(|r| r.wd.doc.as_ref()) else {
            return false;
        };
        let url = md_preview::page_url(&doc.path);
        let dir = ws.dir.clone();
        let steers = self.steers(&url, &dir);
        if let Some(review) = (self.workspaces.get_mut(w))
            .and_then(|ws| ws.tabs.get_mut(t))
            .and_then(Tab::review_mut)
        {
            review.no_browser = !steers;
        }
        if !steers {
            crate::problems::report(md_preview::PROBLEM, NO_EXTENSION);
            self.show_error(NO_EXTENSION.into());
            return false;
        }
        crate::problems::clear(md_preview::PROBLEM);
        self.launch_into(w, t, None, &url).is_some()
    }

    /// Show `mode` in the shown file's tab: a draft comment stays on show,
    /// and what needs the page says when it can't have it.
    pub(crate) fn set_doc_mode(&mut self, mode: DocMode) {
        let (w, Some(t)) = (
            self.active_workspace,
            self.workspace().map(|ws| ws.active_tab),
        ) else {
            return;
        };
        let Some(review) = self.review_tab() else {
            return;
        };
        if review.mode == mode {
            return;
        }
        if mode == DocMode::Preview && review.wd.diff.editing.is_some() {
            return self.show_warning(DRAFT_OPEN.into());
        }
        if mode != DocMode::Review && !self.ensure_doc_page(w, t) {
            return;
        }
        if let Some(review) = self.review_tab_mut() {
            review.mode = mode;
            review.page_asked = true;
        }
        // Only Preview leaves the keys to the page.
        let page = self.tab().and_then(|tab| tab.panes.first());
        let window = page
            .filter(|_| mode == DocMode::Preview)
            .map(|pane| pane.window.clone());
        self.focus(window);
        self.refresh_pointer_focus();
    }

    /// Ctrl+Shift+V in a file's tab: the next mode round.
    pub(crate) fn cycle_doc_mode(&mut self) {
        let Some(tab) = self.review_tab() else {
            return;
        };
        if let Some(doc) = &tab.wd.doc {
            self.set_doc_mode(tab.mode.next(doc.file, self.doc_wide()));
        }
    }

    /// Whether the shown file's tab has the keys now: not in Preview, where
    /// the page has them, unless a sheet of its own is open.
    pub(crate) fn doc_tab_takes_keys(&self) -> bool {
        self.review_tab().is_some_and(|tab| {
            tab.mode != DocMode::Preview || tab.wd.sheet(self.doc_env(tab.mode)).is_some()
        })
    }

    /// Whether the shown file's tab takes the pointer at `pos`: over its
    /// page, the browser does, unless a sheet of its own is open.
    pub(crate) fn doc_tab_covers(&self, pos: Point<f64, Logical>) -> bool {
        let Some(tab) = self.review_tab() else {
            return false;
        };
        if !self.content_rect().to_f64().contains(pos) {
            return false;
        }
        let on_page = tab.page.is_some_and(|page| page.to_f64().contains(pos));
        !on_page || tab.wd.sheet(self.doc_env(tab.mode)).is_some()
    }

    /// Show `file` of feature `slug` in its tab: one already open as it
    /// stands, a new one as `DocMode::opens_on` it.
    pub fn open_doc_tab(&mut self, project: &str, slug: &str, file: DocFile) {
        let (doc, text) = match self.read_doc(project, slug, file) {
            Ok(read) => read,
            Err(failed) => return self.show_failed(failed),
        };
        self.close_panels_for_tab();
        self.in_feature_workspace(project, slug, |gerdr, w| {
            let t = gerdr.doc_tab_in(w, file).unwrap_or_else(|| {
                let shown = DocMode::opens_on(file, gerdr.doc_wide());
                gerdr.insert_doc_tab(w, doc, text, shown)
            });
            gerdr.switch_workspace(w);
            gerdr.switch_tab(t);
        });
        self.refresh_pointer_focus();
    }

    /// The panels a tab coming to show would sit under close, the
    /// repositories panel kept as it was.
    fn close_panels_for_tab(&mut self) {
        self.switcher = None;
        self.project_menu = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.scrollback = None;
        self.worktrees.close();
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        if self.repos.worktree.is_some() {
            self.worktree_save_review();
            self.repos.close_worktree();
        }
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
    }

    /// The tab of workspace `w` showing `file`, if any.
    fn doc_tab_in(&self, w: usize, file: DocFile) -> Option<usize> {
        let tabs = &self.workspaces.get(w)?.tabs;
        tabs.iter().position(|tab| {
            let doc = tab.review().and_then(|r| r.wd.doc.as_ref());
            doc.is_some_and(|doc| doc.file == file)
        })
    }

    /// Make `doc`, read as `text`, a tab of workspace `w` in `mode`, right
    /// after the feature's tab, what is shown kept shown. Where it went.
    fn insert_doc_tab(&mut self, w: usize, doc: DocOf, text: String, mode: DocMode) -> usize {
        let review = match doc.file {
            DocFile::Stage(_) => review::load_file(&doc.comments),
            DocFile::Evidence => Review::default(),
        };
        let mut wd = WorktreeDiff::of_doc(doc, w, review);
        wd.set_doc(Ok(text), self.doc_env(mode));
        let ws = &mut self.workspaces[w];
        let t = usize::from(ws.tabs.first().is_some_and(Tab::is_feature));
        let t = t.min(ws.tabs.len());
        if ws.active_tab >= t && !ws.tabs.is_empty() {
            ws.active_tab += 1;
        }
        ws.tabs.insert(
            t,
            Tab {
                kind: TabKind::Review(Box::new(ReviewTab::new(wd, mode))),
                ..Tab::default()
            },
        );
        t
    }

    /// Bring back the file's tab `saved` as tab `t` of workspace `w`, if
    /// its feature and file are still there. Whether it came.
    pub(crate) fn restore_doc_tab(&mut self, w: usize, saved: DocRecord, t: usize) -> bool {
        let (Some(project), Some(slug)) = (
            self.project.clone(),
            self.workspace_feature(w).map(|f| f.slug.clone()),
        ) else {
            return false;
        };
        let Ok((doc, text)) = self.read_doc(&project, &slug, saved.file) else {
            return false;
        };
        let at = self.insert_doc_tab(w, doc, text, saved.mode.of(saved.file));
        // In the place it was saved at, among the others.
        let ws = &mut self.workspaces[w];
        let t = t.min(ws.tabs.len() - 1);
        if at != t {
            let tab = ws.tabs.remove(at);
            ws.tabs.insert(t, tab);
        }
        true
    }

    /// The file's tab `idx` of the active workspace is closed for good: it
    /// goes once its page's window does, and what its vim edits could undo
    /// goes with it, since its agent may write the file meanwhile.
    pub(crate) fn doc_tab_closing(&mut self, idx: usize) {
        let Some(review) = (self.workspaces.get_mut(self.active_workspace))
            .and_then(|ws| ws.tabs.get_mut(idx))
            .and_then(Tab::review_mut)
        else {
            return;
        };
        review.closing = true;
        let root = review.wd.root.clone();
        self.repos.cuts.retain(|d| d.root != root);
        self.repos.undone.retain(|d| d.root != root);
    }

    /// The gap between the review and its page, if it is at `pos`.
    pub(crate) fn doc_gap_at(&self, pos: Point<f64, Logical>) -> Option<Drag> {
        if self.context_menu.is_some() || self.grabbing_overlay().is_some() {
            return None;
        }
        let view = self.doc_tab_view()?;
        let layout = chrome::repos_layout(&view, self.content_rect());
        if layout.sheet.is_some() {
            return None;
        }
        let gap = layout.page_gap?.to_f64();
        let near = pos.x >= gap.loc.x - 2.0
            && pos.x <= gap.loc.x + gap.size.w + 2.0
            && pos.y >= gap.loc.y
            && pos.y <= gap.loc.y + gap.size.h;
        near.then_some(Drag::DocSplit)
    }

    /// The gap between the review and its page dragged to `pos`.
    pub(crate) fn doc_drag_to(&mut self, pos: Point<f64, Logical>) {
        let area = self.content_rect();
        let share = (pos.x - area.loc.x as f64) / area.size.w.max(1) as f64;
        self.previews.share = share.clamp(DOCK_SHARE_MIN, DOCK_SHARE_MAX);
    }

    /// Where the repositories panel goes: centred in the output; the
    /// review of a file's tab across the tab, with no panel over it.
    pub fn repos_rect(&self) -> Rect {
        match !self.repos.is_open() && self.review_tab().is_some() {
            true => self.content_rect(),
            false => chrome::repos_panel_rect(self.output_size()),
        }
    }

    /// The review of `file`, any tab's: its caret's line and its comments.
    pub(crate) fn review_marks(&self, file: &Path) -> Option<ReviewMarks> {
        let tabs = self.workspaces.iter().flat_map(|ws| &ws.tabs);
        let wd = tabs
            .filter_map(Tab::review)
            .map(|review| &review.wd)
            .find(|wd| wd.doc.as_ref().is_some_and(|doc| doc.path == file))?;
        let comments = match wd.doc.as_ref().is_some_and(DocOf::evidence) {
            true => Vec::new(),
            false => wd.diff.comment_lines(),
        };
        Some((wd.diff.cursor_line(), comments))
    }

    /// Scroll the shown file's page to where its review stands, if that
    /// moved since they last stood together and both are in use.
    fn sync_doc_scroll(&mut self) {
        let Some(tab) = self.tab() else {
            return;
        };
        let Some(pane) = tab.panes.first().map(|p| p.id) else {
            return;
        };
        let Some(top) = (tab.review())
            .filter(|r| r.mode != DocMode::Review)
            .and_then(|r| r.wd.diff.top_line())
        else {
            return;
        };
        if (self.previews.top(pane)).is_some_and(|was| (was - top).abs() < SCROLL_STILL) {
            return;
        }
        if self.scroll_to_browser(pane, top) {
            self.previews.set_top(pane, top);
        }
    }

    /// The file's tab whose page is pane `pane`, with the view's numbers
    /// for its mode.
    fn review_of_page(&mut self, pane: u32) -> Option<(&mut ReviewTab, Env)> {
        let (w, t) = self.pane_place(pane)?;
        let mode = self.workspaces.get(w)?.tabs.get(t)?.review()?.mode;
        let env = self.doc_env(mode);
        let review = self.workspaces[w].tabs[t].review_mut()?;
        Some((review, env))
    }

    /// The page `pane` scrolled to `line` of its file at its top: its
    /// review stands there too.
    pub(crate) fn preview_scrolled(&mut self, pane: u32, line: f64) {
        if !line.is_finite() {
            return;
        }
        let Some((review, env)) = self.review_of_page(pane) else {
            return;
        };
        let top = review.wd.with_diff(env, |diff, ctx| {
            diff.stand_at_line(line, ctx);
            diff.top_line()
        });
        // What the review stands at is what the page shows: not sent back.
        self.previews.set_top(pane, top.unwrap_or(line));
    }

    /// A block clicked in `pane`'s page, from `line` of its file: its
    /// review's caret goes there.
    pub(crate) fn preview_line(&mut self, pane: u32, line: usize) {
        if let Some((review, env)) = self.review_of_page(pane) {
            review.wd.with_diff(env, |diff, ctx| {
                if let Some(row) = diff.row_of_line(line) {
                    diff.caret_to(row, 0, ctx);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::Stage;

    fn mods(ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            shift,
            ..ModifiersState::default()
        }
    }

    #[test]
    fn the_modes_go_round_split_only_on_a_wide_output() {
        let evidence = DocFile::Evidence;
        assert_eq!(DocMode::Preview.next(evidence, true), DocMode::Review);
        assert_eq!(DocMode::Review.next(evidence, true), DocMode::Split);
        assert_eq!(DocMode::Split.next(evidence, true), DocMode::Preview);
        assert_eq!(DocMode::Review.next(evidence, false), DocMode::Preview);
        assert_eq!(DocMode::Split.next(evidence, false), DocMode::Preview);
    }

    #[test]
    fn a_stage_files_modes_go_round_with_no_preview() {
        let plan = DocFile::Stage(Stage::Plan);
        assert_eq!(DocMode::Review.next(plan, true), DocMode::Split);
        assert_eq!(DocMode::Split.next(plan, true), DocMode::Review);
        // Too narrow for Split: nowhere else to go.
        assert_eq!(DocMode::Review.next(plan, false), DocMode::Review);
        assert_eq!(DocMode::Split.next(plan, false), DocMode::Review);
    }

    #[test]
    fn a_tab_saved_on_preview_of_a_stage_file_comes_back_on_its_review() {
        let brief = DocFile::Stage(Stage::Discover);
        assert_eq!(DocMode::Preview.of(brief), DocMode::Review);
        assert_eq!(DocMode::Split.of(brief), DocMode::Split);
        assert_eq!(DocMode::Preview.of(DocFile::Evidence), DocMode::Preview);
    }

    #[test]
    fn a_new_stage_file_tab_opens_split_when_wide_enough() {
        for stage in [Stage::Discover, Stage::Plan] {
            let file = DocFile::Stage(stage);
            assert_eq!(DocMode::opens_on(file, true), DocMode::Split);
            assert_eq!(DocMode::opens_on(file, false), DocMode::Review);
        }
        assert_eq!(DocMode::opens_on(DocFile::Evidence, true), DocMode::Preview);
        assert_eq!(DocMode::opens_on(DocFile::Evidence, false), DocMode::Preview);
    }

    #[test]
    fn a_stage_file_opens_on_its_review_and_the_evidence_on_its_preview() {
        assert_eq!(
            DocMode::default_for(DocFile::Stage(Stage::Discover)),
            DocMode::Review
        );
        assert_eq!(
            DocMode::default_for(DocFile::Stage(Stage::Plan)),
            DocMode::Review
        );
        assert_eq!(DocMode::default_for(DocFile::Evidence), DocMode::Preview);
    }

    #[test]
    fn preview_takes_only_the_keys_acting_on_the_file() {
        let plain = mods(false, false);
        assert!(doc_preview_key(&plain, Keysym::Escape));
        assert!(doc_preview_key(&plain, Keysym::F1));
        assert!(doc_preview_key(&mods(true, true), Keysym::V));
        assert!(doc_preview_key(&mods(true, false), Keysym::Return));
        assert!(doc_preview_key(&mods(true, true), Keysym::Return));
        // The page's own.
        assert!(!doc_preview_key(&plain, Keysym::v));
        assert!(!doc_preview_key(&plain, Keysym::Return));
        assert!(!doc_preview_key(&mods(true, false), Keysym::v));
        assert!(!doc_preview_key(&mods(true, false), Keysym::c));
        assert!(!doc_preview_key(&plain, Keysym::Down));
    }

    #[test]
    fn the_review_takes_its_share_by_mode() {
        assert_eq!(detail_share(DocMode::Preview, true, 0.4), 0.0);
        assert_eq!(detail_share(DocMode::Review, true, 0.4), 1.0);
        assert_eq!(detail_share(DocMode::Split, true, 0.4), 0.4);
        // Kept within its bounds, and whole on a narrow output.
        assert_eq!(detail_share(DocMode::Split, true, 0.99), DOCK_SHARE_MAX);
        assert_eq!(detail_share(DocMode::Split, true, 0.01), DOCK_SHARE_MIN);
        assert_eq!(detail_share(DocMode::Split, false, 0.4), 1.0);
    }

    #[test]
    fn the_switch_says_what_can_be_had() {
        let evidence = DocFile::Evidence;
        let wide = mode_switch(evidence, DocMode::Review, true, false).unwrap();
        assert_eq!(wide.labels, ["Preview", "Review", "Split"]);
        assert_eq!(wide.on, 1);
        assert!(wide.off.is_empty());
        // No room for Split, unless it is what shows.
        let narrow = mode_switch(evidence, DocMode::Preview, false, false).unwrap();
        assert_eq!(narrow.labels, ["Preview", "Review"]);
        let still = mode_switch(evidence, DocMode::Split, false, false).unwrap();
        assert_eq!(still.on, 2);
        // Without the extension only the review is there.
        let off = mode_switch(evidence, DocMode::Review, true, true).unwrap();
        assert_eq!(off.off, [0, 2]);
    }

    #[test]
    fn a_stage_files_switch_has_only_review_and_split() {
        let brief = DocFile::Stage(Stage::Discover);
        let wide = mode_switch(brief, DocMode::Review, true, false).unwrap();
        assert_eq!(wide.labels, ["Review", "Split"]);
        assert_eq!(wide.on, 0);
        let off = mode_switch(brief, DocMode::Review, true, true).unwrap();
        assert_eq!(off.off, [1]);
        // Nothing to go to, so no switch.
        assert_eq!(mode_switch(brief, DocMode::Review, false, false), None);
        // Split showing when the output narrowed: the way back stays.
        let still = mode_switch(brief, DocMode::Split, false, false).unwrap();
        assert_eq!(still.labels, ["Review", "Split"]);
    }

    #[test]
    fn a_tab_goes_by_what_it_holds() {
        let tab = |kind| crate::Tab {
            id: 0,
            layout: None,
            panes: Vec::new(),
            expected: Vec::new(),
            focused: None,
            label: None,
            zoom: None,
            kind,
        };
        let panes = tab(crate::TabKind::Panes);
        assert!(panes.takes_panes() && !panes.is_feature() && panes.review().is_none());
        let feature = tab(crate::TabKind::Feature);
        assert!(!feature.takes_panes() && feature.is_feature() && feature.review().is_none());
        let review = ReviewTab::new(WorktreeDiff::default(), DocMode::Review);
        let file = tab(crate::TabKind::Review(Box::new(review)));
        assert!(!file.takes_panes() && !file.is_feature());
        assert_eq!(file.review().map(|r| r.mode), Some(DocMode::Review));
    }

    #[test]
    fn a_files_tab_is_saved_with_its_file_and_mode() {
        let record = DocRecord {
            file: DocFile::Stage(Stage::Plan),
            mode: DocMode::Split,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<DocRecord>(&json).unwrap(), record);
        assert!(json.contains("\"split\""));
    }
}
