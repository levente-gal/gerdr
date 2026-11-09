//! The features' surfaces (see `crate::features`): the new feature dialog
//! over everything, the feature tab (the full-size first tab of a
//! feature's workspace: a header, the stages' rail, the repositories' strip
//! that unfolds into rows, and the stage page's and activity's columns,
//! each scrolled on its own), and
//! the stage marks the sidebar's feature rows show.

use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    rc::Rc,
    sync::Arc,
};

use super::{
    AGENT_ICON, BUTTON_PADDING, ButtonStyle, CHECKBOX_GAP, CHECKBOX_ROW, CHECKBOX_SIZE, Canvas,
    Check, DETAIL_FONT_SIZE, DIALOG_WIDTH, DOT, Dot, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE,
    ListBar, POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP,
    QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, Thumb, agent_icon,
    band, button_width, check, chevron_down, chevron_side, copy, coverage, cross, dialog_buttons,
    field_lines_height, field_text, focus_ring, footer_hint_spots, gutter_track, hint_strs,
    icon_sdf, measure, measure_bold, measured_footer_height, mix, phys, physical, plus, rect_px,
    segment, segment_tint, status_dot, tab, trash,
};
use super::{
    bars, code_mark, eye, flask, magnifier, plan_review_mark, ship_arrow, sparkle, text_lines,
};
use crate::text_field::Line;
use crate::{
    agent_icons::AgentIcon,
    feature::{Stage, page::Stat},
    md_blocks::{DocBlock, ItemMark},
};
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
/// What the new feature dialog's field says while empty.
pub const TITLE_HINT: &str = "What it does, in a few words";
/// How wide a repository row's base branch button is: the same on all, so
/// they line up, whatever the branch.
const BASE_BUTTON_W: i32 = 168;
/// How far a long branch name stays from that button's ends.
const BASE_BUTTON_INSET: i32 = 8;
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
pub const FEATURE_BUTTONS: [&str; 4] = ["Open tab", "Copy path", "Stats", "New feature\u{2026}"];

/// Where a stage stands in a feature, for its mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StageMark {
    Done,
    /// The first not done: the one being worked on.
    Current,
    Later,
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
    /// What follows the name: what the feature does with it, or why a
    /// project can't be linked.
    pub place: String,
    /// Whether the feature changes it (a project: links it).
    pub touched: bool,
    /// It can't be ticked or left out: the only repository, a project
    /// linked already or not found.
    pub fixed: bool,
    /// The label of its base branch button, a repository's only.
    pub base: Option<String>,
}

/// What the new feature dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeatureView {
    /// Its heading: "New feature in “Shop”".
    pub heading: String,
    /// Its field's text: the title, or what filters the branches.
    pub title: Line,
    /// What its field says while empty.
    pub hint: &'static str,
    /// Its field has the keys.
    pub field_focus: bool,
    /// The row with the keyboard; `None`: the title field.
    pub focus: Option<usize>,
    /// The row slots it makes room for (see `new_feature_layout`).
    pub slots: usize,
    /// The line about where the feature goes: the root, or that it is
    /// being found, or why there is none.
    pub root: String,
    pub root_failed: bool,
    /// The Projects row: the home project and those linked.
    pub projects: String,
    /// The label of the Projects row's button (`LINK_BUTTON`).
    pub link: &'static str,
    /// The rows: the repositories, or the projects to link.
    pub repos: Vec<NewFeatureRepo>,
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
    /// That row's base branch button.
    Base(usize),
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
    /// Where each of those has its base branch button.
    pub bases: Vec<Rect>,
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
    let rows: Vec<Rect> = (0..shown as i32)
        .map(|i| {
            let y = y + i * (CHECKBOX_ROW + ROW_GAP);
            Rectangle::new((x, y).into(), (inner, CHECKBOX_ROW).into())
        })
        .collect();
    let bases = (rows.iter())
        .map(|row| {
            let x = row.loc.x + row.size.w - ROW_INSET - BASE_BUTTON_W;
            Rectangle::new((x, row.loc.y).into(), (BASE_BUTTON_W, row.size.h).into())
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
        bases,
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
        if let Some(slot) = self.bases.iter().position(on) {
            return Some(NewFeatureHit::Base(top + slot));
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
    let area = new_feature_layout(size, view.slots, view.error.is_some()).field;
    FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: view.title.clone(),
    }
}

// ----------------------------------------------------------------- panel

/// What a repository is to the feature, by its dot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RepoKind {
    /// On the feature's branch.
    Active,
    /// Agents read it, and the feature doesn't change it.
    Context,
    /// Not in the feature: taken out, or never added.
    #[default]
    Other,
}

impl RepoKind {
    /// Its dot's colour. Other sits between border and muted, so it stays
    /// quieter than the rest and still shows on a selected row's fill.
    fn color(self, p: &Palette) -> Rgb {
        match self {
            RepoKind::Active => p.success,
            RepoKind::Context => p.muted,
            RepoKind::Other => mix(p.border, p.muted, 0.85),
        }
    }
}

/// What the unfolded rows say about a group they couldn't list yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoNote {
    pub caption: String,
    pub text: String,
    pub error: bool,
}

/// A repository of the feature panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureRepoRow {
    pub name: String,
    /// Its branch, or the base it is at as context.
    pub line: String,
    pub kind: RepoKind,
    /// The heading of the group it starts, on the unfolded rows.
    pub group: Option<String>,
    /// Being touched now.
    pub busy: bool,
    /// The labels of its buttons: what else it can be, Open PR for one
    /// with a pull request.
    pub buttons: Vec<&'static str>,
    /// Whether it shows the button taking it out of the feature.
    pub remove: bool,
    /// Files changed and lines added and removed since its base,
    /// uncommitted changes included, once read (`page::stat`).
    pub stat: Option<Stat>,
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

/// A tick box on the Ship page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShipStepRow {
    /// Its place among the page's rows (`Gerdr`'s `ship_rows`).
    pub row: usize,
    pub label: String,
    /// What goes under it: why it can't run, or what it means.
    pub detail: Option<CheckLine>,
    pub ticked: bool,
    /// It can't run as things are: dim. It can still be ticked or unticked.
    pub off: bool,
}

/// A repository's choice of way on the Ship page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShipRouteRow {
    /// Its place among the page's rows.
    pub row: usize,
    /// The segment that is on, of `SHIP_ROUTES`.
    pub on: usize,
}

/// A repository on the Ship page, or what follows shipping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShipGroup {
    pub head: String,
    /// `head` reads as a caption over its rows, not a line of its own.
    pub caption: bool,
    pub route: Option<ShipRouteRow>,
    pub note: Option<String>,
    pub steps: Vec<ShipStepRow>,
    /// The row under the keyboard's cursor, of all the page's.
    pub selected: Option<usize>,
}

/// Where the Ship page's rows go, unscrolled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipSpots {
    /// Each choice: its row, its outline, and its segments.
    pub routes: Vec<(usize, Rect, [Rect; 3])>,
    /// Each row, the choices too, in the page's order.
    pub rows: Vec<(usize, Rect)>,
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
    /// review, a stop to go on from): a warning dot. Or a done stage whose
    /// agent waits with work sent back to it.
    Waiting,
    /// Done, its agent running again on work sent back to it: filled in the
    /// accent and pulsing, over the current stage, where nothing runs then.
    Fixing,
    /// Done once and not standing now: its mark went when the code changed
    /// or it was reopened, or the flow is back at Build (`RailBack`) and it's
    /// Build or after. A grey ring with a tick.
    Ran,
    /// The stage last worked on: the current one with the flow back at Build
    /// on its account and the next move not its own, or the one the flow has
    /// since gone back from. An accent ring round its number.
    Paused,
}

impl NodeMark {
    /// The colour of its dot on a sidebar card, the one of its node on the
    /// rail; still to come in the placeholder's (dim, but readable as a
    /// mark: 3:1 at least).
    pub(super) fn dot_color(self, p: &Palette) -> Rgb {
        match self {
            Self::Done => p.success,
            Self::Current | Self::Fixing | Self::Paused => p.accent,
            Self::Stale | Self::Waiting => p.warning,
            Self::Ran => p.muted,
            Self::Later => p.placeholder,
        }
    }

    /// The shape of its dot on a sidebar card: a ring for what isn't
    /// behind the flow or at work, as on the rail.
    fn dot_shape(self) -> StageMark {
        match self {
            Self::Later | Self::Paused => StageMark::Later,
            _ => StageMark::Done,
        }
    }

    /// How a stage's tooltip says it stands.
    pub fn word(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Current => "current",
            Self::Later => "later",
            Self::Stale => "code changed",
            Self::Waiting => "waiting for you",
            Self::Fixing => "running again",
            Self::Ran => "ran once",
            Self::Paused => "last worked on",
        }
    }
}

/// The flow turned back to Build: the line on the feature tab's rail, from
/// the stage that sent work back to Build's. Indices into the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RailBack {
    pub from: usize,
    pub to: usize,
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
    /// What a done stage's agent ran as: its logo, and its model, else its
    /// name. Under `line`, as the slot is too narrow for both on one row.
    pub agent: Option<(AgentIcon, String)>,
}

/// The feature tab's areas the keyboard moves between, with Tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabArea {
    /// The rail: the keys pick a stage's page.
    #[default]
    Stages,
    /// The repositories' strip or rows: the keys pick a repository.
    Repos,
    /// AI review's finding cards: the keys pick and decide one.
    Findings,
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
/// know, by its index in `StagePage::buttons`. Its key is left to the footer's
/// hints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageButton {
    pub label: String,
    /// `Primary` for the one to press next, which Enter presses, `Quiet`
    /// for one to have at hand that nothing asks for.
    pub style: ButtonStyle,
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

/// What Code review's page shows under the rail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewView {
    /// The diff, its file list and the comments being written.
    #[default]
    Code,
    /// The fixes the comments asked for: the plan and the comments as cards.
    Fixes,
}

/// A feature's code review's files, as `ReviewSwitch` counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReviewFiles {
    pub viewed: usize,
    pub total: usize,
    /// Viewed, then changed since.
    pub changed: usize,
}

impl ReviewFiles {
    /// What is left to read in a few words, and whether that is all of them
    /// read: "5 of 8 files left · 1 changed since viewed", "All 8 files viewed".
    fn left(self) -> (String, bool) {
        let left = self.total.saturating_sub(self.viewed);
        let files = match self.total {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        if left == 0 {
            return (format!("All {files} viewed"), true);
        }
        let text = match self.changed {
            0 => format!("{left} of {files} left"),
            n => format!("{left} of {files} left \u{b7} {n} changed since viewed"),
        };
        (text, false)
    }
}

/// The line under Code review's rail: the two views to go between, with how far
/// each has come, the files left to review or what to do next, and the
/// stage's actions at its right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSwitch {
    pub view: ReviewView,
    /// None until they can be counted.
    pub files: Option<ReviewFiles>,
    /// How the fixes stand: "2 of 3 made", "planning fixes".
    pub fixes: String,
    /// What to do next, and whether it reads well: all done but Approve, or
    /// approved; none says only how many files are left.
    pub next: Option<(String, bool)>,
    /// The review's actions, from the left: on the code's view alone.
    pub buttons: Vec<ReviewButton>,
}

/// An action on Code review's line: what the review's own title row has in
/// its popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewButton {
    pub button: super::ReposButton,
    pub label: String,
    /// `Primary` for the one to press next, `Off` while it can't be pressed.
    pub style: ButtonStyle,
    /// Its key, for its tooltip: the footer lists it too.
    pub key: String,
    /// Why it can't be pressed now.
    pub disabled: Option<String>,
}

impl ReviewSwitch {
    /// What the line says after the bar, and whether that is the good news.
    fn said(&self) -> Option<(String, bool)> {
        match &self.next {
            Some(next) => Some(next.clone()),
            None => (self.files.filter(|f| f.total > 0)).map(ReviewFiles::left),
        }
    }

    /// What each segment says.
    fn labels(&self) -> [String; 2] {
        let code = match self.files {
            Some(files) => format!("Code review \u{b7} {} of {} files", files.viewed, files.total),
            None => "Code review".to_owned(),
        };
        [code, format!("Fixes \u{b7} {}", self.fixes)]
    }
}

/// The line under Manual test's rail: how far trying its criteria came, in a
/// bar and in words, as Code review's switch says how far its review did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestStrip {
    pub passed: usize,
    pub failed: usize,
    pub untested: usize,
    /// "3 passed · 1 failed · 2 untested".
    pub text: String,
    /// The search through the criteria, open; closed, a button opens it.
    pub search: Option<super::SearchBar>,
}

impl TestStrip {
    fn total(&self) -> usize {
        self.passed + self.failed + self.untested
    }

    /// Whether every criterion passed: the good news.
    fn good(&self) -> bool {
        self.total() > 0 && self.passed == self.total()
    }
}

/// Where `TestStrip`'s parts go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestStripSpots {
    /// The bar, while there is room.
    pub bar: Option<Rect>,
    /// The words.
    pub text: Rect,
    /// The search's parts, at the row's right: its button, or its field
    /// and the button closing it.
    pub search: Vec<(super::SearchPart, Rect)>,
    /// Where the open search says how many criteria it shows.
    pub count: Option<Rect>,
}

/// How wide the criteria's search field is at most, and at least; room for
/// how many it shows.
const STRIP_SEARCH_W: i32 = 260;
const STRIP_SEARCH_MIN: i32 = 120;
const STRIP_COUNT_W: i32 = 76;

/// Where `strip` goes in the row at `(x, y)`, `width` wide: the bar, then its
/// words, then at the right its search (`open`: its field).
fn test_strip_spots(x: i32, y: i32, width: i32, open: bool) -> TestStripSpots {
    use super::SearchPart;
    let h = REVIEW_SWITCH_H;
    let right = x + width;
    let mut search = Vec::new();
    let mut count = None;
    let width = if open {
        // Its buttons to the match before and the next, and closing it.
        let mut left = right;
        let mut buttons = Vec::new();
        for part in [SearchPart::Close, SearchPart::Next, SearchPart::Prev] {
            left -= h;
            buttons.push((part, Rectangle::new((left, y).into(), (h, h).into())));
            left -= QUIT_BUTTON_GAP;
        }
        let count_at = left - STRIP_COUNT_W;
        let field_w = STRIP_SEARCH_W.min(width / 2).max(STRIP_SEARCH_MIN);
        let field_x = (count_at - QUIT_BUTTON_GAP - field_w).max(x);
        let field = Rectangle::new(
            (field_x, y).into(),
            ((count_at - QUIT_BUTTON_GAP - field_x).max(0), h).into(),
        );
        count = Some(Rectangle::new(
            (count_at, y).into(),
            (STRIP_COUNT_W, h).into(),
        ));
        search.push((SearchPart::Field, field));
        search.extend(buttons.into_iter().rev());
        (field_x - COLUMN_GAP - x).max(0)
    } else {
        let button = Rectangle::new((right - h, y).into(), (h, h).into());
        search.push((SearchPart::Open, button));
        (button.loc.x - COLUMN_GAP - x).max(0)
    };
    let has_bar = width >= REVIEW_BAR_W + COLUMN_GAP + REVIEW_LEFT_MIN;
    let bar = has_bar.then(|| {
        Rectangle::new(
            (x, y + (h - REVIEW_BAR_H) / 2).into(),
            (REVIEW_BAR_W, REVIEW_BAR_H).into(),
        )
    });
    let text_x = bar.map_or(x, |bar| bar.loc.x + bar.size.w + COLUMN_GAP);
    let text = Rectangle::new((text_x, y).into(), ((x + width - text_x).max(0), h).into());
    TestStripSpots {
        bar,
        text,
        search,
        count,
    }
}

/// A bar counting `total` things in `bar`: each of `parts` fills its share in
/// its colour, from the left, one after the other; the rest is the track.
fn count_bar(
    c: &mut Canvas,
    bar: Rect,
    parts: &[(usize, Rgb)],
    total: usize,
    scale: f64,
    p: &Palette,
) {
    let (x, y, w, h) = rect_px(bar, scale);
    c.rounded(x, y, w, h, h / 2.0, p.border);
    if total == 0 {
        return;
    }
    // From the whole share down, each over the last: rounded ends overlap.
    let mut upto: usize = parts.iter().map(|(n, _)| n).sum::<usize>().min(total);
    for (n, tone) in parts.iter().rev() {
        if upto > 0 && *n > 0 {
            let f = upto as f32 / total as f32;
            c.rounded(x, y, (w * f).max(h), h, h / 2.0, *tone);
        }
        upto = upto.saturating_sub(*n);
    }
}

/// A part of a stage's page, under its title and what it says.
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
        /// The indices in `StagePage::buttons` of what acts on the agent
        /// (Show agent, Restart), drawn left of `picks`, which they go by.
        buttons: Vec<usize>,
        /// The agents to pick for the stage's next start, if it offers any.
        picks: Vec<AgentChip>,
        /// What the button picking its model says, after `picks`, where
        /// gerdr can start that agent on a model.
        model: Option<String>,
    },
    /// A stage's file (`brief.md`, `plan.md`): its name, how it stands,
    /// and the indices in `StagePage::buttons` of the buttons it holds
    /// (Review or Open, plain or primary), drawn at the card's right.
    File {
        name: String,
        state: CheckLine,
        buttons: Vec<usize>,
    },
    /// An AI review finding, with what can be done of it.
    Finding(FindingCard),
    /// A fix round of the stage, with what can be done of it.
    Round(RoundCard),
    /// Words, each with the indices in `StagePage::buttons` of the buttons
    /// after it, in a row at the right of the caption before it if they fit,
    /// else on a line of their own: Manual test's "Collapse" and "Expand".
    Toolbar {
        groups: Vec<(String, Vec<usize>)>,
    },
    /// A labelled list of lines in their tones: the checks, findings,
    /// criteria, the Ship check, a done stage's note.
    Lines {
        label: String,
        lines: Vec<CheckLine>,
    },
    /// The file named `name` to read, as its blocks, in the page's place
    /// for it: after all else, however long it is.
    Doc {
        name: String,
        blocks: Arc<Vec<DocBlock>>,
    },
    /// Build's choice: one agent through all milestones, or a fresh one
    /// per milestone (`fresh`).
    MilestoneMode { fresh: bool },
    /// Ship's choice and tick boxes for a repository, or what follows.
    ShipGroup(ShipGroup),
    /// A group of Build's list, its `label` ("Build", "Review 1") with "done
    /// of total" and a bar, how it stands if it says (`state`), and each by
    /// name when known (`true` = done; empty until the build log is read).
    /// Those in `running` blink: an agent is at work on them.
    Milestones {
        label: String,
        state: Option<CheckLine>,
        done: usize,
        total: usize,
        items: Vec<(bool, String)>,
        running: Vec<usize>,
        /// The page's buttons it holds, at its caption's right.
        buttons: Vec<usize>,
    },
}

/// A fix round as the page of the stage that asked lists it: how it stands,
/// who made its fixes and when, what it was asked, its fixes ticked as they
/// are made, and what they changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundCard {
    /// What it is called: "Manual test 2".
    pub label: String,
    /// How it stands, as a check's dot says it: blinking while its fixes are
    /// being made.
    pub dot: Dot,
    /// How it stands, in words and their tone: "fixes made".
    pub state: CheckLine,
    /// Who made the fixes and when, muted: "Claude Code · opus · sent 2h ago".
    pub meta: String,
    /// What it was asked to fix, each with how it stands now, under
    /// `asked_caption`: "Asked to fix", a review run's "Suggestions".
    pub asked: Vec<RoundAsked>,
    pub asked_caption: &'static str,
    /// Its fixes as the build log ticks them (`true` = made).
    pub fixes: Vec<(bool, String)>,
    /// Those an agent is at work on: they blink.
    pub running: Vec<usize>,
    /// What its fixes changed, a line per repository: "api · 3 files · +40
    /// −12".
    pub changed: Vec<String>,
    /// It shows all of that; folded, only its head and its buttons.
    pub open: bool,
    /// The indices in `StagePage::buttons` of the buttons it holds.
    pub buttons: Vec<usize>,
}

/// One thing a fix round was asked to fix, on its card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoundAsked {
    /// "AC-2".
    pub id: String,
    pub title: String,
    /// What went wrong, as the user said it then.
    pub note: String,
    /// How it stands now: "passed since", "fix made: try again".
    pub now: Option<CheckLine>,
}

/// What a finding's card says it is, by its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingTag {
    High,
    Medium,
    Low,
    /// The user added it.
    Yours,
    /// Verify's check, by how it stands.
    Passed,
    Failed,
    Running,
    /// Its turn in its folder hasn't come.
    Queued,
    Stopped,
    NotRun,
    Skipped,
}

impl FindingTag {
    /// The dot that says how a check stands, in place of a word. A finding
    /// has none: it has its word.
    pub fn dot(self) -> Option<Dot> {
        match self {
            Self::High | Self::Medium | Self::Low | Self::Yours => None,
            Self::Passed => Some(Dot::Done),
            Self::Failed => Some(Dot::Failed),
            Self::Running => Some(Dot::Working),
            Self::Stopped => Some(Dot::Blocked),
            Self::Queued | Self::NotRun => Some(Dot::Idle),
            Self::Skipped => Some(Dot::Unknown),
        }
    }
}

/// How a finding the user decided on is marked on its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStripe {
    /// It goes to Build with the next fixes.
    ToFix,
    /// It went to Build.
    Sent,
}

/// An AI review finding, a Verify check or a Manual test criterion, as its
/// stage's page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingCard {
    pub tag: FindingTag,
    pub title: String,
    /// Where it stands, once decided: "to fix".
    pub state: String,
    /// Its id, and where in the code it is.
    pub place: String,
    pub body: String,
    /// The lines of output it shows, a check's.
    pub preview: Vec<String>,
    /// The fix to make, labelled: the reviewer's or the user's.
    pub fix: String,
    /// A note on it, labelled: what went wrong, a criterion's.
    pub note: String,
    /// The user's comment on a finding, in a card of its own inside this one.
    pub comment: String,
    /// The indices in `StagePage::buttons` of the buttons that comment holds.
    pub comment_buttons: Vec<usize>,
    /// A finding whose fix the Build agent made: a dot says so, as a check's.
    pub made: bool,
    /// It no longer asks for anything: fixed or dismissed.
    pub faded: bool,
    /// It shows as one line, until selected: what it says and its first button.
    pub collapsed: bool,
    /// The mark down its left edge: what it has been decided to be.
    pub stripe: Option<CardStripe>,
    /// The note or comment being written on it.
    pub field: Option<Line>,
    /// What its field shows while empty.
    pub field_hint: &'static str,
    /// It has the keyboard.
    pub selected: bool,
    /// The indices in `StagePage::buttons` of the buttons it holds.
    pub buttons: Vec<usize>,
    /// A Manual test criterion's: drawn as a row of a sheet of text, not
    /// a card, `collapsed` folding it to its line.
    pub sheet: Option<Box<SheetRow>>,
}

/// What a criterion's row on Manual test's sheet has more than a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetRow {
    /// It is open to its steps, as the user left it: its chevron says so.
    /// The search may show it open without.
    pub open: bool,
    /// Its Given, When and Then, each word with its part; one part with no
    /// word when the criterion doesn't split.
    pub steps: Vec<(&'static str, String)>,
    /// The search hides it.
    pub hidden: bool,
    /// What the search looks for, as typed (see `crate::find`), tinted
    /// where it shows; empty for none.
    pub needle: String,
    /// It looks for `needle` only as a word of its own (vim's `*`).
    pub whole: bool,
}

/// A place in the text of the feature tab's page: which of its lines
/// (`FeatureTabLayout::texts`), and the char in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextSpot {
    pub line: usize,
    pub ch: usize,
}

/// Text of the page selected with the pointer: from where the press was
/// to where it went, and what those two lines said then, so a page that
/// changed under it drops it rather than mark other text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextPick {
    pub anchor: TextSpot,
    pub cursor: TextSpot,
    pub said: [String; 2],
}

impl TextPick {
    /// Its ends, first to last.
    pub fn ends(&self) -> (TextSpot, TextSpot) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }
}

/// What marks the page's text: the search's match gone to (by its index
/// in `FeatureTabLayout::matches`), and the text selected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PageMarks {
    pub found_at: Option<usize>,
    pub pick: Option<TextPick>,
}

/// A line of text on the page: its piece in the page's, the criterion's
/// card it is in (as `FeatureTabHit::Finding` counts them), and whether it
/// goes on the line before it, wrapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageText {
    pub piece: usize,
    pub card: Option<usize>,
    pub joins: bool,
}

/// A place the search found on the page: its line and chars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageMatch {
    pub line: usize,
    pub card: Option<usize>,
    pub range: std::ops::Range<usize>,
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
    /// (Abandon…, Remove worktrees…), after `FEATURE_BUTTONS`. Empty: the header
    /// has no buttons, as on a feature another machine writes.
    pub remove: &'static str,
    /// The nine stages, in order.
    pub rail: Vec<RailNode>,
    /// The line back to Build, while work sent back to it is under way.
    pub back: Option<RailBack>,
    /// The strip's chips, or with `expanded` the rows, all it could hold.
    pub repos: Vec<FeatureRepoRow>,
    /// Why the repositories couldn't be read, shown over them.
    pub repos_error: Option<String>,
    /// The repository with the keyboard while `focus` is `Repos`.
    pub repo: usize,
    pub focus: TabArea,
    /// The repositories show as rows in a card, not chips on the strip.
    pub expanded: bool,
    /// How many repositories the rows offer beyond the strip's chips.
    pub more: usize,
    /// The button the keyboard picked on the selected row.
    pub picked: Option<usize>,
    /// Groups of rows not read yet, or that couldn't be: `expanded` only.
    pub repo_notes: Vec<RepoNote>,
    pub page: StagePage,
    /// A feature's code review takes the tab under the rail: no strip,
    /// columns or footer of its own.
    pub review_page: bool,
    /// Code review's page goes between that review and its fixes.
    pub review_switch: Option<ReviewSwitch>,
    /// Manual test's page counts its criteria.
    pub test_strip: Option<TestStrip>,
    /// That review has the keys: the rail shows no focus.
    pub review_keys: bool,
    pub activity: Vec<ActivityRow>,
    /// How far each column is scrolled down, in logical pixels, indexed
    /// by `TabColumn`.
    pub scroll: [f64; 3],
    pub hover: Option<FeatureTabHit>,
    pub hints: Vec<(String, String)>,
    /// The search's match gone to, and the text selected on the page.
    pub marks: PageMarks,
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
    /// A row's button: the row, then which of its `buttons`.
    RepoButton(usize, usize),
    /// The chevron at the strip's end, folding and unfolding the rows.
    ToggleRepos,
    /// The × on a repository's row, as in `FeatureTabView::repos`.
    RepoRemove(usize),
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
    /// The button picking the agent's model.
    AgentModel,
    /// A segment of Build's milestone choice: 0 one agent, 1 one per
    /// milestone.
    MilestoneMode(usize),
    /// A segment of Code review's switch: 0 its code, 1 its fixes.
    ReviewView(usize),
    /// A button at the right of Code review's line, by its index in
    /// `ReviewSwitch::buttons`.
    ReviewButton(usize),
    /// A segment of a repository's way on the Ship page: its row, then 0
    /// pull request, 1 merge only, 2 nothing.
    ShipRoute(usize, usize),
    /// A row of the Ship page.
    ShipRow(usize),
    /// A finding's card, as `FeatureTabLayout::finding_cards` has them.
    Finding(usize),
    /// The chevron of a criterion's row, indexed as `Finding`.
    Fold(usize),
    /// A part of the criteria's search, on the line under the rail.
    Search(super::SearchPart),
    Hint(usize),
}

/// Where the feature tab's parts go, in output coordinates. What sits in
/// a column (`repos`, `page_buttons`, `pieces`) is placed as if it were
/// not scrolled: `FeatureTabView::scroll` moves it up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureTabLayout {
    /// With `FeatureTabView::review_page`: all under the rail's rule and the
    /// switch.
    pub review: Option<Rect>,
    /// With `FeatureTabView::review_switch`.
    pub review_switch: Option<ReviewSwitchSpots>,
    /// With `FeatureTabView::test_strip`, when there is no switch.
    pub test_strip: Option<TestStripSpots>,
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
    /// go at the end of the page), and Repos none until the rows unfold,
    /// when it goes across the tab over the others.
    pub columns: [Option<Rect>; 3],
    /// How tall each column's contents are, indexed by `TabColumn`: how
    /// far it may scroll is this less its place's height.
    pub content: [i32; 3],
    /// The line of the repositories under the rail.
    pub strip: Rect,
    /// Its label, naming the repositories.
    pub repos_label: Rect,
    /// The chevron at its end, with what is left to see before it.
    pub repos_toggle: Rect,
    /// Each repository's row (or chip, in `strip`, not scrolled), indexed
    /// as `FeatureTabView::repos`.
    pub repos: Vec<Rect>,
    /// Each row's buttons, as its `buttons` has them; empty for one with
    /// no room, for a chip.
    pub repo_buttons: Vec<Vec<Rect>>,
    /// Each repository's × (`FeatureRepoRow::remove`), as in `repos`.
    pub repo_removes: Vec<Option<Rect>>,
    /// On the strip, the chips standing for the repositories that don't
    /// fit, before the shown ones and after them: each chip, the
    /// repository a click selects (the nearest hidden one), and its label.
    pub more_repos: Vec<(Rect, usize, String)>,
    /// Each page button's place, indexed as `StagePage::buttons`.
    pub page_buttons: Vec<Option<Rect>>,
    /// The agent card's chips: their outline, and each chip as its `picks`
    /// has them.
    pub agent_picks: Option<(Rect, Vec<Rect>)>,
    /// The agent card's button picking its model.
    pub agent_model: Option<Rect>,
    /// Build's milestone choice: its outline and two segments, unscrolled.
    pub milestone_mode: Option<(Rect, [Rect; 2])>,
    /// The Ship page's rows.
    pub ship: ShipSpots,
    /// Each finding's card, in the page's order; the Ship page's rows too,
    /// which the keyboard goes through the same way.
    pub finding_cards: Vec<Rect>,
    /// The footer, and where each hint goes in it.
    pub hints: Rect,
    pub hint_spots: Vec<(usize, Rect)>,
    /// What each column paints, indexed by `TabColumn`.
    pub(super) pieces: [Vec<(Rect, Piece)>; 3],
    /// The page's lines of text, top to bottom: what can be selected.
    pub texts: Vec<PageText>,
    /// The places the search found on the page, top to bottom.
    pub matches: Vec<PageMatch>,
}

