//! The sandbox of a feature another machine writes, from here (see `feature::view`).
//!
//! A sandbox runs on the machine that started it. All that takes its processes
//! or containers stays there: this gerdr shows how it stands, its app and its
//! terminals, and says to stop it there.
//!
//! Its app listens inside its network namespace, which the machine's own
//! `localhost` doesn't reach: pasta leads only the other way there, so `ssh -L`
//! would find main's services or nothing. So each connection to a port here gets
//! a command of its own there, which enters the namespace, connects and passes
//! the bytes on (`relay_script`). Nothing stays there once it ends, and it holds
//! nothing that would need to be taken back with the link down.
//!
//! The ports here have the numbers they have there, for the app's own addresses
//! to work. They are open while the feature's workspace here shows a page on
//! `localhost`, its sandbox runs and the link is up: a page closed, or a link
//! lost, closes them, and a link back or a restarted gerdr opens them again.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, ErrorKind, Read, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream},
    os::fd::AsRawFd,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    Gerdr,
    desktop::shell_quote,
    feature::{Feature, Status, remote::Reach, sandbox_run::SandboxRun},
    feature_sandbox::{self, is_local_page, local_port},
};

/// How long a look at the listening sockets waits before seeing whether to stop.
const POLL_MS: i32 = 200;
/// How long a relay's command gets to end, once the connection has.
const END_WAIT: Duration = Duration::from_secs(2);
/// What the shell's `exec` ends with when a program isn't there.
const NOT_FOUND: i32 = 127;

/// The script passing bytes between its input and output and `port` on the
/// `localhost` it runs in. What's first to end ends all of it: either side going
/// away closes the other. A connection refused ends it too, as that port went.
///
/// bash because of `/dev/tcp`: nothing else that can connect comes with every
/// system. It only runs to its `exec`: `$$` is then the `cat` passing replies.
fn relay_script(port: u16) -> String {
    format!(
        "exec 3<>/dev/tcp/localhost/{port} || exit 7\n\
         {{ cat >&3; kill $$; }} <&0 >/dev/null 2>&1 &\n\
         exec cat <&3\n"
    )
}

/// The command there for one connection to `port` in the namespace `holder` keeps.
fn relay_command(holder: i32, port: u16) -> String {
    let enter = format!(
        "exec nsenter --user --net --preserve-credentials -t {holder} -- bash -c {}",
        shell_quote(&relay_script(port))
    );
    format!("sh -c {}", shell_quote(&enter))
}

/// `port` here, listening for connections to pass on. Dropped, it closes them all.
#[derive(Debug)]
struct Relay {
    stop: Arc<AtomicBool>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The sockets for `port` on this machine's loopback, both kinds: a browser tries
/// either for `localhost`, and what else listens on the one kind would answer it
/// in the place of the sandbox. A system without IPv6 has only the one.
fn listen_on(port: u16) -> io::Result<Vec<TcpListener>> {
    let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
    let mut out = vec![v4];
    match TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))) {
        Ok(v6) => out.push(v6),
        Err(err) if err.kind() == ErrorKind::AddrNotAvailable => {}
        Err(err) => return Err(err),
    }
    for listener in &out {
        listener.set_nonblocking(true)?;
    }
    Ok(out)
}

/// What a relay of `port` found that says the whole thing can't work.
#[derive(Clone, Debug, Default)]
struct Missing(Arc<AtomicBool>);

/// Start passing connections to `port` here on, each with the command `command`
/// makes. An error says the port can't be had.
fn start_relay(
    port: u16,
    command: impl Fn() -> Command + Send + 'static,
    missing: Missing,
) -> io::Result<Relay> {
    let listeners = listen_on(port)?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    std::thread::Builder::new()
        .name(format!("relay {port}"))
        .spawn(move || serve(&listeners, command, &thread_stop, missing))?;
    Ok(Relay { stop })
}

/// Connections that are open, for them to be closed from outside.
type Open = Arc<Mutex<Vec<(u64, TcpStream)>>>;

