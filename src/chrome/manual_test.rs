//! The manual test sheet (see `crate::manual_test`): a feature's
//! acceptance criteria, one row each with its mark, its id, its text and a
//! Fail button, a failure's note under its row, then Close, Plan fixes
//! and Mark passed over the key hints.

use super::{
    ButtonStyle, Canvas, DETAIL_FONT_SIZE, DIALOG_WIDTH, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE,
    ListBar, POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_PADDING,
    QUIT_TITLE, Rect, SHADOW, SMALL_FONT_SIZE, Thumb, band, button_width, check, cross, field_text,
    footer_height, footer_hint_spots, gutter_track, hint_strs, icon_sdf, measure, phys,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How high a criterion's row is, and the gap between rows.
const ROW: i32 = 30;
const ROW_GAP: i32 = 2;
/// How high a failure's note under its row is.
const NOTE: i32 = 20;
/// How high a line of the key list is.
const KEY_LINE: i32 = 28;
/// The gap between the sheet's parts.
const SECTION_GAP: i32 = 12;
/// How far a row's contents sit in from its edges.
const ROW_INSET: i32 = 6;
/// How big a criterion's mark is, and where its id starts after it.
const MARK: i32 = 14;
const MARK_ROOM: i32 = 26;
/// How high a row's Fail button is.
const ROW_BUTTON: i32 = 24;
/// The gap between a row's id and its text.
const ID_GAP: i32 = 8;
/// How wide a criterion's id is given: "AC-12" fits.
const ID_W: i32 = 44;
/// The room kept above and below the sheet on a short output.
const MARGIN: i32 = 40;
/// The label of a row's Fail button: a note to write follows.
pub const FAIL_LABEL: &str = "Fail\u{2026}";
/// The sheet's buttons: Close at the left, the others at the right.
pub const MANUAL_BUTTONS: [&str; 3] = ["Close", "Plan fixes", "Mark passed"];
pub const MANUAL_CLOSE: usize = 0;
pub const MANUAL_SEND: usize = 1;
pub const MANUAL_PASS: usize = 2;

/// What the user found of a criterion, as the sheet shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ManualMark {
    Untested,
    Passed,
    Failed,
}

/// A criterion's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualRow {
    /// `AC-1`…
    pub id: String,
    pub text: String,
    pub mark: ManualMark,
    /// What went wrong, for a failure.
    pub note: String,
    /// The failure went to the Build agent.
    pub sent: bool,
}

/// What the manual test sheet shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualTestView {
    /// "Manual test of “title”".
    pub title: String,
    /// The counts, or what is going on.
    pub sub: String,
    pub rows: Vec<ManualRow>,
    /// The row with the keyboard.
    pub cursor: usize,
    /// The first row shown.
    pub top: usize,
    /// The row whose note is being written, and the note.
    pub editing: Option<(usize, Line)>,
    /// Why the last thing asked was refused.
    pub notice: Option<String>,
    pub hover: Option<ManualHit>,
    /// The button drawn as the one to press next (`MANUAL_PASS` once
    /// every criterion passed, `MANUAL_SEND` while failures wait to be
    /// sent), if one is.
    pub primary: Option<usize>,
    /// The keys and what they do, shown instead of the rows (F1).
    pub keys: Option<Vec<(String, String)>>,
    pub hints: Vec<(String, String)>,
}

/// What the pointer is over in the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualHit {
    /// A row, but not its mark or button.
    Row(usize),
    /// A row's mark: a click toggles passed.
    Mark(usize),
    /// A row's Fail button.
    Fail(usize),
    /// The note field.
    Field,
    /// Close, Plan fixes, Mark passed.
    Button(usize),
    Hint(usize),
    /// Somewhere else on it.
    Panel,
}

/// Where a shown row's parts go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManualRowSpot {
    /// Index into `ManualTestView::rows`.
    pub i: usize,
    pub row: Rect,
    pub mark: Rect,
    /// Where its text goes, up to its button.
    pub text: Rect,
    pub fail: Rect,
    /// The note under it, for a failure.
    pub note: Option<Rect>,
    /// The note field, while its note is written.
    pub field: Option<Rect>,
}

