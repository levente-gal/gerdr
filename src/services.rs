//! The errors of the services running in the panes (web servers, workers),
//! as gerdr shows them: a desktop notification as each comes, an inbox
//! item per error (the same one again counted on it), a mark on its
//! workspace in the sidebar until it is looked at. `service_watch` finds
//! them; this keeps them and wires them into gerdr.

use std::{collections::HashSet, path::PathBuf, sync::mpsc, time::Instant};

use smithay::reexports::calloop::{RegistrationToken, channel};

use crate::{
    Gerdr,
    chrome::BarStatus,
    inbox_store,
    service_errors::ServiceError,
    service_watch::{self, Report, Service},
};

/// The prefix of a notification's key for a service's error:
/// `service:<pane id>|<fingerprint>`.
pub const NOTIFY_PREFIX: &str = "service:";

/// The fingerprint of a service having stopped.
const STOPPED: &str = "stopped";

/// An error a service printed (or its stopping), and how many times since.
#[derive(Clone, Debug, PartialEq)]
pub struct ErrorGroup {
    pub pane_id: String,
    pub fingerprint: String,
    pub title: String,
    /// All of its last time, as printed.
    pub lines: Vec<String>,
    pub count: u32,
    /// When it last came.
    pub last: Instant,
    /// When it first came, in seconds since the epoch.
    pub first_secs: u64,
    /// Whether it was looked at since it last came.
    pub seen: bool,
}

/// What gerdr keeps of the services' errors.
#[derive(Default)]
pub struct ServiceErrors {
    pub groups: Vec<ErrorGroup>,
    /// The watcher thread's list of services, while one runs.
    watch: Option<mpsc::Sender<Vec<Service>>>,
    /// The list last sent to it.
    sent: Vec<Service>,
    /// The herdr session watched: what saved anchors are kept by.
    session: String,
    /// The service panes whose command ran at the last round.
    running: HashSet<String>,
    source: Option<RegistrationToken>,
}

impl ServiceErrors {
    /// The errors of herdr pane `pane_id`, the newest first.
    pub fn of_pane(&self, pane_id: &str) -> impl Iterator<Item = &ErrorGroup> {
        self.groups.iter().filter(move |g| g.pane_id == pane_id)
    }

    /// How many errors not looked at the panes `panes` have.
    pub fn unseen_in<'a>(&self, panes: impl Iterator<Item = &'a str>) -> usize {
        panes
            .map(|p| self.of_pane(p).filter(|g| !g.seen).count())
            .sum()
    }
}

/// The key of pane `pane_id` of herdr session `session` among the saved
/// anchors.
fn anchor_key(session: &str, pane_id: &str) -> String {
    format!("{session}|{pane_id}")
}

