//! Other machines gerdr reaches over SSH, for agents and checks to run on.
//!
//! A machine is an SSH target, by a name of the user's, kept in `[machines]`
//! of the settings file. Each has a thread holding a link to it: an `ssh`
//! master connection, a forward to a herdr server there, and a watch on both.
//! That server has a session of its own (`WORKER_SESSION`), so a gerdr open
//! on that machine never sees what runs in it as its own tabs.
//!
//! Everything here runs off the event loop's thread: a call over the forward
//! takes about 100 ms even on a good link (herdr answers on a tick), and up
//! to its timeout on a dead one.
//!
//! Signing in stays with OpenSSH. gerdr never asks for a password nor keeps
//! one, and never turns off the check of a host's key: what SSH refuses is
//! said, with how to fix it (`Failure`).
//!
//! The dialog connecting a machine is `MachineDialog`; a machine saved is
//! kept only once it connected. Machines show as background tasks (see
//! `tasks`), where they are connected, disconnected and removed.

use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, Dot, MachineFocus, MachineHit, MachineView},
    desktop::shell_quote,
    feature_agents::WatchKey,
    herdr::Herdr,
    machine_work::{Work, WorkEvent},
    menu::MenuAction,
    tasks::Task,
    text_field::{Edit, TextField},
};

/// The herdr session work runs in on another machine.
pub const WORKER_SESSION: &str = "gerdr-worker";

/// The oldest herdr gerdr works with there.
const MIN_HERDR: (u32, u32) = (0, 9);
/// How far apart two machines' clocks may be: a run's times are compared
/// across them.
const CLOCK_SKEW: i64 = 2;
/// How long SSH gets to connect, and a command there to end.
const CONNECT_SECS: u32 = 10;
/// How long SSH gets to connect to each address of a machine that has
/// several: the office's, away, mustn't hold up home's.
const CONNECT_EACH_SECS: u32 = 5;
const COMMAND: Duration = Duration::from_secs(20);
/// How long a server just started gets to answer.
const SERVER_START: Duration = Duration::from_secs(6);
/// How often a link looks at its master, and how many looks between pings.
const LOOK: Duration = Duration::from_millis(500);
const LOOKS_PER_PING: u32 = 20;
/// Tries to reach a machine that was lost: the wait starts here and doubles.
const RETRY_FIRST: u64 = 5;
const RETRY_MOST: u64 = 60;

/// Why a machine can't be reached, as the user can act on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Not something `ssh` takes as a target.
    BadTarget,
    NoSsh,
    NoRuntime,
    /// SSH doesn't know its key, or it changed.
    HostKey,
    /// It took none of the user's keys.
    Auth,
    /// What SSH said.
    Unreachable(String),
    NoHerdr,
    /// The version it has.
    OldHerdr(String),
    /// Seconds its clock is off by.
    Clock(i64),
    /// Its SSH server refuses forwarding.
    Forwarding,
    /// The log it would say why in, on that machine.
    ServerStart(String),
    /// The link dropped after it was up.
    LinkLost,
    /// herdr stopped answering, the link still up.
    HerdrDown,
    TimedOut,
    /// Stopped by the user: nothing to say.
    Stopped,
    /// A new address of the machine of this name answered as another
    /// machine (another gerdr machine id): never tried again.
    OtherMachine(String),
    Other(String),
}

/// What a button can do about a `Failure`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Run this in a new tab: SSH asks there.
    Terminal(String),
    /// Copy this command, to run where it says.
    Copy(String),
}

impl Fix {
    /// The label of its button.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Terminal(_) => "Open terminal",
            Self::Copy(_) => "Copy command",
        }
    }
}

impl Failure {
    /// What went wrong, as a sentence.
    pub fn why(&self) -> String {
        match self {
            Self::BadTarget => "Write it as user@host, the way you'd give it to ssh.".into(),
            Self::NoSsh => "gerdr can't find ssh on this machine.".into(),
            Self::NoRuntime => "gerdr has no XDG_RUNTIME_DIR to keep the connection in.".into(),
            Self::HostKey => "SSH doesn't trust this machine's host key.".into(),
            Self::Auth => "It didn't accept any of your SSH keys.".into(),
            Self::Unreachable(line) => {
                let line = line.strip_prefix("ssh: ").unwrap_or(line);
                format!("Can't reach it: {}.", line.trim_end_matches('.'))
            }
            Self::NoHerdr => "herdr isn't installed there.".into(),
            Self::OldHerdr(version) => format!(
                "It has herdr {version}; gerdr needs {}.{} or newer.",
                MIN_HERDR.0, MIN_HERDR.1
            ),
            Self::Clock(secs) => format!("Its clock is {} s off from this one's.", secs.abs()),
            Self::Forwarding => "Its SSH server doesn't allow forwarding.".into(),
            Self::ServerStart(_) => "herdr wouldn't start there.".into(),
            Self::LinkLost => "The SSH connection dropped.".into(),
            Self::HerdrDown => "herdr stopped answering there.".into(),
            Self::TimedOut => "It didn't answer in time.".into(),
            Self::Stopped => "Stopped.".into(),
            Self::OtherMachine(name) => {
                format!("That address reaches another machine, not {name}.")
            }
            Self::Other(line) => format!("SSH said: {}.", line.trim_end_matches('.')),
        }
    }

    /// What to do about it, as a sentence, when that isn't plain.
    pub fn how(&self, target: &str) -> Option<String> {
        Some(match self {
            Self::HostKey => "Run ssh there once in a terminal and check the key it shows.".into(),
            Self::Auth => {
                format!("Add a key with ssh-copy-id {target}. A locked key needs ssh-add.")
            }
            Self::NoHerdr => "Install herdr there, then try again.".into(),
            Self::OldHerdr(_) => "Run herdr update there, then try again.".into(),
            Self::Clock(_) => "Turn on time sync on the machine that is off.".into(),
            Self::Forwarding => {
                "Set AllowTcpForwarding yes in its sshd_config, then try again.".into()
            }
            Self::ServerStart(log) => format!("Its log there: {log}"),
            Self::OtherMachine(_) => {
                "Check the address, or add that machine as one of its own.".into()
            }
            _ => return None,
        })
    }

    /// The button fixing it, if one can help.
    pub fn fix(&self, target: &str) -> Option<Fix> {
        match self {
            Self::HostKey => Some(Fix::Terminal(format!("ssh {}", shell_quote(target)))),
            Self::Auth => Some(Fix::Copy(format!("ssh-copy-id {}", shell_quote(target)))),
            Self::OldHerdr(_) => Some(Fix::Copy("herdr update".into())),
            _ => None,
        }
    }

    /// Whether trying again by itself may help: the machine may just be
    /// away. What the user must fix isn't tried over and over.
    pub(crate) fn retries(&self) -> bool {
        matches!(
            self,
            Self::Unreachable(_) | Self::LinkLost | Self::HerdrDown | Self::TimedOut
        )
    }
}

/// What SSH's error output means.
pub fn classify(stderr: &str) -> Failure {
    let has = |what: &str| stderr.contains(what);
    if has("Host key verification failed") || has("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        return Failure::HostKey;
    }
    if has("Permission denied") || has("Too many authentication failures") {
        return Failure::Auth;
    }
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it ended without saying why")
        .to_owned();
    let away = [
        "Could not resolve hostname",
        "Connection refused",
        "Connection timed out",
        "Connection closed",
        "Connection reset",
        "timed out",
        "No route to host",
        "Network is unreachable",
    ];
    if away.iter().any(|what| has(what)) {
        return Failure::Unreachable(line);
    }
    Failure::Other(line)
}

/// Whether `target` is something to hand `ssh`: one word that can't be taken
/// for an option (`-oProxyCommand=…` would run a command).
pub fn valid_target(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('-')
        && !target.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The name a machine gets unless the user gives one: its host.
pub fn default_name(target: &str) -> String {
    let target = target.strip_prefix("ssh://").unwrap_or(target);
    let host = target.rsplit('@').next().unwrap_or(target);
    host.split('/').next().unwrap_or(host).to_owned()
}

/// What a look around a machine found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub home: String,
    /// Its `XDG_CONFIG_HOME`: herdr's sessions are under it.
    pub config: String,
    /// Its `XDG_DATA_HOME`.
    pub data: String,
    /// Where herdr is, found through its login shell's PATH: a plain
    /// `ssh host command` often doesn't have it.
    pub herdr: Option<String>,
    pub version: String,
    /// Which machine it is, whoever calls it what.
    pub id: String,
    /// Its clock, in Unix seconds.
    pub now: i64,
}

impl Site {
    /// The socket of the worker session's server there.
    pub(crate) fn socket(&self) -> String {
        self.socket_of(WORKER_SESSION)
    }

    /// The socket of session `session`'s server there.
    pub(crate) fn socket_of(&self, session: &str) -> String {
        format!("{}/herdr/sessions/{session}/herdr.sock", self.config)
    }

    fn session(&self) -> String {
        format!("{}/herdr/sessions/{WORKER_SESSION}", self.config)
    }

    /// Why this machine can't take work, if it can't.
    fn problem(&self, clock_off: i64) -> Option<Failure> {
        if self.herdr.is_none() {
            return Some(Failure::NoHerdr);
        }
        if !new_enough(&self.version) {
            return Some(Failure::OldHerdr(self.version.clone()));
        }
        (clock_off.abs() > CLOCK_SKEW).then_some(Failure::Clock(clock_off))
    }
}

/// Whether a version like "0.9.5" is one gerdr works with.
fn new_enough(version: &str) -> bool {
    let mut parts = version.split('.').map(|part| part.parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= MIN_HERDR,
        _ => false,
    }
}

