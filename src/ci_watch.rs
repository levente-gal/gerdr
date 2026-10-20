//! The CI of the shown project's repositories, in gerdr: read now and
//! then (see `ci`), shown as a card under each workspace of a repository,
//! notified of when a row finishes, and the GitHub workspace its pages
//! open in (see `Gerdr::open_github`). A pull request is watched from the
//! tab strip's button while the focused pane shows it.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use smithay::reexports::calloop::{
    channel::{self, Channel},
    timer::{TimeoutAction, Timer},
};

use crate::{
    Gerdr, Tab, Workspace, WorkspaceKind,
    agents::AgentKey,
    chrome::{CiChip, CiView},
    ci::{self, CiState, RepoCi, Subject},
    git,
};

/// What a round of reading the CI hands to the event loop: per repository
/// root, its CI or why it couldn't be read.
pub type CiRead = Vec<(PathBuf, Result<RepoCi, String>)>;

/// The name of a repository's GitHub workspace; with its folder's name
/// after it when the project has another.
const GITHUB_WORKSPACE: &str = "GitHub";

impl Gerdr {
    /// Read the CI of the shown project's repositories now and then, off
    /// the event loop: every `ci::BUSY_INTERVAL` while some of it runs,
    /// else every `ci::IDLE_INTERVAL`.
    pub fn watch_ci(&mut self) {
        let (tx, rx): (_, Channel<CiRead>) = channel::channel();
        let read = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(read) = event {
                state.ci_pending = false;
                state.ci_read(read);
                if std::mem::take(&mut state.ci_again) {
                    state.refresh_ci();
                }
            }
        });
        self.ci_sender = Some(tx);
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), move |_, _, state| {
                state.refresh_ci();
                TimeoutAction::ToDuration(state.ci_interval())
            });
        if let Err(err) = read
            .map_err(|e| e.to_string())
            .and(timer.map_err(|e| e.to_string()))
        {
            tracing::warn!("ci: {err}");
        }
    }

    /// How long until the CI is read again.
    fn ci_interval(&self) -> Duration {
        if self.ci.values().any(RepoCi::busy) {
            ci::BUSY_INTERVAL
        } else {
            ci::IDLE_INTERVAL
        }
    }

    /// The repositories the shown project's workspaces are in, by root.
    fn ci_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .workspaces
            .iter()
            .filter_map(|ws| self.git.get(&ws.dir)?.root.clone())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    /// Read the CI now, off the event loop; once the round under way is
    /// done, if there is one.
    pub fn refresh_ci(&mut self) {
        let Some(tx) = self.ci_sender.clone() else {
            return;
        };
        if self.ci_pending {
            self.ci_again = true;
            return;
        }
        let roots = self.ci_roots();
        self.ci_asked = roots.clone();
        if roots.is_empty() {
            self.ci.clear();
            return;
        }
        self.ci_pending = true;
        ci::read_all(roots, self.projects.ci_watched.clone(), move |read| {
            let _ = tx.send(read);
        });
    }

    /// The Git status was read again: a repository the CI wasn't read for
    /// (a workspace opened, another project shown) is read now.
    pub fn ci_roots_changed(&mut self) {
        if self.ci_roots() != self.ci_asked {
            self.refresh_ci();
        }
    }

    /// Take a round's CI in. A repository that couldn't be read keeps what
    /// was last read of it, unless it isn't asked about any more. Watched
    /// pull requests found closed are watched no more. Rows that finished
    /// are notified of.
    fn ci_read(&mut self, read: CiRead) {
        let asked: HashSet<&PathBuf> = self.ci_asked.iter().collect();
        let mut ci: HashMap<PathBuf, RepoCi> = HashMap::new();
        for (root, result) in read {
            if !asked.contains(&root) {
                continue;
            }
            match result {
                Ok(repo) => {
                    ci.insert(root, repo);
                }
                Err(err) => {
                    tracing::debug!("ci of {}: {err}", root.display());
                    if let Some(old) = self.ci.remove(&root) {
                        ci.insert(root, old);
                    }
                }
            }
        }
        let mut unwatched = false;
        for repo in ci.values() {
            let key = ci::key(&repo.host, &repo.slug);
            for n in &repo.closed {
                unwatched |= self.unwatch_pr(&key, *n);
            }
        }
        if unwatched {
            self.projects.save();
        }
        let repos: Vec<&RepoCi> = ci.values().collect();
        let news = ci::ci_news(&self.ci_states, &repos);
        self.ci_states = repos
            .iter()
            .flat_map(|repo| {
                let key = ci::key(&repo.host, &repo.slug);
                repo.rows
                    .iter()
                    .map(move |row| ((key.clone(), row.subject.clone()), row.state))
            })
            .collect();
        self.ci = ci;
        if let Some(notifier) = self.notifier.as_mut() {
            for news in news {
                notifier.send(
                    &AgentKey::Ci(news.link).to_string(),
                    &news.summary,
                    &news.body,
                );
            }
        }
    }

    /// The CI of the repository workspace `ws` is in, as last read.
    fn ci_of(&self, ws: &Workspace) -> Option<(&Path, &RepoCi)> {
        let root = self.git.get(&ws.dir)?.root.as_deref()?;
        Some((root, self.ci.get(root)?))
    }

    /// The CI card of workspace `ws`, if its repository has CI to show.
    pub fn ci_view(&self, ws: &Workspace) -> Option<CiView> {
        let (_, repo) = self.ci_of(ws)?;
        let chips: Vec<CiChip> = repo
            .rows
            .iter()
            .map(|row| CiChip {
                label: ci::subject_label(&row.subject),
                state: row.state,
                title: row.title.clone(),
                watched: row.watched,
                group: row.group,
                checks: row
                    .checks
                    .iter()
                    .map(|check| (check.name.clone(), check.state))
                    .collect(),
            })
            .collect();
        (!chips.is_empty() || repo.other_prs > 0).then_some(CiView {
            chips,
            other_prs: repo.other_prs,
        })
    }

    /// The row behind chip `k` of workspace `w`'s CI card, with its
    /// repository.
    pub fn ci_row(&self, w: usize, k: usize) -> Option<(&RepoCi, &ci::CiRow)> {
        let (_, repo) = self.ci_of(self.workspaces.get(w)?)?;
        Some((repo, repo.rows.get(k)?))
    }

    /// The CI of workspace `w`'s repository.
    pub fn ci_repo(&self, w: usize) -> Option<&RepoCi> {
        Some(self.ci_of(self.workspaces.get(w)?)?.1)
    }

    /// The pull request the focused pane shows, if it is one of a
    /// repository whose CI is read: its repository's key and number, and
    /// whether it is watched. `None` too for the user's own, shown anyway.
    pub fn focused_pull_request(&self) -> Option<(String, u64, bool)> {
        let tab = self.tab()?;
        let id = tab.focused_id()?;
        let url = tab
            .panes
            .iter()
            .find(|p| p.id == id)?
            .app_command
            .as_deref()?;
        let (host, slug, n) = ci::pull_request_of(url)?;
        let repo = self.ci.values().find(|repo| {
            repo.host.eq_ignore_ascii_case(&host) && repo.slug.eq_ignore_ascii_case(&slug)
        })?;
        let own = repo
            .rows
            .iter()
            .any(|row| row.subject == Subject::Pr(n) && !row.watched);
        let key = ci::key(&repo.host, &repo.slug);
        let watched = self
            .projects
            .ci_watched
            .get(&key)
            .is_some_and(|prs| prs.contains(&n));
        (!own).then_some((key, n, watched))
    }

    /// Watch the CI of the pull request the focused pane shows, or stop
    /// watching it.
    pub fn toggle_watch(&mut self) {
        let Some((key, n, watched)) = self.focused_pull_request() else {
            return;
        };
        if watched {
            self.unwatch(&key, n);
        } else {
            self.projects.ci_watched.entry(key).or_default().insert(n);
            self.projects.save();
            self.show_notice(format!("Watching the CI of #{n}"));
            self.refresh_ci();
        }
    }

    /// Stop watching pull request `n` of repository `key`; its chip goes
    /// right away.
    pub fn unwatch(&mut self, key: &str, n: u64) {
        if self.unwatch_pr(key, n) {
            self.projects.save();
        }
        for repo in self.ci.values_mut() {
            if ci::key(&repo.host, &repo.slug) == key {
                repo.rows
                    .retain(|row| !(row.watched && row.subject == Subject::Pr(n)));
            }
        }
    }

    /// Forget pull request `n` of repository `key` as watched, saying
    /// whether it was.
    fn unwatch_pr(&mut self, key: &str, n: u64) -> bool {
        let watched = &mut self.projects.ci_watched;
        let Some(prs) = watched.get_mut(key) else {
            return false;
        };
        let was = prs.remove(&n);
        if prs.is_empty() {
            watched.remove(key);
        }
        was
    }

    /// Show GitHub page `url` (else the shown workspace's repository's own
    /// page) in the GitHub workspace of its repository: the tab already
    /// showing it, else a new one; the workspace is made when there is
    /// none. It takes splits and tabs like any other.
    pub fn open_github(&mut self, url: Option<String>) {
        let of_url = url.as_deref().and_then(|url| {
            self.ci
                .iter()
                .find(|(_, repo)| url.starts_with(&repo.web_url()))
                .map(|(root, _)| root.clone())
        });
        let shown = self.workspace().map(|ws| {
            self.git
                .get(&ws.dir)
                .and_then(|g| g.root.clone())
                .unwrap_or_else(|| ws.dir.clone())
        });
        let Some(root) = of_url.or(shown) else {
            return;
        };
        let url = url.or_else(|| {
            self.repo_pages
                .entry(root.clone())
                .or_insert_with_key(|dir| git::web_url(dir))
                .clone()
        });
        let Some(url) = url else {
            self.show_notice("No GitHub page: the repository has no remote".into());
            return;
        };
        let existing = self
            .workspaces
            .iter()
            .position(|ws| ws.kind == WorkspaceKind::GitHub && ws.dir == root);
        if let Some(w) = existing
            && let Some((t, leaf)) = showing(&self.workspaces[w], &url)
        {
            return self.show_leaf(w, t, leaf);
        }
        let w = existing.unwrap_or_else(|| {
            let others = self
                .workspaces
                .iter()
                .any(|ws| ws.kind == WorkspaceKind::GitHub);
            let mut ws = Workspace::new(root.clone(), WorkspaceKind::GitHub);
            ws.name = if others {
                format!("{GITHUB_WORKSPACE} · {}", ws.name)
            } else {
                GITHUB_WORKSPACE.to_owned()
            };
            ws.record = self.remember_workspace(&ws.name, ws.kind, &ws.dir, None);
            self.workspaces.push(ws);
            self.workspaces.len() - 1
        });
        let record = self.workspaces[w].record.clone();
        let ws = &mut self.workspaces[w];
        ws.tabs.push(Tab::default());
        ws.active_tab = ws.tabs.len() - 1;
        self.active_workspace = w;
        let t = ws.active_tab;
        if self.launch_into(w, t, None, &url).is_none()
            && existing.is_none()
            && self.workspaces.iter().all(|ws| ws.record != record)
        {
            self.forget_workspace(&record);
        }
        self.show_active();
    }
}

/// The tab of `ws` and the leaf in it showing `url` (or waiting for it),
/// if any; a trailing "/" doesn't count.
fn showing(ws: &Workspace, url: &str) -> Option<(usize, u32)> {
    let same = |command: &Option<String>| {
        command
            .as_deref()
            .is_some_and(|c| c.trim_end_matches('/') == url.trim_end_matches('/'))
    };
    ws.tabs.iter().enumerate().find_map(|(t, tab)| {
        let shown = tab
            .panes
            .iter()
            .find(|p| same(&p.app_command))
            .map(|p| p.id);
        let waiting = || {
            tab.expected
                .iter()
                .find(|e| same(&e.app_command))
                .map(|e| e.pane)
        };
        Some((t, shown.or_else(waiting)?))
    })
}

/// What a tooltip says `state` is.
pub fn ci_state_word(state: CiState) -> &'static str {
    match state {
        CiState::Running => "Running",
        CiState::Passed => "Passed",
        CiState::Failed => "Failed",
        CiState::Unknown => "No runs",
    }
}
