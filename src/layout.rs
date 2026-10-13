//! A tab's panes, laid out the way herdr does it: a binary tree whose leaves
//! are panes and whose inner nodes split their area in two, side by side or
//! stacked. Splitting a pane puts the new one right of or below it; closing
//! one gives its space to its sibling.

use serde::{Deserialize, Serialize};
use smithay::utils::{Logical, Point, Rectangle};

type Rect = Rectangle<i32, Logical>;

/// Where a split puts its second pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Side by side.
    Right,
    /// Stacked.
    Down,
}

/// A direction to look for a neighbouring pane in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, PartialEq)]
pub enum Node {
    Leaf(u32),
    Split {
        dir: Dir,
        /// Share of the room (the area less the divider) given to `first`.
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// A tab's layout as saved with its project, so the tab comes back the way
/// it was: leaves name what they show instead of runtime pane ids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedNode {
    Leaf(SavedLeaf),
    Split {
        dir: Dir,
        ratio: f64,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

/// What a saved leaf shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum SavedLeaf {
    /// A herdr terminal, by its herdr pane id (stable across a server
    /// restart).
    Herdr { pane_id: String },
    /// The workspace's Zed editor.
    Zed,
    /// An app launched from the run prompt, by the command that started it.
    App { command: String },
}

#[allow(dead_code)] // used by project persistence
impl Node {
    /// The layout to save: `leaf` names what each leaf shows; leaves it has
    /// no name for are left out (their sibling takes the space). `None` if
    /// none is named.
    pub fn save(&self, leaf: &impl Fn(u32) -> Option<SavedLeaf>) -> Option<SavedNode> {
        match self {
            Node::Leaf(id) => leaf(*id).map(SavedNode::Leaf),
            Node::Split {
                dir,
                ratio,
                first,
                second,
            } => match (first.save(leaf), second.save(leaf)) {
                (Some(first), Some(second)) => Some(SavedNode::Split {
                    dir: *dir,
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (first, second) => first.or(second),
            },
        }
    }
}

#[allow(dead_code)] // used by project persistence
impl SavedNode {
    /// The layout to show again: `leaf` hands out the pane id for each saved
    /// leaf, or `None` for one that can't come back (its sibling takes the
    /// space). `None` if no leaf comes back. Leaves are handed to `leaf` in
    /// order.
    pub fn restore(&self, leaf: &mut impl FnMut(&SavedLeaf) -> Option<u32>) -> Option<Node> {
        match self {
            SavedNode::Leaf(saved) => leaf(saved).map(Node::Leaf),
            SavedNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let first = first.restore(leaf);
                let second = second.restore(leaf);
                match (first, second) {
                    (Some(first), Some(second)) => Some(Node::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    }),
                    (first, second) => first.or(second),
                }
            }
        }
    }

    /// The herdr pane ids among its leaves, in order.
    pub fn herdr_panes(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_herdr_panes(&mut out);
        out
    }

    fn collect_herdr_panes<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            SavedNode::Leaf(SavedLeaf::Herdr { pane_id }) => out.push(pane_id),
            SavedNode::Leaf(_) => {}
            SavedNode::Split { first, second, .. } => {
                first.collect_herdr_panes(out);
                second.collect_herdr_panes(out);
            }
        }
    }
}

/// The gap between a split's two children.
pub struct Divider {
    /// The way down to the split: `false` for the first child, `true` for the second.
    pub path: Vec<bool>,
    pub dir: Dir,
    pub gap: Rect,
    /// The area the split divides.
    pub area: Rect,
}

impl Divider {
    /// Whether `pos` is within `grab` of the gap.
    pub fn near(&self, pos: Point<f64, Logical>, grab: f64) -> bool {
        let g = self.gap.to_f64();
        let (along, across, start, len, mid) = match self.dir {
            Dir::Right => (pos.y, pos.x, g.loc.y, g.size.h, g.loc.x + g.size.w / 2.0),
            Dir::Down => (pos.x, pos.y, g.loc.x, g.size.w, g.loc.y + g.size.h / 2.0),
        };
        (start..start + len).contains(&along) && (across - mid).abs() <= grab
    }
}

