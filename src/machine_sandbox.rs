//! A sandbox on another machine, from here: that of a feature another machine
//! writes (see `feature::view`), or of one written here that was placed on a
//! machine (`SandboxRun::machine`).
//!
//! All that takes its processes or containers runs there. This gerdr shows how it
//! stands, opens its app and looks at its terminals, and can stop it: over the
//! link when this gerdr writes the feature; else a gerdr open on the writer is
//! asked to (`SandboxAsk`), so its record says so. With none open, or one that
//! can't reach the sandbox's machine, that machine is told directly, and a note
//! left on the writer has its record catch up (`SANDBOX_TOLD`).
//!
//! A sandbox on a machine out of reach when its feature is removed can't be taken
//! down then: what's needed is kept (`LEFT`), and it goes when that machine is back.
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
    feature::{
        Feature, Repo, Status, lease,
        remote::{self, Paths, Reach},
        sandbox,
        sandbox_run::{self, Elsewhere, Host, SandboxRun},
    },
    feature_sandbox::{self, SandboxLine, StopFor, is_local_page, local_port},
    features::{Busy, Done, Job},
    herdr::Herdr,
    machine_checks::Remote,
    machine_work::{open_there, qualified},
    prepare,
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
pub(crate) fn relay_script(port: u16) -> String {
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

/// Starting a sandbox on a machine.
#[derive(Debug)]
pub(crate) struct ThereJob {
    remote: Remote,
    feature: Feature,
    /// What runs first, and in its terminals, by its folder here.
    prepares: Vec<SandboxLine>,
    runs: Vec<SandboxLine>,
    env: Vec<(String, String)>,
    /// Ports other sandboxes hold there, though stopped.
    taken: Vec<u16>,
}

/// `repos`, as named on the machine `paths` lead to.
fn repos_there(repos: &[Repo], paths: &Paths) -> Vec<Repo> {
    (repos.iter())
        .map(|repo| Repo {
            main: paths.map(&repo.main),
            worktree: paths.map(&repo.worktree),
            ..repo.clone()
        })
        .collect()
}

/// The line entering `run`'s namespace for `command` in a terminal there: it
/// takes up the folder's `.venv`, as a terminal here does.
fn terminal_line(holder: i32, slug: &str, env: &[(String, String)], command: &str) -> String {
    let command = format!("{}{command}", remote::ACTIVATE);
    sandbox_run::enter_line(holder, slug, env, &command)
}

impl ThereJob {
    /// Start it, with its terminals. The record, and the commands that got no
    /// terminal.
    pub(crate) fn run(
        self,
        stop: &AtomicBool,
        progress: &mut dyn FnMut(String),
    ) -> (SandboxRun, Vec<String>) {
        let mut run = self.start(stop, progress);
        run.machine = Some(self.remote.id.clone());
        run.elsewhere = Some(Elsewhere {
            asked: true,
            alive: run.failed.is_none(),
            ports: Vec::new(),
        });
        let mut missing = Vec::new();
        if run.failed.is_some() {
            return (run, missing);
        }
        run.not_there = self.not_there(&run, stop);
        let Some(holder) = run.holder.as_ref().map(|h| h.pid) else {
            return (run, missing);
        };
        let mut herdr = Herdr::on_socket(self.remote.socket.clone());
        for line in &self.runs {
            let cwd = self.remote.paths.there(&line.dir);
            let command = self.remote.paths.text(&line.command);
            let opened = open_there(&mut herdr, &self.feature.project, &cwd, stop).and_then(
                |(pane, _, info)| {
                    let line = terminal_line(holder, &self.feature.slug, &self.env, &command);
                    let line = prepare::line_there(&info, &cwd, &line);
                    (herdr.send_shell_line(&pane, &line))
                        .map_err(remote::Trouble::Link)
                        .map(|()| qualified(&self.remote.id, &pane))
                },
            );
            match opened {
                Ok(pane) => run.panes.push(pane),
                Err(trouble) => {
                    tracing::warn!("a sandbox's terminal there: {}", trouble.text());
                    missing.push(format!("`{command}`"));
                }
            }
        }
        (run, missing)
    }

    fn start(&self, stop: &AtomicBool, progress: &mut dyn FnMut(String)) -> SandboxRun {
        let Remote { paths, reach, .. } = &self.remote;
        progress("Bringing its code there".to_owned());
        if let Err(trouble) = self.remote.place_code(&self.feature, stop) {
            return SandboxRun {
                started: crate::feature::now(),
                finished: Some(crate::feature::now()),
                failed: Some(trouble.text().to_owned()),
                ..SandboxRun::default()
            };
        }
        let repos = repos_there(&self.feature.repos, paths);
        let roots = self.roots_there();
        let dir = feature_sandbox::sandbox_dir(&self.feature.project, &self.feature.slug);
        let dir = paths.map(&dir);
        let prepare: Vec<sandbox_run::PrepareStep> = (self.prepares.iter())
            .map(|line| sandbox_run::PrepareStep {
                line: format!("{}{}", remote::ACTIVATE, paths.text(&line.command)),
                dir: paths.map(&line.dir),
                shown: line.command.clone(),
            })
            .collect();
        let host = Host::There(reach.clone());
        let job = sandbox_run::Job {
            host: &host,
            project: &self.feature.project,
            slug: &self.feature.slug,
            owner: &crate::machine_lease::own_id_made().unwrap_or_default(),
            repos: &repos,
            roots: &roots,
            dir: &dir,
            prepare: &prepare,
            env: &self.env,
            taken: &self.taken,
            stop,
        };
        sandbox_run::start(&job, progress)
    }

    fn roots_there(&self) -> Vec<std::path::PathBuf> {
        let links = self.feature.links.iter().map(|l| &l.root);
        (std::iter::once(&self.feature.root).chain(links))
            .map(|root| self.remote.paths.map(root))
            .collect()
    }

    /// What runs under the project here, that `run` found nothing of there: a
    /// worker has no containers but those started on it. Not being able to ask
    /// here means there's nothing to say.
    fn not_there(&self, run: &SandboxRun, stop: &AtomicBool) -> Vec<String> {
        let found = sandbox_run::engines(&Host::Here)
            .and_then(|engines| sandbox_run::running_containers(&Host::Here, &engines, stop));
        let Ok(containers) = found else {
            return Vec::new();
        };
        let links = self.feature.links.iter().map(|l| l.root.clone());
        let roots: Vec<std::path::PathBuf> =
            std::iter::once(self.feature.root.clone()).chain(links).collect();
        let feature = &self.feature;
        (sandbox::stacks_under(&containers, &roots).into_iter())
            .filter(|stack| {
                let name = sandbox::compose_name(&feature.project, &feature.slug, &stack.project);
                !run.stacks.iter().any(|mine| mine.name == name)
            })
            .map(|stack| stack.project)
            .collect()
    }
}

/// What became of asking a gerdr open on the writer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AskEnd {
    /// It took it on.
    Taken,
    /// It can't do that, and why.
    Refused(String),
    NoAnswer,
}

