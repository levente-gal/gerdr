//! The plan review's decisions sheet (see `crate::plan_sheet`): a feature's
//! suggestions, one row each with its mark, severity, kind, title and place
//! in the plan; under them all the cursor's says, in full; then Close, Ask
//! reviewer and the finishing button over the key hints.
//!
//! The sheet stays as high as it is while the cursor moves, so that nothing
//! jumps under the pointer: the list takes what its rows need up to a share
//! of the room, what the suggestion says the rest.

use super::features::wrap_all;
use super::{
    ButtonStyle, Canvas, DETAIL_FONT_SIZE, DIALOG_WIDTH, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE,
    ListBar, POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_PADDING,
    QUIT_TITLE, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, Thumb, band, button_width, check, cross,
    field_text, footer_height, footer_hint_spots, gutter_track, hint_strs, icon_sdf, measure, phys,
};
use crate::feature::{plan_review::Kind, review_import::Severity};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How high a suggestion's row is, and the gap between rows.
const ROW: i32 = 30;
const ROW_GAP: i32 = 2;
/// How high a line of the key list is.
const KEY_LINE: i32 = 28;
/// The gap between the sheet's parts.
const SECTION_GAP: i32 = 12;
/// How far a row's contents sit in from its edges.
const ROW_INSET: i32 = 6;
/// How big a suggestion's mark is, and where what follows starts after it.
const MARK: i32 = 14;
const MARK_ROOM: i32 = 26;
/// How wide a row's severity and kind are given: "medium" and "edge case"
/// fit, and the gap between a row's columns.
const SEVERITY_W: i32 = 48;
const KIND_W: i32 = 60;
const COLUMN_GAP: i32 = 8;
/// The most a row's place in the plan takes, and what takes the place of
/// that: "left out by the reviewer" and "Press Del again" fit.
const PLAN_W: i32 = 130;
const STATUS_W: i32 = 170;
/// The room kept above and below the sheet, and the highest it gets.
const MARGIN: i32 = 40;
const HEIGHT: i32 = 640;
/// Of the room for the list and the text, the share the list gets at most.
const LIST_SHARE: (i32, i32) = (2, 5);
/// How high a line of the text is, a section's caption with its gap above,
/// and how far short of the column it is wrapped: a font at another scale
/// measures a little differently, and a line isn't to be cut for that.
const LINE: i32 = 20;
const CAPTION: i32 = 28;
const WRAP_SLACK: i32 = 8;
/// The gap between the text field and the text under it.
const FIELD_GAP: i32 = 6;

/// The sheet's buttons: Close at the left, the others at the right.
pub const PLAN_CLOSE: usize = 0;
pub const PLAN_ASK: usize = 1;
pub const PLAN_FINISH: usize = 2;
const CLOSE_LABEL: &str = "Close";
const ASK_LABEL: &str = "Ask reviewer";

/// What the user decided of a suggestion, as the sheet shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlanMark {
    Open,
    Accepted,
    Rejected,
}

/// A suggestion's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRow {
    /// `r1-2`, `u1`.
    pub id: String,
    pub mark: PlanMark,
    pub severity: Severity,
    pub kind: Kind,
    pub title: String,
    /// Where in the plan it applies.
    pub plan: String,
    /// The user wrote it.
    pub yours: bool,
    /// The reviewer's last run left it out.
    pub dropped: bool,
    /// What the user said of it.
    pub note: String,
}

/// A citation of the suggestion under the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanCitation {
    /// `api: src/a.rs:12`.
    pub place: String,
    pub quote: String,
    /// gerdr looked, and the quote isn't there.
    pub not_found: bool,
}

/// All the suggestion under the cursor says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanDetail {
    pub title: String,
    pub plan: String,
    pub dropped: bool,
    pub question: bool,
    pub why: String,
    pub change: String,
    pub evidence: Vec<PlanCitation>,
    pub note: String,
}

/// The text field of the sheet, while it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanField {
    /// What it is for, over it.
    pub label: &'static str,
    pub placeholder: &'static str,
    pub line: Line,
}

/// What the plan review's sheet shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanSheetView {
    /// "Plan review of “title”".
    pub title: String,
    /// The counts.
    pub sub: String,
    /// "1 citation not found", when any.
    pub missing: Option<String>,
    pub rows: Vec<PlanRow>,
    /// The row with the keyboard.
    pub cursor: usize,
    /// The first row shown.
    pub top: usize,
    pub detail: Option<PlanDetail>,
    /// The first line of `detail` shown.
    pub detail_top: usize,
    pub editing: Option<PlanField>,
    /// The row that says "Press Del again".
    pub armed: Option<usize>,
    /// Why the last thing asked was refused.
    pub notice: Option<String>,
    pub hover: Option<PlanHit>,
    /// The finishing button's label.
    pub finish: &'static str,
    /// The button drawn as the one to press next: the finishing one once
    /// none is open.
    pub primary: Option<usize>,
    /// The keys and what they do, shown instead of the rest (F1).
    pub keys: Option<Vec<(String, String)>>,
    /// The first key line shown.
    pub keys_top: usize,
    pub hints: Vec<(String, String)>,
}