impl Node {
    /// Replace the leaf `target` with a split: `target` keeps `ratio` of its
    /// space, `new` gets the rest. Returns whether `target` was found.
    pub fn split(&mut self, target: u32, new: u32, dir: Dir, ratio: f64) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                let (first, second) = (Box::new(Node::Leaf(target)), Box::new(Node::Leaf(new)));
                *self = Node::Split {
                    dir,
                    ratio,
                    first,
                    second,
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split(target, new, dir, ratio) || second.split(target, new, dir, ratio)
            }
        }
    }

    /// The layout that puts the panes where `rects` has them (as herdr reports
    /// a tab): split wherever a straight cut runs between them.
    pub fn from_rects(rects: &[(u32, Rect)]) -> Option<Node> {
        if let [(id, _)] = rects {
            return Some(Node::Leaf(*id));
        }
        let left = rects.iter().map(|(_, r)| r.loc.x).min()?;
        let top = rects.iter().map(|(_, r)| r.loc.y).min()?;
        let right = rects.iter().map(|(_, r)| r.loc.x + r.size.w).max()?;
        let bottom = rects.iter().map(|(_, r)| r.loc.y + r.size.h).max()?;
        for dir in [Dir::Right, Dir::Down] {
            let (start, end) = match dir {
                Dir::Right => (left, right),
                Dir::Down => (top, bottom),
            };
            // A pane's start and end across the cut.
            let span = |r: &Rect| match dir {
                Dir::Right => (r.loc.x, r.loc.x + r.size.w),
                Dir::Down => (r.loc.y, r.loc.y + r.size.h),
            };
            // Try each pane's far edge as the cut, nearest first.
            let mut cuts: Vec<i32> = rects
                .iter()
                .map(|(_, r)| span(r).1)
                .filter(|&c| c < end)
                .collect();
            cuts.sort_unstable();
            for cut in cuts {
                let (first, second): (Vec<_>, Vec<_>) =
                    rects.iter().copied().partition(|(_, r)| span(r).1 <= cut);
                if !first.is_empty() && second.iter().all(|(_, r)| span(r).0 >= cut) {
                    let ratio = (cut - start) as f64 / (end - start).max(1) as f64;
                    return Some(Node::Split {
                        dir,
                        ratio,
                        first: Box::new(Node::from_rects(&first)?),
                        second: Box::new(Node::from_rects(&second)?),
                    });
                }
            }
        }
        // No clean cut: just put them side by side.
        let (first, second) = rects.split_at(rects.len() / 2);
        let first = Box::new(Node::from_rects(first)?);
        Some(Node::Split {
            dir: Dir::Right,
            ratio: 0.5,
            first,
            second: Box::new(Node::from_rects(second)?),
        })
    }

    /// The panes in order: left to right, top to bottom.
    pub fn leaves(&self) -> Vec<u32> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<u32>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// Where each pane goes in `area`, with `divider` pixels between split panes.
    pub fn rects(&self, area: Rect, divider: i32) -> Vec<(u32, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(area, divider, &mut out);
        out
    }

    fn collect_rects(&self, area: Rect, divider: i32, out: &mut Vec<(u32, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((
                *id,
                Rectangle::new(area.loc, (area.size.w.max(1), area.size.h.max(1)).into()),
            )),
            Node::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (a, b, _) = halves(*dir, *ratio, area, divider);
                first.collect_rects(a, divider, out);
                second.collect_rects(b, divider, out);
            }
        }
    }

    /// Every split's divider in `area`.
    pub fn dividers(&self, area: Rect, divider: i32) -> Vec<Divider> {
        let mut out = Vec::new();
        self.collect_dividers(area, divider, &mut Vec::new(), &mut out);
        out
    }

    fn collect_dividers(
        &self,
        area: Rect,
        divider: i32,
        path: &mut Vec<bool>,
        out: &mut Vec<Divider>,
    ) {
        let Node::Split {
            dir,
            ratio,
            first,
            second,
        } = self
        else {
            return;
        };
        let (a, b, gap) = halves(*dir, *ratio, area, divider);
        out.push(Divider {
            path: path.clone(),
            dir: *dir,
            gap,
            area,
        });
        for (child, rect, side) in [(first, a, false), (second, b, true)] {
            path.push(side);
            child.collect_dividers(rect, divider, path, out);
            path.pop();
        }
    }

    /// Set the ratio of the split at `path`, if there is one.
    pub fn set_ratio(&mut self, path: &[bool], new_ratio: f64) {
        let Node::Split {
            ratio,
            first,
            second,
            ..
        } = self
        else {
            return;
        };
        match path.split_first() {
            None => *ratio = new_ratio,
            Some((false, rest)) => first.set_ratio(rest, new_ratio),
            Some((true, rest)) => second.set_ratio(rest, new_ratio),
        }
    }

    /// Take the leaf `id` out from under this split; its parent gives way to
    /// the sibling. Returns the sibling's pane nearest to where `id` was.
    fn remove_child(&mut self, id: u32) -> Option<u32> {
        let Node::Split { first, second, .. } = self else {
            return None;
        };
        let (keep, removed_first) = if **first == Node::Leaf(id) {
            (second, true)
        } else if **second == Node::Leaf(id) {
            (first, false)
        } else {
            return first.remove_child(id).or_else(|| second.remove_child(id));
        };
        *self = std::mem::replace(&mut **keep, Node::Leaf(id));
        let leaves = self.leaves();
        if removed_first {
            leaves.first()
        } else {
            leaves.last()
        }
        .copied()
    }
}