impl Gerdr {
    /// Start watching the services of herdr session `session` on `socket`
    /// for errors. A server just started (`fresh`) has new panes under the
    /// old ids: what was saved of them is let go. Returns the event loop
    /// source.
    pub fn watch_services(
        &mut self,
        socket: PathBuf,
        session: &str,
        fresh: bool,
    ) -> Option<RegistrationToken> {
        self.stop_service_watch();
        let prefix = anchor_key(session, "");
        if fresh {
            let before = self.projects.service_anchors.len();
            self.projects
                .service_anchors
                .retain(|key, _| !key.starts_with(&prefix));
            if self.projects.service_anchors.len() != before {
                self.projects.save();
            }
        }
        let anchors = self
            .projects
            .service_anchors
            .iter()
            .filter_map(|(key, a)| Some((key.strip_prefix(&prefix)?.to_owned(), *a)))
            .collect();
        let (tx, rx) = channel::channel::<Report>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(report) = event {
                state.on_service_report(report);
            }
        });
        let token = match inserted {
            Ok(token) => token,
            Err(err) => {
                tracing::warn!("service errors: {err}");
                return None;
            }
        };
        let watch = service_watch::spawn(socket, anchors, move |report| tx.send(report).is_ok());
        self.service_errors.watch = Some(watch);
        self.service_errors.session = session.to_owned();
        self.service_errors.source = Some(token);
        self.feed_services();
        Some(token)
    }

    /// Stop watching the services; their errors are let go.
    pub fn stop_service_watch(&mut self) {
        let errors = &mut self.service_errors;
        errors.watch = None;
        errors.sent.clear();
        errors.groups.clear();
        errors.running.clear();
        if let Some(token) = errors.source.take() {
            self.loop_handle.remove(token);
        }
    }

    /// Tell the watcher which panes run services now: those that ran long
    /// enough, or that gerdr runs again (see `Remembered::long`).
    pub fn feed_services(&mut self) {
        if self.service_errors.watch.is_none() {
            return;
        }
        self.note_stopped_services();
        let Some(watch) = &self.service_errors.watch else {
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
        list.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
        if list == self.service_errors.sent {
            return;
        }
        if watch.send(list.clone()).is_err() {
            self.service_errors.watch = None;
        }
        self.service_errors.sent = list;
        let live: Vec<&str> = self
            .service_errors
            .sent
            .iter()
            .map(|s| s.pane_id.as_str())
            .collect();
        self.service_errors
            .groups
            .retain(|g| live.contains(&g.pane_id.as_str()));
    }

    /// A read of a service's pane: keep where it left off, and count and
    /// tell of the errors it found.
    fn on_service_report(&mut self, report: Report) {
        let key = anchor_key(&self.service_errors.session, &report.pane_id);
        self.projects.service_anchors.insert(key, report.anchor);
        if report.errors.is_empty() {
            return;
        }
        let label = self.pane_bar_content(&report.pane_id).label;
        for error in report.errors {
            self.add_service_error(&report.pane_id, &label, error);
        }
        // Saved with the errors: a restart takes up after them.
        self.projects.save();
    }

    /// Count `error` of herdr pane `pane_id` (service `label`) and tell of
    /// it: the same one again counts on the last.
    fn add_service_error(&mut self, pane_id: &str, label: &str, error: ServiceError) {
        let ServiceError {
            fingerprint,
            title,
            lines,
        } = error;
        let groups = &mut self.service_errors.groups;
        let mut group = match groups
            .iter()
            .position(|g| g.pane_id == pane_id && g.fingerprint == fingerprint)
        {
            Some(at) => {
                let mut group = groups.remove(at);
                group.count += 1;
                group
            }
            None => ErrorGroup {
                pane_id: pane_id.to_owned(),
                fingerprint: fingerprint.clone(),
                title: String::new(),
                lines: Vec::new(),
                count: 1,
                last: Instant::now(),
                first_secs: inbox_store::now_secs(),
                seen: false,
            },
        };
        group.title = title.clone();
        group.lines = lines;
        group.last = Instant::now();
        group.seen = false;
        let count = group.count;
        groups.insert(0, group);
        if let Some(notifier) = self.notifier.as_mut() {
            let key = format!("{NOTIFY_PREFIX}{pane_id}|{fingerprint}");
            let body = match count {
                1 => title,
                n => format!("{title} (×{n})"),
            };
            notifier.send(&key, &format!("{label}: error"), &body);
        }
    }

    /// Note the services that stopped since the last round, and forget
    /// that they did once they run again. One gerdr stops or restarts
    /// itself is no news.
    fn note_stopped_services(&mut self) {
        let services: Vec<String> = self
            .last_commands
            .iter()
            .filter(|(_, r)| r.long)
            .map(|(id, _)| id.clone())
            .collect();
        for pane_id in services {
            let content = self.pane_bar_content(&pane_id);
            let running = content.status == BarStatus::Running;
            let was = self.service_errors.running.contains(&pane_id);
            if running {
                self.service_errors.running.insert(pane_id.clone());
                self.service_errors
                    .groups
                    .retain(|g| !(g.pane_id == pane_id && g.fingerprint == STOPPED));
            } else if content.status == BarStatus::Exited {
                self.service_errors.running.remove(&pane_id);
                if was {
                    let error = ServiceError {
                        fingerprint: STOPPED.to_owned(),
                        title: format!("Stopped: {}", content.label),
                        lines: Vec::new(),
                    };
                    self.add_service_error(&pane_id, &content.label, error);
                }
            } else {
                // Being restarted by gerdr.
                self.service_errors.running.remove(&pane_id);
            }
        }
        let live = &self.last_commands;
        self.service_errors
            .running
            .retain(|id| live.contains_key(id));
    }

    /// Show the error `fingerprint` of herdr pane `pane_id`: its pane, its
    /// output open over it where the error is.
    pub fn show_service_error(&mut self, pane_id: &str, fingerprint: &str) {
        let Some(group) = self
            .service_errors
            .groups
            .iter_mut()
            .find(|g| g.pane_id == pane_id && g.fingerprint == fingerprint)
        else {
            return self.show_notice("That error is gone".into());
        };
        group.seen = true;
        let first = group.lines.first().cloned();
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

    /// A notification of a service's error was clicked: `key` without its
    /// prefix.
    pub fn service_notification_clicked(&mut self, key: &str) {
        if let Some((pane_id, fingerprint)) = key.split_once('|') {
            let (pane_id, fingerprint) = (pane_id.to_owned(), fingerprint.to_owned());
            self.show_service_error(&pane_id, &fingerprint);
        }
    }

    /// Mark herdr pane `pane_id`'s errors as looked at.
    pub fn see_service_errors(&mut self, pane_id: &str) {
        for group in &mut self.service_errors.groups {
            if group.pane_id == pane_id {
                group.seen = true;
            }
        }
    }

    /// Herdr pane `pane_id` is gone: so are its errors, and where its
    /// output was read to.
    pub fn forget_service_pane(&mut self, pane_id: &str) {
        self.service_errors.groups.retain(|g| g.pane_id != pane_id);
        let key = anchor_key(&self.service_errors.session, pane_id);
        self.projects.service_anchors.remove(&key);
    }
}
