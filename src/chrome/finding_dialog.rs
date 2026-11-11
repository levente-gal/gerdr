//! The dialog asking for a finding's text (see `crate::finding_dialog`):
//! why it is dismissed, its fix and the user's comment, or a comment of the
//! user's own. Its fields hold lines, wrapped to the field.

use std::ops::Range;

use super::{
    BUTTON_RADIUS, ButtonStyle, Canvas, DIALOG_WIDTH, FONT_SIZE, LABEL_FONT_SIZE, PADDING,
    POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING,
    QUIT_TITLE, Rect, Rgb, SHADOW, band, caret, dialog_buttons, measure, phys, rect_px,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How high a line of a field is.
const ROW: i32 = 22;
/// The room above and below a field's lines.
const FIELD_PAD: i32 = 6;
/// The Cancel button's index, then the action's.
pub const FINDING_DIALOG_ACTION: usize = 1;
/// The room for the caret at a line's end.
const CARET_ROOM: i32 = 4;

/// A field of the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingFieldView {
    pub label: &'static str,
    pub line: Line,
    pub placeholder: &'static str,
    /// How many lines it shows; the caret's is always among them.
    pub rows: i32,
}

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingDialogView {
    pub title: String,
    /// What it is about: the finding's title.
    pub about: String,
    pub fields: Vec<FindingFieldView>,
    /// The field with the keyboard.
    pub focus: usize,
    /// What its main button says.
    pub action: &'static str,
    /// What is wrong with the form, as a sentence.
    pub error: Option<String>,
    pub hover: Option<FindingDialogHit>,
    /// The keys besides the buttons, said left of them.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// What the pointer is over in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FindingDialogHit {
    Field(usize),
    /// 0 Cancel, 1 the action.
    Button(usize),
}

/// Where the dialog's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingDialogLayout {
    pub panel: Rect,
    pub title: Rect,
    pub about: Rect,
    pub labels: Vec<Rect>,
    pub fields: Vec<Rect>,
    pub error: Option<Rect>,
    pub buttons: [Rect; 2],
}

/// Where the dialog's parts go, centred on an output of `size`.
pub fn finding_dialog_layout(
    view: &FindingDialogView,
    size: Size<i32, Logical>,
) -> FindingDialogLayout {
    let field_h = |rows: i32| rows * ROW + 2 * FIELD_PAD;
    let fields_h: i32 = (view.fields.iter())
        .map(|f| QUIT_BODY + QUIT_LINE_GAP + field_h(f.rows) + QUIT_LINE_GAP)
        .sum();
    let error_h = match view.error {
        Some(_) => QUIT_BODY + QUIT_LINE_GAP,
        None => 0,
    };
    let height = 2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_LINE_GAP
        + QUIT_BODY
        + QUIT_LINE_GAP
        + fields_h
        + error_h
        + super::QUIT_BUTTON;
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
    let about = next(QUIT_BODY);
    let mut labels = Vec::new();
    let mut fields = Vec::new();
    for f in &view.fields {
        labels.push(next(QUIT_BODY));
        fields.push(next(field_h(f.rows)));
    }
    let error = view.error.as_ref().map(|_| next(QUIT_BODY));
    let buttons = dialog_buttons(panel, &["Cancel", view.action]);
    FindingDialogLayout {
        panel,
        title,
        about,
        labels,
        fields,
        error,
        buttons: [buttons[0], buttons[1]],
    }
}

impl FindingDialogLayout {
    /// What `pos` is over.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<FindingDialogHit> {
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some(button) = self.buttons.iter().position(on) {
            return Some(FindingDialogHit::Button(button));
        }
        // A click on a label goes to its field, as on a form.
        let row = |i: usize| on(&self.labels[i].merge(self.fields[i]));
        (0..self.fields.len())
            .find(|i| row(*i))
            .map(FindingDialogHit::Field)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        self.buttons.to_vec()
    }
}