/// Take the pane `id` out of the layout. Returns the pane that took over its
/// space (the nearest one of its sibling), if any is left.
pub fn remove(root: &mut Option<Node>, id: u32) -> Option<u32> {
    match root.take()? {
        Node::Leaf(leaf) if leaf == id => None,
        mut node => {
            let next = node.remove_child(id);
            *root = Some(node);
            next
        }
    }
}

/// The pane next to `from` on `side`: the closest one sharing the most edge with it.
pub fn neighbor(rects: &[(u32, Rect)], from: u32, side: Side) -> Option<u32> {
    let (_, a) = *rects.iter().find(|(id, _)| *id == from)?;
    let overlap =
        |a0: i32, a_len: i32, b0: i32, b_len: i32| (a0 + a_len).min(b0 + b_len) - a0.max(b0);
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|&(id, b)| {
            let (gap, shared) = match side {
                Side::Left => (
                    a.loc.x - (b.loc.x + b.size.w),
                    overlap(a.loc.y, a.size.h, b.loc.y, b.size.h),
                ),
                Side::Right => (
                    b.loc.x - (a.loc.x + a.size.w),
                    overlap(a.loc.y, a.size.h, b.loc.y, b.size.h),
                ),
                Side::Up => (
                    a.loc.y - (b.loc.y + b.size.h),
                    overlap(a.loc.x, a.size.w, b.loc.x, b.size.w),
                ),
                Side::Down => (
                    b.loc.y - (a.loc.y + a.size.h),
                    overlap(a.loc.x, a.size.w, b.loc.x, b.size.w),
                ),
            };
            (gap >= 0 && shared > 0).then_some((gap, -shared, id))
        })
        .min()
        .map(|(_, _, id)| id)
}

