//! Starting and stopping a feature's sandbox (see `sandbox`): its containers
//! under Docker or Podman (`Engine`), the network namespace and the proxy into
//! it. Runs processes: call all of it off the UI thread (checklist D3), except
//! what reads only `/proc`.
//!
//! Network: `holder` keeps a user and network namespace alive, and pasta leads
//! main's port numbers there to the copies' ports on the host. pasta is handed
//! the namespaces as paths with `--no-netns-quit` and stays in the foreground:
//! given a pid or a pid file, Fedora's confinement of it refuses.
//!
//! A volume copies with its container paused, so the copy is what a crash at
//! that moment would leave, which a database recovers from. `Paused` lets go
//! however the copy ends, and notes what it paused in the sandbox's folder:
//! a gerdr that dies meanwhile leaves main's containers paused, and
//! `unpause_left` lets them go at the next start, stop or launch. On a
//! copy-on-write file system it takes under a second.
//!
//! All of it can happen on another machine: `Host` says where. Its containers,
//! files and processes are that machine's, as are the paths in `Job`.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Repo, now,
    remote::Reach,
    sandbox::{self, Engine, Forward, Generated, Input, Stack},
    setup::{self, Exit},
};
use crate::desktop::shell_quote;

/// How long one container command may run: pulling an image can take a while.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long the copies may take to come up, health checks included.
const READY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a copy of one volume may run, with main's container paused.
const COPY_TIMEOUT: Duration = setup::TIMEOUT;
/// How long a stop waits for what it stops.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
/// How much of a failed command's error goes in the message about it.
const ERROR_CHARS: usize = 200;
/// How long what runs in a sandbox gets to end.
const END_WAIT: Duration = Duration::from_secs(3);
/// Folders of the system a bind mount shares on purpose: not data to copy.
const SYSTEM_DIRS: &[&str] = &[
    "/var/run", "/run", "/sys", "/proc", "/dev", "/tmp", "/etc", "/usr",
];
/// How long a small command on another machine may run.
const THERE_TIMEOUT: Duration = Duration::from_secs(60);
/// Where plain `ssh host command` doesn't look, and these tools can be.
const PATH_THERE: &str =
    "PATH=\"$PATH:/usr/local/bin:/usr/sbin:/sbin:$HOME/.local/bin\"; export PATH\n";

/// The variable that tells a pane's commands they are in a sandbox; what
/// keeps them from running again outside it (see `PaneRecord::restart_command`).
pub const SANDBOX_VAR: &str = "GERDR_SANDBOX";

/// The sandbox of a feature: kept in its record, so a restarted gerdr finds it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRun {
    /// The machine it runs on, by id, when that isn't the one writing this
    /// record. Shown from another machine, it is never empty unless that is
    /// this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Unix seconds.
    pub started: u64,
    /// When starting ended, either way; `None` while it starts, or when
    /// gerdr quit before that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u64>,
    /// Why it didn't start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Its compose projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stacks: Vec<StackRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<Forward>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<Proc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pasta: Option<Proc>,
    /// Main's volumes this sandbox has a copy of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copied: Vec<String>,
    /// The services whose volumes started empty, as brokers do.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fresh: Vec<String>,
    /// Running containers with nothing telling how to start them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_copied: Vec<String>,
    /// The herdr panes running its commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<String>,
    /// It was stopped, its data kept for the next start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// What its copied files can reach outside it: variable names, never
    /// their values (see `outward_keys`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outward: Vec<String>,
    /// What its copied files point at that it can't reach (see
    /// `sandbox::out_of_reach`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out_of_reach: Vec<String>,
    /// Compose projects running under the project on the machine that sent it
    /// there, which don't run on the machine it runs on: nothing of those got
    /// copied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_there: Vec<String>,
    /// The pages of its app, the first the one Open app opens, as found
    /// when it started (see `sandbox_pages`): empty when nothing told.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<super::sandbox_pages::Page>,
    /// It runs on another machine, and this is how it stands there. Its numbers
    /// mean nothing here, so nothing is looked up or signalled by them.
    #[serde(skip)]
    pub elsewhere: Option<Elsewhere>,
}

/// How a sandbox on another machine stands, as last asked there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// That machine has said: until then nothing is known of it.
    pub asked: bool,
    pub alive: bool,
    /// What listens inside it, by port, leaving out the proxy's own ports.
    pub ports: Vec<u16>,
}

impl SandboxRun {
    /// Whether it is on another machine, so its numbers mean nothing here.
    pub fn on_another_machine(&self) -> bool {
        self.machine.is_some() || self.elsewhere.is_some()
    }

    /// Whether its namespace is there, so its commands can run.
    pub fn alive(&self) -> bool {
        if self.on_another_machine() {
            return !self.stopped && self.elsewhere.as_ref().is_some_and(|there| there.alive);
        }
        !self.stopped
            && self.failed.is_none()
            && self.holder.as_ref().is_some_and(Proc::alive)
            && self.pasta.as_ref().is_some_and(Proc::alive)
    }

    /// Whether it may be running on the machine that started it, as far as is
    /// known on another: until that machine says, it may.
    pub fn may_run_there(&self) -> bool {
        !self.stopped
            && self.failed.is_none()
            && (self.elsewhere.as_ref()).is_none_or(|there| there.alive || !there.asked)
    }

    /// Stop its network, the only part that takes no docker: its pieces
    /// without a whole sandbox, after a failed start. What still runs inside
    /// goes first: a browser's window would stay, on a dead network.
    ///
    /// `host` is where it runs: an error says that machine couldn't be told.
    pub fn kill_network(&self, host: &Host) -> Result<(), String> {
        if let Host::There(reach) = host {
            let script = network_end_script(self);
            return Host::script(reach, &script, THERE_TIMEOUT, "Ending its network", &NEVER);
        }
        if self.on_another_machine() {
            return Ok(());
        }
        if let Some(holder) = self.holder.as_ref().filter(|h| h.alive()) {
            end_inside(holder);
        }
        if let Some(pasta) = &self.pasta {
            pasta.kill();
        }
        if let Some(holder) = &self.holder {
            holder.kill();
        }
        Ok(())
    }

    /// What listens inside it, by port, leaving out the proxy's own ports.
    pub fn listening(&self) -> Vec<u16> {
        if self.on_another_machine() {
            return (self.elsewhere.iter()).flat_map(|there| there.ports.clone()).collect();
        }
        let Some(holder) = self.holder.as_ref() else {
            return Vec::new();
        };
        let mut ports = listening_of(holder);
        ports.retain(|port| !self.forwards.iter().any(|f| f.main_port == *port));
        ports
    }
}

/// When `listening_of` read a holder's ports, and what they were.
type SeenPorts = (Instant, Vec<u16>);

/// How long `listening_of` goes by what it last read of a holder.
const PORTS_FRESH: Duration = Duration::from_secs(1);

thread_local! {
    /// What `listening_of` last read, by holder, and when. The feature tab
    /// asks every frame and for every pointer move, and a namespace with a
    /// few hundred sockets takes over a millisecond to read.
    static PORTS: std::cell::RefCell<HashMap<(i32, u64), SeenPorts>> =
        std::cell::RefCell::new(HashMap::new());
}

/// The ports listening in the network namespace of `holder`, sorted, as read
/// within `PORTS_FRESH`; none once it is gone.
fn listening_of(holder: &Proc) -> Vec<u16> {
    let key = (holder.pid, holder.start);
    let now = Instant::now();
    let kept = PORTS.with_borrow(|seen| {
        (seen.get(&key))
            .filter(|(at, _)| now.duration_since(*at) < PORTS_FRESH)
            .map(|(_, ports)| ports.clone())
    });
    if let Some(ports) = kept {
        return ports;
    }
    let mut ports = Vec::new();
    if holder.alive() {
        for table in ["tcp", "tcp6"] {
            let path = format!("/proc/{}/net/{table}", holder.pid);
            // Unreadable counts as nothing listening: the holder just ended.
            let text = fs::read_to_string(path).unwrap_or_default();
            ports.extend(listening_in(&text));
        }
    }
    ports.sort_unstable();
    ports.dedup();
    PORTS.with_borrow_mut(|seen| {
        seen.retain(|_, (at, _)| now.duration_since(*at) < PORTS_FRESH);
        seen.insert(key, (now, ports.clone()));
    });
    ports
}

/// Whether what `run` names on its machine is worth asking about there.
pub fn to_ask_there(run: &SandboxRun) -> bool {
    !run.stopped && run.failed.is_none() && run.holder.is_some() && run.pasta.is_some()
}

/// The script asking the machine of `runs` (by a name for each) how they stand:
/// what `read_there` reads. A process that's gone just says nothing.
pub fn there_script<'a>(runs: impl IntoIterator<Item = (&'a str, &'a SandboxRun)>) -> String {
    let mut script = String::new();
    for (name, run) in runs {
        let (Some(holder), Some(pasta)) = (&run.holder, &run.pasta) else {
            continue;
        };
        let name = shell_quote(name);
        let (holder, pasta) = (holder.pid, pasta.pid);
        script.push_str(&format!(
            "echo '@holder' {name}; cat /proc/{holder}/stat 2>/dev/null\n\
             echo '@pasta' {name}; cat /proc/{pasta}/stat 2>/dev/null\n\
             echo '@ports' {name}; cat /proc/{holder}/net/tcp /proc/{holder}/net/tcp6 2>/dev/null\n"
        ));
    }
    script.push_str("exit 0\n");
    script
}

/// The script ending what `run` has running on its machine: what runs inside
/// its namespace first, as `end_inside` does, then its proxy and its holder. A
/// process with another start time is another's, and left alone. A shell in a
/// terminal ignores what ends the others, and no gerdr may be there to close that
/// terminal: a hangup ends it.
pub fn network_end_script(run: &SandboxRun) -> String {
    let same = "same() {\n\
                s=$(cat /proc/$1/stat 2>/dev/null) || return 1\n\
                s=${s##*) }\n\
                w=$2\n\
                set -- $s\n\
                [ \"$1\" != Z ] && [ \"${20}\" = \"$w\" ]\n\
                }\n";
    let mut script = same.to_owned();
    // Never what this shell is in: that is everything else on the machine, as a
    // holder that isn't one would have it.
    script.push_str("own=$(readlink /proc/$$/ns/net)\n");
    if let Some(holder) = &run.holder {
        let (pid, start) = (holder.pid, holder.start);
        script.push_str(&format!(
            "net=\nif same {pid} {start}; then net=$(readlink /proc/{pid}/ns/net); fi\n\
             if [ -n \"$own\" ] && [ -n \"$net\" ] && [ \"$net\" != \"$own\" ]; then\n\
             inside() {{ ls -l /proc/[0-9]*/ns/net 2>/dev/null | awk -v n=\"$net\" '$NF == n {{ split($(NF-2), a, \"/\"); if (a[3] != {pid}) print a[3] }}'; }}\n\
             for p in $(inside); do kill \"$p\" 2>/dev/null; done\n\
             i=0\n\
             while [ \"$i\" -lt 30 ] && [ -n \"$(inside)\" ]; do sleep 0.1; i=$((i+1)); done\n\
             for p in $(inside); do kill -HUP \"$p\" 2>/dev/null; done\n\
             fi\n"
        ));
    }
    for proc in [&run.pasta, &run.holder].into_iter().flatten() {
        let (pid, start) = (proc.pid, proc.start);
        script.push_str(&format!("if same {pid} {start}; then kill {pid}; fi\n"));
    }
    script.push_str("exit 0\n");
    script
}

/// How `run`, called `name`, stands by what `there_script` said.
pub fn read_there(said: &str, name: &str, run: &SandboxRun) -> Elsewhere {
    let section = |what: &str| -> String {
        let head = format!("@{what} {name}");
        let mut lines = said.lines().skip_while(|l| *l != head);
        lines.next();
        let body: Vec<&str> = lines.take_while(|l| !l.starts_with('@')).collect();
        body.join("\n")
    };
    let same = |proc: &Option<Proc>, what: &str| {
        proc.as_ref()
            .is_some_and(|p| start_in_stat(&section(what)) == Some(p.start))
    };
    let alive = same(&run.holder, "holder") && same(&run.pasta, "pasta");
    let mut ports = Vec::new();
    if alive {
        // Two tables, each with its heading: a heading has no port to read.
        ports = listening_in(&section("ports"));
        ports.sort_unstable();
        ports.dedup();
        ports.retain(|port| !run.forwards.iter().any(|f| f.main_port == *port));
    }
    Elsewhere {
        asked: true,
        alive,
        ports,
    }
}

/// End every process in the namespace `holder` keeps but `holder`, waiting
/// `END_WAIT` at most: a browser writing into its profile can't be removed.
fn end_inside(holder: &Proc) {
    let Ok(net) = fs::read_link(format!("/proc/{}/ns/net", holder.pid)) else {
        return;
    };
    // Never what gerdr is in: that is everything else on the machine, as a holder
    // that isn't one would have it.
    if fs::read_link("/proc/self/ns/net").map_or(true, |own| own == net) {
        tracing::error!("a sandbox's holder is in this machine's own network: nothing ended");
        return;
    }
    let inside = || -> Vec<i32> {
        // A folder that can't be listed leaves nothing to end.
        let entries = fs::read_dir("/proc").into_iter().flatten().flatten();
        entries
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
            .filter(|pid| *pid != holder.pid)
            .filter(|pid| start_of(*pid).is_some())
            .filter(|pid| fs::read_link(format!("/proc/{pid}/ns/net")).ok().as_ref() == Some(&net))
            .collect()
    };
    for pid in inside() {
        // SAFETY: a plain signal; a process gone meanwhile is ESRCH.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let started = Instant::now();
    while started.elapsed() < END_WAIT && !inside().is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The ports listening in the text of a `/proc/<pid>/net/tcp`.
fn listening_in(table: &str) -> Vec<u16> {
    const LISTEN: &str = "0A";
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let local = fields.nth(1)?;
            let state = fields.nth(1)?;
            let port = local.rsplit_once(':')?.1;
            (state == LISTEN).then(|| u16::from_str_radix(port, 16).ok())?
        })
        .collect()
}

