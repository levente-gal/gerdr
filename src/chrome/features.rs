//! The features' surfaces (see `crate::features`): the new feature dialog
//! over everything, the feature panel with a feature's repositories and
//! stages, and the stage marks the sidebar's feature rows show.

use super::{
    ButtonStyle, CHECKBOX_GAP, CHECKBOX_ROW, Canvas, Check, DETAIL_FONT_SIZE, DIALOG_WIDTH,
    FONT_SIZE, FieldSpot, LABEL_FONT_SIZE, POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON,
    QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, Rect, Rgb, SHADOW, band,
    button_width, check, dialog_buttons, field_text, footer_height, footer_hint_spots, hint_strs,
    icon_sdf, phys,
};
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
/// How high a row of the feature panel is, and the gap between rows.
const PANEL_ROW: i32 = 30;
const PANEL_ROW_GAP: i32 = 2;
/// The gap between the panel's sections.
const SECTION_GAP: i32 = 12;
/// How high a button on a row is.
const ROW_BUTTON: i32 = 24;
/// How wide the previous and next feature buttons are.
const SWITCH_W: i32 = 28;
/// How big a stage's mark is, and where its label starts after it.
const MARK: i32 = 14;
const MARK_ROOM: i32 = 26;
/// How high a line of the Ship check is, and how many show at most.
const CHECK_LINE: i32 = 20;
pub const MAX_CHECK_LINES: usize = 4;
/// The feature panel's buttons, before the one removing worktrees.
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

/// A stage's mark in `rect`: a check for done, a dot for the current, a
/// ring for a later one.
fn paint_mark(c: &mut Canvas, rect: Rect, mark: StageMark, scale: f64, p: &Palette) {
    let s = scale as f32;
    let color = mark.color(p);
    match mark {
        StageMark::Done => icon_sdf(c, rect, scale, color, |x, y| check(x, y, s * 1.2)),
        StageMark::Current => {
            icon_sdf(c, rect, scale, color, |x, y| stage_dot(mark, x, y, 9.0 * s))
        }
        StageMark::Later => icon_sdf(c, rect, scale, color, |x, y| {
            stage_dot(mark, x, y, 10.0 * s)
        }),
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

/// A stage of the feature panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageRow {
    pub label: String,
    /// When it was done, or what is done in it.
    pub detail: String,
    pub mark: StageMark,
    /// The label of its button (Reopen, Mark done, Ship…); none for a
    /// later one.
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

/// What the feature panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeaturePanelView {
    pub title: String,
    /// Its slug and folder.
    pub sub: String,
    /// Which of how many features it is, when there are several.
    pub which: Option<(usize, usize)>,
    pub repos: Vec<FeatureRepoRow>,
    pub stages: Vec<StageRow>,
    /// The latest Ship check, under the Ship row, while Ship is the
    /// current stage: at most `MAX_CHECK_LINES`.
    pub check: Vec<CheckLine>,
    /// The row with the keyboard: the repositories', then the stages'.
    pub cursor: usize,
    /// The first repository shown, when not all fit.
    pub repo_top: usize,
    /// The label of the button removing its worktrees.
    pub remove: &'static str,
    pub hover: Option<FeatureHit>,
    pub hints: Vec<(String, String)>,
}

/// What the pointer is over in the feature panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureHit {
    Panel,
    Prev,
    Next,
    /// Index as in `FeaturePanelView::repos`.
    Repo(usize),
    RepoButton(usize),
    /// Index as in `FeaturePanelView::stages`.
    Stage(usize),
    StageButton(usize),
    /// `FEATURE_BUTTONS`, then the one removing worktrees.
    Button(usize),
    Hint(usize),
}

/// Where the feature panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeaturePanelLayout {
    pub panel: Rect,
    pub title: Rect,
    pub sub: Rect,
    /// "2 of 3", and the buttons to the previous and next feature.
    pub switch: Option<(Rect, Rect, Rect)>,
    /// The slots the repositories from `repo_top` are shown in, each with
    /// its Touch button if it has one.
    pub repos: Vec<(Rect, Option<Rect>)>,
    /// Each stage's row and its button, if it has one.
    pub stages: Vec<(Rect, Option<Rect>)>,
    /// The lines of the Ship check, under the stages.
    pub check: Vec<Rect>,
    pub buttons: Vec<Rect>,
    pub hints: Vec<(usize, Rect)>,
}

