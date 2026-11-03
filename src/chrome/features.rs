//! The features' surfaces (see `crate::features`): the new feature dialog
//! over everything, the feature tab (the full-size first tab of a
//! feature's workspace: a header, the stages' rail, and the repositories',
//! stage page's and activity's columns, each scrolled on its own), and
//! the stage marks the sidebar's feature rows show.

use super::{
    ButtonStyle, CHECKBOX_GAP, CHECKBOX_ROW, Canvas, Check, DETAIL_FONT_SIZE, DIALOG_WIDTH, DOT,
    Dot, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE, ListBar, POPUP_RADIUS, Painter, Palette, QUIT_BODY,
    QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, Rect, Rgb, SHADOW,
    SMALL_FONT_SIZE, Thumb, agent_icon, band, button_width, check, chevron_side, copy,
    dialog_buttons, dot_shape, field_text, focus_ring, footer_hint_spots, gutter_track, hint_strs,
    icon_sdf, measure, measured_footer_height, mix, phys, physical, plus, rect_px, tab, trash,
};
use crate::agent_icons::AgentIcon;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The gap between rows, and the room between the last and the buttons.
const ROW_GAP: i32 = 4;
const ROWS_END: i32 = 16;
/// How far a row's contents sit in from its edges.
const ROW_INSET: i32 = 6;
/// The gap between a row's name and what follows it.
const PLACE_GAP: i32 = 12;
/// The room kept above and below a dialog or panel on a short output.
const MARGIN: i32 = 40;
/// The new feature dialog's buttons.
pub const NEW_FEATURE_BUTTONS: [&str; 2] = ["Cancel", "Create"];
/// The Create button, after Cancel: the dialog's default.
pub const CREATE: usize = 1;
/// The new feature dialog's Projects row button: the project picker,
/// and back from it.
pub const LINK_BUTTON: [&str; 2] = ["Link project\u{2026}", "Done"];
/// How high a button on a repository row or an agent card is.
const ROW_BUTTON: i32 = 24;
/// How wide the previous and next feature buttons are.
const SWITCH_W: i32 = 28;
/// How many lines a list on a stage's page shows at most (the Ship
/// check, Verify's failures): past it, the last says how many more.
pub const MAX_CHECK_LINES: usize = 4;
/// The feature tab's header buttons, before the one removing worktrees.
pub const FEATURE_BUTTONS: [&str; 3] = ["Open tab", "Copy path", "New feature\u{2026}"];

/// Where a stage stands in a feature, for its mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StageMark {
    Done,
    /// The first not done: the one being worked on.
    Current,
    Later,
}

impl StageMark {
    /// Its colour: done in the success colour, the current one in the
    /// accent, the later ones in the placeholder's (dim, but readable as
    /// a mark: 3:1 at least).
    pub(super) fn color(self, p: &Palette) -> Rgb {
        match self {
            Self::Done => p.success,
            Self::Current => p.accent,
            Self::Later => p.placeholder,
        }
    }
}

/// Signed distance to a stage's mark `d` across, centred on the origin: a
/// dot for done and current, a ring for a later one.
pub(super) fn stage_dot(mark: StageMark, x: f32, y: f32, d: f32) -> f32 {
    let r = d / 2.0;
    match mark {
        StageMark::Later => (x.hypot(y) - 0.75 * r).abs() - 0.25 * r,
        _ => x.hypot(y) - r,
    }
}

// ---------------------------------------------------------------- dialog

/// A row the new feature dialog offers: a repository, or a project in
/// its project picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeatureRepo {
    pub name: String,
    /// What follows the name: the branch it starts from and what the
    /// feature does with it, or why a project can't be linked.
    pub place: String,
    /// Whether the feature changes it (a project: links it).
    pub touched: bool,
    /// It can't be ticked or left out: the only repository, a project
    /// linked already or not found.
    pub fixed: bool,
}

/// What the new feature dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeatureView {
    /// Its heading: "New feature in “Shop”".
    pub heading: String,
    pub title: Line,
    /// The repository row with the keyboard; `None`: the title field.
    pub focus: Option<usize>,
    /// The line about where the feature goes: the root, or that it is
    /// being found, or why there is none.
    pub root: String,
    pub root_failed: bool,
    /// The Projects row: the home project and those linked.
    pub projects: String,
    /// The label of the Projects row's button (`LINK_BUTTON`); none
    /// when linking to an open feature, where the rows are the picker.
    pub link: Option<&'static str>,
    /// The rows: the repositories, or the projects to link.
    pub repos: Vec<NewFeatureRepo>,
    /// The label of the button doing it: Create, or Link.
    pub go: &'static str,
    /// The first repository shown, when not all fit.
    pub top: usize,
    /// What is wrong with the form, as a sentence.
    pub error: Option<String>,
    pub hover: Option<NewFeatureHit>,
    /// The keys besides the buttons, said left of them.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// What the pointer is over in the new feature dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewFeatureHit {
    Field,
    /// Index as in `NewFeatureView::repos`.
    Repo(usize),
    /// The Projects row's button.
    Link,
    /// 0 Cancel, 1 Create.
    Button(usize),
}

/// Where the new feature dialog's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeatureLayout {
    pub panel: Rect,
    pub title: Rect,
    pub field: Rect,
    pub error: Option<Rect>,
    pub root: Rect,
    /// The Projects row, and its button at its end.
    pub projects: Rect,
    pub link: Rect,
    /// One per repository that fits: the slots the ones from
    /// `NewFeatureView::top` are shown in.
    pub rows: Vec<Rect>,
    /// Cancel, Create.
    pub buttons: [Rect; 2],
}

/// Where the new feature dialog's parts go with `repos` repositories, an
/// error line if `error`, centred on an output of `size`: as many
/// repositories as fit.
pub fn new_feature_layout(size: Size<i32, Logical>, repos: usize, error: bool) -> NewFeatureLayout {
    let error_h = if error { QUIT_BODY + QUIT_LINE_GAP } else { 0 };
    let fixed = 2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_LINE_GAP
        + QUIT_BUTTON
        + QUIT_LINE_GAP
        + error_h
        + QUIT_BODY
        + QUIT_LINE_GAP
        + QUIT_BUTTON
        + QUIT_LINE_GAP
        + ROWS_END
        - ROW_GAP
        + QUIT_BUTTON;
    let fit = ((size.h - 2 * MARGIN - fixed) / (CHECKBOX_ROW + ROW_GAP)).max(1) as usize;
    let shown = repos.min(fit);
    let height = fixed + shown as i32 * (CHECKBOX_ROW + ROW_GAP);
    let width = DIALOG_WIDTH.min(size.w);
    let panel = Rectangle::new(
        (
            ((size.w - width) / 2).max(0),
            ((size.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let (x, mut y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = width - 2 * QUIT_PADDING;
    let mut next = |h: i32| {
        let rect = Rectangle::new((x, y).into(), (inner, h).into());
        y += h + QUIT_LINE_GAP;
        rect
    };
    let title = next(QUIT_TITLE);
    let field = next(QUIT_BUTTON);
    let error = error.then(|| next(QUIT_BODY));
    let root = next(QUIT_BODY);
    let projects = next(QUIT_BUTTON);
    let link_w = button_width(LINK_BUTTON[0]).max(button_width(LINK_BUTTON[1]));
    let link = Rectangle::new(
        (projects.loc.x + projects.size.w - link_w, projects.loc.y).into(),
        (link_w, QUIT_BUTTON).into(),
    );
    let rows = (0..shown as i32)
        .map(|i| {
            let y = y + i * (CHECKBOX_ROW + ROW_GAP);
            Rectangle::new((x, y).into(), (inner, CHECKBOX_ROW).into())
        })
        .collect();
    let buttons = dialog_buttons(panel, &NEW_FEATURE_BUTTONS);
    NewFeatureLayout {
        panel,
        title,
        field,
        error,
        root,
        projects,
        link,
        rows,
        buttons: [buttons[0], buttons[1]],
    }
}

impl NewFeatureLayout {
    /// What `pos` is over, the repositories shown from `top`.
    pub fn hit(&self, top: usize, pos: Point<f64, Logical>) -> Option<NewFeatureHit> {
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some(button) = self.buttons.iter().position(on) {
            return Some(NewFeatureHit::Button(button));
        }
        if on(&self.field) {
            return Some(NewFeatureHit::Field);
        }
        if on(&self.link) {
            return Some(NewFeatureHit::Link);
        }
        self.rows
            .iter()
            .position(on)
            .map(|slot| NewFeatureHit::Repo(top + slot))
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        (self.rows.iter().chain(&self.buttons))
            .chain(std::iter::once(&self.link))
            .copied()
            .collect()
    }
}

/// The new feature dialog's title field, as the caret and the pointer
/// care.
pub fn new_feature_field(view: &NewFeatureView, size: Size<i32, Logical>, scale: f64) -> FieldSpot {
    let area = new_feature_layout(size, view.repos.len(), view.error.is_some()).field;
    FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: view.title.clone(),
    }
}

// ----------------------------------------------------------------- panel

/// A repository of the feature panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureRepoRow {
    pub name: String,
    /// Its branch, or the base it is at as context.
    pub line: String,
    pub touched: bool,
    /// Being touched now.
    pub busy: bool,
    /// The label of its button: Touch for one the feature doesn't change,
    /// Open PR for one with a pull request; none else.
    pub button: Option<&'static str>,
}

/// How a line of the Ship check reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckTone {
    /// What stops shipping: in the warning colour.
    Blocker,
    /// It can ship: in the success colour.
    Ready,
    /// Worth knowing, or still being checked: muted.
    Note,
}

impl CheckTone {
    pub(super) fn color(self, p: &Palette) -> Rgb {
        match self {
            Self::Blocker => p.warning,
            Self::Ready => p.success,
            Self::Note => p.muted,
        }
    }
}

/// A line of the Ship check, under the Ship row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckLine {
    pub text: String,
    pub tone: CheckTone,
}

/// How a stage's node on the feature tab's rail reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeMark {
    /// Done: a success ring with a tick.
    Done,
    /// The stage the feature is at: filled in the accent, pulsing.
    Current,
    /// Still to come: a border ring.
    Later,
    /// Done, but the code changed since: a warning ring.
    Stale,
    /// The current stage, with a gate waiting for the user (a file to
    /// review, a stop to go on from): a warning dot.
    Waiting,
}

