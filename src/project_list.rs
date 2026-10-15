//! What the welcome screen and the project menu share: matching the projects
//! against what was typed, keeping the selection in view, and finding out
//! which projects' herdr sessions run.

use std::cmp::Reverse;
use std::time::Instant;

use smithay::reexports::calloop::channel;

use crate::{Gerdr, anim, chrome::ProjectRow, fuzzy, herdr};

/// The most characters a path shown in a project row or menu entry takes.
pub const PATH_CHARS: usize = 40;

/// The hover pill of a row fading in: what it is over and since when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hover<T> {
    pub hit: T,
    fade: anim::Fade,
}

impl<T: Copy + PartialEq> Hover<T> {
    /// The pointer is over `hit` (or nothing) at `now`: a new thing starts
    /// its fade, the same keeps its place.
    pub fn moved(this: Option<Self>, hit: Option<T>, now: Instant) -> Option<Self> {
        let hit = hit?;
        match this {
            Some(h) if h.hit == hit => Some(h),
            _ => Some(Self {
                hit,
                fade: anim::Fade::new(now, anim::FAST, true),
            }),
        }
    }

    /// The pill's opacity at `now`, quantized for the cache key.
    pub fn alpha(&self, now: Instant) -> u8 {
        anim::quantize(self.fade.value(now))
    }
}

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
    fn scrolls_as_little_as_it_can() {
        assert_eq!(scrolled(0, 2, 3), 0);
        assert_eq!(scrolled(0, 5, 3), 3);
        assert_eq!(scrolled(4, 2, 3), 2);
    }

    #[test]
    fn a_hover_fades_in_once_per_thing() {
        let start = Instant::now();
        let h = Hover::moved(None, Some(1), start);
        assert_eq!(h.map(|h| h.hit), Some(1));
        assert_eq!(h.unwrap().alpha(start), 0);
        assert_eq!(h.unwrap().alpha(start + anim::FAST), 16);
        // Still over the same thing: the fade keeps its start.
        let same = Hover::moved(h, Some(1), start + anim::FAST);
        assert_eq!(same, h);
        let other = Hover::moved(h, Some(2), start + anim::FAST);
        assert_eq!(other.unwrap().alpha(start + anim::FAST), 0);
        assert_eq!(Hover::moved(h, None, start), None);
    }
}
