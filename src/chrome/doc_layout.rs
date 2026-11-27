//! A markdown file's blocks (`md_blocks`) laid out in a column and painted:
//! the copy of a stage's file on its feature page. What is laid out is
//! pieces from `(0, 0)`, kept for a width (`laid_out`), so a page can put
//! them where it will. Runs of a line (`md_blocks::Inline`) are set in
//! their own face: bold in bold, italic slanted, code in the monospace
//! font, a link in the accent, struck through with a line through it.

use std::{
    cell::RefCell,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use smithay::utils::{Logical, Point, Rectangle};

use super::features::{Font, Ink, StageMark, body_line, small_line, stage_dot, width_of};
use super::glyphs::{self, Face};
use super::{ButtonStyle, Canvas, Painter, Palette, Rect, Rgb, band, check, icon_sdf, rect_px};
use crate::md_blocks::{Alert, Align, DocBlock, Inline, ItemMark, Styled, spans};
use crate::syntax::{Kind, Spans};

/// A file's blocks: the room above a block, its list items' indent and
/// gutter, a quote's indent and bar, a code block's inner room, and how
/// far in lists stop indenting more.
const DOC_GAP: i32 = 8;
const DOC_HEADING_GAP: i32 = 14;
const DOC_ITEM_GAP: i32 = 2;
const DOC_INDENT: i32 = 16;
const DOC_MARK: i32 = 20;
const DOC_QUOTE: i32 = 12;
const DOC_QUOTE_BAR: i32 = 3;
pub(super) const DOC_CODE_PAD: i32 = 8;
const DOC_DEPTH_MAX: u8 = 4;
/// An inline code span's ground: the room in it on each side of the
/// words, how high it is for its font, and how far it is toward the text.
const DOC_SPAN_PAD: f32 = 4.0;
const DOC_SPAN_HIGH: f32 = 1.4;
pub(super) const DOC_SPAN_TINT: f32 = 0.08;
const DOC_RULE: i32 = 13;
/// The highest an image in a file is drawn, in logical pixels: a tall one
/// shrinks to it.
const DOC_IMAGE_HIGH: i32 = 480;
/// A table's cells: the room around their words, and the widest a
/// column's longest word holds it open (past that the word breaks).
const DOC_CELL_PAD_X: i32 = 8;
const DOC_CELL_PAD_Y: i32 = 4;
const DOC_CELL_WORD_MAX: i32 = 160;
/// How far off a line's width added up by word may be from it measured
/// whole, in logical pixels: kerning and rounding at each space.
const DOC_WIDTH_SLACK: f32 = 8.0;

/// A thing a file's blocks paint, in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DocPiece {
    /// A line of text, with its runs (as `Styled` has them).
    Text {
        text: String,
        runs: Vec<(Range<usize>, Inline)>,
        font: Font,
        ink: Ink,
        bold: bool,
    },
    /// What a list item starts with, right in its gutter.
    ItemMark(ItemMark),
    /// A line of a code block, cut where the column ends, in its language's
    /// colours, on the `CodeFill` before it.
    CodeLine(String, Spans),
    /// A code block's ground behind one of its lines: put before the line,
    /// so a pick paints on it.
    CodeFill,
    /// The room above (`top`) or below a code block's lines, with the
    /// ground's rounded corners.
    CodeEdge { top: bool },
    /// A file's code block's top, as `CodeEdge`'s, with the language its
    /// fence names on the left and "Copy" on the right (`copy_spot`), a
    /// press on which copies `text`, the block's lines.
    CodeHead { lang: String, text: String },
    /// The rounded ground behind an inline code span, as other markdown
    /// views have it: put before its line, so a pick or a find paints on it.
    CodeGround,
    /// The bar beside a quote, in its alert's colour.
    QuoteBar(Option<Alert>),
    /// A PNG image of a file's, drawn to fit.
    Image(PathBuf),
    /// A thematic break.
    Rule,
    /// A table cell's ground and lines: its top and left, and its right
    /// and bottom on the table's edge. The header's is filled.
    Cell {
        head: bool,
        right: bool,
        bottom: bool,
    },
}

/// What a code block's head offers, to copy its lines.
pub(super) const COPY_CODE: &str = "Copy";

/// Code block head `rect`'s "Copy" button, where a press copies its lines:
/// the word with the block's room either side, half that room in from the
/// head's right, top and bottom.
pub(super) fn copy_spot(rect: Rect) -> Rect {
    let wide = width_in(COPY_CODE, Face::Ui, Font::Small.size()).ceil() as i32 + 2 * DOC_CODE_PAD;
    let m = DOC_CODE_PAD / 2;
    let wide = wide.min(rect.size.w - 2 * m);
    let x = rect.loc.x + rect.size.w - m - wide;
    Rectangle::new(
        (x, rect.loc.y + m).into(),
        (wide, rect.size.h - 2 * m).into(),
    )
}

/// A code block's ground, as an inline code span's: off `element` too, as
/// a plan's card in the chat is on it.
pub(super) fn code_ground(p: &Palette) -> Rgb {
    super::theme::mix(p.element, p.text, DOC_SPAN_TINT)
}

/// How a line's run `on` is set, the line bold or not.
pub(super) fn face(on: Option<&Inline>, bold: bool) -> Face {
    match on {
        Some(Inline::Code) => Face::Mono,
        Some(Inline::Bold) => Face::Bold,
        _ if bold => Face::Bold,
        Some(Inline::Italic) => Face::Italic,
        _ => Face::Ui,
    }
}

/// How wide `text` is in `face` at `size`, in logical pixels, guessed when
/// there is no font.
pub(super) fn width_in(text: &str, face: Face, size: f32) -> f32 {
    match glyphs::available() {
        true => glyphs::width(text, face, size),
        false => text.chars().count() as f32 * size * 0.55,
    }
}

/// The room run `on` has on each side of its words: a code span's in its
/// ground.
pub(super) fn run_pad(on: Option<&Inline>) -> f32 {
    match on {
        Some(Inline::Code) => DOC_SPAN_PAD,
        _ => 0.0,
    }
}