/// A stage's node on the feature tab's rail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailNode {
    pub label: String,
    /// One short line under the label ("2h ago", "running 14m", "ready
    /// for review", "failed"), or empty.
    pub line: String,
    pub mark: NodeMark,
    /// Its page is the one shown.
    pub selected: bool,
}

/// The feature tab's areas the keyboard moves between, with Tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabArea {
    /// The rail: the keys pick a stage's page.
    #[default]
    Stages,
    /// The repositories' column: the keys pick a repository.
    Repos,
}

/// The feature tab's columns, each scrolled on its own: the index into
/// `FeatureTabView::scroll`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabColumn {
    Repos = 0,
    Page = 1,
    Activity = 2,
}

impl TabColumn {
    pub const ALL: [Self; 3] = [Self::Repos, Self::Page, Self::Activity];
}

/// A button on a stage's page. What it does is for `crate::features` to
/// know, by its index in `StagePage::buttons`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageButton {
    pub label: String,
    /// `Primary` for the one to press next, `Destructive` for one that
    /// throws work away.
    pub style: ButtonStyle,
    /// The key that presses it, shown after the label ("⏎"), if any.
    pub key: Option<&'static str>,
}

/// A part of a stage's page, under its button row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageSection {
    /// A stage's agent: its logo and name, its dot, where it runs, and
    /// the last lines of its pane (empty until Inc 2 reads them).
    Agent {
        icon: AgentIcon,
        name: String,
        dot: Dot,
        /// "working · tab 2", or why it isn't running.
        status: String,
        tail: Vec<String>,
        /// The index in `StagePage::buttons` of the card's own button
        /// (Show agent), if any.
        button: Option<usize>,
    },
    /// A labelled list of lines in their tones: a file's state, the
    /// checks, findings, criteria, the Ship check, a done stage's note.
    Lines {
        label: String,
        lines: Vec<CheckLine>,
    },
    /// The milestones, "done of total", with a bar, and each by name when
    /// known (`true` = done; empty until Inc 2 reads them).
    Milestones {
        done: usize,
        total: usize,
        items: Vec<(bool, String)>,
    },
}

/// The selected stage's page, in the feature tab's middle column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagePage {
    pub title: String,
    /// What the stage is for.
    pub about: String,
    /// Where it stands now, as the old panel's row detail said it.
    pub status: Option<CheckLine>,
    pub buttons: Vec<PageButton>,
    pub sections: Vec<PageSection>,
}

/// A line of the feature tab's activity: one of the feature's history,
/// newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityRow {
    pub what: String,
    /// "14m ago".
    pub ago: String,
    /// Its dot's colour: `Ready` for a tick, `Blocker` for a send-back or
    /// a failure, `Note` else.
    pub tone: CheckTone,
}

/// What the feature tab shows: the whole tab area, not a centred card.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureTabView {
    pub title: String,
    /// Its slug, folder and age.
    pub sub: String,
    /// Which of how many features it is, when there are several.
    pub which: Option<(usize, usize)>,
    /// The label of the header's last button, removing its worktrees
    /// (Abandon…, Remove worktrees…), after `FEATURE_BUTTONS`.
    pub remove: &'static str,
    /// The eight stages, in order.
    pub rail: Vec<RailNode>,
    pub repos: Vec<FeatureRepoRow>,
    /// Why the repositories couldn't be read, shown in their column in
    /// place of an empty list.
    pub repos_error: Option<String>,
    /// The repository with the keyboard while `focus` is `Repos`.
    pub repo: usize,
    pub focus: TabArea,
    pub page: StagePage,
    pub activity: Vec<ActivityRow>,
    /// How far each column is scrolled down, in logical pixels, indexed
    /// by `TabColumn`.
    pub scroll: [f64; 3],
    pub hover: Option<FeatureTabHit>,
    pub hints: Vec<(String, String)>,
}

/// What the pointer is over on the feature tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureTabHit {
    /// The tab, but nothing on it that acts.
    Tab,
    /// Over a column, but nothing in it that acts: where the wheel goes.
    Column(TabColumn),
    Prev,
    Next,
    /// `FEATURE_BUTTONS`, then the one removing worktrees.
    Button(usize),
    /// A stage's node or label on the rail, by index into `rail`.
    Node(usize),
    /// Index as in `FeatureTabView::repos`.
    Repo(usize),
    RepoButton(usize),
    /// The narrow tab's "+N" / "N+" chip at an end of the repositories'
    /// strip, standing for the ones that don't fit: the repository a click
    /// selects (the first hidden one that way), as in `repos`.
    MoreRepos(usize),
    /// Index as in `StagePage::buttons`.
    PageButton(usize),
    Hint(usize),
}

/// Where the feature tab's parts go, in output coordinates. What sits in
/// a column (`repos`, `page_buttons`, `pieces`) is placed as if it were
/// not scrolled: `FeatureTabView::scroll` moves it up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureTabLayout {
    pub frame: Rect,
    pub title: Rect,
    pub sub: Rect,
    /// "2 of 3", and the buttons to the previous and next feature.
    pub switch: Option<(Rect, Rect, Rect)>,
    /// `FEATURE_BUTTONS`, then the one removing worktrees.
    pub buttons: Vec<Rect>,
    /// The header's buttons show only their icons: the tab is narrow.
    pub compact: bool,
    /// Each stage's node, with its label and line under it: what a click
    /// picks.
    pub nodes: Vec<Rect>,
    /// Each column's place (its card), indexed by `TabColumn`: Activity
    /// has none when the tab is narrower than `ACTIVITY_BELOW` (its rows
    /// go at the end of the page), and Repos none when narrower than
    /// `REPOS_BELOW`, where the repositories are a row of chips in
    /// `strip` under the rail.
    pub columns: [Option<Rect>; 3],
    /// How tall each column's contents are, indexed by `TabColumn`: how
    /// far it may scroll is this less its place's height.
    pub content: [i32; 3],
    /// The row of repository chips under the rail, on a narrow tab.
    pub strip: Option<Rect>,
    /// Each repository's row (or chip, in `strip`, not scrolled) and its
    /// button, indexed as `FeatureTabView::repos`.
    pub repos: Vec<(Rect, Option<Rect>)>,
    /// Each page button's place, indexed as `StagePage::buttons`.
    pub page_buttons: Vec<Option<Rect>>,
    /// The footer, and where each hint goes in it.
    pub hints: Rect,
    pub hint_spots: Vec<(usize, Rect)>,
    /// What each column paints, indexed by `TabColumn`.
    pub(super) pieces: [Vec<(Rect, Piece)>; 3],
}

impl FeatureTabLayout {
    /// What is at `pos` on the tab showing `view`, scrolled as it is.
    pub fn hit(&self, view: &FeatureTabView, pos: Point<i32, Logical>) -> Option<FeatureTabHit> {
        if !self.frame.contains(pos) {
            return None;
        }
        let on = |r: &Rect| r.contains(pos);
        if let Some((i, _)) = self.hint_spots.iter().find(|(_, r)| on(r)) {
            return Some(FeatureTabHit::Hint(*i));
        }
        if let Some((_, prev, next)) = &self.switch {
            if on(prev) {
                return Some(FeatureTabHit::Prev);
            }
            if on(next) {
                return Some(FeatureTabHit::Next);
            }
        }
        if let Some(b) = self.buttons.iter().position(on) {
            return Some(FeatureTabHit::Button(b));
        }
        if let Some(n) = self.nodes.iter().position(on) {
            return Some(FeatureTabHit::Node(n));
        }
        if self.strip.is_some_and(|s| s.contains(pos)) {
            return self.repo_at(pos).or(Some(FeatureTabHit::Tab));
        }
        for column in TabColumn::ALL {
            let Some(place) = self.columns[column as usize] else {
                continue;
            };
            if !place.contains(pos) {
                continue;
            }
            let down = view.scroll[column as usize].round() as i32;
            let at = Point::from((pos.x, pos.y + down));
            let found = match column {
                TabColumn::Repos => self.repo_at(at),
                TabColumn::Page => self
                    .page_buttons
                    .iter()
                    .position(|b| b.is_some_and(|b| b.contains(at)))
                    .map(FeatureTabHit::PageButton),
                TabColumn::Activity => None,
            };
            return found.or(Some(FeatureTabHit::Column(column)));
        }
        Some(FeatureTabHit::Tab)
    }

    /// The repository row, chip or button at `at` (unscrolled).
    fn repo_at(&self, at: Point<i32, Logical>) -> Option<FeatureTabHit> {
        for (i, (row, button)) in self.repos.iter().enumerate() {
            if button.is_some_and(|b| b.contains(at)) {
                return Some(FeatureTabHit::RepoButton(i));
            }
            if row.contains(at) {
                return Some(FeatureTabHit::Repo(i));
            }
        }
        None
    }

    /// How far `column` may scroll down, in logical pixels.
    pub fn scroll_room(&self, column: TabColumn) -> i32 {
        let i = column as usize;
        let shown = self.columns[i].map_or(0, |r| r.size.h);
        (self.content[i] - shown).max(0)
    }

    /// Where the repositories' column should stand, scrolled `down` now,
    /// to show repository `i`'s row whole: `down` when it shows already,
    /// else the nearest that does (the column's top for the first, so its
    /// caption shows too, and its end for the last). `None` on a narrow tab, whose strip of chips
    /// shows the selected one by itself.
    pub fn reveal_repo(&self, i: usize, down: f64) -> Option<f64> {
        let place = self.columns[TabColumn::Repos as usize]?;
        let (row, _) = self.repos.get(i)?;
        let top = f64::from(row.loc.y - place.loc.y);
        let bottom = top + f64::from(row.size.h);
        let view = f64::from(place.size.h);
        let room = f64::from(self.scroll_room(TabColumn::Repos));
        let to = if i == 0 {
            0.0
        } else if i + 1 == self.repos.len() && bottom > down + view {
            room
        } else if top - f64::from(CARD_PAD) < down {
            top - f64::from(CARD_PAD)
        } else if bottom + f64::from(CARD_PAD) > down + view {
            bottom + f64::from(CARD_PAD) - view
        } else {
            return Some(down);
        };
        Some(to.clamp(0.0, room))
    }

