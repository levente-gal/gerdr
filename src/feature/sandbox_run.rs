//! Starting and stopping a feature's sandbox (see `sandbox`): docker, the
//! network namespace and the proxy into it. Runs processes: call all of it off
//! the UI thread (checklist D3), except what reads only `/proc`.
//!
//! Network: `holder` keeps a user and network namespace alive, and pasta leads
//! main's port numbers there to the copies' ports on the host. pasta is handed
//! the namespaces as paths with `--no-netns-quit` and stays in the foreground:
//! given a pid or a pid file, Fedora's confinement of it refuses.
//!
//! A volume copies with its container paused, so the copy is what a crash at
//! that moment would leave, which a database recovers from. `Paused` lets go
//! however the copy ends. On a copy-on-write file system it takes under a
//! second.

use std::{
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
    sandbox::{self, Forward, Generated, Input, Stack},
    setup::{self, Exit},
};
use crate::desktop::shell_quote;

/// How long one docker command may run: pulling an image can take a while.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long the copies may take to come up, health checks included.
const READY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a copy of one volume may run, with main's container paused.
const COPY_TIMEOUT: Duration = setup::TIMEOUT;
/// How long a stop waits for what it stops.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long what runs in a sandbox gets to end.
const END_WAIT: Duration = Duration::from_secs(3);
/// Folders of the system a bind mount shares on purpose: not data to copy.
const SYSTEM_DIRS: &[&str] = &[
    "/var/run", "/run", "/sys", "/proc", "/dev", "/tmp", "/etc", "/usr",
];

/// The variable that tells a pane's commands they are in a sandbox; what
/// keeps them from running again outside it (see `PaneRecord::restart_command`).
pub const SANDBOX_VAR: &str = "GERDR_SANDBOX";

/// The sandbox of a feature: kept in its record, so a restarted gerdr finds it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRun {
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
}

impl SandboxRun {
    /// Whether its namespace is there, so its commands can run.
    pub fn alive(&self) -> bool {
        !self.stopped
            && self.failed.is_none()
            && self.holder.as_ref().is_some_and(Proc::alive)
            && self.pasta.as_ref().is_some_and(Proc::alive)
    }

    /// Stop its network, the only part that takes no docker: its pieces
    /// without a whole sandbox, after a failed start. What still runs inside
    /// goes first: a browser's window would stay, on a dead network.
    pub fn kill_network(&self) {
        if let Some(holder) = self.holder.as_ref().filter(|h| h.alive()) {
            end_inside(holder);
        }
        if let Some(pasta) = &self.pasta {
            pasta.kill();
        }
        if let Some(holder) = &self.holder {
            holder.kill();
        }
    }

    /// What listens inside it, by port, leaving out the proxy's own ports.
    pub fn listening(&self) -> Vec<u16> {
        let Some(holder) = self.holder.as_ref().filter(|h| h.alive()) else {
            return Vec::new();
        };
        let mut ports = Vec::new();
        for table in ["tcp", "tcp6"] {
            let path = format!("/proc/{}/net/{table}", holder.pid);
            // Unreadable counts as nothing listening: the holder just ended.
            let text = fs::read_to_string(path).unwrap_or_default();
            ports.extend(listening_in(&text));
        }
        ports.sort_unstable();
        ports.dedup();
        ports.retain(|port| !self.forwards.iter().any(|f| f.main_port == *port));
        ports
    }
}

/// End every process in the namespace `holder` keeps but `holder`, waiting
/// `END_WAIT` at most: a browser writing into its profile can't be removed.
fn end_inside(holder: &Proc) {
    let Ok(net) = fs::read_link(format!("/proc/{}/ns/net", holder.pid)) else {
        return;
    };
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
    env.lines()
        .filter_map(|line| {
            let line = line.trim_start().trim_start_matches("export ");
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            let set = !value.trim().is_empty();
            (set && !key.starts_with('#') && WORDS.iter().any(|word| key.contains(word)))
                .then(|| key.to_owned())
        })
        .collect()
}

