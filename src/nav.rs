//! Going back and forward through the tabs shown (see `nav_history`),
//! across workspaces and projects. A tab of a project left is parked by
//! its workspace's record id and its herdr tab (else its place among the
//! tabs without one); going back to it shows its project, the way the
//! last-project key does (asking first when that asks), and then the tab,
//! once the project's tabs are back. The history, parked, is kept in the
//! store every `RECORDS_INTERVAL` and when gerdr quits, and taken back as
//! gerdr starts. A project shown (gerdr starting, a switch) comes back on
//! the latest of its tabs in the history, the one it was left on.

use std::time::{Duration, Instant};

use crate::{
    Gerdr,
    nav_history::{NavHistory, Visit},
    tab_strip::visited_label,
};

/// How long a tab of another project gone to is waited for once that
/// project shows: its tabs come back from herdr a moment after. Past it,
/// the project's own tab shown counts as the visit.
const PENDING_WAIT: Duration = Duration::from_secs(8);

/// A tab to show once its project's tabs are back: a step back
/// (`step` true) or forward to it, or (`None`) the tab the project shown
/// was left on.
#[derive(Debug)]
pub struct PendingVisit {
    to: Visit,
    step: Option<bool>,
    since: Instant,
}

impl Gerdr {
    /// Where `visit` is, if it is a tab of the shown project: its
    /// workspace and its place there.
    fn visit_place(&self, visit: &Visit) -> Option<(usize, usize)> {
        let Visit::Parked {
            project,
            workspace,
            herdr_tab,
            index,
            ..
        } = visit
        else {
            let Visit::Tab(id) = visit else { return None };
            return self.tab_place(*id);
        };
        if self.project.as_deref() != Some(project) {
            return None;
        }
        let w = self
            .workspaces
            .iter()
            .position(|ws| ws.record == *workspace)?;
        let tabs = &self.workspaces[w].tabs;
        let t = match herdr_tab {
            Some(herdr) => tabs
                .iter()
                .position(|t| t.herdr_tab_id() == Some(herdr.as_str()))?,
            None => {
                tabs.iter()
                    .enumerate()
                    .filter(|(_, t)| t.herdr_tab_id().is_none())
                    .nth(*index)?
                    .0
            }
        };
        Some((w, t))
    }

    /// Whether `visit` can be gone to: a tab of the shown project still
    /// there, or one of another project still kept.
    pub fn can_visit(&self, visit: &Visit) -> bool {
        match visit {
            Visit::Parked { project, .. } if self.project.as_deref() != Some(project) => {
                self.projects.project(project).is_some()
            }
            _ => self.visit_place(visit).is_some(),
        }
    }

    /// Whether `visit` stays listed: a tab of the shown project still
    /// there, or one parked of a project still kept (the shown one's
    /// tabs may still be coming back).
    fn keeps_visit(&self, visit: &Visit) -> bool {
        match visit {
            Visit::Parked { project, .. } => self.projects.project(project).is_some(),
            Visit::Tab(id) => self.tab_place(*id).is_some(),
        }
    }

    /// `visit` as a tab of the shown project, once it is back; else as it is.
    fn unpark(&self, visit: &Visit) -> Visit {
        match visit {
            Visit::Parked { .. } => self
                .visit_place(visit)
                .map(|(w, t)| Visit::Tab(self.workspaces[w].tabs[t].id)),
            Visit::Tab(_) => None,
        }
        .unwrap_or_else(|| visit.clone())
    }

    /// What the back and forward buttons' menus call `visit`: its
    /// workspace, number and title, after its project's name when another
    /// project's.
    pub fn visit_label(&self, visit: &Visit) -> Option<String> {
        if let Some((w, t)) = self.visit_place(visit) {
            let ws = &self.workspaces[w];
            let title = self.tab_views(ws).get(t)?.title.clone();
            return Some(visited_label(&ws.name, t, &title));
        }
        let Visit::Parked { project, label, .. } = visit else {
            return None;
        };
        let name = self.projects.project(project)?.name.as_str();
        Some(format!("{name} › {label}"))
    }

    /// Note the tab shown now as visited, once tabs parked of the shown
    /// project are known by their ids again, and unless a step to another
    /// project's tab still waits for it to show.
    pub fn note_visit(&mut self) {
        if let Some(project) = self.project.as_deref()
            && self
                .nav
                .history
                .any(|v| matches!(v, Visit::Parked { project: p, .. } if p == project))
        {
            let mut nav = std::mem::take(&mut self.nav.history);
            nav.rebind(|v| Some(self.unpark(v)));
            self.nav.history = nav;
        }
        if self.take_pending_visit() {
            return;
        }
        let shown = self.tab().map(|tab| Visit::Tab(tab.id));
        let mut nav = std::mem::take(&mut self.nav.history);
        nav.visit(shown, |v| self.keeps_visit(v));
        self.nav.history = nav;
    }

    /// Take the step waiting for another project once its tab is back;
    /// whether it still waits. It waits while that project is asked for
    /// (the switch dialog is open) and for `PENDING_WAIT` once it shows.
    fn take_pending_visit(&mut self) -> bool {
        let Some(mut pending) = self.nav.pending.take() else {
            return false;
        };
        if self.overlays.quit.is_some() {
            pending.since = Instant::now();
        }
        let waited = pending.since.elapsed() > PENDING_WAIT;
        let Visit::Parked { project, .. } = &pending.to else {
            return false;
        };
        if self.project.as_deref() != Some(project) {
            if !waited {
                self.nav.pending = Some(pending);
            }
            return false;
        }
        let to = self.unpark(&pending.to);
        if let Visit::Tab(_) = to {
            match pending.step {
                Some(back) => self.step_to(&to, back),
                None => self.show_visit(&to),
            }
            return false;
        }
        // The tab left on being gone is no news: the project shows another.
        if waited && pending.step.is_none() {
            return false;
        }
        if waited {
            // Its project shows, but not the tab: closed since, or its
            // herdr session restarted.
            if let Some(label) = self.visit_label(&pending.to) {
                self.show_warning(format!("{label} isn't open any more"));
            }
            return false;
        }
        self.nav.pending = Some(pending);
        true
    }