/// What the pointer is over in the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanHit {
    /// A row, but not its mark.
    Row(usize),
    /// A row's mark: a click toggles accepted.
    Mark(usize),
    /// Citation `.0` of the suggestion under the cursor.
    Citation(usize),
    /// The text field.
    Field,
    /// Close, Ask reviewer, the finishing one.
    Button(usize),
    Hint(usize),
    /// Somewhere else on it.
    Panel,
}

/// What a line of the text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanPiece {
    /// A section's name.
    Caption,
    Body,
    /// Said of the suggestion, not by it.
    Aside,
    /// Where citation `.0` is: a click opens it.
    Place(usize),
    Quote,
    /// "not found in the code".
    Missing,
}

/// A line of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanLine {
    pub piece: PlanPiece,
    pub text: String,
}

impl PlanLine {
    fn height(&self) -> i32 {
        match self.piece {
            PlanPiece::Caption => CAPTION,
            _ => LINE,
        }
    }
}

/// `detail` in lines as wide as `width`, nothing of it left out.
pub fn plan_lines(detail: &PlanDetail, width: i32) -> Vec<PlanLine> {
    let mut lines = Vec::new();
    let mut put = |piece, text: &str, size| {
        lines.extend(
            wrap_all(text, size, width)
                .into_iter()
                .map(|text| PlanLine { piece, text }),
        );
    };
    put(PlanPiece::Body, &detail.title, FONT_SIZE);
    if !detail.plan.is_empty() {
        let text = format!("In the plan: {}", detail.plan);
        put(PlanPiece::Aside, &text, DETAIL_FONT_SIZE);
    }
    if detail.dropped {
        put(
            PlanPiece::Aside,
            "left out by the reviewer",
            DETAIL_FONT_SIZE,
        );
    }
    for (caption, text) in [("Why", &detail.why), ("Change", &detail.change)] {
        if !text.is_empty() {
            put(PlanPiece::Caption, caption, DETAIL_FONT_SIZE);
            put(PlanPiece::Body, text, FONT_SIZE);
        }
    }
    if !detail.evidence.is_empty() {
        put(PlanPiece::Caption, "Evidence", DETAIL_FONT_SIZE);
    }
    for (i, citation) in detail.evidence.iter().enumerate() {
        put(PlanPiece::Place(i), &citation.place, DETAIL_FONT_SIZE);
        put(PlanPiece::Quote, &citation.quote, DETAIL_FONT_SIZE);
        if citation.not_found {
            put(
                PlanPiece::Missing,
                "not found in the code",
                DETAIL_FONT_SIZE,
            );
        }
    }
    let caption = if detail.question {
        "Your answer"
    } else {
        "Your note"
    };
    if !detail.note.is_empty() {
        put(PlanPiece::Caption, caption, DETAIL_FONT_SIZE);
        put(PlanPiece::Body, &detail.note, FONT_SIZE);
    } else if detail.question {
        put(PlanPiece::Caption, caption, DETAIL_FONT_SIZE);
        put(PlanPiece::Aside, "not answered yet", DETAIL_FONT_SIZE);
    }
    lines
}

/// Where a shown row's parts go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanRowSpot {
    /// Index into `PlanSheetView::rows`.
    pub i: usize,
    pub row: Rect,
    pub mark: Rect,
    /// Its severity and kind, or "yours" across both.
    pub label: Rect,
    /// Its title.
    pub text: Rect,
    /// Its place in the plan, or what stands for that.
    pub end: Rect,
}

/// Where the sheet's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanSheetLayout {
    pub panel: Rect,
    pub title: Rect,
    pub sub: Rect,
    /// Where the rows (or the keys) go.
    pub list: Rect,
    pub rows: Vec<PlanRowSpot>,
    /// One per key line shown, with F1, and `view.keys_top` the first.
    pub keys: Vec<Rect>,
    /// The most the keys can be scrolled.
    pub keys_most: usize,
    pub thumb: Option<Thumb>,
    /// Where the text goes.
    pub detail: Rect,
    /// Over the field, and the field, while it is open.
    pub field_label: Option<Rect>,
    pub field: Option<Rect>,
    /// The lines of the text shown.
    pub lines: Vec<(Rect, PlanLine)>,
    /// The most the text can be scrolled, and how many lines a page is.
    pub detail_most: usize,
    pub page: usize,
    pub detail_thumb: Option<Thumb>,
    pub notice: Option<Rect>,
    /// Close, Ask reviewer, the finishing one.
    pub buttons: [Rect; 3],
    pub hints: Vec<(usize, Rect)>,
}