/// The port of `ports` most like a web app's: a dev server's usual ones first,
/// else the lowest. An API can listen beside a frontend, and only a person
/// would know for sure: `workflow.sandbox_url` settles it.
pub fn app_port(ports: &[u16]) -> Option<u16> {
    const FRONTENDS: &[u16] = &[5173, 4200, 3000, 3001, 8080, 8000, 5000, 9000];
    FRONTENDS
        .iter()
        .find(|port| ports.contains(port))
        .or_else(|| ports.iter().min())
        .copied()
}

/// The names of the variables in `env`, a `.env` file's text, that let what
/// runs there reach the outside on its own: real mail above all. Never their
/// values: those can be passwords.
pub fn outward_keys(env: &str) -> Vec<String> {
    const WORDS: &[&str] = &[
        "SMTP",
        "EMAIL_HOST",
        "MAIL_HOST",
        "SENDGRID",
        "TWILIO",
        "WEBHOOK",
        "STRIPE",
    ];
    sandbox::env_pairs(env)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| WORDS.iter().any(|word| key.contains(word)))
        .collect()
}

/// What never asks anything to stop.
static NEVER: AtomicBool = AtomicBool::new(false);

/// Where a sandbox's commands run.
#[derive(Clone, Debug, Default)]
pub enum Host {
    #[default]
    Here,
    /// On another machine, over its link.
    There(Reach),
}

impl Host {
    /// Run `script` there, with where its tools are. Nothing comes of what it
    /// wrote: for what `output` isn't.
    fn script(
        reach: &Reach,
        script: &str,
        limit: Duration,
        what: &str,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let script = format!("{PATH_THERE}{script}");
        (reach.script_for(&script, limit, what, stop)).map(drop).map_err(|t| t.text().to_owned())
    }

    fn said(reach: &Reach, script: &str, stop: &AtomicBool) -> Result<String, String> {
        let script = format!("{PATH_THERE}{script}");
        (reach.script_for(&script, THERE_TIMEOUT, "A command there failed", stop))
            .map_err(|t| t.text().to_owned())
    }

    /// `output` where this is.
    fn output(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
        stop: &AtomicBool,
    ) -> Result<String, String> {
        self.output_in(None, program, args, timeout, stop)
    }

    /// `output` where this is, run in folder `dir` when given.
    fn output_in(
        &self,
        dir: Option<&Path>,
        program: &str,
        args: &[&str],
        timeout: Duration,
        stop: &AtomicBool,
    ) -> Result<String, String> {
        let Self::There(reach) = self else {
            return output(dir, program, args, timeout, stop);
        };
        let mut line = PATH_THERE.to_owned();
        if let Some(dir) = dir {
            line.push_str(&format!(
                "cd {} || exit 1\n",
                shell_quote(&dir.to_string_lossy())
            ));
        }
        line.push_str(&format!("exec {}", shell_quote(program)));
        for arg in args {
            line.push(' ');
            line.push_str(&shell_quote(arg));
        }
        line.push('\n');
        (reach.script_for(&line, timeout, program, stop)).map_err(|t| t.text().to_owned())
    }

    /// The user's home folder there.
    fn home(&self) -> Option<PathBuf> {
        let Self::There(reach) = self else {
            return std::env::var_os("HOME").map(PathBuf::from);
        };
        let said = Self::said(reach, "printf %s \"$HOME\"\n", &NEVER).ok()?;
        (!said.trim().is_empty()).then(|| PathBuf::from(said.trim()))
    }

    /// Whether `tool` is there to run.
    fn has(&self, tool: &str) -> bool {
        let Self::There(reach) = self else {
            let found = Command::new(tool)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return found.is_ok();
        };
        let script = format!("command -v {} >/dev/null 2>&1\n", shell_quote(tool));
        Self::said(reach, &script, &NEVER).is_ok()
    }

    /// The text of file `path`, if it can be read.
    fn read(&self, path: &Path) -> Option<String> {
        let Self::There(reach) = self else {
            return fs::read_to_string(path).ok();
        };
        let script = format!("cat {} 2>/dev/null\n", shell_quote(&path.to_string_lossy()));
        Self::said(reach, &script, &NEVER).ok()
    }

    /// Write `text` to file `path`, making its folder.
    fn write(&self, path: &Path, text: &str) -> Result<(), String> {
        let shown = path.display();
        let parent = path.parent().unwrap_or(path);
        self.make_dir(parent)?;
        let Self::There(reach) = self else {
            return fs::write(path, text).map_err(|e| format!("couldn't write {shown}: {e}"));
        };
        let script = format!(
            "cat > {} <<'GERDR_SANDBOX_FILE'\n{text}\nGERDR_SANDBOX_FILE\n",
            shell_quote(&path.to_string_lossy())
        );
        Self::said(reach, &script, &NEVER)
            .map(drop)
            .map_err(|why| format!("couldn't write {shown}: {why}"))
    }

    fn make_dir(&self, path: &Path) -> Result<(), String> {
        let shown = path.display();
        let Self::There(reach) = self else {
            return fs::create_dir_all(path).map_err(|e| format!("couldn't make {shown}: {e}"));
        };
        let script = format!("mkdir -p {}\n", shell_quote(&path.to_string_lossy()));
        Self::said(reach, &script, &NEVER)
            .map(drop)
            .map_err(|why| format!("couldn't make {shown}: {why}"))
    }

    /// Whether `path` is there.
    fn exists(&self, path: &Path) -> bool {
        let Self::There(reach) = self else {
            return path.exists();
        };
        let script = format!("[ -e {} ]\n", shell_quote(&path.to_string_lossy()));
        Self::said(reach, &script, &NEVER).is_ok()
    }

    /// Whether git tracks what `path` names, a file or a folder's content.
    fn tracked(&self, path: &Path) -> bool {
        let Self::There(reach) = self else {
            let (dir, name) = if path.is_dir() {
                (path, ".".to_owned())
            } else {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                (path.parent().unwrap_or(path), name.unwrap_or_default())
            };
            let tracked = Command::new("git")
                .args(["ls-files", "--", &name])
                .current_dir(dir)
                .stderr(Stdio::null())
                .output();
            return matches!(tracked, Ok(out) if out.status.success() && !out.stdout.is_empty());
        };
        let path = shell_quote(&path.to_string_lossy());
        let script = format!(
            "if [ -d {path} ]; then cd {path} && git ls-files -- .; else cd \"$(dirname {path})\" && git ls-files -- \"$(basename {path})\"; fi 2>/dev/null\n"
        );
        Self::said(reach, &script, &NEVER).is_ok_and(|said| !said.trim().is_empty())
    }

    /// A port to publish a copy on, free here on both loopback addresses and
    /// not among `taken`.
    fn free_port(&self, taken: &std::collections::BTreeSet<u16>) -> Result<u16, String> {
        let Self::There(reach) = self else {
            return sandbox::free_port(taken);
        };
        let tables = "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null; exit 0\n";
        let said = Self::said(reach, tables, &NEVER)?;
        let held: std::collections::BTreeSet<u16> = listening_in(&said).into_iter().collect();
        let spread = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |at| at.subsec_nanos());
        free_among(&held, taken, spread).ok_or_else(|| "no free port found".to_owned())
    }

    /// Start the process that keeps the namespaces alive.
    fn start_holder(&self) -> Result<Proc, String> {
        let Self::There(reach) = self else {
            return start_holder();
        };
        let said = Self::said(reach, HOLDER_SCRIPT, &NEVER);
        said.ok().and_then(|said| started_proc(&said)).ok_or_else(|| {
            // Most likely this system doesn't let a user make namespaces.
            "Couldn't start its network: unshare made no namespace".to_owned()
        })
    }

    /// Start pasta, the proxy leading `forwards`' ports inside to the host.
    fn start_pasta(&self, holder: &Proc, forwards: &[Forward], dir: &Path) -> Result<Proc, String> {
        let Self::There(reach) = self else {
            return start_pasta(holder, forwards, dir);
        };
        let log = dir.join("pasta.log");
        let script = pasta_script(&pasta_args(holder.pid, forwards), &log.to_string_lossy());
        let said = Self::said(reach, &script, &NEVER).map_err(|why| format!("Couldn't start pasta: {why}"))?;
        started_proc(&said).ok_or_else(|| {
            let text = self.read(&log).unwrap_or_default();
            format!("pasta stopped: {}", last_lines(&text, "no reason given"))
        })
    }

    /// Run the shell line `command` in `dir`, what it says added to file `log`.
    fn run_logged(
        reach: &Reach,
        command: &str,
        dir: &Path,
        log: &Path,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let script = format!(
            "cd {} || exit 1\n{command} >> {} 2>&1\n",
            shell_quote(&dir.to_string_lossy()),
            shell_quote(&log.to_string_lossy()),
        );
        Self::script(reach, &script, setup::TIMEOUT, "It", stop)
    }

    /// Remove folder `dir` and what's in it.
    fn remove_dir(&self, dir: &Path) -> Result<(), String> {
        let shown = dir.display();
        let Self::There(reach) = self else {
            return match fs::remove_dir_all(dir) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(format!("{shown}: {err}")),
            };
        };
        let script = format!("rm -rf -- {}\n", shell_quote(&dir.to_string_lossy()));
        Self::said(reach, &script, &NEVER).map(drop).map_err(|why| format!("{shown}: {why}"))
    }
}

/// A port not in `held` nor `taken`, starting from a place `spread` picks: two
/// sandboxes starting at once shouldn't go for one port.
fn free_among(
    held: &std::collections::BTreeSet<u16>,
    taken: &std::collections::BTreeSet<u16>,
    spread: u32,
) -> Option<u16> {
    const FIRST: u32 = 49152;
    const COUNT: u32 = 10000;
    (0..COUNT)
        .map(|i| (FIRST + (spread + i) % COUNT) as u16)
        .find(|port| !held.contains(port) && !taken.contains(port))
}

/// The script starting what keeps the namespaces there, and saying its pid and
/// `/proc` stat line. A background job here is no group leader, so `setsid` runs
/// `unshare` in place, and it becomes `sleep`: the pid stays.
const HOLDER_SCRIPT: &str = "own=$(readlink /proc/$$/ns/net)\n\
setsid unshare --user --map-current-user --net sleep infinity </dev/null >/dev/null 2>&1 &\n\
p=$!\n\
i=0\n\
while [ \"$i\" -lt 150 ]; do\n\
net=$(readlink /proc/$p/ns/net 2>/dev/null)\n\
if [ -n \"$net\" ] && [ \"$net\" != \"$own\" ]; then echo \"$p\"; cat /proc/$p/stat; exit 0; fi\n\
sleep 0.02\n\
i=$((i+1))\n\
done\n\
kill \"$p\" 2>/dev/null\n\
exit 3\n";

/// The script starting pasta there, its output in file `log`, and saying its pid
/// and `/proc` stat line once it stayed a moment. Nothing more once it is gone.
fn pasta_script(args: &[String], log: &str) -> String {
    let args: Vec<String> = args.iter().map(|arg| shell_quote(arg)).collect();
    format!(
        "setsid pasta {} </dev/null >{} 2>&1 &\np=$!\nsleep 0.3\necho \"$p\"\ncat /proc/$p/stat 2>/dev/null\nexit 0\n",
        args.join(" "),
        shell_quote(log),
    )
}

/// The process a start script says: its pid, then its `/proc` stat line.
fn started_proc(said: &str) -> Option<Proc> {
    let mut lines = said.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let start = start_in_stat(lines.next()?)?;
    Some(Proc { pid, start })
}

/// A compose project of a sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackRun {
    pub name: String,
    pub file: PathBuf,
    /// What runs it: main's stack's.
    #[serde(default, skip_serializing_if = "Engine::is_docker")]
    pub engine: Engine,
}

/// A process, told from a later one with the same pid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proc {
    pub pid: i32,
    /// When it started, as `/proc/<pid>/stat` says.
    pub start: u64,
}

impl Proc {
    fn of(pid: i32) -> Option<Self> {
        Some(Self {
            pid,
            start: start_of(pid)?,
        })
    }

    /// Whether it still runs.
    pub fn alive(&self) -> bool {
        start_of(self.pid) == Some(self.start)
    }

    fn kill(&self) {
        if self.alive() {
            // SAFETY: a plain signal to the process `alive` just told from any
            // later one with this pid.
            unsafe { libc::kill(self.pid, libc::SIGTERM) };
        }
    }
}