/// A compose project of a sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackRun {
    pub name: String,
    pub file: PathBuf,
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
    program: &str,
    args: &[&str],
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<String, String> {
    let mut child = Command::new(program)
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

fn docker(args: &[&str], stop: &AtomicBool) -> Result<String, String> {
    output("docker", args, DOCKER_TIMEOUT, stop)
}

/// Which tool is missing for a sandbox, in words, or the docker daemon being
/// down.
pub fn tools_missing() -> Option<String> {
    let stop = AtomicBool::new(false);
    for tool in ["pasta", "unshare", "nsenter"] {
        let found = Command::new(tool)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if found.is_err() {
            return Some(format!("{tool} isn't installed, and a sandbox needs it"));
        }
    }
    let info = ["info", "--format", "{{.ServerVersion}}"];
    if output("docker", &info, Duration::from_secs(15), &stop).is_err() {
        return Some("Docker isn't running, and a sandbox needs it".to_owned());
    }
    let compose = ["compose", "version"];
    if output("docker", &compose, Duration::from_secs(15), &stop).is_err() {
        return Some("Docker Compose isn't installed, and a sandbox needs it".to_owned());
    }
    None
}

/// What it takes to start a sandbox.
pub struct Job<'a> {
    pub project: &'a str,
    pub slug: &'a str,
    pub repos: &'a [Repo],
    /// The folders whose compose projects are the feature's: its projects'.
    pub roots: &'a [PathBuf],
    /// The sandbox's own folder.
    pub dir: &'a Path,
    /// What to run once in the namespace before its services: folder and
    /// shell line.
    pub prepare: &'a [(PathBuf, String)],
    pub env: &'a [(String, String)],
    /// Ports other sandboxes hold, though stopped.
    pub taken: &'a [u16],
    pub stop: &'a AtomicBool,
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
    if let Some(missing) = tools_missing() {
        return Err(missing);
    }
    fs::create_dir_all(job.dir).map_err(|e| format!("couldn't make {}: {e}", job.dir.display()))?;
    let containers = running_containers(job.stop)?;
    let stacks = sandbox::stacks_under(&containers, job.roots);
    run.not_copied = sandbox::loose(&containers)
        .iter()
        .map(|c| c.name.clone())
        .collect();
    run.outward = outward_of(job);
    let mut taken: std::collections::BTreeSet<u16> = job.taken.iter().copied().collect();
    for stack in &stacks {
        progress(format!("Reading {}", stack.project));
        let config = compose_config(stack, job)?;
        let name = sandbox::compose_name(job.project, job.slug, &stack.project);
        let label = sandbox::feature_label(job.project, job.slug);
        let running = stack.services.keys().cloned().collect();
        let file = job.dir.join(format!("{name}.json"));
        let mut next_port = sandbox::free_port;
        let generated = sandbox::generate(
            &config,
            Input {
                name: &name,
                label: &label,
                running: &running,
                data_dir: job.dir,
                taken: &taken,
                copies_bind: &|path| copies_bind(path),
                next_port: &mut next_port,
            },
        )?;
        taken.extend(generated.forwards.iter().map(|f| f.host_port));
        let text = serde_json::to_string_pretty(&generated.compose).map_err(|e| e.to_string())?;
        fs::write(&file, text).map_err(|e| format!("couldn't write {}: {e}", file.display()))?;
        copy_data(stack, &generated, &config, job, run, progress)?;
        run.forwards.extend(generated.forwards);
        run.stacks.push(StackRun { name, file });
    }
    for stack in run.stacks.clone() {
        progress(format!("Starting {}", stack.name));
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
        docker(&args, job.stop)?;
        let args = ["compose", "-p", &stack.name, "-f", &file, "up", "-d"];
        docker(&args, job.stop)?;
        wait_ready(&stack, job.stop)?;
    }
    progress("Making its network".to_owned());
    let holder = start_holder()?;
    run.holder = Some(holder.clone());
    let pasta = start_pasta(&holder, &run.forwards, job.dir)?;
    run.pasta = Some(pasta);
    wait_loopback(&holder);
    for (i, (dir, line)) in job.prepare.iter().enumerate() {
        if job.stop.load(Ordering::Relaxed) {
            return Err("Stopped".to_owned());
        }
        progress(format!("{line} ({} of {})", i + 1, job.prepare.len()));
        run_inside(&holder, job, dir, line)?;
    }
    Ok(())
}