/// How a stop from another machine went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopAfar {
    /// A gerdr open on the writer took it on: its record will say.
    Asked,
    /// The machine was told directly, with no gerdr to ask.
    Told,
}

/// All a stop from another machine needs, off the UI thread.
struct AfarJob {
    what: StopFor,
    /// The machine writing the feature, if it can be reached: its link and data
    /// folder, and the feature's folder there.
    writer: Option<(Reach, String, String)>,
    project: String,
    writer_name: String,
    my_name: String,
    run: SandboxRun,
    /// Where it runs, or that machine's name, and its folder there.
    host: Result<Host, String>,
    dir: Option<std::path::PathBuf>,
}

impl AfarJob {
    fn go(&self) -> Result<StopAfar, String> {
        let never = AtomicBool::new(false);
        if let Some((reach, data, dir)) = &self.writer {
            let open = (reach.script(&lease::open_script(data, &self.project), &never))
                .map_err(|t| t.text().to_owned())?;
            if open.trim() == "HELD" {
                match self.ask(reach, dir)? {
                    AskEnd::Taken => return Ok(StopAfar::Asked),
                    AskEnd::Refused(why) if self.what.starts_again() || self.host.is_err() => {
                        return Err(why);
                    }
                    // This machine may reach what that one can't.
                    AskEnd::Refused(_) | AskEnd::NoAnswer => {}
                }
            }
        }
        if self.what.starts_again() {
            return Err(format!(
                "gerdr isn't answering on {}: open it there, or stop the sandbox from here",
                self.writer_name
            ));
        }
        let host = (self.host.as_ref()).map_err(|name| format!("{name} can't be reached"))?;
        let Some(dir) = &self.dir else {
            return Err("its folder there isn't known".to_owned());
        };
        sandbox_run::stop(&self.run, dir, self.what.removes(), host)?;
        self.leave_note();
        Ok(StopAfar::Told)
    }

