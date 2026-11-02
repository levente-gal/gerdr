//! Watching the services' panes of a herdr session for errors, and for
//! their command stopping, on a thread of its own so reading their output
//! never holds up the compositor.
//!
//! Each service pane gets a herdr connection of its own ringing when an
//! error line comes into its last `WINDOW` lines (`pane.output_matched`).
//! That is a doorbell only: herdr rings once as the first error comes in,
//! then not again until the window is clear of them, and says nothing of
//! which is new. So a ring reads the pane's last lines and takes the
//! errors past where the last read left off (its `Anchor`); while the
//! window still holds an error, the pane is read every `POLL` as well.
//!
//! herdr matches the shell's line of the command too. A pane is watched
//! only once its command's line has left the window (read every `POLL`
//! until then), and that line is never taken for an error.
//!
//! Every `POLL` the thread also asks what runs in each pane: its command
//! gone back to the shell is a service stopped (herdr has no exit status
//! to tell a crash from a Ctrl+C: gerdr tells them apart by the keys it
//! sent), and back again, running.

use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{
    herdr::{Herdr, OutputWatch},
    projects::{self, Foreground},
    service_errors::{self, Anchor, Patterns, ServiceError},
};

/// How many of a pane's last lines herdr watches for errors.
pub const WINDOW: usize = 20;
/// How many lines a read takes: all `recent` gives.
const READ_LINES: usize = 1000;
/// How many of its last lines a service stopped keeps.
pub const STOP_LINES: usize = 40;
/// How often a pane herdr can't ring for is read, and every pane is
/// asked what runs in it.
const POLL: Duration = Duration::from_secs(5);
/// How long before a connection that failed is opened again.
const RETRY: Duration = Duration::from_secs(10);
/// How long the thread waits for a ring at most before looking at its
/// list again.
const TICK: Duration = Duration::from_millis(500);

/// A service to watch: its pane, and the command it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub pane_id: String,
    pub command: String,
}

/// What the thread found in a service's pane.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// A read moved on: the new errors in it (in order), and where it left
    /// off.
    Read {
        pane_id: String,
        errors: Vec<ServiceError>,
        anchor: Anchor,
    },
    /// Its command went back to the shell, leaving these last lines, the
    /// last error among them if any; `interrupted` by a Ctrl+C the
    /// terminal echoed.
    Stopped {
        pane_id: String,
        lines: Vec<String>,
        error: Option<ServiceError>,
        interrupted: bool,
    },
    /// Its command runs again.
    Started { pane_id: String },
}

struct Watch {
    command: String,
    anchor: Option<Anchor>,
    doorbell: Option<OutputWatch>,
    /// When the pane is next read, without a ring.
    next_read: Option<Instant>,
    /// When a doorbell may be opened again, after one failed.
    retry_at: Option<Instant>,
    /// Whether the pane has been read yet.
    read: bool,
    /// Whether the last read saw the command's line in the window.
    command_shown: bool,
    /// Whether its command ran when last asked; `None` before.
    running: Option<bool>,
    /// When it is next asked what runs in it.
    next_check: Instant,
}