/// The note being written on a card of the tab in `frame`, where it shows,
/// as the pointer and the selection care: `None` while it is scrolled away.
pub fn feature_tab_field(view: &FeatureTabView, frame: Rect, scale: f64) -> Option<FieldSpot> {
    let layout = feature_tab_layout(view, frame);
    // The criteria's search, while it has the keys.
    let search = (view.test_strip.as_ref()).and_then(|s| s.search.as_ref());
    if let Some(bar) = search.filter(|b| b.focused) {
        let spots = layout.test_strip.as_ref()?;
        let (_, area) = (spots.search.iter()).find(|(p, _)| *p == super::SearchPart::Field)?;
        return Some(FieldSpot {
            area: *area,
            text: super::search_field_text(*area, scale),
            size: FONT_SIZE,
            line: bar.query.clone(),
        });
    }
    let col = TabColumn::Page as usize;
    let (rect, k) = layout.pieces[col]
        .iter()
        .find_map(|(rect, piece)| match piece {
            Piece::CardField(k) => Some((*rect, *k)),
            _ => None,
        })?;
    let Some(PageSection::Finding(FindingCard {
        field: Some(line), ..
    })) = view.page.sections.get(k)
    else {
        return None;
    };
    let down = view.scroll[col].round() as i32;
    let area = Rectangle::new((rect.loc.x, rect.loc.y - down).into(), rect.size);
    let place = layout.columns[col]?;
    if area.loc.y < place.loc.y || area.loc.y + area.size.h > place.loc.y + place.size.h {
        return None;
    }
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: line.clone(),
    })
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
        if let Some(spots) = &self.test_strip
            && let Some((part, _)) = spots.search.iter().find(|(_, r)| on(r))
        {
            return Some(FeatureTabHit::Search(*part));
        }
        if let Some(spots) = &self.review_switch {
            if let Some(i) = spots.segments.iter().position(on) {
                return Some(FeatureTabHit::ReviewView(i));
            }
            if let Some(i) = (spots.buttons.iter()).position(|b| b.as_ref().is_some_and(on)) {
                return Some(FeatureTabHit::ReviewButton(i));
            }
        }
        if on(&self.repos_toggle) {
            return Some(FeatureTabHit::ToggleRepos);
        }
        if on(&self.strip) {
            if let Some((_, i, _)) = self.more_repos.iter().find(|(r, ..)| on(r)) {
                return Some(FeatureTabHit::MoreRepos(*i));
            }
            let chip = (!view.expanded).then(|| self.repo_at(pos));
            return chip.flatten().or(Some(FeatureTabHit::Tab));
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
                    .or_else(|| self.agent_pick_at(view, at))
                    .or_else(|| {
                        (self.agent_model?.contains(at)).then_some(FeatureTabHit::AgentModel)
                    })
                    .or_else(|| {
                        let (_, segments) = self.milestone_mode.as_ref()?;
                        let i = segments.iter().position(|r| r.contains(at))?;
                        Some(FeatureTabHit::MilestoneMode(i))
                    })
                    .or_else(|| {
                        (self.ship.routes.iter()).find_map(|(row, _, segments)| {
                            let i = segments.iter().position(|r| r.contains(at))?;
                            Some(FeatureTabHit::ShipRoute(*row, i))
                        })
                    })
                    .or_else(|| {
                        let on = (self.ship.rows.iter()).find(|(_, r)| r.contains(at))?;
                        Some(FeatureTabHit::ShipRow(on.0))
                    })
                    .or_else(|| {
                        let k = self.pieces[TabColumn::Page as usize].iter().find_map(
                            |(r, piece)| match piece {
                                Piece::Fold(k) if r.contains(at) => Some(*k),
                                _ => None,
                            },
                        )?;
                        let card = (view.page.sections[..k].iter())
                            .filter(|s| matches!(s, PageSection::Finding(_)))
                            .count();
                        Some(FeatureTabHit::Fold(card))
                    })
                    .or_else(|| {
                        let on = |r: &Rect| r.contains(at);
                        self.finding_cards
                            .iter()
                            .position(on)
                            .map(FeatureTabHit::Finding)
                    }),
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
        for (i, row) in self.repos.iter().enumerate() {
            if (self.repo_removes.get(i).copied().flatten()).is_some_and(|b| b.contains(at)) {
                return Some(FeatureTabHit::RepoRemove(i));
            }
            let buttons = self.repo_buttons.get(i).map_or(&[][..], Vec::as_slice);
            if let Some(b) = buttons.iter().position(|b| b.contains(at)) {
                return Some(FeatureTabHit::RepoButton(i, b));
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
    /// caption shows too, and its end for the last). `None` while folded:
    /// the strip of chips shows the selected one by itself.
    pub fn reveal_repo(&self, i: usize, down: f64) -> Option<f64> {
        let place = self.columns[TabColumn::Repos as usize]?;
        let row = self.repos.get(i)?;
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

    /// The line of text on the page at `pos`, and the char in it nearest
    /// the pointer: on the line under it, nearest it across. With `near`,
    /// also off the text or the page: the line nearest, as a drag goes on
    /// past the page's edge.
    pub fn text_at(
        &self,
        view: &FeatureTabView,
        pos: Point<i32, Logical>,
        near: bool,
    ) -> Option<TextSpot> {
        let col = TabColumn::Page as usize;
        let place = self.columns[col]?;
        if !near && !place.contains(pos) {
            return None;
        }
        let down = view.scroll[col].round() as i32;
        let at: Point<i32, Logical> = Point::from((pos.x, pos.y + down));
        let pieces = &self.pieces[col];
        let rect = |t: &PageText| pieces[t.piece].0;
        let apart = |t: &PageText| {
            let r = rect(t);
            let dy = (r.loc.y - at.y).max(at.y - (r.loc.y + r.size.h - 1)).max(0);
            let (text, font, bold) = piece_text(&pieces[t.piece].1).unwrap_or(("", Font::Body, false));
            let end = r.loc.x + text_width_of(text, font, bold).ceil() as i32;
            let dx = (r.loc.x - at.x).max(at.x - end).max(0);
            (dy, dx)
        };
        let (line, t) = (self.texts.iter().enumerate()).min_by_key(|(_, t)| apart(t))?;
        if !near && apart(t) != (0, 0) {
            return None;
        }
        let (r, (text, font, bold)) = (rect(t), piece_text(&pieces[t.piece].1)?);
        let ch = char_at_x(text, font, bold, (at.x - r.loc.x) as f32);
        Some(TextSpot { line, ch })
    }

    /// What line `line` of the page's text says.
    pub fn line_text(&self, line: usize) -> Option<&str> {
        let t = self.texts.get(line)?;
        piece_text(&self.pieces[TabColumn::Page as usize].get(t.piece)?.1).map(|(text, ..)| text)
    }

    /// The text `view` has selected, as it goes on the clipboard: lines
    /// side by side or wrapped joined by a space, others by a new line.
    /// `None` with none, or once the page changed under it.
    pub fn picked_text(&self, view: &FeatureTabView) -> Option<String> {
        let pick = view.marks.pick.as_ref()?;
        let says = |spot: TextSpot, text: &str| self.line_text(spot.line) == Some(text);
        if !says(pick.anchor, &pick.said[0]) || !says(pick.cursor, &pick.said[1]) {
            return None;
        }
        let (from, to) = pick.ends();
        let pieces = &self.pieces[TabColumn::Page as usize];
        let mut out = String::new();
        for line in from.line..=to.line {
            let text = self.line_text(line)?;
            let n = text.chars().count();
            let (start, end) = match (line == from.line, line == to.line) {
                (true, true) => (from.ch, to.ch),
                (true, false) => (from.ch, n),
                (false, true) => (0, to.ch),
                (false, false) => (0, n),
            };
            if line > from.line {
                let (before, now) = (&self.texts[line - 1], &self.texts[line]);
                let beside = pieces[before.piece].0.loc.y == pieces[now.piece].0.loc.y;
                out.push(if beside || now.joins { ' ' } else { '\n' });
            }
            out.extend(text.chars().skip(start).take(end.saturating_sub(start)));
        }
        (!out.is_empty()).then_some(out)
    }

    /// How far down the page to scroll, from `down`, to show line `line`
    /// of its text.
    pub fn reveal_line(&self, line: usize, down: f64) -> Option<f64> {
        let place = self.columns[TabColumn::Page as usize]?;
        let t = self.texts.get(line)?;
        let r = self.pieces[TabColumn::Page as usize].get(t.piece)?.0;
        let top = f64::from(r.loc.y - place.loc.y) - f64::from(CARD_PAD);
        let bottom = f64::from(r.loc.y - place.loc.y + r.size.h) + f64::from(CARD_PAD);
        let view = f64::from(place.size.h);
        let room = f64::from(self.scroll_room(TabColumn::Page));
        let to = if top < down {
            top
        } else if bottom > down + view {
            bottom - view
        } else {
            return Some(down);
        };
        Some(to.clamp(0.0, room))
    }

    /// How far down the page to scroll, from `down`, to show finding `i`:
    /// its top first when it is taller than the page shows.
    pub fn reveal_finding(&self, i: usize, down: f64) -> Option<f64> {
        let place = self.columns[TabColumn::Page as usize]?;
        let card = self.finding_cards.get(i)?;
        let top = f64::from(card.loc.y - place.loc.y) - f64::from(CARD_PAD);
        let bottom = f64::from(card.loc.y - place.loc.y + card.size.h) + f64::from(CARD_PAD);
        let view = f64::from(place.size.h);
        let room = f64::from(self.scroll_room(TabColumn::Page));
        let to = if top < down || bottom - top > view {
            top
        } else if bottom > down + view {
            bottom - view
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
/// The room between a done stage's agent logo and its model on the rail.
const RAIL_AGENT_GAP: i32 = 4;
/// The room left between a node and the line joining it to the next.
const CONNECTOR_GAP: i32 = 6;
/// The room kept over the rail for the line back to Build, there or not, so
/// the columns stay put when it comes.
const RAIL_BACK: i32 = 8;
/// How far over its nodes that line runs, how far from them it stops, the
/// radius of its corners and the size of its arrowhead.
const RAIL_BACK_RISE: i32 = 15;
const RAIL_BACK_CLEAR: i32 = 5;
const RAIL_BACK_CORNER: i32 = 5;
const RAIL_BACK_ARROW: i32 = 4;
/// How wide the activity's column is: the page takes the rest.
pub const ACTIVITY_W: i32 = 240;
/// Narrower than this, the activity has no column: its rows go at the
/// end of the page.
pub const ACTIVITY_BELOW: i32 = 1180;
/// The gap between the columns' cards, the room inside them, and their
/// corners.
const COLUMN_GAP: i32 = 12;
/// The height of the switch on Code review's page, and the gaps round it.
const REVIEW_SWITCH_H: i32 = 30;
const REVIEW_SWITCH_GAP: i32 = 8;
/// The size of its bar of files viewed, and the least room left for its words.
const REVIEW_BAR_W: i32 = 120;
const REVIEW_BAR_H: i32 = 8;
const REVIEW_LEFT_MIN: i32 = 60;
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
/// A repository's row on the unfolded card, and the gap between two.
const REPO_ROW: i32 = 28;
const REPO_GAP: i32 = 4;
/// The room above a group of rows' heading, past the first.
const REPO_GROUP_GAP: i32 = 6;
/// The room after the strip's label.
const REPOS_LABEL_GAP: i32 = 10;
/// What the strip's label says.
const REPOS_LABEL: &str = "Repos";
/// How wide the chevron is on the strip's toggle, beside its text.
const TOGGLE_ICON: i32 = 22;
/// At least this many rows show when unfolded, however short the tab,
/// else the card takes at most `REPOS_CARD_SHARE` percent of the body.
const REPOS_CARD_ROWS: i32 = 3;
const REPOS_CARD_SHARE: i32 = 40;
/// A row's second part shows only with this much room.
const REPO_LINE_MIN: i32 = 40;
/// Room for a dot left of a row's text.
const DOT_ROOM: i32 = 18;
/// A line of small text, of body text, and of an agent's pane.
const SMALL_LINE: i32 = 20;
const BODY_LINE: i32 = 22;
/// How high a stage page's title is.
const TITLE_LINE: i32 = BODY_LINE + 6;
const TAIL_LINE: i32 = 18;
/// The gap between the parts of a page.
const PAGE_GAP: i32 = 16;
/// An agent card's inner room and its header's height.
const AGENT_PAD: i32 = 10;
/// The room between the parts of a finding's card.
const FINDING_GAP: i32 = 6;
/// The room inside the card of the user's comment on a finding.
const COMMENT_PAD: i32 = 8;
/// How much of the accent tints that card, as a comment's card in a diff.
const COMMENT_TINT: f32 = 0.1;
/// How far a check card's text stands right of its dot's left edge.
const CHECK_DOT_ROOM: i32 = DOT + 8;
/// How far a round card's lines under one thing it was asked go in.
const ROUND_INDENT: i32 = 12;
/// The room after each thing a round card says it was asked.
const ROUND_ITEM_GAP: i32 = 4;
/// A sheet row's room above and below its text, its chevron's width, the
/// least its title gets beside its buttons, and the gap after the words of
/// its steps.
const SHEET_PAD: i32 = 6;
const SHEET_FOLD: i32 = 18;
const SHEET_TITLE_MIN: i32 = 120;
const SHEET_WORD_GAP: i32 = 10;
/// Between a toolbar's buttons, and before its next word.
const SHEET_BUTTON_GAP: i32 = 2;
const TOOLBAR_GAP: i32 = 14;
const TOOLBAR_PADDING: i32 = 10;
/// How much of the accent the ground of selected text on the page has,
/// as the scrollback viewer's selection.
const PICKED_TINT: f32 = 0.35;

/// Where `text` says `needle` (see `crate::find`), by char, for
/// `found_tints`.
fn found_ranges(text: &str, needle: &str, whole: bool) -> Vec<(std::ops::Range<usize>, bool)> {
    (crate::find::find(text, needle, whole).into_iter())
        .map(|r| (r, false))
        .collect()
}

/// A line of `text`, tinted where it says what `sheet` looks for.
fn found_piece(text: String, font: Font, ink: Ink, bold: bool, sheet: &SheetRow) -> Piece {
    let says = !sheet.needle.is_empty()
        && !crate::find::find(&text, &sheet.needle, sheet.whole).is_empty();
    match says {
        true => Piece::Found {
            text,
            font,
            ink,
            bold,
            needle: sheet.needle.clone(),
            whole: sheet.whole,
            current: None,
        },
        false => Piece::Text {
            text,
            font,
            ink,
            bold,
        },
    }
}

/// How wide toolbar `groups` run: their words and buttons, as `Stack::toolbar`
/// lays them.
fn toolbar_width(groups: &[(String, Vec<usize>)], page: &StagePage) -> i32 {
    let mut w = 0;
    for (word, held) in groups {
        w += width_of(word, Font::Small.size()).ceil() as i32 + 1 + TOOLBAR_GAP;
        for &b in held.iter().filter(|b| **b < page.buttons.len()) {
            w += toolbar_button_width(&page.buttons[b].label) + SHEET_BUTTON_GAP;
        }
    }
    w
}

/// A toolbar button's width: its label and a little room, not a full
/// button's least width, so a row of them stays short.
fn toolbar_button_width(label: &str) -> i32 {
    width_of(label, FONT_SIZE).ceil() as i32 + 2 * TOOLBAR_PADDING
}
const AGENT_HEAD: i32 = 28;
/// Where an agent card's name starts, past its logo and dot.
const AGENT_TEXT_AT: i32 = 36;
/// The room an agent card keeps for its status beside its buttons.
const AGENT_STATUS_MIN: i32 = 96;
/// Between an agent card's head and the rows of buttons that left it.
const AGENT_ROWS_GAP: i32 = 4;
/// An agent card's chips: the outline's height, as its button's, the room
/// between it and its chips, a chip's width, a logo's size, and the label
/// in front of them on a card with no head.
const PICK_TRACK: i32 = ROW_BUTTON;
const PICK_TRACK_PAD: i32 = 2;
const PICK_CHIP: i32 = 28;
const PICK_ICON: i32 = 14;
/// How far a chip that can't be picked is from the outline's ground to
/// `placeholder`: a logo plainly dimmer than one that can be.
const PICK_OFF: f32 = 0.5;
const PICK_LABEL: i32 = 48;
/// The widest the button picking an agent's model gets.
const AGENT_MODEL_MAX: i32 = 220;
/// Build's milestone choice: its segments' words.
const MODE_SEGMENTS: [&str; 2] = ["One agent", "Agent per milestone"];
/// The ways a repository can go on the Ship page, in `Route::ALL`'s order.
const SHIP_ROUTES: [&str; 3] = ["Pull request", "Merge only", "Nothing"];
/// How far a Ship page tick box sits in from its row's left edge.
const SHIP_STEP_INSET: i32 = 4;
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
    Warning,
    Accent,
    Tone(CheckTone),
}

impl Ink {
    fn color(self, p: &Palette) -> Rgb {
        match self {
            Self::Text => p.text,
            Self::Muted => p.muted,
            Self::Error => p.error,
            Self::Warning => p.warning,
            Self::Accent => p.accent,
            Self::Tone(tone) => tone.color(p),
        }
    }
}

/// A thing a feature tab's column paints, in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Piece {
    /// A section's uppercase label.
    Caption(String),
    /// A list of milestones' or fixes' uppercase label, with the buttons of
    /// its section `k` at its right.
    GroupCaption(usize, String),
    Text {
        text: String,
        font: Font,
        ink: Ink,
        bold: bool,
    },
    /// The milestones' progress.
    Bar { done: usize, total: usize },
    /// A milestone: done, the next one, or one still to come. One `running`
    /// blinks (see `feature_tab_blinkers`).
    Milestone {
        done: bool,
        next: bool,
        running: bool,
        text: String,
    },
    /// The agent card of `StagePage::sections[i]`.
    Agent(usize),
    /// Build's milestone choice, its label at the row's left.
    MilestoneMode { fresh: bool },
    /// The choice of the Ship group of `StagePage::sections[i]`.
    ShipRoute(usize),
    /// Tick box `.1` of the Ship group of `StagePage::sections[.0]`.
    ShipStep(usize, usize),
    /// The file card of `StagePage::sections[i]`.
    File(usize),
    /// The frame of the finding card of `StagePage::sections[i]`, with its
    /// buttons; its text goes in pieces of its own. A sheet's row has no
    /// frame: its ground when selected or hovered, and a rule under it.
    FindingCard(usize),
    /// The frame of the fix round card of `StagePage::sections[i]`, with
    /// its buttons; its text goes in pieces of its own.
    RoundCard(usize),
    /// The chevron folding or opening the sheet row of
    /// `StagePage::sections[i]`.
    Fold(usize),
    /// A line of text, the places saying `needle` (see `crate::find`)
    /// tinted, the `current`th of them more: the match gone to.
    Found {
        text: String,
        font: Font,
        ink: Ink,
        bold: bool,
        needle: String,
        whole: bool,
        current: Option<usize>,
    },
    /// The ground of the chars `range` of the line of text after it,
    /// selected: `text` in `font`, bold or not, as that line has it.
    Picked {
        text: String,
        font: Font,
        bold: bool,
        range: std::ops::Range<usize>,
    },
    /// The card of the user's comment on a finding, behind its text.
    YourComment,
    /// The note being written on the card of `StagePage::sections[i]`.
    CardField(usize),
    /// How a check stands, on its card. It blinks when at work: see
    /// `feature_tab_blinkers`.
    CheckDot(Dot),
    /// `FeatureTabView::repos[i]`.
    Repo(usize),
    /// `StagePage::buttons[i]`, one acting on the stage as a whole.
    Button(usize),
    /// `FeatureTabView::activity[row]`, its `what` wrapped in `lines`.
    Activity { row: usize, lines: Vec<String> },
    /// What a file's list item starts with, right in its gutter.
    ItemMark(ItemMark),
    /// A line of a file's code block, cut where the column ends, on the
    /// block's ground; an empty one is the block's edge.
    CodeLine(String),
    /// A line of a check's output on its card: a code block's, on a ground
    /// that shows on the card in either theme.
    OutputLine(String),
    /// The bar beside a file's quote.
    QuoteBar,
    /// A file's thematic break.
    Rule,
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

thread_local! {
    /// What `wrapped` measured, by a hash of what it was asked: the tab is
    /// laid out every frame, and a page of findings has many long texts.
    static WRAP_MEMO: RefCell<HashMap<u64, Rc<Vec<String>>>> = RefCell::new(HashMap::new());
}

/// How many texts `WRAP_MEMO` holds before it starts again.
const WRAP_MEMOS: usize = 2048;

/// `wrap_all` of `text`, kept: a page of findings has many long texts.
fn wrapped(text: &str, size: f32, width: i32) -> Rc<Vec<String>> {
    let mut hasher = DefaultHasher::new();
    (text, size.to_bits(), width).hash(&mut hasher);
    let key = hasher.finish();
    WRAP_MEMO.with_borrow_mut(|memo| {
        if let Some(lines) = memo.get(&key) {
            return lines.clone();
        }
        if memo.len() >= WRAP_MEMOS {
            memo.clear();
        }
        let lines = Rc::new(wrap_all(text, size, width));
        memo.insert(key, lines.clone());
        lines
    })
}

/// `text` cut to fit `width` at `size`, with an ellipsis if it had to be.
fn cut_to_width(text: &str, size: f32, width: i32) -> String {
    let width = width as f32;
    if width_of(text, size) <= width {
        return text.to_owned();
    }
    let mut cut: String = text.to_owned();
    while !cut.is_empty() {
        cut.pop();
        let trimmed = cut.trim_end();
        if width_of(&format!("{trimmed}\u{2026}"), size) <= width {
            return format!("{trimmed}\u{2026}");
        }
    }
    "\u{2026}".to_owned()
}

/// What `wrap_to` gave lately, by its text, size, width and line limit.
type WrapKey = (String, u32, i32, usize);

/// How many wraps `WRAPS` keeps before it starts over: a tab's worth, many
/// times over.
const WRAPS_MAX: usize = 8192;

thread_local! {
    /// The tab is laid out every frame and for every pointer move, and
    /// measuring a line word by word is most of that: a feature's long
    /// activity took 8 ms a layout, and the pointer lagged seconds behind.
    static WRAPS: RefCell<HashMap<WrapKey, Vec<String>>> = RefCell::new(HashMap::new());
}

/// `wrap` to at most `max` lines.
fn wrap_to(text: &str, size: f32, width: i32, max: usize) -> Vec<String> {
    let key = (text.to_owned(), size.to_bits(), width, max);
    if let Some(lines) = WRAPS.with_borrow(|w| w.get(&key).cloned()) {
        return lines;
    }
    let lines = wrap_measured(text, size, width, max);
    WRAPS.with_borrow_mut(|w| {
        if w.len() >= WRAPS_MAX {
            w.clear();
        }
        w.insert(key, lines.clone());
    });
    lines
}

/// `wrap_to`, measuring.
fn wrap_measured(text: &str, size: f32, width: i32, max: usize) -> Vec<String> {
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

/// `text` in full, in lines no wider than `width` at `size`: its own line
/// breaks kept, and what is too long for a line (a long word, a bracket
/// group that never closes) broken where it must be, so that no part of it
/// is cut when painted.
pub(super) fn wrap_all(text: &str, size: f32, width: i32) -> Vec<String> {
    (text.lines())
        .filter(|para| !para.trim().is_empty())
        .flat_map(|para| wrap_to(para, size, width, usize::MAX))
        .flat_map(|line| break_long(line, size, width))
        .collect()
}

/// `line` broken to fit `width` at `size`: at its last space that fits,
/// else inside its word.
fn break_long(line: String, size: f32, width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    if width_of(&line, size) <= width as f32 {
        lines.push(line);
        return lines;
    }
    let mut now = String::new();
    for ch in line.chars() {
        now.push(ch);
        if now.chars().count() > 1 && width_of(&now, size) > width as f32 {
            let at = now.trim_end().rfind(' ').filter(|at| *at > 0);
            let cut = at.unwrap_or(now.len() - ch.len_utf8());
            let rest = now.split_off(cut);
            lines.push(std::mem::replace(&mut now, rest.trim_start().to_owned()));
        }
    }
    if !now.trim().is_empty() {
        lines.push(now);
    }
    lines
}

/// The room left between a repository row's name and its stat, between
/// the stat's counts, and the least of the name the stat leaves showing.
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

/// A file's blocks on the page: the room above a block, its list items'
/// indent and gutter, a quote's indent and bar, a code block's inner room,
/// and how far in lists stop indenting more.
const DOC_GAP: i32 = 8;
const DOC_HEADING_GAP: i32 = 14;
const DOC_ITEM_GAP: i32 = 2;
const DOC_INDENT: i32 = 16;
const DOC_MARK: i32 = 20;
const DOC_QUOTE: i32 = 12;
const DOC_QUOTE_BAR: i32 = 3;
const DOC_CODE_PAD: i32 = 8;
/// How far the ground of a check's output goes from the card's towards its
/// text.
const OUTPUT_GROUND: f32 = 0.06;
const DOC_DEPTH_MAX: u8 = 4;
const DOC_RULE: i32 = 13;
/// How far off a line's width added up by word may be from it measured
/// whole, in logical pixels: kerning and rounding at each space.
const DOC_WIDTH_SLACK: f32 = 8.0;

/// `text` broken into lines no wider than `width` by `measure`, all of it
/// kept: a word wider than a line (a URL) breaks where it must. A long file
/// has many words: a line's width is added up word by word, and measured
/// whole only when that comes within `DOC_WIDTH_SLACK` of the edge.
fn doc_lines(text: &str, width: i32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let width = width as f32;
    let space = measure("a a") - 2.0 * measure("a");
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0.0;
    for word in text.split_whitespace() {
        let wide = measure(word);
        if !line.is_empty() {
            let about = used + space + wide;
            if about < width - DOC_WIDTH_SLACK {
                line.push(' ');
                line.push_str(word);
                used = about;
                continue;
            }
            if about <= width + DOC_WIDTH_SLACK {
                let longer = format!("{line} {word}");
                let whole = measure(&longer);
                if whole <= width {
                    line = longer;
                    used = whole;
                    continue;
                }
            }
            lines.push(std::mem::take(&mut line));
        }
        if wide <= width {
            line = word.to_owned();
            used = wide;
            continue;
        }
        let mut rest = word;
        while measure(rest) > width {
            let ends: Vec<usize> = rest.char_indices().skip(1).map(|(i, _)| i).collect();
            let fit = ends.partition_point(|end| measure(&rest[..*end]) <= width);
            let Some(end) = fit
                .checked_sub(1)
                .map(|i| ends[i])
                .or(ends.first().copied())
            else {
                break;
            };
            lines.push(rest[..end].to_owned());
            rest = &rest[end..];
        }
        line = rest.to_owned();
        used = measure(rest);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// What a number mark says.
fn number_mark(n: usize) -> String {
    format!("{n}.")
}

/// A file's blocks laid out for a column `w` wide, from `(0, 0)`.
fn doc_stack(blocks: &[DocBlock], w: i32) -> Stack {
    let mut s = Stack {
        x: 0,
        w,
        y: 0,
        pieces: Vec::new(),
        joins: Vec::new(),
    };
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            s.y += match block {
                DocBlock::Heading { .. } => DOC_HEADING_GAP,
                DocBlock::Item {
                    mark: ItemMark::Under,
                    ..
                } => DOC_GAP / 2,
                DocBlock::Item { .. } if matches!(blocks[i - 1], DocBlock::Item { .. }) => {
                    DOC_ITEM_GAP
                }
                _ => DOC_GAP,
            };
        }
        s.doc_block(block);
    }
    s
}

/// The latest file laid out, with the width: laying one out measures its
/// every word, and the tab is laid out every frame. It holds on to what it
/// was laid out from, so that can't be another's at the same address.
struct DocMemo {
    blocks: Arc<Vec<DocBlock>>,
    w: i32,
    pieces: Rc<Vec<(Rect, Piece)>>,
    h: i32,
}

/// How many files `DOC_MEMO` holds: the stages looked between.
const DOC_MEMOS: usize = 2;

thread_local! {
    static DOC_MEMO: RefCell<Vec<DocMemo>> = const { RefCell::new(Vec::new()) };
}

/// `blocks` laid out for a column `w` wide: the pieces from `(0, 0)`, and
/// how high they all are.
fn doc_laid_out(blocks: &Arc<Vec<DocBlock>>, w: i32) -> (Rc<Vec<(Rect, Piece)>>, i32) {
    DOC_MEMO.with_borrow_mut(|memo| {
        let found = (memo.iter()).position(|m| m.w == w && Arc::ptr_eq(&m.blocks, blocks));
        let entry = match found {
            Some(i) => memo.remove(i),
            None => {
                let s = doc_stack(blocks, w);
                DocMemo {
                    blocks: blocks.clone(),
                    w,
                    h: s.y,
                    pieces: Rc::new(s.pieces),
                }
            }
        };
        let laid = (entry.pieces.clone(), entry.h);
        memo.insert(0, entry);
        memo.truncate(DOC_MEMOS);
        laid
    })
}

/// A column's contents, stacked from its top.
struct Stack {
    x: i32,
    w: i32,
    y: i32,
    pieces: Vec<(Rect, Piece)>,
    /// The pieces that are lines going on the line of text before them,
    /// wrapped: copied, a space joins them, not a new line.
    joins: Vec<usize>,
}

impl Stack {
    /// Note the next piece as going on the line before it.
    fn join_next(&mut self, first: bool) {
        if !first {
            self.joins.push(self.pieces.len());
        }
    }

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
        for (i, line) in wrap(text, font.size(), self.w).into_iter().enumerate() {
            self.join_next(i == 0);
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

    /// `text` wrapped to the column, all of it, a line `h` high each.
    fn all_text(&mut self, text: &str, font: Font, ink: Ink, bold: bool, h: i32) {
        // Bold runs wider than `wrapped` measures.
        let w = if bold { self.w - self.w / 16 } else { self.w };
        for (i, line) in wrapped(text, font.size(), w).iter().enumerate() {
            self.join_next(i == 0);
            self.put(
                h,
                Piece::Text {
                    text: line.clone(),
                    font,
                    ink,
                    bold,
                },
            );
        }
    }

    /// Finding card `k`: its frame, its text, and `buttons`' places in it.
    fn finding(
        &mut self,
        k: usize,
        card: &FindingCard,
        page: &StagePage,
        buttons: &mut [Option<Rect>],
    ) -> Rect {
        if let Some(sheet) = &card.sheet {
            return self.sheet_row(k, card, sheet, page, buttons);
        }
        let frame_at = self.pieces.len();
        let (x, w, top) = (self.x, self.w, self.y);
        self.x += AGENT_PAD;
        self.w = (w - 2 * AGENT_PAD).max(1);
        self.y += AGENT_PAD;
        if card.collapsed {
            self.collapsed_card(card, page, buttons);
            self.y += AGENT_PAD;
            (self.x, self.w) = (x, w);
            let frame = Rectangle::new((x, top).into(), (w, self.y - top).into());
            self.pieces.insert(frame_at, (frame, Piece::FindingCard(k)));
            return frame;
        }
        let body = if card.faded { Ink::Muted } else { Ink::Text };
        let (left, width) = (self.x, self.w);
        // A check's dot says how it stands; its text goes right of it.
        if let Some(dot) = card.tag.dot().or(card.made.then_some(Dot::Done)) {
            self.pieces.push((
                Rectangle::new(
                    (self.x, self.y + (BODY_LINE - DOT) / 2).into(),
                    (DOT, DOT).into(),
                ),
                Piece::CheckDot(dot),
            ));
            self.x = left + CHECK_DOT_ROOM;
            self.w = (width - CHECK_DOT_ROOM).max(1);
        }
        self.all_text(&card.title, Font::Body, body, true, BODY_LINE);
        // A finding's tag in its colour, then the rest of the line muted.
        let tag = match card.tag {
            FindingTag::High => Some(("high", Ink::Error)),
            FindingTag::Medium => Some(("medium", Ink::Warning)),
            FindingTag::Low => Some(("low", Ink::Muted)),
            FindingTag::Yours => Some(("yours", Ink::Accent)),
            _ => None,
        };
        // What the user decided stands out from the rest of the line.
        let marked = card.stripe == Some(CardStripe::ToFix) && !card.state.is_empty();
        let mut rest = Vec::new();
        rest.extend((!card.state.is_empty() && !marked).then(|| card.state.clone()));
        rest.push(card.place.clone());
        let mut rest = rest.join(" \u{b7} ");
        let word = |this: &mut Self, text: &str, ink: Ink, bold: bool| {
            let w = width_of(text, Font::Small.size()).ceil() as i32 + 1;
            // Bold runs wider than it measures.
            let w = if bold { w * 9 / 8 } else { w };
            this.pieces.push((
                Rectangle::new((this.x, this.y).into(), (w, TAIL_LINE).into()),
                Piece::Text {
                    text: text.to_owned(),
                    font: Font::Small,
                    ink,
                    bold,
                },
            ));
            // Past the dot too, if it has one.
            this.x += w;
            this.w = (this.w - w).max(1);
        };
        if let Some((tag, ink)) = tag {
            let ink = if card.faded { Ink::Muted } else { ink };
            word(self, tag, ink, false);
            rest = format!("\u{b7} {rest}");
        }
        if marked {
            let text = match tag {
                Some(_) => format!("\u{b7} {}", card.state),
                None => card.state.clone(),
            };
            word(self, &text, Ink::Accent, true);
            rest = match tag {
                Some(_) => rest,
                None => format!("\u{b7} {rest}"),
            };
        }
        self.all_text(&rest, Font::Small, Ink::Muted, false, TAIL_LINE);
        (self.x, self.w) = (left, width);
        if !card.preview.is_empty() {
            self.y += FINDING_GAP;
            let line = |s: &mut Self, h: i32, text: &str| {
                let rect = Rectangle::new((s.x, s.y).into(), (s.w, h).into());
                s.pieces.push((rect, Piece::OutputLine(text.to_owned())));
                s.y += h;
            };
            line(self, DOC_CODE_PAD / 2, "");
            for text in &card.preview {
                line(self, TAIL_LINE, text);
            }
            line(self, DOC_CODE_PAD / 2, "");
        }
        for text in [&card.body, &card.fix, &card.note] {
            if !text.is_empty() {
                self.y += FINDING_GAP;
                self.all_text(text, Font::Body, body, false, BODY_LINE);
            }
        }
        if !card.comment.is_empty() {
            self.y += FINDING_GAP;
            self.your_comment(card, page, buttons);
        }
        if let Some(field) = &card.field {
            self.y += FINDING_GAP;
            let h = field_lines_height(field.text.split('\n').count());
            let rect = Rectangle::new((self.x, self.y).into(), (self.w, h).into());
            self.pieces.push((rect, Piece::CardField(k)));
            self.y += h;
        }
        let held: Vec<usize> = (card.buttons.iter().copied())
            .filter(|b| *b < page.buttons.len())
            .collect();
        if !held.is_empty() {
            self.y += FINDING_GAP;
            let widths: Vec<i32> = (held.iter())
                .map(|b| button_width(&page.buttons[*b].label))
                .collect();
            let placed = rows_from_right(&widths, self.x, self.x + self.w);
            for (b, (bx, row)) in held.iter().zip(&placed) {
                let by = self.y + row * (ROW_BUTTON + QUIT_BUTTON_GAP);
                let w = button_width(&page.buttons[*b].label);
                buttons[*b] = Some(Rectangle::new((*bx, by).into(), (w, ROW_BUTTON).into()));
            }
            self.y += rows_height(&placed);
        }
        self.y += AGENT_PAD;
        (self.x, self.w) = (x, w);
        let frame = Rectangle::new((x, top).into(), (w, self.y - top).into());
        self.pieces.insert(frame_at, (frame, Piece::FindingCard(k)));
        frame
    }

    /// Fix round card `k`: its frame, its text, and `buttons`' places in it.
    /// Its head says which round it is and how it stands, then who made the
    /// fixes and when; open, what it was asked, its fixes with a bar, and what
    /// they changed; its buttons last, from the right.
    fn round(
        &mut self,
        k: usize,
        card: &RoundCard,
        page: &StagePage,
        buttons: &mut [Option<Rect>],
    ) -> Rect {
        let frame_at = self.pieces.len();
        let (x, w, top) = (self.x, self.w, self.y);
        self.x += AGENT_PAD;
        self.w = (w - 2 * AGENT_PAD).max(1);
        self.y += AGENT_PAD;
        let (left, width) = (self.x, self.w);
        // Its dot, then its name in bold, then how it stands in its tone.
        self.pieces.push((
            Rectangle::new(
                (self.x, self.y + (BODY_LINE - DOT) / 2).into(),
                (DOT, DOT).into(),
            ),
            Piece::CheckDot(card.dot),
        ));
        let name_x = left + CHECK_DOT_ROOM;
        // Bold runs wider than it measures.
        let name_w = (width_of(&card.label, Font::Body.size()).ceil() as i32 + 1) * 9 / 8;
        let name_w = name_w.min((left + width - name_x).max(1));
        self.pieces.push((
            Rectangle::new((name_x, self.y).into(), (name_w, BODY_LINE).into()),
            Piece::Text {
                text: card.label.clone(),
                font: Font::Body,
                ink: Ink::Text,
                bold: true,
            },
        ));
        let state_x = name_x + name_w + PAGE_GAP / 2;
        if state_x < left + width && !card.state.text.is_empty() {
            self.join_next(false);
            self.pieces.push((
                Rectangle::new(
                    (state_x, self.y).into(),
                    (left + width - state_x, BODY_LINE).into(),
                ),
                Piece::Text {
                    text: card.state.text.clone(),
                    font: Font::Body,
                    ink: Ink::Tone(card.state.tone),
                    bold: false,
                },
            ));
        }
        self.y += BODY_LINE;
        if !card.meta.is_empty() {
            (self.x, self.w) = (name_x, (left + width - name_x).max(1));
            self.text(&card.meta, Font::Small, Ink::Muted, TAIL_LINE);
            (self.x, self.w) = (left, width);
        }
        if card.open {
            if !card.asked.is_empty() {
                self.y += FINDING_GAP;
                self.caption(card.asked_caption);
                for asked in &card.asked {
                    let head = match (asked.id.is_empty(), asked.title.is_empty()) {
                        (_, true) => asked.id.clone(),
                        (true, false) => asked.title.clone(),
                        (false, false) => format!("{} \u{b7} {}", asked.id, asked.title),
                    };
                    self.all_text(&head, Font::Body, Ink::Text, false, BODY_LINE);
                    (self.x, self.w) = (left + ROUND_INDENT, (width - ROUND_INDENT).max(1));
                    if !asked.note.is_empty() {
                        let note = format!("\u{201c}{}\u{201d}", asked.note);
                        self.all_text(&note, Font::Small, Ink::Muted, false, TAIL_LINE);
                    }
                    if let Some(now) = &asked.now {
                        self.text(&now.text, Font::Small, Ink::Tone(now.tone), TAIL_LINE);
                    }
                    (self.x, self.w) = (left, width);
                    self.y += ROUND_ITEM_GAP;
                }
            }
            if !card.fixes.is_empty() {
                self.y += FINDING_GAP;
                let done = card.fixes.iter().filter(|(d, _)| *d).count();
                let total = card.fixes.len();
                self.caption(&format!("Fixes \u{b7} {done} of {total} made"));
                self.y += 2;
                self.put(PROGRESS, Piece::Bar { done, total });
                self.y += 8;
                let next = (card.fixes.iter())
                    .position(|(d, _)| !d)
                    .filter(|_| card.running.is_empty());
                for (i, (done, text)) in card.fixes.iter().enumerate() {
                    self.put(
                        MILESTONE_ROW,
                        Piece::Milestone {
                            done: *done,
                            next: next == Some(i),
                            running: card.running.contains(&i),
                            text: text.clone(),
                        },
                    );
                }
            }
            if !card.changed.is_empty() {
                self.y += FINDING_GAP;
                self.caption("Changed");
                for line in &card.changed {
                    self.text(line, Font::Small, Ink::Text, TAIL_LINE);
                }
            }
        }
        let held: Vec<usize> = (card.buttons.iter().copied())
            .filter(|b| *b < page.buttons.len())
            .collect();
        if !held.is_empty() {
            self.y += FINDING_GAP;
            let widths: Vec<i32> = (held.iter())
                .map(|b| button_width(&page.buttons[*b].label))
                .collect();
            let placed = rows_from_right(&widths, self.x, self.x + self.w);
            for (b, (bx, row)) in held.iter().zip(&placed) {
                let by = self.y + row * (ROW_BUTTON + QUIT_BUTTON_GAP);
                let w = button_width(&page.buttons[*b].label);
                buttons[*b] = Some(Rectangle::new((*bx, by).into(), (w, ROW_BUTTON).into()));
            }
            self.y += rows_height(&placed);
        }
        self.y += AGENT_PAD;
        (self.x, self.w) = (x, w);
        let frame = Rectangle::new((x, top).into(), (w, self.y - top).into());
        self.pieces.insert(frame_at, (frame, Piece::RoundCard(k)));
        frame
    }

    /// `groups` of a toolbar in `row`, from its right: each word, muted,
    /// then its buttons.
    fn toolbar(
        &mut self,
        groups: &[(String, Vec<usize>)],
        page: &StagePage,
        buttons: &mut [Option<Rect>],
        row: Rect,
    ) {
        let mut x = row.loc.x + row.size.w;
        for (word, held) in groups.iter().rev() {
            for &b in held.iter().rev().filter(|b| **b < page.buttons.len()) {
                let w = toolbar_button_width(&page.buttons[b].label);
                x -= w;
                let rect = Rectangle::new((x, row.loc.y).into(), (w, ROW_BUTTON).into());
                buttons[b] = Some(rect);
                self.pieces.push((rect, Piece::Button(b)));
                x -= SHEET_BUTTON_GAP;
            }
            let w = width_of(word, Font::Small.size()).ceil() as i32 + 1;
            x -= w;
            let y = row.loc.y + (ROW_BUTTON - TAIL_LINE) / 2;
            self.pieces.push((
                Rectangle::new((x, y).into(), (w, TAIL_LINE).into()),
                Piece::Text {
                    text: word.clone(),
                    font: Font::Small,
                    ink: Ink::Muted,
                    bold: false,
                },
            ));
            x -= TOOLBAR_GAP;
        }
    }

    /// `text` wrapped to the column as `all_text` does, the places saying
    /// what `sheet` looks for tinted; none while it looks for nothing.
    #[allow(clippy::too_many_arguments)]
    fn found_text(&mut self, text: &str, font: Font, ink: Ink, bold: bool, h: i32, sheet: &SheetRow) {
        if sheet.needle.is_empty() {
            return self.all_text(text, font, ink, bold, h);
        }
        let w = if bold { self.w - self.w / 16 } else { self.w };
        for (i, line) in wrapped(text, font.size(), w).iter().enumerate() {
            self.join_next(i == 0);
            let piece = found_piece(line.clone(), font, ink, bold, sheet);
            self.put(h, piece);
        }
    }

    /// Criterion card `k` as a row of Manual test's sheet: its chevron, dot
    /// and title, its buttons at the right; open, its steps under them,
    /// each word in a column of its own, and what went wrong. A rule ends
    /// it. Hidden by the search, it takes no room.
    fn sheet_row(
        &mut self,
        k: usize,
        card: &FindingCard,
        sheet: &SheetRow,
        page: &StagePage,
        buttons: &mut [Option<Rect>],
    ) -> Rect {
        let (x, w, top) = (self.x, self.w, self.y);
        if sheet.hidden {
            return Rectangle::new((x, top).into(), (w, 0).into());
        }
        let frame_at = self.pieces.len();
        let ink = if card.faded { Ink::Muted } else { Ink::Text };
        self.y += SHEET_PAD;
        // Its buttons at the right of its first line, both always there.
        let held: Vec<usize> = (card.buttons.iter().copied())
            .filter(|b| *b < page.buttons.len())
            .collect();
        let widths: Vec<i32> = (held.iter())
            .map(|b| button_width(&page.buttons[*b].label))
            .collect();
        let text_x = x + SHEET_FOLD + CHECK_DOT_ROOM;
        let right = x + w - SHEET_PAD;
        let placed = rows_from_right(&widths, text_x + SHEET_TITLE_MIN, right);
        for (b, ((bx, row), bw)) in held.iter().zip(placed.iter().zip(&widths)) {
            let by = self.y + row * (ROW_BUTTON + QUIT_BUTTON_GAP);
            buttons[*b] = Some(Rectangle::new((*bx, by).into(), (*bw, ROW_BUTTON).into()));
        }
        let buttons_left = placed.iter().map(|(bx, _)| *bx).min().unwrap_or(right);
        let buttons_h = rows_height(&placed).max(ROW_BUTTON);
        let head = self.y;
        // Its chevron, then its dot, lined up with the first line's text.
        let line_y = head + (ROW_BUTTON - BODY_LINE) / 2;
        self.pieces.push((
            Rectangle::new((x, head).into(), (SHEET_FOLD, ROW_BUTTON).into()),
            Piece::Fold(k),
        ));
        if let Some(dot) = card.tag.dot().or(card.made.then_some(Dot::Done)) {
            self.pieces.push((
                Rectangle::new(
                    (x + SHEET_FOLD, line_y + (BODY_LINE - DOT) / 2).into(),
                    (DOT, DOT).into(),
                ),
                Piece::CheckDot(dot),
            ));
        }
        let room = (buttons_left - QUIT_BUTTON_GAP - text_x).max(1);
        let tail = match card.state.is_empty() {
            true => card.place.clone(),
            false => format!("{} \u{b7} {}", card.place, card.state),
        };
        if card.collapsed {
            // One line: the title cut to fit, then its id and how it stands.
            let tail_w = (width_of(&tail, Font::Small.size()).ceil() as i32 + 1).min(room / 2);
            let title_room = (room - tail_w - QUIT_BUTTON_GAP).max(1);
            let title = cut_to_width(&card.title, Font::Body.size(), title_room * 8 / 9);
            let title_w = (width_of(&title, Font::Body.size()).ceil() as i32 * 9 / 8 + 1)
                .min(title_room);
            let piece = |text, font, ink, bold| found_piece(text, font, ink, bold, sheet);
            self.pieces.push((
                Rectangle::new((text_x, line_y).into(), (title_w, BODY_LINE).into()),
                piece(title, Font::Body, ink, true),
            ));
            let tail_x = text_x + title_w + QUIT_BUTTON_GAP;
            let tail_room = (buttons_left - QUIT_BUTTON_GAP - tail_x).max(1);
            self.pieces.push((
                Rectangle::new(
                    (tail_x, head + (ROW_BUTTON - TAIL_LINE) / 2).into(),
                    (tail_room, TAIL_LINE).into(),
                ),
                piece(
                    cut_to_width(&tail, Font::Small.size(), tail_room),
                    Font::Small,
                    Ink::Muted,
                    false,
                ),
            ));
            self.y = head + buttons_h;
        } else {
            // The title whole, beside the buttons, and its id under it.
            (self.x, self.w) = (text_x, room);
            self.y = line_y;
            self.found_text(&card.title, Font::Body, ink, true, BODY_LINE, sheet);
            self.found_text(&tail, Font::Small, Ink::Muted, false, TAIL_LINE, sheet);
            self.y = self.y.max(head + buttons_h);
            // Its steps, under all that, across the row.
            let word_w = (sheet.steps.iter())
                .map(|(word, _)| width_of(word, Font::Body.size()).ceil() as i32)
                .max()
                .unwrap_or(0);
            let word_room = match word_w {
                0 => 0,
                w => w + SHEET_WORD_GAP,
            };
            if !sheet.steps.is_empty() {
                self.y += FINDING_GAP;
            }
            for (word, text) in &sheet.steps {
                let at = self.y;
                if !word.is_empty() {
                    self.pieces.push((
                        Rectangle::new((text_x, at).into(), (word_w + 1, BODY_LINE).into()),
                        Piece::Text {
                            text: (*word).to_owned(),
                            font: Font::Body,
                            ink: Ink::Muted,
                            bold: false,
                        },
                    ));
                }
                (self.x, self.w) = (text_x + word_room, (right - text_x - word_room).max(1));
                self.found_text(text, Font::Body, ink, false, BODY_LINE, sheet);
            }
            (self.x, self.w) = (text_x, (right - text_x).max(1));
            if !card.body.is_empty() {
                self.y += FINDING_GAP;
                self.all_text(&card.body, Font::Small, Ink::Muted, false, TAIL_LINE);
            }
            if !card.note.is_empty() {
                self.y += FINDING_GAP;
                self.found_text(&card.note, Font::Body, Ink::Error, false, BODY_LINE, sheet);
            }
            // As tall as its lines, as on a card.
            if let Some(field) = &card.field {
                self.y += FINDING_GAP;
                let h = field_lines_height(field.text.split('\n').count());
                let rect = Rectangle::new((self.x, self.y).into(), (self.w, h).into());
                self.pieces.push((rect, Piece::CardField(k)));
                self.y += h;
            }
        }
        self.y += SHEET_PAD;
        (self.x, self.w) = (x, w);
        let frame = Rectangle::new((x, top).into(), (w, self.y - top).into());
        self.pieces.insert(frame_at, (frame, Piece::FindingCard(k)));
        frame
    }

    /// A collapsed card's one line: tag, title and where it stands cut to fit,
    /// its first button at the right.
    fn collapsed_card(
        &mut self,
        card: &FindingCard,
        page: &StagePage,
        buttons: &mut [Option<Rect>],
    ) {
        let first = card
            .buttons
            .first()
            .copied()
            .filter(|b| *b < page.buttons.len());
        let line = if first.is_some() {
            ROW_BUTTON
        } else {
            BODY_LINE
        };
        let (mut left, mut right) = (self.x, self.x + self.w);
        if let Some(b) = first {
            let w = button_width(&page.buttons[b].label);
            right -= w;
            buttons[b] = Some(Rectangle::new(
                (right, self.y).into(),
                (w, ROW_BUTTON).into(),
            ));
            right -= QUIT_BUTTON_GAP;
        }
        let tag = match card.tag {
            FindingTag::High => Some("high"),
            FindingTag::Medium => Some("medium"),
            FindingTag::Low => Some("low"),
            FindingTag::Yours => Some("yours"),
            _ => None,
        };
        let text_y = self.y + (line - TAIL_LINE) / 2;
        if let Some(tag) = tag {
            let w = width_of(tag, Font::Small.size()).ceil() as i32 + 1;
            self.pieces.push((
                Rectangle::new((left, text_y).into(), (w, TAIL_LINE).into()),
                Piece::Text {
                    text: tag.to_owned(),
                    font: Font::Small,
                    ink: Ink::Muted,
                    bold: false,
                },
            ));
            left += w + QUIT_BUTTON_GAP;
        }
        let mut text = card.title.clone();
        if !card.state.is_empty() {
            text.push_str(&format!(" \u{b7} {}", card.state));
        }
        let room = (right - left).max(1);
        self.pieces.push((
            Rectangle::new((left, text_y).into(), (room, TAIL_LINE).into()),
            Piece::Text {
                text: cut_to_width(&text, Font::Small.size(), room),
                font: Font::Small,
                ink: Ink::Muted,
                bold: false,
            },
        ));
        self.y += line;
    }

    /// The user's comment on a finding, a card of its own inside the
    /// finding's: "Your comment" and its buttons, then what it says.
    fn your_comment(&mut self, card: &FindingCard, page: &StagePage, buttons: &mut [Option<Rect>]) {
        let at = self.pieces.len();
        let (x, w, top) = (self.x, self.w, self.y);
        self.x += COMMENT_PAD;
        self.w = (w - 2 * COMMENT_PAD).max(1);
        self.y += COMMENT_PAD;
        let held: Vec<usize> = (card.comment_buttons.iter().copied())
            .filter(|b| *b < page.buttons.len())
            .collect();
        let widths: Vec<i32> = (held.iter())
            .map(|b| button_width(&page.buttons[*b].label))
            .collect();
        let word = "Your comment";
        // Bold runs wider than it measures.
        let word_w = width_of(word, Font::Small.size()).ceil() as i32 * 9 / 8;
        let placed = rows_from_right(&widths, self.x + word_w + QUIT_BUTTON_GAP, self.x + self.w);
        let line = rows_height(&placed).max(TAIL_LINE);
        self.pieces.push((
            Rectangle::new(
                (self.x, self.y).into(),
                (word_w, ROW_BUTTON.min(line)).into(),
            ),
            Piece::Text {
                text: word.to_owned(),
                font: Font::Small,
                ink: Ink::Text,
                bold: true,
            },
        ));
        for (i, (b, (bx, row))) in held.iter().zip(&placed).enumerate() {
            let by = self.y + row * (ROW_BUTTON + QUIT_BUTTON_GAP);
            let rect = Rectangle::new((*bx, by).into(), (widths[i], ROW_BUTTON).into());
            buttons[*b] = Some(rect);
            self.pieces.push((rect, Piece::Button(*b)));
        }
        self.y += line + FINDING_GAP / 2;
        self.all_text(&card.comment, Font::Body, Ink::Text, false, BODY_LINE);
        self.y += COMMENT_PAD;
        (self.x, self.w) = (x, w);
        let frame = Rectangle::new((x, top).into(), (w, self.y - top).into());
        self.pieces.insert(at, (frame, Piece::YourComment));
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

    /// `text` as a file's text: all of its lines, a line `h` high each.
    fn doc_text(&mut self, text: &str, font: Font, ink: Ink, bold: bool, h: i32) {
        let size = font.size();
        let lines = match bold {
            true => doc_lines(text, self.w, |t| {
                measure_bold(t, size).unwrap_or_else(|| width_of(t, size))
            }),
            false => doc_lines(text, self.w, |t| width_of(t, size)),
        };
        for text in lines {
            self.put(
                h,
                Piece::Text {
                    text,
                    font,
                    ink,
                    bold,
                },
            );
        }
    }

    /// Run `body` in the column `by` further in.
    fn doc_indented(&mut self, by: i32, body: impl FnOnce(&mut Stack)) {
        let (x, w) = (self.x, self.w);
        let by = by.min(w - 1).max(0);
        self.x += by;
        self.w -= by;
        body(self);
        self.x = x;
        self.w = w;
    }

    /// A list item, its text hanging right of its mark.
    fn doc_item(&mut self, depth: u8, mark: ItemMark, text: &str) {
        let gutter = match mark {
            ItemMark::Number(n) => {
                let w = width_of(&number_mark(n), Font::Body.size()).ceil() as i32 + 6;
                w.max(DOC_MARK)
            }
            _ => DOC_MARK,
        };
        let indent = i32::from(depth.min(DOC_DEPTH_MAX)) * DOC_INDENT;
        let top = self.y;
        self.doc_indented(indent, |s| {
            if mark != ItemMark::Under {
                let spot = Rectangle::new((s.x, s.y).into(), (gutter, BODY_LINE).into());
                s.pieces.push((spot, Piece::ItemMark(mark)));
            }
            s.doc_indented(gutter, |s| {
                s.doc_text(text, Font::Body, Ink::Text, false, BODY_LINE);
            });
        });
        // A mark with no words of its own still takes its line.
        self.y = self.y.max(top + BODY_LINE);
    }

    /// A file's block, from here down.
    fn doc_block(&mut self, block: &DocBlock) {
        match block {
            DocBlock::Heading { level: 1, text } => {
                self.doc_text(text, Font::Heading, Ink::Text, true, BODY_LINE + 6);
            }
            DocBlock::Heading { text, .. } => {
                self.doc_text(text, Font::Body, Ink::Text, true, BODY_LINE);
            }
            DocBlock::Para(text) => self.doc_text(text, Font::Body, Ink::Text, false, BODY_LINE),
            DocBlock::Note(text) => self.doc_text(text, Font::Body, Ink::Muted, false, BODY_LINE),
            DocBlock::Item { depth, mark, text } => self.doc_item(*depth, *mark, text),
            DocBlock::Code(lines) => {
                let edge = |s: &mut Stack| s.put(DOC_CODE_PAD / 2, Piece::CodeLine(String::new()));
                edge(self);
                for code in lines {
                    self.put(SMALL_LINE, Piece::CodeLine(code.clone()));
                }
                edge(self);
            }
            DocBlock::Quote(text) => {
                let top = self.y;
                self.doc_indented(DOC_QUOTE, |s| {
                    s.doc_text(text, Font::Body, Ink::Muted, false, BODY_LINE);
                });
                let bar =
                    Rectangle::new((self.x, top).into(), (DOC_QUOTE_BAR, self.y - top).into());
                self.pieces.push((bar, Piece::QuoteBar));
            }
            DocBlock::Table(rows) => {
                for (i, row) in rows.iter().enumerate() {
                    self.doc_text(row, Font::Small, Ink::Text, i == 0, SMALL_LINE);
                }
            }
            DocBlock::Rule => {
                self.put(DOC_RULE, Piece::Rule);
            }
        }
    }

    /// A file's blocks, from here down.
    fn doc(&mut self, blocks: &Arc<Vec<DocBlock>>) {
        let (laid, h) = doc_laid_out(blocks, self.w);
        let (x, y) = (self.x, self.y);
        self.pieces.extend(laid.iter().map(|(rect, piece)| {
            let at = Rectangle::new((rect.loc.x + x, rect.loc.y + y).into(), rect.size);
            (at, piece.clone())
        }));
        self.y += h;
    }
}

/// The repositories' unfolded rows, by group.
fn repo_stack(
    view: &FeatureTabView,
    s: &mut Stack,
    repos: &mut Vec<Rect>,
    buttons: &mut Vec<Vec<Rect>>,
    removes: &mut Vec<Option<Rect>>,
) {
    if let Some(why) = &view.repos_error {
        s.text(
            &format!("couldn't read: {why}"),
            Font::Small,
            Ink::Error,
            SMALL_LINE,
        );
    }
    for (i, repo) in view.repos.iter().enumerate() {
        if let Some(group) = &repo.group {
            if i > 0 {
                s.y += REPO_GROUP_GAP;
            }
            s.caption(group);
        }
        let row = s.put(REPO_ROW, Piece::Repo(i));
        s.y += REPO_GAP;
        let y = row.loc.y + (REPO_ROW - ROW_BUTTON) / 2;
        // The × keeps its slot on every row, so the buttons line up.
        let slot = row.loc.x + row.size.w - ROW_BUTTON - 4;
        removes.push(
            (repo.remove && !repo.busy)
                .then(|| Rectangle::new((slot, y).into(), (ROW_BUTTON, ROW_BUTTON).into())),
        );
        // From the right. One with no room left has none: the keys reach it.
        let mut rects = vec![Rect::default(); repo.buttons.len()];
        let mut end = slot - 2;
        let left = row.loc.x + DOT_ROOM + 4 + STAT_NAME_MIN;
        for (b, label) in repo.buttons.iter().enumerate().rev() {
            let w = button_width(label);
            if repo.busy || end - w < left {
                break;
            }
            rects[b] = Rectangle::new((end - w, y).into(), (w, ROW_BUTTON).into());
            end -= w + QUIT_BUTTON_GAP;
        }
        buttons.push(rects);
        repos.push(row);
    }
    for (i, note) in view.repo_notes.iter().enumerate() {
        if i > 0 || !view.repos.is_empty() {
            s.y += REPO_GROUP_GAP;
        }
        s.caption(&note.caption);
        let ink = if note.error { Ink::Error } else { Ink::Muted };
        s.text(&note.text, Font::Small, ink, SMALL_LINE);
    }
    if view.repos.is_empty() && view.repo_notes.is_empty() && view.repos_error.is_none() {
        s.text("No repositories", Font::Small, Ink::Muted, SMALL_LINE);
    }
}

/// A repository's `stat` as words: its files, the lines added, the lines
/// removed, each left out when 0.
fn stat_parts(stat: Stat) -> [Option<String>; 3] {
    let files = match stat.files {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    [
        (stat.files > 0).then_some(files),
        (stat.added > 0).then(|| format!("+{}", stat.added)),
        (stat.removed > 0).then(|| format!("\u{2212}{}", stat.removed)),
    ]
}

/// How wide a repository's stat is, or nothing for none.
fn stat_width(stat: Option<Stat>) -> i32 {
    let Some(stat) = stat else {
        return 0;
    };
    let parts = stat_parts(stat);
    let widths = (parts.iter().flatten()).map(|t| width_of(t, DETAIL_FONT_SIZE).ceil() as i32);
    let widths: Vec<i32> = widths.collect();
    widths.iter().sum::<i32>() + STAT_SPACE * (widths.len() as i32 - 1).max(0)
}

/// What the strip's toggle says beside its chevron.
fn repos_toggle_label(view: &FeatureTabView) -> Option<String> {
    (!view.expanded && view.more > 0).then(|| format!("{} more", view.more))
}

/// How wide things `widths` wide are on one row, with their gaps.
fn row_width(widths: &[i32]) -> i32 {
    widths.iter().sum::<i32>() + QUIT_BUTTON_GAP * (widths.len() as i32 - 1).max(0)
}

/// Where things `widths` wide go, in their order, between `left` and
/// `right`: each its `x` and row, as many to a row as fit, the rows ending
/// at `right`.
fn rows_from_right(widths: &[i32], left: i32, right: i32) -> Vec<(i32, i32)> {
    let mut rows: Vec<Vec<i32>> = Vec::new();
    for &w in widths {
        match rows.last_mut() {
            Some(row) if row_width(row) + QUIT_BUTTON_GAP + w <= right - left => row.push(w),
            _ => rows.push(vec![w]),
        }
    }
    let mut placed = Vec::with_capacity(widths.len());
    for (r, row) in rows.iter().enumerate() {
        let mut x = right - row_width(row);
        for w in row {
            placed.push((x, r as i32));
            x += w + QUIT_BUTTON_GAP;
        }
    }
    placed
}

/// How high the rows of `rows_from_right` are, none for nothing placed.
fn rows_height(placed: &[(i32, i32)]) -> i32 {
    placed
        .last()
        .map_or(0, |(_, row)| (row + 1) * ROW_BUTTON + row * QUIT_BUTTON_GAP)
}

/// The page's column: the selected stage's title, about, status, buttons
/// and sections, then the activity if it has no column.
/// How wide `n` agent chips' outline is, with the room.
fn agent_picks_width(n: usize) -> i32 {
    n as i32 * PICK_CHIP + 2 * PICK_TRACK_PAD
}

/// How wide the button picking an agent's model is, its label ending in an
/// ellipsis past that: a model's id can be long.
fn agent_model_width(label: &str) -> i32 {
    button_width(label).min(AGENT_MODEL_MAX)
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

/// Build's milestone choice in `row`: after its label, each segment as wide
/// as its words, the two squeezed alike on a narrow column.
fn milestone_mode_rects(row: Rect) -> (Rect, [Rect; 2]) {
    let x = row.loc.x + PICK_LABEL;
    let room = (row.size.w - PICK_LABEL - 2 * PICK_TRACK_PAD).max(2);
    let [one, each] = MODE_SEGMENTS.map(button_width);
    let scaled = |w: i32| match one + each > room {
        true => w * room / (one + each),
        false => w,
    };
    let (one, each) = (scaled(one), scaled(each));
    let y = row.loc.y + PICK_TRACK_PAD;
    let h = PICK_TRACK - 2 * PICK_TRACK_PAD;
    let left = Rectangle::new((x + PICK_TRACK_PAD, y).into(), (one, h).into());
    let right = Rectangle::new((x + PICK_TRACK_PAD + one, y).into(), (each, h).into());
    let track = Rectangle::new(
        (x, row.loc.y).into(),
        (one + each + 2 * PICK_TRACK_PAD, PICK_TRACK).into(),
    );
    (track, [left, right])
}

/// Where a Ship group's choice goes in `row`: its outline and segments.
fn ship_route_rects(row: Rect) -> (Rect, [Rect; 3]) {
    let room = (row.size.w - 2 * PICK_TRACK_PAD).max(3);
    let widths = SHIP_ROUTES.map(button_width);
    let all: i32 = widths.iter().sum();
    let scaled = widths.map(|w| match all > room {
        true => w * room / all,
        false => w,
    });
    let y = row.loc.y + PICK_TRACK_PAD;
    let h = PICK_TRACK - 2 * PICK_TRACK_PAD;
    let mut x = row.loc.x + PICK_TRACK_PAD;
    let segments = scaled.map(|w| {
        let segment = Rectangle::new((x, y).into(), (w, h).into());
        x += w;
        segment
    });
    let track = Rectangle::new(
        row.loc,
        (scaled.iter().sum::<i32>() + 2 * PICK_TRACK_PAD, PICK_TRACK).into(),
    );
    (track, segments)
}

/// Where the agent card's choices go: its chips (their outline, then each)
/// and the button picking its model.
#[derive(Default)]
struct AgentChoiceSpots {
    picks: Option<(Rect, Vec<Rect>)>,
    model: Option<Rect>,
}

#[allow(clippy::too_many_arguments)]
fn page_stack(
    view: &FeatureTabView,
    s: &mut Stack,
    buttons: &mut [Option<Rect>],
    choices: &mut AgentChoiceSpots,
    milestone_mode: &mut Option<(Rect, [Rect; 2])>,
    ship: &mut ShipSpots,
    finding_cards: &mut Vec<Rect>,
    activity: bool,
) {
    let page = &view.page;
    // What acts on the stage as a whole: what no card holds. At the title's
    // right if it fits, else in rows of their own further down.
    let held: Vec<usize> = page
        .sections
        .iter()
        .flat_map(|sec| match sec {
            PageSection::Agent { buttons, .. } => buttons.clone(),
            PageSection::File { buttons, .. } | PageSection::Milestones { buttons, .. } => {
                buttons.clone()
            }
            PageSection::Toolbar { groups } => groups.iter().flat_map(|(_, b)| b.clone()).collect(),
            PageSection::Finding(card) => (card.buttons.iter().chain(&card.comment_buttons))
                .copied()
                .collect(),
            PageSection::Round(card) => card.buttons.clone(),
            _ => Vec::new(),
        })
        .collect();
    let free: Vec<usize> = (0..page.buttons.len())
        .filter(|i| !held.contains(i))
        .collect();
    let widths: Vec<i32> = (free.iter())
        .map(|i| button_width(&page.buttons[*i].label))
        .collect();
    let (left, right) = (s.x, s.x + s.w);
    let title_w = measure_bold(&page.title, Font::Heading.size())
        .unwrap_or_else(|| width_of(&page.title, Font::Heading.size()))
        .ceil() as i32;
    let beside = row_width(&widths) <= s.w - title_w - PAGE_GAP;
    if beside {
        let y = s.y + (TITLE_LINE - ROW_BUTTON) / 2;
        for (i, (x, _)) in free.iter().zip(rows_from_right(&widths, left, right)) {
            let w = button_width(&page.buttons[*i].label);
            let rect = Rectangle::new((x, y).into(), (w, ROW_BUTTON).into());
            buttons[*i] = Some(rect);
            s.pieces.push((rect, Piece::Button(*i)));
        }
    }
    s.put(
        TITLE_LINE,
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
    if !beside {
        s.y += 10;
        let placed = rows_from_right(&widths, left, right);
        for (i, (x, row)) in free.iter().zip(&placed) {
            let w = button_width(&page.buttons[*i].label);
            let y = s.y + row * (ROW_BUTTON + QUIT_BUTTON_GAP);
            let rect = Rectangle::new((*x, y).into(), (w, ROW_BUTTON).into());
            buttons[*i] = Some(rect);
            s.pieces.push((rect, Piece::Button(*i)));
        }
        s.y += rows_height(&placed);
    }
    // The toolbar went at the right of the caption before it.
    let mut beside = None;
    let sheet = |k: usize| {
        matches!(page.sections.get(k), Some(PageSection::Finding(c)) if c.sheet.is_some())
    };
    for (k, section) in page.sections.iter().enumerate() {
        if matches!(section, PageSection::Doc { .. }) || beside == Some(k) {
            continue;
        }
        // A sheet's rows go one under the other, a rule between them.
        if !(sheet(k) && k > 0 && (sheet(k - 1) || beside == Some(k - 1))) {
            s.y += PAGE_GAP;
        }
        match section {
            PageSection::Doc { .. } => {}
            PageSection::Toolbar { groups } => {
                let row = s.put(ROW_BUTTON, Piece::Caption(String::new()));
                s.toolbar(groups, page, buttons, row);
            }
            PageSection::Agent {
                name,
                tail,
                buttons: held,
                picks,
                model,
                ..
            } => {
                let model_w = model.as_deref().map(agent_model_width);
                let held: Vec<usize> = (held.iter().copied())
                    .filter(|b| *b < page.buttons.len())
                    .collect();
                let tail_h = match tail.len() {
                    0 => 0,
                    n => n as i32 * TAIL_LINE + 4,
                };
                let inner = (s.w - 2 * AGENT_PAD).max(1);
                let inner_x = s.x + AGENT_PAD;
                let inner_right = inner_x + inner;
                let widths = (held.iter()).map(|b| button_width(&page.buttons[*b].label));
                if name.is_empty() {
                    // No run yet: its chips after their label, its buttons
                    // at the card's end.
                    let widths: Vec<i32> = widths.collect();
                    let card = s.put(AGENT_PAD * 2 + AGENT_HEAD, Piece::Agent(k));
                    let row = card.loc.y + AGENT_PAD + (AGENT_HEAD - ROW_BUTTON) / 2;
                    let placed = rows_from_right(&widths, inner_x, inner_right);
                    for (b, ((x, _), w)) in held.iter().zip(placed.iter().zip(&widths)) {
                        buttons[*b] =
                            Some(Rectangle::new((*x, row).into(), (*w, ROW_BUTTON).into()));
                    }
                    if !picks.is_empty() {
                        let from = inner_x + PICK_LABEL;
                        let end = placed
                            .first()
                            .map_or(inner_right, |(x, _)| x - QUIT_BUTTON_GAP);
                        // The model after the chips, who give way to it.
                        let taken = model_w.map_or(0, |w| w + QUIT_BUTTON_GAP);
                        let room = (end - from - taken).max(1);
                        let (track, chips) = agent_pick_rects(picks.len(), from, row, room);
                        if let Some(w) = model_w {
                            let x = track.loc.x + track.size.w + QUIT_BUTTON_GAP;
                            let w = w.min((end - x).max(1));
                            choices.model =
                                Some(Rectangle::new((x, row).into(), (w, ROW_BUTTON).into()));
                        }
                        choices.picks = Some((track, chips));
                    }
                    continue;
                }
                // A run: buttons, chips, then its model, at the head's end
                // while its name and some of its status keep room, else in
                // rows below.
                let chips = (!picks.is_empty()).then(|| agent_picks_width(picks.len()));
                let widths: Vec<i32> = widths.chain(chips).chain(model_w).collect();
                let name_w = measure_bold(name, FONT_SIZE)
                    .unwrap_or_else(|| width_of(name, FONT_SIZE))
                    .ceil() as i32;
                let fits = row_width(&widths) <= inner - AGENT_TEXT_AT - name_w - AGENT_STATUS_MIN;
                let placed = rows_from_right(&widths, inner_x, inner_right);
                let below = match fits {
                    true => 0,
                    false => rows_height(&placed) + AGENT_ROWS_GAP,
                };
                let card = s.put(AGENT_PAD * 2 + AGENT_HEAD + below + tail_h, Piece::Agent(k));
                let head_y = card.loc.y + AGENT_PAD + (AGENT_HEAD - ROW_BUTTON) / 2;
                let rows_y = card.loc.y + AGENT_PAD + AGENT_HEAD + AGENT_ROWS_GAP;
                let spot = |(x, row): &(i32, i32)| match fits {
                    true => (*x, head_y),
                    false => (*x, rows_y + row * (ROW_BUTTON + QUIT_BUTTON_GAP)),
                };
                for ((b, place), w) in held.iter().zip(&placed).zip(&widths) {
                    buttons[*b] = Some(Rectangle::new(spot(place).into(), (*w, ROW_BUTTON).into()));
                }
                if let (Some(w), Some(place)) = (chips, placed.get(held.len())) {
                    let (x, y) = spot(place);
                    choices.picks = Some(agent_pick_rects(picks.len(), x, y, w));
                }
                if let (Some(w), Some(place)) = (model_w, placed.last()) {
                    choices.model =
                        Some(Rectangle::new(spot(place).into(), (w, ROW_BUTTON).into()));
                }
            }
            PageSection::MilestoneMode { fresh } => {
                let row = s.put(PICK_TRACK, Piece::MilestoneMode { fresh: *fresh });
                *milestone_mode = Some(milestone_mode_rects(row));
            }
            PageSection::ShipGroup(group) => {
                if group.caption {
                    s.caption(&group.head);
                } else {
                    s.text(&group.head, Font::Body, Ink::Text, BODY_LINE);
                    s.y += 4;
                }
                if let Some(route) = &group.route {
                    let row = s.put(PICK_TRACK, Piece::ShipRoute(k));
                    let (track, segments) = ship_route_rects(row);
                    ship.routes.push((route.row, track, segments));
                    ship.rows.push((route.row, row));
                    finding_cards.push(row);
                    s.y += 4;
                }
                if let Some(note) = &group.note {
                    s.text(note, Font::Small, Ink::Muted, SMALL_LINE);
                }
                for (j, step) in group.steps.iter().enumerate() {
                    let detail = step.detail.as_ref().map_or(0, |_| SMALL_LINE);
                    let row = s.put(CHECKBOX_ROW + detail, Piece::ShipStep(k, j));
                    ship.rows.push((step.row, row));
                    finding_cards.push(row);
                }
            }
            PageSection::File { buttons: held, .. } => {
                let card = s.put(AGENT_PAD * 2 + AGENT_HEAD, Piece::File(k));
                // Its buttons from the card's right, in their order.
                let mut bx = card.loc.x + card.size.w - AGENT_PAD;
                let by = card.loc.y + AGENT_PAD + (AGENT_HEAD - ROW_BUTTON) / 2;
                for &b in held.iter().rev().filter(|b| **b < page.buttons.len()) {
                    let w = button_width(&page.buttons[b].label);
                    bx -= w;
                    buttons[b] = Some(Rectangle::new((bx, by).into(), (w, ROW_BUTTON).into()));
                    bx -= QUIT_BUTTON_GAP;
                }
            }
            PageSection::Finding(card) => {
                let frame = s.finding(k, card, page, buttons);
                finding_cards.push(frame);
            }
            PageSection::Round(card) => {
                s.round(k, card, page, buttons);
            }
            PageSection::Lines { label, lines } => {
                if !label.is_empty() {
                    let caption = s.put(CAPTION, Piece::Caption(label.to_uppercase()));
                    if let Some(PageSection::Toolbar { groups }) = page.sections.get(k + 1) {
                        let room = caption.size.w
                            - width_of(&label.to_uppercase(), DETAIL_FONT_SIZE).ceil() as i32
                            - PAGE_GAP;
                        if toolbar_width(groups, page) <= room {
                            let row = Rectangle::new(
                                (caption.loc.x, caption.loc.y + (CAPTION - ROW_BUTTON) / 2).into(),
                                (caption.size.w, ROW_BUTTON).into(),
                            );
                            s.toolbar(groups, page, buttons, row);
                            s.y = s.y.max(row.loc.y + ROW_BUTTON) + 2;
                            beside = Some(k + 1);
                        }
                    }
                }
                for line in lines {
                    s.text(&line.text, Font::Body, Ink::Tone(line.tone), BODY_LINE);
                }
            }
            PageSection::Milestones {
                label,
                state,
                done,
                total,
                items,
                running,
                buttons: held,
            } => {
                let said = format!("{label} \u{b7} {done} of {total}").to_uppercase();
                let caption = s.put(CAPTION, Piece::GroupCaption(k, said));
                // Its buttons from the caption's right, in their order.
                let mut bx = caption.loc.x + caption.size.w;
                let by = caption.loc.y + (CAPTION - ROW_BUTTON) / 2;
                for &b in held.iter().rev().filter(|b| **b < page.buttons.len()) {
                    let w = button_width(&page.buttons[b].label);
                    bx -= w;
                    buttons[b] = Some(Rectangle::new((bx, by).into(), (w, ROW_BUTTON).into()));
                    bx -= QUIT_BUTTON_GAP;
                }
                s.y += 2;
                s.put(
                    PROGRESS,
                    Piece::Bar {
                        done: *done,
                        total: *total,
                    },
                );
                s.y += 8;
                if let Some(state) = state {
                    s.text(&state.text, Font::Body, Ink::Tone(state.tone), BODY_LINE);
                }
                // What runs says where the work is; else what comes next.
                let next = items
                    .iter()
                    .position(|(d, _)| !d)
                    .filter(|_| running.is_empty());
                for (i, (done, text)) in items.iter().enumerate() {
                    s.put(
                        MILESTONE_ROW,
                        Piece::Milestone {
                            done: *done,
                            next: next == Some(i),
                            running: running.contains(&i),
                            text: text.clone(),
                        },
                    );
                }
            }
        }
    }
    let inline = activity && !view.activity.is_empty();
    if inline {
        s.y += PAGE_GAP;
        s.caption("Activity");
        s.activity(&view.activity);
    }
    // Last: however long it is, what else the page says stays near its top.
    for section in &page.sections {
        if let PageSection::Doc { name, blocks } = section {
            s.y += PAGE_GAP;
            // Away from its file's card, it says what it is.
            if inline {
                s.caption(name);
            }
            s.doc(blocks);
        }
    }
}

/// Where `ReviewSwitch`'s parts go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSwitchSpots {
    pub track: Rect,
    pub segments: [Rect; 2],
    /// How many files are viewed, drawn as a bar, while there is room.
    pub bar: Option<Rect>,
    /// What is left, in words.
    pub left: Rect,
    /// The segments say their figures: there is room.
    pub full: bool,
    /// Where each of `ReviewSwitch::buttons` goes; none for one the row has
    /// no room for, the leftmost going first.
    pub buttons: Vec<Option<Rect>>,
}

/// Where `switch` goes in the row at `(x, y)`, `width` wide: the choice at its
/// left, its figures cut if they wouldn't fit, then the bar and what is left,
/// the buttons at the right.
fn review_switch_spots(switch: &ReviewSwitch, x: i32, y: i32, width: i32) -> ReviewSwitchSpots {
    let measure = |label: &str| width_of(label, SMALL_FONT_SIZE).ceil() as i32 + 2 * CHIP_PAD;
    let [code, fixes] = switch.labels();
    let widths = [measure(&code), measure(&fixes)];
    let h = REVIEW_SWITCH_H;
    // The buttons first: the least of the switch and its words is all they
    // leave. The rightmost, the one to press next, stays longest.
    let least = measure("Code review") + measure("Fixes") + 2 * PICK_TRACK_PAD + COLUMN_GAP;
    let mut sizes: Vec<i32> = (switch.buttons.iter())
        .map(|b| button_width(&b.label))
        .collect();
    let mut dropped = 0;
    while sizes.len() > 1 && row_width(&sizes) > width - least - REVIEW_LEFT_MIN {
        sizes.remove(0);
        dropped += 1;
    }
    let fits = !sizes.is_empty() && row_width(&sizes) <= width - least;
    let end = x + width;
    let mut buttons = vec![None; switch.buttons.len()];
    let by = y + (h - ROW_BUTTON) / 2;
    if fits {
        for (i, (bx, _)) in rows_from_right(&sizes, x, end).into_iter().enumerate() {
            let rect = Rectangle::new((bx, by).into(), (sizes[i], ROW_BUTTON).into());
            buttons[dropped + i] = Some(rect);
        }
    }
    let end = match buttons.iter().flatten().next() {
        Some(first) => first.loc.x - COLUMN_GAP,
        None => end,
    };
    let width = end - x;
    let room = width - 2 * PICK_TRACK_PAD;
    let full = widths[0] + widths[1] <= room;
    let widths = match full {
        true => widths,
        false => [measure("Code review"), measure("Fixes")],
    };
    let inner = h - 2 * PICK_TRACK_PAD;
    let first = Rectangle::new(
        (x + PICK_TRACK_PAD, y + PICK_TRACK_PAD).into(),
        (widths[0], inner).into(),
    );
    let second = Rectangle::new(
        (first.loc.x + widths[0], y + PICK_TRACK_PAD).into(),
        (widths[1], inner).into(),
    );
    let track_w = widths[0] + widths[1] + 2 * PICK_TRACK_PAD;
    let track = Rectangle::new((x, y).into(), (track_w, h).into());
    let from = x + track_w + COLUMN_GAP;
    let has_bar = switch.files.is_some() && end - from >= REVIEW_BAR_W + 2 * COLUMN_GAP + REVIEW_LEFT_MIN;
    let bar = has_bar.then(|| {
        Rectangle::new(
            (from, y + (h - REVIEW_BAR_H) / 2).into(),
            (REVIEW_BAR_W, REVIEW_BAR_H).into(),
        )
    });
    let left_x = bar.map_or(from, |bar| bar.loc.x + bar.size.w + COLUMN_GAP);
    let left = Rectangle::new((left_x, y).into(), ((end - left_x).max(0), h).into());
    ReviewSwitchSpots {
        track,
        segments: [first, second],
        bar,
        left,
        full,
        buttons,
    }
}

/// Where the feature tab's parts go in `frame`, its tab's area: a header,
/// the stages' rail, then the repositories', page's and activity's
/// columns, and the hints at the foot.
pub fn feature_tab_layout(view: &FeatureTabView, frame: Rect) -> FeatureTabLayout {
    // A pointer move hit-tests the tab several times over the same view,
    // and the frame lays it out again: the last few layouts are kept.
    let found = TAB_LAYOUTS.with_borrow(|memo| {
        (memo.iter())
            .find(|(v, f, _)| *f == frame && v == view)
            .map(|(.., l)| l.clone())
    });
    if let Some(l) = found {
        return l;
    }
    let l = lay_out_tab(view, frame);
    TAB_LAYOUTS.with_borrow_mut(|memo| {
        memo.insert(0, (view.clone(), frame, l.clone()));
        memo.truncate(TAB_LAYOUTS_MAX);
    });
    l
}

/// How many layouts `TAB_LAYOUTS` keeps: the frame's, and the slices' laid
/// out at their own origins.
const TAB_LAYOUTS_MAX: usize = 4;

thread_local! {
    static TAB_LAYOUTS: RefCell<Vec<(FeatureTabView, Rect, FeatureTabLayout)>> =
        const { RefCell::new(Vec::new()) };
}

/// `feature_tab_layout`, laying out.
fn lay_out_tab(view: &FeatureTabView, frame: Rect) -> FeatureTabLayout {
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
        .filter(|_| !view.remove.is_empty())
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
    let rail_top = top + header_h + RAIL_GAP + RAIL_BACK;
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
    // So is one that says what ran a done stage.
    let ran = view.rail.iter().any(|node| node.agent.is_some());
    let rail_h = NODE
        + NODE_GAP
        + NODE_LABEL
        + NODE_LINE
        + if wraps { RAIL_LABEL_STEP } else { 0 }
        + if ran { NODE_LINE } else { 0 };
    let nodes = (0..view.rail.len() as i32)
        .map(|i| {
            let (left, next) = slot(i);
            Rectangle::new((left, rail_top).into(), (next - left, rail_h).into())
        })
        .collect();
    // Under the rail's rule, the columns; or across it all, the review.
    let rule_end = rail_top + rail_h + RAIL_GAP + 1;
    let review_switch = view.review_switch.as_ref().map(|switch| {
        review_switch_spots(switch, x, rule_end + REVIEW_SWITCH_GAP, inner)
    });
    let test_strip = (view.test_strip.as_ref())
        .filter(|_| review_switch.is_none())
        .map(|strip| {
            let open = strip.search.is_some();
            test_strip_spots(x, rule_end + REVIEW_SWITCH_GAP, inner, open)
        });
    let rule_end = match review_switch.is_some() || test_strip.is_some() {
        true => rule_end + 2 * REVIEW_SWITCH_GAP + REVIEW_SWITCH_H,
        false => rule_end,
    };
    let review = view.review_page.then(|| {
        Rectangle::new(
            (frame.loc.x, rule_end).into(),
            (width, (frame.loc.y + frame.size.h - rule_end).max(0)).into(),
        )
    });
    let mut body_top = rule_end + COLUMN_GAP;

    let hints_h = measured_footer_height(&hint_strs(&view.hints), width);
    let hints = Rectangle::new(
        (frame.loc.x, frame.loc.y + frame.size.h - hints_h).into(),
        (width, hints_h).into(),
    );
    let hint_spots = footer_hint_spots(&hint_strs(&view.hints), hints);

    let mut repos = Vec::new();
    let mut repo_buttons = Vec::new();
    let mut repo_removes = Vec::new();
    let mut more_repos = Vec::new();
    let strip_h = if view.review_page { 0 } else { CHIP_ROW };
    let strip = Rectangle::new((x, body_top).into(), (inner, strip_h).into());
    body_top += CHIP_ROW + COLUMN_GAP;
    let chip_y = strip.loc.y + (CHIP_ROW - ROW_BUTTON) / 2;
    let label_w = width_of(REPOS_LABEL, SMALL_FONT_SIZE).ceil() as i32;
    let repos_label = Rectangle::new((x, strip.loc.y).into(), (label_w, CHIP_ROW).into());
    let toggle_w = repos_toggle_label(view).map_or(ROW_BUTTON, |label| {
        CHIP_PAD + width_of(&label, SMALL_FONT_SIZE).ceil() as i32 + TOGGLE_ICON
    });
    let repos_toggle = Rectangle::new(
        (right - toggle_w, chip_y).into(),
        (if view.review_page { 0 } else { toggle_w }, ROW_BUTTON).into(),
    );
    if !view.expanded && view.repos_error.is_none() && !view.review_page {
        // The chips that fit, from the selected one if it would be past
        // them.
        let start = x + label_w + REPOS_LABEL_GAP;
        let end = repos_toggle.loc.x - QUIT_BUTTON_GAP;
        let chip_w = |r: &FeatureRepoRow| {
            let stat = match stat_width(r.stat) {
                0 => 0,
                w => w + STAT_GAP,
            };
            width_of(&r.name, SMALL_FONT_SIZE).ceil() as i32 + stat + DOT_ROOM + 2 * CHIP_PAD
        };
        let n = view.repos.len();
        let more_w = |label: &str| width_of(label, SMALL_FONT_SIZE).ceil() as i32 + 2 * CHIP_PAD;
        // How many chips fit from `from` after a chip `lead` wide, with
        // room kept for the "+N" chip when some are left over.
        let fits = |from: usize, lead: i32| {
            let mut cx = start + lead;
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
        if first > 0 {
            let w = more_w(&more_label(first, false));
            let chip = Rectangle::new((start, chip_y).into(), (w, ROW_BUTTON).into());
            more_repos.push((chip, first - 1, more_label(first, false)));
        }
        let mut cx = start + lead;
        for (i, repo) in view.repos.iter().enumerate() {
            if !shown.contains(&i) {
                repos.push(Rect::default());
                continue;
            }
            let w = chip_w(repo);
            let chip = Rectangle::new((cx, chip_y).into(), (w, ROW_BUTTON).into());
            cx += w + CHIP_GAP;
            repos.push(chip);
        }
        if shown.end < n {
            let label = more_label(n - shown.end, true);
            let w = more_w(&label);
            let chip = Rectangle::new((cx, chip_y).into(), (w, ROW_BUTTON).into());
            more_repos.push((chip, shown.end, label));
        }
        repo_buttons = vec![Vec::new(); repos.len()];
        repo_removes = vec![None; repos.len()];
    }

    let bottom = hints.loc.y - 4;
    // Unfolded, the rows' card goes across the tab, as tall as they need
    // up to its share of the body, pushing the other columns down.
    let repos_card = (view.expanded && !view.review_page).then(|| {
        let mut dry = Stack {
            x: 0,
            w: (inner - 2 * CARD_PAD).max(1),
            y: 0,
            pieces: Vec::new(),
            joins: Vec::new(),
        };
        repo_stack(
            view,
            &mut dry,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        );
        let body = (bottom - body_top).max(0);
        let least = REPOS_CARD_ROWS * (REPO_ROW + REPO_GAP) + CAPTION + 2 * CARD_PAD;
        let cap = (body * REPOS_CARD_SHARE / 100).max(least).min(body);
        let h = (dry.y + 2 * CARD_PAD).min(cap);
        let card = Rectangle::new((x, body_top).into(), (inner, h).into());
        body_top += h + COLUMN_GAP;
        card
    });
    let col_h = (bottom - body_top).max(0);
    let card = |cx: i32, w: i32| Rectangle::new((cx, body_top).into(), (w, col_h).into());
    let mut columns = [None; 3];
    let page_x = x;
    let mut page_w = inner;
    columns[TabColumn::Repos as usize] = repos_card;
    if !view.review_page {
        if width >= ACTIVITY_BELOW {
            columns[TabColumn::Activity as usize] = Some(card(right - ACTIVITY_W, ACTIVITY_W));
            page_w -= ACTIVITY_W + COLUMN_GAP;
        }
        columns[TabColumn::Page as usize] = Some(card(page_x, page_w.max(1)));
    }

    let mut pieces: [Vec<(Rect, Piece)>; 3] = Default::default();
    let mut content = [0; 3];
    let mut page_buttons = vec![None; view.page.buttons.len()];
    let mut choices = AgentChoiceSpots::default();
    let mut milestone_mode = None;
    let mut ship = ShipSpots::default();
    let mut finding_cards = Vec::new();
    let (mut texts, mut matches) = (Vec::new(), Vec::new());
    for column in TabColumn::ALL {
        let Some(place) = columns[column as usize] else {
            continue;
        };
        let mut s = Stack {
            x: place.loc.x + CARD_PAD,
            w: (place.size.w - 2 * CARD_PAD).max(1),
            y: place.loc.y + CARD_PAD,
            pieces: Vec::new(),
            joins: Vec::new(),
        };
        match column {
            TabColumn::Repos => repo_stack(
                view,
                &mut s,
                &mut repos,
                &mut repo_buttons,
                &mut repo_removes,
            ),
            TabColumn::Page => page_stack(
                view,
                &mut s,
                &mut page_buttons,
                &mut choices,
                &mut milestone_mode,
                &mut ship,
                &mut finding_cards,
                columns[TabColumn::Activity as usize].is_none(),
            ),
            TabColumn::Activity => {
                s.caption("Activity");
                s.activity(&view.activity);
            }
        }
        content[column as usize] = s.y + CARD_PAD - place.loc.y;
        if column == TabColumn::Page {
            (s.pieces, texts, matches) = mark_page(view, s.pieces, &s.joins);
        }
        pieces[column as usize] = s.pieces;
    }

    FeatureTabLayout {
        review_switch,
        test_strip,
        review,
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
        repos_label,
        repos_toggle,
        repos,
        repo_buttons,
        repo_removes,
        more_repos,
        page_buttons,
        agent_picks: choices.picks,
        agent_model: choices.model,
        milestone_mode,
        ship,
        finding_cards,
        hints,
        hint_spots,
        pieces,
        texts,
        matches,
    }
}

/// The page's `pieces` marked as `view` has it: each place the search
/// found numbered down the page, the one gone to tinted more, and the
/// ground of the text selected under it. Gives them, with the page's lines
/// of text and those places. `joins` are the pieces going on the line
/// before them (see `Stack::joins`).
fn mark_page(
    view: &FeatureTabView,
    pieces: Vec<(Rect, Piece)>,
    joins: &[usize],
) -> (Vec<(Rect, Piece)>, Vec<PageText>, Vec<PageMatch>) {
    let said: Vec<&str> = (pieces.iter())
        .filter_map(|(_, piece)| piece_text(piece).map(|(text, ..)| text))
        .collect();
    // A selection of lines no longer saying what they did is gone.
    let pick = (view.marks.pick.as_ref())
        .filter(|pick| {
            let says = |spot: TextSpot, text: &str| said.get(spot.line) == Some(&text);
            says(pick.anchor, &pick.said[0]) && says(pick.cursor, &pick.said[1])
        })
        .map(TextPick::ends)
        .filter(|(from, to)| from != to);
    let mut out = Vec::with_capacity(pieces.len() + 4);
    let (mut texts, mut matches) = (Vec::new(), Vec::new());
    let mut card: Option<(usize, Rect)> = None;
    for (i, (rect, mut piece)) in pieces.into_iter().enumerate() {
        if let Piece::FindingCard(k) = piece {
            let n = (view.page.sections[..k].iter())
                .filter(|s| matches!(s, PageSection::Finding(_)))
                .count();
            card = Some((n, rect));
        }
        let Some((text, font, bold)) = piece_text(&piece) else {
            out.push((rect, piece));
            continue;
        };
        let line = texts.len();
        let in_card = card.filter(|(_, r)| r.contains(rect.loc)).map(|(n, _)| n);
        if let Some((from, to)) = pick.filter(|(from, to)| (from.line..=to.line).contains(&line)) {
            let start = if line == from.line { from.ch } else { 0 };
            let end = if line == to.line { to.ch } else { text.chars().count() };
            if start < end {
                let (text, range) = (text.to_owned(), start..end);
                out.push((rect, Piece::Picked { text, font, bold, range }));
            }
        }
        if let Piece::Found {
            text,
            needle,
            whole,
            current,
            ..
        } = &mut piece
        {
            for (n, range) in crate::find::find(text, needle, *whole).into_iter().enumerate() {
                if view.marks.found_at == Some(matches.len()) {
                    *current = Some(n);
                }
                matches.push(PageMatch {
                    line,
                    card: in_card,
                    range,
                });
            }
        }
        texts.push(PageText {
            piece: out.len(),
            card: in_card,
            joins: joins.contains(&i),
        });
        out.push((rect, piece));
    }
    (out, texts, matches)
}

/// A piece's line of text, its font and whether bold: what can be
/// selected on the page.
fn piece_text(piece: &Piece) -> Option<(&str, Font, bool)> {
    match piece {
        Piece::Text {
            text, font, bold, ..
        }
        | Piece::Found {
            text, font, bold, ..
        } => Some((text, *font, *bold)),
        _ => None,
    }
}

/// How wide `text` is in `font`, bold or not, in logical pixels.
fn text_width_of(text: &str, font: Font, bold: bool) -> f32 {
    match bold {
        true => measure_bold(text, font.size()).unwrap_or_else(|| width_of(text, font.size())),
        false => width_of(text, font.size()),
    }
}

/// The char of `text` nearest `x`, its first at 0: the one whose middle
/// `x` is before, else past the last.
fn char_at_x(text: &str, font: Font, bold: bool, x: f32) -> usize {
    let mut pen = 0.0;
    for (i, ch) in text.chars().enumerate() {
        let advance = text_width_of(ch.encode_utf8(&mut [0; 4]), font, bold);
        if x < pen + advance / 2.0 {
            return i;
        }
        pen += advance;
    }
    text.chars().count()
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
/// The ground of the user's comment on a finding.
fn comment_ground(p: &Palette) -> Rgb {
    mix(p.background, p.accent, COMMENT_TINT)
}

/// The stripe down the left of that comment.
fn comment_stripe(p: &Palette) -> Rgb {
    p.accent
}

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
        (row.loc.x + 6, row.loc.y + (row.size.h - REPO_DOT) / 2).into(),
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

/// Where a milestone row's mark goes.
fn milestone_mark(row: Rect) -> Rect {
    Rectangle::new((row.loc.x, row.loc.y + 4).into(), (14, 14).into())
}

/// The blinking dot (`DOT` across) over a repository's still dot's
/// `spot`.
fn repo_blinker(spot: Rect) -> Rect {
    Rectangle::new(spot.loc, (DOT, DOT).into())
}

/// Where the blinking dots of the feature tab showing `view` go, laid
/// out as `l`: the dot of an agent card whose agent is at work, and of a
/// repository being touched. Each as laid out, unscrolled, with the
/// column it is in (and scrolls with); `None` for one in the head (the
/// strip's chips).
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
        if let Piece::CheckDot(dot) = piece
            && dot.blinks()
        {
            dots.push((*rect, Some(TabColumn::Page)));
        }
        if let Piece::Milestone { running: true, .. } = piece {
            let mark = milestone_mark(*rect);
            let inset = (mark.size.w - DOT) / 2;
            let spot = Rectangle::new(
                (mark.loc.x + inset, mark.loc.y + inset).into(),
                (DOT, DOT).into(),
            );
            dots.push((spot, Some(TabColumn::Page)));
        }
    }
    if view.repos_error.is_none() {
        let busy = |i: usize| view.repos.get(i).is_some_and(|r| r.busy);
        match view.expanded {
            false => dots.extend(
                (l.repos.iter().enumerate())
                    .filter(|(i, chip)| busy(*i) && chip.size.w > 0)
                    .map(|(_, chip)| (repo_blinker(repo_chip_dot(*chip)), None)),
            ),
            true => dots.extend(
                (l.repos.iter().enumerate())
                    .filter(|(i, _)| busy(*i))
                    .map(|(_, row)| (repo_blinker(repo_row_dot(*row)), Some(TabColumn::Repos))),
            ),
        }
    }
    dots
}

/// The colour of the line back to Build follows who has the next move: the
/// Build agent while it runs, else the user if something waits for them.
fn rail_back_color(view: &FeatureTabView, back: RailBack, p: &Palette) -> Rgb {
    let mark = |i: usize| view.rail.get(i).map(|n| n.mark);
    if mark(back.to) == Some(NodeMark::Fixing) {
        p.accent
    } else if [back.to, back.from]
        .iter()
        .any(|i| mark(*i) == Some(NodeMark::Waiting))
    {
        p.warning
    } else {
        p.accent
    }
}

/// The line back to Build: up from node `from`, along over the rail, and
/// down into node `to`, on its left, with an arrowhead.
fn rail_back(c: &mut Canvas, from: Rect, to: Rect, scale: f64, color: Rgb) {
    let s = scale as f32;
    let centre = |node: Rect| (node.loc.x + NODE / 2) as f32 * s;
    let (fx, tx) = (centre(from), centre(to));
    let end = (from.loc.y - RAIL_BACK_CLEAR) as f32 * s;
    let top = (from.loc.y - RAIL_BACK_RISE) as f32 * s;
    let r = RAIL_BACK_CORNER as f32 * s;
    let arrow = RAIL_BACK_ARROW as f32 * s;
    // As wide as the rail's connectors.
    let half = s;
    let quarter = |x: f32, y: f32, cx: f32, right: bool| {
        let cy = top + r;
        match y <= cy && (x >= cx) == right {
            true => ((x - cx).hypot(y - cy) - r).abs(),
            false => f32::INFINITY,
        }
    };
    let pad = (2.0 * s).ceil() as i32;
    let (x, y) = ((tx - arrow).floor() as i32 - pad, top.floor() as i32 - pad);
    let w = fx.ceil() as i32 + pad - x;
    let h = end.ceil() as i32 + pad - y;
    c.paint_in(x, y, w, h, color, |x, y| {
        let line = (segment(x, y, fx, end, fx, top + r))
            .min(quarter(x, y, fx - r, true))
            .min(segment(x, y, fx - r, top, tx + r, top))
            .min(quarter(x, y, tx + r, false))
            .min(segment(x, y, tx, top + r, tx, end))
            .min(segment(x, y, tx, end, tx - arrow, end - arrow))
            .min(segment(x, y, tx, end, tx + arrow, end - arrow));
        coverage(line - half)
    });
}

/// How far the glow round the current stage's node reaches past it.
const PULSE_HALO: i32 = 7;
/// How opaque the glow is at the top of its pulse.
const PULSE_PEAK: f32 = 0.7;

/// Where the glow pulsing round the current stage's node goes on the tab
/// showing `view`, laid out as `l`, if a stage is current.
pub fn feature_tab_pulse(view: &FeatureTabView, l: &FeatureTabLayout) -> Option<Rect> {
    let at = |mark| view.rail.iter().position(|n| n.mark == mark);
    let i = at(NodeMark::Fixing).or_else(|| at(NodeMark::Current))?;
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
    /// A column's contents, unscrolled: scrolling it moves its buffer. A
    /// very long column's buffer holds only what shows (see `TALL_COLUMN`).
    Column(TabColumn),
    Hints,
}

/// Physical pixels a column's buffer may be high: one taller holds only
/// what shows of it, and is drawn again as it scrolls.
const TALL_COLUMN: f64 = 8192.0;

impl FeatureTabLayout {
    /// What of the tab, laid out unscrolled, `slice` holds when scrolled
    /// `down` (its column's scroll): a column's contents in full when
    /// they're not too tall at `scale`, else only what shows. A column's
    /// shows `down` higher than this, so at its place. `None` for a column
    /// the tab hasn't, or one with nothing in it.
    pub(super) fn slice_rect(&self, slice: TabSlice, down: f64, scale: f64) -> Option<Rect> {
        match slice {
            TabSlice::Head => {
                // To the strip's end: it fills, and any column's card is below.
                let bottom = self.strip.loc.y + self.strip.size.h + COLUMN_GAP;
                Some(Rectangle::new(
                    self.frame.loc,
                    (self.frame.size.w, bottom - self.frame.loc.y).into(),
                ))
            }
            TabSlice::Hints => Some(self.hints),
            TabSlice::Column(column) => {
                let place = self.columns[column as usize]?;
                let x = place.loc.x + CARD_PAD;
                let w = (place.size.w - 2 * CARD_PAD).max(1);
                let h = self.content[column as usize];
                if h <= 0 || place.size.h <= 0 {
                    return None;
                }
                if f64::from(h) * scale <= TALL_COLUMN {
                    return Some(Rectangle::new((x, place.loc.y).into(), (w, h).into()));
                }
                let dy = down.round() as i32;
                Some(Rectangle::new(
                    (x, place.loc.y + dy).into(),
                    (w, place.size.h).into(),
                ))
            }
        }
    }
}

/// `view` with only what `slice` draws from it kept (and the rest
/// emptied): what its buffer is cached by, with where it goes.
fn slice_view(view: &FeatureTabView, slice: TabSlice) -> FeatureTabView {
    let mut v = FeatureTabView {
        title: String::new(),
        sub: String::new(),
        which: None,
        remove: "",
        rail: Vec::new(),
        back: None,
        repos: Vec::new(),
        repos_error: None,
        repo: 0,
        focus: TabArea::Stages,
        review_page: view.review_page,
        review_switch: None,
        test_strip: None,
        review_keys: view.review_keys,
        expanded: view.expanded,
        more: 0,
        picked: None,
        repo_notes: Vec::new(),
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
        marks: PageMarks::default(),
    };
    use FeatureTabHit as H;
    let strip = !view.expanded;
    let repos = |v: &mut FeatureTabView| {
        v.repos = view.repos.clone();
        v.repos_error = view.repos_error.clone();
        v.repo = view.repo;
        v.focus = view.focus;
    };
    v.hover = view.hover.filter(|hit| match slice {
        TabSlice::Head => {
            matches!(
                hit,
                H::Prev
                    | H::Next
                    | H::Button(_)
                    | H::Node(_)
                    | H::ToggleRepos
                    | H::ReviewView(_)
                    | H::ReviewButton(_)
            ) || strip && matches!(hit, H::Repo(_) | H::MoreRepos(_))
        }
        TabSlice::Column(TabColumn::Repos) => {
            matches!(hit, H::Repo(_) | H::RepoButton(..) | H::RepoRemove(_))
        }
        TabSlice::Column(TabColumn::Page) => {
            matches!(
                hit,
                H::PageButton(_)
                    | H::AgentPick(_)
                    | H::AgentModel
                    | H::MilestoneMode(_)
                    | H::ShipRoute(..)
                    | H::ShipRow(_)
            )
        }
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
            v.back = view.back;
            v.focus = view.focus;
            v.more = view.more;
            v.review_switch = view.review_switch.clone();
            v.test_strip = view.test_strip.clone();
            if strip {
                repos(&mut v);
            }
        }
        TabSlice::Column(TabColumn::Repos) => {
            repos(&mut v);
            v.picked = view.picked;
            v.repo_notes = view.repo_notes.clone();
        }
        TabSlice::Column(TabColumn::Page) => {
            v.page = view.page.clone();
            v.activity = view.activity.clone();
            v.marks = view.marks.clone();
        }
        TabSlice::Column(TabColumn::Activity) => v.activity = view.activity.clone(),
        TabSlice::Hints => v.hints = view.hints.clone(),
    }
    v
}

/// A run of the page column's pieces drawn in a buffer of its own: a card,
/// a row or a line, and what of its neighbours reaches into it. A hover or
/// a selection then draws again only the bands it changes, not the whole
/// column (a long Manual test sheet took a tenth of a second each).
pub(super) struct PageBand {
    /// Where it goes, unscrolled, in output coordinates: across the column,
    /// its edges on whole physical pixels.
    pub(super) rect: Rect,
    /// Its pieces, as `FeatureTabLayout::pieces` has them, in their order.
    pub(super) pieces: Vec<usize>,
}

/// The page section piece `piece` paints from, if one.
fn piece_section(piece: &Piece) -> Option<usize> {
    match piece {
        Piece::GroupCaption(k, _)
        | Piece::ShipRoute(k)
        | Piece::ShipStep(k, _)
        | Piece::Agent(k)
        | Piece::File(k)
        | Piece::FindingCard(k)
        | Piece::Fold(k)
        | Piece::CardField(k) => Some(*k),
        _ => None,
    }
}

/// The page buttons piece `piece` paints (see `Painter::tab_piece`).
fn piece_buttons(view: &FeatureTabView, piece: &Piece) -> Vec<usize> {
    let section = piece_section(piece).and_then(|k| view.page.sections.get(k));
    match (piece, section) {
        (Piece::Button(i), _) => vec![*i],
        (Piece::GroupCaption(..), Some(PageSection::Milestones { buttons, .. }))
        | (Piece::Agent(_), Some(PageSection::Agent { buttons, .. }))
        | (Piece::File(_), Some(PageSection::File { buttons, .. })) => buttons.clone(),
        (Piece::FindingCard(_), Some(PageSection::Finding(card))) => card.buttons.clone(),
        _ => Vec::new(),
    }
}

/// The places of the layout besides its own and its buttons' that piece
/// `piece` paints at.
fn piece_spots(view: &FeatureTabView, l: &FeatureTabLayout, piece: &Piece) -> Vec<Rect> {
    let section = piece_section(piece).and_then(|k| view.page.sections.get(k));
    match (piece, section) {
        (Piece::MilestoneMode { .. }, _) => (l.milestone_mode.iter())
            .flat_map(|(track, segments)| [*track, segments[0], segments[1]])
            .collect(),
        (Piece::ShipRoute(_), Some(PageSection::ShipGroup(group))) => {
            let row = group.route.as_ref().map(|r| r.row);
            let pad = PICK_TRACK_PAD;
            (l.ship.routes.iter())
                .filter(|r| Some(r.0) == row)
                .flat_map(|(_, track, segments)| {
                    let around = Rectangle::new(
                        (track.loc.x - pad, track.loc.y - pad).into(),
                        (track.size.w + 2 * pad, track.size.h + 2 * pad).into(),
                    );
                    [around, *track, segments[0], segments[1], segments[2]]
                })
                .collect()
        }
        (Piece::Agent(_), _) => (l.agent_picks.iter())
            .flat_map(|(track, chips)| std::iter::once(*track).chain(chips.iter().copied()))
            .chain(l.agent_model)
            .collect(),
        (Piece::Repo(i), _) => (l.repo_buttons.get(*i).into_iter().flatten().copied())
            .chain(l.repo_removes.get(*i).copied().flatten())
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether piece `piece`, painting `buttons`, looks otherwise with the
/// pointer over `hit`.
fn piece_hovered(
    view: &FeatureTabView,
    piece: &Piece,
    buttons: &[usize],
    hit: FeatureTabHit,
) -> bool {
    use FeatureTabHit as H;
    // Which finding card a section is, as hits count them.
    let finding = |k: usize| {
        (view.page.sections.iter().take(k))
            .filter(|s| matches!(s, PageSection::Finding(_)))
            .count()
    };
    match (piece, hit) {
        (_, H::PageButton(i)) => buttons.contains(&i),
        (Piece::MilestoneMode { .. }, H::MilestoneMode(_))
        | (Piece::ShipRoute(_), H::ShipRoute(..))
        | (Piece::ShipStep(..), H::ShipRow(_))
        | (Piece::Agent(_), H::AgentPick(_) | H::AgentModel) => true,
        (Piece::FindingCard(k) | Piece::Fold(k), H::Finding(n) | H::Fold(n)) => finding(*k) == n,
        (Piece::Repo(i), H::Repo(r) | H::RepoButton(r, _) | H::RepoRemove(r)) => r == *i,
        _ => false,
    }
}

/// The page column's pieces in bands (see `PageBand`), the column's
/// contents starting at `top`; `None` when they don't split on whole
/// physical pixels at `scale`, or one is too tall for a buffer, when the
/// column is drawn whole.
fn page_bands(
    view: &FeatureTabView,
    l: &FeatureTabLayout,
    (x, top, w): (i32, i32, i32),
    scale: f64,
) -> Option<Vec<PageBand>> {
    let pieces = &l.pieces[TabColumn::Page as usize];
    // Each piece's extent down the column: its own and what it paints at.
    let mut spans: Vec<(i32, i32, usize)> = (pieces.iter().enumerate())
        .map(|(k, (rect, piece))| {
            let buttons = piece_buttons(view, piece);
            let around = (buttons.iter())
                .filter_map(|i| l.page_buttons.get(*i).copied().flatten())
                .chain(piece_spots(view, l, piece));
            let (y0, y1) = around.fold((rect.loc.y, rect.loc.y + rect.size.h), |(a, b), r| {
                (a.min(r.loc.y), b.max(r.loc.y + r.size.h))
            });
            (y0, y1, k)
        })
        .filter(|(y0, y1, _)| y1 > y0)
        .collect();
    spans.sort_unstable();
    // Whether `y` falls on a whole physical pixel of the column's buffer.
    let whole = |y: i32| {
        let px = f64::from(y - top) * scale;
        (px - px.round()).abs() < 1e-6
    };
    const REACH: i32 = 16;
    let down = |y: i32| (0..REACH).map(|d| y - d).find(|y| whole(*y));
    let up = |y: i32| (0..REACH).map(|d| y + d).find(|y| whole(*y));
    // Pieces reaching over one another go in a band together.
    let mut groups: Vec<(i32, i32)> = Vec::new();
    for (y0, y1, _) in &spans {
        match groups.last_mut() {
            Some((_, b)) if *y0 < *b => *b = (*b).max(*y1),
            _ => groups.push((*y0, *y1)),
        }
    }
    // Its edges go out to whole pixels, over the next band's edge when that
    // isn't on one: a band paints every piece reaching into it, so where two
    // overlap both hold the same pixels.
    let mut bands: Vec<(i32, i32, Vec<usize>)> = Vec::new();
    for (y0, y1) in groups {
        let (y0, y1) = (down(y0)?, up(y1)?);
        let pieces = (spans.iter())
            .filter(|(a, b, _)| *a < y1 && *b > y0)
            .map(|(.., k)| *k)
            .collect();
        bands.push((y0, y1, pieces));
    }
    let tall = |h: i32| f64::from(h) * scale > TALL_COLUMN;
    if bands.iter().any(|(y0, y1, _)| tall(y1 - y0)) {
        return None;
    }
    Some(
        (bands.into_iter())
            .map(|(y0, y1, mut pieces)| {
                pieces.sort_unstable();
                PageBand {
                    rect: Rectangle::new((x, y0).into(), (w, y1 - y0).into()),
                    pieces,
                }
            })
            .collect(),
    )
}

/// What a band of the page column is drawn from: its pieces where they go
/// in it, and what of the view and the layout they paint from.
#[derive(PartialEq)]
struct BandKey {
    size: Size<i32, Logical>,
    theme: super::Theme,
    scale: f64,
    pieces: Vec<(Rect, Piece)>,
    sections: Vec<PageSection>,
    buttons: Vec<(Option<PageButton>, Option<Rect>)>,
    spots: Vec<Rect>,
    activity: Vec<Option<ActivityRow>>,
    repos: Vec<Option<FeatureRepoRow>>,
    /// The repositories' focus, selected row and picked button, with any.
    repo_state: Option<(TabArea, usize, Option<usize>)>,
    hover: Option<FeatureTabHit>,
}

fn band_key(
    view: &FeatureTabView,
    l: &FeatureTabLayout,
    band: &PageBand,
    theme: super::Theme,
    scale: f64,
) -> BandKey {
    let pieces = &l.pieces[TabColumn::Page as usize];
    let at = |r: Rect| Rectangle::new(r.loc - band.rect.loc, r.size);
    let mut key = BandKey {
        size: band.rect.size,
        theme,
        scale,
        pieces: Vec::new(),
        sections: Vec::new(),
        buttons: Vec::new(),
        spots: Vec::new(),
        activity: Vec::new(),
        repos: Vec::new(),
        repo_state: None,
        hover: None,
    };
    for &k in &band.pieces {
        let (rect, piece) = &pieces[k];
        key.pieces.push((at(*rect), piece.clone()));
        if let Some(section) = piece_section(piece).and_then(|k| view.page.sections.get(k)) {
            key.sections.push(section.clone());
        }
        let buttons = piece_buttons(view, piece);
        key.buttons.extend(buttons.iter().map(|i| {
            let spot = l.page_buttons.get(*i).copied().flatten().map(at);
            (view.page.buttons.get(*i).cloned(), spot)
        }));
        let spots = piece_spots(view, l, piece);
        key.spots.extend(spots.into_iter().map(at));
        match piece {
            Piece::Activity { row, .. } => key.activity.push(view.activity.get(*row).cloned()),
            Piece::Repo(i) => {
                key.repos.push(view.repos.get(*i).cloned());
                key.repo_state = Some((view.focus, view.repo, view.picked));
            }
            _ => {}
        }
        let hovered = |hit: &FeatureTabHit| piece_hovered(view, piece, &buttons, *hit);
        if let Some(hit) = view.hover.filter(hovered) {
            key.hover = Some(hit);
        }
    }
    key
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
    /// The page column's bands (see `PageBand`).
    bands: super::layers::RowCache<BandKey>,
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
        let p = theme.page_palette();
        let output = area.size;
        let layout = view
            .as_ref()
            .map(|v| feature_tab_layout(v, Rectangle::from_size(output)));
        let placed = layout
            .as_ref()
            .map(|l| ((output, l.columns, theme, scale), area.loc, output));
        let drawn = &self.drawn;
        let (head, columns, bands, hints, thumbs, glow) = (
            &mut self.head,
            &mut self.columns,
            &mut self.bands,
            &mut self.hints,
            &mut self.thumbs,
            &mut self.pulse,
        );
        let more = |_: &BackKey| {
            let (Some(view), Some(l)) = (&view, &layout) else {
                return Vec::new();
            };
            let at = |r: Rect| Point::from((area.loc.x + r.loc.x, area.loc.y + r.loc.y));
            let slice = |cache: &mut super::Cached<SliceKey>, slice: TabSlice, down: f64| {
                let region = l.slice_rect(slice, down, scale)?;
                let key = slice_view(view, slice);
                let buffer = cache.get((key, region, output, theme, scale), |_| {
                    drawn.set(drawn.get() + 1);
                    painter.feature_tab_slice(view, output, slice, region, scale, p)
                });
                Some(Part::new(buffer, at(region), region.size, 1.0))
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
                let region = l.slice_rect(TabSlice::Column(column), 0.0, scale);
                let banded = (region.filter(|_| column == TabColumn::Page))
                    .and_then(|r| page_bands(view, l, (r.loc.x, r.loc.y, r.size.w), scale));
                if let Some(banded) = banded {
                    for band in &banded {
                        let rect = band.rect;
                        let (y0, y1) = (rect.loc.y, rect.loc.y + rect.size.h);
                        let shown = f64::from(y1) - down > f64::from(place.loc.y)
                            && f64::from(y0) - down < f64::from(place.loc.y + place.size.h);
                        if !shown {
                            continue;
                        }
                        let key = band_key(view, l, band, theme, scale);
                        let buffer = bands.get(key, rect.size, scale, |_| {
                            drawn.set(drawn.get() + 1);
                            painter.feature_tab_band(view, l, band, scale, p)
                        });
                        let part = Part::new(&buffer, at(rect), rect.size, 1.0);
                        let loc = (part.loc.x, part.loc.y - down).into();
                        parts.push(part.at(loc).clipped(inside));
                    }
                } else if let Some(part) = slice(&mut columns[i], TabSlice::Column(column), down) {
                    let loc = (part.loc.x, part.loc.y - down).into();
                    parts.push(part.at(loc).clipped(inside));
                }
            }
            bands.sweep();
            parts.extend(slice(hints, TabSlice::Hints, 0.0));
            parts.extend(slice(head, TabSlice::Head, 0.0));
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
    let dot = super::blink_dot(Dot::Working, scale, theme.page_palette());
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
        let layout = new_feature_layout(size, view.slots, view.error.is_some());
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
            view.hint,
            shift(layout.field),
            view.field_focus,
            scale,
            p,
        );
        if let (Some(rect), Some(error)) = (layout.error, &view.error) {
            line(&mut c, error, rect, FONT_SIZE, p.error);
        }
        let root_color = if view.root_failed { p.error } else { p.muted };
        line(&mut c, &view.root, layout.root, FONT_SIZE, root_color);
        let mut projects = layout.projects;
        projects.size.w = layout.link.loc.x - projects.loc.x - QUIT_BUTTON_GAP;
        let hovered = view.hover == Some(NewFeatureHit::Link);
        self.button(
            &mut c,
            view.link,
            shift(layout.link),
            (hovered, false),
            ButtonStyle::Plain,
            scale,
            p,
        );
        line(&mut c, &view.projects, projects, FONT_SIZE, p.muted);

        let shown = view.repos.iter().enumerate().skip(view.top);
        let slots = layout.rows.iter().zip(&layout.bases);
        for ((rect, base), (i, repo)) in slots.zip(shown) {
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
            let mut end = phys(rect, scale).1 - (ROW_INSET as f32 * s).round();
            if let Some(label) = &repo.base {
                let base = shift(*base);
                let on = view.hover == Some(NewFeatureHit::Base(i));
                // A long branch name ellipsizes inside it.
                let centred = self.text_width(label, FONT_SIZE, scale)
                    <= ((BASE_BUTTON_W - 2 * BASE_BUTTON_INSET) as f64 * scale) as f32;
                let word = if centred { label.as_str() } else { "" };
                self.button(
                    &mut c,
                    word,
                    base,
                    (on, false),
                    ButtonStyle::Plain,
                    scale,
                    p,
                );
                if !centred {
                    let inside = Rectangle::new(
                        (base.loc.x + BASE_BUTTON_INSET, base.loc.y).into(),
                        (BASE_BUTTON_W - 2 * BASE_BUTTON_INSET, base.size.h).into(),
                    );
                    let (x, to) = phys(inside, scale);
                    let at = band(inside, scale);
                    self.text(&mut c, label, FONT_SIZE, x, to, at, scale, p.text);
                }
                end = phys(base, scale).0 - (PLACE_GAP as f32 * s).round();
            }
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
        for (i, (rect, label)) in layout.buttons.iter().zip(NEW_FEATURE_BUTTONS).enumerate() {
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
                    self.tab_piece(&mut c, view, &l, *rect, piece, Point::default(), scale, p);
                }
            }
            TabSlice::Hints => {
                c.fill(0, 0, c.w, c.h, p.background);
                self.tab_hints(&mut c, view, &l, scale, p);
            }
        }
        c.into_buffer(true)
    }

    /// Band `band` of the page column (see `page_bands`) of the tab laid
    /// out as `l`, in output coordinates: its pieces on the column's ground.
    pub(super) fn feature_tab_band(
        &self,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        band: &PageBand,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(band.rect.size, scale));
        c.fill(0, 0, c.w, c.h, p.panel);
        let pieces = &l.pieces[TabColumn::Page as usize];
        for &k in &band.pieces {
            let (rect, piece) = &pieces[k];
            let rect = Rectangle::new(rect.loc - band.rect.loc, rect.size);
            self.tab_piece(&mut c, view, l, rect, piece, band.rect.loc, scale, p);
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
                2 => bars(x, y, s),
                3 => plus(x, y, 0.0, 0.0, 4.5 * s, s),
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
            // A stage with work sent back to it was passed all the same.
            let past = |i: usize| view.rail.get(i).map(|n| n.mark);
            let done = matches!(past(i), Some(NodeMark::Done | NodeMark::Stale))
                || !matches!(past(i + 1), Some(NodeMark::Later) | None);
            // Turned back, the flow stands good up to Build, and ran once from
            // there on.
            let ran = |i: usize| matches!(past(i), Some(NodeMark::Ran | NodeMark::Paused));
            let into_build = view.back.is_some_and(|back| i + 1 == back.to);
            let once =
                view.back.is_some_and(|back| i >= back.to) || ran(i) || (ran(i + 1) && !into_build);
            let color = match (done, once) {
                (false, _) => p.border,
                (true, false) => p.success,
                (true, true) => p.muted,
            };
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
        if let Some(back) = view.back
            && let (Some(from), Some(to)) = (l.nodes.get(back.from), l.nodes.get(back.to))
        {
            let color = rail_back_color(view, back, p);
            rail_back(c, circle(*from), circle(*to), scale, color);
        }
        for (i, (slot, node)) in l.nodes.iter().zip(&view.rail).enumerate() {
            let hovered = view.hover == Some(FeatureTabHit::Node(i));
            let spot = circle(*slot);
            if node.selected {
                let ring = if view.focus == TabArea::Stages && !view.review_keys {
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
                NodeMark::Later | NodeMark::Ran if !node.selected => p.muted,
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
                    NodeMark::Current | NodeMark::Fixing | NodeMark::Paused => p.accent,
                    NodeMark::Waiting | NodeMark::Stale => p.warning,
                    _ => p.muted,
                };
                self.centred_cut(c, &node.line, DETAIL_FONT_SIZE, line, false, scale, color);
            }
            if let Some((icon, what)) = &node.agent {
                let below = (lines.len() as i32 - 1) * RAIL_LABEL_STEP + NODE_LINE;
                let row = Rectangle::new(
                    (label.loc.x, label.loc.y + NODE_LABEL + below).into(),
                    (label.size.w, NODE_LINE).into(),
                );
                self.rail_agent(c, *icon, what, row, scale, p);
            }
        }
        // The rule under the rail.
        if let Some(last) = l.nodes.first() {
            let y = last.loc.y + last.size.h + RAIL_GAP;
            c.fill(0, px(y), c.w, px(1).max(1), p.border_variant);
        }

        self.review_switch(c, view, l, scale, p);
        self.test_strip(c, view, l, scale, p);
        self.repo_strip(c, view, l, scale, p);
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

    /// What ran a done stage on the rail: `icon` then `what`, centred
    /// together in `row`, `what` cut at its end when too wide.
    fn rail_agent(
        &self,
        c: &mut Canvas,
        icon: AgentIcon,
        what: &str,
        row: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let (x, end) = phys(row, scale);
        let logo = ((AGENT_ICON + RAIL_AGENT_GAP) as f64 * scale) as f32;
        let width = logo + self.text_width(what, DETAIL_FONT_SIZE, scale);
        let from = (x + ((end - x) - width) / 2.0).max(x);
        let spot = Rectangle::new(
            ((f64::from(from) / scale).round() as i32, row.loc.y).into(),
            (AGENT_ICON, row.size.h).into(),
        );
        agent_icon(c, spot, AGENT_ICON, icon, p.muted, scale);
        let text = (from + logo).round();
        self.text(
            c,
            what,
            DETAIL_FONT_SIZE,
            text,
            end,
            band(row, scale),
            scale,
            p.muted,
        );
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
            NodeMark::Done | NodeMark::Stale | NodeMark::Ran => {
                let color = match mark {
                    NodeMark::Done => p.success,
                    NodeMark::Stale => p.warning,
                    _ => p.muted,
                };
                disc(c, 0.0, color);
                disc(c, ring, mix(p.background, color, 0.18));
                icon_sdf(c, spot, scale, color, |x, y| check(x, y, s * 1.1));
            }
            NodeMark::Current | NodeMark::Waiting | NodeMark::Fixing => {
                let color = if mark == NodeMark::Waiting {
                    p.warning
                } else {
                    p.accent
                };
                disc(c, 0.0, color);
                self.bold_centred(c, &number, spot, scale, p.on_accent);
            }
            NodeMark::Later => {
                disc(c, 0.0, p.border);
                disc(c, ring, p.background);
                self.bold_centred(c, &number, spot, scale, p.muted);
            }
            NodeMark::Paused => {
                disc(c, 0.0, p.accent);
                disc(c, ring, p.background);
                self.bold_centred(c, &number, spot, scale, p.text);
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

    /// The line on Manual test's page: how many criteria passed, failed and
    /// are left, as a bar and in words.
    fn test_strip(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        scale: f64,
        p: &Palette,
    ) {
        let (Some(strip), Some(spots)) = (&view.test_strip, &l.test_strip) else {
            return;
        };
        let hover = match view.hover {
            Some(FeatureTabHit::Search(part)) => Some(part),
            _ => None,
        };
        let bar = strip.search.as_ref();
        self.search_bar(c, bar, &spots.search, spots.count, hover, scale, p);
        if let Some(bar) = spots.bar.filter(|_| strip.total() > 0) {
            let parts = [(strip.passed, p.success), (strip.failed, p.error)];
            count_bar(c, bar, &parts, strip.total(), scale, p);
        }
        let tone = if strip.good() { p.success } else { p.warning };
        let (x, end) = phys(spots.text, scale);
        self.bold_text(
            c,
            &strip.text,
            FONT_SIZE,
            x,
            end,
            band(spots.text, scale),
            scale,
            tone,
        );
    }

    /// The switch on Code review's page, with the figures that say how far its
    /// review has come.
    fn review_switch(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        scale: f64,
        p: &Palette,
    ) {
        let (Some(switch), Some(spots)) = (&view.review_switch, &l.review_switch) else {
            return;
        };
        let [code, fixes] = switch.labels();
        let labels: [&str; 2] = match spots.full {
            true => [&code, &fixes],
            false => ["Code review", "Fixes"],
        };
        let hover = match view.hover {
            Some(FeatureTabHit::ReviewView(i)) => Some(i),
            _ => None,
        };
        let on = usize::from(switch.view == ReviewView::Fixes);
        let labelled = [(labels[0], spots.segments[0]), (labels[1], spots.segments[1])];
        self.segmented(c, spots.track, &labelled, on, hover, scale, p);
        for (i, (b, rect)) in switch.buttons.iter().zip(&spots.buttons).enumerate() {
            let Some(rect) = rect else {
                continue;
            };
            let lit = view.hover == Some(FeatureTabHit::ReviewButton(i));
            self.button(c, &b.label, *rect, (lit, false), b.style, scale, p);
        }
        if let Some(files) = switch.files.filter(|files| files.total > 0)
            && let Some(bar) = spots.bar
        {
            let tone = if files.left().1 { p.success } else { p.warning };
            count_bar(c, bar, &[(files.viewed, tone)], files.total, scale, p);
        }
        let Some((said, good)) = switch.said() else {
            return;
        };
        let tone = if good { p.success } else { p.warning };
        let (x, end) = phys(spots.left, scale);
        self.bold_text(
            c,
            &said,
            FONT_SIZE,
            x,
            end,
            band(spots.left, scale),
            scale,
            tone,
        );
    }

    /// The repositories' line under the rail: its label, the chips or why
    /// they couldn't be read, and the toggle.
    fn repo_strip(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        l: &FeatureTabLayout,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let line = |c: &mut Canvas, text: &str, r: Rect, color: Rgb| {
            let (x, end) = phys(r, scale);
            self.text(
                c,
                text,
                SMALL_FONT_SIZE,
                x,
                end,
                band(r, scale),
                scale,
                color,
            )
        };
        // The review over the tab has no strip.
        if view.review_page {
            return;
        }
        let focused = view.focus == TabArea::Repos;
        let label = if focused { p.accent } else { p.muted };
        line(c, REPOS_LABEL, l.repos_label, label);
        let chip_ground = |c: &mut Canvas, chip: Rect, edge: Rgb, fill: Rgb| {
            let (x, y, w, h) = rect_px(chip, scale);
            let b = s.round().max(1.0);
            c.rounded(x, y, w, h, h / 2.0, edge);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, h / 2.0 - b, fill);
        };
        let lit_fill =
            |lit: bool| super::row_fill(p, false, if lit { 1.0 } else { 0.0 }).unwrap_or(p.panel);
        if !view.expanded
            && let Some(why) = &view.repos_error
        {
            let from = l.repos_label.loc.x + l.repos_label.size.w + REPOS_LABEL_GAP;
            let text = Rectangle::new(
                (from, l.strip.loc.y).into(),
                (
                    (l.repos_toggle.loc.x - QUIT_BUTTON_GAP - from).max(1),
                    CHIP_ROW,
                )
                    .into(),
            );
            line(c, &format!("couldn't read: {why}"), text, p.error);
        }
        if !view.expanded {
            for (i, (chip, repo)) in l.repos.iter().zip(&view.repos).enumerate() {
                if chip.size.w <= 0 {
                    continue;
                }
                let selected = i == view.repo && focused;
                let lit = matches!(view.hover, Some(FeatureTabHit::Repo(r)) if r == i);
                let fill =
                    super::row_fill(p, selected, if lit { 1.0 } else { 0.0 }).unwrap_or(p.panel);
                // The keyboard's chip has the accent's outline.
                let edge = if selected { p.accent } else { p.border_variant };
                chip_ground(c, *chip, edge, fill);
                self.repo_dot(c, repo_chip_dot(*chip), repo, scale, p);
                let from = chip.loc.x + CHIP_PAD + DOT_ROOM - 4;
                let to = chip.loc.x + chip.size.w - CHIP_PAD;
                let room = Rectangle::new(
                    (from, chip.loc.y).into(),
                    ((to - from).max(0), chip.size.h).into(),
                );
                let name_end = match self.repo_stat(c, repo.stat, room, scale, p) {
                    Some(stat) => stat - STAT_GAP,
                    None => to + CHIP_PAD,
                };
                let text = Rectangle::new(
                    (from, chip.loc.y).into(),
                    ((name_end - from).max(1), chip.size.h).into(),
                );
                let color = match repo.kind {
                    RepoKind::Active => p.text,
                    _ => p.muted,
                };
                line(c, &repo.name, text, color);
            }
        }
        // The chips standing for those that don't fit.
        for (chip, i, label) in &l.more_repos {
            let lit = view.hover == Some(FeatureTabHit::MoreRepos(*i));
            chip_ground(c, *chip, p.border_variant, lit_fill(lit));
            let color = if lit { p.text } else { p.muted };
            self.centered(c, label, SMALL_FONT_SIZE, *chip, scale, color);
        }
        // The toggle: what is left to see, and which way it goes.
        let rect = l.repos_toggle;
        let lit = view.hover == Some(FeatureTabHit::ToggleRepos);
        chip_ground(c, rect, p.border_variant, lit_fill(lit));
        let color = if lit { p.text } else { p.muted };
        let mut icon = rect;
        if let Some(label) = repos_toggle_label(view) {
            let text = Rectangle::new(
                (rect.loc.x + CHIP_PAD, rect.loc.y).into(),
                ((rect.size.w - CHIP_PAD - TOGGLE_ICON).max(1), rect.size.h).into(),
            );
            line(c, &label, text, color);
            icon.loc.x = rect.loc.x + rect.size.w - TOGGLE_ICON;
            icon.size.w = TOGGLE_ICON;
        }
        let up = view.expanded;
        icon_sdf(c, icon, scale, color, |x, y| {
            chevron_down(x, if up { -y } else { y }, s)
        });
    }

    /// The × taking repository `i` out of the feature, in `rect`.
    fn repo_remove(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        i: usize,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let color = if view.hover == Some(FeatureTabHit::RepoRemove(i)) {
            p.text
        } else {
            p.muted
        };
        let s = scale as f32;
        icon_sdf(c, rect, scale, color, |x, y| cross(x, y, 0.8 * s));
    }

    /// A repository's dot in `spot`: on the branch in the success colour,
    /// context a ring, else a quiet one. One being touched blinks: it is
    /// composed over the tab (see `feature_tab_blinkers`).
    fn repo_dot(&self, c: &mut Canvas, spot: Rect, repo: &FeatureRepoRow, scale: f64, p: &Palette) {
        if repo.busy {
            return;
        }
        let d = spot.size.w as f32 * scale as f32;
        let solid = |x: f32, y: f32| x.hypot(y) - d / 2.0;
        match repo.kind {
            RepoKind::Active | RepoKind::Other => {
                icon_sdf(c, spot, scale, repo.kind.color(p), solid)
            }
            RepoKind::Context => {
                let line = 1.5 * scale as f32;
                icon_sdf(c, spot, scale, repo.kind.color(p), |x, y| {
                    (x.hypot(y) - (d - line) / 2.0).abs() - line / 2.0
                });
            }
        }
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
        off: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let at = |r: Rect| Rectangle::new(r.loc - off, r.size);
        let line = |c: &mut Canvas, text: &str, r: Rect, size: f32, color: Rgb| {
            let (x, end) = phys(r, scale);
            self.text(c, text, size, x, end, band(r, scale), scale, color)
        };
        match piece {
            Piece::Caption(text) => {
                line(c, text, rect, DETAIL_FONT_SIZE, p.muted);
            }
            Piece::GroupCaption(k, text) => {
                line(c, text, rect, DETAIL_FONT_SIZE, p.muted);
                let Some(PageSection::Milestones { buttons, .. }) = view.page.sections.get(*k)
                else {
                    return;
                };
                for &i in buttons {
                    if let Some(r) = l.page_buttons.get(i).copied().flatten() {
                        self.page_button(c, view, i, at(r), scale, p);
                    }
                }
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
            Piece::ItemMark(mark) => {
                let spot = Rectangle::new((rect.loc.x, rect.loc.y + 4).into(), (14, 14).into());
                match mark {
                    ItemMark::Bullet => {
                        icon_sdf(c, spot, scale, p.muted, |x, y| x.hypot(y) - 2.0 * s)
                    }
                    ItemMark::Task(true) => {
                        icon_sdf(c, spot, scale, p.success, |x, y| check(x, y, s));
                    }
                    ItemMark::Task(false) => icon_sdf(c, spot, scale, p.muted, |x, y| {
                        stage_dot(StageMark::Later, x, y, 9.0 * s)
                    }),
                    ItemMark::Number(n) => {
                        line(c, &number_mark(*n), rect, FONT_SIZE, p.muted);
                    }
                    ItemMark::Under => {}
                }
            }
            Piece::CodeLine(text) => self.code_line(c, rect, text, p.element, scale, p),
            Piece::OutputLine(text) => {
                let ground = mix(p.background, p.text, OUTPUT_GROUND);
                self.code_line(c, rect, text, ground, scale, p);
            }
            Piece::YourComment => {
                let (x, y, w, h) = rect_px(rect, scale);
                let r = AGENT_RADIUS * s;
                c.rounded(x, y, w, h, r, comment_ground(p));
                c.clipped(
                    x as i32,
                    y as i32,
                    (2.0 * s).round().max(1.0) as i32,
                    h as i32,
                    |c| {
                        c.rounded(x, y, w, h, r, comment_stripe(p));
                    },
                );
            }
            Piece::QuoteBar => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, w / 2.0, p.border);
            }
            Piece::Rule => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.fill(
                    x as i32,
                    (y + h / 2.0) as i32,
                    w as i32,
                    s.ceil() as i32,
                    p.border,
                );
            }
            Piece::Milestone {
                done,
                next,
                running,
                text,
            } => {
                let mark = milestone_mark(rect);
                let (stage, color) = match (done, next) {
                    (true, _) => (StageMark::Done, p.success),
                    (false, true) => (StageMark::Current, p.accent),
                    _ => (StageMark::Later, p.muted),
                };
                match stage {
                    // At work it blinks: its dot is composed over the tab.
                    _ if *running => {}
                    StageMark::Done => icon_sdf(c, mark, scale, color, |x, y| check(x, y, s)),
                    _ => icon_sdf(c, mark, scale, color, |x, y| {
                        stage_dot(stage, x, y, 9.0 * s)
                    }),
                }
                let text_rect = Rectangle::new(
                    (rect.loc.x + DOT_ROOM + 4, rect.loc.y).into(),
                    (rect.size.w - DOT_ROOM - 4, rect.size.h).into(),
                );
                let color = if *done || *next || *running {
                    p.text
                } else {
                    p.muted
                };
                line(c, text, text_rect, SMALL_FONT_SIZE, color);
            }
            Piece::MilestoneMode { fresh } => {
                let Some((track, segments)) = l.milestone_mode.as_ref() else {
                    return;
                };
                let track = at(*track);
                let label = Rectangle::new(
                    (rect.loc.x, track.loc.y).into(),
                    (PICK_LABEL, track.size.h).into(),
                );
                line(c, "Agents", label, SMALL_FONT_SIZE, p.muted);
                let [one, each] = MODE_SEGMENTS;
                let hover = match view.hover {
                    Some(FeatureTabHit::MilestoneMode(i)) => Some(i),
                    _ => None,
                };
                let labelled = [(one, at(segments[0])), (each, at(segments[1]))];
                self.segmented(c, track, &labelled, usize::from(*fresh), hover, scale, p);
            }
            Piece::ShipRoute(k) => {
                let Some(PageSection::ShipGroup(group)) = view.page.sections.get(*k) else {
                    return;
                };
                let Some(route) = &group.route else {
                    return;
                };
                let Some((_, track, segments)) = (l.ship.routes.iter()).find(|r| r.0 == route.row)
                else {
                    return;
                };
                // Around the choice alone: wider, it reads as a wider track.
                let around = at(*track);
                let around = Rectangle::new(
                    (around.loc.x - PICK_TRACK_PAD, around.loc.y - PICK_TRACK_PAD).into(),
                    (
                        around.size.w + 2 * PICK_TRACK_PAD,
                        around.size.h + 2 * PICK_TRACK_PAD,
                    )
                        .into(),
                );
                let selected = group.selected == Some(route.row);
                self.row_highlight(c, around, (selected, 0.0), scale, p);
                let hover = match view.hover {
                    Some(FeatureTabHit::ShipRoute(row, i)) if row == route.row => Some(i),
                    _ => None,
                };
                let labelled: Vec<(&str, Rect)> = (SHIP_ROUTES.iter().copied())
                    .zip(segments.iter().map(|r| at(*r)))
                    .collect();
                self.segmented(c, at(*track), &labelled, route.on, hover, scale, p);
            }
            Piece::ShipStep(k, j) => {
                let Some(PageSection::ShipGroup(group)) = view.page.sections.get(*k) else {
                    return;
                };
                let Some(step) = group.steps.get(*j) else {
                    return;
                };
                let hovered = view.hover == Some(FeatureTabHit::ShipRow(step.row));
                let selected = group.selected == Some(step.row);
                let lit = if hovered { 1.0 } else { 0.0 };
                self.row_highlight(c, rect, (selected, lit), scale, p);
                let tick = Rectangle::new(
                    (rect.loc.x + SHIP_STEP_INSET, rect.loc.y).into(),
                    (CHECKBOX_SIZE, CHECKBOX_ROW).into(),
                );
                let state = if step.ticked { Check::On } else { Check::Off };
                self.checkbox(c, tick, state, hovered, scale, p);
                let text_x = tick.loc.x + CHECKBOX_SIZE + CHECKBOX_GAP;
                let text_w = (rect.loc.x + rect.size.w - text_x - SHIP_STEP_INSET).max(1);
                let label =
                    Rectangle::new((text_x, rect.loc.y).into(), (text_w, CHECKBOX_ROW).into());
                let ink = if step.off { p.muted } else { p.text };
                line(c, &step.label, label, FONT_SIZE, ink);
                if let Some(detail) = &step.detail {
                    let under = Rectangle::new(
                        (text_x, rect.loc.y + CHECKBOX_ROW).into(),
                        (text_w, SMALL_LINE).into(),
                    );
                    line(
                        c,
                        &detail.text,
                        under,
                        SMALL_FONT_SIZE,
                        detail.tone.color(p),
                    );
                }
            }
            Piece::Agent(k) => {
                let Some(PageSection::Agent {
                    icon,
                    name,
                    dot,
                    status,
                    tail,
                    buttons,
                    picks,
                    model,
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
                        // One that can't be picked stands well back.
                        let tint = match chip.off && !chip.lit {
                            true => mix(p.element, p.placeholder, PICK_OFF),
                            false => segment_tint(i, on, &off, p),
                        };
                        agent_icon(c, *rect, PICK_ICON, chip.icon, tint, scale);
                    }
                }
                let model_spot = l.agent_model.filter(|_| model.is_some()).map(at);
                if let (Some(label), Some(r)) = (model, model_spot) {
                    self.agent_model_button(c, view, label, r, scale, p);
                }
                let held: Vec<Rect> = (buttons.iter())
                    .filter_map(|i| l.page_buttons.get(*i).copied().flatten())
                    .map(at)
                    .chain(model_spot)
                    .collect();
                for (i, r) in (buttons.iter())
                    .filter_map(|i| Some((*i, l.page_buttons.get(*i).copied().flatten()?)))
                {
                    self.page_button(c, view, i, at(r), scale, p);
                }
                if name.is_empty() {
                    return;
                }
                let icon_spot = Rectangle::new(head.loc, (16, AGENT_HEAD).into());
                agent_icon(c, icon_spot, 16, *icon, p.text, scale);
                // A dot at work blinks: it is composed over the tab.
                status_dot(c, agent_card_dot(rect), *dot, scale, p);
                let track = (!picks.is_empty())
                    .then_some(l.agent_picks.as_ref())
                    .flatten()
                    .map(|(track, _)| at(*track));
                // What shares the head's row ends its text.
                let end_x = (held.iter().chain(&track))
                    .filter(|r| r.loc.y < head.loc.y + AGENT_HEAD)
                    .map(|r| r.loc.x - QUIT_BUTTON_GAP)
                    .min()
                    .unwrap_or(head.loc.x + head.size.w);
                let text = Rectangle::new(
                    (head.loc.x + AGENT_TEXT_AT, head.loc.y).into(),
                    ((end_x - head.loc.x - AGENT_TEXT_AT).max(1), AGENT_HEAD).into(),
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
                // From the card's foot: rows of buttons may come before it.
                let tail_top = rect.loc.y + rect.size.h - AGENT_PAD - tail.len() as i32 * TAIL_LINE;
                for (j, tail_line) in tail.iter().enumerate() {
                    let r = Rectangle::new(
                        (head.loc.x, tail_top + j as i32 * TAIL_LINE).into(),
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
            }
            Piece::Repo(i) => {
                let Some(repo) = view.repos.get(*i) else {
                    return;
                };
                let selected = view.focus == TabArea::Repos && view.repo == *i;
                let lit = matches!(
                    view.hover,
                    Some(FeatureTabHit::Repo(r) | FeatureTabHit::RepoButton(r, _) | FeatureTabHit::RepoRemove(r))
                        if r == *i
                );
                self.row_highlight(c, rect, (selected, if lit { 1.0 } else { 0.0 }), scale, p);
                let buttons = l.repo_buttons.get(*i).map_or(&[][..], Vec::as_slice);
                let remove = l.repo_removes.get(*i).copied().flatten().map(at);
                let first = buttons.iter().find(|b| b.size.w > 0).copied().map(at);
                let end = first
                    .or(remove)
                    .map_or(rect.loc.x + rect.size.w - 4, |b| b.loc.x - QUIT_BUTTON_GAP);
                let left = rect.loc.x + DOT_ROOM + 4;
                let y = rect.loc.y + (rect.size.h - 20) / 2;
                // The lines it changes, right of its name and line, when
                // they leave the name room.
                let stat = Rectangle::new(
                    (left + STAT_NAME_MIN, y).into(),
                    ((end - left - STAT_NAME_MIN).max(0), 20).into(),
                );
                let text_end = match self.repo_stat(c, repo.stat, stat, scale, p) {
                    Some(from) => from - STAT_GAP,
                    None => end,
                };
                self.repo_dot(c, repo_row_dot(rect), repo, scale, p);
                let detail = if repo.busy {
                    format!("{} \u{b7} touching\u{2026}", repo.line)
                } else {
                    repo.line.clone()
                };
                // The name whole if the line then still fits, else its share.
                let room = (text_end - left).max(1);
                let name_w = width_of(&repo.name, FONT_SIZE).ceil() as i32 + 1;
                let line_w = width_of(&detail, DETAIL_FONT_SIZE).ceil() as i32 + 1;
                let name_w = match name_w + PLACE_GAP + line_w <= room {
                    true => name_w,
                    false => name_w.min(room * 45 / 100),
                };
                let name = Rectangle::new((left, y).into(), (name_w.max(1), 20).into());
                let color = match repo.kind {
                    RepoKind::Active => p.text,
                    _ => p.muted,
                };
                line(c, &repo.name, name, FONT_SIZE, color);
                let from = left + name_w + PLACE_GAP;
                if text_end - from >= REPO_LINE_MIN {
                    let place = Rectangle::new((from, y).into(), ((text_end - from), 20).into());
                    line(c, &detail, place, DETAIL_FONT_SIZE, p.muted);
                }
                for (b, (spot, label)) in buttons.iter().zip(&repo.buttons).enumerate() {
                    if spot.size.w <= 0 {
                        continue;
                    }
                    let hovered = view.hover == Some(FeatureTabHit::RepoButton(*i, b));
                    let style = match selected && view.picked == Some(b) {
                        true => ButtonStyle::Primary,
                        false => ButtonStyle::Plain,
                    };
                    self.button(c, label, at(*spot), (hovered, false), style, scale, p);
                }
                if let Some(b) = remove {
                    self.repo_remove(c, view, *i, b, scale, p);
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
            Piece::FindingCard(k) => {
                let Some(PageSection::Finding(card)) = view.page.sections.get(*k) else {
                    return;
                };
                if card.sheet.is_some() {
                    let n = (view.page.sections[..*k].iter())
                        .filter(|s| matches!(s, PageSection::Finding(_)))
                        .count();
                    let hovered = matches!(view.hover,
                        Some(FeatureTabHit::Finding(i) | FeatureTabHit::Fold(i)) if i == n);
                    let lit = (card.selected, if hovered { 1.0 } else { 0.0 });
                    self.row_highlight(c, rect, lit, scale, p);
                    // The rule under it, across the row.
                    let (x, y, w, h) = rect_px(rect, scale);
                    let b = (scale as f32).round().max(1.0);
                    c.rounded(x, y + h - b, w, b, 0.0, p.border);
                    for &i in &card.buttons {
                        if let Some(r) = l.page_buttons.get(i).copied().flatten() {
                            self.page_button(c, view, i, at(r), scale, p);
                        }
                    }
                    return;
                }
                card_frame(c, rect, scale, p);
                // Inside the frame: the column cuts off what goes past it.
                if card.selected {
                    let (x, y, w, h) = rect_px(rect, scale);
                    let (s, b) = (scale as f32, (2.0 * scale as f32).round());
                    let r = AGENT_RADIUS * s;
                    c.rounded(x, y, w, h, r, p.accent);
                    c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.background);
                }
                if let Some(stripe) = card.stripe {
                    let (x, y, _, h) = rect_px(rect, scale);
                    let s = scale as f32;
                    let colour = match stripe {
                        CardStripe::ToFix => p.accent,
                        CardStripe::Sent => p.muted,
                    };
                    let inset = AGENT_RADIUS * s;
                    let (bar, edge) = ((3.0 * s).round(), (2.0 * s).round());
                    let high = (h - 2.0 * inset).max(0.0);
                    c.rounded(x + edge, y + inset, bar, high, bar / 2.0, colour);
                }
                for &i in &card.buttons {
                    if let Some(r) = l.page_buttons.get(i).copied().flatten() {
                        self.page_button(c, view, i, at(r), scale, p);
                    }
                }
            }
            Piece::RoundCard(k) => {
                let Some(PageSection::Round(card)) = view.page.sections.get(*k) else {
                    return;
                };
                card_frame(c, rect, scale, p);
                for &i in &card.buttons {
                    if let Some(r) = l.page_buttons.get(i).copied().flatten() {
                        self.page_button(c, view, i, at(r), scale, p);
                    }
                }
            }
            Piece::Fold(k) => {
                let Some(PageSection::Finding(card)) = view.page.sections.get(*k) else {
                    return;
                };
                let open = card.sheet.as_ref().is_some_and(|sheet| sheet.open);
                let n = (view.page.sections[..*k].iter())
                    .filter(|s| matches!(s, PageSection::Finding(_)))
                    .count();
                let color = match view.hover == Some(FeatureTabHit::Fold(n)) {
                    true => p.text,
                    false => p.muted,
                };
                icon_sdf(c, rect, scale, color, |x, y| match open {
                    true => chevron_down(x, y, 1.0 * s),
                    false => chevron_side(false, x, y, 1.0 * s),
                });
            }
            Piece::Picked {
                text,
                font,
                bold,
                range,
            } => {
                let (x, end) = phys(rect, scale);
                let (y, h) = band(rect, scale);
                let size = font.size();
                let width = |t: &str| match bold {
                    true => self.bold_width(t, size, scale),
                    false => self.text_width(t, size, scale),
                };
                let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(b, _)| b);
                let x0 = x + width(&text[..byte(range.start)]);
                let x1 = (x + width(&text[..byte(range.end)])).min(end);
                if x1 > x0 {
                    let color = mix(p.background, p.accent, PICKED_TINT);
                    c.rounded(x0, y as f32, x1 - x0, h as f32, 2.0 * s, color);
                }
            }
            Piece::Found {
                text,
                font,
                ink,
                bold,
                needle,
                whole,
                current,
            } => {
                let (x, end) = phys(rect, scale);
                let b = band(rect, scale);
                let size = font.size();
                let mut found = found_ranges(text, needle, *whole);
                if let Some(at) = (*current).and_then(|n| found.get_mut(n)) {
                    at.1 = true;
                }
                let width = |t: &str| self.text_width(t, size, scale);
                self.found_tints(c, text, &found, (x, end), b, width, p.background, scale, p);
                if *bold {
                    self.bold_text(c, text, size, x, end, b, scale, ink.color(p));
                } else {
                    self.text(c, text, size, x, end, b, scale, ink.color(p));
                }
            }
            Piece::CardField(k) => {
                let Some(PageSection::Finding(card)) = view.page.sections.get(*k) else {
                    return;
                };
                let Some(text) = &card.field else {
                    return;
                };
                let hint = card.field_hint;
                self.field_of_lines(c, text, hint, rect, true, scale, p);
            }
            // A dot at work blinks: it is composed over the tab.
            Piece::CheckDot(dot) => status_dot(c, rect, *dot, scale, p),
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

    /// A repository's `stat` as "F files +A −R" (files muted, added in
    /// the success colour, removed in the error colour, a count that is 0
    /// left out), from the right of `rect` if it fits in it: where it
    /// starts (in logical pixels), or `None` when nothing is drawn.
    fn repo_stat(
        &self,
        c: &mut Canvas,
        stat: Option<Stat>,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) -> Option<i32> {
        let parts: Vec<(String, Rgb)> = (stat_parts(stat?).into_iter())
            .zip([p.muted, p.success, p.error])
            .filter_map(|(text, color)| Some((text?, color)))
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

    /// A line of a code block in `rect`, on `ground`. On whole pixels, so a
    /// block's lines meet with no seam.
    fn code_line(
        &self,
        c: &mut Canvas,
        rect: Rect,
        text: &str,
        ground: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let (x, y, w, h) = rect_px(rect, scale);
        c.fill(x as i32, y as i32, w as i32, h as i32, ground);
        let pad = (f64::from(DOC_CODE_PAD) * scale) as f32;
        let (from, to) = (x + pad, x + w - pad);
        let b = band(rect, scale);
        self.mono_text(c, text, SMALL_FONT_SIZE, from, to, b, scale, p.text);
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
            &button.label,
            rect,
            (hovered, false),
            button.style,
            scale,
            p,
        );
    }

    /// The button picking an agent's model: its label from the left, as it
    /// may not fit whole.
    fn agent_model_button(
        &self,
        c: &mut Canvas,
        view: &FeatureTabView,
        label: &str,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let hovered = view.hover == Some(FeatureTabHit::AgentModel);
        self.button(c, "", rect, (hovered, false), ButtonStyle::Plain, scale, p);
        let text = Rectangle::new(
            (rect.loc.x + BUTTON_PADDING, rect.loc.y).into(),
            ((rect.size.w - 2 * BUTTON_PADDING).max(1), rect.size.h).into(),
        );
        let (x, end) = phys(text, scale);
        self.text(
            c,
            label,
            FONT_SIZE,
            x,
            end,
            band(text, scale),
            scale,
            p.text,
        );
    }

    /// `card`'s stages as a row of marks in `rect`, from its left, the one
    /// it is at bigger. The one that runs is left out: it blinks over this
    /// (see `sidebar_stage_blinkers`).
    pub(super) fn stage_marks(
        &self,
        c: &mut Canvas,
        card: &FeatureCard,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let n = card.marks.len() as i32;
        debug_assert!(stage_marks_width(n as usize) <= rect.size.w || rect.size.w == 0);
        let big = card.big_dot();
        for (i, mark) in card.marks.iter().enumerate() {
            if card.running_dot() == Some(i) {
                continue;
            }
            let spot = stage_dot_rect(rect, big, i);
            icon_sdf(c, spot, scale, mark.dot_color(p), |x, y| {
                stage_dot(mark.dot_shape(), x, y, stage_dot_across(spot, scale))
            });
        }
    }
}

/// How a feature's current stage stands, for its card's mark and line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CardTone {
    /// Nothing runs and nothing waits.
    #[default]
    Idle,
    /// An agent or a job of it runs.
    Working,
    /// It waits for the user.
    Waits,
    /// It failed or is blocked.
    Failed,
}

impl CardTone {
    /// The colour of the card's stage mark.
    pub(super) fn mark_color(self, p: &Palette) -> Rgb {
        match self {
            Self::Idle => p.placeholder,
            Self::Working => p.accent,
            Self::Waits => p.warning,
            Self::Failed => p.error,
        }
    }

    /// The colour of the card's line.
    pub(super) fn text_color(self, p: &Palette) -> Rgb {
        match self {
            Self::Idle | Self::Working => p.muted,
            Self::Waits => p.warning,
            // The error colour alone is short of 4.5:1 on a hovered row.
            Self::Failed => mix(p.error, p.text, 0.3),
        }
    }
}

/// What a feature's card says of it, on its row in the Features section or
/// on the header of its own workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FeatureCard {
    /// Each stage as the rail of its feature tab reads it.
    pub marks: Vec<NodeMark>,
    /// The stage it is at; none once every stage is done.
    pub stage: Option<Stage>,
    /// How that stage stands, in words: "Build · milestone 2/5".
    pub line: String,
    pub tone: CardTone,
    /// Whether a gate holds it for the user, as the inbox counts them.
    pub gate: bool,
    /// How many repositories it holds.
    pub repos: usize,
}

impl FeatureCard {
    /// Which stage dot has `mark`, the first if several do.
    fn dot_marked(&self, mark: NodeMark) -> Option<usize> {
        self.marks.iter().position(|m| *m == mark)
    }

    /// The stage dot that is big: the stage its icon and line are about,
    /// unless the flow went back from it: then the one the rail pulses.
    pub(super) fn big_dot(&self) -> Option<usize> {
        let stage = (self.stage).and_then(|s| Stage::ALL.iter().position(|a| *a == s));
        let left =
            stage.filter(|k| matches!(self.marks.get(*k), Some(NodeMark::Ran | NodeMark::Paused)));
        self.dot_marked(NodeMark::Fixing)
            .or_else(|| left.and_then(|_| self.dot_marked(NodeMark::Current)))
            .or(stage)
    }

    /// The stage dot that blinks: the big one, while it runs.
    pub(super) fn running_dot(&self) -> Option<usize> {
        self.big_dot().filter(|k| {
            self.tone == CardTone::Working || self.marks.get(*k) == Some(&NodeMark::Fixing)
        })
    }

    /// Whether it shows how many repositories it holds: only with several.
    pub(super) fn shows_repos(&self) -> bool {
        self.repos > 1
    }
}

/// A sidebar row for an open feature that has no workspace.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FeatureRowView {
    pub title: String,
    pub card: FeatureCard,
}

/// Signed distance to the mark of `stage` (none: every stage is done),
/// centred on the origin.
pub(super) fn stage_glyph(stage: Option<Stage>, x: f32, y: f32, s: f32) -> f32 {
    match stage {
        Some(Stage::Discover) => magnifier(x, y, s),
        Some(Stage::Plan) => text_lines(x, y, s),
        Some(Stage::PlanReview) => plan_review_mark(x, y, s),
        Some(Stage::Build) => code_mark(x, y, s),
        Some(Stage::Verify) | None => check(x, y, s),
        Some(Stage::AiReview) => sparkle(x, y, s),
        Some(Stage::CodeReview) => eye(x, y, s),
        Some(Stage::ManualTest) => flask(x, y, s),
        Some(Stage::Ship) => ship_arrow(x, y, s),
    }
}

/// How big a stage dot of a sidebar row is, and the gap between two.
pub(super) const STAGE_DOT: i32 = 8;
pub(super) const STAGE_DOT_GAP: i32 = 2;
/// How big the dot of the stage a feature is at is: it is the one to click.
pub(super) const STAGE_DOT_BIG: i32 = 14;

/// Where stage dot `k` of the dots starting at `dots` goes, with dot `big`
/// the big one.
pub(super) fn stage_dot_rect(dots: Rect, big: Option<usize>, k: usize) -> Rect {
    let across = |i: usize| {
        if big == Some(i) {
            STAGE_DOT_BIG
        } else {
            STAGE_DOT
        }
    };
    let x = (0..k).map(|i| across(i) + STAGE_DOT_GAP).sum::<i32>();
    let d = across(k);
    Rectangle::new(
        (dots.loc.x + x, dots.loc.y + (dots.size.h - d) / 2).into(),
        (d, d).into(),
    )
}

/// How far across a stage dot is drawn in its `spot`, in pixels: a pixel
/// short of it, so its edge stays smooth.
fn stage_dot_across(spot: Rect, scale: f64) -> f32 {
    (spot.size.w - 1) as f32 * scale as f32
}

/// Which of the stage dots starting at `dots`, with dot `big` the big one,
/// is at `x`: the nearest, as the gaps between them belong to them.
pub(super) fn stage_dot_at(dots: Rect, big: Option<usize>, x: f64) -> usize {
    (0..Stage::ALL.len() - 1)
        .find(|k| {
            let r = stage_dot_rect(dots, big, *k);
            x < (r.loc.x + r.size.w) as f64 + STAGE_DOT_GAP as f64 / 2.0
        })
        .unwrap_or(Stage::ALL.len() - 1)
}

/// How wide `marks` stage dots are in a sidebar row, one of them big.
pub(super) fn stage_marks_width(marks: usize) -> i32 {
    if marks == 0 {
        return 0;
    }
    marks as i32 * (STAGE_DOT + STAGE_DOT_GAP) - STAGE_DOT_GAP + STAGE_DOT_BIG - STAGE_DOT
}

/// A running stage's dot on its own, big, on a clear buffer: composed over
/// a sidebar painted without it.
pub(super) fn stage_blink_dot(scale: f64, p: &Palette) -> MemoryRenderBuffer {
    let spot: Rect = Rectangle::from_size((STAGE_DOT_BIG, STAGE_DOT_BIG).into());
    let mut c = Canvas::new(super::physical(spot.size, scale));
    let color = NodeMark::Current.dot_color(p);
    icon_sdf(&mut c, spot, scale, color, |x, y| {
        stage_dot(StageMark::Current, x, y, stage_dot_across(spot, scale))
    });
    c.into_buffer(false)
}

/// Whether each band of `view`'s page column (see `page_bands`) holds the
/// pixels the whole column has there at `scale`, and the column has only
/// its ground between them; for the goldens.
#[cfg(test)]
pub(super) fn bands_match_column(
    painter: &Painter,
    view: &FeatureTabView,
    output: Size<i32, Logical>,
    scale: f64,
) -> Result<usize, String> {
    let bytes = |mut buffer: MemoryRenderBuffer| {
        let mut out = Vec::new();
        let _ = buffer.render().draw(|mem| {
            out.extend_from_slice(mem);
            Ok::<_, ()>(Vec::new())
        });
        out
    };
    let p = super::Theme::Dark.page_palette();
    let l = feature_tab_layout(view, Rectangle::from_size(output));
    let slice = TabSlice::Column(TabColumn::Page);
    let Some(region) = l.slice_rect(slice, 0.0, scale) else {
        return Ok(0);
    };
    let Some(bands) = page_bands(view, &l, (region.loc.x, region.loc.y, region.size.w), scale)
    else {
        return Ok(0);
    };
    let whole = bytes(painter.feature_tab_slice(view, output, slice, region, scale, p));
    let row = (physical(region.size, scale).w * 4) as usize;
    let rows = whole.len() / row;
    let [r, g, b] = p.panel;
    let ground: Vec<u8> = [b, g, r, 0xff].repeat(row / 4);
    let mut covered = vec![false; rows];
    for band in &bands {
        let got = bytes(painter.feature_tab_band(view, &l, band, scale, p));
        let from = (f64::from(band.rect.loc.y - region.loc.y) * scale).round() as usize;
        for (i, line) in got.chunks(row).enumerate() {
            let at = from + i;
            // A tall column is drawn whole only as far as it shows.
            if at >= rows && region.size.h < l.content[TabColumn::Page as usize] {
                break;
            }
            let want = whole.get(at * row..(at + 1) * row).unwrap_or(&ground);
            if line != want {
                return Err(format!("band at {:?}, its row {i}", band.rect));
            }
            if let Some(c) = covered.get_mut(at) {
                *c = true;
            }
        }
    }
    for (at, _) in covered.iter().enumerate().filter(|(_, c)| !**c) {
        if whole[at * row..(at + 1) * row] != ground[..] {
            return Err(format!("row {at} between bands"));
        }
    }
    Ok(bands.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::sidebar::feature_dots_rect as feature_row_dots;
    use crate::chrome::{
        FIELD_LINE, ONE_DARK, ONE_LIGHT, ONE_LIGHT_PAGE, contrast, field_line_spot, inside,
    };

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

    /// `n` feature rows with nothing on their cards.
    fn plain_rows(n: usize) -> Vec<FeatureRowView> {
        vec![
            FeatureRowView {
                title: "A feature".into(),
                card: FeatureCard::default(),
            };
            n
        ]
    }

    #[test]
    fn feature_rows_sit_under_the_workspaces_and_scroll_with_them() {
        use crate::chrome::{Density, SidebarHit, sidebar_layout_at};
        let base = sidebar_layout_at(&[], &[], 260, Density::Comfortable);
        assert!(base.features_title.is_none() && base.features.is_empty());
        let layout = sidebar_layout_at(&[], &[], 260, Density::Comfortable).with_features(
            &plain_rows(2),
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
                ws.feature = Some(FeatureCard {
                    marks: vec![NodeMark::Done, NodeMark::Current],
                    ..FeatureCard::default()
                });
            }
            ws
        };
        let d = Density::Comfortable;
        let workspaces = [ws("a", false), ws("b", true), ws("c", true)];
        let layout =
            sidebar_layout_at(&workspaces, &[], 260, d).with_features(&plain_rows(1), 260, d);
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
    fn a_feature_card_keeps_its_dots_where_its_name_starts() {
        use crate::WorkspaceKind;
        use crate::chrome::{Density, SIDEBAR_MIN_WIDTH, WorkspaceView, sidebar_layout_at};
        let d = Density::Comfortable;
        let mut ws = WorkspaceView::new("Bulk".into(), WorkspaceKind::Terminal, &[]);
        ws.branch = Some("feature/bulk".into());
        ws.feature = Some(FeatureCard {
            marks: vec![NodeMark::Later; Stage::ALL.len()],
            repos: 3,
            ..FeatureCard::default()
        });
        let layout = sidebar_layout_at(std::slice::from_ref(&ws), &[], 200, d);
        let row = &layout.rows[0];
        let (name, dots) = (row.name, row.stages.expect("its dots"));
        assert_eq!(dots.loc.x, name.loc.x);
        // Its branch comes from its name, so its line holds only the rest.
        assert_eq!(row.branch, None);
        let count = row.repos.expect("its count of repositories");
        assert!(count.loc.x >= dots.loc.x + dots.size.w);
        // The line under the dots, within the header.
        let info = row.info.expect("its line");
        assert_eq!(info.loc.y, dots.loc.y + dots.size.h);
        assert_eq!(info.loc.y + info.size.h, row.rect.loc.y + row.rect.size.h);
        // Too narrow for the count, the dots stay and the count goes.
        let narrow = sidebar_layout_at(std::slice::from_ref(&ws), &[], SIDEBAR_MIN_WIDTH, d);
        assert_eq!(narrow.rows[0].stages.map(|r| r.loc.x), Some(dots.loc.x));
        assert_eq!(narrow.rows[0].repos, None);
    }

    #[test]
    fn the_features_rule_offers_to_start_a_feature() {
        use crate::WorkspaceKind;
        use crate::chrome::{Density, SidebarHit, WorkspaceView, sidebar_layout_at};
        let d = Density::Comfortable;
        let ws = WorkspaceView::new("api".into(), WorkspaceKind::Terminal, &[]);
        let plain = || sidebar_layout_at(std::slice::from_ref(&ws), &[], 200, d);
        let end = plain().max_scroll(0);
        let mid = |r: Rect| (r.loc.x as f64 + 8.0, r.loc.y as f64 + 8.0).into();
        // No feature yet: the heading shows, and counts toward scrolling.
        let empty = || {
            plain()
                .with_features(&[], 200, d)
                .with_feature_head(true, 0, 200)
        };
        let scrolled = empty().scrolled(4, 0);
        let empty = empty();
        assert!(empty.features.is_empty());
        let title = empty.features_title.expect("a heading");
        let plus = empty.features_plus.expect("a plus");
        assert_eq!(empty.hit(mid(plus)), Some(SidebarHit::NewFeature));
        assert_eq!(empty.hit(mid(title)), Some(SidebarHit::FeaturesTitle));
        assert!(empty.max_scroll(0) > end);
        // On the rule: just over the heading, and inside the sidebar.
        assert_eq!(plus.loc.y + plus.size.h, title.loc.y);
        assert!(plus.loc.x + plus.size.w <= 200);
        // The empty sidebar has only its hint: no project to start one in.
        let none = sidebar_layout_at(&[], &[], 200, d)
            .with_features(&[], 200, d)
            .with_feature_head(false, 0, 200);
        assert!(none.features_title.is_none() && none.features_plus.is_none());
        // It moves with the rows.
        let moved = scrolled.features_plus.expect("a plus");
        assert_eq!(moved.loc.y, plus.loc.y - 4);
    }

    #[test]
    fn the_features_heading_counts_what_waits_beside_the_label_only() {
        use crate::WorkspaceKind;
        use crate::chrome::{
            Density, SIDEBAR_MIN_WIDTH, SidebarHit, WorkspaceView, features_waiting,
            sidebar_layout_at,
        };
        let d = Density::Comfortable;
        let waits = FeatureCard {
            gate: true,
            ..FeatureCard::default()
        };
        let mut on_workspace = WorkspaceView::new("api".into(), WorkspaceKind::Terminal, &[]);
        on_workspace.feature = Some(waits.clone());
        let rows = [
            FeatureRowView {
                title: "a".into(),
                card: waits,
            },
            FeatureRowView {
                title: "b".into(),
                card: FeatureCard::default(),
            },
        ];
        let workspaces = [on_workspace];
        assert_eq!(features_waiting(&workspaces, &rows), 2);
        let layout = sidebar_layout_at(&workspaces, &[], 200, d)
            .with_features(&plain_rows(2), 200, d)
            .with_feature_head(true, 2, 200);
        let pill = layout.features_waiting.expect("a pill");
        let mid = (pill.loc.x as f64 + 2.0, pill.loc.y as f64 + 8.0).into();
        assert_eq!(layout.hit(mid), Some(SidebarHit::FeaturesWaiting));
        let quiet = sidebar_layout_at(&workspaces, &[], 200, d)
            .with_features(&plain_rows(2), 200, d)
            .with_feature_head(true, 0, 200);
        assert_eq!(quiet.features_waiting, None);
        let narrow = sidebar_layout_at(&workspaces, &[], SIDEBAR_MIN_WIDTH, d)
            .with_features(&plain_rows(2), SIDEBAR_MIN_WIDTH, d)
            .with_feature_head(true, 2, SIDEBAR_MIN_WIDTH);
        assert_eq!(narrow.features_waiting, None, "it never covers the label");
    }

    #[test]
    fn the_strip_of_agents_leaves_a_feature_cards_lines_alone() {
        use crate::agent_icons::AgentIcon;
        use crate::chrome::{Density, WorkspaceView, sidebar_layout_at};
        let mut ws = WorkspaceView::new("Bulk".into(), crate::WorkspaceKind::Terminal, &[]);
        ws.picks = vec![AgentIcon::Claude];
        ws.feature = Some(FeatureCard {
            marks: vec![NodeMark::Later; Stage::ALL.len()],
            ..FeatureCard::default()
        });
        let layout = sidebar_layout_at(std::slice::from_ref(&ws), &[], 200, Density::Comfortable);
        let (mut placed, height) = (layout, 600);
        placed.place_strips(0, height);
        let row = &placed.rows[0];
        let info = row.info.expect("its line");
        assert_eq!(
            row.picks.expect("its strip").loc.y,
            info.loc.y + info.size.h
        );
    }

    #[test]
    fn a_click_on_the_stage_dots_lands_on_the_nearest() {
        use crate::chrome::{Density, SidebarHit, WorkspaceView, sidebar_layout_at};
        let d = Density::Comfortable;
        let mut ws = WorkspaceView::new("Bulk".into(), crate::WorkspaceKind::Terminal, &[]);
        ws.feature = Some(FeatureCard {
            marks: vec![NodeMark::Later; Stage::ALL.len()],
            ..FeatureCard::default()
        });
        let layout = sidebar_layout_at(std::slice::from_ref(&ws), &[], 200, d)
            .with_features(&plain_rows(1), 200, d)
            .with_feature_head(true, 0, 200);
        let pitch = STAGE_DOT + STAGE_DOT_GAP;
        let on = |dots: Rect, k: i32| {
            let x = dots.loc.x + k * pitch + STAGE_DOT / 2;
            (x as f64, (dots.loc.y + dots.size.h / 2) as f64).into()
        };
        let dots = layout.rows[0].stages.expect("its dots");
        for k in 0..Stage::ALL.len() {
            let hit = layout.hit(on(dots, k as i32));
            assert_eq!(hit, Some(SidebarHit::Stage(0, k)));
            assert_eq!(hit.and_then(SidebarHit::row), Some(0));
        }
        // A row's dots light the row, and its other parts stay the row.
        let row = layout.features[0];
        let dots = feature_row_dots(row);
        for k in 0..Stage::ALL.len() {
            let hit = layout.hit(on(dots, k as i32)).expect("a dot");
            assert_eq!(hit, SidebarHit::FeatureDot(0, k));
            assert_eq!(hit.pill(), SidebarHit::Feature(0));
        }
        let title = (row.loc.x as f64 + 40.0, row.loc.y as f64 + 4.0).into();
        assert_eq!(layout.hit(title), Some(SidebarHit::Feature(0)));
    }

    #[test]
    fn a_feature_card_only_counts_several_repositories() {
        let card = |repos| FeatureCard {
            repos,
            ..FeatureCard::default()
        };
        assert!(!card(1).shows_repos());
        assert!(card(2).shows_repos());
    }

    #[test]
    fn a_feature_card_reads_on_the_panels_in_both_themes() {
        use crate::chrome::{mix, sidebar::ACTIVE_TINT};
        let tones = [
            CardTone::Idle,
            CardTone::Working,
            CardTone::Waits,
            CardTone::Failed,
        ];
        for p in [&ONE_DARK, &ONE_LIGHT] {
            let active = mix(p.panel, p.selected, ACTIVE_TINT);
            for tone in tones {
                for ground in [p.panel, p.element, active] {
                    let mark = contrast(tone.mark_color(p), ground);
                    assert!(mark >= 3.0, "{tone:?} mark at {mark:.2}:1");
                    let text = contrast(tone.text_color(p), ground);
                    assert!(text >= 4.5, "{tone:?} text at {text:.2}:1");
                }
            }
        }
    }

    #[test]
    fn the_dot_of_the_stage_a_feature_is_at_is_the_big_target() {
        use crate::chrome::{Density, SidebarHit, sidebar_layout_at};
        let d = Density::Comfortable;
        let rows = [FeatureRowView {
            title: "A feature".into(),
            card: FeatureCard {
                marks: vec![NodeMark::Later; Stage::ALL.len()],
                stage: Some(Stage::Build),
                ..FeatureCard::default()
            },
        }];
        let big = rows[0].card.big_dot().expect("a stage");
        let layout = sidebar_layout_at(&[], &[], 200, d).with_features(&rows, 200, d);
        let dots = feature_row_dots(layout.features[0]);
        // Every dot takes a click anywhere on it, past the big one too.
        for k in 0..Stage::ALL.len() {
            let r = stage_dot_rect(dots, Some(big), k);
            for x in [r.loc.x, r.loc.x + r.size.w - 1] {
                let at = (x as f64, (dots.loc.y + dots.size.h / 2) as f64).into();
                assert_eq!(layout.hit(at), Some(SidebarHit::FeatureDot(0, k)));
            }
        }
        // It fits its line, and is much wider than the others.
        let r = stage_dot_rect(dots, Some(big), big);
        assert_eq!((r.size.w, r.size.h), (STAGE_DOT_BIG, STAGE_DOT_BIG));
        assert!(r.loc.y >= dots.loc.y && r.loc.y + r.size.h <= dots.loc.y + dots.size.h);
        let last = stage_dot_rect(dots, Some(big), Stage::ALL.len() - 1);
        assert_eq!(last.loc.x + last.size.w, dots.loc.x + dots.size.w);
    }

    #[test]
    fn only_the_dot_of_a_stage_that_runs_blinks() {
        use crate::chrome::{Density, SidebarView, WorkspaceView, sidebar_layout_at};
        let d = Density::Comfortable;
        let card = |tone| FeatureCard {
            marks: vec![NodeMark::Later; Stage::ALL.len()],
            stage: Some(Stage::Verify),
            tone,
            ..FeatureCard::default()
        };
        // On a row and on a feature workspace's header alike.
        let view = |tone: CardTone| {
            let mut ws = WorkspaceView::new("Bulk".into(), crate::WorkspaceKind::Terminal, &[]);
            ws.feature = Some(card(tone));
            SidebarView {
                workspaces: vec![ws],
                agents: Vec::new(),
                active: None,
                project: None,
                header_lit: false,
                features: vec![FeatureRowView {
                    title: "Row".into(),
                    card: card(tone),
                }],
                state: Default::default(),
            }
        };
        let blinkers =
            |view: &SidebarView| {
                let layout = sidebar_layout_at(&view.workspaces, &view.agents, 200, d)
                    .with_features(&view.features, 200, d);
                let rects = crate::chrome::sidebar_stage_blinkers(view, &layout);
                (layout, rects)
            };
        for tone in [CardTone::Idle, CardTone::Waits, CardTone::Failed] {
            let view = view(tone);
            assert!(blinkers(&view).1.is_empty(), "{tone:?}");
            assert!(!view.moves(), "{tone:?}");
        }
        let view = view(CardTone::Working);
        assert!(view.moves(), "the frames keep coming for the blink");
        let (layout, rects) = blinkers(&view);
        let k = Stage::ALL.iter().position(|s| *s == Stage::Verify);
        let header = layout.rows[0].stages.expect("its dots");
        assert_eq!(
            rects,
            vec![
                stage_dot_rect(header, k, k.unwrap()),
                stage_dot_rect(feature_row_dots(layout.features[0]), k, k.unwrap()),
            ]
        );
        // Every stage done: no stage to be big or to blink.
        let done = FeatureCard {
            marks: vec![NodeMark::Done; Stage::ALL.len()],
            tone: CardTone::Working,
            ..FeatureCard::default()
        };
        assert_eq!((done.big_dot(), done.running_dot()), (None, None));
        // Turned back, the dots follow the rail: the one it pulses is big
        // and blinks, not the stage that sent the work back.
        let mut marks = vec![NodeMark::Later; Stage::ALL.len()];
        let at = |stage| Stage::ALL.iter().position(|s| *s == stage).unwrap();
        let (build, review) = (at(Stage::Build), at(Stage::AiReview));
        marks[build] = NodeMark::Fixing;
        marks[review] = NodeMark::Paused;
        let fixing = FeatureCard {
            marks: marks.clone(),
            stage: Some(Stage::AiReview),
            tone: CardTone::Failed,
            ..FeatureCard::default()
        };
        assert_eq!(
            (fixing.big_dot(), fixing.running_dot()),
            (Some(build), Some(build))
        );
        marks[build] = NodeMark::Current;
        marks[review] = NodeMark::Paused;
        let cleared = FeatureCard {
            marks: marks.clone(),
            stage: Some(Stage::AiReview),
            tone: CardTone::Working,
            ..FeatureCard::default()
        };
        assert_eq!(
            (cleared.big_dot(), cleared.running_dot()),
            (Some(build), Some(build))
        );
        // At rest, waiting on the stage that asked: that one stays big.
        marks[build] = NodeMark::Ran;
        marks[review] = NodeMark::Waiting;
        let waits = FeatureCard {
            marks,
            stage: Some(Stage::AiReview),
            tone: CardTone::Waits,
            ..FeatureCard::default()
        };
        assert_eq!((waits.big_dot(), waits.running_dot()), (Some(review), None));
    }

    #[test]
    fn stage_marks_read_on_the_panels_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT, &ONE_LIGHT_PAGE] {
            for mark in [
                NodeMark::Done,
                NodeMark::Current,
                NodeMark::Later,
                NodeMark::Stale,
                NodeMark::Waiting,
                NodeMark::Fixing,
                NodeMark::Ran,
                NodeMark::Paused,
            ] {
                for ground in [p.panel, p.elevated, p.selected] {
                    let ratio = contrast(mark.dot_color(p), ground);
                    assert!(ratio >= 3.0, "{mark:?} at {ratio:.2}:1");
                }
            }
        }
    }

    #[test]
    fn the_ship_check_reads_on_the_panels_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT, &ONE_LIGHT_PAGE] {
            for tone in [CheckTone::Blocker, CheckTone::Ready, CheckTone::Note] {
                for ground in [p.panel, p.elevated] {
                    let ratio = contrast(tone.color(p), ground);
                    assert!(ratio >= 4.5, "{tone:?} at {ratio:.2}:1");
                }
            }
        }
    }

    #[test]
    fn a_checks_dot_and_output_read_on_its_card_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT_PAGE] {
            for tag in [
                FindingTag::Passed,
                FindingTag::Failed,
                FindingTag::Running,
                FindingTag::Queued,
                FindingTag::Stopped,
                FindingTag::NotRun,
                FindingTag::Skipped,
            ] {
                let dot = tag.dot().expect("a check has a dot");
                let ratio = contrast(dot.color(p), p.background);
                assert!(ratio >= 3.0, "{tag:?} at {ratio:.2}:1");
            }
            let ground = mix(p.background, p.text, OUTPUT_GROUND);
            let ratio = contrast(p.text, ground);
            assert!(ratio >= 4.5, "output at {ratio:.2}:1");
        }
    }

    // ---------------------------------------------------------- feature tab

    /// The golden fixture: Build running, three repositories, the second
    /// of two features.
    fn tab_view() -> FeatureTabView {
        crate::chrome::golden::feature_tab()
    }

    #[test]
    fn a_review_on_the_page_takes_all_under_the_rail() {
        let mut view = super::super::golden::feature_tab();
        let plain = tab_at(&view, 1180);
        assert!(plain.review.is_none());
        view.review_page = true;
        view.hints.clear();
        let l = tab_at(&view, 1180);
        let review = l.review.expect("the review's place");
        // Under the rail, over all the rest of the tab.
        let rail_end = l.nodes.iter().map(|n| n.loc.y + n.size.h).max().unwrap();
        assert!(review.loc.y >= rail_end);
        assert_eq!(review.size.w, 1180);
        assert_eq!(review.loc.y + review.size.h, l.frame.loc.y + l.frame.size.h);
        // No columns, chips or toggle left to press under it.
        assert!(l.columns.iter().all(Option::is_none));
        assert_eq!(l.strip.size.h, 0);
        assert_eq!(l.repos_toggle.size.w, 0);
        assert!(l.repos.iter().all(|r| r.size.w == 0));
        // The rail still works.
        assert_eq!(
            l.hit(&view, centre(l.nodes[2])),
            Some(FeatureTabHit::Node(2))
        );
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
        assert!(wide.columns[TabColumn::Page as usize].is_some());
        assert!(wide.columns[TabColumn::Activity as usize].is_some());
        assert!(!wide.compact);
        let at = tab_at(&view, ACTIVITY_BELOW);
        assert!(at.columns[TabColumn::Activity as usize].is_some());
        let under = tab_at(&view, ACTIVITY_BELOW - 1);
        assert!(under.columns[TabColumn::Activity as usize].is_none());
        assert!(!under.compact);
        assert!(tab_at(&view, COMPACT_BUTTONS_BELOW - 1).compact);
        // The repositories are a line on every width, until they unfold.
        for w in [800, ACTIVITY_BELOW, 1920] {
            let l = tab_at(&view, w);
            assert!(l.columns[TabColumn::Repos as usize].is_none());
            assert!(l.strip.size.w > 0 && l.repos_toggle.size.w > 0);
        }
        // The page keeps its room beside the activity.
        const PAGE_MIN_W: i32 = 420;
        let page = tab_at(&view, ACTIVITY_BELOW).columns[TabColumn::Page as usize].unwrap();
        assert!(page.size.w >= PAGE_MIN_W, "{}", page.size.w);
    }

    /// `tab_view` with its repositories' rows unfolded.
    fn unfolded() -> FeatureTabView {
        let mut view = tab_view();
        view.expanded = true;
        view
    }

    #[test]
    fn the_rows_unfold_across_the_tab_over_the_page() {
        let view = unfolded();
        let l = tab_at(&view, 1920);
        let card = l.columns[TabColumn::Repos as usize].unwrap();
        let page = l.columns[TabColumn::Page as usize].unwrap();
        let activity = l.columns[TabColumn::Activity as usize].unwrap();
        assert_eq!(card.size.w, l.strip.size.w);
        assert!(card.loc.y >= l.strip.loc.y + l.strip.size.h);
        assert!(page.loc.y >= card.loc.y + card.size.h);
        assert_eq!(page.loc.y, activity.loc.y);
        // Folded, the page starts where the card did.
        let folded = tab_at(&tab_view(), 1920);
        assert_eq!(
            folded.columns[TabColumn::Page as usize].unwrap().loc.y,
            card.loc.y
        );
        // The rows don't chip: the strip only holds its toggle.
        assert!(l.more_repos.is_empty());
        assert_eq!(
            l.hit(&view, centre(l.repos_toggle)),
            Some(FeatureTabHit::ToggleRepos)
        );
    }

    #[test]
    fn many_rows_stop_at_their_share_of_the_body_and_scroll() {
        let mut view = unfolded();
        let row = view.repos[0].clone();
        view.repos = vec![row; 40];
        let l = tab_at(&view, 1920);
        let card = l.columns[TabColumn::Repos as usize].unwrap();
        let body = l.hints.loc.y - 4 - card.loc.y;
        assert!(card.size.h <= body * REPOS_CARD_SHARE / 100 + 1);
        assert!(l.scroll_room(TabColumn::Repos) > 0);
        let page = l.columns[TabColumn::Page as usize].unwrap();
        assert!(page.size.h > 100, "{}", page.size.h);
    }

    #[test]
    fn a_rows_buttons_line_up_and_each_hits() {
        let mut view = unfolded();
        view.repos[0].buttons = vec!["Context"];
        view.repos[1].buttons = vec!["Feature", "Context"];
        view.repos[1].remove = false;
        let l = tab_at(&view, 1000);
        let (one, two) = (&l.repo_buttons[0], &l.repo_buttons[1]);
        assert_eq!(one[0].loc.x + one[0].size.w, two[1].loc.x + two[1].size.w);
        assert!(two[0].loc.x + two[0].size.w <= two[1].loc.x);
        assert_eq!(
            l.hit(&view, centre(two[0])),
            Some(FeatureTabHit::RepoButton(1, 0))
        );
        assert_eq!(
            l.hit(&view, centre(two[1])),
            Some(FeatureTabHit::RepoButton(1, 1))
        );
        let remove = l.repo_removes[0].unwrap();
        assert_eq!(
            l.hit(&view, centre(remove)),
            Some(FeatureTabHit::RepoRemove(0))
        );
        assert!(l.repo_removes[1].is_none());
        // A row too narrow for them keeps none, and no overlap.
        let thin = tab_at(&view, 200);
        assert!(thin.repo_buttons[1].iter().all(|b| b.size.w == 0));
    }

    #[test]
    fn the_context_dot_differs_from_the_quiet_one() {
        let painter = crate::chrome::Painter::new();
        let spot: Rect = Rectangle::from_size((REPO_DOT, REPO_DOT).into());
        let pixels = |kind: RepoKind| {
            let mut c = Canvas::new(physical((REPO_DOT, REPO_DOT).into(), 2.0));
            let p = crate::chrome::Theme::Dark.palette();
            c.fill(0, 0, c.w, c.h, p.panel);
            let mut row = tab_view().repos[0].clone();
            row.kind = kind;
            painter.repo_dot(&mut c, spot, &row, 2.0, p);
            c.px.clone()
        };
        assert_ne!(pixels(RepoKind::Context), pixels(RepoKind::Other));
        assert_ne!(pixels(RepoKind::Active), pixels(RepoKind::Other));
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
        // A feature another machine writes has none of them.
        let mut seen = view.clone();
        seen.remove = "";
        assert!(tab_at(&seen, 1920).buttons.is_empty());
        for (i, n) in l.nodes.iter().enumerate() {
            assert_eq!(hit(&l, &view, *n), Some(FeatureTabHit::Node(i)));
        }
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
        let chip = l.repos[1];
        assert_eq!(l.hit(&view, centre(chip)), Some(FeatureTabHit::Repo(1)));
        // Folded it only informs: no button, no ×.
        assert!(l.repo_buttons.iter().all(|b| b.is_empty()));
        assert!(l.repo_removes.iter().all(|r| r.is_none()));
        assert_eq!(
            l.hit(&view, centre(l.repos_toggle)),
            Some(FeatureTabHit::ToggleRepos)
        );
    }

    #[test]
    fn the_selected_repository_scrolls_into_view() {
        let mut view = unfolded();
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
            let r = l.repos[i];
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
        // Folded chips have no column to scroll.
        view.expanded = false;
        assert_eq!(tab_at(&view, 1920).reveal_repo(20, 0.0), None);
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

    /// A measure of one unit a character.
    fn chars(text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn a_files_lines_keep_all_of_it_and_break_a_long_word() {
        assert_eq!(doc_lines("a bb ccc", 4, chars), ["a bb", "ccc"]);
        // Brackets don't hold words together, as they do on a card's line.
        assert_eq!(doc_lines("x [a b c]", 4, chars), ["x [a", "b c]"]);
        assert_eq!(
            doc_lines("see abcdefghij end", 4, chars),
            ["see", "abcd", "efgh", "ij", "end"]
        );
        assert!(doc_lines("", 4, chars).is_empty());
        // Too narrow for a character: still all of it, one to a line.
        assert_eq!(doc_lines("abc", 0, chars), ["a", "b", "c"]);
        // Wider than one unit a byte can't split a character.
        assert_eq!(
            doc_lines("\u{e9}\u{e9}\u{e9}", 2, chars).concat(),
            "\u{e9}\u{e9}\u{e9}"
        );
    }

    /// `view`'s page carrying `blocks` as its file.
    fn with_doc(mut view: FeatureTabView, blocks: Vec<DocBlock>) -> FeatureTabView {
        view.page.sections.push(PageSection::Doc {
            name: "brief.md".to_owned(),
            blocks: Arc::new(blocks),
        });
        view
    }

    fn says(piece: &Piece, what: &str) -> bool {
        matches!(piece, Piece::Text { text, .. } if text == what)
    }

    /// The file goes last wherever it is put; on a narrow tab, under the
    /// activity that came in beside it, with its name over it.
    #[test]
    fn a_file_reads_under_all_else_on_its_page() {
        let blocks = vec![DocBlock::Para("the-doc-text".to_owned())];
        let view = with_doc(tab_view(), blocks);
        let page = TabColumn::Page as usize;
        let place = |l: &FeatureTabLayout, what: &dyn Fn(&Piece) -> bool| {
            (l.pieces[page].iter()).position(|(_, piece)| what(piece))
        };
        let wide = tab_at(&view, 1920);
        let text = place(&wide, &|p| says(p, "the-doc-text")).expect("its text");
        assert_eq!(text, wide.pieces[page].len() - 1);
        assert!(!(wide.pieces[page].iter()).any(|(_, p)| *p == Piece::Caption("BRIEF.MD".into())));
        let narrow = tab_at(&view, ACTIVITY_BELOW - 1);
        let text = place(&narrow, &|p| says(p, "the-doc-text")).expect("its text");
        assert_eq!(text, narrow.pieces[page].len() - 1);
        let caption = place(&narrow, &|p| *p == Piece::Caption("BRIEF.MD".into()));
        assert_eq!(caption, Some(text - 1));
        let activity = place(&narrow, &|p| *p == Piece::Caption("ACTIVITY".into()));
        assert!(activity.expect("its activity") < text - 1);
    }

    /// A list item's lines all start right of its mark, deeper the deeper.
    #[test]
    fn a_list_items_text_hangs_right_of_its_mark() {
        let long = "word ".repeat(80);
        let item = |depth, mark| DocBlock::Item {
            depth,
            mark,
            text: long.clone(),
        };
        let blocks = [item(0, ItemMark::Bullet), item(1, ItemMark::Number(3))];
        let s = doc_stack(&blocks, 300);
        let marks: Vec<Rect> = (s.pieces.iter())
            .filter(|(_, p)| matches!(p, Piece::ItemMark(_)))
            .map(|(r, _)| *r)
            .collect();
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].loc.x, 0);
        assert_eq!(marks[1].loc.x, DOC_INDENT);
        let lines = |under: i32, to: i32| -> Vec<Rect> {
            (s.pieces.iter())
                .filter(|(r, p)| {
                    matches!(p, Piece::Text { .. }) && r.loc.y >= under && r.loc.y < to
                })
                .map(|(r, _)| *r)
                .collect()
        };
        let first = lines(0, marks[1].loc.y);
        assert!(first.len() > 1, "it wraps");
        assert!(first.iter().all(|r| r.loc.x == DOC_MARK));
        let second = lines(marks[1].loc.y, s.y);
        assert!(second.len() > 1, "it wraps");
        let x = second[0].loc.x;
        assert!(x >= DOC_INDENT + DOC_MARK);
        assert!(second.iter().all(|r| r.loc.x == x));
        for r in first.iter().chain(&second) {
            assert!(r.loc.x + r.size.w <= 300);
        }
    }

    /// Laid out every frame, a file is measured once a width; its next
    /// read, equal or not, is another `Arc` and laid out anew.
    #[test]
    fn a_file_is_laid_out_once_a_width() {
        let blocks = Arc::new(vec![DocBlock::Para("words to lay out".to_owned())]);
        let (first, h) = doc_laid_out(&blocks, 300);
        let (again, same) = doc_laid_out(&blocks, 300);
        assert!(Rc::ptr_eq(&first, &again));
        assert_eq!(h, same);
        let (wider, _) = doc_laid_out(&blocks, 400);
        assert!(!Rc::ptr_eq(&first, &wider));
        // Going between two widths, both are kept.
        assert!(Rc::ptr_eq(&first, &doc_laid_out(&blocks, 300).0));
        let other = Arc::new(blocks.as_ref().clone());
        assert!(!Rc::ptr_eq(&first, &doc_laid_out(&other, 300).0));
    }

    /// A long file scrolls its page, and painting it paints only what
    /// shows; a hover or a scroll draws no more for it.
    #[test]
    fn a_long_file_scrolls_and_draws_only_its_slice() {
        use crate::chrome::Theme;
        use std::time::{Duration, Instant};
        // Each line its own: a band like one drawn already is not drawn.
        let lines = (0..500).map(|i| DocBlock::Para(format!("line {i} of the plan")));
        let view = with_doc(tab_view(), lines.collect());
        let plain = tab_at(&tab_view(), 1920);
        let l = tab_at(&view, 1920);
        assert!(l.scroll_room(TabColumn::Page) > plain.scroll_room(TabColumn::Page) + 5_000);
        assert!(l.thumb(TabColumn::Page, 0.0).is_some());
        let painter = crate::chrome::Painter::new();
        let area = Rectangle::from_size((1920, 900).into());
        let mut layers = FeatureTabLayers::default();
        let t0 = Instant::now();
        let step = |layers: &mut FeatureTabLayers, view: &FeatureTabView, k: u64| {
            let before = layers.drawn.get();
            layers.parts(
                Some(view.clone()),
                area,
                Theme::Dark,
                1.0,
                t0 + Duration::from_secs(k),
                &painter,
                0.5,
                None,
            );
            layers.drawn.get() - before
        };
        step(&mut layers, &view, 0);
        step(&mut layers, &view, 1);
        assert_eq!(step(&mut layers, &view, 2), 0);
        let mut hovered = view.clone();
        hovered.hover = Some(FeatureTabHit::Node(3));
        assert_eq!(step(&mut layers, &hovered, 3), 1);
        // Scrolled, what shows now is drawn, and only that: the bands
        // shown already are kept.
        let mut scrolled = hovered.clone();
        scrolled.scroll[TabColumn::Page as usize] = 4000.4;
        let shown = shown_bands(&scrolled, area, 4000.4);
        assert_eq!(step(&mut layers, &scrolled, 4), shown);
        assert_eq!(step(&mut layers, &scrolled, 5), 0);
        scrolled.scroll[TabColumn::Page as usize] = 4040.4;
        let more = step(&mut layers, &scrolled, 6);
        assert!(more > 0 && more < shown_bands(&scrolled, area, 4040.4));
        scrolled.scroll[TabColumn::Page as usize] = 4000.4;
        let place = feature_tab_layout(&scrolled, area).columns[TabColumn::Page as usize].unwrap();
        let parts = layers.parts(
            Some(scrolled),
            area,
            Theme::Dark,
            1.0,
            t0 + Duration::from_secs(7),
            &painter,
            0.5,
            None,
        );
        // Each band where it shows, in the column, at its place in the page
        // as scrolled: the first at the column's top or across it.
        let page: Vec<_> = (parts.iter())
            .filter(|p| p.clip.is_some_and(|c| c.loc.x == place.loc.x + 1))
            .filter(|p| p.size.w < place.size.w - 2)
            .collect();
        let shown = shown_bands(&scrolled_again(&view), area, 4000.4);
        assert_eq!(page.len(), shown);
        let top = f64::from(place.loc.y);
        let bottom = f64::from(place.loc.y + place.size.h);
        let inside = |y: f64, h: i32| y < bottom && y + f64::from(h) > top;
        assert!(page.iter().all(|p| inside(p.loc.y, p.size.h)));
        assert!(page.iter().any(|p| p.loc.y <= top + 0.5));
    }

    /// `view` scrolled down its page by 4000.4.
    fn scrolled_again(view: &FeatureTabView) -> FeatureTabView {
        let mut v = view.clone();
        v.scroll[TabColumn::Page as usize] = 4000.4;
        v
    }

    /// How many bands of `view`'s page show in `area`, its page scrolled
    /// `down`.
    fn shown_bands(view: &FeatureTabView, area: Rect, down: f64) -> usize {
        let l = feature_tab_layout(view, area);
        let place = l.columns[TabColumn::Page as usize].unwrap();
        let slice = TabSlice::Column(TabColumn::Page);
        let r = l.slice_rect(slice, 0.0, 1.0).unwrap();
        let bands = page_bands(view, &l, (r.loc.x, r.loc.y, r.size.w), 1.0).unwrap();
        (bands.iter())
            .filter(|b| {
                f64::from(b.rect.loc.y + b.rect.size.h) - down > f64::from(place.loc.y)
                    && f64::from(b.rect.loc.y) - down < f64::from(place.loc.y + place.size.h)
            })
            .count()
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

    /// How long the tab takes with a long file on its page: laid out at
    /// first, laid out again, and its parts for a frame with nothing to
    /// draw. Run with `cargo test --profile fast --bin gerdr
    /// a_long_files_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_long_files_cost() {
        use crate::chrome::Theme;
        use std::time::{Duration, Instant};
        let painter = crate::chrome::Painter::new();
        let area = Rectangle::from_size((1920, 1080).into());
        let para = "A line of the plan that says what is to be done, and goes on \
                    long enough to wrap in a column of the tab at this width.";
        // 500 lines, and as much as `FILE_MAX` lets through.
        for n in [500, 512 * 1024 / para.len()] {
            let view = with_doc(tab_view(), vec![DocBlock::Para(para.to_owned()); n]);
            let start = Instant::now();
            let l = feature_tab_layout(&view, area);
            eprintln!("{n} blocks, first layout: {:?}", start.elapsed());
            eprintln!("  {} pieces", l.pieces[TabColumn::Page as usize].len());
            let start = Instant::now();
            for _ in 0..20 {
                feature_tab_layout(&view, area);
            }
            eprintln!("  layout: {:?}", start.elapsed() / 20);
            let mut layers = FeatureTabLayers::default();
            let t0 = Instant::now();
            let mut frame = |view: &FeatureTabView, k: u64| {
                layers.parts(
                    Some(view.clone()),
                    area,
                    Theme::Dark,
                    1.0,
                    t0 + Duration::from_secs(k),
                    &painter,
                    0.5,
                    None,
                );
            };
            let start = Instant::now();
            frame(&view, 0);
            eprintln!("  first parts: {:?}", start.elapsed());
            frame(&view, 1);
            let start = Instant::now();
            for k in 0..20 {
                frame(&view, 2 + k);
            }
            eprintln!("  parts, nothing to draw: {:?}", start.elapsed() / 20);
        }
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
        // The head, the hints, the activity and each band of the page that
        // shows, then the ground once it starts to show.
        let bands = shown_bands(&view, area, 0.0);
        assert!(bands > 1);
        assert_eq!(step(&mut layers, &view, 0).0, 3 + bands);
        assert_eq!(step(&mut layers, &view, 1).0, 1);
        assert_eq!(step(&mut layers, &view, 2).0, 0);
        // The band with the button, then the head; the band as it was
        // without the hover is kept.
        view.hover = Some(FeatureTabHit::PageButton(0));
        assert_eq!(step(&mut layers, &view, 2).0, 1);
        view.hover = Some(FeatureTabHit::Node(3));
        assert_eq!(step(&mut layers, &view, 3).0, 1);
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
        let shown = l.repos.iter().filter(|r| r.size.w > 0).count();
        assert!(shown > 0 && shown < 12);
        let [(chip, i, label)] = l.more_repos.as_slice() else {
            panic!("one mark: {:?}", l.more_repos);
        };
        assert_eq!(
            (*i, label.as_str()),
            (shown, format!("+{}", 12 - shown).as_str())
        );
        assert!(chip.loc.x + chip.size.w <= l.repos_toggle.loc.x);
        assert_eq!(
            l.hit(&view, centre(*chip)),
            Some(FeatureTabHit::MoreRepos(shown))
        );
        // From the 10th: the strip starts there, after "9+".
        view.repo = 10;
        let l = tab_at(&view, 850);
        let (lead, i, label) = &l.more_repos[0];
        assert_eq!((*i, label.as_str()), (9, "10+"));
        assert!(l.repos[10].size.w > 0);
        assert!(lead.loc.x + lead.size.w <= l.repos[10].loc.x);
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
        // Build's list blinks where it says: all that are at work, in its rows.
        let l = tab_at(&view, 1920);
        let rows: Vec<Rect> = (l.pieces[TabColumn::Page as usize].iter())
            .filter(|(_, piece)| matches!(piece, Piece::Milestone { running: true, .. }))
            .map(|(rect, _)| *rect)
            .collect();
        assert_eq!(rows.len(), 2);
        let dots = feature_tab_blinkers(&view, &l);
        for row in rows {
            assert!(dots.iter().any(|(dot, column)| {
                *column == Some(TabColumn::Page) && row.contains_rect(*dot)
            }));
        }
        // A round card's dot blinks while its fixes are made.
        let l = tab_at(&view, 1920);
        let round = (l.pieces[TabColumn::Page as usize].iter())
            .find(|(_, piece)| matches!(piece, Piece::RoundCard(_)))
            .map(|(rect, _)| *rect)
            .unwrap();
        let dots = feature_tab_blinkers(&view, &l);
        assert!(dots.iter().any(|(dot, _)| round.contains_rect(*dot)));
        for section in &mut view.page.sections {
            match section {
                PageSection::Milestones { running, .. } => running.clear(),
                PageSection::Round(card) => card.dot = Dot::Done,
                _ => {}
            }
        }
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
        view.expanded = true;
        let l = tab_at(&view, 1920);
        let dots = feature_tab_blinkers(&view, &l);
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].1, Some(TabColumn::Repos));
        assert!(l.repos[1].contains_rect(dots[0].0));

        view.expanded = false;
        let l = tab_at(&view, 850);
        let dots = feature_tab_blinkers(&view, &l);
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].1, None);
        assert!(l.repos[1].contains_rect(dots[0].0));
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
        assert!(halo.contains_rect(node_circle(l.nodes[3])));
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

    /// Each kind of dot reads on a row at rest, hovered and selected.
    #[test]
    fn the_repository_dots_read_on_the_rows_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT_PAGE] {
            for kind in [RepoKind::Active, RepoKind::Context, RepoKind::Other] {
                for ground in [p.panel, p.element, p.selected] {
                    let ratio = contrast(kind.color(p), ground);
                    assert!(ratio >= 3.0, "{kind:?} on {ground:?} at {ratio:.2}:1");
                }
            }
        }
    }

    /// A stat says only the counts it has, a lone file in the singular.
    #[test]
    fn the_stat_leaves_out_what_is_zero() {
        let parts = |files, added, removed| {
            let stat = Stat {
                files,
                added,
                removed,
            };
            stat_parts(stat).into_iter().flatten().collect::<Vec<_>>()
        };
        assert_eq!(parts(3, 12, 4), ["3 files", "+12", "\u{2212}4"]);
        assert_eq!(parts(1, 0, 0), ["1 file"]);
        assert_eq!(parts(0, 0, 0), [""; 0]);
        assert_eq!(stat_width(None), 0);
        assert_eq!(stat_width(Some(Stat::default())), 0);
    }

    /// The line stat's colours read on the repositories' column, on a
    /// row at rest, hovered and selected.
    #[test]
    fn the_line_stat_reads_on_the_rows_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT_PAGE] {
            for color in [p.success, p.error] {
                for ground in [p.panel, p.element, p.selected] {
                    let ratio = contrast(color, ground);
                    assert!(ratio >= 3.0, "{color:?} on {ground:?} at {ratio:.2}:1");
                }
            }
        }
    }

    /// The fixture's agent card, with chips.
    #[test]
    fn the_milestone_choice_is_hit_by_segment_and_scrolls_with_the_page() {
        let mut view = tab_with_picks(false);
        view.page.sections = vec![PageSection::MilestoneMode { fresh: false }];
        let l = tab_at(&view, 1180);
        let (track, [one, each]) = l.milestone_mode.expect("placed");
        assert!(track.size.w >= one.size.w + each.size.w);
        assert_eq!(one.loc.x + one.size.w, each.loc.x);
        let mid = |r: Rect| Point::from((r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2));
        assert_eq!(
            l.hit(&view, mid(one)),
            Some(FeatureTabHit::MilestoneMode(0))
        );
        assert_eq!(
            l.hit(&view, mid(each)),
            Some(FeatureTabHit::MilestoneMode(1))
        );
        view.scroll[TabColumn::Page as usize] = 10.0;
        let mut lifted = mid(each);
        lifted.y -= 10;
        assert_eq!(l.hit(&view, lifted), Some(FeatureTabHit::MilestoneMode(1)));
    }

    #[test]
    fn the_milestone_choice_fits_a_narrow_column() {
        let row = Rectangle::new((0, 0).into(), (200, PICK_TRACK).into());
        let (track, [_, each]) = milestone_mode_rects(row);
        assert!(track.loc.x + track.size.w <= 200);
        assert!(each.loc.x + each.size.w <= 200);
    }

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
    fn the_line_back_moves_nothing_and_is_drawn_with_the_head() {
        let frame = Rectangle::from_size((1400, 800).into());
        let mut view = super::super::golden::feature_tab();
        let without = feature_tab_layout(&view, frame);
        view.back = Some(RailBack { from: 5, to: 3 });
        let with = feature_tab_layout(&view, frame);
        assert_eq!(with.nodes, without.nodes);
        assert_eq!(slice_view(&view, TabSlice::Head).back, view.back);
        assert_eq!(slice_view(&view, TabSlice::Hints).back, None);
        // It runs clear of the header above and of a node's focus ring below.
        const {
            assert!(RAIL_BACK_RISE < RAIL_GAP + RAIL_BACK);
            assert!(RAIL_BACK_CLEAR as f32 > crate::chrome::FOCUS_GAP + crate::chrome::FOCUS_RING);
        }
    }

    #[test]
    fn grey_marks_read_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT, &ONE_LIGHT_PAGE] {
            // A ran stage's tick on its disc, its ring on the ground.
            let disc = mix(p.background, p.muted, 0.18);
            assert!(
                contrast(p.muted, disc) >= 3.0,
                "tick {:.2}",
                contrast(p.muted, disc)
            );
            let ring = contrast(p.muted, p.background);
            assert!(ring >= 3.0, "ring {ring:.2}");
            // A paused stage's number.
            let number = contrast(p.text, p.background);
            assert!(number >= 4.5, "number {number:.2}");
            // The line back, in its two colours.
            for line in [p.accent, p.warning] {
                let ratio = contrast(line, p.background);
                assert!(ratio >= 3.0, "line {ratio:.2}");
            }
        }
    }

    #[test]
    fn the_rail_is_a_line_taller_once_it_says_what_ran_a_stage() {
        let frame = Rectangle::from_size((1400, 800).into());
        let height = |view: &FeatureTabView| feature_tab_layout(view, frame).nodes[0].size.h;
        let mut view = super::super::golden::feature_tab();
        let with = height(&view);
        for node in &mut view.rail {
            node.agent = None;
        }
        assert_eq!(with, height(&view) + NODE_LINE);
    }

    /// `view` with its agent card's model button saying `label`.
    fn with_model(mut view: FeatureTabView, label: &str) -> FeatureTabView {
        for section in &mut view.page.sections {
            if let PageSection::Agent { model, .. } = section {
                *model = Some(label.to_owned());
            }
        }
        view
    }

    #[test]
    fn the_model_button_follows_the_chips_and_is_hit_where_it_shows() {
        let mut view = with_model(tab_with_picks(false), "Model: opus");
        let l = feature_tab_layout(&view, Rectangle::from_size((1180, 800).into()));
        let (track, _) = l.agent_picks.clone().expect("chips placed");
        let button = l.agent_model.expect("placed");
        assert_eq!(button.loc.y, track.loc.y);
        assert!(button.loc.x >= track.loc.x + track.size.w);
        let at = |r: Rect| (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2).into();
        assert_eq!(l.hit(&view, at(button)), Some(FeatureTabHit::AgentModel));
        view.scroll[TabColumn::Page as usize] = 40.0;
        let lifted = Rectangle::new(button.loc - Point::from((0, 40)), button.size);
        let l = feature_tab_layout(&view, Rectangle::from_size((1180, 800).into()));
        assert_eq!(l.hit(&view, at(lifted)), Some(FeatureTabHit::AgentModel));
    }

    #[test]
    fn no_label_no_model_button() {
        let view = tab_with_picks(false);
        assert_eq!(
            feature_tab_layout(&view, Rectangle::from_size((1180, 800).into())).agent_model,
            None
        );
    }

    #[test]
    fn a_long_model_stays_on_the_card_of_a_narrow_tab() {
        let long = "Model: claude-opus-4-8[context=1m,effort=high,fast=false]";
        let view = with_model(tab_with_picks(false), long);
        for w in [500, 860, 1180] {
            let l = feature_tab_layout(&view, Rectangle::from_size((w, 800).into()));
            let button = l.agent_model.expect("placed");
            assert!(button.size.w <= AGENT_MODEL_MAX, "{button:?}");
            assert!(button.loc.x + button.size.w <= w, "{w}: {button:?}");
            let (track, _) = l.agent_picks.clone().expect("chips placed");
            let (a, b) = (button, track);
            let apart = a.loc.x >= b.loc.x + b.size.w || a.loc.y >= b.loc.y + b.size.h;
            assert!(apart, "{w}: {a:?} on {b:?}");
        }
    }

    #[test]
    fn agent_chips_too_wide_together_share_the_room() {
        let (track, chips) = agent_pick_rects(8, 10, 20, 150);
        assert!(track.size.w <= 150, "{track:?}");
        assert_eq!(chips[0].size.w, chips[7].size.w);
        assert!(chips[7].loc.x + chips[7].size.w <= 10 + 150, "{chips:?}");
    }

    #[test]
    fn rows_from_right_fill_from_the_first_and_end_at_the_right() {
        // Two fit, the third starts a row, all ending at 100.
        assert_eq!(
            rows_from_right(&[40, 40, 60], 0, 100),
            [
                (100 - 40 - QUIT_BUTTON_GAP - 40, 0),
                (100 - 40, 0),
                (100 - 60, 1)
            ]
        );
        assert_eq!(
            rows_height(&rows_from_right(&[40, 40, 60], 0, 100)),
            2 * ROW_BUTTON + QUIT_BUTTON_GAP
        );
        assert_eq!(rows_height(&[]), 0);
    }

    /// A view with a page of `n` finding cards, each with three buttons.
    fn findings_view(n: usize, text: &str) -> FeatureTabView {
        let mut view = tab_view();
        view.page.buttons.clear();
        view.page.sections.clear();
        for i in 0..n {
            let at = view.page.buttons.len();
            for label in ["Fix", "Edit\u{2026}", "Dismiss\u{2026}", "Show in diff"] {
                view.page.buttons.push(PageButton {
                    label: label.to_owned(),
                    style: ButtonStyle::Plain,
                });
            }
            view.page.sections.push(PageSection::Finding(FindingCard {
                sheet: None,
                tag: FindingTag::High,
                title: format!("Finding {i}: {text}"),
                state: String::new(),
                place: format!("r1-{i} \u{b7} src/a.rs:12"),
                body: format!("{text}\n\n{text}"),
                preview: Vec::new(),
                fix: format!("Fix: {text}"),
                note: String::new(),
                comment: String::new(),
                comment_buttons: Vec::new(),
                made: false,
                faded: false,
                collapsed: false,
                stripe: None,
                field: None,
                field_hint: "",
                selected: false,
                buttons: (at..at + 4).collect(),
            }));
        }
        view
    }

    #[test]
    fn a_note_being_written_holds_its_lines_and_the_pointer_finds_each() {
        let field_of = |view: &FeatureTabView| {
            let l = tab_at(view, 1200);
            let page = &l.pieces[TabColumn::Page as usize];
            let (rect, _) = (page.iter())
                .find(|(_, p)| matches!(p, Piece::CardField(0)))
                .expect("the note's field");
            (*rect, l.finding_cards[0], l.finding_cards[1])
        };
        let mut view = findings_view(2, "short");
        let note = |view: &mut FeatureTabView, text: &str| {
            if let Some(PageSection::Finding(card)) = view.page.sections.get_mut(0) {
                card.field = Some(Line {
                    text: text.into(),
                    caret: text.len(),
                    anchor: text.len(),
                });
            }
        };
        note(&mut view, "one line");
        let (one, card, _) = field_of(&view);
        assert_eq!(one.size.h, QUIT_BUTTON);
        note(&mut view, "first\nsecond\nthird");
        let (three, grown, next) = field_of(&view);
        assert_eq!(three.size.h - one.size.h, 2 * FIELD_LINE);
        assert_eq!(grown.size.h - card.size.h, 2 * FIELD_LINE);
        assert!(three.loc.y + three.size.h <= grown.loc.y + grown.size.h);
        assert!(grown.loc.y + grown.size.h <= next.loc.y, "the next card moves down");
        let whole = Line {
            text: "first\nsecond\nthird".into(),
            caret: 0,
            anchor: 0,
        };
        let at = |y: i32| Point::from((three.loc.x as f64 + 5.0, (three.loc.y + y) as f64));
        let line_at = |y: i32| {
            let (spot, base) = field_line_spot(three, three, &whole, at(y), 1.0);
            (spot.line.text, base)
        };
        assert_eq!(line_at(5), ("first".to_owned(), 0));
        assert_eq!(line_at(FIELD_LINE + 5), ("second".to_owned(), 6));
        assert_eq!(line_at(three.size.h + 40), ("third".to_owned(), 13));
    }

    #[test]
    fn the_users_comment_on_a_finding_sits_inside_it_with_its_text_and_buttons() {
        let text = "a long sentence that goes on and on ".repeat(12);
        let mut view = findings_view(2, "short");
        let at = view.page.buttons.len();
        for label in ["Edit", "Delete"] {
            view.page.buttons.push(PageButton {
                label: label.to_owned(),
                style: ButtonStyle::Plain,
            });
        }
        if let Some(PageSection::Finding(card)) = view.page.sections.get_mut(0) {
            card.comment = text.clone();
            card.comment_buttons = vec![at, at + 1];
        }
        for width in [870, 1200, 1920] {
            let l = tab_at(&view, width);
            let page = &l.pieces[TabColumn::Page as usize];
            let (back, _) = page
                .iter()
                .find(|(_, p)| matches!(p, Piece::YourComment))
                .expect("the comment's card");
            let card = l.finding_cards[0];
            assert!(
                back.loc.x >= card.loc.x
                    && back.loc.x + back.size.w <= card.loc.x + card.size.w
                    && back.loc.y >= card.loc.y
                    && back.loc.y + back.size.h <= card.loc.y + card.size.h,
                "inside the finding at {width}"
            );
            // Its first line and its last are in it, as are its buttons.
            let words = page.iter().filter(|(r, p)| {
                matches!(
                    p,
                    Piece::Text {
                        ink: Ink::Text,
                        bold: false,
                        ..
                    }
                ) && r.loc.y >= back.loc.y
                    && r.loc.y < back.loc.y + back.size.h
            });
            let last = words.max_by_key(|(r, _)| r.loc.y).expect("what it says").0;
            assert!(last.loc.y + last.size.h <= back.loc.y + back.size.h);
            for b in [at, at + 1] {
                // On it alone, not with the page's own too.
                let places = page
                    .iter()
                    .filter(|(_, p)| matches!(p, Piece::Button(i) if *i == b));
                assert_eq!(places.count(), 1, "button {b} at {width}");
                let r = l.page_buttons[b].expect("placed");
                assert!(r.loc.x >= back.loc.x && r.loc.x + r.size.w <= back.loc.x + back.size.w);
                assert!(r.loc.y >= back.loc.y && r.loc.y + r.size.h <= back.loc.y + back.size.h);
            }
        }
    }

    #[test]
    fn the_users_comment_on_a_finding_reads_in_both_themes() {
        for p in [&ONE_DARK, &ONE_LIGHT, &ONE_LIGHT_PAGE] {
            let ground = comment_ground(p);
            let ratio = contrast(p.text, ground);
            assert!(ratio >= 4.5, "its words are {ratio:.2}:1");
            let ratio = contrast(comment_stripe(p), ground);
            assert!(ratio >= 3.0, "its stripe is {ratio:.2}:1");
        }
    }

    #[test]
    fn a_list_of_fixes_holds_its_button_at_its_captions_right() {
        let area = Rectangle::new((0, 0).into(), (1000, 800).into());
        let mut view = tab_view();
        view.page.sections.clear();
        view.page.buttons = vec![PageButton {
            label: "Open plan".into(),
            style: ButtonStyle::Plain,
        }];
        view.page.sections.push(PageSection::Milestones {
            label: "Code review 1".into(),
            state: None,
            done: 1,
            total: 1,
            items: vec![(true, "a.rs:1".into())],
            running: Vec::new(),
            buttons: vec![0],
        });
        let l = feature_tab_layout(&view, area);
        let button = l.page_buttons[0].expect("placed");
        let caption = (l.pieces.iter().flatten())
            .find_map(|(rect, piece)| matches!(piece, Piece::GroupCaption(0, _)).then_some(*rect))
            .expect("its caption");
        assert_eq!(button.loc.x + button.size.w, caption.loc.x + caption.size.w);
        assert!((button.loc.y - caption.loc.y).abs() <= ROW_BUTTON - CAPTION);
    }

    #[test]
    fn a_collapsed_card_is_one_line_with_only_its_first_button() {
        let area = Rectangle::new((0, 0).into(), (1000, 800).into());
        let mut view = tab_view();
        view.page.sections.clear();
        view.page.buttons = ["Reopen", "Comment", "Show in diff"]
            .map(|label| PageButton {
                label: label.into(),
                style: ButtonStyle::Plain,
            })
            .to_vec();
        let card = |collapsed| FindingCard {
            sheet: None,
            tag: FindingTag::Low,
            title: "A title long enough to go past the end of any line it is cut to fit on, \
                    however wide the page it sits on happens to be today"
                .repeat(3),
            state: "dismissed: why".into(),
            place: "r1-1 \u{b7} a.rs:1".into(),
            body: "Body".into(),
            preview: Vec::new(),
            fix: "Fix: this".into(),
            note: String::new(),
            comment: String::new(),
            comment_buttons: Vec::new(),
            made: false,
            faded: true,
            collapsed,
            stripe: None,
            field: None,
            field_hint: "",
            selected: false,
            buttons: vec![0, 1, 2],
        };
        view.page.sections.push(PageSection::Finding(card(true)));
        let closed = feature_tab_layout(&view, area);
        let frame = closed.finding_cards[0];
        assert_eq!(frame.size.h, 2 * AGENT_PAD + ROW_BUTTON);
        assert!(closed.page_buttons[0].is_some());
        assert!(closed.page_buttons[1].is_none() && closed.page_buttons[2].is_none());
        let button = closed.page_buttons[0].unwrap();
        assert!(button.loc.x + button.size.w <= frame.loc.x + frame.size.w);
        // Its text stays clear of the button.
        assert_eq!(
            width_of(&cut_to_width("abc def ghi", 12.0, 1), 12.0) as i32,
            width_of("\u{2026}", 12.0) as i32
        );
        assert_eq!(cut_to_width("short", 12.0, 500), "short");
        view.page.sections[0] = PageSection::Finding(card(false));
        let open = feature_tab_layout(&view, area);
        assert!(open.finding_cards[0].size.h > frame.size.h);
        assert!(open.page_buttons.iter().take(3).all(Option::is_some));
    }

    #[test]
    fn a_finding_cards_frame_holds_all_its_text_and_buttons_at_any_width() {
        let text = "a long sentence that goes on and on ".repeat(12);
        let view = findings_view(2, &text);
        for width in [870, 1200, 1920] {
            let l = tab_at(&view, width);
            let page = &l.pieces[TabColumn::Page as usize];
            assert_eq!(l.finding_cards.len(), 2);
            let (a, b) = (l.finding_cards[0], l.finding_cards[1]);
            assert!(a.loc.y + a.size.h <= b.loc.y, "cards apart at {width}");
            for (k, card) in l.finding_cards.iter().enumerate() {
                let at = page
                    .iter()
                    .position(|(r, p)| r == card && matches!(p, Piece::FindingCard(i) if *i == k));
                let at = at.expect("its frame");
                // The frame paints first, under what it holds.
                let held: Vec<&Rect> = page[at + 1..]
                    .iter()
                    .take_while(|(_, p)| !matches!(p, Piece::FindingCard(_)))
                    .filter(|(_, p)| matches!(p, Piece::Text { .. }))
                    .map(|(r, _)| r)
                    .collect();
                assert!(held.len() > 4, "all of its text at {width}");
                for r in held {
                    assert!(card.contains_rect(*r), "{r:?} in {card:?} at {width}");
                }
                for b in k * 4..k * 4 + 4 {
                    let r = l.page_buttons[b].expect("placed");
                    assert!(card.contains_rect(r), "button {b} in {card:?} at {width}");
                }
            }
        }
    }

    #[test]
    fn a_narrow_card_wraps_its_buttons_rather_than_cut_them() {
        let view = findings_view(1, "short");
        let l = tab_at(&view, 420);
        let card = l.finding_cards[0];
        for b in 0..4 {
            let r = l.page_buttons[b].expect("placed");
            assert!(card.contains_rect(r), "{r:?} in {card:?}");
        }
    }

    #[test]
    fn a_card_is_hit_past_its_buttons_and_the_keyboards_one_scrolls_into_view() {
        let view = findings_view(30, "some words");
        let l = tab_at(&view, 1200);
        let (card, button) = (l.finding_cards[1], l.page_buttons[4].unwrap());
        let on = (card.loc.x + 3, card.loc.y + 3).into();
        assert_eq!(l.hit(&view, on), Some(FeatureTabHit::Finding(1)));
        assert_eq!(
            l.hit(&view, centre(button)),
            Some(FeatureTabHit::PageButton(4))
        );
        // In view: where it was. Below: its bottom shows. Above: its top.
        assert_eq!(l.reveal_finding(0, 0.0), Some(0.0));
        let place = l.columns[TabColumn::Page as usize].unwrap();
        let last = l.finding_cards[29];
        let to = l.reveal_finding(29, 0.0).unwrap();
        assert!(to > 0.0);
        assert!(
            f64::from(last.loc.y + last.size.h - place.loc.y)
                <= to + f64::from(place.size.h) + f64::from(CARD_PAD)
        );
        assert!(l.reveal_finding(0, to).unwrap() < to);
        assert_eq!(l.reveal_finding(99, 0.0), None);
    }

    /// Run by hand (`--ignored --nocapture`): what a full page of findings,
    /// at the import's caps, costs a frame's layout.
    #[test]
    #[ignore]
    fn a_full_page_of_findings_lays_out_fast() {
        let text = "word ".repeat(800);
        let view = findings_view(100, &text);
        let first = std::time::Instant::now();
        let _ = tab_at(&view, 1920);
        let first = first.elapsed();
        let again = std::time::Instant::now();
        let _ = tab_at(&view, 1920);
        println!("first layout {first:?}, then {:?}", again.elapsed());
    }

    #[test]
    fn a_stages_own_buttons_end_its_titles_line_and_leave_when_they_dont_fit() {
        let title_row = |view: &FeatureTabView, width: i32| {
            let l = tab_at(view, width);
            let (title, _) = l.pieces[TabColumn::Page as usize][..]
                .iter()
                .find(|(_, p)| matches!(p, Piece::Text { bold: true, .. }))
                .expect("a title");
            let title = *title;
            let free = l.page_buttons[0].expect("placed");
            (title, free)
        };
        let view = tab_view();
        let (title, free) = title_row(&view, 1180);
        assert!(free.loc.y >= title.loc.y);
        assert!(free.loc.y + free.size.h <= title.loc.y + title.size.h);
        assert_eq!(free.loc.x + free.size.w, title.loc.x + title.size.w);
        // Too many for the title's line: below what the page says, at its end.
        let mut many = tab_view();
        many.page.buttons = (0..12)
            .map(|i| PageButton {
                label: format!("Button {i}"),
                style: ButtonStyle::Plain,
            })
            .collect();
        let (title, first) = title_row(&many, 1180);
        assert!(first.loc.y >= title.loc.y + title.size.h);
    }

    #[test]
    fn a_narrow_agent_card_keeps_its_name_and_moves_its_buttons_below() {
        let view = tab_with_picks(false);
        let wide = tab_at(&view, 1180);
        let narrow = tab_at(&view, 420);
        let card = |l: &FeatureTabLayout| {
            l.pieces[TabColumn::Page as usize]
                .iter()
                .find(|(_, p)| matches!(p, Piece::Agent(_)))
                .map(|(r, _)| *r)
                .expect("a card")
        };
        let (wide_card, narrow_card) = (card(&wide), card(&narrow));
        assert!(narrow_card.size.h > wide_card.size.h);
        let (track, _) = narrow.agent_picks.clone().expect("placed");
        assert!(track.loc.y >= narrow_card.loc.y + AGENT_PAD + AGENT_HEAD);
        assert!(
            track.loc.x >= narrow_card.loc.x,
            "{track:?} {narrow_card:?}"
        );
        assert!(track.loc.y + track.size.h <= narrow_card.loc.y + narrow_card.size.h);
        for r in narrow.page_buttons.iter().flatten() {
            assert!(r.loc.x >= narrow_card.loc.x, "{r:?} {narrow_card:?}");
        }
    }

    #[test]
    fn a_runs_chips_end_its_heads_row_right_of_its_buttons() {
        let view = tab_with_picks(false);
        let l = tab_at(&view, 1180);
        let (track, _) = l.agent_picks.clone().expect("placed");
        let button = (view.page.sections.iter())
            .find_map(|s| match s {
                PageSection::Agent { buttons, .. } => buttons.last().copied(),
                _ => None,
            })
            .and_then(|i| l.page_buttons[i])
            .expect("the fixture's card has a button");
        assert_eq!(track.loc.y, button.loc.y);
        assert_eq!(track.size.h, button.size.h);
        assert_eq!(button.loc.x + button.size.w + QUIT_BUTTON_GAP, track.loc.x);
    }
}
