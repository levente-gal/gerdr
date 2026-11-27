//! A markdown file's page in its tab (see `crate::doc_page`), painted in
//! the place the tab's flat panel leaves it (`ReposLayout::page`): its
//! blocks laid out by `doc_layout` in a column, scrolled; a bar left of the
//! block its review's caret is in, and a dot further left of each block
//! with comments.

use std::{ops::Range, rc::Rc, sync::Arc};

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    utils::{Logical, Point, Rectangle},
};

use super::doc_layout::{self, DocPiece};
use super::features::body_line;
use super::{Canvas, Painter, Palette, Rect, physical};
use crate::md_blocks::DocBlock;

/// The widest the page's column goes, as the agent chat's.
const PAGE_MAX_W: i32 = 800;
/// The room around the column; left of it, the caret's bar and the
/// comments' dots.
const PAGE_PAD: i32 = 32;
/// That room as a page scrolls by it: the room above its content.
pub const DOC_PAGE_ROOM: f64 = PAGE_PAD as f64;
const CARET_BAR: i32 = 3;
const CARET_GAP: i32 = 10;
const SAID_DOT: i32 = 6;
const SAID_GAP: i32 = 22;

/// What a file's page shows.
#[derive(Clone, Debug)]
pub struct DocPageView {
    /// Where it goes, in output coordinates.
    pub rect: Rect,
    pub blocks: Arc<Vec<DocBlock>>,
    /// How far down it is scrolled, in logical pixels.
    pub top: f64,
    /// The block its review's caret is in, and those with comments, by
    /// their index in `blocks`.
    pub caret: Option<usize>,
    pub said: Vec<usize>,
}

impl PartialEq for DocPageView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.blocks, &other.blocks)
            && self.rect == other.rect
            && self.top == other.top
            && self.caret == other.caret
            && self.said == other.said
    }
}

/// What is at a place on a file's page.
#[derive(Clone, Debug, PartialEq)]
pub enum DocPageHit {
    /// A link, to where it points.
    Link(String),
    /// A code block's "Copy", with the lines it copies.
    Copy(String),
    /// The dot of the block with comments, by index.
    Said(usize),
    /// A block, by index, and how far down the page the place is.
    Block(usize, f64),
}

/// A file's page `view` laid out: its column's left and width in output
/// coordinates, and the blocks laid out for it.
struct PageLaid {
    x: i32,
    laid: doc_layout::Laid,
}

fn laid(view: &DocPageView) -> PageLaid {
    let r = view.rect;
    let w = (r.size.w - 2 * PAGE_PAD).clamp(1, PAGE_MAX_W);
    PageLaid {
        x: r.loc.x + (r.size.w - w) / 2,
        laid: doc_layout::laid_spans(&view.blocks, w),
    }
}

/// How high `blocks` are, all of them, on a page `rect`, and where each of
/// them is down it: what its scrolling and its review's go by.
pub fn doc_page_spans(blocks: &Arc<Vec<DocBlock>>, rect: Rect) -> (f64, Rc<Vec<(i32, i32)>>) {
    let view = DocPageView {
        rect,
        blocks: blocks.clone(),
        top: 0.0,
        caret: None,
        said: Vec::new(),
    };
    let laid = laid(&view).laid;
    (f64::from(laid.h + 2 * PAGE_PAD), laid.spans)
}

/// How far down the page, from its content's top, output place `y` is.
fn page_y(view: &DocPageView, y: f64) -> f64 {
    y - f64::from(view.rect.loc.y + PAGE_PAD) + view.top
}

/// What is at `pos` on `view`.
pub fn doc_page_hit(view: &DocPageView, pos: Point<f64, Logical>) -> Option<DocPageHit> {
    if !view.rect.to_f64().contains(pos) {
        return None;
    }
    let PageLaid { x, laid } = laid(view);
    let y = page_y(view, pos.y);
    let at = Point::from(((pos.x - f64::from(x)) as i32, y.floor() as i32));
    for (rect, piece) in laid.pieces.iter() {
        if !rect.contains(at) {
            continue;
        }
        match piece {
            DocPiece::CodeHead { text, .. } if doc_layout::copy_spot(*rect).contains(at) => {
                return Some(DocPageHit::Copy(text.clone()));
            }
            DocPiece::Text {
                text,
                runs,
                font,
                bold,
                ..
            } => {
                let along = (at.x - rect.loc.x) as f32;
                if let Some(url) = doc_layout::link_at(text, runs, *font, *bold, along) {
                    return Some(DocPageHit::Link(url.to_owned()));
                }
            }
            _ => {}
        }
    }
    let y_in = |&(top, bottom): &(i32, i32)| f64::from(top) <= y && y < f64::from(bottom);
    let block = laid.spans.iter().position(y_in)?;
    let left = pos.x < f64::from(x);
    match left && view.said.contains(&block) {
        true => Some(DocPageHit::Said(block)),
        false => Some(DocPageHit::Block(block, y)),
    }
}

