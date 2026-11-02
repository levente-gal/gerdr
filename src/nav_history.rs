//! Back and forward through the tabs visited, as in a browser: each tab
//! shown (in whatever workspace or project) is a visit; going back shows
//! the one shown before it, going forward undoes that. A tab of the shown
//! project is known by its `Tab::id`, so closing or moving others leaves
//! the history right; a tab that is gone is skipped. The tabs of a project
//! left are parked under ids that outlive its windows (see `Visit`), and
//! become tabs again once it is shown again.

/// How many tabs each way are remembered.
const LIMIT: usize = 50;

/// A tab visited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Visit {
    /// A tab of the shown project, by its `Tab::id`.
    Tab(u64),
    /// A tab of a project shown before: its project, its workspace's
    /// record id, the herdr tab it showed if any, else its place, and what
    /// menus call it.
    Parked {
        project: String,
        workspace: String,
        herdr_tab: Option<String>,
        index: usize,
        label: String,
    },
}

#[derive(Debug)]
pub struct NavHistory<T> {
    /// The tabs shown before, the last one last; each tab once.
    back: Vec<T>,
    /// The tabs gone back from, the next one last.
    forward: Vec<T>,
    /// The tab shown now, if any.
    at: Option<T>,
}

impl<T> Default for NavHistory<T> {
    fn default() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
            at: None,
        }
    }
}

impl<T: Clone + PartialEq> NavHistory<T> {
    /// The shown tab is now `tab`. A tab other than the last one shown is a
    /// visit: the last one goes onto the back list, and the forward list
    /// is dropped, as in a browser. Tabs `keep` says are gone are
    /// forgotten. `None` (nothing shown) changes nothing.
    pub fn visit(&mut self, tab: Option<T>, keep: impl Fn(&T) -> bool) {
        let Some(tab) = tab else { return };
        if self.at.as_ref() == Some(&tab) {
            return;
        }
        if let Some(at) = self.at.take().filter(|at| keep(at)) {
            self.back.retain(|t| *t != at);
            self.back.push(at);
        }
        self.back.retain(|t| *t != tab && keep(t));
        if self.back.len() > LIMIT {
            self.back.drain(..self.back.len() - LIMIT);
        }
        self.forward.clear();
        self.at = Some(tab);
    }

    /// Put `f`'s answer for each tab in its place, `None` dropping it: a
    /// tab parked or brought back. Each tab is then still listed once.
    pub fn rebind(&mut self, mut f: impl FnMut(&T) -> Option<T>) {
        self.at = self.at.as_ref().and_then(&mut f);
        for list in [&mut self.back, &mut self.forward] {
            let mut seen: Vec<T> = Vec::new();
            // From the end: the latest place of a tab listed twice wins.
            for t in std::mem::take(list).iter().rev().filter_map(&mut f) {
                if !seen.contains(&t) && self.at.as_ref() != Some(&t) {
                    seen.push(t);
                }
            }
            seen.reverse();
            *list = seen;
        }
    }

    /// Whether any tab is listed that `f` matches.
    pub fn any(&self, f: impl Fn(&T) -> bool) -> bool {
        self.at.iter().chain(&self.back).chain(&self.forward).any(f)
    }

    /// Go back to `to`, a tab on the back list: those after it go onto
    /// the forward list, as going back one at a time would. `to` if it
    /// was there and `exists`.
    pub fn back_to(&mut self, to: &T, exists: impl Fn(&T) -> bool) -> Option<T> {
        let i = self.back.iter().rposition(|t| t == to)?;
        if !exists(to) {
            return None;
        }
        let passed = self.back.split_off(i + 1);
        let to = self.back.pop()?;
        let behind = self
            .at
            .replace(to.clone())
            .into_iter()
            .chain(passed.into_iter().rev());
        self.forward.extend(behind.filter(|t| exists(t)));
        Some(to)
    }

    /// Go forward to `to`, a tab on the forward list: the mirror of
    /// `back_to`.
    pub fn forward_to(&mut self, to: &T, exists: impl Fn(&T) -> bool) -> Option<T> {
        let i = self.forward.iter().rposition(|t| t == to)?;
        if !exists(to) {
            return None;
        }
        let passed = self.forward.split_off(i + 1);
        let to = self.forward.pop()?;
        let ahead = self
            .at
            .replace(to.clone())
            .into_iter()
            .chain(passed.into_iter().rev());
        for t in ahead.filter(|t| exists(t)) {
            self.back.retain(|b| *b != t);
            self.back.push(t);
        }
        Some(to)
    }

    /// The tabs going back would show, the next first, that still `exists`.
    pub fn backs(&self, exists: impl Fn(&T) -> bool) -> Vec<T> {
        self.back
            .iter()
            .rev()
            .filter(|t| exists(t))
            .cloned()
            .collect()
    }

