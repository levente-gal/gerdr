//! Moving the panes of a tab about: to the side of a neighbour or to the
//! tab's edge, swapped with a neighbour, their split turned, zoomed to fill
//! the tab, or off to another tab. Each move edits the tab's layout (see `layout`) and keeps
//! the moved pane focused. herdr hears of swaps (best effort) and of panes
//! moved to another tab (it must: gerdr's tabs are matched to herdr's by
//! their panes on restore). It refuses to move a pane within its tab, so
//! the other moves stay gerdr's: its saved layout is what restore uses.
//!
//! The keys (`move-left` …), the pane bar's menu and a pane dragged by its
//! bar (see `pane_drag`) all end up here.

use smithay::utils::{Logical, Rectangle};

use crate::{
    DIVIDER, Expected, Gerdr, Pane, SPLIT_RATIO, Tab, herdr,
    layout::{self, Dir, Node, Side},
};

impl Gerdr {
    /// Move pane `id` of the shown tab to `side`: beside its neighbour
    /// there (see `layout::neighbor`), or, without one, to the tab's edge.
    pub fn move_pane(&mut self, id: u32, side: Side) {
        let area = self.content_rect();
        let Some(tab) = self.tab().filter(|t| has_leaf(t, id)) else {
            return;
        };
        if let Some(target) = neighbor_of(tab, id, side, area) {
            return self.move_pane_beside(id, target, side);
        }
        let moved = self
            .shown_tab_mut()
            .and_then(|t| t.layout.as_mut())
            .is_some_and(|l| l.move_to_edge(id, side, SPLIT_RATIO));
        if moved {
            self.settle(id);
        }
    }

    /// Swap pane `id` of the shown tab with its neighbour on `side`;
    /// nothing without one.
    pub fn swap_pane(&mut self, id: u32, side: Side) {
        let area = self.content_rect();
        let Some(tab) = self.tab().filter(|t| has_leaf(t, id)) else {
            return;
        };
        if let Some(other) = neighbor_of(tab, id, side, area) {
            self.swap_panes(id, other);
        }
    }

    /// Turn the split pane `id` of the shown tab is in: side by side
    /// becomes stacked and back.
    pub fn flip_pane(&mut self, id: u32) {
        let flipped = self
            .shown_tab_mut()
            .and_then(|t| t.layout.as_mut())
            .is_some_and(|l| l.flip(id));
        if flipped {
            self.arrange();
        }
    }

    /// Zoom pane `id` of the shown tab to fill it, focused, as herdr's
    /// prefix+z does; or, zoomed already, give the others their places
    /// back. Any change to the layout ends the zoom too, and so does
    /// focusing another pane of the tab (see `Gerdr::focus`).
    pub fn zoom_pane(&mut self, id: u32) {
        let Some(tab) = self.shown_tab_mut().filter(|t| has_leaf(t, id)) else {
            return;
        };
        tab.toggle_zoom(id);
        self.arrange();
        self.focus_pane(|_| Some(id));
    }

    /// Put pane `id` of the shown tab on `side` of pane `target` of it.
    pub fn move_pane_beside(&mut self, id: u32, target: u32, side: Side) {
        let moved = self
            .shown_tab_mut()
            .and_then(|t| t.layout.as_mut())
            .is_some_and(|l| l.move_beside(id, target, side, SPLIT_RATIO));
        if moved {
            self.settle(id);
        }
    }

    /// Swap panes `a` and `b` of the shown tab.
    pub fn swap_panes(&mut self, a: u32, b: u32) {
        let swapped = self
            .shown_tab_mut()
            .and_then(|t| t.layout.as_mut())
            .is_some_and(|l| l.swap(a, b));
        if !swapped {
            return;
        }
        self.settle(a);
        let Some(tab) = self.tab() else { return };
        let Some((a, b)) = herdr_of(tab, a).zip(herdr_of(tab, b)) else {
            return;
        };
        let (a, b) = (a.pane_id.clone(), b.pane_id.clone());
        if let Some(herdr) = self.herdr.as_mut()
            && let Err(err) = herdr.pane_swap(&a, &b)
        {
            tracing::warn!("herdr: {err}");
        }
    }