/// How wide line `text` with `runs` is, bold or not, each run measured by
/// `width` in its face, with its room (`run_pad`).
pub(super) fn line_width(
    text: &str,
    runs: &[(Range<usize>, Inline)],
    bold: bool,
    width: impl Fn(&str, Face) -> f32,
) -> f32 {
    (spans(text, runs))
        .map(|(range, on)| width(&text[range], face(on, bold)) + 2.0 * run_pad(on))
        .sum()
}

/// How far into line `text` with `runs` its byte `at` is, as `line_width`
/// measures: inside a run, past the room before it only.
pub(super) fn x_at(
    text: &str,
    runs: &[(Range<usize>, Inline)],
    bold: bool,
    at: usize,
    width: impl Fn(&str, Face) -> f32,
) -> f32 {
    let inside = (runs.iter()).find(|(r, _)| r.start < at && at < r.end);
    line_width(&text[..at], runs, bold, width) - run_pad(inside.map(|(_, on)| on))
}

/// The link `x` logical pixels into line `text` with `runs`, in `font`,
/// bold or not.
pub(super) fn link_at<'a>(
    text: &str,
    runs: &'a [(Range<usize>, Inline)],
    font: Font,
    bold: bool,
    x: f32,
) -> Option<&'a str> {
    (link_spans(text, runs, font, bold).into_iter())
        .find(|(span, _)| span.contains(&x))
        .map(|(_, url)| url)
}

/// Where line `text` with `runs`'s links are along it, in logical pixels
/// from its start, in `font`, bold or not.
pub(super) fn link_spans<'a>(
    text: &str,
    runs: &'a [(Range<usize>, Inline)],
    font: Font,
    bold: bool,
) -> Vec<(Range<f32>, &'a str)> {
    let mut pen = 0.0;
    let mut links = Vec::new();
    for (range, on) in spans(text, runs) {
        let from = pen;
        pen += width_in(&text[range], face(on, bold), font.size()) + 2.0 * run_pad(on);
        if let Some(Inline::Link(url)) = on {
            links.push((from..pen, url.as_str()));
        }
    }
    links
}

/// `text` broken into lines no wider than `width` by `measure`, all of it
/// kept: a word wider than a line (a URL) breaks where it must. Runs go on
/// over the lines they are cut into, and a space between two words of one
/// run is in it. A long file has many words: a line's width is added up
/// word by word, and measured whole only when that comes within
/// `DOC_WIDTH_SLACK` of the edge.
fn doc_lines(text: &Styled, width: i32, measure: impl Fn(&Styled) -> f32) -> Vec<Styled> {
    let width = width as f32;
    // A space between two words of run `on`: in its face, and no room
    // between them (`run_pad`).
    let joined = |on: Option<&Inline>| {
        let of = |t: &str| {
            let mut text = Styled::default();
            text.push(t, on);
            measure(&text)
        };
        of("a a") - 2.0 * of("a")
    };
    let space = joined(None);
    let mut lines = Vec::new();
    let mut line = Styled::default();
    let mut used = 0.0;
    // Where the line's last word ended in `text`.
    let mut last = 0;
    for word in text.text.split_whitespace() {
        let at = word.as_ptr() as usize - text.text.as_ptr() as usize;
        let word = text.slice(at..at + word.len());
        let wide = measure(&word);
        if !line.text.is_empty() {
            let gap = text.at(last).filter(|on| text.at(at) == Some(*on));
            let space = match gap {
                None => space,
                Some(_) => joined(gap),
            };
            let about = used + space + wide;
            if about < width - DOC_WIDTH_SLACK {
                line.push(" ", gap);
                line.append(&word);
                (used, last) = (about, at + word.text.len());
                continue;
            }
            if about <= width + DOC_WIDTH_SLACK {
                let mut longer = line.clone();
                longer.push(" ", gap);
                longer.append(&word);
                let whole = measure(&longer);
                if whole <= width {
                    (line, used, last) = (longer, whole, at + word.text.len());
                    continue;
                }
            }
            lines.push(std::mem::take(&mut line));
        }
        last = at + word.text.len();
        if wide <= width {
            (line, used) = (word, wide);
            continue;
        }
        let mut rest = word;
        while measure(&rest) > width {
            let ends: Vec<usize> = (rest.text.char_indices()).skip(1).map(|(i, _)| i).collect();
            let fit = ends.partition_point(|end| measure(&rest.slice(0..*end)) <= width);
            let Some(end) = fit
                .checked_sub(1)
                .map(|i| ends[i])
                .or(ends.first().copied())
            else {
                break;
            };
            lines.push(rest.slice(0..end));
            rest = rest.slice(end..rest.text.len());
        }
        used = measure(&rest);
        line = rest;
    }
    if !line.text.is_empty() {
        lines.push(line);
    }
    lines
}

/// What a number mark says.
fn number_mark(n: usize) -> String {
    format!("{n}.")
}

/// A column being laid out, from its top. A table too wide for it may
/// reach out to `room`'s left and right.
struct Column {
    x: i32,
    w: i32,
    y: i32,
    room: (i32, i32),
    pieces: Vec<(Rect, DocPiece)>,
    /// How a line narrower than the column sits in it: a table cell's.
    align: Align,
    /// Inside a quote (not an alert's): its text muted.
    quoted: bool,
}

impl Column {
    fn put(&mut self, h: i32, piece: DocPiece) {
        let rect = Rectangle::new((self.x, self.y).into(), (self.w, h).into());
        self.pieces.push((rect, piece));
        self.y += h;
    }

    /// `text` as a file's text: all of its lines, a line `h` high each.
    fn text(&mut self, text: &Styled, font: Font, ink: Ink, bold: bool, h: i32) {
        let ink = match ink {
            Ink::Text if self.quoted => Ink::Muted,
            ink => ink,
        };
        let size = font.size();
        let measure = |t: &Styled| line_width(&t.text, &t.runs, bold, |t, f| width_in(t, f, size));
        // A hard break (`md_blocks`) starts a line of its own.
        let mut from = 0;
        let mut lines = Vec::new();
        for part in text.text.split('\n') {
            let part_at = from..from + part.len();
            lines.extend(doc_lines(&text.slice(part_at), self.w, measure));
            from += part.len() + 1;
        }
        for line in lines {
            let shift = match self.align {
                Align::Left => 0,
                align => {
                    let room = (self.w - measure(&line).ceil() as i32).max(0);
                    if align == Align::Right {
                        room
                    } else {
                        room / 2
                    }
                }
            };
            let Styled { text, runs } = line;
            self.x += shift;
            self.w -= shift;
            self.code_grounds(&text, &runs, size, bold, h);
            self.put(
                h,
                DocPiece::Text {
                    text,
                    runs,
                    font,
                    ink,
                    bold,
                },
            );
            self.x -= shift;
            self.w += shift;
        }
    }