impl Painter {
    /// File page `view`, in a buffer its size.
    pub(super) fn doc_page(&self, view: &DocPageView, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let r = view.rect;
        let mut c = Canvas::new(physical(r.size, scale));
        c.fill(0, 0, c.w, c.h, p.elevated);
        let PageLaid { x, laid } = laid(view);
        let px = |v: f64| (v * scale).round() as i32;
        // The column's top, in the buffer's logical pixels.
        let (cx, cy) = (x - r.loc.x, PAGE_PAD - view.top.round() as i32);
        let shown: Range<i32> = -cy..r.size.h - cy;
        for (rect, piece) in laid.pieces.iter() {
            if rect.loc.y + rect.size.h < shown.start || rect.loc.y > shown.end {
                continue;
            }
            let at = Rectangle::new((rect.loc.x + cx, rect.loc.y + cy).into(), rect.size);
            self.doc_piece(&mut c, at, piece, scale, p);
        }
        let span = |b: usize| laid.spans.get(b).map(|&(top, bottom)| (top + cy, bottom - top));
        if let Some((top, h)) = view.caret.and_then(span) {
            let bar = f64::from(cx - CARET_GAP - CARET_BAR);
            let (w, h) = (f64::from(CARET_BAR), f64::from(h));
            let (x, y) = (px(bar), px(f64::from(top)));
            c.rounded(x as f32, y as f32, px(w) as f32, px(h) as f32, px(w / 2.0) as f32, p.accent);
        }
        for (top, _) in view.said.iter().filter_map(|&b| span(b)) {
            let d = f64::from(SAID_DOT);
            let x = f64::from(cx - SAID_GAP);
            let y = f64::from(top) + (f64::from(body_line()) - d) / 2.0;
            let (x, y, d) = (px(x) as f32, px(y) as f32, px(d) as f32);
            c.rounded(x, y, d, d, d / 2.0, p.warning);
        }
        c.into_buffer(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(md: &str) -> DocPageView {
        DocPageView {
            rect: Rectangle::new((100, 50).into(), (600, 400).into()),
            blocks: Arc::new(crate::md_blocks::blocks(md)),
            top: 0.0,
            caret: None,
            said: vec![1],
        }
    }

    #[test]
    fn a_place_on_the_page_is_a_block_its_dot_or_a_link() {
        let v = view("# Head\n\nSee [the site](https://x.y) now.\n");
        let (_, spans) = doc_page_spans(&v.blocks, v.rect);
        let x = 100.0 + 32.0;
        let y = |b: usize, off: i32| f64::from(50 + PAGE_PAD + spans[b].0 + off);
        assert!(matches!(
            doc_page_hit(&v, Point::from((x + 4.0, y(0, 2)))),
            Some(DocPageHit::Block(0, _))
        ));
        assert_eq!(
            doc_page_hit(&v, Point::from((x - 20.0, y(1, 2)))),
            Some(DocPageHit::Said(1))
        );
        let link = (1..400)
            .map(|dx| doc_page_hit(&v, Point::from((x + f64::from(dx), y(1, 4)))))
            .find(|hit| matches!(hit, Some(DocPageHit::Link(_))));
        assert_eq!(link, Some(Some(DocPageHit::Link("https://x.y".into()))));
        assert_eq!(doc_page_hit(&v, Point::from((10.0, 10.0))), None);
    }

    #[test]
    fn a_page_scrolled_down_finds_what_is_under_it_then() {
        let mut v = view("one\n\ntwo\n");
        let (_, spans) = doc_page_spans(&v.blocks, v.rect);
        v.top = f64::from(spans[1].0);
        let hit = doc_page_hit(&v, Point::from((300.0, f64::from(50 + PAGE_PAD + 2))));
        assert!(matches!(hit, Some(DocPageHit::Block(1, _))));
    }
}
