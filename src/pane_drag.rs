//! A pane dragged by its bar: hold the bar's label with the left button
//! and move. Over another pane of the tab, it lands on the side nearest
//! the pointer (a band along each edge) or, in the middle, swaps with it;
//! over the tab strip, it goes to that tab, or a new one on the "+". The
//! place it would land lights up meanwhile (`ChromeView::pane_drop`). The
//! drop itself goes through `pane_moves`.

use smithay::utils::{Logical, Point};

use crate::{Gerdr, chrome::Rect, layout::Side};

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

impl Gerdr {
    /// Where pane `id` of the shown tab, dragged to `pos`, would land.
    /// `None` over itself or nothing it can land on.
    pub fn pane_drop_at(&self, id: u32, pos: Point<f64, Logical>) -> Option<PaneDrop> {
        // TODO(pane-move): wave 1, drag agent.
        let _ = (id, pos);
        None
    }

    /// The area `pane_drop` lands in, to light up while the pane is held.
    pub fn pane_drop_rect(&self) -> Option<Rect> {
        // TODO(pane-move): wave 1, drag agent.
        None
    }

    /// Drop the held pane `id` where `pane_drop` says, and forget it.
    pub fn drop_pane(&mut self, id: u32) {
        // TODO(pane-move): wave 1, drag agent.
        let _ = id;
        self.pane_drop = None;
    }
}