    /// The column whose place holds `pos`: where the wheel goes.
    pub fn column_at(&self, pos: Point<i32, Logical>) -> Option<TabColumn> {
        TabColumn::ALL
            .into_iter()
            .find(|c| self.columns[*c as usize].is_some_and(|r| r.contains(pos)))
    }

    /// The scrollbar of `column` scrolled `down`, while its contents
    /// don't all show: to paint it and to hold its thumb.
    pub fn thumb(&self, column: TabColumn, down: f64) -> Option<Thumb> {
        let place = self.columns[column as usize]?;
        let list = Rectangle::new(
            (place.loc.x, place.loc.y + BAR_INSET).into(),
            (place.size.w, place.size.h - 2 * BAR_INSET).into(),
        );
        let track = gutter_track(place.loc.x + place.size.w - BAR_INSET - 5, list);
        Thumb::of(
            track,
            place.size.h as f64,
            ListBar {
                down,
                total: self.content[column as usize] as f64,
            },
        )
    }
}

// ------------------------------------------------------------ feature tab

/// The room between the tab's edges and what is on it.
const TAB_PAD: i32 = 16;
/// The feature's title, in the header.
const TAB_TITLE_FONT: f32 = 20.0;
/// How high the header's title and sub lines are.
const TAB_TITLE: i32 = QUIT_TITLE + 4;
const TAB_SUB: i32 = 20;
/// How high the header's buttons and the page's buttons are.
const TAB_BUTTON: i32 = 30;
/// The header's buttons show only their icons on a tab narrower than
/// this, each `TAB_BUTTON` square.
pub const COMPACT_BUTTONS_BELOW: i32 = 1000;
/// The gap between the header and the rail, and under the rail.
const RAIL_GAP: i32 = 14;
/// A rail node's diameter, the gap under it, and the heights of the
/// stage's label and short line under that.
const NODE: i32 = 24;
const NODE_GAP: i32 = 6;
const NODE_LABEL: i32 = 18;
const NODE_LINE: i32 = 16;
/// The room left between a node and the line joining it to the next.
const CONNECTOR_GAP: i32 = 6;
/// How wide the repositories' and activity's columns are: the page
/// between them takes the rest.
pub const REPOS_W: i32 = 220;
pub const ACTIVITY_W: i32 = 240;
/// Narrower than this, the activity has no column: its rows go at the
/// end of the page.
pub const ACTIVITY_BELOW: i32 = 1180;
/// Narrower than this (O4), the repositories are a row of chips under
/// the rail.
pub const REPOS_BELOW: i32 = 870;
/// The gap between the columns' cards, the room inside them, and their
/// corners.
const COLUMN_GAP: i32 = 12;
const CARD_PAD: i32 = 12;
const CARD_RADIUS: f32 = 8.0;
/// How far in from a card's top and bottom its scrollbar runs.
const BAR_INSET: i32 = 6;
/// The row of repository chips on a narrow tab, and a chip's height,
/// inner room and the gap between two.
const CHIP_ROW: i32 = 28;
const CHIP_PAD: i32 = 10;
const CHIP_GAP: i32 = 6;
/// A column's uppercase caption.
const CAPTION: i32 = 22;
/// A repository's row (its name, then its branch under it) and the gap
/// between two.
const REPO_ROW: i32 = 42;
const REPO_GAP: i32 = 4;
/// Room for a dot left of a row's text.
const DOT_ROOM: i32 = 18;
/// A line of small text, of body text, and of an agent's pane.
const SMALL_LINE: i32 = 20;
const BODY_LINE: i32 = 22;
const TAIL_LINE: i32 = 18;
/// The gap between the parts of a page.
const PAGE_GAP: i32 = 16;
/// An agent card's inner room and its header's height.
const AGENT_PAD: i32 = 10;
const AGENT_HEAD: i32 = 28;
const AGENT_RADIUS: f32 = 6.0;
/// A milestone's row, and the progress bar's height.
const MILESTONE_ROW: i32 = 22;
const PROGRESS: i32 = 6;
/// The most lines a wrapped text takes; the last ends in "…".
const MAX_WRAP: usize = 4;

/// What a feature tab's text is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Font {
    Heading,
    Body,
    Small,
    Detail,
}

impl Font {
    fn size(self) -> f32 {
        match self {
            Self::Heading => LABEL_FONT_SIZE,
            Self::Body => FONT_SIZE,
            Self::Small => SMALL_FONT_SIZE,
            Self::Detail => DETAIL_FONT_SIZE,
        }
    }
}

/// What colour a piece of text is, from the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ink {
    Text,
    Muted,
    Error,
    Tone(CheckTone),
}

impl Ink {
    fn color(self, p: &Palette) -> Rgb {
        match self {
            Self::Text => p.text,
            Self::Muted => p.muted,
            Self::Error => p.error,
            Self::Tone(tone) => tone.color(p),
        }
    }
}

/// A thing a feature tab's column paints, in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Piece {
    /// A section's uppercase label.
    Caption(String),
    Text {
        text: String,
        font: Font,
        ink: Ink,
        bold: bool,
    },
    /// The milestones' progress.
    Bar { done: usize, total: usize },
    /// A milestone: done, the next one, or one still to come.
    Milestone {
        done: bool,
        next: bool,
        text: String,
    },
    /// The agent card of `StagePage::sections[i]`.
    Agent(usize),
    /// `FeatureTabView::repos[i]`.
    Repo(usize),
    /// `StagePage::buttons[i]`, not on an agent card.
    Button(usize),
    /// `FeatureTabView::activity[row]`, its `what` wrapped in `lines`.
    Activity { row: usize, lines: Vec<String> },
}

/// How wide `text` is at `size`, in logical pixels, guessed when there is
/// no font.
fn width_of(text: &str, size: f32) -> f32 {
    measure(text, size).unwrap_or(text.chars().count() as f32 * size * 0.55)
}