    /// Move pane `id` of the shown tab to tab `to` of the active workspace
    /// (split off its focused pane along the longer side), or to a new tab
    /// (`None`), and show that tab with the pane focused. A tab left empty
    /// goes away.
    pub fn move_pane_to_tab(&mut self, id: u32, to: Option<usize>) {
        let area = self.content_rect();
        let w = self.active_workspace;
        let Some(ws) = self.workspaces.get_mut(w) else {
            return;
        };
        let from = ws.active_tab;
        let Some(source) = ws.tabs.get(from).filter(|t| has_leaf(t, id)) else {
            return;
        };
        if to.is_some_and(|t| t == from || t >= ws.tabs.len()) {
            return;
        }
        // Alone in its tab, it has a tab of its own already.
        if to.is_none() && source.layout == Some(Node::Leaf(id)) {
            return;
        }
        let Some(mut moved) = take_pane(&mut ws.tabs[from], id) else {
            return;
        };

        // Where herdr puts it: the herdr tab the destination mirrors, next
        // to the pane gerdr split for it (else the one a new terminal would
        // go next to), split the same way. herdr only puts it right of or
        // below its target, so one put first by gerdr comes second there.
        let mut herdr_dest = None;
        if let Some(t) = to {
            let dest = &mut ws.tabs[t];
            // Read before the pane joins: its own (old) herdr tab would do.
            let herdr_tab = dest.herdr_tab_id().map(str::to_owned);
            let fallback = dest.herdr_split_target().map(|p| p.pane_id.clone());
            dest.split(None, id, None, area);
            let (dir, partner) = dest
                .layout
                .as_ref()
                .and_then(|l| split_partner(l, id))
                .unwrap_or((Dir::Right, None));
            let target = partner
                .and_then(|p| herdr_of(dest, p))
                .map(|p| p.pane_id.clone())
                .or(fallback);
            herdr_dest = herdr_tab.map(|tab_id| (tab_id, target, dir));
        }
        let pane = moved
            .herdr()
            .map(|p| (p.pane_id.clone(), p.workspace_id.clone()));
        if let Some((pane_id, workspace_id)) = pane
            && let Some(herdr) = self.herdr.as_mut()
        {
            // herdr must follow: gerdr's tabs are matched to herdr's by
            // their panes' tab when they come back.
            let result = match &herdr_dest {
                Some((tab_id, target, dir)) => {
                    herdr.pane_move(&pane_id, tab_id, target.as_deref(), *dir, SPLIT_RATIO)
                }
                None => herdr.pane_move_to_new_tab(&pane_id, &workspace_id),
            };
            match result {
                Ok(info) => moved.set_herdr(info),
                Err(err) => tracing::warn!("herdr: {err}"),
            }
        }

        let ws = &mut self.workspaces[w];
        ws.active_tab = match to {
            Some(t) => {
                put_pane(&mut ws.tabs[t], moved);
                t
            }
            None => {
                let mut tab = Tab {
                    layout: Some(Node::Leaf(id)),
                    ..Default::default()
                };
                put_pane(&mut tab, moved);
                ws.tabs.push(tab);
                ws.tabs.len() - 1
            }
        };
        self.show_active();
        // The source tab, if left empty, goes away; the shown one stays.
        self.prune();
    }