/// Where the sheet's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct ManualTestLayout {
    pub panel: Rect,
    pub title: Rect,
    pub sub: Rect,
    /// Where the rows (or the keys) go.
    pub list: Rect,
    pub rows: Vec<ManualRowSpot>,
    /// One per key line shown, with F1.
    pub keys: Vec<Rect>,
    pub thumb: Option<Thumb>,
    pub notice: Option<Rect>,
    /// Close, Plan fixes, Mark passed.
    pub buttons: [Rect; 3],
    pub hints: Vec<(usize, Rect)>,
}

/// How high row `i` of `view` is, with what goes under it.
fn row_height(view: &ManualTestView, i: usize) -> i32 {
    let editing = view.editing.as_ref().is_some_and(|(at, _)| *at == i);
    let failed = view
        .rows
        .get(i)
        .is_some_and(|r| r.mark == ManualMark::Failed);
    ROW + if editing {
        ROW_GAP + QUIT_BUTTON
    } else if failed {
        NOTE
    } else {
        0
    }
}

/// How many rows of `view` from `top` fit in `room`: at least one.
fn fitting(view: &ManualTestView, top: usize, room: i32) -> usize {
    let mut used = 0;
    let mut n = 0;
    for i in top..view.rows.len() {
        let h = row_height(view, i) + ROW_GAP;
        if n > 0 && used + h > room {
            break;
        }
        used += h;
        n += 1;
    }
    n.max(1).min(view.rows.len().saturating_sub(top).max(1))
}

/// The first row to show so that `cursor` shows, from `top`, on an output
/// of `output`.
pub fn manual_reveal(view: &ManualTestView, output: Size<i32, Logical>) -> usize {
    let room = list_room(view, output);
    let cursor = view.cursor.min(view.rows.len().saturating_sub(1));
    let mut top = view.top.min(cursor);
    while top < cursor && top + fitting(view, top, room) <= cursor {
        top += 1;
    }
    top
}

/// The most the rows can be scrolled down on an output of `output`.
pub fn manual_most_top(view: &ManualTestView, output: Size<i32, Logical>) -> usize {
    let room = list_room(view, output);
    let mut top = view.rows.len().saturating_sub(1);
    while top > 0 && fitting(view, top - 1, room) >= view.rows.len() - (top - 1) {
        top -= 1;
    }
    top
}