/// `text` broken into lines no wider than `width` at `size`: at most
/// `MAX_WRAP`, the last taking the rest (cut short when painted).
fn wrap(text: &str, size: f32, width: i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let longer = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || lines.len() + 1 == MAX_WRAP || width_of(&longer, size) <= width as f32
        {
            line = longer;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// What a page button says: its label, then its key.
/// The UI font has neither "⏎" nor "↵": a button's key shows it as
/// "Enter", as the hints do.
fn page_label(button: &PageButton) -> String {
    match button.key {
        Some(key) => format!("{}  {}", button.label, key.replace('\u{23ce}', "Enter")),
        None => button.label.clone(),
    }
}

/// A column's contents, stacked from its top.
struct Stack {
    x: i32,
    w: i32,
    y: i32,
    pieces: Vec<(Rect, Piece)>,
}

impl Stack {
    fn put(&mut self, h: i32, piece: Piece) -> Rect {
        let rect = Rectangle::new((self.x, self.y).into(), (self.w, h).into());
        self.pieces.push((rect, piece));
        self.y += h;
        rect
    }

    fn caption(&mut self, text: &str) {
        self.put(CAPTION, Piece::Caption(text.to_uppercase()));
    }

    /// `text` wrapped to the column, a line `h` high each.
    fn text(&mut self, text: &str, font: Font, ink: Ink, h: i32) {
        for line in wrap(text, font.size(), self.w) {
            self.put(
                h,
                Piece::Text {
                    text: line,
                    font,
                    ink,
                    bold: false,
                },
            );
        }
    }

    /// The activity's rows, newest first.
    fn activity(&mut self, rows: &[ActivityRow]) {
        for (row, a) in rows.iter().enumerate() {
            let ago = width_of(&a.ago, Font::Detail.size()).ceil() as i32 + 8;
            let lines = wrap(&a.what, Font::Small.size(), self.w - DOT_ROOM - ago);
            let h = lines.len() as i32 * TAIL_LINE + 6;
            self.put(h, Piece::Activity { row, lines });
        }
    }
}

/// The repositories' column, from its caption.
fn repo_stack(view: &FeatureTabView, s: &mut Stack, repos: &mut Vec<(Rect, Option<Rect>)>) {
    s.caption("Repositories");
    if let Some(why) = &view.repos_error {
        s.text(
            &format!("couldn't read: {why}"),
            Font::Small,
            Ink::Error,
            SMALL_LINE,
        );
        return;
    }
    for (i, repo) in view.repos.iter().enumerate() {
        let row = s.put(REPO_ROW, Piece::Repo(i));
        s.y += REPO_GAP;
        let button = repo.button.filter(|_| !repo.busy).map(|label| {
            let w = button_width(label);
            Rectangle::new(
                (
                    row.loc.x + row.size.w - w - 4,
                    row.loc.y + (REPO_ROW - ROW_BUTTON) / 2,
                )
                    .into(),
                (w, ROW_BUTTON).into(),
            )
        });
        repos.push((row, button));
    }
}

/// The page's column: the selected stage's title, about, status, buttons
/// and sections, then the activity if it has no column.
fn page_stack(view: &FeatureTabView, s: &mut Stack, buttons: &mut [Option<Rect>], activity: bool) {
    let page = &view.page;
    s.put(
        BODY_LINE + 6,
        Piece::Text {
            text: page.title.clone(),
            font: Font::Heading,
            ink: Ink::Text,
            bold: true,
        },
    );
    s.text(&page.about, Font::Small, Ink::Muted, SMALL_LINE);
    if let Some(status) = &page.status {
        s.y += 4;
        s.text(&status.text, Font::Body, Ink::Tone(status.tone), BODY_LINE);
    }
    // The buttons an agent card doesn't hold, wrapped in rows.
    let held: Vec<usize> = page
        .sections
        .iter()
        .filter_map(|sec| match sec {
            PageSection::Agent { button, .. } => *button,
            _ => None,
        })
        .collect();
    let free: Vec<usize> = (0..page.buttons.len())
        .filter(|i| !held.contains(i))
        .collect();
    if !free.is_empty() {
        s.y += 10;
        let mut x = s.x;
        for i in free {
            let w = button_width(&page_label(&page.buttons[i]));
            if x > s.x && x + w > s.x + s.w {
                x = s.x;
                s.y += TAB_BUTTON + QUIT_BUTTON_GAP;
            }
            let rect = Rectangle::new((x, s.y).into(), (w, TAB_BUTTON).into());
            buttons[i] = Some(rect);
            s.pieces.push((rect, Piece::Button(i)));
            x += w + QUIT_BUTTON_GAP;
        }
        s.y += TAB_BUTTON;
    }
    for (k, section) in page.sections.iter().enumerate() {
        s.y += PAGE_GAP;
        match section {
            PageSection::Agent { tail, button, .. } => {
                let tail_h = match tail.len() {
                    0 => 0,
                    n => n as i32 * TAIL_LINE + 4,
                };
                let card = s.put(AGENT_PAD * 2 + AGENT_HEAD + tail_h, Piece::Agent(k));
                if let Some(b) = button.filter(|b| *b < page.buttons.len()) {
                    let w = button_width(&page_label(&page.buttons[b]));
                    buttons[b] = Some(Rectangle::new(
                        (
                            card.loc.x + card.size.w - AGENT_PAD - w,
                            card.loc.y + AGENT_PAD + (AGENT_HEAD - ROW_BUTTON) / 2,
                        )
                            .into(),
                        (w, ROW_BUTTON).into(),
                    ));
                }
            }
            PageSection::Lines { label, lines } => {
                if !label.is_empty() {
                    s.caption(label);
                }
                for line in lines {
                    s.text(&line.text, Font::Body, Ink::Tone(line.tone), BODY_LINE);
                }
            }
            PageSection::Milestones { done, total, items } => {
                s.caption(&format!("Milestones \u{b7} {done} of {total}"));
                s.y += 2;
                s.put(
                    PROGRESS,
                    Piece::Bar {
                        done: *done,
                        total: *total,
                    },
                );
                s.y += 8;
                let next = items.iter().position(|(d, _)| !d);
                for (i, (done, text)) in items.iter().enumerate() {
                    s.put(
                        MILESTONE_ROW,
                        Piece::Milestone {
                            done: *done,
                            next: next == Some(i),
                            text: text.clone(),
                        },
                    );
                }
            }
        }
    }
    if activity && !view.activity.is_empty() {
        s.y += PAGE_GAP;
        s.caption("Activity");
        s.activity(&view.activity);
    }
}

/// Where the feature tab's parts go in `frame`, its tab's area: a header,
/// the stages' rail, then the repositories', page's and activity's
/// columns, and the hints at the foot.
pub fn feature_tab_layout(view: &FeatureTabView, frame: Rect) -> FeatureTabLayout {
    let width = frame.size.w;
    let x = frame.loc.x + TAB_PAD;
    let inner = (width - 2 * TAB_PAD).max(1);
    let right = x + inner;
    let top = frame.loc.y + TAB_PAD;
    let header_h = TAB_TITLE + TAB_SUB;

    // The header's buttons from the right: the one removing worktrees
    // last, then the switch left of them.
    let compact = width < COMPACT_BUTTONS_BELOW;
    let labels: Vec<&str> = FEATURE_BUTTONS
        .iter()
        .copied()
        .chain([view.remove])
        .collect();
    let by = top + (header_h - TAB_BUTTON) / 2;
    let mut bx = right;
    let mut buttons = vec![Rect::default(); labels.len()];
    for (i, label) in labels.iter().enumerate().rev() {
        let w = if compact {
            TAB_BUTTON
        } else {
            button_width(label)
        };
        bx -= w;
        buttons[i] = Rectangle::new((bx, by).into(), (w, TAB_BUTTON).into());
        bx -= QUIT_BUTTON_GAP;
    }
    let switch = view.which.filter(|(_, n)| *n > 1).map(|(i, n)| {
        let count_w = width_of(&format!("{} of {n}", i + 1), FONT_SIZE).ceil() as i32 + 12;
        let next = Rectangle::new(
            (bx - 8 - SWITCH_W, by).into(),
            (SWITCH_W, TAB_BUTTON).into(),
        );
        let count = Rectangle::new(
            (next.loc.x - count_w, by).into(),
            (count_w, TAB_BUTTON).into(),
        );
        let prev = Rectangle::new(
            (count.loc.x - SWITCH_W, by).into(),
            (SWITCH_W, TAB_BUTTON).into(),
        );
        bx = prev.loc.x - 8;
        (count, prev, next)
    });
    let text_w = (bx - PLACE_GAP - x).max(1);
    let title = Rectangle::new((x, top).into(), (text_w, TAB_TITLE).into());
    let sub = Rectangle::new((x, top + TAB_TITLE).into(), (text_w, TAB_SUB).into());

    // The rail: a slot per stage, its node at the slot's top middle.
    let rail_top = top + header_h + RAIL_GAP;
    let n = view.rail.len().max(1) as i32;
    let rail_h = NODE + NODE_GAP + NODE_LABEL + NODE_LINE;
    let nodes = (0..view.rail.len() as i32)
        .map(|i| {
            let left = x + inner * i / n;
            let next = x + inner * (i + 1) / n;
            Rectangle::new((left, rail_top).into(), (next - left, rail_h).into())
        })
        .collect();
    // Under the rail's rule, the columns.
    let mut body_top = rail_top + rail_h + RAIL_GAP + 1 + COLUMN_GAP;

    let hints_h = measured_footer_height(&hint_strs(&view.hints), width);
    let hints = Rectangle::new(
        (frame.loc.x, frame.loc.y + frame.size.h - hints_h).into(),
        (width, hints_h).into(),
    );
    let hint_spots = footer_hint_spots(&hint_strs(&view.hints), hints);

    let mut repos = Vec::new();
    let strip = (width < REPOS_BELOW).then(|| {
        let strip = Rectangle::new((x, body_top).into(), (inner, CHIP_ROW).into());
        body_top += CHIP_ROW + COLUMN_GAP;
        strip
    });
    if let Some(strip) = strip
        && view.repos_error.is_none()
    {
        // The chips that fit, from the selected one if it would be past
        // them; the selected one's button at the strip's end.
        let button = view
            .repos
            .get(view.repo)
            .and_then(|r| r.button.filter(|_| !r.busy))
            .map(|label| {
                let w = button_width(label);
                Rectangle::new(
                    (right - w, strip.loc.y + (CHIP_ROW - ROW_BUTTON) / 2).into(),
                    (w, ROW_BUTTON).into(),
                )
            });
        let end = button.map_or(right, |b| b.loc.x - QUIT_BUTTON_GAP);
        let chip_w = |r: &FeatureRepoRow| {
            width_of(&r.name, SMALL_FONT_SIZE).ceil() as i32 + DOT_ROOM + 2 * CHIP_PAD
        };
        let fits = |from: usize| {
            let mut cx = x;
            view.repos[from..]
                .iter()
                .take_while(|r| {
                    cx += chip_w(r) + CHIP_GAP;
                    cx - CHIP_GAP <= end
                })
                .count()
        };
        let first = if view.repo < view.repos.len() && view.repo >= fits(0) {
            view.repo
        } else {
            0
        };
        let shown = first..first + fits(first);
        let mut cx = x;
        for (i, repo) in view.repos.iter().enumerate() {
            if !shown.contains(&i) {
                repos.push((Rect::default(), None));
                continue;
            }
            let w = chip_w(repo);
            let chip = Rectangle::new(
                (cx, strip.loc.y + (CHIP_ROW - ROW_BUTTON) / 2).into(),
                (w, ROW_BUTTON).into(),
            );
            cx += w + CHIP_GAP;
            repos.push((chip, None));
        }
        // The selected one's button even when its chip doesn't show.
        if let Some(slot) = repos.get_mut(view.repo) {
            slot.1 = button;
        }
    }

    let bottom = hints.loc.y - 4;
    let col_h = (bottom - body_top).max(0);
    let card = |cx: i32, w: i32| Rectangle::new((cx, body_top).into(), (w, col_h).into());
    let mut columns = [None; 3];
    let mut page_x = x;
    let mut page_w = inner;
    if strip.is_none() {
        columns[TabColumn::Repos as usize] = Some(card(x, REPOS_W));
        page_x += REPOS_W + COLUMN_GAP;
        page_w -= REPOS_W + COLUMN_GAP;
    }
    if width >= ACTIVITY_BELOW {
        columns[TabColumn::Activity as usize] = Some(card(right - ACTIVITY_W, ACTIVITY_W));
        page_w -= ACTIVITY_W + COLUMN_GAP;
    }
    columns[TabColumn::Page as usize] = Some(card(page_x, page_w.max(1)));

    let mut pieces: [Vec<(Rect, Piece)>; 3] = Default::default();
    let mut content = [0; 3];
    let mut page_buttons = vec![None; view.page.buttons.len()];
    for column in TabColumn::ALL {
        let Some(place) = columns[column as usize] else {
            continue;
        };
        let mut s = Stack {
            x: place.loc.x + CARD_PAD,
            w: (place.size.w - 2 * CARD_PAD).max(1),
            y: place.loc.y + CARD_PAD,
            pieces: Vec::new(),
        };
        match column {
            TabColumn::Repos => repo_stack(view, &mut s, &mut repos),
            TabColumn::Page => page_stack(
                view,
                &mut s,
                &mut page_buttons,
                columns[TabColumn::Activity as usize].is_none(),
            ),
            TabColumn::Activity => {
                s.caption("Activity");
                s.activity(&view.activity);
            }
        }
        content[column as usize] = s.y + CARD_PAD - place.loc.y;
        pieces[column as usize] = s.pieces;
    }

    FeatureTabLayout {
        frame,
        title,
        sub,
        switch,
        buttons,
        compact,
        nodes,
        columns,
        content,
        strip,
        repos,
        page_buttons,
        hints,
        hint_spots,
        pieces,
    }
}

impl Painter {
    /// The new feature dialog for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow: the title field, the root, a
    /// checkbox row per repository shown (from `top`), Cancel and Create.
    pub(super) fn new_feature_dialog(
        &self,
        view: &NewFeatureView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = new_feature_layout(size, view.repos.len(), view.error.is_some());
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let s = scale as f32;
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let line = |c: &mut Canvas, text: &str, rect: Rect, size: f32, color: Rgb| {
            let rect = shift(rect);
            let (x, end) = phys(rect, scale);
            self.text(c, text, size, x, end, band(rect, scale), scale, color)
        };
        line(&mut c, &view.heading, layout.title, LABEL_FONT_SIZE, p.text);
        self.field(
            &mut c,
            &view.title,
            "What it does, in a few words",
            shift(layout.field),
            view.focus.is_none(),
            scale,
            p,
        );
        if let (Some(rect), Some(error)) = (layout.error, &view.error) {
            line(&mut c, error, rect, FONT_SIZE, p.error);
        }
        let root_color = if view.root_failed { p.error } else { p.muted };
        line(&mut c, &view.root, layout.root, FONT_SIZE, root_color);
        let mut projects = layout.projects;
        if let Some(label) = view.link {
            projects.size.w = layout.link.loc.x - projects.loc.x - QUIT_BUTTON_GAP;
            let hovered = view.hover == Some(NewFeatureHit::Link);
            self.button(
                &mut c,
                label,
                shift(layout.link),
                (hovered, false),
                ButtonStyle::Plain,
                scale,
                p,
            );
        }
        line(&mut c, &view.projects, projects, FONT_SIZE, p.muted);

        let shown = view.repos.iter().enumerate().skip(view.top);
        for (rect, (i, repo)) in layout.rows.iter().zip(shown) {
            let rect = shift(*rect);
            let hovered = view.hover == Some(NewFeatureHit::Repo(i)) && !repo.fixed;
            self.row_highlight(
                &mut c,
                rect,
                (view.focus == Some(i), if hovered { 1.0 } else { 0.0 }),
                scale,
                p,
            );
            let check = Rectangle::new(
                (rect.loc.x + ROW_INSET, rect.loc.y).into(),
                (rect.size.w - ROW_INSET, rect.size.h).into(),
            );
            let state = if repo.touched { Check::On } else { Check::Off };
            let box_end = self.checkbox(&mut c, check, state, false, scale, p);
            let end = phys(rect, scale).1 - (ROW_INSET as f32 * s).round();
            let row_band = band(rect, scale);
            let after = self.text(
                &mut c,
                &repo.name,
                FONT_SIZE,
                box_end + (CHECKBOX_GAP as f32 * s).round(),
                end,
                row_band,
                scale,
                if repo.touched { p.text } else { p.muted },
            );
            self.text(
                &mut c,
                &repo.place,
                FONT_SIZE,
                after + (PLACE_GAP as f32 * s).round(),
                end,
                row_band,
                scale,
                p.placeholder,
            );
        }

        // Left of the buttons, how many repositories don't show, else the
        // keys besides the buttons; then Cancel and Create, the default.
        let first = shift(layout.buttons[0]);
        let left = shift(layout.field).loc.x;
        let end = first.loc.x - QUIT_BUTTON_GAP;
        match crate::restart::more_rows(view.top, layout.rows.len(), view.repos.len()) {
            Some(more) => {
                let spot = Rectangle::new(
                    (left, first.loc.y).into(),
                    (end - left, first.size.h).into(),
                );
                let (x, end) = phys(spot, scale);
                self.text(
                    &mut c,
                    &more,
                    FONT_SIZE,
                    x,
                    end,
                    band(spot, scale),
                    scale,
                    p.muted,
                );
            }
            None => {
                let row = Rectangle::new((left, first.loc.y).into(), (0, first.size.h).into());
                self.dialog_keys(&mut c, &view.keys, row, end, scale, p);
            }
        }
        let labels = [NEW_FEATURE_BUTTONS[0], view.go];
        for (i, (rect, label)) in layout.buttons.iter().zip(labels).enumerate() {
            let style = if i == CREATE {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(NewFeatureHit::Button(i));
            self.button(
                &mut c,
                label,
                shift(*rect),
                (hovered, false),
                style,
                scale,
                p,
            );
        }
        c.into_buffer(false)
    }

    /// The feature tab filling `output`, its tab's area: the header, the
    /// stages' rail, the columns each clipped and scrolled on its own,
    /// and the hints.
    pub(super) fn feature_tab(
        &self,
        view: &FeatureTabView,
        output: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let l = feature_tab_layout(view, Rectangle::from_size(output));
        let mut c = Canvas::new(physical(output, scale));
        let s = scale as f32;
        let px = |v: i32| (v as f64 * scale).round() as i32;
        c.fill(0, 0, c.w, c.h, p.background);

        // The header: the title and sub line, the switch, the buttons.
        let (tx, tend) = phys(l.title, scale);
        self.bold_text(
            &mut c,
            &view.title,
            TAB_TITLE_FONT,
            tx,
            tend,
            band(l.title, scale),
            scale,
            p.text,
        );
        let (sx, send) = phys(l.sub, scale);
        self.text(
            &mut c,
            &view.sub,
            DETAIL_FONT_SIZE,
            sx,
            send,
            band(l.sub, scale),
            scale,
            p.muted,
        );
        if let (Some((count, prev, next)), Some((i, n))) = (l.switch, view.which) {
            self.centered(
                &mut c,
                &format!("{} of {n}", i + 1),
                FONT_SIZE,
                count,
                scale,
                p.muted,
            );
            for (rect, left, hit) in [
                (prev, true, FeatureTabHit::Prev),
                (next, false, FeatureTabHit::Next),
            ] {
                let hovered = view.hover == Some(hit);
                self.button(
                    &mut c,
                    "",
                    rect,
                    (hovered, false),
                    ButtonStyle::Plain,
                    scale,
                    p,
                );
                let color = if hovered { p.text } else { p.muted };
                icon_sdf(&mut c, rect, scale, color, |x, y| {
                    chevron_side(left, x, y, s)
                });
            }
        }
        let labels = FEATURE_BUTTONS.iter().copied().chain([view.remove]);
        for (i, (rect, label)) in l.buttons.iter().zip(labels).enumerate() {
            let destructive = i == FEATURE_BUTTONS.len();
            let style = if destructive {
                ButtonStyle::Destructive
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(FeatureTabHit::Button(i));
            if !l.compact {
                self.button(&mut c, label, *rect, (hovered, false), style, scale, p);
                continue;
            }
            self.button(&mut c, "", *rect, (hovered, false), style, scale, p);
            let color = if destructive { p.on_accent } else { p.text };
            icon_sdf(&mut c, *rect, scale, color, |x, y| match i {
                0 => tab(x, y, s * 0.8),
                1 => copy(x, y, s * 1.2),
                2 => plus(x, y, 0.0, 0.0, 4.5 * s, s),
                _ => trash(x, y, s),
            });
        }

        // The rail: the lines joining the nodes, then each node with its
        // label and line under it.
        let circle = |slot: Rect| {
            Rectangle::new(
                (slot.loc.x + (slot.size.w - NODE) / 2, slot.loc.y).into(),
                (NODE, NODE).into(),
            )
        };
        // The hovered node's band first: the connectors run over it.
        if let Some(FeatureTabHit::Node(i)) = view.hover
            && let Some(slot) = l.nodes.get(i)
        {
            self.row_highlight(&mut c, *slot, (false, 1.0), scale, p);
        }
        for (i, pair) in l.nodes.windows(2).enumerate() {
            let (a, b) = (circle(pair[0]), circle(pair[1]));
            let from = a.loc.x + NODE + CONNECTOR_GAP;
            let to = b.loc.x - CONNECTOR_GAP;
            let done = matches!(
                view.rail.get(i).map(|n| n.mark),
                Some(NodeMark::Done | NodeMark::Stale)
            );
            let color = if done { p.success } else { p.border };
            if to > from {
                c.fill(
                    px(from),
                    px(a.loc.y + NODE / 2 - 1),
                    px(to) - px(from),
                    px(2).max(1),
                    color,
                );
            }
        }
        for (i, (slot, node)) in l.nodes.iter().zip(&view.rail).enumerate() {
            let hovered = view.hover == Some(FeatureTabHit::Node(i));
            let spot = circle(*slot);
            if node.selected {
                let ring = if view.focus == TabArea::Stages {
                    p.accent
                } else {
                    p.border
                };
                let behind = if hovered {
                    super::row_fill(p, false, 1.0).unwrap_or(p.background)
                } else {
                    p.background
                };
                focus_ring(&mut c, spot, NODE as f32 / 2.0, behind, scale, ring);
            }
            self.rail_node(&mut c, spot, i + 1, node.mark, scale, p);
            let label = Rectangle::new(
                (slot.loc.x + 2, spot.loc.y + NODE + NODE_GAP).into(),
                (slot.size.w - 4, NODE_LABEL).into(),
            );
            let label_color = match node.mark {
                NodeMark::Later if !node.selected => p.muted,
                _ => p.text,
            };
            self.centred_cut(
                &mut c,
                &node.label,
                SMALL_FONT_SIZE,
                label,
                node.selected,
                scale,
                label_color,
            );
            if !node.line.is_empty() {
                let line = Rectangle::new(
                    (label.loc.x, label.loc.y + NODE_LABEL).into(),
                    (label.size.w, NODE_LINE).into(),
                );
                let color = match node.mark {
                    NodeMark::Current => p.accent,
                    NodeMark::Waiting | NodeMark::Stale => p.warning,
                    _ => p.muted,
                };
                self.centred_cut(
                    &mut c,
                    &node.line,
                    DETAIL_FONT_SIZE,
                    line,
                    false,
                    scale,
                    color,
                );
            }
        }
        // The rule under the rail.
        if let Some(last) = l.nodes.first() {
            let y = last.loc.y + last.size.h + RAIL_GAP;
            c.fill(0, px(y), c.w, px(1).max(1), p.border_variant);
        }

        // On a narrow tab, the repositories as chips.
        if let Some(strip) = l.strip {
            self.repo_strip(&mut c, view, &l, strip, scale, p);
        }

        // The columns, each a card scrolled on its own.
        for column in TabColumn::ALL {
            let Some(place) = l.columns[column as usize] else {
                continue;
            };
            let (x, y, w, h) = rect_px(place, scale);
            let b = s.round().max(1.0);
            c.rounded(x, y, w, h, CARD_RADIUS * s, p.border_variant);
            c.rounded(
                x + b,
                y + b,
                w - 2.0 * b,
                h - 2.0 * b,
                CARD_RADIUS * s - b,
                p.panel,
            );
            let down = view.scroll[column as usize];
            let dy = down.round() as i32;
            let (cx, cy, cw, ch) = (
                x as i32,
                y as i32 + b as i32,
                w as i32,
                h as i32 - 2 * b as i32,
            );
            c.clipped(cx, cy, cw, ch, |c| {
                for (rect, piece) in &l.pieces[column as usize] {
                    let rect = Rectangle::new((rect.loc.x, rect.loc.y - dy).into(), rect.size);
                    if rect.loc.y > place.loc.y + place.size.h
                        || rect.loc.y + rect.size.h < place.loc.y
                    {
                        continue;
                    }
                    self.tab_piece(c, view, &l, rect, piece, dy, scale, p);
                }
            });
            if let Some(thumb) = l.thumb(column, down) {
                self.thumb_in(&mut c, &thumb, scale, p);
            }
        }

        let hover = match view.hover {
            Some(FeatureTabHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(
            &mut c,
            &hint_strs(&view.hints),
            &l.hint_spots,
            hover,
            scale,
            p,
        );
        c.into_buffer(true)
    }

    /// `text` centred in `rect`, or from its left and cut short with "…"
    /// when it is wider; in bold when `bold`.
    #[allow(clippy::too_many_arguments)]
    fn centred_cut(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        rect: Rect,
        bold: bool,
        scale: f64,
        color: Rgb,
    ) {
        let (x, end) = phys(rect, scale);
        let w = if bold {
            self.bold_width(text, size, scale)
        } else {
            self.text_width(text, size, scale)
        };
        let from = (x + ((end - x) - w) / 2.0).max(x).round();
        if bold {
            self.bold_text(c, text, size, from, end, band(rect, scale), scale, color);
        } else {
            self.text(c, text, size, from, end, band(rect, scale), scale, color);
        }
    }

    /// A rail node in `spot` for stage `number`, marked `mark`.
    fn rail_node(
        &self,
        c: &mut Canvas,
        spot: Rect,
        number: usize,
        mark: NodeMark,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(spot, scale);
        let r = w / 2.0;
        let ring = (1.5 * s).round().max(1.0);
        let disc = |c: &mut Canvas, by: f32, color: Rgb| {
            c.rounded(x + by, y + by, w - 2.0 * by, h - 2.0 * by, r - by, color)
        };
        let number = number.to_string();
        match mark {
            NodeMark::Done | NodeMark::Stale => {
                let color = if mark == NodeMark::Done {
                    p.success
                } else {
                    p.warning
                };
                disc(c, 0.0, color);
                disc(c, ring, mix(p.background, color, 0.18));
                icon_sdf(c, spot, scale, color, |x, y| check(x, y, s * 1.1));
            }
            NodeMark::Current | NodeMark::Waiting => {
                let color = if mark == NodeMark::Current {
                    p.accent
                } else {
                    p.warning
                };
                disc(c, 0.0, color);
                self.bold_centred(c, &number, spot, scale, p.on_accent);
            }
            NodeMark::Later => {
                disc(c, 0.0, p.border);
                disc(c, ring, p.background);
                self.bold_centred(c, &number, spot, scale, p.muted);
            }
        }
    }

    /// `text` in bold, small, centred on `rect`.
    fn bold_centred(&self, c: &mut Canvas, text: &str, rect: Rect, scale: f64, color: Rgb) {
        let (x, end) = phys(rect, scale);
        let w = self.bold_width(text, SMALL_FONT_SIZE, scale);
        let from = (x + ((end - x) - w) / 2.0).round();
        self.bold_text(
            c,
            text,
            SMALL_FONT_SIZE,
            from,
            end,
            band(rect, scale),
            scale,
            color,
        );
    }

    /// The repositories as a row of chips in `strip`, or why they
    /// couldn't be read.
    fn repo_strip(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        strip: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        if let Some(why) = &view.repos_error {
            let (x, end) = phys(strip, scale);
            let text = format!("couldn't read: {why}");
            self.text(
                c,
                &text,
                SMALL_FONT_SIZE,
                x,
                end,
                band(strip, scale),
                scale,
                p.error,
            );
            return;
        }
        for (i, ((chip, button), repo)) in l.repos.iter().zip(&view.repos).enumerate() {
            if chip.size.w > 0 {
                let selected = i == view.repo && view.focus == TabArea::Repos;
                let lit = matches!(view.hover, Some(FeatureTabHit::Repo(r)) if r == i);
                let (x, y, w, h) = rect_px(*chip, scale);
                let fill =
                    super::row_fill(p, selected, if lit { 1.0 } else { 0.0 }).unwrap_or(p.panel);
                let b = s.round().max(1.0);
                // The keyboard's chip has the accent's outline.
                let edge = if selected { p.accent } else { p.border_variant };
                c.rounded(x, y, w, h, h / 2.0, edge);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, h / 2.0 - b, fill);
                let dot = Rectangle::new(
                    (chip.loc.x + CHIP_PAD, chip.loc.y + (chip.size.h - 8) / 2).into(),
                    (8, 8).into(),
                );
                self.repo_dot(c, dot, repo, scale, p);
                let text = Rectangle::new(
                    (chip.loc.x + CHIP_PAD + DOT_ROOM - 4, chip.loc.y).into(),
                    (chip.size.w - CHIP_PAD - DOT_ROOM + 4, chip.size.h).into(),
                );
                let (tx, tend) = phys(text, scale);
                let color = if repo.touched { p.text } else { p.muted };
                self.text(
                    c,
                    &repo.name,
                    SMALL_FONT_SIZE,
                    tx,
                    tend,
                    band(text, scale),
                    scale,
                    color,
                );
            }
            if let (Some(b), Some(label)) = (button, repo.button) {
                let hovered = view.hover == Some(FeatureTabHit::RepoButton(i));
                self.button(c, label, *b, (hovered, false), ButtonStyle::Plain, scale, p);
            }
        }
    }

    /// A repository's dot in `spot`: touched in the success colour, being
    /// touched in the accent, else a quiet one.
    fn repo_dot(&self, c: &mut Canvas, spot: Rect, repo: &FeatureRepoRow, scale: f64, p: &Palette) {
        let color = if repo.busy {
            p.accent
        } else if repo.touched {
            p.success
        } else {
            // Between the two, to show on a selected row's fill too.
            mix(p.border, p.muted, 0.4)
        };
        let d = spot.size.w as f32 * scale as f32;
        icon_sdf(c, spot, scale, color, |x, y| x.hypot(y) - d / 2.0);
    }

    /// One of a column's pieces, in `rect` (scrolled by `dy`).
    #[allow(clippy::too_many_arguments)]
    fn tab_piece(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        rect: Rect,
        piece: &Piece,
        dy: i32,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let at = |r: Rect| Rectangle::new((r.loc.x, r.loc.y - dy).into(), r.size);
        let line = |c: &mut Canvas, text: &str, r: Rect, size: f32, color: Rgb| {
            let (x, end) = phys(r, scale);
            self.text(c, text, size, x, end, band(r, scale), scale, color)
        };
        match piece {
            Piece::Caption(text) => {
                let focused = view.focus == TabArea::Repos && text == "REPOSITORIES";
                let color = if focused { p.accent } else { p.muted };
                line(c, text, rect, DETAIL_FONT_SIZE, color);
            }
            Piece::Text {
                text,
                font,
                ink,
                bold,
            } => {
                let (x, end) = phys(rect, scale);
                let b = band(rect, scale);
                if *bold {
                    self.bold_text(c, text, font.size(), x, end, b, scale, ink.color(p));
                } else {
                    self.text(c, text, font.size(), x, end, b, scale, ink.color(p));
                }
            }
            Piece::Bar { done, total } => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, h / 2.0, p.element);
                if *total > 0 && *done > 0 {
                    let f = (*done).min(*total) as f32 / *total as f32;
                    c.rounded(x, y, (w * f).max(h), h, h / 2.0, p.accent);
                }
            }
            Piece::Milestone { done, next, text } => {
                let mark = Rectangle::new((rect.loc.x, rect.loc.y + 4).into(), (14, 14).into());
                let (stage, color) = match (done, next) {
                    (true, _) => (StageMark::Done, p.success),
                    (false, true) => (StageMark::Current, p.accent),
                    _ => (StageMark::Later, p.muted),
                };
                match stage {
                    StageMark::Done => icon_sdf(c, mark, scale, color, |x, y| check(x, y, s)),
                    _ => icon_sdf(c, mark, scale, color, |x, y| {
                        stage_dot(stage, x, y, 9.0 * s)
                    }),
                }
                let text_rect = Rectangle::new(
                    (rect.loc.x + DOT_ROOM + 4, rect.loc.y).into(),
                    (rect.size.w - DOT_ROOM - 4, rect.size.h).into(),
                );
                let color = if *done || *next { p.text } else { p.muted };
                line(c, text, text_rect, SMALL_FONT_SIZE, color);
            }
            Piece::Agent(k) => {
                let Some(PageSection::Agent {
                    icon,
                    name,
                    dot,
                    status,
                    tail,
                    button,
                }) = view.page.sections.get(*k)
                else {
                    return;
                };
                let (x, y, w, h) = rect_px(rect, scale);
                let b = s.round().max(1.0);
                c.rounded(x, y, w, h, AGENT_RADIUS * s, p.border_variant);
                c.rounded(
                    x + b,
                    y + b,
                    w - 2.0 * b,
                    h - 2.0 * b,
                    AGENT_RADIUS * s - b,
                    p.background,
                );
                let head = Rectangle::new(
                    (rect.loc.x + AGENT_PAD, rect.loc.y + AGENT_PAD).into(),
                    (rect.size.w - 2 * AGENT_PAD, AGENT_HEAD).into(),
                );
                let icon_spot = Rectangle::new(head.loc, (16, AGENT_HEAD).into());
                agent_icon(c, icon_spot, 16, *icon, p.text, scale);
                let dot_spot = Rectangle::new(
                    (head.loc.x + 22, head.loc.y + (AGENT_HEAD - DOT) / 2).into(),
                    (DOT, DOT).into(),
                );
                let d = DOT as f32 * s;
                icon_sdf(c, dot_spot, scale, dot.color(p), |x, y| {
                    dot_shape(*dot, x, y, d)
                });
                let button_rect = button.and_then(|i| l.page_buttons.get(i).copied().flatten());
                let end_x =
                    button_rect.map_or(head.loc.x + head.size.w, |r| r.loc.x - QUIT_BUTTON_GAP);
                let text = Rectangle::new(
                    (head.loc.x + 36, head.loc.y).into(),
                    ((end_x - head.loc.x - 36).max(1), AGENT_HEAD).into(),
                );
                let (tx, tend) = phys(text, scale);
                let hb = band(text, scale);
                let after = self.bold_text(c, name, FONT_SIZE, tx, tend, hb, scale, p.text);
                self.text(
                    c,
                    status,
                    SMALL_FONT_SIZE,
                    after + (8.0 * s).round(),
                    tend,
                    hb,
                    scale,
                    p.muted,
                );
                for (j, tail_line) in tail.iter().enumerate() {
                    let r = Rectangle::new(
                        (
                            head.loc.x,
                            head.loc.y + AGENT_HEAD + 4 + j as i32 * TAIL_LINE,
                        )
                            .into(),
                        (head.size.w, TAIL_LINE).into(),
                    );
                    let (lx, lend) = phys(r, scale);
                    self.mono_text(
                        c,
                        tail_line,
                        DETAIL_FONT_SIZE,
                        lx,
                        lend,
                        band(r, scale),
                        scale,
                        p.muted,
                    );
                }
                if let (Some(i), Some(r)) = (button, button_rect) {
                    self.page_button(c, view, *i, at(r), scale, p);
                }
            }
            Piece::Repo(i) => {
                let Some(repo) = view.repos.get(*i) else {
                    return;
                };
                let selected = view.focus == TabArea::Repos && view.repo == *i;
                let lit = matches!(view.hover, Some(FeatureTabHit::Repo(r) | FeatureTabHit::RepoButton(r)) if r == *i);
                self.row_highlight(c, rect, (selected, if lit { 1.0 } else { 0.0 }), scale, p);
                let button = l.repos.get(*i).and_then(|(_, b)| *b).map(at);
                let end =
                    button.map_or(rect.loc.x + rect.size.w - 4, |b| b.loc.x - QUIT_BUTTON_GAP);
                let name = Rectangle::new(
                    (rect.loc.x + DOT_ROOM + 4, rect.loc.y + 2).into(),
                    ((end - rect.loc.x - DOT_ROOM - 4).max(1), 20).into(),
                );
                let dot = Rectangle::new((rect.loc.x + 6, name.loc.y + 6).into(), (8, 8).into());
                self.repo_dot(c, dot, repo, scale, p);
                let color = if repo.touched { p.text } else { p.muted };
                line(c, &repo.name, name, FONT_SIZE, color);
                let branch = Rectangle::new(
                    (name.loc.x, name.loc.y + 20).into(),
                    (name.size.w, 18).into(),
                );
                let detail = if repo.busy {
                    format!("{} \u{b7} touching\u{2026}", repo.line)
                } else {
                    repo.line.clone()
                };
                line(c, &detail, branch, DETAIL_FONT_SIZE, p.muted);
                if let (Some(b), Some(label)) = (button, repo.button) {
                    let hovered = view.hover == Some(FeatureTabHit::RepoButton(*i));
                    self.button(c, label, b, (hovered, false), ButtonStyle::Plain, scale, p);
                }
            }
            Piece::Button(i) => self.page_button(c, view, *i, rect, scale, p),
            Piece::Activity { row, lines } => {
                let Some(a) = view.activity.get(*row) else {
                    return;
                };
                let dot = Rectangle::new((rect.loc.x + 1, rect.loc.y + 5).into(), (8, 8).into());
                let d = 8.0 * s;
                icon_sdf(c, dot, scale, a.tone.color(p), |x, y| x.hypot(y) - d / 2.0);
                let ago = Rectangle::new(rect.loc, (rect.size.w, TAIL_LINE).into());
                let (ax, aend) = phys(ago, scale);
                let w = self.text_width(&a.ago, DETAIL_FONT_SIZE, scale);
                self.text(
                    c,
                    &a.ago,
                    DETAIL_FONT_SIZE,
                    (aend - w - 1.0).max(ax).floor(),
                    aend,
                    band(ago, scale),
                    scale,
                    p.muted,
                );
                let room = (w / s).ceil() as i32 + 8;
                for (j, text) in lines.iter().enumerate() {
                    let r = Rectangle::new(
                        (rect.loc.x + DOT_ROOM, rect.loc.y + j as i32 * TAIL_LINE).into(),
                        ((rect.size.w - DOT_ROOM - room).max(1), TAIL_LINE).into(),
                    );
                    line(c, text, r, SMALL_FONT_SIZE, p.text);
                }
            }
        }
    }

    /// The page's button `i` in `rect`.
    fn page_button(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        i: usize,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let Some(button) = view.page.buttons.get(i) else {
            return;
        };
        let hovered = view.hover == Some(FeatureTabHit::PageButton(i));
        self.button(
            c,
            &page_label(button),
            rect,
            (hovered, false),
            button.style,
            scale,
            p,
        );
    }

    /// A feature's stages as a row of `marks` in `rect`, from the right:
    /// what the sidebar's feature rows end with.
    pub(super) fn stage_marks(
        &self,
        c: &mut Canvas,
        marks: &[StageMark],
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let n = marks.len() as i32;
        let pitch = STAGE_DOT + STAGE_DOT_GAP;
        let x0 = rect.loc.x + rect.size.w - n * pitch + STAGE_DOT_GAP;
        let y = rect.loc.y + (rect.size.h - STAGE_DOT) / 2;
        for (i, mark) in marks.iter().enumerate() {
            let spot = Rectangle::new(
                (x0 + i as i32 * pitch, y).into(),
                (STAGE_DOT, STAGE_DOT).into(),
            );
            icon_sdf(c, spot, scale, mark.color(p), |x, y| {
                stage_dot(*mark, x, y, 7.0 * s)
            });
        }
    }
}

/// A sidebar row for an open feature: its title and its stages' marks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FeatureRowView {
    pub title: String,
    pub marks: Vec<StageMark>,
}

impl FeatureRowView {
    /// The mark the row starts with: the current stage's, else done.
    pub(super) fn mark(&self) -> StageMark {
        if self.marks.contains(&StageMark::Current) {
            StageMark::Current
        } else {
            StageMark::Done
        }
    }
}

/// How big a stage dot of a sidebar row is, and the gap between two.
pub(super) const STAGE_DOT: i32 = 8;
pub(super) const STAGE_DOT_GAP: i32 = 3;

/// How wide `marks` stage dots are in a sidebar row.
pub(super) fn stage_marks_width(marks: usize) -> i32 {
    marks as i32 * (STAGE_DOT + STAGE_DOT_GAP) - STAGE_DOT_GAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::{ONE_DARK, ONE_LIGHT, contrast, inside};

    #[test]
    fn the_dialog_stacks_its_parts_inside_it() {
        let l = new_feature_layout((1920, 1080).into(), 3, true);
        assert_eq!(l.rows.len(), 3);
        for part in l
            .rows
            .iter()
            .chain(&l.buttons)
            .chain([&l.title, &l.field, &l.root])
        {
            assert!(inside(l.panel, *part));
        }
        assert!(
            l.error
                .is_some_and(|e| e.loc.y > l.field.loc.y && e.loc.y < l.root.loc.y)
        );
        let short = new_feature_layout((800, 360).into(), 30, false);
        assert!(!short.rows.is_empty() && short.rows.len() < 30);
        assert!(short.panel.size.h <= 360);
    }

    #[test]
    fn feature_rows_sit_under_the_workspaces_and_scroll_with_them() {
        use crate::chrome::{Density, SidebarHit, sidebar_layout_at};
        let base = sidebar_layout_at(&[], &[], 260, Density::Comfortable);
        assert!(base.features_title.is_none() && base.features.is_empty());
        let layout = sidebar_layout_at(&[], &[], 260, Density::Comfortable).with_features(
            2,
            260,
            Density::Comfortable,
        );
        let title = layout.features_title.expect("a heading");
        assert_eq!(layout.features.len(), 2);
        assert!(layout.features[0].loc.y >= title.loc.y + title.size.h);
        let mid = |r: Rect| (r.loc.x as f64 + 4.0, r.loc.y as f64 + 4.0).into();
        assert_eq!(layout.hit(mid(title)), Some(SidebarHit::FeaturesTitle));
        assert_eq!(
            layout.hit(mid(layout.features[1])),
            Some(SidebarHit::Feature(1))
        );
        // The rows count toward how far the sidebar scrolls.
        let short = 60;
        assert!(layout.max_scroll(short) > base.max_scroll(short));
        let top = layout.features[0].loc.y;
        let scrolled = layout.scrolled(20, short);
        assert_eq!(scrolled.features[0].loc.y, top - 20);
    }

    #[test]
    fn feature_workspaces_sit_under_the_features_heading() {
        use crate::WorkspaceKind;
        use crate::chrome::{Density, WorkspaceView, sidebar_layout_at};
        let ws = |name: &str, feature: bool| {
            let mut ws = WorkspaceView::new(name.into(), WorkspaceKind::Terminal, &[]);
            if feature {
                ws.stages = vec![StageMark::Done, StageMark::Current];
            }
            ws
        };
        let d = Density::Comfortable;
        let workspaces = [ws("a", false), ws("b", true), ws("c", true)];
        let layout = sidebar_layout_at(&workspaces, &[], 260, d).with_features(1, 260, d);
        let title = layout.features_title.expect("a heading");
        let blocks = &layout.blocks;
        assert!(title.loc.y >= blocks[0].loc.y + blocks[0].size.h);
        assert!(blocks[1].loc.y >= title.loc.y + title.size.h);
        // The rows of features with no workspace come after theirs.
        assert!(layout.features[0].loc.y >= blocks[2].loc.y + blocks[2].size.h);
        // A feature's workspace among others has no heading over it.
        let mixed = [ws("b", true), ws("a", false)];
        let layout = sidebar_layout_at(&mixed, &[], 260, d);
        assert!(layout.features_title.is_none());
    }

    #[test]
    fn stage_marks_read_on_the_panels_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT] {
            for mark in [StageMark::Done, StageMark::Current, StageMark::Later] {
                for ground in [p.panel, p.elevated, p.selected] {
                    let ratio = contrast(mark.color(p), ground);
                    assert!(ratio >= 3.0, "{mark:?} at {ratio:.2}:1");
                }
            }
        }
    }

    #[test]
    fn the_ship_check_reads_on_the_panels_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT] {
            for tone in [CheckTone::Blocker, CheckTone::Ready, CheckTone::Note] {
                for ground in [p.panel, p.elevated] {
                    let ratio = contrast(tone.color(p), ground);
                    assert!(ratio >= 4.5, "{tone:?} at {ratio:.2}:1");
                }
            }
        }
    }

    // ---------------------------------------------------------- feature tab

    /// The golden fixture: Build running, three repositories, the second
    /// of two features.
    fn tab_view() -> FeatureTabView {
        crate::chrome::golden::feature_tab()
    }

    fn tab_at(view: &FeatureTabView, w: i32) -> FeatureTabLayout {
        feature_tab_layout(view, Rectangle::from_size((w, 900).into()))
    }

    fn centre(r: Rect) -> Point<i32, Logical> {
        (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2).into()
    }

    #[test]
    fn the_tab_drops_columns_as_it_narrows() {
        let view = tab_view();
        let wide = tab_at(&view, 1920);
        assert!(
            TabColumn::ALL
                .iter()
                .all(|c| wide.columns[*c as usize].is_some())
        );
        assert!(wide.strip.is_none() && !wide.compact);
        let at = tab_at(&view, ACTIVITY_BELOW);
        assert!(at.columns[TabColumn::Activity as usize].is_some());
        let under = tab_at(&view, ACTIVITY_BELOW - 1);
        assert!(under.columns[TabColumn::Activity as usize].is_none());
        assert!(under.columns[TabColumn::Repos as usize].is_some());
        assert!(!under.compact);
        let compact = tab_at(&view, COMPACT_BUTTONS_BELOW - 1);
        assert!(compact.compact && compact.strip.is_none());
        let narrow = tab_at(&view, REPOS_BELOW - 1);
        assert!(narrow.columns[TabColumn::Repos as usize].is_none());
        assert!(narrow.columns[TabColumn::Activity as usize].is_none());
        assert!(narrow.strip.is_some());
        // The page keeps its room (the plan's 420 px at least) where the
        // columns beside it are.
        const PAGE_MIN_W: i32 = 420;
        for w in [REPOS_BELOW, ACTIVITY_BELOW] {
            let page = tab_at(&view, w).columns[TabColumn::Page as usize].unwrap();
            assert!(page.size.w >= PAGE_MIN_W, "{w}: {}", page.size.w);
        }
    }

    #[test]
    fn the_tab_parts_stay_inside_and_apart() {
        let view = tab_view();
        for w in [700, 860, 999, 1000, 1179, 1180, 1920] {
            let l = tab_at(&view, w);
            let inside = |r: Rect| r.loc.x >= 0 && r.loc.x + r.size.w <= w && r.loc.y >= 0;
            assert!(l.buttons.iter().all(|b| inside(*b)), "{w}");
            assert!(l.nodes.iter().all(|n| inside(*n)), "{w}");
            let (_, prev, next) = l.switch.unwrap();
            assert!(prev.loc.x + prev.size.w <= next.loc.x, "{w}");
            assert!(next.loc.x + next.size.w <= l.buttons[0].loc.x, "{w}");
            assert!(l.title.loc.x + l.title.size.w <= prev.loc.x, "{w}");
            for pair in l.nodes.windows(2) {
                assert!(pair[0].loc.x + pair[0].size.w <= pair[1].loc.x, "{w}");
            }
            let body = l.nodes[0].loc.y + l.nodes[0].size.h;
            for column in l.columns.iter().flatten() {
                assert!(column.loc.y > body && column.loc.y + column.size.h <= l.hints.loc.y);
            }
        }
    }

    #[test]
    fn the_tab_hits_what_is_under_the_pointer() {
        let mut view = tab_view();
        let l = tab_at(&view, 1920);
        let hit = |l: &FeatureTabLayout, v: &FeatureTabView, r: Rect| l.hit(v, centre(r));
        let (_, prev, next) = l.switch.unwrap();
        assert_eq!(hit(&l, &view, prev), Some(FeatureTabHit::Prev));
        assert_eq!(hit(&l, &view, next), Some(FeatureTabHit::Next));
        for (i, b) in l.buttons.iter().enumerate() {
            assert_eq!(hit(&l, &view, *b), Some(FeatureTabHit::Button(i)));
        }
        assert_eq!(l.buttons.len(), FEATURE_BUTTONS.len() + 1);
        for (i, n) in l.nodes.iter().enumerate() {
            assert_eq!(hit(&l, &view, *n), Some(FeatureTabHit::Node(i)));
        }
        let (row, button) = l.repos[2];
        assert_eq!(
            l.hit(&view, (row.loc.x + 4, row.loc.y + 4).into()),
            Some(FeatureTabHit::Repo(2))
        );
        assert_eq!(
            hit(&l, &view, button.unwrap()),
            Some(FeatureTabHit::RepoButton(2))
        );
        assert!(l.repos[0].1.is_none());
        let first = l.page_buttons[0].unwrap();
        assert_eq!(hit(&l, &view, first), Some(FeatureTabHit::PageButton(0)));
        let (i, spot) = l.hint_spots[0];
        assert_eq!(hit(&l, &view, spot), Some(FeatureTabHit::Hint(i)));
        let page = l.columns[TabColumn::Page as usize].unwrap();
        let empty = (page.loc.x + page.size.w / 2, page.loc.y + page.size.h - 10).into();
        assert_eq!(
            l.hit(&view, empty),
            Some(FeatureTabHit::Column(TabColumn::Page))
        );
        assert_eq!(l.column_at(empty), Some(TabColumn::Page));
        assert_eq!(
            l.hit(&view, (4, l.hints.loc.y - 2).into()),
            Some(FeatureTabHit::Tab)
        );
        assert_eq!(l.hit(&view, (-4, 4).into()), None);
        // Scrolled, the page's button sits higher by as much.
        view.scroll[TabColumn::Page as usize] = 20.0;
        let lifted = (centre(first).x, centre(first).y - 20).into();
        assert_eq!(l.hit(&view, lifted), Some(FeatureTabHit::PageButton(0)));
        // The Show agent button in the agent card counts as the page's too.
        view.scroll = [0.0; 3];
        let agent = l.page_buttons[2].unwrap();
        assert_eq!(hit(&l, &view, agent), Some(FeatureTabHit::PageButton(2)));
    }

    #[test]
    fn the_narrow_tab_hits_its_repository_chips() {
        let mut view = tab_view();
        view.repo = 2;
        let l = tab_at(&view, 860);
        let (chip, _) = l.repos[1];
        assert_eq!(l.hit(&view, centre(chip)), Some(FeatureTabHit::Repo(1)));
        // The selected repository's button, at the strip's end.
        let button = l.repos[2].1.unwrap();
        assert_eq!(
            l.hit(&view, centre(button)),
            Some(FeatureTabHit::RepoButton(2))
        );
        let strip = l.strip.unwrap();
        assert!(button.loc.x + button.size.w <= strip.loc.x + strip.size.w);
    }

    #[test]
    fn the_selected_repository_scrolls_into_view() {
        let mut view = tab_view();
        let row = view.repos[1].clone();
        view.repos = (0..30)
            .map(|i| FeatureRepoRow {
                name: format!("repo-{i}"),
                ..row.clone()
            })
            .collect();
        let l = tab_at(&view, 1920);
        let place = l.columns[TabColumn::Repos as usize].unwrap();
        let room = f64::from(l.scroll_room(TabColumn::Repos));
        assert!(room > 0.0);
        let shows = |i: usize, down: f64| {
            let r = l.repos[i].0;
            let top = f64::from(r.loc.y) - down;
            top >= f64::from(place.loc.y)
                && top + f64::from(r.size.h) <= f64::from(place.loc.y + place.size.h)
        };
        // One showing stays where it is; the first goes to the top.
        assert_eq!(l.reveal_repo(1, 0.0), Some(0.0));
        assert_eq!(l.reveal_repo(0, 40.0), Some(0.0));
        // One below comes up to the bottom, one above down to the top.
        let down = l.reveal_repo(20, 0.0).unwrap();
        assert!(down > 0.0 && shows(20, down) && !shows(20, down - 20.0));
        let up = l.reveal_repo(5, room).unwrap();
        assert!(up < room && shows(5, up) && !shows(5, up + 20.0));
        // The last lands at the end, no further.
        assert_eq!(l.reveal_repo(29, 0.0), Some(room));
        // A narrow tab's chips have no column to scroll.
        assert_eq!(tab_at(&view, REPOS_BELOW - 1).reveal_repo(20, 0.0), None);
    }

    #[test]
    fn long_columns_scroll_by_what_does_not_show() {
        let mut view = tab_view();
        assert_eq!(tab_at(&view, 1920).scroll_room(TabColumn::Page), 0);
        let row = view.activity[0].clone();
        view.activity = (0..200).map(|_| row.clone()).collect();
        let l = tab_at(&view, 1920);
        let shown = l.columns[TabColumn::Activity as usize].unwrap().size.h;
        let room = l.scroll_room(TabColumn::Activity);
        assert!(room > 0);
        assert_eq!(room, l.content[TabColumn::Activity as usize] - shown);
        assert!(l.thumb(TabColumn::Activity, 0.0).is_some());
        assert_eq!(l.scroll_room(TabColumn::Page), 0);
        // Under ACTIVITY_BELOW the rows go to the page, and it scrolls.
        let narrow = tab_at(&view, 1000);
        assert_eq!(narrow.scroll_room(TabColumn::Activity), 0);
        assert!(narrow.scroll_room(TabColumn::Page) > room);
    }
}
