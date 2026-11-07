//! The stats panel: what the features' AI work came to, read from its index
//! (see `ai_work::stats`), by feature, by agent and by check, and one
//! feature in detail.
//!
//! It is read off the loop (see `Gerdr::feature_work`): on opening, on a
//! change of tab, scope or feature, and after Ctrl+R has the index drawn
//! again. A transcript or log it opens is unpacked off the loop too, then
//! shown in the scrollback viewer in its place.
//!
//! Until gerdr holds the panel itself, `Chrome::ai_stats` does, so the
//! chrome paints it from there (see `chrome::stats`).

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    Gerdr,
    ai_work::{
        AgentKind, index,
        stats::{self, Scope, Tab, View},
        thread::Job,
        zst,
    },
    chrome::{self, StatsColumn, StatsFocus, StatsHit, StatsRow, StatsView},
    features::Done,
    input::Overlay,
    projects::ago,
    scrollback::{Scrollback, Source},
    text_field::{Edit, PASTE, TextField},
    toast::ToastHit,
    tooltip::Tooltip,
};

/// The tabs, in the order the segmented control shows them.
const TABS: [Tab; 3] = [Tab::Features, Tab::Agents, Tab::Checks];

/// Every key the panel takes, as F1 lists them.
pub(crate) const KEYS: &[(&str, &str)] = &[
    ("Up / Down", "Move the selection"),
    ("Ctrl+J / Ctrl+K", "Move the selection down or up"),
    ("PgUp / PgDn", "Move a page up or down"),
    ("Home / End", "Go to the first or last row"),
    (
        "Tab / Shift+Tab",
        "Go between the tabs, the scope and the list",
    ),
    ("Left / Right", "Pick the tab or the scope that has focus"),
    ("Ctrl+PgUp / Ctrl+PgDn", "Go to the previous or next tab"),
    ("Enter", "Show the feature's stages, runs and fix rounds"),
    ("Ctrl+F / /", "Search the rows"),
    ("Ctrl+O", "Open the run's transcript or the check's log"),
    ("Ctrl+C", "Copy the row"),
    ("Ctrl+R", "Draw the index again from the kept files"),
    ("Esc", "Clear the search, go back, then close"),
    ("F1", "List these keys"),
];

/// What the panel asks of gerdr, once a key or a click is taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Close,
    /// Read the index for the panel's scope.
    Read,
    /// Show the file at `path`, `what` it is, of `title`.
    Open {
        path: PathBuf,
        what: String,
        title: String,
    },
    /// The selected row has nothing to open.
    NothingToOpen,
    /// Put the row's text on the clipboard.
    Copy(String),
    /// Draw the index again from the files.
    Rebuild,
}

/// A row of the list, as the panel works it out from the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    /// The columns it is laid out in, by index into `columns`.
    pub set: usize,
    pub cells: Vec<String>,
    /// A section's heading in a feature's detail: it names the columns
    /// under it, and can't be selected.
    pub heading: bool,
    /// What Ctrl+O opens on it: a run's transcript, a check's log.
    pub open: Option<(PathBuf, String)>,
    /// The feature Enter shows in detail (project, slug).
    pub drill: Option<(String, String)>,
}

impl Line {
    fn row(set: usize, cells: Vec<String>) -> Self {
        Self {
            set,
            cells,
            heading: false,
            open: None,
            drill: None,
        }
    }

    fn heading(set: usize, cells: Vec<String>) -> Self {
        Self {
            heading: true,
            ..Self::row(set, cells)
        }
    }
}

/// A list left for a feature's detail: what was read for it, its
/// selection and its first row shown.
type Back = (Option<Result<View, String>>, usize, usize);

/// The stats panel while it is open.
#[derive(Debug, Default)]
pub(crate) struct StatsPanel {
    /// What is asked of the index.
    pub scope: Scope,
    /// The open project (id, name); `None` without one.
    pub project: Option<(String, String)>,
    /// What the index held for `scope` when last read, or why it couldn't
    /// be read.
    pub view: Option<Result<View, String>>,
    /// A read is under way.
    pub reading: bool,
    /// The selected row, by index into `lines`.
    pub selected: usize,
    /// The first row shown.
    pub first: usize,
    pub focus: StatsFocus,
    pub query: TextField,
    /// The query has the keys.
    pub searching: bool,
    /// Every key listed in place of the rows (F1).
    pub keys: bool,
    /// What the pointer is over.
    pub hover: Option<StatsHit>,
    /// The Features list as it was before Enter showed a feature, to go
    /// back to.
    back: Option<Back>,
}

impl StatsPanel {
    /// The panel on the Features tab of the open `project` (id, name), or of
    /// all of them without one.
    pub fn new(project: Option<(String, String)>) -> Self {
        Self {
            scope: Scope {
                tab: Tab::Features,
                project: project.as_ref().map(|(id, _)| id.clone()),
                detail: None,
            },
            project,
            ..Self::default()
        }
    }

    /// What the index held for the scope as it is now: nothing while the
    /// read for it is still to come.
    fn current(&self) -> Option<&Result<View, String>> {
        self.view
            .as_ref()
            .filter(|v| v.as_ref().is_err() || v.as_ref().is_ok_and(|v| v.scope == self.scope))
    }

    /// The column sets of the scope shown: one for a tab, one per section
    /// of a feature's detail. A width of 0 takes the room left.
    pub fn columns(&self) -> Vec<Vec<StatsColumn>> {
        let col = |title: &str, width: i32, right: bool| StatsColumn {
            title: title.to_owned(),
            width,
            right,
        };
        if self.scope.detail.is_some() {
            return vec![
                vec![
                    col("Stages", 0, false),
                    col("Started", 110, false),
                    col("Ended", 110, false),
                    col("Took", 80, true),
                ],
                vec![
                    col("Runs", 110, false),
                    col("Agent", 90, false),
                    col("Model", 0, false),
                    col("Took", 64, true),
                    col("Tokens", 96, true),
                    col("Nudges", 56, true),
                    col("Sent back", 70, true),
                ],
                vec![
                    col("Fix rounds", 0, false),
                    col("Opened", 110, false),
                    col("Closed", 110, false),
                    col("Fixes", 80, true),
                ],
                vec![
                    col("Verify runs", 0, false),
                    col("Started", 110, false),
                    col("Took", 64, true),
                    col("Passed", 60, true),
                    col("Failed", 60, true),
                ],
            ];
        }
        vec![match self.scope.tab {
            Tab::Features => vec![
                col("Feature", 0, false),
                col("Status", 84, false),
                col("Made", 76, false),
                col("AI time", 64, true),
                col("Runs", 44, true),
                col("Fix rounds", 70, true),
                col("Tokens in / out", 104, true),
                col("Nudges", 56, true),
                col("Sent back", 70, true),
            ],
            Tab::Agents => vec![
                col("Stage", 76, false),
                col("Agent", 84, false),
                col("Model", 0, false),
                col("Runs", 40, true),
                col("Median", 56, true),
                col("Tokens", 56, true),
                col("Nudged", 56, true),
                col("Sent back", 66, true),
                col("Findings", 96, false),
                col("Fixes", 44, true),
                col("Verify", 52, true),
            ],
            Tab::Checks => vec![
                col("Repository", 150, false),
                col("Command", 0, false),
                col("Runs", 48, true),
                col("Failing", 64, true),
                col("Median", 64, true),
                col("Last failed", 96, false),
            ],
        }]
    }

