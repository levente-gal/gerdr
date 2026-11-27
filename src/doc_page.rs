//! A markdown file's page in its tab (`doc_tab`), painted by gerdr
//! (`chrome::doc_page`): its blocks (`md_blocks::lined`) in a column,
//! across the tab in Preview or beside its review in Split. Each block
//! knows the lines of the file it comes from, so the page marks the block
//! the review's caret is in and those with comments, a click on a block
//! puts the review's caret on the line clicked, and the two scroll
//! together: each stands with the same line of the file at its top, to
//! the fraction (`DiffView::top_line`).
//!
//! The markdown is an agent's: raw HTML in it keeps only its words
//! (`md_blocks`). Its links open through gerdr, a file's in Zed and a web
//! page's in a tab of the workspace.

use std::{
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
    time::Instant,
};

use crate::{
    md_blocks::{self, DocBlock, Lines},
    md_preview::FILE_MAX,
    scroll::{Bounds, Motion, NOTCH_PX, Scroll},
};

/// What the page says of a file too big for it.
const TOO_BIG: &str = "Too big to preview: use Review";

/// A file's page: its blocks and how far down it is scrolled.
#[derive(Default)]
pub struct DocPage {
    /// What the blocks were made from: its text's hash.
    of: Option<u64>,
    pub blocks: Arc<Vec<DocBlock>>,
    pub lines: Arc<Lines>,
    /// How far down, in logical pixels.
    pub scroll: Scroll,
    /// The lines at the top of the page and of its review as they last
    /// stood together: the one moved off its own moves the other.
    pub stood: Option<(f64, f64)>,
}

impl DocPage {
    /// Its blocks made from `text`, a file in `dir` (its links relative to
    /// it), if they aren't yet; why it can't be read in its place.
    pub fn set_text(&mut self, text: Result<&str, &str>, dir: Option<&Path>) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        dir.hash(&mut hasher);
        let of = hasher.finish();
        if self.of == Some(of) {
            return;
        }
        self.of = Some(of);
        let (blocks, lines) = match text {
            Ok(text) if text.len() as u64 > FILE_MAX => (vec![DocBlock::Note(TOO_BIG.into())], vec![]),
            Ok(text) => match dir {
                Some(dir) => md_blocks::lined_in(text, dir),
                None => md_blocks::lined(text),
            },
            Err(why) => (vec![DocBlock::Note(why.to_owned())], vec![]),
        };
        // A note has no lines of the file: it stands for none of them.
        let lines = match lines.len() == blocks.len() {
            true => lines,
            false => Vec::new(),
        };
        self.blocks = Arc::new(blocks);
        self.lines = Arc::new(lines);
        // Laid out anew, it stands where its review does.
        self.stood = None;
    }

    /// How far down it stands, in logical pixels.
    pub fn top(&self) -> f64 {
        self.scroll.pos()
    }
}

/// How a page `h` high in all, shown `view` high, scrolls.
pub fn bounds(h: f64, view: f64) -> Bounds {
    Bounds {
        max: (h - view).max(0.0),
        notch: NOTCH_PX,
        unit: 1.0,
        next: 1.0,
        prev: 1.0,
        view,
    }
}

/// The wheel or the touchpad moved `page`, as `b` bounds it.
pub fn scroll(page: &mut DocPage, motion: Motion, b: Bounds) {
    page.scroll.apply(motion, Instant::now(), b);
}

/// Of the blocks from `lines`, the one holding `line`: or else the last
/// before it (a blank line between blocks); none before the first.
pub fn pick_block(lines: &[(usize, usize)], line: usize) -> Option<usize> {
    let holding = lines.iter().rposition(|&(a, b)| a <= line && line <= b);
    holding.or_else(|| lines.iter().rposition(|&(a, _)| a <= line))
}

/// A block's lines and place down the page, `(first, last, top, height)`.
type Span = (f64, f64, f64, f64);

