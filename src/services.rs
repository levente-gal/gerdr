//! The errors of the services running in the panes (web servers, workers),
//! as gerdr shows them: a desktop notification as each comes, an inbox
//! item per error (the same one again counted on it), a count on its
//! workspace in the sidebar until it is looked at. A service whose command
//! stops by itself (not by a Ctrl+C typed into its pane, nor by gerdr
//! stopping or restarting it) is told the same way, until it runs again.
//!
//! Every project whose herdr session runs is watched, the one shown and
//! the others (by the commands their records keep), unless another gerdr
//! shows it: an error in a project out of sight is told too, and going to
//! it switches to its project. What was found is kept in `state.json`, so
//! it outlives a restart of gerdr, and where reading each pane left off
//! too, so a restarted gerdr tells what came while it was down.
//!
//! `service_watch` finds them, on a thread per session; this keeps them
//! and wires them into gerdr.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::{
    RegistrationToken, channel,
    timer::{TimeoutAction, Timer},
};

use crate::{
    Gerdr, Tab, chrome,
    config::Services,
    herdr, inbox_store, project_lock,
    service_errors::ServiceError,
    service_watch::{self, Report, Service},
};

/// The prefix of a notification's key for a service's error:
/// `service:<project>|<pane id>|<fingerprint>`.
pub const NOTIFY_PREFIX: &str = "service:";
/// The fingerprint of a service having stopped.
pub const STOPPED: &str = "stopped";
/// How long after a Ctrl+C typed into a pane its service stopping is the
/// user's doing.
const INTERRUPT_GRACE: Duration = Duration::from_secs(15);
/// How often the projects out of sight are looked for running sessions.
const DISCOVER_EVERY: Duration = Duration::from_secs(30);
/// How long an error asked for in another project waits for it to show.
const SHOW_WAIT: Duration = Duration::from_secs(20);
/// The most errors kept, the newest.
const MAX_GROUPS: usize = 200;

/// An error a service printed (or its stopping), and how many times since.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorGroup {
    /// The project of its pane, by id.
    pub project: String,
    pub pane_id: String,
    pub fingerprint: String,
    pub title: String,
    /// All of its last time, as printed; a stopped service's last lines.
    pub lines: Vec<String>,
    /// What the service was called when it last came.
    #[serde(default)]
    pub service: String,
    pub count: u32,
    /// When it first and last came, in seconds since the epoch.
    pub first_secs: u64,
    pub last_secs: u64,
    /// Whether it was looked at since it last came.
    #[serde(default)]
    pub seen: bool,
}

impl ErrorGroup {
    /// Whether it is a service having stopped.
    pub fn is_stop(&self) -> bool {
        self.fingerprint == STOPPED
    }
}

/// The watcher of a project's session.
struct Watcher {
    session: String,
    tx: mpsc::Sender<Vec<Service>>,
    token: RegistrationToken,
    /// The list last sent to it.
    sent: Vec<Service>,
}

/// What gerdr keeps of the watching (the errors themselves are in the
/// store, `Store::service_errors`).
#[derive(Default)]
pub struct ServiceErrors {
    /// By project id.
    watchers: HashMap<String, Watcher>,
    /// When Ctrl+C was last typed into each pane of the project shown.
    interrupts: HashMap<String, Instant>,
    pub settings: Services,
    /// An error to show once its project does: project, pane,
    /// fingerprint, until when.
    pending: Option<(String, String, String, Instant)>,
    /// Whether the projects out of sight are being looked at.
    discovering: bool,
    /// The source of their answer, while it is awaited.
    discovery: Option<RegistrationToken>,
}

impl ServiceErrors {
    /// Watching nothing yet, with the `[services]` settings.
    pub fn new(settings: Services) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }
}

/// The key of pane `pane_id` of herdr session `session` among the saved
/// anchors.
fn anchor_key(session: &str, pane_id: &str) -> String {
    format!("{session}|{pane_id}")
}

/// What a notification or an inbox item calls `command`.
fn command_label(command: &str) -> String {
    chrome::command_line(command).unwrap_or_else(|| command.to_owned())
}

