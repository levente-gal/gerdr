//! What the welcome screen and the project menu share: matching the projects
//! against what was typed, keeping the selection in view, and finding out
//! which projects' herdr sessions run.

use std::cmp::Reverse;

use smithay::reexports::calloop::channel;

use crate::{Gerdr, chrome::ProjectRow, fuzzy, herdr};

/// The indices into `rows` of the projects `query` matches (by name, then
/// detail), best first. Equal scores (all of them, without a query) keep
/// their order in `rows`.
pub fn matches(query: &str, rows: &[(String, ProjectRow)]) -> Vec<usize> {
    let mut found: Vec<(i32, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, (_, row))| {
            let (score, _) = fuzzy::score(query, &format!("{} {}", row.name, row.detail))?;
            Some((score, i))
        })
        .collect();
    // A stable sort.
    found.sort_by_key(|(score, _)| Reverse(*score));
    found.into_iter().map(|(_, i)| i).collect()
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
        assert_eq!(matches("", &rows), vec![0, 1]);
        assert_eq!(matches("aft", &rows), vec![1]);
        assert_eq!(matches("work", &rows), vec![1]);
        assert!(matches("zzz", &rows).is_empty());
    }

    #[test]
    fn scrolls_as_little_as_it_can() {
        assert_eq!(scrolled(0, 2, 3), 0);
        assert_eq!(scrolled(0, 5, 3), 3);
        assert_eq!(scrolled(4, 2, 3), 2);
    }
}