/// How high the parts of the sheet besides the list and the text are.
fn fixed_height(view: &PlanSheetView) -> i32 {
    let notice = if view.notice.is_some() { QUIT_BODY } else { 0 };
    2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_BODY
        + SECTION_GAP
        + SECTION_GAP
        + notice
        + QUIT_BUTTON
        + footer_height(view.hints.len())
}

/// How high the sheet is on an output of `output`: the same however long
/// the suggestion under the cursor, cut to the output when that is short.
fn sheet_height(view: &PlanSheetView, output: Size<i32, Logical>) -> i32 {
    // Nothing to read: only as high as it takes to say so.
    if view.rows.is_empty() && view.keys.is_none() {
        let field = if view.editing.is_some() {
            SECTION_GAP + LINE + QUIT_BUTTON
        } else {
            0
        };
        return (fixed_height(view) + ROW + field).min(output.h);
    }
    (output.h - 2 * MARGIN)
        .min(HEIGHT)
        .max(fixed_height(view) + 2 * ROW)
        .min(output.h)
}

/// How high the list and the text can be together.
fn room(view: &PlanSheetView, output: Size<i32, Logical>) -> i32 {
    (sheet_height(view, output) - fixed_height(view)).max(ROW)
}

/// How many rows of `view` show at once, of the `room` there is.
fn rows_shown(view: &PlanSheetView, room: i32) -> usize {
    let share = room * LIST_SHARE.0 / LIST_SHARE.1;
    let most = ((share + ROW_GAP) / (ROW + ROW_GAP)).max(1) as usize;
    view.rows.len().min(most)
}

/// The first row to show so that the cursor shows, on an output of `output`.
pub fn plan_sheet_reveal(view: &PlanSheetView, output: Size<i32, Logical>) -> usize {
    let shown = rows_shown(view, room(view, output)).max(1);
    let cursor = view.cursor.min(view.rows.len().saturating_sub(1));
    let top = view.top.min(plan_sheet_most_top(view, output));
    if cursor < top {
        cursor
    } else if cursor >= top + shown {
        cursor + 1 - shown
    } else {
        top
    }
}

/// The most the rows can be scrolled down on an output of `output`.
pub fn plan_sheet_most_top(view: &PlanSheetView, output: Size<i32, Logical>) -> usize {
    view.rows
        .len()
        .saturating_sub(rows_shown(view, room(view, output)))
}

/// Where the sheet's parts go on an output of `output`: centred, as wide as
/// a dialog, what is past what fits scrolled.
pub fn plan_sheet_layout(view: &PlanSheetView, output: Size<i32, Logical>) -> PlanSheetLayout {
    let room = room(view, output);
    let height = fixed_height(view) + room;
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

    let mut layout = PlanSheetLayout {
        panel,
        title,
        sub,
        list: Rectangle::new((x, y).into(), (inner, room).into()),
        rows: Vec::new(),
        keys: Vec::new(),
        keys_most: 0,
        thumb: None,
        detail: Rectangle::new((x, y + room).into(), (inner, 0).into()),
        field_label: None,
        field: None,
        lines: Vec::new(),
        detail_most: 0,
        page: 1,
        detail_thumb: None,
        notice: None,
        buttons: [Rectangle::default(); 3],
        hints: Vec::new(),
    };
    if let Some(keys) = &view.keys {
        keys_part(&mut layout, keys.len(), view.keys_top);
    } else {
        rows_part(&mut layout, view, room);
        detail_part(&mut layout, view, room);
    }
    y += room + SECTION_GAP;
    layout.notice = view.notice.as_ref().map(|_| {
        let r = Rectangle::new((x, y).into(), (inner, QUIT_BODY).into());
        y += QUIT_BODY;
        r
    });
    let finish_w = button_width(view.finish);
    let finish = Rectangle::new(
        (x + inner - finish_w, y).into(),
        (finish_w, QUIT_BUTTON).into(),
    );
    let ask_w = button_width(ASK_LABEL);
    let ask = Rectangle::new(
        (finish.loc.x - QUIT_BUTTON_GAP - ask_w, y).into(),
        (ask_w, QUIT_BUTTON).into(),
    );
    let close = Rectangle::new(
        (x, y).into(),
        (button_width(CLOSE_LABEL), QUIT_BUTTON).into(),
    );
    layout.buttons = [close, ask, finish];
    y += QUIT_BUTTON;
    let footer = Rectangle::new(
        (panel.loc.x, y).into(),
        (width, footer_height(view.hints.len())).into(),
    );
    layout.hints = footer_hint_spots(&hint_strs(&view.hints), footer);
    layout
}