/// How high the parts of the sheet besides its list are.
fn fixed_height(view: &ManualTestView) -> i32 {
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

/// How high the list can be on an output of `output`.
fn list_room(view: &ManualTestView, output: Size<i32, Logical>) -> i32 {
    (output.h - 2 * MARGIN - fixed_height(view)).max(ROW)
}

/// Where the sheet's parts go on an output of `output`: centred, as wide
/// as a dialog, the rows past what fits scrolled.
pub fn manual_test_layout(view: &ManualTestView, output: Size<i32, Logical>) -> ManualTestLayout {
    let room = list_room(view, output);
    let shown: Vec<usize> = if view.keys.is_some() {
        Vec::new()
    } else {
        let top = view.top.min(view.rows.len().saturating_sub(1));
        (top..top + fitting(view, top, room))
            .filter(|i| *i < view.rows.len())
            .collect()
    };
    let list_h = match &view.keys {
        Some(keys) => (keys.len() as i32 * KEY_LINE).min(room / KEY_LINE * KEY_LINE),
        None if shown.is_empty() => ROW,
        None => {
            shown
                .iter()
                .map(|i| row_height(view, *i) + ROW_GAP)
                .sum::<i32>()
                - ROW_GAP
        }
    };
    let footer_h = footer_height(view.hints.len());
    let height = fixed_height(view) + list_h;
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
    let list = Rectangle::new((x, y).into(), (inner, list_h).into());
    let fail_w = button_width(FAIL_LABEL);
    let mut rows = Vec::new();
    let mut ry = y;
    for i in shown {
        let row = Rectangle::new((x, ry).into(), (inner, ROW).into());
        let fail = Rectangle::new(
            (x + inner - ROW_INSET - fail_w, ry + (ROW - ROW_BUTTON) / 2).into(),
            (fail_w, ROW_BUTTON).into(),
        );
        let mark = Rectangle::new((x, ry).into(), (ROW_INSET + MARK_ROOM, ROW).into());
        let text_x = x + ROW_INSET + MARK_ROOM + ID_W + ID_GAP;
        let text = Rectangle::new(
            (text_x, ry).into(),
            ((fail.loc.x - QUIT_BUTTON_GAP - text_x).max(1), ROW).into(),
        );
        let under = Rectangle::new(
            (text_x, ry + ROW).into(),
            ((x + inner - ROW_INSET - text_x).max(1), NOTE).into(),
        );
        let editing = view.editing.as_ref().is_some_and(|(at, _)| *at == i);
        let field = editing.then(|| {
            Rectangle::new(
                (text_x, ry + ROW + ROW_GAP).into(),
                (under.size.w, QUIT_BUTTON).into(),
            )
        });
        let failed = view.rows[i].mark == ManualMark::Failed;
        let note = (failed && !editing).then_some(under);
        rows.push(ManualRowSpot {
            i,
            row,
            mark,
            text,
            fail,
            note,
            field,
        });
        ry += row_height(view, i) + ROW_GAP;
    }
    let keys = match &view.keys {
        Some(keys) => (0..keys.len().min((list_h / KEY_LINE) as usize) as i32)
            .map(|k| Rectangle::new((x, y + k * KEY_LINE).into(), (inner, KEY_LINE).into()))
            .collect(),
        None => Vec::new(),
    };
    let thumb = view.keys.is_none().then(|| {
        let total: i32 = (0..view.rows.len())
            .map(|i| row_height(view, i) + ROW_GAP)
            .sum::<i32>()
            - ROW_GAP;
        let down: i32 = (0..view.top.min(view.rows.len()))
            .map(|i| row_height(view, i) + ROW_GAP)
            .sum();
        let track = gutter_track(x + inner, list);
        Thumb::of(
            track,
            list_h as f64,
            ListBar {
                down: down as f64,
                total: total as f64,
            },
        )
    });
    y += list_h + SECTION_GAP;
    let notice = view.notice.as_ref().map(|_| {
        let r = Rectangle::new((x, y).into(), (inner, QUIT_BODY).into());
        y += QUIT_BODY;
        r
    });
    let close = Rectangle::new(
        (x, y).into(),
        (button_width(MANUAL_BUTTONS[MANUAL_CLOSE]), QUIT_BUTTON).into(),
    );
    let pass_w = button_width(MANUAL_BUTTONS[MANUAL_PASS]);
    let pass = Rectangle::new((x + inner - pass_w, y).into(), (pass_w, QUIT_BUTTON).into());
    let send_w = button_width(MANUAL_BUTTONS[MANUAL_SEND]);
    let send = Rectangle::new(
        (pass.loc.x - QUIT_BUTTON_GAP - send_w, y).into(),
        (send_w, QUIT_BUTTON).into(),
    );
    y += QUIT_BUTTON;
    let footer = Rectangle::new((panel.loc.x, y).into(), (width, footer_h).into());
    let hints = footer_hint_spots(&hint_strs(&view.hints), footer);
    ManualTestLayout {
        panel,
        title,
        sub,
        list,
        rows,
        keys,
        thumb: thumb.flatten(),
        notice,
        buttons: [close, send, pass],
        hints,
    }
}

impl ManualTestLayout {
    /// What `pos` is over; `None` off the sheet.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<ManualHit> {
        if !self.panel.to_f64().contains(pos) {
            return None;
        }
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some((i, _)) = self.hints.iter().find(|(_, r)| on(r)) {
            return Some(ManualHit::Hint(*i));
        }
        if let Some(b) = self.buttons.iter().position(on) {
            return Some(ManualHit::Button(b));
        }
        for spot in &self.rows {
            if spot.field.as_ref().is_some_and(on) {
                return Some(ManualHit::Field);
            }
            if on(&spot.fail) {
                return Some(ManualHit::Fail(spot.i));
            }
            if on(&spot.mark) {
                return Some(ManualHit::Mark(spot.i));
            }
            let mut whole = spot.row;
            whole.size.h += spot.note.map_or(0, |n| n.size.h);
            if on(&whole) {
                return Some(ManualHit::Row(spot.i));
            }
        }
        Some(ManualHit::Panel)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        self.rows
            .iter()
            .flat_map(|s| [Some(s.mark), Some(s.fail), s.field])
            .flatten()
            .chain(self.buttons.iter().copied())
            .chain(self.hints.iter().map(|(_, r)| *r))
            .collect()
    }

    /// The rows shown, for the wheel's pitch.
    pub fn row_rects(&self) -> Vec<Rect> {
        self.rows.iter().map(|s| s.row).collect()
    }
}

/// The note field of the sheet, while a note is written, as the caret
/// and the pointer care.
pub fn manual_test_field(
    view: &ManualTestView,
    output: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let (_, line) = view.editing.as_ref()?;
    let area = manual_test_layout(view, output)
        .rows
        .iter()
        .find_map(|s| s.field)?;
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: line.clone(),
    })
}

