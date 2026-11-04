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
    dialog_buttons, field_text, focus_ring, footer_hint_spots, gutter_track, hint_strs, icon_sdf,
    measure, measure_bold, measured_footer_height, mix, phys, physical, plus, rect_px,
    segment_tint, status_dot, tab, trash,
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
    /// Lines added and removed since its base, uncommitted changes
    /// included, once read (`git diff --numstat`).
    pub stat: Option<(usize, usize)>,
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

/// An agent to pick on a stage's page: the one its next start runs is lit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentChip {
    pub icon: AgentIcon,
    /// Only its tooltip says it.
    pub name: String,
    pub lit: bool,
    /// It can't be picked: it built the code this stage reviews.
    pub off: bool,
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
        /// The agents to pick for the stage's next start, if it offers any.
        picks: Vec<AgentChip>,
    },
    /// A stage's file (`brief.md`, `plan.md`): its name, how it stands,
    /// and the indices in `StagePage::buttons` of the buttons it holds
    /// (Review, Preview), drawn at the card's right.
    File {
        name: String,
        state: CheckLine,
        buttons: Vec<usize>,
    },
    /// A labelled list of lines in their tones: the checks, findings,
    /// criteria, the Ship check, a done stage's note.
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

impl FeatureTabView {
    /// The `i`th chip on the page's agent card.
    pub fn agent_chip(&self, i: usize) -> Option<&AgentChip> {
        self.page.sections.iter().find_map(|s| match s {
            PageSection::Agent { picks, .. } => picks.get(i),
            _ => None,
        })
    }
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
    /// An agent's chip on the page's agent card, as its `picks` has them.
    AgentPick(usize),
    /// One of those that can't be picked: it only says why.
    AgentPickOff(usize),
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
    /// On the strip, the chips standing for the repositories that don't
    /// fit, before the shown ones and after them: each chip, the
    /// repository a click selects (the nearest hidden one), and its label.
    pub more_repos: Vec<(Rect, usize, String)>,
    /// Each page button's place, indexed as `StagePage::buttons`.
    pub page_buttons: Vec<Option<Rect>>,
    /// The agent card's chips: their outline, and each chip as its `picks`
    /// has them.
    pub agent_picks: Option<(Rect, Vec<Rect>)>,
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
            if let Some((_, i, _)) = self.more_repos.iter().find(|(r, ..)| on(r)) {
                return Some(FeatureTabHit::MoreRepos(*i));
            }
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
                    .map(FeatureTabHit::PageButton)
                    .or_else(|| self.agent_pick_at(view, at)),
                TabColumn::Activity => None,
            };
            return found.or(Some(FeatureTabHit::Column(column)));
        }
        Some(FeatureTabHit::Tab)
    }

    /// The agent's chip at `at` (unscrolled).
    fn agent_pick_at(
        &self,
        view: &FeatureTabView,
        at: Point<i32, Logical>,
    ) -> Option<FeatureTabHit> {
        let (_, chips) = self.agent_picks.as_ref()?;
        let i = chips.iter().position(|c| c.contains(at))?;
        Some(match view.agent_chip(i).is_some_and(|c| c.off) {
            true => FeatureTabHit::AgentPickOff(i),
            false => FeatureTabHit::AgentPick(i),
        })
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
/// How far a label's second line sits under its first, when it is too
/// wide for its slot and takes the short line's place.
const RAIL_LABEL_STEP: i32 = 16;
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
/// An agent card's chips: the outline's height, as its button's, the room
/// between it and its chips, a chip's width, a logo's size, and the label
/// in front of them on a card with no head.
const PICK_TRACK: i32 = ROW_BUTTON;
const PICK_TRACK_PAD: i32 = 2;
const PICK_CHIP: i32 = 28;
const PICK_ICON: i32 = 14;
const PICK_LABEL: i32 = 48;
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
    /// The file card of `StagePage::sections[i]`.
    File(usize),
    /// `FeatureTabView::repos[i]`.
    Repo(usize),
    /// `StagePage::buttons[i]`, not on an agent or file card.
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
    wrap_to(text, size, width, MAX_WRAP)
}

/// `text`'s words, a group in brackets ("(run / 21)") kept as one, so a
/// line doesn't break inside it.
fn words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut depth = 0i32;
    for word in text.split_whitespace() {
        match words.last_mut() {
            Some(last) if depth > 0 => {
                last.push(' ');
                last.push_str(word);
            }
            _ => words.push(word.to_owned()),
        }
        depth += word.matches(['(', '[']).count() as i32;
        depth -= word.matches([')', ']']).count() as i32;
        depth = depth.max(0);
    }
    words
}