/// The key lines in the list's place, `top` the first shown.
fn keys_part(layout: &mut PlanSheetLayout, total: usize, top: usize) {
    let list = layout.list;
    let fit = ((list.size.h / KEY_LINE).max(1) as usize).min(total);
    layout.keys_most = total - fit;
    let top = top.min(layout.keys_most);
    layout.keys = (0..fit as i32)
        .map(|k| {
            Rectangle::new(
                (list.loc.x, list.loc.y + k * KEY_LINE).into(),
                (list.size.w, KEY_LINE).into(),
            )
        })
        .collect();
    layout.thumb = Thumb::of(
        gutter_track(list.loc.x + list.size.w, list),
        (fit as i32 * KEY_LINE) as f64,
        ListBar {
            down: (top as i32 * KEY_LINE) as f64,
            total: (total as i32 * KEY_LINE) as f64,
        },
    );
}

/// What the end of row `i` says, and the most room that takes: "Press Del
/// again" once armed, else that the reviewer left it out, else its place in
/// the plan.
fn row_end(view: &PlanSheetView, i: usize) -> (&str, i32) {
    let row = &view.rows[i];
    if view.armed == Some(i) {
        ("Press Del again", STATUS_W)
    } else if row.dropped {
        ("left out by the reviewer", STATUS_W)
    } else {
        (row.plan.as_str(), PLAN_W)
    }
}

/// Where a row's columns go, and the rows shown.
fn rows_part(layout: &mut PlanSheetLayout, view: &PlanSheetView, room: i32) {
    let (x, y, inner) = (layout.list.loc.x, layout.list.loc.y, layout.list.size.w);
    let shown = rows_shown(view, room);
    let list_h = if shown == 0 {
        ROW
    } else {
        shown as i32 * (ROW + ROW_GAP) - ROW_GAP
    };
    layout.list.size.h = list_h;
    let top = view.top.min(view.rows.len() - shown);
    let mark_w = ROW_INSET + MARK_ROOM;
    let label_w = SEVERITY_W + COLUMN_GAP + KIND_W;
    let right = x + inner - ROW_INSET;
    for (k, i) in (top..top + shown).enumerate() {
        let ry = y + k as i32 * (ROW + ROW_GAP);
        let (end_text, most) = row_end(view, i);
        let end_w =
            (measure(end_text, DETAIL_FONT_SIZE).unwrap_or(0.0).ceil() as i32 + 1).min(most);
        let text_x = x + mark_w + label_w + COLUMN_GAP;
        let end_x = right - end_w;
        layout.rows.push(PlanRowSpot {
            i,
            row: Rectangle::new((x, ry).into(), (inner, ROW).into()),
            mark: Rectangle::new((x, ry).into(), (mark_w, ROW).into()),
            label: Rectangle::new((x + mark_w, ry).into(), (label_w, ROW).into()),
            text: Rectangle::new(
                (text_x, ry).into(),
                ((end_x - COLUMN_GAP - text_x).max(1), ROW).into(),
            ),
            end: Rectangle::new((end_x, ry).into(), (end_w, ROW).into()),
        });
    }
    layout.thumb = Thumb::of(
        gutter_track(x + inner, layout.list),
        list_h as f64,
        ListBar {
            down: (top as i32 * (ROW + ROW_GAP)) as f64,
            total: (view.rows.len() as i32 * (ROW + ROW_GAP) - ROW_GAP) as f64,
        },
    );
}

/// The field and the lines of the text shown, under the list.
fn detail_part(layout: &mut PlanSheetLayout, view: &PlanSheetView, room: i32) {
    let (x, inner) = (layout.list.loc.x, layout.list.size.w);
    let mut y = layout.list.loc.y + layout.list.size.h + SECTION_GAP;
    let bottom = layout.list.loc.y + room;
    layout.detail = Rectangle::new((x, y).into(), (inner, (bottom - y).max(0)).into());
    if view.editing.is_some() {
        layout.field_label = Some(Rectangle::new((x, y).into(), (inner, LINE).into()));
        y += LINE;
        layout.field = Some(Rectangle::new((x, y).into(), (inner, QUIT_BUTTON).into()));
        y += QUIT_BUTTON + FIELD_GAP;
    }
    let Some(detail) = &view.detail else {
        return;
    };
    let lines = plan_lines(detail, inner - WRAP_SLACK);
    let text_h = (bottom - y).max(0);
    let total: i32 = lines.iter().map(PlanLine::height).sum();
    // The first line from which all the rest fit.
    let mut below = 0;
    let mut most = lines.len();
    while most > 0 && below + lines[most - 1].height() <= text_h {
        most -= 1;
        below += lines[most].height();
    }
    layout.detail_most = most.min(lines.len().saturating_sub(1));
    let top = view.detail_top.min(layout.detail_most);
    let mut shown = 0usize;
    let mut ly = y;
    for line in &lines[top..] {
        let h = line.height();
        if ly + h > bottom && shown > 0 {
            break;
        }
        layout.lines.push((
            Rectangle::new((x, ly).into(), (inner, h).into()),
            line.clone(),
        ));
        ly += h;
        shown += 1;
    }
    // A caption at the fold with what it names under it says nothing.
    if shown > 1 && top + shown < lines.len() && lines[top + shown - 1].piece == PlanPiece::Caption
    {
        layout.lines.pop();
        shown -= 1;
    }
    layout.page = shown.saturating_sub(1).max(1);
    let down: i32 = lines[..top].iter().map(PlanLine::height).sum();
    let track = Rectangle::new((x, y).into(), (inner, text_h).into());
    layout.detail_thumb = Thumb::of(
        gutter_track(x + inner, track),
        text_h as f64,
        ListBar {
            down: down as f64,
            total: total as f64,
        },
    );
}