impl Gerdr {
    /// Start looking for the sessions of the projects out of sight.
    pub fn start_services(&mut self) {
        let timer = Timer::immediate();
        let tick = |_, _: &mut (), state: &mut Gerdr| {
            state.discover_services();
            TimeoutAction::ToDuration(DISCOVER_EVERY)
        };
        if let Err(err) = self.loop_handle.insert_source(timer, tick) {
            tracing::warn!("service errors: {err}");
        }
    }

    /// Watch the services of project `project`'s herdr session, on
    /// `socket`. A server just started (`fresh`) has new panes under the
    /// old ids: what was found and saved of them is let go.
    pub fn watch_project_services(&mut self, project: &str, socket: PathBuf, fresh: bool) {
        let Some(session) = self.projects.project(project).map(|p| p.session.clone()) else {
            return;
        };
        let prefix = anchor_key(&session, "");
        if fresh {
            self.drop_service_watcher(project);
            self.projects
                .service_anchors
                .retain(|key, _| !key.starts_with(&prefix));
            self.projects
                .service_errors
                .retain(|g| g.project != project);
            self.projects.save();
        }
        if self.service_errors.watchers.contains_key(project) {
            return;
        }
        let anchors = self
            .projects
            .service_anchors
            .iter()
            .filter_map(|(key, a)| Some((key.strip_prefix(&prefix)?.to_owned(), *a)))
            .collect();
        let (tx, rx) = channel::channel::<Report>();
        let id = project.to_owned();
        let inserted = self.loop_handle.insert_source(rx, move |event, _, state| {
            if let channel::Event::Msg(report) = event {
                state.on_service_report(&id, report);
            }
        });
        let token = match inserted {
            Ok(token) => token,
            Err(err) => return tracing::warn!("service errors: {err}"),
        };
        let patterns = self.service_errors.settings.patterns.clone();
        let feed = service_watch::spawn(socket, patterns, anchors, move |report| {
            tx.send(report).is_ok()
        });
        tracing::debug!("watching the services of {project}");
        let watcher = Watcher {
            session,
            tx: feed,
            token,
            sent: Vec::new(),
        };
        self.service_errors
            .watchers
            .insert(project.to_owned(), watcher);
    }

    /// Stop watching project `project`'s services.
    pub fn drop_service_watcher(&mut self, project: &str) {
        if let Some(watcher) = self.service_errors.watchers.remove(project) {
            self.loop_handle.remove(watcher.token);
        }
    }

    /// The herdr server of the project shown went away: its watcher with it.
    pub fn services_server_gone(&mut self) {
        if let Some(id) = self.project.clone() {
            self.drop_service_watcher(&id);
        }
    }

    /// The services project `project`'s records keep: the commands its
    /// panes run again after a reboot.
    fn recorded_services(&self, project: &str) -> Vec<Service> {
        let Some(p) = self.projects.project(project) else {
            return Vec::new();
        };
        p.workspaces
            .iter()
            .filter(|r| r.herdr_id.is_some())
            .flat_map(|r| &r.panes)
            .filter_map(|pane| {
                Some(Service {
                    pane_id: pane.pane_id.clone(),
                    command: pane.restart_command()?.to_owned(),
                })
            })
            .collect()
    }

    /// Send project `project`'s watcher `list` if it changed, and let go
    /// of the errors of the panes no longer in it.
    fn feed_watcher(&mut self, project: &str, mut list: Vec<Service>) {
        list.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
        let Some(watcher) = self.service_errors.watchers.get_mut(project) else {
            return;
        };
        if list == watcher.sent {
            return;
        }
        if watcher.tx.send(list.clone()).is_err() {
            return self.drop_service_watcher(project);
        }
        watcher.sent = list;
        let live: Vec<String> = watcher.sent.iter().map(|s| s.pane_id.clone()).collect();
        let before = self.projects.service_errors.len();
        self.projects
            .service_errors
            .retain(|g| g.project != project || live.contains(&g.pane_id));
        if self.projects.service_errors.len() != before {
            self.projects.save();
        }
    }