    /// The rows of the scope shown at `now` (seconds since the epoch),
    /// those the query matches.
    pub fn lines(&self, now: u64) -> Vec<Line> {
        let Some(Ok(view)) = self.current() else {
            return Vec::new();
        };
        let all = self.scope.project.is_none();
        let lines = match (&view.detail, self.scope.detail.is_some()) {
            (Some(detail), true) => detail_lines(detail, now),
            (None, true) => Vec::new(),
            _ => match self.scope.tab {
                Tab::Features => view
                    .features
                    .iter()
                    .map(|f| feature_line(f, all, now))
                    .collect(),
                Tab::Agents => view.agents.iter().map(agent_line).collect(),
                Tab::Checks => view.checks.iter().map(|c| check_line(c, now)).collect(),
            },
        };
        filtered(lines, &self.query)
    }

    /// What the index holds nothing of, or couldn't be read, said in place
    /// of the rows; `None` with rows to show. The bool: it is an error.
    pub fn note(&self, now: u64) -> Option<(String, bool)> {
        let Some(read) = self.current() else {
            return Some(("Reading…".into(), false));
        };
        let view = match read {
            Err(err) => return Some((format!("Couldn't read the stats: {err}"), true)),
            Ok(view) => view,
        };
        if !self.lines(now).is_empty() {
            return None;
        }
        if !self.query.is_empty() {
            let query = &*self.query;
            return Some((format!("Nothing matches \u{201c}{query}\u{201d}"), false));
        }
        let nothing = view.features.is_empty()
            && view.agents.is_empty()
            && view.checks.is_empty()
            && view.detail.is_none();
        let note = match (self.scope.detail.is_some(), self.scope.tab) {
            (true, _) => "No AI work kept for this feature yet",
            (false, _) if nothing => "No AI work kept yet",
            (false, Tab::Features) => "No features with AI work kept yet",
            (false, Tab::Agents) => "No agent runs kept yet",
            (false, Tab::Checks) => "No check runs kept yet",
        };
        Some((note.into(), false))
    }

    /// The selected row, if one can be.
    fn selected_line(&self, now: u64) -> Option<Line> {
        self.lines(now)
            .into_iter()
            .nth(self.selected)
            .filter(|l| !l.heading)
    }

    /// Put the selection on a row that can be selected, as near `at` as
    /// one is, going `down` first; and the first row shown where the
    /// selection shows, `page` rows at a time.
    fn settle(&mut self, at: usize, down: bool, page: usize, now: u64) {
        let lines = self.lines(now);
        if lines.is_empty() {
            self.selected = 0;
            self.first = 0;
            return;
        }
        let at = at.min(lines.len() - 1);
        let pick = |down: bool| {
            if down {
                (at..lines.len()).find(|&i| !lines[i].heading)
            } else {
                (0..=at).rev().find(|&i| !lines[i].heading)
            }
        };
        self.selected = pick(down).or_else(|| pick(!down)).unwrap_or(0);
        let page = page.max(1);
        // The heading over a section's first row shows with it.
        let top = match self.selected {
            i if i > 0 && lines[i - 1].heading => i - 1,
            i => i,
        };
        if top < self.first {
            self.first = top;
        } else if self.selected >= self.first + page {
            self.first = self.selected + 1 - page;
        }
        self.first = self.first.min(lines.len().saturating_sub(page));
    }

    /// Move the selection by `by` rows, `page` showing.
    fn step(&mut self, by: isize, page: usize, now: u64) {
        let at = self.selected.saturating_add_signed(by);
        self.settle(at, by >= 0, page, now);
    }

    /// Scroll the list by `rows`. The selection stays on a row shown, so
    /// Enter never acts on one out of sight.
    pub fn scroll_by(&mut self, rows: isize, page: usize, now: u64) {
        let page = page.max(1);
        let n = self.lines(now).len();
        let last = n.saturating_sub(page);
        self.first = self.first.saturating_add_signed(rows).min(last);
        if self.selected < self.first {
            self.settle(self.first, true, page, now);
        } else if self.selected >= self.first + page {
            self.settle(self.first + page - 1, false, page, now);
        }
    }

    /// Show tab `tab`, its list from the top.
    fn show_tab(&mut self, tab: Tab) -> Option<Outcome> {
        if self.scope.tab == tab && self.scope.detail.is_none() {
            return None;
        }
        self.scope.tab = tab;
        self.scope.detail = None;
        self.back = None;
        self.selected = 0;
        self.first = 0;
        Some(Outcome::Read)
    }

    /// The tab `by` along from this one, round the ends.
    fn step_tab(&mut self, by: isize) -> Option<Outcome> {
        let i = TABS.iter().position(|t| *t == self.scope.tab).unwrap_or(0);
        let n = TABS.len() as isize;
        self.show_tab(TABS[(i as isize + by).rem_euclid(n) as usize])
    }

    /// Show every project's AI work, or the open one's.
    fn show_all(&mut self, all: bool) -> Option<Outcome> {
        let project = match all {
            true => None,
            false => Some(self.project.as_ref()?.0.clone()),
        };
        if self.scope.project == project {
            return None;
        }
        self.scope.project = project;
        self.scope.detail = None;
        self.back = None;
        self.selected = 0;
        self.first = 0;
        Some(Outcome::Read)
    }

    /// Show the selected feature in detail, if it is one.
    fn drill(&mut self, now: u64) -> Option<Outcome> {
        let (project, slug) = self.selected_line(now)?.drill?;
        let view = self.view.take();
        self.back = Some((view, self.selected, self.first));
        self.scope.detail = Some((project, slug));
        self.query.clear();
        self.searching = false;
        self.selected = 0;
        self.first = 0;
        Some(Outcome::Read)
    }

    /// Go back from a feature's detail to the list it was picked in, as it
    /// was then.
    fn leave_detail(&mut self) {
        self.scope.detail = None;
        self.query.clear();
        self.searching = false;
        if let Some((view, selected, first)) = self.back.take() {
            self.view = view;
            self.selected = selected;
            self.first = first;
        }
    }

    /// What Ctrl+O opens on the selected row.
    fn open(&self, now: u64) -> Outcome {
        let Some(line) = self.selected_line(now) else {
            return Outcome::NothingToOpen;
        };
        match line.open {
            Some((path, what)) => Outcome::Open {
                path,
                what,
                title: line.cells.first().cloned().unwrap_or_default(),
            },
            None => Outcome::NothingToOpen,
        }
    }