impl PlanSheetLayout {
    /// What `pos` is over; `None` off the sheet.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<PlanHit> {
        if !self.panel.to_f64().contains(pos) {
            return None;
        }
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some((i, _)) = self.hints.iter().find(|(_, r)| on(r)) {
            return Some(PlanHit::Hint(*i));
        }
        if let Some(b) = self.buttons.iter().position(on) {
            return Some(PlanHit::Button(b));
        }
        if self.field.as_ref().is_some_and(on) {
            return Some(PlanHit::Field);
        }
        for spot in &self.rows {
            if on(&spot.mark) {
                return Some(PlanHit::Mark(spot.i));
            }
            if on(&spot.row) {
                return Some(PlanHit::Row(spot.i));
            }
        }
        for (rect, line) in &self.lines {
            if let (PlanPiece::Place(i), true) = (line.piece, on(rect)) {
                return Some(PlanHit::Citation(i));
            }
        }
        Some(PlanHit::Panel)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        let places = self
            .lines
            .iter()
            .filter(|(_, line)| matches!(line.piece, PlanPiece::Place(_)))
            .map(|(rect, _)| *rect);
        self.rows
            .iter()
            .map(|s| s.mark)
            .chain(places)
            .chain(self.field)
            .chain(self.buttons.iter().copied())
            .chain(self.hints.iter().map(|(_, r)| *r))
            .collect()
    }

    /// The rows shown, for the wheel's pitch.
    pub fn row_rects(&self) -> Vec<Rect> {
        self.rows.iter().map(|s| s.row).collect()
    }

    /// How far apart the text's lines are, for the wheel's pitch.
    pub fn line_pitch(&self) -> f64 {
        LINE as f64
    }

    /// How far apart the key lines are, for the wheel's pitch.
    pub fn key_pitch(&self) -> f64 {
        KEY_LINE as f64
    }
}

/// The text field of the sheet, while it is open, as the caret and the
/// pointer care.
pub fn plan_sheet_field(
    view: &PlanSheetView,
    output: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let editing = view.editing.as_ref()?;
    let area = plan_sheet_layout(view, output).field?;
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: editing.line.clone(),
    })
}