/// Each block's lines with where it went, `spans` its tops and bottoms.
fn joined(lines: &[(usize, usize)], spans: &[(i32, i32)]) -> Vec<Span> {
    (lines.iter().zip(spans))
        .map(|(&(a, b), &(top, bottom))| (a as f64, b as f64, top as f64, (bottom - top) as f64))
        .collect()
}

/// Where down the page line `line` of the file is, to the fraction: as far
/// into its block as the line is into its lines, or between the blocks
/// around a line between them. The page's top is the start of the line
/// before the first block.
pub fn line_y(lines: &[(usize, usize)], spans: &[(i32, i32)], line: f64) -> f64 {
    let spans = joined(lines, spans);
    if spans.is_empty() {
        return 0.0;
    }
    let mut holding = None;
    let mut before: Option<Span> = None;
    let mut after = None;
    for &span in &spans {
        let (first, last, ..) = span;
        if first <= line && line < last + 1.0 {
            holding = Some(span);
        }
        if last + 1.0 <= line && before.is_none_or(|b| last >= b.1) {
            before = Some(span);
        }
        if first > line && after.is_none() {
            after = Some(span);
        }
    }
    if let Some((first, last, top, height)) = holding {
        return top + (line - first) / (last + 1.0 - first) * height;
    }
    let (from, from_y) = match (before, after) {
        (Some(b), _) => (b.1 + 1.0, b.2 + b.3),
        (None, Some(a)) => (a.0 - 1.0, 0.0),
        (None, None) => (1.0, 0.0),
    };
    let Some((to, _, to_y, _)) = after else {
        return from_y;
    };
    if to <= from {
        return to_y;
    }
    (from_y + (line - from) / (to - from) * (to_y - from_y)).max(0.0)
}

/// The line of the file at `y` down the page, to the fraction: `line_y`
/// turned round.
pub fn y_line(lines: &[(usize, usize)], spans: &[(i32, i32)], y: f64) -> f64 {
    let spans = joined(lines, spans);
    if spans.is_empty() {
        return 1.0;
    }
    let mut holding = None;
    let mut before: Option<Span> = None;
    let mut after: Option<Span> = None;
    for &span in &spans {
        let (_, _, top, height) = span;
        if top <= y && y < top + height {
            holding = Some(span);
        }
        if top + height <= y && before.is_none_or(|b| top + height >= b.2 + b.3) {
            before = Some(span);
        }
        if top > y && after.is_none_or(|a| top < a.2) {
            after = Some(span);
        }
    }
    if let Some((first, last, top, height)) = holding {
        return first + (y - top) / height.max(1.0) * (last + 1.0 - first);
    }
    let (from, from_y) = match (before, after) {
        (Some(b), _) => (b.1 + 1.0, b.2 + b.3),
        (None, Some(a)) => (a.0 - 1.0, 0.0),
        (None, None) => (1.0, 0.0),
    };
    let Some((to, _, to_y, _)) = after else {
        return from;
    };
    if to_y <= from_y {
        return to;
    }
    from + (y - from_y) / (to_y - from_y) * (to - from)
}

