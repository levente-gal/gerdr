//! A markdown file's blocks (`md_blocks`) laid out in a column and painted:
//! the copy of a stage's file on its feature page. What is laid out is
//! pieces from `(0, 0)`, kept for a width (`laid_out`), so a page can put
//! them where it will. Runs of a line (`md_blocks::Inline`) are set in
//! their own face: bold in bold, code in the monospace font, a link in the
//! accent.

use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc};

use smithay::utils::{Logical, Point, Rectangle};

use super::features::{BODY_LINE, Font, Ink, SMALL_LINE, StageMark, stage_dot, width_of};
use super::glyphs::{self, Face};
use super::{Canvas, Painter, Palette, Rect, Rgb, band, check, icon_sdf, rect_px};
use crate::md_blocks::{DocBlock, Inline, ItemMark, Styled, spans};

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
const DOC_RULE: i32 = 13;
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
    /// A line of a code block, cut where the column ends, on the block's
    /// ground; an empty one is the block's edge.
    CodeLine(String),
    /// The bar beside a quote.
    QuoteBar,
    /// A thematic break.
    Rule,
}

/// How a line's run `on` is set, the line bold or not.
pub(super) fn face(on: Option<&Inline>, bold: bool) -> Face {
    match on {
        Some(Inline::Code) => Face::Mono,
        Some(Inline::Bold) => Face::Bold,
        _ if bold => Face::Bold,
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

/// How wide line `text` with `runs` is, bold or not, each run measured by
/// `width` in its face.
pub(super) fn line_width(
    text: &str,
    runs: &[(Range<usize>, Inline)],
    bold: bool,
    width: impl Fn(&str, Face) -> f32,
) -> f32 {
    (spans(text, runs))
        .map(|(range, on)| width(&text[range], face(on, bold)))
        .sum()
}

/// The web link `x` logical pixels into line `text` with `runs`, in `font`,
/// bold or not.
pub(super) fn link_at<'a>(
    text: &str,
    runs: &'a [(Range<usize>, Inline)],
    font: Font,
    bold: bool,
    x: f32,
) -> Option<&'a str> {
    let mut pen = 0.0;
    for (range, on) in spans(text, runs) {
        pen += width_in(&text[range], face(on, bold), font.size());
        if x < pen {
            return match on {
                Some(Inline::Link(url)) => Some(url),
                _ => None,
            };
        }
    }
    None
}