/// The whole title of the suggestion at `pos`, and where it shows, when the
/// row cuts it short: for a tooltip.
pub fn plan_sheet_tip(
    view: &PlanSheetView,
    output: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<(String, Rect)> {
    let layout = plan_sheet_layout(view, output);
    let spot = layout.rows.iter().find(|s| s.text.to_f64().contains(pos))?;
    let text = &view.rows.get(spot.i)?.title;
    let wide = measure(text, FONT_SIZE).unwrap_or(0.0);
    (wide > spot.text.size.w as f32).then(|| (text.clone(), spot.text))
}

/// A severity's colour: what weighs most stands out most.
fn severity_color(severity: Severity, p: &Palette) -> Rgb {
    match severity {
        Severity::High => p.error,
        Severity::Medium => p.warning,
        Severity::Low => p.muted,
    }
}

/// How many of `hints` the sheet's footer shows, at its full width.
#[cfg(test)]
pub fn footer_fit(hints: &[(&str, &str)]) -> usize {
    let footer = Rectangle::new(
        (0, 0).into(),
        (DIALOG_WIDTH, footer_height(hints.len())).into(),
    );
    footer_hint_spots(hints, footer).len()
}

impl Painter {
    /// The plan review's sheet on an output of `output`, inside a margin of
    /// `SHADOW` holding its shadow.
    pub(super) fn plan_sheet(
        &self,
        view: &PlanSheetView,
        output: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = plan_sheet_layout(view, output);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let s = scale as f32;
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let line = |c: &mut Canvas, text: &str, rect: Rect, size: f32, color| {
            let rect = shift(rect);
            let (x, end) = phys(rect, scale);
            self.text(c, text, size, x, end, band(rect, scale), scale, color)
        };
        line(&mut c, &view.title, layout.title, LABEL_FONT_SIZE, p.text);
        let after = line(&mut c, &view.sub, layout.sub, DETAIL_FONT_SIZE, p.muted);
        if let Some(missing) = &view.missing {
            let rect = shift(layout.sub);
            let (_, end) = phys(rect, scale);
            let gap = (COLUMN_GAP as f32 * s).round();
            self.text(
                &mut c,
                missing,
                DETAIL_FONT_SIZE,
                after + gap,
                end,
                band(rect, scale),
                scale,
                p.warning,
            );
        }
        if let Some(keys) = &view.keys {
            let top = view.keys_top.min(layout.keys_most);
            for (rect, (key, what)) in layout.keys.iter().zip(keys.iter().skip(top)) {
                let rect = shift(*rect);
                let (x, end) = phys(rect, scale);
                let after = self.keycaps(&mut c, key, x, band(rect, scale), scale, p);
                let gap = (COLUMN_GAP as f32 * s).round();
                self.text(
                    &mut c,
                    what,
                    SMALL_FONT_SIZE,
                    after + gap,
                    end,
                    band(rect, scale),
                    scale,
                    p.text,
                );
            }
        } else if view.rows.is_empty() {
            line(&mut c, "Nothing to change", layout.list, FONT_SIZE, p.muted);
        }
        for spot in &layout.rows {
            let Some(row) = view.rows.get(spot.i) else {
                continue;
            };
            let lit = matches!(
                view.hover,
                Some(PlanHit::Row(i) | PlanHit::Mark(i)) if i == spot.i
            );
            let selected = view.cursor == spot.i;
            self.row_highlight(
                &mut c,
                shift(spot.row),
                (selected, if lit { 1.0 } else { 0.0 }),
                scale,
                p,
            );
            // A lit row's ground is too close to the words' own colours.
            let ink = |color| if lit || selected { p.text } else { color };
            let r = shift(spot.row);
            let mark = Rectangle::new(
                (r.loc.x + ROW_INSET, r.loc.y + (ROW - MARK) / 2).into(),
                (MARK, MARK).into(),
            );
            match row.mark {
                PlanMark::Accepted => {
                    icon_sdf(&mut c, mark, scale, p.success, |x, y| check(x, y, s * 1.2))
                }
                PlanMark::Rejected => icon_sdf(&mut c, mark, scale, p.error, |x, y| {
                    cross(x, y, 0.75 * s) - 0.3 * s
                }),
                PlanMark::Open => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                    let r = 5.0 * s;
                    (x.hypot(y) - 0.75 * r).abs() - 0.25 * r
                }),
            }
            let label = shift(spot.label);
            let column = |c: &mut Canvas, text: &str, at: i32, w: i32, color| {
                let rect = Rectangle::new((label.loc.x + at, label.loc.y).into(), (w, ROW).into());
                let (x, end) = phys(rect, scale);
                self.text(
                    c,
                    text,
                    SMALL_FONT_SIZE,
                    x,
                    end,
                    band(rect, scale),
                    scale,
                    color,
                );
            };
            if row.yours {
                column(&mut c, "yours", 0, label.size.w, ink(p.accent));
            } else {
                let color = ink(severity_color(row.severity, p));
                column(&mut c, row.severity.label(), 0, SEVERITY_W, color);
                let at = SEVERITY_W + COLUMN_GAP;
                column(&mut c, row.kind.label(), at, KIND_W, ink(p.muted));
            }
            let color = match row.mark {
                PlanMark::Open => p.text,
                _ => ink(p.muted),
            };
            line(&mut c, &row.title, spot.text, FONT_SIZE, color);
            let (end, _) = row_end(view, spot.i);
            let color = ink(if view.armed == Some(spot.i) {
                p.warning
            } else {
                p.muted
            });
            line(&mut c, end, spot.end, DETAIL_FONT_SIZE, color);
        }
        for thumb in [&layout.thumb, &layout.detail_thumb].into_iter().flatten() {
            let mut moved = *thumb;
            moved.track = shift(thumb.track);
            moved.top += (SHADOW - panel.loc.y) as f64;
            self.thumb_in(&mut c, &moved, scale, p);
        }
        if let (Some(rect), Some(field)) = (layout.field_label, &view.editing) {
            line(&mut c, field.label, rect, DETAIL_FONT_SIZE, p.muted);
        }
        if let (Some(rect), Some(field)) = (layout.field, &view.editing) {
            self.field(
                &mut c,
                &field.line,
                field.placeholder,
                shift(rect),
                true,
                scale,
                p,
            );
        }
        for (rect, text) in &layout.lines {
            let (size, color) = match text.piece {
                PlanPiece::Caption => (DETAIL_FONT_SIZE, p.muted),
                PlanPiece::Body => (FONT_SIZE, p.text),
                PlanPiece::Aside | PlanPiece::Quote => (DETAIL_FONT_SIZE, p.muted),
                PlanPiece::Place(i) if view.hover == Some(PlanHit::Citation(i)) => {
                    (DETAIL_FONT_SIZE, p.text)
                }
                PlanPiece::Place(_) => (DETAIL_FONT_SIZE, p.accent),
                PlanPiece::Missing => (DETAIL_FONT_SIZE, p.warning),
            };
            // A caption's room is above it: it sits on what it names.
            let at = match text.piece {
                PlanPiece::Caption => Rectangle::new(
                    (rect.loc.x, rect.loc.y + CAPTION - LINE).into(),
                    (rect.size.w, LINE).into(),
                ),
                _ => *rect,
            };
            line(&mut c, &text.text, at, size, color);
        }
        if let (Some(rect), Some(notice)) = (layout.notice, &view.notice) {
            line(&mut c, notice, rect, FONT_SIZE, p.error);
        }
        let labels = [CLOSE_LABEL, ASK_LABEL, view.finish];
        for (i, (rect, label)) in layout.buttons.iter().zip(labels).enumerate() {
            let style = if view.primary == Some(i) {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(PlanHit::Button(i));
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
            Some(PlanHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hint_strs(&view.hints), &hints, hover, scale, p);
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: usize) -> PlanRow {
        PlanRow {
            id: format!("r1-{i}"),
            mark: PlanMark::Open,
            severity: Severity::High,
            kind: Kind::EdgeCase,
            title: "The retry loop has no cap".into(),
            plan: "Milestone 3".into(),
            yours: false,
            dropped: false,
            note: String::new(),
        }
    }

    fn detail(why: &str) -> PlanDetail {
        PlanDetail {
            title: "The retry loop has no cap".into(),
            plan: "Milestone 3".into(),
            dropped: false,
            question: false,
            why: why.into(),
            change: "Cap it at five".into(),
            evidence: vec![PlanCitation {
                place: "api: src/a.rs:12".into(),
                quote: "loop { retry() }".into(),
                not_found: true,
            }],
            note: String::new(),
        }
    }

    fn view(n: usize) -> PlanSheetView {
        PlanSheetView {
            title: "Plan review of \u{201c}Bulk edit\u{201d}".into(),
            sub: "3 to decide".into(),
            missing: None,
            rows: (0..n).map(row).collect(),
            cursor: 0,
            top: 0,
            detail: (n > 0).then(|| detail("It spins for ever when the server is down")),
            detail_top: 0,
            editing: None,
            armed: None,
            notice: None,
            hover: None,
            finish: "Write final plan",
            primary: None,
            keys: None,
            keys_top: 0,
            hints: vec![("Esc".into(), "close".into())],
        }
    }

    fn mid(r: Rect) -> Point<f64, Logical> {
        Point::from((
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        ))
    }

    #[test]
    fn it_fits_a_small_output_and_scrolls_the_rest() {
        let small: Size<i32, Logical> = (480, 420).into();
        let mut v = view(30);
        v.detail = Some(detail(&"It spins for ever. ".repeat(60)));
        let l = plan_sheet_layout(&v, small);
        assert!(l.panel.size.w <= 480 && l.panel.loc.x >= 0 && l.panel.loc.y >= 0);
        assert!(l.panel.loc.y + l.panel.size.h <= 420);
        assert!(!l.rows.is_empty() && l.rows.len() < 30);
        assert!(l.thumb.is_some() && l.detail_thumb.is_some());
        assert!(l.detail_most > 0, "the text scrolls");
        // Nothing of it runs past its room, or into the buttons.
        let bottom = l.detail.loc.y + l.detail.size.h;
        assert!(l.lines.iter().all(|(r, _)| r.loc.y + r.size.h <= bottom));
        assert!(bottom <= l.buttons[PLAN_CLOSE].loc.y);
        assert!(l.buttons[PLAN_ASK].loc.x > l.buttons[PLAN_CLOSE].loc.x);
        // The cursor past the fold scrolls into view.
        v.cursor = 20;
        v.top = plan_sheet_reveal(&v, small);
        let l = plan_sheet_layout(&v, small);
        assert!(l.rows.iter().any(|s| s.i == 20));
        assert!(plan_sheet_most_top(&v, small) > 0);
        assert_eq!(plan_sheet_most_top(&view(1), small), 0);
    }

    #[test]
    fn it_stays_as_high_as_it_is_whatever_the_cursor_says() {
        let output = (870, 720).into();
        let mut v = view(4);
        let short = plan_sheet_layout(&v, output).panel;
        v.detail = Some(detail(&"long ".repeat(400)));
        assert_eq!(plan_sheet_layout(&v, output).panel, short);
        v.detail = None;
        assert_eq!(plan_sheet_layout(&v, output).panel, short);
        assert_eq!(short.size.h, HEIGHT);
    }

    #[test]
    fn the_text_scrolls_to_its_last_line_and_no_further() {
        let output = (870, 720).into();
        let mut v = view(1);
        v.detail = Some(detail(&"word ".repeat(1000)));
        let l = plan_sheet_layout(&v, output);
        let all = plan_lines(v.detail.as_ref().unwrap(), l.detail.size.w - WRAP_SLACK);
        v.detail_top = usize::MAX;
        let end = plan_sheet_layout(&v, output);
        assert_eq!(end.lines.last().map(|(_, line)| line), all.last());
        assert_eq!(end.lines[0].1, all[end.detail_most]);
    }

    #[test]
    fn every_word_of_the_text_is_there() {
        let mut d = detail("first\nsecond (never closed and quite a lot more words to wrap");
        d.note = "a note".into();
        d.dropped = true;
        let lines = plan_lines(&d, 120);
        let all = (lines.iter().map(|l| l.text.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        let squeezed: String = all.split_whitespace().collect();
        for word in ["first", "second", "(never", "wrap", "Cap", "anote"] {
            assert!(squeezed.contains(word), "{word} in {squeezed}");
        }
        assert!(lines.iter().any(|l| l.piece == PlanPiece::Missing));
        assert!(lines.iter().any(|l| l.text == "left out by the reviewer"));
        assert!(lines.iter().any(|l| l.piece == PlanPiece::Place(0)));
        assert!((lines.iter()).all(|l| l.piece == PlanPiece::Caption
            || measure(&l.text, FONT_SIZE).unwrap_or(0.0) <= 124.0),);
    }

    #[test]
    fn hits_find_marks_rows_citations_and_buttons() {
        let v = view(3);
        let l = plan_sheet_layout(&v, (1920, 1080).into());
        assert_eq!(l.hit(mid(l.rows[2].mark)), Some(PlanHit::Mark(2)));
        assert_eq!(l.hit(mid(l.rows[2].text)), Some(PlanHit::Row(2)));
        assert_eq!(
            l.hit(mid(l.buttons[PLAN_FINISH])),
            Some(PlanHit::Button(PLAN_FINISH))
        );
        let (place, _) = (l.lines.iter())
            .find(|(_, line)| line.piece == PlanPiece::Place(0))
            .expect("the citation shows");
        assert_eq!(l.hit(mid(*place)), Some(PlanHit::Citation(0)));
        assert!(l.controls().contains(place));
        assert_eq!(l.hit((0.0, 0.0).into()), None);
    }

    #[test]
    fn the_field_goes_over_the_text_and_takes_its_room() {
        let output = (1920, 1080).into();
        let mut v = view(3);
        let before = plan_sheet_layout(&v, output).lines[0].0.loc.y;
        v.editing = Some(PlanField {
            label: "Your note",
            placeholder: "",
            line: Line::default(),
        });
        let l = plan_sheet_layout(&v, output);
        let field = l.field.expect("the field");
        assert_eq!(l.hit(mid(field)), Some(PlanHit::Field));
        assert!(l.lines[0].0.loc.y >= field.loc.y + field.size.h);
        assert!(l.lines[0].0.loc.y > before);
        assert!(plan_sheet_field(&v, output, 1.0).is_some());
    }

    #[test]
    fn the_words_read_on_the_sheet_and_its_rows() {
        use super::super::{Theme, sidebar::contrast};
        for theme in [Theme::Dark, Theme::Light] {
            let p = theme.palette();
            // On a row not lit, in their own colours.
            for severity in [Severity::High, Severity::Medium, Severity::Low] {
                let ratio = contrast(severity_color(severity, p), p.elevated);
                assert!(ratio >= 4.5, "{theme:?} {severity:?} is {ratio:.2}:1");
            }
            for (what, color) in [
                ("accent", p.accent),
                ("warning", p.warning),
                ("muted", p.muted),
            ] {
                let ratio = contrast(color, p.elevated);
                assert!(ratio >= 4.5, "{theme:?} {what} is {ratio:.2}:1");
            }
            // On a lit one, they all go to the text's colour.
            for ground in [p.selected, p.element] {
                let ratio = contrast(p.text, ground);
                assert!(ratio >= 4.5, "{theme:?} text on a lit row is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn no_suggestions_leave_no_rows_or_text() {
        let l = plan_sheet_layout(&view(0), (870, 720).into());
        assert!(l.rows.is_empty() && l.lines.is_empty() && l.thumb.is_none());
        assert!(l.panel.size.h < HEIGHT / 2, "no room kept for nothing");
        // Adding one of the user's own has room for its field.
        let mut v = view(0);
        v.editing = Some(PlanField {
            label: "Your own suggestion",
            placeholder: "",
            line: Line::default(),
        });
        let l = plan_sheet_layout(&v, (870, 720).into());
        let field = l.field.expect("the field");
        assert!(field.loc.y + field.size.h <= l.buttons[PLAN_CLOSE].loc.y);
        assert!(field.loc.y >= l.list.loc.y + l.list.size.h);
    }

    #[test]
    fn the_keys_scroll_when_they_do_not_all_fit() {
        let keys: Vec<_> = (0..14).map(|i| (format!("k{i}"), "x".to_owned())).collect();
        let mut v = view(3);
        v.keys = Some(keys);
        let tall = plan_sheet_layout(&v, (1920, 1080).into());
        assert_eq!((tall.keys.len(), tall.keys_most), (14, 0));
        let short = plan_sheet_layout(&v, (480, 420).into());
        assert!(short.keys.len() < 14 && short.keys_most > 0);
        assert!(short.thumb.is_some());
    }
}