    /// Move pane `id` of the shown tab to a new tab of workspace `to`,
    /// another one, and show it there, focused. herdr moves a terminal
    /// first, to the herdr workspace `to` mirrors; one `to` has none for
    /// (no terminal in it), or that herdr refuses, stays where it is. A
    /// tab, or workspace, left empty goes away.
    pub fn move_pane_to_workspace(&mut self, id: u32, to: usize) {
        let from_w = self.active_workspace;
        if to == from_w || to >= self.workspaces.len() {
            return;
        }
        let dest_herdr = self.workspaces[to].herdr_id().map(str::to_owned);
        let ws = &self.workspaces[from_w];
        let from = ws.active_tab;
        let Some(source) = ws.tabs.get(from).filter(|t| has_leaf(t, id)) else {
            return;
        };
        let name = self.workspaces[to].name.clone();
        let mut info = None;
        if let Some(pane) = herdr_of(source, id).map(|p| p.pane_id.clone()) {
            let Some(dest) = dest_herdr else {
                self.show_warning(format!("{name} has no terminal to put a terminal pane by"));
                return;
            };
            let Some(herdr) = self.herdr.as_mut() else {
                return;
            };
            match herdr.pane_move_to_new_tab(&pane, &dest) {
                Ok(moved) => info = Some(moved),
                Err(err) => {
                    tracing::warn!("herdr: {err}");
                    self.show_error(format!("Couldn't move the pane to {name}"));
                    return;
                }
            }
        }
        let Some(mut moved) = take_pane(&mut self.workspaces[from_w].tabs[from], id) else {
            return;
        };
        if let Some(info) = info {
            moved.set_herdr(info);
        }
        let mut tab = Tab {
            layout: Some(Node::Leaf(id)),
            ..Default::default()
        };
        put_pane(&mut tab, moved);
        let dest = &mut self.workspaces[to];
        dest.tabs.push(tab);
        dest.active_tab = dest.tabs.len() - 1;
        self.switch_workspace(to);
        self.prune();
        self.focus_pane(|_| Some(id));
        self.t3_pane_moved(id);
    }

    /// The shown tab, to change.
    fn shown_tab_mut(&mut self) -> Option<&mut Tab> {
        let ws = self.workspaces.get_mut(self.active_workspace)?;
        ws.tabs.get_mut(ws.active_tab)
    }

    /// Lay the shown tab out anew after pane `id` moved, keeping the focus
    /// on it (if it has a window yet).
    fn settle(&mut self, id: u32) {
        self.arrange();
        self.focus_pane(|_| Some(id));
    }
}

/// A pane taken out of its tab, with its window or what is expected there.
enum Moved {
    Shown(Pane),
    Expected(Expected),
}

impl Moved {
    /// The herdr pane it shows, for a terminal.
    fn herdr(&self) -> Option<&herdr::PaneInfo> {
        match self {
            Moved::Shown(p) => p.herdr.as_ref(),
            Moved::Expected(e) => e.herdr.as_ref(),
        }
    }

    fn set_herdr(&mut self, info: herdr::PaneInfo) {
        match self {
            Moved::Shown(p) => p.herdr = Some(info),
            Moved::Expected(e) => e.herdr = Some(info),
        }
    }
}

/// Whether pane `id` is in `tab`'s layout.
fn has_leaf(tab: &Tab, id: u32) -> bool {
    tab.layout
        .as_ref()
        .is_some_and(|l| l.leaves().contains(&id))
}

/// The herdr pane pane `id` of `tab` shows, with its window or still
/// waiting for it.
fn herdr_of(tab: &Tab, id: u32) -> Option<&herdr::PaneInfo> {
    tab.herdr_pane(id).or_else(|| {
        tab.expected
            .iter()
            .find(|e| e.pane == id)
            .and_then(|e| e.herdr.as_ref())
    })
}

/// The pane with a window next to pane `id` of `tab` on `side`. A pane
/// still waiting for its window looks from the place kept for it.
fn neighbor_of(tab: &Tab, id: u32, side: Side, area: Rectangle<i32, Logical>) -> Option<u32> {
    let mut rects = tab.shown_rects(area);
    if !rects.iter().any(|(p, _)| *p == id) {
        let own = tab.layout.as_ref()?.rects(area, DIVIDER);
        rects.extend(own.into_iter().filter(|(p, _)| *p == id));
    }
    layout::neighbor(&rects, id, side)
}

/// The split leaf `id` is a direct child of: its direction, and the other
/// child if that is a single pane.
fn split_partner(node: &Node, id: u32) -> Option<(Dir, Option<u32>)> {
    let Node::Split {
        dir, first, second, ..
    } = node
    else {
        return None;
    };
    let other = match (&**first, &**second) {
        (Node::Leaf(a), other) | (other, Node::Leaf(a)) if *a == id => other,
        _ => return split_partner(first, id).or_else(|| split_partner(second, id)),
    };
    let other = match other {
        Node::Leaf(o) => Some(*o),
        Node::Split { .. } => None,
    };
    Some((*dir, other))
}