/// The areas a split gives its first and second child, and the gap between them.
fn halves(dir: Dir, ratio: f64, area: Rect, divider: i32) -> (Rect, Rect, Rect) {
    let (x, y, w, h) = (area.loc.x, area.loc.y, area.size.w, area.size.h);
    match dir {
        Dir::Right => {
            let room = (w - divider).max(0);
            let a = (room as f64 * ratio).round() as i32;
            (
                Rectangle::new((x, y).into(), (a, h).into()),
                Rectangle::new((x + a + divider, y).into(), (room - a, h).into()),
                Rectangle::new((x + a, y).into(), (divider, h).into()),
            )
        }
        Dir::Down => {
            let room = (h - divider).max(0);
            let a = (room as f64 * ratio).round() as i32;
            (
                Rectangle::new((x, y).into(), (w, a).into()),
                Rectangle::new((x, y + a + divider).into(), (w, room - a).into()),
                Rectangle::new((x, y + a).into(), (w, divider).into()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rectangle::new((0, 0).into(), (102, 52).into())
    }

    /// 1 | (2 over 3)
    fn three() -> Node {
        let mut root = Node::Leaf(1);
        assert!(root.split(1, 2, Dir::Right, 0.5));
        assert!(root.split(2, 3, Dir::Down, 0.5));
        root
    }

    #[test]
    fn split_and_rects() {
        let root = three();
        assert_eq!(root.leaves(), [1, 2, 3]);
        let rects = root.rects(area(), 2);
        assert_eq!(rects[0].1, Rectangle::new((0, 0).into(), (50, 52).into()));
        assert_eq!(rects[1].1, Rectangle::new((52, 0).into(), (50, 25).into()));
        assert_eq!(rects[2].1, Rectangle::new((52, 27).into(), (50, 25).into()));
        assert!(!Node::Leaf(1).split(9, 10, Dir::Right, 0.5));
    }

    #[test]
    fn remove_collapses_to_sibling() {
        let mut root = Some(three());
        assert_eq!(remove(&mut root, 2), Some(3));
        assert_eq!(root.as_ref().unwrap().leaves(), [1, 3]);
        assert_eq!(remove(&mut root, 1), Some(3));
        assert_eq!(root, Some(Node::Leaf(3)));
        assert_eq!(remove(&mut root, 3), None);
        assert_eq!(root, None);
    }

    #[test]
    fn remove_second_gives_nearest() {
        // (1 | 2) over 3: closing 3 hands its space to the last pane above it.
        let mut root = Node::Leaf(1);
        root.split(1, 3, Dir::Down, 0.5);
        root.split(1, 2, Dir::Right, 0.5);
        let mut root = Some(root);
        assert_eq!(remove(&mut root, 3), Some(2));
        assert_eq!(remove(&mut root, 9), None);
        assert_eq!(root.unwrap().leaves(), [1, 2]);
    }

    #[test]
    fn dividers_and_ratio() {
        let mut root = three();
        let dividers = root.dividers(area(), 2);
        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[1].path, [true]);
        assert!(dividers[0].near((51.0, 10.0).into(), 4.0));
        assert!(!dividers[0].near((40.0, 10.0).into(), 4.0));
        root.set_ratio(&[true], 0.2);
        assert_eq!(root.rects(area(), 2)[1].1.size.h, 10);
    }

    #[test]
    fn from_rects_rebuilds_tree() {
        // herdr's cells: 1 | (2 over 3), as in `three()`.
        let r = |x, y, w, h| Rectangle::new((x, y).into(), (w, h).into());
        let rects = [
            (1, r(0, 0, 40, 30)),
            (2, r(40, 0, 60, 10)),
            (3, r(40, 10, 60, 20)),
        ];
        let root = Node::from_rects(&rects).unwrap();
        assert_eq!(root.leaves(), [1, 2, 3]);
        let Node::Split {
            dir: Dir::Right,
            ratio,
            second,
            ..
        } = &root
        else {
            panic!("{root:?}")
        };
        assert!((ratio - 0.4).abs() < 1e-9);
        assert!(matches!(**second, Node::Split { dir: Dir::Down, .. }));
        assert_eq!(Node::from_rects(&[(7, r(0, 0, 5, 5))]), Some(Node::Leaf(7)));
        assert_eq!(Node::from_rects(&[]), None);
    }

    /// What `three()`'s panes show: a herdr terminal, Zed, an app.
    fn named(id: u32) -> Option<SavedLeaf> {
        match id {
            1 => Some(SavedLeaf::Herdr {
                pane_id: "w1:p1".into(),
            }),
            2 => Some(SavedLeaf::Zed),
            3 => Some(SavedLeaf::App {
                command: "firefox".into(),
            }),
            _ => None,
        }
    }

    #[test]
    fn save_and_restore_round_trip() {
        let mut root = three();
        root.set_ratio(&[], 0.3);
        root.set_ratio(&[true], 0.7);
        let saved = root.save(&named).unwrap();
        let SavedNode::Split {
            dir: Dir::Right,
            ratio,
            second,
            ..
        } = &saved
        else {
            panic!("{saved:?}")
        };
        assert_eq!(*ratio, 0.3);
        assert!(matches!(
            **second,
            SavedNode::Split {
                dir: Dir::Down,
                ratio: 0.7,
                ..
            }
        ));
        // New ids for the leaves, in order.
        let mut next = 10;
        let mut seen = Vec::new();
        let restored = saved
            .restore(&mut |leaf| {
                seen.push(leaf.clone());
                next += 1;
                Some(next)
            })
            .unwrap();
        assert_eq!(seen, [1, 2, 3].map(|id| named(id).unwrap()));
        assert_eq!(restored.leaves(), [11, 12, 13]);
        assert_eq!(restored.save(&|id| named(id - 10)), Some(saved));
    }

    #[test]
    fn save_leaves_out_unnamed() {
        // 1 | (2 over 3) without 2: 1 | 3, keeping the outer split.
        let saved = three().save(&|id| named(id).filter(|_| id != 2)).unwrap();
        let SavedNode::Split {
            dir: Dir::Right,
            first,
            second,
            ..
        } = &saved
        else {
            panic!("{saved:?}")
        };
        assert_eq!(**first, SavedNode::Leaf(named(1).unwrap()));
        assert_eq!(**second, SavedNode::Leaf(named(3).unwrap()));
        // Only 3 named: just its leaf.
        assert_eq!(
            three().save(&|id| named(id).filter(|_| id == 3)),
            Some(SavedNode::Leaf(named(3).unwrap()))
        );
        assert_eq!(three().save(&|_| None), None);
    }

    #[test]
    fn restore_leaves_out_missing() {
        let saved = three().save(&named).unwrap();
        // The app can't come back: 1 | 2.
        let restored = saved.restore(&mut |leaf| match leaf {
            SavedLeaf::Herdr { .. } => Some(1),
            SavedLeaf::Zed => Some(2),
            SavedLeaf::App { .. } => None,
        });
        assert_eq!(
            restored,
            Some(Node::Split {
                dir: Dir::Right,
                ratio: 0.5,
                first: Box::new(Node::Leaf(1)),
                second: Box::new(Node::Leaf(2)),
            })
        );
        // Only the herdr pane comes back.
        assert_eq!(
            saved.restore(&mut |leaf| matches!(leaf, SavedLeaf::Herdr { .. }).then_some(1)),
            Some(Node::Leaf(1))
        );
        assert_eq!(saved.restore(&mut |_| None), None);
    }

    #[test]
    fn herdr_panes_in_order() {
        // (h2 | Zed) over (h1 over h3)
        let herdr = |id: &str| SavedNode::Leaf(SavedLeaf::Herdr { pane_id: id.into() });
        let split = |dir, first, second| SavedNode::Split {
            dir,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        };
        let saved = split(
            Dir::Down,
            split(Dir::Right, herdr("w1:p2"), SavedNode::Leaf(SavedLeaf::Zed)),
            split(Dir::Down, herdr("w1:p1"), herdr("w1:p3")),
        );
        assert_eq!(saved.herdr_panes(), ["w1:p2", "w1:p1", "w1:p3"]);
        assert!(SavedNode::Leaf(SavedLeaf::Zed).herdr_panes().is_empty());
    }

    #[test]
    fn saved_json_shape() {
        let mut root = three();
        root.set_ratio(&[], 0.3);
        let saved = root.save(&named).unwrap();
        let json = serde_json::to_string(&saved).unwrap();
        // Saved projects are read back by later versions: keep this shape.
        assert_eq!(
            json,
            concat!(
                r#"{"split":{"dir":"right","ratio":0.3,"#,
                r#""first":{"leaf":{"kind":"herdr","pane_id":"w1:p1"}},"#,
                r#""second":{"split":{"dir":"down","ratio":0.5,"#,
                r#""first":{"leaf":{"kind":"zed"}},"#,
                r#""second":{"leaf":{"kind":"app","command":"firefox"}}}}}}"#,
            )
        );
        assert_eq!(serde_json::from_str::<SavedNode>(&json).unwrap(), saved);
    }

    #[test]
    fn neighbors() {
        let rects = three().rects(area(), 2);
        assert_eq!(neighbor(&rects, 1, Side::Right), Some(2));
        assert_eq!(neighbor(&rects, 3, Side::Left), Some(1));
        assert_eq!(neighbor(&rects, 2, Side::Down), Some(3));
        assert_eq!(neighbor(&rects, 3, Side::Up), Some(2));
        assert_eq!(neighbor(&rects, 1, Side::Left), None);
    }
}