    /// The grounds of line `text`'s code spans, `h` high, at the line.
    fn code_grounds(
        &mut self,
        text: &str,
        runs: &[(Range<usize>, Inline)],
        size: f32,
        bold: bool,
        h: i32,
    ) {
        let high = ((size * DOC_SPAN_HIGH).ceil() as i32).min(h);
        let y = self.y + (h - high) / 2;
        let mut pen: f32 = 0.0;
        for (range, on) in spans(text, runs) {
            let wide = width_in(&text[range], face(on, bold), size) + 2.0 * run_pad(on);
            if on == Some(&Inline::Code) {
                let from = pen.floor() as i32;
                let to = ((pen + wide).ceil() as i32).min(self.w);
                let rect = Rectangle::new((self.x + from, y).into(), (to - from, high).into());
                self.pieces.push((rect, DocPiece::CodeGround));
            }
            pen += wide;
        }
    }

    /// Run `body` in the column `by` further in.
    fn indented(&mut self, by: i32, body: impl FnOnce(&mut Column)) {
        let (x, w) = (self.x, self.w);
        let by = by.min(w - 1).max(0);
        self.x += by;
        self.w -= by;
        body(self);
        self.x = x;
        self.w = w;
    }

    /// A table's rows, its cells' words wrapped in them: as wide as its
    /// words, up to the column; wider, out into `room` centred on it, and
    /// wrapped to fit that.
    fn table(&mut self, align: &[Align], rows: &[Vec<Styled>]) {
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        if n == 0 {
            return;
        }
        let size = Font::Small.size();
        let measure = |t: &Styled, bold: bool| {
            line_width(&t.text, &t.runs, bold, |t, f| width_in(t, f, size)).ceil() as i32
        };
        // Each column's widest line, and its longest word.
        let (mut most, mut least) = (vec![1; n], vec![1; n]);
        for (i, row) in rows.iter().enumerate() {
            for (j, cell) in row.iter().enumerate() {
                let mut at = 0;
                for part in cell.text.split('\n') {
                    let line = cell.slice(at..at + part.len());
                    most[j] = most[j].max(measure(&line, i == 0));
                    at += part.len() + 1;
                }
                for word in cell.text.split_whitespace() {
                    let at = word.as_ptr() as usize - cell.text.as_ptr() as usize;
                    let wide = measure(&cell.slice(at..at + word.len()), i == 0);
                    least[j] = least[j].max(wide.min(DOC_CELL_WORD_MAX));
                }
            }
        }
        let pads = n as i32 * 2 * DOC_CELL_PAD_X;
        let whole = most.iter().sum::<i32>() + pads;
        let (x, widths) = if whole <= self.w {
            (self.x, most)
        } else {
            let (left, right) = self.room;
            let tw = whole.min(right - left);
            let x = (self.x + (self.w - tw) / 2).min(right - tw).max(left);
            (x, fitted(&most, &least, tw - pads))
        };
        let (x0, w0) = (self.x, self.w);
        for (i, row) in rows.iter().enumerate() {
            let (top, from) = (self.y, self.pieces.len());
            let mut bottom = top + DOC_CELL_PAD_Y + small_line();
            let mut cells = Vec::with_capacity(n);
            let mut cx = x;
            for (j, &cw) in widths.iter().enumerate() {
                (self.x, self.w, self.y) = (cx + DOC_CELL_PAD_X, cw, top + DOC_CELL_PAD_Y);
                if let Some(cell) = row.get(j) {
                    self.align = align.get(j).copied().unwrap_or_default();
                    self.text(cell, Font::Small, Ink::Text, i == 0, small_line());
                    self.align = Align::Left;
                }
                bottom = bottom.max(self.y);
                cells.push((cx, cw + 2 * DOC_CELL_PAD_X));
                cx += cw + 2 * DOC_CELL_PAD_X;
            }
            let h = bottom + DOC_CELL_PAD_Y - top;
            // Under the row's words, so painted first.
            let grounds = cells.into_iter().enumerate().map(|(j, (cx, cw))| {
                let cell = DocPiece::Cell {
                    head: i == 0,
                    right: j + 1 == n,
                    bottom: i + 1 == rows.len(),
                };
                (Rectangle::new((cx, top).into(), (cw, h).into()), cell)
            });
            self.pieces.splice(from..from, grounds);
            self.y = top + h;
        }
        (self.x, self.w) = (x0, w0);
    }

    /// A list item, its text hanging right of its mark.
    fn item(&mut self, depth: u8, mark: ItemMark, text: &Styled) {
        let gutter = match mark {
            ItemMark::Number(n) => {
                let w = width_of(&number_mark(n), Font::Body.size()).ceil() as i32 + 6;
                w.max(DOC_MARK)
            }
            _ => DOC_MARK,
        };
        let indent = i32::from(depth.min(DOC_DEPTH_MAX)) * DOC_INDENT;
        let top = self.y;
        self.indented(indent, |s| {
            if mark != ItemMark::Under {
                let spot = Rectangle::new((s.x, s.y).into(), (gutter, body_line()).into());
                s.pieces.push((spot, DocPiece::ItemMark(mark)));
            }
            s.indented(gutter, |s| {
                s.text(text, Font::Body, Ink::Text, false, body_line());
            });
        });
        // A mark with no words of its own still takes its line.
        self.y = self.y.max(top + body_line());
    }