    /// Tell the watcher of the project shown which panes run services now:
    /// those that ran long enough or that gerdr runs again (see
    /// `Remembered::long`), and those its records keep. Show an error asked
    /// for in another project once it does.
    pub fn feed_services(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let mut list: Vec<Service> = self
            .last_commands
            .iter()
            .filter(|(_, r)| r.long)
            .map(|(pane_id, r)| Service {
                pane_id: pane_id.clone(),
                command: r.captured.command.clone(),
            })
            .collect();
        for service in self.recorded_services(&project) {
            if list.iter().all(|s| s.pane_id != service.pane_id) {
                list.push(service);
            }
        }
        // Only the panes there: a record may outlive its pane a while.
        let live: Vec<&str> = self
            .herdr_tabs
            .values()
            .flat_map(|d| &d.panes)
            .map(|p| p.pane_id.as_str())
            .collect();
        if !live.is_empty() {
            list.retain(|s| live.contains(&s.pane_id.as_str()));
        }
        self.feed_watcher(&project, list);
        self.show_pending_service_error();
    }

    /// Look for the projects out of sight whose session runs, off the
    /// event loop, then watch them (see `on_discovered`).
    fn discover_services(&mut self) {
        if self.service_errors.discovering {
            return;
        }
        let lock_dir = project_lock::dir();
        let candidates: Vec<(String, String)> = self
            .projects
            .projects
            .iter()
            .filter(|p| self.project.as_deref() != Some(p.id.as_str()))
            .filter(|p| {
                !lock_dir
                    .as_ref()
                    .is_some_and(|dir| project_lock::is_open_elsewhere(dir, &p.id))
            })
            .filter(|p| !self.recorded_services(&p.id).is_empty())
            .map(|p| (p.id.clone(), p.session.clone()))
            .collect();
        let (tx, rx) = channel::channel::<Vec<(String, bool)>>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(found) = event {
                state.on_discovered(found);
            }
        });
        let Ok(token) = inserted else {
            return;
        };
        self.service_errors.discovering = true;
        std::thread::spawn(move || {
            let found = candidates
                .into_iter()
                .map(|(id, session)| {
                    let running = herdr::is_running(&session);
                    (id, running)
                })
                .collect();
            let _ = tx.send(found);
        });
        // The channel's source goes once it has answered.
        self.service_errors.discovery = Some(token);
    }

    /// The projects out of sight, each with whether its session runs:
    /// watch those that do (by their records), let go of the rest.
    fn on_discovered(&mut self, found: Vec<(String, bool)>) {
        self.service_errors.discovering = false;
        if let Some(token) = self.service_errors.discovery.take() {
            self.loop_handle.remove(token);
        }
        let shown = self.project.clone();
        // Watchers of projects no longer looked at (closed elsewhere,
        // removed): gone.
        let stale: Vec<String> = self
            .service_errors
            .watchers
            .keys()
            .filter(|id| Some(id.as_str()) != shown.as_deref())
            .filter(|id| found.iter().all(|(f, running)| f != *id || !running))
            .cloned()
            .collect();
        for id in stale {
            self.drop_service_watcher(&id);
        }
        // Those of projects removed.
        let before = self.projects.service_errors.len();
        let store = &self.projects;
        let kept: Vec<_> = store
            .service_errors
            .iter()
            .filter(|g| store.project(&g.project).is_some())
            .cloned()
            .collect();
        self.projects.service_errors = kept;
        let mut changed = self.projects.service_errors.len() != before;
        for (id, running) in found {
            if Some(id.as_str()) == shown.as_deref() {
                continue;
            }
            if !running {
                let before = self.projects.service_errors.len();
                self.projects.service_errors.retain(|g| g.project != id);
                changed |= self.projects.service_errors.len() != before;
                continue;
            }
            let Some(session) = self.projects.project(&id).map(|p| p.session.clone()) else {
                continue;
            };
            self.watch_project_services(&id, herdr::socket_path(&session), false);
            let list = self.recorded_services(&id);
            self.feed_watcher(&id, list);
        }
        if changed {
            self.projects.save();
        }
    }

    /// What project `project`'s service in pane `pane_id` is called.
    fn service_label(&self, project: &str, pane_id: &str) -> String {
        if self.project.as_deref() == Some(project) {
            return self.pane_bar_content(pane_id).label;
        }
        self.service_errors
            .watchers
            .get(project)
            .and_then(|w| w.sent.iter().find(|s| s.pane_id == pane_id))
            .map(|s| command_label(&s.command))
            .unwrap_or_else(|| "service".to_owned())
    }

    /// What project `project`'s watcher found.
    fn on_service_report(&mut self, project: &str, report: Report) {
        match report {
            Report::Read {
                pane_id,
                errors,
                anchor,
            } => {
                if let Some(watcher) = self.service_errors.watchers.get(project) {
                    let key = anchor_key(&watcher.session, &pane_id);
                    self.projects.service_anchors.insert(key, anchor);
                }
                if errors.is_empty() {
                    return;
                }
                let label = self.service_label(project, &pane_id);
                for error in errors {
                    self.add_service_error(project, &pane_id, &label, error, None);
                }
                // Saved with the errors: a restart takes up after them.
                self.projects.save();
            }
            Report::Stopped {
                pane_id,
                lines,
                error,
                interrupted,
            } => {
                if !self.service_errors.settings.stopped
                    || interrupted
                    || self.stopped_on_purpose(project, &pane_id)
                {
                    return;
                }
                let label = self.service_label(project, &pane_id);
                let said = error
                    .map(|e| e.title)
                    .or_else(|| lines.last().cloned())
                    .unwrap_or_default();
                let stop = ServiceError {
                    fingerprint: STOPPED.to_owned(),
                    title: format!("Stopped: {label}"),
                    lines,
                };
                self.add_service_error(project, &pane_id, &label, stop, Some(said));
                self.projects.save();
            }
            Report::Started { pane_id } => {
                let before = self.projects.service_errors.len();
                self.projects
                    .service_errors
                    .retain(|g| !(g.project == project && g.pane_id == pane_id && g.is_stop()));
                if self.projects.service_errors.len() != before {
                    self.projects.save();
                }
            }
        }
    }

    /// Whether project `project`'s service in pane `pane_id` was stopped on
    /// purpose: by gerdr (stopping or restarting it) or by a Ctrl+C typed
    /// into its pane a moment before.
    fn stopped_on_purpose(&self, project: &str, pane_id: &str) -> bool {
        if self.project.as_deref() != Some(project) {
            return false;
        }
        self.pane_jobs.contains_key(pane_id)
            || self
                .service_errors
                .interrupts
                .get(pane_id)
                .is_some_and(|at| at.elapsed() < INTERRUPT_GRACE)
    }

    /// Count `error` of project `project`'s pane `pane_id` (service
    /// `label`) and tell of it: the same one again counts on the last. A
    /// stop's notification says `said`, what the service said last.
    fn add_service_error(
        &mut self,
        project: &str,
        pane_id: &str,
        label: &str,
        error: ServiceError,
        said: Option<String>,
    ) {
        let ServiceError {
            fingerprint,
            title,
            lines,
        } = error;
        let now = inbox_store::now_secs();
        let groups = &mut self.projects.service_errors;
        let at = groups.iter().position(|g| {
            g.project == project && g.pane_id == pane_id && g.fingerprint == fingerprint
        });
        let mut group = match at {
            Some(at) => {
                let mut group = groups.remove(at);
                group.count += 1;
                group
            }
            None => ErrorGroup {
                project: project.to_owned(),
                pane_id: pane_id.to_owned(),
                fingerprint: fingerprint.clone(),
                title: String::new(),
                lines: Vec::new(),
                service: String::new(),
                count: 1,
                first_secs: now,
                last_secs: now,
                seen: false,
            },
        };
        group.title = title.clone();
        group.lines = lines;
        group.service = label.to_owned();
        group.last_secs = now;
        group.seen = false;
        let count = group.count;
        let stop = group.is_stop();
        groups.insert(0, group);
        groups.truncate(MAX_GROUPS);
        if !self.service_errors.settings.notify {
            return;
        }
        let elsewhere = (self.project.as_deref() != Some(project))
            .then(|| self.projects.project(project).map(|p| p.name.clone()))
            .flatten();
        let summary = match (stop, elsewhere) {
            (true, None) => format!("{label} stopped"),
            (true, Some(name)) => format!("{name} · {label} stopped"),
            (false, None) => format!("{label}: error"),
            (false, Some(name)) => format!("{name} · {label}: error"),
        };
        let body = match (said, count) {
            (Some(said), _) => said,
            (None, 1) => title,
            (None, n) => format!("{title} (×{n})"),
        };
        if let Some(notifier) = self.notifier.as_mut() {
            let key = format!("{NOTIFY_PREFIX}{project}|{pane_id}|{fingerprint}");
            notifier.send(&key, &summary, &body);
        }
    }

    /// Show the error `fingerprint` of project `project`'s pane `pane_id`:
    /// its pane, its output open over it where the error is. One of
    /// another project switches to it first, then shows once it is there.
    pub fn show_service_error(&mut self, project: &str, pane_id: &str, fingerprint: &str) {
        let found =
            self.projects.service_errors.iter().any(|g| {
                g.project == project && g.pane_id == pane_id && g.fingerprint == fingerprint
            });
        if !found {
            return self.show_notice("That error is gone".into());
        }
        if self.project.as_deref() != Some(project) {
            self.service_errors.pending = Some((
                project.to_owned(),
                pane_id.to_owned(),
                fingerprint.to_owned(),
                Instant::now() + SHOW_WAIT,
            ));
            return self.switch_project(project);
        }
        self.service_errors.pending = None;
        let Some(group) =
            self.projects.service_errors.iter_mut().find(|g| {
                g.project == project && g.pane_id == pane_id && g.fingerprint == fingerprint
            })
        else {
            return;
        };
        group.seen = true;
        // A stopped service's last lines end its output: shown at the end.
        let first = (!group.is_stop())
            .then(|| group.lines.first().cloned())
            .flatten();
        self.projects.save();
        if !self.show_herdr_pane(pane_id) {
            return self.show_notice("Its pane isn't shown".into());
        }
        let Some(id) = self.tab().and_then(|tab| tab.focused_id()) else {
            return;
        };
        self.open_scrollback(id);
        let fit = self.scrollback_fit_for(id);
        if let (Some(viewer), Some(fit), Some(first)) = (self.scrollback.as_mut(), fit, first) {
            viewer.reveal_last(&first, fit);
        }
    }

    /// Show the error asked for in another project, once that project is
    /// shown with its pane; given up on after `SHOW_WAIT`.
    fn show_pending_service_error(&mut self) {
        let Some((project, pane_id, fingerprint, until)) = self.service_errors.pending.clone()
        else {
            return;
        };
        if Instant::now() > until {
            self.service_errors.pending = None;
            return;
        }
        // Its window up, not just the pane known.
        let shown = self.workspaces.iter().flat_map(|ws| &ws.tabs).any(|tab| {
            tab.panes
                .iter()
                .any(|p| p.herdr.as_ref().is_some_and(|h| h.pane_id == pane_id))
        });
        if self.project.as_deref() != Some(project.as_str()) || !shown {
            return;
        }
        self.show_service_error(&project, &pane_id, &fingerprint);
    }

    /// A notification of a service's error was clicked: `key` without its
    /// prefix.
    pub fn service_notification_clicked(&mut self, key: &str) {
        let mut parts = key.splitn(3, '|');
        if let (Some(project), Some(pane_id), Some(fingerprint)) =
            (parts.next(), parts.next(), parts.next())
        {
            let (project, pane_id, fingerprint) = (
                project.to_owned(),
                pane_id.to_owned(),
                fingerprint.to_owned(),
            );
            self.show_service_error(&project, &pane_id, &fingerprint);
        }
    }

    /// Mark the shown project's pane `pane_id`'s errors as looked at.
    pub fn see_service_errors(&mut self, pane_id: &str) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let mut changed = false;
        for g in &mut self.projects.service_errors {
            if g.project == project && g.pane_id == pane_id && !g.seen {
                g.seen = true;
                changed = true;
            }
        }
        if changed {
            self.projects.save();
        }
    }

    /// Mark project `project`'s error `fingerprint` of pane `pane_id` as
    /// looked at.
    pub fn see_service_error(&mut self, project: &str, pane_id: &str, fingerprint: &str) {
        let found =
            self.projects.service_errors.iter_mut().find(|g| {
                g.project == project && g.pane_id == pane_id && g.fingerprint == fingerprint
            });
        if let Some(g) = found.filter(|g| !g.seen) {
            g.seen = true;
            self.projects.save();
        }
    }

    /// The shown project's herdr pane `pane_id` is gone: so are its errors,
    /// and where its output was read to.
    pub fn forget_service_pane(&mut self, pane_id: &str) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let before = self.projects.service_errors.len();
        self.projects
            .service_errors
            .retain(|g| !(g.project == project && g.pane_id == pane_id));
        if let Some(watcher) = self.service_errors.watchers.get(&project) {
            let key = anchor_key(&watcher.session, pane_id);
            self.projects.service_anchors.remove(&key);
        }
        if self.projects.service_errors.len() != before {
            self.projects.save();
        }
    }

    /// Ctrl+C (or Ctrl+\) was typed into the focused window: if it is a
    /// terminal pane, its service stopping next is the user's doing.
    pub fn note_interrupt(&mut self) {
        let pane_id = self.tab().and_then(|tab| {
            let id = tab.focused_id()?;
            tab.herdr_pane(id).map(|p| p.pane_id.clone())
        });
        if let Some(pane_id) = pane_id {
            self.service_errors
                .interrupts
                .insert(pane_id, Instant::now());
        }
    }

    /// The shown project's errors in workspace `w`'s panes, the newest
    /// first.
    pub fn workspace_service_errors(&self, w: usize) -> Vec<&ErrorGroup> {
        let (Some(project), Some(ws)) = (self.project.as_deref(), self.workspaces.get(w)) else {
            return Vec::new();
        };
        let panes: Vec<&str> = ws
            .tabs
            .iter()
            .flat_map(Tab::herdr_panes)
            .map(|p| p.pane_id.as_str())
            .collect();
        self.projects
            .service_errors
            .iter()
            .filter(|g| g.project == project && panes.contains(&g.pane_id.as_str()))
            .collect()
    }

    /// Show the newest error of workspace `w` not looked at, else its
    /// newest.
    pub fn show_workspace_service_error(&mut self, w: usize) {
        let errors = self.workspace_service_errors(w);
        let Some(group) = errors.iter().find(|g| !g.seen).or(errors.first()) else {
            return;
        };
        let (project, pane_id, fingerprint) = (
            group.project.clone(),
            group.pane_id.clone(),
            group.fingerprint.clone(),
        );
        self.show_service_error(&project, &pane_id, &fingerprint);
    }

    /// What a project's service in pane `pane_id` is called, for the inbox.
    pub fn service_item_label(&self, project: &str, pane_id: &str) -> String {
        self.service_label(project, pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_round_trip() {
        let g = ErrorGroup {
            project: "api".into(),
            pane_id: "w1:p2".into(),
            fingerprint: STOPPED.into(),
            title: "Stopped: npm run dev".into(),
            lines: vec!["boom".into()],
            service: "npm run dev".into(),
            count: 2,
            first_secs: 1,
            last_secs: 2,
            seen: false,
        };
        let text = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<ErrorGroup>(&text).unwrap(), g);
        assert!(g.is_stop());
    }

    #[test]
    fn labels_commands() {
        assert_eq!(command_label("npm run dev"), "npm run dev");
    }

    #[test]
    fn anchors_are_kept_by_session() {
        assert_eq!(anchor_key("gerdr-api", "w1:p1"), "gerdr-api|w1:p1");
    }
}