/// Watch the services sent on the returned sender (the whole list, each
/// time it changes) of the herdr server on `socket` for lines `patterns`
/// takes for errors, starting from where the last gerdr left off
/// (`anchors`), and hand what it finds to `send`. Stops once `send`
/// returns false or the sender is dropped.
pub fn spawn(
    socket: PathBuf,
    patterns: Patterns,
    anchors: HashMap<String, Anchor>,
    mut send: impl FnMut(Report) -> bool + Send + 'static,
) -> mpsc::Sender<Vec<Service>> {
    let (tx, rx) = mpsc::channel::<Vec<Service>>();
    std::thread::spawn(move || {
        let mut herdr = Herdr::on_socket(socket);
        let mut saved = anchors;
        let mut watches: HashMap<String, Watch> = HashMap::new();
        loop {
            loop {
                match rx.try_recv() {
                    Ok(list) => update(&mut watches, &mut saved, list),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            let rung = wait(&mut watches, TICK);
            let now = Instant::now();
            let ids: Vec<String> = watches.keys().cloned().collect();
            for pane_id in ids {
                let Some(watch) = watches.get_mut(&pane_id) else {
                    continue;
                };
                let mut reports = Vec::new();
                if rung.contains(&pane_id) || watch.next_read.is_some_and(|at| at <= now) {
                    reports.extend(read(&mut herdr, &patterns, &pane_id, watch, now));
                }
                if watch.next_check <= now {
                    reports.extend(check(&mut herdr, &patterns, &pane_id, watch, now));
                }
                for report in reports {
                    if !send(report) {
                        return;
                    }
                }
            }
            for (pane_id, watch) in &mut watches {
                open_doorbell(&herdr, &patterns, pane_id, watch, now);
            }
        }
    });
    tx
}

/// Take `list` as the services to watch: new ones start from their saved
/// anchor, gone ones are let go.
fn update(
    watches: &mut HashMap<String, Watch>,
    saved: &mut HashMap<String, Anchor>,
    list: Vec<Service>,
) {
    watches.retain(|id, _| list.iter().any(|s| s.pane_id == *id));
    let now = Instant::now();
    for service in list {
        match watches.get_mut(&service.pane_id) {
            Some(watch) => watch.command = service.command,
            None => {
                let anchor = saved.remove(&service.pane_id);
                watches.insert(
                    service.pane_id,
                    Watch {
                        command: service.command,
                        anchor,
                        doorbell: None,
                        next_read: Some(now),
                        retry_at: None,
                        read: false,
                        command_shown: true,
                        running: None,
                        next_check: now,
                    },
                );
            }
        }
    }
}

/// Wait up to `timeout` for doorbells to ring; the panes that rang. One
/// gone is dropped, to be opened again.
fn wait(watches: &mut HashMap<String, Watch>, timeout: Duration) -> Vec<String> {
    let ids: Vec<String> = watches
        .iter()
        .filter(|(_, w)| w.doorbell.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    let mut fds: Vec<libc::pollfd> = ids
        .iter()
        .map(|id| libc::pollfd {
            fd: watches[id]
                .doorbell
                .as_ref()
                .map_or(-1, |d| d.as_fd().as_raw_fd()),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let ms = timeout.as_millis() as libc::c_int;
    // Safety: `fds` is a live array of `fds.len()` pollfds.
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if ready <= 0 {
        return Vec::new();
    }
    let mut rung = Vec::new();
    for (id, fd) in ids.iter().zip(&fds) {
        if fd.revents == 0 {
            continue;
        }
        let watch = watches.get_mut(id).unwrap();
        match watch.doorbell.as_mut().map(OutputWatch::read) {
            Some(Ok(0)) | None => {}
            Some(Ok(_)) => rung.push(id.clone()),
            Some(Err(err)) => {
                tracing::debug!("output watch of {id}: {err}");
                watch.doorbell = None;
                watch.retry_at = Some(Instant::now() + RETRY);
                watch.next_read.get_or_insert_with(Instant::now);
            }
        }
    }
    rung
}

/// Read pane `pane_id`'s last lines for errors past its anchor. The first
/// read with no anchor saved takes what is there as seen.
fn read(
    herdr: &mut Herdr,
    patterns: &Patterns,
    pane_id: &str,
    watch: &mut Watch,
    now: Instant,
) -> Option<Report> {
    let text = match herdr.pane_read(pane_id, "recent_unwrapped", READ_LINES) {
        Ok(text) => text,
        Err(err) => {
            tracing::debug!("reading {pane_id} for errors: {err}");
            watch.next_read = Some(now + POLL);
            return None;
        }
    };
    read_text(patterns, &text, pane_id, watch, now)
}

fn read_text(
    patterns: &Patterns,
    text: &str,
    pane_id: &str,
    watch: &mut Watch,
    now: Instant,
) -> Option<Report> {
    let scan = service_errors::scan(
        patterns,
        text,
        &watch.command,
        watch.anchor.as_ref(),
        WINDOW,
    );
    let first = !watch.read;
    watch.read = true;
    watch.command_shown = scan.command_shown;
    // Read again while herdr can't ring: no doorbell is open yet, or the
    // window holds an error (or the command's line).
    let ringable = watch.doorbell.is_some() && !scan.dirty && !scan.command_shown;
    watch.next_read = (!ringable).then_some(now + POLL);
    let anchor = scan.anchor?;
    let moved = watch.anchor != Some(anchor);
    let fresh = first && watch.anchor.is_none();
    watch.anchor = Some(anchor);
    let errors = if fresh { Vec::new() } else { scan.errors };
    (moved || !errors.is_empty()).then(|| Report::Read {
        pane_id: pane_id.to_owned(),
        errors,
        anchor,
    })
}

/// Ask what runs in pane `pane_id`: its command gone (back to the shell,
/// or another in its place) since last asked is a service stopped (its errors read first, and its
/// last lines kept), back again one started.
fn check(
    herdr: &mut Herdr,
    patterns: &Patterns,
    pane_id: &str,
    watch: &mut Watch,
    now: Instant,
) -> Vec<Report> {
    watch.next_check = now + POLL;
    let running = match herdr.process_info(pane_id) {
        // Its own command, not another typed in after it stopped; one
        // whose command line can't be read back (gunicorn's title) counts.
        Ok(info) => match projects::foreground_command(&info, None) {
            Some(Foreground::Idle | Foreground::Untracked { denied: true, .. }) => false,
            Some(Foreground::Busy { captured, .. }) => captured.command == watch.command,
            Some(Foreground::Untracked { denied: false, .. }) => true,
            None => return Vec::new(),
        },
        Err(err) => {
            tracing::debug!("asking what runs in {pane_id}: {err}");
            return Vec::new();
        }
    };
    let was = watch.running.replace(running);
    match (was, running) {
        (Some(true), false) => {
            let text = herdr
                .pane_read(pane_id, "recent_unwrapped", READ_LINES)
                .unwrap_or_default();
            let mut reports: Vec<Report> = read_text(patterns, &text, pane_id, watch, now)
                .into_iter()
                .collect();
            let ending = service_errors::ending(patterns, &text, &watch.command, STOP_LINES);
            reports.push(Report::Stopped {
                pane_id: pane_id.to_owned(),
                lines: ending.lines,
                error: ending.error,
                interrupted: ending.interrupted,
            });
            reports
        }
        (Some(false), true) => vec![Report::Started {
            pane_id: pane_id.to_owned(),
        }],
        _ => Vec::new(),
    }
}

/// Open pane `pane_id`'s doorbell if it has none, once its command's line
/// has left the window (so herdr's first ring isn't the command).
fn open_doorbell(
    herdr: &Herdr,
    patterns: &Patterns,
    pane_id: &str,
    watch: &mut Watch,
    now: Instant,
) {
    if watch.doorbell.is_some()
        || !watch.read
        || watch.command_shown
        || watch.retry_at.is_some_and(|at| at > now)
    {
        return;
    }
    match herdr.watch_output(pane_id, WINDOW, patterns.herdr()) {
        Ok(doorbell) => {
            tracing::debug!("watching {pane_id} for errors");
            watch.doorbell = Some(doorbell);
            watch.retry_at = None;
        }
        Err(err) => {
            tracing::debug!("output watch of {pane_id}: {err}");
            watch.retry_at = Some(now + RETRY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What arrives on `rx` within `wait`, up to the first report `stop`
    /// takes.
    fn until(
        rx: &mpsc::Receiver<Report>,
        wait: Duration,
        stop: impl Fn(&Report) -> bool,
    ) -> Option<Report> {
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(r) if stop(&r) => return Some(r),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        None
    }

    fn titles(report: Option<Report>) -> Vec<String> {
        match report {
            Some(Report::Read { errors, .. }) => errors.into_iter().map(|e| e.title).collect(),
            _ => Vec::new(),
        }
    }

    fn has_errors(r: &Report) -> bool {
        matches!(r, Report::Read { errors, .. } if !errors.is_empty())
    }

    /// Against a scratch herdr server: `GERDR_PROBE_SOCKET` its socket,
    /// `GERDR_PROBE_PANE` a pane of it at a shell prompt.
    #[test]
    #[ignore]
    fn probe_live_server() {
        let socket = PathBuf::from(std::env::var("GERDR_PROBE_SOCKET").unwrap());
        let pane = std::env::var("GERDR_PROBE_PANE").unwrap();
        let mut herdr = Herdr::on_socket(socket.clone());
        herdr.send_line(&pane, "printf 'start%s\\n' ed").unwrap();
        let command = "sleep 1000";
        herdr.send_line(&pane, command).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let (tx, rx) = mpsc::channel();
        let feed = spawn(socket, Patterns::default(), HashMap::new(), move |r| {
            tx.send(r).is_ok()
        });
        feed.send(vec![Service {
            pane_id: pane.clone(),
            command: command.into(),
        }])
        .unwrap();
        // The first read takes what is there as seen.
        assert!(until(&rx, Duration::from_secs(2), has_errors).is_none());
        // Stopping it is told.
        herdr.send_keys(&pane, &["ctrl+c"]).unwrap();
        let stopped = until(&rx, Duration::from_secs(7), |r| {
            matches!(r, Report::Stopped { .. })
        });
        eprintln!("stopped: {stopped:?}");
        // The terminal echoed the Ctrl+C: stopped on purpose.
        assert!(matches!(
            stopped,
            Some(Report::Stopped {
                interrupted: true,
                ..
            })
        ));
        // 30 clean lines push the command's line out of the window.
        herdr
            .send_line(&pane, "for i in $(seq 30); do echo clean $i; done")
            .unwrap();
        assert!(until(&rx, Duration::from_secs(7), has_errors).is_none());
        // The doorbell rings at once.
        let at = Instant::now();
        herdr.send_line(&pane, "printf 'E%sOR one\\n' RR").unwrap();
        let found = titles(until(&rx, Duration::from_secs(2), has_errors));
        eprintln!("one: {found:?} after {:?}", at.elapsed());
        assert_eq!(found, ["ERROR one"]);
        // Another while the window holds the first: read by the poll.
        let at = Instant::now();
        herdr.send_line(&pane, "printf 'E%sOR two\\n' RR").unwrap();
        let found = titles(until(&rx, Duration::from_secs(7), has_errors));
        eprintln!("two: {found:?} after {:?}", at.elapsed());
        assert_eq!(found, ["ERROR two"]);
        // Clear, then a traceback.
        herdr
            .send_line(&pane, "for i in $(seq 30); do echo clean $i; done")
            .unwrap();
        let _ = until(&rx, Duration::from_secs(7), has_errors);
        let at = Instant::now();
        herdr
            .send_line(
                &pane,
                "printf 'Trace%s (most recent call last):\\n  File \"a.py\", line 1, in f\\nValueError: bad\\n' back",
            )
            .unwrap();
        let found = titles(until(&rx, Duration::from_secs(7), has_errors));
        eprintln!("traceback: {found:?} after {:?}", at.elapsed());
        assert_eq!(found, ["ValueError: bad"]);
        assert!(at.elapsed() < Duration::from_secs(2), "rung, not polled");
        // Running again is told.
        herdr.send_line(&pane, "sleep 1000").unwrap();
        let started = until(&rx, Duration::from_secs(7), |r| {
            matches!(r, Report::Started { .. })
        });
        assert!(started.is_some());
        herdr.send_keys(&pane, &["ctrl+c"]).unwrap();
        // One that ends by itself, its error printed: not on purpose.
        feed.send(vec![Service {
            pane_id: pane.clone(),
            command: "sleep 6".into(),
        }])
        .unwrap();
        herdr
            .send_line(&pane, "printf 'E%sOR crashing\\n' RR; sleep 6")
            .unwrap();
        let stopped = until(&rx, Duration::from_secs(20), |r| {
            matches!(r, Report::Stopped { .. })
        });
        eprintln!("crashed: {stopped:?}");
        let Some(Report::Stopped {
            interrupted, error, ..
        }) = stopped
        else {
            panic!("no stop");
        };
        assert!(!interrupted);
        assert_eq!(error.unwrap().title, "ERROR crashing");
    }
}