/// How wide the text of a field of an output `size` wide may run.
fn text_width(size: Size<i32, Logical>) -> f32 {
    (DIALOG_WIDTH.min(size.w) - 2 * QUIT_PADDING - 2 * PADDING as i32 - CARET_ROOM).max(1) as f32
}

/// How wide `text` is, guessed when there is no font.
fn width(text: &str) -> f32 {
    measure(text, FONT_SIZE).unwrap_or(text.chars().count() as f32 * FONT_SIZE * 0.55)
}

/// `text`'s lines as shown in a field `room` wide, as byte ranges: at its
/// line breaks, and at the last space that fits, else the last character.
fn soft_rows(text: &str, room: f32) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut at = 0;
    for part in text.split('\n') {
        let end = at + part.len();
        let mut start = at;
        let mut space = None;
        for (i, c) in part.char_indices() {
            let (here, after) = (at + i, at + i + c.len_utf8());
            if width(&text[start..after]) > room && here > start {
                let to = space.filter(|s| *s > start).unwrap_or(here);
                rows.push(start..to);
                start = to;
            }
            if c == ' ' {
                space = Some(after);
            }
        }
        rows.push(start..end);
        at = end + 1;
    }
    rows
}

/// The row of `rows` byte `caret` is on: at a wrap, the one it starts.
fn row_of(rows: &[Range<usize>], caret: usize) -> usize {
    rows.iter()
        .rposition(|r| r.start <= caret)
        .unwrap_or_default()
}

/// The byte of `text` in `row` nearest `x` from the row's start.
fn byte_at(text: &str, row: &Range<usize>, x: f32) -> usize {
    let line = &text[row.clone()];
    let mut best = (row.start, f32::MAX);
    let ends = line.char_indices().map(|(i, _)| i).chain([line.len()]);
    for i in ends {
        let off = (width(&line[..i]) - x).abs();
        if off < best.1 {
            best = (row.start + i, off);
        }
    }
    best.0
}

/// Where Up (`up`) or Down moves the caret of `line`, in a field on an
/// output of `size`; none on the first or the last line shown.
pub fn finding_field_step(line: &Line, up: bool, size: Size<i32, Logical>) -> Option<usize> {
    let rows = soft_rows(line, text_width(size));
    let at = row_of(&rows, line.caret);
    let to = if up { at.checked_sub(1)? } else { at + 1 };
    let row = rows.get(to)?;
    let x = width(&line[rows[at].start..line.caret]);
    Some(byte_at(line, row, x))
}

/// The first of `rows` a field showing `shown` of them shows: the one its
/// view started at (`Line::scroll`), moved only as far as the caret's row
/// needs to be among them, and back for the last row to fill the field.
fn first_shown(rows: &[Range<usize>], line: &Line, shown: i32) -> usize {
    let shown = shown.max(1) as usize;
    let caret = row_of(rows, line.caret);
    let from = row_of(rows, line.scroll);
    let fill = rows.len().saturating_sub(shown);
    from.clamp(caret.saturating_sub(shown - 1), caret).min(fill)
}

/// The byte field `i`'s view starts at as it is drawn (see `Line::scroll`):
/// the field keeps it for the next frame.
pub fn finding_field_first(
    view: &FindingDialogView,
    i: usize,
    size: Size<i32, Logical>,
) -> Option<usize> {
    let field = view.fields.get(i)?;
    let rows = soft_rows(&field.line, text_width(size));
    Some(rows.get(first_shown(&rows, &field.line, field.rows))?.start)
}

/// A press in field `i` held `by` rows past its shown ones (up negative),
/// the pointer at output x `x`: the byte the selection reaches, on the row
/// `by` from the caret's, as far as there are.
pub fn finding_field_past(
    view: &FindingDialogView,
    i: usize,
    size: Size<i32, Logical>,
    by: i64,
    x: f64,
) -> Option<usize> {
    let layout = finding_dialog_layout(view, size);
    let (field, rect) = (view.fields.get(i)?, *layout.fields.get(i)?);
    let rows = soft_rows(&field.line, text_width(size));
    let at = row_of(&rows, field.line.caret) as i64;
    let to = (at + by).clamp(0, rows.len() as i64 - 1) as usize;
    let x = x as f32 - (rect.loc.x as f32 + PADDING);
    Some(byte_at(&field.line, rows.get(to)?, x.max(0.0)))
}

