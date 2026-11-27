//! A feature stage's file in a tab of its own: flat across the tab, not a
//! popup. The tab (`TabKind::Review`) shows its review, or that and the file
//! rendered side by side; Verify's evidence, only read, the file rendered
//! too on its own (`DocMode::choices`). A stage's own page shows its file to
//! read, so its tab is for working on it. The switch in the header and
//! Ctrl+Shift+V move between them. It is the same review as ever
//! (`WorktreeDiff`, painted by the repositories panel's own `ReposLayers`),
//! and the file's page (`doc_page`) is painted by gerdr where the flat panel
//! leaves a hole (`ReposLayout::page`).
//!
//! The review owns the keys, but in Preview, where the page scrolls by them
//! and only what acts on the file reaches the review (`page_key`). A click
//! on a block of the page puts the review's caret on the line clicked, and
//! the two scroll together, each standing with the line of the file the
//! other has at its top (`DiffView::top_line`); a caret moved off the page
//! moves the page as little as shows its block. The gap between them drags
//! their split, kept in `state.json`.
//!
//! The tab is saved and comes back on its file and mode; one whose feature
//! is gone doesn't. Esc goes back to the feature's tab and closes the
//! file's, but for one with a comment being written.

use std::path::Path;

use serde::{Deserialize, Serialize};
use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};
use std::time::Instant;

use crate::{
    Drag, Gerdr, Tab, TabKind,
    chat::plan_review,
    chrome::{
        self, DOCK_MIN_WIDTH, DOCK_SHARE_MAX, DOCK_SHARE_MIN, DetailView, DocPageHit,
        DocPageView, ModeSwitch, Rect, ReposButton, ReposView,
    },
    doc_page::{self, DocPage},
    doc_review::{self, DocFile, Failed},
    review::{self, Review},
    scroll::Motion,
    tooltip::Tooltip,
    worktree_diff::{ChatPlanOf, DRAFT_OPEN, DocOf, Env, WorktreeDiff},
};

/// The key going round a file's tab's modes.
pub const PREVIEW_KEY: &str = "Ctrl+Shift+V";