/// The script looking around a machine, for `sh` there: it prints what
/// `parse_site` reads, and leaves the machine an id (`new_id`) if it has
/// none. Noise from a login shell's startup files doesn't matter: only the
/// lines of `env` asked for are kept.
fn look_script(new_id: &str, leave: &str) -> String {
    format!(
        r#"e=$("${{SHELL:-/bin/sh}}" -lc env </dev/null 2>/dev/null)
pick() {{ printf '%s\n' "$e" | sed -n "s/^$1=//p" | tail -n 1; }}
p=$(pick PATH); d=$(pick XDG_DATA_HOME); c=$(pick XDG_CONFIG_HOME)
d=${{d:-$HOME/.local/share}}; c=${{c:-$HOME/.config}}
echo "HOME=$HOME"; echo "DATA=$d"; echo "CONFIG=$c"
h=$(PATH="$p:$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; export PATH; command -v herdr)
echo "HERDR=$h"
if [ -n "$h" ]; then echo "VERSION=$("$h" --version 2>/dev/null)"; fi
f="$d/gerdr/machine-id"
mkdir -p "$d/gerdr" && {{ [ -s "$f" ] || echo {new_id} > "$f"; }}
{leave}echo "MACHINE=$(cat "$f" 2>/dev/null)"
echo "NOW=$(date +%s)"
"#
    )
}

/// The socket of this machine's own worker session, where another gerdr's work
/// runs here.
pub(crate) fn worker_socket_here() -> PathBuf {
    crate::herdr::socket_path(WORKER_SESSION)
}

/// What `look_script` printed.
pub fn parse_site(text: &str) -> Site {
    let mut site = Site::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "HOME" => site.home = value.to_owned(),
            "DATA" => site.data = value.to_owned(),
            "CONFIG" => site.config = value.to_owned(),
            "HERDR" if value.starts_with('/') => site.herdr = Some(value.to_owned()),
            "VERSION" => {
                site.version = value
                    .strip_prefix("herdr ")
                    .unwrap_or(value)
                    .trim()
                    .to_owned()
            }
            "MACHINE" => site.id = value.to_owned(),
            "NOW" => site.now = value.parse().unwrap_or_default(),
            _ => {}
        }
    }
    site
}

/// What is left to say about a machine that is up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkInfo {
    pub site: Site,
    /// The forward to its worker session's server, here.
    pub socket: PathBuf,
    /// The master connection's control socket, for later `ssh -S`.
    pub control: PathBuf,
}

/// What a link's thread tells.
#[derive(Clone, Debug)]
pub enum LinkEvent {
    /// How far connecting to address `target` got.
    Step { target: String, text: String },
    /// Up at address `target`.
    Up { target: String, info: LinkInfo },
    /// It went down, or couldn't come up at any address: `target` is the
    /// one `failure` is for. `retrying` if it keeps trying.
    Down {
        target: String,
        failure: Failure,
        retrying: bool,
    },
}

/// A folder of this gerdr's sockets to machines; each gerdr has its own
/// masters, so one quitting doesn't take the others' links.
fn links_dir() -> Option<PathBuf> {
    Some(crate::herdr::runtime_dir()?.join("gerdr/m"))
}

/// The start of this gerdr's socket names for a connection to `target`: a
/// Unix socket's path must stay short, so it isn't the target itself. Each
/// connection has its own: one ending removes its sockets after the next to
/// the same machine started, and must not take that one's.
fn link_stem(dir: &Path, target: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    target.hash(&mut hash);
    dir.join(format!(
        "{}-{n}-{:08x}",
        std::process::id(),
        hash.finish() & 0xffff_ffff
    ))
}