/// The whole text of the criterion at `pos`, and where it shows, when
/// the row cuts it short: for a tooltip.
pub fn manual_test_tip(
    view: &ManualTestView,
    output: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<(String, Rect)> {
    let layout = manual_test_layout(view, output);
    let spot = layout.rows.iter().find(|s| s.text.to_f64().contains(pos))?;
    let text = &view.rows.get(spot.i)?.text;
    let wide = measure(text, FONT_SIZE).unwrap_or(0.0);
    (wide > spot.text.size.w as f32).then(|| (text.clone(), spot.text))
}

impl Painter {
    /// The manual test sheet on an output of `output`, inside a margin of
    /// `SHADOW` holding its shadow.
    pub(super) fn manual_test_sheet(
        &self,
        view: &ManualTestView,
        output: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = manual_test_layout(view, output);
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
        line(&mut c, &view.sub, layout.sub, DETAIL_FONT_SIZE, p.muted);

        if let Some(keys) = &view.keys {
            for (rect, (key, what)) in layout.keys.iter().zip(keys) {
                let rect = shift(*rect);
                let (x, end) = phys(rect, scale);
                let after = self.keycaps(&mut c, key, x, band(rect, scale), scale, p);
                let gap = (ID_GAP as f32 * s).round();
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
        }
        if view.keys.is_none() && view.rows.is_empty() {
            line(
                &mut c,
                "The brief has no acceptance criteria",
                layout.list,
                FONT_SIZE,
                p.muted,
            );
        }
        for spot in &layout.rows {
            let Some(row) = view.rows.get(spot.i) else {
                continue;
            };
            let lit = matches!(
                view.hover,
                Some(ManualHit::Row(i) | ManualHit::Mark(i) | ManualHit::Fail(i)) if i == spot.i
            );
            let mut whole = shift(spot.row);
            whole.size.h += spot.note.map_or(0, |n| n.size.h);
            self.row_highlight(
                &mut c,
                whole,
                (view.cursor == spot.i, if lit { 1.0 } else { 0.0 }),
                scale,
                p,
            );
            let r = shift(spot.row);
            let mark = Rectangle::new(
                (r.loc.x + ROW_INSET, r.loc.y + (ROW - MARK) / 2).into(),
                (MARK, MARK).into(),
            );
            match row.mark {
                ManualMark::Passed => {
                    icon_sdf(&mut c, mark, scale, p.success, |x, y| check(x, y, s * 1.2))
                }
                ManualMark::Failed => icon_sdf(&mut c, mark, scale, p.error, |x, y| {
                    cross(x, y, 0.75 * s) - 0.3 * s
                }),
                ManualMark::Untested => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                    let r = 5.0 * s;
                    (x.hypot(y) - 0.75 * r).abs() - 0.25 * r
                }),
            }
            let id = Rectangle::new(
                (r.loc.x + ROW_INSET + MARK_ROOM, r.loc.y).into(),
                (ID_W, ROW).into(),
            );
            let (ix, iend) = phys(id, scale);
            self.text(
                &mut c,
                &row.id,
                SMALL_FONT_SIZE,
                ix,
                iend,
                band(id, scale),
                scale,
                p.muted,
            );
            let color = match row.mark {
                ManualMark::Untested => p.text,
                _ => p.muted,
            };
            line(&mut c, &row.text, spot.text, FONT_SIZE, color);
            let hovered = view.hover == Some(ManualHit::Fail(spot.i));
            self.button(
                &mut c,
                FAIL_LABEL,
                shift(spot.fail),
                (hovered, false),
                ButtonStyle::Plain,
                scale,
                p,
            );
            if let Some(note) = spot.note {
                let rect = shift(note);
                let (x, end) = phys(rect, scale);
                let text = if row.note.is_empty() {
                    "failed".to_owned()
                } else {
                    row.note.clone()
                };
                let after = self.text(
                    &mut c,
                    &text,
                    DETAIL_FONT_SIZE,
                    x,
                    end,
                    band(rect, scale),
                    scale,
                    p.error,
                );
                if row.sent {
                    let gap = (ID_GAP as f32 * s).round();
                    self.text(
                        &mut c,
                        "\u{b7} sent to Build",
                        DETAIL_FONT_SIZE,
                        after + gap,
                        end,
                        band(rect, scale),
                        scale,
                        p.muted,
                    );
                }
            }
            if let (Some(field), Some((_, text))) = (spot.field, &view.editing) {
                self.field(
                    &mut c,
                    text,
                    "What went wrong",
                    shift(field),
                    true,
                    scale,
                    p,
                );
            }
        }
        if let Some(thumb) = &layout.thumb {
            let mut moved = *thumb;
            moved.track = shift(thumb.track);
            moved.top += (SHADOW - panel.loc.y) as f64;
            self.thumb_in(&mut c, &moved, scale, p);
        }
        if let (Some(rect), Some(notice)) = (layout.notice, &view.notice) {
            line(&mut c, notice, rect, FONT_SIZE, p.error);
        }
        for (i, (rect, label)) in layout.buttons.iter().zip(MANUAL_BUTTONS).enumerate() {
            let style = if view.primary == Some(i) {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(ManualHit::Button(i));
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
            Some(ManualHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hint_strs(&view.hints), &hints, hover, scale, p);
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(n: usize) -> ManualTestView {
        ManualTestView {
            title: "Manual test of \u{201c}Bulk edit\u{201d}".into(),
            sub: "0 of 3 criteria passed".into(),
            rows: (0..n)
                .map(|i| ManualRow {
                    id: format!("AC-{}", i + 1),
                    text: "Given two rows, when I edit both, then both change".into(),
                    mark: if i == 1 {
                        ManualMark::Failed
                    } else {
                        ManualMark::Untested
                    },
                    note: "nothing happens".into(),
                    sent: false,
                })
                .collect(),
            cursor: 0,
            top: 0,
            editing: None,
            notice: None,
            hover: None,
            primary: None,
            keys: None,
            hints: vec![("Esc".into(), "close".into())],
        }
    }

    #[test]
    fn it_fits_a_small_output_and_scrolls_the_rest() {
        let small = (860, 420).into();
        let mut v = view(30);
        let l = manual_test_layout(&v, small);
        assert!(l.panel.size.w <= 860 && l.panel.loc.x >= 0);
        assert!(l.panel.loc.y + l.panel.size.h <= 420);
        assert!(l.rows.len() < 30 && !l.rows.is_empty());
        assert!(l.thumb.is_some());
        // The cursor past the fold scrolls into view.
        v.cursor = 20;
        v.top = manual_reveal(&v, small);
        let l = manual_test_layout(&v, small);
        assert!(l.rows.iter().any(|s| s.i == 20));
        assert!(manual_most_top(&v, small) > 0);
        assert!(manual_most_top(&view(2), small) == 0);
    }

    #[test]
    fn a_failure_shows_its_note_and_the_editor_its_field() {
        let mut v = view(3);
        let l = manual_test_layout(&v, (1920, 1080).into());
        assert!(l.rows[1].note.is_some() && l.rows[0].note.is_none());
        assert!(l.thumb.is_none());
        v.editing = Some((0, Line::default()));
        let l = manual_test_layout(&v, (1920, 1080).into());
        let field = l.rows[0].field.expect("the field");
        assert!(field.loc.y >= l.rows[0].row.loc.y + ROW);
        assert!(l.rows[1].row.loc.y >= field.loc.y + field.size.h);
        assert_eq!(
            l.hit(field.to_f64().loc + Point::from((2.0, 2.0))),
            Some(ManualHit::Field)
        );
    }

    #[test]
    fn hits_find_marks_buttons_and_rows() {
        let v = view(3);
        let l = manual_test_layout(&v, (1920, 1080).into());
        let mid = |r: Rect| {
            Point::from((
                r.loc.x as f64 + r.size.w as f64 / 2.0,
                r.loc.y as f64 + r.size.h as f64 / 2.0,
            ))
        };
        assert_eq!(l.hit(mid(l.rows[2].mark)), Some(ManualHit::Mark(2)));
        assert_eq!(l.hit(mid(l.rows[2].fail)), Some(ManualHit::Fail(2)));
        assert_eq!(l.hit(mid(l.rows[2].text)), Some(ManualHit::Row(2)));
        assert_eq!(
            l.hit(mid(l.buttons[MANUAL_PASS])),
            Some(ManualHit::Button(MANUAL_PASS))
        );
        assert!(l.buttons[MANUAL_CLOSE].loc.x < l.buttons[MANUAL_SEND].loc.x);
        assert_eq!(l.hit((0.0, 0.0).into()), None);
    }
}
