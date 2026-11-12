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
//! An error the user ignores (by its fingerprint, in every pane of its
//! project) is kept apart, one record for all its panes: still counted as
//! it comes, listed light in the inbox, but never notified nor counted in
//! the sidebar. Unlike `[services] ignore`, which never takes its lines for
//! errors at all, it stays in sight.
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
    Gerdr, Tab, chrome, cli_agents,
    config::Services,
    herdr,
    inbox::Section,
    inbox_sources, project_lock,
    service_errors::ServiceError,
    service_watch::{self, Report, Service},
    test_runs,
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
/// The most ignored errors kept, the last to come.
const MAX_IGNORED: usize = 100;

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
    /// Whether its pane was closed since it last came: an ignored error's
    /// record outlives its pane.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pane_gone: bool,
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

/// Which error of which pane of which project: what an Undo marks as not
/// looked at again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceErrorKey {
    /// The project's id.
    pub project: String,
    pub pane_id: String,
    pub fingerprint: String,
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

    /// Take `settings`, the `[services]` table read again: new patterns
    /// start every watcher over with them, from where each left off.
    pub fn set_service_settings(&mut self, settings: Services) {
        let restart = settings.patterns != self.service_errors.settings.patterns;
        self.service_errors.settings = settings;
        if !restart {
            return;
        }
        let ids: Vec<String> = self.service_errors.watchers.keys().cloned().collect();
        for id in &ids {
            self.drop_service_watcher(id);
        }
        // The shown project's at once; the others' as they are found.
        if let Some(project) = self.project.clone().filter(|p| ids.contains(p))
            && let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf())
        {
            self.watch_project_services(&project, socket, false);
            self.feed_services();
        }
        self.discover_services();
    }

    /// The herdr server of the project shown went away: its watcher with it.
    pub fn services_server_gone(&mut self) {
        if let Some(id) = self.project.clone() {
            self.drop_service_watcher(&id);
        }
    }

    /// Whether `command` can be a service. A test run isn't, however long it
    /// runs: it ends by itself, and its card tells how it went. Nor is an
    /// agent: its card tells of it too, and what it prints isn't a service's
    /// log (the commands it runs, its talk of errors).
    pub(crate) fn may_be_service(&self, command: &str) -> bool {
        !test_runs::runs_tests(command)
            && !crate::feature::verify::runs_check(command)
            && !cli_agents::runs_agent(command, &self.agent_config)
    }

    /// The services project `project`'s records keep: the commands its
    /// panes run again after a reboot, those that can be one.
    fn recorded_services(&self, project: &str) -> Vec<Service> {
        let Some(p) = self.projects.project(project) else {
            return Vec::new();
        };
        p.workspaces
            .iter()
            .filter(|r| r.herdr_id.is_some())
            .flat_map(|r| &r.panes)
            .filter_map(|pane| {
                let command = pane.restart_command()?;
                self.may_be_service(command).then(|| Service {
                    pane_id: pane.pane_id.clone(),
                    command: command.to_owned(),
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
        let gone = mark_panes_gone(&mut self.projects.ignored_errors, project, |pane| {
            !live.iter().any(|l| l == pane)
        });
        if self.projects.service_errors.len() != before || gone {
            self.projects.save();
        }
    }

    /// Tell the watcher of the project shown which panes run services now:
    /// those that ran long enough or that gerdr runs again (see
    /// `Remembered::long`), and those its records keep, if it can be a
    /// service and herdr sees no agent in the pane. Show an error asked for in
    /// another project once it does.
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
        list.retain(|s| self.may_be_service(&s.command));
        // Only the panes there: a record may outlive its pane a while.
        let panes: Vec<&herdr::PaneState> =
            self.herdr_tabs.values().flat_map(|d| &d.panes).collect();
        if !panes.is_empty() {
            list.retain(|s| {
                panes
                    .iter()
                    .any(|p| p.pane_id == s.pane_id && p.agent.is_none())
            });
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
        let before = self.projects.ignored_errors.len();
        let store = &mut self.projects;
        let ids: Vec<String> = store.projects.iter().map(|p| p.id.clone()).collect();
        store.ignored_errors.retain(|g| ids.contains(&g.project));
        changed |= store.ignored_errors.len() != before;
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
        let now = crate::clock::now_secs();
        // An error ignored is only counted on its record: no group, no
        // notification.
        if record_ignored(
            &mut self.projects.ignored_errors,
            project,
            pane_id,
            label,
            &error,
            now,
        ) {
            return;
        }
        let ServiceError {
            fingerprint,
            title,
            lines,
        } = error;
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
                pane_gone: false,
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
        // As an agent's: none while its pane is in sight.
        if !self.service_errors.settings.notify || self.service_in_sight(project, pane_id) {
            return;
        }
        let elsewhere = (self.project.as_deref() != Some(project))
            .then(|| self.projects.project(project).map(|p| p.name.clone()))
            .flatten();
        let summary = match (stop, elsewhere) {
            (true, None) => format!("{label} stopped"),
            (true, Some(name)) => format!("{name} · {label} stopped"),
            (false, None) => format!("{label} printed an error"),
            (false, Some(name)) => format!("{name} · {label} printed an error"),
        };
        let body = match (said, count) {
            (Some(said), _) => said,
            (None, 1) => title,
            (None, n) => format!("{title} (×{n})"),
        };
        // `[notifications] enabled` turns every notification off.
        if let Some(notifier) = self.notifier.as_mut().filter(|_| self.notify) {
            let key = format!("{NOTIFY_PREFIX}{project}|{pane_id}|{fingerprint}");
            notifier.send(&key, &summary, &body);
        }
    }

    /// Whether project `project`'s pane `pane_id` is in sight: the project
    /// and the pane's tab shown, gerdr's window with the keyboard.
    fn service_in_sight(&self, project: &str, pane_id: &str) -> bool {
        self.host_focused
            && self.project.as_deref() == Some(project)
            && self
                .herdr_pane_place(pane_id)
                .is_some_and(|(w, t)| self.is_visible(w, t))
    }

    /// Show the error `fingerprint` of project `project`'s pane `pane_id`:
    /// its pane, its output open over it where the error is. One of
    /// another project switches to it first, then shows once it is there.
    pub fn show_service_error(&mut self, project: &str, pane_id: &str, fingerprint: &str) {
        let at = |g: &&ErrorGroup| {
            g.project == project && g.pane_id == pane_id && g.fingerprint == fingerprint
        };
        let found = (self.projects.service_errors.iter())
            .chain(&self.projects.ignored_errors)
            .find(|g| at(g));
        let Some(found) = found else {
            return self.show_warning("That error is gone".into());
        };
        // Its pane is closed: nothing to switch to, nor to wait for.
        let closed = found.pane_gone;
        if closed && !self.show_herdr_pane(pane_id) {
            return self.show_warning("Its pane is closed".into());
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
        let group =
            self.projects.service_errors.iter_mut().find(|g| {
                g.project == project && g.pane_id == pane_id && g.fingerprint == fingerprint
            });
        let first = match group {
            Some(group) => {
                group.seen = true;
                // A stopped service's last lines end its output: shown at
                // the end.
                let first = (!group.is_stop())
                    .then(|| group.lines.first().cloned())
                    .flatten();
                self.projects.save();
                first
            }
            // One ignored: its record's last time.
            None => match self.projects.ignored_errors.iter().find(|g| at(g)) {
                Some(record) => record.lines.first().cloned(),
                None => return,
            },
        };
        if !self.show_herdr_pane(pane_id) {
            let why = if closed {
                "Its pane is closed"
            } else {
                "Its pane isn't shown"
            };
            return self.show_warning(why.into());
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

    /// Mark errors `keys` as not looked at again (an Undo of looking): those
    /// still there.
    pub fn unsee_service_errors(&mut self, keys: &[ServiceErrorKey]) {
        let mut changed = false;
        for g in &mut self.projects.service_errors {
            let key = keys.iter().any(|k| {
                k.project == g.project && k.pane_id == g.pane_id && k.fingerprint == g.fingerprint
            });
            if key && g.seen {
                g.seen = false;
                changed = true;
            }
        }
        if changed {
            self.projects.save();
        }
    }

    /// The shown project's herdr pane `pane_id` is gone: so are its errors
    /// (an ignored one's record is kept, its pane marked gone), and where
    /// its output was read to.
    pub fn forget_service_pane(&mut self, pane_id: &str) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let before = self.projects.service_errors.len();
        self.projects
            .service_errors
            .retain(|g| !(g.project == project && g.pane_id == pane_id));
        // An ignored error's record stays, its pane known to be closed.
        let gone = mark_panes_gone(&mut self.projects.ignored_errors, &project, |pane| {
            pane == pane_id
        });
        if let Some(watcher) = self.service_errors.watchers.get(&project) {
            let key = anchor_key(&watcher.session, pane_id);
            self.projects.service_anchors.remove(&key);
        }
        if self.projects.service_errors.len() != before || gone {
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

    /// Open the inbox on workspace `w`'s newest error not looked at, else
    /// its newest; on its Services section if that one is hidden. The error
    /// counts as looked at, so the workspace's badge counts one less.
    pub fn show_workspace_service_error(&mut self, w: usize) {
        let errors = self.workspace_service_errors(w);
        let picked = errors
            .iter()
            .find(|g| !g.seen)
            .or(errors.first())
            .map(|g| (g.project.clone(), g.pane_id.clone(), g.fingerprint.clone()));
        let item = picked.as_ref().map(|(project, pane_id, fingerprint)| {
            self.see_service_error(project, pane_id, fingerprint);
            inbox_sources::service_item_id(project, pane_id, fingerprint)
        });
        self.open_inbox_on(Some(Section::Services), item);
    }

    /// What a project's service in pane `pane_id` is called, for the inbox.
    pub fn service_item_label(&self, project: &str, pane_id: &str) -> String {
        self.service_label(project, pane_id)
    }
}

/// An error's state across both lists, by project and fingerprint, before
/// it was ignored, ignored no more or forgotten: what an Undo puts back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorsSnapshot {
    /// The project's id.
    pub project: String,
    pub fingerprint: String,
    /// Its groups, a pane each, the newest first.
    pub groups: Vec<ErrorGroup>,
    /// Its record, when it was ignored.
    pub ignored: Option<ErrorGroup>,
    /// How many times it had come, across both lists, right after: what
    /// is over that came since.
    pub after: u32,
}

impl ErrorsSnapshot {
    /// How many times it had come, across both lists.
    fn total(&self) -> u32 {
        self.groups
            .iter()
            .chain(&self.ignored)
            .map(|g| g.count)
            .sum()
    }
}

/// Whether `g` is project `project`'s error `fingerprint`, of any pane.
fn same_error(g: &ErrorGroup, project: &str, fingerprint: &str) -> bool {
    g.project == project && g.fingerprint == fingerprint
}

/// Project `project`'s error `fingerprint` as `groups` and `ignored` hold
/// it, `after` its count as it is: what ignoring it or ignoring it no
/// more leaves.
fn snapshot(
    groups: &[ErrorGroup],
    ignored: &[ErrorGroup],
    project: &str,
    fingerprint: &str,
) -> ErrorsSnapshot {
    let mut snap = ErrorsSnapshot {
        project: project.to_owned(),
        fingerprint: fingerprint.to_owned(),
        groups: groups
            .iter()
            .filter(|g| same_error(g, project, fingerprint))
            .cloned()
            .collect(),
        ignored: ignored
            .iter()
            .find(|g| same_error(g, project, fingerprint))
            .cloned(),
        after: 0,
    };
    snap.after = snap.total();
    snap
}

/// Ignore project `project`'s error `fingerprint`: its groups (one per
/// pane) leave `groups` for one record in `ignored`, their counts summed,
/// the newest's lines kept. `None` for a service having stopped (every
/// stop shares that fingerprint) or an error not there.
fn ignore_in(
    groups: &mut Vec<ErrorGroup>,
    ignored: &mut Vec<ErrorGroup>,
    project: &str,
    fingerprint: &str,
) -> Option<ErrorsSnapshot> {
    if fingerprint == STOPPED {
        return None;
    }
    let before = snapshot(groups, ignored, project, fingerprint);
    let newest = before.groups.iter().max_by_key(|g| g.last_secs)?;
    let mut record = before.ignored.clone().unwrap_or_else(|| ErrorGroup {
        count: 0,
        first_secs: newest.first_secs,
        ..newest.clone()
    });
    if record.last_secs <= newest.last_secs {
        take_last(&mut record, newest);
    }
    record.count += before.groups.iter().map(|g| g.count).sum::<u32>();
    record.first_secs = before
        .groups
        .iter()
        .map(|g| g.first_secs)
        .fold(record.first_secs, u64::min);
    record.seen = true;
    groups.retain(|g| !same_error(g, project, fingerprint));
    ignored.retain(|g| !same_error(g, project, fingerprint));
    insert_newest_first(ignored, record, MAX_IGNORED);
    Some(before)
}

/// Stop ignoring project `project`'s error `fingerprint`: its record goes
/// back among `groups`, as looked at. `None` if it wasn't ignored.
fn unignore_in(
    groups: &mut Vec<ErrorGroup>,
    ignored: &mut Vec<ErrorGroup>,
    project: &str,
    fingerprint: &str,
) -> Option<ErrorsSnapshot> {
    let before = snapshot(groups, ignored, project, fingerprint);
    let at = ignored
        .iter()
        .position(|g| same_error(g, project, fingerprint))?;
    let mut record = ignored.remove(at);
    record.seen = true;
    // A group of the same pane can't be there: ignored errors make none.
    groups.retain(|g| !(same_error(g, project, fingerprint) && g.pane_id == record.pane_id));
    insert_newest_first(groups, record, MAX_GROUPS);
    Some(before)
}

/// Forget project `project`'s ignored error `fingerprint`: its record
/// leaves `ignored`, so the next time it comes it is told of as new.
/// `None` if it wasn't ignored.
fn forget_in(
    groups: &[ErrorGroup],
    ignored: &mut Vec<ErrorGroup>,
    project: &str,
    fingerprint: &str,
) -> Option<ErrorsSnapshot> {
    let mut before = snapshot(groups, ignored, project, fingerprint);
    before.after -= before.ignored.as_ref()?.count;
    ignored.retain(|g| !same_error(g, project, fingerprint));
    Some(before)
}

/// Put project `snap.project`'s error `snap.fingerprint` back in `groups`
/// and `ignored` as `snap` holds it, with what came of it since: counted
/// on its record when that is put back, else on its group of the pane it
/// last came in (a new one if none), as not looked at.
fn restore_in(groups: &mut Vec<ErrorGroup>, ignored: &mut Vec<ErrorGroup>, snap: ErrorsSnapshot) {
    let (project, fingerprint) = (snap.project.clone(), snap.fingerprint.clone());
    let now = snapshot(groups, ignored, &project, &fingerprint);
    let came = now.total().saturating_sub(snap.after);
    let newest = (now.groups.iter())
        .chain(&now.ignored)
        .max_by_key(|g| g.last_secs)
        .cloned()
        .filter(|_| came > 0);
    groups.retain(|g| !same_error(g, &project, &fingerprint));
    ignored.retain(|g| !same_error(g, &project, &fingerprint));
    let ErrorsSnapshot {
        groups: mut kept,
        ignored: mut record,
        ..
    } = snap;
    if let Some(newest) = newest {
        match record.as_mut() {
            Some(record) => {
                record.count += came;
                if record.last_secs <= newest.last_secs {
                    take_last(record, &newest);
                }
            }
            None => match kept.iter_mut().find(|g| g.pane_id == newest.pane_id) {
                Some(group) => {
                    group.count += came;
                    take_last(group, &newest);
                    group.seen = false;
                }
                None => kept.push(ErrorGroup {
                    count: came,
                    first_secs: newest.last_secs,
                    seen: false,
                    ..newest
                }),
            },
        }
    }
    for group in kept {
        insert_newest_first(groups, group, MAX_GROUPS);
    }
    if let Some(record) = record {
        insert_newest_first(ignored, record, MAX_IGNORED);
    }
}

/// Take `newest`'s last time (when, where, what) onto `g`.
fn take_last(g: &mut ErrorGroup, newest: &ErrorGroup) {
    g.pane_id = newest.pane_id.clone();
    g.title = newest.title.clone();
    g.lines = newest.lines.clone();
    g.service = newest.service.clone();
    g.last_secs = newest.last_secs;
    g.pane_gone = newest.pane_gone;
}

/// Put `group` into `groups` (the newest first) where its last time goes,
/// keeping at most `max`.
fn insert_newest_first(groups: &mut Vec<ErrorGroup>, group: ErrorGroup, max: usize) {
    let at = groups
        .iter()
        .position(|g| g.last_secs <= group.last_secs)
        .unwrap_or(groups.len());
    groups.insert(at, group);
    groups.truncate(max);
}

/// Mark the records in `ignored` of project `project` whose pane `closed`
/// says is closed as such; whether one was not yet.
fn mark_panes_gone(
    ignored: &mut [ErrorGroup],
    project: &str,
    closed: impl Fn(&str) -> bool,
) -> bool {
    let mut changed = false;
    for record in ignored
        .iter_mut()
        .filter(|g| g.project == project && !g.pane_gone && closed(&g.pane_id))
    {
        record.pane_gone = true;
        changed = true;
    }
    changed
}

/// Count `error` of project `project`'s pane `pane_id` (service `label`)
/// on its record in `ignored`, at `now`, the record then first: whether
/// it is ignored.
fn record_ignored(
    ignored: &mut Vec<ErrorGroup>,
    project: &str,
    pane_id: &str,
    label: &str,
    error: &ServiceError,
    now: u64,
) -> bool {
    let Some(at) = ignored
        .iter()
        .position(|g| same_error(g, project, &error.fingerprint))
    else {
        return false;
    };
    let mut record = ignored.remove(at);
    record.count += 1;
    record.pane_id = pane_id.to_owned();
    record.title = error.title.clone();
    record.lines = error.lines.clone();
    record.service = label.to_owned();
    record.last_secs = now;
    record.pane_gone = false;
    ignored.insert(0, record);
    true
}

impl Gerdr {
    /// Ignore project `project`'s error `fingerprint` (see `ignore_in`),
    /// saved; what an Undo puts back.
    pub fn ignore_service_error(
        &mut self,
        project: &str,
        fingerprint: &str,
    ) -> Option<ErrorsSnapshot> {
        let store = &mut self.projects;
        let before = ignore_in(
            &mut store.service_errors,
            &mut store.ignored_errors,
            project,
            fingerprint,
        )?;
        store.save();
        Some(before)
    }

    /// Stop ignoring project `project`'s error `fingerprint` (see
    /// `unignore_in`), saved; what an Undo puts back.
    pub fn unignore_service_error(
        &mut self,
        project: &str,
        fingerprint: &str,
    ) -> Option<ErrorsSnapshot> {
        let store = &mut self.projects;
        let before = unignore_in(
            &mut store.service_errors,
            &mut store.ignored_errors,
            project,
            fingerprint,
        )?;
        store.save();
        Some(before)
    }

    /// Forget project `project`'s ignored error `fingerprint` (see
    /// `forget_in`), saved; what an Undo puts back.
    pub fn forget_ignored_error(
        &mut self,
        project: &str,
        fingerprint: &str,
    ) -> Option<ErrorsSnapshot> {
        let store = &mut self.projects;
        let before = forget_in(
            &store.service_errors,
            &mut store.ignored_errors,
            project,
            fingerprint,
        )?;
        store.save();
        Some(before)
    }

    /// Put an error back as `snap` holds it, with what came of it since:
    /// an Undo of ignoring it, of ignoring it no more, or of forgetting it.
    pub fn restore_service_errors(&mut self, snap: ErrorsSnapshot) {
        let store = &mut self.projects;
        restore_in(&mut store.service_errors, &mut store.ignored_errors, snap);
        store.save();
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
            pane_gone: false,
        };
        let text = serde_json::to_string(&g).unwrap();
        // Not written while its pane is there.
        assert!(!text.contains("pane_gone"));
        assert_eq!(serde_json::from_str::<ErrorGroup>(&text).unwrap(), g);
        assert!(g.is_stop());
        let gone = ErrorGroup {
            pane_gone: true,
            ..g
        };
        let text = serde_json::to_string(&gone).unwrap();
        assert_eq!(serde_json::from_str::<ErrorGroup>(&text).unwrap(), gone);
    }

    fn group(pane: &str, fingerprint: &str, count: u32, first: u64, last: u64) -> ErrorGroup {
        ErrorGroup {
            project: "api".into(),
            pane_id: pane.into(),
            fingerprint: fingerprint.into(),
            title: format!("ValueError {pane}"),
            lines: vec![format!("line {pane}")],
            service: "runserver".into(),
            count,
            first_secs: first,
            last_secs: last,
            seen: false,
            pane_gone: false,
        }
    }

    #[test]
    fn ignoring_merges_the_panes_into_one_record() {
        let mut groups = vec![
            group("w1:p1", "f", 3, 10, 50),
            group("w1:p1", "g", 1, 20, 40),
            group("w2:p1", "f", 2, 5, 30),
        ];
        let mut ignored = Vec::new();
        let before = ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].fingerprint, "g");
        assert_eq!(ignored.len(), 1);
        let record = &ignored[0];
        assert_eq!(
            (record.count, record.first_secs, record.last_secs),
            (5, 5, 50)
        );
        assert_eq!(record.pane_id, "w1:p1");
        assert_eq!(record.lines, vec!["line w1:p1".to_owned()]);
        assert!(record.seen);
        assert_eq!(before.groups.len(), 2);
        assert!(before.ignored.is_none());
        // Another project's same fingerprint stays.
        let mut other = vec![ErrorGroup {
            project: "web".into(),
            ..group("w1:p1", "f", 1, 1, 1)
        }];
        assert!(ignore_in(&mut other, &mut ignored, "api", "f").is_none());
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn a_stop_cant_be_ignored() {
        let mut groups = vec![group("w1:p1", STOPPED, 1, 1, 1)];
        let mut ignored = Vec::new();
        assert!(ignore_in(&mut groups, &mut ignored, "api", STOPPED).is_none());
        assert_eq!(groups.len(), 1);
        assert!(ignored.is_empty());
    }

    #[test]
    fn an_ignored_error_counts_on_its_record() {
        let mut groups = vec![group("w1:p1", "f", 1, 1, 1)];
        let mut ignored = Vec::new();
        ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        let error = ServiceError {
            fingerprint: "f".into(),
            title: "ValueError: tool 3".into(),
            lines: vec!["new".into()],
        };
        assert!(record_ignored(
            &mut ignored,
            "api",
            "w9:p1",
            "dev",
            &error,
            99
        ));
        assert_eq!(ignored[0].count, 2);
        assert_eq!(ignored[0].pane_id, "w9:p1");
        assert_eq!(ignored[0].title, "ValueError: tool 3");
        assert_eq!(ignored[0].last_secs, 99);
        let other = ServiceError {
            fingerprint: "g".into(),
            ..error
        };
        assert!(!record_ignored(
            &mut ignored,
            "api",
            "w9:p1",
            "dev",
            &other,
            99
        ));
    }

    #[test]
    fn ignored_no_more_it_lists_again_looked_at() {
        let mut groups = vec![
            group("w1:p1", "g", 1, 1, 100),
            group("w1:p1", "f", 1, 1, 10),
        ];
        let mut ignored = Vec::new();
        ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        let before = unignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        assert!(ignored.is_empty());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].fingerprint, "f");
        assert!(groups[1].seen);
        assert!(before.ignored.is_some());
        assert!(unignore_in(&mut groups, &mut ignored, "api", "f").is_none());
    }

    fn error(fingerprint: &str, line: &str) -> ServiceError {
        ServiceError {
            fingerprint: fingerprint.into(),
            title: format!("ValueError: {line}"),
            lines: vec![line.into()],
        }
    }

    #[test]
    fn undo_of_ignoring_keeps_what_came_since() {
        let mut groups = vec![
            group("w1:p1", "f", 3, 10, 50),
            group("w2:p1", "f", 2, 5, 30),
        ];
        let mut ignored = Vec::new();
        let before = ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        // Twice more while ignored, in the second pane.
        for at in [60, 70] {
            assert!(record_ignored(
                &mut ignored,
                "api",
                "w2:p1",
                "dev",
                &error("f", "late"),
                at
            ));
        }
        restore_in(&mut groups, &mut ignored, before);
        assert!(ignored.is_empty());
        assert_eq!(groups.len(), 2);
        // Counted on the pane it came in, as not looked at, the newest now.
        let late = &groups[0];
        assert_eq!(late.pane_id, "w2:p1");
        assert_eq!((late.count, late.first_secs, late.last_secs), (4, 5, 70));
        assert_eq!(late.lines, vec!["late".to_owned()]);
        assert!(!late.seen);
        assert_eq!((groups[1].pane_id.as_str(), groups[1].count), ("w1:p1", 3));

        // Come in a pane it had no group of: a group of its own.
        let mut groups = vec![group("w1:p1", "f", 3, 10, 50)];
        let before = ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        record_ignored(&mut ignored, "api", "w9:p1", "dev", &error("f", "new"), 80);
        restore_in(&mut groups, &mut ignored, before);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            (
                groups[0].pane_id.as_str(),
                groups[0].count,
                groups[0].first_secs
            ),
            ("w9:p1", 1, 80)
        );
        assert_eq!(groups[1].count, 3);
    }

    #[test]
    fn undo_of_ignoring_no_more_keeps_what_came_since() {
        let mut groups = vec![group("w1:p1", "f", 3, 10, 50)];
        let mut ignored = Vec::new();
        ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        let before = unignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        // Since: once more in its pane, once in another (told of, a
        // group of its own).
        groups[0].count += 1;
        groups[0].last_secs = 60;
        groups.insert(
            0,
            ErrorGroup {
                lines: vec!["other".into()],
                ..group("w2:p1", "f", 1, 90, 90)
            },
        );
        restore_in(&mut groups, &mut ignored, before);
        assert!(groups.is_empty());
        assert_eq!(ignored.len(), 1);
        let record = &ignored[0];
        assert_eq!((record.count, record.last_secs), (5, 90));
        assert_eq!(record.pane_id, "w2:p1");
        assert_eq!(record.lines, vec!["other".to_owned()]);
    }

    #[test]
    fn a_forgotten_record_comes_back_with_an_undo() {
        let mut groups = vec![group("w1:p1", "f", 3, 10, 50)];
        let mut ignored = Vec::new();
        ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        let kept = ignored.clone();
        let before = forget_in(&groups, &mut ignored, "api", "f").unwrap();
        assert!(ignored.is_empty());
        assert!(forget_in(&groups, &mut ignored, "api", "f").is_none());
        restore_in(&mut groups, &mut ignored, before);
        assert_eq!(ignored, kept);
        // One that came while forgotten made a group: back on the record.
        let before = forget_in(&groups, &mut ignored, "api", "f").unwrap();
        groups.push(group("w1:p1", "f", 1, 70, 70));
        restore_in(&mut groups, &mut ignored, before);
        assert!(groups.is_empty());
        assert_eq!((ignored[0].count, ignored[0].last_secs), (4, 70));
    }

    #[test]
    fn ignored_records_are_kept_the_last_to_come_first_and_capped() {
        let mut groups = Vec::new();
        let mut ignored = Vec::new();
        for k in 0..MAX_IGNORED as u64 + 5 {
            let fingerprint = format!("f{k}");
            groups.push(group("w1:p1", &fingerprint, 1, k, k));
            ignore_in(&mut groups, &mut ignored, "api", &fingerprint).unwrap();
        }
        assert_eq!(ignored.len(), MAX_IGNORED);
        assert_eq!(ignored[0].fingerprint, format!("f{}", MAX_IGNORED + 4));
        // The oldest went.
        assert!(ignored.iter().all(|g| g.fingerprint != "f0"));
        // One coming again goes first.
        let last = ignored.last().unwrap().fingerprint.clone();
        record_ignored(&mut ignored, "api", "w1:p1", "dev", &error(&last, "x"), 999);
        assert_eq!(ignored[0].fingerprint, last);
    }

    #[test]
    fn a_closed_panes_record_stays_marked_until_it_comes_again() {
        let mut groups = vec![group("w1:p1", "f", 1, 1, 1), group("w2:p1", "g", 1, 1, 1)];
        let mut ignored = Vec::new();
        ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        ignore_in(&mut groups, &mut ignored, "api", "g").unwrap();
        assert!(mark_panes_gone(&mut ignored, "api", |p| p == "w1:p1"));
        // Once only; another project's untouched.
        assert!(!mark_panes_gone(&mut ignored, "api", |p| p == "w1:p1"));
        assert!(!mark_panes_gone(&mut ignored, "web", |_| true));
        let gone = |ignored: &[ErrorGroup], f: &str| {
            ignored
                .iter()
                .find(|g| g.fingerprint == f)
                .unwrap()
                .pane_gone
        };
        assert!(gone(&ignored, "f"));
        assert!(!gone(&ignored, "g"));
        record_ignored(&mut ignored, "api", "w3:p1", "dev", &error("f", "x"), 9);
        assert!(!gone(&ignored, "f"));
    }

    #[test]
    fn undo_puts_both_lists_back() {
        let start = vec![
            group("w1:p1", "f", 3, 10, 50),
            group("w1:p1", "g", 1, 20, 40),
            group("w2:p1", "f", 2, 5, 30),
        ];
        let mut groups = start.clone();
        let mut ignored = Vec::new();
        let before = ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        restore_in(&mut groups, &mut ignored, before);
        assert_eq!(groups, start);
        assert!(ignored.is_empty());

        ignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        let (kept_groups, kept_ignored) = (groups.clone(), ignored.clone());
        let before = unignore_in(&mut groups, &mut ignored, "api", "f").unwrap();
        restore_in(&mut groups, &mut ignored, before);
        assert_eq!((groups, ignored), (kept_groups, kept_ignored));
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