/// How far, in lines, the review may move before its page follows.
const SCROLL_STILL: f64 = 0.01;
/// How far a line key moves the page, in logical pixels; a page key moves
/// it a page less this.
const LINE_STEP: f64 = 40.0;

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
    /// evidence and the fix plan have no page but this, so theirs have all.
    pub fn choices(file: DocFile) -> &'static [DocMode] {
        match file {
            // A chat's plan reads as a card in its chat: Split shows its
            // page beside its comments.
            DocFile::Stage(_) | DocFile::ChatPlan(_) => &[DocMode::Review, DocMode::Split],
            DocFile::Evidence
            | DocFile::Decisions
            | DocFile::Research(_)
            | DocFile::Findings
            | DocFile::FixPlan
            | DocFile::ConflictPlan(_) => &DocMode::ALL,
            DocFile::PlanChanges | DocFile::DraftChanges(..) | DocFile::Suggestions => {
                &[DocMode::Review]
            }
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
            DocFile::Stage(_)
            | DocFile::FixPlan
            | DocFile::ConflictPlan(_)
            | DocFile::ChatPlan(_) => DocMode::Review,
            DocFile::Evidence | DocFile::Decisions | DocFile::Research(_) | DocFile::Findings => {
                DocMode::Preview
            }
            DocFile::PlanChanges | DocFile::DraftChanges(..) | DocFile::Suggestions => {
                DocMode::Review
            }
        }
    }

    /// What a new tab of `file` opens on, on an output `wide` enough for
    /// Split or not: a stage's file or a plan beside its page, so it is read
    /// as it is reviewed. Too narrow, no page starts unseen.
    pub fn opens_on(file: DocFile, wide: bool) -> DocMode {
        match file {
            DocFile::Stage(_)
            | DocFile::FixPlan
            | DocFile::ConflictPlan(_)
            | DocFile::ChatPlan(_)
                if wide =>
            {
                DocMode::Split
            }
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
    /// Where the file's page goes, as the view was laid out last: none in
    /// Review, where it hides.
    pub page: Option<Rect>,
    pub doc: DocPage,
    /// The block of the page the review's caret was in last: the page shows
    /// one it moved to.
    caret: Option<usize>,
    /// Closed by the user: it goes with its panes (see `Gerdr::prune`).
    pub closing: bool,
}

impl ReviewTab {
    pub fn new(wd: WorktreeDiff, mode: DocMode) -> Self {
        ReviewTab {
            mode,
            wd,
            page: None,
            doc: DocPage::default(),
            caret: None,
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

/// What a key does in Preview: moves the page, goes on to the review (only
/// what acts on the file), or nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PageKey {
    /// By as many lines (`LINE_STEP`), down positive.
    Lines(f64),
    /// By as many pages.
    Pages(f64),
    /// To its top, or (`true`) its end.
    End(bool),
    Review,
    Nothing,
}

/// What `sym` does in Preview: the keys a browser's page scrolls by, and
/// what acts on the file.
pub fn page_key(mods: &ModifiersState, sym: Keysym) -> PageKey {
    let shift = mods.shift;
    let plain = !mods.ctrl && !mods.alt && !mods.logo;
    let ctrl = mods.ctrl && !mods.alt && !mods.logo;
    match sym {
        Keysym::Escape | Keysym::F1 if plain && !shift => PageKey::Review,
        Keysym::v | Keysym::V if ctrl && shift => PageKey::Review,
        Keysym::Return | Keysym::KP_Enter if ctrl => PageKey::Review,
        _ if !plain => PageKey::Nothing,
        Keysym::Up | Keysym::KP_Up => PageKey::Lines(-1.0),
        Keysym::Down | Keysym::KP_Down => PageKey::Lines(1.0),
        Keysym::Page_Up | Keysym::KP_Page_Up => PageKey::Pages(-1.0),
        Keysym::Page_Down | Keysym::KP_Page_Down => PageKey::Pages(1.0),
        Keysym::space => PageKey::Pages(if shift { -1.0 } else { 1.0 }),
        Keysym::Home | Keysym::KP_Home => PageKey::End(false),
        Keysym::End | Keysym::KP_End => PageKey::End(true),
        _ => PageKey::Nothing,
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

/// The switch of `file`'s tab: Split only on an output `wide` enough. None
/// with nothing to go to.
pub fn mode_switch(file: DocFile, mode: DocMode, wide: bool) -> Option<ModeSwitch> {
    let shown = (DocMode::choices(file).iter().copied())
        .filter(|m| wide || *m != DocMode::Split || mode == DocMode::Split);
    let shown: Vec<DocMode> = shown.collect();
    (shown.len() > 1).then(|| ModeSwitch {
        labels: shown.iter().map(|m| m.label().to_owned()).collect(),
        on: shown.iter().position(|m| *m == mode).unwrap_or(0),
    })
}

/// The block of `tab`'s page its review's caret is in, and those with
/// comments with what they say, by their index among its blocks.
fn page_marks(tab: &ReviewTab) -> (Option<usize>, Vec<(usize, String)>) {
    let lines = &tab.doc.lines;
    let caret = (tab.wd.diff.cursor_line()).and_then(|line| doc_page::pick_block(lines, line));
    let comments = match tab.wd.doc.as_ref().is_some_and(DocOf::only_read) {
        true => Vec::new(),
        false => tab.wd.diff.comment_lines(),
    };
    let mut said: Vec<(usize, String)> = Vec::new();
    for (line, text) in comments {
        let Some(b) = doc_page::pick_block(lines, line) else {
            continue;
        };
        match said.iter_mut().find(|(at, _)| *at == b) {
            Some((_, all)) => {
                all.push_str("\n\n");
                all.push_str(&text);
            }
            None => said.push((b, text)),
        }
    }
    (caret, said)
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
            _ => detail_share(mode, self.doc_wide(), self.doc_share),
        };
        let (left, ..) = chrome::split_rects(area, share);
        Env {
            page: chrome::diff_only_page(area, self.settings.density),
            cols: chrome::diff_only_cols(left),
            density: self.settings.density,
            sheet_rows: chrome::repos_sheet_rows(area, true),
            tree_page: chrome::diff_tree_page(area, self.settings.density),
            tree_share: 0.0,
        }
    }

    /// What the shown file's tab paints, whatever is over it.
    pub(crate) fn doc_tab_view(&self) -> Option<ReposView> {
        let Some(tab) = self.review_tab() else {
            return self.inline_review_view();
        };
        let file = tab.wd.doc.as_ref()?.file;
        let mut view = tab.wd.repos_view(self.doc_env(tab.mode));
        let wide = self.doc_wide();
        view.flat = true;
        view.detail_share = detail_share(tab.mode, wide, self.doc_share);
        view.modes = mode_switch(file, tab.mode, wide);
        if tab.mode == DocMode::Preview {
            view.detail = DetailView::default();
            (view.hints)
                .retain(|h| !matches!(h.button, ReposButton::Comment | ReposButton::CopyLines));
        }
        Some(view)
    }

    /// Bring the shown file's tab up to date, every frame: read when it
    /// comes to show; its page laid out where the view leaves room for it,
    /// standing where its review does.
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
        // No file's tab: a feature's review on its page paints the same way.
        if shown.is_none() {
            return self.sync_inline_review();
        }
        self.doc_stand();
        let workspace = self.active_workspace;
        let env = self.doc_env(self.review_tab()?.mode);
        let review = self.review_tab_mut()?;
        review.wd.workspace = workspace;
        // Its lines wrap at the width it shows in, as the gap is dragged too.
        review.wd.with_diff(env, |diff, ctx| diff.rewrap(ctx));
        let view = self.doc_tab_view()?;
        let page = chrome::repos_layout(&view, self.content_rect()).page;
        let review = self.review_tab_mut()?;
        review.page = page;
        let doc = review.wd.doc.as_ref()?;
        let text = doc.text.as_deref().ok_or("The file can't be read");
        let dir = doc.path.parent().map(Path::to_path_buf);
        review.doc.set_text(text, dir.as_deref());
        self.sync_doc_scroll(env);
        Some(view)
    }

    /// Show `mode` in the shown file's tab: a draft comment stays on show.
    pub(crate) fn set_doc_mode(&mut self, mode: DocMode) {
        let Some(review) = self.review_tab() else {
            return;
        };
        if review.mode == mode {
            return;
        }
        if mode == DocMode::Preview && review.wd.diff.editing.is_some() {
            return self.show_warning(DRAFT_OPEN.into());
        }
        if let Some(review) = self.review_tab_mut() {
            review.mode = mode;
        }
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

    /// Whether this press goes to the next of a file's tab's modes: Ctrl+Shift+V
    /// is a paste only while a comment or the search is being typed.
    pub(crate) fn doc_cycles(&self, mods: &ModifiersState, sym: Keysym) -> bool {
        let v = matches!(sym, Keysym::v | Keysym::V);
        v && mods.ctrl
            && mods.shift
            && !(mods.alt || mods.logo)
            && self
                .review_tab()
                .is_some_and(|tab| tab.wd.diff.editing.is_none() && !tab.wd.diff.search_focused())
    }

    /// Whether the shown file's tab has the keys now: a feature's review on
    /// its page once pressed on, a file's tab always.
    pub(crate) fn doc_tab_takes_keys(&self) -> bool {
        if let Some(wd) = self.inline_wd() {
            let rail = self.features.panel.as_ref().is_some_and(|p| p.rail_keys);
            return !rail || wd.sheet(self.inline_env()).is_some();
        }
        self.review_tab().is_some()
    }

    /// Whether the shown file's tab takes the pointer at `pos`.
    pub(crate) fn doc_tab_covers(&self, pos: Point<f64, Logical>) -> bool {
        match self.review_tab() {
            Some(_) => self.content_rect().to_f64().contains(pos),
            None => self.inline_wd().is_some() && self.review_area().to_f64().contains(pos),
        }
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

    /// Show plan item `item_id` of pane `pane_id`'s chat in a file's tab of
    /// the pane's workspace, to comment on and send back
    /// (`chat::plan_review`): one already open as it stands.
    pub(crate) fn open_chat_plan(&mut self, pane_id: &str, item_id: &str) {
        let Some(text) = self.chat_plan_text(pane_id, item_id) else {
            return self.show_warning("That plan is gone from the chat".into());
        };
        let Some((w, _)) = self.herdr_pane_place(pane_id) else {
            return self.show_warning("That agent's pane is gone".into());
        };
        let key = plan_review::key(item_id);
        let file = DocFile::ChatPlan(key);
        let open = self.doc_tab_in(w, file);
        if open.is_none() {
            plan_review::tidy(key);
        }
        let dir = (self.chat_agent(pane_id).and_then(|a| a.cwd.clone()))
            .unwrap_or_else(plan_review::dir);
        self.close_panels_for_tab();
        let t = open.unwrap_or_else(|| {
            let chat = ChatPlanOf {
                pane_id: pane_id.into(),
                item_id: item_id.into(),
                waits: self.plan_ask_waits(pane_id, item_id),
            };
            let project = self.project.clone().unwrap_or_default();
            let doc = doc_review::chat_plan_doc(&project, key, chat, &text, &dir);
            self.insert_doc_tab(w, doc, text, DocMode::opens_on(file, self.doc_wide()))
        });
        self.switch_workspace(w);
        self.switch_tab(t);
        self.refresh_pointer_focus();
    }

    /// The panels a tab coming to show would sit under close, the
    /// repositories panel kept as it was.
    pub(crate) fn close_panels_for_tab(&mut self) {
        self.overlays.switcher = None;
        self.overlays.project_menu = None;
        self.overlays.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.overlays.scrollback = None;
        self.worktrees.close();
        self.stop_key_repeat();
        self.close_menu();
        self.hover.tooltip.hide();
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

    /// Make `doc`, read as `text`, a tab of workspace `w` in `mode`, after
    /// the others, so none is numbered again; what is shown kept shown.
    /// Where it went.
    fn insert_doc_tab(&mut self, w: usize, doc: DocOf, text: String, mode: DocMode) -> usize {
        let review = match doc.file {
            DocFile::Stage(_)
            | DocFile::FixPlan
            | DocFile::ConflictPlan(_)
            | DocFile::ChatPlan(_) => review::load_file(&doc.comments),
            DocFile::Evidence
            | DocFile::Decisions
            | DocFile::Research(_)
            | DocFile::Findings
            | DocFile::PlanChanges
            | DocFile::DraftChanges(..)
            | DocFile::Suggestions => Review::default(),
        };
        let mut wd = WorktreeDiff::of_doc(doc, w, review);
        wd.set_doc(Ok(text), self.doc_env(mode));
        let ws = &mut self.workspaces[w];
        let t = ws.tabs.len();
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
        // A chat's plan isn't brought back: its card opens it again, its
        // comments kept.
        if let DocFile::ChatPlan(_) = saved.file {
            return false;
        }
        let (Some(project), Some(slug)) = (
            self.project.clone(),
            self.workspace_feature(w).map(|f| f.slug.clone()),
        ) else {
            return false;
        };
        let (doc, text) = match self.read_doc(&project, &slug, saved.file) {
            Ok(read) => read,
            // Its file gone or unreadable, the tab goes: nothing to show.
            Err(failed) => {
                let why = match failed {
                    Failed::Warning(why) | Failed::Error(why) => why,
                };
                let file = saved.file;
                tracing::info!("not bringing back the {file:?} tab of {project}/{slug}: {why}");
                return false;
            }
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

    /// The tabs of feature `slug`'s files that `done` says are done with
    /// close, but one with a comment being written: once its stage is
    /// approved its files are only read, and the stage's page opens them
    /// again. One shown leaves the feature's tab showing.
    pub(crate) fn close_done_doc_tabs(
        &mut self,
        project: &str,
        slug: &str,
        done: impl Fn(DocFile) -> bool,
    ) {
        let Some(w) = self.feature_workspace(project, slug) else {
            return;
        };
        let ws = &self.workspaces[w];
        let ids: Vec<u64> = (ws.tabs.iter())
            .filter(|tab| {
                tab.review().is_some_and(|r| {
                    r.wd.diff.editing.is_none() && r.wd.doc.as_ref().is_some_and(|d| done(d.file))
                })
            })
            .map(|tab| tab.id)
            .collect();
        let shown = ws
            .tabs
            .get(ws.active_tab)
            .is_some_and(|t| ids.contains(&t.id));
        if shown {
            self.workspaces[w].active_tab = 0;
        }
        for id in ids {
            if let Some((w, t)) = self.tab_place(id) {
                self.close_tab_in(w, t);
            }
        }
        if shown && w == self.active_workspace {
            self.switch_tab(0);
        }
    }

    /// The file's tab `idx` of workspace `w` is closed for good: it goes
    /// once its page's window does, and what its vim edits could undo goes
    /// with it, since its agent may write the file meanwhile.
    pub(crate) fn doc_tab_closing(&mut self, w: usize, idx: usize) {
        let Some(review) = (self.workspaces.get_mut(w))
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
        if self.overlays.context_menu.is_some() || self.grabbing_overlay().is_some() {
            return None;
        }
        let view = self.doc_tab_view()?;
        let layout = chrome::repos_layout(&view, self.review_area());
        if layout.sheet.is_some() {
            return None;
        }
        // Beside a feature's review, its file list.
        if layout.splitter.to_f64().contains(pos) {
            return Some(Drag::ReposSplit);
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
        self.doc_share = share.clamp(DOCK_SHARE_MIN, DOCK_SHARE_MAX);
    }

    /// Where the repositories panel goes: centred in the output; the
    /// review of a file's tab across the tab, with no panel over it.
    pub fn repos_rect(&self) -> Rect {
        let flat = self.review_tab().is_some() || self.inline_wd().is_some();
        match !self.repos.is_open() && flat {
            true => self.review_area(),
            false => chrome::repos_panel_rect(self.output_size()),
        }
    }

    /// Where a review laid flat into its tab goes: across the tab, but for
    /// a feature's on its page, under the rail.
    pub(crate) fn review_area(&self) -> Rect {
        let on_page = self.inline_wd().and(self.features.panel.as_ref());
        (on_page.and_then(|panel| panel.review_rect)).unwrap_or_else(|| self.content_rect())
    }

    /// The shown file's tab's page, as the chrome paints it: none in
    /// Review, where it hides.
    pub(crate) fn doc_page_view(&self) -> Option<DocPageView> {
        let tab = self.review_tab()?;
        let (caret, said) = page_marks(tab);
        Some(DocPageView {
            rect: tab.page?,
            blocks: tab.doc.blocks.clone(),
            top: tab.doc.top(),
            caret,
            said: said.into_iter().map(|(b, _)| b).collect(),
        })
    }

    /// Stand the shown file's page and its review together: the one moved
    /// since they last stood so takes the other along, the review when
    /// both did (as a new text has the page stand where its review does).
    /// Then a caret moved to a block out of view moves the page as little
    /// as shows it, and the review stays where it is.
    fn sync_doc_scroll(&mut self, env: Env) {
        let Some(tab) = self.review_tab_mut() else {
            return;
        };
        let Some(rect) = tab.page else {
            tab.doc.stood = None;
            tab.caret = None;
            return;
        };
        let (h, spans) = chrome::doc_page_spans(&tab.doc.blocks, rect);
        let b = doc_page::bounds(h, f64::from(rect.size.h));
        let lines = tab.doc.lines.clone();
        let page_line = |doc: &DocPage| doc_page::y_line(&lines, &spans, doc.top());
        let review = tab.wd.diff.top_line();
        let moved = |was: f64, now: f64| (was - now).abs() >= SCROLL_STILL;
        let (page, review) = match (tab.doc.stood, review) {
            (Some((p, r)), Some(now)) if !moved(r, now) && moved(p, page_line(&tab.doc)) => {
                let line = page_line(&tab.doc);
                let top = tab.wd.with_diff(env, |diff, ctx| {
                    diff.stand_at_line(line, ctx);
                    diff.top_line()
                });
                (line, top.unwrap_or(line))
            }
            (Some((p, r)), Some(now)) if !moved(r, now) => (p, r),
            (_, Some(now)) => {
                let y = doc_page::line_y(&lines, &spans, now);
                tab.doc.scroll.jump(y.clamp(0.0, b.max));
                (page_line(&tab.doc), now)
            }
            (_, None) => (page_line(&tab.doc), 1.0),
        };
        tab.doc.stood = Some((page, review));
        let caret = page_marks(tab).0;
        if std::mem::replace(&mut tab.caret, caret) == caret {
            return;
        }
        let Some(&(top, bottom)) = caret.and_then(|c| spans.get(c)) else {
            return;
        };
        // The page's content starts this far down it.
        let (top, bottom) = (f64::from(top), f64::from(bottom) + chrome::DOC_PAGE_ROOM);
        let (shown, view) = (tab.doc.top(), f64::from(rect.size.h) - chrome::DOC_PAGE_ROOM);
        let to = match () {
            _ if top < shown => top,
            _ if bottom > shown + view => (bottom - view).min(top),
            _ => return,
        };
        tab.doc.scroll.jump(to.clamp(0.0, b.max));
        tab.doc.stood = Some((page_line(&tab.doc), review));
    }

    /// Move the shown file's page on to `now`. Whether it still moves.
    pub(crate) fn doc_page_scroll_tick(&mut self, now: Instant) -> bool {
        let Some(tab) = self.review_tab_mut() else {
            return false;
        };
        let Some(rect) = tab.page else {
            return false;
        };
        let (h, _) = chrome::doc_page_spans(&tab.doc.blocks, rect);
        tab.doc.scroll.tick(now, doc_page::bounds(h, f64::from(rect.size.h)))
    }

    /// The wheel or the touchpad moved the shown file's page, if `pos` is
    /// on it. Whether it was.
    pub(crate) fn doc_page_scroll(&mut self, pos: Point<f64, Logical>, motion: Motion) -> bool {
        let Some(tab) = self.review_tab_mut() else {
            return false;
        };
        let Some(rect) = tab.page.filter(|r| r.to_f64().contains(pos)) else {
            return false;
        };
        let (h, _) = chrome::doc_page_spans(&tab.doc.blocks, rect);
        doc_page::scroll(&mut tab.doc, motion, doc_page::bounds(h, f64::from(rect.size.h)));
        true
    }

    /// A key in Preview: the page moves by it, or it goes on to the review
    /// (only what acts on the file), or it does nothing. Whether the page
    /// took it.
    pub(crate) fn doc_page_key(&mut self, mods: &ModifiersState, sym: Keysym) -> bool {
        let env = self.doc_env(DocMode::Preview);
        let Some(tab) = self.review_tab_mut() else {
            return false;
        };
        if tab.mode != DocMode::Preview || tab.wd.sheet(env).is_some() {
            return false;
        }
        let Some(rect) = tab.page else {
            return false;
        };
        let (h, _) = chrome::doc_page_spans(&tab.doc.blocks, rect);
        let b = doc_page::bounds(h, f64::from(rect.size.h));
        let page = (f64::from(rect.size.h) - LINE_STEP).max(LINE_STEP);
        let top = tab.doc.scroll.pos();
        let to = match page_key(mods, sym) {
            PageKey::Review => return false,
            PageKey::Nothing => return true,
            PageKey::Lines(n) => top + n * LINE_STEP,
            PageKey::Pages(n) => top + n * page,
            PageKey::End(end) => match end {
                true => b.max,
                false => 0.0,
            },
        };
        tab.doc.scroll.glide_to(to.clamp(0.0, b.max), Instant::now(), b);
        true
    }

    /// A press at `pos` on the shown file's page, if it is on it: a link
    /// opens, a code block's "Copy" copies it, and a block puts the review's
    /// caret on the line pressed. The page stays where it is, and so does
    /// the review: a double click or a drag would land on what moved under
    /// it. Whether it was on the page.
    pub(crate) fn doc_page_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(view) = self.doc_page_view() else {
            return false;
        };
        if !view.rect.to_f64().contains(pos) {
            return false;
        }
        let Some(hit) = chrome::doc_page_hit(&view, pos) else {
            return true;
        };
        let (b, y) = match hit {
            DocPageHit::Link(url) => {
                self.doc_page_link(url);
                return true;
            }
            DocPageHit::Copy(text) => {
                let note = crate::page_text::copied_note(&text);
                self.copy_text(text);
                self.show_success(note);
                return true;
            }
            DocPageHit::Said(b) => (b, None),
            DocPageHit::Block(b, y) => (b, Some(y)),
        };
        let env = self.doc_env(self.review_tab().map_or(DocMode::Review, |t| t.mode));
        let rect = view.rect;
        let Some(tab) = self.review_tab_mut() else {
            return true;
        };
        let (_, spans) = chrome::doc_page_spans(&tab.doc.blocks, rect);
        let lines = tab.doc.lines.clone();
        let line = match y {
            Some(y) => doc_page::clicked_line(&lines, &spans, b, y),
            None => lines.get(b).map_or(1, |l| l.0),
        };
        let top = tab.wd.with_diff(env, |diff, ctx| {
            if let Some(row) = diff.row_of_line(line) {
                diff.caret_to(row, 0, ctx);
            }
            diff.top_line()
        });
        let page = doc_page::y_line(&lines, &spans, tab.doc.top());
        tab.doc.stood = top.map(|top| (page, top));
        tab.caret = page_marks(tab).0;
        true
    }

    /// Link `url` pressed on the shown file's page: a web page opens in a
    /// tab of the workspace, a file in Zed.
    fn doc_page_link(&mut self, url: String) {
        if crate::is_web_link(&url) {
            return self.open_page(self.active_workspace, url);
        }
        if let Some(file) = crate::md_blocks::file_target(&url, None) {
            self.open_linked_file(file);
        }
    }

    /// What the pointer at `pos` over the shown file's page offers: a dot's
    /// comments. `None` when it isn't on the page.
    pub(crate) fn doc_page_hover(&mut self, pos: Point<f64, Logical>) -> Option<Option<Tooltip>> {
        let view = self.doc_page_view()?;
        if !view.rect.to_f64().contains(pos) {
            return None;
        }
        if let Some(tab) = self.review_tab_mut() {
            tab.wd.hover = None;
        }
        let Some(DocPageHit::Said(b)) = chrome::doc_page_hit(&view, pos) else {
            return Some(None);
        };
        let said = page_marks(self.review_tab()?).1;
        let text = said.into_iter().find(|(at, _)| *at == b)?.1;
        let at = Rect::new(pos.to_i32_round(), (1, 1).into());
        Some(Some(Tooltip::new(&text, at)))
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
        let brief = DocFile::Stage(Stage::Discover);
        let plan = DocFile::Stage(Stage::Plan);
        for file in [brief, plan, DocFile::FixPlan, DocFile::ConflictPlan(1)] {
            assert_eq!(DocMode::opens_on(file, true), DocMode::Split);
            assert_eq!(DocMode::opens_on(file, false), DocMode::Review);
        }
        assert_eq!(DocMode::opens_on(DocFile::Evidence, true), DocMode::Preview);
        assert_eq!(
            DocMode::opens_on(DocFile::Evidence, false),
            DocMode::Preview
        );
    }

    #[test]
    fn a_chat_plan_has_review_and_split_and_opens_beside_its_page() {
        let plan = DocFile::ChatPlan(7);
        assert_eq!(DocMode::choices(plan), [DocMode::Review, DocMode::Split]);
        assert_eq!(DocMode::opens_on(plan, true), DocMode::Split);
        assert_eq!(DocMode::opens_on(plan, false), DocMode::Review);
        assert_eq!(DocMode::Preview.of(plan), DocMode::Review);
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
    fn preview_scrolls_by_a_pages_keys_and_passes_on_what_acts_on_the_file() {
        let plain = mods(false, false);
        let shift = ModifiersState {
            shift: true,
            ..plain
        };
        assert_eq!(page_key(&plain, Keysym::Escape), PageKey::Review);
        assert_eq!(page_key(&plain, Keysym::F1), PageKey::Review);
        assert_eq!(page_key(&mods(true, true), Keysym::V), PageKey::Review);
        assert_eq!(page_key(&mods(true, false), Keysym::Return), PageKey::Review);
        assert_eq!(page_key(&mods(true, true), Keysym::Return), PageKey::Review);
        // The page's own.
        assert_eq!(page_key(&plain, Keysym::Down), PageKey::Lines(1.0));
        assert_eq!(page_key(&plain, Keysym::Page_Up), PageKey::Pages(-1.0));
        assert_eq!(page_key(&shift, Keysym::space), PageKey::Pages(-1.0));
        assert_eq!(page_key(&plain, Keysym::End), PageKey::End(true));
        // What would act on the hidden review does nothing.
        assert_eq!(page_key(&plain, Keysym::v), PageKey::Nothing);
        assert_eq!(page_key(&plain, Keysym::Return), PageKey::Nothing);
        assert_eq!(page_key(&mods(true, false), Keysym::v), PageKey::Nothing);
        assert_eq!(page_key(&mods(true, false), Keysym::Down), PageKey::Nothing);
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

    /// An output wide enough for Split, and one too narrow.
    const WIDE: bool = true;
    const NARROW: bool = false;

    #[test]
    fn the_switch_says_what_can_be_had() {
        let evidence = DocFile::Evidence;
        let wide = mode_switch(evidence, DocMode::Review, WIDE).unwrap();
        assert_eq!(wide.labels, ["Preview", "Review", "Split"]);
        assert_eq!(wide.on, 1);
        // No room for Split, unless it is what shows.
        let narrow = mode_switch(evidence, DocMode::Preview, NARROW).unwrap();
        assert_eq!(narrow.labels, ["Preview", "Review"]);
        let still = mode_switch(evidence, DocMode::Split, NARROW).unwrap();
        assert_eq!(still.on, 2);
    }

    #[test]
    fn a_stage_files_switch_has_only_review_and_split() {
        let brief = DocFile::Stage(Stage::Discover);
        let wide = mode_switch(brief, DocMode::Review, WIDE).unwrap();
        assert_eq!(wide.labels, ["Review", "Split"]);
        assert_eq!(wide.on, 0);
        // Nothing to go to, so no switch.
        assert_eq!(mode_switch(brief, DocMode::Review, NARROW), None);
        // Split showing when the output narrowed: the way back stays.
        let still = mode_switch(brief, DocMode::Split, NARROW).unwrap();
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
            recent: Vec::new(),
            label: None,
            zoom: None,
            kind,
            last_shown: None,
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