/// What the `.env` files of the feature's repositories can reach, unless the
/// project set it otherwise for sandboxes.
fn outward_of(job: &Job) -> Vec<String> {
    let mut keys: Vec<String> = job
        .repos
        .iter()
        .filter_map(|repo| fs::read_to_string(repo.worktree.join(".env")).ok())
        .flat_map(|text| outward_keys(&text))
        .filter(|key| !overridden(key, job.env))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Whether the project set `key` for sandboxes. Setting `EMAIL_HOST` also
/// covers `EMAIL_HOST_USER`: the login is of no use with the host gone.
fn overridden(key: &str, env: &[(String, String)]) -> bool {
    env.iter().any(|(set, _)| key.starts_with(set.as_str()))
}

/// The running containers, whichever project they are of.
fn running_containers(stop: &AtomicBool) -> Result<Vec<sandbox::Container>, String> {
    let ids = docker(&["ps", "-q"], stop)?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["inspect"];
    args.extend(&ids);
    sandbox::containers_of(&docker(&args, stop)?)
}

/// `path` in the feature if it is there, else as it is.
fn in_feature(path: &Path, repos: &[Repo]) -> PathBuf {
    let mapped = sandbox::in_worktree(path, repos);
    if mapped.exists() {
        mapped
    } else {
        path.to_owned()
    }
}

/// `docker compose config` of `stack`, read from the feature's files. Under
/// main's project name, so the names it spells out are the ones of main's
/// volumes, which are what gets copied.
fn compose_config(stack: &Stack, job: &Job) -> Result<Value, String> {
    let dir = in_feature(&stack.working_dir, job.repos);
    let dir = dir.to_string_lossy().into_owned();
    let mut owned = vec![
        "compose".to_owned(),
        "-p".to_owned(),
        stack.project.clone(),
        "--project-directory".to_owned(),
        dir,
    ];
    if let Some(env) = &stack.env_file {
        owned.push("--env-file".to_owned());
        owned.push(in_feature(env, job.repos).to_string_lossy().into_owned());
    }
    for file in &stack.config_files {
        owned.push("-f".to_owned());
        owned.push(in_feature(file, job.repos).to_string_lossy().into_owned());
    }
    owned.extend(["config", "--format", "json"].map(str::to_owned));
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    let text = docker(&args, job.stop)?;
    serde_json::from_str(&text).map_err(|e| format!("couldn't read the compose file: {e}"))
}

/// Whether a writable bind mount of `path` holds data to copy. What git tracks
/// is the project's files, like a config, and the system's folders are shared
/// on purpose, like the docker socket.
fn copies_bind(path: &Path) -> bool {
    if SYSTEM_DIRS.iter().any(|dir| path.starts_with(dir)) {
        return false;
    }
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
    !matches!(tracked, Ok(out) if out.status.success() && !out.stdout.is_empty())
}

/// Unpauses its containers however a copy ends. If gerdr dies meanwhile they
/// stay paused, until `docker unpause`: nothing here looks for that, and the
/// next start fails on pausing them again.
struct Paused(Vec<String>);

impl Paused {
    fn new(containers: Vec<String>, stop: &AtomicBool) -> Result<Self, String> {
        let mut paused = Self(Vec::new());
        for container in containers {
            docker(&["pause", &container], stop)?;
            paused.0.push(container);
        }
        Ok(paused)
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        let never = AtomicBool::new(false);
        for container in &self.0 {
            if let Err(why) = docker(&["unpause", container], &never) {
                tracing::error!("couldn't unpause {container}: {why}");
            }
        }
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
        if mark.exists() {
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
        if docker(&["volume", "inspect", &copy.from], job.stop).is_err() {
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
        let _ = docker(&["volume", "rm", &copy.to], job.stop);
        docker(&["volume", "create", "--label", &label, &copy.to], job.stop)?;
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
            let _paused = Paused::new(containers, job.stop)?;
            output("docker", &args, COPY_TIMEOUT, job.stop)
        };
        if let Err(why) = copied {
            // A copy cut short mustn't be taken for data. Failing to remove it
            // changes nothing: without a mark, the next start copies again.
            let _ = docker(&["volume", "rm", &copy.to], job.stop);
            return Err(format!("Couldn't copy volume {}: {why}", copy.from));
        }
        write_mark(&mark)?;
    }
    for bind in &generated.binds {
        let mark = bind.to.with_extension("copied");
        if mark.exists() {
            continue;
        }
        if !bind.from.exists() {
            fs::create_dir_all(&bind.to)
                .map_err(|e| format!("couldn't make {}: {e}", bind.to.display()))?;
            continue;
        }
        progress(format!("Copying {}", bind.from.display()));
        if let Some(parent) = bind.to.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("couldn't make {}: {e}", parent.display()))?;
        }
        let (from, to) = (bind.from.to_string_lossy(), bind.to.to_string_lossy());
        let args = ["-a", "--reflink=auto", from.as_ref(), to.as_ref()];
        output("cp", &args, COPY_TIMEOUT, job.stop)
            .map_err(|why| format!("Couldn't copy {from}: {why}"))?;
        write_mark(&mark)?;
    }
    run.fresh
        .extend(generated.fresh.iter().map(|f| f.service.clone()));
    Ok(())
}