/// The line a click at `y` down the page landed on, in block `block`.
pub fn clicked_line(lines: &[(usize, usize)], spans: &[(i32, i32)], block: usize, y: f64) -> usize {
    let (Some(&(first, last)), Some(&span)) = (lines.get(block), spans.get(block)) else {
        return 1;
    };
    let line = y_line(&[(first, last)], &[span], y).floor() as usize;
    line.clamp(first, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines 1, 3-5 and 9; down the page at 0-20, 30-90 and 100-120.
    const LINES: [(usize, usize); 3] = [(1, 1), (3, 5), (9, 9)];
    const SPANS: [(i32, i32); 3] = [(0, 20), (30, 90), (100, 120)];

    #[test]
    fn a_line_finds_its_block_or_the_one_before() {
        assert_eq!(pick_block(&LINES, 4), Some(1));
        assert_eq!(pick_block(&LINES, 7), Some(1));
        assert_eq!(pick_block(&LINES, 9), Some(2));
        assert_eq!(pick_block(&[(3, 4)], 1), None);
    }

    #[test]
    fn a_line_and_its_place_down_the_page_go_both_ways() {
        assert_eq!(line_y(&LINES, &SPANS, 1.0), 0.0);
        assert_eq!(line_y(&LINES, &SPANS, 4.0), 50.0);
        // Line 2 is between the first block's end and the second's start.
        assert_eq!(line_y(&LINES, &SPANS, 2.5), 25.0);
        assert_eq!(line_y(&LINES, &SPANS, 12.0), 120.0);
        for line in [1.0, 1.5, 3.0, 4.25, 7.0, 9.5] {
            let back = y_line(&LINES, &SPANS, line_y(&LINES, &SPANS, line));
            assert!((back - line).abs() < 1e-9, "{line} came back as {back}");
        }
        assert_eq!(y_line(&[], &[], 50.0), 1.0);
    }

    #[test]
    fn a_page_above_its_first_block_and_past_its_last_stands_at_its_ends() {
        // A heading (line 1) at 0-40, items 3 and 4-6 at 60-85 and 85-160, a
        // paragraph (8-9) at 180-220.
        let lines = [(1, 1), (3, 3), (4, 6), (8, 9)];
        let spans = [(0, 40), (60, 85), (85, 160), (180, 220)];
        assert_eq!(line_y(&lines, &spans, 1.5), 20.0);
        assert_eq!(line_y(&lines, &spans, 5.0), 85.0 + 25.0);
        assert_eq!(line_y(&lines, &spans, 2.0), 40.0);
        assert_eq!(line_y(&lines, &spans, 7.5), 170.0);
        assert_eq!(line_y(&lines, &spans, 50.0), 220.0);
        assert_eq!(y_line(&lines, &spans, 1000.0), 10.0);
        // Above the first block, the line before it, from the page's top.
        let low = [(1, 1), (3, 3)];
        let low_spans = [(32, 72), (100, 120)];
        assert_eq!(line_y(&low, &low_spans, 0.0), 0.0);
        assert_eq!(line_y(&low, &low_spans, 0.5), 16.0);
        assert_eq!(line_y(&low, &low_spans, -2.0), 0.0);
        assert_eq!(y_line(&low, &low_spans, 0.0), 0.0);
        assert_eq!(y_line(&low, &low_spans, 16.0), 0.5);
        assert_eq!(line_y(&[], &[], 3.0), 0.0);
    }

    #[test]
    fn a_click_lands_on_a_line_of_its_block() {
        assert_eq!(clicked_line(&LINES, &SPANS, 1, 31.0), 3);
        assert_eq!(clicked_line(&LINES, &SPANS, 1, 70.0), 5);
        assert_eq!(clicked_line(&LINES, &SPANS, 1, 200.0), 5);
        // Lines 10 to 19, 20 px each, from 100.
        let (l, sp) = ([(10, 19)], [(100, 300)]);
        assert_eq!(clicked_line(&l, &sp, 0, 155.0), 12);
        assert_eq!(clicked_line(&l, &sp, 0, 299.0), 19);
        assert_eq!(clicked_line(&l, &sp, 0, 90.0), 10);
    }

    #[test]
    fn a_page_is_made_again_only_for_another_text() {
        let mut page = DocPage::default();
        page.set_text(Ok("# a\n\nb\n"), None);
        let blocks = page.blocks.clone();
        page.set_text(Ok("# a\n\nb\n"), None);
        assert!(Arc::ptr_eq(&blocks, &page.blocks));
        assert_eq!(*page.lines, [(1, 1), (3, 3)]);
        page.set_text(Err("brief.md is gone"), None);
        assert_eq!(*page.blocks, [DocBlock::Note("brief.md is gone".into())]);
        assert!(page.lines.is_empty());
        page.set_text(Ok(&"x".repeat(FILE_MAX as usize + 1)), None);
        assert_eq!(*page.blocks, [DocBlock::Note(TOO_BIG.into())]);
    }
}