    /// What Esc does now, as the footer says it: "clear", "back" or "close".
    pub fn esc_word(&self) -> &'static str {
        if !self.query.is_empty() {
            "clear"
        } else if self.searching || self.scope.detail.is_some() {
            "back"
        } else {
            "close"
        }
    }

    /// A key, `typed` with Shift and the layout applied, with `page` rows
    /// showing, at `now`.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        page: usize,
        now: u64,
    ) -> Option<Outcome> {
        let bare = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        let ctrl = mods.ctrl && !(mods.alt || mods.logo);
        // The keys listed take only what closes them.
        if self.keys {
            if bare && matches!(sym, Keysym::Escape | Keysym::F1 | Keysym::Return) {
                self.keys = false;
            }
            return None;
        }
        let page = page.max(1);
        match sym {
            Keysym::F1 if bare => {
                self.keys = true;
                return None;
            }
            Keysym::Escape => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.settle(0, true, page, now);
                } else if self.searching {
                    self.searching = false;
                } else if self.scope.detail.is_some() {
                    self.leave_detail();
                    self.settle(self.selected, true, page, now);
                } else {
                    return Some(Outcome::Close);
                }
                return None;
            }
            Keysym::Tab | Keysym::ISO_Left_Tab if !mods.ctrl => {
                let back = mods.shift || typed == Keysym::ISO_Left_Tab;
                self.searching = false;
                self.focus = self.focus_step(back);
                return None;
            }
            Keysym::Page_Up | Keysym::KP_Page_Up if ctrl => return self.step_tab(-1),
            Keysym::Page_Down | Keysym::KP_Page_Down if ctrl => return self.step_tab(1),
            Keysym::Up | Keysym::KP_Up => self.step(-1, page, now),
            Keysym::Down | Keysym::KP_Down => self.step(1, page, now),
            Keysym::k if ctrl => self.step(-1, page, now),
            Keysym::j if ctrl => self.step(1, page, now),
            Keysym::Page_Up | Keysym::KP_Page_Up => self.step(-(page as isize), page, now),
            Keysym::Page_Down | Keysym::KP_Page_Down => self.step(page as isize, page, now),
            Keysym::Home | Keysym::KP_Home => self.settle(0, true, page, now),
            Keysym::End | Keysym::KP_End => self.settle(usize::MAX, false, page, now),
            Keysym::Left | Keysym::Right if self.focus != StatsFocus::List && !self.searching => {
                let by = if sym == Keysym::Left { -1 } else { 1 };
                return match self.focus {
                    StatsFocus::Tabs => self.step_tab(by),
                    _ => self.show_all(by > 0),
                };
            }
            Keysym::Return | Keysym::KP_Enter => {
                if self.focus != StatsFocus::List {
                    self.focus = StatsFocus::List;
                    return None;
                }
                return self.drill(now);
            }
            Keysym::f if ctrl => {
                self.searching = true;
                return None;
            }
            Keysym::o if ctrl => return Some(self.open(now)),
            Keysym::r if ctrl => return Some(Outcome::Rebuild),
            // What is selected in the query copies from there.
            Keysym::c if ctrl && !(self.searching && !self.query.line().selection().is_empty()) => {
                let line = self.selected_line(now);
                return line.map(|l| Outcome::Copy(l.cells.join("\t")));
            }
            _ => return self.type_key(mods, sym, typed, page, now),
        }
        // A key that moves the list gives it the keys.
        self.focus = StatsFocus::List;
        None
    }

    /// A key no other way took: `/` starts a search, which takes what is
    /// typed while it has the keys.
    fn type_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        page: usize,
        now: u64,
    ) -> Option<Outcome> {
        if !self.searching && !mods.ctrl && (sym == Keysym::slash || typed == Keysym::slash) {
            self.searching = true;
            return None;
        }
        // Text pasted starts a search with it.
        if sym == PASTE {
            self.searching = true;
        }
        if self.searching && self.query.key(mods, sym, typed) == Edit::Changed {
            self.focus = StatsFocus::List;
            self.settle(0, true, page, now);
        }
        None
    }

    /// The focus area `back` or on from this one.
    fn focus_step(&self, back: bool) -> StatsFocus {
        use StatsFocus::*;
        // A feature's detail has no scope to pick.
        let scope = self.scope.detail.is_none();
        match (self.focus, back) {
            (Tabs, false) if scope => Scope,
            (Tabs, false) => List,
            (Scope, false) => List,
            (List, false) => Tabs,
            (Tabs, true) => List,
            (Scope, true) => Tabs,
            (List, true) if scope => Scope,
            (List, true) => Tabs,
        }
    }

    /// A left click on `hit`, `page` rows showing.
    pub fn click(&mut self, hit: StatsHit, page: usize, now: u64) -> Option<Outcome> {
        match hit {
            StatsHit::Tab(i) => {
                self.focus = StatsFocus::Tabs;
                self.show_tab(*TABS.get(i)?)
            }
            StatsHit::Scope(all) => {
                self.focus = StatsFocus::Scope;
                self.show_all(all)
            }
            StatsHit::Back => {
                self.leave_detail();
                self.settle(self.selected, true, page, now);
                None
            }
            StatsHit::Query => {
                self.searching = true;
                None
            }
            StatsHit::Row(i) => {
                if self.lines(now).get(i).is_none_or(|l| l.heading) {
                    return None;
                }
                self.focus = StatsFocus::List;
                self.settle(i, true, page, now);
                // A feature clicked is shown, as Enter shows it.
                self.drill(now)
            }
            StatsHit::Hint(_) | StatsHit::Panel => None,
        }
    }

    /// The footer's hints, by what the selected row offers.
    fn hints(&self, now: u64) -> Vec<(String, String)> {
        let line = self.selected_line(now);
        let mut hints: Vec<(&str, &str)> = vec![("Tab", "focus")];
        if line.as_ref().is_some_and(|l| l.drill.is_some()) {
            hints.push(("Enter", "details"));
        }
        if line.as_ref().is_some_and(|l| l.open.is_some()) {
            hints.push(("Ctrl+O", "open"));
        }
        hints.push(("Ctrl+F", "search"));
        if line.is_some() {
            hints.push(("Ctrl+C", "copy"));
        }
        hints.push(("Ctrl+R", "rebuild"));
        hints.push(("F1", "keys"));
        hints.push(("Esc", self.esc_word()));
        hints
            .into_iter()
            .map(|(k, w)| (k.to_owned(), w.to_owned()))
            .collect()
    }

    /// What the panel shows at `now`.
    pub fn view(&self, now: u64) -> StatsView {
        let lines = self.lines(now);
        let detail = self.scope.detail.as_ref();
        let title = match (detail, self.current()) {
            (
                Some(_),
                Some(Ok(View {
                    detail: Some(d), ..
                })),
            ) if !d.title.is_empty() => d.title.clone(),
            (Some((_, slug)), _) => slug.clone(),
            (None, _) => "AI work".into(),
        };
        let selected = self.selected.min(lines.len().saturating_sub(1));
        let selected = lines
            .get(selected)
            .is_some_and(|l| !l.heading)
            .then_some(selected);
        StatsView {
            title,
            tab: TABS.iter().position(|t| *t == self.scope.tab).unwrap_or(0),
            scope: detail.is_none().then(|| {
                let name = self.project.as_ref().map(|(_, name)| short(name, 24));
                (name, self.scope.project.is_none())
            }),
            back: detail.is_some(),
            focus: self.focus,
            query: self.query.line(),
            searching: self.searching,
            placeholder: match (detail.is_some(), self.scope.tab) {
                (true, _) => "Search this feature…",
                (false, Tab::Features) => "Search features…",
                (false, Tab::Agents) => "Search agents…",
                (false, Tab::Checks) => "Search checks…",
            }
            .into(),
            columns: self.columns(),
            header: detail.is_none(),
            rows: lines
                .into_iter()
                .map(|l| StatsRow {
                    set: l.set,
                    cells: l.cells,
                    heading: l.heading,
                })
                .collect(),
            first: self.first,
            selected,
            note: self.note(now),
            hints: self.hints(now),
            keys: match self.keys {
                true => KEYS
                    .iter()
                    .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
                    .collect(),
                false => Vec::new(),
            },
            hover: self.hover,
        }
    }
}

/// `text` cut to `max` characters, "…" in place of the rest.
fn short(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => text.chars().take(max - 1).chain(['…']).collect(),
        false => text.to_owned(),
    }
}

/// `lines` the query matches, any of their cells holding it, case aside;
/// a heading stays while a row under it does.
fn filtered(lines: Vec<Line>, query: &str) -> Vec<Line> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return lines;
    }
    let hit = |l: &Line| l.cells.iter().any(|c| c.to_lowercase().contains(&query));
    let mut out: Vec<Line> = Vec::new();
    let mut heading: Option<Line> = None;
    for line in lines {
        if line.heading {
            heading = Some(line);
        } else if hit(&line) {
            out.extend(heading.take());
            out.push(line);
        }
    }
    out
}

