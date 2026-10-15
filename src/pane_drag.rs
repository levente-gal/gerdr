//! A pane dragged by its bar: hold the bar's label with the left button
//! and move. Over another pane of the tab, it lands on the side nearest
//! the pointer (a band along each edge) or, in the middle, swaps with it;
//! over the tab strip, it goes to that tab, or a new one on the "+". The
//! place it would land lights up meanwhile (`ChromeView::pane_drop`). The
//! drop itself goes through `pane_moves`.

use smithay::utils::{Logical, Point, Rectangle};

use crate::{
    Gerdr, Reorderable,
    chrome::{self, Rect, TabHit},
    layout::Side,
};

/// How far in from each edge of a pane, as a share of its width or
/// height, the bands that put the held pane beside it reach. Inside them
/// is the middle, which swaps.
const EDGE_BAND: f64 = 0.25;

/// Where a held pane lands if dropped now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDrop {
    /// On `side` of pane `target` of the shown tab.
    Beside { target: u32, side: Side },
    /// In the place of pane `target`, which takes the held pane's.
    Swap(u32),
    /// Into tab `t` of the active workspace.
    Tab(usize),
    /// Into a new tab of the active workspace.
    NewTab,
}

/// The part of a pane the pointer is over while a pane is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    /// The band along this edge.
    Edge(Side),
    /// The middle, inside all the bands.
    Middle,
}

/// The part of `rect` at `pos`, if `pos` is in it: the middle box, inset
/// by `EDGE_BAND` on every side, or else the edge nearest, measured as a
/// share of the pane's width or height so the bands meet on the diagonals.
fn drop_zone(rect: Rect, pos: Point<f64, Logical>) -> Option<Zone> {
    if rect.size.w <= 0 || rect.size.h <= 0 || !rect.to_f64().contains(pos) {
        return None;
    }
    let fx = (pos.x - rect.loc.x as f64) / rect.size.w as f64;
    let fy = (pos.y - rect.loc.y as f64) / rect.size.h as f64;
    let inner = EDGE_BAND..=1.0 - EDGE_BAND;
    if inner.contains(&fx) && inner.contains(&fy) {
        return Some(Zone::Middle);
    }
    let edges = [
        (fx, Side::Left),
        (1.0 - fx, Side::Right),
        (fy, Side::Up),
        (1.0 - fy, Side::Down),
    ];
    let (_, side) = edges
        .into_iter()
        .fold((f64::INFINITY, Side::Left), |best, (d, side)| {
            if d < best.0 { (d, side) } else { best }
        });
    Some(Zone::Edge(side))
}

/// The half of `rect` a pane put on `side` of it takes, the divider
/// between them left out.
fn near_half(rect: Rect, side: Side) -> Rect {
    let (w, h) = (rect.size.w, rect.size.h);
    let (half_w, half_h) = (w / 2, h / 2);
    let (x, y) = (rect.loc.x, rect.loc.y);
    match side {
        Side::Left => Rectangle::new((x, y).into(), (half_w, h).into()),
        Side::Right => Rectangle::new((x + w - half_w, y).into(), (half_w, h).into()),
        Side::Up => Rectangle::new((x, y).into(), (w, half_h).into()),
        Side::Down => Rectangle::new((x, y + h - half_h).into(), (w, half_h).into()),
    }
}

impl Gerdr {
    /// Where pane `id` of the shown tab, dragged to `pos`, would land.
    /// `None` over itself or nothing it can land on.
    pub fn pane_drop_at(&self, id: u32, pos: Point<f64, Logical>) -> Option<PaneDrop> {
        if pos.x < self.sidebar_width as f64 {
            return None;
        }
        if pos.y < self.tab_strip_height() as f64 {
            let view = self.tab_strip_view();
            return match self.tab_strip_hit_at(&view, pos)? {
                TabHit::Tab(t) | TabHit::Close(t) => (t != view.active).then_some(PaneDrop::Tab(t)),
                TabHit::Plus => Some(PaneDrop::NewTab),
                _ => None,
            };
        }
        let (target, rect) = self
            .pane_rects()
            .into_iter()
            .find(|(_, rect)| rect.to_f64().contains(pos))?;
        if target == id {
            return None;
        }
        Some(match drop_zone(rect, pos)? {
            Zone::Middle => PaneDrop::Swap(target),
            Zone::Edge(side) => PaneDrop::Beside { target, side },
        })
    }