/// `wrap` to at most `max` lines.
fn wrap_to(text: &str, size: f32, width: i32, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in words(text) {
        let longer = if line.is_empty() {
            word.clone()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || lines.len() + 1 >= max || width_of(&longer, size) <= width as f32 {
            line = longer;
        } else {
            lines.push(std::mem::replace(&mut line, word));
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// The room left between a repository row's name and its line stat,
/// between the stat's two counts, and the least of the name the stat
/// leaves showing.
const STAT_GAP: i32 = 8;
const STAT_SPACE: i32 = 5;
const STAT_NAME_MIN: i32 = 40;

/// An activity row's dot in `spot`, by its `tone`: a tick in the success
/// colour for what went through, a filled dot in the warning colour for
/// what was sent back or failed, and a ring in the muted colour for the
/// rest. Told apart by their shapes too, not by colour alone.
fn activity_dot(c: &mut Canvas, spot: Rect, tone: CheckTone, scale: f64, p: &Palette) {
    let s = scale as f32;
    let d = spot.size.w as f32 * s;
    let color = tone.color(p);
    match tone {
        CheckTone::Ready => icon_sdf(c, spot, scale, color, |x, y| check(x, y, s * 0.95)),
        CheckTone::Blocker => icon_sdf(c, spot, scale, color, |x, y| x.hypot(y) - d / 2.0),
        CheckTone::Note => icon_sdf(c, spot, scale, color, |x, y| {
            stage_dot(StageMark::Later, x, y, d)
        }),
    }
}

/// The most lines an activity row takes; the last ends in "…".
const ACTIVITY_LINES: usize = 2;

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
            let lines = wrap_to(
                &a.what,
                Font::Small.size(),
                self.w - DOT_ROOM - ago,
                ACTIVITY_LINES,
            );
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
/// How wide `n` agent chips' outline is, with the room.
fn agent_picks_width(n: usize) -> i32 {
    n as i32 * PICK_CHIP + 2 * PICK_TRACK_PAD
}

/// Where `n` agent chips go on a row at `(x, y)`, `room` wide: their
/// outline and each chip, narrower alike when they don't fit.
fn agent_pick_rects(n: usize, x: i32, y: i32, room: i32) -> (Rect, Vec<Rect>) {
    let inner = (room - 2 * PICK_TRACK_PAD).max(1);
    let w = PICK_CHIP.min(inner / n.max(1) as i32);
    let chips: Vec<Rect> = (0..n as i32)
        .map(|i| {
            Rectangle::new(
                (x + PICK_TRACK_PAD + i * w, y + PICK_TRACK_PAD).into(),
                (w, PICK_TRACK - 2 * PICK_TRACK_PAD).into(),
            )
        })
        .collect();
    let track = Rectangle::new(
        (x, y).into(),
        (n as i32 * w + 2 * PICK_TRACK_PAD, PICK_TRACK).into(),
    );
    (track, chips)
}

fn page_stack(
    view: &FeatureTabView,
    s: &mut Stack,
    buttons: &mut [Option<Rect>],
    agent_picks: &mut Option<(Rect, Vec<Rect>)>,
    activity: bool,
) {
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
    // The buttons no card holds, wrapped in rows.
    let held: Vec<usize> = page
        .sections
        .iter()
        .flat_map(|sec| match sec {
            PageSection::Agent { button, .. } => button.iter().copied().collect(),
            PageSection::File { buttons, .. } => buttons.clone(),
            _ => Vec::new(),
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
            PageSection::Agent {
                name,
                tail,
                button,
                picks,
                ..
            } => {
                let tail_h = match tail.len() {
                    0 => 0,
                    n => n as i32 * TAIL_LINE + 4,
                };
                let card = s.put(AGENT_PAD * 2 + AGENT_HEAD + tail_h, Piece::Agent(k));
                let row = card.loc.y + AGENT_PAD + (AGENT_HEAD - ROW_BUTTON) / 2;
                let mut right = card.loc.x + card.size.w - AGENT_PAD;
                if let Some(b) = button.filter(|b| *b < page.buttons.len()) {
                    let w = button_width(&page_label(&page.buttons[b]));
                    buttons[b] = Some(Rectangle::new(
                        (right - w, row).into(),
                        (w, ROW_BUTTON).into(),
                    ));
                    right -= w + QUIT_BUTTON_GAP;
                }
                if !picks.is_empty() {
                    // No run yet: after their label. A run: at the head's
                    // end, its name and status taking what is left.
                    let left = card.loc.x + AGENT_PAD + PICK_LABEL;
                    let (x, room) = match name.is_empty() {
                        true => (left, right - left),
                        false => {
                            let room = agent_picks_width(picks.len()).min(right - left);
                            (right - room, room)
                        }
                    };
                    *agent_picks = Some(agent_pick_rects(picks.len(), x, row, room));
                }
            }
            PageSection::File { buttons: held, .. } => {
                let card = s.put(AGENT_PAD * 2 + AGENT_HEAD, Piece::File(k));
                // Its buttons from the card's right, in their order.
                let mut bx = card.loc.x + card.size.w - AGENT_PAD;
                let by = card.loc.y + AGENT_PAD + (AGENT_HEAD - ROW_BUTTON) / 2;
                for &b in held.iter().rev().filter(|b| **b < page.buttons.len()) {
                    let w = button_width(&page_label(&page.buttons[b]));
                    bx -= w;
                    buttons[b] = Some(Rectangle::new((bx, by).into(), (w, ROW_BUTTON).into()));
                    bx -= QUIT_BUTTON_GAP;
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
    let slot = |i: i32| (x + inner * i / n, x + inner * (i + 1) / n);
    // A label on two lines puts its node's short line under them: the
    // rail is a line taller for it.
    let wraps = (0..view.rail.len() as i32)
        .zip(&view.rail)
        .any(|(i, node)| {
            let (left, next) = slot(i);
            rail_label(&node.label, next - left - 4).len() > 1
        });
    let rail_h = NODE + NODE_GAP + NODE_LABEL + NODE_LINE + if wraps { RAIL_LABEL_STEP } else { 0 };
    let nodes = (0..view.rail.len() as i32)
        .map(|i| {
            let (left, next) = slot(i);
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
    let mut more_repos = Vec::new();
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
        let n = view.repos.len();
        let more_w = |label: &str| width_of(label, SMALL_FONT_SIZE).ceil() as i32 + 2 * CHIP_PAD;
        // How many chips fit from `from` after a chip `lead` wide, with
        // room kept for the "+N" chip when some are left over.
        let fits = |from: usize, lead: i32| {
            let mut cx = x + lead;
            let mut k = 0;
            for (j, r) in view.repos[from..].iter().enumerate() {
                let left = n - from - j - 1;
                let tail = match left {
                    0 => 0,
                    left => CHIP_GAP + more_w(&more_label(left, true)),
                };
                if cx + chip_w(r) + tail > end {
                    break;
                }
                cx += chip_w(r) + CHIP_GAP;
                k += 1;
            }
            k
        };
        let first = if view.repo < n && view.repo >= fits(0, 0) {
            view.repo
        } else {
            0
        };
        let lead = match first {
            0 => 0,
            first => more_w(&more_label(first, false)) + CHIP_GAP,
        };
        let shown = first..first + fits(first, lead);
        let chip_y = strip.loc.y + (CHIP_ROW - ROW_BUTTON) / 2;
        if first > 0 {
            let w = more_w(&more_label(first, false));
            let chip = Rectangle::new((x, chip_y).into(), (w, ROW_BUTTON).into());
            more_repos.push((chip, first - 1, more_label(first, false)));
        }
        let mut cx = x + lead;
        for (i, repo) in view.repos.iter().enumerate() {
            if !shown.contains(&i) {
                repos.push((Rect::default(), None));
                continue;
            }
            let w = chip_w(repo);
            let chip = Rectangle::new((cx, chip_y).into(), (w, ROW_BUTTON).into());
            cx += w + CHIP_GAP;
            repos.push((chip, None));
        }
        if shown.end < n {
            let label = more_label(n - shown.end, true);
            let w = more_w(&label);
            let chip = Rectangle::new((cx, chip_y).into(), (w, ROW_BUTTON).into());
            more_repos.push((chip, shown.end, label));
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
    let mut agent_picks = None;
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
                &mut agent_picks,
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
        more_repos,
        page_buttons,
        agent_picks,
        hints,
        hint_spots,
        pieces,
    }
}

/// What the strip's chip standing for `n` hidden repositories says: "+N"
/// for those after the chips, "N+" for those before.
fn more_label(n: usize, after: bool) -> String {
    match after {
        true => format!("+{n}"),
        false => format!("{n}+"),
    }
}

/// A rail node's label as it goes in a slot `w` wide: on one line, or on
/// two when it is wider than that in bold (as it is selected, so picking
/// it doesn't move it). A word too long for its line is cut when it is
/// drawn. The layout and the painter both go by it.
pub(super) fn rail_label(text: &str, w: i32) -> Vec<String> {
    let fits = |t: &str| {
        let width =
            measure_bold(t, SMALL_FONT_SIZE).unwrap_or_else(|| width_of(t, SMALL_FONT_SIZE));
        width <= w as f32
    };
    if fits(text) {
        return vec![text.to_owned()];
    }
    // Where to break it: the most even split that fits, else after the
    // first word.
    let words: Vec<&str> = text.split_whitespace().collect();
    let split = |k: usize| (words[..k].join(" "), words[k..].join(" "));
    let best = (1..words.len())
        .map(split)
        .filter(|(a, b)| fits(a) && fits(b))
        .min_by_key(|(a, b)| a.chars().count().abs_diff(b.chars().count()));
    match best.or_else(|| (words.len() > 1).then(|| split(1))) {
        Some((a, b)) => vec![a, b],
        None => vec![text.to_owned()],
    }
}

/// A rail node's circle in its `slot`: at the slot's top middle.
fn node_circle(slot: Rect) -> Rect {
    Rectangle::new(
        (slot.loc.x + (slot.size.w - NODE) / 2, slot.loc.y).into(),
        (NODE, NODE).into(),
    )
}

/// A card's frame on `c` at `rect`: a hairline border round the ground,
/// as the agent and file cards have it. Gives the card's head row, inside
/// its padding.
fn card_frame(c: &mut Canvas, rect: Rect, scale: f64, p: &Palette) -> Rect {
    let s = scale as f32;
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
    Rectangle::new(
        (rect.loc.x + AGENT_PAD, rect.loc.y + AGENT_PAD).into(),
        (rect.size.w - 2 * AGENT_PAD, AGENT_HEAD).into(),
    )
}

/// Where the status dot of the agent card at `card` goes.
fn agent_card_dot(card: Rect) -> Rect {
    Rectangle::new(
        (
            card.loc.x + AGENT_PAD + 22,
            card.loc.y + AGENT_PAD + (AGENT_HEAD - DOT) / 2,
        )
            .into(),
        (DOT, DOT).into(),
    )
}

/// How wide a repository's dot is, on its row or chip.
const REPO_DOT: i32 = 8;

/// Where the dot of a repository's row at `row` goes, left of its name.
fn repo_row_dot(row: Rect) -> Rect {
    Rectangle::new(
        (row.loc.x + 6, row.loc.y + 8).into(),
        (REPO_DOT, REPO_DOT).into(),
    )
}

/// Where the dot of a repository's chip at `chip` goes.
fn repo_chip_dot(chip: Rect) -> Rect {
    Rectangle::new(
        (
            chip.loc.x + CHIP_PAD,
            chip.loc.y + (chip.size.h - REPO_DOT) / 2,
        )
            .into(),
        (REPO_DOT, REPO_DOT).into(),
    )
}

/// The blinking dot (`DOT` across) over a repository's still dot's
/// `spot`.
fn repo_blinker(spot: Rect) -> Rect {
    Rectangle::new(spot.loc, (DOT, DOT).into())
}

/// Where the blinking dots of the feature tab showing `view` go, laid
/// out as `l`: the dot of an agent card whose agent is at work, and of a
/// repository being touched. Each as laid out, unscrolled, with the
/// column it is in (and scrolls with); `None` for one in the head (a
/// narrow tab's strip of chips).
pub fn feature_tab_blinkers(
    view: &FeatureTabView,
    l: &FeatureTabLayout,
) -> Vec<(Rect, Option<TabColumn>)> {
    let mut dots = Vec::new();
    for (rect, piece) in &l.pieces[TabColumn::Page as usize] {
        if let Piece::Agent(k) = piece
            && let Some(PageSection::Agent { dot, .. }) = view.page.sections.get(*k)
            && dot.blinks()
        {
            dots.push((agent_card_dot(*rect), Some(TabColumn::Page)));
        }
    }
    if view.repos_error.is_none() {
        let busy = |i: usize| view.repos.get(i).is_some_and(|r| r.busy);
        match l.strip {
            Some(_) => dots.extend(
                (l.repos.iter().enumerate())
                    .filter(|(i, (chip, _))| busy(*i) && chip.size.w > 0)
                    .map(|(_, (chip, _))| (repo_blinker(repo_chip_dot(*chip)), None)),
            ),
            None => {
                dots.extend((l.repos.iter().enumerate()).filter(|(i, _)| busy(*i)).map(
                    |(_, (row, _))| (repo_blinker(repo_row_dot(*row)), Some(TabColumn::Repos)),
                ))
            }
        }
    }
    dots
}

/// How far the glow round the current stage's node reaches past it.
const PULSE_HALO: i32 = 7;
/// How opaque the glow is at the top of its pulse.
const PULSE_PEAK: f32 = 0.7;

/// Where the glow pulsing round the current stage's node goes on the tab
/// showing `view`, laid out as `l`, if a stage is current.
pub fn feature_tab_pulse(view: &FeatureTabView, l: &FeatureTabLayout) -> Option<Rect> {
    let i = view.rail.iter().position(|n| n.mark == NodeMark::Current)?;
    let spot = node_circle(*l.nodes.get(i)?);
    Some(Rectangle::new(
        (spot.loc.x - PULSE_HALO, spot.loc.y - PULSE_HALO).into(),
        (NODE + 2 * PULSE_HALO, NODE + 2 * PULSE_HALO).into(),
    ))
}

/// How opaque the current node's glow is drawn when the pulse stands at
/// `pulse` (0 to 1, see `anim::pulse`).
pub fn rail_pulse_alpha(pulse: f32) -> f32 {
    PULSE_PEAK * pulse.clamp(0.0, 1.0)
}

/// A part of the feature tab drawn in a buffer of its own, over the
/// ground and cards of `Painter::feature_tab_back`, so a change to one
/// (a hover, a key move, a column scrolled) draws only that one again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabSlice {
    /// The header, the rail and its rule, and a narrow tab's strip of
    /// repository chips: from the tab's top to the columns.
    Head,
    /// A column's contents, unscrolled: scrolling it moves its buffer.
    Column(TabColumn),
    Hints,
}

/// Physical pixels a column's buffer may be high: one taller holds only
/// what shows of it, and is drawn again as it scrolls.
const TALL_COLUMN: f64 = 8192.0;

impl FeatureTabLayout {
    /// Where `slice` goes on the tab, scrolled `down` (its column's
    /// scroll), and whether it moves with it: a column's contents in
    /// full when they're not too tall at `scale`, from the column's top
    /// (so it shows `down` higher), else only what shows. `None` for a
    /// column the tab hasn't, or one with nothing in it.
    pub(super) fn slice_rect(
        &self,
        slice: TabSlice,
        down: f64,
        scale: f64,
    ) -> Option<(Rect, bool)> {
        match slice {
            TabSlice::Head => {
                let bottom = self.columns[TabColumn::Page as usize]?.loc.y;
                Some((
                    Rectangle::new(
                        self.frame.loc,
                        (self.frame.size.w, bottom - self.frame.loc.y).into(),
                    ),
                    false,
                ))
            }
            TabSlice::Hints => Some((self.hints, false)),
            TabSlice::Column(column) => {
                let place = self.columns[column as usize]?;
                let x = place.loc.x + CARD_PAD;
                let w = (place.size.w - 2 * CARD_PAD).max(1);
                let h = self.content[column as usize];
                if h <= 0 || place.size.h <= 0 {
                    return None;
                }
                if f64::from(h) * scale <= TALL_COLUMN {
                    return Some((Rectangle::new((x, place.loc.y).into(), (w, h).into()), true));
                }
                let dy = down.round() as i32;
                Some((
                    Rectangle::new((x, place.loc.y + dy).into(), (w, place.size.h).into()),
                    false,
                ))
            }
        }
    }
}

/// `view` with only what `slice` draws from it kept (and the rest
/// emptied): what its buffer is cached by, with where it goes. `strip`:
/// the tab has a strip of repository chips in its head.
fn slice_view(view: &FeatureTabView, slice: TabSlice, strip: bool) -> FeatureTabView {
    let mut v = FeatureTabView {
        title: String::new(),
        sub: String::new(),
        which: None,
        remove: "",
        rail: Vec::new(),
        repos: Vec::new(),
        repos_error: None,
        repo: 0,
        focus: TabArea::Stages,
        page: StagePage {
            title: String::new(),
            about: String::new(),
            status: None,
            buttons: Vec::new(),
            sections: Vec::new(),
        },
        activity: Vec::new(),
        scroll: [0.0; 3],
        hover: None,
        hints: Vec::new(),
    };
    use FeatureTabHit as H;
    let repos = |v: &mut FeatureTabView| {
        v.repos = view.repos.clone();
        v.repos_error = view.repos_error.clone();
        v.repo = view.repo;
        v.focus = view.focus;
    };
    v.hover = view.hover.filter(|hit| match slice {
        TabSlice::Head => {
            matches!(hit, H::Prev | H::Next | H::Button(_) | H::Node(_))
                || strip && matches!(hit, H::Repo(_) | H::RepoButton(_) | H::MoreRepos(_))
        }
        TabSlice::Column(TabColumn::Repos) => matches!(hit, H::Repo(_) | H::RepoButton(_)),
        TabSlice::Column(TabColumn::Page) => matches!(hit, H::PageButton(_) | H::AgentPick(_)),
        TabSlice::Column(TabColumn::Activity) => false,
        TabSlice::Hints => matches!(hit, H::Hint(_)),
    });
    match slice {
        TabSlice::Head => {
            v.title = view.title.clone();
            v.sub = view.sub.clone();
            v.which = view.which;
            v.remove = view.remove;
            v.rail = view.rail.clone();
            v.focus = view.focus;
            if strip {
                repos(&mut v);
            }
        }
        TabSlice::Column(TabColumn::Repos) => repos(&mut v),
        TabSlice::Column(TabColumn::Page) => {
            v.page = view.page.clone();
            v.activity = view.activity.clone();
        }
        TabSlice::Column(TabColumn::Activity) => v.activity = view.activity.clone(),
        TabSlice::Hints => v.hints = view.hints.clone(),
    }
    v
}

/// What a feature tab's ground is drawn from: the tab's size, its
/// columns' places, the theme and the scale.
type BackKey = (Size<i32, Logical>, [Option<Rect>; 3], super::Theme, f64);
/// What a slice is drawn from: what it shows of the view, where it goes
/// (in the tab), the tab's size, the theme and the scale.
type SliceKey = (FeatureTabView, Rect, Size<i32, Logical>, super::Theme, f64);

/// What a column's thumb is drawn from: its size, the theme and the
/// scale.
type ThumbKey = (Size<i32, Logical>, super::Theme, f64);

/// The feature tab as the chrome shows it, fading in and out (see
/// `Overlay`): its ground and cards, then a buffer per slice (see
/// `TabSlice`) and a thumb per column that scrolls, each drawn again
/// only when what it shows changes.
#[derive(Default)]
pub(super) struct FeatureTabLayers {
    overlay: super::Overlay<BackKey>,
    head: super::Cached<SliceKey>,
    columns: [super::Cached<SliceKey>; 3],
    hints: super::Cached<SliceKey>,
    /// The thumbs, by their size.
    thumbs: [super::Cached<ThumbKey>; 3],
    /// The glow round the current stage's node, composed over the head
    /// at the pulse's opacity, so the head isn't drawn again for it.
    pulse: super::Cached<(super::Theme, f64)>,
    /// How many buffers were drawn, for tests.
    drawn: std::cell::Cell<usize>,
}

impl FeatureTabLayers {
    /// The parts showing the feature tab in `area` at `now`, front to
    /// back: the current stage's node glowing at `pulse` (see
    /// `anim::pulse`), and `blink`'s dot over what is at work.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn parts(
        &mut self,
        view: Option<FeatureTabView>,
        area: Rect,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
        pulse: f32,
        blink: super::layers::Blink,
    ) -> Vec<super::layers::Part> {
        use super::layers::Part;
        let p = theme.palette();
        let output = area.size;
        let layout = view
            .as_ref()
            .map(|v| feature_tab_layout(v, Rectangle::from_size(output)));
        let placed = layout
            .as_ref()
            .map(|l| ((output, l.columns, theme, scale), area.loc, output));
        let drawn = &self.drawn;
        let (head, columns, hints, thumbs, glow) = (
            &mut self.head,
            &mut self.columns,
            &mut self.hints,
            &mut self.thumbs,
            &mut self.pulse,
        );
        let more = |_: &BackKey| {
            let (Some(view), Some(l)) = (&view, &layout) else {
                return Vec::new();
            };
            let strip = l.strip.is_some();
            let at = |r: Rect| Point::from((area.loc.x + r.loc.x, area.loc.y + r.loc.y));
            let slice = |cache: &mut super::Cached<SliceKey>, slice: TabSlice, down: f64| {
                let (region, moves) = l.slice_rect(slice, down, scale)?;
                let mut key = slice_view(view, slice, strip);
                if let (TabSlice::Column(c), false) = (slice, moves) {
                    key.scroll[c as usize] = down;
                }
                let buffer = cache.get((key, region, output, theme, scale), |_| {
                    drawn.set(drawn.get() + 1);
                    painter.feature_tab_slice(view, output, slice, region, scale, p)
                });
                let part = Part::new(buffer, at(region), region.size, 1.0);
                Some((part, moves))
            };
            // Inside a column's card's edge: what of it shows.
            let inside = |place: Rect| {
                Rectangle::new(
                    at(place) + Point::from((1, 1)),
                    (place.size.w - 2, place.size.h - 2).into(),
                )
            };
            // In front: the dots of what is at work, moving with their
            // column, and the current node's glow, over the slices so
            // these aren't drawn again as they blink and pulse.
            let mut parts = Vec::new();
            if let Some((dot, alpha)) = blink.filter(|(_, a)| *a > 0.0) {
                for (rect, column) in feature_tab_blinkers(view, l) {
                    let part = Part::new(dot, at(rect), rect.size, alpha);
                    let part = match column.and_then(|c| Some((c, l.columns[c as usize]?))) {
                        Some((c, place)) => {
                            let loc = (part.loc.x, part.loc.y - view.scroll[c as usize]).into();
                            part.at(loc).clipped(inside(place))
                        }
                        None => part,
                    };
                    parts.push(part);
                }
            }
            let alpha = rail_pulse_alpha(pulse);
            if let Some(halo) = feature_tab_pulse(view, l).filter(|_| alpha > 0.0) {
                let buffer = glow.get((theme, scale), |_| painter.rail_pulse(scale, p));
                parts.push(Part::new(buffer, at(halo), halo.size, alpha));
            }
            for column in TabColumn::ALL {
                let i = column as usize;
                let Some(place) = l.columns[i] else {
                    continue;
                };
                let down = view.scroll[i];
                if let Some(thumb) = l.thumb(column, down) {
                    let size = Size::from((thumb.track.size.w, thumb.h.round() as i32));
                    let buffer = thumbs[i].get((size, theme, scale), |(size, ..)| {
                        painter.bar_thumb(*size, scale, p)
                    });
                    let loc = at(thumb.track).to_f64();
                    parts.push(
                        Part::new(buffer, at(thumb.track), size, 1.0).at((
                            loc.x,
                            f64::from(area.loc.y) + thumb.top,
                        )
                            .into()),
                    );
                }
                let inside = inside(place);
                if let Some((part, moves)) = slice(&mut columns[i], TabSlice::Column(column), down)
                {
                    let part = match moves {
                        true => {
                            let loc = (part.loc.x, part.loc.y - down).into();
                            part.at(loc)
                        }
                        false => part,
                    };
                    parts.push(part.clipped(inside));
                }
            }
            parts.extend(slice(hints, TabSlice::Hints, 0.0).map(|(part, _)| part));
            parts.extend(slice(head, TabSlice::Head, 0.0).map(|(part, _)| part));
            parts
        };
        self.overlay.parts(
            placed,
            now,
            true,
            |(output, columns, ..)| {
                drawn.set(drawn.get() + 1);
                painter.feature_tab_back(*output, columns, scale, p)
            },
            more,
        )
    }
}

/// The feature tab filling `output` as its parts lay it out, settled
/// (faded in), in one buffer: for the goldens and the dumps.
#[cfg(test)]
pub(super) fn feature_tab_composed(
    painter: &Painter,
    view: &FeatureTabView,
    output: Size<i32, Logical>,
    theme: super::Theme,
    scale: f64,
) -> MemoryRenderBuffer {
    let mut layers = FeatureTabLayers::default();
    let now = std::time::Instant::now();
    let area = Rectangle::from_size(output);
    // The glow half-way, the dots bright.
    let dot = super::blink_dot(Dot::Working, scale, theme.palette());
    let blink = Some((&dot, 1.0));
    let part = |layers: &mut FeatureTabLayers, now| {
        layers.parts(
            Some(view.clone()),
            area,
            theme,
            scale,
            now,
            painter,
            0.5,
            blink,
        )
    };
    let _ = part(&mut layers, now);
    let parts = part(&mut layers, now + std::time::Duration::from_secs(2));
    compose(&parts, output, scale)
}

/// `parts` (front to back) laid one over another on a clear buffer the
/// size of `output`, as the renderer would.
#[cfg(test)]
pub(super) fn compose(
    parts: &[super::layers::Part],
    output: Size<i32, Logical>,
    scale: f64,
) -> MemoryRenderBuffer {
    let mut c = Canvas::new(physical(output, scale));
    let (cw, ch) = (c.w, c.h);
    for part in parts.iter().rev() {
        let loc = part.loc.to_physical(scale).to_i32_round::<i32>();
        let size = physical(part.size, scale);
        let [x0, y0, x1, y1] = match part.clip {
            Some(clip) => {
                let r = clip.to_physical_precise_round::<f64, i32>(scale);
                [r.loc.x, r.loc.y, r.loc.x + r.size.w, r.loc.y + r.size.h]
            }
            None => [i32::MIN, i32::MIN, i32::MAX, i32::MAX],
        };
        let mut buffer = part.buffer.clone();
        let _ = buffer.render().draw(|mem| {
            for y in 0..size.h {
                let oy = loc.y + y;
                if oy < 0 || oy >= ch || oy < y0 || oy >= y1 {
                    continue;
                }
                for x in 0..size.w {
                    let ox = loc.x + x;
                    if ox < 0 || ox >= cw || ox < x0 || ox >= x1 {
                        continue;
                    }
                    let src = ((y * size.w + x) * 4) as usize;
                    let dst = ((oy * cw + ox) * 4) as usize;
                    let Some(px) = mem.get(src..src + 4) else {
                        continue;
                    };
                    let a = u32::from(px[3]);
                    for (d, s) in c.px[dst..dst + 4].iter_mut().zip(px) {
                        *d = (u32::from(*s) + u32::from(*d) * (255 - a) / 255) as u8;
                    }
                }
            }
            Ok::<_, ()>(Vec::new())
        });
    }
    c.into_buffer(false)
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

    /// The feature tab's `slice` (see `TabSlice`), at `region` of a tab
    /// filling `output`: its own buffer, `region` big. A column's slice is
    /// its contents, unscrolled, from `region`'s top; the rest are what is
    /// in their place.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn feature_tab_slice(
        &self,
        view: &FeatureTabView,
        output: Size<i32, Logical>,
        slice: TabSlice,
        region: Rect,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        // Laid out with the region's top left at the origin: the canvas's.
        let frame = Rectangle::new((-region.loc.x, -region.loc.y).into(), output);
        let l = feature_tab_layout(view, frame);
        let mut c = Canvas::new(physical(region.size, scale));
        match slice {
            TabSlice::Head => {
                c.fill(0, 0, c.w, c.h, p.background);
                self.tab_head(&mut c, view, &l, scale, p);
            }
            TabSlice::Column(column) => {
                c.fill(0, 0, c.w, c.h, p.panel);
                let tall = Rectangle::from_size(region.size);
                for (rect, piece) in &l.pieces[column as usize] {
                    if rect.loc.y > tall.size.h || rect.loc.y + rect.size.h < 0 {
                        continue;
                    }
                    self.tab_piece(&mut c, view, &l, *rect, piece, 0, scale, p);
                }
            }
            TabSlice::Hints => {
                c.fill(0, 0, c.w, c.h, p.background);
                self.tab_hints(&mut c, view, &l, scale, p);
            }
        }
        c.into_buffer(true)
    }

    /// What is behind the feature tab's slices filling `output`: its
    /// ground, and each column's card in `columns` (see
    /// `FeatureTabLayout::columns`).
    pub(super) fn feature_tab_back(
        &self,
        output: Size<i32, Logical>,
        columns: &[Option<Rect>; 3],
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(output, scale));
        let s = scale as f32;
        c.fill(0, 0, c.w, c.h, p.background);
        for place in columns.iter().flatten() {
            let (x, y, w, h) = rect_px(*place, scale);
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
        }
        c.into_buffer(true)
    }

    /// The tab's header, rail, the rule under it and, on a narrow tab, the
    /// strip of repository chips, on `c`.
    fn tab_head(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let px = |v: i32| (v as f64 * scale).round() as i32;
        // The header: the title and sub line, the switch, the buttons.
        let (tx, tend) = phys(l.title, scale);
        self.bold_text(
            c,
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
            c,
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
                c,
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
                self.button(c, "", rect, (hovered, false), ButtonStyle::Plain, scale, p);
                let color = if hovered { p.text } else { p.muted };
                icon_sdf(c, rect, scale, color, |x, y| chevron_side(left, x, y, s));
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
                self.button(c, label, *rect, (hovered, false), style, scale, p);
                continue;
            }
            self.button(c, "", *rect, (hovered, false), style, scale, p);
            let color = if destructive { p.on_accent } else { p.text };
            icon_sdf(c, *rect, scale, color, |x, y| match i {
                0 => tab(x, y, s * 0.8),
                1 => copy(x, y, s * 1.2),
                2 => plus(x, y, 0.0, 0.0, 4.5 * s, s),
                _ => trash(x, y, s),
            });
        }

        // The rail: the lines joining the nodes, then each node with its
        // label and line under it.
        let circle = node_circle;
        // The hovered node's band first: the connectors run over it.
        if let Some(FeatureTabHit::Node(i)) = view.hover
            && let Some(slot) = l.nodes.get(i)
        {
            self.row_highlight(c, *slot, (false, 1.0), scale, p);
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
                focus_ring(c, spot, NODE as f32 / 2.0, behind, scale, ring);
            }
            self.rail_node(c, spot, i + 1, node.mark, scale, p);
            let label = Rectangle::new(
                (slot.loc.x + 2, spot.loc.y + NODE + NODE_GAP).into(),
                (slot.size.w - 4, NODE_LABEL).into(),
            );
            let label_color = match node.mark {
                NodeMark::Later if !node.selected => p.muted,
                _ => p.text,
            };
            // A label too wide for its slot takes two lines, its node's
            // short line under both.
            let lines = rail_label(&node.label, label.size.w);
            for (j, text) in lines.iter().enumerate() {
                let rect = Rectangle::new(
                    (label.loc.x, label.loc.y + j as i32 * RAIL_LABEL_STEP).into(),
                    label.size,
                );
                let bold = node.selected;
                self.centred_cut(c, text, SMALL_FONT_SIZE, rect, bold, scale, label_color);
            }
            if !node.line.is_empty() {
                let below = (lines.len() as i32 - 1) * RAIL_LABEL_STEP;
                let line = Rectangle::new(
                    (label.loc.x, label.loc.y + NODE_LABEL + below).into(),
                    (label.size.w, NODE_LINE).into(),
                );
                let color = match node.mark {
                    NodeMark::Current => p.accent,
                    NodeMark::Waiting | NodeMark::Stale => p.warning,
                    _ => p.muted,
                };
                self.centred_cut(c, &node.line, DETAIL_FONT_SIZE, line, false, scale, color);
            }
        }
        // The rule under the rail.
        if let Some(last) = l.nodes.first() {
            let y = last.loc.y + last.size.h + RAIL_GAP;
            c.fill(0, px(y), c.w, px(1).max(1), p.border_variant);
        }

        // On a narrow tab, the repositories as chips.
        if let Some(strip) = l.strip {
            self.repo_strip(c, view, l, strip, scale, p);
        }
    }

    /// The tab's hints, on `c`.
    fn tab_hints(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        scale: f64,
        p: &Palette,
    ) {
        let hover = match view.hover {
            Some(FeatureTabHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(c, &hint_strs(&view.hints), &l.hint_spots, hover, scale, p);
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

    /// The glow pulsing round the current stage's node: the accent,
    /// fading out from the node's edge over `PULSE_HALO`, on a clear
    /// buffer `NODE + 2 * PULSE_HALO` square. Its opacity is given when
    /// it is composed (`rail_pulse_alpha`).
    pub(super) fn rail_pulse(&self, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let size = NODE + 2 * PULSE_HALO;
        let mut c = Canvas::new(physical(Size::from((size, size)), scale));
        let s = scale as f32;
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let (r, reach) = (NODE as f32 / 2.0 * s, PULSE_HALO as f32 * s);
        let (w, h) = (c.w, c.h);
        c.paint_in(0, 0, w, h, p.accent, |x, y| {
            let d = (x - cx).hypot(y - cy) - r;
            let edge = (d + 0.5).clamp(0.0, 1.0);
            let fade = (1.0 - d / reach).clamp(0.0, 1.0);
            edge * fade * fade
        });
        c.into_buffer(false)
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
                self.repo_dot(c, repo_chip_dot(*chip), repo, scale, p);
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
        // The chips standing for those that don't fit.
        for (chip, i, label) in &l.more_repos {
            let lit = view.hover == Some(FeatureTabHit::MoreRepos(*i));
            let (x, y, w, h) = rect_px(*chip, scale);
            let fill = super::row_fill(p, false, if lit { 1.0 } else { 0.0 }).unwrap_or(p.panel);
            let b = s.round().max(1.0);
            c.rounded(x, y, w, h, h / 2.0, p.border_variant);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, h / 2.0 - b, fill);
            let color = if lit { p.text } else { p.muted };
            self.centered(c, label, SMALL_FONT_SIZE, *chip, scale, color);
        }
    }

    /// A repository's dot in `spot`: touched in the success colour, else
    /// a quiet one. One being touched blinks: it is composed over the tab
    /// (see `feature_tab_blinkers`).
    fn repo_dot(&self, c: &mut Canvas, spot: Rect, repo: &FeatureRepoRow, scale: f64, p: &Palette) {
        let color = if repo.busy {
            return;
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
                    picks,
                }) = view.page.sections.get(*k)
                else {
                    return;
                };
                let head = card_frame(c, rect, scale, p);
                if let Some((track, chips)) = (!picks.is_empty())
                    .then_some(l.agent_picks.as_ref())
                    .flatten()
                {
                    let track = at(*track);
                    if name.is_empty() {
                        let label = Rectangle::new(
                            (head.loc.x, track.loc.y).into(),
                            (PICK_LABEL, track.size.h).into(),
                        );
                        line(c, "Agent", label, SMALL_FONT_SIZE, p.muted);
                    }
                    let rects: Vec<Rect> = chips.iter().map(|r| at(*r)).collect();
                    let on = picks.iter().position(|c| c.lit).unwrap_or(usize::MAX);
                    let hover = match view.hover {
                        Some(FeatureTabHit::AgentPick(i)) => Some(i),
                        _ => None,
                    };
                    let off: Vec<usize> = (picks.iter().enumerate())
                        .filter(|(_, c)| c.off)
                        .map(|(i, _)| i)
                        .collect();
                    self.segment_grounds(c, track, &rects, (on, hover, &off), scale, p);
                    for (i, (chip, rect)) in picks.iter().zip(&rects).enumerate() {
                        let tint = segment_tint(i, on, &off, p);
                        agent_icon(c, *rect, PICK_ICON, chip.icon, tint, scale);
                    }
                }
                if name.is_empty() {
                    return;
                }
                let icon_spot = Rectangle::new(head.loc, (16, AGENT_HEAD).into());
                agent_icon(c, icon_spot, 16, *icon, p.text, scale);
                // A dot at work blinks: it is composed over the tab.
                status_dot(c, agent_card_dot(rect), *dot, scale, p);
                let button_rect = button.and_then(|i| l.page_buttons.get(i).copied().flatten());
                let track = (!picks.is_empty())
                    .then_some(l.agent_picks.as_ref())
                    .flatten()
                    .map(|(track, _)| at(*track));
                let end_x = track
                    .or(button_rect)
                    .map_or(head.loc.x + head.size.w, |r| r.loc.x - QUIT_BUTTON_GAP);
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
                let left = rect.loc.x + DOT_ROOM + 4;
                // The lines it changes, at the end of its name's line,
                // when they leave the name room.
                let stat = Rectangle::new(
                    (left + STAT_NAME_MIN, rect.loc.y + 2).into(),
                    ((end - left - STAT_NAME_MIN).max(0), 20).into(),
                );
                let name_end = match self.repo_stat(c, repo.stat, stat, scale, p) {
                    Some(from) => from - STAT_GAP,
                    None => end,
                };
                let name = Rectangle::new(
                    (left, rect.loc.y + 2).into(),
                    ((name_end - left).max(1), 20).into(),
                );
                self.repo_dot(c, repo_row_dot(rect), repo, scale, p);
                let color = if repo.touched { p.text } else { p.muted };
                line(c, &repo.name, name, FONT_SIZE, color);
                let branch = Rectangle::new(
                    (name.loc.x, name.loc.y + 20).into(),
                    ((end - left).max(1), 18).into(),
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
            Piece::File(k) => {
                let Some(PageSection::File {
                    name,
                    state,
                    buttons,
                }) = view.page.sections.get(*k)
                else {
                    return;
                };
                let head = card_frame(c, rect, scale, p);
                let first = buttons
                    .iter()
                    .find_map(|i| l.page_buttons.get(*i).copied().flatten());
                let end_x = first.map_or(head.loc.x + head.size.w, |r| r.loc.x - QUIT_BUTTON_GAP);
                let text =
                    Rectangle::new(head.loc, ((end_x - head.loc.x).max(1), AGENT_HEAD).into());
                let (tx, tend) = phys(text, scale);
                let hb = band(text, scale);
                let after = self.bold_text(c, name, FONT_SIZE, tx, tend, hb, scale, p.text);
                let ink = Ink::Tone(state.tone).color(p);
                let from = after + (8.0 * s).round();
                self.text(c, &state.text, SMALL_FONT_SIZE, from, tend, hb, scale, ink);
                for &i in buttons {
                    if let Some(r) = l.page_buttons.get(i).copied().flatten() {
                        self.page_button(c, view, i, at(r), scale, p);
                    }
                }
            }
            Piece::Button(i) => self.page_button(c, view, *i, rect, scale, p),
            Piece::Activity { row, lines } => {
                let Some(a) = view.activity.get(*row) else {
                    return;
                };
                let dot = Rectangle::new((rect.loc.x + 1, rect.loc.y + 5).into(), (8, 8).into());
                activity_dot(c, dot, a.tone, scale, p);
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

    /// A repository's `stat` as "+A −R" (added in the success
    /// colour, removed in the error colour, a side that is 0 left out),
    /// from the right of `rect` if it fits in it: where it starts (in
    /// logical pixels), or `None` when nothing is drawn.
    fn repo_stat(
        &self,
        c: &mut Canvas,
        stat: Option<(usize, usize)>,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) -> Option<i32> {
        let (added, removed) = stat?;
        let parts: Vec<(String, Rgb)> = [
            (added, format!("+{added}"), p.success),
            (removed, format!("\u{2212}{removed}"), p.error),
        ]
        .into_iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(_, text, color)| (text, color))
        .collect();
        if parts.is_empty() {
            return None;
        }
        let s = scale as f32;
        let gap = (STAT_SPACE as f32 * s).round();
        let widths: Vec<f32> = (parts.iter())
            .map(|(text, _)| self.text_width(text, DETAIL_FONT_SIZE, scale).ceil())
            .collect();
        let total = widths.iter().sum::<f32>() + gap * (parts.len() - 1) as f32;
        let (x, end) = phys(rect, scale);
        if total > end - x {
            return None;
        }
        let mut from = (end - total).floor();
        let start = from;
        for ((text, color), w) in parts.iter().zip(&widths) {
            self.text(
                c,
                text,
                DETAIL_FONT_SIZE,
                from,
                from + w + 1.0,
                band(rect, scale),
                scale,
                *color,
            );
            from += w + gap;
        }
        Some((start / s).floor() as i32)
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

    /// How long drawing the tab's parts takes at 2810x1856, scale 1:
    /// all of them, then after a page button's hover changes, a column
    /// scrolls a step and a rail node's hover changes. Run with `cargo
    /// test --release --bin gerdr paint_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_tab_paint_cost() {
        use crate::chrome::{Theme, layers::Part};
        use std::time::{Duration, Instant};
        let painter = crate::chrome::Painter::new();
        let area = Rectangle::from_size((2810, 1856).into());
        let mut view = tab_view();
        let row = view.activity[0].clone();
        view.activity = (0..120).map(|_| row.clone()).collect();
        let t0 = Instant::now();
        let frame = std::cell::Cell::new(0);
        let parts = |layers: &mut FeatureTabLayers, view: &FeatureTabView| -> Vec<Part> {
            frame.set(frame.get() + 1);
            let now = t0 + Duration::from_secs(frame.get());
            layers.parts(
                Some(view.clone()),
                area,
                Theme::Dark,
                1.0,
                now,
                &painter,
                0.5,
                None,
            )
        };
        let n = 20;
        let time = |what: &str, f: &mut dyn FnMut(usize)| {
            let start = Instant::now();
            for i in 0..n {
                f(i);
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
            eprintln!("{what}: {ms:.2} ms");
        };
        time("full", &mut |_| {
            let _ = parts(&mut FeatureTabLayers::default(), &view);
        });
        let mut layers = FeatureTabLayers::default();
        let _ = parts(&mut layers, &view);
        let mut run = |what: &str, change: &dyn Fn(&mut FeatureTabView, usize)| {
            let before = layers.drawn.get();
            time(what, &mut |i| {
                change(&mut view, i);
                let _ = parts(&mut layers, &view);
            });
            eprintln!("  buffers drawn: {}", layers.drawn.get() - before);
        };
        run("hover page button", &|v, i| {
            v.hover = Some(FeatureTabHit::PageButton(i % 2));
        });
        run("scroll activity", &|v, i| {
            v.hover = None;
            v.scroll[TabColumn::Activity as usize] = (i + 1) as f64 * 20.0;
        });
        run("hover rail node", &|v, i| {
            v.hover = Some(FeatureTabHit::Node(i % 2));
        });
    }

    /// A change to one slice of the view draws only that slice again,
    /// and a scroll only moves its column's buffer.
    #[test]
    fn a_hover_or_a_scroll_draws_only_its_slice() {
        use crate::chrome::Theme;
        use std::time::{Duration, Instant};
        let painter = crate::chrome::Painter::new();
        let area = Rectangle::from_size((1920, 900).into());
        let mut view = tab_view();
        let row = view.activity[0].clone();
        view.activity = (0..80).map(|_| row.clone()).collect();
        let mut layers = FeatureTabLayers::default();
        let t0 = Instant::now();
        let step = |layers: &mut FeatureTabLayers, view: &FeatureTabView, k: u64| {
            let now = t0 + Duration::from_secs(k);
            let before = layers.drawn.get();
            let parts = layers.parts(
                Some(view.clone()),
                area,
                Theme::Dark,
                1.0,
                now,
                &painter,
                0.5,
                None,
            );
            (layers.drawn.get() - before, parts)
        };
        // The head, the hints and the three columns, then the ground once
        // it starts to show.
        assert_eq!(step(&mut layers, &view, 0).0, 5);
        assert_eq!(step(&mut layers, &view, 1).0, 1);
        assert_eq!(step(&mut layers, &view, 2).0, 0);
        view.hover = Some(FeatureTabHit::PageButton(0));
        assert_eq!(step(&mut layers, &view, 2).0, 1);
        view.hover = Some(FeatureTabHit::Node(3));
        assert_eq!(step(&mut layers, &view, 3).0, 2);
        view.hover = Some(FeatureTabHit::Column(TabColumn::Page));
        assert_eq!(step(&mut layers, &view, 4).0, 1);
        let (_, at_top) = step(&mut layers, &view, 5);
        view.scroll[TabColumn::Activity as usize] = 40.0;
        let (drawn, scrolled) = step(&mut layers, &view, 6);
        assert_eq!(drawn, 0);
        let moved: Vec<f64> = (at_top.iter().zip(&scrolled))
            .map(|(a, b)| a.loc.y - b.loc.y)
            .filter(|dy| *dy != 0.0)
            .collect();
        assert_eq!(moved.len(), 2, "the column's contents and its thumb");
        assert_eq!(moved[1], 40.0);
    }

    /// A narrow tab's strip ends in "+N" for the repositories past it,
    /// and starts with "N+" when it starts past the first; a click on
    /// either selects the nearest hidden one.
    #[test]
    fn the_strip_marks_the_repositories_it_hides() {
        let mut view = tab_view();
        let row = view.repos[0].clone();
        view.repos = (0..12)
            .map(|i| FeatureRepoRow {
                name: format!("repository-{i}"),
                ..row.clone()
            })
            .collect();
        let l = tab_at(&view, 850);
        let shown = l.repos.iter().filter(|(r, _)| r.size.w > 0).count();
        assert!(shown > 0 && shown < 12);
        let [(chip, i, label)] = l.more_repos.as_slice() else {
            panic!("one mark: {:?}", l.more_repos);
        };
        assert_eq!(
            (*i, label.as_str()),
            (shown, format!("+{}", 12 - shown).as_str())
        );
        let strip = l.strip.unwrap();
        assert!(chip.loc.x + chip.size.w <= strip.loc.x + strip.size.w);
        assert_eq!(
            l.hit(&view, centre(*chip)),
            Some(FeatureTabHit::MoreRepos(shown))
        );
        // From the 10th: the strip starts there, after "9+".
        view.repo = 10;
        let l = tab_at(&view, 850);
        let (lead, i, label) = &l.more_repos[0];
        assert_eq!((*i, label.as_str()), (9, "10+"));
        assert!(l.repos[10].0.size.w > 0);
        assert!(lead.loc.x + lead.size.w <= l.repos[10].0.loc.x);
        assert_eq!(
            l.hit(&view, centre(*lead)),
            Some(FeatureTabHit::MoreRepos(9))
        );
    }

    /// A label wider than its slot (in bold, as when selected) goes on
    /// two lines. "Manual test" and "Code review" fit on one down to
    /// about 540 px wide (a slot of about 64 px in bold).
    #[test]
    fn long_rail_labels_take_two_lines_on_a_narrow_tab() {
        let painter = crate::chrome::Painter::new();
        assert!(painter.font.is_some(), "the widths need a font");
        let view = tab_view();
        let lines = |w: i32| -> Vec<Vec<String>> {
            let l = tab_at(&view, w);
            (l.nodes.iter().zip(&view.rail))
                .map(|(slot, node)| rail_label(&node.label, slot.size.w - 4))
                .collect()
        };
        for w in [870, 1180, 1920] {
            assert!(
                lines(w).iter().all(|l| l.len() == 1),
                "at {w}: {:?}",
                lines(w)
            );
        }
        let narrow = lines(500);
        let at = |label: &str| {
            let i = view.rail.iter().position(|n| n.label == label).unwrap();
            narrow[i].clone()
        };
        assert_eq!(at("Manual test"), ["Manual", "test"]);
        assert_eq!(at("Code review"), ["Code", "review"]);
        assert_eq!(at("Ship"), ["Ship"]);
        // One word stays one line, however narrow: it is cut instead.
        assert_eq!(rail_label("Discover", 10), ["Discover"]);
    }

    /// The dots blinking over the tab: the agent card's while its agent
    /// works, a repository's while it is touched, on its row or chip.
    #[test]
    fn the_blinking_dots_sit_on_what_is_at_work() {
        let mut view = tab_view();
        let l = tab_at(&view, 1920);
        let dots = feature_tab_blinkers(&view, &l);
        let card = (l.pieces[TabColumn::Page as usize].iter())
            .find(|(_, piece)| matches!(piece, Piece::Agent(_)))
            .map(|(rect, _)| *rect)
            .unwrap();
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].1, Some(TabColumn::Page));
        assert!(card.contains_rect(dots[0].0));
        assert_eq!(dots[0].0.size, Size::from((DOT, DOT)));

        if let Some(PageSection::Agent { dot, .. }) = view.page.sections.first_mut() {
            *dot = Dot::Idle;
        }
        view.repos[1].busy = true;
        let l = tab_at(&view, 1920);
        let dots = feature_tab_blinkers(&view, &l);
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].1, Some(TabColumn::Repos));
        assert!(l.repos[1].0.contains_rect(dots[0].0));

        let l = tab_at(&view, 850);
        let dots = feature_tab_blinkers(&view, &l);
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].1, None);
        assert!(l.repos[1].0.contains_rect(dots[0].0));
        view.repos_error = Some("no".into());
        assert!(feature_tab_blinkers(&view, &tab_at(&view, 850)).is_empty());
    }

    /// The current node's glow and the dots at work change only their
    /// parts' opacity: nothing is drawn again for them. A dot in a
    /// column moves with its scroll and shows only inside the column.
    #[test]
    fn the_pulse_and_the_blink_draw_nothing_again() {
        use crate::chrome::Theme;
        use std::time::{Duration, Instant};
        let painter = crate::chrome::Painter::new();
        let area = Rectangle::from_size((1920, 900).into());
        let p = Theme::Dark.palette();
        let dot = crate::chrome::blink_dot(Dot::Working, 1.0, p);
        let mut view = tab_view();
        let row = view.activity[0].clone();
        view.activity = (0..80).map(|_| row.clone()).collect();
        let mut layers = FeatureTabLayers::default();
        let t0 = Instant::now();
        let mut step = |view: &FeatureTabView, k: u64, pulse: f32, blink: f32| {
            let now = t0 + Duration::from_secs(k);
            let before = layers.drawn.get();
            let parts = layers.parts(
                Some(view.clone()),
                area,
                Theme::Dark,
                1.0,
                now,
                &painter,
                pulse,
                Some((&dot, blink)),
            );
            (layers.drawn.get() - before, parts)
        };
        let _ = step(&view, 0, 0.0, 1.0);
        let _ = step(&view, 1, 0.0, 1.0);
        let l = tab_at(&view, 1920);
        let halo = feature_tab_pulse(&view, &l).unwrap();
        assert!(halo.contains_rect(node_circle(l.nodes[2])));
        let (drawn, parts) = step(&view, 2, 0.8, 0.4);
        assert_eq!(drawn, 0);
        let glow = (parts.iter())
            .find(|part| part.loc == halo.loc.to_f64())
            .unwrap();
        assert_eq!(glow.alpha, rail_pulse_alpha(0.8));
        let (spot, _) = feature_tab_blinkers(&view, &l)[0];
        let blinker = |parts: &[crate::chrome::layers::Part]| {
            parts
                .iter()
                .find(|part| part.alpha == 0.4 && part.size == spot.size)
                .cloned()
                .unwrap()
        };
        assert_eq!(blinker(&parts).loc, spot.loc.to_f64());
        let page = l.columns[TabColumn::Page as usize].unwrap();
        assert!(page.contains_rect(blinker(&parts).clip.unwrap()));
        view.scroll[TabColumn::Page as usize] = 12.5;
        let (_, parts) = step(&view, 3, 0.8, 0.4);
        assert_eq!(blinker(&parts).loc.y, f64::from(spot.loc.y) - 12.5);
        // No pulse at all: no glow.
        let (drawn, parts) = step(&view, 4, 0.0, 0.4);
        assert_eq!(drawn, 0);
        assert!(!parts.iter().any(|part| part.loc == halo.loc.to_f64()));
        for node in &mut view.rail {
            node.mark = NodeMark::Done;
        }
        assert_eq!(feature_tab_pulse(&view, &l), None);
    }

    /// The line stat's colours read on the repositories' column, on a
    /// row at rest, hovered and selected.
    #[test]
    fn the_line_stat_reads_on_the_rows_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT] {
            for color in [p.success, p.error] {
                for ground in [p.panel, p.element, p.selected] {
                    let ratio = contrast(color, ground);
                    assert!(ratio >= 3.0, "{color:?} on {ground:?} at {ratio:.2}:1");
                }
            }
        }
    }

    /// The fixture's agent card, with chips.
    fn tab_with_picks(off: bool) -> FeatureTabView {
        let mut view = tab_view();
        let chips = vec![
            AgentChip {
                icon: AgentIcon::Claude,
                name: "Claude Code".into(),
                lit: true,
                off: false,
            },
            AgentChip {
                icon: AgentIcon::Cursor,
                name: "Cursor".into(),
                lit: false,
                off,
            },
        ];
        let card = view
            .page
            .sections
            .iter_mut()
            .find_map(|s| match s {
                PageSection::Agent { picks, .. } => Some(picks),
                _ => None,
            })
            .expect("the fixture has an agent card");
        *card = chips;
        view
    }

    #[test]
    fn an_agent_chip_is_hit_where_it_shows_also_scrolled() {
        let mut view = tab_with_picks(false);
        let l = tab_at(&view, 1180);
        let (track, chips) = l.agent_picks.clone().expect("placed");
        assert!(chips[0].size.w > 0 && chips[1].loc.x >= chips[0].loc.x + chips[0].size.w);
        let page = l.columns[TabColumn::Page as usize].unwrap();
        assert!(inside(page, track));
        assert_eq!(
            l.hit(&view, centre(chips[1])),
            Some(FeatureTabHit::AgentPick(1))
        );
        view.scroll[TabColumn::Page as usize] = 12.0;
        let lifted = Point::from((centre(chips[0]).x, centre(chips[0]).y - 12));
        assert_eq!(l.hit(&view, lifted), Some(FeatureTabHit::AgentPick(0)));
    }

    #[test]
    fn a_chip_that_cant_be_picked_only_says_why() {
        let view = tab_with_picks(true);
        let l = tab_at(&view, 1180);
        let (_, chips) = l.agent_picks.clone().expect("placed");
        assert_eq!(
            l.hit(&view, centre(chips[1])),
            Some(FeatureTabHit::AgentPickOff(1))
        );
    }

    #[test]
    fn agent_chips_too_wide_together_share_the_room() {
        let (track, chips) = agent_pick_rects(8, 10, 20, 150);
        assert!(track.size.w <= 150, "{track:?}");
        assert_eq!(chips[0].size.w, chips[7].size.w);
        assert!(chips[7].loc.x + chips[7].size.w <= 10 + 150, "{chips:?}");
    }

    #[test]
    fn a_runs_chips_share_its_heads_row_left_of_its_button() {
        let view = tab_with_picks(false);
        let l = tab_at(&view, 1180);
        let (track, _) = l.agent_picks.clone().expect("placed");
        let button = (view.page.sections.iter())
            .find_map(|s| match s {
                PageSection::Agent { button, .. } => *button,
                _ => None,
            })
            .and_then(|i| l.page_buttons[i])
            .expect("the fixture's card has a button");
        assert_eq!(track.loc.y, button.loc.y);
        assert_eq!(track.size.h, button.size.h);
        assert_eq!(track.loc.x + track.size.w + QUIT_BUTTON_GAP, button.loc.x);
    }
}