/// Take pane `id` out of `tab`: its window (or what is expected there) and
/// its leaf. The focus goes where it would if the pane closed: to the pane
/// that took its space.
fn take_pane(tab: &mut Tab, id: u32) -> Option<Moved> {
    let moved = match tab.panes.iter().position(|p| p.id == id) {
        Some(i) => Moved::Shown(tab.panes.remove(i)),
        None => {
            let i = tab.expected.iter().position(|e| e.pane == id)?;
            Moved::Expected(tab.expected.remove(i))
        }
    };
    let next = layout::remove(&mut tab.layout, id);
    if let Moved::Shown(pane) = &moved
        && tab.focused.as_ref() == Some(&pane.window)
    {
        let next = next
            .and_then(|n| tab.window(n))
            .or(tab.panes.first().map(|p| &p.window));
        tab.focused = next.cloned();
    }
    Some(moved)
}

/// Give `tab` the pane taken out of another; its leaf is already in
/// `tab`'s layout. A pane with a window takes the tab's focus.
fn put_pane(tab: &mut Tab, moved: Moved) {
    match moved {
        Moved::Shown(pane) => {
            tab.focused = Some(pane.window.clone());
            tab.panes.push(pane);
        }
        Moved::Expected(e) => tab.expected.push(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u32) -> Box<Node> {
        Box::new(Node::Leaf(id))
    }

    fn split(dir: Dir, first: Box<Node>, second: Box<Node>) -> Box<Node> {
        Box::new(Node::Split {
            dir,
            ratio: SPLIT_RATIO,
            first,
            second,
        })
    }

    #[test]
    fn split_partner_finds_direct_parent() {
        // 1 | (2 / (3 | 4))
        let tree = split(
            Dir::Right,
            leaf(1),
            split(Dir::Down, leaf(2), split(Dir::Right, leaf(3), leaf(4))),
        );
        assert_eq!(split_partner(&tree, 4), Some((Dir::Right, Some(3))));
        assert_eq!(split_partner(&tree, 3), Some((Dir::Right, Some(4))));
        // Its sibling is a split, not one pane.
        assert_eq!(split_partner(&tree, 2), Some((Dir::Down, None)));
        assert_eq!(split_partner(&tree, 1), Some((Dir::Right, None)));
        assert_eq!(split_partner(&tree, 9), None);
        assert_eq!(split_partner(&Node::Leaf(1), 1), None);
    }

    #[test]
    fn expected_pane_moves_between_tabs() {
        let mut source = Tab {
            layout: Some(*split(Dir::Right, leaf(1), leaf(2))),
            expected: vec![
                Expected::new(None, "zed", 1, false),
                Expected::new(None, "zed", 2, true),
            ],
            ..Default::default()
        };
        let mut dest = Tab {
            layout: Some(Node::Leaf(3)),
            expected: vec![Expected::new(None, "zed", 3, false)],
            ..Default::default()
        };
        let moved = take_pane(&mut source, 2).expect("pane 2 is there");
        assert!(matches!(&moved, Moved::Expected(e) if e.pane == 2));
        assert_eq!(source.layout, Some(Node::Leaf(1)));
        assert_eq!(source.expected.len(), 1);

        let area = Rectangle::new((0, 0).into(), (800, 400).into());
        dest.split(None, 2, None, area);
        put_pane(&mut dest, moved);
        assert_eq!(dest.layout, Some(*split(Dir::Right, leaf(3), leaf(2))));
        assert_eq!(
            dest.expected.iter().map(|e| e.pane).collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(
            split_partner(dest.layout.as_ref().unwrap(), 2),
            Some((Dir::Right, Some(3)))
        );
    }

    #[test]
    fn take_pane_leaves_unknown_alone() {
        // A leaf with neither a window nor anything expected stays put.
        let mut tab = Tab {
            layout: Some(*split(Dir::Down, leaf(1), leaf(2))),
            expected: vec![Expected::new(None, "zed", 1, false)],
            ..Default::default()
        };
        assert!(take_pane(&mut tab, 2).is_none());
        assert_eq!(tab.layout, Some(*split(Dir::Down, leaf(1), leaf(2))));
    }
}