    /// The tabs going forward would show, the next first, that still
    /// `exists`.
    pub fn forwards(&self, exists: impl Fn(&T) -> bool) -> Vec<T> {
        self.forward
            .iter()
            .rev()
            .filter(|t| exists(t))
            .cloned()
            .collect()
    }

    /// Whether going back would show a tab that still `exists`.
    pub fn can_back(&self, exists: impl Fn(&T) -> bool) -> bool {
        self.back.iter().any(exists)
    }

    /// Whether going forward would show a tab that still `exists`.
    pub fn can_forward(&self, exists: impl Fn(&T) -> bool) -> bool {
        self.forward.iter().any(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::NavHistory;

    fn all(_: &u64) -> bool {
        true
    }

    fn visited(tabs: &[u64]) -> NavHistory<u64> {
        let mut h = NavHistory::default();
        for &t in tabs {
            h.visit(Some(t), all);
        }
        h
    }

    fn back(h: &mut NavHistory<u64>, exists: impl Fn(&u64) -> bool) -> Option<u64> {
        let to = *h.backs(&exists).first()?;
        h.back_to(&to, exists)
    }

    fn forward(h: &mut NavHistory<u64>, exists: impl Fn(&u64) -> bool) -> Option<u64> {
        let to = *h.forwards(&exists).first()?;
        h.forward_to(&to, exists)
    }

    #[test]
    fn back_and_forward_walk_the_visits() {
        let mut h = visited(&[1, 2, 3]);
        assert_eq!(h.backs(all), [2, 1]);
        assert_eq!(back(&mut h, all), Some(2));
        assert_eq!(back(&mut h, all), Some(1));
        assert_eq!(back(&mut h, all), None);
        assert_eq!(forward(&mut h, all), Some(2));
        assert_eq!(forward(&mut h, all), Some(3));
        assert_eq!(forward(&mut h, all), None);
        assert_eq!(h.backs(all), [2, 1]);
    }

    #[test]
    fn showing_the_tab_shown_is_no_visit_and_nothing_shown_changes_nothing() {
        let mut h = visited(&[1, 2, 2]);
        h.visit(None, all);
        assert_eq!(h.backs(all), [1]);
        assert_eq!(back(&mut h, all), Some(1));
    }

    #[test]
    fn a_new_visit_drops_the_forward_list() {
        let mut h = visited(&[1, 2, 3]);
        back(&mut h, all);
        h.visit(Some(4), all);
        assert_eq!(h.forwards(all), Vec::<u64>::new());
        assert_eq!(h.backs(all), [2, 1]);
    }

    #[test]
    fn each_tab_is_on_the_back_list_once_where_it_was_last_left() {
        let h = visited(&[1, 2, 1, 3, 2]);
        assert_eq!(h.backs(all), [3, 1]);
    }

    #[test]
    fn gone_tabs_are_skipped() {
        let mut h = visited(&[1, 2, 3, 4]);
        let open = |t: &u64| *t != 3 && *t != 1;
        assert_eq!(h.backs(open), [2]);
        assert_eq!(back(&mut h, open), Some(2));
        assert_eq!(back(&mut h, open), None);
        assert_eq!(forward(&mut h, open), Some(4));
        // The tab shown being closed doesn't go back on the list.
        let mut h = visited(&[1, 2]);
        h.visit(Some(5), |t| *t != 2);
        assert_eq!(h.backs(all), [1]);
    }

    #[test]
    fn going_to_a_listed_tab_passes_those_between() {
        let mut h = visited(&[1, 2, 3, 4]);
        assert_eq!(h.back_to(&1, all), Some(1));
        assert_eq!(h.forwards(all), [2, 3, 4]);
        assert_eq!(h.forward_to(&3, all), Some(3));
        assert_eq!(h.backs(all), [2, 1]);
        assert_eq!(h.forwards(all), [4]);
        assert_eq!(h.back_to(&9, all), None);
    }

    #[test]
    fn rebinding_renames_drops_and_keeps_each_tab_once() {
        let mut h = visited(&[1, 2, 3, 4]);
        back(&mut h, all);
        // 1, 2 and 3 (shown) under new names; 4 is dropped.
        h.rebind(|t| match t {
            4 => None,
            t => Some(t * 10),
        });
        assert_eq!(h.backs(all), [20, 10]);
        assert_eq!(h.forwards(all), Vec::<u64>::new());
        assert!(h.any(|t| *t == 30));
        // Two names becoming one: listed once, where it was left last.
        let mut h = visited(&[1, 2, 3, 4]);
        h.rebind(|t| Some(if *t == 1 { 3 } else { *t }));
        assert_eq!(h.backs(all), [3, 2]);
        assert!(!h.any(|t| *t == 1));
    }

    #[test]
    fn the_back_list_keeps_the_last_fifty() {
        let h = visited(&(0..80).collect::<Vec<_>>());
        let backs = h.backs(all);
        assert_eq!(backs.len(), super::LIMIT);
        assert_eq!(backs[0], 78);
    }
}