fn serve(
    listeners: &[TcpListener],
    command: impl Fn() -> Command,
    stop: &AtomicBool,
    missing: Missing,
) {
    let open: Open = Arc::default();
    let mut serial = 0;
    while !stop.load(Ordering::Relaxed) {
        wait_for_any(listeners);
        for listener in listeners {
            match listener.accept() {
                Ok((stream, _)) => {
                    serial += 1;
                    pass_on(stream, command(), serial, &open, &missing);
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                Err(err) => tracing::warn!("taking a connection to the sandbox: {err}"),
            }
        }
    }
    if let Ok(open) = open.lock() {
        for (_, stream) in open.iter() {
            // Gone already is what's wanted.
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Wait until one of `listeners` has a connection, or a while.
fn wait_for_any(listeners: &[TcpListener]) {
    let mut fds: Vec<libc::pollfd> = listeners
        .iter()
        .map(|l| libc::pollfd {
            fd: l.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    // SAFETY: `fds` holds `fds.len()` entries and outlives the call. A failure
    // only means the sockets get looked at sooner.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, POLL_MS) };
}

/// Give `stream` a thread that passes it on through `command`, until either ends.
fn pass_on(stream: TcpStream, command: Command, serial: u64, open: &Open, missing: &Missing) {
    let Ok(ours) = stream.try_clone() else {
        tracing::warn!("couldn't keep a connection to the sandbox");
        return;
    };
    if let Ok(mut open) = open.lock() {
        open.push((serial, ours));
    }
    let (open, missing) = (Arc::clone(open), missing.clone());
    let spawned = std::thread::Builder::new()
        .name("relay connection".into())
        .spawn(move || {
            relay_one(stream, command, &missing);
            if let Ok(mut open) = open.lock() {
                open.retain(|(n, _)| *n != serial);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("passing on a connection to the sandbox: {err}");
    }
}

/// Pass what `from` has on to `to` as it comes, until either ends. Not `io::copy`:
/// between a socket and a pipe it moves the bytes in the kernel, and a few of them
/// were seen to wait there for more that never came.
fn pass_bytes(from: &mut impl Read, to: &mut impl Write) {
    let mut buffer = [0; 16 * 1024];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => {
                if to.write_all(&buffer[..n]).is_err() {
                    return;
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Pass `stream` and what `command` reads and writes to each other, to the end
/// of either.
fn relay_one(mut stream: TcpStream, mut command: Command, missing: &Missing) {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            tracing::warn!("couldn't run ssh for a connection to the sandbox: {err}");
            return;
        }
    };
    let (Some(mut to), Some(mut from)) = (child.stdin.take(), child.stdout.take()) else {
        return;
    };
    // Small requests shouldn't wait for more to come.
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!("a connection to the sandbox stays buffered: {err}");
    }
    let up = stream.try_clone().map(|mut incoming| {
        std::thread::spawn(move || {
            // However it ends, closing its input tells the command there.
            pass_bytes(&mut incoming, &mut to);
        })
    });
    // The connection or the command ended: what remains of both goes.
    pass_bytes(&mut from, &mut stream);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    if let Ok(up) = up {
        // It ends with the connection just shut. A panic in it has nothing to say.
        let _ = up.join();
    }
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() < END_WAIT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => {
                // It may have just ended; `wait` reaps it either way.
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    if code == Some(NOT_FOUND) {
        missing.0.store(true, Ordering::Relaxed);
    }
}

/// What this gerdr has open of a feature's sandbox elsewhere.
#[derive(Debug)]
pub struct SandboxView {
    title: String,
    /// The sandbox it reaches: a new one has other numbers.
    holder: i32,
    relays: BTreeMap<u16, Relay>,
    /// Ports it listens on that can't be had here.
    taken: BTreeSet<u16>,
    missing: Missing,
    /// `missing` was said.
    said_missing: bool,
}

/// What `feature`'s page says of its sandbox elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SandboxSeen {
    /// The machine it runs on.
    pub name: String,
    /// The ports open here.
    pub here: Vec<u16>,
    /// The ports that can't be had here.
    pub taken: Vec<u16>,
}

/// What's said of `port` being taken: it goes on the badge too, for what it takes
/// is for the user to do.
fn taken_text(title: &str, port: u16) -> String {
    format!(
        "Port {port} is taken here, so the sandbox of \u{201c}{title}\u{201d} can't be reached at it: stop what holds it"
    )
}

impl SandboxView {
    fn new(title: &str, holder: i32) -> Self {
        Self {
            title: title.to_owned(),
            holder,
            relays: BTreeMap::new(),
            taken: BTreeSet::new(),
            missing: Missing::default(),
            said_missing: false,
        }
    }

    /// The sandbox listens on `ports`: what isn't open here opens, and what it no
    /// longer listens on closes. The ports found taken just now.
    fn follow(
        &mut self,
        reach: &Reach,
        ports: &[u16],
        start: &impl Fn(&Reach, i32, u16, Missing) -> io::Result<Relay>,
    ) -> Vec<u16> {
        self.relays.retain(|port, _| ports.contains(port));
        for port in self.taken.clone() {
            if !ports.contains(&port) {
                self.taken.remove(&port);
                crate::problems::forget(feature_sandbox::PROBLEMS, &taken_text(&self.title, port));
            }
        }
        let mut newly = Vec::new();
        for &port in ports {
            if self.relays.contains_key(&port) {
                continue;
            }
            match start(reach, self.holder, port, self.missing.clone()) {
                Ok(relay) => {
                    self.relays.insert(port, relay);
                    if self.taken.remove(&port) {
                        let text = taken_text(&self.title, port);
                        crate::problems::forget(feature_sandbox::PROBLEMS, &text);
                    }
                }
                Err(err) => {
                    if self.taken.insert(port) {
                        tracing::warn!("port {port} for the sandbox here: {err}");
                        newly.push(port);
                    }
                }
            }
        }
        newly
    }
}

impl Drop for SandboxView {
    fn drop(&mut self) {
        for &port in &self.taken {
            crate::problems::forget(feature_sandbox::PROBLEMS, &taken_text(&self.title, port));
        }
    }
}

/// Start `port` here, reaching it in the namespace `holder` keeps on `reach`.
fn relay_over(reach: &Reach, holder: i32, port: u16, missing: Missing) -> io::Result<Relay> {
    let reach = reach.clone();
    start_relay(port, move || reach.relay(&relay_command(holder, port)), missing)
}

/// Whether `ws`'s tabs show a page on `localhost`, or are about to.
fn has_local_page(ws: &crate::Workspace) -> bool {
    crate::ci_watch::showing(ws, is_local_page).is_some()
}

impl Gerdr {
    /// The machine writing `feature`, with its sandbox if that runs.
    fn seen_sandbox(feature: &Feature) -> Option<(&str, &SandboxRun)> {
        let run = feature.sandbox.as_ref().filter(|r| r.alive())?;
        Some((feature.writer.as_deref()?, run))
    }

    /// Open what's asked for of the sandboxes running elsewhere, and close what
    /// no longer is. It follows what is so: it opens again after a lost link or a
    /// restart, with the page still there.
    pub(crate) fn sandbox_views_tick(&mut self) {
        let wanted: Vec<(String, String)> = (self.features.viewed.values().flatten())
            .filter(|f| f.status == Status::Open)
            .filter(|f| {
                Self::seen_sandbox(f).is_some_and(|(id, _)| self.work_of(id).is_some())
                    && (self.feature_workspace(&f.project, &f.slug))
                        .and_then(|w| self.workspaces.get(w))
                        .is_some_and(has_local_page)
            })
            .map(|f| (f.project.clone(), f.slug.clone()))
            .collect();
        self.machines.sandboxes.retain(|key, _| wanted.contains(key));
        for (project, slug) in wanted {
            self.follow_sandbox(&project, &slug);
        }
    }

    /// Bring the ports open here of feature `slug`'s sandbox elsewhere in line.
    /// Those just said to be taken.
    fn follow_sandbox(&mut self, project: &str, slug: &str) -> Vec<u16> {
        let Some(feature) = self.feature(project, slug) else {
            return Vec::new();
        };
        let Some((id, run)) = Self::seen_sandbox(feature) else {
            return Vec::new();
        };
        let Some(reach) = self.work_of(id).map(crate::machine_work::Work::reach) else {
            return Vec::new();
        };
        let Some(holder) = run.holder.as_ref().map(|h| h.pid) else {
            return Vec::new();
        };
        let name = self.machine_name(id);
        let (title, ports) = (feature.title.clone(), run.listening());
        let key = (project.to_owned(), slug.to_owned());
        let view = (self.machines.sandboxes.entry(key))
            .or_insert_with(|| SandboxView::new(&title, holder));
        if view.holder != holder {
            *view = SandboxView::new(&title, holder);
        }
        let newly = view.follow(&reach, &ports, &relay_over);
        let missing = view.missing.0.load(Ordering::Relaxed) && !view.said_missing;
        view.said_missing |= missing;
        for &port in &newly {
            let text = taken_text(&title, port);
            crate::problems::report(feature_sandbox::PROBLEMS, text.clone());
            self.show_warning(text);
        }
        if missing {
            let text = format!(
                "The sandbox of \u{201c}{title}\u{201d} can't be reached from here: {name} needs bash and nsenter for that"
            );
            tracing::warn!("{text}");
            self.show_error(text);
        }
        newly
    }

    /// Open feature `slug`'s sandbox's app, which runs on the machine writing it:
    /// its ports open here first, and a page opens on one that is really its own.
    pub(crate) fn open_seen_sandbox_app(&mut self, project: &str, slug: &str, id: &str) {
        let name = self.machine_name(id);
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(ports) = Self::seen_sandbox(feature).map(|(_, run)| run.listening()) else {
            return self.show_warning(format!("The sandbox isn't running on {name}"));
        };
        if self.work_of(id).is_none() {
            return self.show_warning(format!("Connect {name} to open its sandbox's app"));
        }
        let Some(url) = self.sandbox_url(project, &ports) else {
            return self.show_warning("Nothing listens in the sandbox yet".into());
        };
        let said = self.follow_sandbox(project, slug);
        let key = (project.to_owned(), slug.to_owned());
        let taken = local_port(&url).filter(|port| {
            (self.machines.sandboxes.get(&key)).is_some_and(|view| view.taken.contains(port))
        });
        // It would show what holds that port here, as if it were the sandbox.
        if let Some(port) = taken {
            if !said.contains(&port)
                && let Some(title) = self.feature(project, slug).map(|f| f.title.clone())
            {
                self.show_warning(taken_text(&title, port));
            }
            return;
        }
        self.open_feature_page(project, slug, url);
    }

    /// Show the terminals of feature `slug`'s sandbox, which runs on the machine
    /// writing it, each in a tab here. Those only look: they stay that machine's.
    pub(crate) fn show_sandbox_terminals(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(id) = feature.writer.clone() else {
            return;
        };
        let panes = (feature.sandbox.as_ref()).map_or_else(Vec::new, |run| run.panes.clone());
        let name = self.machine_name(&id);
        if panes.is_empty() {
            return self.show_warning(format!("The sandbox has no terminals on {name}"));
        }
        for pane in panes {
            if !self.show_machine_pane(&pane) {
                // It said why, and the others would say the same.
                return;
            }
        }
    }

    /// What `feature`'s page says of its sandbox, when another machine has it.
    pub(crate) fn sandbox_seen(&self, feature: &Feature) -> Option<SandboxSeen> {
        let name = self.machine_name(feature.writer.as_deref()?);
        let key = (feature.project.clone(), feature.slug.clone());
        let view = self.machines.sandboxes.get(&key);
        Some(SandboxSeen {
            name,
            here: view.map_or_else(Vec::new, |v| v.relays.keys().copied().collect()),
            taken: view.map_or_else(Vec::new, |v| v.taken.iter().copied().collect()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::Shutdown,
        os::unix::process::ExitStatusExt,
        process::Stdio,
    };

    use super::*;

    /// A port nothing listens on now.
    fn free_port() -> u16 {
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        probe.local_addr().unwrap().port()
    }

    fn sh(script: &'static str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    fn connect(port: u16) -> TcpStream {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }

    #[test]
    fn a_port_here_passes_on_to_a_command_per_connection() {
        let port = free_port();
        let relay = start_relay(port, || sh("cat"), Missing::default()).unwrap();
        // Two at a time: each has its own.
        let mut one = connect(port);
        let mut two = connect(port);
        one.write_all(b"one\n").unwrap();
        two.write_all(b"two\n").unwrap();
        let mut got = [0; 4];
        one.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"one\n");
        two.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"two\n");
        // Closed, what's open closes too: nothing stays behind.
        drop(relay);
        let mut rest = Vec::new();
        assert_eq!(one.read_to_end(&mut rest).unwrap_or(0), 0);
        std::thread::sleep(Duration::from_millis(2 * POLL_MS as u64));
        assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
    }

    #[test]
    fn a_port_taken_here_is_refused() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(start_relay(port, || sh("cat"), Missing::default()).is_err());
    }

    #[test]
    fn a_port_taken_only_on_the_other_kind_of_address_is_taken_too() {
        let Ok(held) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) else {
            return; // No IPv6 here.
        };
        let port = held.local_addr().unwrap().port();
        assert!(listen_on(port).is_err());
    }

    #[test]
    fn a_missing_program_there_is_said() {
        let port = free_port();
        let missing = Missing::default();
        let _relay = start_relay(port, || sh("exit 127"), missing.clone()).unwrap();
        let mut stream = connect(port);
        let mut rest = Vec::new();
        assert_eq!(stream.read_to_end(&mut rest).unwrap_or(0), 0);
        let started = Instant::now();
        while !missing.0.load(Ordering::Relaxed) {
            assert!(started.elapsed() < Duration::from_secs(5), "not said");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `relay_script(port)` running as a relay's command would.
    fn script_child(port: u16) -> std::process::Child {
        Command::new("bash")
            .args(["-c", &relay_script(port)])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn wait_exit(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        None
    }

    #[test]
    fn the_script_there_ends_when_the_app_closes() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = server.local_addr().unwrap().port();
        let mut child = script_child(port);
        let (mut app, _) = server.accept().unwrap();
        app.write_all(b"hello").unwrap();
        app.shutdown(Shutdown::Both).unwrap();
        let mut out = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello");
        assert!(wait_exit(&mut child).is_some(), "still running");
    }

    #[test]
    fn the_script_there_ends_when_the_browser_closes() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = server.local_addr().unwrap().port();
        let mut child = script_child(port);
        let (mut app, _) = server.accept().unwrap();
        // What's written gets there, and closing lets the app know.
        let mut input = child.stdin.take().unwrap();
        input.write_all(b"request").unwrap();
        drop(input);
        app.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut got = Vec::new();
        app.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"request");
        // Killed by its other half: nothing keeps the connection.
        let status = wait_exit(&mut child).expect("still running");
        assert!(status.signal().is_some() || status.success(), "{status:?}");
    }

    #[test]
    fn the_script_there_says_when_nothing_listens() {
        let mut child = script_child(free_port());
        let status = wait_exit(&mut child).expect("still running");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn a_connection_enters_the_sandbox_it_is_for() {
        let line = relay_command(4242, 5173);
        assert!(line.starts_with("sh -c "), "{line}");
        assert!(line.contains("-t 4242"), "{line}");
        assert!(line.contains("/dev/tcp/localhost/5173"), "{line}");
    }

    fn reach() -> Reach {
        Reach {
            target: "box".into(),
            control: "/nowhere".into(),
        }
    }

    #[test]
    fn ports_follow_what_listens_there_and_a_taken_one_is_said_once() {
        use std::cell::RefCell;
        let refused: RefCell<BTreeSet<u16>> = RefCell::new(BTreeSet::from([2]));
        let start = |_: &Reach, _: i32, port: u16, _: Missing| {
            if refused.borrow().contains(&port) {
                return Err(io::Error::from(ErrorKind::AddrInUse));
            }
            Ok(Relay {
                stop: Arc::default(),
            })
        };
        let mut view = SandboxView::new("t", 1);
        assert_eq!(view.follow(&reach(), &[1, 2], &start), [2]);
        assert_eq!(view.relays.keys().copied().collect::<Vec<_>>(), [1]);
        // Still taken: said once.
        assert!(view.follow(&reach(), &[1, 2], &start).is_empty());
        // Free now: it opens by itself. One that went closes.
        refused.borrow_mut().clear();
        assert!(view.follow(&reach(), &[2], &start).is_empty());
        assert_eq!(view.relays.keys().copied().collect::<Vec<_>>(), [2]);
        assert!(view.taken.is_empty());
    }
}
