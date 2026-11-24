//! A field of lines wrapped to its width, for what is written to someone:
//! a note, a comment, what goes to an agent. It is as tall as its rows, up
//! to `WRAP_MAX_ROWS`; past them its view follows the caret, and a
//! scrollbar down its right edge shows how much more there is above and
//! below. The finding dialog's fields wrap the same way, at their own
//! height.

use std::ops::Range;

use super::{
    CARET_WIDTH, Canvas, FieldSpot, ListBar, PADDING, Painter, Palette, Rect, Thumb,
    field_lines_height, field_text, font_size, measure, mix, rect_px,
};
use crate::text_field::Line;
use smithay::utils::{Logical, Point, Rectangle};

use super::painter::SubLine;

/// The most rows a wrapped field shows before it scrolls.
pub const WRAP_MAX_ROWS: usize = 5;
/// How wide the scrollbar in a wrapped field's right padding is, and how
/// far it stands from the field's top and bottom.
const BAR: i32 = 4;
const BAR_INSET: i32 = 5;

/// How wide `text` is, guessed when there is no font.
pub(super) fn width(text: &str) -> f32 {
    measure(text, font_size()).unwrap_or(text.chars().count() as f32 * font_size() * 0.55)
}

/// `text`'s lines as shown in a field `room` wide, as byte ranges: at its
/// line breaks, and at the last space that fits, else the last character.
pub(super) fn soft_rows(text: &str, room: f32) -> Vec<Range<usize>> {
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
pub(super) fn row_of(rows: &[Range<usize>], caret: usize) -> usize {
    rows.iter()
        .rposition(|r| r.start <= caret)
        .unwrap_or_default()
}

/// The byte of `text` in `row` nearest `x` from the row's start.
pub(super) fn byte_at(text: &str, row: &Range<usize>, x: f32) -> usize {
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

/// Where Up (`up`) or Down moves the caret of `line` wrapped as `rows`: to
/// the row above or below, as far along it as the caret is along its own;
/// none from the first row up or the last down.
pub(super) fn row_step(line: &Line, rows: &[Range<usize>], up: bool) -> Option<usize> {
    let at = row_of(rows, line.caret);
    let to = if up { at.checked_sub(1)? } else { at + 1 };
    let row = rows.get(to)?;
    let x = width(&line[rows[at].start..line.caret]);
    Some(byte_at(line, row, x))
}

/// The first of `rows` a field showing `shown` of them shows: the one its
/// view started at (`Line::scroll`), moved only as far as the caret's row
/// needs to be among them, and back for the last row to fill the field.
pub(super) fn first_shown(rows: &[Range<usize>], line: &Line, shown: i32) -> usize {
    let shown = shown.max(1) as usize;
    let caret = row_of(rows, line.caret);
    let from = row_of(rows, line.scroll);
    let fill = rows.len().saturating_sub(shown);
    from.clamp(caret.saturating_sub(shown - 1), caret).min(fill)
}

/// What the wheel turned `by` rows (up negative) does to `line` wrapped as
/// `rows` in a field showing `shown` of them: the byte its view then starts
/// at, and the byte its caret goes to, on the nearest row in view, as far
/// along it as it was along its own (the view follows the caret, so the
/// caret comes along). None while every row shows.
pub(super) fn wheel_rows(
    line: &Line,
    rows: &[Range<usize>],
    shown: i32,
    by: isize,
) -> Option<(usize, usize)> {
    let shown = shown.max(1) as usize;
    let fill = rows.len().checked_sub(shown).filter(|fill| *fill > 0)?;
    let from = first_shown(rows, line, shown as i32);
    let to = from.saturating_add_signed(by).min(fill);
    let at = row_of(rows, line.caret);
    let row = at.clamp(to, to + shown - 1);
    let caret = match row == at {
        true => line.caret,
        false => byte_at(line, &rows[row], width(&line[rows[at].start..line.caret])),
    };
    Some((rows[to].start, caret))
}

/// What the wheel turned `by` rows does to the wrapped field `spot` (see
/// `wheel_rows`).
pub fn wrap_field_wheel(spot: &FieldSpot, by: isize) -> Option<(usize, usize)> {
    let rows = soft_rows(&spot.line, room(spot.area.size.w));
    wheel_rows(&spot.line, &rows, shown_rows(&spot.line, spot.area.size.w) as i32, by)
}

/// How wide the text of a wrapped field `w` wide runs (as `field_text`
/// has it, with a pixel or two to spare for the font at any scale).
fn room(w: i32) -> f32 {
    (w as f32 - 2.0 * PADDING - (CARET_WIDTH + 3) as f32).max(1.0)
}

/// How many rows a wrapped field `w` wide shows of `text`.
fn shown_rows(text: &str, w: i32) -> usize {
    soft_rows(text, room(w)).len().clamp(1, WRAP_MAX_ROWS)
}

/// How high a wrapped field `w` wide holding `text` is.
pub fn wrap_field_height(text: &str, w: i32) -> i32 {
    field_lines_height(shown_rows(text, w))
}

/// What a wrapped field `w` wide shows of `whole`: its rows in view, each
/// as a line of a field of several (see `sub_lines`), the first one's
/// index and how many rows there are in all.
fn view_of(whole: &Line, w: i32) -> (Vec<SubLine>, usize, usize) {
    let rows = soft_rows(whole, room(w));
    let shown = shown_rows(whole, w);
    let first = first_shown(&rows, whole, shown as i32);
    let caret = row_of(&rows, whole.caret);
    let subs = (rows.iter().enumerate().skip(first).take(shown))
        .map(|(i, r)| {
            let inside = |at: usize| at.clamp(r.start, r.end) - r.start;
            SubLine {
                line: Line {
                    text: whole[r.clone()].to_owned(),
                    caret: inside(whole.caret),
                    anchor: inside(whole.anchor),
                    scroll: 0,
                },
                start: r.start,
                has_caret: i == caret,
            }
        })
        .collect();
    (subs, first, rows.len())
}

/// The row of the wrapped field in `area` holding `whole` the pointer at
/// `pos` is on, and the byte of `whole` its text starts at: above or below
/// the rows shown, the nearest.
pub fn wrap_field_spot(
    area: Rect,
    whole: &Line,
    pos: Point<f64, Logical>,
    scale: f64,
) -> (FieldSpot, usize) {
    let (subs, _, _) = view_of(whole, area.size.w);
    let at = ((pos.y - area.loc.y as f64) / super::field_line() as f64).floor();
    let sub = &subs[(at.max(0.0) as usize).min(subs.len() - 1)];
    let spot = FieldSpot {
        area,
        text: field_text(area, scale),
        size: font_size(),
        line: sub.line.clone(),
    };
    (spot, sub.start)
}

/// The byte the view of the wrapped field `spot` starts at, as it is drawn
/// (see `Line::scroll`): the field keeps it for the next frame.
pub fn wrap_field_first(spot: &FieldSpot) -> usize {
    let (subs, _, _) = view_of(&spot.line, spot.area.size.w);
    subs.first().map_or(0, |sub| sub.start)
}

/// Where Up (`up`) or Down moves the caret of the wrapped field `spot`: a
/// row up or down as it wraps; none on its first row up or last down.
pub fn wrap_field_step(spot: &FieldSpot, up: bool) -> Option<usize> {
    let rows = soft_rows(&spot.line, room(spot.area.size.w));
    row_step(&spot.line, &rows, up)
}

/// A press in the wrapped field `spot` held `by` rows past its shown ones
/// (up negative), the pointer at output x `x`: the byte the selection
/// reaches, on the row `by` from the caret's, as far as there are.
pub fn wrap_field_past(spot: &FieldSpot, by: i64, x: f64) -> Option<usize> {
    let rows = soft_rows(&spot.line, room(spot.area.size.w));
    let at = row_of(&rows, spot.line.caret) as i64;
    let to = (at + by).clamp(0, rows.len() as i64 - 1) as usize;
    let x = x as f32 - (spot.area.loc.x as f32 + PADDING);
    Some(byte_at(&spot.line, rows.get(to)?, x.max(0.0)))
}

/// How far `pos` is above (negative) or below the wrapped field `spot`, in
/// logical pixels; inside it, none.
pub fn wrap_field_edge(spot: &FieldSpot, pos: Point<f64, Logical>) -> f64 {
    let top = spot.area.loc.y as f64;
    let bottom = top + spot.area.size.h as f64;
    match pos.y {
        y if y < top => y - top,
        y if y >= bottom => y - bottom + 1.0,
        _ => 0.0,
    }
}

/// The scrollbar of a wrapped field in `rect` showing `shown` of its
/// `total` rows from row `first`; none while they all show.
pub(super) fn wrap_thumb(rect: Rect, first: usize, shown: usize, total: usize) -> Option<Thumb> {
    let track = Rectangle::new(
        (
            rect.loc.x + rect.size.w - (PADDING as i32 + BAR) / 2,
            rect.loc.y + BAR_INSET,
        )
            .into(),
        (BAR, rect.size.h - 2 * BAR_INSET).into(),
    );
    let bar = ListBar {
        down: first as f64,
        total: total as f64,
    };
    Thumb::of(track, shown as f64, bar)
}

impl Painter {
    /// A wrapped field in `rect` holding `whole`: its rows in view as a
    /// field of lines draws its lines, `placeholder` while it is empty, and
    /// a scrollbar while rows are out of view above or below.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn wrap_field(
        &self,
        c: &mut Canvas,
        whole: &Line,
        placeholder: &str,
        rect: Rect,
        focused: bool,
        scale: f64,
        p: &Palette,
    ) {
        self.field_box(c, rect, scale, p);
        let (subs, first, total) = view_of(whole, rect.size.w);
        self.sub_lines_in(c, &subs, placeholder, rect, focused, scale, p);
        if let Some(thumb) = wrap_thumb(rect, first, subs.len(), total) {
            self.wrap_bar(c, &thumb, scale, p);
        }
    }

    /// A field's scrollbar: its track, faint, and its thumb in the muted
    /// ink, stronger than a list's, as it is all that says the field holds
    /// more than it shows.
    pub(super) fn wrap_bar(&self, c: &mut Canvas, thumb: &Thumb, scale: f64, p: &Palette) {
        let (x, y, w, h) = rect_px(thumb.track, scale);
        c.rounded(x, y, w, h, w / 2.0, mix(p.element, p.muted, 0.2));
        let (ty, th) = (
            (thumb.top * scale).round() as f32,
            (thumb.h * scale).round() as f32,
        );
        c.rounded(x, ty, w, th, w / 2.0, p.muted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::{field_line, painter::field_line_rect};

    fn at(text: &str, caret: usize, scroll: usize) -> Line {
        Line {
            text: text.into(),
            caret,
            anchor: caret,
            scroll,
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
        assert_eq!(first_shown(&rows, &at(text, 0, 0), 3), 0);
        assert_eq!(first_shown(&rows, &at(text, 4, 0), 3), 0);
        assert_eq!(first_shown(&rows, &at(text, 8, 0), 3), 2);
        // Started from "c": with the caret on "c" it stays there, on "b"
        // it goes one row up, no further.
        assert_eq!(first_shown(&rows, &at(text, 4, 4), 3), 2);
        assert_eq!(first_shown(&rows, &at(text, 2, 4), 3), 1);
        // Never past the last row's filling the field.
        assert_eq!(first_shown(&rows, &at(text, 8, 8), 3), 2);
    }

    #[test]
    fn a_wrapped_field_grows_with_its_rows_up_to_five_then_scrolls() {
        let w = 300;
        let long = "word ".repeat(200);
        assert_eq!(wrap_field_height("", w), field_lines_height(1));
        assert_eq!(wrap_field_height("one\ntwo", w), field_lines_height(2));
        // A long line wraps: more rows than line breaks.
        let para = "word ".repeat(30);
        assert!(wrap_field_height(&para, w) > field_lines_height(1));
        assert_eq!(
            wrap_field_height(&long, w),
            field_lines_height(WRAP_MAX_ROWS)
        );
        // The view follows the caret to the end, and shows the bar.
        let rect = Rectangle::new((0, 0).into(), (w, wrap_field_height(&long, w)).into());
        let end = at(&long, long.len(), 0);
        let (subs, first, total) = view_of(&end, w);
        assert_eq!(subs.len(), WRAP_MAX_ROWS);
        assert_eq!(first + subs.len(), total);
        assert!(subs.last().unwrap().has_caret);
        let thumb = wrap_thumb(rect, first, subs.len(), total).unwrap();
        assert_eq!(
            thumb.top + thumb.h,
            (thumb.track.loc.y + thumb.track.size.h) as f64
        );
        assert!(thumb.track.loc.x > rect.loc.x + rect.size.w - PADDING as i32);
        assert!(thumb.track.loc.x + BAR < rect.loc.x + rect.size.w);
        // All of it in view: no bar.
        let (subs, first, total) = view_of(&at("one", 0, 0), w);
        assert!(wrap_thumb(rect, first, subs.len(), total).is_none());
    }

    #[test]
    fn a_click_and_up_and_down_go_by_the_rows_as_wrapped() {
        let w = 200;
        let para = "word ".repeat(12);
        let line = at(&para, 0, 0);
        let h = wrap_field_height(&para, w);
        let area = Rectangle::new((10, 100).into(), (w, h).into());
        let rows = soft_rows(&para, room(w));
        assert!(rows.len() >= 3, "{rows:?}");
        // The second row's band: its spot starts where the row does.
        let y = field_line_rect(area, 1).loc.y as f64 + field_line() as f64 / 2.0;
        let (spot, base) = wrap_field_spot(area, &line, (20.0, y).into(), 1.0);
        assert_eq!(base, rows[1].start);
        assert_eq!(spot.line.text, &para[rows[1].clone()]);
        // Down from the first row lands on the second, Up from it on none.
        let whole = FieldSpot {
            area,
            text: field_text(area, 1.0),
            size: font_size(),
            line: line.clone(),
        };
        let down = wrap_field_step(&whole, false).unwrap();
        assert_eq!(row_of(&rows, down), 1);
        assert_eq!(wrap_field_step(&whole, true), None);
        // Held below it, the selection goes on a row a step, to the last.
        assert_eq!(wrap_field_edge(&whole, (20.0, 99.0).into()), -1.0);
        assert_eq!(wrap_field_edge(&whole, (20.0, 120.0).into()), 0.0);
        let past = wrap_field_past(&whole, 2, 15.0).unwrap();
        assert_eq!(row_of(&rows, past), 2);
        let far = wrap_field_past(&whole, 99, 15.0).unwrap();
        assert_eq!(row_of(&rows, far), rows.len() - 1);
    }

    #[test]
    fn the_wheel_moves_the_rows_and_takes_the_caret_along() {
        let text = "a\nb\nc\nd\ne\nf\ng";
        let rows = soft_rows(text, 10_000.0);
        assert_eq!(rows.len(), 7);
        // Caret on the first row: down two, the view starts at "c" and the
        // caret comes onto it.
        let (first, caret) = wheel_rows(&at(text, 0, 0), &rows, 5, 2).unwrap();
        assert_eq!((first, caret), (rows[2].start, rows[2].start));
        // No further than the last row filling the field.
        let (first, _) = wheel_rows(&at(text, 0, 0), &rows, 5, 9).unwrap();
        assert_eq!(first, rows[2].start);
        // Up from the bottom, the caret on the last row comes up into view.
        let end = text.len();
        let (first, caret) = wheel_rows(&at(text, end, rows[2].start), &rows, 5, -2).unwrap();
        assert_eq!((first, caret), (0, rows[4].start + 1));
        // A field showing all its rows leaves the wheel to the page.
        assert_eq!(wheel_rows(&at(text, 0, 0), &rows, 7, 1), None);
    }
}