/// Remove the sockets of gerdrs that are gone.
fn clear_leftovers() {
    let Some(dir) = links_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let pid = name.split('-').next().unwrap_or_default();
        if !pid.is_empty() && !Path::new("/proc").join(pid).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `ssh` over the master at `control`.
pub(crate) fn over_master(control: &Path, target: &str) -> Command {
    let mut ssh = Command::new("ssh");
    ssh.arg("-S")
        .arg(control)
        .args(["-o", "BatchMode=yes", "-o", "ControlMaster=no"])
        .arg(target);
    ssh
}

/// A command for the POSIX shell there, whatever the login shell is.
fn in_sh(script: &str) -> String {
    format!("sh -c {}", shell_quote(script))
}

/// Run `command` to its end, or until `limit` or `cancel`.
pub(crate) fn run(
    command: Command,
    limit: Duration,
    cancel: &AtomicBool,
) -> Result<Output, Failure> {
    run_with_input(command, &[], limit, cancel)
}

/// `run`, with `input` on its standard input.
pub(crate) fn run_with_input(
    mut command: Command,
    input: &[u8],
    limit: Duration,
    cancel: &AtomicBool,
) -> Result<Output, Failure> {
    command
        .stdin(match input.is_empty() {
            true => Stdio::null(),
            false => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => Failure::NoSsh,
        _ => Failure::Other(err.to_string()),
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        // On its own thread: a pipe full of what isn't read yet mustn't hold
        // up the limit below. It ends with the child, whatever it wrote.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let out = reader(child.stdout.take().map(|p| Box::new(p) as _));
    let err = reader(child.stderr.take().map(|p| Box::new(p) as _));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(err) => return Err(Failure::Other(err.to_string())),
        }
        let failure = if cancel.load(Ordering::Relaxed) {
            Some(Failure::Stopped)
        } else if start.elapsed() > limit {
            Some(Failure::TimedOut)
        } else {
            None
        };
        if let Some(failure) = failure {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// A machine connected: dropped, its connection ends and its sockets go.
struct Connection {
    master: Child,
    info: LinkInfo,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.master.kill();
        let _ = self.master.wait();
        let _ = std::fs::remove_file(&self.info.control);
        let _ = std::fs::remove_file(&self.info.socket);
    }
}

/// A link up, as its thread holds it: a `Connection`, or a stand-in in
/// tests.
trait Held {
    fn info(&self) -> &LinkInfo;
    /// Wait for the link to end, and say how.
    fn hold(&mut self, cancel: &AtomicBool) -> Failure;
}

impl Held for Connection {
    fn info(&self) -> &LinkInfo {
        &self.info
    }

    fn hold(&mut self, cancel: &AtomicBool) -> Failure {
        Connection::hold(self, cancel)
    }
}

impl Connection {
    /// Wait for the link to end, and say how.
    fn hold(&mut self, cancel: &AtomicBool) -> Failure {
        let mut herdr = Herdr::on_socket(self.info.socket.clone());
        let mut looks = 0;
        let mut missed = 0;
        loop {
            std::thread::sleep(LOOK);
            if cancel.load(Ordering::Relaxed) {
                return Failure::Stopped;
            }
            if !matches!(self.master.try_wait(), Ok(None)) {
                return Failure::LinkLost;
            }
            looks += 1;
            if looks < LOOKS_PER_PING {
                continue;
            }
            looks = 0;
            missed = match herdr.call("ping", json!({})) {
                Ok(_) => 0,
                Err(_) => missed + 1,
            };
            // Once could be the link dropping: the master says so soon.
            if missed >= 2 {
                return Failure::HerdrDown;
            }
        }
    }
}

/// Start the master connection to `target`, and wait for it to take
/// commands. SSH gets `secs` to connect.
fn start_master(
    target: &str,
    control: &Path,
    secs: u32,
    cancel: &AtomicBool,
) -> Result<Child, Failure> {
    let _ = std::fs::remove_file(control);
    let mut ssh = Command::new("ssh");
    ssh.args(["-N", "-M", "-S"])
        .arg(control)
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ControlPersist=no",
            "-o",
            "StreamLocalBindUnlink=yes",
            "-o",
            "ServerAliveInterval=3",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
        ])
        .arg(format!("ConnectTimeout={secs}"))
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // It goes when this thread does, gerdr crashing or not: an `ssh` left
    // behind would hold a connection nothing uses. No thread of this link
    // ends while the master is needed.
    unsafe {
        ssh.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut master = ssh.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => Failure::NoSsh,
        _ => Failure::Other(err.to_string()),
    })?;
    let stderr = master.stderr.take();
    let said = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    let start = Instant::now();
    let limit = Duration::from_secs(secs as u64 + 5);
    let fail = |master: &mut Child, failure: Failure| {
        let _ = master.kill();
        let _ = master.wait();
        failure
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(fail(&mut master, Failure::Stopped));
        }
        if !matches!(master.try_wait(), Ok(None)) {
            return Err(classify(&said.join().unwrap_or_default()));
        }
        let mut check = over_master(control, target);
        check.args(["-O", "check"]);
        let up = control.exists()
            && run(check, Duration::from_secs(3), cancel).is_ok_and(|out| out.status.success());
        if up {
            return Ok(master);
        }
        if start.elapsed() > limit {
            return Err(fail(&mut master, Failure::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether the server behind `socket` answers.
fn answers(socket: &Path) -> bool {
    Herdr::on_socket(socket.to_path_buf())
        .call("ping", json!({}))
        .is_ok()
}

/// The first step connecting to `target` tells, and how long SSH gets to
/// connect: one of `several` addresses is named, and given less time.
fn first_step(target: &str, several: bool) -> (String, u32) {
    match several {
        true => (format!("trying {target}\u{2026}"), CONNECT_EACH_SECS),
        false => ("reaching it over SSH\u{2026}".into(), CONNECT_SECS),
    }
}

/// Connect to `target`, one of `several` addresses or not, all the way,
/// telling each step.
fn connect(
    target: &str,
    several: bool,
    cancel: &AtomicBool,
    step: &mut dyn FnMut(String),
) -> Result<Connection, Failure> {
    if !valid_target(target) {
        return Err(Failure::BadTarget);
    }
    let dir = links_dir().ok_or(Failure::NoRuntime)?;
    create_private_dir(&dir).map_err(|err| Failure::Other(err.to_string()))?;
    let stem = link_stem(&dir, target);
    let control = stem.with_extension("ctl");
    let socket = stem.with_extension("sock");
    let (first, secs) = first_step(target, several);
    step(first);
    let master = start_master(target, &control, secs, cancel)?;
    let info = LinkInfo {
        site: Site::default(),
        socket,
        control,
    };
    // From here it cleans up after itself, however it ends.
    let mut link = Connection { master, info };
    step("SSH reached, looking around\u{2026}".into());
    let before = crate::clock::now_secs() as i64;
    let new_id = format!(
        "{:x}{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        std::process::id()
    );
    let mut look = over_master(&link.info.control, target);
    // Leave word there of this gerdr, for that machine's own gerdr.
    let leave = (crate::machine_lease::own_id_made()).map_or_else(String::new, |own| {
        let user = std::env::var("USER").unwrap_or_default();
        crate::machine_reach::leave_lines(&own, &crate::machine_lease::this_machines_name(), &user)
    });
    look.arg(in_sh(&look_script(&new_id, &leave)));
    let out = run(look, COMMAND, cancel)?;
    if !out.status.success() {
        return Err(classify(&String::from_utf8_lossy(&out.stderr)));
    }
    let site = parse_site(&String::from_utf8_lossy(&out.stdout));
    let clock_off = site.now - (before + crate::clock::now_secs() as i64) / 2;
    if let Some(failure) = site.problem(clock_off) {
        return Err(failure);
    }
    let herdr = site.herdr.clone().unwrap_or_default();
    step(format!(
        "herdr {}, opening a way to it\u{2026}",
        site.version
    ));
    let mut forward = over_master(&link.info.control, target);
    forward
        .args(["-o", "StreamLocalBindUnlink=yes", "-O", "forward", "-L"])
        .arg(format!("{}:{}", link.info.socket.display(), site.socket()));
    let out = run(forward, COMMAND, cancel)?;
    if !out.status.success() {
        return Err(Failure::Forwarding);
    }
    if !answers(&link.info.socket) {
        step(format!(
            "herdr {}, starting its server\u{2026}",
            site.version
        ));
        start_server(&link.info.control, target, &herdr, cancel)?;
        let start = Instant::now();
        while !answers(&link.info.socket) {
            if cancel.load(Ordering::Relaxed) {
                return Err(Failure::Stopped);
            }
            if start.elapsed() > SERVER_START {
                return Err(why_not_answering(&link.info.control, target, &site, cancel));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    link.info.site = site;
    Ok(link)
}

/// Start the worker session's server there, in a login shell: what runs in
/// its panes then finds what the user's own shells find. It outlives the
/// link, as what runs in it must.
fn start_server(
    control: &Path,
    target: &str,
    herdr: &str,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    let server = format!(
        "setsid -f {} --session {WORKER_SESSION} server </dev/null >/dev/null 2>&1",
        shell_quote(herdr)
    );
    let script = format!("exec \"${{SHELL:-/bin/sh}}\" -lc {}", shell_quote(&server));
    let mut start = over_master(control, target);
    start.arg(in_sh(&script));
    let out = run(start, COMMAND, cancel)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(classify(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// The server's socket is there, or isn't: there and silent means the
/// forward is what's refused (`AllowTcpForwarding no` denies a socket's too,
/// which the client sees only as a connection reset).
fn why_not_answering(control: &Path, target: &str, site: &Site, cancel: &AtomicBool) -> Failure {
    let mut test = over_master(control, target);
    test.arg(in_sh(&format!("[ -S {} ]", shell_quote(&site.socket()))));
    match run(test, COMMAND, cancel) {
        Ok(out) if out.status.success() => Failure::Forwarding,
        Ok(_) => Failure::ServerStart(format!("{}/herdr-server.log", site.session())),
        Err(failure) => failure,
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// The addresses in the order a link tries them: as the machine has them,
/// but the one that answered `last` first while it is still one of them.
pub(crate) fn try_order(addresses: &[String], last: Option<&str>) -> Vec<String> {
    let mut order = addresses.to_vec();
    if let Some(at) = last.and_then(|last| order.iter().position(|a| a == last)) {
        let last = order.remove(at);
        order.insert(0, last);
    }
    order
}

/// How much `failure` says of why a machine can't be reached, of what
/// several addresses came to: one away (timed out, refused) says least, as
/// some address of a machine that has several usually is; another machine
/// answering says most.
fn telling(failure: &Failure) -> u8 {
    match failure {
        Failure::TimedOut => 0,
        Failure::Unreachable(_) => 1,
        Failure::OtherMachine(_) => 3,
        _ => 2,
    }
}

/// What a link says when none of its addresses came up, of what each came
/// to (in the order tried): the first of those saying most, with its
/// address. None for no address.
fn most_telling(tries: Vec<(String, Failure)>) -> Option<(String, Failure)> {
    let most = tries.iter().map(|(_, f)| telling(f)).max()?;
    tries.into_iter().find(|(_, f)| telling(f) == most)
}

/// Try the addresses `order` of machine `name` in turn with `connect`,
/// until one answers as that machine, one of the ids `known` (any, for a
/// machine never seen: see `machine_edit::same_machine`), which `id` reads
/// off what `connect` gave. That address and what it gave; an address
/// answering as another machine is passed over, as `OtherMachine`. Else
/// what each came to, in order; stopped, only that.
fn try_addresses<C>(
    name: &str,
    order: &[String],
    known: &[String],
    mut connect: impl FnMut(&str) -> Result<C, Failure>,
    id: impl Fn(&C) -> &str,
) -> Result<(String, C), Vec<(String, Failure)>> {
    let mut tries = Vec::new();
    for target in order {
        match connect(target) {
            Ok(up) if crate::machine_edit::same_machine(known, id(&up)) => {
                return Ok((target.clone(), up));
            }
            // Dropped, its connection ends.
            Ok(up) => {
                tracing::warn!(
                    "machine {name}: {target} answers as machine {}, not as one of {known:?}: passed over",
                    id(&up)
                );
                tries.push((target.clone(), Failure::OtherMachine(name.to_owned())));
            }
            Err(Failure::Stopped) => return Err(vec![(target.clone(), Failure::Stopped)]),
            Err(failure) => {
                tracing::info!("machine {name}: {target}: {failure:?}");
                tries.push((target.clone(), failure));
            }
        }
    }
    Err(tries)
}

/// A link's thread to machine `name`: connect to the first of the
/// addresses `order` that comes up as that machine (one of the ids `known`),
/// hold it, and, if `retry` or it was ever up, try them all again while the
/// machine may just be away, the one that answered last first.
fn run_link(
    name: String,
    order: Vec<String>,
    known: Vec<String>,
    retry: bool,
    cancel: Arc<AtomicBool>,
    tx: impl Fn(LinkEvent),
) {
    run_link_with(name, order, known, retry, &cancel, tx, connect);
}

/// `run_link`, connecting with `connect` (see `connect`).
fn run_link_with<H: Held>(
    name: String,
    mut order: Vec<String>,
    mut known: Vec<String>,
    retry: bool,
    cancel: &AtomicBool,
    tx: impl Fn(LinkEvent),
    mut connect: impl FnMut(&str, bool, &AtomicBool, &mut dyn FnMut(String)) -> Result<H, Failure>,
) {
    let mut wait = RETRY_FIRST;
    let mut was_up = false;
    let several = order.len() > 1;
    loop {
        let tried = try_addresses(
            &name,
            &order,
            &known,
            |target| {
                connect(target, several, cancel, &mut |text| {
                    let target = target.to_owned();
                    tx(LinkEvent::Step { target, text });
                })
            },
            |link: &H| link.info().site.id.as_str(),
        );
        let (target, failure, retries) = match tried {
            Ok((target, mut link)) => {
                let info = link.info().clone();
                // Seen now: another machine at its addresses is never it.
                if known.is_empty() && !info.site.id.is_empty() {
                    known.push(info.site.id.clone());
                }
                tx(LinkEvent::Up {
                    target: target.clone(),
                    info,
                });
                was_up = true;
                wait = RETRY_FIRST;
                order = try_order(&order, Some(&target));
                let failure = link.hold(cancel);
                let retries = failure.retries();
                (target, failure, retries)
            }
            Err(tries) => {
                // Any address may come back, whatever the others said.
                let retries = tries.iter().any(|(_, f)| f.retries());
                match most_telling(tries) {
                    Some((target, failure)) => (target, failure, retries),
                    None => (String::new(), Failure::BadTarget, false),
                }
            }
        };
        if failure == Failure::Stopped {
            return;
        }
        let retrying = (retry || was_up) && retries;
        tx(LinkEvent::Down {
            target,
            failure,
            retrying,
        });
        if !retrying {
            return;
        }
        let until = Instant::now() + Duration::from_secs(wait);
        while Instant::now() < until {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(LOOK);
        }
        wait = (wait * 2).min(RETRY_MOST);
    }
}

/// Where a machine's link is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Saying how far it got.
    Connecting(String),
    Up(LinkInfo),
    /// Lost or away; its thread keeps trying.
    Retrying(Failure),
    /// Given up: the user has to do something.
    Failed(Failure),
    /// Disconnected by the user.
    Off,
}

/// A machine's link, while it has a thread.
#[derive(Debug)]
pub(crate) struct Link {
    /// The address it uses: the one that answered while it is up, else the
    /// one it is trying.
    target: String,
    /// Every address of the machine, in the order they are tried.
    pub(crate) addresses: crate::config::Addresses,
    /// Which thread it is: what an older one says is stale. Its events find
    /// it by this, not by its name, which a rename changes.
    pub(crate) serial: u64,
    cancel: Arc<AtomicBool>,
    pub(crate) state: LinkState,
    /// It was up and went away: said, so its coming back is too. Away from
    /// the start is what its dot says.
    lost: bool,
    /// What runs on the machine, while the link is up.
    pub(crate) work: Option<Work>,
    /// When the link came up, while it is.
    pub(crate) connected_since: Option<Instant>,
    /// When the machine last said how it is: up, or a look around it.
    pub(crate) last_seen: Option<Instant>,
}

impl Link {
    fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The address it connects to.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }
}

/// The machines, and the dialog connecting one.
#[derive(Debug, Default)]
pub struct Machines {
    /// By name, as the settings file has them.
    pub(crate) saved: BTreeMap<String, crate::config::Addresses>,
    pub(crate) links: HashMap<String, Link>,
    next_serial: u64,
    tx: Option<channel::Sender<(String, u64, LinkEvent)>>,
    pub dialog: Option<MachineDialog>,
    /// What was typed when the dialog closed: address, name.
    pub(crate) draft: Option<(String, String)>,
    /// Where links' work threads say things.
    pub(crate) work_tx: Option<channel::Sender<(String, WorkEvent)>>,
    /// The name of each machine seen connected, by its id: what its runs are
    /// called by when it isn't.
    pub(crate) names: HashMap<String, String>,
    /// The runs whose agents are on their way up on a machine.
    pub(crate) starting: HashMap<WatchKey, String>,
    /// The local pane showing each pane on a machine.
    pub(crate) views: HashMap<String, String>,
    /// What's open here of sandboxes elsewhere, by project and slug.
    pub(crate) sandboxes: HashMap<(String, String), crate::machine_sandbox::SandboxView>,
    /// How sandboxes on other machines stand, by the machine that said, for the
    /// features whose record this gerdr holds apart from that machine.
    pub(crate) sandboxes_there:
        HashMap<(String, String), (String, crate::feature::sandbox_run::Elsewhere)>,
    /// When this gerdr last looked for gerdrs that reached this machine.
    pub(crate) reach_looked: Option<Instant>,
    /// The repositories looked for on each machine, by its id and the project:
    /// a look goes again once the project uses others (see `machine_repos`).
    pub(crate) repos_looked: HashMap<(String, String), Vec<std::path::PathBuf>>,
    /// What sandboxes of gone features left, by the machine they are on (none:
    /// here), as last looked for: the tasks panel offers to remove each.
    pub(crate) left: HashMap<Option<String>, Vec<crate::feature_sandbox::LeftStack>>,
    /// What gone features left that was already offered to be removed.
    pub(crate) left_told: std::collections::HashSet<crate::feature_sandbox::LeftStack>,
    /// When a line was last typed to a pane on a machine.
    pub(crate) lines: HashMap<String, Instant>,
    /// What keeps each feature's work from coming back.
    pub(crate) troubles: HashMap<(String, String), String>,
    /// The new address of a machine tried from the machines panel's edit
    /// form, while it is: kept only once that machine answers there.
    pub(crate) editing: Option<crate::machine_edit::EditTry>,
}

impl Machines {
    /// The name of the machine whose link thread is `serial`, if it still has
    /// that one.
    pub(crate) fn link_named(&self, serial: u64) -> Option<String> {
        (self.links.iter())
            .find(|(_, link)| link.serial == serial)
            .map(|(name, _)| name.clone())
    }

    /// The machine called `old` is called `new` now: its address, its link
    /// (left running: its thread's events find it by serial) and the ids it
    /// was seen by move over. The ids moved.
    pub(crate) fn rename(&mut self, old: &str, new: &str) -> Vec<String> {
        if let Some(target) = self.saved.remove(old) {
            self.saved.insert(new.to_owned(), target);
        }
        if let Some(link) = self.links.remove(old) {
            self.links.insert(new.to_owned(), link);
        }
        let mut ids = Vec::new();
        for (id, name) in &mut self.names {
            if name == old {
                *name = new.to_owned();
                ids.push(id.clone());
            }
        }
        ids.sort();
        ids
    }

    /// Stop machine `name`'s link and leave it disconnected, as the user
    /// would: what it last said of itself stays.
    pub(crate) fn link_off(&mut self, name: &str) {
        if let Some(link) = self.links.get_mut(name) {
            link.stop();
            link.work = None;
            link.connected_since = None;
            link.state = LinkState::Off;
        }
    }
}

/// What the dialog asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineOutcome {
    /// Closed, what was typed kept.
    Cancel,
    Connect,
    /// Stop connecting; the dialog stays.
    Stop,
    /// Press the button fixing what failed.
    Fix,
}

/// The connect dialog's state, while it is open.
#[derive(Debug, Default)]
pub struct MachineDialog {
    target: TextField,
    name: TextField,
    focus: MachineFocus,
    /// The name being connected, while it is.
    attempt: Option<String>,
    progress: Option<String>,
    /// Why the last try failed: the target it was for, and what happened.
    failed: Option<(String, Failure)>,
    /// What is wrong with the form.
    error: Option<String>,
    hover: Option<MachineHit>,
}

/// The dialog's buttons, left to right, the last the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Cancel,
    Stop,
    Fix,
    Connect,
}

impl MachineDialog {
    fn buttons(&self) -> Vec<Button> {
        if self.attempt.is_some() {
            return vec![Button::Stop];
        }
        let mut buttons = vec![Button::Cancel];
        if self.fix().is_some() {
            buttons.push(Button::Fix);
        }
        buttons.push(Button::Connect);
        buttons
    }

    fn fix(&self) -> Option<Fix> {
        let (target, failure) = self.failed.as_ref()?;
        failure.fix(target)
    }

    fn field_mut(&mut self) -> &mut TextField {
        match self.focus {
            MachineFocus::Target => &mut self.target,
            MachineFocus::Name => &mut self.name,
        }
    }

    fn key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Option<MachineOutcome> {
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let alt = mods.alt && !mods.ctrl && !mods.logo;
        let busy = self.attempt.is_some();
        match sym {
            // Esc steps back: first what runs, then the dialog.
            Keysym::Escape if busy => return Some(MachineOutcome::Stop),
            Keysym::Escape => return Some(MachineOutcome::Cancel),
            // Fields wait while it connects, as what it connects to is shown.
            _ if busy => {}
            Keysym::Return | Keysym::KP_Enter => return Some(MachineOutcome::Connect),
            Keysym::x | Keysym::X if alt && self.fix().is_some() => {
                return Some(MachineOutcome::Fix);
            }
            Keysym::Tab
            | Keysym::ISO_Left_Tab
            | Keysym::Up
            | Keysym::Down
            | Keysym::KP_Up
            | Keysym::KP_Down => self.focus = self.focus.other(),
            Keysym::j | Keysym::J | Keysym::k | Keysym::K if ctrl => {
                self.focus = self.focus.other();
            }
            _ => {
                if self.field_mut().key(mods, sym, typed) == Edit::Changed {
                    self.error = None;
                }
            }
        }
        None
    }

    fn view(&self) -> MachineView {
        let body = self.progress.clone().unwrap_or_else(|| {
            "Agents and checks can run there. It uses your SSH keys.".to_owned()
        });
        let target = self.target.trim();
        let (error, how) = match (&self.error, &self.failed) {
            (Some(error), _) => (Some(error.clone()), None),
            (None, Some((target, failure))) => (Some(failure.why()), failure.how(target)),
            (None, None) => (None, None),
        };
        let buttons = self
            .buttons()
            .into_iter()
            .map(|button| match button {
                Button::Cancel => "Cancel".to_owned(),
                Button::Stop => "Stop".to_owned(),
                Button::Fix => self
                    .fix()
                    .map_or_else(String::new, |f| f.label().to_owned()),
                Button::Connect if self.failed.is_some() => "Try again".to_owned(),
                Button::Connect => "Connect".to_owned(),
            })
            .collect();
        let mut keys = Vec::new();
        if self.attempt.is_some() {
            keys.push(("Esc", "stop"));
        } else {
            keys.push(("Tab", "next"));
            if self.fix().is_some() {
                keys.push(("Alt+X", "fix"));
            }
            keys.push(("Esc", "close"));
        }
        MachineView {
            body,
            target: self.target.line(),
            name: self.name.line(),
            name_hint: default_name(target),
            focus: self.focus,
            error,
            how,
            buttons,
            busy: self.attempt.is_some(),
            hover: self.hover,
            keys,
        }
    }
}

impl Gerdr {
    /// Connect the machines saved, and clear what gerdrs before this one
    /// left behind.
    pub fn watch_machines(&mut self, saved: BTreeMap<String, crate::config::Addresses>) {
        clear_leftovers();
        self.set_saved_machines(saved);
        self.load_viewed();
        let projects: Vec<String> = (self.projects.projects.iter())
            .map(|p| p.id.clone())
            .collect();
        for project in projects {
            self.show_left(&project);
        }
    }

    /// The settings say these machines: start what is new or changed, and
    /// stop what is gone. A machine disconnected stays so while unchanged.
    pub fn set_saved_machines(&mut self, saved: BTreeMap<String, crate::config::Addresses>) {
        let old = std::mem::replace(&mut self.machines.saved, saved.clone());
        for (name, target) in &old {
            if saved.get(name) != Some(target)
                && let Some(link) = self.machines.links.remove(name)
            {
                link.stop();
            }
            // Taken out of the settings, it is removed like from the dialog,
            // which says what stays there.
            if !saved.contains_key(name) {
                let stays = self.sandboxes_on(name);
                if !stays.is_empty() {
                    self.show_warning(format!(
                        "{name} is out of the settings, and keeps the sandboxes of {}",
                        stays.join(", ")
                    ));
                }
                self.forget_viewed_of(name);
            }
        }
        for (name, target) in saved {
            if old.get(&name) != Some(&target) && !self.machines.links.contains_key(&name) {
                self.start_link(name, target, true);
            }
        }
    }

    /// Where a link's thread sends what it comes to; set up the first time.
    fn machines_sender(&mut self) -> Option<channel::Sender<(String, u64, LinkEvent)>> {
        if self.machines.tx.is_none() {
            let (tx, rx) = channel::channel();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((name, serial, event)) = event {
                    state.link_event(name, serial, event);
                }
            });
            match inserted {
                Ok(_) => self.machines.tx = Some(tx),
                Err(err) => {
                    tracing::warn!("machines: {err}");
                    return None;
                }
            }
        }
        self.machines.tx.clone()
    }

    /// Start the thread of machine `name`'s link, to its addresses in turn:
    /// the one that answered last first, and only as the machine it was
    /// seen to be (`machine_ids`).
    pub(crate) fn start_link(
        &mut self,
        name: String,
        addresses: crate::config::Addresses,
        retry: bool,
    ) {
        let last = self.projects.machine_last.get(&name).map(String::as_str);
        let order = try_order(&addresses.0, last);
        let target = order.first().cloned().unwrap_or_default();
        // Before this link takes the old one's place: what it was up as.
        let known = self.machine_ids(&name);
        let Some(tx) = self.machines_sender() else {
            self.show_error(format!("gerdr couldn't start connecting to {name}"));
            return;
        };
        self.machines.next_serial += 1;
        let serial = self.machines.next_serial;
        let cancel = Arc::new(AtomicBool::new(false));
        let last_seen = self.machines.links.get(&name).and_then(|l| l.last_seen);
        let link = Link {
            target: target.clone(),
            addresses,
            serial,
            cancel: cancel.clone(),
            state: LinkState::Connecting("connecting\u{2026}".into()),
            lost: false,
            work: None,
            connected_since: None,
            last_seen,
        };
        if let Some(old) = self.machines.links.insert(name.clone(), link) {
            old.stop();
        }
        let spawned = std::thread::Builder::new()
            .name(format!("machine {name}"))
            .spawn(move || {
                let started_as = name.clone();
                run_link(started_as, order, known, retry, cancel, |event| {
                    let _ = tx.send((name.clone(), serial, event));
                });
            });
        if let Err(err) = spawned {
            tracing::warn!("machines: {err}");
            self.show_error(format!("gerdr couldn't start connecting: {err}"));
        }
    }

    /// What a link's thread came to. The thread says the name it started
    /// under, but the link is found by its serial: a machine renamed keeps its
    /// link, under the new name.
    fn link_event(&mut self, _started_as: String, serial: u64, event: LinkEvent) {
        let Some(name) = self.machines.link_named(serial) else {
            // Stopped or replaced since it said so.
            return;
        };
        // A new address tried from the edit form: it isn't the machine's link
        // coming, going or lost until it is kept.
        let trying = (self.machines.editing.as_ref()).is_some_and(|t| t.serial == serial);
        let mut kept = None;
        if trying {
            match &event {
                LinkEvent::Step { text, .. } => self.edit_progress(text.clone()),
                // Its thread passed over another machine at that address
                // already; this checks it again, as the try's own say.
                LinkEvent::Up { info, .. } => match self.edit_answered(&name, info) {
                    Some(tried) => kept = Some(tried),
                    None => return,
                },
                LinkEvent::Down { failure, .. } => {
                    return self.edit_failed(&name, failure.clone());
                }
            }
        }
        let Some(link) = self.machines.links.get_mut(&name) else {
            return;
        };
        let was_up = matches!(link.state, LinkState::Up(_));
        let was_away = matches!(link.state, LinkState::Retrying(_));
        let asked = self
            .machines
            .dialog
            .as_ref()
            .and_then(|d| d.attempt.as_ref())
            == Some(&name);
        match event {
            LinkEvent::Step { target, text } => {
                // Trying again, it stays away until it is back: a chip going
                // back and forth says nothing, and the address its failure
                // is for stays with it.
                if !was_away {
                    link.target = target;
                    link.state = LinkState::Connecting(text.clone());
                }
                if asked && let Some(dialog) = &mut self.machines.dialog {
                    dialog.progress = Some(text);
                }
            }
            LinkEvent::Up { target, info } => {
                tracing::info!(
                    "machine {name} connected at {target}: herdr {}",
                    info.site.version
                );
                link.target = target.clone();
                link.state = LinkState::Up(info.clone());
                link.connected_since = Some(Instant::now());
                link.last_seen = link.connected_since;
                let link_was_lost = std::mem::take(&mut link.lost);
                self.answered_at(&name, &target, &info.site.id);
                self.start_work(&name, target.clone(), info);
                if let Some(tried) = kept {
                    self.edit_kept(tried);
                } else if asked {
                    self.machine_connected(name, target);
                } else if link_was_lost {
                    self.show_success(format!("{name} is back"));
                }
            }
            LinkEvent::Down {
                target,
                mut failure,
                retrying,
            } => {
                tracing::warn!("machine {name} at {target}: {failure:?}");
                // Its thread says the name it started under: renamed since,
                // the machine is called as it is now.
                if let Failure::OtherMachine(called) = &mut failure {
                    called.clone_from(&name);
                }
                link.target = target.clone();
                link.lost |= was_up;
                link.work = None;
                link.connected_since = None;
                link.state = if retrying {
                    LinkState::Retrying(failure.clone())
                } else {
                    LinkState::Failed(failure.clone())
                };
                if asked {
                    // Never saved: the dialog holds what it tried.
                    self.machines.links.remove(&name);
                    if let Some(dialog) = &mut self.machines.dialog {
                        dialog.attempt = None;
                        dialog.progress = None;
                        dialog.failed = Some((target, failure));
                    }
                } else if was_up {
                    let tail = if retrying { ", trying again" } else { "" };
                    self.show_warning(format!("Lost {name}{tail}"));
                }
            }
        }
        self.starts_lost();
        self.report_machines();
    }

    /// Machine `name` answered at address `target`: tried first next time
    /// (`try_order`), in `state.json`.
    fn answered_at(&mut self, name: &str, target: &str, id: &str) {
        let last = &mut self.projects.machine_last;
        let mut changed = last.get(name).is_none_or(|t| t != target);
        last.insert(name.to_owned(), target.to_owned());
        let ids = self.projects.machine_ids.entry(name.to_owned()).or_default();
        if !id.is_empty() && !ids.iter().any(|i| i == id) {
            ids.push(id.to_owned());
            changed = true;
        }
        if changed {
            self.projects.save();
        }
    }

    /// Machine `old` is called `new` now: the address it answered at last
    /// and the ids it answered as move with it.
    pub(crate) fn rename_last(&mut self, old: &str, new: &str) {
        let ids = moved_last(&mut self.projects.machine_ids, old, new);
        if moved_last(&mut self.projects.machine_last, old, new) || ids {
            self.projects.save();
        }
    }

    /// Machine `drop` was merged into `keep`: the ids it answered as are
    /// `keep`'s too, and its last address goes (`keep`'s own stays first).
    pub(crate) fn merge_remembered(&mut self, keep: &str, drop: &str) {
        let had_last = self.projects.machine_last.remove(drop).is_some();
        let dropped = self.projects.machine_ids.remove(drop).unwrap_or_default();
        let ids = self.projects.machine_ids.entry(keep.to_owned()).or_default();
        for id in &dropped {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        if had_last || !dropped.is_empty() {
            self.projects.save();
        }
    }

    /// What needs the user goes on the "!" badge: a machine given up on.
    /// Away is what its dot says, and it keeps trying.
    pub(crate) fn report_machines(&self) {
        let mut texts: Vec<String> = (self.machines.links.iter())
            .filter_map(|(name, link)| match &link.state {
                LinkState::Failed(failure) => Some(format!("Machine {name}: {}", failure.why())),
                _ => None,
            })
            .collect();
        texts.sort();
        crate::problems::set("machines", texts);
    }

    /// The machine asked for in the dialog is up: kept, and the dialog done.
    fn machine_connected(&mut self, name: String, target: String) {
        self.machines.dialog = None;
        self.machines.draft = None;
        let addresses = crate::config::Addresses::one(&target);
        self.machines.saved.insert(name.clone(), addresses.clone());
        self.refresh_pointer_focus();
        match crate::config::write_machine(&name, Some(&addresses)) {
            Ok(()) => {
                // Written here: no need to read it back and say so.
                self.settings.settings_modified = crate::config::modified();
                self.show_success(format!("Connected to {name}"));
            }
            Err(why) => {
                tracing::warn!("machines: saving {name}: {why}");
                self.show_error(format!(
                    "Connected to {name}, but gerdr couldn't keep it in the settings: {why}"
                ));
            }
        }
    }

    /// Open the dialog connecting a machine, with what was typed before.
    pub fn open_machine_dialog(&mut self) {
        if self.machines.dialog.is_some() {
            return;
        }
        let mut dialog = MachineDialog::default();
        if let Some((target, name)) = self.machines.draft.take() {
            dialog.target.set(&target);
            dialog.name.set(&name);
        }
        self.overlays.context_menu = None;
        self.stop_key_repeat();
        self.machines.dialog = Some(dialog);
        self.refresh_pointer_focus();
    }

    pub fn machine_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<MachineOutcome> {
        self.machines.dialog.as_mut()?.key(mods, sym, typed)
    }

    /// Do what the dialog asked.
    pub fn apply_machine(&mut self, outcome: MachineOutcome) {
        match outcome {
            MachineOutcome::Cancel => {
                if let Some(dialog) = self.machines.dialog.take() {
                    let target = dialog.target.trim().to_owned();
                    let name = dialog.name.trim().to_owned();
                    if !(target.is_empty() && name.is_empty()) {
                        self.machines.draft = Some((target, name));
                    }
                    if let Some(name) = dialog.attempt
                        && let Some(link) = self.machines.links.remove(&name)
                    {
                        link.stop();
                    }
                }
                self.refresh_pointer_focus();
            }
            MachineOutcome::Stop => {
                let Some(dialog) = self.machines.dialog.as_mut() else {
                    return;
                };
                let attempt = dialog.attempt.take();
                dialog.progress = None;
                if let Some(link) = attempt.and_then(|name| self.machines.links.remove(&name)) {
                    link.stop();
                }
            }
            MachineOutcome::Connect => self.connect_from_dialog(),
            MachineOutcome::Fix => {
                if let Some(fix) = self.machines.dialog.as_ref().and_then(MachineDialog::fix) {
                    self.run_fix(fix);
                }
            }
        }
    }

    /// Press the button fixing what failed: copy its command, or run it in a
    /// new tab.
    pub(crate) fn run_fix(&mut self, fix: Fix) {
        match fix {
            Fix::Copy(command) => {
                self.show_success(format!("Copied: {command}"));
                self.copy_text(command);
            }
            Fix::Terminal(command) => self.run_in_new_tab(command),
        }
    }

    /// Press Connect: start a link to what the form says, or say what is
    /// wrong with it.
    fn connect_from_dialog(&mut self) {
        let saved = &self.machines.saved;
        let Some(dialog) = self.machines.dialog.as_mut() else {
            return;
        };
        if dialog.attempt.is_some() {
            return;
        }
        dialog.failed = None;
        let target = dialog.target.trim().to_owned();
        let typed = dialog.name.trim().to_owned();
        let name = if typed.is_empty() {
            default_name(&target)
        } else {
            typed
        };
        let already = saved.iter().find(|(_, t)| t.contains(&target));
        let problem = if target.is_empty() {
            dialog.focus = MachineFocus::Target;
            Some("Say which machine, as user@host.".to_owned())
        } else if !valid_target(&target) {
            dialog.focus = MachineFocus::Target;
            Some(Failure::BadTarget.why())
        } else if let Some((name, _)) = already {
            Some(format!("It is here already, as {name}."))
        } else if saved.contains_key(&name) {
            dialog.focus = MachineFocus::Name;
            Some(format!("Another machine is called {name}."))
        } else {
            None
        };
        if let Some(problem) = problem {
            dialog.error = Some(problem);
            return;
        }
        dialog.error = None;
        dialog.attempt = Some(name.clone());
        dialog.progress = Some("connecting\u{2026}".into());
        self.start_link(name, crate::config::Addresses::one(&target), false);
    }

    /// Open a tab running `command` in the workspace shown, for what SSH
    /// has to ask in a terminal; the dialog steps aside for it, what was
    /// typed kept.
    fn run_in_new_tab(&mut self, command: String) {
        let w = self.active_workspace;
        let Some(dir) = self.workspaces.get(w).map(|w| w.dir.clone()) else {
            self.show_warning(format!(
                "Open a project first, or run this yourself: {command}"
            ));
            return;
        };
        self.apply_machine(MachineOutcome::Cancel);
        self.new_tab_running(w, &dir, command);
    }

    /// A left click while the dialog is open.
    pub fn machine_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.machines.dialog.as_mut() else {
            return;
        };
        let layout = chrome::machine_layout(&dialog.view(), size);
        let busy = dialog.attempt.is_some();
        let outcome = match layout.hit(pos) {
            Some(MachineHit::Button(i)) => match dialog.buttons().get(i) {
                Some(Button::Cancel) => Some(MachineOutcome::Cancel),
                Some(Button::Stop) => Some(MachineOutcome::Stop),
                Some(Button::Fix) => Some(MachineOutcome::Fix),
                Some(Button::Connect) => Some(MachineOutcome::Connect),
                None => None,
            },
            Some(MachineHit::Target) if !busy => {
                dialog.focus = MachineFocus::Target;
                None
            }
            Some(MachineHit::Name) if !busy => {
                dialog.focus = MachineFocus::Name;
                None
            }
            // Off it, it stays: a click away never throws what was typed away.
            _ => None,
        };
        if let Some(outcome) = outcome {
            self.apply_machine(outcome);
        }
    }

    pub fn machine_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.machines.dialog.as_mut() {
            dialog.hover = chrome::machine_layout(&dialog.view(), size).hit(pos);
        }
    }

    /// What the pointer can press in the dialog.
    pub fn machine_controls(&self) -> Vec<chrome::Rect> {
        self.machines
            .dialog
            .as_ref()
            .map_or_else(Vec::new, |dialog| {
                chrome::machine_layout(&dialog.view(), self.output_size()).buttons
            })
    }

    /// The dialog's focused field, for the pointer and pastes; none while it
    /// connects, as its fields wait then.
    pub(crate) fn machine_field_mut(&mut self) -> Option<&mut TextField> {
        let dialog = self
            .machines
            .dialog
            .as_mut()
            .filter(|d| d.attempt.is_none())?;
        Some(dialog.field_mut())
    }

    pub fn machine_view(&self) -> Option<MachineView> {
        self.machines.dialog.as_ref().map(MachineDialog::view)
    }

    /// Connect a saved machine again, from its task.
    pub fn connect_machine(&mut self, name: &str) {
        if let Some(target) = self.machines.saved.get(name).cloned() {
            self.start_link(name.to_owned(), target, true);
            self.report_machines();
        }
    }

    /// Stop a machine's link, from its task. What runs there keeps running.
    pub fn disconnect_machine(&mut self, name: &str) {
        // A new address tried for it ends with it: its link says nothing more.
        if (self.machines.editing.as_ref()).is_some_and(|t| t.name == name) {
            self.machines.editing = None;
        }
        let Some(link) = self.machines.links.get_mut(name) else {
            return;
        };
        link.stop();
        link.work = None;
        link.connected_since = None;
        link.state = LinkState::Off;
        self.starts_lost();
        self.report_machines();
        self.show_success(format!(
            "Disconnected {name}; its herdr keeps running there"
        ));
    }

    /// The Remove button of a machine's task: asks only if sandboxes this gerdr
    /// answers for are there, as nothing here will know of them after.
    pub fn ask_remove_machine(&mut self, name: &str) {
        let stays = self.sandboxes_on(name);
        if stays.is_empty() {
            return self.remove_machine(name);
        }
        self.ask(
            format!("Remove {name}?"),
            remove_machine_body(&stays),
            "Remove",
            crate::confirm::Then::RemoveMachine(name.to_owned()),
        );
    }

    /// Forget a machine, from its task or the machines panel, with an Undo
    /// toast. What runs there keeps running.
    pub fn remove_machine(&mut self, name: &str) {
        let target = self.machines.saved.get(name).cloned();
        if let Some(link) = self.machines.links.remove(name) {
            link.stop();
        }
        self.forget_viewed_of(name);
        self.machines.saved.remove(name);
        self.starts_lost();
        self.report_machines();
        match crate::config::write_machine(name, None) {
            Ok(()) => {
                self.settings.settings_modified = crate::config::modified();
                let undo = target.map(|addresses| {
                    let name = name.to_owned();
                    (
                        "Undo".to_owned(),
                        MenuAction::RestoreMachine { name, addresses },
                    )
                });
                self.toast_with(
                    format!("Removed {name}; its herdr keeps running there"),
                    crate::toast::Level::Success,
                    undo,
                );
            }
            Err(why) => {
                tracing::warn!("machines: removing {name}: {why}");
                self.show_error(format!(
                    "gerdr couldn't take {name} out of the settings: {why}"
                ));
            }
        }
    }

    /// Undo removing machine `name`: back in the settings at `addresses`, and
    /// connecting.
    pub(crate) fn restore_machine(&mut self, name: &str, addresses: crate::config::Addresses) {
        if self.machines.saved.contains_key(name) {
            return self.show_warning(format!("{name} is there already"));
        }
        if let Err(why) = crate::config::write_machine(name, Some(&addresses)) {
            tracing::warn!("machines: restoring {name}: {why}");
            return self.show_error(format!(
                "gerdr couldn't put {name} back in the settings: {why}"
            ));
        }
        self.settings.settings_modified = crate::config::modified();
        (self.machines.saved).insert(name.to_owned(), addresses.clone());
        self.start_link(name.to_owned(), addresses, true);
        self.report_machines();
    }

    /// Machine `id` said how it is: seen now.
    pub(crate) fn machine_seen(&mut self, id: &str) {
        let link = (self.machines.links.values_mut())
            .find(|l| l.work.as_ref().is_some_and(|w| w.id == id));
        if let Some(link) = link {
            link.last_seen = Some(Instant::now());
        }
    }

    /// How many machines are connected, for the launcher.
    pub fn machines_up(&self) -> usize {
        (self.machines.links.values())
            .filter(|l| matches!(l.state, LinkState::Up(_)))
            .count()
    }

    /// The machines saved, as tasks: connected ones and those in trouble
    /// show in the footer, those disconnected only in the panel.
    pub fn machine_tasks(&self) -> Vec<Task> {
        (self.machines.saved.iter())
            .map(|(name, saved)| match self.machines.links.get(name) {
                // What it tries: a new address tried from the edit form is
                // the link's before it is saved.
                Some(link) => {
                    machine_task(name, &link.addresses, Some((link.target(), &link.state)))
                }
                None => machine_task(name, saved, None),
            })
            .collect()
    }
}

/// What asking to remove a machine says of the sandboxes of `titles` on it.
fn remove_machine_body(titles: &[String]) -> String {
    format!(
        "These sandboxes stay on it, with their data:\n{}\nAdd the machine again to reach them.",
        titles.join("\n")
    )
}

/// `last`, the address each machine answered at last by its name, with
/// machine `old`'s under `new`: whether it changed.
fn moved_last<T>(last: &mut BTreeMap<String, T>, old: &str, new: &str) -> bool {
    if old == new {
        return false;
    }
    let Some(target) = last.remove(old) else {
        return false;
    };
    last.insert(new.to_owned(), target);
    true
}

/// The address a machine's task names, and its others, which it has
/// `addresses`, its link at an address in a state: where it is up, or
/// what it tries, the one its failure is for; else its first.
fn task_addresses<'a>(
    addresses: &'a crate::config::Addresses,
    link: Option<(&'a str, &LinkState)>,
) -> (&'a str, Vec<&'a str>) {
    let shown = match link {
        Some((at, state)) if !matches!(state, LinkState::Off) && !at.is_empty() => at,
        _ => addresses.first(),
    };
    let others = (addresses.0.iter())
        .map(String::as_str)
        .filter(|a| *a != shown)
        .collect();
    (shown, others)
}

/// Machine `name`'s task: it has `addresses`, and its link, if it has one,
/// is at an address in a state.
fn machine_task(
    name: &str,
    addresses: &crate::config::Addresses,
    link: Option<(&str, &LinkState)>,
) -> Task {
    let (target, others) = task_addresses(addresses, link);
    let also = match others.is_empty() {
        true => String::new(),
        false => format!(" · also: {}", others.join(", ")),
    };
    let state = link.map(|(_, state)| state);
    let open = ("Open", MenuAction::OpenMachines(name.to_owned()));
    let connect = ("Connect", MenuAction::ConnectMachine(name.to_owned()));
    let disconnect = ("Disconnect", MenuAction::DisconnectMachine(name.to_owned()));
    let remove = ("Remove", MenuAction::RemoveMachine(name.to_owned()));
    let task = |chip: String, detail: String, dot, actions| Task {
        title: format!("Machine {name}"),
        chip,
        detail: detail + &also,
        dot,
        actions,
    };
    match state {
        Some(LinkState::Up(info)) => task(
            name.to_owned(),
            format!("{target} · herdr {}", info.site.version),
            Dot::Done,
            vec![open, disconnect, remove],
        ),
        Some(LinkState::Connecting(step)) => task(
            format!("{name} connecting"),
            format!("{target} · {step}"),
            Dot::Working,
            vec![open, disconnect, remove],
        ),
        Some(LinkState::Retrying(failure)) => task(
            format!("{name} away"),
            format!("{target} · {} Trying again", failure.why()),
            Dot::Blocked,
            vec![open, disconnect, remove],
        ),
        Some(LinkState::Failed(failure)) => task(
            format!("{name}: can't connect"),
            format!("{target} · {}", failure.why()),
            Dot::Failed,
            vec![open, connect, remove],
        ),
        Some(LinkState::Off) | None => task(
            format!("{name} disconnected"),
            format!("{target} · not connected"),
            Dot::Idle,
            vec![open, connect, remove],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_moves_the_machine_and_keeps_its_link() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut machines = Machines::default();
        machines.saved.insert("box".into(), "me@box".into());
        machines.saved.insert("zen".into(), "me@zen".into());
        let link = Link {
            target: "me@box".into(),
            addresses: "me@box".into(),
            serial: 7,
            cancel: cancel.clone(),
            state: LinkState::Connecting("x".into()),
            lost: false,
            work: None,
            connected_since: None,
            last_seen: None,
        };
        machines.links.insert("box".into(), link);
        machines.names.insert("id1".into(), "box".into());
        machines.names.insert("id0".into(), "box".into());
        machines.names.insert("id2".into(), "zen".into());
        assert_eq!(machines.link_named(7).as_deref(), Some("box"));
        let moved = machines.rename("box", "crate");
        assert_eq!(moved, ["id0", "id1"]);
        assert_eq!(machines.saved.keys().collect::<Vec<_>>(), ["crate", "zen"]);
        assert_eq!(machines.saved["crate"], "me@box".into());
        // The same link, still running, and what its thread says finds it.
        assert!(!machines.links.contains_key("box"));
        assert_eq!(machines.links["crate"].serial, 7);
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(machines.link_named(7).as_deref(), Some("crate"));
        assert_eq!(machines.link_named(8), None);
        assert_eq!(machines.names["id0"], "crate");
        assert_eq!(machines.names["id1"], "crate");
        assert_eq!(machines.names["id2"], "zen");
        // Turned off, it is stopped and says so.
        machines.link_off("crate");
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(machines.links["crate"].state, LinkState::Off);
    }

    #[test]
    fn what_ssh_says_is_told_apart() {
        assert_eq!(
            classify("Host key verification failed.\n"),
            Failure::HostKey
        );
        assert_eq!(
            classify("ana@h: Permission denied (publickey).\n"),
            Failure::Auth
        );
        assert_eq!(
            classify("ssh: connect to host h port 22: Connection refused\n"),
            Failure::Unreachable("ssh: connect to host h port 22: Connection refused".into())
        );
        assert_eq!(
            classify("something new\n"),
            Failure::Other("something new".into())
        );
        assert!(matches!(classify(""), Failure::Other(_)));
    }

    #[test]
    fn a_target_cant_be_taken_for_an_option() {
        assert!(valid_target("ana@10.0.0.5"));
        assert!(valid_target("build-box"));
        assert!(!valid_target("-oProxyCommand=touch /tmp/x"));
        assert!(!valid_target("-J"));
        assert!(!valid_target("ana@h extra"));
        assert!(!valid_target(""));
    }

    #[test]
    fn a_rename_moves_what_was_remembered_of_a_machine() {
        let mut ids = BTreeMap::from([("old".to_owned(), vec!["id1".to_owned()])]);
        assert!(moved_last(&mut ids, "old", "new"));
        assert_eq!(ids.get("new"), Some(&vec!["id1".to_owned()]));
        assert!(!moved_last(&mut ids, "gone", "other"), "nothing to move");
    }

    #[test]
    fn each_connection_has_sockets_of_its_own() {
        let dir = Path::new("/run/user/1000/gerdr/m");
        let (one, two) = (link_stem(dir, "me@box"), link_stem(dir, "me@box"));
        assert_ne!(one, two, "a link ending would take the next one's sockets");
        let name = one.file_name().unwrap().to_string_lossy().into_owned();
        let pid = std::process::id().to_string();
        assert_eq!(
            name.split('-').next(),
            Some(pid.as_str()),
            "clear_leftovers reads the pid"
        );
    }

    #[test]
    fn a_machine_is_called_by_its_host() {
        assert_eq!(default_name("ana@10.0.0.5"), "10.0.0.5");
        assert_eq!(default_name("build-box"), "build-box");
        assert_eq!(default_name("ssh://ana@host/x"), "host");
    }

    #[test]
    fn a_look_around_is_read_past_a_login_shells_noise() {
        let site = parse_site(
            "welcome to the box\nHOME=/home/ana\nDATA=/home/ana/.local/share\n\
             CONFIG=/home/ana/.config\nHERDR=/home/ana/.local/bin/herdr\n\
             VERSION=herdr 0.9.5\nMACHINE=abc\nNOW=1700000000\n",
        );
        assert_eq!(site.home, "/home/ana");
        assert_eq!(site.herdr.as_deref(), Some("/home/ana/.local/bin/herdr"));
        assert_eq!(site.version, "0.9.5");
        assert_eq!(site.id, "abc");
        assert_eq!(site.now, 1_700_000_000);
        assert_eq!(
            site.socket(),
            "/home/ana/.config/herdr/sessions/gerdr-worker/herdr.sock"
        );
    }

    #[test]
    fn the_look_script_finds_what_a_login_shell_finds() {
        let home = std::env::temp_dir().join(format!("gerdr-look-{}", std::process::id()));
        let bin = home.join("mybin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("herdr");
        std::fs::write(&fake, "#!/bin/sh\necho herdr 0.9.5\n").unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // A login "shell" whose env says where herdr is, past some noise.
        let shell = home.join("shell");
        let script = format!(
            "#!/bin/sh\necho hello from a profile\nexec env PATH={}:/usr/bin:/bin\n",
            bin.display()
        );
        std::fs::write(&shell, script).unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let out = Command::new("/bin/sh")
            .args(["-c", &look_script("newid", "")])
            .env_clear()
            .env("HOME", &home)
            .env("SHELL", &shell)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let site = parse_site(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(site.herdr, Some(fake.display().to_string()));
        assert_eq!(site.version, "0.9.5");
        assert_eq!(site.id, "newid");
        assert_eq!(site.data, format!("{}/.local/share", home.display()));
        assert!(site.now > 0);
        // Asked again, it keeps its id.
        let out = Command::new("/bin/sh")
            .args(["-c", &look_script("other", "")])
            .env_clear()
            .env("HOME", &home)
            .env("SHELL", &shell)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert_eq!(
            parse_site(&String::from_utf8_lossy(&out.stdout)).id,
            "newid"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_machine_that_cant_take_work_says_why() {
        let good = Site {
            herdr: Some("/x/herdr".into()),
            version: "0.9.5".into(),
            ..Site::default()
        };
        assert_eq!(good.problem(0), None);
        assert_eq!(good.problem(2), None);
        assert_eq!(good.problem(-3), Some(Failure::Clock(-3)));
        let none = Site::default();
        assert_eq!(none.problem(0), Some(Failure::NoHerdr));
        let old = Site {
            version: "0.8.2".into(),
            ..good.clone()
        };
        assert_eq!(old.problem(0), Some(Failure::OldHerdr("0.8.2".into())));
        assert!(new_enough("1.0.0"));
        assert!(!new_enough("nonsense"));
    }

    #[test]
    fn only_what_may_pass_by_itself_is_tried_again() {
        assert!(Failure::LinkLost.retries());
        assert!(Failure::Unreachable("x".into()).retries());
        assert!(!Failure::Auth.retries());
        assert!(!Failure::HostKey.retries());
        assert!(!Failure::Forwarding.retries());
    }

    #[test]
    fn a_host_key_is_never_accepted_for_the_user() {
        let target = "ana@h";
        assert_eq!(
            Failure::HostKey.fix(target),
            Some(Fix::Terminal("ssh ana@h".into()))
        );
        assert_eq!(
            Failure::Auth.fix(target),
            Some(Fix::Copy("ssh-copy-id ana@h".into()))
        );
    }

    #[test]
    fn what_a_task_offers_goes_with_its_state() {
        let labels = |task: &Task| task.actions.iter().map(|(l, _)| *l).collect::<Vec<_>>();
        let one = "a@b".into();
        let off = machine_task("b", &one, None);
        assert_eq!(off.dot, Dot::Idle);
        assert!(!off.runs(), "a machine disconnected isn't in the footer");
        assert_eq!(labels(&off), ["Open", "Connect", "Remove"]);
        assert_eq!(off.detail, "a@b · not connected");
        assert!(remove_machine_body(&["A".to_owned(), "B".to_owned()]).contains("\nA\nB\n"));
        let failed = machine_task("b", &one, Some(("a@b", &LinkState::Failed(Failure::Auth))));
        assert_eq!(failed.dot, Dot::Failed);
        assert_eq!(labels(&failed), ["Open", "Connect", "Remove"]);
        let lost = LinkState::Retrying(Failure::LinkLost);
        let away = machine_task("b", &one, Some(("a@b", &lost)));
        assert_eq!(labels(&away), ["Open", "Disconnect", "Remove"]);
    }

    #[test]
    fn a_task_names_the_address_in_use_and_the_others() {
        let two = crate::config::Addresses(vec!["a@office".into(), "a@home".into()]);
        let up = LinkState::Up(LinkInfo {
            site: Site {
                version: "0.9.1".into(),
                ..Site::default()
            },
            socket: PathBuf::new(),
            control: PathBuf::new(),
        });
        let task = machine_task("b", &two, Some(("a@home", &up)));
        assert_eq!(task.detail, "a@home · herdr 0.9.1 · also: a@office");
        // Not connected: its first.
        let task = machine_task("b", &two, Some(("a@home", &LinkState::Off)));
        assert_eq!(task.detail, "a@office · not connected · also: a@home");
        let task = machine_task("b", &two, None);
        assert_eq!(task.detail, "a@office · not connected · also: a@home");
        // Failed: the address its failure is for.
        let failed = LinkState::Failed(Failure::Auth);
        let (shown, others) = task_addresses(&two, Some(("a@home", &failed)));
        assert_eq!((shown, others), ("a@home", vec!["a@office"]));
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_address_that_answered_last_is_tried_first() {
        let list = strings(&["a@office", "a@home", "a@vpn"]);
        assert_eq!(try_order(&list, None), list);
        assert_eq!(
            try_order(&list, Some("a@vpn")),
            strings(&["a@vpn", "a@office", "a@home"])
        );
        assert_eq!(try_order(&list, Some("a@office")), list);
        // One no longer the machine's is no reason to change the order.
        assert_eq!(try_order(&list, Some("a@gone")), list);
        assert!(try_order(&[], Some("a@x")).is_empty());
        // Renamed, it moves with the machine.
        let mut last = BTreeMap::from([("box".to_owned(), "a@home".to_owned())]);
        assert!(moved_last(&mut last, "box", "crate"));
        assert_eq!(
            last,
            BTreeMap::from([("crate".to_owned(), "a@home".to_owned())])
        );
        assert!(!moved_last(&mut last, "box", "zen"));
        assert!(!moved_last(&mut last, "crate", "crate"));
        assert_eq!(last.len(), 1);
    }

    /// The address up and its id, or what each address came to.
    type Tried = Result<(String, String), Vec<(String, Failure)>>;

    /// What `try_addresses` comes to over addresses each answering as
    /// `answers` says (`Ok(id)`, or a failure), and the addresses tried.
    fn tried(known: &[&str], answers: &[(&str, Result<&str, Failure>)]) -> (Tried, Vec<String>) {
        let order: Vec<String> = answers.iter().map(|(a, _)| (*a).to_owned()).collect();
        let mut seen = Vec::new();
        let result = try_addresses(
            "box",
            &order,
            &strings(known),
            |target| {
                seen.push(target.to_owned());
                let (_, answer) = answers.iter().find(|(a, _)| *a == target).unwrap();
                answer.clone().map(str::to_owned)
            },
            |id: &String| id.as_str(),
        );
        (result, seen)
    }

    #[test]
    fn addresses_are_tried_in_turn_until_one_answers() {
        let (result, seen) = tried(
            &["id1"],
            &[
                ("a@office", Err(Failure::TimedOut)),
                ("a@home", Ok("id1")),
                ("a@vpn", Ok("id1")),
            ],
        );
        assert_eq!(result.unwrap(), ("a@home".to_owned(), "id1".to_owned()));
        assert_eq!(seen, ["a@office", "a@home"], "none after the one up");
        // Stopped, it tries no more.
        let (result, seen) = tried(
            &[],
            &[("a@office", Err(Failure::Stopped)), ("a@home", Ok("id1"))],
        );
        assert_eq!(
            result.unwrap_err(),
            [("a@office".to_owned(), Failure::Stopped)]
        );
        assert_eq!(seen, ["a@office"]);
    }

    #[test]
    fn an_address_answering_as_another_machine_is_passed_over() {
        let (result, seen) = tried(
            &["id1", "id0"],
            &[("a@office", Ok("other")), ("a@home", Ok("id0"))],
        );
        assert_eq!(result.unwrap(), ("a@home".to_owned(), "id0".to_owned()));
        assert_eq!(seen, ["a@office", "a@home"]);
        // A machine never seen takes the first that answers.
        let (result, _) = tried(&[], &[("a@office", Ok("other")), ("a@home", Ok("id0"))]);
        assert_eq!(result.unwrap(), ("a@office".to_owned(), "other".to_owned()));
        // None of them it: said so, over any address that was away.
        let (result, _) = tried(
            &["id1"],
            &[
                ("a@office", Err(Failure::TimedOut)),
                ("a@home", Ok("other")),
                ("a@vpn", Err(Failure::Auth)),
            ],
        );
        let tries = result.unwrap_err();
        assert_eq!(tries.len(), 3);
        assert_eq!(
            most_telling(tries),
            Some(("a@home".to_owned(), Failure::OtherMachine("box".into())))
        );
    }

    #[test]
    fn what_no_address_came_to_is_the_most_telling() {
        let at = |a: &str, f: Failure| (a.to_owned(), f);
        let away = Failure::Unreachable("Connection refused".into());
        // Away at one, refused its keys at the other: the keys.
        let tries = vec![
            at("a@office", Failure::TimedOut),
            at("a@home", Failure::Auth),
        ];
        assert_eq!(most_telling(tries), Some(at("a@home", Failure::Auth)));
        // What SSH said over a plain timeout.
        let tries = vec![
            at("a@office", Failure::TimedOut),
            at("a@home", away.clone()),
        ];
        assert_eq!(most_telling(tries), Some(at("a@home", away.clone())));
        // Alike, the first tried.
        let tries = vec![
            at("a@office", Failure::NoHerdr),
            at("a@home", Failure::HostKey),
        ];
        assert_eq!(most_telling(tries), Some(at("a@office", Failure::NoHerdr)));
        assert_eq!(most_telling(Vec::new()), None);
        // Several are named as tried, and given less time each.
        assert_eq!(
            first_step("a@home", true),
            ("trying a@home\u{2026}".into(), CONNECT_EACH_SECS)
        );
        assert_eq!(first_step("a@home", false).1, CONNECT_SECS);
    }

    /// A link up in a test: `hold` ends it as told.
    struct Fake(LinkInfo, Failure);

    impl Held for Fake {
        fn info(&self) -> &LinkInfo {
            &self.0
        }

        fn hold(&mut self, _: &AtomicBool) -> Failure {
            self.1.clone()
        }
    }

    /// What `run_link_with` tells, its addresses answering as `answers`
    /// says, each link up ending in `ends`.
    fn link_events(
        known: &[&str],
        answers: &[(&str, Result<&str, Failure>)],
        ends: Failure,
    ) -> Vec<LinkEvent> {
        let events = std::cell::RefCell::new(Vec::new());
        let order = answers.iter().map(|(a, _)| (*a).to_owned()).collect();
        run_link_with(
            "box".into(),
            order,
            strings(known),
            false,
            &AtomicBool::new(false),
            |event| events.borrow_mut().push(event),
            |target: &str, several, _: &AtomicBool, step: &mut dyn FnMut(String)| {
                step(first_step(target, several).0);
                let (_, answer) = answers.iter().find(|(a, _)| *a == target).unwrap();
                let site = |id: &str| Site {
                    id: id.to_owned(),
                    ..Site::default()
                };
                let info = |id| LinkInfo {
                    site: site(id),
                    socket: PathBuf::new(),
                    control: PathBuf::new(),
                };
                answer.clone().map(|id| Fake(info(id), ends.clone()))
            },
        );
        events.into_inner()
    }

    #[test]
    fn a_link_up_says_which_address_answered() {
        let events = link_events(
            &["id1"],
            &[("a@office", Err(Failure::TimedOut)), ("a@home", Ok("id1"))],
            Failure::Auth,
        );
        let said: Vec<String> = (events.iter())
            .map(|e| match e {
                LinkEvent::Step { target, text } => format!("step {target}: {text}"),
                LinkEvent::Up { target, info } => format!("up {target} as {}", info.site.id),
                LinkEvent::Down {
                    target,
                    failure,
                    retrying,
                } => format!("down {target}: {failure:?} {retrying}"),
            })
            .collect();
        assert_eq!(
            said,
            [
                "step a@office: trying a@office\u{2026}",
                "step a@home: trying a@home\u{2026}",
                "up a@home as id1",
                "down a@home: Auth false",
            ]
        );
        // None up: the most telling failure, with its address.
        let events = link_events(
            &["id1"],
            &[("a@office", Err(Failure::TimedOut)), ("a@home", Ok("id9"))],
            Failure::Stopped,
        );
        let Some(LinkEvent::Down {
            target, failure, ..
        }) = events.last()
        else {
            panic!("{events:?}");
        };
        assert_eq!(target, "a@home");
        assert_eq!(failure, &Failure::OtherMachine("box".into()));
        // Stopped while up: nothing more said.
        let events = link_events(&[], &[("a@home", Ok("id1"))], Failure::Stopped);
        assert!(
            matches!(events.last(), Some(LinkEvent::Up { .. })),
            "{events:?}"
        );
        assert_eq!(events.len(), 2);
    }

    fn press(dialog: &mut MachineDialog, sym: Keysym) -> Option<MachineOutcome> {
        dialog.key(&ModifiersState::default(), sym, sym)
    }

    #[test]
    fn esc_stops_what_runs_before_it_closes() {
        let mut dialog = MachineDialog {
            attempt: Some("b".into()),
            ..MachineDialog::default()
        };
        assert_eq!(
            press(&mut dialog, Keysym::Escape),
            Some(MachineOutcome::Stop)
        );
        dialog.attempt = None;
        assert_eq!(
            press(&mut dialog, Keysym::Escape),
            Some(MachineOutcome::Cancel)
        );
    }

    #[test]
    fn its_buttons_follow_what_happened() {
        let mut dialog = MachineDialog::default();
        assert_eq!(dialog.buttons(), [Button::Cancel, Button::Connect]);
        assert_eq!(dialog.view().buttons, ["Cancel", "Connect"]);
        dialog.failed = Some(("a@b".into(), Failure::Auth));
        assert_eq!(
            dialog.view().buttons,
            ["Cancel", "Copy command", "Try again"]
        );
        dialog.failed = Some(("a@b".into(), Failure::NoHerdr));
        assert_eq!(dialog.view().buttons, ["Cancel", "Try again"]);
        dialog.attempt = Some("b".into());
        assert_eq!(dialog.view().buttons, ["Stop"]);
    }
}