/// When process `pid` started; `None` when it is gone or a zombie.
fn start_of(pid: i32) -> Option<u64> {
    start_in_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The start time in the text of a `/proc/<pid>/stat`, whose second field (the
/// name) can hold anything. A zombie counts as gone.
fn start_in_stat(stat: &str) -> Option<u64> {
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    if fields.next()? == "Z" {
        return None;
    }
    // After the name and state: starttime is the 22nd field of the line.
    fields.nth(18)?.parse().ok()
}

/// The shell line that turns a pane's shell into `command` inside the sandbox
/// whose namespace `holder` keeps, with `env` set on top, and then into a shell
/// there, so whatever is typed or run again stays inside.
///
/// The command isn't typed for that shell to read: its rc files can read the
/// terminal too, and then a command loses its first letters. With `set -m` it
/// runs as a job of its own, the terminal's foreground, so it shows as what the
/// pane runs, and Ctrl+C ends it and not what comes after.
pub fn enter_line(holder: i32, slug: &str, env: &[(String, String)], command: &str) -> String {
    let mut line = format!(
        "exec nsenter --user --net --preserve-credentials -t {holder} -- env {SANDBOX_VAR}={}",
        shell_quote(slug)
    );
    for (key, value) in env {
        line.push_str(&format!(" {}={}", key, shell_quote(value)));
    }
    let script = format!("set -m; {command}\nexec \"${{SHELL:-sh}}\"");
    line.push_str(&format!(" sh -c {}", shell_quote(&script)));
    line
}

/// `command` with what names a repository's main folder naming its worktree: a
/// command seen running names its program by full path, and main's `.venv`
/// would import main's code.
pub fn in_worktrees(command: &str, repos: &[Repo]) -> String {
    let mut repos: Vec<&Repo> = repos.iter().collect();
    // Deepest first, as repositories can nest.
    repos.sort_by_key(|repo| std::cmp::Reverse(repo.main.components().count()));
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    'text: while !rest.is_empty() {
        for repo in &repos {
            let main = repo.main.to_string_lossy();
            if let Some(after) = rest.strip_prefix(main.as_ref())
                && after.chars().next().is_none_or(|c| !is_name_char(c))
                && out
                    .chars()
                    .next_back()
                    .is_none_or(|c| !is_name_char(c) && c != '/')
            {
                out.push_str(&repo.worktree.to_string_lossy());
                rest = after;
                continue 'text;
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Whether `c` can go on a file name, so a path going on with it is another
/// path: `/ao/api2` isn't inside `/ao/api`.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Run `program` with `args`, its output read: what it wrote to stdout, or why
/// it failed. Past `timeout` or once `stop` is set, it is killed.
fn output(
    dir: Option<&Path>,
    program: &str,
    args: &[&str],
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<String, String> {
    let mut command = Command::new(program);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("couldn't run {program}: {err}"))?;
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            // What was read before an error is what there is to say.
            let _ = pipe.read_to_end(&mut text);
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = child.stdout.take().map(|pipe| read(Box::new(pipe)));
    let err = child.stderr.take().map(|pipe| read(Box::new(pipe)));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout || stop.load(Ordering::Relaxed) => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => return Err(format!("couldn't follow {program}: {err}")),
        }
    };
    if status.is_none() {
        // It may have just ended; `wait` reaps it either way.
        let _ = child.kill();
        let _ = child.wait();
    }
    let out = out.and_then(|t| t.join().ok()).unwrap_or_default();
    let err = err.and_then(|t| t.join().ok()).unwrap_or_default();
    match status {
        Some(status) if status.success() => Ok(out),
        Some(status) => Err(last_lines(&err, &format!("{program} {status}"))),
        None => Err(format!("{program} was stopped or ran out of time")),
    }
}

/// The end of `text` as the reason for a failure, or `otherwise` when empty.
fn last_lines(text: &str, otherwise: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    match lines.len() {
        0 => otherwise.to_owned(),
        n => lines[n.saturating_sub(3)..].join(" "),
    }
}

/// Run `engine`'s program with `args`.
fn tool(host: &Host, engine: Engine, args: &[&str], stop: &AtomicBool) -> Result<String, String> {
    host.output(engine.program(), args, ENGINE_TIMEOUT, stop)
}

/// Which tool is missing for a sandbox's network, in words. What runs its
/// containers is `engines`' to find.
pub fn tools_missing(host: &Host) -> Option<String> {
    let tools = ["pasta", "unshare", "nsenter"];
    let tools = tools.iter().chain(match host {
        Host::Here => &[][..],
        // A sandbox's terminals and a look at its app need it there; the
        // holder and the proxy start through it.
        Host::There(_) => &["setsid", "bash"][..],
    });
    for tool in tools {
        if !host.has(tool) {
            return Some(format!("{tool} isn't installed, and a sandbox needs it"));
        }
    }
    None
}

/// What runs containers on `host` with their compose projects: Docker,
/// Podman, or both. When neither does, why, in words, with what to do.
pub fn engines(host: &Host) -> Result<Vec<Engine>, String> {
    let stop = AtomicBool::new(false);
    let quick = Duration::from_secs(15);
    let mut found = Vec::new();
    let mut problems = Vec::new();
    if host.has("docker") {
        let info = ["info", "--format", "{{.ServerVersion}}"];
        if host.output("docker", &info, quick, &stop).is_err() {
            problems.push("Docker isn't running, and a sandbox needs it");
        } else if (host.output("docker", &["compose", "version"], quick, &stop)).is_err() {
            problems.push("Docker Compose isn't installed, and a sandbox needs it");
        } else {
            found.push(Engine::Docker);
        }
    }
    if host.has("podman") {
        match host.output("podman", &["compose", "version"], quick, &stop) {
            Ok(said) if said.contains("podman-compose") => found.push(Engine::PodmanCompose),
            // Docker's compose talks to Podman's service, which runs only once
            // the user turns it on.
            Ok(_) if (host.output("podman", &["compose", "ls"], quick, &stop)).is_ok() => {
                found.push(Engine::Podman);
            }
            Ok(_) => problems.push(
                "Podman's service isn't on, and docker-compose needs it: run systemctl --user enable --now podman.socket",
            ),
            Err(_) => problems.push(
                "Podman has no compose, and a sandbox needs one: install podman-compose or docker-compose",
            ),
        }
    }
    if !found.is_empty() {
        // Its containers aren't looked at: a sandbox that finds none of a
        // project's says why only here.
        for problem in &problems {
            tracing::warn!("sandbox: {problem}");
        }
        return Ok(found);
    }
    Err((problems.first().copied())
        .unwrap_or("Neither Docker nor Podman is installed, and a sandbox needs one")
        .to_owned())
}

/// What it takes to start a sandbox. The paths are those of the machine it
/// starts on.
pub struct Job<'a> {
    /// Where it starts.
    pub host: &'a Host,
    pub project: &'a str,
    pub slug: &'a str,
    /// This machine's id, for what's started to say whose it is. Empty if it has none.
    pub owner: &'a str,
    pub repos: &'a [Repo],
    /// The folders whose compose projects are the feature's: its projects'.
    pub roots: &'a [PathBuf],
    /// The sandbox's own folder.
    pub dir: &'a Path,
    /// What to run once in the namespace before its services.
    pub prepare: &'a [PrepareStep],
    pub env: &'a [(String, String)],
    /// Ports other sandboxes hold, though stopped.
    pub taken: &'a [u16],
    pub stop: &'a AtomicBool,
}

/// A command to run once in the namespace before its services.
pub struct PrepareStep {
    pub dir: PathBuf,
    /// The shell line that runs it.
    pub line: String,
    /// The command as the project wrote it: what the user is told of.
    pub shown: String,
}

/// Start a sandbox. Everything a failed start did stays for the next start or
/// `remove` to deal with; the result names it, so nothing is lost track of.
pub fn start(job: &Job, progress: &mut dyn FnMut(String)) -> SandboxRun {
    let mut run = SandboxRun {
        started: now(),
        ..SandboxRun::default()
    };
    if let Err(why) = start_into(&mut run, job, progress) {
        tracing::warn!("sandbox of {}: {why}", job.slug);
        run.failed = Some(why);
    }
    run.finished = Some(now());
    run
}

fn start_into(
    run: &mut SandboxRun,
    job: &Job,
    progress: &mut dyn FnMut(String),
) -> Result<(), String> {
    if let Some(missing) = tools_missing(job.host) {
        return Err(missing);
    }
    let engines = engines(job.host)?;
    job.host.make_dir(job.dir)?;
    // Paused, main's containers would be left out of the copy.
    unpause_left(job.host, job.dir)?;
    let containers = running_containers(job.host, &engines, job.stop)?;
    let mut stacks = sandbox::stacks_under(&containers, job.roots);
    // Main's stack needn't run for its data to be copied: after a reboot it
    // mostly doesn't, and without it the sandbox has no database.
    let stopped = stopped_containers(job.host, &engines, job.stop)?;
    let stopped = sandbox::stopped_stacks(&stopped, &stacks, job.roots);
    for stack in &stopped {
        tracing::info!(
            "sandbox of {}: {} isn't running in main, copying it as it was left",
            job.slug,
            stack.project
        );
    }
    stacks.extend(stopped);
    run.not_copied = Vec::new();
    for shown in sandbox::loose(&containers).iter().map(|c| c.shown()) {
        // A pod's containers go by its name, once.
        if !run.not_copied.contains(&shown) {
            run.not_copied.push(shown);
        }
    }
    run.outward = outward_of(job);
    run.out_of_reach = out_of_reach_of(job, &containers);
    let mut taken: std::collections::BTreeSet<u16> = job.taken.iter().copied().collect();
    let mut starting: Vec<Vec<String>> = Vec::new();
    for stack in &stacks {
        progress(format!("Reading {}", stack.project));
        let config = compose_config(stack, job)?;
        let engine = stack.engine;
        let name = sandbox::compose_name(job.project, job.slug, &stack.project);
        let label = sandbox::feature_label(job.project, job.slug);
        let running = stack.services.keys().cloned().collect();
        let file = job.dir.join(format!("{name}.json"));
        let mut next_port = |taken: &std::collections::BTreeSet<u16>| job.host.free_port(taken);
        let generated = sandbox::generate(
            &config,
            Input {
                name: &name,
                label: &label,
                owner: job.owner,
                running: &running,
                data_dir: job.dir,
                taken: &taken,
                copies_bind: &|path| copies_bind(job.host, path),
                next_port: &mut next_port,
            },
        )?;
        taken.extend(generated.forwards.iter().map(|f| f.host_port));
        let mut compose = generated.compose.clone();
        if engine == Engine::PodmanCompose {
            sandbox::for_podman_compose(&mut compose);
        }
        let text = serde_json::to_string_pretty(&compose).map_err(|e| e.to_string())?;
        job.host.write(&file, &text)?;
        copy_data(stack, &generated, &config, job, run, progress)?;
        run.forwards.extend(generated.forwards);
        let services = (compose["services"].as_object().into_iter().flatten())
            .map(|(service, _)| service.clone())
            .collect();
        starting.push(services);
        run.stacks.push(StackRun { name, file, engine });
    }
    for (stack, services) in run.stacks.clone().into_iter().zip(starting) {
        progress(format!("Starting {}", stack.name));
        if stack.engine == Engine::PodmanCompose {
            start_podman_compose(job.host, &stack, &services, job.stop)?;
            continue;
        }
        let file = stack.file.to_string_lossy();
        // Fresh containers each start: the file names new host ports, and `up`
        // on a stopped stack would recreate them and orphan each anonymous
        // volume, which then outlives the sandbox. Named volumes stay.
        let args = [
            "compose",
            "-p",
            &stack.name,
            "-f",
            &file,
            "rm",
            "-f",
            "-s",
            "-v",
        ];
        tool(job.host, stack.engine, &args, job.stop)?;
        let args = ["compose", "-p", &stack.name, "-f", &file, "up", "-d"];
        tool(job.host, stack.engine, &args, job.stop)?;
        wait_ready(job.host, &stack, job.stop)?;
    }
    progress("Making its network".to_owned());
    let holder = job.host.start_holder()?;
    run.holder = Some(holder.clone());
    let pasta = job.host.start_pasta(&holder, &run.forwards, job.dir)?;
    run.pasta = Some(pasta);
    wait_loopback(job.host, &holder);
    for (i, step) in job.prepare.iter().enumerate() {
        if job.stop.load(Ordering::Relaxed) {
            return Err("Stopped".to_owned());
        }
        progress(format!(
            "{} ({} of {})",
            step.shown,
            i + 1,
            job.prepare.len()
        ));
        run_inside(&holder, job, step).map_err(|why| {
            // The likely reason it failed, which its own error can't tell.
            if run.out_of_reach.is_empty() {
                why
            } else {
                format!("{why}. It {}", out_of_reach_text(&run.out_of_reach))
            }
        })?;
    }
    Ok(())
}

/// What the `.env` files of the feature's repositories can reach, unless the
/// project set it otherwise for sandboxes.
fn outward_of(job: &Job) -> Vec<String> {
    let mut keys: Vec<String> = job
        .repos
        .iter()
        .filter_map(|repo| job.host.read(&repo.worktree.join(".env")))
        .flat_map(|text| outward_keys(&text))
        .filter(|key| !overridden(key, job.env))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// What the `.env` files of the feature's repositories point at that the
/// sandbox won't have, going by what the project set for sandboxes.
fn out_of_reach_of(job: &Job, containers: &[sandbox::Container]) -> Vec<String> {
    let mut found: Vec<String> = job
        .repos
        .iter()
        .filter_map(|repo| job.host.read(&repo.worktree.join(".env")))
        .flat_map(|text| {
            sandbox::out_of_reach(&sandbox_pairs(&text, job.env), containers, job.roots)
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The variables a sandbox runs with: the `.env` file's in `text`, with `env`
/// going over them.
fn sandbox_pairs(text: &str, env: &[(String, String)]) -> Vec<(String, String)> {
    let mut pairs = sandbox::env_pairs(text);
    pairs.retain(|(key, _)| !overridden(key, env));
    pairs.extend_from_slice(env);
    pairs
}

/// What's wrong with a sandbox that has `found` out of reach, and what to do
/// about it, to follow its name.
pub fn out_of_reach_text(found: &[String]) -> String {
    format!(
        "can't reach what runs outside its project: {}. Give the project its own compose stack, or set workflow.sandbox_env in the project file",
        found.join(", ")
    )
}

/// Whether the project set `key` for sandboxes. Setting `EMAIL_HOST` also
/// covers `EMAIL_HOST_USER`: the login is of no use with the host gone.
fn overridden(key: &str, env: &[(String, String)]) -> bool {
    env.iter().any(|(set, _)| key.starts_with(set.as_str()))
}

/// The running containers of `engines`, whichever project they are of.
pub fn running_containers(
    host: &Host,
    engines: &[Engine],
    stop: &AtomicBool,
) -> Result<Vec<sandbox::Container>, String> {
    // Podman leaves paused ones out unless asked, Docker lists them.
    containers_in(host, engines, &["running", "paused"], stop)
}

/// The stopped containers of `engines`: exited, or made and never started.
fn stopped_containers(
    host: &Host,
    engines: &[Engine],
    stop: &AtomicBool,
) -> Result<Vec<sandbox::Container>, String> {
    containers_in(host, engines, &["exited", "created"], stop)
}

/// The containers of `engines` in one of `states`.
fn containers_in(
    host: &Host,
    engines: &[Engine],
    states: &[&str],
    stop: &AtomicBool,
) -> Result<Vec<sandbox::Container>, String> {
    let filters: Vec<String> = states.iter().map(|s| format!("status={s}")).collect();
    let mut all = Vec::new();
    for &engine in engines {
        let mut ps = vec!["ps", "-a", "-q"];
        for filter in &filters {
            ps.extend(["--filter", filter.as_str()]);
        }
        let ids = tool(host, engine, &ps, stop)?;
        let ids: Vec<&str> = ids.split_whitespace().collect();
        if ids.is_empty() {
            continue;
        }
        let mut args = vec!["inspect"];
        args.extend(&ids);
        let mut containers = sandbox::containers_of(&tool(host, engine, &args, stop)?, engine)?;
        if containers.iter().any(|c| c.pod.is_some()) {
            let format = ["pod", "ps", "--no-trunc", "--format", "{{.ID}} {{.Name}}"];
            match tool(host, engine, &format, stop) {
                Ok(pods) => sandbox::name_pods(&mut containers, &pods),
                Err(why) => {
                    tracing::warn!("sandbox: couldn't read pod names: {why}");
                    sandbox::name_pods(&mut containers, "");
                }
            }
        }
        all.extend(containers);
    }
    Ok(all)
}

/// `path` in the feature if it is there, else as it is.
fn in_feature(host: &Host, path: &Path, repos: &[Repo]) -> PathBuf {
    let mapped = sandbox::in_worktree(path, repos);
    if host.exists(&mapped) {
        mapped
    } else {
        path.to_owned()
    }
}

/// `docker compose config` of `stack`, read from the feature's files. Under
/// main's project name, so the names it spells out are the ones of main's
/// volumes, which are what gets copied.
fn compose_config(stack: &Stack, job: &Job) -> Result<Value, String> {
    let dir = in_feature(job.host, &stack.working_dir, job.repos);
    let podman_compose = stack.engine == Engine::PodmanCompose;
    let mut owned = vec!["compose".to_owned(), "-p".to_owned(), stack.project.clone()];
    if !podman_compose {
        owned.push("--project-directory".to_owned());
        owned.push(dir.to_string_lossy().into_owned());
    }
    if let Some(env) = &stack.env_file {
        owned.push("--env-file".to_owned());
        owned.push(in_feature(job.host, env, job.repos).to_string_lossy().into_owned());
    }
    for file in &stack.config_files {
        let file = config_file(job.host, file, &stack.working_dir);
        owned.push("-f".to_owned());
        owned.push(
            in_feature(job.host, &file, job.repos)
                .to_string_lossy()
                .into_owned(),
        );
    }
    if podman_compose {
        // It has no `--project-directory` and prints only YAML, in the
        // short syntax as written.
        owned.push("config".to_owned());
        let args: Vec<&str> = owned.iter().map(String::as_str).collect();
        let program = stack.engine.program();
        let text = job
            .host
            .output_in(Some(&dir), program, &args, ENGINE_TIMEOUT, job.stop)?;
        let home = job.host.home();
        return sandbox::from_podman_compose(&text, &stack.project, &dir, home.as_deref());
    }
    owned.extend(["config", "--format", "json"].map(str::to_owned));
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    let text = tool(job.host, stack.engine, &args, job.stop)?;
    serde_json::from_str(&text).map_err(|e| format!("couldn't read the compose file: {e}"))
}

/// Compose file `file` of a stack in folder `dir`. podman-compose names it as
/// it was given, from wherever it ran: mostly the folder, else the file is
/// looked for there by name.
fn config_file(host: &Host, file: &Path, dir: &Path) -> PathBuf {
    match file.file_name() {
        Some(name) if !host.exists(file) && host.exists(&dir.join(name)) => dir.join(name),
        _ => file.to_owned(),
    }
}

/// Whether a writable bind mount of `path` holds data to copy. What git tracks
/// is the project's files, like a config, and the system's folders are shared
/// on purpose, like the docker socket.
fn copies_bind(host: &Host, path: &Path) -> bool {
    if SYSTEM_DIRS.iter().any(|dir| path.starts_with(dir)) {
        return false;
    }
    !host.tracked(path)
}

/// Unpauses its containers however a copy ends. If gerdr dies meanwhile they
/// stay paused, and main's app with them: the note `paused_file` holds lets
/// `unpause_left` find them.
struct Paused {
    containers: Vec<String>,
    host: Host,
    engine: Engine,
    dir: PathBuf,
}

/// A container a copy paused, as the sandbox's folder notes it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PausedNote {
    engine: Engine,
    container: String,
}

/// Where sandbox folder `dir` notes the containers a copy has paused. Empty
/// or missing: none.
fn paused_file(dir: &Path) -> PathBuf {
    dir.join("paused")
}

impl Paused {
    fn new(
        host: &Host,
        engine: Engine,
        containers: Vec<String>,
        dir: &Path,
        stop: &AtomicBool,
    ) -> Result<Self, String> {
        let notes: Vec<PausedNote> = (containers.iter())
            .map(|container| PausedNote {
                engine,
                container: container.clone(),
            })
            .collect();
        let text = serde_json::to_string(&notes).map_err(|e| e.to_string())?;
        // Noted before pausing: a gerdr that dies between the two leaves
        // nothing unnoted.
        host.write(&paused_file(dir), &text)?;
        let mut paused = Self {
            containers: Vec::new(),
            host: host.clone(),
            engine,
            dir: dir.to_owned(),
        };
        for container in containers {
            tool(host, engine, &["pause", &container], stop)?;
            paused.containers.push(container);
        }
        Ok(paused)
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        let never = AtomicBool::new(false);
        let mut all = true;
        for container in &self.containers {
            if let Err(why) = tool(&self.host, self.engine, &["unpause", container], &never) {
                tracing::error!("couldn't unpause {container}: {why}");
                all = false;
            }
        }
        // What didn't unpause stays noted, for `unpause_left` to try again.
        if all && let Err(why) = self.host.write(&paused_file(&self.dir), "") {
            tracing::error!("sandbox: couldn't clear {}: {why}", self.dir.display());
        }
    }
}

/// What `paused_file`'s `text` notes.
fn paused_notes(text: &str) -> Result<Vec<PausedNote>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// Unpause what a copy of sandbox folder `dir` paused and a gerdr that died
/// left so. What another unpaused already, or what is gone, needs nothing.
pub fn unpause_left(host: &Host, dir: &Path) -> Result<(), String> {
    let file = paused_file(dir);
    let Some(text) = host.read(&file) else {
        return Ok(());
    };
    let notes = paused_notes(&text).map_err(|why| format!("{}: {why}", file.display()))?;
    if notes.is_empty() {
        return Ok(());
    }
    let never = AtomicBool::new(false);
    let mut problems = Vec::new();
    for PausedNote { engine, container } in &notes {
        let state = ["inspect", "--format", "{{.State.Paused}}", container];
        match tool(host, *engine, &state, &never) {
            Ok(said) if said.trim() == "true" => {
                match tool(host, *engine, &["unpause", container], &never) {
                    Ok(_) => tracing::info!(
                        "sandbox: unpaused {container}, which a copy cut short left paused"
                    ),
                    Err(why) => problems.push(format!("Couldn't unpause {container}: {why}")),
                }
            }
            Ok(_) => {}
            Err(why) => {
                tracing::warn!("sandbox: couldn't look at {container}, noted paused: {why}")
            }
        }
    }
    if problems.is_empty() {
        host.write(&file, "")
    } else {
        Err(problems.join("; "))
    }
}

/// The mark that volume `name`'s copy finished. Its folder is the sandbox's, so
/// it goes with it. A copy cut short has none, and is made again.
fn copy_mark(dir: &Path, name: &str) -> PathBuf {
    dir.join("copied").join(name)
}

/// Copy `stack`'s data for `generated`, what isn't there yet: what a sandbox
/// did to its data stays over a stop.
fn copy_data(
    stack: &Stack,
    generated: &Generated,
    config: &Value,
    job: &Job,
    run: &mut SandboxRun,
    progress: &mut dyn FnMut(String),
) -> Result<(), String> {
    for copy in &generated.copies {
        run.copied.push(copy.from.clone());
        let mark = copy_mark(job.dir, &copy.to);
        if job.host.exists(&mark) {
            continue;
        }
        let image = copy
            .services
            .iter()
            .find_map(|service| Some(stack.services.get(service)?.image.clone()))
            .or_else(|| {
                let service = copy.services.first()?;
                config["services"][service]["image"]
                    .as_str()
                    .map(str::to_owned)
            })
            .ok_or_else(|| format!("Volume {} has no image to copy it with", copy.from))?;
        let engine = stack.engine;
        if tool(
            job.host,
            engine,
            &["volume", "inspect", &copy.from],
            job.stop,
        )
        .is_err()
        {
            // Nothing in main to copy: it starts empty, as main's did.
            run.copied.pop();
            continue;
        }
        let containers: Vec<String> = copy
            .services
            .iter()
            .filter_map(|service| Some(stack.services.get(service)?.name.clone()))
            .collect();
        progress(format!(
            "Copying {} ({} paused)",
            copy.from,
            containers.join(", ")
        ));
        let label = format!(
            "{}={}",
            sandbox::LABEL,
            sandbox::feature_label(job.project, job.slug)
        );
        // Whatever is in it is a copy cut short.
        let _ = tool(job.host, engine, &["volume", "rm", &copy.to], job.stop);
        let create = ["volume", "create", "--label", &label, &copy.to];
        tool(job.host, engine, &create, job.stop)?;
        let from = format!("{}:/from:ro", copy.from);
        let to = format!("{}:/to", copy.to);
        let args = [
            "run",
            "--rm",
            "--network",
            "none",
            "--user",
            "0:0",
            "--entrypoint",
            "cp",
            "-v",
            &from,
            "-v",
            &to,
            &image,
            "-a",
            "--reflink=auto",
            "/from/.",
            "/to/",
        ];
        let copied = {
            let _paused = Paused::new(job.host, engine, containers, job.dir, job.stop)?;
            job.host
                .output(engine.program(), &args, COPY_TIMEOUT, job.stop)
        };
        if let Err(why) = copied {
            // A copy cut short mustn't be taken for data. Failing to remove it
            // changes nothing: without a mark, the next start copies again.
            let _ = tool(job.host, engine, &["volume", "rm", &copy.to], job.stop);
            return Err(format!("Couldn't copy volume {}: {why}", copy.from));
        }
        job.host.write(&mark, "")?;
    }
    for bind in &generated.binds {
        let mark = bind.to.with_extension("copied");
        if job.host.exists(&mark) {
            continue;
        }
        if !job.host.exists(&bind.from) {
            job.host.make_dir(&bind.to)?;
            continue;
        }
        progress(format!("Copying {}", bind.from.display()));
        if let Some(parent) = bind.to.parent() {
            job.host.make_dir(parent)?;
        }
        let (from, to) = (bind.from.to_string_lossy(), bind.to.to_string_lossy());
        let args = ["-a", "--reflink=auto", from.as_ref(), to.as_ref()];
        (job.host.output("cp", &args, COPY_TIMEOUT, job.stop))
            .map_err(|why| format!("Couldn't copy {from}: {why}"))?;
        job.host.write(&mark, "")?;
    }
    run.fresh
        .extend(generated.fresh.iter().map(|f| f.service.clone()));
    Ok(())
}

/// How a compose project's containers stand.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ready {
    Yes,
    Waiting(String),
    Failed(String),
}

/// Read `docker compose ps -a --format json`: a JSON object a line. A one-off
/// that ended well counts as ready.
fn ready_of(text: &str) -> Ready {
    let mut waiting = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(item) = serde_json::from_str::<Value>(line) else {
            return Ready::Waiting("reading compose".to_owned());
        };
        let service = item["Service"].as_str().unwrap_or("a service").to_owned();
        let state = item["State"].as_str().unwrap_or_default();
        let health = item["Health"].as_str().unwrap_or_default();
        let code = item["ExitCode"].as_i64().unwrap_or(0);
        match (state, health) {
            ("exited" | "dead", _) if code != 0 => {
                return Ready::Failed(format!("{service} exited with {code}"));
            }
            ("exited", _) | ("running", "" | "healthy") => {}
            (_, "unhealthy") => return Ready::Failed(format!("{service} is unhealthy")),
            _ => waiting = Some(service),
        }
    }
    waiting.map_or(Ready::Yes, Ready::Waiting)
}

/// Wait for `stack`'s containers to come up, health checks included.
fn wait_ready(host: &Host, stack: &StackRun, stop: &AtomicBool) -> Result<(), String> {
    let file = stack.file.to_string_lossy();
    let args = [
        "compose",
        "-p",
        &stack.name,
        "-f",
        &file,
        "ps",
        "-a",
        "--format",
        "json",
    ];
    let started = Instant::now();
    let program = stack.engine.program();
    loop {
        match ready_of(&tool(host, stack.engine, &args, stop)?) {
            Ready::Yes => return Ok(()),
            Ready::Failed(why) => {
                return Err(format!("{why} ({program} compose -p {} logs)", stack.name));
            }
            Ready::Waiting(service) if started.elapsed() >= READY_TIMEOUT => {
                return Err(format!("{service} didn't come up in time"));
            }
            Ready::Waiting(_) => {}
        }
        if stop.load(Ordering::Relaxed) {
            return Err("Stopped".to_owned());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Start podman-compose project `stack` afresh, its containers up with health
/// checks passed. podman-compose has no `rm`, and its `up` waits without end
/// for a service another needs when that one fails: what runs is watched
/// meanwhile, and a failure cuts it short.
fn start_podman_compose(
    host: &Host,
    stack: &StackRun,
    services: &[String],
    stop: &AtomicBool,
) -> Result<(), String> {
    // Fresh containers, as with Docker's (see `start_into`).
    remove_by_labels(host, stack.engine, &stack.name, false, stop)?;
    let file = stack.file.to_string_lossy();
    let args = ["compose", "-p", &stack.name, "-f", &file, "up", "-d"];
    let cut = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let (up, failed) = std::thread::scope(|scope| {
        let watch = scope.spawn(|| {
            let mut last = Instant::now();
            while !done.load(Ordering::Relaxed) {
                if stop.load(Ordering::Relaxed) {
                    cut.store(true, Ordering::Relaxed);
                    return None;
                }
                if last.elapsed() >= Duration::from_secs(2) {
                    last = Instant::now();
                    let ready = ready_by_labels(host, stack, services, stop);
                    if let Ok(Ready::Failed(why)) = ready {
                        cut.store(true, Ordering::Relaxed);
                        return Some(why);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            None
        });
        let up = tool(host, stack.engine, &args, &cut);
        done.store(true, Ordering::Relaxed);
        (up, watch.join().ok().flatten())
    });
    if let Some(why) = failed {
        return Err(why);
    }
    up?;
    let started = Instant::now();
    loop {
        match ready_by_labels(host, stack, services, stop)? {
            Ready::Yes => return Ok(()),
            Ready::Failed(why) => return Err(why),
            Ready::Waiting(service) if started.elapsed() >= READY_TIMEOUT => {
                return Err(format!("{service} didn't come up in time"));
            }
            Ready::Waiting(_) => {}
        }
        if stop.load(Ordering::Relaxed) {
            return Err("Stopped".to_owned());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The filter for what compose project `stack` made.
fn project_filter(stack: &str) -> String {
    format!("label={}={stack}", sandbox::LABEL_PROJECT)
}

/// How the containers of `stack` stand, read from their labels: for a compose
/// that can't say.
fn ready_by_labels(
    host: &Host,
    stack: &StackRun,
    services: &[String],
    stop: &AtomicBool,
) -> Result<Ready, String> {
    let filter = project_filter(&stack.name);
    let ids = tool(
        host,
        stack.engine,
        &["ps", "-a", "-q", "--filter", &filter],
        stop,
    )?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if ids.is_empty() {
        return Ok(Ready::Waiting("its containers".to_owned()));
    }
    let mut args = vec!["inspect"];
    args.extend(&ids);
    let text = tool(host, stack.engine, &args, stop)?;
    Ok(ready_of_inspect(&text, services, stack.engine.program()))
}

/// Read `<engine> inspect` of a compose project's containers, which should
/// run `services`. A one-off that ended well counts as ready. `program` names
/// what shows a container's logs.
fn ready_of_inspect(text: &str, services: &[String], program: &str) -> Ready {
    let Ok(list) = serde_json::from_str::<Vec<Value>>(text) else {
        return Ready::Waiting("reading its containers".to_owned());
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut waiting = None;
    for item in &list {
        let service = item["Config"]["Labels"][sandbox::LABEL_SERVICE]
            .as_str()
            .unwrap_or("a service")
            .to_owned();
        seen.insert(service.clone());
        let name = item["Name"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches('/');
        let state = &item["State"];
        let status = state["Status"].as_str().unwrap_or_default();
        let health = state["Health"]["Status"].as_str().unwrap_or_default();
        let code = state["ExitCode"].as_i64().unwrap_or(0);
        let error = state["Error"].as_str().unwrap_or_default();
        match (status, health) {
            ("exited" | "stopped" | "dead", _) if code != 0 => {
                return Ready::Failed(format!(
                    "{service} exited with {code} ({program} logs {name})"
                ));
            }
            (status, _) if !error.is_empty() && status != "running" => {
                return Ready::Failed(format!("{service} didn't start: {error}"));
            }
            (_, "unhealthy") => {
                return Ready::Failed(format!("{service} is unhealthy ({program} logs {name})"));
            }
            ("exited" | "stopped", _) | ("running", "" | "healthy") => {}
            _ => waiting = Some(service),
        }
    }
    let missing = || {
        services
            .iter()
            .find(|service| !seen.contains(*service))
            .cloned()
    };
    waiting.or_else(missing).map_or(Ready::Yes, Ready::Waiting)
}

/// Remove the containers of compose project `stack` with their anonymous
/// volumes, as `compose rm -f -s -v` does, and with `all` also its networks
/// and the pod podman-compose put them in: for when its file is gone, or the
/// compose has no `rm`. Named volumes stay.
fn remove_by_labels(
    host: &Host,
    engine: Engine,
    stack: &str,
    all: bool,
    stop: &AtomicBool,
) -> Result<(), String> {
    let filter = project_filter(stack);
    let ids = tool(host, engine, &["ps", "-a", "-q", "--filter", &filter], stop)?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if !ids.is_empty() {
        let mut args = vec!["rm", "-f", "-v"];
        args.extend(&ids);
        tool(host, engine, &args, stop)?;
    }
    if engine == Engine::PodmanCompose {
        let pod = format!("pod_{stack}");
        tool(host, engine, &["pod", "rm", "-f", "--ignore", &pod], stop)?;
    }
    if all {
        let ids = tool(
            host,
            engine,
            &["network", "ls", "-q", "--filter", &filter],
            stop,
        )?;
        for id in ids.split_whitespace() {
            tool(host, engine, &["network", "rm", id], stop)?;
        }
    }
    Ok(())
}

/// Stop the containers of compose project `stack`, by their labels.
fn stop_by_labels(
    host: &Host,
    engine: Engine,
    stack: &str,
    stop: &AtomicBool,
) -> Result<(), String> {
    let filter = project_filter(stack);
    let ids = tool(host, engine, &["ps", "-q", "--filter", &filter], stop)?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if ids.is_empty() {
        return Ok(());
    }
    let mut args = vec!["stop"];
    args.extend(&ids);
    tool(host, engine, &args, stop).map(drop)
}

/// Start `command` in a session of its own, so it outlives a gerdr restart and
/// no signal for a terminal reaches it; a thread reaps it when it ends. Its
/// output goes to `log`.
fn start_detached(command: &mut Command, log: Option<File>) -> Result<i32, String> {
    let (out, err) = match log {
        Some(file) => {
            let copy = file.try_clone().map_err(|e| e.to_string())?;
            (Stdio::from(file), Stdio::from(copy))
        }
        None => (Stdio::null(), Stdio::null()),
    };
    command.stdin(Stdio::null()).stdout(out).stderr(err);
    // SAFETY: `setsid` is async-signal-safe, and nothing else runs in the hook.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id() as i32;
    std::thread::spawn(move || {
        // Only reaps it: nobody waits for how it ended.
        let _ = child.wait();
    });
    Ok(pid)
}

/// Start the process that keeps the namespaces alive. `unshare` becomes `sleep`,
/// keeping its pid.
fn start_holder() -> Result<Proc, String> {
    let own = fs::read_link("/proc/self/ns/net").map_err(|e| e.to_string())?;
    let pid = start_detached(
        Command::new("unshare").args([
            "--user",
            "--map-current-user",
            "--net",
            "sleep",
            "infinity",
        ]),
        None,
    )
    .map_err(|why| format!("Couldn't start its network: {why}"))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        if let Ok(net) = fs::read_link(format!("/proc/{pid}/ns/net"))
            && net != own
            && let Some(proc) = Proc::of(pid)
        {
            return Ok(proc);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Most likely this system doesn't let a user make namespaces.
    Err("Couldn't start its network: unshare made no namespace".to_owned())
}

/// Start pasta, the proxy leading `forwards`' ports inside to the host.
fn start_pasta(holder: &Proc, forwards: &[Forward], dir: &Path) -> Result<Proc, String> {
    let log = dir.join("pasta.log");
    let file = File::create(&log).map_err(|e| format!("couldn't write {}: {e}", log.display()))?;
    let pid = start_detached(
        Command::new("pasta").args(pasta_args(holder.pid, forwards)),
        Some(file),
    )
    .map_err(|why| format!("Couldn't start pasta: {why}"))?;
    std::thread::sleep(Duration::from_millis(300));
    Proc::of(pid).ok_or_else(|| {
        let text = fs::read_to_string(&log).unwrap_or_default();
        format!("pasta stopped: {}", last_lines(&text, "no reason given"))
    })
}

fn pasta_args(holder: i32, forwards: &[Forward]) -> Vec<String> {
    let mut args: Vec<String> = ["-f", "--config-net", "-t", "none", "-u", "none"]
        .map(str::to_owned)
        .to_vec();
    args.extend(["-T".to_owned(), sandbox::pasta_spec(forwards, false)]);
    args.extend(["-U".to_owned(), sandbox::pasta_spec(forwards, true)]);
    args.extend([
        "--no-netns-quit".to_owned(),
        "--userns".to_owned(),
        format!("/proc/{holder}/ns/user"),
        "--netns".to_owned(),
        format!("/proc/{holder}/ns/net"),
    ]);
    args
}

/// Wait until the loopback address inside is up, which is pasta's to do. Without
/// `ip` to ask, the first connection will tell.
fn wait_loopback(host: &Host, holder: &Proc) {
    let pid = holder.pid.to_string();
    let args = [
        "--user",
        "--net",
        "--preserve-credentials",
        "-t",
        &pid,
        "--",
        "ip",
        "-o",
        "addr",
        "show",
        "lo",
    ];
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        match host.output("nsenter", &args, Duration::from_secs(2), &stop) {
            Ok(text) if text.contains("127.0.0.1") => return,
            Ok(_) | Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    tracing::warn!("the sandbox's loopback address didn't show up");
}

/// Run shell `line` inside the sandbox, in `dir`, its output logged.
fn run_inside(holder: &Proc, job: &Job, step: &PrepareStep) -> Result<(), String> {
    let PrepareStep { dir, line, shown } = step;
    use std::io::Write;
    let log = job.dir.join("prepare.log");
    if let Host::There(reach) = job.host {
        return run_inside_there(reach, holder, job, step, &log);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| format!("couldn't write {}: {e}", log.display()))?;
    // A log line lost doesn't stop the sandbox.
    let _ = writeln!(file, "== {line}");
    // Where this run's output starts: the log keeps the earlier runs'.
    let from = file.metadata().map_or(0, |m| m.len() as usize);
    let out = file.try_clone().map_err(|e| e.to_string())?;
    let command = format!("exec {}", inside_line(holder, job, line));
    let exit: Exit = setup::shell_until(
        &command,
        dir,
        job.slug,
        Stdio::from(out),
        Stdio::from(file),
        setup::TIMEOUT,
        job.stop,
    );
    if exit.passed() {
        Ok(())
    } else {
        // Unreadable, the log it points at says as much as this would.
        let said = fs::read(&log).unwrap_or_default();
        let error = last_error(said.get(from..).unwrap_or_default())
            .map_or_else(String::new, |error| format!(": {error}"));
        Err(format!(
            "`{shown}` {}{error} (see {})",
            exit.describe(),
            log.display()
        ))
    }
}

/// The shell line running `line` in the sandbox `holder` keeps.
fn inside_line(holder: &Proc, job: &Job, line: &str) -> String {
    let mut command = format!(
        "nsenter --user --net --preserve-credentials -t {} -- env {SANDBOX_VAR}={}",
        holder.pid,
        shell_quote(job.slug)
    );
    for (key, value) in job.env {
        command.push_str(&format!(" {}={}", key, shell_quote(value)));
    }
    command.push_str(&format!(" sh -c {}", shell_quote(line)));
    command
}

/// `run_inside` on another machine. Stopping it here ends only the connection:
/// what's in the sandbox then goes with its network, as a stop takes that down.
fn run_inside_there(
    reach: &Reach,
    holder: &Proc,
    job: &Job,
    step: &PrepareStep,
    log: &Path,
) -> Result<(), String> {
    let PrepareStep { dir, line, shown } = step;
    let quoted = shell_quote(&log.to_string_lossy());
    // Where this run's output starts: the log keeps the earlier runs'.
    let mark = format!("wc -c < {quoted} 2>/dev/null || echo 0\n");
    let from: usize = (job.host.output("sh", &["-c", &mark], THERE_TIMEOUT, job.stop))
        .ok()
        .and_then(|said| said.trim().parse().ok())
        .unwrap_or(0);
    let command = inside_line(holder, job, line);
    let ran = Host::run_logged(reach, &command, dir, log, job.stop);
    ran.map_err(|why| {
        // Unreadable, the log it points at says as much as this would.
        let said = job.host.read(log).unwrap_or_default();
        let error = last_error(said.as_bytes().get(from..).unwrap_or_default())
            .unwrap_or(why);
        format!("`{shown}` failed: {error} (see {} there)", log.display())
    })
}

/// The line of output `raw` that best says why a command failed: the last one
/// not indented. What's indented continues it: a traceback's error comes last
/// but for its hints.
fn last_error(raw: &[u8]) -> Option<String> {
    let lines = super::verify::clean_lines(raw, false);
    let said: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let line = said
        .iter()
        .rev()
        .find(|line| !line.starts_with(char::is_whitespace))
        .or(said.last())?
        .trim();
    Some(match line.char_indices().nth(ERROR_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &line[..end]),
        None => line.to_owned(),
    })
}

/// Stop `run`: its network, then its containers. With `remove`, its containers,
/// its data and `dir` too, and nothing of it remains. What couldn't be stopped
/// is the error, with the rest still done.
///
/// `host` is where it runs, and `dir` its folder there: what a record read on
/// another machine says of a path there may have been translated. With its compose
/// file gone docker finds the project by its name, and then knows only the volumes
/// compose made: those made for a copy go by name, as in `remove_stack`.
pub fn stop(run: &SandboxRun, dir: &Path, remove: bool, host: &Host) -> Result<(), String> {
    if matches!(host, Host::Here) && run.on_another_machine() {
        return Err("it runs on another machine".to_owned());
    }
    let never = AtomicBool::new(false);
    let mut problems = Vec::new();
    // Before its folder can go, with the note of what to unpause.
    if let Err(why) = unpause_left(host, dir) {
        problems.push(why);
    }
    if let Err(why) = run.kill_network(host) {
        problems.push(why);
    }
    for stack in &run.stacks {
        let file = dir.join(format!("{}.json", stack.name));
        let file = match host {
            Host::Here => stack.file.clone(),
            Host::There(_) => file,
        };
        let shown = file.to_string_lossy();
        let has_file = host.exists(&file);
        let mut args = vec!["compose", "-p", &stack.name];
        if has_file {
            args.extend(["-f", &shown]);
        }
        if remove {
            args.extend(["down", "-v", "--remove-orphans"]);
        } else {
            args.push("stop");
        }
        let engine = stack.engine;
        // podman-compose takes no command without a file.
        let done = match (engine, has_file) {
            (Engine::PodmanCompose, false) if remove => {
                remove_by_labels(host, engine, &stack.name, true, &never)
            }
            (Engine::PodmanCompose, false) => stop_by_labels(host, engine, &stack.name, &never),
            _ => host
                .output(engine.program(), &args, STOP_TIMEOUT, &never)
                .map(drop),
        };
        if let Err(why) = done {
            problems.push(format!("{}: {why}", stack.name));
        } else if remove && let Err(why) = remove_copies(host, engine, &stack.name, &never) {
            problems.push(format!("{}: {why}", stack.name));
        }
    }
    if remove
        && problems.is_empty()
        && let Err(why) = host.remove_dir(dir)
    {
        problems.push(why);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// Remove compose project `stack`, whose file may be gone, with its volumes.
/// Volumes made for a copy before compose ran go by name, which holds a hash
/// of the feature's, so nothing else has it.
///
/// `dir` is the sandbox's folder on a machine, where nothing else takes it away.
pub fn remove_stack(
    stack: &str,
    engine: Engine,
    dir: Option<&Path>,
    host: &Host,
) -> Result<(), String> {
    let never = AtomicBool::new(false);
    if engine == Engine::PodmanCompose {
        remove_by_labels(host, engine, stack, true, &never)?;
    } else {
        let args = ["compose", "-p", stack, "down", "-v", "--remove-orphans"];
        host.output(engine.program(), &args, STOP_TIMEOUT, &never)?;
    }
    remove_copies(host, engine, stack, &never)?;
    dir.map_or(Ok(()), |dir| host.remove_dir(dir))
}

/// Remove the volumes of compose project `stack` that made it through its going
/// down. Their names hold a hash of the feature's, so nothing else has them.
fn remove_copies(
    host: &Host,
    engine: Engine,
    stack: &str,
    stop: &AtomicBool,
) -> Result<(), String> {
    let filter = format!("name=^{stack}_");
    let names = tool(
        host,
        engine,
        &["volume", "ls", "-q", "--filter", &filter],
        stop,
    )?;
    for name in names.split_whitespace() {
        tool(host, engine, &["volume", "rm", name], stop)?;
    }
    Ok(())
}

/// A compose project carrying `LABEL`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Labelled {
    /// What `LABEL` says.
    pub feature: String,
    pub stack: String,
    /// What `OWNER` says. Empty for what started before there was one.
    pub owner: String,
    /// The day its first container was made, like "2026-11-07".
    pub started: String,
    /// What runs it.
    pub engine: Engine,
}

/// The compose projects whose containers on `host` carry `LABEL`: what a removed
/// feature could leave behind.
pub fn labelled(host: &Host, stop: &AtomicBool) -> Result<Vec<Labelled>, String> {
    let filter = format!("label={}", sandbox::LABEL);
    let format = format!(
        "{{{{.Label \"{}\"}}}}\t{{{{.Label \"{}\"}}}}\t{{{{.Label \"{}\"}}}}\t{{{{.CreatedAt}}}}",
        sandbox::LABEL,
        sandbox::LABEL_PROJECT,
        sandbox::OWNER
    );
    let mut found = Vec::new();
    let mut failed = None;
    let engines = engines(host)?;
    for &engine in &engines {
        let args = ["ps", "-a", "--filter", &filter, "--format", &format];
        match tool(host, engine, &args, stop) {
            Ok(text) => found.extend(read_labelled(&text, engine)),
            Err(why) => failed = Some(format!("{}: {why}", engine.name())),
        }
    }
    match failed {
        // What one of them has is still worth telling.
        Some(why) if engines.len() == 1 => Err(why),
        Some(why) => {
            tracing::warn!("looking for sandboxes left behind: {why}");
            Ok(found)
        }
        None => Ok(found),
    }
}

/// What `<engine> ps` said in the format `labelled` asks.
fn read_labelled(text: &str, engine: Engine) -> Vec<Labelled> {
    let mut found: Vec<Labelled> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(Labelled {
                feature: parts.next()?.to_owned(),
                stack: parts.next()?.to_owned(),
                owner: parts.next().unwrap_or_default().to_owned(),
                started: (parts.next().unwrap_or_default())
                    .split(' ')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                engine,
            })
        })
        .collect();
    // A stack a line per container: the earliest says when it started.
    found.sort();
    found.dedup_by(|later, first| {
        let key = |l: &Labelled| {
            (
                l.engine,
                l.feature.clone(),
                l.stack.clone(),
                l.owner.clone(),
            )
        };
        key(later) == key(first)
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_carries_the_label_is_read_with_or_without_an_owner() {
        let found = read_labelled(
            "p/a\tgerdr-a\tme\t2026-11-08 10:00:00 +0200 EET\n\
             p/a\tgerdr-a\tme\t2026-11-07 09:00:00 +0200 EET\n\
             p/old\tgerdr-old\t\t2026-01-02 03:04:05 +0000 UTC\n",
            Engine::Podman,
        );
        let of = |feature: &str, stack: &str, owner: &str, started: &str| Labelled {
            feature: feature.into(),
            stack: stack.into(),
            owner: owner.into(),
            started: started.into(),
            engine: Engine::Podman,
        };
        assert_eq!(
            found,
            [
                of("p/a", "gerdr-a", "me", "2026-11-07"),
                of("p/old", "gerdr-old", "", "2026-01-02")
            ]
        );
    }

    fn podman_container(
        service: &str,
        status: &str,
        code: i64,
        health: &str,
        error: &str,
    ) -> Value {
        serde_json::json!({
            "Name": format!("s_{service}_1"),
            "Config": {"Labels": {"com.docker.compose.service": service}},
            "State": {"Status": status, "ExitCode": code, "Health": {"Status": health}, "Error": error},
        })
    }

    #[test]
    fn readiness_is_read_from_the_containers_themselves() {
        let services = ["db".to_owned(), "web".to_owned(), "migrate".to_owned()];
        let read = |items: Vec<Value>| {
            ready_of_inspect(&Value::Array(items).to_string(), &services, "podman")
        };
        let db = podman_container("db", "running", 0, "healthy", "");
        let web = podman_container("web", "running", 0, "", "");
        let migrate = podman_container("migrate", "exited", 0, "", "");
        assert_eq!(
            read(vec![db.clone(), web.clone(), migrate.clone()]),
            Ready::Yes
        );
        assert_eq!(
            read(vec![db.clone(), web.clone()]),
            Ready::Waiting("migrate".to_owned())
        );
        let starting = podman_container("db", "running", 0, "starting", "");
        assert_eq!(
            read(vec![starting, web.clone(), migrate.clone()]),
            Ready::Waiting("db".to_owned())
        );
        let crashed = podman_container("migrate", "exited", 3, "", "");
        assert_eq!(
            read(vec![db.clone(), web.clone(), crashed]),
            Ready::Failed("migrate exited with 3 (podman logs s_migrate_1)".to_owned())
        );
        let refused = podman_container("web", "created", 0, "", "port 80 is taken");
        assert_eq!(
            read(vec![db.clone(), refused, migrate.clone()]),
            Ready::Failed("web didn't start: port 80 is taken".to_owned())
        );
        let sick = podman_container("db", "running", 0, "unhealthy", "");
        assert_eq!(
            read(vec![sick, web, migrate]),
            Ready::Failed("db is unhealthy (podman logs s_db_1)".to_owned())
        );
    }

    #[test]
    fn a_stack_of_docker_keeps_its_old_record() {
        let docker = StackRun {
            name: "s".into(),
            file: "/f".into(),
            engine: Engine::Docker,
        };
        let text = serde_json::to_string(&docker).unwrap();
        assert_eq!(text, r#"{"name":"s","file":"/f"}"#);
        let podman = StackRun {
            engine: Engine::PodmanCompose,
            ..docker
        };
        let text = serde_json::to_string(&podman).unwrap();
        assert!(text.contains(r#""engine":"podman-compose""#), "{text}");
        assert_eq!(serde_json::from_str::<StackRun>(&text).unwrap(), podman);
    }

    fn repo(main: &str, worktree: &str) -> Repo {
        Repo {
            name: "r".to_owned(),
            main: PathBuf::from(main),
            base: "main".to_owned(),
            base_commit: "abc".to_owned(),
            worktree: PathBuf::from(worktree),
            touched: true,
            branch: None,
            project: None,
        }
    }

    #[test]
    fn a_stat_line_gives_the_start_time_whatever_the_name() {
        let tail = "S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 424242 20 21";
        assert_eq!(start_in_stat(&format!("99 (sleep) {tail}")), Some(424242));
        assert_eq!(start_in_stat(&format!("99 (a) b (c) {tail}")), Some(424242));
        let zombie = tail.replacen('S', "Z", 1);
        assert_eq!(start_in_stat(&format!("99 (sleep) {zombie}")), None);
        assert_eq!(start_in_stat("nonsense"), None);
    }

    #[test]
    fn this_process_is_alive_and_a_wrong_start_time_is_not() {
        let pid = std::process::id() as i32;
        let me = Proc::of(pid).unwrap();
        assert!(me.alive());
        assert!(
            !Proc {
                start: me.start + 1,
                pid
            }
            .alive()
        );
    }

    #[test]
    fn a_run_is_alive_only_with_its_network() {
        let me = Proc::of(std::process::id() as i32).unwrap();
        let live = SandboxRun {
            holder: Some(me.clone()),
            pasta: Some(me.clone()),
            ..SandboxRun::default()
        };
        assert!(live.alive());
        assert!(
            !SandboxRun {
                stopped: true,
                ..live.clone()
            }
            .alive()
        );
        assert!(
            !SandboxRun {
                failed: Some("x".into()),
                ..live.clone()
            }
            .alive()
        );
        assert!(
            !SandboxRun {
                pasta: None,
                ..live
            }
            .alive()
        );
    }

    #[test]
    fn a_seen_command_moves_to_the_worktree() {
        let repos = [repo("/ao/api", "/ao/.features/x/api")];
        assert_eq!(
            in_worktrees(
                "/ao/api/.venv/bin/python /ao/api/.venv/bin/celery -A api worker",
                &repos
            ),
            "/ao/.features/x/api/.venv/bin/python /ao/.features/x/api/.venv/bin/celery -A api worker"
        );
        assert_eq!(
            in_worktrees("cd /ao/api && run", &repos),
            "cd /ao/.features/x/api && run"
        );
    }

    #[test]
    fn only_a_whole_path_moves() {
        let repos = [repo("/ao/api", "/w")];
        assert_eq!(in_worktrees("/ao/api2/x", &repos), "/ao/api2/x");
        assert_eq!(in_worktrees("/ao/api.old/x", &repos), "/ao/api.old/x");
        assert_eq!(in_worktrees("/mnt/ao/api/x", &repos), "/mnt/ao/api/x");
        assert_eq!(in_worktrees("npm run dev", &repos), "npm run dev");
    }

    #[test]
    fn the_enter_line_quotes_what_it_sets() {
        let env = [("EMAIL_HOST".to_owned(), "a b".to_owned())];
        let line = enter_line(42, "fix", &env, "npm run dev");
        assert!(
            line.starts_with(&format!(
                "exec nsenter --user --net --preserve-credentials -t 42 -- env {SANDBOX_VAR}=fix EMAIL_HOST='a b' sh -c "
            )),
            "{line}"
        );
        assert!(line.contains("set -m; npm run dev"), "{line}");
        assert!(line.contains("exec \"${SHELL:-sh}\""), "{line}");
    }

    #[test]
    fn pasta_gets_explicit_namespaces() {
        let forwards = [Forward {
            service: "db".to_owned(),
            main_port: 5433,
            host_port: 41001,
            udp: false,
        }];
        assert_eq!(
            pasta_args(7, &forwards).join(" "),
            "-f --config-net -t none -u none -T 5433:41001 -U none --no-netns-quit \
             --userns /proc/7/ns/user --netns /proc/7/ns/net"
        );
    }

    #[test]
    fn readiness_is_read_from_compose() {
        let line = |service: &str, state: &str, health: &str, code: i32| {
            format!(
                r#"{{"Service":"{service}","State":"{state}","Health":"{health}","ExitCode":{code}}}"#
            )
        };
        assert_eq!(ready_of(""), Ready::Yes);
        assert_eq!(ready_of(&line("db", "running", "", 0)), Ready::Yes);
        assert_eq!(ready_of(&line("db", "running", "healthy", 0)), Ready::Yes);
        assert_eq!(ready_of(&line("init", "exited", "", 0)), Ready::Yes);
        assert_eq!(
            ready_of(&line("db", "running", "starting", 0)),
            Ready::Waiting("db".to_owned())
        );
        assert_eq!(
            ready_of(&line("db", "restarting", "", 1)),
            Ready::Waiting("db".to_owned())
        );
        assert_eq!(
            ready_of(&line("db", "exited", "", 3)),
            Ready::Failed("db exited with 3".to_owned())
        );
        assert_eq!(
            ready_of(&line("db", "running", "unhealthy", 0)),
            Ready::Failed("db is unhealthy".to_owned())
        );
        let two = format!(
            "{}\n{}",
            line("a", "running", "", 0),
            line("b", "created", "", 0)
        );
        assert_eq!(ready_of(&two), Ready::Waiting("b".to_owned()));
    }

    #[test]
    fn a_sandbox_elsewhere_may_run_until_its_machine_says_otherwise() {
        let there = |elsewhere| SandboxRun {
            elsewhere: Some(elsewhere),
            ..SandboxRun::default()
        };
        assert!(there(Elsewhere::default()).may_run_there());
        let off = Elsewhere {
            asked: true,
            ..Elsewhere::default()
        };
        assert!(!there(off.clone()).may_run_there());
        let on = Elsewhere {
            alive: true,
            ..off
        };
        assert!(there(on.clone()).may_run_there());
        let stopped = SandboxRun {
            stopped: true,
            ..there(on)
        };
        assert!(!stopped.may_run_there());
    }

    #[test]
    fn a_sandbox_elsewhere_stands_as_its_machine_says() {
        let run = SandboxRun {
            holder: Some(Proc { pid: 1, start: 22 }),
            pasta: Some(Proc { pid: 2, start: 33 }),
            forwards: vec![Forward {
                service: "db".into(),
                main_port: 5432,
                host_port: 15432,
                udp: false,
            }],
            ..SandboxRun::default()
        };
        let stat = |start: u64| {
            let fields = vec!["0"; 18].join(" ");
            format!("1 (a b) S {fields} {start} 0")
        };
        let table = "  sl  local_address rem_address   st\n\
                     0: 00000000:1435 00000000:0000 0A\n\
                     1: 00000000:1538 00000000:0000 0A\n\
                     2: 00000000:0050 00000000:0000 01";
        let v6 = "  sl  local_address rem_address   st\n\
                  0: 00000000000000000000000000000000:1F40 00000000000000000000000000000000:0000 0A";
        let said = format!(
            "@holder x\n{}\n@pasta x\n{}\n@ports x\n{table}\n{v6}\n@holder other\nnope\n",
            stat(22),
            stat(33),
        );
        // The proxy's own port is left out, and what isn't listening.
        assert_eq!(
            read_there(&said, "x", &run),
            Elsewhere {
                asked: true,
                alive: true,
                ports: vec![5173, 8000],
            }
        );
        // A later process with that number isn't it, and nothing said is nothing.
        let later = said.replace(&stat(33), &stat(34));
        assert!(!read_there(&later, "x", &run).alive);
        assert_eq!(read_there(&later, "x", &run).ports, Vec::<u16>::new());
        assert!(!read_there("", "x", &run).alive);
    }

    #[test]
    fn the_numbers_of_a_sandbox_elsewhere_say_nothing_here() {
        // This very process is alive, and it isn't what the record means.
        let me = Proc::of(std::process::id() as i32).unwrap();
        let run = SandboxRun {
            holder: Some(me.clone()),
            pasta: Some(me),
            elsewhere: Some(Elsewhere::default()),
            ..SandboxRun::default()
        };
        assert!(!run.alive());
        assert!(run.listening().is_empty());
        assert!(stop(&run, Path::new("/nonexistent"), true, &Host::Here).is_err());
        // Never saved: what it is depends on who reads it.
        let json = serde_json::to_string(&run).unwrap();
        assert!(!json.contains("elsewhere"), "{json}");
    }

    #[test]
    fn listening_ports_are_read_from_the_table() {
        let table =
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid
   0: 0100007F:2328 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000
   1: 0100007F:1068 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000
   2: 00000000:1068 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000
";
        assert_eq!(listening_in(table), [9000, 4200]);
        assert_eq!(listening_in(""), [0u16; 0]);
    }

    #[test]
    fn the_app_is_a_dev_server_before_an_api() {
        assert_eq!(app_port(&[4200, 9000]), Some(4200));
        assert_eq!(app_port(&[5000, 9000]), Some(5000));
        assert_eq!(app_port(&[7777, 6000]), Some(6000));
        assert_eq!(app_port(&[]), None);
    }

    #[test]
    fn only_outward_names_are_told_never_values() {
        let env = "
DATABASE_HOST=localhost
EMAIL_HOST=smtp.example.com
export SMTP_PASSWORD=hunter2
#MAIL_HOST=off
STRIPE_KEY=
WEBHOOK_URL=https://x
";
        let keys = outward_keys(env);
        assert_eq!(keys, ["EMAIL_HOST", "SMTP_PASSWORD", "WEBHOOK_URL"]);
        assert!(!keys.join(" ").contains("hunter2"));
    }

    #[test]
    fn what_a_sandbox_sets_goes_over_the_env_file() {
        let env = [("DATABASE_URL".to_owned(), "sqlite:///x".to_owned())];
        let text = "DATABASE_URL=postgres://127.0.0.1:5433/app\nCACHE=redis://localhost:6379\n";
        let pairs = sandbox_pairs(text, &env);
        let urls: Vec<&str> = pairs.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(urls, ["redis://localhost:6379", "sqlite:///x"]);
    }

    #[test]
    fn a_traceback_tells_its_error_not_its_hint() {
        let log = b"Traceback (most recent call last):\n  File \"x.py\", line 1\n\
django.db.utils.OperationalError: connection refused\n\
\tIs the server running?\n\n";
        assert_eq!(
            last_error(log).as_deref(),
            Some("django.db.utils.OperationalError: connection refused")
        );
    }

    #[test]
    fn an_error_is_cut_and_nothing_said_is_none() {
        assert_eq!(last_error(b"\n  \n"), None);
        assert_eq!(
            last_error(b"  only indented\n").as_deref(),
            Some("only indented")
        );
        let long = "\u{e9}".repeat(ERROR_CHARS + 1);
        let cut = last_error(long.as_bytes()).unwrap();
        assert_eq!(cut.chars().count(), ERROR_CHARS + 1);
        assert!(cut.ends_with('\u{2026}'));
    }

    #[test]
    fn overriding_a_key_covers_its_login_too() {
        let env = [("EMAIL_HOST".to_owned(), "127.0.0.1".to_owned())];
        assert!(overridden("EMAIL_HOST", &env));
        assert!(overridden("EMAIL_HOST_PASSWORD", &env));
        assert!(!overridden("SMTP_PASSWORD", &env));
        assert!(!overridden("EMAIL_HOST", &[]));
    }

    #[test]
    fn system_folders_stay_shared() {
        assert!(!copies_bind(&Host::Here, Path::new("/var/run/docker.sock")));
        assert!(!copies_bind(&Host::Here, Path::new("/etc/hosts")));
    }

    /// Against real docker, on the stack `gerdr-spike-src` the sandbox spike
    /// left: `docker compose -p gerdr-spike-src -f ~/.cache/gerdr-sandbox-spike/compose.yml up -d`
    /// first, then `cargo test --bin gerdr real_sandbox -- --ignored --nocapture`.
    /// Leaves nothing of the copy behind; never touches another stack.
    #[test]
    #[ignore = "needs docker, pasta and the spike's stack running"]
    fn real_sandbox_copies_data_and_leads_ports_to_it() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let root = home.join(".cache/gerdr-sandbox-spike");
        let dir = home.join(".cache/gerdr-sandbox-test");
        let stop = AtomicBool::new(false);
        let job = Job {
            host: &Host::Here,
            project: "spike",
            slug: "test",
            owner: "me",
            repos: &[],
            roots: std::slice::from_ref(&root),
            dir: &dir,
            prepare: &[],
            env: &[],
            taken: &[],
            stop: &stop,
        };
        let run = start(&job, &mut |what| eprintln!("{what}"));
        eprintln!("{run:#?}");
        let result = (|| -> Result<(), String> {
            if let Some(why) = &run.failed {
                return Err(why.clone());
            }
            assert!(run.alive());
            assert!(run.copied.iter().any(|v| v == "gerdr-spike-src_pgdata"));
            let db = run
                .forwards
                .iter()
                .find(|f| f.main_port == 45433)
                .ok_or("no forward for the database")?;
            let pid = run.holder.as_ref().unwrap().pid.to_string();
            let probe = format!("exec 3<>/dev/tcp/127.0.0.1/{} && echo up", db.main_port);
            let args = [
                "--user",
                "--net",
                "--preserve-credentials",
                "-t",
                &pid,
                "--",
                "bash",
                "-c",
                &probe,
            ];
            let said = output(None, "nsenter", &args, Duration::from_secs(5), &stop)?;
            assert_eq!(said.trim(), "up");
            let name = &run.stacks[0].name;
            let container = format!("{name}-db-1");
            let mounts = tool(
                &Host::Here,
                Engine::Docker,
                &[
                    "inspect",
                    &container,
                    "--format",
                    "{{range .Mounts}}{{.Name}} {{end}}",
                ],
                &stop,
            )?;
            assert!(!mounts.contains("gerdr-spike-src_pgdata"), "{mounts}");
            let rows = tool(
                &Host::Here,
                Engine::Docker,
                &[
                    "exec",
                    &container,
                    "psql",
                    "-U",
                    "postgres",
                    "-d",
                    "spike",
                    "-tc",
                    "select * from t",
                ],
                &stop,
            )?;
            assert!(rows.contains("from-source"), "{rows}");
            Ok(())
        })();
        let stopped = stop_for_test(&run, &dir);
        result.unwrap();
        stopped.unwrap();
        let never = AtomicBool::new(false);
        let left = tool(&Host::Here, Engine::Docker, &["volume", "ls", "-q"], &never).unwrap();
        assert!(!left.contains("gerdr-test"), "{left}");
        assert!(!dir.exists());
        assert!(!run.holder.unwrap().alive());
    }

    #[test]
    fn paused_notes_read_empty_as_none() {
        assert_eq!(paused_notes(""), Ok(Vec::new()));
        assert_eq!(paused_notes(" \n"), Ok(Vec::new()));
        let text = r#"[{"engine":"podman-compose","container":"asset-track_db_1"}]"#;
        let notes = paused_notes(text).unwrap();
        assert_eq!(
            notes,
            [PausedNote {
                engine: Engine::PodmanCompose,
                container: "asset-track_db_1".to_owned()
            }]
        );
        assert!(paused_notes("{broken").is_err());
    }

    #[test]
    fn a_folder_with_no_note_has_nothing_to_unpause() {
        let dir = std::env::temp_dir().join(format!("gerdr-paused-{}", std::process::id()));
        assert_eq!(unpause_left(&Host::Here, &dir), Ok(()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(paused_file(&dir), "").unwrap();
        assert_eq!(unpause_left(&Host::Here, &dir), Ok(()));
        fs::write(paused_file(&dir), "{broken").unwrap();
        assert!(unpause_left(&Host::Here, &dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A gerdr that died copying: the container it paused is listed as
    /// running, and the next look unpauses it and clears the note.
    #[test]
    #[ignore = "needs podman and the alpine:3.20 image"]
    fn real_podman_container_left_paused_is_seen_and_unpaused() {
        let stop = AtomicBool::new(false);
        let podman = |args: &[&str]| tool(&Host::Here, Engine::Podman, args, &stop);
        let name = format!("gerdr-paused-test-{}", std::process::id());
        let dir = std::env::temp_dir().join(&name);
        fs::create_dir_all(&dir).unwrap();
        podman(&[
            "run",
            "-d",
            "--name",
            &name,
            "--entrypoint",
            "sleep",
            "docker.io/library/alpine:3.20",
            "300",
        ])
        .unwrap();
        let paused =
            Paused::new(&Host::Here, Engine::Podman, vec![name.clone()], &dir, &stop).unwrap();
        std::mem::forget(paused);
        let result = (|| -> Result<(), String> {
            let listed = running_containers(&Host::Here, &[Engine::Podman], &stop)?;
            if !listed.iter().any(|c| c.name == name) {
                return Err("a paused container isn't listed".into());
            }
            unpause_left(&Host::Here, &dir)?;
            let state = podman(&["inspect", "--format", "{{.State.Paused}}", &name])?;
            if state.trim() != "false" {
                return Err(format!("still paused: {state}"));
            }
            match fs::read_to_string(paused_file(&dir)) {
                Ok(text) if text.is_empty() => Ok(()),
                other => Err(format!("note not cleared: {other:?}")),
            }
        })();
        podman(&["rm", "-f", &name]).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        result.unwrap();
    }

    /// Against real podman with podman-compose, on a stack in folder
    /// `$GERDR_PODMAN_STACK` that podman compose runs there: a service `db`
    /// publishing a port, with file `x` in its volume at `/data` and file `b`
    /// in its bind mount at `/bind`. Set
    /// `PODMAN_COMPOSE_PROVIDER` to try podman-compose when Docker's compose
    /// would be found first.
    /// Leaves nothing of the copy behind; never touches another stack.
    #[test]
    #[ignore = "needs podman, podman-compose, pasta and the stack running"]
    fn real_podman_sandbox_copies_data_and_leads_ports_to_it() {
        let root = PathBuf::from(std::env::var("GERDR_PODMAN_STACK").unwrap());
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let dir = home.join(".cache/gerdr-podman-sandbox-test");
        let stop = AtomicBool::new(false);
        let found = engines(&Host::Here).unwrap();
        assert!(found.iter().any(|e| !e.is_docker()), "{found:?}");
        let job = Job {
            host: &Host::Here,
            project: "podman",
            slug: "test",
            owner: "me",
            repos: &[],
            roots: std::slice::from_ref(&root),
            dir: &dir,
            prepare: &[],
            env: &[],
            taken: &[],
            stop: &stop,
        };
        let run = start(&job, &mut |what| eprintln!("{what}"));
        eprintln!("{run:#?}");
        let podman = |args: &[&str]| tool(&Host::Here, Engine::Podman, args, &stop);
        let result = (|| -> Result<(), String> {
            if let Some(why) = &run.failed {
                return Err(why.clone());
            }
            assert!(run.alive());
            let engine = run.stacks[0].engine;
            assert!(!engine.is_docker());
            let db = (run.forwards.iter())
                .find(|f| f.service == "db" && !f.udp)
                .ok_or("no forward for the database")?;
            let pid = run.holder.as_ref().unwrap().pid.to_string();
            let probe = format!("exec 3<>/dev/tcp/127.0.0.1/{} && echo up", db.main_port);
            let args = [
                "--user",
                "--net",
                "--preserve-credentials",
                "-t",
                &pid,
                "--",
                "bash",
                "-c",
                &probe,
            ];
            let said = output(None, "nsenter", &args, Duration::from_secs(5), &stop);
            // Nothing listens in the container: a refusal from it still
            // shows the way there is open, a timeout that it isn't.
            eprintln!("probe: {said:?}");
            let filter = project_filter(&run.stacks[0].name);
            let db = format!("label={}=db", sandbox::LABEL_SERVICE);
            let container = podman(&["ps", "-q", "--filter", &filter, "--filter", &db])?;
            let container = container.trim();
            let mounts = podman(&[
                "inspect",
                container,
                "--format",
                "{{range .Mounts}}{{.Name}} {{end}}",
            ])?;
            assert!(
                mounts.contains(&format!("{}_pgdata", run.stacks[0].name)),
                "{mounts}"
            );
            let data = podman(&["exec", container, "cat", "/data/x", "/bind/b"])?;
            assert_eq!(data, "hi\ndata\n");
            let left = labelled(&Host::Here, &stop)?;
            assert!(
                (left.iter()).any(|l| l.stack == run.stacks[0].name
                    && l.engine == engine
                    && l.feature == "podman/test"),
                "{left:?}"
            );
            // Stopped with its file gone, it goes by its labels.
            fs::remove_file(&run.stacks[0].file).map_err(|e| e.to_string())?;
            super::stop(&run, &dir, false, &Host::Here)?;
            let running = podman(&["ps", "-q", "--filter", &project_filter(&run.stacks[0].name)])?;
            assert!(running.trim().is_empty(), "{running}");
            Ok(())
        })();
        let stopped = stop_for_test(&run, &dir);
        result.unwrap();
        stopped.unwrap();
        let left = podman(&["volume", "ls", "-q"]).unwrap();
        assert!(!left.contains("gerdr-test"), "{left}");
        let left = podman(&["ps", "-a", "--format", "{{.Names}}"]).unwrap();
        assert!(!left.contains("gerdr-test"), "{left}");
        let left = podman(&["pod", "ls", "--format", "{{.Name}}"]).unwrap();
        assert!(!left.contains("gerdr-test"), "{left}");
        assert!(!dir.exists());
        assert!(!run.holder.unwrap().alive());
    }

    fn stop_for_test(run: &SandboxRun, dir: &Path) -> Result<(), String> {
        stop(run, dir, true, &Host::Here)
    }

    /// A process to be ended by a script: asleep until then.
    fn sleeper() -> (std::process::Child, Proc) {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let proc = Proc::of(child.id() as i32).unwrap();
        (child, proc)
    }

    fn ended(child: &mut std::process::Child) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn sh(script: &str) -> std::process::Output {
        Command::new("sh").args(["-c", script]).output().unwrap()
    }

    #[test]
    fn a_holder_in_this_machines_own_network_ends_nothing_else() {
        let (mut bystander, _) = sleeper();
        let (mut pasta, pasta_proc) = sleeper();
        let (mut holder, holder_proc) = sleeper();
        let run = SandboxRun {
            holder: Some(holder_proc),
            pasta: Some(pasta_proc),
            ..SandboxRun::default()
        };
        let out = sh(&network_end_script(&run));
        assert!(out.status.success(), "{out:?}");
        assert!(ended(&mut pasta), "the proxy stays");
        assert!(ended(&mut holder), "the holder stays");
        assert!(bystander.try_wait().unwrap().is_none(), "ended another's process");
        bystander.kill().unwrap();
        bystander.wait().unwrap();
    }

    #[test]
    fn what_runs_inside_ends_with_the_network_and_nothing_else() {
        let out = sh(HOLDER_SCRIPT);
        // Not every system lets a user make namespaces.
        if !out.status.success() {
            return;
        }
        let said = String::from_utf8(out.stdout).unwrap();
        let holder = started_proc(&said).expect(&said);
        let (mut bystander, _) = sleeper();
        let enter = |script: &str| {
            Command::new("nsenter")
                .args(["--user", "--net", "--preserve-credentials", "-t"])
                .arg(holder.pid.to_string())
                .args(["--", "sh", "-c", script])
                .spawn()
                .unwrap()
        };
        let mut plain = enter("exec sleep 60");
        // As an interactive shell does.
        let mut deaf = enter("trap '' TERM; while :; do sleep 0.1; done");
        std::thread::sleep(Duration::from_millis(300));
        let run = SandboxRun {
            holder: Some(holder.clone()),
            ..SandboxRun::default()
        };
        let began = Instant::now();
        let out = sh(&network_end_script(&run));
        assert!(out.status.success(), "{out:?}");
        assert!(began.elapsed() < Duration::from_secs(10), "{:?}", began.elapsed());
        assert!(ended(&mut plain), "what's inside stays");
        assert!(ended(&mut deaf), "what ignores the first signal stays");
        assert!(bystander.try_wait().unwrap().is_none(), "ended another's process");
        bystander.kill().unwrap();
        bystander.wait().unwrap();
    }

    #[test]
    fn a_pid_that_came_to_another_process_is_left_alone() {
        let (mut other, proc) = sleeper();
        let run = SandboxRun {
            pasta: Some(Proc {
                start: proc.start + 1,
                ..proc
            }),
            ..SandboxRun::default()
        };
        let out = sh(&network_end_script(&run));
        assert!(out.status.success(), "{out:?}");
        std::thread::sleep(Duration::from_millis(100));
        assert!(other.try_wait().unwrap().is_none(), "ended another's process");
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn a_network_gone_already_ends_without_complaint() {
        let (mut gone, proc) = sleeper();
        gone.kill().unwrap();
        gone.wait().unwrap();
        let run = SandboxRun {
            holder: Some(proc.clone()),
            pasta: Some(proc),
            ..SandboxRun::default()
        };
        assert!(sh(&network_end_script(&run)).status.success());
    }

    #[test]
    fn a_holder_started_there_says_who_it_is_and_has_a_namespace_of_its_own() {
        let out = sh(HOLDER_SCRIPT);
        // Not every system lets a user make namespaces.
        if !out.status.success() {
            return;
        }
        let said = String::from_utf8(out.stdout).unwrap();
        let proc = started_proc(&said).expect(&said);
        assert!(proc.alive());
        let own = fs::read_link("/proc/self/ns/net").unwrap();
        assert_ne!(fs::read_link(format!("/proc/{}/ns/net", proc.pid)).unwrap(), own);
        // It outlives the shell that started it, in a session of its own.
        let run = SandboxRun {
            holder: Some(proc.clone()),
            ..SandboxRun::default()
        };
        assert!(sh(&network_end_script(&run)).status.success());
        let started = Instant::now();
        while proc.alive() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!proc.alive());
    }

    #[test]
    fn a_proxy_gone_at_once_says_no_process() {
        let tmp = std::env::temp_dir().join(format!("gerdr-pasta-{}", std::process::id()));
        let script = pasta_script(&["--no-such-flag".to_owned()], &tmp.to_string_lossy());
        // No pasta here, or one that refuses: either way it isn't there after.
        let said = String::from_utf8(sh(&script).stdout).unwrap();
        assert!(started_proc(&said).is_none(), "{said}");
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn nothing_started_says_no_process() {
        assert_eq!(started_proc(""), None);
        assert_eq!(started_proc("12\n"), None);
        assert_eq!(started_proc("x\nstat\n"), None);
    }

    #[test]
    fn a_port_there_is_free_of_what_listens_and_what_others_hold() {
        use std::collections::BTreeSet;
        let held: BTreeSet<u16> = [49152].into();
        let taken: BTreeSet<u16> = [49153].into();
        assert_eq!(free_among(&held, &taken, 0), Some(49154));
        assert_eq!(free_among(&held, &taken, 2), Some(49154));
        assert_eq!(free_among(&held, &taken, 3), Some(49155));
        // Past the end of the range it goes round.
        assert_eq!(free_among(&held, &taken, 9999), Some(59151));
        let all: BTreeSet<u16> = (49152..59152).collect();
        assert_eq!(free_among(&all, &taken, 0), None);
    }

    #[test]
    fn a_sandbox_placed_on_a_machine_is_never_looked_up_here() {
        // This pid is alive: that means nothing for one on another machine.
        let me = Proc::of(std::process::id() as i32).unwrap();
        let mut run = SandboxRun {
            machine: Some("w".into()),
            holder: Some(me.clone()),
            pasta: Some(me),
            ..SandboxRun::default()
        };
        assert!(!run.alive());
        assert!(run.listening().is_empty());
        assert!(stop(&run, Path::new("/nonexistent"), true, &Host::Here).is_err());
        assert!(run.kill_network(&Host::Here).is_ok());
        // As that machine said, until it is stopped.
        run.elsewhere = Some(Elsewhere {
            asked: true,
            alive: true,
            ports: vec![80],
        });
        assert!(run.alive());
        assert_eq!(run.listening(), [80]);
        run.stopped = true;
        assert!(!run.alive());
    }

    #[test]
    fn where_a_sandbox_runs_stays_in_its_record() {
        let run = SandboxRun {
            machine: Some("w".into()),
            ..SandboxRun::default()
        };
        let json = serde_json::to_string(&run).unwrap();
        let back: SandboxRun = serde_json::from_str(&json).unwrap();
        assert_eq!(back.machine.as_deref(), Some("w"));
        let plain = serde_json::to_string(&SandboxRun::default()).unwrap();
        assert!(!plain.contains("machine"), "{plain}");
        assert!(!plain.contains("not_there"), "{plain}");
    }
}