/// How high the Ship check's `lines` are, with the gap above them.
fn check_height(lines: usize) -> i32 {
    match lines.min(MAX_CHECK_LINES) {
        0 => 0,
        n => ROW_GAP + n as i32 * CHECK_LINE,
    }
}

/// How many of `view`'s repositories the feature panel shows on an output
/// of `output`: at least one.
pub fn feature_repos_shown(view: &FeaturePanelView, output: Size<i32, Logical>) -> usize {
    let (repos, stages, hints) = (view.repos.len(), view.stages.len(), view.hints.len());
    let pitch = PANEL_ROW + PANEL_ROW_GAP;
    let fixed = 2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_BODY
        + SECTION_GAP
        + SECTION_GAP
        + stages as i32 * pitch
        + check_height(view.check.len())
        + SECTION_GAP
        + QUIT_BUTTON
        + footer_height(hints);
    let fit = ((output.h - 2 * MARGIN - fixed) / pitch).max(1) as usize;
    repos.min(fit)
}

/// Where the feature panel's parts go on an output of `output`: centred,
/// as wide as a dialog, the repositories past what fits scrolled.
pub fn feature_panel_layout(
    view: &FeaturePanelView,
    output: Size<i32, Logical>,
) -> FeaturePanelLayout {
    let pitch = PANEL_ROW + PANEL_ROW_GAP;
    let shown = feature_repos_shown(view, output);
    let check_h = check_height(view.check.len());
    let repos_h = if shown == 0 {
        0
    } else {
        shown as i32 * pitch + SECTION_GAP
    };
    let footer_h = footer_height(view.hints.len());
    let height = 2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_BODY
        + SECTION_GAP
        + repos_h
        + view.stages.len() as i32 * pitch
        + check_h
        + SECTION_GAP
        + QUIT_BUTTON
        + footer_h;
    let width = DIALOG_WIDTH.min(output.w);
    let panel = Rectangle::new(
        (
            ((output.w - width) / 2).max(0),
            ((output.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let x = panel.loc.x + QUIT_PADDING;
    let inner = width - 2 * QUIT_PADDING;
    let mut y = panel.loc.y + QUIT_PADDING;
    let title = Rectangle::new((x, y).into(), (inner, QUIT_TITLE).into());
    y += QUIT_TITLE;
    let sub = Rectangle::new((x, y).into(), (inner, QUIT_BODY).into());
    y += QUIT_BODY + SECTION_GAP;
    let switch = view.which.filter(|(_, n)| *n > 1).map(|_| {
        let right = x + inner;
        let next = Rectangle::new(
            (right - SWITCH_W, title.loc.y).into(),
            (SWITCH_W, QUIT_TITLE).into(),
        );
        let prev = Rectangle::new(
            (right - 2 * SWITCH_W - 2, title.loc.y).into(),
            (SWITCH_W, QUIT_TITLE).into(),
        );
        let count_w = 64;
        let count = Rectangle::new(
            (prev.loc.x - count_w - 4, title.loc.y).into(),
            (count_w, QUIT_TITLE).into(),
        );
        (count, prev, next)
    });
    let row_button = |row: Rect, label: &str| {
        let w = button_width(label);
        Rectangle::new(
            (
                row.loc.x + row.size.w - ROW_INSET - w,
                row.loc.y + (PANEL_ROW - ROW_BUTTON) / 2,
            )
                .into(),
            (w, ROW_BUTTON).into(),
        )
    };
    let mut repos = Vec::new();
    for (slot, repo) in view
        .repos
        .iter()
        .skip(view.repo_top)
        .take(shown)
        .enumerate()
    {
        let row = Rectangle::new(
            (x, y + slot as i32 * pitch).into(),
            (inner, PANEL_ROW).into(),
        );
        let button = repo
            .button
            .filter(|_| !repo.busy)
            .map(|label| row_button(row, label));
        repos.push((row, button));
    }
    y += repos_h;
    let stages = view
        .stages
        .iter()
        .enumerate()
        .map(|(i, stage)| {
            let row = Rectangle::new((x, y + i as i32 * pitch).into(), (inner, PANEL_ROW).into());
            let button = stage.button.map(|label| row_button(row, label));
            (row, button)
        })
        .collect();
    y += view.stages.len() as i32 * pitch;
    // The check's lines line up with the stages' labels.
    let check_x = x + ROW_INSET + MARK_ROOM;
    let check = (0..view.check.len().min(MAX_CHECK_LINES) as i32)
        .map(|i| {
            Rectangle::new(
                (check_x, y + ROW_GAP + i * CHECK_LINE).into(),
                (inner - ROW_INSET - MARK_ROOM, CHECK_LINE).into(),
            )
        })
        .collect();
    y += check_h + SECTION_GAP;
    let mut buttons = Vec::new();
    let mut bx = x;
    for label in FEATURE_BUTTONS {
        let w = button_width(label);
        buttons.push(Rectangle::new((bx, y).into(), (w, QUIT_BUTTON).into()));
        bx += w + QUIT_BUTTON_GAP;
    }
    let w = button_width(view.remove);
    buttons.push(Rectangle::new(
        (x + inner - w, y).into(),
        (w, QUIT_BUTTON).into(),
    ));
    y += QUIT_BUTTON;
    let footer = Rectangle::new((panel.loc.x, y).into(), (width, footer_h).into());
    let hints = footer_hint_spots(&hint_strs(&view.hints), footer);
    FeaturePanelLayout {
        panel,
        title,
        sub,
        switch,
        repos,
        stages,
        check,
        buttons,
        hints,
    }
}

impl FeaturePanelLayout {
    /// What `pos` is over, the repositories shown from `repo_top`; `None`
    /// off the panel.
    pub fn hit(&self, repo_top: usize, pos: Point<f64, Logical>) -> Option<FeatureHit> {
        if !self.panel.to_f64().contains(pos) {
            return None;
        }
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some((i, _)) = self.hints.iter().find(|(_, r)| on(r)) {
            return Some(FeatureHit::Hint(*i));
        }
        if let Some((_, prev, next)) = &self.switch {
            if on(prev) {
                return Some(FeatureHit::Prev);
            }
            if on(next) {
                return Some(FeatureHit::Next);
            }
        }
        if let Some(b) = self.buttons.iter().position(on) {
            return Some(FeatureHit::Button(b));
        }
        for (slot, (row, button)) in self.repos.iter().enumerate() {
            if button.as_ref().is_some_and(on) {
                return Some(FeatureHit::RepoButton(repo_top + slot));
            }
            if on(row) {
                return Some(FeatureHit::Repo(repo_top + slot));
            }
        }
        for (i, (row, button)) in self.stages.iter().enumerate() {
            if button.as_ref().is_some_and(on) {
                return Some(FeatureHit::StageButton(i));
            }
            if on(row) {
                return Some(FeatureHit::Stage(i));
            }
        }
        Some(FeatureHit::Panel)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        let rows = self.repos.iter().chain(&self.stages);
        rows.flat_map(|(row, button)| [Some(*row), *button])
            .flatten()
            .chain(self.buttons.iter().copied())
            .chain(self.hints.iter().map(|(_, r)| *r))
            .chain(
                self.switch
                    .iter()
                    .flat_map(|(_, prev, next)| [*prev, *next]),
            )
            .collect()
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

    /// The feature panel on an output of `output`, inside a margin of
    /// `SHADOW` holding its shadow.
    pub(super) fn feature_panel(
        &self,
        view: &FeaturePanelView,
        output: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = feature_panel_layout(view, output);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let s = scale as f32;
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let gap = (PLACE_GAP as f32 * s).round();

        // The title, and when there are several features, which this is
        // and the buttons to the others.
        let title = shift(layout.title);
        let mut title_end = phys(title, scale).1;
        if let (Some((count, prev, next)), Some((i, n))) = (layout.switch, view.which) {
            let count = shift(count);
            let (cx, cend) = phys(count, scale);
            let text = format!("{} of {n}", i + 1);
            let w = super::measure(&text, FONT_SIZE).unwrap_or(0.0) * s;
            self.text(
                &mut c,
                &text,
                FONT_SIZE,
                (cend - w).max(cx),
                cend,
                band(count, scale),
                scale,
                p.muted,
            );
            title_end = cx - gap;
            for (rect, left, hit) in [
                (prev, true, FeatureHit::Prev),
                (next, false, FeatureHit::Next),
            ] {
                let rect = shift(rect);
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
                    super::chevron_side(left, x, y, s)
                });
            }
        }
        let (tx, _) = phys(title, scale);
        self.text(
            &mut c,
            &view.title,
            LABEL_FONT_SIZE,
            tx,
            title_end,
            band(title, scale),
            scale,
            p.text,
        );
        let sub = shift(layout.sub);
        let (sx, send) = phys(sub, scale);
        self.text(
            &mut c,
            &view.sub,
            DETAIL_FONT_SIZE,
            sx,
            send,
            band(sub, scale),
            scale,
            p.muted,
        );

        // A row: lit under the keyboard's cursor or the pointer, a mark or
        // nothing, the name, what follows it, and its button.
        let row = |c: &mut Canvas,
                   rect: Rect,
                   button: Option<(Rect, &str, ButtonStyle, bool)>,
                   (selected, lit): (bool, bool),
                   mark: Option<StageMark>,
                   (name, name_color): (&str, Rgb),
                   (detail, detail_color): (&str, Rgb)| {
            let rect = shift(rect);
            self.row_highlight(c, rect, (selected, if lit { 1.0 } else { 0.0 }), scale, p);
            let mut x = rect.loc.x + ROW_INSET;
            if let Some(mark) = mark {
                let spot = Rectangle::new(
                    (x, rect.loc.y + (PANEL_ROW - MARK) / 2).into(),
                    (MARK, MARK).into(),
                );
                paint_mark(c, spot, mark, scale, p);
                x += MARK_ROOM;
            }
            let end = match &button {
                Some((b, ..)) => shift(*b).loc.x - QUIT_BUTTON_GAP,
                None => rect.loc.x + rect.size.w - ROW_INSET,
            };
            let text = Rectangle::new(
                (x, rect.loc.y).into(),
                ((end - x).max(1), rect.size.h).into(),
            );
            let (tx, tend) = phys(text, scale);
            let row_band = band(text, scale);
            let after = self.text(c, name, FONT_SIZE, tx, tend, row_band, scale, name_color);
            self.text(
                c,
                detail,
                DETAIL_FONT_SIZE,
                after + gap,
                tend,
                row_band,
                scale,
                detail_color,
            );
            if let Some((b, label, style, hovered)) = button {
                self.button(c, label, shift(b), (hovered, false), style, scale, p);
            }
        };

        let repos = view.repos.len();
        for (slot, (rect, button)) in layout.repos.iter().enumerate() {
            let i = view.repo_top + slot;
            let Some(repo) = view.repos.get(i) else {
                continue;
            };
            let lit = matches!(view.hover, Some(FeatureHit::Repo(r) | FeatureHit::RepoButton(r)) if r == i);
            let detail = if repo.busy {
                format!("{} \u{b7} touching\u{2026}", repo.line)
            } else {
                repo.line.clone()
            };
            let button = button.zip(repo.button).map(|(b, label)| {
                let hovered = view.hover == Some(FeatureHit::RepoButton(i));
                (b, label, ButtonStyle::Plain, hovered)
            });
            let name_color = if repo.touched { p.text } else { p.muted };
            row(
                &mut c,
                *rect,
                button,
                (view.cursor == i, lit),
                None,
                (&repo.name, name_color),
                (&detail, p.placeholder),
            );
        }
        for (i, ((rect, button), stage)) in layout.stages.iter().zip(&view.stages).enumerate() {
            let lit = matches!(view.hover, Some(FeatureHit::Stage(r) | FeatureHit::StageButton(r)) if r == i);
            let button = button.zip(stage.button).map(|(b, label)| {
                let hovered = view.hover == Some(FeatureHit::StageButton(i));
                let style = if stage.mark == StageMark::Current {
                    ButtonStyle::Primary
                } else {
                    ButtonStyle::Plain
                };
                (b, label, style, hovered)
            });
            let (name_color, detail_color) = match stage.mark {
                StageMark::Later => (p.muted, p.placeholder),
                _ => (p.text, p.muted),
            };
            row(
                &mut c,
                *rect,
                button,
                (view.cursor == repos + i, lit),
                Some(stage.mark),
                (&stage.label, name_color),
                (&stage.detail, detail_color),
            );
        }

        // The Ship check, one line each.
        for (rect, line) in layout.check.iter().zip(&view.check) {
            let rect = shift(*rect);
            let (x, end) = phys(rect, scale);
            self.text(
                &mut c,
                &line.text,
                FONT_SIZE,
                x,
                end,
                band(rect, scale),
                scale,
                line.tone.color(p),
            );
        }

        // The buttons, the one removing worktrees at the right.
        let labels = FEATURE_BUTTONS.iter().copied().chain([view.remove]);
        for (i, (rect, label)) in layout.buttons.iter().zip(labels).enumerate() {
            let style = if i == FEATURE_BUTTONS.len() {
                ButtonStyle::Destructive
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(FeatureHit::Button(i));
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
        let hints: Vec<(usize, Rect)> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hover = match view.hover {
            Some(FeatureHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hint_strs(&view.hints), &hints, hover, scale, p);
        c.into_buffer(false)
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

    fn panel_view(repos: usize) -> FeaturePanelView {
        FeaturePanelView {
            title: "Bulk export".into(),
            sub: "bulk-export \u{b7} ~/Work/.features/bulk-export".into(),
            which: Some((0, 2)),
            repos: (0..repos)
                .map(|i| FeatureRepoRow {
                    name: format!("repo-{i}"),
                    line: "feature/bulk-export".into(),
                    touched: i == 0,
                    busy: false,
                    button: (i != 0).then_some("Touch"),
                })
                .collect(),
            stages: (0..8)
                .map(|i| StageRow {
                    label: format!("Stage {i}"),
                    detail: String::new(),
                    mark: match i {
                        0 | 1 => StageMark::Done,
                        2 => StageMark::Current,
                        _ => StageMark::Later,
                    },
                    button: match i {
                        0 | 1 => Some("Reopen"),
                        2 => Some("Mark done"),
                        _ => None,
                    },
                })
                .collect(),
            check: Vec::new(),
            cursor: 0,
            repo_top: 0,
            remove: "Abandon",
            hover: None,
            hints: vec![("Esc".into(), "close".into())],
        }
    }

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
    fn the_panel_fits_a_small_output_and_clamps_the_repositories() {
        let output: Size<i32, Logical> = (860, 900).into();
        let view = panel_view(30);
        let l = feature_panel_layout(&view, output);
        assert!(l.panel.size.w <= 860 && l.panel.size.h <= 900);
        assert!(l.repos.len() < 30 && !l.repos.is_empty());
        assert_eq!(l.stages.len(), 8);
        for (row, button) in l.repos.iter().chain(&l.stages) {
            assert!(inside(l.panel, *row));
            if let Some(b) = button {
                assert!(inside(*row, *b));
            }
        }
        // Only the stages done and the current one have a button.
        let with: Vec<bool> = l.stages.iter().map(|(_, b)| b.is_some()).collect();
        assert_eq!(with, [true, true, true, false, false, false, false, false]);
        // The untouched repositories have Touch.
        assert!(l.repos[0].1.is_none() && l.repos[1].1.is_some());
        assert!(l.switch.is_some());
        let (_, prev, next) = l.switch.unwrap();
        let mid = |r: Rect| (r.loc.x as f64 + 2.0, r.loc.y as f64 + 2.0).into();
        assert_eq!(l.hit(0, mid(prev)), Some(FeatureHit::Prev));
        assert_eq!(l.hit(0, mid(next)), Some(FeatureHit::Next));
        assert_eq!(
            l.hit(0, mid(l.stages[2].1.unwrap())),
            Some(FeatureHit::StageButton(2))
        );
        assert_eq!(l.hit(0, (0.0, 0.0).into()), None);
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
}
