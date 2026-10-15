//! Moving the panes of a tab about: to the side of a neighbour or to the
//! tab's edge, swapped with a neighbour, their split turned, or off to
//! another tab. Each move edits the tab's layout (see `layout`), keeps the
//! moved pane focused and tells herdr about it (best effort: herdr's own
//! tree matters only when gerdr has no saved layout to come back to).
//!
//! The keys (`move-left` …), the pane bar's menu and a pane dragged by its
//! bar (see `pane_drag`) all end up here.

use crate::{Gerdr, layout::Side};

impl Gerdr {
    /// Move pane `id` of the shown tab to `side`: beside its neighbour
    /// there (see `layout::neighbor`), or, without one, to the tab's edge.
    pub fn move_pane(&mut self, id: u32, side: Side) {
        // TODO(pane-move): wave 1, moves agent.
        let _ = (id, side);
    }

    /// Swap pane `id` of the shown tab with its neighbour on `side`;
    /// nothing without one.
    pub fn swap_pane(&mut self, id: u32, side: Side) {
        // TODO(pane-move): wave 1, moves agent.
        let _ = (id, side);
    }

    /// Turn the split pane `id` of the shown tab is in: side by side
    /// becomes stacked and back.
    pub fn flip_pane(&mut self, id: u32) {
        // TODO(pane-move): wave 1, moves agent.
        let _ = id;
    }

    /// Put pane `id` of the shown tab on `side` of pane `target` of it.
    pub fn move_pane_beside(&mut self, id: u32, target: u32, side: Side) {
        // TODO(pane-move): wave 1, moves agent.
        let _ = (id, target, side);
    }

    /// Swap panes `a` and `b` of the shown tab.
    pub fn swap_panes(&mut self, a: u32, b: u32) {
        // TODO(pane-move): wave 1, moves agent.
        let _ = (a, b);
    }

    /// Move pane `id` of the shown tab to tab `to` of the active workspace
    /// (split off its focused pane along the longer side), or to a new tab
    /// (`None`), and show that tab with the pane focused. A tab left empty
    /// goes away.
    pub fn move_pane_to_tab(&mut self, id: u32, to: Option<usize>) {
        // TODO(pane-move): wave 1, moves agent.
        let _ = (id, to);
    }
}