    /// The area `pane_drop` lands in, to light up while the pane is held.
    pub fn pane_drop_rect(&self) -> Option<Rect> {
        if !self
            .reorder
            .is_some_and(|r| matches!(r.what, Reorderable::Pane(_)))
        {
            return None;
        }
        let target = |id: u32| {
            self.pane_rects()
                .into_iter()
                .find_map(|(pane, rect)| (pane == id).then_some(rect))
        };
        let in_strip = |pick: fn(&chrome::TabLayout, usize) -> Option<Rect>, t: usize| {
            let view = self.tab_strip_view();
            let strip = self.tab_strip_rect();
            let layout =
                chrome::tab_layout(view.tabs.len(), strip.size, view.prefix, view.overflow());
            let mut rect = pick(&layout, t)?;
            rect.loc += strip.loc;
            Some(rect)
        };
        match self.pane_drop? {
            PaneDrop::Beside { target: t, side } => Some(near_half(target(t)?, side)),
            PaneDrop::Swap(t) => target(t),
            PaneDrop::Tab(t) => in_strip(|layout, t| layout.tabs.get(t).copied(), t),
            PaneDrop::NewTab => in_strip(|layout, _| Some(layout.plus), 0),
        }
    }

    /// Drop the held pane `id` where `pane_drop` says, and forget it.
    pub fn drop_pane(&mut self, id: u32) {
        match self.pane_drop.take() {
            Some(PaneDrop::Beside { target, side }) => self.move_pane_beside(id, target, side),
            Some(PaneDrop::Swap(target)) => self.swap_panes(id, target),
            Some(PaneDrop::Tab(t)) => self.move_pane_to_tab(id, Some(t)),
            Some(PaneDrop::NewTab) => self.move_pane_to_tab(id, None),
            None => {}
        }
    }

    /// Where the panes of the shown tab go, those still waiting for their
    /// window too: anything there can take a dropped pane.
    fn pane_rects(&self) -> Vec<(u32, Rect)> {
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        let area = self.content_rect();
        let mut rects = tab.shown_rects(area);
        rects.extend(
            tab.expected_rects(area)
                .into_iter()
                .map(|(expected, rect)| (expected.pane, rect)),
        );
        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rectangle::new((100, 50).into(), (400, 200).into())
    }

    fn at(x: f64, y: f64) -> Option<Zone> {
        drop_zone(rect(), (x, y).into())
    }

    #[test]
    fn each_edge_band_puts_the_pane_on_that_side() {
        assert_eq!(at(110.0, 150.0), Some(Zone::Edge(Side::Left)));
        assert_eq!(at(490.0, 150.0), Some(Zone::Edge(Side::Right)));
        assert_eq!(at(300.0, 60.0), Some(Zone::Edge(Side::Up)));
        assert_eq!(at(300.0, 240.0), Some(Zone::Edge(Side::Down)));
    }

    #[test]
    fn the_middle_swaps() {
        assert_eq!(at(300.0, 150.0), Some(Zone::Middle));
        // The inner box reaches a quarter in from every edge.
        assert_eq!(at(200.0, 100.0), Some(Zone::Middle));
        assert_eq!(at(400.0, 200.0), Some(Zone::Middle));
        assert_eq!(at(199.0, 150.0), Some(Zone::Edge(Side::Left)));
    }

    #[test]
    fn corners_go_to_the_nearer_edge() {
        // 5% in from the left, 10% down from the top.
        assert_eq!(at(120.0, 70.0), Some(Zone::Edge(Side::Left)));
        // 10% in from the left, 5% down from the top.
        assert_eq!(at(140.0, 60.0), Some(Zone::Edge(Side::Up)));
        // 5% in from the right, 10% up from the bottom.
        assert_eq!(at(480.0, 230.0), Some(Zone::Edge(Side::Right)));
        // 10% in from the right, 5% up from the bottom.
        assert_eq!(at(460.0, 240.0), Some(Zone::Edge(Side::Down)));
    }

    #[test]
    fn outside_the_pane_is_no_zone() {
        assert_eq!(at(99.0, 150.0), None);
        assert_eq!(at(300.0, 250.0), None);
        assert_eq!(
            drop_zone(
                Rectangle::new((0, 0).into(), (0, 10).into()),
                (0.0, 5.0).into()
            ),
            None
        );
    }

    #[test]
    fn the_near_half_is_highlighted() {
        let r = rect();
        assert_eq!(
            near_half(r, Side::Left),
            Rectangle::new((100, 50).into(), (200, 200).into())
        );
        assert_eq!(
            near_half(r, Side::Right),
            Rectangle::new((300, 50).into(), (200, 200).into())
        );
        assert_eq!(
            near_half(r, Side::Up),
            Rectangle::new((100, 50).into(), (400, 100).into())
        );
        assert_eq!(
            near_half(r, Side::Down),
            Rectangle::new((100, 150).into(), (400, 100).into())
        );
    }
}
