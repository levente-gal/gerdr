//! What the welcome screen and the project menu share: matching the projects
//! against what was typed, keeping the selection in view, and finding out
//! which projects' herdr sessions run.

use std::cmp::Reverse;

use smithay::reexports::calloop::channel;

use crate::{
    Gerdr,
    chrome::ProjectRow,
    fuzzy, herdr,
    scroll::{Bounds, Scroll},
};

/// The most characters a path shown in a project row or menu entry takes.
pub const PATH_CHARS: usize = 40;

/// The indices into `rows` of the projects `query` matches (by name, then
/// detail), best first, each with the positions (in chars) of the
/// characters matched in "name detail". Equal scores (all of them, without
/// a query) keep their order in `rows`.
pub fn matches(query: &str, rows: &[(String, ProjectRow)]) -> Vec<(usize, Vec<usize>)> {
    let mut found: Vec<(i32, usize, Vec<usize>)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, (_, row))| {
            let (score, hits) = fuzzy::score(query, &format!("{} {}", row.name, row.detail))?;
            Some((score, i, hits))
        })
        .collect();
    // A stable sort.
    found.sort_by_key(|(score, ..)| Reverse(*score));
    found.into_iter().map(|(_, i, hits)| (i, hits)).collect()
}

/// The first row to show so that `selected` is in the `page` rows shown,
/// moving as little as possible from `scroll` (as in the switcher).
pub fn scrolled(scroll: usize, selected: usize, page: usize) -> usize {
    scroll
        .min(selected)
        .max((selected + 1).saturating_sub(page))
}

/// Where a scrolled list's rows start down the list, in logical pixels,
/// and where the last one ends: what a `Scroll` standing in rows of
/// differing heights shows.
#[derive(Clone, Debug, PartialEq)]
pub struct RowTops(Vec<f64>);

impl Default for RowTops {
    fn default() -> Self {
        Self(vec![0.0])
    }
}

/// How close pixels count as the same, for sums of fractions.
const NEAR: f64 = 1e-6;

impl RowTops {
    /// The rows of `heights`, in order.
    pub fn new(heights: impl IntoIterator<Item = i32>) -> Self {
        let mut tops = vec![0.0];
        let mut y = 0.0;
        for h in heights {
            y += h as f64;
            tops.push(y);
        }
        Self(tops)
    }

    pub fn len(&self) -> usize {
        self.0.len() - 1
    }

    /// Where row `i` starts; the list's end for a row past it.
    pub fn top(&self, i: usize) -> f64 {
        self.0[i.min(self.len())]
    }

    /// How high row `i` is; none past the end.
    pub fn height(&self, i: usize) -> f64 {
        self.top(i + 1) - self.top(i)
    }

    /// Where the last row ends.
    pub fn end(&self) -> f64 {
        self.top(self.len())
    }

    /// How far down the list a position in rows stands, in pixels.
    pub fn px(&self, pos: f64) -> f64 {
        let pos = pos.max(0.0);
        let i = pos.floor() as usize;
        if i >= self.len() {
            return self.end();
        }
        self.top(i) + pos.fract() * self.height(i)
    }

    /// The position in rows standing `px` pixels down the list.
    pub fn pos(&self, px: f64) -> f64 {
        if px <= 0.0 {
            return 0.0;
        }
        let i = self.0[1..].partition_point(|&end| end <= px);
        if i >= self.len() {
            return self.len() as f64;
        }
        let h = self.height(i);
        i as f64 + if h > 0.0 { (px - self.top(i)) / h } else { 0.0 }
    }

    /// What moves a list `view` high standing as `scroll`: a notch moves
    /// it a row, and it goes down until its end shows at the bottom.
    pub fn bounds(&self, scroll: &Scroll, view: f64) -> Bounds {
        Bounds {
            max: self.pos(self.end() - view),
            notch: 1.0,
            unit: self.height(scroll.first()),
            view,
        }
    }

    /// Whether a list `view` high standing at `pos` shows all of the
    /// pixels from `top` to `bottom` down it.
    pub fn shows(&self, pos: f64, (top, bottom): (f64, f64), view: f64) -> bool {
        let at = self.px(pos);
        top >= at - NEAR && bottom <= at + view + NEAR
    }

