//! Back and forward through the tabs visited, as in a browser: each tab
//! shown (in whatever workspace) is a visit; going back shows the one
//! shown before it, going forward undoes that. Tabs are known by their
//! `Tab::id`, so closing or moving others leaves the history right; a tab
//! that is gone is skipped. The history is the shown project's: switching
//! projects starts it over.

/// How many tabs each way are remembered.
const LIMIT: usize = 50;

#[derive(Debug, Default)]
pub struct NavHistory {
    /// The tabs shown before, the last one last; each tab once.
    back: Vec<u64>,
    /// The tabs gone back from, the next one last.
    forward: Vec<u64>,
    /// The tab shown now, if any.
    at: Option<u64>,
}

impl NavHistory {
    /// The shown tab is now `tab`. A tab other than the last one shown is a
    /// visit: the last one goes onto the back list, and the forward list
    /// is dropped, as in a browser. Tabs `exists` says are gone are
    /// forgotten. `None` (nothing shown) changes nothing.
    pub fn visit(&mut self, tab: Option<u64>, exists: impl Fn(u64) -> bool) {
        let Some(tab) = tab else { return };
        if self.at == Some(tab) {
            return;
        }
        if let Some(at) = self.at.take().filter(|&at| exists(at)) {
            self.back.retain(|&t| t != at);
            self.back.push(at);
        }
        self.back.retain(|&t| t != tab && exists(t));
        if self.back.len() > LIMIT {
            self.back.drain(..self.back.len() - LIMIT);
        }
        self.forward.clear();
        self.at = Some(tab);
    }

    /// Go back to the last tab shown that still `exists`: the tab to show,
    /// if any. The tab shown now goes onto the forward list.
    pub fn back(&mut self, exists: impl Fn(u64) -> bool) -> Option<u64> {
        let to = *self.back.iter().rev().find(|&&t| exists(t))?;
        self.back_to(to, exists)
    }

    /// Go forward to the next tab gone back from that still `exists`.
    pub fn forward(&mut self, exists: impl Fn(u64) -> bool) -> Option<u64> {
        let to = *self.forward.iter().rev().find(|&&t| exists(t))?;
        self.forward_to(to, exists)
    }

    /// Go back to `to`, a tab on the back list: those after it go onto
    /// the forward list, as going back one at a time would. `to` if it
    /// was there and `exists`.
    pub fn back_to(&mut self, to: u64, exists: impl Fn(u64) -> bool) -> Option<u64> {
        let i = self.back.iter().rposition(|&t| t == to)?;
        if !exists(to) {
            return None;
        }
        let passed = self.back.split_off(i + 1);
        self.back.pop();
        let behind = self
            .at
            .replace(to)
            .into_iter()
            .chain(passed.into_iter().rev());
        self.forward.extend(behind.filter(|&t| exists(t)));
        Some(to)
    }

    /// Go forward to `to`, a tab on the forward list: the mirror of
    /// `back_to`.
    pub fn forward_to(&mut self, to: u64, exists: impl Fn(u64) -> bool) -> Option<u64> {
        let i = self.forward.iter().rposition(|&t| t == to)?;
        if !exists(to) {
            return None;
        }
        let passed = self.forward.split_off(i + 1);
        self.forward.pop();
        let ahead = self
            .at
            .replace(to)
            .into_iter()
            .chain(passed.into_iter().rev());
        for t in ahead.filter(|&t| exists(t)) {
            self.back.retain(|&b| b != t);
            self.back.push(t);
        }
        Some(to)
    }

    /// Whether going back would show a tab that still `exists`.
    pub fn can_back(&self, exists: impl Fn(u64) -> bool) -> bool {
        self.back.iter().any(|&t| exists(t))
    }

    /// Whether going forward would show a tab that still `exists`.
    pub fn can_forward(&self, exists: impl Fn(u64) -> bool) -> bool {
        self.forward.iter().any(|&t| exists(t))
    }

    /// The tabs going back would show, the next first, that still `exists`.
    pub fn backs(&self, exists: impl Fn(u64) -> bool) -> Vec<u64> {
        self.back
            .iter()
            .rev()
            .copied()
            .filter(|&t| exists(t))
            .collect()
    }

    /// The tabs going forward would show, the next first, that still
    /// `exists`.
    pub fn forwards(&self, exists: impl Fn(u64) -> bool) -> Vec<u64> {
        self.forward
            .iter()
            .rev()
            .copied()
            .filter(|&t| exists(t))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::NavHistory;

    fn all(_: u64) -> bool {
        true
    }

    fn visited(tabs: &[u64]) -> NavHistory {
        let mut h = NavHistory::default();
        for &t in tabs {
            h.visit(Some(t), all);
        }
        h
    }

    #[test]
    fn back_and_forward_walk_the_visits() {
        let mut h = visited(&[1, 2, 3]);
        assert_eq!(h.backs(all), [2, 1]);
        assert_eq!(h.back(all), Some(2));
        assert_eq!(h.back(all), Some(1));
        assert_eq!(h.back(all), None);
        assert_eq!(h.forward(all), Some(2));
        assert_eq!(h.forward(all), Some(3));
        assert_eq!(h.forward(all), None);
        assert_eq!(h.backs(all), [2, 1]);
    }

    #[test]
    fn showing_the_tab_shown_is_no_visit_and_nothing_shown_changes_nothing() {
        let mut h = visited(&[1, 2, 2]);
        h.visit(None, all);
        assert_eq!(h.backs(all), [1]);
        assert_eq!(h.back(all), Some(1));
    }

    #[test]
    fn a_new_visit_drops_the_forward_list() {
        let mut h = visited(&[1, 2, 3]);
        h.back(all);
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
        let open = |t: u64| t != 3 && t != 1;
        assert_eq!(h.backs(open), [2]);
        assert_eq!(h.back(open), Some(2));
        assert_eq!(h.back(open), None);
        assert_eq!(h.forward(open), Some(4));
        // The tab shown being closed doesn't go back on the list.
        let mut h = visited(&[1, 2]);
        h.visit(Some(5), |t| t != 2);
        assert_eq!(h.backs(all), [1]);
    }

    #[test]
    fn going_to_a_listed_tab_passes_those_between() {
        let mut h = visited(&[1, 2, 3, 4]);
        assert_eq!(h.back_to(1, all), Some(1));
        assert_eq!(h.forwards(all), [2, 3, 4]);
        assert_eq!(h.forward_to(3, all), Some(3));
        assert_eq!(h.backs(all), [2, 1]);
        assert_eq!(h.forwards(all), [4]);
        assert_eq!(h.back_to(9, all), None);
    }

    #[test]
    fn the_back_list_keeps_the_last_fifty() {
        let h = visited(&(0..80).collect::<Vec<_>>());
        let backs = h.backs(all);
        assert_eq!(backs.len(), super::LIMIT);
        assert_eq!(backs[0], 78);
    }
}