/// How long `secs` is, for people: "45s", "12m", "1h 05m", "2d 3h".
pub(crate) fn span(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
        s => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

/// A count of tokens, for people: "950", "12.3k", "340k", "1.2M".
pub(crate) fn tokens(n: u64) -> String {
    match n {
        n if n < 1000 => n.to_string(),
        n if n < 100_000 => format!("{:.1}k", n as f64 / 1000.0),
        n if n < 1_000_000 => format!("{}k", n / 1000),
        n => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// A share, 0 to 1, as a percentage: "40%".
fn share(of: f32) -> String {
    format!("{:.0}%", of * 100.0)
}

/// What a missing value shows as.
const NONE: &str = "–";

/// How an agent named `agent` (as a run records it) is called here.
fn agent_name(agent: &str) -> String {
    match AgentKind::of(agent) {
        AgentKind::Claude => "Claude Code".into(),
        AgentKind::Cursor => "Cursor".into(),
        AgentKind::Codex => "Codex".into(),
        AgentKind::OpenCode => "OpenCode".into(),
        AgentKind::T3 => "T3 Code".into(),
        AgentKind::Unknown if agent.is_empty() => NONE.into(),
        AgentKind::Unknown => agent.to_owned(),
    }
}

/// A model as shown: the CLI's own when none was picked.
fn model_name(model: &str) -> String {
    match model.is_empty() {
        true => "default".into(),
        false => model.to_owned(),
    }
}

/// When `at` was, from `now`: "3h ago".
fn when(at: Option<u64>, now: u64) -> String {
    at.map_or_else(|| NONE.into(), |at| ago(now, at))
}

/// How long from `from` to `to`.
fn took(from: Option<u64>, to: Option<u64>) -> String {
    match (from, to) {
        (Some(from), Some(to)) => span(to.saturating_sub(from)),
        _ => NONE.into(),
    }
}

fn feature_line(f: &stats::FeatureRow, all: bool, now: u64) -> Line {
    let name = match f.title.is_empty() {
        true => &f.slug,
        false => &f.title,
    };
    let title = match all {
        true => format!("{} · {name}", f.project),
        false => name.clone(),
    };
    Line {
        drill: Some((f.project.clone(), f.slug.clone())),
        ..Line::row(
            0,
            vec![
                title,
                f.status.clone(),
                ago(now, f.created),
                span(f.ai_secs),
                f.runs.to_string(),
                f.fix_rounds.to_string(),
                format!("{} / {}", tokens(f.tokens_in), tokens(f.tokens_out)),
                f.nudges.to_string(),
                f.send_backs.to_string(),
            ],
        )
    }
}

fn agent_line(a: &stats::AgentRow) -> Line {
    // The worst first, as the AI review sorts them.
    let order = ["critical", "high", "medium", "low"];
    let rank = |s: &str| order.iter().position(|o| *o == s).unwrap_or(order.len());
    let mut findings: Vec<(&String, &u32)> = a.findings.iter().filter(|(_, n)| **n > 0).collect();
    findings.sort_by_key(|(s, _)| rank(s));
    let findings = match findings.is_empty() {
        true => NONE.to_owned(),
        false => findings
            .iter()
            .map(|(s, n)| format!("{n} {s}"))
            .collect::<Vec<_>>()
            .join(", "),
    };
    Line::row(
        0,
        vec![
            a.stage
                .map_or_else(|| NONE.into(), |s| s.label().to_owned()),
            agent_name(&a.agent),
            model_name(&a.model),
            a.runs.to_string(),
            a.median_secs.map_or_else(|| NONE.into(), span),
            a.tokens_per_run.map_or_else(|| NONE.into(), tokens),
            share(a.nudge_rate),
            share(a.send_back_rate),
            findings,
            a.fixes.to_string(),
            a.verify_after_build.map_or_else(|| NONE.into(), share),
        ],
    )
}

fn check_line(c: &stats::CheckRow, now: u64) -> Line {
    Line {
        open: c
            .last_log
            .clone()
            .map(|path| (path, format!("Last failure of {}", c.command))),
        ..Line::row(
            0,
            vec![
                c.repo.clone(),
                c.command.clone(),
                c.runs.to_string(),
                share(c.failure_rate),
                c.median_secs.map_or_else(|| NONE.into(), span),
                when(c.last_failure, now),
            ],
        )
    }
}

/// A feature's detail: a section each for its stages, runs, fix rounds and
/// Verify runs, those it has, each under the heading naming its columns.
fn detail_lines(d: &stats::Detail, now: u64) -> Vec<Line> {
    let mut lines = Vec::new();
    let heading = |set: usize, titles: &[&str]| {
        Line::heading(set, titles.iter().map(|t| (*t).to_owned()).collect())
    };
    if !d.stages.is_empty() {
        lines.push(heading(0, &["Stages", "Started", "Ended", "Took"]));
        lines.extend(d.stages.iter().map(|s| {
            Line::row(
                0,
                vec![
                    s.stage.label().to_owned(),
                    when(s.started, now),
                    when(s.ended, now),
                    took(s.started, s.ended),
                ],
            )
        }));
    }
    if !d.runs.is_empty() {
        let titles = [
            "Runs",
            "Agent",
            "Model",
            "Took",
            "Tokens",
            "Nudges",
            "Sent back",
        ];
        lines.push(heading(1, &titles));
        lines.extend(d.runs.iter().map(|r| {
            let run = format!("{} {}", r.stage.label(), r.n);
            Line {
                open: r
                    .transcript
                    .clone()
                    .map(|path| (path, format!("Transcript of {run}"))),
                ..Line::row(
                    1,
                    vec![
                        run.clone(),
                        agent_name(r.agent.name()),
                        model_name(&r.model),
                        took(Some(r.started), r.ended),
                        format!("{} / {}", tokens(r.tokens_in), tokens(r.tokens_out)),
                        r.nudges.to_string(),
                        r.send_backs.to_string(),
                    ],
                )
            }
        }));
    }
    if !d.fix_rounds.is_empty() {
        lines.push(heading(2, &["Fix rounds", "Opened", "Closed", "Fixes"]));
        lines.extend(d.fix_rounds.iter().map(|f| {
            let closed = match f.closed {
                None if f.opened.is_some() => "open".into(),
                closed => when(closed, now),
            };
            Line::row(
                2,
                vec![
                    format!("Round {}", f.n),
                    when(f.opened, now),
                    closed,
                    f.fixes.to_string(),
                ],
            )
        }));
    }
    if !d.verify_runs.is_empty() {
        let titles = ["Verify runs", "Started", "Took", "Passed", "Failed"];
        lines.push(heading(3, &titles));
        lines.extend(d.verify_runs.iter().map(|v| {
            // `v{n}@{started}`: the run's number is what people know it by.
            let n = v.run.split('@').next().unwrap_or(&v.run);
            let n = n.strip_prefix('v').unwrap_or(n);
            Line::row(
                3,
                vec![
                    format!("Verify {n}"),
                    when(v.started, now),
                    took(v.started, v.ended),
                    v.passed.to_string(),
                    v.failed.to_string(),
                ],
            )
        }));
    }
    lines
}

/// Read the index under `root` for `scope`: nothing kept yet while there is
/// no index, or AI work isn't kept at all.
fn read(root: Option<&Path>, scope: &Scope) -> Result<View, String> {
    let nothing = || View {
        scope: scope.clone(),
        ..View::default()
    };
    let Some(root) = root else {
        return Ok(nothing());
    };
    if !index::path(root).exists() {
        return Ok(nothing());
    }
    let conn = index::open_read(root)?;
    stats::query(&conn, scope)
}

/// The text of a kept file at `path`, to read: unpacked when it is
/// zstd's, a chat's lines (`.jsonl`) as who said what.
pub(crate) fn readable(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let plain = name.strip_suffix(".zst").unwrap_or(&name);
    if plain.ends_with(".db") {
        return Err("a Cursor chat is kept as its database, which can't be shown as text".into());
    }
    let raw = std::fs::read(path).map_err(|err| err.to_string())?;
    let bytes = match plain.len() < name.len() {
        true => zst::decompress(&raw).map_err(|err| err.to_string())?,
        false => raw,
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(match plain.ends_with(".jsonl") {
        true => chat_text(&text),
        false => text.into_owned(),
    })
}

/// A chat's lines of JSON (Claude Code's, Codex's) as who said what, a
/// blank line between: what isn't said in words (a tool called) by name.
pub(crate) fn chat_text(jsonl: &str) -> String {
    let mut out = String::new();
    for line in jsonl.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let body = value
            .get("message")
            .or_else(|| value.get("payload"))
            .unwrap_or(&value);
        let role = [body.get("role"), value.get("role"), value.get("type")]
            .into_iter()
            .flatten()
            .find_map(Value::as_str)
            .unwrap_or("");
        let mut said = Vec::new();
        if let Some(content) = body.get("content") {
            words(content, &mut said);
        }
        let said = said.join("\n");
        if said.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if !role.is_empty() {
            out.push_str(role);
            out.push_str(": ");
        }
        out.push_str(said.trim_end());
    }
    out
}

/// What `content` says: its text, and a tool called by its name.
fn words(content: &Value, out: &mut Vec<String>) {
    match content {
        Value::String(text) => out.push(text.clone()),
        Value::Array(parts) => parts.iter().for_each(|part| words(part, out)),
        Value::Object(part) => {
            if let Some(Value::String(text)) = part.get("text") {
                out.push(text.clone());
            } else if let Some(Value::String(name)) = part.get("name") {
                out.push(format!("[{name}]"));
            } else if let Some(inner) = part.get("content") {
                words(inner, out);
            }
        }
        _ => {}
    }
}

/// Now, in seconds since the epoch.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Gerdr {
    /// Open the stats panel, or close it again.
    pub(crate) fn toggle_ai_stats(&mut self) {
        match self.chrome.ai_stats.is_some() {
            true => self.close_ai_stats(),
            false => self.open_ai_stats(),
        }
    }

    /// Open the stats panel on the open project's features, and read them.
    pub(crate) fn open_ai_stats(&mut self) {
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.inbox.close();
        self.scrollback = None;
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let project = self.project.clone().map(|id| {
            let name = self
                .projects
                .project(&id)
                .map_or_else(|| id.clone(), |p| p.name.clone());
            (id, name)
        });
        self.chrome.ai_stats = Some(StatsPanel::new(project));
        self.ai_stats_request();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the stats panel.
    pub(crate) fn close_ai_stats(&mut self) {
        self.chrome.ai_stats = None;
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// Read the index again for the panel, if it is open: the AI work's
    /// thread took new lines in.
    #[allow(dead_code)] // on `Report::Indexed`, once the thread runs (AI work, wave 2)
    pub(crate) fn ai_stats_refresh(&mut self) {
        if self.chrome.ai_stats.as_ref().is_some_and(|p| !p.reading) {
            self.ai_stats_request();
        }
    }

    /// Ask for the panel's scope off the loop; `ai_stats_read` takes it.
    fn ai_stats_request(&mut self) {
        let Some(panel) = self.chrome.ai_stats.as_mut() else {
            return;
        };
        panel.reading = true;
        let scope = panel.scope.clone();
        let root = self.ai_work.root().map(Path::to_path_buf);
        self.feature_work(None, move |_| {
            let result = read(root.as_deref(), &scope);
            Done::AiStats { scope, result }
        });
    }

    /// The index was read for `scope`.
    pub(crate) fn ai_stats_read(&mut self, scope: Scope, result: Result<View, String>) {
        if let Err(err) = &result {
            tracing::warn!("reading the AI work's stats: {err}");
        }
        let page = self.ai_stats_page();
        // Closed since, or moved on to another scope: what was read is of
        // no use, and a newer read is under way for the scope shown.
        let Some(panel) = self.chrome.ai_stats.as_mut() else {
            return;
        };
        if panel.scope != scope {
            return;
        }
        panel.reading = false;
        panel.view = Some(result.map(|view| View { scope, ..view }));
        panel.settle(panel.selected, true, page, now_secs());
    }

    /// How many rows the panel's list shows.
    fn ai_stats_page(&self) -> usize {
        chrome::stats_page(self.output_size())
    }

    /// What the panel shows, while it is open.
    pub(crate) fn ai_stats_view(&self) -> Option<StatsView> {
        Some(self.chrome.ai_stats.as_ref()?.view(now_secs()))
    }

    /// A key while the panel has the keyboard. `sym` is the key's own
    /// symbol, `typed` the one with Shift and the layout applied.
    pub(crate) fn ai_stats_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let page = self.ai_stats_page();
        let panel = self.chrome.ai_stats.as_mut()?;
        panel.key(mods, sym, typed, page, now_secs())
    }

    /// Do what the panel asked.
    pub(crate) fn apply_ai_stats(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_ai_stats(),
            Outcome::Read => self.ai_stats_request(),
            Outcome::NothingToOpen => {
                self.show_warning("This row has no transcript or log kept".into());
            }
            Outcome::Copy(text) => {
                self.copy_text(text);
                self.show_success("Copied the row".into());
            }
            Outcome::Rebuild => {
                if !self.ai_work.is_on() {
                    return self.show_warning("AI work isn't kept on this gerdr".into());
                }
                self.ai_work.send(Job::Rescan);
                self.show_notice("Drawing the AI work's index again from its files".into());
                self.ai_stats_request();
            }
            Outcome::Open { path, what, title } => self.ai_stats_open(path, what, title),
        }
    }

    /// Unpack the file at `path` off the loop, and show it in the
    /// scrollback viewer in the panel's place.
    fn ai_stats_open(&mut self, path: PathBuf, what: String, title: String) {
        let (tx, rx) = channel::channel::<Result<String, String>>();
        let shown = what.clone();
        let inserted = self.loop_handle.insert_source(rx, move |event, _, state| {
            if let channel::Event::Msg(result) = event {
                state.ai_stats_opened(&shown, &title, result);
            }
        });
        if let Err(err) = inserted {
            tracing::warn!("opening {}: {err}", path.display());
            let what = what.to_lowercase();
            return self.show_error(format!("Couldn't open the {what}: {err}"));
        }
        std::thread::spawn(move || {
            let result = readable(&path)
                .inspect_err(|err| tracing::warn!("reading {}: {err}", path.display()));
            // It fails only once the loop is gone: gerdr is quitting.
            let _ = tx.send(result);
        });
    }

    /// The text of `what` came, to show in the viewer in the panel's place.
    /// With the panel closed meanwhile it isn't shown: the user moved on.
    fn ai_stats_opened(&mut self, what: &str, title: &str, result: Result<String, String>) {
        if self.chrome.ai_stats.is_none() {
            return;
        }
        let text = match result {
            Ok(text) => text,
            Err(err) => {
                let what = what.to_lowercase();
                return self.show_error(format!("Couldn't open the {what}: {err}"));
            }
        };
        self.chrome.ai_stats = None;
        self.stop_key_repeat();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let source = Source::File {
            what: what.to_owned(),
        };
        let mut viewer = Scrollback::of(source, title.to_owned(), &text, 0);
        // Over the app area, as the viewer goes with no pane under it.
        let area = self.content_rect();
        let inset = 4;
        let panel = Rectangle::new(
            (area.loc.x + inset, area.loc.y + inset).into(),
            (
                (area.size.w - 2 * inset).max(1),
                (area.size.h - 2 * inset).max(1),
            )
                .into(),
        );
        viewer.fit(chrome::scrollback_fit(panel));
        self.scrollback = Some(viewer);
        self.refresh_pointer_focus();
    }

    /// A left click while the panel is open: outside it closes it, but on a
    /// toast presses that.
    pub(crate) fn ai_stats_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.ai_stats_view() else {
            return;
        };
        let layout = chrome::stats_layout(&view, self.output_size());
        let outcome = match chrome::stats_hit(&view, &layout, pos) {
            Some(StatsHit::Hint(i)) => {
                if let Some((keys, _)) = view.hints.get(i) {
                    self.press_hint(Overlay::AiStats, keys, pos);
                }
                return;
            }
            Some(hit) => {
                let page = self.ai_stats_page();
                let panel = self.chrome.ai_stats.as_mut();
                panel.and_then(|panel| panel.click(hit, page, now_secs()))
            }
            None => match self.toast_press(pos) {
                Some(ToastHit::Action(action)) => {
                    self.menu_action(action);
                    None
                }
                Some(_) => None,
                None => Some(Outcome::Close),
            },
        };
        if let Some(outcome) = outcome {
            self.apply_ai_stats(outcome);
        }
    }

    /// The pointer moved while the panel is open: what it is over lights
    /// up, and the Back button says its key.
    pub(crate) fn ai_stats_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.ai_stats_view()?;
        let layout = chrome::stats_layout(&view, self.output_size());
        let hit = chrome::stats_hit(&view, &layout, pos);
        self.chrome.ai_stats.as_mut()?.hover = hit;
        match hit? {
            StatsHit::Back => Some(Tooltip::new("Back to the list", layout.back?).key("Esc")),
            _ => None,
        }
    }

    /// Whether a control of the panel is at `pos`: the pointer is a hand
    /// over it.
    pub(crate) fn ai_stats_control_at(&self, pos: Point<f64, Logical>) -> bool {
        self.ai_stats_view().is_some_and(|view| {
            let layout = chrome::stats_layout(&view, self.output_size());
            !matches!(
                chrome::stats_hit(&view, &layout, pos),
                None | Some(StatsHit::Panel | StatsHit::Query)
            )
        })
    }

    /// The wheel or the touchpad moved the panel's list.
    pub(crate) fn ai_stats_scroll(&mut self, motion: crate::scroll::Motion) {
        if self.chrome.ai_stats.is_none() {
            return;
        }
        let rows = self.motion_rows(motion, chrome::STATS_ROW as f64);
        let page = self.ai_stats_page();
        if let Some(panel) = self.chrome.ai_stats.as_mut() {
            panel.scroll_by(rows, page, now_secs());
        }
    }

    /// The query's field, for a press or a drag in it.
    pub(crate) fn ai_stats_field_mut(&mut self) -> Option<&mut TextField> {
        Some(&mut self.chrome.ai_stats.as_mut()?.query)
    }
}