/// How far `pos` is past field `i`'s shown rows, in logical pixels: above
/// them negative, below positive, none within.
pub fn finding_field_edge(
    view: &FindingDialogView,
    i: usize,
    size: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> f64 {
    let layout = finding_dialog_layout(view, size);
    let (Some(field), Some(rect)) = (view.fields.get(i), layout.fields.get(i)) else {
        return 0.0;
    };
    let top = (rect.loc.y + FIELD_PAD) as f64;
    let bottom = top + (field.rows.max(1) * ROW) as f64;
    match pos.y {
        y if y < top => y - top,
        y if y >= bottom => y - bottom + 1.0,
        _ => 0.0,
    }
}

/// The byte a click at `pos` falls on in field `i` of the dialog.
pub fn finding_field_at(
    view: &FindingDialogView,
    i: usize,
    size: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<usize> {
    let layout = finding_dialog_layout(view, size);
    let (field, rect) = (view.fields.get(i)?, *layout.fields.get(i)?);
    let rows = soft_rows(&field.line, text_width(size));
    let first = first_shown(&rows, &field.line, field.rows);
    // Off its shown rows, the nearest: a press held past them scrolls by
    // `finding_field_past`.
    let down = ((pos.y as i32 - rect.loc.y - FIELD_PAD).max(0) / ROW) as usize;
    let down = down.min(field.rows.max(1) as usize - 1);
    let row = rows.get(first + down).or(rows.last())?;
    let x = pos.x as f32 - (rect.loc.x as f32 + PADDING);
    Some(byte_at(&field.line, row, x.max(0.0)))
}

impl Painter {
    /// A field's lines in `rect`, as `Painter::field` draws one line: the
    /// placeholder without any text, and when `focused`, the selection
    /// behind the text and the caret.
    #[allow(clippy::too_many_arguments)]
    fn lines_field(
        &self,
        c: &mut Canvas,
        field: &FindingFieldView,
        rect: Rect,
        focused: bool,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = s.round().max(1.0);
        let border = if focused { p.accent } else { p.border };
        c.rounded(x, y, w, h, BUTTON_RADIUS * s, border);
        c.rounded(
            x + b,
            y + b,
            w - 2.0 * b,
            h - 2.0 * b,
            BUTTON_RADIUS * s - b,
            p.element,
        );
        let line = &field.line;
        let rows = soft_rows(line, text_width(size));
        let first = first_shown(&rows, line, field.rows);
        let caret_row = row_of(&rows, line.caret);
        let text_x = x + PADDING * s;
        let end = x + w - PADDING * s;
        let selection = line.selection();
        let shown = rows
            .iter()
            .enumerate()
            .skip(first)
            .take(field.rows as usize);
        for (k, (i, row)) in shown.enumerate() {
            let at = Rectangle::new(
                (rect.loc.x, rect.loc.y + FIELD_PAD + k as i32 * ROW).into(),
                (rect.size.w, ROW).into(),
            );
            let row_band = band(at, scale);
            let upto = |byte: usize| self.text_width(&line[row.start..byte], FONT_SIZE, scale);
            let (from, to) = (selection.start.max(row.start), selection.end.min(row.end));
            if focused && from < to {
                let (a, z) = (text_x + upto(from), (text_x + upto(to)).min(end));
                c.rounded(
                    a,
                    row_band.0 as f32 + 2.0 * s,
                    z - a,
                    row_band.1 as f32 - 4.0 * s,
                    2.0 * s,
                    p.selected,
                );
            }
            if line.is_empty() {
                let nudge = 2.0 * s;
                let hint = field.placeholder;
                self.text(
                    c,
                    hint,
                    FONT_SIZE,
                    text_x + nudge,
                    end,
                    row_band,
                    scale,
                    p.placeholder,
                );
            } else {
                let text = &line[row.clone()];
                self.text(c, text, FONT_SIZE, text_x, end, row_band, scale, p.text);
            }
            if focused && i == caret_row {
                caret(c, text_x + upto(line.caret), row_band, scale, p.accent);
            }
        }
    }

    /// The dialog for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow.
    pub(super) fn finding_dialog(
        &self,
        view: &FindingDialogView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = finding_dialog_layout(view, size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
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
        line(&mut c, &view.title, layout.title, LABEL_FONT_SIZE, p.text);
        line(&mut c, &view.about, layout.about, FONT_SIZE, p.muted);
        for (i, field) in view.fields.iter().enumerate() {
            line(&mut c, field.label, layout.labels[i], FONT_SIZE, p.muted);
            let focused = view.focus == i;
            self.lines_field(
                &mut c,
                field,
                shift(layout.fields[i]),
                focused,
                size,
                scale,
                p,
            );
        }
        if let (Some(rect), Some(error)) = (layout.error, &view.error) {
            line(&mut c, error, rect, FONT_SIZE, p.error);
        }
        let first = shift(layout.buttons[0]);
        let left = shift(layout.title).loc.x;
        let row = Rectangle::new((left, first.loc.y).into(), (0, first.size.h).into());
        self.dialog_keys(
            &mut c,
            &view.keys,
            Some(view.action),
            row,
            first.loc.x - QUIT_BUTTON_GAP,
            scale,
            p,
        );
        let labels = ["Cancel", view.action];
        for (i, (rect, label)) in layout.buttons.iter().zip(labels).enumerate() {
            let style = match i == FINDING_DIALOG_ACTION {
                true => ButtonStyle::Primary,
                false => ButtonStyle::Plain,
            };
            let hovered = view.hover == Some(FindingDialogHit::Button(i));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(rows: &[i32], error: bool) -> FindingDialogView {
        FindingDialogView {
            title: "Edit r1-2".into(),
            about: "Off by one".into(),
            fields: rows
                .iter()
                .map(|rows| FindingFieldView {
                    label: "Fix",
                    line: Line::default(),
                    placeholder: "",
                    rows: *rows,
                })
                .collect(),
            focus: 0,
            action: "Save",
            error: error.then(|| "Say why.".to_owned()),
            hover: None,
            keys: Vec::new(),
        }
    }

    #[test]
    fn lines_break_at_line_breaks_and_spaces_and_cover_the_text() {
        let rows = soft_rows("one two\n\nthree", 10_000.0);
        assert_eq!(rows, [0..7, 8..8, 9..14]);
        let text = "aaaa bbbb cccc dddd";
        let room = width("aaaa bbbb ");
        let rows = soft_rows(text, room);
        assert!(rows.len() > 1, "{rows:?}");
        assert_eq!(rows.first().unwrap().start, 0);
        assert_eq!(rows.last().unwrap().end, text.len());
        for pair in rows.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "nothing lost at a wrap");
        }
        // A word wider than the field breaks anyway, and ends.
        let long = "x".repeat(40);
        let rows = soft_rows(&long, width("xxxxx"));
        assert!(rows.len() > 2);
        assert_eq!(rows.last().unwrap().end, long.len());
    }

    #[test]
    fn the_caret_stays_among_the_lines_shown() {
        let text = "a\nb\nc\nd\ne";
        let rows = soft_rows(text, 10_000.0);
        let at = |caret: usize, scroll: usize| Line {
            text: text.into(),
            caret,
            anchor: caret,
            scroll,
        };
        assert_eq!(first_shown(&rows, &at(0, 0), 3), 0);
        assert_eq!(first_shown(&rows, &at(4, 0), 3), 0);
        assert_eq!(first_shown(&rows, &at(8, 0), 3), 2);
        // Started from "c": with the caret on "c" it stays there, on "b"
        // it goes one row up, no further.
        assert_eq!(first_shown(&rows, &at(4, 4), 3), 2);
        assert_eq!(first_shown(&rows, &at(2, 4), 3), 1);
        // Never past the last row's filling the field.
        assert_eq!(first_shown(&rows, &at(8, 8), 3), 2);
    }

    #[test]
    fn a_press_held_past_a_fields_rows_goes_on_a_row_at_a_time() {
        let size: Size<i32, Logical> = (1200, 800).into();
        let text = "a\nb\nc\nd\ne";
        let with = |caret: usize| FindingDialogView {
            fields: vec![FindingFieldView {
                label: "Why",
                line: Line {
                    text: text.into(),
                    caret,
                    anchor: caret,
                    scroll: 0,
                },
                placeholder: "",
                rows: 2,
            }],
            ..view(&[2], false)
        };
        let rect = finding_dialog_layout(&with(0), size).fields[0];
        let x = rect.loc.x as f64 + 200.0;
        // From "a", two rows down, then far past the end: "c", then "e".
        assert_eq!(finding_field_past(&with(0), 0, size, 2, x), Some(5));
        assert_eq!(finding_field_past(&with(4), 0, size, 99, x), Some(9));
        assert_eq!(finding_field_past(&with(4), 0, size, -99, x), Some(1));
        // Past its shown rows, by how far.
        let y = (rect.loc.y + FIELD_PAD) as f64;
        let edge = |dy: f64| finding_field_edge(&with(0), 0, size, (x, y + dy).into());
        assert_eq!(edge(-10.0), -10.0);
        assert_eq!(edge(5.0), 0.0);
        assert!(edge((2 * ROW) as f64 + 4.0) > 0.0);
        // A press below its rows lands on the last shown, not past it.
        let below = (x, y + (5 * ROW) as f64).into();
        assert_eq!(finding_field_at(&with(0), 0, size, below), Some(3));
    }

    #[test]
    fn up_and_down_go_through_wrapped_lines_and_stop_at_the_ends() {
        let size = (1200, 800).into();
        let line = |text: &str, caret: usize| Line {
            text: text.into(),
            caret,
            anchor: caret,
            scroll: 0,
        };
        assert_eq!(finding_field_step(&line("ab\ncd", 1), false, size), Some(4));
        assert_eq!(finding_field_step(&line("ab\ncd", 4), true, size), Some(1));
        assert_eq!(finding_field_step(&line("ab\ncd", 1), true, size), None);
        assert_eq!(finding_field_step(&line("ab\ncd", 4), false, size), None);
    }

    #[test]
    fn it_grows_with_its_fields_and_fits_a_small_output() {
        let size = (870, 600).into();
        let one = finding_dialog_layout(&view(&[3], false), size);
        let two = finding_dialog_layout(&view(&[6, 3], true), size);
        assert!(two.panel.size.h > one.panel.size.h);
        assert_eq!(two.fields.len(), 2);
        assert!(two.fields[0].size.h > two.fields[1].size.h);
        assert!(two.panel.size.h <= 600, "{}", two.panel.size.h);
        assert!(two.fields[1].loc.y < two.error.unwrap().loc.y);
        assert!(two.error.unwrap().loc.y < two.buttons[0].loc.y);
    }

    #[test]
    fn a_label_goes_to_its_field_and_buttons_are_buttons() {
        let size = (1200, 800).into();
        let l = finding_dialog_layout(&view(&[6, 3], false), size);
        let centre = |r: Rect| Point::from(((r.loc.x + 4) as f64, (r.loc.y + 4) as f64));
        assert_eq!(l.hit(centre(l.labels[1])), Some(FindingDialogHit::Field(1)));
        assert_eq!(l.hit(centre(l.fields[0])), Some(FindingDialogHit::Field(0)));
        assert_eq!(
            l.hit(centre(l.buttons[1])),
            Some(FindingDialogHit::Button(1))
        );
        assert_eq!(l.hit(Point::from((0.0, 0.0))), None);
    }
}
