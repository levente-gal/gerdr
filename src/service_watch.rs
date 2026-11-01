//! Watching the services' panes for errors, on a thread of its own so
//! reading their output never holds up the compositor.
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

use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{
    herdr::{Herdr, OutputWatch},
    service_errors::{self, Anchor, ServiceError},
};

/// How many of a pane's last lines herdr watches for errors.
pub const WINDOW: usize = 20;
/// How many lines a read takes: all `recent` gives.
const READ_LINES: usize = 1000;
/// How often a pane herdr can't ring for is read.
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

/// What a read of a service's pane found.
#[derive(Clone, Debug)]
pub struct Report {
    pub pane_id: String,
    /// The new errors, in order.
    pub errors: Vec<ServiceError>,
    /// Where the read left off.
    pub anchor: Anchor,
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
}

/// Watch the services sent on the returned sender (the whole list, each
/// time it changes) of the herdr server on `socket`, starting from where
/// the last gerdr left off (`anchors`), and hand each read that moved on
/// to `send`. Stops once `send` returns false or the sender is dropped.
pub fn spawn(
    socket: PathBuf,
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
            let due: Vec<String> = watches
                .iter()
                .filter(|(id, w)| rung.contains(id) || w.next_read.is_some_and(|at| at <= now))
                .map(|(id, _)| id.clone())
                .collect();
            for pane_id in due {
                let Some(watch) = watches.get_mut(&pane_id) else {
                    continue;
                };
                if let Some(report) = read(&mut herdr, &pane_id, watch, now)
                    && !send(report)
                {
                    return;
                }
            }
            for (pane_id, watch) in &mut watches {
                open_doorbell(&herdr, pane_id, watch, now);
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
                        next_read: Some(Instant::now()),
                        retry_at: None,
                        read: false,
                        command_shown: true,
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
fn read(herdr: &mut Herdr, pane_id: &str, watch: &mut Watch, now: Instant) -> Option<Report> {
    let text = match herdr.pane_read(pane_id, "recent_unwrapped", READ_LINES) {
        Ok(text) => text,
        Err(err) => {
            tracing::debug!("reading {pane_id} for errors: {err}");
            watch.next_read = Some(now + POLL);
            return None;
        }
    };
    let scan = service_errors::scan(&text, &watch.command, watch.anchor.as_ref(), WINDOW);
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
    (moved || !errors.is_empty()).then(|| Report {
        pane_id: pane_id.to_owned(),
        errors,
        anchor,
    })
}

/// Open pane `pane_id`'s doorbell if it has none, once its command's line
/// has left the window (so herdr's first ring isn't the command).
fn open_doorbell(herdr: &Herdr, pane_id: &str, watch: &mut Watch, now: Instant) {
    if watch.doorbell.is_some()
        || !watch.read
        || watch.command_shown
        || watch.retry_at.is_some_and(|at| at > now)
    {
        return;
    }
    match herdr.watch_output(pane_id, WINDOW, service_errors::HERDR_PATTERN) {
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

    /// Against a scratch herdr server: `GERDR_PROBE_SOCKET` its socket,
    /// `GERDR_PROBE_PANE` a pane of it at a shell prompt.
    #[test]
    #[ignore]
    fn probe_live_server() {
        let socket = PathBuf::from(std::env::var("GERDR_PROBE_SOCKET").unwrap());
        let pane = std::env::var("GERDR_PROBE_PANE").unwrap();
        let mut herdr = Herdr::on_socket(socket.clone());
        let command = "printf 'started\\n'; sleep 1000";
        herdr.send_line(&pane, command).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let (tx, rx) = mpsc::channel();
        let feed = spawn(socket, HashMap::new(), move |r| tx.send(r).is_ok());
        feed.send(vec![Service {
            pane_id: pane.clone(),
            command: command.into(),
        }])
        .unwrap();
        let errors = |rx: &mpsc::Receiver<Report>, wait: Duration| {
            let until = Instant::now() + wait;
            let mut found = Vec::new();
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(r) if !r.errors.is_empty() => {
                        found.extend(r.errors.into_iter().map(|e| e.title));
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            found
        };
        // The first read takes what is there as seen.
        assert!(errors(&rx, Duration::from_secs(2)).is_empty());
        let stop = |herdr: &mut Herdr| herdr.send_keys(&pane, &["ctrl+c"]).unwrap();
        stop(&mut herdr);
        // 30 clean lines push the command's line out of the window.
        herdr
            .send_line(&pane, "for i in $(seq 30); do echo clean $i; done")
            .unwrap();
        assert!(errors(&rx, Duration::from_secs(7)).is_empty());
        // The doorbell rings at once.
        let at = Instant::now();
        herdr.send_line(&pane, "printf 'E%sOR one\\n' RR").unwrap();
        let found = errors(&rx, Duration::from_secs(2));
        eprintln!("one: {found:?} after {:?}", at.elapsed());
        assert_eq!(found, ["ERROR one"]);
        assert!(at.elapsed() < Duration::from_secs(2), "rung, not polled");
        // Another while the window holds the first: read by the poll.
        let at = Instant::now();
        herdr.send_line(&pane, "printf 'E%sOR two\\n' RR").unwrap();
        let found = errors(&rx, Duration::from_secs(7));
        eprintln!("two: {found:?} after {:?}", at.elapsed());
        assert_eq!(found, ["ERROR two"]);
        // Clear, then a traceback.
        herdr
            .send_line(&pane, "for i in $(seq 30); do echo clean $i; done")
            .unwrap();
        let _ = errors(&rx, Duration::from_secs(7));
        let at = Instant::now();
        herdr
            .send_line(
                &pane,
                "printf 'Trace%s (most recent call last):\\n  File \"a.py\", line 1, in f\\nValueError: bad\\n' back",
            )
            .unwrap();
        let found = errors(&rx, Duration::from_secs(7));
        eprintln!("traceback: {found:?} after {:?}", at.elapsed());
        assert_eq!(found, ["ValueError: bad"]);
        assert!(at.elapsed() < Duration::from_secs(2), "rung, not polled");
    }
}