fn write_mark(mark: &Path) -> Result<(), String> {
    if let Some(parent) = mark.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't make {}: {e}", parent.display()))?;
    }
    File::create(mark)
        .map(drop)
        .map_err(|e| format!("couldn't write {}: {e}", mark.display()))
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
            return Ready::Waiting("reading docker".to_owned());
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
fn wait_ready(stack: &StackRun, stop: &AtomicBool) -> Result<(), String> {
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
    loop {
        match ready_of(&docker(&args, stop)?) {
            Ready::Yes => return Ok(()),
            Ready::Failed(why) => {
                return Err(format!("{why} (docker compose -p {} logs)", stack.name));
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
fn wait_loopback(holder: &Proc) {
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
        match output("nsenter", &args, Duration::from_secs(2), &stop) {
            Ok(text) if text.contains("127.0.0.1") => return,
            Ok(_) | Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    tracing::warn!("the sandbox's loopback address didn't show up");
}

/// Run shell `line` inside the sandbox, in `dir`, its output logged.
fn run_inside(holder: &Proc, job: &Job, dir: &Path, line: &str) -> Result<(), String> {
    use std::io::Write;
    let log = job.dir.join("prepare.log");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| format!("couldn't write {}: {e}", log.display()))?;
    // A log line lost doesn't stop the sandbox.
    let _ = writeln!(file, "== {line}");
    let out = file.try_clone().map_err(|e| e.to_string())?;
    let mut command = format!(
        "exec nsenter --user --net --preserve-credentials -t {} -- env {SANDBOX_VAR}={}",
        holder.pid,
        shell_quote(job.slug)
    );
    for (key, value) in job.env {
        command.push_str(&format!(" {}={}", key, shell_quote(value)));
    }
    command.push_str(&format!(" sh -c {}", shell_quote(line)));
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
        Err(format!(
            "`{line}` {} (see {})",
            exit.describe(),
            log.display()
        ))
    }
}

/// Stop `run`: its network, then its containers. With `remove`, its containers,
/// its data and `dir` too, and nothing of it remains. What couldn't be stopped
/// is the error, with the rest still done.
pub fn stop(run: &SandboxRun, dir: &Path, remove: bool) -> Result<(), String> {
    run.kill_network();
    let never = AtomicBool::new(false);
    let mut problems = Vec::new();
    for stack in &run.stacks {
        let file = stack.file.to_string_lossy();
        let base = ["compose", "-p", &stack.name, "-f", &file];
        let mut args = base.to_vec();
        if remove {
            args.extend(["down", "-v", "--remove-orphans"]);
        } else {
            args.push("stop");
        }
        if let Err(why) = output("docker", &args, STOP_TIMEOUT, &never) {
            problems.push(format!("{}: {why}", stack.name));
        }
    }
    if remove && problems.is_empty() {
        match fs::remove_dir_all(dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => problems.push(format!("{}: {err}", dir.display())),
        }
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
pub fn remove_stack(stack: &str) -> Result<(), String> {
    let never = AtomicBool::new(false);
    let args = ["compose", "-p", stack, "down", "-v", "--remove-orphans"];
    output("docker", &args, STOP_TIMEOUT, &never)?;
    let filter = format!("name=^{stack}_");
    let names = docker(&["volume", "ls", "-q", "--filter", &filter], &never)?;
    for name in names.split_whitespace() {
        docker(&["volume", "rm", name], &never)?;
    }
    Ok(())
}

/// The compose project names of the containers and volumes carrying `LABEL`,
/// with the feature each is of: what a removed feature could leave behind.
pub fn labelled(stop: &AtomicBool) -> Result<Vec<(String, String)>, String> {
    let filter = format!("label={}", sandbox::LABEL);
    let format = format!(
        "{{{{.Label \"{}\"}}}}\t{{{{.Label \"com.docker.compose.project\"}}}}",
        sandbox::LABEL
    );
    let text = docker(
        &["ps", "-a", "--filter", &filter, "--format", &format],
        stop,
    )?;
    let mut found: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let (feature, stack) = line.split_once('\t')?;
            Some((feature.to_owned(), stack.to_owned()))
        })
        .collect();
    found.sort();
    found.dedup();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn overriding_a_key_covers_its_login_too() {
        let env = [("EMAIL_HOST".to_owned(), "127.0.0.1".to_owned())];
        assert!(overridden("EMAIL_HOST", &env));
        assert!(overridden("EMAIL_HOST_PASSWORD", &env));
        assert!(!overridden("SMTP_PASSWORD", &env));
        assert!(!overridden("EMAIL_HOST", &[]));
    }

    #[test]
    fn system_folders_stay_shared() {
        assert!(!copies_bind(Path::new("/var/run/docker.sock")));
        assert!(!copies_bind(Path::new("/etc/hosts")));
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
            project: "spike",
            slug: "test",
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
            let said = output("nsenter", &args, Duration::from_secs(5), &stop)?;
            assert_eq!(said.trim(), "up");
            let name = &run.stacks[0].name;
            let container = format!("{name}-db-1");
            let mounts = docker(
                &[
                    "inspect",
                    &container,
                    "--format",
                    "{{range .Mounts}}{{.Name}} {{end}}",
                ],
                &stop,
            )?;
            assert!(!mounts.contains("gerdr-spike-src_pgdata"), "{mounts}");
            let rows = docker(
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
        let left = docker(&["volume", "ls", "-q"], &AtomicBool::new(false)).unwrap();
        assert!(!left.contains("gerdr-test"), "{left}");
        assert!(!dir.exists());
        assert!(!run.holder.unwrap().alive());
    }

    fn stop_for_test(run: &SandboxRun, dir: &Path) -> Result<(), String> {
        stop(run, dir, true)
    }
}