/// `text` broken into lines no wider than `width` by `measure`, all of it
/// kept: a word wider than a line (a URL) breaks where it must. Runs go on
/// over the lines they are cut into, and a space between two words of one
/// run is in it. A long file has many words: a line's width is added up
/// word by word, and measured whole only when that comes within
/// `DOC_WIDTH_SLACK` of the edge.
fn doc_lines(text: &Styled, width: i32, measure: impl Fn(&Styled) -> f32) -> Vec<Styled> {
    let width = width as f32;
    let space = measure(&"a a".into()) - 2.0 * measure(&"a".into());
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

/// A column being laid out, from its top.
struct Column {
    x: i32,
    w: i32,
    y: i32,
    pieces: Vec<(Rect, DocPiece)>,
}

impl Column {
    fn put(&mut self, h: i32, piece: DocPiece) {
        let rect = Rectangle::new((self.x, self.y).into(), (self.w, h).into());
        self.pieces.push((rect, piece));
        self.y += h;
    }

    /// `text` as a file's text: all of its lines, a line `h` high each.
    fn text(&mut self, text: &Styled, font: Font, ink: Ink, bold: bool, h: i32) {
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
        for Styled { text, runs } in lines {
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
                let spot = Rectangle::new((s.x, s.y).into(), (gutter, BODY_LINE).into());
                s.pieces.push((spot, DocPiece::ItemMark(mark)));
            }
            s.indented(gutter, |s| {
                s.text(text, Font::Body, Ink::Text, false, BODY_LINE);
            });
        });
        // A mark with no words of its own still takes its line.
        self.y = self.y.max(top + BODY_LINE);
    }

    /// A file's block, from here down.
    fn block(&mut self, block: &DocBlock) {
        match block {
            DocBlock::Heading { level: 1, text } => {
                self.text(text, Font::Heading, Ink::Text, true, BODY_LINE + 6);
            }
            DocBlock::Heading { text, .. } => {
                self.text(text, Font::Body, Ink::Text, true, BODY_LINE);
            }
            DocBlock::Para(text) => self.text(text, Font::Body, Ink::Text, false, BODY_LINE),
            DocBlock::Note(text) => {
                self.text(
                    &text.as_str().into(),
                    Font::Body,
                    Ink::Muted,
                    false,
                    BODY_LINE,
                );
            }
            DocBlock::Item { depth, mark, text } => self.item(*depth, *mark, text),
            DocBlock::Code(lines) => {
                let edge =
                    |s: &mut Column| s.put(DOC_CODE_PAD / 2, DocPiece::CodeLine(String::new()));
                edge(self);
                for code in lines {
                    self.put(SMALL_LINE, DocPiece::CodeLine(code.clone()));
                }
                edge(self);
            }
            DocBlock::Quote(text) => {
                let top = self.y;
                self.indented(DOC_QUOTE, |s| {
                    s.text(text, Font::Body, Ink::Muted, false, BODY_LINE);
                });
                let bar =
                    Rectangle::new((self.x, top).into(), (DOC_QUOTE_BAR, self.y - top).into());
                self.pieces.push((bar, DocPiece::QuoteBar));
            }
            DocBlock::Table(rows) => {
                for (i, row) in rows.iter().enumerate() {
                    self.text(row, Font::Small, Ink::Text, i == 0, SMALL_LINE);
                }
            }
            DocBlock::Rule => {
                self.put(DOC_RULE, DocPiece::Rule);
            }
        }
    }
}

/// A file's blocks laid out for a column `w` wide, from `(0, 0)`.
fn stack(blocks: &[DocBlock], w: i32) -> Column {
    let mut s = Column {
        x: 0,
        w,
        y: 0,
        pieces: Vec::new(),
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
        s.block(block);
    }
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
    static DOC_MEMO: RefCell<Vec<DocMemo>> = const { RefCell::new(Vec::new()) };
}

/// `blocks` laid out for a column `w` wide: the pieces from `(0, 0)`, and
/// how high they all are.
pub(super) fn laid_out(blocks: &Arc<Vec<DocBlock>>, w: i32) -> (Rc<Vec<(Rect, DocPiece)>>, i32) {
    DOC_MEMO.with_borrow_mut(|memo| {
        let found = (memo.iter()).position(|m| m.w == w && Arc::ptr_eq(&m.blocks, blocks));
        let entry = match found {
            Some(i) => memo.remove(i),
            None => {
                let s = stack(blocks, w);
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
                    pen = match face(on, *bold) {
                        Face::Mono => self.mono_text(c, run, size, pen, end, b, scale, color),
                        Face::Bold => self.bold_text(c, run, size, pen, end, b, scale, color),
                        _ => self.text(c, run, size, pen, end, b, scale, color),
                    };
                }
            }
            DocPiece::ItemMark(mark) => {
                let spot = Rectangle::new(
                    (rect.loc.x, rect.loc.y + (BODY_LINE - 14) / 2).into(),
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
            DocPiece::CodeLine(text) => self.code_line(c, rect, text, p.element, scale, p),
            DocPiece::QuoteBar => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, w / 2.0, p.border);
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
        }
    }

    /// A line of a code block in `rect`, on `ground`. On whole pixels, so a
    /// block's lines meet with no seam.
    pub(super) fn code_line(
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
        self.mono_text(c, text, Font::Small.size(), from, to, b, scale, p.text);
    }
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
        assert_eq!(
            styled_lines("ab `cd` ef", 7),
            [run("ab cd", &[(3..5, code.clone())]), run("ef", &[])]
        );
        assert_eq!(lines("ab cd ef", 7), ["ab cd", "ef"]);
        assert_eq!(styled_lines("ab `cd` ef", 6).len(), 3);
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
        let s = stack(&blocks, 300);
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
}