    /// What this asks of a gerdr on the writer, as JSON.
    fn ask_json(&self) -> Result<String, String> {
        serde_json::to_string(&lease::SandboxAsk {
            what: self.what,
            name: self.my_name.clone(),
            started: self.run.started,
        })
        .map_err(|err| err.to_string())
    }

    /// Have the writer's record catch up with what was just done. It can't be told
    /// when out of reach: its page then reads that the network ended, which is so.
    fn leave_note(&self) {
        let never = AtomicBool::new(false);
        let Some((reach, _, dir)) = &self.writer else {
            return;
        };
        let told = (self.ask_json())
            .and_then(|json| {
                let script = lease::sandbox_told_script(dir, &json);
                reach.script(&script, &never).map_err(|t| t.text().to_owned())
            });
        if let Err(why) = told {
            tracing::warn!("leaving word of a sandbox on its writer: {why}");
        }
    }

    /// Ask a gerdr open on the writer, and wait to see what comes of it.
    fn ask(&self, reach: &Reach, dir: &str) -> Result<AskEnd, String> {
        let never = AtomicBool::new(false);
        let json = self.ask_json()?;
        (reach.script(&lease::sandbox_ask_script(dir, &json), &never))
            .map_err(|t| t.text().to_owned())?;
        let until = Instant::now() + lease::ASK_FOR;
        while Instant::now() < until {
            let said = (reach.script(&lease::sandbox_waits_script(dir), &never))
                .map_err(|t| t.text().to_owned())?;
            if said.trim().is_empty() {
                let script = lease::record_script(dir, lease::SANDBOX_ANSWER);
                let said = reach.script(&script, &never).map_err(|t| t.text().to_owned())?;
                return Ok(match serde_json::from_str::<lease::SandboxRefusal>(&said) {
                    Ok(refusal) => AskEnd::Refused(refusal.why),
                    Err(_) => AskEnd::Taken,
                });
            }
            std::thread::sleep(ASK_EVERY);
        }
        if let Err(trouble) = reach.script(&lease::sandbox_unask_script(dir), &never) {
            tracing::warn!("taking back what was asked of a sandbox: {}", trouble.text());
        }
        Ok(AskEnd::NoAnswer)
    }
}