    /// A file's blocks, from here down, with the room between them.
    fn blocks(&mut self, blocks: &[DocBlock]) {
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 {
                self.y += match block {
                    DocBlock::Heading { .. } => DOC_HEADING_GAP,
                    DocBlock::Item {
                        mark: ItemMark::Under,
                        ..
                    }
                    | DocBlock::Nested { .. } => DOC_GAP / 2,
                    DocBlock::Item { .. } if matches!(blocks[i - 1], DocBlock::Item { .. }) => {
                        DOC_ITEM_GAP
                    }
                    _ => DOC_GAP,
                };
            }
            self.block(block);
        }
    }

    /// A file's block, from here down.
    fn block(&mut self, block: &DocBlock) {
        match block {
            DocBlock::Heading { level, text } => {
                let (font, ink, h) = match level {
                    1 => (Font::Heading, Ink::Text, body_line() + 6),
                    2 => (Font::Subheading, Ink::Text, body_line() + 3),
                    3 | 4 => (Font::Body, Ink::Text, body_line()),
                    5 => (Font::Small, Ink::Text, small_line()),
                    _ => (Font::Small, Ink::Muted, small_line()),
                };
                self.text(text, font, ink, true, h);
            }
            DocBlock::Para(text) => self.text(text, Font::Body, Ink::Text, false, body_line()),
            DocBlock::Note(text) => {
                self.text(
                    &text.as_str().into(),
                    Font::Body,
                    Ink::Muted,
                    false,
                    body_line(),
                );
            }
            DocBlock::Item { depth, mark, text } => self.item(*depth, *mark, text),
            DocBlock::Code {
                lang,
                lines,
                syntax,
            } => {
                let edge =
                    |s: &mut Column, top| s.put(DOC_CODE_PAD / 2, DocPiece::CodeEdge { top });
                match lang {
                    Some(lang) => {
                        let (lang, text) = (lang.clone(), lines.join("\n"));
                        // Room around its Copy button (`copy_spot`).
                        let h = small_line() + DOC_CODE_PAD;
                        self.put(h, DocPiece::CodeHead { lang, text });
                    }
                    None => edge(self, true),
                }
                for (i, code) in lines.iter().enumerate() {
                    let spans = syntax.get(i).cloned().unwrap_or_default();
                    let at = Rectangle::new((self.x, self.y).into(), (self.w, small_line()).into());
                    self.pieces.push((at, DocPiece::CodeFill));
                    self.put(small_line(), DocPiece::CodeLine(code.clone(), spans));
                }
                edge(self, false);
            }
            DocBlock::Quote { alert, blocks } => {
                let top = self.y;
                let quoted = self.quoted;
                self.indented(DOC_QUOTE, |s| {
                    s.quoted = alert.is_none();
                    if let Some((_, title)) = alert {
                        s.text(
                            &title.as_str().into(),
                            Font::Body,
                            Ink::Text,
                            true,
                            body_line(),
                        );
                        if !blocks.is_empty() {
                            s.y += DOC_ITEM_GAP;
                        }
                    }
                    s.blocks(blocks);
                });
                self.quoted = quoted;
                let bar =
                    Rectangle::new((self.x, top).into(), (DOC_QUOTE_BAR, self.y - top).into());
                let kind = alert.as_ref().map(|(kind, _)| *kind);
                self.pieces.push((bar, DocPiece::QuoteBar(kind)));
            }
            DocBlock::Nested { depth, blocks } => {
                let indent = i32::from((*depth).min(DOC_DEPTH_MAX)) * DOC_INDENT + DOC_MARK;
                self.indented(indent, |s| s.blocks(blocks));
            }
            DocBlock::Table { align, rows } => self.table(align, rows),
            DocBlock::Image { url, alt } => self.image(url, alt),
            DocBlock::Rule => {
                self.put(DOC_RULE, DocPiece::Rule);
            }
        }
    }

    /// An image alone in its paragraph: a PNG file's drawn, as wide as it
    /// is up to the column and `DOC_IMAGE_HIGH`; any other (a web page's,
    /// one that can't be read) its alt text as a link to it.
    fn image(&mut self, url: &str, alt: &str) {
        let path = Path::new(url);
        let png =
            path.is_absolute() && (path.extension()).is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let size = png.then(|| crate::shot_images::size(path)).flatten();
        if let Some((w, h)) = size.filter(|(w, h)| *w > 0 && *h > 0) {
            let (w, h) = (i64::from(w), i64::from(h));
            let wide = w.min(i64::from(self.w.max(1)));
            let high = (h * wide / w).max(1);
            let (wide, high) = match high > i64::from(DOC_IMAGE_HIGH) {
                true => (
                    (wide * i64::from(DOC_IMAGE_HIGH) / high).max(1),
                    i64::from(DOC_IMAGE_HIGH),
                ),
                false => (wide, high),
            };
            let rect = Rectangle::new((self.x, self.y).into(), (wide as i32, high as i32).into());
            self.pieces.push((rect, DocPiece::Image(path.to_owned())));
            self.y += high as i32;
            return;
        }
        let words = match alt.trim() {
            "" => url.rsplit('/').next().unwrap_or(url),
            alt => alt,
        };
        let mut text = Styled::default();
        text.push(words, Some(&Inline::Link(url.to_owned())));
        self.text(&text, Font::Body, Ink::Text, false, body_line());
    }
}

/// Columns' widths adding up to `room` or less: each as wide as its
/// longest word (`least`) first, what is left shared by how much wider
/// each would be whole (`most`). Short of even that, all shrink alike and
/// their words break.
fn fitted(most: &[i32], least: &[i32], room: i32) -> Vec<i32> {
    let room = room.max(most.len() as i32);
    let low: i32 = least.iter().sum();
    if low >= room {
        return least.iter().map(|l| (l * room / low).max(1)).collect();
    }
    let want: i32 = most.iter().zip(least).map(|(m, l)| m - l).sum();
    let spare = (room - low).min(want);
    let more = |m: i32, l: i32| if want > 0 { (m - l) * spare / want } else { 0 };
    (most.iter().zip(least))
        .map(|(&m, &l)| l + more(m, l))
        .collect()
}

/// A file's blocks laid out for a column `w` wide, from `(0, 0)`, a table
/// reaching out to `room` at most.
fn stack(blocks: &[DocBlock], w: i32, room: (i32, i32)) -> Column {
    let mut s = Column {
        x: 0,
        w,
        y: 0,
        room,
        pieces: Vec::new(),
        align: Align::Left,
        quoted: false,
    };
    s.blocks(blocks);
    s
}

