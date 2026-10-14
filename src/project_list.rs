//! What the welcome screen and the project menu share: matching the projects
//! against what was typed, keeping the selection in view, and finding out
//! which projects' herdr sessions run.

use std::cmp::Reverse;
use std::path::Path;
use std::time::Instant;

use smithay::reexports::calloop::channel;

use crate::{Gerdr, anim, chrome::ProjectRow, fuzzy, herdr, paths::tilde};

/// The most characters a path shown in a project row or menu entry takes.
pub const PATH_CHARS: usize = 40;

/// `path` as the chrome shows it: from "~" when it is inside the home
/// folder `home`, and with whole folders in its middle replaced by "…" so
/// it is at most `max_chars` long: "~/Work/…/service/api". The first and
/// the last folder always stay.
///
/// Needs: this is a stand-in for `paths::display`, added by A4 with the same
/// signature; integration should swap the calls over and delete this one.
pub fn display(path: &Path, home: Option<&Path>, max_chars: usize) -> String {
    squeeze(&tilde(path, home), max_chars)
}

/// `shown`, a path already written with "~", cut to `max_chars` by
/// replacing whole folders in its middle with "…".
pub fn squeeze(shown: &str, max_chars: usize) -> String {
    if shown.chars().count() <= max_chars {
        return shown.to_owned();
    }
    let absolute = shown.starts_with('/');
    let parts: Vec<&str> = shown.trim_start_matches('/').split('/').collect();
    // The start that always stays: "~" or "/" and the first folder.
    let head_len = if parts.first() == Some(&"~") { 2 } else { 1 };
    if parts.len() < head_len + 2 {
        // Nothing in the middle to cut: keep the end, which tells most.
        let chars: Vec<char> = shown.chars().collect();
        let keep = max_chars.saturating_sub(1).min(chars.len());
        return format!(
            "…{}",
            chars[chars.len() - keep..].iter().collect::<String>()
        );
    }
    let (head, rest) = parts.split_at(head_len);
    let join = |tail: &[&str]| {
        let mut names: Vec<&str> = head.to_vec();
        names.push("…");
        names.extend(tail);
        let joined = names.join("/");
        if absolute {
            format!("/{joined}")
        } else {
            joined
        }
    };
    // As many of the last folders as fit, one at least.
    for keep in (1..rest.len()).rev() {
        let out = join(&rest[rest.len() - keep..]);
        if out.chars().count() <= max_chars {
            return out;
        }
    }
    join(&rest[rest.len() - 1..])
}

/// A row's detail ("3 repos · ~/Work/aft") with its path, if it ends in
/// one, squeezed to `PATH_CHARS`.
pub fn squeeze_detail(detail: &str) -> String {
    match detail.rsplit_once(" · ") {
        Some((head, path)) if path.starts_with('/') || path.starts_with('~') => {
            format!("{head} · {}", squeeze(path, PATH_CHARS))
        }
        _ => detail.to_owned(),
    }
}

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
    fn display_cuts_whole_folders_from_the_middle() {
        let home = Path::new("/home/me");
        assert_eq!(
            display(Path::new("/home/me/Work/aft"), Some(home), 40),
            "~/Work/aft"
        );
        assert_eq!(display(Path::new("/home/me"), Some(home), 40), "~");
        assert_eq!(
            display(
                Path::new("/home/me/Work/clients/acme/service/api"),
                Some(home),
                24
            ),
            "~/Work/…/service/api"
        );
        assert_eq!(
            display(
                Path::new("/home/me/Work/clients/acme/service/api"),
                Some(home),
                14
            ),
            "~/Work/…/api"
        );
        assert_eq!(
            display(Path::new("/srv/data/sets/2026/october"), None, 20),
            "/srv/…/2026/october"
        );
        // Too long even with one folder on each side: the end stays.
        assert_eq!(
            display(Path::new("/srv/data/sets/2026/october"), None, 8),
            "/srv/…/october"
        );
        assert_eq!(
            squeeze("/a-very-long-single-folder-name", 12),
            "…folder-name"
        );
        assert_eq!(
            squeeze_detail("2 repos · /tmp/claude/-home-me-Personal-gerdr/8078/scratch/gh"),
            "2 repos · /tmp/…/8078/scratch/gh"
        );
        assert_eq!(squeeze_detail("2 repos"), "2 repos");
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