/// `port` here, listening for connections to pass on. Dropped, it closes them all.
#[derive(Debug)]
pub(crate) struct Relay {
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

/// How often what was asked of a sandbox is looked at, to see it was taken.
const ASK_EVERY: Duration = Duration::from_millis(400);
/// Under what's kept of a machine: the records of features removed here whose
/// sandboxes that machine still has.
pub(crate) const LEFT: &str = "sandboxes-left";

/// Where feature `slug`'s record waits for machine `id` to be back.
pub(crate) fn left_file(id: &str, project: &str, slug: &str) -> std::path::PathBuf {
    crate::feature::view::root(&crate::projects::data_dir(), id)
        .join(LEFT)
        .join(format!("{project}--{slug}.json"))
}

/// What the problems list says of a removed feature's sandbox on a machine.
fn left_text(title: &str, name: &str) -> String {
    format!("The sandbox of \u{201c}{title}\u{201d} stays on {name} until that can be reached")
}

/// The records waiting for machine `id` to be back, with their files.
fn left_records(id: &str) -> Vec<(std::path::PathBuf, Feature)> {
    let dir = crate::feature::view::root(&crate::projects::data_dir(), id).join(LEFT);
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    (entries.map(|entry| entry.path()))
        .filter_map(|file| {
            let read = std::fs::read(&file).map_err(|err| err.to_string()).and_then(|raw| {
                serde_json::from_slice::<Feature>(&raw).map_err(|err| err.to_string())
            });
            match read {
                Ok(feature) => Some((file, feature)),
                Err(why) => {
                    tracing::warn!("reading {}: {why}", file.display());
                    None
                }
            }
        })
        .collect()
}

/// Keep `feature` at `file`, for what it left to go later.
fn keep_left(file: &std::path::Path, feature: &Feature) -> Result<(), String> {
    let text = serde_json::to_vec(feature).map_err(|err| err.to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(file, text).map_err(|err| err.to_string())
}

/// Taking down the sandbox of a feature that is removed, off the UI thread.
#[derive(Debug)]
pub(crate) struct SandboxRemoval {
    run: SandboxRun,
    /// Where it runs, or that machine's name.
    host: Result<Host, String>,
    dir: Option<std::path::PathBuf>,
    /// What else the feature left on that machine.
    forget: Option<String>,
    /// On a machine: its name, and the record to keep if that can't be reached.
    left: Option<(String, std::path::PathBuf, Box<Feature>)>,
}

impl SandboxRemoval {
    fn remove(&self) -> Result<(), String> {
        let host = (self.host.as_ref()).map_err(|name| format!("{name} can't be reached"))?;
        let dir = (self.dir.as_ref()).ok_or_else(|| "its folder there isn't known".to_owned())?;
        sandbox_run::stop(&self.run, dir, true, host)?;
        if let (Host::There(reach), Some(script)) = (host, &self.forget) {
            let never = AtomicBool::new(false);
            reach.script(script, &never).map_err(|t| t.text().to_owned())?;
        }
        Ok(())
    }

    /// Take it down. On a machine that can't be reached now, it goes once it can.
    pub(crate) fn go(&self) {
        let Err(why) = self.remove() else {
            return;
        };
        tracing::error!("removing a feature's sandbox: {why}");
        let text = match &self.left {
            Some((name, file, feature)) => match keep_left(file, feature) {
                Ok(()) => left_text(&feature.title, name),
                Err(err) => {
                    tracing::error!("keeping {}: {err}", file.display());
                    format!("A removed feature's sandbox stayed behind: {why}")
                }
            },
            None => format!("A removed feature's sandbox stayed behind: {why}"),
        };
        crate::problems::report(feature_sandbox::PROBLEMS, text);
    }
}

/// What a relay of `port` found that says the whole thing can't work.
#[derive(Clone, Debug, Default)]
pub(crate) struct Missing(pub(crate) Arc<AtomicBool>);

/// Start passing connections to `port` here on, each with the command `command`
/// makes. An error says the port can't be had.
fn start_relay(
    port: u16,
    command: impl Fn() -> Command + Send + 'static,
    missing: Missing,
) -> io::Result<Relay> {
    start_relay_on(listen_on(port)?, port, command, missing)
}

/// `start_relay` on `listeners`, which listen on `port`.
pub(crate) fn start_relay_on(
    listeners: Vec<TcpListener>,
    port: u16,
    command: impl Fn() -> Command + Send + 'static,
    missing: Missing,
) -> io::Result<Relay> {
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
pub(crate) fn taken_text(title: &str, port: u16) -> String {
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
    /// The machine `feature`'s sandbox runs on, if that's another and it runs.
    fn seen_sandbox(feature: &Feature) -> Option<(&str, &SandboxRun)> {
        let run = feature.sandbox.as_ref().filter(|r| r.alive())?;
        Some((run.machine.as_deref()?, run))
    }

    /// Where `run` runs. `Err`: on a machine with no link now, by that name.
    pub(crate) fn sandbox_host(&self, run: &SandboxRun) -> Result<Host, String> {
        let Some(id) = run.machine.as_deref() else {
            return Ok(Host::Here);
        };
        (self.work_of(id))
            .map(|work| Host::There(work.reach()))
            .ok_or_else(|| self.machine_name(id))
    }

    /// The folder of feature `slug`'s sandbox on the machine `run` runs on.
    pub(crate) fn sandbox_dir_of(
        &self,
        project: &str,
        slug: &str,
        run: &SandboxRun,
    ) -> Option<std::path::PathBuf> {
        let Some(id) = run.machine.as_deref() else {
            return Some(feature_sandbox::sandbox_dir(project, slug));
        };
        Some(std::path::PathBuf::from(self.data_there(id, project, slug)?).join("sandbox"))
    }

    /// Feature `slug`'s data folder on machine `id`, whose data folder that machine
    /// said. Not by this machine's path of it: for a feature another machine writes,
    /// that is the copy kept here, under a folder only this machine has.
    fn data_there(&self, id: &str, project: &str, slug: &str) -> Option<String> {
        let data = &self.work_of(id)?.info.site.data;
        Some(format!("{data}/gerdr/features/{project}/{slug}"))
    }

    /// What a thread needs to start `feature`'s sandbox on machine `id`, if that
    /// is up.
    pub(crate) fn sandbox_there_job(
        &self,
        feature: &Feature,
        id: &str,
        env: &BTreeMap<String, String>,
        taken: Vec<u16>,
    ) -> Option<ThereJob> {
        let remote = self.machine_remote(id, feature)?;
        let on = |lines: Vec<SandboxLine>| -> Vec<SandboxLine> {
            lines.into_iter().filter(|line| line.on).collect()
        };
        let prepares = self.projects.workflows(feature, |w| w.prepare.clone());
        let runs = self.projects.workflows(feature, |w| w.run.clone());
        Some(ThereJob {
            remote,
            prepares: on(feature_sandbox::lines_of(feature, &prepares, true)),
            runs: on(feature_sandbox::lines_of(feature, &runs, false)),
            feature: feature.clone(),
            env: env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            taken,
        })
    }

    /// Whether `feature`'s sandbox has to wait for a setup on machine `id`, as
    /// each machine has its own worktrees: it starts by itself after that.
    pub(crate) fn set_up_there_first(&mut self, feature: &Feature, id: &str) -> bool {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let key = (project.to_owned(), slug.to_owned());
        let running = self.setting_up(project, slug);
        let has = crate::feature::setup::has_work(feature);
        let first = crate::feature_build::setup_first(has, feature.setup_at(Some(id)), running, false);
        let waits = match first {
            crate::feature_build::SetupFirst::No => false,
            crate::feature_build::SetupFirst::Wait => true,
            crate::feature_build::SetupFirst::Run => {
                self.start_setup_on(project, slug, None, Some(id.to_owned()))
            }
        };
        if waits {
            self.features.build.pending_sandbox.insert(key);
            self.show_warning(format!(
                "Setting up \u{201c}{}\u{201d} on {}; its sandbox starts once that is done",
                feature.title,
                self.machine_name(id)
            ));
        }
        waits
    }

    /// Feature `slug`, written on machine `writer`: do `what` to its sandbox from
    /// here, off the thread.
    pub(crate) fn stop_from_afar(&mut self, project: &str, slug: &str, what: StopFor, writer: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(run) = feature.sandbox.clone() else {
            return;
        };
        let job = AfarJob {
            what,
            writer: self.work_of(writer).and_then(|work| {
                let data = format!("{}/gerdr", work.info.site.data);
                Some((work.reach(), data, self.data_there(writer, project, slug)?))
            }),
            project: project.to_owned(),
            writer_name: self.machine_name(writer),
            my_name: crate::machine_lease::this_machines_name(),
            host: self.sandbox_host(&run),
            dir: self.sandbox_dir_of(project, slug, &run),
            run,
        };
        let title = feature.title.clone();
        let key = (project.to_owned(), slug.to_owned());
        let stop = Arc::new(AtomicBool::new(false));
        self.features.build.sandbox_stops.insert(key, stop);
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Sandbox,
            repo: None,
            what: format!("Stopping the sandbox of \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::SandboxAsked {
            result: job.go(),
            project,
            slug,
        });
    }

    /// A stop from here of a sandbox another machine's gerdr writes of ended.
    pub(crate) fn sandbox_asked(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<StopAfar, String>,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.sandbox_stops.remove(&key);
        let writer = self.feature_writer(project, slug).map(str::to_owned);
        let name = writer.map_or_else(String::new, |id| self.machine_name(&id));
        match result {
            Ok(StopAfar::Asked) => self.show_success(format!("gerdr on {name} is doing that")),
            Ok(StopAfar::Told) => self.show_success(format!(
                "That's done, and gerdr on {name} is told"
            )),
            Err(why) => {
                tracing::warn!("a sandbox from another machine: {why}");
                self.show_error(format!("The sandbox is as it was: {why}"));
            }
        }
    }

    /// Why feature `slug`'s sandbox can't have `ask` done to it from here now.
    pub(crate) fn sandbox_refusal(
        &self,
        project: &str,
        slug: &str,
        ask: &lease::SandboxAsk,
    ) -> Option<String> {
        let key = (project.to_owned(), slug.to_owned());
        if self.features.build.sandbox_stops.contains_key(&key) {
            return (ask.what.starts_again()).then(|| "it is being started or stopped".to_owned());
        }
        let run = self.feature(project, slug)?.sandbox.as_ref()?;
        (self.sandbox_host(run).err()).map(|name| format!("{name} can't be reached from there"))
    }

    /// Another machine did `ask` to feature `slug`'s sandbox while no gerdr could
    /// be asked: the record catches up. One that started since is another sandbox.
    pub(crate) fn sandbox_told(&mut self, project: &str, slug: &str, ask: &lease::SandboxAsk) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if feature.sandbox.as_ref().is_none_or(|run| run.started != ask.started) {
            return;
        }
        let name = ask.name.clone();
        if let Some(f) = self.feature_mut(project, slug) {
            let what = match ask.what {
                StopFor::Remove => "Sandbox removed",
                _ => "Sandbox stopped",
            };
            f.log(crate::feature::now(), format!("{what} from {name}"));
        }
        self.sandbox_stopped(project, slug, ask.what, Ok(()));
    }

    /// What it takes to take down the sandbox `run` of `feature`, which is removed.
    pub(crate) fn sandbox_removal(&self, feature: &Feature, run: &SandboxRun) -> SandboxRemoval {
        let (project, slug) = (&feature.project, &feature.slug);
        let id = run.machine.as_deref();
        SandboxRemoval {
            run: run.clone(),
            host: self.sandbox_host(run),
            dir: self.sandbox_dir_of(project, slug, run),
            forget: id.and_then(|id| self.forget_script(id, feature)),
            left: id.map(|id| {
                let file = left_file(id, project, slug);
                (self.machine_name(id), file, Box::new(feature.clone()))
            }),
        }
    }

    /// Machine `id` is back: what removed features left on it goes.
    pub(crate) fn clean_left_there(&mut self, id: &str) {
        for (file, feature) in left_records(id) {
            let Some(run) = feature.sandbox.clone() else {
                continue;
            };
            let removal = self.sandbox_removal(&feature, &run);
            let title = feature.title.clone();
            let name = self.machine_name(id);
            self.feature_work(None, move |_| Done::SandboxLeftGone {
                result: removal.remove(),
                file,
                title,
                name,
            });
        }
    }

    /// Where stack `left` is, and its sandbox's folder on a machine.
    pub(crate) fn left_stack_place(
        &self,
        left: &feature_sandbox::LeftStack,
    ) -> (Result<Host, String>, Option<std::path::PathBuf>) {
        let Some(id) = left.machine.as_deref() else {
            return (Ok(Host::Here), None);
        };
        let Some(work) = self.work_of(id) else {
            return (Err(self.machine_name(id)), None);
        };
        let dir = (left.feature.split_once('/'))
            .and_then(|(project, slug)| self.data_there(id, project, slug))
            .map(|data| std::path::PathBuf::from(data).join("sandbox"));
        (Ok(Host::There(work.reach())), dir)
    }

    /// The titles of the features whose sandboxes this gerdr answers for on machine
    /// `name`: those running there, and those of removed features waiting for it.
    pub(crate) fn sandboxes_on(&self, name: &str) -> Vec<String> {
        let mut titles = Vec::new();
        for (id, _) in (self.machines.names.iter()).filter(|(_, called)| *called == name) {
            let runs = (self.features.all.values().flatten()).filter(|f| {
                (f.sandbox.as_ref()).is_some_and(|run| run.machine.as_deref() == Some(id))
            });
            titles.extend(runs.map(|f| f.title.clone()));
            titles.extend(left_records(id).into_iter().map(|(_, f)| f.title));
        }
        titles.sort();
        titles.dedup();
        titles
    }

    /// Machine `id` is forgotten: what it left is no longer said. Its records stay,
    /// for it to be cleaned if it is added again.
    pub(crate) fn left_there_forgotten(&mut self, id: &str) {
        let name = self.machine_name(id);
        for (_, feature) in left_records(id) {
            crate::problems::forget(feature_sandbox::PROBLEMS, &left_text(&feature.title, &name));
        }
        (self.machines.left_told).retain(|left| left.machine.as_deref() != Some(id));
        self.machines.left.remove(&Some(id.to_owned()));
    }

    /// What a removed feature left on a machine went, or still can't.
    pub(crate) fn sandbox_left_gone(
        &mut self,
        file: &std::path::Path,
        title: &str,
        name: &str,
        result: Result<(), String>,
    ) {
        if let Err(why) = result {
            // It is tried again when that machine is next back.
            tracing::warn!("what the sandbox of {title} left on {name}: {why}");
            return self.show_warning(format!(
                "The sandbox of \u{201c}{title}\u{201d} is still on {name}: {why}"
            ));
        }
        if let Err(err) = std::fs::remove_file(file) {
            tracing::warn!("removing {}: {err}", file.display());
        }
        crate::problems::forget(feature_sandbox::PROBLEMS, &left_text(title, name));
        self.show_success(format!(
            "The sandbox of \u{201c}{title}\u{201d} is gone from {name}"
        ));
    }

    /// Say what removed features left on machines, as a restart forgets having said.
    pub(crate) fn report_left_there(&self) {
        let data = crate::projects::data_dir();
        for id in crate::feature::view::kept_ids(&data) {
            for (_, feature) in left_records(&id) {
                let text = left_text(&feature.title, &self.machine_name(&id));
                crate::problems::report(feature_sandbox::PROBLEMS, text);
            }
        }
    }

    /// Do what another machine asked of feature `slug`'s sandbox, as if pressed.
    /// While it starts, it stops when that's the point.
    pub(crate) fn answer_sandbox_ask(&mut self, project: &str, slug: &str, ask: &lease::SandboxAsk) {
        let key = (project.to_owned(), slug.to_owned());
        if let Some(stop) = self.features.build.sandbox_stops.get(&key) {
            if matches!(ask.what, StopFor::Stop | StopFor::Remove) {
                stop.store(true, Ordering::Relaxed);
            }
            return;
        }
        let name = ask.name.clone();
        if let Some(f) = self.feature_mut(project, slug) {
            let now = crate::feature::now();
            let what = match ask.what {
                StopFor::Stop => "Sandbox stopped",
                StopFor::Remove => "Sandbox removed",
                StopFor::Restart => "Sandbox restarted",
                StopFor::Reset => "Sandbox data reset",
            };
            f.log(now, format!("{what} from {name}"));
        }
        self.save_feature(project, slug);
        self.finish_stop(project, slug, ask.what);
    }

    /// Note how a sandbox just started on a machine stands, until that's asked.
    pub(crate) fn note_sandbox_there(&mut self, project: &str, slug: &str, run: &SandboxRun) {
        let (Some(id), Some(there)) = (&run.machine, &run.elsewhere) else {
            return;
        };
        let key = (project.to_owned(), slug.to_owned());
        self.machines.sandboxes_there.insert(key, (id.clone(), there.clone()));
    }

    /// A machine said how the sandboxes asked of it stand.
    pub(crate) fn sandboxes_looked(&mut self, id: &str, said: Vec<(String, String, Elsewhere)>) {
        for (project, slug, there) in said {
            let key = (project, slug);
            self.machines.sandboxes_there.insert(key, (id.to_owned(), there));
        }
    }

    /// Ask each machine how the sandboxes on it stand, those not asked of through
    /// the machine writing them, and put what's known in their records.
    pub(crate) fn sandbox_places_tick(&mut self) {
        let features = (self.features.all.values().flatten())
            .chain(self.features.viewed.values().flatten());
        let mut asked: BTreeMap<String, Vec<(String, String, SandboxRun)>> = BTreeMap::new();
        let mut wanted: BTreeSet<(String, String)> = BTreeSet::new();
        for feature in features.filter(|f| f.status == Status::Open) {
            let Some(run) = feature.sandbox.as_ref() else {
                continue;
            };
            let Some(id) = run.machine.as_deref() else {
                continue;
            };
            if feature.writer.as_deref() == Some(id) {
                continue;
            }
            let key = (feature.project.clone(), feature.slug.clone());
            wanted.insert(key.clone());
            if sandbox_run::to_ask_there(run) {
                let (project, slug) = key;
                asked.entry(id.to_owned()).or_default().push((project, slug, run.clone()));
            }
        }
        let links = (self.machines.links.values()).filter_map(|link| link.work.as_ref());
        let up: BTreeSet<String> = links.clone().map(|work| work.id.clone()).collect();
        for work in links {
            work.ask_sandboxes(asked.remove(&work.id).unwrap_or_default());
        }
        (self.machines.sandboxes_there)
            .retain(|key, (id, _)| wanted.contains(key) && up.contains(id));
        let known = &self.machines.sandboxes_there;
        let features = (self.features.all.values_mut().flatten())
            .chain(self.features.viewed.values_mut().flatten());
        for feature in features {
            let key = (feature.project.clone(), feature.slug.clone());
            let Some(run) = feature.sandbox.as_mut().filter(|run| run.machine.is_some()) else {
                continue;
            };
            if feature.writer.is_some() && feature.writer == run.machine {
                continue;
            }
            run.elsewhere = Some(known.get(&key).map_or_else(Elsewhere::default, |(_, there)| there.clone()));
        }
    }

    /// Open what's asked for of the sandboxes running elsewhere, and close what
    /// no longer is. It follows what is so: it opens again after a lost link or a
    /// restart, with the page still there.
    pub(crate) fn sandbox_views_tick(&mut self) {
        let wanted: Vec<(String, String)> = (self.features.viewed.values().flatten())
            .chain(self.features.all.values().flatten())
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

    /// Open feature `slug`'s sandbox's app, which runs on machine `id`:
    /// its ports open here first, and a page opens on one that is really its own.
    pub(crate) fn open_seen_sandbox_app(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        page: usize,
    ) {
        let name = self.machine_name(id);
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(run) = Self::seen_sandbox(feature).map(|(_, run)| run.clone()) else {
            return self.show_warning(format!("The sandbox isn't running on {name}"));
        };
        if self.work_of(id).is_none() {
            return self.show_warning(format!("Connect {name} to open its sandbox's app"));
        }
        let ports = run.listening();
        let set = self.set_sandbox_url(project);
        let Some(url) = crate::feature_sandbox::sandbox_url(&run, page, &ports, set.as_deref())
        else {
            return self.show_warning(crate::feature_sandbox::nothing_listens(&run, page));
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

    /// Show the terminals of feature `slug`'s sandbox, which runs on another
    /// machine, each in a tab here. Closing those only ends the view.
    pub(crate) fn show_sandbox_terminals(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let on = feature.sandbox.as_ref().and_then(|run| run.machine.clone());
        let Some(id) = on.or_else(|| feature.writer.clone()) else {
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
        let id = match feature.sandbox.as_ref() {
            Some(run) => run.machine.as_deref()?,
            None => feature.writer.as_deref()?,
        };
        let name = self.machine_name(id);
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

    /// A relay on a port that was free. A port seen free can be taken by the time
    /// it is bound, and not only by another test: a process starting on another
    /// thread holds a copy of the closed probe until it has started.
    fn relay_on_free_port(
        command: impl Fn() -> Command + Clone + Send + 'static,
        missing: &Missing,
    ) -> (u16, Relay) {
        for _ in 0..50 {
            let port = free_port();
            if let Ok(relay) = start_relay(port, command.clone(), missing.clone()) {
                return (port, relay);
            }
        }
        panic!("no free port found");
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
        let (port, relay) = relay_on_free_port(|| sh("cat"), &Missing::default());
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
        // A process starting elsewhere can hold the closed port for a moment.
        let started = Instant::now();
        while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
            assert!(started.elapsed() < Duration::from_secs(5), "still open");
            std::thread::sleep(Duration::from_millis(20));
        }
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
        let missing = Missing::default();
        let (port, _relay) = relay_on_free_port(|| sh("exit 127"), &missing);
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
    fn a_record_kept_for_a_machine_says_what_to_take_down() {
        let dir = std::env::temp_dir().join(format!("gerdr-left-{}", std::process::id()));
        let file = dir.join("deep/p--s.json");
        let mut feature = crate::feature::tests::feature("s", 1);
        feature.sandbox = Some(SandboxRun {
            machine: Some("w".into()),
            started: 9,
            ..SandboxRun::default()
        });
        keep_left(&file, &feature).unwrap();
        let back: Feature = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(back.sandbox, feature.sandbox);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(left_text("T", "box").contains("box"));
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