/// The latest file laid out, with the width: laying one out measures its
/// every word, and a page is laid out every frame. It holds on to what it
/// was laid out from, so that can't be another's at the same address.
struct DocMemo {
    blocks: Arc<Vec<DocBlock>>,
    w: i32,
    pieces: Rc<Vec<(Rect, DocPiece)>>,
    h: i32,
}

/// How many files `DOC_MEMO` holds: the stages looked between.
const DOC_MEMOS: usize = 2;

thread_local! {
    static DOC_MEMO: RefCell<(u32, Vec<DocMemo>)> = const { RefCell::new((0, Vec::new())) };
}

/// `blocks` laid out for a column `w` wide: the pieces from `(0, 0)`, and
/// how high they all are.
pub(super) fn laid_out(blocks: &Arc<Vec<DocBlock>>, w: i32) -> (Rc<Vec<(Rect, DocPiece)>>, i32) {
    DOC_MEMO.with_borrow_mut(|memo| {
        let memo = super::sized(memo);
        let found = (memo.iter()).position(|m| m.w == w && Arc::ptr_eq(&m.blocks, blocks));
        let entry = match found {
            Some(i) => memo.remove(i),
            None => {
                let s = stack(blocks, w, (0, w));
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

/// `blocks` laid out for a column `w` wide as `laid_out` does, not kept:
/// for what keeps its own (a chat's items).
pub(super) fn stack_of(blocks: &[DocBlock], w: i32) -> (Vec<(Rect, DocPiece)>, i32) {
    stack_in(blocks, w, (0, w))
}

/// `stack_of`, a table too wide for `w` reaching out to `room`'s left and
/// right (`(0, w)` is the column's own).
pub(super) fn stack_in(
    blocks: &[DocBlock],
    w: i32,
    room: (i32, i32),
) -> (Vec<(Rect, DocPiece)>, i32) {
    let s = stack(blocks, w, room);
    (s.pieces, s.y)
}

/// `pieces` (as `laid_out` gives them) painted with their `(0, 0)` at `at`.
#[cfg_attr(not(test), expect(dead_code, reason = "the agent chat paints with it"))]
pub(super) fn paint(
    painter: &Painter,
    c: &mut Canvas,
    pieces: &[(Rect, DocPiece)],
    at: Point<i32, Logical>,
    scale: f64,
    p: &Palette,
) {
    for (rect, piece) in pieces {
        let rect = Rectangle::new(rect.loc + at, rect.size);
        painter.doc_piece(c, rect, piece, scale, p);
    }
}

impl Painter {
    /// One of a file's pieces, in `rect`.
    pub(super) fn doc_piece(
        &self,
        c: &mut Canvas,
        rect: Rect,
        piece: &DocPiece,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        match piece {
            DocPiece::Text {
                text,
                runs,
                font,
                ink,
                bold,
            } => {
                let ((x, end), b) = (super::phys(rect, scale), band(rect, scale));
                let (size, color) = (font.size(), ink.color(p));
                if runs.is_empty() {
                    let at = ((x, end), b);
                    return self.page_text(c, text, (size, *bold), at, scale, (color, p));
                }
                let mut pen = x;
                for (range, on) in spans(text, runs) {
                    let run = &text[range];
                    let color = match on {
                        Some(Inline::Link(_)) => p.accent,
                        _ => color,
                    };
                    let from = pen;
                    pen = match face(on, *bold) {
                        Face::Mono => {
                            let pad = run_pad(on) * s;
                            pad + self.mono_text(c, run, size, pen + pad, end, b, scale, color)
                        }
                        face => self.face_text(face, c, run, size, pen, end, b, scale, color),
                    };
                    if on == Some(&Inline::Strike) {
                        // Through the middle of its small letters.
                        let t = s.ceil() as i32;
                        let y = b.0 + b.1 / 2 + (size * s * 0.05).round() as i32 - t / 2;
                        let (from, to) = (from.round() as i32, pen.min(end).round() as i32);
                        c.fill(from, y, to - from, t, color);
                    }
                }
            }
            DocPiece::ItemMark(mark) => {
                let spot = Rectangle::new(
                    (rect.loc.x, rect.loc.y + (body_line() - 14) / 2).into(),
                    (14, 14).into(),
                );
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
                        let (x, end) = super::phys(rect, scale);
                        let size = Font::Body.size();
                        let b = band(rect, scale);
                        self.text(c, &number_mark(*n), size, x, end, b, scale, p.muted);
                    }
                    ItemMark::Under => {}
                }
            }
            DocPiece::CodeLine(text, spans) => {
                self.code_line(c, rect, (text, spans), code_ground(p), scale, p);
            }
            DocPiece::CodeFill => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.fill(x as i32, y as i32, w as i32, h as i32, code_ground(p));
            }
            DocPiece::CodeEdge { top } => code_edge(c, rect, *top, scale, p),
            DocPiece::CodeHead { lang, .. } => self.code_head(c, rect, lang, false, scale, p),
            DocPiece::CodeGround => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, super::CAP_RADIUS * s, code_ground(p));
            }
            DocPiece::QuoteBar(alert) => {
                let (x, y, w, h) = rect_px(rect, scale);
                let color = match alert {
                    None => p.border,
                    Some(Alert::Note | Alert::Important) => p.accent,
                    Some(Alert::Tip) => p.success,
                    Some(Alert::Warning) => p.warning,
                    Some(Alert::Caution) => p.error,
                };
                c.rounded(x, y, w, h, w / 2.0, color);
            }
            DocPiece::Image(path) => {
                let (x, y, w, h) = rect_px(rect, scale);
                match crate::shot_images::picture(path, w.max(h).round() as usize) {
                    Some(picture) => {
                        let size = rect.size.w.max(rect.size.h);
                        c.picture_sized(&picture, rect, scale, p.text, size);
                    }
                    // Its file is gone: a box where it was.
                    None => c.rounded(x, y, w, h, 0.0, p.element),
                }
            }
            DocPiece::Rule => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.fill(
                    x as i32,
                    (y + h / 2.0) as i32,
                    w as i32,
                    s.ceil() as i32,
                    p.border,
                );
            }
            DocPiece::Cell {
                head,
                right,
                bottom,
            } => {
                let (x, y, w, h) = rect_px(rect, scale);
                let (x, y, w, h) = (x as i32, y as i32, w as i32, h as i32);
                let t = s.ceil() as i32;
                if *head {
                    c.fill(x, y, w, h, p.element);
                }
                c.fill(x, y, w, t, p.border);
                c.fill(x, y, t, h, p.border);
                if *right {
                    c.fill(x + w - t, y, t, h, p.border);
                }
                if *bottom {
                    c.fill(x, y + h - t, w, t, p.border);
                }
            }
        }
    }

    /// A code block's head in `rect`: the top of its ground, the language
    /// its fence names on the left, and its Copy button on the right, `lit`
    /// while the pointer is on it.
    pub(super) fn code_head(
        &self,
        c: &mut Canvas,
        rect: Rect,
        lang: &str,
        lit: bool,
        scale: f64,
        p: &Palette,
    ) {
        code_edge(c, rect, true, scale, p);
        let x = super::phys(rect, scale).0;
        let pad = (f64::from(DOC_CODE_PAD) * scale) as f32;
        let copy = copy_spot(rect);
        let end = super::phys(copy, scale).0 - pad;
        let size = Font::Small.size();
        self.text(
            c,
            lang,
            size,
            x + pad,
            end,
            band(rect, scale),
            scale,
            p.text,
        );
        self.button(
            c,
            COPY_CODE,
            copy,
            (lit, false),
            ButtonStyle::Plain,
            scale,
            p,
        );
    }

    /// A line of a code block in `rect`, its runs `spans` (by char) in
    /// code's colours for `ground`, which is painted before it (whole
    /// pixels, so a block's lines meet with no seam).
    pub(super) fn code_line(
        &self,
        c: &mut Canvas,
        rect: Rect,
        (text, spans): (&str, &[(Range<usize>, Kind)]),
        ground: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let (x, _, w, _) = rect_px(rect, scale);
        let pad = (f64::from(DOC_CODE_PAD) * scale) as f32;
        let (mut pen, to) = (x + pad, x + w - pad);
        let b = band(rect, scale);
        let size = Font::Small.size();
        let bytes: Vec<usize> = (text.char_indices().map(|(at, _)| at))
            .chain([text.len()])
            .collect();
        let byte = |n: usize| bytes[n.min(bytes.len() - 1)];
        let mut paint = |from: usize, end: usize, color: Rgb| {
            let (from, end) = (byte(from), byte(end));
            if from < end {
                pen = self.mono_text(c, &text[from..end], size, pen, to, b, scale, color);
            }
        };
        let mut at = 0;
        for (range, kind) in spans {
            paint(at, range.start, p.text);
            paint(range.start, range.end, p.code_on(*kind, ground));
            at = range.end;
        }
        paint(at, usize::MAX, p.text);
    }
}