    /// Whether going back, and going forward, would show a tab.
    pub fn can_go(&self) -> (bool, bool) {
        let can = |v: &Visit| self.can_visit(v);
        (
            self.nav.history.can_back(can),
            self.nav.history.can_forward(can),
        )
    }

    /// Show the tab shown before this one, in whatever workspace or
    /// project.
    pub fn go_back(&mut self) {
        if let Some(to) = self
            .nav
            .history
            .backs(|v| self.can_visit(v))
            .into_iter()
            .next()
        {
            self.go_to_visited(to, true);
        }
    }

    /// Undo a step back.
    pub fn go_forward(&mut self) {
        if let Some(to) = self
            .nav
            .history
            .forwards(|v| self.can_visit(v))
            .into_iter()
            .next()
        {
            self.go_to_visited(to, false);
        }
    }

    /// Go back (`back`) or forward to `to`: a tab of another project once
    /// that project shows.
    pub fn go_to_visited(&mut self, to: Visit, back: bool) {
        if let Visit::Parked { project, .. } = &to
            && self.project.as_deref() != Some(project)
        {
            if self.projects.project(project).is_none() {
                return;
            }
            let project = project.clone();
            self.nav.pending = Some(PendingVisit {
                to,
                step: Some(back),
                since: Instant::now(),
            });
            return self.switch_project(&project);
        }
        self.nav.pending = None;
        self.step_to(&to, back);
    }

    /// Step back (`back`) or forward to `to`, a tab of the shown project,
    /// and show it.
    fn step_to(&mut self, to: &Visit, back: bool) {
        let mut nav = std::mem::take(&mut self.nav.history);
        let keep = |v: &Visit| self.keeps_visit(v);
        let to = if back {
            nav.back_to(to, keep)
        } else {
            nav.forward_to(to, keep)
        };
        self.nav.history = nav;
        if let Some(to) = to {
            self.show_visit(&to);
        }
    }

    /// Show `visit`, a tab of the shown project, if it is there.
    fn show_visit(&mut self, visit: &Visit) {
        let Some((w, t)) = self.visit_place(visit) else {
            return;
        };
        self.active_workspace = w;
        self.workspaces[w].active_tab = t;
        self.show_active();
    }

    /// The project just shown comes back on the latest of its tabs in the
    /// history, once its tabs are back: whichever of its workspaces came
    /// back first would show else. Not while a step back or forward goes to
    /// one of its tabs.
    pub fn show_last_tab(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let of = |v: &Visit| matches!(v, Visit::Parked { project: p, .. } if *p == project);
        if self.nav.pending.as_ref().is_some_and(|p| of(&p.to)) {
            return;
        }
        if let Some(to) = self.nav.history.latest(of).cloned() {
            self.nav.pending = Some(PendingVisit {
                to,
                step: None,
                since: Instant::now(),
            });
        }
    }

    /// Park the shown project's tabs in the history, as it is let go of:
    /// each by ids that outlive its windows.
    /// Whether the store's copy changed.
    pub fn park_visits(&mut self) -> bool {
        let kept = self.keep_nav(true);
        self.nav.history = self.parked_nav();
        kept
    }

    /// The history with the shown project's tabs parked, as it outlives
    /// them.
    fn parked_nav(&self) -> NavHistory<Visit> {
        let mut nav = self.nav.history.clone();
        let project = self.project.as_deref();
        nav.rebind(|v| match (v, project) {
            (Visit::Tab(id), Some(project)) => self.park(project, *id),
            (Visit::Tab(_), None) => None,
            (parked, _) => Some(parked.clone()),
        });
        nav
    }

    /// Put the history, parked, in the store to save with it, if it
    /// changed since it was last put there, or anyway when `fresh` (a tab
    /// may have moved, or got its herdr tab, since); whether the store's
    /// copy changed.
    pub fn keep_nav(&mut self, fresh: bool) -> bool {
        if !fresh && self.nav.kept.as_ref() == Some(&self.nav.history) {
            return false;
        }
        self.nav.kept = Some(self.nav.history.clone());
        let parked = self.parked_nav();
        if self.projects.nav == parked {
            return false;
        }
        self.projects.nav = parked;
        true
    }

    /// Tab `id` of `project`, the shown one, parked; `None` if it is gone.
    fn park(&self, project: &str, id: u64) -> Option<Visit> {
        let (w, t) = self.tab_place(id)?;
        let ws = &self.workspaces[w];
        let tab = &ws.tabs[t];
        let herdr_tab = tab.herdr_tab_id().map(str::to_owned);
        let index = ws.tabs[..t]
            .iter()
            .filter(|t| t.herdr_tab_id().is_none())
            .count();
        let title = self.tab_views(ws).get(t)?.title.clone();
        Some(Visit::Parked {
            project: project.to_owned(),
            workspace: ws.record.clone(),
            herdr_tab,
            index,
            label: visited_label(&ws.name, t, &title),
        })
    }
}

impl crate::state::Nav {
    /// Take the history kept in the store (`kept`), as gerdr starts: its tabs are
    /// all parked (a `Tab::id` of an earlier run means nothing now).
    pub fn restore_nav(&mut self, kept: &NavHistory<Visit>) {
        let mut nav = kept.clone();
        nav.rebind(|v| matches!(v, Visit::Parked { .. }).then(|| v.clone()));
        self.history = nav.restored();
    }
}