    /// The position nearest `pos` at which a list `view` high shows the
    /// pixels from `top` to `bottom` down it; the top of them when they
    /// don't all fit.
    pub fn reveal(&self, pos: f64, (top, bottom): (f64, f64), view: f64) -> f64 {
        let at = self.px(pos);
        let to = if top < at - NEAR || bottom - top > view {
            top
        } else if bottom > at + view + NEAR {
            bottom - view
        } else {
            return pos;
        };
        self.pos(to.clamp(0.0, (self.end() - view).max(0.0)))
    }
}

impl Gerdr {
    /// Find out off the event loop which projects' sessions run; the answer
    /// lands in `running`, through `running_status`, set up the first time.
    pub fn check_running(&mut self) {
        if self.running_status.is_none() {
            let (tx, rx) = channel::channel::<Vec<(String, bool)>>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg(status) = event {
                    state.running = status.into_iter().collect();
                    state.refresh_project_menu();
                }
            });
            match inserted {
                Ok(_) => self.running_status = Some(tx),
                Err(err) => {
                    tracing::warn!("checking which projects run: {err}");
                    return;
                }
            }
        }
        let Some(tx) = self.running_status.clone() else {
            return;
        };
        let sessions: Vec<(String, String)> = self
            .projects
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.session.clone()))
            .collect();
        std::thread::spawn(move || {
            // Each check may wait for a server that doesn't answer: all at once.
            let status = std::thread::scope(|scope| {
                let checks: Vec<_> = sessions
                    .iter()
                    .map(|(id, session)| {
                        scope.spawn(move || (id.clone(), herdr::is_running(session)))
                    })
                    .collect();
                checks
                    .into_iter()
                    .filter_map(|check| check.join().ok())
                    .collect()
            });
            let _ = tx.send(status);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, detail: &str) -> (String, ProjectRow) {
        let row = ProjectRow {
            name: name.into(),
            detail: detail.into(),
            ago: String::new(),
            running: None,
            current: false,
        };
        (name.into(), row)
    }

    #[test]
    fn matches_best_first_and_keeps_order_without_a_query() {
        let rows = [row("gerdr", "~/Personal/gerdr"), row("aft", "~/Work/aft")];
        let found = |query: &str| -> Vec<usize> {
            matches(query, &rows).into_iter().map(|(i, _)| i).collect()
        };
        assert_eq!(found(""), vec![0, 1]);
        assert_eq!(found("aft"), vec![1]);
        assert_eq!(found("work"), vec![1]);
        assert!(found("zzz").is_empty());
        // The hits say which characters matched, in "name detail".
        assert_eq!(matches("aft", &rows), vec![(1, vec![0, 1, 2])]);
        assert_eq!(matches("", &rows)[0].1, Vec::<usize>::new());
    }

    #[test]
    fn row_tops_map_positions_to_pixels_and_back() {
        let tops = RowTops::new([22, 40, 40]);
        assert_eq!((tops.len(), tops.end()), (3, 102.0));
        assert_eq!(tops.px(1.5), 42.0);
        assert_eq!(tops.pos(42.0), 1.5);
        assert_eq!((tops.px(9.0), tops.pos(500.0)), (102.0, 3.0));
        assert_eq!(tops.pos(-3.0), 0.0);
        // The end shows at the bottom of a view 50 high part way into the
        // second row.
        let b = tops.bounds(&Scroll::at(1.0), 50.0);
        assert_eq!((b.max, b.unit), (1.0 + 30.0 / 40.0, 40.0));
        assert_eq!(tops.bounds(&Scroll::default(), 200.0).max, 0.0);
    }

    #[test]
    fn a_reveal_moves_as_little_as_it_can() {
        let tops = RowTops::new([40; 10]);
        // Shown already: kept, part of a row and all.
        assert_eq!(tops.reveal(1.25, (80.0, 120.0), 120.0), 1.25);
        assert!(tops.shows(1.25, (80.0, 120.0), 120.0));
        assert!(!tops.shows(1.25, (40.0, 80.0), 120.0));
        // Above: to its top; below: to its bottom at the view's.
        assert_eq!(tops.reveal(3.0, (40.0, 80.0), 120.0), 1.0);
        assert_eq!(tops.reveal(0.0, (200.0, 240.0), 120.0), 3.0);
        // Never past the end.
        assert_eq!(tops.reveal(0.0, (360.0, 400.0), 300.0), 2.5);
    }

    #[test]
    fn scrolls_as_little_as_it_can() {
        assert_eq!(scrolled(0, 2, 3), 0);
        assert_eq!(scrolled(0, 5, 3), 3);
        assert_eq!(scrolled(4, 2, 3), 2);
    }
}