/// A code block's room above (`top`) or below its lines: its ground a
/// corner's room taller, cut to the edge, so its corners round and its side
/// to the lines is square. Up from its bottom: a page's band paints a piece
/// reaching in from above at a negative top, which rounds the other way
/// (`rect_px`), and the corners would move a pixel.
fn code_edge(c: &mut Canvas, rect: Rect, top: bool, scale: f64, p: &Palette) {
    let s = scale as f32;
    let (x, y, w, h) = rect_px(rect, scale);
    let bottom = (y + h) as i32;
    let h = (f64::from(rect.size.h) * scale).round() as i32;
    let (x, y, w) = (x as i32, bottom - h, w as i32);
    let r = (super::CAP_RADIUS * s).round() as i32;
    let from = if top { y } else { y - r };
    c.clipped(x, y, w, h, |c| {
        let (x, from, w, h) = (x as f32, from as f32, w as f32, (h + r) as f32);
        c.rounded(x, from, w, h, r as f32, code_ground(p));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A measure of one unit a character, two in the monospace font.
    fn chars(text: &Styled) -> f32 {
        line_width(&text.text, &text.runs, false, |t, face| {
            let unit = if face == Face::Mono { 2.0 } else { 1.0 };
            t.chars().count() as f32 * unit
        })
    }

    /// `text`'s plain lines `width` wide by `chars`.
    fn lines(text: &str, width: i32) -> Vec<String> {
        let lines = doc_lines(&text.into(), width, chars);
        lines.into_iter().map(|line| line.text).collect()
    }

    /// Markdown `md`'s paragraph broken into lines `width` wide by `chars`.
    fn styled_lines(md: &str, width: i32) -> Vec<Styled> {
        let Some(DocBlock::Para(text)) = crate::md_blocks::blocks(md).into_iter().next() else {
            panic!("a paragraph");
        };
        doc_lines(&text, width, chars)
    }

    /// `text` with `runs`.
    fn run(text: &str, runs: &[(Range<usize>, Inline)]) -> Styled {
        Styled {
            text: text.into(),
            runs: runs.to_vec(),
        }
    }

    /// A line of code spans is no wider than its place: their spaces are
    /// measured in the monospace font.
    #[test]
    fn spaces_inside_code_count_at_their_own_width() {
        // 20 words and 19 spaces, two units each, and the span's room on
        // both sides: 86 wide.
        let line = format!("`{}`", ["a"; 20].join(" "));
        assert_eq!(styled_lines(&line, 86).len(), 1);
        assert_eq!(styled_lines(&line, 78).len(), 2);
    }

    #[test]
    fn a_files_lines_keep_all_of_it_and_break_a_long_word() {
        assert_eq!(lines("a bb ccc", 4), ["a bb", "ccc"]);
        // Brackets don't hold words together, as they do on a card's line.
        assert_eq!(lines("x [a b c]", 4), ["x [a", "b c]"]);
        assert_eq!(
            lines("see abcdefghij end", 4),
            ["see", "abcd", "efgh", "ij", "end"]
        );
        assert!(lines("", 4).is_empty());
        // Too narrow for a character: still all of it, one to a line.
        assert_eq!(lines("abc", 0), ["a", "b", "c"]);
        // Wider than one unit a byte can't split a character.
        assert_eq!(
            lines("\u{e9}\u{e9}\u{e9}", 2).concat(),
            "\u{e9}\u{e9}\u{e9}"
        );
    }

    /// A run cut by a line's end goes on on the next; a space inside a run
    /// is in it, one between two runs isn't.
    #[test]
    fn runs_wrap_across_lines_and_keep_their_spaces() {
        let bold = Inline::Bold;
        assert_eq!(
            styled_lines("a **bb cc** dd", 5),
            [
                run("a bb", &[(2..4, bold.clone())]),
                run("cc dd", &[(0..2, bold.clone())])
            ]
        );
        assert_eq!(
            styled_lines("a **bb cc** dd", 7),
            [run("a bb cc", &[(2..7, bold.clone())]), run("dd", &[])]
        );
        assert_eq!(
            styled_lines("**a** **b**", 9),
            [run("a b", &[(0..1, bold.clone()), (2..3, bold.clone())])]
        );
        // A word in two runs, broken where it must, keeps both.
        assert_eq!(
            styled_lines("**abc**d", 2),
            [
                run("ab", &[(0..2, bold.clone())]),
                run("cd", &[(0..1, bold)])
            ]
        );
    }

    /// A line is as wide as its runs in their faces: code takes more here.
    #[test]
    fn a_runs_face_sets_how_much_fits() {
        let code = Inline::Code;
        // `cd` is 4 wide and its room 4 on each side: 12.
        assert_eq!(
            styled_lines("ab `cd` ef", 15),
            [run("ab cd", &[(3..5, code.clone())]), run("ef", &[])]
        );
        assert_eq!(lines("ab cd ef", 7), ["ab cd", "ef"]);
        assert_eq!(styled_lines("ab `cd` ef", 14).len(), 3);
        assert_eq!(lines("ab cd ef", 6).len(), 2);
    }

    /// A press finds the link it is on, by the runs' widths.
    #[test]
    fn a_link_is_found_where_it_is_drawn() {
        let url = || Inline::Link("https://x.y".into());
        let text = "go **there** now";
        let Some(DocBlock::Para(line)) =
            crate::md_blocks::blocks("go [**there**](https://x.y) now")
                .into_iter()
                .next()
        else {
            panic!("a paragraph");
        };
        assert_eq!(line, run(text.replace("**", "").as_str(), &[(3..8, url())]));
        let at = |x: f32| link_at(&line.text, &line.runs, Font::Body, false, x);
        let start = width_in("go ", Face::Ui, Font::Body.size());
        let end = start + width_in("there", Face::Ui, Font::Body.size());
        assert_eq!(at(start - 1.0), None);
        assert_eq!(at(start + 1.0), Some("https://x.y"));
        assert_eq!(at(end - 1.0), Some("https://x.y"));
        assert_eq!(at(end + 1.0), None);
    }

    /// A list item's lines all start right of its mark, deeper the deeper.
    #[test]
    fn a_list_items_text_hangs_right_of_its_mark() {
        let long = "word ".repeat(80);
        let item = |depth, mark| DocBlock::Item {
            depth,
            mark,
            text: long.as_str().into(),
        };
        let blocks = [item(0, ItemMark::Bullet), item(1, ItemMark::Number(3))];
        let s = stack(&blocks, 300, (0, 300));
        let marks: Vec<Rect> = (s.pieces.iter())
            .filter(|(_, p)| matches!(p, DocPiece::ItemMark(_)))
            .map(|(r, _)| *r)
            .collect();
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].loc.x, 0);
        assert_eq!(marks[1].loc.x, DOC_INDENT);
        let lines = |under: i32, to: i32| -> Vec<Rect> {
            (s.pieces.iter())
                .filter(|(r, p)| {
                    matches!(p, DocPiece::Text { .. }) && r.loc.y >= under && r.loc.y < to
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
        let blocks = Arc::new(vec![DocBlock::Para("words to lay out".into())]);
        let (first, h) = laid_out(&blocks, 300);
        let (again, same) = laid_out(&blocks, 300);
        assert!(Rc::ptr_eq(&first, &again));
        assert_eq!(h, same);
        let (wider, _) = laid_out(&blocks, 400);
        assert!(!Rc::ptr_eq(&first, &wider));
        // Going between two widths, both are kept.
        assert!(Rc::ptr_eq(&first, &laid_out(&blocks, 300).0));
        let other = Arc::new(blocks.as_ref().clone());
        assert!(!Rc::ptr_eq(&first, &laid_out(&other, 300).0));
    }

    /// Painted at an origin, the pieces land there: a rule's line moves
    /// with it.
    #[test]
    fn pieces_paint_at_their_origin() {
        let p = super::super::Theme::Dark.page_palette();
        let painter = Painter::new();
        let (laid, h) = laid_out(&Arc::new(vec![DocBlock::Rule]), 40);
        let lit = |at: (i32, i32)| {
            let mut c = Canvas::new((60, 60).into());
            paint(&painter, &mut c, &laid, at.into(), 1.0, p);
            let row = (c.w * 4) as usize;
            (0..c.h)
                .filter(|&y| c.px[y as usize * row..][..row].iter().any(|&b| b != 0))
                .collect::<Vec<_>>()
        };
        let (top, down) = (lit((0, 0)), lit((10, 20)));
        assert!(!top.is_empty() && top.iter().all(|&y| y < h));
        assert_eq!(down, top.iter().map(|y| y + 20).collect::<Vec<_>>());
    }
    #[test]
    fn columns_get_their_longest_word_then_share_what_is_left() {
        assert_eq!(fitted(&[100, 50], &[20, 10], 90), [60, 30]);
        assert_eq!(fitted(&[100, 50], &[60, 40], 50), [30, 20]);
    }

    #[test]
    fn a_tables_cells_sit_side_by_side_and_wrap_to_fit() {
        let table = [DocBlock::Table {
            align: Vec::new(),
            rows: vec![
                vec!["Case".into(), "Reply".into()],
                vec![
                    "a long cell of many words that has to wrap".into(),
                    "401".into(),
                ],
            ],
        }];
        let texts = |pieces: &[(Rect, DocPiece)]| -> Vec<(Rect, String, bool)> {
            (pieces.iter())
                .filter_map(|(r, p)| match p {
                    DocPiece::Text { text, bold, .. } => Some((*r, text.clone(), *bold)),
                    _ => None,
                })
                .collect()
        };
        let cells = |pieces: &[(Rect, DocPiece)]| {
            (pieces.iter())
                .filter(|(_, p)| matches!(p, DocPiece::Cell { .. }))
                .count()
        };
        let (wide, wide_h) = stack_of(&table, 2000);
        let words = texts(&wide);
        assert_eq!(words.len(), 4, "{words:?}");
        assert_eq!(cells(&wide), 4);
        // The header bold, and its cells on one line.
        assert!(words[0].2 && words[1].2 && !words[2].2);
        assert_eq!(words[0].0.loc.y, words[1].0.loc.y);
        assert!(words[1].0.loc.x > words[0].0.loc.x);
        let (narrow, narrow_h) = stack_of(&table, 200);
        assert!(texts(&narrow).len() > 4);
        assert!(narrow_h > wide_h);
        let edge = (narrow.iter()).map(|(r, _)| r.loc.x + r.size.w).max();
        assert!(edge <= Some(200), "{edge:?}");
        // Cells' grounds come before the words on them.
        assert!(matches!(narrow[0].1, DocPiece::Cell { head: true, .. }));
    }

    /// The texts `md` lays out at `w`: where each is, and what it says.
    fn texts_of(md: &str, w: i32) -> Vec<(Rect, String)> {
        let (pieces, _) = stack_of(&crate::md_blocks::blocks(md), w);
        (pieces.into_iter())
            .filter_map(|(r, p)| match p {
                DocPiece::Text { text, .. } => Some((r, text)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_columns_alignment_sets_where_its_words_sit() {
        let words = texts_of(
            "| left | mid | right |\n|:--|:-:|--:|\n| a | b | c |\n",
            2000,
        );
        let at = |t: &str| words.iter().find(|(_, w)| w == t).expect(t).0;
        assert_eq!(at("a").loc.x, at("left").loc.x);
        let (mid, b) = (at("mid"), at("b"));
        assert!(b.loc.x > mid.loc.x, "{b:?} {mid:?}");
        let (right, c) = (at("right"), at("c"));
        assert_eq!(c.loc.x + c.size.w, right.loc.x + right.size.w);
        assert!(c.loc.x > right.loc.x);
    }

    #[test]
    fn blocks_in_a_quote_or_a_list_item_sit_inside_it() {
        let (pieces, _) = stack_of(
            &crate::md_blocks::blocks("> ```\n> ls\n> ```\n\n- Run:\n\n  ```\n  ls\n  ```\n"),
            400,
        );
        let codes: Vec<Rect> = (pieces.iter())
            .filter(|(_, p)| matches!(p, DocPiece::CodeLine(..)))
            .map(|(r, _)| *r)
            .collect();
        assert_eq!(codes.len(), 2, "{pieces:?}");
        assert_eq!(codes[0].loc.x, DOC_QUOTE);
        assert_eq!(codes[1].loc.x, DOC_MARK);
        let bars: Vec<_> = (pieces.iter())
            .filter(|(_, p)| matches!(p, DocPiece::QuoteBar(None)))
            .collect();
        assert_eq!(bars.len(), 1);
        // The bar runs along all the quote's code.
        assert!(bars[0].0.loc.y + bars[0].0.size.h >= codes[0].loc.y + codes[0].size.h);
    }

    #[test]
    fn an_alert_has_its_title_and_its_colours_bar() {
        let (pieces, _) = stack_of(&crate::md_blocks::blocks("> [!CAUTION]\n> Hot.\n"), 400);
        assert!((pieces.iter()).any(|(_, p)| *p == DocPiece::QuoteBar(Some(Alert::Caution))));
        let words: Vec<_> = (pieces.iter())
            .filter_map(|(_, p)| match p {
                DocPiece::Text {
                    text, ink, bold, ..
                } => Some((text.as_str(), *ink, *bold)),
                _ => None,
            })
            .collect();
        // An alert's words are not muted as a quote's are.
        assert_eq!(
            words,
            [("Caution", Ink::Text, true), ("Hot.", Ink::Text, false)]
        );
        let (pieces, _) = stack_of(&crate::md_blocks::blocks("> Said.\n"), 400);
        assert!((pieces.iter()).any(|(_, p)| matches!(
            p,
            DocPiece::Text {
                ink: Ink::Muted,
                ..
            }
        )));
    }

    #[test]
    fn headings_step_down_in_size() {
        let fonts: Vec<(Font, bool)> = (stack_of(
            &crate::md_blocks::blocks("# a\n## b\n### c\n#### d\n##### e\n###### f\n"),
            400,
        )
        .0
        .into_iter())
        .filter_map(|(_, p)| match p {
            DocPiece::Text { font, bold, .. } => Some((font, bold)),
            _ => None,
        })
        .collect();
        let sizes: Vec<f32> = fonts.iter().map(|(f, _)| f.size()).collect();
        assert!(fonts.iter().all(|(_, bold)| *bold));
        assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2], "{sizes:?}");
        assert!(sizes[3] > sizes[4], "{sizes:?}");
    }

    #[test]
    fn a_png_image_is_drawn_to_fit_others_are_their_alt_as_a_link() {
        let dir = std::env::temp_dir().join(format!("gerdr-doc-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        resvg::tiny_skia::Pixmap::new(800, 100)
            .unwrap()
            .save_png(&path)
            .unwrap();
        let md = format!("![wide]({})\n\n![web](https://x.y/a.png)\n", path.display());
        let (pieces, h) = stack_of(&crate::md_blocks::blocks(&md), 400);
        let image = (pieces.iter()).find(|(_, p)| matches!(p, DocPiece::Image(_)));
        let (rect, _) = image.expect("drawn");
        assert_eq!((rect.size.w, rect.size.h), (400, 50));
        assert!(h > 50);
        let web = (pieces.iter()).find_map(|(_, p)| match p {
            DocPiece::Text { text, runs, .. } => Some((text.clone(), runs.clone())),
            _ => None,
        });
        assert_eq!(
            web,
            Some((
                "web".into(),
                vec![(0..3, Inline::Link("https://x.y/a.png".into()))]
            ))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