/// Fake index views for tests, the golden pictures' too.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::collections::BTreeMap;

    use super::*;
    use crate::feature::Stage;

    /// The time the fakes are seen at.
    pub const NOW: u64 = 1_800_000_000;

    /// Two features, an agent and a check of project gerdr.
    pub fn view(scope: Scope) -> View {
        View {
            scope,
            features: vec![
                stats::FeatureRow {
                    project: "gerdr".into(),
                    slug: "ai-work".into(),
                    title: "Keep all AI work".into(),
                    status: "building".into(),
                    created: NOW - 3 * 86_400,
                    ai_secs: 4 * 3600 + 300,
                    runs: 9,
                    fix_rounds: 2,
                    tokens_in: 1_250_000,
                    tokens_out: 84_000,
                    nudges: 3,
                    send_backs: 1,
                },
                stats::FeatureRow {
                    project: "gerdr".into(),
                    slug: "stats".into(),
                    title: "Stats panel".into(),
                    status: "done".into(),
                    created: NOW - 7200,
                    ai_secs: 1500,
                    runs: 3,
                    ..stats::FeatureRow::default()
                },
            ],
            agents: vec![stats::AgentRow {
                stage: Some(Stage::Build),
                agent: "claude".into(),
                model: "opus".into(),
                runs: 12,
                median_secs: Some(1800),
                tokens_per_run: Some(240_000),
                nudge_rate: 0.25,
                send_back_rate: 0.1,
                findings: BTreeMap::from([("high".into(), 2), ("low".into(), 5)]),
                fixes: 7,
                verify_after_build: Some(0.8),
            }],
            checks: vec![stats::CheckRow {
                repo: "gerdr".into(),
                command: "cargo test".into(),
                runs: 20,
                failure_rate: 0.15,
                median_secs: Some(95),
                last_failure: Some(NOW - 600),
                last_log: Some("/d/blobs/ab/ab12.zst".into()),
            }],
            detail: None,
        }
    }

    /// Feature ai-work in detail: two stages, a run, a fix round and a
    /// Verify run.
    pub fn detail(scope: Scope) -> View {
        View {
            scope,
            detail: Some(stats::Detail {
                project: "gerdr".into(),
                slug: "ai-work".into(),
                title: "Keep all AI work".into(),
                stages: vec![
                    stats::StageSpan {
                        stage: Stage::Plan,
                        started: Some(NOW - 86_400),
                        ended: Some(NOW - 80_000),
                    },
                    stats::StageSpan {
                        stage: Stage::Build,
                        started: Some(NOW - 80_000),
                        ended: None,
                    },
                ],
                runs: vec![stats::RunRow {
                    stage: Stage::Build,
                    n: 1,
                    agent: AgentKind::Claude,
                    model: "opus".into(),
                    started: NOW - 80_000,
                    ended: Some(NOW - 70_000),
                    tokens_in: 900_000,
                    tokens_out: 60_000,
                    nudges: 1,
                    send_backs: 0,
                    transcript: Some("/d/sessions/claude-1/transcript.jsonl.zst".into()),
                }],
                fix_rounds: vec![stats::FixRow {
                    n: 1,
                    opened: Some(NOW - 60_000),
                    closed: None,
                    fixes: 4,
                }],
                verify_runs: vec![stats::VerifyRow {
                    run: format!("v2@{}", NOW - 50_000),
                    started: Some(NOW - 50_000),
                    ended: Some(NOW - 49_000),
                    passed: 5,
                    failed: 1,
                }],
            }),
            ..View::default()
        }
    }

    /// A panel on gerdr's Features: its view as `make` has it of the scope.
    pub fn panel(make: impl FnOnce(Scope) -> Option<Result<View, String>>) -> StatsPanel {
        let mut panel = StatsPanel::new(Some(("gerdr".into(), "gerdr".into())));
        panel.view = make(panel.scope.clone());
        panel
    }

    /// The same, shown in detail on feature ai-work.
    pub fn detail_panel() -> StatsPanel {
        let mut panel = panel(|scope| Some(Ok(view(scope))));
        panel.key(
            &ModifiersState::default(),
            Keysym::Return,
            Keysym::Return,
            10,
            NOW,
        );
        panel.view = Some(Ok(detail(panel.scope.clone())));
        panel.settle(0, true, 10, NOW);
        panel
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{NOW, detail, detail_panel, panel, view};
    use super::*;

    fn mods(ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            shift,
            ..ModifiersState::default()
        }
    }

    /// A key as input sends it: the unshifted sym, and what Shift made of
    /// it.
    fn press(p: &mut StatsPanel, m: ModifiersState, sym: Keysym, typed: Keysym) -> Option<Outcome> {
        p.key(&m, sym, typed, 10, NOW)
    }

    fn key(p: &mut StatsPanel, sym: Keysym) -> Option<Outcome> {
        press(p, mods(false, false), sym, sym)
    }

    fn ctrl(p: &mut StatsPanel, sym: Keysym) -> Option<Outcome> {
        press(p, mods(true, false), sym, sym)
    }

    fn loaded() -> StatsPanel {
        panel(|scope| Some(Ok(view(scope))))
    }

    #[test]
    fn it_opens_on_the_projects_features_reading() {
        let panel = StatsPanel::new(Some(("p1".into(), "gerdr".into())));
        assert_eq!(panel.scope.project.as_deref(), Some("p1"));
        assert_eq!(panel.scope.tab, Tab::Features);
        assert_eq!(panel.note(NOW), Some(("Reading…".into(), false)));
        // With no project, every project's.
        assert_eq!(StatsPanel::new(None).scope.project, None);
    }

    #[test]
    fn nothing_kept_and_a_failed_read_are_told_apart() {
        let mut panel = StatsPanel::new(None);
        panel.view = Some(Ok(View::default()));
        assert_eq!(panel.note(NOW), Some(("No AI work kept yet".into(), false)));
        panel.view = Some(Err("disk I/O error".into()));
        assert_eq!(
            panel.note(NOW),
            Some(("Couldn't read the stats: disk I/O error".into(), true))
        );
        assert!(panel.lines(NOW).is_empty());
    }

    #[test]
    fn a_read_for_another_scope_shows_as_reading() {
        let mut panel = loaded();
        assert_eq!(panel.note(NOW), None);
        assert_eq!(ctrl(&mut panel, Keysym::Page_Down), Some(Outcome::Read));
        assert_eq!(panel.scope.tab, Tab::Agents);
        assert_eq!(panel.note(NOW), Some(("Reading…".into(), false)));
    }

    #[test]
    fn the_arrows_and_ctrl_j_k_move_and_stop_at_the_ends() {
        let mut panel = loaded();
        assert_eq!(panel.selected, 0);
        key(&mut panel, Keysym::Down);
        assert_eq!(panel.selected, 1);
        key(&mut panel, Keysym::Down);
        assert_eq!(panel.selected, 1);
        ctrl(&mut panel, Keysym::k);
        assert_eq!(panel.selected, 0);
        ctrl(&mut panel, Keysym::j);
        assert_eq!(panel.selected, 1);
        key(&mut panel, Keysym::Home);
        assert_eq!(panel.selected, 0);
        key(&mut panel, Keysym::End);
        assert_eq!(panel.selected, 1);
        key(&mut panel, Keysym::Page_Up);
        assert_eq!(panel.selected, 0);
        // Ctrl+N and Ctrl+P never move.
        ctrl(&mut panel, Keysym::n);
        assert_eq!(panel.selected, 0);
        ctrl(&mut panel, Keysym::p);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn the_wheel_takes_the_selection_along_to_rows_shown() {
        let mut panel = panel(|scope| {
            let mut v = view(scope);
            let row = v.features[1].clone();
            v.features = (0..30)
                .map(|i| stats::FeatureRow {
                    slug: format!("f{i}"),
                    ..row.clone()
                })
                .collect();
            Some(Ok(v))
        });
        panel.scroll_by(12, 10, NOW);
        assert_eq!((panel.first, panel.selected), (12, 12));
        panel.scroll_by(100, 10, NOW);
        assert_eq!((panel.first, panel.selected), (20, 20));
        panel.scroll_by(-15, 10, NOW);
        assert_eq!((panel.first, panel.selected), (5, 14));
    }

    #[test]
    fn ctrl_page_keys_switch_tabs_round_the_ends() {
        let mut panel = loaded();
        assert_eq!(ctrl(&mut panel, Keysym::Page_Up), Some(Outcome::Read));
        assert_eq!(panel.scope.tab, Tab::Checks);
        ctrl(&mut panel, Keysym::Page_Down);
        assert_eq!(panel.scope.tab, Tab::Features);
        // Plain PgDn moves the list instead.
        assert_eq!(key(&mut panel, Keysym::Page_Down), None);
        assert_eq!(panel.scope.tab, Tab::Features);
    }

    #[test]
    fn tab_and_shift_tab_go_round_the_focus_both_ways() {
        let mut panel = loaded();
        assert_eq!(panel.focus, StatsFocus::List);
        key(&mut panel, Keysym::Tab);
        assert_eq!(panel.focus, StatsFocus::Tabs);
        key(&mut panel, Keysym::Tab);
        assert_eq!(panel.focus, StatsFocus::Scope);
        // Shift+Tab as input sends it: Tab, and what Shift made of it.
        press(
            &mut panel,
            mods(false, true),
            Keysym::Tab,
            Keysym::ISO_Left_Tab,
        );
        assert_eq!(panel.focus, StatsFocus::Tabs);
        press(
            &mut panel,
            mods(false, true),
            Keysym::Tab,
            Keysym::ISO_Left_Tab,
        );
        assert_eq!(panel.focus, StatsFocus::List);
        // In a feature's detail there is no scope to stop at.
        let mut panel = detail_panel();
        key(&mut panel, Keysym::Tab);
        key(&mut panel, Keysym::Tab);
        assert_eq!(panel.focus, StatsFocus::List);
    }

    #[test]
    fn left_and_right_pick_the_focused_tab_or_scope() {
        let mut panel = loaded();
        key(&mut panel, Keysym::Tab);
        assert_eq!(key(&mut panel, Keysym::Right), Some(Outcome::Read));
        assert_eq!(panel.scope.tab, Tab::Agents);
        key(&mut panel, Keysym::Tab);
        assert_eq!(key(&mut panel, Keysym::Right), Some(Outcome::Read));
        assert_eq!(panel.scope.project, None);
        assert_eq!(key(&mut panel, Keysym::Right), None);
        assert_eq!(key(&mut panel, Keysym::Left), Some(Outcome::Read));
        assert_eq!(panel.scope.project.as_deref(), Some("gerdr"));
        // Enter gives the list the keys.
        key(&mut panel, Keysym::Return);
        assert_eq!(panel.focus, StatsFocus::List);
    }

    #[test]
    fn enter_shows_a_feature_and_esc_goes_back_to_where_it_was() {
        let mut panel = loaded();
        key(&mut panel, Keysym::Down);
        assert_eq!(key(&mut panel, Keysym::Return), Some(Outcome::Read));
        assert_eq!(panel.scope.detail, Some(("gerdr".into(), "stats".into())));
        panel.view = Some(Ok(detail(panel.scope.clone())));
        panel.settle(0, true, 10, NOW);
        // The selection skips the headings.
        assert!(panel.lines(NOW)[0].heading);
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.view(NOW).title, "Keep all AI work");
        assert_eq!(key(&mut panel, Keysym::Escape), None);
        assert_eq!(panel.scope.detail, None);
        assert_eq!(panel.selected, 1);
        // The list as it was, with no read.
        assert_eq!(panel.note(NOW), None);
        assert_eq!(key(&mut panel, Keysym::Escape), Some(Outcome::Close));
    }

    #[test]
    fn esc_clears_the_query_then_leaves_the_search_then_closes() {
        let mut panel = loaded();
        assert_eq!(ctrl(&mut panel, Keysym::f), None);
        assert!(panel.searching);
        key(&mut panel, Keysym::s);
        key(&mut panel, Keysym::t);
        assert_eq!(&*panel.query, "st");
        assert_eq!(panel.lines(NOW).len(), 1);
        assert_eq!(panel.esc_word(), "clear");
        key(&mut panel, Keysym::Escape);
        assert_eq!(&*panel.query, "");
        assert!(panel.searching);
        assert_eq!(panel.esc_word(), "back");
        key(&mut panel, Keysym::Escape);
        assert!(!panel.searching);
        assert_eq!(panel.esc_word(), "close");
        assert_eq!(key(&mut panel, Keysym::Escape), Some(Outcome::Close));
    }

    #[test]
    fn slash_starts_a_search_and_is_typed_once_searching() {
        let mut panel = loaded();
        key(&mut panel, Keysym::slash);
        assert!(panel.searching);
        assert_eq!(&*panel.query, "");
        key(&mut panel, Keysym::slash);
        key(&mut panel, Keysym::q);
        assert_eq!(&*panel.query, "/q");
        // A search finding nothing says so, not that nothing is kept.
        assert_eq!(
            panel.note(NOW).unwrap().0,
            "Nothing matches \u{201c}/q\u{201d}"
        );
        // Letters go nowhere until a search is open.
        let mut panel = loaded();
        key(&mut panel, Keysym::s);
        assert_eq!(&*panel.query, "");
    }

    #[test]
    fn a_search_keeps_the_headings_of_what_it_finds() {
        let mut panel = detail_panel();
        ctrl(&mut panel, Keysym::f);
        for sym in [Keysym::o, Keysym::p, Keysym::u, Keysym::s] {
            key(&mut panel, sym);
        }
        let lines = panel.lines(NOW);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].heading && lines[0].cells[0] == "Runs");
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn ctrl_o_opens_what_the_row_keeps_and_says_when_it_keeps_none() {
        let mut panel = loaded();
        assert_eq!(ctrl(&mut panel, Keysym::o), Some(Outcome::NothingToOpen));
        ctrl(&mut panel, Keysym::Page_Up);
        panel.view = Some(Ok(view(panel.scope.clone())));
        assert_eq!(
            ctrl(&mut panel, Keysym::o),
            Some(Outcome::Open {
                path: "/d/blobs/ab/ab12.zst".into(),
                what: "Last failure of cargo test".into(),
                title: "gerdr".into(),
            })
        );
        let mut panel = detail_panel();
        key(&mut panel, Keysym::Down);
        key(&mut panel, Keysym::Down);
        let Some(Outcome::Open { what, .. }) = ctrl(&mut panel, Keysym::o) else {
            panic!("no transcript");
        };
        assert_eq!(what, "Transcript of Build 1");
    }

    #[test]
    fn ctrl_c_copies_the_row_and_ctrl_r_rebuilds() {
        let mut panel = loaded();
        let Some(Outcome::Copy(text)) = ctrl(&mut panel, Keysym::c) else {
            panic!("no copy");
        };
        assert!(text.starts_with("Keep all AI work\tbuilding\t3d ago"));
        assert_eq!(ctrl(&mut panel, Keysym::r), Some(Outcome::Rebuild));
    }

    #[test]
    fn f1_lists_the_keys_and_only_closing_keys_reach_it() {
        let mut panel = loaded();
        key(&mut panel, Keysym::F1);
        assert!(panel.keys);
        assert_eq!(panel.view(NOW).keys.len(), KEYS.len());
        assert_eq!(key(&mut panel, Keysym::Down), None);
        assert_eq!(panel.selected, 0);
        assert_eq!(key(&mut panel, Keysym::Escape), None);
        assert!(!panel.keys);
    }

    #[test]
    fn a_click_on_a_feature_shows_it_and_on_a_tab_reads_it() {
        let mut panel = loaded();
        assert_eq!(panel.click(StatsHit::Row(1), 10, NOW), Some(Outcome::Read));
        assert_eq!(panel.scope.detail, Some(("gerdr".into(), "stats".into())));
        assert_eq!(panel.click(StatsHit::Back, 10, NOW), None);
        assert_eq!(panel.scope.detail, None);
        assert_eq!(panel.click(StatsHit::Tab(2), 10, NOW), Some(Outcome::Read));
        assert_eq!(panel.scope.tab, Tab::Checks);
        let all = panel.click(StatsHit::Scope(true), 10, NOW);
        assert_eq!(all, Some(Outcome::Read));
        assert_eq!(panel.scope.project, None);
    }

    #[test]
    fn the_footer_says_what_esc_and_the_row_offer() {
        let mut panel = loaded();
        let hints = panel.hints(NOW);
        let keys: Vec<&str> = hints.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"Enter") && !keys.contains(&"Ctrl+O"));
        assert_eq!(hints.last().unwrap(), &("Esc".into(), "close".into()));
        ctrl(&mut panel, Keysym::Page_Up);
        panel.view = Some(Ok(view(panel.scope.clone())));
        let keys: Vec<String> = panel.hints(NOW).into_iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&"Ctrl+O".to_owned()) && !keys.contains(&"Enter".to_owned()));
    }

    #[test]
    fn each_footer_hint_presses_a_key_that_does_something() {
        // A click on a hint presses its key (`press_hint`): each parses,
        // and the panel takes it.
        for (keys, _) in loaded().hints(NOW) {
            let (m, sym) = crate::keys::hint_key(&keys).expect(&keys);
            let mut panel = loaded();
            let before = format!("{panel:?}");
            let out = panel.key(&m, sym, sym, 10, NOW);
            assert!(out.is_some() || format!("{panel:?}") != before, "{keys}");
        }
    }

    #[test]
    fn counts_read_as_people_say_them() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(720), "12m");
        assert_eq!(span(3900), "1h 05m");
        assert_eq!(span(2 * 86_400 + 3 * 3600), "2d 3h");
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(12_340), "12.3k");
        assert_eq!(tokens(340_000), "340k");
        assert_eq!(tokens(1_250_000), "1.2M");
    }

    #[test]
    fn a_chat_reads_as_who_said_what() {
        let jsonl = [
            r#"{"type":"user","message":{"role":"user","content":"Fix the build"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"On it."},{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}"#,
            "not json",
            r#"{"type":"summary"}"#,
        ]
        .join("\n");
        assert_eq!(
            chat_text(&jsonl),
            "user: Fix the build\n\nassistant: On it.\n[Bash]\n\nassistant: Done"
        );
    }

    #[test]
    fn a_kept_file_is_unpacked_and_a_cursor_chat_refused() {
        let dir = std::env::temp_dir().join(format!("gerdr-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("ab12.zst");
        std::fs::write(&log, zst::compress(b"error: it failed\n", 3).unwrap()).unwrap();
        assert_eq!(readable(&log).unwrap(), "error: it failed\n");
        let chat = dir.join("transcript.jsonl.zst");
        let line = br#"{"message":{"role":"user","content":"hi"}}"#;
        std::fs::write(&chat, zst::compress(line, 3).unwrap()).unwrap();
        assert_eq!(readable(&chat).unwrap(), "user: hi");
        assert!(readable(&dir.join("store.db.zst")).is_err());
        assert!(readable(&dir.join("gone.zst")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_index_nothing_is_kept_yet() {
        let dir = std::env::temp_dir().join(format!("gerdr-stats-none-{}", std::process::id()));
        let scope = Scope::default();
        assert_eq!(read(Some(&dir), &scope), Ok(View::default()));
        assert_eq!(read(None, &scope), Ok(View::default()));
    }
}
